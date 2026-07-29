//! Projection-authoritative entity conversion at the generated/client boundary.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error as StdError;

use type_bridge_contract::codec::{from_canonical_json, to_canonical_json};
use type_bridge_contract::id::{TypeId, TypeKind, is_canonical_thing_iid};
use type_bridge_contract::projection::{
    BindingTarget, ModelProjection, ProjectedContainer, ProjectedMultiplicity, ProjectedTypeRef,
    RuntimeProjection,
};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::value::{CanonicalString, ValueTypeTag};
use type_bridge_orm::{
    AttributeValue, DynamicAttributeMap, DynamicEntityRow, EntityDescriptor,
    InstalledRuntimeProjection,
};

use crate::__codegen::{
    CanonicalDouble, Date, DateTime, DateTimeTz, Decimal, Duration, EncodedCreate, EncodedScalar,
    HydratedRow, IntoEncodedCreate, ValidationError,
};
use crate::Result;
use crate::error::{Error, ModelValidationPhase};

pub(crate) fn lower_entity_create<T: IntoEncodedCreate>(
    input: T,
    requested_id: &TypeId,
    installed: &InstalledRuntimeProjection,
) -> Result<DynamicAttributeMap> {
    let encoded = input.into_encoded_create().map_err(map_generated_input)?;
    lower_encoded_entity_create(&encoded, requested_id, installed)
}

pub(crate) fn resolve_entity_authority(
    identity_json: &str,
    installed: &InstalledRuntimeProjection,
    phase: ModelValidationPhase,
    require_constructible: bool,
) -> Result<(TypeId, EntityDescriptor)> {
    let id = from_canonical_json::<TypeId>(identity_json.as_bytes()).map_err(|e| {
        sourced_validation(
            phase,
            "invalid_model_identity",
            vec!["type".into()],
            "generated model identity is not canonical",
            e,
        )
    })?;
    let canonical = canonical_type_identity(&id, phase, vec!["type".into()])?;
    if canonical.as_slice() != identity_json.as_bytes() {
        return Err(validation(
            phase,
            "invalid_model_identity",
            vec!["type".into()],
            "generated model identity is not byte-for-byte canonical",
        ));
    }
    let model = selected_entity_model(identity_json, &id, installed, phase)?;
    if require_constructible
        && (!model.declaration().is_constructible() || !model.create().enabled())
    {
        return Err(validation(
            phase,
            "model_not_constructible",
            vec!["type".into()],
            "the selected entity is not constructible",
        ));
    }
    let descriptor = installed
        .entity_descriptor(&id)
        .map_err(|e| {
            sourced_validation(
                phase,
                "model_not_projected",
                vec!["type".into()],
                "the selected entity descriptor is not installed",
                e,
            )
        })?
        .clone();
    Ok((id, descriptor))
}

pub(crate) fn resolve_discovered_entity(
    type_name: &str,
    installed: &InstalledRuntimeProjection,
) -> Result<(TypeId, EntityDescriptor)> {
    let id = TypeId::new(TypeKind::Entity, type_name).map_err(|e| {
        sourced_validation(
            ModelValidationPhase::Hydration,
            "invalid_discovered_type",
            vec!["type".into()],
            "discovered entity type label is invalid",
            e,
        )
    })?;
    let rendered = to_canonical_json(&id).map_err(|e| {
        sourced_validation(
            ModelValidationPhase::Hydration,
            "invalid_discovered_type",
            vec!["type".into()],
            "discovered entity type cannot be rendered canonically",
            e,
        )
    })?;
    let rendered = std::str::from_utf8(&rendered).map_err(|e| {
        sourced_validation(
            ModelValidationPhase::Hydration,
            "invalid_discovered_type",
            vec!["type".into()],
            "discovered entity identity is not UTF-8",
            e,
        )
    })?;
    let (_, descriptor) =
        resolve_entity_authority(rendered, installed, ModelValidationPhase::Hydration, true)?;
    Ok((id, descriptor))
}

pub(crate) fn map_validation_error(error: ValidationError, phase: ModelValidationPhase) -> Error {
    Error::model_validation(
        phase,
        error.code().to_owned(),
        split_path(error.field()),
        error.to_string(),
        Some(Box::new(error)),
    )
}

