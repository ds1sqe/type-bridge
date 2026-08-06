//! Schema-derived proof for additive model-query compatibility metadata.
//!
//! Query-plan V2 carries a complete hydration graph so a remote response can
//! be validated without client-side discovery. The graph is still untrusted
//! wire data: this module recomputes every provider-semantic claim from the
//! resolved schema before a plan can become [`ValidatedQuery`](crate::ValidatedQuery).
//! Registry-owned aliases and ordered/distinct model flags are intentionally
//! outside this proof; they never reach TypeQL and are checked by the model
//! facade against its request-bound registry projection.

use std::collections::{BTreeMap, BTreeSet};

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::BindingId;
use type_bridge_contract::query_plan::{
    HydrationDescriptorV2, HydrationProjectionV2, ModelQueryV2, QueryModelOutputSlotV2,
    QueryOutput, QueryPattern, QueryPatternV2, QueryPlan, QueryPlanV2Compatibility, ReadStage,
};
use type_bridge_schema::{ResolvedSchema, ResolvedType};

/// Schema-derived compatibility facts consumed by ordinary query validation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct V2SchemaClaimProof {
    empty_runtime_bindings: BTreeSet<BindingId>,
}

impl V2SchemaClaimProof {
    /// Whether schema authority proves one model thing binding has no possible
    /// concrete runtime type under its exact/subtype target.
    pub(crate) fn proves_empty_runtime_binding(&self, binding: BindingId) -> bool {
        self.empty_runtime_bindings.contains(&binding)
    }
}

/// Recompute all provider-semantic V2 model claims from the resolved schema.
pub(crate) fn validate_v2_schema_claims(
    plan: &QueryPlan,
    schema: &ResolvedSchema,
) -> Result<V2SchemaClaimProof, Diagnostic> {
    let Some(compatibility) = plan.v2_compatibility() else {
        return Ok(V2SchemaClaimProof::default());
    };
    let Some(model_query) = compatibility.model_query() else {
        if compatibility.predicate().is_some() || !compatibility.allowed_cross_joins().is_empty() {
            return Err(claim_failure(
                "query_plan_v2_compatibility_correspondence",
                "compatibility predicates and topology require one closed model terminal",
            ));
        }
        return Ok(V2SchemaClaimProof::default());
    };
    let hydration = model_hydration(model_query);
    validate_adapter_normal_form(plan, compatibility, model_query, hydration)?;
    let targets = root_thing_targets(plan)?;
    let mut proof = V2SchemaClaimProof::default();
    let mut admitted = BTreeSet::new();

    for binding in hydration.bindings() {
        let Some((declared, include_subtypes)) = targets.get(&binding.binding()) else {
            return Err(claim_failure(
                "query_plan_v2_binding_claim",
                "a hydrated binding lacks one unambiguous root thing target",
            ));
        };
        if declared != binding.declared_descriptor() {
            return Err(claim_failure(
                "query_plan_v2_binding_claim",
                "a hydrated binding declaration contradicts its root thing target",
            ));
        }
        let expected = constructible_closure(schema, declared, *include_subtypes)?;
        if binding.concrete_descriptors() != expected.as_slice() {
            return Err(claim_failure(
                "query_plan_v2_binding_closure_claim",
                "a hydrated binding concrete closure contradicts resolved schema",
            ));
        }
        if expected.is_empty() {
            proof.empty_runtime_bindings.insert(binding.binding());
        }
        admitted.extend(expected);
    }

    // Released nested relation-player hydration is deliberately shallow:
    // selected relation bindings expose complete roles, while a nested player
    // exposes only its descriptor and attributes. Expanding roles
    // transitively would hydrate unrelated graph regions and would contradict
    // the V1 result validator, which stops at nested-player attributes.
    let role_hydrated_descriptors = admitted
        .iter()
        .filter(|descriptor| descriptor.kind() == TypeKind::Relation)
        .cloned()
        .collect::<BTreeSet<_>>();
    for descriptor in &role_hydrated_descriptors {
        let resolved = schema.types().get(descriptor).ok_or_else(|| {
            claim_failure(
                "query_plan_v2_descriptor_claim",
                "a hydration descriptor is absent from resolved schema",
            )
        })?;
        for role in resolved.relates().keys() {
            let accepted = schema
                .roles()
                .get(role)
                .ok_or_else(|| {
                    claim_failure(
                        "query_plan_v2_role_claim",
                        "an effective relation role is absent from resolved role authority",
                    )
                })?
                .accepted_players();
            admitted.extend(accepted.iter().cloned());
        }
    }

    let claimed = hydration
        .descriptors()
        .iter()
        .map(|descriptor| descriptor.descriptor().clone())
        .collect::<BTreeSet<_>>();
    if claimed != admitted {
        return Err(claim_failure(
            "query_plan_v2_descriptor_closure_claim",
            "hydration descriptors are not the exact schema-derived recursive closure",
        ));
    }
    for descriptor in hydration.descriptors() {
        validate_descriptor_claim(
            descriptor,
            role_hydrated_descriptors.contains(descriptor.descriptor()),
            schema,
        )?;
    }
    validate_compatibility_references(compatibility.predicate(), hydration, &targets, schema)?;
    Ok(proof)
}

