//! Typed adapters between generated Rust values and binding-neutral projected DTOs.

use type_bridge_contract::codec::{from_canonical_json, to_canonical_json};
use type_bridge_contract::id::{RoleId, TypeId, TypeKind};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::value::CanonicalValue;
use type_bridge_orm::{
    AttributeValue, InstalledRuntimeProjection, ProjectedAttributeValue, ProjectedCreate,
    ProjectedReference, ProjectedRolePlayer, ProjectedThing,
};

use crate::__codegen::{
    CanonicalDouble, CompleteModel, Date, DateTime, DateTimeTz, Decimal, Duration, EncodedCreate,
    EncodedReference, EncodedScalar, HydratedPlayer, HydratedRow, HydrationCapability,
    IntoEncodedCreate, ReferenceOrigin, ValidationPath,
};
use crate::Result;
use crate::entity_codec::map_validation_error;
use crate::error::{Error, ModelValidationPhase};

/// Lower one generated create value into the common projected DTO without a
/// serialized model or dynamic field-name representation.
pub(crate) fn project_create<T: IntoEncodedCreate>(
    input: T,
    expected_type: &TypeId,
    installed: &InstalledRuntimeProjection,
) -> Result<ProjectedCreate> {
    let encoded = input
        .into_encoded_create()
        .map_err(|error| map_validation_error(error, ModelValidationPhase::Input))?;
    project_encoded_create(&encoded, expected_type, installed)
}

pub(crate) fn validate_encoded_create(
    encoded: &EncodedCreate,
    installed: &InstalledRuntimeProjection,
) -> Result<()> {
    let expected_type = decode_type_identity(
        encoded.type_id_json(),
        ModelValidationPhase::Input,
        "invalid_model_identity",
        vec!["type".into()],
        "generated model identity is not canonical",
    )?;
    project_encoded_create(encoded, &expected_type, installed).map(|_| ())
}

pub(crate) fn validate_hydrated_row(
    row: &HydratedRow,
    installed: &InstalledRuntimeProjection,
) -> Result<()> {
    project_hydrated_row(row, installed).map(|_| ())
}

pub(crate) fn project_encoded_create(
    encoded: &EncodedCreate,
    expected_type: &TypeId,
    installed: &InstalledRuntimeProjection,
) -> Result<ProjectedCreate> {
    let encoded_type = decode_type_identity(
        encoded.type_id_json(),
        ModelValidationPhase::Input,
        "invalid_model_identity",
        vec!["type".into()],
        "generated model identity is not canonical",
    )?;
    if &encoded_type != expected_type {
        return Err(model_error(
            ModelValidationPhase::Input,
            "wrong_model_identity",
            vec!["type".into()],
            "generated and requested model identities differ",
        ));
    }

    let mut fields = Vec::with_capacity(encoded.fields().len());
    for (field_index, (identity, values)) in encoded.fields().iter().enumerate() {
        let field_path = vec![format!("fields[{field_index}]")];
        let field_id = resolve_owns_identity(
            installed,
            expected_type,
            identity,
            ModelValidationPhase::Input,
            "unexpected_field_evidence",
            field_path.clone(),
            "generated ownership identity is not one projected field of the selected model",
        )?;
        let attribute_type =
            TypeId::new(TypeKind::Attribute, field_id.attribute().label().as_str()).map_err(
                |source| {
                    Error::model_validation(
                        ModelValidationPhase::Input,
                        "invalid_field_identity",
                        field_path.clone(),
                        "generated ownership identity has an invalid attribute type",
                        Some(Box::new(source)),
                    )
                },
            )?;
        let mut projected_values = Vec::with_capacity(values.len());
        for (value_index, value) in values.iter().enumerate() {
            let path = ValidationPath::root()
                .join(format!("fields[{field_index}]"))
                .join_index(value_index);
            let canonical = value
                .to_canonical_value(&path)
                .map_err(|error| map_validation_error(error, ModelValidationPhase::Input))?;
            projected_values.push(
                ProjectedAttributeValue::try_new(installed, attribute_type.clone(), canonical)
                    .map_err(|error| {
                        Error::from_sdk_execution(error, ModelValidationPhase::Input)
                    })?,
            );
        }
        fields.push((field_id, projected_values));
    }

    let mut roles = Vec::with_capacity(encoded.roles().len());
    for (role_index, (identity, references)) in encoded.roles().iter().enumerate() {
        let role_id = decode_role_identity(
            identity,
            ModelValidationPhase::Input,
            "invalid_role_identity",
            vec![format!("roles[{role_index}]")],
            "generated role identity is not canonical",
        )?;
        let role_segment = installed
            .projection()
            .models()
            .get(expected_type)
            .and_then(|model| model.query_tokens().roles().get(&role_id))
            .map_or_else(
                || role_id.label().as_str().to_owned(),
                |token| token.target_name().as_str().to_owned(),
            );
        let mut projected_references = Vec::with_capacity(references.len());
        for (reference_index, reference) in references.iter().enumerate() {
            projected_references.push(project_reference(
                reference,
                &role_segment,
                reference_index,
                installed,
            )?);
        }
        roles.push((role_id, projected_references));
    }

    ProjectedCreate::try_new(installed, expected_type.clone(), fields, roles)
        .map_err(|error| Error::from_sdk_execution(error, ModelValidationPhase::Input))
}