fn lower_encoded_entity_create(
    encoded: &EncodedCreate,
    requested_id: &TypeId,
    installed: &InstalledRuntimeProjection,
) -> Result<DynamicAttributeMap> {
    let model = selected_entity_model(
        encoded.type_id_json(),
        requested_id,
        installed,
        ModelValidationPhase::Input,
    )?;
    let descriptor = installed
        .entity_descriptor(requested_id)
        .map_err(|source| {
            sourced_validation(
                ModelValidationPhase::Input,
                "model_not_projected",
                Vec::new(),
                "the requested entity descriptor is not installed",
                source,
            )
        })?;
    if !model.declaration().is_constructible() || !model.create().enabled() {
        return Err(validation(
            ModelValidationPhase::Input,
            "model_not_constructible",
            Vec::new(),
            "the requested entity is not constructible",
        ));
    }
    if !encoded.roles().is_empty() {
        return Err(validation(
            ModelValidationPhase::Input,
            "entity_roles_not_allowed",
            vec!["roles".into()],
            "entity create evidence cannot contain relation roles",
        ));
    }

    lower_projected_create_fields(
        encoded,
        model,
        &descriptor.owned_attributes,
        installed.projection(),
    )
}

pub(crate) fn lower_projected_create_fields(
    encoded: &EncodedCreate,
    model: &ModelProjection,
    descriptor_attributes: &[type_bridge_orm::OwnedAttributeDescriptor],
    projection: &RuntimeProjection,
) -> Result<DynamicAttributeMap> {
    let mut by_declaring = BTreeMap::<Vec<u8>, (&[EncodedScalar], usize)>::new();
    for (index, (identity, values)) in encoded.fields().iter().enumerate() {
        let Some((canonical, token)) = model.query_tokens().fields().values().find_map(|token| {
            let canonical = canonical_owns_identity(
                token.declaring_id(),
                ModelValidationPhase::Input,
                vec![format!("fields[{index}]")],
            )
            .ok()?;
            (canonical.as_slice() == identity.as_bytes()).then_some((canonical, token))
        }) else {
            return Err(validation(
                ModelValidationPhase::Input,
                "unexpected_field_evidence",
                vec![format!("fields[{index}]")],
                "field evidence is not one canonical declaring ownership token of the selected model",
            ));
        };
        let classification = classify_create_membership(
            true,
            model
                .create()
                .fields()
                .iter()
                .any(|field| field.token() == token.id()),
        );
        if classification != "accepted" {
            return Err(validation(
                ModelValidationPhase::Input,
                classification,
                vec![format!("fields[{index}]")],
                "field evidence names a selected-model query token outside the create facet",
            ));
        }
        if by_declaring
            .insert(canonical, (values.as_slice(), index))
            .is_some()
        {
            return Err(validation(
                ModelValidationPhase::Input,
                "duplicate_field_evidence",
                vec![format!("fields[{index}]")],
                "create evidence repeats one declaring ownership identity",
            ));
        }
    }

    let mut output = Vec::new();
    let mut consumed = BTreeSet::new();
    for field in model.create().fields() {
        let token = query_field(model, field.token(), ModelValidationPhase::Input)?;
        if !descriptor_attributes.iter().any(|attribute| {
            attribute.attr_name == token.id().attribute().label().as_str()
                && attribute.field_name == token.target_name().as_str()
        }) {
            return Err(validation(
                ModelValidationPhase::Input,
                "invalid_installed_projection",
                Vec::new(),
                "projected ownership is absent from the installed provider descriptor",
            ));
        }
        let declaring_json = canonical_owns_identity(
            token.declaring_id(),
            ModelValidationPhase::Input,
            Vec::new(),
        )?;
        let path = vec![token.target_name().as_str().to_owned()];
        let values = match by_declaring.get(&declaring_json) {
            Some((values, index)) => {
                consumed.insert(*index);
                *values
            }
            None => &[],
        };
        enforce_cardinality(
            values.len(),
            field.multiplicity(),
            ModelValidationPhase::Input,
            path.clone(),
        )?;
        let domain = projected_domain(projection, field.value(), ModelValidationPhase::Input)?;
        for (index, value) in values.iter().enumerate() {
            let value_path = indexed_path(&path, field.multiplicity(), index);
            output.push((
                token.id().attribute().label().as_str().to_owned(),
                lower_scalar(value, domain, ModelValidationPhase::Input, value_path)?,
            ));
        }
    }

    if consumed.len() != by_declaring.len() {
        let (_, (_, index)) = by_declaring
            .iter()
            .find(|(_, (_, index))| !consumed.contains(index))
            .expect("unequal lengths imply one unconsumed create field");
        return Err(validation(
            ModelValidationPhase::Input,
            "unexpected_field_evidence",
            vec![format!("fields[{index}]")],
            "create evidence contains an identity outside the selected create facet",
        ));
    }
    Ok(output)
}

