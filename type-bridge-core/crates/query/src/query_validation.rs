//! Schema-aware validation producing execution-ready query plans.
//!
//! Only Rust can produce a [`ValidatedQuery`]: it resolves the plan's
//! bindings against one exact resolved schema and managed state, derives
//! every binding's runtime domain, walks the stage pipeline to the output
//! row schema, and refuses ambiguity instead of defaulting it.

use std::collections::{BTreeMap, BTreeSet};

use type_bridge_contract::diagnostic::{
    Diagnostic, DiagnosticCategory, DiagnosticCode,
};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::migration_assertion::BindingId;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::query_plan::{
    DocumentSource, QueryOutput, QueryPlan, ReadStage, Reducer,
};
use type_bridge_contract::schema_delta::ManagedSchemaState;
use type_bridge_contract::value::ValueTypeTag;

use crate::engine::{self, EngineCode, EngineCodes};
use crate::{
    BindingDomain, DocumentColumn, DocumentColumnShape, DocumentSchema,
    MigrationAssertionValidationContext, OutputSchema, RowColumn, RowSchema,
};

/// The stable diagnostic vocabulary of query-plan validation.
const QUERY_ENGINE_CODES: EngineCodes = EngineCodes {
    unknown_type: EngineCode {
        code: "query_plan_unknown_type",
        message: "isa pattern references a type outside the resolved schema",
    },
    unknown_attribute: EngineCode {
        code: "query_plan_unknown_attribute",
        message: "has pattern references an attribute outside the resolved schema",
    },
    unknown_relation: EngineCode {
        code: "query_plan_unknown_relation",
        message: "links pattern relation is absent or not relation-kind",
    },
    unknown_role: EngineCode {
        code: "query_plan_unknown_role",
        message: "links pattern references a role outside the resolved schema",
    },
    role_relation_mismatch: EngineCode {
        code: "query_plan_role_relation_mismatch",
        message: "links role is not effective on the declared relation",
    },
    root_reference_not_positive: EngineCode {
        code: "query_plan_binding_not_positive",
        message: "a root-scope reference is not positively established at the root",
    },
    negation_unbound: EngineCode {
        code: "query_plan_negation_unbound_binding",
        message: "negation-local reference is not positively established in its body",
    },
    empty_negated_domain: EngineCode {
        code: "query_plan_empty_negated_domain",
        message: "negated pattern has an impossible schema domain",
    },
    value_domain_mismatch: EngineCode {
        code: "query_plan_value_domain_mismatch",
        message: "value comparison operands have different scalar domains",
    },
    binding_not_scalar: EngineCode {
        code: "query_plan_binding_not_scalar",
        message: "value operand binding has no uniform attribute scalar domain",
    },
    nonuniform_value_domain: EngineCode {
        code: "query_plan_nonuniform_value_domain",
        message: "binding domain mixes incompatible scalar domains",
    },
    disconnected_topology: EngineCode {
        code: "query_plan_disconnected_topology",
        message: "positive query bindings form a disconnected cross join",
    },
    unknown_input: EngineCode {
        code: "query_plan_unknown_input_column",
        message: "pattern references an undeclared input column",
    },
    unknown_function: EngineCode {
        code: "query_plan_unknown_function",
        message: "call references a function outside the resolved schema",
    },
    function_return_unsupported: EngineCode {
        code: "query_plan_function_return_unsupported",
        message: "the first function vocabulary admits scalar non-optional returns only",
    },
    function_arity_mismatch: EngineCode {
        code: "query_plan_function_arity_mismatch",
        message: "call arguments do not match the function signature arity",
    },
    function_argument_type: EngineCode {
        code: "query_plan_function_argument_type",
        message: "call argument disagrees with the declared parameter type",
    },
    value_binding_misuse: EngineCode {
        code: "query_plan_value_binding_misuse",
        message: "a value binding may appear only as a comparison or argument operand",
    },
    try_unbound: EngineCode {
        code: "query_plan_try_unbound_binding",
        message: "try-body reference is not established in its body or the root",
    },
    try_uncorrelated: EngineCode {
        code: "query_plan_try_not_correlated",
        message: "a try body must reference at least one mandatory root binding",
    },
    empty_try_domain: EngineCode {
        code: "query_plan_empty_try_domain",
        message: "try body has an impossible schema domain",
    },
    local_unbound: EngineCode {
        code: "query_plan_local_function_unbound",
        message: "local-body reference is not a parameter or body-established",
    },
    local_uncorrelated: EngineCode {
        code: "query_plan_local_function_uncorrelated",
        message: "a local function body must reference every parameter",
    },
    empty_local_domain: EngineCode {
        code: "query_plan_empty_local_function_domain",
        message: "local function body has an impossible schema domain",
    },
    local_return_domain: EngineCode {
        code: "query_plan_local_function_return_domain",
        message: "the declared return does not fit the reducer over its input domain",
    },
};