/// Adapter-authored compatibility plans have one closed executable form.
///
/// The ordinary root is only a target/output skeleton. The compatibility tree
/// is the sole predicate source consumed by compatibility lowering, so any
/// extra native pattern or stage would be a second, attacker-controlled
/// semantic program. Reject that ambiguity before ordinary analysis or
/// lowering.
fn validate_adapter_normal_form(
    plan: &QueryPlan,
    compatibility: &QueryPlanV2Compatibility,
    model_query: &ModelQueryV2,
    hydration: &HydrationProjectionV2,
) -> Result<(), Diagnostic> {
    if !plan.inputs().is_empty() || !plan.functions().is_empty() {
        return Err(correspondence_failure(
            "adapter compatibility plans cannot carry native inputs or local functions",
        ));
    }
    if hydration.bindings().len() != plan.bindings().len()
        || hydration
            .bindings()
            .iter()
            .zip(plan.bindings())
            .any(|(hydrated, declared)| hydrated.binding() != declared.id())
    {
        return Err(correspondence_failure(
            "adapter hydration bindings must exactly cover dense native bindings",
        ));
    }
    if plan
        .bindings()
        .iter()
        .enumerate()
        .any(|(index, binding)| binding.variable().as_str() != format!("b{index}"))
    {
        return Err(correspondence_failure(
            "adapter native binding variables are not in canonical ordinal form",
        ));
    }

    let Some(ReadStage::Match { patterns }) = plan.pipeline().first() else {
        return Err(correspondence_failure(
            "adapter compatibility plans require one root match skeleton",
        ));
    };
    if patterns.len() != hydration.bindings().len()
        || patterns
            .iter()
            .zip(hydration.bindings())
            .any(|(pattern, hydrated)| {
                !matches!(
                    pattern,
                    QueryPattern::Isa {
                        binding,
                        type_id,
                        ..
                    } if *binding == hydrated.binding()
                        && type_id == hydrated.declared_descriptor()
                )
            })
    {
        return Err(correspondence_failure(
            "adapter root match must contain exactly one ordered isa target per binding",
        ));
    }

    let output_columns = model_native_output_columns(model_query);
    let mut selected = output_columns.clone();
    selected.sort_unstable();
    selected.dedup();
    if plan.pipeline().len() != 3
        || !matches!(
            &plan.pipeline()[1],
            ReadStage::Select { bindings } if bindings == &selected
        )
        || !matches!(&plan.pipeline()[2], ReadStage::Distinct)
    {
        return Err(correspondence_failure(
            "adapter native pipeline must be the canonical select-distinct skeleton",
        ));
    }
    if !matches!(
        plan.output(),
        QueryOutput::Rows { columns } if columns == &output_columns
    ) {
        return Err(correspondence_failure(
            "adapter native output does not match its compatibility terminal identity",
        ));
    }

    // Explicit cross joins live only in the compatibility topology proof.
    // Their context-free ordering/bounds were checked by the contract; the
    // ordinary engine consumes them through `v2_root_topology`.
    let _ = compatibility.allowed_cross_joins();
    Ok(())
}