pub(crate) fn hydrate_entity(
    row: DynamicEntityRow,
    requested_id: &TypeId,
    installed: &InstalledRuntimeProjection,
) -> Result<HydratedRow> {
    let identity_json =
        canonical_type_identity(requested_id, ModelValidationPhase::Hydration, Vec::new())?;
    let model = selected_entity_model(
        std::str::from_utf8(&identity_json).expect("canonical JSON is UTF-8"),
        requested_id,
        installed,
        ModelValidationPhase::Hydration,
    )?;
    if !model.declaration().is_constructible() {
        return Err(validation(
            ModelValidationPhase::Hydration,
            "model_not_constructible",
            Vec::new(),
            "the requested entity cannot be a concrete hydration result",
        ));
    }
    let descriptor = installed
        .entity_descriptor(requested_id)
        .map_err(|source| {
            sourced_validation(
                ModelValidationPhase::Hydration,
                "model_not_projected",
                Vec::new(),
                "the requested entity descriptor is not installed",
                source,
            )
        })?;

    let iid = row.iid.ok_or_else(|| {
        validation(
            ModelValidationPhase::Hydration,
            "missing_iid",
            vec!["iid".into()],
            "provider entity row omitted its IID",
        )
    })?;
    if !is_canonical_thing_iid(&iid) {
        return Err(validation(
            ModelValidationPhase::Hydration,
            "noncanonical_iid",
            vec!["iid".into()],
            "provider entity row contains a noncanonical IID",
        ));
    }
    let type_name = row.type_name.ok_or_else(|| {
        validation(
            ModelValidationPhase::Hydration,
            "missing_concrete_type",
            vec!["type".into()],
            "provider entity row omitted its concrete type",
        )
    })?;
    if type_name != descriptor.type_name || type_name != requested_id.label().as_str() {
        return Err(validation(
            ModelValidationPhase::Hydration,
            "wrong_concrete_type",
            vec!["type".into()],
            "provider entity row has the wrong exact concrete type",
        ));
    }

    let fields = hydrate_projected_read_fields(&row.attributes, model, installed.projection())?;
    Ok(HydratedRow::from_owned(
        String::from_utf8(identity_json).expect("canonical JSON is UTF-8"),
        iid,
        fields,
        Vec::new(),
    ))
}

