//! Provider descriptors derived one-way from a trusted runtime projection.

use std::collections::BTreeMap;
use std::sync::Arc;

use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::projection::{ModelProjection, ProjectedAnnotation, RuntimeProjection};
use type_bridge_contract::schema::SchemaAnnotationValue;
use type_bridge_contract::temporal::{
    CanonicalDate, CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration,
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
        let registry = DescriptorRegistry::for_installed_projection();
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
        let annotations = model.declaration().value_annotations();
        if !annotations
            .values()
            .any(|annotation| is_value_constraint(annotation.value()))
        {
            return Ok(());
        }
        let canonical = canonical_attribute_value(value)
            .map_err(|code| projected_value_error(id, "value", code))?;
        validate_projected_annotations(id, "value", &canonical, annotations.values())
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
        validate_projected_annotations(id, target_name, &canonical, field.annotations().values())
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

fn validate_projected_annotations<'a>(
    id: &TypeId,
    path: &str,
    value: &CanonicalValue,
    annotations: impl IntoIterator<Item = &'a ProjectedAnnotation>,
) -> Result<()> {
    for annotation in annotations {
        match annotation.value() {
            SchemaAnnotationValue::Regex(pattern) => {
                let CanonicalValue::String(text) = value else {
                    return Err(projected_value_error(id, path, "wrong_scalar_domain"));
                };
                let expression = regex::Regex::new(pattern.as_str())
                    .map_err(|_| projected_value_error(id, path, "invalid_regex_pattern"))?;
                if !expression.is_match(text.as_str()) {
                    return Err(projected_value_error(id, path, "regex_violation"));
                }
            }
            SchemaAnnotationValue::Range(range) => {
                if let Some(lower) = range.lower() {
                    let ordering = value
                        .semantic_cmp_same_domain(lower)
                        .ok_or_else(|| projected_value_error(id, path, "wrong_scalar_domain"))?;
                    if ordering == std::cmp::Ordering::Less {
                        return Err(projected_value_error(id, path, "range_violation"));
                    }
                }
                if let Some(upper) = range.upper() {
                    let ordering = value
                        .semantic_cmp_same_domain(upper)
                        .ok_or_else(|| projected_value_error(id, path, "wrong_scalar_domain"))?;
                    if ordering == std::cmp::Ordering::Greater {
                        return Err(projected_value_error(id, path, "range_violation"));
                    }
                }
            }
            SchemaAnnotationValue::Values(allowed) => {
                let accepted = allowed.iter().any(|candidate| {
                    value.semantic_cmp_same_domain(candidate) == Some(std::cmp::Ordering::Equal)
                        || value == candidate
                });
                if !accepted {
                    return Err(projected_value_error(id, path, "values_violation"));
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

fn canonical_attribute_value(
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
        AttributeValue::DateTimeTZ(value) => value
            .strip_suffix("+00:00")
            .map_or_else(|| value.clone(), |local| format!("{local}Z"))
            .parse::<CanonicalDateTimeTz>()
            .map(CanonicalValue::DateTimeTz)
            .map_err(|_| "wrong_scalar_domain"),
        AttributeValue::Decimal(value) => DecimalValue::new(value)
            .map(CanonicalValue::Decimal)
            .map_err(|_| "wrong_scalar_domain"),
        AttributeValue::Duration(value) => value
            .parse::<CanonicalDuration>()
            .map(CanonicalValue::Duration)
            .map_err(|_| "wrong_scalar_domain"),
    }
}

fn projected_value_error(id: &TypeId, path: &str, code: &str) -> OrmError {
    OrmError::DescriptorValidation {
        type_name: id.label().as_str().to_owned(),
        message: format!("{code} at {path}"),
    }
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
            ordered: false,
            distinct: false,
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
                is_ordered: false,
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
}
