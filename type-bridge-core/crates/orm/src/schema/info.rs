//! Type-erased schema metadata containers.
//!
//! These types store metadata extracted from `TypeBridgeEntity` and
//! `TypeBridgeRelation` trait impls at registration time, enabling
//! runtime schema operations without generic type parameters.

use std::collections::BTreeMap;

use crate::attribute::ValueType;
use crate::entity::Annotation;

use super::diff::SchemaDiff;
use super::error::SchemaError;
use super::generator;

/// Metadata for one owned attribute in a schema entry (owned `String` version).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedAttributeEntry {
    /// Attribute type name.
    pub attr_name: String,
    /// Value type.
    pub value_type: ValueType,
    /// Ownership annotations.
    pub annotations: Vec<Annotation>,
}

impl OwnedAttributeEntry {
    /// Format the annotation flags as a TypeQL annotation string.
    ///
    /// Returns strings like `"@key"`, `"@unique"`, `"@card(2..5)"`.
    pub fn flags_string(&self) -> String {
        let mut parts = Vec::new();
        for ann in &self.annotations {
            match ann {
                Annotation::Key => parts.push("@key".to_string()),
                Annotation::Unique => parts.push("@unique".to_string()),
                Annotation::Card(min, max) => {
                    let max_str = match max {
                        Some(m) => m.to_string(),
                        None => "*".to_string(),
                    };
                    parts.push(format!("@card({min}..{max_str})"));
                }
            }
        }
        parts.join(" ")
    }
}

/// Metadata for one role in a relation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleEntry {
    /// Role name (e.g. `"employee"`).
    pub role_name: String,
    /// Entity type that plays this role.
    pub player_type_name: String,
}

/// Schema entry for an entity type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntitySchemaEntry {
    /// Entity type name.
    pub type_name: String,
    /// Whether this entity is abstract.
    pub is_abstract: bool,
    /// Parent entity type name (for `sub` hierarchies).
    pub parent_type: Option<String>,
    /// Owned attributes.
    pub owned_attributes: Vec<OwnedAttributeEntry>,
}

/// Schema entry for a relation type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationSchemaEntry {
    /// Relation type name.
    pub type_name: String,
    /// Whether this relation is abstract.
    pub is_abstract: bool,
    /// Parent relation type name (for `sub` hierarchies).
    pub parent_type: Option<String>,
    /// Owned attributes.
    pub owned_attributes: Vec<OwnedAttributeEntry>,
    /// Roles and their player types.
    pub roles: Vec<RoleEntry>,
}

/// Metadata for a standalone attribute type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributeSchemaEntry {
    /// Attribute type name.
    pub attr_name: String,
    /// Value type.
    pub value_type: ValueType,
}

/// Complete schema information extracted from registered models.
#[derive(Debug, Clone, Default)]
pub struct SchemaInfo {
    /// Registered entity types, keyed by type name.
    pub entities: BTreeMap<String, EntitySchemaEntry>,
    /// Registered relation types, keyed by type name.
    pub relations: BTreeMap<String, RelationSchemaEntry>,
    /// All attribute types referenced by entities and relations.
    pub attributes: BTreeMap<String, AttributeSchemaEntry>,
}

impl SchemaInfo {
    /// Look up an entity by name.
    pub fn get_entity_by_name(&self, name: &str) -> Option<&EntitySchemaEntry> {
        self.entities.get(name)
    }

    /// Look up a relation by name.
    pub fn get_relation_by_name(&self, name: &str) -> Option<&RelationSchemaEntry> {
        self.relations.get(name)
    }

    /// Validate the schema for internal consistency.
    ///
    /// Checks for duplicate attribute names with different value types.
    pub fn validate(&self) -> Result<(), SchemaError> {
        // Check for attribute name conflicts (same name, different value type)
        let mut attr_types: BTreeMap<&str, ValueType> = BTreeMap::new();
        for attr in self.attributes.values() {
            if let Some(&existing) = attr_types.get(attr.attr_name.as_str()) {
                if existing != attr.value_type {
                    return Err(SchemaError::Validation {
                        message: format!(
                            "Attribute '{}' has conflicting value types: {} vs {}",
                            attr.attr_name, existing, attr.value_type
                        ),
                    });
                }
            } else {
                attr_types.insert(&attr.attr_name, attr.value_type);
            }
        }
        Ok(())
    }

    /// Generate a TypeQL `define` block from this schema.
    pub fn to_typeql(&self) -> Result<String, SchemaError> {
        self.validate()?;
        Ok(generator::generate_define_block(self))
    }

    /// Compare this schema to another and return the diff.
    pub fn compare(&self, other: &SchemaInfo) -> SchemaDiff {
        SchemaDiff::compute(self, other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_info_empty() {
        let info = SchemaInfo::default();
        assert!(info.entities.is_empty());
        assert!(info.relations.is_empty());
        assert!(info.attributes.is_empty());
    }

    #[test]
    fn get_entity_by_name_found() {
        let mut info = SchemaInfo::default();
        info.entities.insert(
            "person".into(),
            EntitySchemaEntry {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
            },
        );
        assert!(info.get_entity_by_name("person").is_some());
        assert!(info.get_entity_by_name("nonexistent").is_none());
    }

    #[test]
    fn get_relation_by_name_found() {
        let mut info = SchemaInfo::default();
        info.relations.insert(
            "employment".into(),
            RelationSchemaEntry {
                type_name: "employment".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![],
            },
        );
        assert!(info.get_relation_by_name("employment").is_some());
    }

    #[test]
    fn validate_passes_for_consistent_attrs() {
        let mut info = SchemaInfo::default();
        info.attributes.insert(
            "name".into(),
            AttributeSchemaEntry {
                attr_name: "name".into(),
                value_type: ValueType::String,
            },
        );
        info.attributes.insert(
            "age".into(),
            AttributeSchemaEntry {
                attr_name: "age".into(),
                value_type: ValueType::Long,
            },
        );
        assert!(info.validate().is_ok());
    }

    #[test]
    fn flags_string_key() {
        let entry = OwnedAttributeEntry {
            attr_name: "name".into(),
            value_type: ValueType::String,
            annotations: vec![Annotation::Key],
        };
        assert_eq!(entry.flags_string(), "@key");
    }

    #[test]
    fn flags_string_card_bounded() {
        let entry = OwnedAttributeEntry {
            attr_name: "tag".into(),
            value_type: ValueType::String,
            annotations: vec![Annotation::Card(2, Some(5))],
        };
        assert_eq!(entry.flags_string(), "@card(2..5)");
    }

    #[test]
    fn flags_string_card_unbounded() {
        let entry = OwnedAttributeEntry {
            attr_name: "phone".into(),
            value_type: ValueType::String,
            annotations: vec![Annotation::Card(0, None)],
        };
        assert_eq!(entry.flags_string(), "@card(0..*)");
    }

    #[test]
    fn flags_string_multiple() {
        let entry = OwnedAttributeEntry {
            attr_name: "email".into(),
            value_type: ValueType::String,
            annotations: vec![Annotation::Unique, Annotation::Card(1, Some(3))],
        };
        assert_eq!(entry.flags_string(), "@unique @card(1..3)");
    }
}
