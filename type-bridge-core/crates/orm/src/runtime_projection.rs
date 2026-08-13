//! Provider descriptors derived one-way from a trusted runtime projection.

use std::collections::BTreeMap;
use std::sync::Arc;

use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::projection::{ModelProjection, ProjectedAnnotation, RuntimeProjection};
use type_bridge_contract::schema::{AnnotationKindId, SchemaAnnotationValue};
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticDetailValue, SdkDiagnosticMessage, SdkDiagnosticName,
    SdkDiagnosticPathSegment, SdkExecutionDiagnostic,
};
use type_bridge_contract::temporal::{
    CanonicalDate, CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration, TimeZoneDesignator,
};
use type_bridge_contract::value::{
    CanonicalDouble, CanonicalString, CanonicalValue, Cardinality, DecimalValue, ValueTypeTag,
};

use crate::_attribute::ValueType;
use crate::_descriptor::{
    EntityDescriptor, OwnedAttributeDescriptor, RelationDescriptor, RoleDescriptor, TypeDescriptor,
};
use crate::_dynamic::DynamicAttributeMap;
use crate::_entity::Annotation;
use crate::_registry::DescriptorRegistry;
use crate::error::{OrmError, Result};
use crate::value::AttributeValue;

/// One package-scoped trusted projection and its provider-facing descriptors.
pub struct InstalledRuntimeProjection {
    projection: Arc<RuntimeProjection>,
    descriptors: BTreeMap<TypeId, TypeDescriptor>,
}

impl InstalledRuntimeProjection {
    /// Derive provider descriptors without registering them in the global V1 registry.
    pub fn try_new(projection: RuntimeProjection) -> Result<Self> {
        let mut descriptors = BTreeMap::new();
        for (id, model) in projection.models() {
            let descriptor = match id.kind() {
                TypeKind::Entity => TypeDescriptor::Entity(entity_descriptor(&projection, model)?),
                TypeKind::Relation => {
                    TypeDescriptor::Relation(relation_descriptor(&projection, model)?)
                }
                TypeKind::Attribute | TypeKind::Struct => continue,
            };
            descriptors.insert(id.clone(), descriptor);
        }
        Ok(Self {
            projection: Arc::new(projection),
            descriptors,
        })
    }

    /// Decode a verified Rust runtime projection from JSON bytes and derive provider descriptors.
    pub fn from_verified_rust_json(
        runtime_json: &[u8],
        semantic_json: &[u8],
        projection_json: &[u8],
    ) -> Result<Self> {
        let runtime = type_bridge_contract::projection_wire::decode_runtime_projection_verified(
            runtime_json,
            semantic_json,
            projection_json,
        )
        .map_err(|err| OrmError::DescriptorValidation {
            type_name: "<runtime_projection>".to_owned(),
            message: err.to_string(),
        })?;
        if runtime.target() != type_bridge_contract::projection::BindingTarget::Rust {
            return Err(OrmError::DescriptorValidation {
                type_name: "<runtime_projection>".to_owned(),
                message: format!(
                    "target mismatch: expected Rust binding target, found {:?}",
                    runtime.target()
                ),
            });
        }
        Self::try_new(runtime)
    }

    /// Return the trusted source projection.
    pub fn projection(&self) -> &Arc<RuntimeProjection> {
        &self.projection
    }

    /// Resolve one provider descriptor by its kind-qualified identity.
    pub fn descriptor(&self, id: &TypeId) -> Result<&TypeDescriptor> {
        self.descriptors.get(id).ok_or_else(|| {
            OrmError::DescriptorNotFound(format!(
                "{}:{}",
                kind_name(id.kind()),
                id.label().as_str()
            ))
        })
    }

    /// Resolve one entity descriptor.
    pub fn entity_descriptor(&self, id: &TypeId) -> Result<&EntityDescriptor> {
        match self.descriptor(id)? {
            TypeDescriptor::Entity(value) => Ok(value),
            TypeDescriptor::Relation(_) => Err(kind_conflict(id, "entity")),
        }
    }

    /// Resolve one relation descriptor.
    pub fn relation_descriptor(&self, id: &TypeId) -> Result<&RelationDescriptor> {
        match self.descriptor(id)? {
            TypeDescriptor::Relation(value) => Ok(value),
            TypeDescriptor::Entity(_) => Err(kind_conflict(id, "relation")),
        }
    }

