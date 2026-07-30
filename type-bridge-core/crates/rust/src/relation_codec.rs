//! Projection-authoritative relation model authority (private F2C-02A seam).
use crate::__codegen::{EncodedCreate, EncodedReference, HydratedPlayer, HydratedRow};
use crate::entity_codec::{
    canonical_owns_identity, canonical_type_identity, hydrate_projected_read_fields,
    hydrate_scalar, lower_scalar, projected_domain,
};
use crate::{
    Result,
    error::{Error, ModelValidationPhase},
};
use std::collections::BTreeMap;
use type_bridge_contract::id::RoleId;
use type_bridge_contract::value::Cardinality;
use type_bridge_contract::{
    codec::{from_canonical_json, to_canonical_json},
    id::{TypeId, TypeKind, is_canonical_thing_iid},
    projection::{CreateRoleProjection, ModelProjection, RoleTokenProjection},
};
use type_bridge_orm::{
    DynamicAttributeMap, DynamicRelationRow, DynamicRolePlayer, DynamicRolePlayerInput,
    InstalledRuntimeProjection, RelationDescriptor, RoleDescriptor,
};

pub(crate) fn resolve_relation_authority(
    identity_json: &str,
    installed: &InstalledRuntimeProjection,
    phase: ModelValidationPhase,
    require_constructible: bool,
) -> Result<(TypeId, RelationDescriptor)> {
    let id = from_canonical_json::<TypeId>(identity_json.as_bytes()).map_err(|e| {
        Error::model_validation(
            phase,
            "invalid_model_identity",
            vec!["type".into()],
            "generated model identity is not canonical",
            Some(Box::new(e)),
        )
    })?;
    let canonical = canonical_type_identity(&id, phase, vec!["type".into()])?;
    if canonical.as_slice() != identity_json.as_bytes() {
        return Err(Error::model_validation(
            phase,
            "invalid_model_identity",
            vec!["type".into()],
            "generated model identity is not byte-for-byte canonical",
            None,
        ));
    }
    if id.kind() != TypeKind::Relation {
        return Err(Error::model_validation(
            phase,
            "wrong_model_kind",
            vec!["type".into()],
            "the selected model is not a relation",
            None,
        ));
    }
    if installed.projection().target() != type_bridge_contract::projection::BindingTarget::Rust {
        return Err(Error::model_validation(
            phase,
            "model_not_projected",
            vec!["type".into()],
            "the installed projection is not a Rust projection",
            None,
        ));
    }
    let model = installed.projection().models().get(&id).ok_or_else(|| {
        Error::model_validation(
            phase,
            "model_not_projected",
            vec!["type".into()],
            "the selected relation is not projected",
            None,
        )
    })?;
    if require_constructible
        && (!model.declaration().is_constructible() || !model.create().enabled())
    {
        return Err(Error::model_validation(
            phase,
            "model_not_constructible",
            vec!["type".into()],
            "the selected relation is not constructible",
            None,
        ));
    }
    let descriptor = installed
        .relation_descriptor(&id)
        .map_err(|e| {
            Error::model_validation(
                phase,
                "model_not_projected",
                vec!["type".into()],
                "the selected relation descriptor is not installed",
                Some(Box::new(e)),
            )
        })?
        .clone();
    Ok((id, descriptor))
}

pub(crate) fn resolve_discovered_relation(
    type_name: &str,
    installed: &InstalledRuntimeProjection,
) -> Result<(TypeId, RelationDescriptor)> {
    let id = TypeId::new(TypeKind::Relation, type_name).map_err(|e| {
        Error::model_validation(
            ModelValidationPhase::Hydration,
            "invalid_discovered_type",
            vec!["type".into()],
            "discovered relation type label is invalid",
            Some(Box::new(e)),
        )
    })?;
    let rendered = to_canonical_json(&id).map_err(|e| {
        Error::model_validation(
            ModelValidationPhase::Hydration,
            "invalid_discovered_type",
            vec!["type".into()],
            "discovered relation identity cannot be rendered canonically",
            Some(Box::new(e)),
        )
    })?;
    let rendered = std::str::from_utf8(&rendered).map_err(|e| {
        Error::model_validation(
            ModelValidationPhase::Hydration,
            "invalid_discovered_type",
            vec!["type".into()],
            "discovered relation identity is not UTF-8",
            Some(Box::new(e)),
        )
    })?;
    resolve_relation_authority(rendered, installed, ModelValidationPhase::Hydration, true)
}

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PreparedRelationCreate {
    pub(crate) attributes: DynamicAttributeMap,
    pub(crate) role_players: Vec<DynamicRolePlayerInput>,
}