fn project_reference(
    reference: &EncodedReference,
    role_segment: &str,
    reference_index: usize,
    installed: &InstalledRuntimeProjection,
) -> Result<ProjectedReference> {
    let base = format!("{role_segment}[{reference_index}]");
    let type_id = decode_type_identity(
        reference.type_id_json(),
        ModelValidationPhase::Input,
        "invalid_player_identity",
        vec![base.clone(), "type".into()],
        "generated role-player identity is not canonical",
    )?;
    let mut keys = Vec::with_capacity(reference.keys().len());
    for (key_index, (identity, scalar)) in reference.keys().iter().enumerate() {
        let key_path = vec![base.clone(), format!("keys[{key_index}]")];
        let field_id = resolve_owns_identity(
            installed,
            &type_id,
            identity,
            ModelValidationPhase::Input,
            "unexpected_reference_key",
            key_path.clone(),
            "generated reference-key identity is not one projected field of the selected model",
        )?;
        let attribute_type =
            TypeId::new(TypeKind::Attribute, field_id.attribute().label().as_str()).map_err(
                |source| {
                    Error::model_validation(
                        ModelValidationPhase::Input,
                        "invalid_reference_key_identity",
                        key_path.clone(),
                        "generated reference key has an invalid attribute type",
                        Some(Box::new(source)),
                    )
                },
            )?;
        let path = ValidationPath::root()
            .join(base.clone())
            .join(format!("keys[{key_index}]"));
        let canonical = scalar
            .to_canonical_value(&path)
            .map_err(|error| map_validation_error(error, ModelValidationPhase::Input))?;
        let value = ProjectedAttributeValue::try_new(installed, attribute_type, canonical)
            .map_err(|error| Error::from_sdk_execution(error, ModelValidationPhase::Input))?;
        keys.push((field_id, value));
    }
    ProjectedReference::try_new_with_origin_carrier(
        installed,
        type_id,
        reference.iid().map(str::to_owned),
        keys,
        reference.origin().projected(),
    )
    .map_err(|error| {
        if error.code().as_str() == "noncanonical_iid" {
            model_error(
                ModelValidationPhase::Input,
                "noncanonical_player_iid",
                vec![base, "iid".into()],
                "relation reference IID must be canonical",
            )
        } else {
            Error::from_sdk_execution(error, ModelValidationPhase::Input)
        }
    })
}

fn project_hydrated_row(
    row: &HydratedRow,
    installed: &InstalledRuntimeProjection,
) -> Result<ProjectedThing> {
    let type_id = decode_type_identity(
        row.type_id_json(),
        ModelValidationPhase::Hydration,
        "invalid_model_identity",
        vec!["type".into()],
        "hydrated model identity is not canonical",
    )?;

    let mut fields = Vec::with_capacity(row.fields().len());
    for (field_index, (identity, values)) in row.fields().iter().enumerate() {
        let field_path = vec![format!("fields[{field_index}]")];
        let field_id = resolve_owns_identity(
            installed,
            &type_id,
            identity,
            ModelValidationPhase::Hydration,
            "unexpected_field_evidence",
            field_path.clone(),
            "hydrated ownership identity is not one projected field of the selected model",
        )?;
        let attribute_type =
            TypeId::new(TypeKind::Attribute, field_id.attribute().label().as_str()).map_err(
                |source| {
                    Error::model_validation(
                        ModelValidationPhase::Hydration,
                        "invalid_field_identity",
                        field_path.clone(),
                        "hydrated ownership identity has an invalid attribute type",
                        Some(Box::new(source)),
                    )
                },
            )?;
        let mut projected_values = Vec::with_capacity(values.len());
        for value in values {
            projected_values.push(project_hydrated_attribute_value(
                installed,
                attribute_type.clone(),
                value,
            )?);
        }
        fields.push((field_id, projected_values));
    }

    let mut roles = Vec::with_capacity(row.roles().len());
    for (role_index, (identity, players)) in row.roles().iter().enumerate() {
        let role_id = decode_role_identity(
            identity,
            ModelValidationPhase::Hydration,
            "invalid_role_identity",
            vec![format!("roles[{role_index}]")],
            "hydrated role identity is not canonical",
        )?;
        let role_segment = installed
            .projection()
            .models()
            .get(&type_id)
            .and_then(|model| model.query_tokens().roles().get(&role_id))
            .map_or_else(
                || role_id.label().as_str().to_owned(),
                |token| token.target_name().as_str().to_owned(),
            );
        let projected_players = players
            .iter()
            .enumerate()
            .map(|(player_index, player)| {
                project_hydrated_player(player, &role_segment, player_index, installed)
            })
            .collect::<Result<Vec<_>>>()?;
        roles.push((role_id, projected_players));
    }

    ProjectedThing::try_new_with_origin_carrier(
        installed,
        type_id,
        row.iid().to_owned(),
        fields,
        roles,
        row.origin().projected(),
    )
    .map_err(|error| Error::from_sdk_execution(error, ModelValidationPhase::Hydration))
}