    /// Build the exact match registry for this installed generated
    /// projection.
    ///
    /// The registry carries its trusted projection provenance into prepared
    /// execution snapshots so generated-client requests use the direct typed
    /// executor instead of reconstructing schema authority from lossy dynamic
    /// descriptors.
    pub fn match_registry(&self) -> Result<DescriptorRegistry> {
        let registry = DescriptorRegistry::for_installed_projection(
            self.projection.semantic_fingerprint().clone(),
            self.projection.target(),
            self.projection.projection_fingerprint().clone(),
            self.projection.functions().clone(),
        );
        for descriptor in self.descriptors.values().cloned() {
            match descriptor {
                TypeDescriptor::Entity(entity) => {
                    registry.register_entity(entity)?;
                }
                TypeDescriptor::Relation(relation) => {
                    registry.register_relation(relation)?;
                }
            }
        }
        Ok(registry)
    }

    /// Decode one fetched role player's raw provider attribute arrays against
    /// this projection's descriptor set.
    pub fn role_player_attributes(
        &self,
        id: &TypeId,
        values: &[(String, serde_json::Value)],
    ) -> Result<DynamicAttributeMap> {
        let descriptors = match self.descriptor(id)? {
            TypeDescriptor::Entity(descriptor) => &descriptor.owned_attributes,
            TypeDescriptor::Relation(descriptor) => &descriptor.owned_attributes,
        };
        decode_role_player_attributes(id, descriptors, values)
    }

    /// Validate one generated attribute scalar against its effective value annotations.
    pub fn validate_attribute_value(&self, id: &TypeId, value: &AttributeValue) -> Result<()> {
        let model = self.projection.models().get(id).ok_or_else(|| {
            OrmError::DescriptorNotFound(format!(
                "{}:{}",
                kind_name(id.kind()),
                id.label().as_str()
            ))
        })?;
        if id.kind() != TypeKind::Attribute {
            return Err(descriptor_error(
                id,
                "generated scalar validation requires an attribute type",
            ));
        }
        if !model
            .declaration()
            .value_annotations()
            .values()
            .any(|annotation| is_value_constraint(annotation.value()))
        {
            return Ok(());
        }
        let canonical = canonical_attribute_value(value)
            .map_err(|code| projected_value_error(id, "value", code))?;
        self.validate_canonical_attribute_value(id, &canonical)
            .map_err(|diagnostic| projected_diagnostic_error(id, "value", diagnostic))
    }

    /// Validate one generated owned-field scalar against attribute and ownership constraints.
    pub fn validate_field_value(
        &self,
        id: &TypeId,
        target_name: &str,
        value: &AttributeValue,
    ) -> Result<()> {
        let model = self.projection.models().get(id).ok_or_else(|| {
            OrmError::DescriptorNotFound(format!(
                "{}:{}",
                kind_name(id.kind()),
                id.label().as_str()
            ))
        })?;
        let field = model
            .query_tokens()
            .fields()
            .values()
            .find(|field| field.target_name().as_str() == target_name)
            .ok_or_else(|| {
                descriptor_error(id, "generated value references an unknown projected field")
            })?;
        let attribute_id =
            TypeId::new(TypeKind::Attribute, field.id().attribute().label().as_str())
                .map_err(contract_error)?;
        self.validate_attribute_value(&attribute_id, value)?;
        if !field
            .annotations()
            .values()
            .any(|annotation| is_value_constraint(annotation.value()))
        {
            return Ok(());
        }
        let canonical = canonical_attribute_value(value)
            .map_err(|code| projected_value_error(id, target_name, code))?;
        self.validate_canonical_field_value(id, field.id(), &canonical)
            .map_err(|diagnostic| projected_diagnostic_error(id, target_name, diagnostic))
    }