pub(crate) fn hydrate_projected_read_fields(
    attributes: &[(String, AttributeValue)],
    model: &ModelProjection,
    projection: &RuntimeProjection,
) -> Result<Vec<(String, Vec<EncodedScalar>)>> {
    let mut by_attribute =
        BTreeMap::<&str, Vec<&type_bridge_contract::projection::ReadFieldProjection>>::new();
    for field in model.complete_read().fields() {
        by_attribute
            .entry(field.token().attribute().label().as_str())
            .or_default()
            .push(field);
    }
    let mut evidence = BTreeMap::<&str, Vec<AttributeValue>>::new();
    for (index, (attribute, value)) in attributes.iter().enumerate() {
        let candidates = by_attribute
            .get(attribute.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        require_unambiguous_provider_attribute(attribute, index, candidates.len())?;
        evidence.entry(attribute).or_default().push(value.clone());
    }

    let mut fields = Vec::new();
    for field in model.complete_read().fields() {
        let token = query_field(model, field.token(), ModelValidationPhase::Hydration)?;
        let provider_name = token.id().attribute().label().as_str();
        let values = evidence.remove(provider_name).unwrap_or_default();
        let path = vec![token.target_name().as_str().to_owned()];
        enforce_cardinality(
            values.len(),
            field.multiplicity(),
            ModelValidationPhase::Hydration,
            path.clone(),
        )?;
        let domain = projected_domain(projection, field.value(), ModelValidationPhase::Hydration)?;
        let mut encoded = Vec::with_capacity(values.len());
        for (index, value) in values.iter().enumerate() {
            encoded.push(hydrate_scalar(
                value,
                domain,
                indexed_path(&path, field.multiplicity(), index),
            )?);
        }
        if !encoded.is_empty() {
            fields.push((
                String::from_utf8(canonical_owns_identity(
                    token.declaring_id(),
                    ModelValidationPhase::Hydration,
                    path,
                )?)
                .expect("canonical JSON is UTF-8"),
                encoded,
            ));
        }
    }
    Ok(fields)
}

fn require_unambiguous_provider_attribute(
    attribute: &str,
    index: usize,
    candidate_count: usize,
) -> Result<()> {
    if candidate_count == 0 {
        return Err(validation(
            ModelValidationPhase::Hydration,
            "unexpected_provider_attribute",
            vec![format!("attributes[{index}]")],
            format!("provider row contains unprojected attribute '{attribute}'"),
        ));
    }
    if candidate_count != 1 {
        return Err(validation(
            ModelValidationPhase::Hydration,
            "ambiguous_provider_attribute",
            vec![format!("attributes[{index}]")],
            format!("provider attribute '{attribute}' has ambiguous projected ownership"),
        ));
    }
    Ok(())
}

fn classify_create_membership(is_query_token: bool, is_create_field: bool) -> &'static str {
    match (is_query_token, is_create_field) {
        (true, true) => "accepted",
        (true, false) => "non_create_field_evidence",
        (false, _) => "unexpected_field_evidence",
    }
}

fn selected_entity_model<'a>(
    identity_json: &str,
    requested_id: &TypeId,
    installed: &'a InstalledRuntimeProjection,
    phase: ModelValidationPhase,
) -> Result<&'a ModelProjection> {
    if installed.projection().target() != BindingTarget::Rust {
        return Err(validation(
            phase,
            "model_not_projected",
            vec!["type".into()],
            "the installed projection is not a Rust projection",
        ));
    }
    let decoded = from_canonical_json::<TypeId>(identity_json.as_bytes()).map_err(|source| {
        sourced_validation(
            phase,
            "invalid_model_identity",
            vec!["type".into()],
            "generated model identity is not one canonical TypeId",
            source,
        )
    })?;
    let rendered = canonical_type_identity(&decoded, phase, vec!["type".into()])?;
    if rendered != identity_json.as_bytes() {
        return Err(validation(
            phase,
            "invalid_model_identity",
            vec!["type".into()],
            "generated model identity is not byte-for-byte canonical",
        ));
    }
    if &decoded != requested_id {
        return Err(validation(
            phase,
            "wrong_model_identity",
            vec!["type".into()],
            "generated and requested model identities differ",
        ));
    }
    if decoded.kind() != TypeKind::Entity {
        return Err(validation(
            phase,
            "wrong_model_kind",
            vec!["type".into()],
            "the selected model is not an entity",
        ));
    }
    installed
        .projection()
        .models()
        .get(&decoded)
        .ok_or_else(|| {
            validation(
                phase,
                "model_not_projected",
                vec!["type".into()],
                "the selected entity is absent from the installed Rust projection",
            )
        })
}

fn query_field<'a>(
    model: &'a ModelProjection,
    id: &OwnsFactId,
    phase: ModelValidationPhase,
) -> Result<&'a type_bridge_contract::projection::FieldTokenProjection> {
    model.query_tokens().fields().get(id).ok_or_else(|| {
        validation(
            phase,
            "invalid_installed_projection",
            Vec::new(),
            "selected model facet refers to an absent ownership token",
        )
    })
}

