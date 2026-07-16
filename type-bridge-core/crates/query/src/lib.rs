//! Schema-aware validation for typed query foundations.

mod engine;
mod query_validation;
mod safety_condition;

pub use query_validation::{ValidatedQuery, validate_query_plan};

pub use safety_condition::{
    lower_condition_to_plan, safety_condition_to_assertion_plan,
};

use std::collections::{BTreeMap, BTreeSet};

use type_bridge_contract::diagnostic::{
    Diagnostic, DiagnosticCategory, DiagnosticCode,
};
use type_bridge_contract::id::TypeId;
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::migration_assertion::{
    AssertionPattern, BindingId, MigrationAssertionPlan, QueryVariable, ValueOperand,
};
use type_bridge_contract::query_plan::{QueryOperand, QueryPattern};
use type_bridge_contract::schema_delta::ManagedSchemaState;
use type_bridge_contract::value::ValueTypeTag;
use type_bridge_schema::ResolvedSchema;

/// Trusted schema inputs which bind a plan to one exact managed selection.
#[derive(Clone, Copy, Debug)]
pub struct MigrationAssertionValidationContext<'a> {
    resolved_schema: &'a ResolvedSchema,
    managed_state: &'a ManagedSchemaState,
}

impl<'a> MigrationAssertionValidationContext<'a> {
    /// Bind the resolved schema view to its trusted managed state.
    pub const fn new(
        resolved_schema: &'a ResolvedSchema,
        managed_state: &'a ManagedSchemaState,
    ) -> Self {
        Self {
            resolved_schema,
            managed_state,
        }
    }

    /// Return the resolved schema used for schema-derived domains.
    pub const fn resolved_schema(&self) -> &'a ResolvedSchema {
        self.resolved_schema
    }

    /// Return the exact selected managed state.
    pub const fn managed_state(&self) -> &'a ManagedSchemaState {
        self.managed_state
    }
}

/// Schema-derived possible runtime types and optional scalar value domain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BindingDomain {
    type_ids: BTreeSet<TypeId>,
    value_type: Option<ValueTypeTag>,
}

impl BindingDomain {
    pub(crate) const fn new(
        type_ids: BTreeSet<TypeId>,
        value_type: Option<ValueTypeTag>,
    ) -> Self {
        Self { type_ids, value_type }
    }

    /// Return possible concrete runtime types.
    pub const fn type_ids(&self) -> &BTreeSet<TypeId> {
        &self.type_ids
    }

    /// Return the uniform scalar domain for attribute bindings.
    pub const fn value_type(&self) -> Option<ValueTypeTag> {
        self.value_type
    }
}

/// One validator-derived output column.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RowColumn {
    binding: BindingId,
    domain: BindingDomain,
    variable: QueryVariable,
}

impl RowColumn {
    pub(crate) const fn new(
        binding: BindingId,
        domain: BindingDomain,
        variable: QueryVariable,
    ) -> Self {
        Self { binding, domain, variable }
    }

    /// Return the output binding.
    pub const fn binding(&self) -> BindingId {
        self.binding
    }

    /// Return the canonical query variable.
    pub const fn variable(&self) -> &QueryVariable {
        &self.variable
    }

    /// Return the schema-derived binding domain.
    pub const fn domain(&self) -> &BindingDomain {
        &self.domain
    }
}

/// Ordered output row schema derived solely by validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RowSchema {
    columns: Vec<RowColumn>,
}

impl RowSchema {
    pub(crate) const fn new(columns: Vec<RowColumn>) -> Self {
        Self { columns }
    }

    /// Return ordered output columns.
    pub fn columns(&self) -> &[RowColumn] {
        &self.columns
    }
}

/// Opaque, non-serializable result of schema-aware assertion validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedMigrationAssertionPlan {
    binding_domains: BTreeMap<BindingId, BindingDomain>,
    plan: MigrationAssertionPlan,
    row_schema: RowSchema,
    source_state: ManagedSchemaState,
    structural_limits: StructuralLimits,
    witnesses: BTreeSet<BindingId>,
}

impl ValidatedMigrationAssertionPlan {
    /// Return the context-free trusted plan.
    pub const fn plan(&self) -> &MigrationAssertionPlan {
        &self.plan
    }

    /// Return one schema-derived binding domain.
    pub fn binding_domain(&self, id: &BindingId) -> Option<&BindingDomain> {
        self.binding_domains.get(id)
    }

    /// Return validator-derived output shape.
    pub const fn row_schema(&self) -> &RowSchema {
        &self.row_schema
    }

    /// Return the exact managed and full declared schema identity validated here.
    pub const fn source_state(&self) -> &ManagedSchemaState {
        &self.source_state
    }

    /// Return the exact structural policy used during validation.
    pub const fn structural_limits(&self) -> StructuralLimits {
        self.structural_limits
    }

    /// Return hidden witness bindings.
    pub const fn witnesses(&self) -> &BTreeSet<BindingId> {
        &self.witnesses
    }
}