    /// Validate one canonical scalar against an exact projected attribute type.
    pub fn validate_canonical_attribute_value(
        &self,
        id: &TypeId,
        value: &CanonicalValue,
    ) -> std::result::Result<(), SdkExecutionDiagnostic> {
        let path = vec![SdkDiagnosticPathSegment::Type(id.clone())];
        let model = self.projection.models().get(id).ok_or_else(|| {
            invalid_input(
                "attribute_not_projected",
                "The attribute type is absent from the installed runtime projection",
                path.clone(),
            )
        })?;
        if id.kind() != TypeKind::Attribute {
            return Err(invalid_input(
                "wrong_attribute_kind",
                "Canonical scalar validation requires an attribute type",
                path,
            ));
        }
        let expected = model.declaration().value_type().ok_or_else(|| {
            integrity(
                "projected_attribute_domain_missing",
                "The projected attribute omits its canonical scalar domain",
                vec![SdkDiagnosticPathSegment::Type(id.clone())],
            )
        })?;
        if value.value_type() != expected {
            return Err(invalid_input(
                "wrong_scalar_domain",
                "The canonical scalar belongs to a different attribute domain",
                vec![SdkDiagnosticPathSegment::Type(id.clone())],
            )
            .try_with_detail(
                sdk_name("expected_value_type"),
                SdkDiagnosticDetailValue::ValueType(expected),
            )
            .expect("the static scalar-domain detail is unique")
            .try_with_detail(
                sdk_name("actual_value_type"),
                SdkDiagnosticDetailValue::ValueType(value.value_type()),
            )
            .expect("the static scalar-domain details fit the contract"));
        }
        validate_canonical_annotations(
            value,
            model.declaration().value_annotations().values(),
            vec![SdkDiagnosticPathSegment::Type(id.clone())],
        )
    }

    /// Validate one canonical scalar against an exact owner-branded field token.
    pub fn validate_canonical_field_value(
        &self,
        owner: &TypeId,
        field_id: &type_bridge_contract::schema::OwnsFactId,
        value: &CanonicalValue,
    ) -> std::result::Result<(), SdkExecutionDiagnostic> {
        let path = vec![
            SdkDiagnosticPathSegment::Type(owner.clone()),
            SdkDiagnosticPathSegment::Field(field_id.clone()),
        ];
        if field_id.owner() != owner {
            return Err(invalid_input(
                "field_owner_mismatch",
                "The ownership token belongs to a different projected model",
                path,
            ));
        }
        let model = self.projection.models().get(owner).ok_or_else(|| {
            invalid_input(
                "model_not_projected",
                "The model type is absent from the installed runtime projection",
                path.clone(),
            )
        })?;
        let field = model.query_tokens().fields().get(field_id).ok_or_else(|| {
            invalid_input(
                "field_not_projected",
                "The ownership token is absent from the selected projected model",
                path.clone(),
            )
        })?;
        let attribute_id =
            TypeId::new(TypeKind::Attribute, field.id().attribute().label().as_str()).map_err(
                |_| {
                    integrity(
                        "projected_attribute_identity_invalid",
                        "The projected ownership has an invalid attribute identity",
                        path.clone(),
                    )
                },
            )?;
        self.validate_canonical_attribute_value(&attribute_id, value)
            .map_err(|diagnostic| replace_sdk_path(diagnostic, path.clone()))?;
        validate_canonical_annotations(value, field.annotations().values(), path)
    }
}

const fn is_value_constraint(value: &SchemaAnnotationValue) -> bool {
    matches!(
        value,
        SchemaAnnotationValue::Regex(_)
            | SchemaAnnotationValue::Range(_)
            | SchemaAnnotationValue::Values(_)
    )
}

