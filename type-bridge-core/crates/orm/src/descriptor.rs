//! Owned runtime descriptors for dynamic ORM access.
//!
//! The static Rust traits remain the typed API. These descriptors mirror the
//! same schema metadata using owned data so language bindings and generated
//! schemas can register types without borrowing from Rust model structs.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::attribute::ValueType;
use crate::entity::Annotation;

/// Owned metadata about one attribute owned by an entity or relation type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnedAttributeDescriptor {
    /// Binding-facing field name, stable across language facades.
    pub field_name: String,
    /// TypeDB attribute type name.
    pub attr_name: String,
    /// TypeDB value type.
    pub value_type: ValueType,
    /// Ownership annotations such as `@key`, `@unique`, or `@card`.
    pub annotations: Vec<Annotation>,
    /// Whether the field may be omitted from dynamic input and hydration.
    pub is_optional: bool,
}

impl OwnedAttributeDescriptor {
    /// Whether this attribute has a `@key` annotation.
    pub fn is_key(&self) -> bool {
        self.annotations
            .iter()
            .any(|annotation| matches!(annotation, Annotation::Key))
    }

    /// Whether this attribute has a `@unique` annotation.
    pub fn is_unique(&self) -> bool {
        self.annotations
            .iter()
            .any(|annotation| matches!(annotation, Annotation::Unique))
    }

    /// Return the cardinality annotation, if present.
    pub fn cardinality(&self) -> Option<(u32, Option<u32>)> {
        self.annotations
            .iter()
            .find_map(|annotation| match annotation {
                Annotation::Card(min, max) => Some((*min, *max)),
                _ => None,
            })
    }
}

/// Owned metadata about one role in a relation type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleDescriptor {
    /// Role name within the relation.
    pub role_name: String,
    /// Entity type names allowed to play this role.
    pub player_type_names: Vec<String>,
    /// Optional role cardinality, where `None` max means unbounded.
    pub cardinality: Option<(u32, Option<u32>)>,
}

/// Runtime descriptor for an entity type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityDescriptor {
    /// TypeDB entity type name.
    pub type_name: String,
    /// Whether this entity type is abstract.
    pub is_abstract: bool,
    /// Optional parent type name.
    pub parent_type: Option<String>,
    /// Attributes owned by this entity.
    pub owned_attributes: Vec<OwnedAttributeDescriptor>,
}

impl EntityDescriptor {
    /// Return the first key attribute, if any.
    pub fn key_attribute(&self) -> Option<&OwnedAttributeDescriptor> {
        self.owned_attributes.iter().find(|attr| attr.is_key())
    }

    /// Lookup an attribute by field name or TypeDB attribute name.
    pub fn attribute(&self, name: &str) -> Option<&OwnedAttributeDescriptor> {
        self.owned_attributes
            .iter()
            .find(|attr| attr.field_name == name || attr.attr_name == name)
    }
}

/// Runtime descriptor for a relation type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationDescriptor {
    /// TypeDB relation type name.
    pub type_name: String,
    /// Whether this relation type is abstract.
    pub is_abstract: bool,
    /// Optional parent type name.
    pub parent_type: Option<String>,
    /// Attributes owned by this relation.
    pub owned_attributes: Vec<OwnedAttributeDescriptor>,
    /// Roles declared by this relation.
    pub roles: Vec<RoleDescriptor>,
}

impl RelationDescriptor {
    /// Return the first key attribute, if any.
    pub fn key_attribute(&self) -> Option<&OwnedAttributeDescriptor> {
        self.owned_attributes.iter().find(|attr| attr.is_key())
    }

    /// Lookup an attribute by field name or TypeDB attribute name.
    pub fn attribute(&self, name: &str) -> Option<&OwnedAttributeDescriptor> {
        self.owned_attributes
            .iter()
            .find(|attr| attr.field_name == name || attr.attr_name == name)
    }

    /// Lookup a role by role name.
    pub fn role(&self, name: &str) -> Option<&RoleDescriptor> {
        self.roles.iter().find(|role| role.role_name == name)
    }
}

/// Owned descriptor for any registered TypeDB object type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "descriptor", rename_all = "snake_case")]
pub enum TypeDescriptor {
    /// Entity type descriptor.
    Entity(EntityDescriptor),
    /// Relation type descriptor.
    Relation(RelationDescriptor),
}

impl TypeDescriptor {
    /// Return the TypeDB type name for this descriptor.
    pub fn type_name(&self) -> &str {
        match self {
            Self::Entity(descriptor) => &descriptor.type_name,
            Self::Relation(descriptor) => &descriptor.type_name,
        }
    }
}

/// Shared reference to any registered descriptor.
#[derive(Debug, Clone)]
pub enum TypeDescriptorRef {
    /// Registered entity descriptor.
    Entity(Arc<EntityDescriptor>),
    /// Registered relation descriptor.
    Relation(Arc<RelationDescriptor>),
}

impl TypeDescriptorRef {
    /// Return the TypeDB type name for this descriptor reference.
    pub fn type_name(&self) -> &str {
        match self {
            Self::Entity(descriptor) => &descriptor.type_name,
            Self::Relation(descriptor) => &descriptor.type_name,
        }
    }

    /// Clone this reference into an owned descriptor snapshot.
    pub fn to_owned_descriptor(&self) -> TypeDescriptor {
        match self {
            Self::Entity(descriptor) => TypeDescriptor::Entity((**descriptor).clone()),
            Self::Relation(descriptor) => TypeDescriptor::Relation((**descriptor).clone()),
        }
    }
}
