//! Schema-aware validation producing execution-ready query plans.
//!
//! Only Rust can produce a [`ValidatedQuery`]: it resolves the plan's
//! bindings against one exact resolved schema and managed state, derives
//! every binding's runtime domain, walks the stage pipeline to the output
//! row schema, and refuses ambiguity instead of defaulting it.

use std::collections::{BTreeMap, BTreeSet};

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::id::{TypeId, TypeKind, is_typeql_3_12_builtin_function_name};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::migration_assertion::BindingId;
use type_bridge_contract::query_plan::{
    DocumentSource, LocalFunction, QueryOperand, QueryOutput, QueryPattern, QueryPatternV2,
    QueryPlan, ReadStage, Reducer,
};
use type_bridge_contract::schema::{AnnotationKindId, SchemaAnnotationValue};
use type_bridge_contract::schema_delta::ManagedSchemaState;
use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};

use crate::engine::{self, EngineCode, EngineCodes};
use crate::query_v2_claims::validate_v2_schema_claims;
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
    value_comparator_unsupported: EngineCode {
        code: "query_plan_value_comparator_unsupported",
        message: "ordered comparisons require a provider-orderable scalar domain",
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
    function_dependency_cycle: EngineCode {
        code: "query_plan_function_dependency_cycle",
        message: "function-call result dependencies must be acyclic",
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
    try_binding_shared: EngineCode {
        code: "query_plan_try_binding_shared",
        message: "an optional binding belongs to exactly one try body and no negation scope",
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
    root_visibility: Vec<BindingId>,
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

    /// Return the bindings positively visible in a root row, dense order.
    ///
    /// This is the validator-derived row environment after the pattern
    /// conjunction and before any Select or Reduce stage: a binding
    /// established only inside a negation is a witness, never a column.
    /// Execution derives implicit projection from exactly this set, so a
    /// plan without an explicit Select still requests only columns the
    /// provider can produce.
    pub fn root_visibility(&self) -> &[BindingId] {
        &self.root_visibility
    }
}

/// Return the compatibility-algebra edges that are guaranteed positive at the
/// root, plus explicit V1 cross-join permissions.
///
/// The ordinary engine still owns the one topology check. Compatibility
/// metadata contributes only edges with the released V1 meaning: conjunction
/// unions edges, disjunction keeps their intersection, and negation exports
/// none. This prevents an `or`-only or negated connection from laundering a
/// disconnected product while allowing the production V1 bridge to keep its
/// already-validated topology contract.
fn v2_root_topology(plan: &QueryPlan) -> BTreeSet<(BindingId, BindingId)> {
    let Some(compatibility) = plan.v2_compatibility() else {
        return BTreeSet::new();
    };
    let mut edges = compatibility
        .predicate()
        .map(definite_v2_edges)
        .unwrap_or_default();
    edges.extend(
        compatibility
            .allowed_cross_joins()
            .iter()
            .map(|pair| canonical_topology_edge(pair.left(), pair.right())),
    );
    edges
}

fn definite_v2_edges(pattern: &QueryPatternV2) -> BTreeSet<(BindingId, BindingId)> {
    match pattern {
        QueryPatternV2::FieldValue { .. } => BTreeSet::new(),
        QueryPatternV2::FieldComparison { left, right, .. } => {
            BTreeSet::from([canonical_topology_edge(left.binding(), right.binding())])
        }
        QueryPatternV2::RoleEdge {
            relation, player, ..
        } => BTreeSet::from([canonical_topology_edge(*relation, *player)]),
        QueryPatternV2::Reachable { source, target, .. } => {
            BTreeSet::from([canonical_topology_edge(*source, *target)])
        }
        QueryPatternV2::And { patterns } => patterns
            .iter()
            .flat_map(definite_v2_edges)
            .collect::<BTreeSet<_>>(),
        QueryPatternV2::Or { patterns } => {
            let mut patterns = patterns.iter();
            let Some(first) = patterns.next() else {
                return BTreeSet::new();
            };
            patterns.fold(definite_v2_edges(first), |common, pattern| {
                common
                    .intersection(&definite_v2_edges(pattern))
                    .copied()
                    .collect()
            })
        }
        QueryPatternV2::Not { .. } => BTreeSet::new(),
    }
}

const fn canonical_topology_edge(left: BindingId, right: BindingId) -> (BindingId, BindingId) {
    if left.get() <= right.get() {
        (left, right)
    } else {
        (right, left)
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
    if plan.functions().iter().any(|function| {
        is_typeql_3_12_builtin_function_name(function.name().label().as_str())
            || patterns_contain_typeql_builtin_function(function.body())
    }) || plan.pipeline().iter().any(|stage| match stage {
        ReadStage::Match { patterns } => patterns_contain_typeql_builtin_function(patterns),
        _ => false,
    }) {
        return Err(typeql_builtin_function_collision());
    }
    // The supplied limits are validation authority: the plan's entire
    // structure — root pipeline and local functions under one aggregate
    // predicate-node budget — re-checks under them, not a subset.
    plan.check_structural_limits(limits)?;

    let schema = context.resolved_schema();
    // V2 model metadata is untrusted wire state. Recompute its provider-facing
    // descriptor, closure, field, role, and player claims before ordinary
    // engine analysis or any lowering can consume it.
    let v2_claims = validate_v2_schema_claims(plan, schema)?;
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
        let signature = engine::analyze_local_function(function, schema, &QUERY_ENGINE_CODES)?;
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
        &v2_root_topology(plan),
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
        if projected.contains(&id) && !positive.contains(&id) && !optional_positive.contains(&id) {
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
            && !v2_claims.proves_empty_runtime_binding(id)
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
                None => engine::uniform_value_type(&type_ids, schema, &QUERY_ENGINE_CODES)?,
            };
            Ok((id, BindingDomain::new(type_ids, value_type)))
        })
        .collect::<Result<BTreeMap<_, _>, Diagnostic>>()?;

    for stage in plan.pipeline() {
        match stage {
            ReadStage::Select { bindings } => {
                for binding in bindings {
                    if !positive.contains(binding) && !optional_positive.contains(binding) {
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
            ReadStage::Reduce {
                assignments,
                groups,
            } => {
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
                        Reducer::Sum | Reducer::Max | Reducer::Min => match input_scalar {
                            Some(tag @ (ValueTypeTag::Long | ValueTypeTag::Double)) => tag,
                            _ => {
                                return Err(plan_failure(
                                    DiagnosticCategory::InvalidContract,
                                    "query_plan_reduce_input_domain",
                                    "this reducer requires a uniform numeric scalar input",
                                ));
                            }
                        },
                        Reducer::Mean => match input_scalar {
                            Some(ValueTypeTag::Long | ValueTypeTag::Double) => ValueTypeTag::Double,
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
                    if !positive.contains(&term.binding())
                        && !reduce_assigned.contains(&term.binding())
                    {
                        return Err(plan_failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_stage_unknown_binding",
                            "sort references a binding outside the mandatory row environment",
                        ));
                    }
                    let scalar = binding_domains
                        .get(&term.binding())
                        .and_then(|domain| domain.value_type());
                    let Some(scalar) = scalar else {
                        return Err(plan_failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_sort_not_scalar",
                            "sort keys require a validated uniform scalar domain",
                        ));
                    };
                    if !provider_sort_is_orderable(scalar) {
                        return Err(plan_failure(
                            DiagnosticCategory::InvalidContract,
                            "query_plan_sort_not_orderable",
                            "sort keys require a scalar domain ordered by the target provider",
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

    // A window consumes a total order: page membership must not depend on
    // provider iteration among tied rows. Every binding visible at the
    // window must be determined by the sort tuple. Besides identity-total
    // sort keys, the proof admits owners identified by one unique attribute,
    // deterministic plan-local function results over determined arguments,
    // and reducer results once the complete group tuple is determined. A
    // global reduce is already at most one row, so its order is vacuously
    // total (the structural contract still requires an explicit Sort stage).
    let windowed = plan
        .pipeline()
        .iter()
        .any(|stage| matches!(stage, ReadStage::Offset { .. } | ReadStage::Limit { .. }));
    if windowed {
        let sort_keys: BTreeSet<BindingId> = plan
            .pipeline()
            .iter()
            .filter_map(|stage| match stage {
                ReadStage::Sort { terms } => Some(terms),
                _ => None,
            })
            .flatten()
            .map(|term| term.binding())
            .collect();
        let reduce = plan.pipeline().iter().find_map(|stage| match stage {
            ReadStage::Reduce {
                assignments,
                groups,
            } => Some((assignments.as_slice(), groups.as_slice())),
            _ => None,
        });
        let window_environment: Vec<BindingId> = if let Some((assignments, groups)) = reduce {
            groups
                .iter()
                .copied()
                .chain(assignments.iter().map(|assignment| assignment.assigned()))
                .collect()
        } else if let Some(ReadStage::Select { bindings }) = plan
            .pipeline()
            .iter()
            .find(|stage| matches!(stage, ReadStage::Select { .. }))
        {
            bindings.clone()
        } else {
            positive.union(&optional_positive).copied().collect()
        };

        let not_total = || {
            plan_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_window_order_not_total",
                "offset and limit require a sort tuple proven total for every visible column",
            )
        };
        let global_reduce = reduce.is_some_and(|(_, groups)| groups.is_empty());
        if !global_reduce {
            let mut determined = BTreeSet::new();
            for binding in &sort_keys {
                let domain = binding_domains.get(binding).ok_or_else(&not_total)?;
                if !sort_key_domain_is_identity_total(domain, schema) {
                    return Err(not_total());
                }
                determined.insert(*binding);
            }

            loop {
                let previous_len = determined.len();
                for pattern in patterns {
                    match pattern {
                        QueryPattern::Has {
                            owner,
                            attribute,
                            attribute_id,
                        } if determined.contains(attribute) => {
                            let owner_domain = binding_domains.get(owner).ok_or_else(&not_total)?;
                            if !owner_domain.type_ids().is_empty()
                                && binding_domains.get(attribute).is_some_and(|domain| {
                                    sort_key_domain_is_identity_total(domain, schema)
                                })
                                && one_unique_owns_scope_covers_domain(
                                    owner_domain.type_ids(),
                                    attribute_id,
                                    schema,
                                )
                            {
                                determined.insert(*owner);
                            }
                        }
                        QueryPattern::FunctionCall {
                            arguments,
                            assigned,
                            function,
                        } if locals.contains_key(function)
                            && arguments.iter().all(|argument| match argument {
                                QueryOperand::Binding { binding } => determined.contains(binding),
                                QueryOperand::Literal { .. } | QueryOperand::Input { .. } => true,
                            }) =>
                        {
                            // Plan-local functions are closed aggregate
                            // programs and therefore deterministic for one
                            // argument tuple. Schema functions intentionally
                            // receive no such proof from their signature.
                            determined.insert(*assigned);
                        }
                        _ => {}
                    }
                }
                if let Some((assignments, groups)) = reduce
                    && groups.iter().all(|group| determined.contains(group))
                {
                    determined.extend(assignments.iter().map(|assignment| assignment.assigned()));
                }
                if determined.len() == previous_len {
                    break;
                }
            }

            if window_environment
                .iter()
                .any(|binding| !determined.contains(binding))
            {
                return Err(not_total());
            }
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
                            let Some(value_type) = binding_domains[binding].value_type() else {
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
                            if !positive.contains(owner) {
                                return Err(plan_failure(
                                    DiagnosticCategory::InvalidContract,
                                    "query_plan_output_not_visible",
                                    "attribute lists require a mandatory owner binding",
                                ));
                            }
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
                            let reachable = binding_domains[owner].type_ids().iter().any(|id| {
                                schema
                                    .types()
                                    .get(id)
                                    .is_some_and(|resolved| resolved.owns().contains_key(attribute))
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
        root_visibility: positive.union(&optional_positive).copied().collect(),
        source_state: context.managed_state().clone(),
        structural_limits: limits,
    })
}

/// Validate one plan-local function against exact resolved schema authority.
///
/// Incremental authoring calls this before committing a local-function scope
/// claim. The ordinary whole-plan validator repeats the same analysis at
/// finalization; this seam exists only so a rejected function cannot corrupt
/// a mutable builder that has no root match stage yet.
pub fn validate_query_local_function(
    function: &LocalFunction,
    context: &MigrationAssertionValidationContext<'_>,
) -> Result<(), Diagnostic> {
    let schema = context.resolved_schema();
    if is_typeql_3_12_builtin_function_name(function.name().label().as_str())
        || patterns_contain_typeql_builtin_function(function.body())
    {
        return Err(typeql_builtin_function_collision());
    }
    if schema.functions().contains_key(function.name()) {
        return Err(plan_failure(
            DiagnosticCategory::InvalidContract,
            "query_plan_local_function_shadows_schema",
            "a plan-local function cannot shadow a schema function",
        ));
    }
    engine::analyze_local_function(function, schema, &QUERY_ENGINE_CODES)?;
    Ok(())
}

const fn provider_sort_is_orderable(value_type: ValueTypeTag) -> bool {
    !matches!(value_type, ValueTypeTag::Duration)
}

fn sort_key_domain_is_identity_total(
    domain: &BindingDomain,
    schema: &type_bridge_schema::ResolvedSchema,
) -> bool {
    let Some(value_type) = domain.value_type() else {
        return false;
    };
    // TypeDB compares attributes by scalar value, not by the complete typed
    // identity. Signed double zero and datetime-tz values with different
    // designators can remain distinct identities while comparing equal, so
    // those domains never receive an injectivity proof here.
    if !provider_comparison_equality_matches_canonical_identity(value_type) {
        return false;
    }
    if domain.type_ids().len() <= 1 {
        return true;
    }

    finite_attribute_value_domains_are_pairwise_disjoint(domain, value_type, schema)
}

const fn provider_comparison_equality_matches_canonical_identity(value_type: ValueTypeTag) -> bool {
    matches!(
        value_type,
        ValueTypeTag::String
            | ValueTypeTag::Long
            | ValueTypeTag::Boolean
            | ValueTypeTag::Date
            | ValueTypeTag::DateTime
            | ValueTypeTag::Decimal
    )
}

/// Prove a polymorphic scalar domain has no cross-type provider comparison ties.
///
/// `@values` is an exhaustive restriction. For scalar domains whose canonical
/// equality agrees with provider comparison equality, exact set disjointness
/// therefore proves that two different attribute types cannot contribute tied
/// identities. Missing or malformed resolved evidence fails closed.
fn finite_attribute_value_domains_are_pairwise_disjoint(
    domain: &BindingDomain,
    value_type: ValueTypeTag,
    schema: &type_bridge_schema::ResolvedSchema,
) -> bool {
    let mut seen = BTreeSet::<&CanonicalValue>::new();
    for type_id in domain.type_ids() {
        if type_id.kind() != TypeKind::Attribute {
            return false;
        }
        let Some(resolved) = schema.types().get(type_id) else {
            return false;
        };
        if !resolved.is_constructible() {
            return false;
        }
        let Some(resolved_value) = resolved.value_type() else {
            return false;
        };
        if resolved_value.value_type() != value_type {
            return false;
        }
        let Some(SchemaAnnotationValue::Values(values)) =
            resolved_value.annotations().get(&AnnotationKindId::Values)
        else {
            return false;
        };
        for value in values.iter() {
            if value.value_type() != value_type || !seen.insert(value) {
                return false;
            }
        }
    }
    true
}

/// Prove one attribute value identifies an owner across the complete domain.
///
/// TypeDB scopes `@unique` (and the uniqueness implied by `@key`) to the
/// owner type that declared the owns fact and that type's descendants. Two
/// unrelated owner types may each declare the same attribute unique while
/// still owning the same value. Requiring one shared declaration origin keeps
/// a sort key injective across the whole union instead of proving uniqueness
/// independently for each member.
fn one_unique_owns_scope_covers_domain(
    domain: &BTreeSet<TypeId>,
    attribute: &type_bridge_contract::id::AttributeId,
    schema: &type_bridge_schema::ResolvedSchema,
) -> bool {
    let mut origin = None;
    for type_id in domain {
        let Some(owns) = schema
            .types()
            .get(type_id)
            .and_then(|resolved| resolved.owns().get(attribute))
        else {
            return false;
        };
        if !owns.is_unique() {
            return false;
        }
        match &origin {
            Some(expected) if expected != owns.origin().declared() => return false,
            Some(_) => {}
            None => origin = Some(owns.origin().declared().clone()),
        }
    }
    origin.is_some()
}

fn typeql_builtin_function_collision() -> Diagnostic {
    plan_failure(
        DiagnosticCategory::InvalidContract,
        "query_plan_builtin_function_collision",
        "TypeQL 3.12 built-in function names cannot identify schema calls or plan-local functions",
    )
}

fn patterns_contain_typeql_builtin_function(patterns: &[QueryPattern]) -> bool {
    patterns.iter().any(|pattern| match pattern {
        QueryPattern::FunctionCall { function, .. } => {
            is_typeql_3_12_builtin_function_name(function.label().as_str())
        }
        QueryPattern::Or { branches } => branches
            .iter()
            .any(|branch| patterns_contain_typeql_builtin_function(branch)),
        QueryPattern::Not { patterns } | QueryPattern::Try { patterns } => {
            patterns_contain_typeql_builtin_function(patterns)
        }
        QueryPattern::Isa { .. }
        | QueryPattern::Has { .. }
        | QueryPattern::Links { .. }
        | QueryPattern::Value { .. }
        | QueryPattern::Reachable { .. } => false,
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

#[cfg(test)]
mod tests {
    use type_bridge_contract::id::{FunctionId, RoleId, TypeId, TypeKind};

    use super::*;

    #[test]
    fn builtin_function_collision_scan_descends_every_pattern_container() {
        let assigned = BindingId::new(0).expect("binding");
        let nested = vec![QueryPattern::Not {
            patterns: vec![QueryPattern::Try {
                patterns: vec![QueryPattern::FunctionCall {
                    arguments: Vec::new(),
                    assigned,
                    function: FunctionId::new("abs").expect("contextual function ID"),
                }],
            }],
        }];
        assert!(patterns_contain_typeql_builtin_function(&nested));

        let safe = vec![QueryPattern::FunctionCall {
            arguments: Vec::new(),
            assigned,
            function: FunctionId::new("absolute").expect("function ID"),
        }];
        assert!(!patterns_contain_typeql_builtin_function(&safe));
    }

    #[test]
    fn compatibility_topology_exports_only_definite_positive_edges() {
        let binding = |value| BindingId::new(value).expect("binding");
        let role_edge =
            |relation, player, relation_label: &str, role: &str| QueryPatternV2::RoleEdge {
                include_relation_subtypes: false,
                player: binding(player),
                relation: binding(relation),
                relation_type: TypeId::new(TypeKind::Relation, relation_label).expect("relation"),
                role: RoleId::new(relation_label, role).expect("role"),
            };
        let edge_01 = role_edge(0, 1, "first-link", "member");
        let edge_12 = role_edge(1, 2, "second-link", "member");

        let conjunction = QueryPatternV2::And {
            patterns: vec![edge_01.clone(), edge_12.clone()],
        };
        assert_eq!(
            definite_v2_edges(&conjunction),
            BTreeSet::from([(binding(0), binding(1)), (binding(1), binding(2))]),
        );

        let disjunction = QueryPatternV2::Or {
            patterns: vec![
                conjunction,
                QueryPatternV2::And {
                    patterns: vec![edge_01.clone()],
                },
            ],
        };
        assert_eq!(
            definite_v2_edges(&disjunction),
            BTreeSet::from([(binding(0), binding(1))]),
            "an edge absent from one branch is not a root topology proof",
        );
        assert!(
            definite_v2_edges(&QueryPatternV2::Not {
                pattern: Box::new(edge_12),
            })
            .is_empty(),
            "negated edges never connect the positive root graph",
        );
    }
}