fn validate_canonical_annotations<'a>(
    value: &CanonicalValue,
    annotations: impl IntoIterator<Item = &'a ProjectedAnnotation>,
    path: Vec<SdkDiagnosticPathSegment>,
) -> std::result::Result<(), SdkExecutionDiagnostic> {
    for annotation in annotations {
        match annotation.value() {
            SchemaAnnotationValue::Regex(pattern) => {
                let CanonicalValue::String(text) = value else {
                    return Err(integrity(
                        "projected_regex_domain_mismatch",
                        "The installed regular-expression constraint has a different scalar domain",
                        path,
                    ));
                };
                let expression = regex::Regex::new(pattern.as_str()).map_err(|_| {
                    integrity(
                        "invalid_projected_regex",
                        "The installed projection contains an invalid regular expression",
                        path.clone(),
                    )
                })?;
                if !expression.is_match(text.as_str()) {
                    return Err(invalid_input(
                        "regex_constraint_violation",
                        "The canonical scalar violates a projected regular expression",
                        path,
                    ));
                }
            }
            SchemaAnnotationValue::Range(range) => {
                if let Some(lower) = range.lower() {
                    let ordering = value.semantic_cmp_same_domain(lower).ok_or_else(|| {
                        integrity(
                            "projected_range_domain_mismatch",
                            "The installed range constraint has a different scalar domain",
                            path.clone(),
                        )
                    })?;
                    if ordering == std::cmp::Ordering::Less {
                        return Err(invalid_input(
                            "range_constraint_violation",
                            "The canonical scalar is outside a projected range",
                            path,
                        ));
                    }
                }
                if let Some(upper) = range.upper() {
                    let ordering = value.semantic_cmp_same_domain(upper).ok_or_else(|| {
                        integrity(
                            "projected_range_domain_mismatch",
                            "The installed range constraint has a different scalar domain",
                            path.clone(),
                        )
                    })?;
                    if ordering == std::cmp::Ordering::Greater {
                        return Err(invalid_input(
                            "range_constraint_violation",
                            "The canonical scalar is outside a projected range",
                            path,
                        ));
                    }
                }
            }
            SchemaAnnotationValue::Values(allowed) => {
                if allowed
                    .iter()
                    .any(|candidate| candidate.value_type() != value.value_type())
                {
                    return Err(integrity(
                        "projected_values_domain_mismatch",
                        "The installed allowed-values constraint has a different scalar domain",
                        path,
                    ));
                }
                let accepted = allowed.iter().any(|candidate| {
                    value.semantic_cmp_same_domain(candidate) == Some(std::cmp::Ordering::Equal)
                        || value == candidate
                });
                if !accepted {
                    return Err(invalid_input(
                        "values_constraint_violation",
                        "The canonical scalar is absent from the projected allowed values",
                        path,
                    ));
                }
            }
            SchemaAnnotationValue::Presence
            | SchemaAnnotationValue::Cardinality(_)
            | SchemaAnnotationValue::Doc(_)
            | SchemaAnnotationValue::Meta(_) => {}
        }
    }
    Ok(())
}

fn invalid_input(
    code: &'static str,
    message: &'static str,
    path: Vec<SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    sdk_path(
        SdkExecutionDiagnostic::invalid_input(sdk_code(code), sdk_message(message)),
        path,
    )
}

fn integrity(
    code: &'static str,
    message: &'static str,
    path: Vec<SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    sdk_path(
        SdkExecutionDiagnostic::integrity(sdk_code(code), sdk_message(message)),
        path,
    )
}

fn replace_sdk_path(
    diagnostic: SdkExecutionDiagnostic,
    path: Vec<SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    let mut replaced = match diagnostic.category() {
        type_bridge_contract::sdk_diagnostic::SdkDiagnosticCategory::InvalidInput => {
            SdkExecutionDiagnostic::invalid_input(diagnostic.code().clone(), diagnostic.message())
        }
        type_bridge_contract::sdk_diagnostic::SdkDiagnosticCategory::Integrity => {
            SdkExecutionDiagnostic::integrity(diagnostic.code().clone(), diagnostic.message())
        }
        _ => return diagnostic,
    };
    for (key, value) in diagnostic.details() {
        replaced = replaced
            .try_with_detail(key.clone(), value.clone())
            .expect("validated SDK diagnostic details remain bounded and unique");
    }
    sdk_path(replaced, path)
}

fn sdk_path(
    mut diagnostic: SdkExecutionDiagnostic,
    path: Vec<SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    for segment in path {
        diagnostic = diagnostic
            .try_at(segment)
            .expect("projected-value diagnostic paths fit the SDK contract");
    }
    diagnostic
}

fn sdk_code(value: &'static str) -> SdkDiagnosticCode {
    SdkDiagnosticCode::new(value).expect("static projected-value diagnostic code is canonical")
}

fn sdk_message(value: &'static str) -> SdkDiagnosticMessage {
    SdkDiagnosticMessage::new(value).expect("static projected-value diagnostic message is valid")
}

fn sdk_name(value: &'static str) -> SdkDiagnosticName {
    SdkDiagnosticName::new(value).expect("static projected-value diagnostic name is canonical")
}