fn model_native_output_columns(query: &ModelQueryV2) -> Vec<BindingId> {
    match query {
        ModelQueryV2::Rows { output, .. } => output
            .slots()
            .into_iter()
            .filter_map(|slot| match slot {
                QueryModelOutputSlotV2::One { binding, .. } => Some(*binding),
                QueryModelOutputSlotV2::Collect { .. } => None,
            })
            .collect(),
        ModelQueryV2::Page { root, .. }
        | ModelQueryV2::DistinctCount { root, .. }
        | ModelQueryV2::DistinctExists { root, .. } => vec![*root],
    }
}

fn correspondence_failure(message: &'static str) -> Diagnostic {
    claim_failure("query_plan_v2_compatibility_correspondence", message)
}

fn model_hydration(query: &ModelQueryV2) -> &HydrationProjectionV2 {
    match query {
        ModelQueryV2::Rows { hydration, .. }
        | ModelQueryV2::Page { hydration, .. }
        | ModelQueryV2::DistinctCount { hydration, .. }
        | ModelQueryV2::DistinctExists { hydration, .. } => hydration,
    }
}

fn root_thing_targets(plan: &QueryPlan) -> Result<BTreeMap<BindingId, (TypeId, bool)>, Diagnostic> {
    let Some(ReadStage::Match { patterns }) = plan.pipeline().first() else {
        return Err(claim_failure(
            "query_plan_v2_binding_claim",
            "a model query lacks its root match stage",
        ));
    };
    let mut targets = BTreeMap::new();
    for pattern in patterns {
        if let QueryPattern::Isa {
            binding,
            include_subtypes,
            type_id,
        } = pattern
            && targets
                .insert(*binding, (type_id.clone(), *include_subtypes))
                .is_some()
        {
            return Err(claim_failure(
                "query_plan_v2_binding_claim",
                "a model binding has more than one root thing target",
            ));
        }
    }
    Ok(targets)
}

fn constructible_closure(
    schema: &ResolvedSchema,
    declared: &TypeId,
    include_subtypes: bool,
) -> Result<Vec<TypeId>, Diagnostic> {
    if !matches!(declared.kind(), TypeKind::Entity | TypeKind::Relation) {
        return Err(claim_failure(
            "query_plan_v2_binding_claim",
            "a model target must declare an entity or relation",
        ));
    }
    let resolved = schema.types().get(declared).ok_or_else(|| {
        claim_failure(
            "query_plan_v2_binding_claim",
            "a model target is absent from resolved schema",
        )
    })?;
    let candidates = std::iter::once(declared).chain(
        include_subtypes
            .then_some(resolved.subtypes().iter())
            .into_iter()
            .flatten(),
    );
    let mut candidates = candidates
        .filter(|candidate| {
            schema
                .types()
                .get(*candidate)
                .is_some_and(ResolvedType::is_constructible)
        })
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.dedup();
    Ok(candidates)
}

