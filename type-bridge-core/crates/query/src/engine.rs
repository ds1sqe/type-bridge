//! The shared schema-aware pattern engine over the reusable query algebra.
//!
//! Both migration assertions and query plans validate their closed pattern
//! conjunctions here. The engine walks [`QueryPattern`] — assertions convert
//! injectively before calling — and reports through a caller-supplied
//! diagnostic table so each entry point keeps its released stable codes.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use type_bridge_contract::diagnostic::{
    Diagnostic, DiagnosticCategory, DiagnosticCode,
};
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::migration_assertion::BindingId;
use type_bridge_contract::query_plan::{QueryOperand, QueryPattern};
use type_bridge_contract::value::ValueTypeTag;
use type_bridge_schema::ResolvedSchema;

/// One stable diagnostic identity owned by an engine entry point.
#[derive(Clone, Copy)]
pub(crate) struct EngineCode {
    pub(crate) code: &'static str,
    pub(crate) message: &'static str,
}

/// The complete per-entry-point diagnostic vocabulary of the engine.
#[derive(Clone, Copy)]
pub(crate) struct EngineCodes {
    pub(crate) unknown_type: EngineCode,
    pub(crate) unknown_attribute: EngineCode,
    pub(crate) unknown_relation: EngineCode,
    pub(crate) unknown_role: EngineCode,
    pub(crate) role_relation_mismatch: EngineCode,
    pub(crate) root_reference_not_positive: EngineCode,
    pub(crate) negation_unbound: EngineCode,
    pub(crate) empty_negated_domain: EngineCode,
    pub(crate) value_domain_mismatch: EngineCode,
    pub(crate) binding_not_scalar: EngineCode,
    pub(crate) nonuniform_value_domain: EngineCode,
    pub(crate) disconnected_topology: EngineCode,
    pub(crate) unknown_input: EngineCode,
}

/// The trusted result of one root-conjunction analysis.
pub(crate) struct PatternAnalysis {
    pub(crate) domains: BTreeMap<BindingId, BTreeSet<TypeId>>,
    pub(crate) positive: BTreeSet<BindingId>,
    pub(crate) scoped_positive: BTreeSet<BindingId>,
    pub(crate) used: BTreeSet<BindingId>,
}