pub(crate) fn prepare_relation_create(
    encoded: &EncodedCreate,
    requested: &TypeId,
    installed: &InstalledRuntimeProjection,
) -> Result<PreparedRelationCreate> {
    let (resolved, descriptor) = resolve_relation_authority(
        encoded.type_id_json(),
        installed,
        ModelValidationPhase::Input,
        true,
    )?;
    if resolved != *requested {
        return Err(Error::model_validation(
            ModelValidationPhase::Input,
            "wrong_model_identity",
            vec!["type".into()],
            "generated and requested model identities differ",
            None,
        ));
    }
    let model = installed
        .projection()
        .models()
        .get(requested)
        .ok_or_else(|| {
            Error::model_validation(
                ModelValidationPhase::Input,
                "model_not_projected",
                vec!["type".into()],
                "selected relation is not projected",
                None,
            )
        })?;
    let attributes = crate::entity_codec::lower_projected_create_fields(
        encoded,
        model,
        &descriptor.owned_attributes,
        installed.projection(),
    )?;
    let mut evidence: BTreeMap<RoleId, (&[EncodedReference], usize)> = BTreeMap::new();
    for (index, (identity, refs)) in encoded.roles().iter().enumerate() {
        let role_id = from_canonical_json::<RoleId>(identity.as_bytes()).map_err(|e| {
            Error::model_validation(
                ModelValidationPhase::Input,
                "invalid_role_identity",
                vec![format!("roles[{index}]")],
                "role identity is not canonical",
                Some(Box::new(e)),
            )
        })?;
        let rendered = to_canonical_json(&role_id).map_err(|e| {
            Error::model_validation(
                ModelValidationPhase::Input,
                "invalid_role_identity",
                vec![format!("roles[{index}]")],
                "role identity cannot be rendered",
                Some(Box::new(e)),
            )
        })?;
        if rendered.as_slice() != identity.as_bytes() {
            return Err(Error::model_validation(
                ModelValidationPhase::Input,
                "invalid_role_identity",
                vec![format!("roles[{index}]")],
                "role identity is not byte-for-byte canonical",
                None,
            ));
        }
        if !model.query_tokens().roles().contains_key(&role_id) {
            return Err(Error::model_validation(
                ModelValidationPhase::Input,
                "unexpected_role_evidence",
                vec![format!("roles[{index}]")],
                "role is not a query role",
                None,
            ));
        }
        if !model.create().roles().contains_key(&role_id) {
            return Err(Error::model_validation(
                ModelValidationPhase::Input,
                "non_create_role_evidence",
                vec![format!("roles[{index}]")],
                "role is outside the create facet",
                None,
            ));
        }
        if evidence.insert(role_id, (refs.as_slice(), index)).is_some() {
            return Err(Error::model_validation(
                ModelValidationPhase::Input,
                "duplicate_role_evidence",
                vec![format!("roles[{index}]")],
                "role evidence is duplicated",
                None,
            ));
        }
    }
    let mut role_players = Vec::new();
    for (role_id, create_role) in model.create().roles() {
        let token = model.query_tokens().roles().get(role_id).ok_or_else(|| {
            Error::model_validation(
                ModelValidationPhase::Input,
                "invalid_installed_projection",
                Vec::new(),
                "create role has no query token",
                None,
            )
        })?;
        let label = token.role().label().as_str();
        let matches: Vec<_> = descriptor
            .roles
            .iter()
            .filter(|r| r.role_name == label)
            .collect();
        if matches.len() != 1 {
            return Err(Error::model_validation(
                ModelValidationPhase::Input,
                "invalid_installed_projection",
                Vec::new(),
                "role descriptor does not match projected role",
                None,
            ));
        }
        let refs = evidence.get(role_id).map(|(r, _)| *r).unwrap_or(&[]);
        enforce_role_cardinality(
            refs.len(),
            create_role.multiplicity().cardinality(),
            ModelValidationPhase::Input,
            token.target_name().as_str(),
        )?;
        for (player_index, reference) in refs.iter().enumerate() {
            role_players.push(lower_reference_player(
                reference,
                player_index,
                token,
                create_role,
                matches[0],
                installed,
            )?);
        }
    }
    Ok(PreparedRelationCreate {
        attributes,
        role_players,
    })
}