pub(crate) fn projected_domain(
    projection: &RuntimeProjection,
    value: &ProjectedTypeRef,
    phase: ModelValidationPhase,
) -> Result<ValueTypeTag> {
    match value {
        ProjectedTypeRef::Scalar(domain) => Ok(*domain),
        ProjectedTypeRef::Model(model)
            if model.id().kind() == TypeKind::Attribute
                && projection
                    .models()
                    .get(model.id())
                    .and_then(|attribute| attribute.declaration().value_type())
                    .is_some() =>
        {
            Ok(projection
                .models()
                .get(model.id())
                .and_then(|attribute| attribute.declaration().value_type())
                .expect("guard proves an attribute scalar domain"))
        }
        ProjectedTypeRef::Model(_) | ProjectedTypeRef::Struct(_) => Err(validation(
            phase,
            "invalid_installed_projection",
            Vec::new(),
            "entity ownership does not resolve to a scalar attribute domain",
        )),
    }
}

fn enforce_cardinality(
    count: usize,
    multiplicity: ProjectedMultiplicity,
    phase: ModelValidationPhase,
    path: Vec<String>,
) -> Result<()> {
    let count = u64::try_from(count).map_err(|source| {
        sourced_validation(
            phase,
            "cardinality_overflow",
            path.clone(),
            "field evidence length exceeds the supported cardinality domain",
            source,
        )
    })?;
    let cardinality = multiplicity.cardinality();
    if count == 0 && cardinality.min() > 0 {
        return Err(validation(
            phase,
            "missing_field_evidence",
            path,
            "required field evidence is absent",
        ));
    }
    if cardinality.max() == Some(1) && count > 1 {
        return Err(validation(
            phase,
            "duplicate_scalar_evidence",
            path,
            "scalar field contains duplicate evidence",
        ));
    }
    if count < cardinality.min() || cardinality.max().is_some_and(|max| count > max) {
        return Err(validation(
            phase,
            "cardinality_violation",
            path,
            "field evidence violates its exact projected cardinality",
        ));
    }
    Ok(())
}

pub(crate) fn lower_scalar(
    value: &EncodedScalar,
    domain: ValueTypeTag,
    phase: ModelValidationPhase,
    path: Vec<String>,
) -> Result<AttributeValue> {
    let converted = match (domain, value) {
        (ValueTypeTag::String, EncodedScalar::String(value)) => {
            CanonicalString::new(value).map_err(|source| {
                sourced_validation(
                    phase,
                    "string_limit_exceeded",
                    path.clone(),
                    "string value is outside the canonical scalar envelope",
                    source,
                )
            })?;
            AttributeValue::String(value.clone())
        }
        (ValueTypeTag::Long, EncodedScalar::Long(value)) => AttributeValue::Long(*value),
        (ValueTypeTag::Double, EncodedScalar::Double(value)) => AttributeValue::Double(value.get()),
        (ValueTypeTag::Boolean, EncodedScalar::Boolean(value)) => AttributeValue::Boolean(*value),
        (ValueTypeTag::Date, EncodedScalar::Date(value)) => {
            AttributeValue::Date(value.as_str().to_owned())
        }
        (ValueTypeTag::DateTime, EncodedScalar::DateTime(value)) => {
            AttributeValue::DateTime(value.as_str().to_owned())
        }
        (ValueTypeTag::DateTimeTz, EncodedScalar::DateTimeTz(value)) => {
            AttributeValue::DateTimeTZ(value.as_str().to_owned())
        }
        (ValueTypeTag::Decimal, EncodedScalar::Decimal(value)) => {
            AttributeValue::Decimal(value.as_str().to_owned())
        }
        (ValueTypeTag::Duration, EncodedScalar::Duration(value)) => {
            AttributeValue::Duration(value.as_str().to_owned())
        }
        _ => {
            return Err(validation(
                phase,
                "wrong_scalar_domain",
                path,
                "scalar variant does not match the projected attribute domain",
            ));
        }
    };
    Ok(converted)
}

