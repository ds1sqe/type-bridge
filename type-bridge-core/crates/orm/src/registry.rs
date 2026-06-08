//! Explicit runtime descriptor registry.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use crate::descriptor::{EntityDescriptor, RelationDescriptor, TypeDescriptor, TypeDescriptorRef};
use crate::error::{OrmError, Result};

/// Thread-safe registry for runtime entity and relation descriptors.
///
/// The registry is intentionally standalone: it has no database, transaction,
/// manager, Python, or TypeScript dependency. Bindings normalize their metadata
/// into descriptors before registration.
#[derive(Debug, Default)]
pub struct DescriptorRegistry {
    descriptors: RwLock<HashMap<String, TypeDescriptorRef>>,
}

impl DescriptorRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an entity descriptor.
    ///
    /// Identical re-registration is idempotent and returns the canonical stored
    /// descriptor. Conflicting shapes or type-kind conflicts return typed ORM
    /// errors.
    pub fn register_entity(&self, descriptor: EntityDescriptor) -> Result<Arc<EntityDescriptor>> {
        validate_entity_descriptor(&descriptor)?;

        let mut descriptors = self.descriptors.write().map_err(lock_error)?;
        match descriptors.get(&descriptor.type_name) {
            Some(TypeDescriptorRef::Entity(existing)) if existing.as_ref() == &descriptor => {
                Ok(Arc::clone(existing))
            }
            Some(TypeDescriptorRef::Entity(_)) => Err(OrmError::DescriptorConflict {
                type_name: descriptor.type_name,
                message: "entity descriptor shape differs from registered descriptor".into(),
            }),
            Some(TypeDescriptorRef::Relation(_)) => Err(OrmError::DescriptorConflict {
                type_name: descriptor.type_name,
                message: "type name is already registered as a relation".into(),
            }),
            None => {
                let descriptor = Arc::new(descriptor);
                descriptors.insert(
                    descriptor.type_name.clone(),
                    TypeDescriptorRef::Entity(Arc::clone(&descriptor)),
                );
                Ok(descriptor)
            }
        }
    }

    /// Register a relation descriptor.
    ///
    /// Identical re-registration is idempotent and returns the canonical stored
    /// descriptor. Conflicting shapes or type-kind conflicts return typed ORM
    /// errors.
    pub fn register_relation(
        &self,
        descriptor: RelationDescriptor,
    ) -> Result<Arc<RelationDescriptor>> {
        validate_relation_descriptor(&descriptor)?;

        let mut descriptors = self.descriptors.write().map_err(lock_error)?;
        match descriptors.get(&descriptor.type_name) {
            Some(TypeDescriptorRef::Relation(existing)) if existing.as_ref() == &descriptor => {
                Ok(Arc::clone(existing))
            }
            Some(TypeDescriptorRef::Relation(_)) => Err(OrmError::DescriptorConflict {
                type_name: descriptor.type_name,
                message: "relation descriptor shape differs from registered descriptor".into(),
            }),
            Some(TypeDescriptorRef::Entity(_)) => Err(OrmError::DescriptorConflict {
                type_name: descriptor.type_name,
                message: "type name is already registered as an entity".into(),
            }),
            None => {
                let descriptor = Arc::new(descriptor);
                descriptors.insert(
                    descriptor.type_name.clone(),
                    TypeDescriptorRef::Relation(Arc::clone(&descriptor)),
                );
                Ok(descriptor)
            }
        }
    }

    /// Lookup an entity descriptor by TypeDB type name.
    pub fn entity(&self, type_name: &str) -> Result<Arc<EntityDescriptor>> {
        match self.get(type_name) {
            Some(TypeDescriptorRef::Entity(descriptor)) => Ok(descriptor),
            Some(TypeDescriptorRef::Relation(_)) => Err(OrmError::DescriptorConflict {
                type_name: type_name.to_string(),
                message: "requested entity but descriptor is a relation".into(),
            }),
            None => Err(OrmError::DescriptorNotFound(type_name.to_string())),
        }
    }

    /// Lookup a relation descriptor by TypeDB type name.
    pub fn relation(&self, type_name: &str) -> Result<Arc<RelationDescriptor>> {
        match self.get(type_name) {
            Some(TypeDescriptorRef::Relation(descriptor)) => Ok(descriptor),
            Some(TypeDescriptorRef::Entity(_)) => Err(OrmError::DescriptorConflict {
                type_name: type_name.to_string(),
                message: "requested relation but descriptor is an entity".into(),
            }),
            None => Err(OrmError::DescriptorNotFound(type_name.to_string())),
        }
    }

    /// Lookup any descriptor by TypeDB type name.
    pub fn get(&self, type_name: &str) -> Option<TypeDescriptorRef> {
        self.descriptors
            .read()
            .ok()
            .and_then(|descriptors| descriptors.get(type_name).cloned())
    }

    /// Return an owned snapshot of all registered descriptors.
    pub fn snapshot(&self) -> Vec<TypeDescriptor> {
        let mut descriptors: Vec<_> = self
            .descriptors
            .read()
            .map(|descriptors| {
                descriptors
                    .values()
                    .map(TypeDescriptorRef::to_owned_descriptor)
                    .collect()
            })
            .unwrap_or_default();
        descriptors.sort_by(|left, right| left.type_name().cmp(right.type_name()));
        descriptors
    }
}

