//! The shared schema-aware pattern engine over the reusable query algebra.
//!
//! Both migration assertions and query plans validate their closed pattern
//! conjunctions here. The engine walks [`QueryPattern`] — assertions convert
//! injectively before calling — and reports through a caller-supplied
//! diagnostic table so each entry point keeps its released stable codes.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::id::{FunctionId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::{BindingId, ValueComparator};
use type_bridge_contract::query_plan::{QueryOperand, QueryPattern};
use type_bridge_contract::schema::{FunctionReturnMode, TypeReference};
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
    pub(crate) value_comparator_unsupported: EngineCode,
    pub(crate) binding_not_scalar: EngineCode,
    pub(crate) nonuniform_value_domain: EngineCode,
    pub(crate) disconnected_topology: EngineCode,
    pub(crate) unknown_input: EngineCode,
    pub(crate) unknown_function: EngineCode,
    pub(crate) function_return_unsupported: EngineCode,
    pub(crate) function_arity_mismatch: EngineCode,
    pub(crate) function_argument_type: EngineCode,
    pub(crate) function_dependency_cycle: EngineCode,
    pub(crate) value_binding_misuse: EngineCode,
    pub(crate) try_unbound: EngineCode,
    pub(crate) try_uncorrelated: EngineCode,
    pub(crate) empty_try_domain: EngineCode,
    pub(crate) try_binding_shared: EngineCode,
    pub(crate) local_unbound: EngineCode,
    pub(crate) local_uncorrelated: EngineCode,
    pub(crate) empty_local_domain: EngineCode,
    pub(crate) local_return_domain: EngineCode,
}

/// The trusted result of one root-conjunction analysis.
pub(crate) struct PatternAnalysis {
    pub(crate) domains: BTreeMap<BindingId, BTreeSet<TypeId>>,
    pub(crate) optional_positive: BTreeSet<BindingId>,
    pub(crate) positive: BTreeSet<BindingId>,
    pub(crate) scoped_positive: BTreeSet<BindingId>,
    pub(crate) used: BTreeSet<BindingId>,
    pub(crate) value_bindings: BTreeMap<BindingId, ValueTypeTag>,
}

/// The validated call signature of one plan-local function.
pub(crate) struct LocalFunctionSignature {
    pub(crate) parameters: Vec<BTreeSet<TypeId>>,
    pub(crate) returns: ValueTypeTag,
}