/// Analyze one closed root conjunction against the resolved schema.
///
/// Establishes schema-derived binding domains via fixpoint refinement,
/// positive/negative scope discipline, shallow value-domain agreement
/// (inputs typed by their dense declarations), and connected root topology.
pub(crate) fn analyze_patterns(
    patterns: &[QueryPattern],
    binding_count: usize,
    inputs: &[ValueTypeTag],
    schema: &ResolvedSchema,
    codes: &EngineCodes,
) -> Result<PatternAnalysis, Diagnostic> {
    let all_constructible = schema
        .types()
        .iter()
        .filter(|(_, resolved)| resolved.is_constructible())
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    let binding_ids = (0..binding_count)
        .map(|index| BindingId::new(u16::try_from(index).expect("dense ordinal")))
        .collect::<Result<Vec<_>, _>>()?;
    let mut domains = binding_ids
        .iter()
        .map(|id| (*id, all_constructible.clone()))
        .collect::<BTreeMap<_, _>>();

    refine_positive(patterns, &mut domains, schema, codes)?;
    let mut positive = BTreeSet::new();
    let mut root_references = BTreeSet::new();
    let mut topology = binding_ids
        .iter()
        .map(|id| (*id, BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    collect_scope_topology(patterns, &mut positive, &mut root_references, &mut topology);
    if root_references.iter().any(|id| !positive.contains(id)) {
        return Err(fail(codes.root_reference_not_positive));
    }
    topology.retain(|id, _| root_references.contains(id));
    let mut scoped_positive = BTreeSet::new();
    validate_negations(
        patterns,
        &domains,
        schema,
        inputs,
        &positive,
        &all_constructible,
        &mut scoped_positive,
        codes,
    )?;
    validate_values_shallow(patterns, &domains, schema, inputs, codes)?;
    let mut used = BTreeSet::new();
    collect_references(patterns, &mut used);
    ensure_connected(&topology, codes)?;

    Ok(PatternAnalysis {
        domains,
        positive,
        scoped_positive,
        used,
    })
}

/// Return the uniform scalar domain of one refined binding domain.
pub(crate) fn uniform_value_type(
    domain: &BTreeSet<TypeId>,
    schema: &ResolvedSchema,
    codes: &EngineCodes,
) -> Result<Option<ValueTypeTag>, Diagnostic> {
    let mut uniform: Option<Option<ValueTypeTag>> = None;
    for id in domain {
        let value_type = schema.types()[id].value_type().map(|value| value.value_type());
        match uniform {
            None => uniform = Some(value_type),
            Some(expected) if expected == value_type => {}
            Some(_) => return Err(fail(codes.nonuniform_value_domain)),
        }
    }
    Ok(uniform.flatten())
}

fn refine_positive(
    patterns: &[QueryPattern],
    domains: &mut BTreeMap<BindingId, BTreeSet<TypeId>>,
    schema: &ResolvedSchema,
    codes: &EngineCodes,
) -> Result<(), Diagnostic> {
    loop {
        let mut changed = false;
        for pattern in patterns {
            changed |= refine_pattern(pattern, domains, schema, codes)?;
        }
        if !changed {
            break;
        }
    }
    Ok(())
}

fn refine_pattern(
    pattern: &QueryPattern,
    domains: &mut BTreeMap<BindingId, BTreeSet<TypeId>>,
    schema: &ResolvedSchema,
    codes: &EngineCodes,
) -> Result<bool, Diagnostic> {
    match pattern {
        QueryPattern::Isa {
            binding,
            include_subtypes,
            type_id,
        } => {
            let resolved = schema
                .types()
                .get(type_id)
                .ok_or_else(|| fail(codes.unknown_type))?;
            let mut allowed = BTreeSet::from([type_id.clone()]);
            if *include_subtypes {
                allowed.extend(resolved.subtypes().iter().cloned());
            }
            allowed.retain(|id| {
                schema.types().get(id).is_some_and(|ty| ty.is_constructible())
            });
            Ok(intersect_mut(
                domains.get_mut(binding).expect("declared binding"),
                &allowed,
            ))
        }
        QueryPattern::Has {
            attribute,
            attribute_id,
            owner,
        } => {
            let attribute_type = TypeId::new(
                TypeKind::Attribute,
                attribute_id.label().as_str().to_owned(),
            )?;
            if !schema.types().contains_key(&attribute_type) {
                return Err(fail(codes.unknown_attribute));
            }
            let allowed_owners = schema
                .types()
                .iter()
                .filter(|(_, resolved)| {
                    resolved.is_constructible() && resolved.owns().contains_key(attribute_id)
                })
                .map(|(id, _)| id.clone())
                .collect::<BTreeSet<_>>();
            let owner_changed = intersect_mut(
                domains.get_mut(owner).expect("declared owner"),
                &allowed_owners,
            );
            let attribute_changed = intersect_mut(
                domains.get_mut(attribute).expect("declared attribute"),
                &BTreeSet::from([attribute_type]),
            );
            Ok(owner_changed || attribute_changed)
        }
        QueryPattern::Links {
            players,
            relation,
            relation_id,
        } => {
            if relation_id.kind() != TypeKind::Relation
                || !schema.types().contains_key(relation_id)
            {
                return Err(fail(codes.unknown_relation));
            }
            let mut changed = intersect_mut(
                domains.get_mut(relation).expect("declared relation"),
                &BTreeSet::from([relation_id.clone()]),
            );
            for player in players {
                let role = schema
                    .roles()
                    .get(player.role())
                    .ok_or_else(|| fail(codes.unknown_role))?;
                if !schema.types()[relation_id]
                    .relates()
                    .contains_key(player.role())
                {
                    return Err(fail(codes.role_relation_mismatch));
                }
                let accepted = role
                    .accepted_players()
                    .iter()
                    .filter(|id| {
                        schema.types().get(*id).is_some_and(|ty| ty.is_constructible())
                    })
                    .cloned()
                    .collect::<BTreeSet<_>>();
                changed |= intersect_mut(
                    domains.get_mut(&player.player()).expect("declared player"),
                    &accepted,
                );
            }
            Ok(changed)
        }
        QueryPattern::Value { .. } | QueryPattern::Not { .. } => Ok(false),
    }
}

fn intersect_mut(target: &mut BTreeSet<TypeId>, allowed: &BTreeSet<TypeId>) -> bool {
    let before = target.len();
    target.retain(|id| allowed.contains(id));
    target.len() != before
}

fn collect_scope_topology(
    patterns: &[QueryPattern],
    positive: &mut BTreeSet<BindingId>,
    referenced: &mut BTreeSet<BindingId>,
    topology: &mut BTreeMap<BindingId, BTreeSet<BindingId>>,
) {
    for pattern in patterns {
        match pattern {
            QueryPattern::Isa { binding, .. } => {
                referenced.insert(*binding);
                positive.insert(*binding);
            }
            QueryPattern::Has { attribute, owner, .. } => {
                referenced.extend([*owner, *attribute]);
                positive.extend([*owner, *attribute]);
                connect(topology, *owner, *attribute);
            }
            QueryPattern::Links { relation, players, .. } => {
                referenced.insert(*relation);
                for player in players {
                    referenced.insert(player.player());
                    positive.extend([*relation, player.player()]);
                    connect(topology, *relation, player.player());
                }
            }
            QueryPattern::Value { left, right, .. } => {
                let left = operand_binding(left);
                let right = operand_binding(right);
                referenced.extend(left.into_iter().chain(right));
                if let (Some(left), Some(right)) = (left, right) {
                    connect(topology, left, right);
                }
            }
            QueryPattern::Not { .. } => {}
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "negation scoping threads the complete outer analysis state"
)]
fn validate_negations(
    patterns: &[QueryPattern],
    outer_domains: &BTreeMap<BindingId, BTreeSet<TypeId>>,
    schema: &ResolvedSchema,
    inputs: &[ValueTypeTag],
    outer_positive: &BTreeSet<BindingId>,
    all_constructible: &BTreeSet<TypeId>,
    scoped_positive: &mut BTreeSet<BindingId>,
    codes: &EngineCodes,
) -> Result<(), Diagnostic> {
    for pattern in patterns {
        if let QueryPattern::Not { patterns } = pattern {
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
                return Err(fail(codes.negation_unbound));
            }
            let mut nested = outer_domains.clone();
            for (id, domain) in &mut nested {
                if !outer_positive.contains(id) {
                    *domain = all_constructible.clone();
                }
            }
            refine_positive(patterns, &mut nested, schema, codes)?;
            if body_positive.iter().any(|id| nested[id].is_empty()) {
                return Err(fail(codes.empty_negated_domain));
            }
            validate_values_shallow(patterns, &nested, schema, inputs, codes)?;
            topology.retain(|id, _| body_references.contains(id));
            ensure_connected(&topology, codes)?;
            scoped_positive.extend(body_positive.iter().copied());
            validate_negations(
                patterns,
                &nested,
                schema,
                inputs,
                &body_scope,
                all_constructible,
                scoped_positive,
                codes,
            )?;
        }
    }
    Ok(())
}

fn validate_values_shallow(
    patterns: &[QueryPattern],
    domains: &BTreeMap<BindingId, BTreeSet<TypeId>>,
    schema: &ResolvedSchema,
    inputs: &[ValueTypeTag],
    codes: &EngineCodes,
) -> Result<(), Diagnostic> {
    for pattern in patterns {
        if let QueryPattern::Value { left, right, .. } = pattern {
            let left = operand_value_type(left, domains, schema, inputs, codes)?;
            let right = operand_value_type(right, domains, schema, inputs, codes)?;
            if left != right {
                return Err(fail(codes.value_domain_mismatch));
            }
        }
    }
    Ok(())
}

fn operand_value_type(
    operand: &QueryOperand,
    domains: &BTreeMap<BindingId, BTreeSet<TypeId>>,
    schema: &ResolvedSchema,
    inputs: &[ValueTypeTag],
    codes: &EngineCodes,
) -> Result<ValueTypeTag, Diagnostic> {
    match operand {
        QueryOperand::Literal { value } => Ok(value.value_type()),
        QueryOperand::Binding { binding } => {
            uniform_value_type(&domains[binding], schema, codes)?
                .ok_or_else(|| fail(codes.binding_not_scalar))
        }
        QueryOperand::Input { column } => inputs
            .get(usize::from(column.get()))
            .copied()
            .ok_or_else(|| fail(codes.unknown_input)),
    }
}

fn collect_references(patterns: &[QueryPattern], output: &mut BTreeSet<BindingId>) {
    for pattern in patterns {
        match pattern {
            QueryPattern::Isa { binding, .. } => {
                output.insert(*binding);
            }
            QueryPattern::Has { attribute, owner, .. } => {
                output.extend([*owner, *attribute]);
            }
            QueryPattern::Links { relation, players, .. } => {
                output.insert(*relation);
                output.extend(players.iter().map(|player| player.player()));
            }
            QueryPattern::Value { left, right, .. } => {
                output.extend(operand_binding(left));
                output.extend(operand_binding(right));
            }
            QueryPattern::Not { patterns } => collect_references(patterns, output),
        }
    }
}

fn operand_binding(operand: &QueryOperand) -> Option<BindingId> {
    match operand {
        QueryOperand::Binding { binding } => Some(*binding),
        QueryOperand::Literal { .. } | QueryOperand::Input { .. } => None,
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
    codes: &EngineCodes,
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
        Err(fail(codes.disconnected_topology))
    }
}

fn fail(code: EngineCode) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new(code.code).expect("static engine diagnostic code"),
        code.message,
    )
}