fn project_hydrated_player(
    player: &HydratedPlayer,
    role_segment: &str,
    player_index: usize,
    installed: &InstalledRuntimeProjection,
) -> Result<ProjectedRolePlayer> {
    let base = format!("{role_segment}[{player_index}]");
    let type_id = decode_type_identity(
        player.type_id_json(),
        ModelValidationPhase::Hydration,
        "invalid_player_identity",
        vec![base.clone(), "type".into()],
        "hydrated role-player identity is not canonical",
    )?;
    let mut keys = Vec::with_capacity(player.keys().len());
    for (key_index, (identity, scalar)) in player.keys().iter().enumerate() {
        let key_path = vec![base.clone(), format!("keys[{key_index}]")];
        let field_id = resolve_owns_identity(
            installed,
            &type_id,
            identity,
            ModelValidationPhase::Hydration,
            "unexpected_reference_key",
            key_path.clone(),
            "hydrated reference-key identity is not one projected field of the selected model",
        )?;
        let attribute_type =
            TypeId::new(TypeKind::Attribute, field_id.attribute().label().as_str()).map_err(
                |source| {
                    Error::model_validation(
                        ModelValidationPhase::Hydration,
                        "invalid_reference_key_identity",
                        key_path.clone(),
                        "hydrated reference key has an invalid attribute type",
                        Some(Box::new(source)),
                    )
                },
            )?;
        let value = project_hydrated_attribute_value(installed, attribute_type, scalar)?;
        keys.push((field_id, value));
    }
    let reference = ProjectedReference::try_new_for_hydration_with_origin_carrier(
        installed,
        type_id,
        player.iid().map(str::to_owned),
        keys,
        player.origin().projected(),
    )
    .map_err(|error| Error::from_sdk_execution(error, ModelValidationPhase::Hydration))?;
    ProjectedRolePlayer::try_new(installed, reference)
        .map_err(|error| Error::from_sdk_execution(error, ModelValidationPhase::Hydration))
}

fn project_hydrated_attribute_value(
    installed: &InstalledRuntimeProjection,
    attribute_type: TypeId,
    value: &EncodedScalar,
) -> Result<ProjectedAttributeValue> {
    let projected = match value {
        EncodedScalar::DateTimeTz(value) => {
            ProjectedAttributeValue::try_from_hydrated_canonical_value(
                installed,
                attribute_type,
                CanonicalValue::DateTimeTz(value.canonical().clone()),
            )
        }
        value => ProjectedAttributeValue::try_from_hydrated_attribute_value(
            installed,
            attribute_type,
            &hydrated_attribute_value(value),
        ),
    };
    projected.map_err(|error| Error::from_sdk_execution(error, ModelValidationPhase::Hydration))
}

fn hydrated_attribute_value(value: &EncodedScalar) -> AttributeValue {
    match value {
        EncodedScalar::String(value) => AttributeValue::String(value.clone()),
        EncodedScalar::Long(value) => AttributeValue::Long(*value),
        EncodedScalar::Double(value) => AttributeValue::Double(value.get()),
        EncodedScalar::Decimal(value) => AttributeValue::Decimal(value.as_str().to_owned()),
        EncodedScalar::Boolean(value) => AttributeValue::Boolean(*value),
        EncodedScalar::Date(value) => AttributeValue::Date(value.as_str().to_owned()),
        EncodedScalar::DateTime(value) => AttributeValue::DateTime(value.as_str().to_owned()),
        EncodedScalar::DateTimeTz(_) => {
            unreachable!("datetime-tz hydration retains its exact canonical evidence")
        }
        EncodedScalar::Duration(value) => AttributeValue::Duration(value.as_str().to_owned()),
    }
}