/// Validate topology and exact effective schema domains without provider I/O.
pub fn validate_migration_assertion_plan(
    plan: &MigrationAssertionPlan,
    context: &MigrationAssertionValidationContext<'_>,
    limits: StructuralLimits,
) -> Result<ValidatedMigrationAssertionPlan, Diagnostic> {
    if plan.managed_semantics() != context.managed_state().managed_semantic_schema() {
        return Err(Diagnostic::new(
            DiagnosticCategory::Integrity,
            DiagnosticCode::new("migration_assertion_managed_semantic_mismatch")
                .expect("static query diagnostic code is canonical"),
            "assertion managed semantic fingerprint does not match validation state",
        ));
    }
    if context.resolved_schema().declared_identity_fingerprint()
        != context.managed_state().declared_identity()
    {
        return Err(Diagnostic::new(
            DiagnosticCategory::Integrity,
            DiagnosticCode::new("migration_assertion_declared_identity_mismatch")
                .expect("static query diagnostic code is canonical"),
            "resolved schema declaration identity does not match validation state",
        ));
    }
    validate_limits(plan, limits)?;
    let schema = context.resolved_schema();
    let converted = convert_assertion_patterns(plan.patterns());
    let engine::PatternAnalysis {
        domains,
        positive,
        scoped_positive,
        used,
        value_bindings: _,
    } = engine::analyze_patterns(
        &converted,
        plan.bindings().len(),
        &[],
        schema,
        &ASSERTION_ENGINE_CODES,
    )?;

    for binding in plan.bindings() {
        let id = binding.id();
        let is_output = plan.outputs().binary_search(&id).is_ok();
        let is_witness = plan.witnesses().binary_search(&id).is_ok();
        if is_output && !positive.contains(&id) {
            return Err(query_failure(
                "migration_assertion_binding_not_positive",
                "an output binding must be positively established at the root",
            ));
        }
        if is_witness
            && !positive.contains(&id)
            && !scoped_positive.contains(&id)
        {
            return Err(query_failure(
                "migration_assertion_invalid_witness",
                "witness must be positively established in its lexical scope",
            ));
        }
        if !used.contains(&id) {
            return Err(query_failure(
                "migration_assertion_binding_not_used",
                "every declared binding must be referenced by an assertion pattern",
            ));
        }
        if positive.contains(&id) && domains[&id].is_empty() {
            return Err(query_failure(
                "migration_assertion_empty_domain",
                "schema validation reduced a binding to an empty runtime domain",
            ));
        }
        if !is_output && !is_witness {
            return Err(query_failure(
                "migration_assertion_unclassified_binding",
                "every binding must be an output or a hidden witness",
            ));
        }
    }
    for witness in plan.witnesses() {
        if !used.contains(witness)
            || (!positive.contains(witness) && !scoped_positive.contains(witness))
        {
            return Err(query_failure(
                "migration_assertion_invalid_witness",
                "witness must be positively bound and used",
            ));
        }
    }

    let binding_domains = domains
        .into_iter()
        .filter(|(id, _)| positive.contains(id))
        .map(|(id, type_ids)| {
            let value_type =
                engine::uniform_value_type(&type_ids, schema, &ASSERTION_ENGINE_CODES)?;
            Ok((id, BindingDomain { type_ids, value_type }))
        })
        .collect::<Result<BTreeMap<_, _>, Diagnostic>>()?;
    let columns = plan
        .outputs()
        .iter()
        .map(|id| {
            let binding = plan.binding(*id).expect("validated output binding exists");
            RowColumn {
                binding: *id,
                domain: binding_domains[id].clone(),
                variable: binding.variable().clone(),
            }
        })
        .collect();
    Ok(ValidatedMigrationAssertionPlan {
        binding_domains,
        plan: plan.clone(),
        row_schema: RowSchema { columns },
        source_state: context.managed_state().clone(),
        structural_limits: limits,
        witnesses: plan.witnesses().iter().copied().collect(),
    })
}

fn validate_limits(
    plan: &MigrationAssertionPlan,
    limits: StructuralLimits,
) -> Result<(), Diagnostic> {
    if !limits.allows_bindings(plan.bindings().len())
        || !limits.allows_selected_slots(plan.outputs().len())
        || plan
            .bindings()
            .iter()
            .any(|binding| binding.variable().as_str().len() > limits.output_name_bytes)
    {
        return Err(query_failure(
            "migration_assertion_validation_limit",
            "assertion exceeds caller structural limits",
        ));
    }
    let mut nodes = 0;
    inspect_limits(plan.patterns(), 1, limits, &mut nodes)
}

fn inspect_limits(
    patterns: &[AssertionPattern],
    depth: usize,
    limits: StructuralLimits,
    nodes: &mut usize,
) -> Result<(), Diagnostic> {
    if patterns.len() > limits.boolean_terms {
        return Err(query_failure(
            "migration_assertion_validation_limit",
            "assertion boolean term count exceeds caller limits",
        ));
    }
    for pattern in patterns {
        *nodes += 1;
        if !limits.allows_predicate_nodes(*nodes) || !limits.allows_predicate_depth(depth) {
            return Err(query_failure(
                "migration_assertion_validation_limit",
                "assertion pattern size exceeds caller limits",
            ));
        }
        if let AssertionPattern::Not { patterns } = pattern {
            inspect_limits(patterns, depth + 1, limits, nodes)?;
        }
    }
    Ok(())
}