pub(crate) fn canonical_attribute_value(
    value: &AttributeValue,
) -> std::result::Result<CanonicalValue, &'static str> {
    match value {
        AttributeValue::String(value) => CanonicalString::new(value.clone())
            .map(CanonicalValue::String)
            .map_err(|_| "wrong_scalar_domain"),
        AttributeValue::Long(value) => Ok(CanonicalValue::Long(*value)),
        AttributeValue::Double(value) => CanonicalDouble::new(*value)
            .map(CanonicalValue::Double)
            .map_err(|_| "wrong_scalar_domain"),
        AttributeValue::Boolean(value) => Ok(CanonicalValue::Boolean(*value)),
        AttributeValue::Date(value) => value
            .parse::<CanonicalDate>()
            .map(CanonicalValue::Date)
            .map_err(|_| "wrong_scalar_domain"),
        AttributeValue::DateTime(value) => value
            .parse::<CanonicalDateTime>()
            .map(CanonicalValue::DateTime)
            .map_err(|_| "wrong_scalar_domain"),
        AttributeValue::DateTimeTZ(value) => {
            type_bridge_schema::parse_provider_datetime_tz_evidence(value)
                .map(CanonicalValue::DateTimeTz)
                .map_err(|_| "wrong_scalar_domain")
        }
        AttributeValue::Decimal(value) => DecimalValue::new(value)
            .map(CanonicalValue::Decimal)
            .map_err(|_| "wrong_scalar_domain"),
        AttributeValue::Duration(value) => value
            .parse::<CanonicalDuration>()
            .map(CanonicalValue::Duration)
            .map_err(|_| "wrong_scalar_domain"),
    }
}

pub(crate) fn attribute_value_from_canonical(value: &CanonicalValue) -> AttributeValue {
    match value {
        CanonicalValue::String(value) => AttributeValue::String(value.as_str().to_owned()),
        CanonicalValue::Long(value) => AttributeValue::Long(*value),
        CanonicalValue::Double(value) => AttributeValue::Double(value.get()),
        CanonicalValue::Boolean(value) => AttributeValue::Boolean(*value),
        CanonicalValue::Date(value) => AttributeValue::Date(value.to_string()),
        CanonicalValue::DateTime(value) => AttributeValue::DateTime(value.to_string()),
        CanonicalValue::DateTimeTz(value) => {
            AttributeValue::DateTimeTZ(datetime_tz_evidence(value))
        }
        CanonicalValue::Decimal(value) => AttributeValue::Decimal(value.as_str().to_owned()),
        CanonicalValue::Duration(value) => AttributeValue::Duration(value.to_string()),
    }
}

fn datetime_tz_evidence(value: &CanonicalDateTimeTz) -> String {
    let TimeZoneDesignator::Named(name) = value.zone() else {
        return value.to_string();
    };
    format!(
        "{}{}[{name}]",
        value.local(),
        canonical_offset(value.effective_offset_seconds())
    )
}

fn canonical_offset(seconds: i32) -> String {
    if seconds == 0 {
        return "Z".to_owned();
    }
    let sign = if seconds < 0 { '-' } else { '+' };
    let absolute = seconds.unsigned_abs();
    let hours = absolute / 3_600;
    let minutes = absolute % 3_600 / 60;
    let seconds = absolute % 60;
    if seconds == 0 {
        format!("{sign}{hours:02}:{minutes:02}")
    } else {
        format!("{sign}{hours:02}:{minutes:02}:{seconds:02}")
    }
}

fn projected_value_error(id: &TypeId, path: &str, code: &str) -> OrmError {
    OrmError::DescriptorValidation {
        type_name: id.label().as_str().to_owned(),
        message: format!("{code} at {path}"),
    }
}

fn projected_diagnostic_error(
    id: &TypeId,
    path: &str,
    diagnostic: SdkExecutionDiagnostic,
) -> OrmError {
    let legacy_code = match diagnostic.code().as_str() {
        "invalid_projected_regex" => "invalid_regex_pattern",
        "regex_constraint_violation" => "regex_violation",
        "projected_range_domain_mismatch" | "projected_values_domain_mismatch" => {
            "wrong_scalar_domain"
        }
        "range_constraint_violation" => "range_violation",
        "values_constraint_violation" => "values_violation",
        code => code,
    };
    projected_value_error(id, path, legacy_code)
}