/// Convert one engine-validated projected thing into the closed generated
/// hydration representation, then materialize the generated nominal model.
pub(crate) fn materialize_projected<M: CompleteModel>(
    thing: ProjectedThing,
    installed: &InstalledRuntimeProjection,
) -> Result<M> {
    let row = projected_to_hydrated_row(&thing, installed)?;
    M::materialize(&row, &HydrationCapability::new())
        .map_err(|error| map_validation_error(error, ModelValidationPhase::Hydration))
}

pub(crate) fn projected_to_hydrated_row(
    thing: &ProjectedThing,
    installed: &InstalledRuntimeProjection,
) -> Result<HydratedRow> {
    let type_identity = encode_type_identity(thing.type_id(), vec!["type".into()])?;
    let mut fields = Vec::with_capacity(thing.fields().len());
    for (field_id, values) in thing.fields() {
        let identity = encode_owns_identity(
            declaring_owns_identity(installed, thing.type_id(), field_id, vec!["fields".into()])?,
            vec!["fields".into()],
        )?;
        let values = values
            .iter()
            .map(|value| encoded_scalar(value.value()))
            .collect::<Result<Vec<_>>>()?;
        fields.push((identity, values));
    }

    let mut roles = Vec::with_capacity(thing.roles().len());
    for (role_id, players) in thing.roles() {
        let identity = encode_role_identity(role_id, vec!["roles".into()])?;
        let mut hydrated_players = Vec::with_capacity(players.len());
        for player in players {
            let player_type = encode_type_identity(player.type_id(), vec!["roles".into()])?;
            let mut keys = Vec::with_capacity(player.keys().len());
            for (field_id, value) in player.keys() {
                keys.push((
                    encode_owns_identity(
                        declaring_owns_identity(
                            installed,
                            player.type_id(),
                            field_id,
                            vec!["roles".into(), "keys".into()],
                        )?,
                        vec!["roles".into(), "keys".into()],
                    )?,
                    encoded_scalar(value.value())?,
                ));
            }
            hydrated_players.push(HydratedPlayer::from_owned_with_origin(
                player_type,
                Some(player.iid().to_owned()),
                keys,
                ReferenceOrigin::from_projected(player.reference().origin_carrier()),
            ));
        }
        roles.push((identity, hydrated_players));
    }

    Ok(HydratedRow::from_owned_with_origin(
        type_identity,
        thing.iid().to_owned(),
        fields,
        roles,
        ReferenceOrigin::from_projected(thing.origin_carrier()),
    ))
}

fn encoded_scalar(value: &CanonicalValue) -> Result<EncodedScalar> {
    let mapped = match value {
        CanonicalValue::String(value) => EncodedScalar::String(value.as_str().to_owned()),
        CanonicalValue::Long(value) => EncodedScalar::Long(*value),
        CanonicalValue::Double(value) => EncodedScalar::Double(
            CanonicalDouble::try_from_bits(value.bits()).map_err(hydration_scalar_error)?,
        ),
        CanonicalValue::Boolean(value) => EncodedScalar::Boolean(*value),
        CanonicalValue::Date(value) => {
            EncodedScalar::Date(Date::try_new(value.to_string()).map_err(hydration_scalar_error)?)
        }
        CanonicalValue::DateTime(value) => EncodedScalar::DateTime(
            DateTime::try_new(value.to_string()).map_err(hydration_scalar_error)?,
        ),
        CanonicalValue::DateTimeTz(value) => {
            EncodedScalar::DateTimeTz(DateTimeTz::from_canonical(value.clone()))
        }
        CanonicalValue::Decimal(value) => EncodedScalar::Decimal(
            Decimal::try_new(value.as_str()).map_err(hydration_scalar_error)?,
        ),
        CanonicalValue::Duration(value) => EncodedScalar::Duration(
            Duration::try_new(value.to_string()).map_err(hydration_scalar_error)?,
        ),
    };
    Ok(mapped)
}

fn hydration_scalar_error(source: crate::__codegen::ValidationError) -> Error {
    Error::model_validation(
        ModelValidationPhase::Hydration,
        "invalid_projected_scalar",
        Vec::new(),
        "engine-projected scalar cannot be represented by the generated Rust domain",
        Some(Box::new(source)),
    )
}