pub(crate) fn hydrate_scalar(
    value: &AttributeValue,
    domain: ValueTypeTag,
    path: Vec<String>,
) -> Result<EncodedScalar> {
    let phase = ModelValidationPhase::Hydration;
    match (domain, value) {
        (ValueTypeTag::String, AttributeValue::String(value)) => {
            CanonicalString::new(value).map_err(|source| {
                sourced_validation(
                    phase,
                    "string_limit_exceeded",
                    path.clone(),
                    "provider string is outside the canonical scalar envelope",
                    source,
                )
            })?;
            Ok(EncodedScalar::String(value.clone()))
        }
        (ValueTypeTag::Long, AttributeValue::Long(value)) => Ok(EncodedScalar::Long(*value)),
        (ValueTypeTag::Double, AttributeValue::Double(value)) => CanonicalDouble::try_new(*value)
            .map(EncodedScalar::Double)
            .map_err(|source| map_scalar_validation(source, path)),
        (ValueTypeTag::Boolean, AttributeValue::Boolean(value)) => {
            Ok(EncodedScalar::Boolean(*value))
        }
        (ValueTypeTag::Date, AttributeValue::Date(value)) => Date::try_new(value)
            .map(EncodedScalar::Date)
            .map_err(|source| map_scalar_validation(source, path)),
        (ValueTypeTag::DateTime, AttributeValue::DateTime(value)) => DateTime::try_new(value)
            .map(EncodedScalar::DateTime)
            .map_err(|source| map_scalar_validation(source, path)),
        (ValueTypeTag::DateTimeTz, AttributeValue::DateTimeTZ(value)) => DateTimeTz::try_new(value)
            .map(EncodedScalar::DateTimeTz)
            .map_err(|source| map_scalar_validation(source, path)),
        (ValueTypeTag::Decimal, AttributeValue::Decimal(value)) => Decimal::try_new(value)
            .map(EncodedScalar::Decimal)
            .map_err(|source| map_scalar_validation(source, path)),
        (ValueTypeTag::Duration, AttributeValue::Duration(value)) => Duration::try_new(value)
            .map(EncodedScalar::Duration)
            .map_err(|source| map_scalar_validation(source, path)),
        _ => Err(validation(
            phase,
            "wrong_scalar_domain",
            path,
            "provider scalar variant does not match the projected attribute domain",
        )),
    }
}

fn map_generated_input(source: ValidationError) -> Error {
    let code = source.code().to_owned();
    let path = split_path(source.field());
    let message = source.to_string();
    Error::model_validation(
        ModelValidationPhase::Input,
        code,
        path,
        message,
        Some(Box::new(source)),
    )
}

fn map_scalar_validation(source: ValidationError, path: Vec<String>) -> Error {
    let code = source.code().to_owned();
    let message = source.to_string();
    Error::model_validation(
        ModelValidationPhase::Hydration,
        code,
        path,
        message,
        Some(Box::new(source)),
    )
}

pub(crate) fn canonical_type_identity(
    value: &TypeId,
    phase: ModelValidationPhase,
    path: Vec<String>,
) -> Result<Vec<u8>> {
    to_canonical_json(value).map_err(|source| {
        sourced_validation(
            phase,
            "invalid_model_identity",
            path,
            "projection identity cannot be rendered canonically",
            source,
        )
    })
}

pub(crate) fn canonical_owns_identity(
    value: &OwnsFactId,
    phase: ModelValidationPhase,
    path: Vec<String>,
) -> Result<Vec<u8>> {
    to_canonical_json(value).map_err(|source| {
        sourced_validation(
            phase,
            "invalid_field_identity",
            path,
            "projection ownership identity cannot be rendered canonically",
            source,
        )
    })
}

fn indexed_path(path: &[String], multiplicity: ProjectedMultiplicity, index: usize) -> Vec<String> {
    let mut path = path.to_vec();
    if multiplicity.container() == ProjectedContainer::Sequence
        && let Some(last) = path.last_mut()
    {
        last.push_str(&format!("[{index}]"));
    }
    path
}

fn split_path(path: &str) -> Vec<String> {
    if path.is_empty() {
        Vec::new()
    } else {
        path.split('.').map(str::to_owned).collect()
    }
}

fn validation(
    phase: ModelValidationPhase,
    code: &'static str,
    path: Vec<String>,
    message: impl Into<String>,
) -> Error {
    Error::model_validation(phase, code, path, message, None)
}

fn sourced_validation(
    phase: ModelValidationPhase,
    code: &'static str,
    path: Vec<String>,
    message: impl Into<String>,
    source: impl StdError + Send + Sync + 'static,
) -> Error {
    Error::model_validation(phase, code, path, message, Some(Box::new(source)))
}

#[cfg(test)]
mod tests;