fn validate_descriptor_claim(
    claimed: &HydrationDescriptorV2,
    hydrate_roles: bool,
    schema: &ResolvedSchema,
) -> Result<(), Diagnostic> {
    let resolved = schema.types().get(claimed.descriptor()).ok_or_else(|| {
        claim_failure(
            "query_plan_v2_descriptor_claim",
            "a hydration descriptor is absent from resolved schema",
        )
    })?;
    if !resolved.is_constructible()
        || !matches!(
            claimed.descriptor().kind(),
            TypeKind::Entity | TypeKind::Relation
        )
    {
        return Err(claim_failure(
            "query_plan_v2_descriptor_claim",
            "hydration descriptors must be constructible resolved things",
        ));
    }

    let claimed_attributes = claimed
        .fields()
        .iter()
        .map(|field| field.attribute().clone())
        .collect::<BTreeSet<_>>();
    let expected_attributes = resolved.owns().keys().cloned().collect::<BTreeSet<_>>();
    if claimed_attributes != expected_attributes {
        return Err(claim_failure(
            "query_plan_v2_field_set_claim",
            "hydration fields are not the complete effective ownership set",
        ));
    }
    for field in claimed.fields() {
        let owns = resolved.owns().get(field.attribute()).ok_or_else(|| {
            claim_failure(
                "query_plan_v2_field_claim",
                "a hydration field is absent from its resolved owner",
            )
        })?;
        let attribute = attribute_type(schema, field.attribute())?;
        if attribute.value_type().map(|value| value.value_type()) != Some(field.value_type())
            || owns.cardinality() != field.cardinality()
            || (owns.is_key() || owns.is_unique()) != field.unique()
            || expected_field_reference_owners(schema, resolved, field.attribute())
                != field.reference_owners()
        {
            return Err(claim_failure(
                "query_plan_v2_field_claim",
                "a hydration field provider identity, type, cardinality, uniqueness, or reference owner contradicts resolved schema",
            ));
        }
    }

    if claimed.descriptor().kind() == TypeKind::Entity || !hydrate_roles {
        if claimed.roles().is_empty() {
            return Ok(());
        }
        return Err(claim_failure(
            "query_plan_v2_role_set_claim",
            "a descriptor outside top-level relation hydration claims relation roles",
        ));
    }
    validate_relation_roles(claimed, resolved, schema)
}

fn attribute_type<'schema>(
    schema: &'schema ResolvedSchema,
    attribute: &AttributeId,
) -> Result<&'schema ResolvedType, Diagnostic> {
    let id =
        TypeId::new(TypeKind::Attribute, attribute.label().as_str().to_owned()).map_err(|_| {
            claim_failure(
                "query_plan_v2_field_claim",
                "a hydration provider attribute identity is malformed",
            )
        })?;
    schema.types().get(&id).ok_or_else(|| {
        claim_failure(
            "query_plan_v2_field_claim",
            "a hydration provider attribute is absent from resolved schema",
        )
    })
}

fn expected_field_reference_owners<'schema>(
    schema: &'schema ResolvedSchema,
    concrete: &'schema ResolvedType,
    attribute: &AttributeId,
) -> Vec<TypeId> {
    let Some(concrete_owns) = concrete.owns().get(attribute) else {
        return Vec::new();
    };
    let declared = concrete_owns.origin().declared();
    let mut owners = std::iter::once(concrete.id())
        .chain(concrete.supertypes())
        .filter(|owner| {
            schema
                .types()
                .get(*owner)
                .and_then(|resolved| resolved.owns().get(attribute))
                .is_some_and(|owns| owns.origin().declared() == declared)
        })
        .cloned()
        .collect::<Vec<_>>();
    owners.sort();
    owners.dedup();
    owners
}