fn decode_type_identity(
    identity: &str,
    phase: ModelValidationPhase,
    code: &'static str,
    path: Vec<String>,
    message: &'static str,
) -> Result<TypeId> {
    from_canonical_json(identity.as_bytes()).map_err(|source| {
        Error::model_validation(phase, code, path, message, Some(Box::new(source)))
    })
}

fn resolve_owns_identity(
    installed: &InstalledRuntimeProjection,
    owner: &TypeId,
    identity: &str,
    phase: ModelValidationPhase,
    code: &'static str,
    path: Vec<String>,
    message: &'static str,
) -> Result<OwnsFactId> {
    let Some(model) = installed.projection().models().get(owner) else {
        return Err(model_error(
            phase,
            "model_not_projected",
            vec!["type".into()],
            "selected model is absent from the installed projection",
        ));
    };
    // Generated packages carry declaration authority, while projected DTOs
    // are keyed by the selected model's effective ownership identities.
    let mut matches = model.query_tokens().fields().values().filter_map(|token| {
        let canonical = to_canonical_json(token.declaring_id()).ok()?;
        (canonical.as_slice() == identity.as_bytes()).then(|| token.id().clone())
    });
    let Some(field_id) = matches.next() else {
        return Err(model_error(phase, code, path, message));
    };
    if matches.next().is_some() {
        return Err(model_error(
            phase,
            "ambiguous_field_identity",
            path,
            "generated ownership identity resolves to more than one projected field",
        ));
    }
    Ok(field_id)
}

fn declaring_owns_identity<'a>(
    installed: &'a InstalledRuntimeProjection,
    owner: &TypeId,
    effective: &OwnsFactId,
    path: Vec<String>,
) -> Result<&'a OwnsFactId> {
    installed
        .projection()
        .models()
        .get(owner)
        .and_then(|model| model.query_tokens().fields().get(effective))
        .map(|token| token.declaring_id())
        .ok_or_else(|| {
            model_error(
                ModelValidationPhase::Hydration,
                "invalid_installed_projection",
                path,
                "effective ownership identity has no declaring generated field token",
            )
        })
}

fn decode_role_identity(
    identity: &str,
    phase: ModelValidationPhase,
    code: &'static str,
    path: Vec<String>,
    message: &'static str,
) -> Result<RoleId> {
    from_canonical_json(identity.as_bytes()).map_err(|source| {
        Error::model_validation(phase, code, path, message, Some(Box::new(source)))
    })
}

fn encode_type_identity(value: &TypeId, path: Vec<String>) -> Result<String> {
    let bytes = to_canonical_json(value).map_err(|source| {
        Error::model_validation(
            ModelValidationPhase::Hydration,
            "invalid_model_identity",
            path.clone(),
            "engine-projected type identity cannot be rendered canonically",
            Some(Box::new(source)),
        )
    })?;
    identity_string(bytes, "invalid_model_identity", path)
}

fn encode_owns_identity(value: &OwnsFactId, path: Vec<String>) -> Result<String> {
    let bytes = to_canonical_json(value).map_err(|source| {
        Error::model_validation(
            ModelValidationPhase::Hydration,
            "invalid_field_identity",
            path.clone(),
            "engine-projected ownership identity cannot be rendered canonically",
            Some(Box::new(source)),
        )
    })?;
    identity_string(bytes, "invalid_field_identity", path)
}

fn encode_role_identity(value: &RoleId, path: Vec<String>) -> Result<String> {
    let bytes = to_canonical_json(value).map_err(|source| {
        Error::model_validation(
            ModelValidationPhase::Hydration,
            "invalid_role_identity",
            path.clone(),
            "engine-projected role identity cannot be rendered canonically",
            Some(Box::new(source)),
        )
    })?;
    identity_string(bytes, "invalid_role_identity", path)
}

fn identity_string(bytes: Vec<u8>, code: &'static str, path: Vec<String>) -> Result<String> {
    String::from_utf8(bytes).map_err(|source| {
        Error::model_validation(
            ModelValidationPhase::Hydration,
            code,
            path,
            "engine-projected identity is not UTF-8",
            Some(Box::new(source)),
        )
    })
}

fn model_error(
    phase: ModelValidationPhase,
    code: &'static str,
    path: Vec<String>,
    message: &'static str,
) -> Error {
    Error::model_validation(phase, code, path, message, None)
}