/// Opaque, non-serializable result of schema-aware plan validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedQuery {
    binding_domains: BTreeMap<BindingId, BindingDomain>,
    output_schema: OutputSchema,
    plan: QueryPlan,
    source_state: ManagedSchemaState,
    structural_limits: StructuralLimits,
}

impl ValidatedQuery {
    /// Return the context-free trusted plan.
    pub const fn plan(&self) -> &QueryPlan {
        &self.plan
    }

    /// Return one schema-derived binding domain.
    pub fn binding_domain(&self, id: &BindingId) -> Option<&BindingDomain> {
        self.binding_domains.get(id)
    }

    /// Return the validator-derived output shape.
    pub const fn output_schema(&self) -> &OutputSchema {
        &self.output_schema
    }

    /// Return the exact managed and declared schema identity validated here.
    pub const fn source_state(&self) -> &ManagedSchemaState {
        &self.source_state
    }

    /// Return the exact structural policy used during validation.
    pub const fn structural_limits(&self) -> StructuralLimits {
        self.structural_limits
    }
}

/// Validate one reusable plan against exact resolved schema authority.
pub fn validate_query_plan(
    plan: &QueryPlan,
    context: &MigrationAssertionValidationContext<'_>,
    limits: StructuralLimits,
) -> Result<ValidatedQuery, Diagnostic> {
    if plan.managed_semantics() != context.managed_state().managed_semantic_schema() {
        return Err(plan_failure(
            DiagnosticCategory::Integrity,
            "query_plan_managed_semantic_mismatch",
            "plan managed semantic fingerprint does not match validation state",
        ));
    }
    if context.resolved_schema().declared_identity_fingerprint()
        != context.managed_state().declared_identity()
    {
        return Err(plan_failure(
            DiagnosticCategory::Integrity,
            "query_plan_declared_identity_mismatch",
            "resolved schema declaration identity does not match validation state",
        ));
    }
    if !limits.allows_bindings(plan.bindings().len())
        || plan
            .bindings()
            .iter()
            .any(|binding| binding.variable().as_str().len() > limits.output_name_bytes)
    {
        return Err(plan_failure(
            DiagnosticCategory::ResourceLimit,
            "query_plan_validation_limit",
            "plan exceeds caller structural limits",
        ));
    }

    let schema = context.resolved_schema();
    let inputs = plan
        .inputs()
        .iter()
        .map(|column| column.value_type())
        .collect::<Vec<ValueTypeTag>>();
    let Some(ReadStage::Match { patterns }) = plan.pipeline().first() else {
        return Err(plan_failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_match_not_first",
            "the pattern conjunction must be the first pipeline stage",
        ));
    };
    let mut locals = BTreeMap::new();
    for function in plan.functions() {
        if schema.functions().contains_key(function.name()) {
            return Err(plan_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_local_function_shadows_schema",
                "a plan-local function cannot shadow a schema function",
            ));
        }
        let signature =
            engine::analyze_local_function(function, schema, &QUERY_ENGINE_CODES)?;
        locals.insert(function.name().clone(), signature);
    }

    let engine::PatternAnalysis {
        domains,
        optional_positive,
        positive,
        scoped_positive,
        used,
        value_bindings,
    } = engine::analyze_patterns(
        patterns,
        plan.bindings().len(),
        &inputs,
        schema,
        &locals,
        &QUERY_ENGINE_CODES,
    )?;

    // Reduce assignments establish fresh value bindings outside the
    // pattern conjunction; collect them before per-binding auditing.
    let reduce_assigned: BTreeSet<BindingId> = plan
        .pipeline()
        .iter()
        .filter_map(|stage| match stage {
            ReadStage::Reduce { assignments, .. } => Some(assignments),
            _ => None,
        })
        .flatten()
        .map(|assignment| assignment.assigned())
        .collect();

    let projected: BTreeSet<BindingId> = match plan.output() {
        QueryOutput::Rows { columns } => columns.iter().copied().collect(),
        QueryOutput::Documents { fields } => fields
            .iter()
            .map(|field| match field.source() {
                DocumentSource::Binding { binding } => *binding,
                DocumentSource::AttributeList { owner, .. } => *owner,
            })
            .collect(),
    };
    for binding in plan.bindings() {
        let id = binding.id();
        if reduce_assigned.contains(&id) {
            continue;
        }
        if !used.contains(&id) {
            return Err(plan_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_binding_not_used",
                "every declared binding must be referenced by a pattern",
            ));
        }
        if projected.contains(&id)
            && !positive.contains(&id)
            && !optional_positive.contains(&id)
        {
            return Err(plan_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_binding_not_positive",
                "an output binding must be positively established at the root",
            ));
        }
        if !projected.contains(&id)
            && !positive.contains(&id)
            && !optional_positive.contains(&id)
            && !scoped_positive.contains(&id)
        {
            return Err(plan_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_invalid_witness",
                "a hidden witness must be positively established in its lexical scope",
            ));
        }
        if (positive.contains(&id) || optional_positive.contains(&id))
            && domains[&id].is_empty()
            && !value_bindings.contains_key(&id)
        {
            return Err(plan_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_empty_domain",
                "schema validation reduced a binding to an empty runtime domain",
            ));
        }
    }

    let mut binding_domains = domains
        .into_iter()
        .filter(|(id, _)| positive.contains(id) || optional_positive.contains(id))
        .map(|(id, type_ids)| {
            let value_type = match value_bindings.get(&id) {
                Some(tag) => Some(*tag),
                None => {
                    engine::uniform_value_type(&type_ids, schema, &QUERY_ENGINE_CODES)?
                }
            };
            Ok((id, BindingDomain::new(type_ids, value_type)))
        })
        .collect::<Result<BTreeMap<_, _>, Diagnostic>>()?;

    for stage in plan.pipeline() {
        match stage {
            ReadStage::Select { bindings } => {
                for binding in bindings {
                    if !positive.contains(binding)
                        && !optional_positive.contains(binding)
                    {
                        return Err(plan_failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_binding_not_positive",
                            "stage bindings must be positively established at the root",
                        ));
                    }
                }
            }
            ReadStage::Require { bindings } => {
                for binding in bindings {
                    if !positive.contains(binding) {
                        return Err(plan_failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_binding_not_positive",
                            "stage bindings must be positively established at the root",
                        ));
                    }
                }
            }
            ReadStage::Reduce { assignments, groups } => {
                for group in groups {
                    if !positive.contains(group) {
                        return Err(plan_failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_binding_not_positive",
                            "stage bindings must be positively established at the root",
                        ));
                    }
                }
                for assignment in assignments {
                    let input_scalar = match assignment.input() {
                        Some(input) => {
                            let admitted = positive.contains(&input)
                                || (optional_positive.contains(&input)
                                    && assignment.reducer().total_without_groups());
                            if !admitted {
                                return Err(plan_failure(
                                    DiagnosticCategory::InvalidContract,
                                    "query_plan_binding_not_positive",
                                    "stage bindings must be positively established at the root",
                                ));
                            }
                            binding_domains
                                .get(&input)
                                .and_then(|domain| domain.value_type())
                        }
                        None => None,
                    };
                    let result_type = match assignment.reducer() {
                        Reducer::Count => ValueTypeTag::Long,
                        Reducer::Sum | Reducer::Max | Reducer::Min => {
                            match input_scalar {
                                Some(
                                    tag @ (ValueTypeTag::Long | ValueTypeTag::Double),
                                ) => tag,
                                _ => {
                                    return Err(plan_failure(
                                        DiagnosticCategory::InvalidContract,
                                        "query_plan_reduce_input_domain",
                                        "this reducer requires a uniform numeric scalar input",
                                    ));
                                }
                            }
                        }
                        Reducer::Mean => match input_scalar {
                            Some(ValueTypeTag::Long | ValueTypeTag::Double) => {
                                ValueTypeTag::Double
                            }
                            _ => {
                                return Err(plan_failure(
                                    DiagnosticCategory::InvalidContract,
                                    "query_plan_reduce_input_domain",
                                    "this reducer requires a uniform numeric scalar input",
                                ));
                            }
                        },
                    };
                    binding_domains.insert(
                        assignment.assigned(),
                        BindingDomain::new(BTreeSet::new(), Some(result_type)),
                    );
                }
            }
            ReadStage::Sort { terms } => {
                for term in terms {
                    let scalar = binding_domains
                        .get(&term.binding())
                        .and_then(|domain| domain.value_type());
                    if scalar.is_none() {
                        return Err(plan_failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_sort_not_scalar",
                            "sort keys require a validated uniform scalar domain",
                        ));
                    }
                }
            }
            ReadStage::Match { .. }
            | ReadStage::Distinct
            | ReadStage::Offset { .. }
            | ReadStage::Limit { .. } => {}
        }
    }

    let output_schema = match plan.output() {
        QueryOutput::Rows { columns } => OutputSchema::Rows(RowSchema::new(
            columns
                .iter()
                .map(|id| {
                    let binding = plan
                        .bindings()
                        .get(usize::from(id.get()))
                        .expect("validated output binding exists");
                    RowColumn::new(
                        *id,
                        binding_domains[id].clone(),
                        binding.variable().clone(),
                        optional_positive.contains(id),
                    )
                })
                .collect::<Vec<_>>(),
        )),
        QueryOutput::Documents { fields } => {
            let columns = fields
                .iter()
                .map(|field| {
                    let shape = match field.source() {
                        DocumentSource::Binding { binding } => {
                            let Some(value_type) =
                                binding_domains[binding].value_type()
                            else {
                                return Err(plan_failure(
                                    DiagnosticCategory::InvalidContract,
                                    "query_plan_document_field_not_scalar",
                                    "document fields fetch uniform scalar bindings",
                                ));
                            };
                            DocumentColumnShape::Scalar {
                                value_type,
                                optional: optional_positive.contains(binding),
                            }
                        }
                        DocumentSource::AttributeList { attribute, owner } => {
                            let attribute_type = TypeId::new(
                                TypeKind::Attribute,
                                attribute.label().as_str().to_owned(),
                            )?;
                            let Some(element_type) = schema
                                .types()
                                .get(&attribute_type)
                                .and_then(|resolved| resolved.value_type())
                                .map(|value| value.value_type())
                            else {
                                return Err(plan_failure(
                                    DiagnosticCategory::InvalidContract,
                                    "query_plan_unknown_attribute",
                                    "attribute list references no resolved scalar attribute",
                                ));
                            };
                            let reachable = binding_domains[owner]
                                .type_ids()
                                .iter()
                                .any(|id| {
                                    schema.types().get(id).is_some_and(|resolved| {
                                        resolved.owns().contains_key(attribute)
                                    })
                                });
                            if !reachable {
                                return Err(plan_failure(
                                    DiagnosticCategory::InvalidContract,
                                    "query_plan_document_unreachable_attribute",
                                    "no type in the owner domain owns the listed attribute",
                                ));
                            }
                            DocumentColumnShape::List {
                                attribute: attribute.clone(),
                                element_type,
                            }
                        }
                    };
                    Ok(DocumentColumn::new(field.key().clone(), shape))
                })
                .collect::<Result<Vec<_>, Diagnostic>>()?;
            OutputSchema::Documents(DocumentSchema::new(columns))
        }
    };

    Ok(ValidatedQuery {
        binding_domains,
        output_schema,
        plan: plan.clone(),
        source_state: context.managed_state().clone(),
        structural_limits: limits,
    })
}

fn plan_failure(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static query-plan diagnostic code"),
        message,
    )
}