/// Prove one encoded role player against the installed projection and lower
/// it into its final common role-player input: canonical identity,
/// entity/relation kind, exact projected model and same-kind descriptor,
/// concrete constructibility, reference capability, exact query/create role
/// membership, an unambiguous descriptor/label cross-check, validation of
/// every carried projected key, and canonical-IID-preferred identity
/// selection with a typed exactly-one-key fallback. Provider type labels
/// come only from the exact decoded projected model, never from generated
/// input.
fn lower_reference_player(
    reference: &crate::__codegen::EncodedReference,
    player_index: usize,
    token: &RoleTokenProjection,
    create_role: &CreateRoleProjection,
    descriptor_role: &RoleDescriptor,
    installed: &InstalledRuntimeProjection,
) -> Result<DynamicRolePlayerInput> {
    let segment = format!("{}[{player_index}]", token.target_name().as_str());
    let path = || vec![segment.clone(), "type".to_owned()];
    let id = from_canonical_json::<TypeId>(reference.type_id_json().as_bytes()).map_err(|e| {
        Error::model_validation(
            ModelValidationPhase::Input,
            "invalid_player_identity",
            path(),
            "relation player identity is not canonical",
            Some(Box::new(e)),
        )
    })?;
    let rendered = to_canonical_json(&id).map_err(|e| {
        Error::model_validation(
            ModelValidationPhase::Input,
            "invalid_player_identity",
            path(),
            "relation player identity cannot be rendered canonically",
            Some(Box::new(e)),
        )
    })?;
    if rendered.as_slice() != reference.type_id_json().as_bytes() {
        return Err(Error::model_validation(
            ModelValidationPhase::Input,
            "invalid_player_identity",
            path(),
            "relation player identity is not byte-for-byte canonical",
            None,
        ));
    }
    let player_model = require_projected_player(
        &id,
        token,
        descriptor_role,
        installed,
        ModelValidationPhase::Input,
        &segment,
    )?;
    if !create_role
        .players()
        .iter()
        .any(|player| player.id() == &id)
    {
        return Err(Error::model_validation(
            ModelValidationPhase::Input,
            "player_not_allowed",
            path(),
            "the selected player is outside the role",
            None,
        ));
    }
    let mut validated_keys = Vec::new();
    'carried: for (key_index, (key_identity, scalar)) in reference.keys().iter().enumerate() {
        for key_id in player_model.reference_read().key_fields() {
            let key_token = player_model
                .query_tokens()
                .fields()
                .get(key_id)
                .ok_or_else(|| {
                    Error::model_validation(
                        ModelValidationPhase::Input,
                        "invalid_installed_projection",
                        Vec::new(),
                        "projected reference key has no query token",
                        None,
                    )
                })?;
            let canonical = canonical_owns_identity(
                key_token.declaring_id(),
                ModelValidationPhase::Input,
                vec![segment.clone(), format!("keys[{key_index}]")],
            )?;
            if canonical.as_slice() != key_identity.as_bytes() {
                continue;
            }
            let read = player_model
                .complete_read()
                .fields()
                .iter()
                .find(|field| field.token() == key_id)
                .ok_or_else(|| {
                    Error::model_validation(
                        ModelValidationPhase::Input,
                        "invalid_installed_projection",
                        Vec::new(),
                        "projected reference key has no complete-read field",
                        None,
                    )
                })?;
            let domain = projected_domain(
                installed.projection(),
                read.value(),
                ModelValidationPhase::Input,
            )?;
            let value = lower_scalar(
                scalar,
                domain,
                ModelValidationPhase::Input,
                vec![segment.clone(), key_token.target_name().as_str().to_owned()],
            )?;
            validated_keys.push((
                key_token.id().attribute().label().as_str().to_owned(),
                value,
            ));
            continue 'carried;
        }
        return Err(Error::model_validation(
            ModelValidationPhase::Input,
            "unexpected_reference_key",
            vec![segment.clone(), format!("keys[{key_index}]")],
            "player reference key is outside the projected reference keys",
            None,
        ));
    }
    let iid = match reference.iid() {
        Some(value) if !is_canonical_thing_iid(value) => {
            return Err(Error::model_validation(
                ModelValidationPhase::Input,
                "noncanonical_player_iid",
                vec![segment.clone(), "iid".to_owned()],
                "player reference IID is not canonical",
                None,
            ));
        }
        other => other.map(str::to_owned),
    };
    let key = match (&iid, validated_keys.len()) {
        (Some(_), _) => None,
        (None, 1) => validated_keys.pop(),
        (None, 0) => {
            return Err(Error::model_validation(
                ModelValidationPhase::Input,
                "missing_reference_identity",
                vec![segment.clone()],
                "player reference carries no identity evidence",
                None,
            ));
        }
        (None, _) => {
            return Err(Error::model_validation(
                ModelValidationPhase::Input,
                "multiple_reference_keys_without_iid",
                vec![segment.clone()],
                "player reference carries multiple keys without an IID",
                None,
            ));
        }
    };
    Ok(DynamicRolePlayerInput {
        role_name: descriptor_role.role_name.clone(),
        player_type_name: player_model.id().label().as_str().to_owned(),
        iid,
        key,
    })
}