fn validate_entity_descriptor(descriptor: &EntityDescriptor) -> Result<()> {
    validate_type_name(&descriptor.type_name)?;
    if let Some(parent_type) = &descriptor.parent_type {
        validate_type_name(parent_type)?;
    }
    validate_attributes(&descriptor.type_name, &descriptor.owned_attributes)
}

fn validate_relation_descriptor(descriptor: &RelationDescriptor) -> Result<()> {
    validate_type_name(&descriptor.type_name)?;
    if let Some(parent_type) = &descriptor.parent_type {
        validate_type_name(parent_type)?;
    }
    validate_attributes(&descriptor.type_name, &descriptor.owned_attributes)?;

    let mut role_names = HashSet::new();
    for role in &descriptor.roles {
        validate_non_empty(&descriptor.type_name, "role name", &role.role_name)?;
        if !role_names.insert(role.role_name.as_str()) {
            return Err(OrmError::DescriptorValidation {
                type_name: descriptor.type_name.clone(),
                message: format!("duplicate role name '{}'", role.role_name),
            });
        }
        for player_type_name in &role.player_type_names {
            validate_type_name(player_type_name)?;
        }
    }

    Ok(())
}

fn validate_attributes(
    type_name: &str,
    attributes: &[crate::descriptor::OwnedAttributeDescriptor],
) -> Result<()> {
    let mut field_names = HashSet::new();
    let mut attr_names = HashSet::new();
    for attr in attributes {
        validate_non_empty(type_name, "field name", &attr.field_name)?;
        validate_non_empty(type_name, "attribute name", &attr.attr_name)?;
        if !field_names.insert(attr.field_name.as_str()) {
            return Err(OrmError::DescriptorValidation {
                type_name: type_name.to_string(),
                message: format!("duplicate field name '{}'", attr.field_name),
            });
        }
        if !attr_names.insert(attr.attr_name.as_str()) {
            return Err(OrmError::DescriptorValidation {
                type_name: type_name.to_string(),
                message: format!("duplicate attribute name '{}'", attr.attr_name),
            });
        }
    }
    Ok(())
}

fn validate_type_name(type_name: &str) -> Result<()> {
    validate_non_empty(type_name, "type name", type_name)
}

fn validate_non_empty(type_name: &str, label: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(OrmError::DescriptorValidation {
            type_name: type_name.to_string(),
            message: format!("{label} cannot be empty"),
        });
    }
    Ok(())
}

fn lock_error<T>(_: std::sync::PoisonError<T>) -> OrmError {
    OrmError::DescriptorValidation {
        type_name: "<registry>".into(),
        message: "descriptor registry lock is poisoned".into(),
    }
}