#[cfg(test)]
mod tests {
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::id::{AttributeId, TypeKind};
    use type_bridge_contract::limits::MAX_CANONICAL_STRING_BYTES;
    use type_bridge_contract::projection::{BindingTarget, ProjectionConfig};
    use type_bridge_contract::schema::DocumentId;
    use type_bridge_contract::sdk_diagnostic::{
        SdkDiagnosticCategory, SdkDiagnosticName, SdkDiagnosticPathSegment, SdkExecutionDiagnostic,
    };
    use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
    use type_bridge_schema_codegen::RustEmitter;

    use super::*;

    macro_rules! canonical_identity {
        ($value:expr) => {
            String::from_utf8(to_canonical_json($value).unwrap()).unwrap()
        };
    }

    #[test]
    fn declaring_owns_identity_resolves_to_the_effective_inherited_field() {
        let (installed, declaring, effective, person) = inherited_field_fixture();
        let identity = String::from_utf8(to_canonical_json(&declaring).unwrap()).unwrap();

        let resolved = resolve_owns_identity(
            &installed,
            &person,
            &identity,
            ModelValidationPhase::Input,
            "unexpected_field_evidence",
            vec!["fields[0]".into()],
            "generated ownership identity must resolve",
        )
        .unwrap();

        assert_eq!(resolved, effective);
    }

    #[test]
    fn effective_inherited_field_hydrates_with_its_declaring_identity() {
        let (installed, declaring, effective, person) = inherited_field_fixture();
        let nickname = TypeId::new(TypeKind::Attribute, "nickname").unwrap();
        let value = type_bridge_orm::ProjectedAttributeValue::try_new(
            &installed,
            nickname,
            type_bridge_contract::value::CanonicalValue::String(
                type_bridge_contract::value::CanonicalString::new("Ada").unwrap(),
            ),
        )
        .unwrap();
        let thing = ProjectedThing::try_new(
            &installed,
            person,
            "0x10".into(),
            vec![(effective, vec![value])],
            vec![],
        )
        .unwrap();

        let row = projected_to_hydrated_row(&thing, &installed).unwrap();
        let expected = String::from_utf8(to_canonical_json(&declaring).unwrap()).unwrap();

        assert_eq!(row.fields()[0].0, expected);
    }

    #[test]
    fn malformed_hydration_identity_retains_the_hydration_phase() {
        let (installed, _, _, _) = inherited_field_fixture();
        let row = HydratedRow::from_owned(
            "not-canonical-json".into(),
            "0x10".into(),
            Vec::new(),
            Vec::new(),
        );

        let error = validate_hydrated_row(&row, &installed).unwrap_err();

        assert_eq!(
            error.model_validation_phase(),
            Some(ModelValidationPhase::Hydration)
        );
        assert_eq!(error.code(), Some("invalid_model_identity"));
    }

    #[test]
    fn hydrated_scalars_and_nested_keys_use_common_integrity_diagnostics() {
        let (installed, person, membership, identifier, nickname, member) = hydration_fixture();
        let invalid_field = HydratedRow::from_owned(
            canonical_identity!(&person),
            "0x10".into(),
            vec![
                (
                    canonical_identity!(&identifier),
                    vec![EncodedScalar::String("ada".into())],
                ),
                (
                    canonical_identity!(&nickname),
                    vec![EncodedScalar::String("ada".into())],
                ),
            ],
            vec![],
        );
        let error = validate_hydrated_row(&invalid_field, &installed).unwrap_err();
        assert_common_hydration_error(
            &error,
            "regex_constraint_violation",
            &[SdkDiagnosticPathSegment::Type(
                TypeId::new(TypeKind::Attribute, "nickname").unwrap(),
            )],
        );

        let oversized = HydratedRow::from_owned(
            canonical_identity!(&person),
            "0x10".into(),
            vec![
                (
                    canonical_identity!(&identifier),
                    vec![EncodedScalar::String(
                        "x".repeat(MAX_CANONICAL_STRING_BYTES + 1),
                    )],
                ),
                (
                    canonical_identity!(&nickname),
                    vec![EncodedScalar::String("Ada".into())],
                ),
            ],
            vec![],
        );
        let error = validate_hydrated_row(&oversized, &installed).unwrap_err();
        assert_common_hydration_error(
            &error,
            "wrong_scalar_domain",
            &[SdkDiagnosticPathSegment::Type(
                TypeId::new(TypeKind::Attribute, "identifier").unwrap(),
            )],
        );

        let invalid_key = HydratedRow::from_owned(
            canonical_identity!(&membership),
            "0x20".into(),
            vec![],
            vec![(
                canonical_identity!(&member),
                vec![HydratedPlayer::from_owned(
                    canonical_identity!(&person),
                    Some("0x10".into()),
                    vec![(
                        canonical_identity!(&identifier),
                        EncodedScalar::String("Ada".into()),
                    )],
                )],
            )],
        );
        let error = validate_hydrated_row(&invalid_key, &installed).unwrap_err();
        assert_common_hydration_error(
            &error,
            "regex_constraint_violation",
            &[SdkDiagnosticPathSegment::Type(
                TypeId::new(TypeKind::Attribute, "identifier").unwrap(),
            )],
        );
    }

