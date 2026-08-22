//! Canonical projected-record adapters over exact installed ORM authority.

use thiserror::Error;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::id::{RoleId, TypeId, TypeKind};
use type_bridge_contract::projected_record::{
    ProjectedCollectionMode, ProjectedField, ProjectedFieldId, ProjectedMemberValues,
    ProjectedRecord, ProjectedRecordContent, ProjectedRecordReference, ProjectedRecordRolePlayer,
    ProjectedReferenceKey, ProjectedRole, ProjectedStructMember,
};
use type_bridge_contract::projection::{
    ProjectedContainer, ProjectedMultiplicity, ReadRoleProjection,
};
use type_bridge_contract::schema::{CollectionMode, OwnsFactId};
use type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic;
use type_bridge_contract::value::CanonicalValue;

use crate::projected_model::{
    ProjectedAttributeValue, ProjectedCreate, ProjectedReference, ProjectedRolePlayer,
    ProjectedThing, ProjectionBrand,
};
use crate::runtime_projection::InstalledRuntimeProjection;

/// A validated ORM value admitted by the shared canonical record seam.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectedCodecValue {
    /// One exact projected scalar wrapper.
    Attribute(ProjectedAttributeValue),
    /// One exact generated struct value.
    Struct(ProjectedStructValue),
    /// One exact generated create payload.
    Create(ProjectedCreate),
    /// One provider-free detached hydrated snapshot.
    Snapshot(ProjectedThing),
    /// One detached IID-or-key reference.
    Reference(ProjectedReference),
}

/// One ordered generated struct value branded by exact installed projection authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedStructValue {
    brand: ProjectionBrand,
    type_id: TypeId,
    members: Vec<Option<CanonicalValue>>,
}

impl ProjectedStructValue {
    /// Validate and brand values in exact semantic declaration order.
    pub fn try_new(
        installed: &InstalledRuntimeProjection,
        type_id: TypeId,
        members: Vec<Option<CanonicalValue>>,
    ) -> Result<Self, ProjectedCodecError> {
        if type_id.kind() != TypeKind::Struct {
            return Err(contract(
                "projected_codec_struct_kind",
                "projected struct value requires a struct type",
            )
            .into());
        }
        let projection = installed
            .projection()
            .structs()
            .values()
            .find(|projection| projection.id().label() == type_id.label())
            .ok_or_else(|| {
                contract(
                    "projected_codec_struct_missing",
                    "struct type is absent from installed projection",
                )
            })?;
        if projection.fields().len() != members.len() {
            return Err(contract(
                "projected_codec_struct_member_count",
                "struct member count differs from installed projection",
            )
            .into());
        }
        for (field, value) in projection.fields().iter().zip(&members) {
            match value {
                None if !field.optional() => {
                    return Err(contract(
                        "projected_codec_required_struct_member_missing",
                        "required struct member is absent",
                    )
                    .into());
                }
                Some(value) if value.value_type() != field.value_type() => {
                    return Err(contract(
                        "projected_codec_struct_member_domain",
                        "struct member has the wrong canonical scalar domain",
                    )
                    .into());
                }
                None | Some(_) => {}
            }
        }
        Ok(Self {
            brand: ProjectionBrand::from_installed(installed),
            type_id,
            members,
        })
    }

    /// Return the exact projected struct type.
    #[must_use]
    pub const fn type_id(&self) -> &TypeId {
        &self.type_id
    }

    /// Return values in semantic declaration order with exact optional absence.
    #[must_use]
    pub fn members(&self) -> &[Option<CanonicalValue>] {
        &self.members
    }

    /// Verify that the value belongs to exact installed projection authority.
    pub fn validate_for(
        &self,
        installed: &InstalledRuntimeProjection,
    ) -> Result<(), ProjectedCodecError> {
        self.brand.validate(installed, Vec::new())?;
        Self::try_new(installed, self.type_id.clone(), self.members.clone()).map(|_| ())
    }
}