fn decode_role_player_attributes(
    id: &TypeId,
    descriptors: &[OwnedAttributeDescriptor],
    values: &[(String, serde_json::Value)],
) -> Result<DynamicAttributeMap> {
    let mut attributes = Vec::new();
    for (name, value) in values {
        let descriptor = descriptors
            .iter()
            .find(|descriptor| descriptor.attr_name == *name)
            .ok_or_else(|| OrmError::Hydration {
                type_name: id.label().as_str().to_owned(),
                message: "role-player row contains an unprojected attribute".into(),
            })?;
        let values = match value {
            serde_json::Value::Array(values) => values.as_slice(),
            value => std::slice::from_ref(value),
        };
        for value in values {
            let value = AttributeValue::from_json(value, descriptor.value_type.as_str())
                .ok_or_else(|| OrmError::Hydration {
                    type_name: id.label().as_str().to_owned(),
                    message: "role-player attribute has the wrong provider value type".into(),
                })?;
            attributes.push((name.clone(), value));
        }
    }
    Ok(attributes)
}

fn entity_descriptor(
    projection: &RuntimeProjection,
    model: &ModelProjection,
) -> Result<EntityDescriptor> {
    Ok(EntityDescriptor {
        type_name: model.id().label().as_str().to_owned(),
        is_abstract: model.declaration().is_abstract(),
        parent_type: model
            .declaration()
            .parent()
            .map(|id| id.label().as_str().to_owned()),
        owned_attributes: owned_attributes(projection, model)?,
        doc: None,
        meta: BTreeMap::new(),
    })
}

fn relation_descriptor(
    projection: &RuntimeProjection,
    model: &ModelProjection,
) -> Result<RelationDescriptor> {
    let mut roles = Vec::new();
    for role in model.query_tokens().roles().values() {
        let distinct = role
            .annotations()
            .keys()
            .any(|id| id.kind() == &AnnotationKindId::Distinct);
        roles.push(RoleDescriptor {
            role_name: role.role().label().as_str().to_owned(),
            player_type_names: role
                .accepted_players()
                .iter()
                .map(|id| id.label().as_str().to_owned())
                .collect(),
            cardinality: Some(provider_cardinality(role.multiplicity().cardinality())?),
            overrides: role.specializes().map(|id| id.label().as_str().to_owned()),
            is_abstract: role.is_abstract(),
            ordered: !role.multiplicity().collection_mode().is_unordered(),
            distinct,
            plays_cardinality: None,
            doc: None,
            meta: BTreeMap::new(),
        });
    }
    Ok(RelationDescriptor {
        type_name: model.id().label().as_str().to_owned(),
        is_abstract: model.declaration().is_abstract(),
        parent_type: model
            .declaration()
            .parent()
            .map(|id| id.label().as_str().to_owned()),
        owned_attributes: owned_attributes(projection, model)?,
        roles,
        doc: None,
        meta: BTreeMap::new(),
    })
}

fn owned_attributes(
    projection: &RuntimeProjection,
    model: &ModelProjection,
) -> Result<Vec<OwnedAttributeDescriptor>> {
    model
        .query_tokens()
        .fields()
        .values()
        .map(|field| {
            let attribute_id =
                TypeId::new(TypeKind::Attribute, field.id().attribute().label().as_str())
                    .map_err(contract_error)?;
            let attribute = projection.models().get(&attribute_id).ok_or_else(|| {
                descriptor_error(
                    model.id(),
                    "projected ownership references an absent attribute model",
                )
            })?;
            let value_type = attribute.declaration().value_type().ok_or_else(|| {
                descriptor_error(&attribute_id, "projected attribute omits its value type")
            })?;
            let mut annotations = Vec::new();
            if field.is_key() {
                annotations.push(Annotation::Key);
            } else if field.is_unique() {
                annotations.push(Annotation::Unique);
            }
            if field
                .annotations()
                .keys()
                .any(|id| id.kind() == &AnnotationKindId::Distinct)
            {
                annotations.push(Annotation::Distinct);
            }
            annotations.push(Annotation::Card(
                provider_cardinality(field.multiplicity().cardinality())?.0,
                provider_cardinality(field.multiplicity().cardinality())?.1,
            ));
            Ok(OwnedAttributeDescriptor {
                field_name: field.target_name().as_str().to_owned(),
                attr_name: field.id().attribute().label().as_str().to_owned(),
                value_type: provider_value_type(value_type),
                annotations,
                is_optional: !field.multiplicity().required(),
                is_ordered: !field.multiplicity().collection_mode().is_unordered(),
                doc: None,
                meta: BTreeMap::new(),
            })
        })
        .collect()
}