    #[test]
    fn hydrated_player_iid_is_integrity_and_provider_free_origins_remain_unbound() {
        let (installed, person, membership, identifier, _, member) = hydration_fixture();
        let row = |player_iid: &str| {
            HydratedRow::from_owned(
                canonical_identity!(&membership),
                "0x20".into(),
                vec![],
                vec![(
                    canonical_identity!(&member),
                    vec![HydratedPlayer::from_owned(
                        canonical_identity!(&person),
                        Some(player_iid.into()),
                        vec![(
                            canonical_identity!(&identifier),
                            EncodedScalar::String("ada".into()),
                        )],
                    )],
                )],
            )
        };

        let error = project_hydrated_row(&row("not-an-iid"), &installed).unwrap_err();
        assert_common_hydration_error(
            &error,
            "noncanonical_iid",
            &[
                SdkDiagnosticPathSegment::Type(person.clone()),
                SdkDiagnosticPathSegment::Argument(SdkDiagnosticName::new("iid").unwrap()),
            ],
        );

        let projected = project_hydrated_row(&row("0x10"), &installed).unwrap();
        assert!(projected.origin_carrier().is_none());
        assert!(
            projected.roles()[&member][0]
                .reference()
                .origin_carrier()
                .is_none()
        );
    }

    #[test]
    fn hydration_member_identity_still_precedes_nested_scalar_and_iid_validation() {
        let (installed, person, membership, _, _, member) = hydration_fixture();
        let invalid_field = HydratedRow::from_owned(
            canonical_identity!(&person),
            "0x10".into(),
            vec![(
                "not-canonical-json".into(),
                vec![EncodedScalar::String("invalid".into())],
            )],
            vec![],
        );
        let error = validate_hydrated_row(&invalid_field, &installed).unwrap_err();
        assert_eq!(error.code(), Some("unexpected_field_evidence"));

        let invalid_key = HydratedRow::from_owned(
            canonical_identity!(&membership),
            "0x20".into(),
            vec![],
            vec![(
                canonical_identity!(&member),
                vec![HydratedPlayer::from_owned(
                    canonical_identity!(&person),
                    Some("not-an-iid".into()),
                    vec![(
                        "not-canonical-json".into(),
                        EncodedScalar::String("invalid".into()),
                    )],
                )],
            )],
        );
        let error = validate_hydrated_row(&invalid_key, &installed).unwrap_err();
        assert_eq!(error.code(), Some("unexpected_reference_key"));
    }

    #[test]
    fn hydrated_named_zone_overlap_retains_the_selected_effective_offset() {
        let (installed, attribute_type) = datetime_tz_fixture();
        let earlier = type_bridge_schema::parse_provider_datetime_tz_evidence(
            "2024-10-27T01:30:00+01:00[Europe/London]",
        )
        .unwrap();
        let later = type_bridge_schema::parse_provider_datetime_tz_evidence(
            "2024-10-27T01:30:00Z[Europe/London]",
        )
        .unwrap();
        let earlier = DateTimeTz::from_canonical(earlier);
        let later = DateTimeTz::from_canonical(later);
        assert_eq!(earlier.as_str(), later.as_str());

        let earlier = project_hydrated_attribute_value(
            &installed,
            attribute_type.clone(),
            &EncodedScalar::DateTimeTz(earlier),
        )
        .unwrap();
        let later = project_hydrated_attribute_value(
            &installed,
            attribute_type,
            &EncodedScalar::DateTimeTz(later),
        )
        .unwrap();
        let CanonicalValue::DateTimeTz(earlier) = earlier.value() else {
            panic!("earlier overlap side changed scalar domain")
        };
        let CanonicalValue::DateTimeTz(later) = later.value() else {
            panic!("later overlap side changed scalar domain")
        };
        assert_eq!(earlier.effective_offset_seconds(), 3_600);
        assert_eq!(later.effective_offset_seconds(), 0);
        assert!(earlier.semantic_utc_nanoseconds() < later.semantic_utc_nanoseconds());
    }