fn query_failure(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new(code).expect("static query diagnostic code is canonical"),
        message,
    )
}

/// The released stable diagnostic vocabulary of assertion validation.
///
/// Assertion validation delegates to the shared pattern engine; these exact
/// codes and messages predate that engine and must never drift.
const ASSERTION_ENGINE_CODES: engine::EngineCodes = engine::EngineCodes {
    unknown_type: engine::EngineCode {
        code: "migration_assertion_unknown_type",
        message: "isa pattern references a type outside the resolved schema",
    },
    unknown_attribute: engine::EngineCode {
        code: "migration_assertion_unknown_attribute",
        message: "has pattern references an attribute outside the resolved schema",
    },
    unknown_relation: engine::EngineCode {
        code: "migration_assertion_unknown_relation",
        message: "links pattern relation is absent or not relation-kind",
    },
    unknown_role: engine::EngineCode {
        code: "migration_assertion_unknown_role",
        message: "links pattern references a role outside the resolved schema",
    },
    role_relation_mismatch: engine::EngineCode {
        code: "migration_assertion_role_relation_mismatch",
        message: "links role is not effective on the declared relation",
    },
    root_reference_not_positive: engine::EngineCode {
        code: "migration_assertion_binding_not_positive",
        message: "a root-scope reference is not positively established at the root",
    },
    negation_unbound: engine::EngineCode {
        code: "migration_assertion_negation_unbound_binding",
        message: "negation-local reference is not positively established in its body",
    },
    empty_negated_domain: engine::EngineCode {
        code: "migration_assertion_empty_negated_domain",
        message: "negated pattern has an impossible schema domain",
    },
    value_domain_mismatch: engine::EngineCode {
        code: "migration_assertion_value_domain_mismatch",
        message: "value comparison operands have different scalar domains",
    },
    binding_not_scalar: engine::EngineCode {
        code: "migration_assertion_binding_not_scalar",
        message: "value operand binding has no uniform attribute scalar domain",
    },
    nonuniform_value_domain: engine::EngineCode {
        code: "migration_assertion_nonuniform_value_domain",
        message: "binding domain mixes incompatible scalar domains",
    },
    disconnected_topology: engine::EngineCode {
        code: "migration_assertion_disconnected_topology",
        message: "positive assertion bindings form a disconnected cross join",
    },
    // Assertion plans cannot express input operands or function calls;
    // these entries never fire.
    unknown_input: engine::EngineCode {
        code: "migration_assertion_unknown_binding",
        message: "assertion patterns cannot reference invocation inputs",
    },
    unknown_function: engine::EngineCode {
        code: "migration_assertion_unknown_binding",
        message: "assertion patterns cannot call schema functions",
    },
    function_return_unsupported: engine::EngineCode {
        code: "migration_assertion_unknown_binding",
        message: "assertion patterns cannot call schema functions",
    },
    function_arity_mismatch: engine::EngineCode {
        code: "migration_assertion_unknown_binding",
        message: "assertion patterns cannot call schema functions",
    },
    function_argument_type: engine::EngineCode {
        code: "migration_assertion_unknown_binding",
        message: "assertion patterns cannot call schema functions",
    },
    value_binding_misuse: engine::EngineCode {
        code: "migration_assertion_unknown_binding",
        message: "assertion patterns cannot call schema functions",
    },
};

fn convert_assertion_patterns(patterns: &[AssertionPattern]) -> Vec<QueryPattern> {
    patterns.iter().map(convert_assertion_pattern).collect()
}

fn convert_assertion_pattern(pattern: &AssertionPattern) -> QueryPattern {
    match pattern {
        AssertionPattern::Isa {
            binding,
            include_subtypes,
            type_id,
        } => QueryPattern::Isa {
            binding: *binding,
            include_subtypes: *include_subtypes,
            type_id: type_id.clone(),
        },
        AssertionPattern::Has {
            attribute,
            attribute_id,
            owner,
        } => QueryPattern::Has {
            attribute: *attribute,
            attribute_id: attribute_id.clone(),
            owner: *owner,
        },
        AssertionPattern::Links {
            players,
            relation,
            relation_id,
        } => QueryPattern::Links {
            players: players.clone(),
            relation: *relation,
            relation_id: relation_id.clone(),
        },
        AssertionPattern::Value {
            comparator,
            left,
            right,
        } => QueryPattern::Value {
            comparator: *comparator,
            left: convert_operand(left),
            right: convert_operand(right),
        },
        AssertionPattern::Not { patterns } => QueryPattern::Not {
            patterns: convert_assertion_patterns(patterns),
        },
    }
}

fn convert_operand(operand: &ValueOperand) -> QueryOperand {
    match operand {
        ValueOperand::Binding { binding } => QueryOperand::Binding { binding: *binding },
        ValueOperand::Literal { value } => QueryOperand::Literal {
            value: value.clone(),
        },
    }
}