fn provider_cardinality(value: Cardinality) -> Result<(u32, Option<u32>)> {
    let min = u32::try_from(value.min()).map_err(|_| OrmError::DescriptorValidation {
        type_name: "<runtime-projection>".into(),
        message: "cardinality minimum exceeds the provider descriptor domain".into(),
    })?;
    let max =
        value
            .max()
            .map(u32::try_from)
            .transpose()
            .map_err(|_| OrmError::DescriptorValidation {
                type_name: "<runtime-projection>".into(),
                message: "cardinality maximum exceeds the provider descriptor domain".into(),
            })?;
    Ok((min, max))
}

const fn provider_value_type(value: ValueTypeTag) -> ValueType {
    match value {
        ValueTypeTag::String => ValueType::String,
        ValueTypeTag::Long => ValueType::Long,
        ValueTypeTag::Double => ValueType::Double,
        ValueTypeTag::Boolean => ValueType::Boolean,
        ValueTypeTag::Date => ValueType::Date,
        ValueTypeTag::DateTime => ValueType::DateTime,
        ValueTypeTag::DateTimeTz => ValueType::DateTimeTz,
        ValueTypeTag::Decimal => ValueType::Decimal,
        ValueTypeTag::Duration => ValueType::Duration,
    }
}

fn descriptor_error(id: &TypeId, message: &str) -> OrmError {
    OrmError::DescriptorValidation {
        type_name: id.label().as_str().to_owned(),
        message: message.to_owned(),
    }
}

fn contract_error(error: type_bridge_contract::diagnostic::Diagnostic) -> OrmError {
    OrmError::DescriptorValidation {
        type_name: "<runtime-projection>".into(),
        message: error.to_string(),
    }
}

fn kind_conflict(id: &TypeId, expected: &str) -> OrmError {
    OrmError::DescriptorConflict {
        type_name: id.label().as_str().to_owned(),
        message: format!("installed descriptor is not an {expected}"),
    }
}

const fn kind_name(kind: TypeKind) -> &'static str {
    match kind {
        TypeKind::Entity => "entity",
        TypeKind::Relation => "relation",
        TypeKind::Attribute => "attribute",
        TypeKind::Struct => "struct",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use type_bridge_contract::temporal::TimeZoneDesignator;

    #[test]
    fn provider_role_player_arrays_decode_once_in_the_orm() {
        let id = TypeId::new(TypeKind::Entity, "person").unwrap();
        let descriptors = vec![OwnedAttributeDescriptor {
            field_name: "scores".into(),
            attr_name: "score".into(),
            value_type: ValueType::Long,
            annotations: vec![],
            is_optional: true,
            is_ordered: false,
            doc: None,
            meta: BTreeMap::new(),
        }];
        let values = vec![("score".into(), serde_json::json!(["9007199254740993", "2"]))];
        assert_eq!(
            decode_role_player_attributes(&id, &descriptors, &values).unwrap(),
            vec![
                ("score".into(), AttributeValue::Long(9_007_199_254_740_993)),
                ("score".into(), AttributeValue::Long(2)),
            ]
        );
        assert!(
            decode_role_player_attributes(
                &id,
                &descriptors,
                &[("unknown".into(), serde_json::json!(1))],
            )
            .is_err()
        );
    }

    #[test]
    fn canonical_match_value_conversion_preserves_double_bits_and_named_zone_evidence() {
        let double = CanonicalValue::Double(CanonicalDouble::new(-0.0).unwrap());
        let AttributeValue::Double(projected) = attribute_value_from_canonical(&double) else {
            panic!("double changed scalar domain")
        };
        assert_eq!(projected.to_bits(), (-0.0_f64).to_bits());

        let named = CanonicalDateTimeTz::new_named_resolved(
            "2024-10-27T01:30:00".parse().unwrap(),
            "Europe/London",
            3_600,
        )
        .unwrap();
        let value = CanonicalValue::DateTimeTz(named.clone());
        let AttributeValue::DateTimeTZ(projected) = attribute_value_from_canonical(&value) else {
            panic!("datetime-tz changed scalar domain")
        };
        assert_eq!(projected, "2024-10-27T01:30:00+01:00[Europe/London]");
        let round_trip = canonical_attribute_value(&AttributeValue::DateTimeTZ(projected)).unwrap();
        assert_eq!(round_trip, CanonicalValue::DateTimeTz(named));
        assert_eq!(
            match round_trip {
                CanonicalValue::DateTimeTz(value) => value.zone().clone(),
                _ => unreachable!(),
            },
            TimeZoneDesignator::Named("Europe/London".to_owned())
        );
    }
}
