//! Schema-aware validation for typed query foundations.

mod safety_condition;

pub use safety_condition::{
    lower_condition_to_plan, safety_condition_to_assertion_plan,
};

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use type_bridge_contract::diagnostic::{
    Diagnostic, DiagnosticCategory, DiagnosticCode,
};
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::migration_assertion::{
    AssertionPattern, BindingId, MigrationAssertionPlan, QueryVariable, ValueOperand,
};
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
    let all_constructible = schema
        .types()
        .iter()
        .filter(|(_, resolved)| resolved.is_constructible())
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    let mut domains = plan
        .bindings()
        .iter()
        .map(|binding| (binding.id(), all_constructible.clone()))
        .collect::<BTreeMap<_, _>>();

    refine_positive(plan.patterns(), &mut domains, schema)?;
    let mut positive = BTreeSet::new();
    let mut root_references = BTreeSet::new();
    let mut topology = plan
        .bindings()
        .iter()
        .map(|binding| (binding.id(), BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    collect_scope_topology(
        plan.patterns(),
        &mut positive,
        &mut root_references,
        &mut topology,
    );
    if root_references.iter().any(|id| !positive.contains(id)) {
        return Err(query_failure(
            "migration_assertion_binding_not_positive",
            "a root-scope reference is not positively established at the root",
        ));
    }
    topology.retain(|id, _| root_references.contains(id));
    let mut scoped_positive = BTreeSet::new();
    validate_negations(
        plan.patterns(),
        &domains,
        schema,
        &positive,
        &all_constructible,
        &mut scoped_positive,
    )?;
    validate_values_shallow(plan.patterns(), &domains, schema)?;
    let mut used = BTreeSet::new();
    collect_references(plan.patterns(), &mut used);

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
    ensure_connected(&topology)?;

    let binding_domains = domains
        .into_iter()
        .filter(|(id, _)| positive.contains(id))
        .map(|(id, type_ids)| {
            let value_type = uniform_value_type(&type_ids, schema)?;
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

fn refine_positive(
    patterns: &[AssertionPattern],
    domains: &mut BTreeMap<BindingId, BTreeSet<TypeId>>,
    schema: &ResolvedSchema,
) -> Result<(), Diagnostic> {
    loop {
        let mut changed = false;
        for pattern in patterns {
            changed |= refine_pattern(pattern, domains, schema)?;
        }
        if !changed {
            break;
        }
    }
    Ok(())
}

fn refine_pattern(
    pattern: &AssertionPattern,
    domains: &mut BTreeMap<BindingId, BTreeSet<TypeId>>,
    schema: &ResolvedSchema,
) -> Result<bool, Diagnostic> {
    match pattern {
        AssertionPattern::Isa {
            binding,
            include_subtypes,
            type_id,
        } => {
            let resolved = schema.types().get(type_id).ok_or_else(|| {
                query_failure(
                    "migration_assertion_unknown_type",
                    "isa pattern references a type outside the resolved schema",
                )
            })?;
            let mut allowed = BTreeSet::from([type_id.clone()]);
            if *include_subtypes {
                allowed.extend(resolved.subtypes().iter().cloned());
            }
            allowed.retain(|id| schema.types().get(id).is_some_and(|ty| ty.is_constructible()));
            Ok(intersect_mut(
                domains.get_mut(binding).expect("declared binding"),
                &allowed,
            ))
        }
        AssertionPattern::Has {
            attribute,
            attribute_id,
            owner,
        } => {
            let attribute_type = TypeId::new(
                TypeKind::Attribute,
                attribute_id.label().as_str().to_owned(),
            )?;
            if !schema.types().contains_key(&attribute_type) {
                return Err(query_failure(
                    "migration_assertion_unknown_attribute",
                    "has pattern references an attribute outside the resolved schema",
                ));
            }
            let allowed_owners = schema
                .types()
                .iter()
                .filter(|(_, resolved)| {
                    resolved.is_constructible() && resolved.owns().contains_key(attribute_id)
                })
                .map(|(id, _)| id.clone())
                .collect::<BTreeSet<_>>();
            let owner_changed = intersect_mut(domains.get_mut(owner).expect("declared owner"), &allowed_owners);
            let attribute_changed = intersect_mut(
                domains.get_mut(attribute).expect("declared attribute"),
                &BTreeSet::from([attribute_type]),
            );
            Ok(owner_changed || attribute_changed)
        }
        AssertionPattern::Links {
            players,
            relation,
            relation_id,
        } => {
            if relation_id.kind() != TypeKind::Relation
                || !schema.types().contains_key(relation_id)
            {
                return Err(query_failure(
                    "migration_assertion_unknown_relation",
                    "links pattern relation is absent or not relation-kind",
                ));
            }
            let mut changed = intersect_mut(
                domains.get_mut(relation).expect("declared relation"),
                &BTreeSet::from([relation_id.clone()]),
            );
            for player in players {
                let role = schema.roles().get(player.role()).ok_or_else(|| {
                    query_failure(
                        "migration_assertion_unknown_role",
                        "links pattern references a role outside the resolved schema",
                    )
                })?;
                if !schema.types()[relation_id]
                    .relates()
                    .contains_key(player.role())
                {
                    return Err(query_failure(
                        "migration_assertion_role_relation_mismatch",
                        "links role is not effective on the declared relation",
                    ));
                }
                let accepted = role
                    .accepted_players()
                    .iter()
                    .filter(|id| schema.types().get(*id).is_some_and(|ty| ty.is_constructible()))
                    .cloned()
                    .collect::<BTreeSet<_>>();
                changed |= intersect_mut(
                    domains.get_mut(&player.player()).expect("declared player"),
                    &accepted,
                );
            }
            Ok(changed)
        }
        AssertionPattern::Value { .. } | AssertionPattern::Not { .. } => Ok(false),
    }
}

fn intersect_mut(target: &mut BTreeSet<TypeId>, allowed: &BTreeSet<TypeId>) -> bool {
    let before = target.len();
    target.retain(|id| allowed.contains(id));
    target.len() != before
}

fn collect_scope_topology(
    patterns: &[AssertionPattern],
    positive: &mut BTreeSet<BindingId>,
    referenced: &mut BTreeSet<BindingId>,
    topology: &mut BTreeMap<BindingId, BTreeSet<BindingId>>,
) {
    for pattern in patterns {
        match pattern {
            AssertionPattern::Isa { binding, .. } => {
                referenced.insert(*binding);
                positive.insert(*binding);
            }
            AssertionPattern::Has { attribute, owner, .. } => {
                referenced.extend([*owner, *attribute]);
                positive.extend([*owner, *attribute]);
                connect(topology, *owner, *attribute);
            }
            AssertionPattern::Links { relation, players, .. } => {
                referenced.insert(*relation);
                for player in players {
                    referenced.insert(player.player());
                    positive.extend([*relation, player.player()]);
                    connect(topology, *relation, player.player());
                }
            }
            AssertionPattern::Value { left, right, .. } => {
                let left = operand_binding(left);
                let right = operand_binding(right);
                referenced.extend(left.into_iter().chain(right));
                if let (Some(left), Some(right)) = (left, right) {
                    connect(topology, left, right);
                }
            }
            AssertionPattern::Not { .. } => {}
        }
    }
}

fn validate_negations(
    patterns: &[AssertionPattern],
    outer_domains: &BTreeMap<BindingId, BTreeSet<TypeId>>,
    schema: &ResolvedSchema,
    outer_positive: &BTreeSet<BindingId>,
    all_constructible: &BTreeSet<TypeId>,
    scoped_positive: &mut BTreeSet<BindingId>,
) -> Result<(), Diagnostic> {
    for pattern in patterns {
        if let AssertionPattern::Not { patterns } = pattern {
            let mut body_positive = BTreeSet::new();
            let mut body_references = BTreeSet::new();
            let mut topology = outer_domains
                .keys()
                .copied()
                .map(|id| (id, BTreeSet::new()))
                .collect::<BTreeMap<_, _>>();
            collect_scope_topology(
                patterns,
                &mut body_positive,
                &mut body_references,
                &mut topology,
            );
            let mut body_scope = outer_positive.clone();
            body_scope.extend(body_positive.iter().copied());
            if body_references.iter().any(|id| !body_scope.contains(id)) {
                return Err(query_failure(
                    "migration_assertion_negation_unbound_binding",
                    "negation-local reference is not positively established in its body",
                ));
            }
            let mut nested = outer_domains.clone();
            for (id, domain) in &mut nested {
                if !outer_positive.contains(id) {
                    *domain = all_constructible.clone();
                }
            }
            refine_positive(patterns, &mut nested, schema)?;
            if body_positive.iter().any(|id| nested[id].is_empty()) {
                return Err(query_failure(
                    "migration_assertion_empty_negated_domain",
                    "negated pattern has an impossible schema domain",
                ));
            }
            validate_values_shallow(patterns, &nested, schema)?;
            topology.retain(|id, _| body_references.contains(id));
            ensure_connected(&topology)?;
            scoped_positive.extend(body_positive.iter().copied());
            validate_negations(
                patterns,
                &nested,
                schema,
                &body_scope,
                all_constructible,
                scoped_positive,
            )?;
        }
    }
    Ok(())
}

fn validate_values_shallow(
    patterns: &[AssertionPattern],
    domains: &BTreeMap<BindingId, BTreeSet<TypeId>>,
    schema: &ResolvedSchema,
) -> Result<(), Diagnostic> {
    for pattern in patterns {
        match pattern {
            AssertionPattern::Value { left, right, .. } => {
                let left = operand_value_type(left, domains, schema)?;
                let right = operand_value_type(right, domains, schema)?;
                if left != right {
                    return Err(query_failure(
                        "migration_assertion_value_domain_mismatch",
                        "value comparison operands have different scalar domains",
                    ));
                }
            }
            AssertionPattern::Not { .. } => {}
            _ => {}
        }
    }
    Ok(())
}

fn operand_value_type(
    operand: &ValueOperand,
    domains: &BTreeMap<BindingId, BTreeSet<TypeId>>,
    schema: &ResolvedSchema,
) -> Result<ValueTypeTag, Diagnostic> {
    match operand {
        ValueOperand::Literal { value } => Ok(value.value_type()),
        ValueOperand::Binding { binding } => uniform_value_type(&domains[binding], schema)?
            .ok_or_else(|| {
                query_failure(
                    "migration_assertion_binding_not_scalar",
                    "value operand binding has no uniform attribute scalar domain",
                )
            }),
    }
}

fn uniform_value_type(
    domain: &BTreeSet<TypeId>,
    schema: &ResolvedSchema,
) -> Result<Option<ValueTypeTag>, Diagnostic> {
    let mut uniform: Option<Option<ValueTypeTag>> = None;
    for id in domain {
        let value_type = schema.types()[id].value_type().map(|value| value.value_type());
        match uniform {
            None => uniform = Some(value_type),
            Some(expected) if expected == value_type => {}
            Some(_) => {
                return Err(query_failure(
                    "migration_assertion_nonuniform_value_domain",
                    "binding domain mixes incompatible scalar domains",
                ));
            }
        }
    }
    Ok(uniform.flatten())
}

fn collect_references(patterns: &[AssertionPattern], output: &mut BTreeSet<BindingId>) {
    for pattern in patterns {
        match pattern {
            AssertionPattern::Isa { binding, .. } => {
                output.insert(*binding);
            }
            AssertionPattern::Has { attribute, owner, .. } => {
                output.extend([*owner, *attribute]);
            }
            AssertionPattern::Links { relation, players, .. } => {
                output.insert(*relation);
                output.extend(players.iter().map(|player| player.player()));
            }
            AssertionPattern::Value { left, right, .. } => {
                output.extend(operand_binding(left));
                output.extend(operand_binding(right));
            }
            AssertionPattern::Not { patterns } => collect_references(patterns, output),
        }
    }
}

fn operand_binding(operand: &ValueOperand) -> Option<BindingId> {
    match operand {
        ValueOperand::Binding { binding } => Some(*binding),
        ValueOperand::Literal { .. } => None,
    }
}

fn connect(
    topology: &mut BTreeMap<BindingId, BTreeSet<BindingId>>,
    left: BindingId,
    right: BindingId,
) {
    topology.get_mut(&left).expect("declared binding").insert(right);
    topology.get_mut(&right).expect("declared binding").insert(left);
}

fn ensure_connected(
    topology: &BTreeMap<BindingId, BTreeSet<BindingId>>,
) -> Result<(), Diagnostic> {
    let Some(start) = topology.keys().next().copied() else {
        return Ok(());
    };
    let mut seen = BTreeSet::from([start]);
    let mut queue = VecDeque::from([start]);
    while let Some(id) = queue.pop_front() {
        for neighbor in &topology[&id] {
            if seen.insert(*neighbor) {
                queue.push_back(*neighbor);
            }
        }
    }
    if seen.len() == topology.len() {
        Ok(())
    } else {
        Err(query_failure(
            "migration_assertion_disconnected_topology",
            "positive assertion bindings form a disconnected cross join",
        ))
    }
}

fn query_failure(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new(code).expect("static query diagnostic code is canonical"),
        message,
    )
}
