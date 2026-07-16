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
use type_bridge_contract::query_plan::{QueryOutput, QueryPlan, ReadStage};
use type_bridge_contract::schema_delta::ManagedSchemaState;
use type_bridge_contract::value::ValueTypeTag;

use crate::engine::{self, EngineCode, EngineCodes};
use crate::{
    BindingDomain, MigrationAssertionValidationContext, RowColumn, RowSchema,
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
};

/// Opaque, non-serializable result of schema-aware plan validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedQuery {
    binding_domains: BTreeMap<BindingId, BindingDomain>,
    plan: QueryPlan,
    row_schema: RowSchema,
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

    /// Return the validator-derived output row schema.
    pub const fn row_schema(&self) -> &RowSchema {
        &self.row_schema
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
    let engine::PatternAnalysis {
        domains,
        positive,
        scoped_positive,
        used,
    } = engine::analyze_patterns(
        patterns,
        plan.bindings().len(),
        &inputs,
        schema,
        &QUERY_ENGINE_CODES,
    )?;

    let QueryOutput::Rows { columns } = plan.output();
    let projected: BTreeSet<BindingId> = columns.iter().copied().collect();
    for binding in plan.bindings() {
        let id = binding.id();
        if !used.contains(&id) {
            return Err(plan_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_binding_not_used",
                "every declared binding must be referenced by a pattern",
            ));
        }
        if projected.contains(&id) && !positive.contains(&id) {
            return Err(plan_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_binding_not_positive",
                "an output binding must be positively established at the root",
            ));
        }
        if !projected.contains(&id)
            && !positive.contains(&id)
            && !scoped_positive.contains(&id)
        {
            return Err(plan_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_invalid_witness",
                "a hidden witness must be positively established in its lexical scope",
            ));
        }
        if positive.contains(&id) && domains[&id].is_empty() {
            return Err(plan_failure(
                DiagnosticCategory::InvalidContract,
                "query_plan_empty_domain",
                "schema validation reduced a binding to an empty runtime domain",
            ));
        }
    }

    let binding_domains = domains
        .into_iter()
        .filter(|(id, _)| positive.contains(id))
        .map(|(id, type_ids)| {
            let value_type =
                engine::uniform_value_type(&type_ids, schema, &QUERY_ENGINE_CODES)?;
            Ok((id, BindingDomain::new(type_ids, value_type)))
        })
        .collect::<Result<BTreeMap<_, _>, Diagnostic>>()?;

    for stage in plan.pipeline() {
        match stage {
            ReadStage::Select { bindings } | ReadStage::Require { bindings } => {
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

    let row_columns = columns
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
            )
        })
        .collect::<Vec<_>>();

    Ok(ValidatedQuery {
        binding_domains,
        plan: plan.clone(),
        row_schema: RowSchema::new(row_columns),
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