fn validate_relation_roles(
    claimed: &HydrationDescriptorV2,
    resolved: &ResolvedType,
    schema: &ResolvedSchema,
) -> Result<(), Diagnostic> {
    let expected_roles = resolved
        .relates()
        .keys()
        .map(|role| {
            RoleId::new(
                claimed.descriptor().label().as_str().to_owned(),
                role.label().as_str().to_owned(),
            )
            .expect("resolved role labels are canonical")
        })
        .collect::<BTreeSet<_>>();
    let claimed_roles = claimed
        .roles()
        .iter()
        .map(|role| role.role().clone())
        .collect::<BTreeSet<_>>();
    if claimed_roles != expected_roles {
        return Err(claim_failure(
            "query_plan_v2_role_set_claim",
            "hydration roles are not the complete effective relation role set",
        ));
    }

    for role in claimed.roles() {
        let (effective_id, effective) = resolved
            .relates()
            .iter()
            .find(|(candidate, _)| candidate.label() == role.role().label())
            .ok_or_else(|| {
                claim_failure(
                    "query_plan_v2_role_claim",
                    "a hydration role is absent from its resolved relation",
                )
            })?;
        if effective.cardinality() != role.cardinality()
            || expected_role_references(schema, resolved, effective_id) != role.reference_roles()
        {
            return Err(claim_failure(
                "query_plan_v2_role_claim",
                "a hydration role owner, cardinality, or reference authority contradicts resolved schema",
            ));
        }
        let accepted = schema
            .roles()
            .get(effective_id)
            .ok_or_else(|| {
                claim_failure(
                    "query_plan_v2_role_claim",
                    "a hydration role lacks resolved player authority",
                )
            })?
            .accepted_players();
        let mut claimed_players = BTreeSet::new();
        for player in role.players() {
            let expected = constructible_closure(schema, player.declared_descriptor(), true)?
                .into_iter()
                .filter(|concrete| accepted.contains(concrete))
                .collect::<Vec<_>>();
            if expected != player.concrete_descriptors() {
                return Err(claim_failure(
                    "query_plan_v2_role_player_claim",
                    "a hydration role-player closure contradicts resolved schema",
                ));
            }
            claimed_players.extend(expected);
        }
        if &claimed_players != accepted {
            return Err(claim_failure(
                "query_plan_v2_role_player_claim",
                "hydration role-player declarations do not cover the exact accepted concrete set",
            ));
        }
    }
    Ok(())
}

fn expected_role_references(
    schema: &ResolvedSchema,
    concrete: &ResolvedType,
    effective_id: &RoleId,
) -> Vec<RoleId> {
    let Some(concrete_relates) = concrete.relates().get(effective_id) else {
        return Vec::new();
    };
    let declared = concrete_relates.origin().declared();
    let mut references = std::iter::once(concrete.id())
        .chain(concrete.supertypes())
        .filter_map(|owner| {
            let resolved = schema.types().get(owner)?;
            let (_, relates) = resolved
                .relates()
                .iter()
                .find(|(role, _)| role.label() == effective_id.label())?;
            (relates.origin().declared() == declared)
                .then(|| RoleId::new(owner.label().as_str(), effective_id.label().as_str()).ok())
                .flatten()
        })
        .collect::<Vec<_>>();
    references.sort();
    references.dedup();
    references
}

fn validate_compatibility_references(
    predicate: Option<&QueryPatternV2>,
    hydration: &HydrationProjectionV2,
    targets: &BTreeMap<BindingId, (TypeId, bool)>,
    schema: &ResolvedSchema,
) -> Result<(), Diagnostic> {
    let Some(predicate) = predicate else {
        return Ok(());
    };
    validate_compatibility_pattern(predicate, hydration, targets, schema)
}

