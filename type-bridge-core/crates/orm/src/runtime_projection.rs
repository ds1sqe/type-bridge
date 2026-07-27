//! Provider descriptors derived one-way from a trusted runtime projection.

use std::collections::BTreeMap;
use std::sync::Arc;

use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::projection::{ModelProjection, RuntimeProjection};
use type_bridge_contract::value::{Cardinality, ValueTypeTag};

use crate::attribute::ValueType;
use crate::descriptor::{
    EntityDescriptor, OwnedAttributeDescriptor, RelationDescriptor, RoleDescriptor, TypeDescriptor,
};
use crate::dynamic::DynamicAttributeMap;
use crate::entity::Annotation;
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