    fn assert_common_hydration_error(error: &Error, code: &str, path: &[SdkDiagnosticPathSegment]) {
        assert_eq!(error.category(), crate::ErrorCategory::ModelValidation);
        assert_eq!(
            error.model_validation_phase(),
            Some(ModelValidationPhase::Hydration)
        );
        assert_eq!(error.code(), Some(code));
        let diagnostic = std::error::Error::source(error)
            .and_then(|source| source.downcast_ref::<SdkExecutionDiagnostic>())
            .expect("common hydration validation retains its SDK diagnostic");
        assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Integrity);
        assert_eq!(diagnostic.code().as_str(), code);
        assert_eq!(diagnostic.path(), path);
    }

    fn hydration_fixture() -> (
        InstalledRuntimeProjection,
        TypeId,
        TypeId,
        OwnsFactId,
        OwnsFactId,
        RoleId,
    ) {
        let documents = SchemaDocumentSet::parse([(
            DocumentId::new("hydration.yaml").unwrap(),
            r#"format: typebridge.schema/v2
attributes:
  identifier:
    value:
      type: string
      regex: "^[a-z]+$"
  nickname:
    value:
      type: string
      regex: "^[A-Z][a-z]+$"
entities:
  person:
    owns:
      identifier: { key: true }
      nickname: { card: 1 }
relations:
  membership:
    relates:
      member: { card: 1 }
plays:
  person:
    membership: [member]
"#,
        )])
        .unwrap();
        let resolved = resolve(
            &normalize_documents(&documents).unwrap(),
            &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
        )
        .unwrap();
        let emitter = RustEmitter::new();
        let projection = project(
            &resolved,
            BindingTarget::Rust,
            &ProjectionConfig::rust(),
            &emitter.generator_handlers(),
            &emitter.code_resources().unwrap(),
        )
        .unwrap();
        let installed = InstalledRuntimeProjection::try_new(projection).unwrap();
        let person = TypeId::new(TypeKind::Entity, "person").unwrap();
        let membership = TypeId::new(TypeKind::Relation, "membership").unwrap();
        let identifier =
            OwnsFactId::new(person.clone(), AttributeId::new("identifier").unwrap()).unwrap();
        let nickname =
            OwnsFactId::new(person.clone(), AttributeId::new("nickname").unwrap()).unwrap();
        let member = RoleId::new("membership", "member").unwrap();
        (installed, person, membership, identifier, nickname, member)
    }

    fn datetime_tz_fixture() -> (InstalledRuntimeProjection, TypeId) {
        let documents = SchemaDocumentSet::parse([(
            DocumentId::new("datetime-tz-hydration.yaml").unwrap(),
            r#"format: typebridge.schema/v2
attributes:
  observed-at: { value: datetime-tz }
entities:
  event:
    owns:
      observed-at: { card: 1 }
"#,
        )])
        .unwrap();
        let resolved = resolve(
            &normalize_documents(&documents).unwrap(),
            &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
        )
        .unwrap();
        let emitter = RustEmitter::new();
        let projection = project(
            &resolved,
            BindingTarget::Rust,
            &ProjectionConfig::rust(),
            &emitter.generator_handlers(),
            &emitter.code_resources().unwrap(),
        )
        .unwrap();
        (
            InstalledRuntimeProjection::try_new(projection).unwrap(),
            TypeId::new(TypeKind::Attribute, "observed-at").unwrap(),
        )
    }

    fn inherited_field_fixture() -> (InstalledRuntimeProjection, OwnsFactId, OwnsFactId, TypeId) {
        let documents = SchemaDocumentSet::parse([(
            DocumentId::new("inherited-field.yaml").unwrap(),
            r#"format: typebridge.schema/v2
attributes:
  nickname: { value: string }
entities:
  actor:
    abstract: true
    owns:
      nickname: { card: { min: 0, max: 1 } }
  person:
    sub: actor
"#,
        )])
        .unwrap();
        let resolved = resolve(
            &normalize_documents(&documents).unwrap(),
            &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
        )
        .unwrap();
        let emitter = RustEmitter::new();
        let projection = project(
            &resolved,
            BindingTarget::Rust,
            &ProjectionConfig::rust(),
            &emitter.generator_handlers(),
            &emitter.code_resources().unwrap(),
        )
        .unwrap();
        let installed = InstalledRuntimeProjection::try_new(projection).unwrap();
        let actor = TypeId::new(TypeKind::Entity, "actor").unwrap();
        let person = TypeId::new(TypeKind::Entity, "person").unwrap();
        let nickname = AttributeId::new("nickname").unwrap();
        let declaring = OwnsFactId::new(actor, nickname.clone()).unwrap();
        let effective = OwnsFactId::new(person.clone(), nickname).unwrap();
        (installed, declaring, effective, person)
    }
}