/// A stable failure from canonical record validation or ORM materialization.
#[derive(Debug, Error)]
pub enum ProjectedCodecError {
    /// The binding-neutral record contract rejected the operation.
    #[error(transparent)]
    Contract(#[from] Diagnostic),
    /// Installed projection validation rejected target materialization.
    #[error(transparent)]
    Materialization(#[from] SdkExecutionDiagnostic),
}

/// Encode one branded scalar through the shared target-independent record contract.
pub fn record_from_attribute(
    installed: &InstalledRuntimeProjection,
    value: &ProjectedAttributeValue,
) -> Result<ProjectedRecord, ProjectedCodecError> {
    value.validate_for(installed)?;
    build_record(
        installed,
        ProjectedRecordContent::AttributeValue {
            r#type: value.attribute_type().clone(),
            value: value.value().clone(),
        },
    )
}

/// Encode one branded generated struct through the shared canonical record contract.
pub fn record_from_struct(
    installed: &InstalledRuntimeProjection,
    value: &ProjectedStructValue,
) -> Result<ProjectedRecord, ProjectedCodecError> {
    value.validate_for(installed)?;
    let projection = installed
        .projection()
        .structs()
        .values()
        .find(|projection| projection.id().label() == value.type_id().label())
        .ok_or_else(|| {
            contract(
                "projected_codec_struct_missing",
                "struct type is absent from installed projection",
            )
        })?;
    let members = projection
        .fields()
        .iter()
        .zip(value.members())
        .map(|(field, value)| match value {
            Some(value) => ProjectedStructMember::present(field.name().clone(), value.clone()),
            None => ProjectedStructMember::absent(field.name().clone()),
        })
        .collect();
    build_record(
        installed,
        ProjectedRecordContent::StructValue {
            r#type: value.type_id().clone(),
            members,
        },
    )
}

/// Encode one exact create payload through the shared target-independent record contract.
pub fn record_from_create(
    installed: &InstalledRuntimeProjection,
    value: &ProjectedCreate,
) -> Result<ProjectedRecord, ProjectedCodecError> {
    value.validate_for(installed)?;
    let model = installed
        .projection()
        .models()
        .get(value.type_id())
        .ok_or_else(|| contract("projected_codec_model_missing", "projected model is absent"))?;
    let fields = model
        .create()
        .fields()
        .iter()
        .map(|field| {
            projected_field(
                value.type_id(),
                field.token(),
                field.multiplicity(),
                value.field_is_present(field.token()),
                value.fields().get(field.token()).map_or(&[], Vec::as_slice),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let content = match value.type_id().kind() {
        TypeKind::Entity => ProjectedRecordContent::EntityCreate {
            r#type: value.type_id().clone(),
            fields,
        },
        TypeKind::Relation => {
            let roles = model
                .create()
                .roles()
                .iter()
                .map(|(role_id, role)| {
                    projected_reference_role(
                        role_id,
                        role.multiplicity(),
                        value.role_is_present(role_id),
                        value.roles().get(role_id).map_or(&[], Vec::as_slice),
                    )
                })
                .collect::<Result<Vec<_>, ProjectedCodecError>>()?;
            ProjectedRecordContent::RelationCreate {
                r#type: value.type_id().clone(),
                fields,
                roles,
            }
        }
        TypeKind::Attribute | TypeKind::Struct => {
            return Err(contract(
                "projected_codec_create_kind",
                "projected create has a non-thing type",
            )
            .into());
        }
    };
    build_record(installed, content)
}

/// Encode one hydrated thing as a provider-free detached canonical snapshot.
pub fn record_from_snapshot(
    installed: &InstalledRuntimeProjection,
    value: &ProjectedThing,
) -> Result<ProjectedRecord, ProjectedCodecError> {
    value.validate_for(installed)?;
    let model = installed
        .projection()
        .models()
        .get(value.type_id())
        .ok_or_else(|| contract("projected_codec_model_missing", "projected model is absent"))?;
    let fields = model
        .complete_read()
        .fields()
        .iter()
        .map(|field| {
            projected_field(
                value.type_id(),
                field.token(),
                field.multiplicity(),
                value.field_is_present(field.token()),
                value.fields().get(field.token()).map_or(&[], Vec::as_slice),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let content = match value.type_id().kind() {
        TypeKind::Entity => ProjectedRecordContent::EntitySnapshot {
            r#type: value.type_id().clone(),
            iid: value.iid().to_owned(),
            fields,
        },
        TypeKind::Relation => {
            let roles = model
                .complete_read()
                .roles()
                .iter()
                .map(|(role_id, role)| {
                    projected_player_role(
                        installed,
                        value,
                        role_id,
                        role,
                        value.roles().get(role_id).map_or(&[], Vec::as_slice),
                    )
                })
                .collect::<Result<Vec<_>, ProjectedCodecError>>()?;
            ProjectedRecordContent::RelationSnapshot {
                r#type: value.type_id().clone(),
                iid: value.iid().to_owned(),
                fields,
                roles,
            }
        }
        TypeKind::Attribute | TypeKind::Struct => {
            return Err(contract(
                "projected_codec_snapshot_kind",
                "projected snapshot has a non-thing type",
            )
            .into());
        }
    };
    build_record(installed, content)
}

/// Encode one detached reference through the shared target-independent record contract.
pub fn record_from_reference(
    installed: &InstalledRuntimeProjection,
    value: &ProjectedReference,
) -> Result<ProjectedRecord, ProjectedCodecError> {
    value.validate_for(installed)?;
    build_record(
        installed,
        ProjectedRecordContent::Reference {
            reference: projected_reference(value)?,
        },
    )
}

/// Validate one canonical record against exact installed authority and materialize it.
///
/// Hydrated records always return provider-free snapshots or references; record bytes
/// never restore database origin or mutation authority.
pub fn materialize_record(
    installed: &InstalledRuntimeProjection,
    record: &ProjectedRecord,
) -> Result<ProjectedCodecValue, ProjectedCodecError> {
    validate_authority(installed, record)?;
    match record.content() {
        ProjectedRecordContent::AttributeValue { r#type, value } => {
            Ok(ProjectedCodecValue::Attribute(
                ProjectedAttributeValue::try_new(installed, r#type.clone(), value.clone())?,
            ))
        }
        ProjectedRecordContent::EntityCreate { r#type, fields } => Ok(ProjectedCodecValue::Create(
            materialize_create(installed, r#type, fields, &[])?,
        )),
        ProjectedRecordContent::RelationCreate {
            r#type,
            fields,
            roles,
        } => Ok(ProjectedCodecValue::Create(materialize_create(
            installed, r#type, fields, roles,
        )?)),
        ProjectedRecordContent::EntitySnapshot {
            r#type,
            iid,
            fields,
        } => Ok(ProjectedCodecValue::Snapshot(materialize_snapshot(
            installed,
            r#type,
            iid,
            fields,
            &[],
        )?)),
        ProjectedRecordContent::RelationSnapshot {
            r#type,
            iid,
            fields,
            roles,
        } => Ok(ProjectedCodecValue::Snapshot(materialize_snapshot(
            installed, r#type, iid, fields, roles,
        )?)),
        ProjectedRecordContent::Reference { reference } => Ok(ProjectedCodecValue::Reference(
            materialize_reference(installed, reference)?,
        )),
        ProjectedRecordContent::StructValue { r#type, members } => {
            let projection = installed
                .projection()
                .structs()
                .values()
                .find(|projection| projection.id().label() == r#type.label())
                .ok_or_else(|| {
                    contract(
                        "projected_codec_struct_missing",
                        "struct type is absent from installed projection",
                    )
                })?;
            if projection.fields().len() != members.len()
                || projection
                    .fields()
                    .iter()
                    .zip(members)
                    .any(|(expected, actual)| expected.name() != actual.name())
            {
                return Err(contract(
                    "projected_codec_struct_member_order",
                    "struct members differ from installed declaration order",
                )
                .into());
            }
            Ok(ProjectedCodecValue::Struct(ProjectedStructValue::try_new(
                installed,
                r#type.clone(),
                members
                    .iter()
                    .map(|member| member.value().cloned())
                    .collect(),
            )?))
        }
    }
}

fn build_record(
    installed: &InstalledRuntimeProjection,
    content: ProjectedRecordContent,
) -> Result<ProjectedRecord, ProjectedCodecError> {
    let profile = installed
        .projection()
        .semantic_fingerprint()
        .as_fingerprint()
        .semantic_profile()
        .cloned()
        .ok_or_else(|| {
            contract(
                "projected_codec_semantic_profile_missing",
                "installed semantic authority has no profile",
            )
        })?;
    let declared = installed
        .declared_schema_identity()
        .cloned()
        .ok_or_else(|| {
            contract(
                "projected_codec_declared_authority_missing",
                "canonical record operations require installed declared-schema authority",
            )
        })?;
    Ok(ProjectedRecord::try_new(profile, declared, content)?)
}

fn validate_authority(
    installed: &InstalledRuntimeProjection,
    record: &ProjectedRecord,
) -> Result<(), ProjectedCodecError> {
    let expected_profile = installed
        .projection()
        .semantic_fingerprint()
        .as_fingerprint()
        .semantic_profile()
        .ok_or_else(|| {
            contract(
                "projected_codec_semantic_profile_missing",
                "installed semantic authority has no profile",
            )
        })?;
    let expected_declared = installed.declared_schema_identity().ok_or_else(|| {
        contract(
            "projected_codec_declared_authority_missing",
            "canonical record operations require installed declared-schema authority",
        )
    })?;
    if record.semantic_profile() != expected_profile {
        return Err(contract(
            "projected_codec_semantic_profile_mismatch",
            "record semantic profile differs from installed authority",
        )
        .into());
    }
    if record.declared_schema_identity() != expected_declared {
        return Err(contract(
            "projected_codec_declared_schema_mismatch",
            "record declared-schema identity differs from installed authority",
        )
        .into());
    }
    Ok(())
}

fn projected_field(
    owner: &TypeId,
    field: &OwnsFactId,
    multiplicity: ProjectedMultiplicity,
    present: bool,
    values: &[ProjectedAttributeValue],
) -> Result<ProjectedField, ProjectedCodecError> {
    let identity = ProjectedFieldId::new(owner.clone(), field.attribute().clone())?;
    let member = if present
        && !(multiplicity.container() == ProjectedContainer::Scalar && values.is_empty())
    {
        ProjectedMemberValues::Present {
            collection: collection_mode(multiplicity),
            values: values.iter().map(|value| value.value().clone()).collect(),
        }
    } else {
        ProjectedMemberValues::Absent
    };
    Ok(ProjectedField::new(identity, member))
}

fn projected_reference_role(
    role: &RoleId,
    multiplicity: ProjectedMultiplicity,
    present: bool,
    values: &[ProjectedReference],
) -> Result<ProjectedRole, ProjectedCodecError> {
    let member = if present
        && !(multiplicity.container() == ProjectedContainer::Scalar && values.is_empty())
    {
        ProjectedMemberValues::Present {
            collection: collection_mode(multiplicity),
            values: values
                .iter()
                .map(|value| {
                    Ok(ProjectedRecordRolePlayer::Reference {
                        reference: projected_reference(value)?,
                    })
                })
                .collect::<Result<Vec<_>, ProjectedCodecError>>()?,
        }
    } else {
        ProjectedMemberValues::Absent
    };
    Ok(ProjectedRole::new(role.clone(), member))
}

fn projected_player_role(
    installed: &InstalledRuntimeProjection,
    thing: &ProjectedThing,
    role_id: &RoleId,
    role: &ReadRoleProjection,
    values: &[ProjectedRolePlayer],
) -> Result<ProjectedRole, ProjectedCodecError> {
    let member = if thing.role_is_present(role_id)
        && !(role.multiplicity().container() == ProjectedContainer::Scalar && values.is_empty())
    {
        ProjectedMemberValues::Present {
            collection: collection_mode(role.multiplicity()),
            values: values
                .iter()
                .map(|player| {
                    let reference = projected_reference(player.reference())?;
                    Ok(match player.exact_form() {
                        Some(type_bridge_contract::projection::ProjectedModelForm::Complete) => {
                            let player_type = player.reference().type_id();
                            let player_model = installed
                                .projection()
                                .models()
                                .get(player_type)
                                .ok_or_else(|| {
                                    contract(
                                        "projected_codec_model_missing",
                                        "role-player model is absent",
                                    )
                                })?;
                            let fields = player_model
                                .complete_read()
                                .fields()
                                .iter()
                                .map(|field| {
                                    projected_field(
                                        player_type,
                                        field.token(),
                                        field.multiplicity(),
                                        player.field_is_present(field.token()),
                                        player
                                            .fields()
                                            .get(field.token())
                                            .map_or(&[], Vec::as_slice),
                                    )
                                })
                                .collect::<Result<Vec<_>, ProjectedCodecError>>()?;
                            ProjectedRecordRolePlayer::Complete { reference, fields }
                        }
                        Some(type_bridge_contract::projection::ProjectedModelForm::Reference)
                        | None => ProjectedRecordRolePlayer::Reference { reference },
                    })
                })
                .collect::<Result<Vec<_>, ProjectedCodecError>>()?,
        }
    } else {
        ProjectedMemberValues::Absent
    };
    Ok(ProjectedRole::new(role_id.clone(), member))
}

fn projected_reference(
    value: &ProjectedReference,
) -> Result<ProjectedRecordReference, ProjectedCodecError> {
    let keys = value
        .keys()
        .iter()
        .map(|(field, scalar)| {
            Ok(ProjectedReferenceKey::new(
                ProjectedFieldId::new(value.type_id().clone(), field.attribute().clone())?,
                scalar.value().clone(),
            ))
        })
        .collect::<Result<Vec<_>, ProjectedCodecError>>()?;
    Ok(ProjectedRecordReference::try_new(
        value.type_id().clone(),
        value.iid().map(str::to_owned),
        keys,
    )?)
}

fn materialize_create(
    installed: &InstalledRuntimeProjection,
    type_id: &TypeId,
    fields: &[ProjectedField],
    roles: &[ProjectedRole],
) -> Result<ProjectedCreate, ProjectedCodecError> {
    let model = installed
        .projection()
        .models()
        .get(type_id)
        .ok_or_else(|| contract("projected_codec_model_missing", "projected model is absent"))?;
    let fields = materialize_fields(
        installed,
        type_id,
        fields,
        model
            .create()
            .fields()
            .iter()
            .map(|field| (field.token(), field.multiplicity())),
    )?;
    if roles.len() != model.create().roles().len() {
        return Err(contract(
            "projected_codec_role_set_mismatch",
            "record roles do not exactly cover the create projection",
        )
        .into());
    }
    let mut materialized_roles = Vec::new();
    for role in roles {
        let expected = model.create().roles().get(role.identity()).ok_or_else(|| {
            contract(
                "projected_codec_role_missing",
                "record role is not creatable",
            )
        })?;
        if let ProjectedMemberValues::Present { collection, values } = role.member() {
            require_collection(*collection, expected.multiplicity())?;
            let mut references = Vec::new();
            for value in values {
                let ProjectedRecordRolePlayer::Reference { reference } = value else {
                    return Err(contract(
                        "projected_codec_create_player_form",
                        "create roles require reference players",
                    )
                    .into());
                };
                references.push(materialize_reference(installed, reference)?);
            }
            materialized_roles.push((role.identity().clone(), references));
        }
    }
    Ok(ProjectedCreate::try_new(
        installed,
        type_id.clone(),
        fields,
        materialized_roles,
    )?)
}

fn materialize_snapshot(
    installed: &InstalledRuntimeProjection,
    type_id: &TypeId,
    iid: &str,
    fields: &[ProjectedField],
    roles: &[ProjectedRole],
) -> Result<ProjectedThing, ProjectedCodecError> {
    let model = installed
        .projection()
        .models()
        .get(type_id)
        .ok_or_else(|| contract("projected_codec_model_missing", "projected model is absent"))?;
    let fields = materialize_fields(
        installed,
        type_id,
        fields,
        model
            .complete_read()
            .fields()
            .iter()
            .map(|field| (field.token(), field.multiplicity())),
    )?;
    if roles.len() != model.complete_read().roles().len() {
        return Err(contract(
            "projected_codec_role_set_mismatch",
            "record roles do not exactly cover the complete-read projection",
        )
        .into());
    }
    let mut materialized_roles = Vec::new();
    for role in roles {
        let expected = model
            .complete_read()
            .roles()
            .get(role.identity())
            .ok_or_else(|| {
                contract(
                    "projected_codec_role_missing",
                    "record role is not readable",
                )
            })?;
        if let ProjectedMemberValues::Present { collection, values } = role.member() {
            require_collection(*collection, expected.multiplicity())?;
            let mut players = Vec::new();
            for value in values {
                players.push(materialize_player(installed, expected, value)?);
            }
            materialized_roles.push((role.identity().clone(), players));
        }
    }
    Ok(ProjectedThing::try_new(
        installed,
        type_id.clone(),
        iid.to_owned(),
        fields,
        materialized_roles,
    )?)
}

fn materialize_fields<'a>(
    installed: &InstalledRuntimeProjection,
    owner: &TypeId,
    fields: &[ProjectedField],
    expected: impl Iterator<Item = (&'a OwnsFactId, ProjectedMultiplicity)>,
) -> Result<Vec<(OwnsFactId, Vec<ProjectedAttributeValue>)>, ProjectedCodecError> {
    let expected = expected.collect::<std::collections::BTreeMap<_, _>>();
    if fields.len() != expected.len() {
        return Err(contract(
            "projected_codec_field_set_mismatch",
            "record fields do not exactly cover the installed projection",
        )
        .into());
    }
    let mut output = Vec::new();
    for field in fields {
        let fact = OwnsFactId::new(owner.clone(), field.identity().attribute().clone())?;
        let multiplicity = expected.get(&fact).ok_or_else(|| {
            contract(
                "projected_codec_field_missing",
                "record field is not projected",
            )
        })?;
        if let ProjectedMemberValues::Present { collection, values } = field.member() {
            require_collection(*collection, *multiplicity)?;
            let attribute_type =
                TypeId::new(TypeKind::Attribute, fact.attribute().label().as_str())?;
            let values = values
                .iter()
                .cloned()
                .map(|value| {
                    ProjectedAttributeValue::try_new(installed, attribute_type.clone(), value)
                })
                .collect::<Result<Vec<_>, _>>()?;
            output.push((fact, values));
        }
    }
    Ok(output)
}

fn materialize_reference(
    installed: &InstalledRuntimeProjection,
    value: &ProjectedRecordReference,
) -> Result<ProjectedReference, ProjectedCodecError> {
    let keys = value
        .keys()
        .iter()
        .map(|key| {
            let fact =
                OwnsFactId::new(value.type_id().clone(), key.identity().attribute().clone())?;
            let attribute_type =
                TypeId::new(TypeKind::Attribute, fact.attribute().label().as_str())?;
            Ok((
                fact,
                ProjectedAttributeValue::try_new(installed, attribute_type, key.value().clone())?,
            ))
        })
        .collect::<Result<Vec<_>, ProjectedCodecError>>()?;
    Ok(ProjectedReference::try_new(
        installed,
        value.type_id().clone(),
        value.iid().map(str::to_owned),
        keys,
    )?)
}

fn materialize_player(
    installed: &InstalledRuntimeProjection,
    role: &ReadRoleProjection,
    value: &ProjectedRecordRolePlayer,
) -> Result<ProjectedRolePlayer, ProjectedCodecError> {
    match value {
        ProjectedRecordRolePlayer::Reference { reference } => {
            Ok(ProjectedRolePlayer::try_new_reference_for_hydration(
                installed,
                role,
                materialize_reference(installed, reference)?,
            )?)
        }
        ProjectedRecordRolePlayer::Complete { reference, fields } => {
            let reference = materialize_reference(installed, reference)?;
            let model = installed
                .projection()
                .models()
                .get(reference.type_id())
                .ok_or_else(|| {
                    contract(
                        "projected_codec_model_missing",
                        "role-player model is absent",
                    )
                })?;
            let fields = materialize_fields(
                installed,
                reference.type_id(),
                fields,
                model
                    .complete_read()
                    .fields()
                    .iter()
                    .map(|field| (field.token(), field.multiplicity())),
            )?;
            Ok(ProjectedRolePlayer::try_new_complete_for_hydration(
                installed, role, reference, fields,
            )?)
        }
    }
}

const fn collection_mode(multiplicity: ProjectedMultiplicity) -> ProjectedCollectionMode {
    match multiplicity.collection_mode() {
        CollectionMode::OrderedList => ProjectedCollectionMode::Ordered,
        CollectionMode::Unordered => ProjectedCollectionMode::Unordered,
    }
}

fn require_collection(
    actual: ProjectedCollectionMode,
    expected: ProjectedMultiplicity,
) -> Result<(), ProjectedCodecError> {
    if actual == collection_mode(expected) {
        Ok(())
    } else {
        Err(contract(
            "projected_codec_collection_mode_mismatch",
            "record collection order differs from installed projection",
        )
        .into())
    }
}

fn contract(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new(code).expect("static projected-codec diagnostic code is valid"),
        message,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::id::AttributeId;
    use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, ProjectionHandler};
    use type_bridge_contract::schema::DocumentId;
    use type_bridge_contract::value::{CanonicalString, CanonicalValue};
    use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};

    use crate::projected_model::tests::origin_projection;

    fn identifier(value: &str) -> CanonicalValue {
        CanonicalValue::String(CanonicalString::new(value).unwrap())
    }

    fn codec_projection() -> InstalledRuntimeProjection {
        const SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  middle: { value: string }
  nickname: { value: string }
entities:
  person:
    owns:
      identifier: { key: true }
      middle: { card: { min: 0, max: 1 } }
      nickname: { card: { min: 0, max: 3 } }
structs:
  display:
    fields:
      - { name: primary, type: string }
      - { name: secondary, type: string, optional: true }
"#;
        let documents =
            SchemaDocumentSet::parse([(DocumentId::new("projected-codec.yaml").unwrap(), SCHEMA)])
                .unwrap();
        let resolved = resolve(
            &normalize_documents(&documents).unwrap(),
            &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
        )
        .unwrap();
        let declared = resolved.declared_identity_fingerprint().clone();
        let runtime = project(
            &resolved,
            BindingTarget::Rust,
            &ProjectionConfig::rust(),
            &[ProjectionHandler::rust_v1()],
            &[],
        )
        .unwrap();
        InstalledRuntimeProjection::try_new(runtime)
            .unwrap()
            .with_declared_schema_identity(declared)
    }

    #[test]
    fn scalar_and_reference_round_trip_through_exact_installed_authority() {
        let installed = codec_projection();
        let attribute_type = TypeId::new(TypeKind::Attribute, "identifier").unwrap();
        let scalar =
            ProjectedAttributeValue::try_new(&installed, attribute_type, identifier("ada"))
                .unwrap();
        let scalar_record = record_from_attribute(&installed, &scalar).unwrap();
        assert_eq!(
            materialize_record(&installed, &scalar_record).unwrap(),
            ProjectedCodecValue::Attribute(scalar)
        );

        let person = TypeId::new(TypeKind::Entity, "person").unwrap();
        let key = OwnsFactId::new(person.clone(), AttributeId::new("identifier").unwrap()).unwrap();
        let key_value = ProjectedAttributeValue::try_new(
            &installed,
            TypeId::new(TypeKind::Attribute, "identifier").unwrap(),
            identifier("ada"),
        )
        .unwrap();
        let reference = ProjectedReference::try_new(
            &installed,
            person,
            Some("0x1".to_owned()),
            vec![(key, key_value)],
        )
        .unwrap();
        let reference_record = record_from_reference(&installed, &reference).unwrap();
        assert_eq!(
            materialize_record(&installed, &reference_record).unwrap(),
            ProjectedCodecValue::Reference(reference)
        );
    }

    #[test]
    fn codec_fails_closed_without_declared_authority() {
        let installed = origin_projection();
        let legacy =
            InstalledRuntimeProjection::try_new(installed.projection().as_ref().clone()).unwrap();
        let scalar = ProjectedAttributeValue::try_new(
            &legacy,
            TypeId::new(TypeKind::Attribute, "identifier").unwrap(),
            identifier("ada"),
        )
        .unwrap();
        let error = record_from_attribute(&legacy, &scalar).unwrap_err();
        assert!(error.to_string().contains("declared-schema authority"));
    }

    #[test]
    fn create_round_trip_preserves_absent_versus_present_empty() {
        let installed = codec_projection();
        let person = TypeId::new(TypeKind::Entity, "person").unwrap();
        let identifier_field =
            OwnsFactId::new(person.clone(), AttributeId::new("identifier").unwrap()).unwrap();
        let alias_field =
            OwnsFactId::new(person.clone(), AttributeId::new("nickname").unwrap()).unwrap();
        let identifier_value = ProjectedAttributeValue::try_new(
            &installed,
            TypeId::new(TypeKind::Attribute, "identifier").unwrap(),
            identifier("ada"),
        )
        .unwrap();
        let required = (identifier_field, vec![identifier_value]);
        let absent =
            ProjectedCreate::try_new(&installed, person.clone(), vec![required.clone()], vec![])
                .unwrap();
        let present_empty = ProjectedCreate::try_new(
            &installed,
            person,
            vec![required, (alias_field, vec![])],
            vec![],
        )
        .unwrap();

        let absent_record = record_from_create(&installed, &absent).unwrap();
        let empty_record = record_from_create(&installed, &present_empty).unwrap();
        assert_ne!(
            absent_record.encode().unwrap(),
            empty_record.encode().unwrap()
        );
        let ProjectedCodecValue::Create(decoded_absent) =
            materialize_record(&installed, &absent_record).unwrap()
        else {
            panic!("create record materialized with the wrong kind");
        };
        let ProjectedCodecValue::Create(decoded_empty) =
            materialize_record(&installed, &empty_record).unwrap()
        else {
            panic!("create record materialized with the wrong kind");
        };
        assert_eq!(
            record_from_create(&installed, &decoded_absent).unwrap(),
            absent_record
        );
        assert_eq!(
            record_from_create(&installed, &decoded_empty).unwrap(),
            empty_record
        );
    }

    #[test]
    fn empty_hydrated_scalar_normalizes_to_absent_without_erasing_empty_sequence() {
        let installed = codec_projection();
        let person = TypeId::new(TypeKind::Entity, "person").unwrap();
        let model = installed.projection().models().get(&person).unwrap();
        let field = |name: &str| {
            let id = OwnsFactId::new(person.clone(), AttributeId::new(name).unwrap()).unwrap();
            let read = model
                .complete_read()
                .fields()
                .iter()
                .find(|candidate| candidate.token() == &id)
                .unwrap();
            (id, read.multiplicity())
        };
        let (middle, middle_multiplicity) = field("middle");
        let (nickname, nickname_multiplicity) = field("nickname");

        assert!(matches!(
            projected_field(&person, &middle, middle_multiplicity, true, &[])
                .unwrap()
                .member(),
            ProjectedMemberValues::Absent
        ));
        assert!(matches!(
            projected_field(&person, &nickname, nickname_multiplicity, true, &[])
                .unwrap()
                .member(),
            ProjectedMemberValues::Present { values, .. } if values.is_empty()
        ));
    }

    #[test]
    fn struct_round_trip_preserves_declaration_order_and_optional_absence() {
        let installed = codec_projection();
        let value = ProjectedStructValue::try_new(
            &installed,
            TypeId::new(TypeKind::Struct, "display").unwrap(),
            vec![Some(identifier("Ada")), None],
        )
        .unwrap();
        let record = record_from_struct(&installed, &value).unwrap();
        assert_eq!(
            materialize_record(&installed, &record).unwrap(),
            ProjectedCodecValue::Struct(value)
        );
    }

    #[test]
    fn materialization_rejects_foreign_authority_and_incomplete_member_sets() {
        let installed = codec_projection();
        let foreign = origin_projection();
        let person = TypeId::new(TypeKind::Entity, "person").unwrap();
        let identifier_field =
            OwnsFactId::new(person.clone(), AttributeId::new("identifier").unwrap()).unwrap();
        let identifier_value = ProjectedAttributeValue::try_new(
            &installed,
            TypeId::new(TypeKind::Attribute, "identifier").unwrap(),
            identifier("ada"),
        )
        .unwrap();
        let create = ProjectedCreate::try_new(
            &installed,
            person.clone(),
            vec![(identifier_field, vec![identifier_value])],
            vec![],
        )
        .unwrap();
        let record = record_from_create(&installed, &create).unwrap();
        assert!(materialize_record(&foreign, &record).is_err());

        let ProjectedRecordContent::EntityCreate { fields, .. } = record.content() else {
            panic!("expected entity create record");
        };
        let incomplete = ProjectedRecord::try_new(
            record.semantic_profile().clone(),
            record.declared_schema_identity().clone(),
            ProjectedRecordContent::EntityCreate {
                r#type: person,
                fields: fields[..1].to_vec(),
            },
        )
        .unwrap();
        let error = materialize_record(&installed, &incomplete).unwrap_err();
        assert!(error.to_string().contains("exactly cover"));
    }
}