/// Prove one already-identified role player against the installed projection:
/// entity/relation kind, exact projected model and same-kind descriptor,
/// concrete constructibility, reference capability, exact role-token player
/// membership, and an unambiguous descriptor/label cross-check.
fn require_projected_player<'a>(
    id: &TypeId,
    token: &RoleTokenProjection,
    descriptor_role: &RoleDescriptor,
    installed: &'a InstalledRuntimeProjection,
    phase: ModelValidationPhase,
    segment: &str,
) -> Result<&'a ModelProjection> {
    let path = || vec![segment.to_owned(), "type".to_owned()];
    if !matches!(id.kind(), TypeKind::Entity | TypeKind::Relation) {
        return Err(Error::model_validation(
            phase,
            "wrong_player_kind",
            path(),
            "the selected player is not an entity or relation",
            None,
        ));
    }
    let player_model = installed.projection().models().get(id).ok_or_else(|| {
        Error::model_validation(
            phase,
            "player_not_projected",
            path(),
            "the selected player is not projected",
            None,
        )
    })?;
    let descriptor_installed = match id.kind() {
        TypeKind::Entity => installed.entity_descriptor(id).err(),
        TypeKind::Relation => installed.relation_descriptor(id).err(),
        TypeKind::Attribute | TypeKind::Struct => {
            return Err(Error::model_validation(
                phase,
                "wrong_player_kind",
                path(),
                "the selected player is not an entity or relation",
                None,
            ));
        }
    };
    if let Some(source) = descriptor_installed {
        return Err(Error::model_validation(
            phase,
            "player_not_projected",
            path(),
            "the selected player descriptor is not installed",
            Some(Box::new(source)),
        ));
    }
    if player_model.declaration().is_abstract() || !player_model.declaration().is_constructible() {
        return Err(Error::model_validation(
            phase,
            "player_not_constructible",
            path(),
            "the selected player is not constructible",
            None,
        ));
    }
    if player_model.reference_read().target_name().is_none() {
        return Err(Error::model_validation(
            phase,
            "player_reference_not_projected",
            path(),
            "the selected player has no projected reference target",
            None,
        ));
    }
    if !token.accepted_players().contains(id) {
        return Err(Error::model_validation(
            phase,
            "player_not_allowed",
            path(),
            "the selected player is outside the role",
            None,
        ));
    }
    let provider_label = player_model.id().label();
    let descriptor_players = descriptor_role
        .player_type_names
        .iter()
        .filter(|name| name.as_str() == provider_label.as_str())
        .count();
    let mut labeled = token
        .accepted_players()
        .iter()
        .filter(|player| player.label() == provider_label);
    let first = labeled.next();
    if descriptor_players != 1 || labeled.next().is_some() || first != Some(id) {
        return Err(Error::model_validation(
            phase,
            "invalid_installed_projection",
            path(),
            "projected player authority is ambiguous",
            None,
        ));
    }
    Ok(player_model)
}