/// Analyze one plan-local function body and derive its call signature.
///
/// Parameters arrive bound: they are pre-positive in the body scope. The
/// body must establish every other reference, reference every parameter,
/// stay connected, and stay possible under the schema. The declared return
/// type must match the reducer over the input binding's scalar domain.
pub(crate) fn analyze_local_function(
    function: &type_bridge_contract::query_plan::LocalFunction,
    schema: &ResolvedSchema,
    codes: &EngineCodes,
) -> Result<LocalFunctionSignature, Diagnostic> {
    use type_bridge_contract::query_plan::Reducer;

    let all_constructible = schema
        .types()
        .iter()
        .filter(|(_, resolved)| resolved.is_constructible())
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    let binding_ids = (0..function.bindings().len())
        .map(|index| BindingId::new(u16::try_from(index).expect("dense ordinal")))
        .collect::<Result<Vec<_>, _>>()?;
    let mut domains = binding_ids
        .iter()
        .map(|id| (*id, all_constructible.clone()))
        .collect::<BTreeMap<_, _>>();
    for (index, label) in function.parameters().iter().enumerate() {
        let allowed = schema_reference_domain(label.as_str(), schema, codes)?;
        intersect_mut(
            domains
                .get_mut(&binding_ids[index])
                .expect("parameter binding"),
            &allowed,
        );
    }
    refine_positive(function.body(), &mut domains, schema, codes)?;

    let parameter_count = function.parameters().len();
    let mut positive: BTreeSet<BindingId> =
        binding_ids[..parameter_count].iter().copied().collect();
    let mut references = BTreeSet::new();
    let mut topology = binding_ids
        .iter()
        .map(|id| (*id, BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    collect_scope_topology(
        function.body(),
        &mut positive,
        &mut references,
        &mut topology,
    );
    if references.iter().any(|id| !positive.contains(id)) {
        return Err(fail(codes.local_unbound));
    }
    if binding_ids[..parameter_count]
        .iter()
        .any(|id| !references.contains(id))
    {
        return Err(fail(codes.local_uncorrelated));
    }
    if positive.iter().any(|id| domains[id].is_empty()) {
        return Err(fail(codes.empty_local_domain));
    }
    validate_values_shallow(
        function.body(),
        &domains,
        schema,
        &[],
        &BTreeMap::new(),
        codes,
    )?;
    retain_topology(&mut topology, &references);
    ensure_connected(&topology, codes)?;

    let returns = function.returns();
    let derived = match returns.reducer() {
        Reducer::Count => ValueTypeTag::Long,
        Reducer::Sum => match uniform_value_type(&domains[&returns.input()], schema, codes)? {
            Some(tag @ (ValueTypeTag::Long | ValueTypeTag::Double)) => tag,
            _ => return Err(fail(codes.local_return_domain)),
        },
        Reducer::Max | Reducer::Min | Reducer::Mean | Reducer::Median | Reducer::Std => {
            return Err(fail(codes.local_return_domain));
        }
    };
    if derived != returns.value_type() {
        return Err(fail(codes.local_return_domain));
    }
    if !references.contains(&returns.input()) {
        return Err(fail(codes.local_unbound));
    }
    Ok(LocalFunctionSignature {
        parameters: binding_ids[..parameter_count]
            .iter()
            .map(|id| domains[id].clone())
            .collect(),
        returns: returns.value_type(),
    })
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
    locals: &BTreeMap<FunctionId, LocalFunctionSignature>,
    additional_root_topology: &BTreeSet<(BindingId, BindingId)>,
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
    for (left, right) in additional_root_topology {
        connect(&mut topology, *left, *right);
    }
    if root_references.iter().any(|id| !positive.contains(id)) {
        return Err(fail(codes.root_reference_not_positive));
    }
    let value_bindings =
        analyze_function_calls(patterns, &mut domains, schema, inputs, locals, codes)?;
    refine_positive(patterns, &mut domains, schema, codes)?;
    for binding in value_bindings.keys() {
        domains.insert(*binding, BTreeSet::new());
    }
    validate_function_dependencies(patterns, codes)?;
    retain_topology(&mut topology, &root_references);
    prune_singleton_function_sources(patterns, &mut topology);
    let mut scoped_positive = BTreeSet::new();
    validate_negations(
        patterns,
        &domains,
        schema,
        inputs,
        &positive,
        &all_constructible,
        &mut scoped_positive,
        &value_bindings,
        codes,
    )?;
    let mut optional_positive = BTreeSet::new();
    validate_tries(
        patterns,
        &mut domains,
        schema,
        inputs,
        &positive,
        &all_constructible,
        &mut optional_positive,
        &value_bindings,
        codes,
    )?;
    if !scoped_positive.is_disjoint(&optional_positive) {
        return Err(fail(codes.try_binding_shared));
    }
    validate_values_shallow(patterns, &domains, schema, inputs, &value_bindings, codes)?;
    let mut used = BTreeSet::new();
    collect_references(patterns, &mut used);
    ensure_connected(&topology, codes)?;

    Ok(PatternAnalysis {
        domains,
        optional_positive,
        positive,
        scoped_positive,
        used,
        value_bindings,
    })
}

/// Resolve every root function call against the schema function table.
///
/// The first function vocabulary admits scalar non-optional returns only.
/// A value-returning call turns its assigned binding into a pure value
/// binding, which may then appear only as a comparison or argument operand.
fn analyze_function_calls(
    patterns: &[QueryPattern],
    domains: &mut BTreeMap<BindingId, BTreeSet<TypeId>>,
    schema: &ResolvedSchema,
    inputs: &[ValueTypeTag],
    locals: &BTreeMap<FunctionId, LocalFunctionSignature>,
    codes: &EngineCodes,
) -> Result<BTreeMap<BindingId, ValueTypeTag>, Diagnostic> {
    let mut value_bindings = BTreeMap::new();

    // Discover every result before checking any argument. Match conjunctions
    // are declarative, so a consumer may precede its producer in plan order.
    // This pass also makes value-return bindings available uniformly to the
    // argument checks below.
    for pattern in patterns {
        let QueryPattern::FunctionCall {
            arguments,
            assigned,
            function,
        } = pattern
        else {
            continue;
        };
        // Plan-local functions resolve before the schema function table;
        // shadowing a schema function is rejected upstream.
        if let Some(local) = locals.get(function) {
            if local.parameters.len() != arguments.len() {
                return Err(fail(codes.function_arity_mismatch));
            }
            if value_bindings.insert(*assigned, local.returns).is_some() {
                return Err(fail(codes.value_binding_misuse));
            }
            continue;
        }
        let resolved = schema
            .functions()
            .get(function)
            .ok_or_else(|| fail(codes.unknown_function))?;
        let signature = resolved.declaration().signature();
        let FunctionReturnMode::Scalar(element) = signature.returns() else {
            return Err(fail(codes.function_return_unsupported));
        };
        if element.optional() {
            return Err(fail(codes.function_return_unsupported));
        }
        if signature.parameters().len() != arguments.len() {
            return Err(fail(codes.function_arity_mismatch));
        }
        match element.type_ref() {
            TypeReference::Value(tag) => {
                if value_bindings.insert(*assigned, *tag).is_some() {
                    return Err(fail(codes.value_binding_misuse));
                }
            }
            TypeReference::Schema(label) => {
                let allowed = schema_reference_domain(label.as_str(), schema, codes)?;
                intersect_mut(
                    domains.get_mut(assigned).expect("declared binding"),
                    &allowed,
                );
            }
        }
    }

    // Apply every schema-position argument constraint against the complete
    // result environment. Keep scalar checks for the final pass: one call's
    // schema constraint may make another call's scalar domain uniform, and
    // conjunction order must not decide which signature is accepted.
    for pattern in patterns {
        let QueryPattern::FunctionCall {
            arguments,
            function,
            ..
        } = pattern
        else {
            continue;
        };
        if let Some(local) = locals.get(function) {
            for (allowed, argument) in local.parameters.iter().zip(arguments) {
                let QueryOperand::Binding { binding } = argument else {
                    return Err(fail(codes.function_argument_type));
                };
                if value_bindings.contains_key(binding) {
                    return Err(fail(codes.value_binding_misuse));
                }
                intersect_mut(domains.get_mut(binding).expect("declared binding"), allowed);
            }
            continue;
        }
        let resolved = schema
            .functions()
            .get(function)
            .ok_or_else(|| fail(codes.unknown_function))?;
        for (parameter, argument) in resolved
            .declaration()
            .signature()
            .parameters()
            .iter()
            .zip(arguments)
        {
            match parameter.type_ref() {
                TypeReference::Value(_) => {}
                TypeReference::Schema(label) => {
                    let QueryOperand::Binding { binding } = argument else {
                        return Err(fail(codes.function_argument_type));
                    };
                    if value_bindings.contains_key(binding) {
                        return Err(fail(codes.value_binding_misuse));
                    }
                    let allowed = schema_reference_domain(label.as_str(), schema, codes)?;
                    intersect_mut(
                        domains.get_mut(binding).expect("declared binding"),
                        &allowed,
                    );
                }
            }
        }
    }

    // Propagate all schema-position call constraints through the ordinary
    // positive pattern fixpoint before any scalar domain is claimed.
    refine_positive(patterns, domains, schema, codes)?;
    for pattern in patterns {
        let QueryPattern::FunctionCall {
            arguments,
            function,
            ..
        } = pattern
        else {
            continue;
        };
        if locals.contains_key(function) {
            continue;
        }
        let resolved = schema
            .functions()
            .get(function)
            .ok_or_else(|| fail(codes.unknown_function))?;
        for (parameter, argument) in resolved
            .declaration()
            .signature()
            .parameters()
            .iter()
            .zip(arguments)
        {
            let TypeReference::Value(expected) = parameter.type_ref() else {
                continue;
            };
            let actual =
                operand_value_type(argument, domains, schema, inputs, &value_bindings, codes)?;
            if actual != *expected {
                return Err(fail(codes.function_argument_type));
            }
        }
    }
    if !value_bindings.is_empty() {
        ensure_value_bindings_stay_scalar(patterns, &value_bindings, codes)?;
    }
    Ok(value_bindings)
}

/// Require root function results to form an acyclic dataflow graph.
///
/// A call result is a positive producer; a binding argument is only a
/// reference. References to non-call producers are checked by the ordinary
/// root-scope discipline, while references to call results form dependency
/// edges checked here independent of conjunction order.
fn validate_function_dependencies(
    patterns: &[QueryPattern],
    codes: &EngineCodes,
) -> Result<(), Diagnostic> {
    let mut producers = BTreeSet::new();
    for pattern in patterns {
        if let QueryPattern::FunctionCall { assigned, .. } = pattern
            && !producers.insert(*assigned)
        {
            return Err(fail(codes.value_binding_misuse));
        }
    }

    let mut dependencies = producers
        .iter()
        .map(|binding| (*binding, BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    let mut dependents = dependencies.clone();
    for pattern in patterns {
        let QueryPattern::FunctionCall {
            arguments,
            assigned,
            ..
        } = pattern
        else {
            continue;
        };
        for dependency in arguments.iter().filter_map(operand_binding) {
            if producers.contains(&dependency)
                && dependencies
                    .get_mut(assigned)
                    .expect("registered producer")
                    .insert(dependency)
            {
                dependents
                    .get_mut(&dependency)
                    .expect("registered producer")
                    .insert(*assigned);
            }
        }
    }

    let mut ready = dependencies
        .iter()
        .filter_map(|(binding, dependencies)| dependencies.is_empty().then_some(*binding))
        .collect::<VecDeque<_>>();
    let mut resolved = 0_usize;
    while let Some(binding) = ready.pop_front() {
        resolved += 1;
        for dependent in &dependents[&binding] {
            let unresolved = dependencies
                .get_mut(dependent)
                .expect("registered dependent");
            unresolved.remove(&binding);
            if unresolved.is_empty() {
                ready.push_back(*dependent);
            }
        }
    }
    if resolved == producers.len() {
        Ok(())
    } else {
        Err(fail(codes.function_dependency_cycle))
    }
}

/// Resolve one schema-position type reference to its constructible domain.
fn schema_reference_domain(
    label: &str,
    schema: &ResolvedSchema,
    codes: &EngineCodes,
) -> Result<BTreeSet<TypeId>, Diagnostic> {
    for kind in [TypeKind::Entity, TypeKind::Relation, TypeKind::Attribute] {
        let Ok(candidate) = TypeId::new(kind, label.to_owned()) else {
            continue;
        };
        let Some(resolved) = schema.types().get(&candidate) else {
            continue;
        };
        let mut allowed = BTreeSet::from([candidate]);
        allowed.extend(resolved.subtypes().iter().cloned());
        allowed.retain(|id| {
            schema
                .types()
                .get(id)
                .is_some_and(|ty| ty.is_constructible())
        });
        return Ok(allowed);
    }
    Err(fail(codes.unknown_function))
}

fn ensure_value_bindings_stay_scalar(
    patterns: &[QueryPattern],
    value_bindings: &BTreeMap<BindingId, ValueTypeTag>,
    codes: &EngineCodes,
) -> Result<(), Diagnostic> {
    for pattern in patterns {
        let misuse = match pattern {
            QueryPattern::Isa { binding, .. } => value_bindings.contains_key(binding),
            QueryPattern::Has {
                attribute, owner, ..
            } => value_bindings.contains_key(attribute) || value_bindings.contains_key(owner),
            QueryPattern::Links {
                relation, players, ..
            } => {
                value_bindings.contains_key(relation)
                    || players
                        .iter()
                        .any(|player| value_bindings.contains_key(&player.player()))
            }
            QueryPattern::Value { .. } | QueryPattern::FunctionCall { .. } => false,
            QueryPattern::Reachable { source, target, .. } => {
                value_bindings.contains_key(source) || value_bindings.contains_key(target)
            }
            QueryPattern::Or { branches } => {
                for branch in branches {
                    ensure_value_bindings_stay_scalar(branch, value_bindings, codes)?;
                }
                false
            }
            QueryPattern::Not { patterns } | QueryPattern::Try { patterns } => {
                ensure_value_bindings_stay_scalar(patterns, value_bindings, codes)?;
                false
            }
        };
        if misuse {
            return Err(fail(codes.value_binding_misuse));
        }
    }
    Ok(())
}

/// Return the uniform scalar domain of one refined binding domain.
pub(crate) fn uniform_value_type(
    domain: &BTreeSet<TypeId>,
    schema: &ResolvedSchema,
    codes: &EngineCodes,
) -> Result<Option<ValueTypeTag>, Diagnostic> {
    let mut uniform: Option<Option<ValueTypeTag>> = None;
    for id in domain {
        let value_type = schema.types()[id]
            .value_type()
            .map(|value| value.value_type());
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
                schema
                    .types()
                    .get(id)
                    .is_some_and(|ty| ty.is_constructible())
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
            if relation_id.kind() != TypeKind::Relation || !schema.types().contains_key(relation_id)
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
                        schema
                            .types()
                            .get(*id)
                            .is_some_and(|ty| ty.is_constructible())
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
        QueryPattern::Reachable {
            relation,
            role_from,
            role_to,
            source,
            target,
            ..
        } => {
            if relation.kind() != TypeKind::Relation || !schema.types().contains_key(relation) {
                return Err(fail(codes.unknown_relation));
            }
            let mut changed = false;
            for (role, binding) in [(role_from, source), (role_to, target)] {
                let resolved = schema
                    .roles()
                    .get(role)
                    .ok_or_else(|| fail(codes.unknown_role))?;
                if !schema.types()[relation].relates().contains_key(role) {
                    return Err(fail(codes.role_relation_mismatch));
                }
                let accepted = resolved
                    .accepted_players()
                    .iter()
                    .filter(|id| {
                        schema
                            .types()
                            .get(*id)
                            .is_some_and(|ty| ty.is_constructible())
                    })
                    .cloned()
                    .collect::<BTreeSet<_>>();
                changed |= intersect_mut(
                    domains.get_mut(binding).expect("declared binding"),
                    &accepted,
                );
            }
            Ok(changed)
        }
        QueryPattern::Or { branches } => {
            let mut union = domains
                .keys()
                .copied()
                .map(|binding| (binding, BTreeSet::new()))
                .collect::<BTreeMap<_, _>>();
            for branch in branches {
                let mut branch_domains = domains.clone();
                refine_positive(branch, &mut branch_domains, schema, codes)?;
                for (binding, domain) in branch_domains {
                    union
                        .get_mut(&binding)
                        .expect("declared binding")
                        .extend(domain);
                }
            }
            let mut changed = false;
            for (binding, allowed) in union {
                changed |= intersect_mut(
                    domains.get_mut(&binding).expect("declared binding"),
                    &allowed,
                );
            }
            Ok(changed)
        }
        QueryPattern::Value { .. }
        | QueryPattern::Not { .. }
        | QueryPattern::Try { .. }
        | QueryPattern::FunctionCall { .. } => Ok(false),
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
            QueryPattern::Has {
                attribute, owner, ..
            } => {
                referenced.extend([*owner, *attribute]);
                positive.extend([*owner, *attribute]);
                connect(topology, *owner, *attribute);
            }
            QueryPattern::Links {
                relation, players, ..
            } => {
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
            QueryPattern::Or { branches } => {
                let mut branch_positive = Vec::with_capacity(branches.len());
                let mut common_references: Option<BTreeSet<BindingId>> = None;
                let mut external_references = BTreeSet::new();
                for branch in branches {
                    let mut local_positive = BTreeSet::new();
                    let mut local_referenced = BTreeSet::new();
                    let mut local_topology = topology
                        .keys()
                        .copied()
                        .map(|binding| (binding, BTreeSet::new()))
                        .collect::<BTreeMap<_, _>>();
                    collect_scope_topology(
                        branch,
                        &mut local_positive,
                        &mut local_referenced,
                        &mut local_topology,
                    );
                    external_references
                        .extend(local_referenced.difference(&local_positive).copied());
                    match &mut common_references {
                        Some(common) => {
                            common.retain(|binding| local_referenced.contains(binding));
                        }
                        None => common_references = Some(local_referenced),
                    }
                    branch_positive.push(local_positive);
                }
                let mut guaranteed = branch_positive.pop().unwrap_or_default();
                for branch in branch_positive {
                    guaranteed.retain(|binding| branch.contains(binding));
                }
                referenced.extend(guaranteed.iter().copied());
                referenced.extend(external_references);
                positive.extend(guaranteed);
                let common_references = common_references.unwrap_or_default();
                for left in &common_references {
                    for right in common_references
                        .range((std::ops::Bound::Excluded(left), std::ops::Bound::Unbounded))
                    {
                        connect(topology, *left, *right);
                    }
                }
            }
            QueryPattern::Not { .. } | QueryPattern::Try { .. } => {}
            QueryPattern::Reachable { source, target, .. } => {
                referenced.extend([*source, *target]);
                positive.extend([*source, *target]);
                connect(topology, *source, *target);
            }
            QueryPattern::FunctionCall {
                arguments,
                assigned,
                ..
            } => {
                referenced.insert(*assigned);
                positive.insert(*assigned);
                for argument in arguments {
                    if let Some(binding) = operand_binding(argument) {
                        referenced.insert(binding);
                        connect(topology, *assigned, binding);
                    }
                }
            }
        }
    }
}

/// Remove call-result vertices that cannot introduce row multiplicity.
///
/// This runs only after every call has resolved to a scalar, non-optional
/// signature. A call whose arguments are all literals or invocation inputs
/// produces one result for the invocation row, so combining it with an
/// independent graph component is not a Cartesian product. Calls with any
/// binding argument remain in the topology and retain the ordinary
/// cross-product guard.
fn prune_singleton_function_sources(
    patterns: &[QueryPattern],
    topology: &mut BTreeMap<BindingId, BTreeSet<BindingId>>,
) {
    let singleton_sources = patterns
        .iter()
        .filter_map(|pattern| {
            let QueryPattern::FunctionCall {
                arguments,
                assigned,
                ..
            } = pattern
            else {
                return None;
            };
            arguments
                .iter()
                .all(|argument| operand_binding(argument).is_none())
                .then_some(*assigned)
        })
        .collect::<BTreeSet<_>>();
    topology.retain(|binding, _| !singleton_sources.contains(binding));
    for neighbors in topology.values_mut() {
        neighbors.retain(|binding| !singleton_sources.contains(binding));
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
    value_bindings: &BTreeMap<BindingId, ValueTypeTag>,
    codes: &EngineCodes,
) -> Result<(), Diagnostic> {
    for pattern in patterns {
        if let QueryPattern::Or { branches } = pattern {
            let mut guaranteed: Option<BTreeSet<BindingId>> = None;
            let mut branch_states = Vec::with_capacity(branches.len());
            for branch in branches {
                let mut branch_positive = BTreeSet::new();
                let mut branch_references = BTreeSet::new();
                let mut branch_topology = outer_domains
                    .keys()
                    .copied()
                    .map(|id| (id, BTreeSet::new()))
                    .collect::<BTreeMap<_, _>>();
                collect_scope_topology(
                    branch,
                    &mut branch_positive,
                    &mut branch_references,
                    &mut branch_topology,
                );
                match &mut guaranteed {
                    Some(intersection) => {
                        intersection.retain(|binding| branch_positive.contains(binding));
                    }
                    None => guaranteed = Some(branch_positive.clone()),
                }
                branch_states.push((branch, branch_positive, branch_references, branch_topology));
            }
            let guaranteed = guaranteed.unwrap_or_default();
            for (branch, branch_positive, branch_references, mut branch_topology) in branch_states {
                let mut branch_scope = outer_positive.clone();
                branch_scope.extend(branch_positive.iter().copied());
                if branch_references
                    .iter()
                    .any(|binding| !branch_scope.contains(binding))
                {
                    return Err(fail(codes.root_reference_not_positive));
                }
                let mut branch_domains = outer_domains.clone();
                refine_positive(branch, &mut branch_domains, schema, codes)?;
                if branch_positive
                    .iter()
                    .any(|binding| branch_domains[binding].is_empty())
                {
                    return Err(fail(codes.empty_negated_domain));
                }
                validate_values_shallow(
                    branch,
                    &branch_domains,
                    schema,
                    inputs,
                    value_bindings,
                    codes,
                )?;
                retain_topology(&mut branch_topology, &branch_references);
                ensure_connected(&branch_topology, codes)?;
                scoped_positive.extend(branch_positive.difference(&guaranteed).copied());
                validate_negations(
                    branch,
                    &branch_domains,
                    schema,
                    inputs,
                    &branch_scope,
                    all_constructible,
                    scoped_positive,
                    value_bindings,
                    codes,
                )?;
            }
        } else if let QueryPattern::Not { patterns } = pattern {
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
            validate_values_shallow(patterns, &nested, schema, inputs, value_bindings, codes)?;
            retain_topology(&mut topology, &body_references);
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
                value_bindings,
                codes,
            )?;
        }
    }
    Ok(())
}

/// Validate every root optional block and export its locals.
///
/// A try body never filters rows: its patterns refine only the bindings it
/// establishes, which join the analysis as optional. The body must be
/// self-contained, internally connected, correlated with at least one
/// mandatory binding, and possible under the schema.
#[expect(
    clippy::too_many_arguments,
    reason = "optional scoping threads the complete outer analysis state"
)]
fn validate_tries(
    patterns: &[QueryPattern],
    domains: &mut BTreeMap<BindingId, BTreeSet<TypeId>>,
    schema: &ResolvedSchema,
    inputs: &[ValueTypeTag],
    outer_positive: &BTreeSet<BindingId>,
    all_constructible: &BTreeSet<TypeId>,
    optional_positive: &mut BTreeSet<BindingId>,
    value_bindings: &BTreeMap<BindingId, ValueTypeTag>,
    codes: &EngineCodes,
) -> Result<(), Diagnostic> {
    for pattern in patterns {
        let QueryPattern::Try { patterns: body } = pattern else {
            continue;
        };
        let mut body_positive = BTreeSet::new();
        let mut body_references = BTreeSet::new();
        let mut topology = domains
            .keys()
            .copied()
            .map(|id| (id, BTreeSet::new()))
            .collect::<BTreeMap<_, _>>();
        collect_scope_topology(
            body,
            &mut body_positive,
            &mut body_references,
            &mut topology,
        );
        let mut body_scope = outer_positive.clone();
        body_scope.extend(body_positive.iter().copied());
        if body_references.iter().any(|id| !body_scope.contains(id)) {
            return Err(fail(codes.try_unbound));
        }
        if !body_references.iter().any(|id| outer_positive.contains(id)) {
            return Err(fail(codes.try_uncorrelated));
        }
        let mut nested = domains.clone();
        for (id, domain) in &mut nested {
            if !outer_positive.contains(id) {
                *domain = all_constructible.clone();
            }
        }
        refine_positive(body, &mut nested, schema, codes)?;
        if body_positive.iter().any(|id| nested[id].is_empty()) {
            return Err(fail(codes.empty_try_domain));
        }
        validate_values_shallow(body, &nested, schema, inputs, value_bindings, codes)?;
        retain_topology(&mut topology, &body_references);
        ensure_connected(&topology, codes)?;
        for id in &body_positive {
            if !outer_positive.contains(id) {
                if !optional_positive.insert(*id) {
                    return Err(fail(codes.try_binding_shared));
                }
                domains.insert(*id, nested[id].clone());
            }
        }
    }
    Ok(())
}

fn validate_values_shallow(
    patterns: &[QueryPattern],
    domains: &BTreeMap<BindingId, BTreeSet<TypeId>>,
    schema: &ResolvedSchema,
    inputs: &[ValueTypeTag],
    value_bindings: &BTreeMap<BindingId, ValueTypeTag>,
    codes: &EngineCodes,
) -> Result<(), Diagnostic> {
    for pattern in patterns {
        if let QueryPattern::Or { branches } = pattern {
            for branch in branches {
                let mut branch_domains = domains.clone();
                refine_positive(branch, &mut branch_domains, schema, codes)?;
                validate_values_shallow(
                    branch,
                    &branch_domains,
                    schema,
                    inputs,
                    value_bindings,
                    codes,
                )?;
            }
            continue;
        }
        if let QueryPattern::Value {
            comparator,
            left,
            right,
        } = pattern
        {
            let left = operand_value_type(left, domains, schema, inputs, value_bindings, codes)?;
            let right = operand_value_type(right, domains, schema, inputs, value_bindings, codes)?;
            if left != right {
                return Err(fail(codes.value_domain_mismatch));
            }
            if left == ValueTypeTag::Duration
                && !matches!(
                    comparator,
                    ValueComparator::Equal | ValueComparator::NotEqual
                )
            {
                return Err(fail(codes.value_comparator_unsupported));
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
    value_bindings: &BTreeMap<BindingId, ValueTypeTag>,
    codes: &EngineCodes,
) -> Result<ValueTypeTag, Diagnostic> {
    match operand {
        QueryOperand::Literal { value } => {
            type_bridge_schema::validate_provider_temporal_literal(value)?;
            Ok(value.value_type())
        }
        QueryOperand::Binding { binding } => {
            if let Some(tag) = value_bindings.get(binding) {
                return Ok(*tag);
            }
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
            QueryPattern::Has {
                attribute, owner, ..
            } => {
                output.extend([*owner, *attribute]);
            }
            QueryPattern::Links {
                relation, players, ..
            } => {
                output.insert(*relation);
                output.extend(players.iter().map(|player| player.player()));
            }
            QueryPattern::Value { left, right, .. } => {
                output.extend(operand_binding(left));
                output.extend(operand_binding(right));
            }
            QueryPattern::Or { branches } => {
                for branch in branches {
                    collect_references(branch, output);
                }
            }
            QueryPattern::Not { patterns } | QueryPattern::Try { patterns } => {
                collect_references(patterns, output);
            }
            QueryPattern::Reachable { source, target, .. } => {
                output.extend([*source, *target]);
            }
            QueryPattern::FunctionCall {
                arguments,
                assigned,
                ..
            } => {
                output.insert(*assigned);
                for argument in arguments {
                    output.extend(operand_binding(argument));
                }
            }
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
    topology
        .get_mut(&left)
        .expect("declared binding")
        .insert(right);
    topology
        .get_mut(&right)
        .expect("declared binding")
        .insert(left);
}

fn retain_topology(
    topology: &mut BTreeMap<BindingId, BTreeSet<BindingId>>,
    retained: &BTreeSet<BindingId>,
) {
    topology.retain(|binding, _| retained.contains(binding));
    for neighbors in topology.values_mut() {
        neighbors.retain(|binding| retained.contains(binding));
    }
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
        let Some(neighbors) = topology.get(&id) else {
            continue;
        };
        for neighbor in neighbors {
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