fn validate_compatibility_pattern(
    pattern: &QueryPatternV2,
    hydration: &HydrationProjectionV2,
    targets: &BTreeMap<BindingId, (TypeId, bool)>,
    schema: &ResolvedSchema,
) -> Result<(), Diagnostic> {
    match pattern {
        QueryPatternV2::FieldValue { field, .. } => {
            validate_field_reference(field, hydration, targets, schema)
        }
        QueryPatternV2::FieldComparison { left, right, .. } => {
            validate_field_reference(left, hydration, targets, schema)?;
            validate_field_reference(right, hydration, targets, schema)
        }
        QueryPatternV2::FieldPresence { field, .. } => {
            validate_field_reference(field, hydration, targets, schema)
        }
        QueryPatternV2::BindingIid { binding, .. } => {
            if targets.contains_key(binding) {
                Ok(())
            } else {
                Err(claim_failure(
                    "query_plan_v2_iid_binding_claim",
                    "a compatibility IID predicate lacks a native binding target",
                ))
            }
        }
        QueryPatternV2::RoleEdge {
            include_relation_subtypes,
            player,
            relation,
            relation_type,
            role,
        } => {
            let Some((declared, include_subtypes)) = targets.get(relation) else {
                return Err(claim_failure(
                    "query_plan_v2_role_edge_claim",
                    "a compatibility role edge lacks a native relation target",
                ));
            };
            if declared != relation_type || include_subtypes != include_relation_subtypes {
                return Err(claim_failure(
                    "query_plan_v2_role_edge_claim",
                    "a compatibility role edge contradicts its native relation target mode",
                ));
            }
            validate_role_reference(
                relation_type,
                role,
                *player,
                hydration,
                targets,
                schema,
                "query_plan_v2_role_edge_claim",
            )
        }
        QueryPatternV2::Reachable {
            relation,
            role_from,
            role_to,
            source,
            target,
            ..
        } => {
            let resolved = schema.types().get(relation).ok_or_else(|| {
                claim_failure(
                    "query_plan_v2_reachable_claim",
                    "a compatibility reachability relation is absent from resolved schema",
                )
            })?;
            if relation.kind() != TypeKind::Relation {
                return Err(claim_failure(
                    "query_plan_v2_reachable_claim",
                    "a compatibility reachability relation is not relation-kind",
                ));
            }
            if role_from.declaring_relation() != resolved.id().label()
                || role_to.declaring_relation() != resolved.id().label()
            {
                return Err(claim_failure(
                    "query_plan_v2_reachable_claim",
                    "a compatibility reachability role is not canonical to its exact relation",
                ));
            }
            validate_role_reference(
                relation,
                role_from,
                *source,
                hydration,
                targets,
                schema,
                "query_plan_v2_reachable_claim",
            )?;
            validate_role_reference(
                relation,
                role_to,
                *target,
                hydration,
                targets,
                schema,
                "query_plan_v2_reachable_claim",
            )
        }
        QueryPatternV2::And { patterns } | QueryPatternV2::Or { patterns } => {
            for child in patterns {
                validate_compatibility_pattern(child, hydration, targets, schema)?;
            }
            Ok(())
        }
        QueryPatternV2::Not { pattern } => {
            validate_compatibility_pattern(pattern, hydration, targets, schema)
        }
    }
}

fn validate_field_reference(
    field: &type_bridge_contract::query_plan::QueryFieldV2,
    hydration: &HydrationProjectionV2,
    targets: &BTreeMap<BindingId, (TypeId, bool)>,
    schema: &ResolvedSchema,
) -> Result<(), Diagnostic> {
    let (declared, _) = targets.get(&field.binding()).ok_or_else(|| {
        claim_failure(
            "query_plan_v2_field_reference_claim",
            "a compatibility field lacks a native binding target",
        )
    })?;
    let declared_type = schema.types().get(declared).ok_or_else(|| {
        claim_failure(
            "query_plan_v2_field_reference_claim",
            "a compatibility field binding target is absent from resolved schema",
        )
    })?;
    if field.descriptor() != declared
        && !declared_type
            .supertypes()
            .iter()
            .any(|supertype| supertype == field.descriptor())
    {
        return Err(claim_failure(
            "query_plan_v2_field_reference_claim",
            "a compatibility field owner is not its binding target or one of its supertypes",
        ));
    }
    let reference_owner = schema.types().get(field.descriptor()).ok_or_else(|| {
        claim_failure(
            "query_plan_v2_field_reference_claim",
            "a compatibility field owner is absent from resolved schema",
        )
    })?;
    let reference_owns = reference_owner
        .owns()
        .get(field.attribute())
        .ok_or_else(|| {
            claim_failure(
                "query_plan_v2_field_reference_claim",
                "a compatibility field attribute is absent from its declared owner",
            )
        })?;
    let binding_owns = declared_type.owns().get(field.attribute()).ok_or_else(|| {
        claim_failure(
            "query_plan_v2_field_reference_claim",
            "a compatibility field attribute is not effective on its binding target",
        )
    })?;
    let attribute = attribute_type(schema, field.attribute())?;
    if reference_owns.origin().declared() != binding_owns.origin().declared()
        || attribute.value_type().map(|value| value.value_type()) != Some(field.value_type())
    {
        return Err(claim_failure(
            "query_plan_v2_field_reference_claim",
            "a compatibility field ownership origin or scalar type contradicts resolved schema",
        ));
    }
    let binding = hydration
        .bindings()
        .iter()
        .find(|binding| binding.binding() == field.binding())
        .ok_or_else(|| {
            claim_failure(
                "query_plan_v2_field_reference_claim",
                "a compatibility field lacks binding hydration authority",
            )
        })?;
    if binding.concrete_descriptors().is_empty() {
        return Ok(());
    }
    let applicable = binding.concrete_descriptors().iter().any(|concrete| {
        hydration
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.descriptor() == concrete)
            .is_some_and(|descriptor| {
                descriptor.fields().iter().any(|projected| {
                    projected.reference_owners().contains(field.descriptor())
                        && projected.attribute() == field.attribute()
                        && projected.value_type() == field.value_type()
                })
            })
    });
    if applicable {
        Ok(())
    } else {
        Err(claim_failure(
            "query_plan_v2_field_reference_claim",
            "a compatibility field has no schema-proven applicable concrete owner",
        ))
    }
}