fn enforce_role_cardinality(
    count: usize,
    cardinality: Cardinality,
    phase: ModelValidationPhase,
    target: &str,
) -> Result<()> {
    let count = u64::try_from(count).map_err(|source| {
        Error::model_validation(
            phase,
            "cardinality_overflow",
            vec![target.to_owned()],
            "relation role evidence cardinality overflow",
            Some(Box::new(source)),
        )
    })?;
    if count == 0 && cardinality.min() > 0 {
        return Err(Error::model_validation(
            phase,
            "missing_role_evidence",
            vec![target.to_owned()],
            "required relation role evidence is missing",
            None,
        ));
    }
    if cardinality.max() == Some(1) && count > 1 {
        return Err(Error::model_validation(
            phase,
            "duplicate_role_evidence",
            vec![target.to_owned()],
            "relation role cardinality violation",
            None,
        ));
    }
    if count < cardinality.min() || cardinality.max().is_some_and(|max| count > max) {
        return Err(Error::model_validation(
            phase,
            "role_cardinality_violation",
            vec![target.to_owned()],
            "relation role cardinality violation",
            None,
        ));
    }
    Ok(())
}

/// Convert exactly one common coalesced relation row into one neutral
/// hydrated row: exact requested concrete relation identity and canonical
/// IID, shared projected ownership/scalar field hydration, unique provider
/// active-role mapping in projected complete-read order with client-side
/// cardinality enforcement, and per-player identity/attribute/key evidence.
pub(crate) fn hydrate_relation(
    row: DynamicRelationRow,
    requested_id: &TypeId,
    installed: &InstalledRuntimeProjection,
) -> Result<HydratedRow> {
    let identity_json =
        canonical_type_identity(requested_id, ModelValidationPhase::Hydration, Vec::new())?;
    let identity_str = std::str::from_utf8(&identity_json).expect("canonical JSON is UTF-8");
    let (_, descriptor) = resolve_relation_authority(
        identity_str,
        installed,
        ModelValidationPhase::Hydration,
        false,
    )?;
    let model = installed
        .projection()
        .models()
        .get(requested_id)
        .ok_or_else(|| {
            Error::model_validation(
                ModelValidationPhase::Hydration,
                "model_not_projected",
                vec!["type".into()],
                "the selected relation is absent from the installed Rust projection",
                None,
            )
        })?;
    if !model.declaration().is_constructible() {
        return Err(Error::model_validation(
            ModelValidationPhase::Hydration,
            "model_not_constructible",
            Vec::new(),
            "the requested relation cannot be a concrete hydration result",
            None,
        ));
    }

    let iid = row.iid.ok_or_else(|| {
        Error::model_validation(
            ModelValidationPhase::Hydration,
            "missing_iid",
            vec!["iid".into()],
            "provider relation row omitted its IID",
            None,
        )
    })?;
    if !is_canonical_thing_iid(&iid) {
        return Err(Error::model_validation(
            ModelValidationPhase::Hydration,
            "noncanonical_iid",
            vec!["iid".into()],
            "provider relation row contains a noncanonical IID",
            None,
        ));
    }
    let type_name = row.type_name.ok_or_else(|| {
        Error::model_validation(
            ModelValidationPhase::Hydration,
            "missing_concrete_type",
            vec!["type".into()],
            "provider relation row omitted its concrete type",
            None,
        )
    })?;
    if type_name != descriptor.type_name || type_name != requested_id.label().as_str() {
        return Err(Error::model_validation(
            ModelValidationPhase::Hydration,
            "wrong_concrete_type",
            vec!["type".into()],
            "provider relation row has the wrong exact concrete type",
            None,
        ));
    }

    let fields = hydrate_projected_read_fields(&row.attributes, model, installed.projection())?;

    let read_roles = model.complete_read().roles();
    let mut by_label = BTreeMap::<&str, &RoleId>::new();
    for role_id in read_roles.keys() {
        if by_label.insert(role_id.label().as_str(), role_id).is_some() {
            return Err(Error::model_validation(
                ModelValidationPhase::Hydration,
                "invalid_installed_projection",
                Vec::new(),
                "projected read roles have ambiguous labels",
                None,
            ));
        }
    }
    let mut evidence = BTreeMap::<&RoleId, Vec<&DynamicRolePlayer>>::new();
    for (index, player) in row.role_players.iter().enumerate() {
        let Some(role_id) = by_label.get(player.role_name.as_str()) else {
            return Err(Error::model_validation(
                ModelValidationPhase::Hydration,
                "unexpected_provider_role",
                vec![format!("role_players[{index}]")],
                "provider row contains an unprojected active role",
                None,
            ));
        };
        evidence.entry(role_id).or_default().push(player);
    }

    let mut roles = Vec::new();
    for (role_id, read_role) in read_roles {
        let token = model.query_tokens().roles().get(role_id).ok_or_else(|| {
            Error::model_validation(
                ModelValidationPhase::Hydration,
                "invalid_installed_projection",
                Vec::new(),
                "read role has no query token",
                None,
            )
        })?;
        let label = token.role().label().as_str();
        let matches: Vec<_> = descriptor
            .roles
            .iter()
            .filter(|role| role.role_name == label)
            .collect();
        if matches.len() != 1 {
            return Err(Error::model_validation(
                ModelValidationPhase::Hydration,
                "invalid_installed_projection",
                Vec::new(),
                "role descriptor does not match projected role",
                None,
            ));
        }
        let players = evidence.get(role_id).map(Vec::as_slice).unwrap_or(&[]);
        enforce_role_cardinality(
            players.len(),
            read_role.multiplicity().cardinality(),
            ModelValidationPhase::Hydration,
            token.target_name().as_str(),
        )?;
        let mut hydrated = Vec::with_capacity(players.len());
        for (player_index, player) in players.iter().enumerate() {
            hydrated.push(hydrate_role_player(
                player,
                player_index,
                token,
                matches[0],
                installed,
            )?);
        }
        if !hydrated.is_empty() {
            let rendered = to_canonical_json(role_id).map_err(|source| {
                Error::model_validation(
                    ModelValidationPhase::Hydration,
                    "invalid_role_identity",
                    vec![token.target_name().as_str().to_owned()],
                    "projected role identity cannot be rendered canonically",
                    Some(Box::new(source)),
                )
            })?;
            roles.push((
                String::from_utf8(rendered).expect("canonical JSON is UTF-8"),
                hydrated,
            ));
        }
    }
    Ok(HydratedRow::from_owned(
        identity_str.to_owned(),
        iid,
        fields,
        roles,
    ))
}

/// Prove one provider role player and convert it into neutral reference
/// evidence: mandatory canonical player IID, exact accepted-player kind/type
/// resolution from the read role's projected player set (never a guessed
/// entity default), projected player authority, descriptor-decoded provider
/// attributes, and retained projected reference-key evidence with exact
/// domain hydration. Duplicate key evidence is rejected; absent key evidence
/// is allowed because the canonical IID is always present.
fn hydrate_role_player(
    player: &DynamicRolePlayer,
    player_index: usize,
    token: &RoleTokenProjection,
    descriptor_role: &RoleDescriptor,
    installed: &InstalledRuntimeProjection,
) -> Result<HydratedPlayer> {
    let phase = ModelValidationPhase::Hydration;
    let segment = format!("{}[{player_index}]", token.target_name().as_str());
    let iid = player.player_iid.as_deref().ok_or_else(|| {
        Error::model_validation(
            phase,
            "missing_player_iid",
            vec![segment.clone(), "iid".to_owned()],
            "provider role player omitted its IID",
            None,
        )
    })?;
    if !is_canonical_thing_iid(iid) {
        return Err(Error::model_validation(
            phase,
            "noncanonical_player_iid",
            vec![segment.clone(), "iid".to_owned()],
            "provider role player contains a noncanonical IID",
            None,
        ));
    }
    let type_name = player.player_type_name.as_deref().ok_or_else(|| {
        Error::model_validation(
            phase,
            "missing_player_type",
            vec![segment.clone(), "type".to_owned()],
            "provider role player omitted its concrete type",
            None,
        )
    })?;
    let mut candidates = token
        .accepted_players()
        .iter()
        .filter(|id| id.label().as_str() == type_name);
    let Some(id) = candidates.next() else {
        return Err(Error::model_validation(
            phase,
            "player_not_allowed",
            vec![segment.clone(), "type".to_owned()],
            "provider role player type is outside the role",
            None,
        ));
    };
    if candidates.next().is_some() {
        return Err(Error::model_validation(
            phase,
            "invalid_installed_projection",
            vec![segment.clone(), "type".to_owned()],
            "projected player authority is ambiguous",
            None,
        ));
    }
    let player_model =
        require_projected_player(id, token, descriptor_role, installed, phase, &segment)?;
    let attributes = installed
        .role_player_attributes(id, &player.attributes)
        .map_err(|source| {
            Error::model_validation(
                phase,
                "invalid_player_attributes",
                vec![segment.clone(), "attributes".to_owned()],
                "provider role player attributes are outside the projected descriptor",
                Some(Box::new(source)),
            )
        })?;
    let mut keys = Vec::new();
    for key_id in player_model.reference_read().key_fields() {
        let key_token = player_model
            .query_tokens()
            .fields()
            .get(key_id)
            .ok_or_else(|| {
                Error::model_validation(
                    phase,
                    "invalid_installed_projection",
                    Vec::new(),
                    "projected reference key has no query token",
                    None,
                )
            })?;
        let attribute_label = key_token.id().attribute().label().as_str();
        let key_path = || vec![segment.clone(), key_token.target_name().as_str().to_owned()];
        let mut values = attributes
            .iter()
            .filter(|(name, _)| name == attribute_label);
        let Some((_, value)) = values.next() else {
            continue;
        };
        if values.next().is_some() {
            return Err(Error::model_validation(
                phase,
                "duplicate_reference_key",
                key_path(),
                "provider role player repeats one projected reference key",
                None,
            ));
        }
        let read = player_model
            .complete_read()
            .fields()
            .iter()
            .find(|field| field.token() == key_id)
            .ok_or_else(|| {
                Error::model_validation(
                    phase,
                    "invalid_installed_projection",
                    Vec::new(),
                    "projected reference key has no complete-read field",
                    None,
                )
            })?;
        let domain = projected_domain(installed.projection(), read.value(), phase)?;
        let scalar = hydrate_scalar(value, domain, key_path())?;
        let identity = canonical_owns_identity(key_token.declaring_id(), phase, key_path())?;
        keys.push((
            String::from_utf8(identity).expect("canonical JSON is UTF-8"),
            scalar,
        ));
    }
    let identity = canonical_type_identity(id, phase, vec![segment.clone(), "type".to_owned()])?;
    Ok(HydratedPlayer::from_owned(
        String::from_utf8(identity).expect("canonical JSON is UTF-8"),
        Some(iid.to_owned()),
        keys,
    ))
}

pub(crate) fn lower_relation_create<T: crate::__codegen::IntoEncodedCreate>(
    input: T,
    requested: &TypeId,
    installed: &InstalledRuntimeProjection,
) -> Result<PreparedRelationCreate> {
    let encoded = input.into_encoded_create().map_err(|error| {
        crate::entity_codec::map_validation_error(error, ModelValidationPhase::Input)
    })?;
    prepare_relation_create(&encoded, requested, installed)
}