fn validate_role_reference(
    relation_type: &TypeId,
    role: &RoleId,
    player: BindingId,
    hydration: &HydrationProjectionV2,
    targets: &BTreeMap<BindingId, (TypeId, bool)>,
    schema: &ResolvedSchema,
    code: &'static str,
) -> Result<(), Diagnostic> {
    let relation = schema.types().get(relation_type).ok_or_else(|| {
        claim_failure(
            code,
            "a compatibility relation target is absent from resolved schema",
        )
    })?;
    let (effective_id, effective) = relation
        .relates()
        .iter()
        .find(|(candidate, _)| candidate.label() == role.label())
        .ok_or_else(|| {
            claim_failure(
                code,
                "a compatibility role is not effective on its relation target",
            )
        })?;
    let reference_relation = TypeId::new(
        TypeKind::Relation,
        role.declaring_relation().as_str().to_owned(),
    )
    .map_err(|_| claim_failure(code, "a compatibility role owner is malformed"))?;
    if reference_relation != *relation_type && !relation.supertypes().contains(&reference_relation)
    {
        return Err(claim_failure(
            code,
            "a compatibility role owner is not its relation target or one of its supertypes",
        ));
    }
    let reference = schema
        .types()
        .get(&reference_relation)
        .and_then(|resolved| {
            resolved
                .relates()
                .iter()
                .find(|(candidate, _)| candidate.label() == role.label())
                .map(|(_, relates)| relates)
        })
        .ok_or_else(|| claim_failure(code, "a compatibility role reference is absent"))?;
    if reference.origin().declared() != effective.origin().declared() {
        return Err(claim_failure(
            code,
            "a compatibility role reference resolves to a different effective role",
        ));
    }
    let (player_declared, _) = targets.get(&player).ok_or_else(|| {
        claim_failure(
            code,
            "a compatibility role player lacks a native binding target",
        )
    })?;
    let player_type = schema.types().get(player_declared).ok_or_else(|| {
        claim_failure(
            code,
            "a compatibility role player target is absent from resolved schema",
        )
    })?;
    let accepted = schema
        .roles()
        .get(effective_id)
        .ok_or_else(|| claim_failure(code, "a compatibility role lacks player authority"))?
        .accepted_players();
    let concrete_overlap = hydration
        .bindings()
        .iter()
        .find(|binding| binding.binding() == player)
        .is_some_and(|binding| {
            binding
                .concrete_descriptors()
                .iter()
                .any(|concrete| accepted.contains(concrete))
        });
    let declared_plays = player_type.plays().keys().any(|played| {
        played == effective_id
            || schema
                .roles()
                .get(effective_id)
                .is_some_and(|resolved| resolved.replacing_roles().contains(played))
    });
    if concrete_overlap || declared_plays {
        Ok(())
    } else {
        Err(claim_failure(
            code,
            "a compatibility role player target is not compatible with the resolved role",
        ))
    }
}

fn claim_failure(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::Integrity,
        DiagnosticCode::new(code).expect("static V2 schema-claim code is canonical"),
        message,
    )
}
