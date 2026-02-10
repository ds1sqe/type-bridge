use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fmt;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SchemaError {
    ParseError {
        message: String,
        line: usize,
        column: usize,
    },
    InheritanceCycle {
        type_name: String,
    },
    UnknownParent {
        child: String,
        parent: String,
    },
    DuplicateDefinition {
        name: String,
        kind: String,
    },
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SchemaError::ParseError {
                message,
                line,
                column,
            } => write!(f, "Parse error at {}:{}: {}", line, column, message),
            SchemaError::InheritanceCycle { type_name } => {
                write!(f, "Inheritance cycle detected involving '{}'", type_name)
            }
            SchemaError::UnknownParent { child, parent } => {
                write!(
                    f,
                    "Type '{}' has unknown parent type '{}'",
                    child, parent
                )
            }
            SchemaError::DuplicateDefinition { name, kind } => {
                write!(f, "Duplicate {} definition: '{}'", kind, name)
            }
        }
    }
}

impl std::error::Error for SchemaError {}

// ---------------------------------------------------------------------------
// Cardinality
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cardinality {
    pub min: u32,
    pub max: Option<u32>, // None = unbounded
}

// ---------------------------------------------------------------------------
// OwnedAttribute
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnedAttribute {
    pub name: String,
    pub is_key: bool,
    pub is_unique: bool,
    pub is_cascade: bool,
    pub subkey_group: Option<String>,
    pub cardinality: Option<Cardinality>,
}

// ---------------------------------------------------------------------------
// PlayedRole
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayedRole {
    pub role_ref: String, // e.g. "friendship:friend"
    pub cardinality: Option<Cardinality>,
}

// ---------------------------------------------------------------------------
// RoleSpec
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleSpec {
    pub name: String,
    pub overrides: Option<String>, // For "relates author as contributor"
    pub cardinality: Option<Cardinality>,
    pub distinct: bool,
}

// ---------------------------------------------------------------------------
// AttributeType
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttributeType {
    pub name: String,
    pub value_type: String,
    pub parent: Option<String>,
    pub is_abstract: bool,
    pub is_independent: bool,
    pub regex: Option<String>,
    pub allowed_values: Option<Vec<String>>,
    pub range_min: Option<String>,
    pub range_max: Option<String>,
}

// ---------------------------------------------------------------------------
// EntityType
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityType {
    pub name: String,
    pub parent: Option<String>,
    pub is_abstract: bool,
    pub owns: Vec<OwnedAttribute>,
    pub owns_order: Vec<String>,
    pub plays: Vec<PlayedRole>,
}

// ---------------------------------------------------------------------------
// RelationType
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationType {
    pub name: String,
    pub parent: Option<String>,
    pub is_abstract: bool,
    pub roles: Vec<RoleSpec>,
    pub owns: Vec<OwnedAttribute>,
    pub owns_order: Vec<String>,
    pub plays: Vec<PlayedRole>,
}

// ---------------------------------------------------------------------------
// TypeSchema
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeSchema {
    pub entities: BTreeMap<String, EntityType>,
    pub relations: BTreeMap<String, RelationType>,
    pub attributes: BTreeMap<String, AttributeType>,
}

impl TypeSchema {
    pub fn new() -> Self {
        TypeSchema {
            entities: BTreeMap::new(),
            relations: BTreeMap::new(),
            attributes: BTreeMap::new(),
        }
    }

    /// Parse a TypeQL `define` block into a fully-resolved `TypeSchema`.
    pub fn from_typeql(input: &str) -> Result<TypeSchema, SchemaError> {
        let mut schema = super::parser::parse_typeql(input)?;
        schema.resolve_inheritance()?;
        Ok(schema)
    }

    /// Deserialize from a JSON string.
    pub fn from_json(json: &str) -> Result<TypeSchema, String> {
        serde_json::from_str(json).map_err(|e| e.to_string())
    }

    /// Serialize to a JSON string.
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|e| e.to_string())
    }

    /// Check if any type with the given name is abstract.
    pub fn is_abstract(&self, type_name: &str) -> bool {
        if let Some(e) = self.entities.get(type_name) {
            return e.is_abstract;
        }
        if let Some(r) = self.relations.get(type_name) {
            return r.is_abstract;
        }
        if let Some(a) = self.attributes.get(type_name) {
            return a.is_abstract;
        }
        false
    }

    /// Get all owned attributes for an entity (already resolved after inheritance).
    pub fn get_all_owned_attributes(&self, entity_name: &str) -> Vec<&OwnedAttribute> {
        if let Some(e) = self.entities.get(entity_name) {
            return e.owns.iter().collect();
        }
        if let Some(r) = self.relations.get(entity_name) {
            return r.owns.iter().collect();
        }
        vec![]
    }

    /// Get all played roles for an entity (already resolved after inheritance).
    pub fn get_all_plays_roles(&self, entity_name: &str) -> Vec<&PlayedRole> {
        if let Some(e) = self.entities.get(entity_name) {
            return e.plays.iter().collect();
        }
        if let Some(r) = self.relations.get(entity_name) {
            return r.plays.iter().collect();
        }
        vec![]
    }

    /// Get all roles defined by a relation (already resolved after inheritance).
    pub fn get_all_relates(&self, relation_name: &str) -> Vec<&RoleSpec> {
        if let Some(r) = self.relations.get(relation_name) {
            return r.roles.iter().collect();
        }
        vec![]
    }

    /// Get an entity by name.
    pub fn get_entity(&self, name: &str) -> Option<&EntityType> {
        self.entities.get(name)
    }

    /// Get a relation by name.
    pub fn get_relation(&self, name: &str) -> Option<&RelationType> {
        self.relations.get(name)
    }

    /// Get an attribute type by name.
    pub fn get_attribute(&self, name: &str) -> Option<&AttributeType> {
        self.attributes.get(name)
    }

    // -----------------------------------------------------------------------
    // Inheritance resolution
    // -----------------------------------------------------------------------

    fn resolve_inheritance(&mut self) -> Result<(), SchemaError> {
        self.detect_cycles()?;
        self.resolve_entity_inheritance();
        self.resolve_relation_inheritance();
        Ok(())
    }

    fn detect_cycles(&self) -> Result<(), SchemaError> {
        // Check entity parent chains
        for name in self.entities.keys() {
            let mut visited = HashSet::new();
            let mut current = Some(name.as_str());
            while let Some(n) = current {
                if !visited.insert(n.to_string()) {
                    return Err(SchemaError::InheritanceCycle {
                        type_name: n.to_string(),
                    });
                }
                current = self
                    .entities
                    .get(n)
                    .and_then(|e| e.parent.as_deref());
            }
        }
        // Check relation parent chains
        for name in self.relations.keys() {
            let mut visited = HashSet::new();
            let mut current = Some(name.as_str());
            while let Some(n) = current {
                if !visited.insert(n.to_string()) {
                    return Err(SchemaError::InheritanceCycle {
                        type_name: n.to_string(),
                    });
                }
                current = self
                    .relations
                    .get(n)
                    .and_then(|r| r.parent.as_deref());
            }
        }
        Ok(())
    }

    fn resolve_entity_inheritance(&mut self) {
        let mut changed = true;
        while changed {
            changed = false;
            let names: Vec<String> = self.entities.keys().cloned().collect();
            for name in &names {
                let parent_name = match self.entities.get(name) {
                    Some(e) => match &e.parent {
                        Some(p) => p.clone(),
                        None => continue,
                    },
                    None => continue,
                };

                let parent = match self.entities.get(&parent_name) {
                    Some(p) => p.clone(),
                    None => continue,
                };

                let entity = self.entities.get_mut(name).unwrap();
                let before = (entity.owns.len(), entity.plays.len());

                // Merge parent owns (skip if child already owns the same attribute)
                let child_own_names: HashSet<String> =
                    entity.owns.iter().map(|o| o.name.clone()).collect();
                for parent_own in &parent.owns {
                    if !child_own_names.contains(&parent_own.name) {
                        entity.owns.push(parent_own.clone());
                    }
                }

                // Merge parent plays
                let child_play_refs: HashSet<String> =
                    entity.plays.iter().map(|p| p.role_ref.clone()).collect();
                for parent_play in &parent.plays {
                    if !child_play_refs.contains(&parent_play.role_ref) {
                        entity.plays.push(parent_play.clone());
                    }
                }

                // Prepend parent owns_order
                let child_order_set: HashSet<String> =
                    entity.owns_order.iter().cloned().collect();
                let parent_attrs: Vec<String> = parent
                    .owns_order
                    .iter()
                    .filter(|a| !child_order_set.contains(a.as_str()))
                    .cloned()
                    .collect();
                if !parent_attrs.is_empty() {
                    let mut new_order = parent_attrs;
                    new_order.append(&mut entity.owns_order);
                    entity.owns_order = new_order;
                }

                let after = (entity.owns.len(), entity.plays.len());
                if before != after {
                    changed = true;
                }
            }
        }
    }

    fn resolve_relation_inheritance(&mut self) {
        let mut changed = true;
        while changed {
            changed = false;
            let names: Vec<String> = self.relations.keys().cloned().collect();
            for name in &names {
                let parent_name = match self.relations.get(name) {
                    Some(r) => match &r.parent {
                        Some(p) => p.clone(),
                        None => continue,
                    },
                    None => continue,
                };

                let parent = match self.relations.get(&parent_name) {
                    Some(p) => p.clone(),
                    None => continue,
                };

                let relation = self.relations.get_mut(name).unwrap();
                let before = (
                    relation.owns.len(),
                    relation.roles.len(),
                    relation.plays.len(),
                );

                // Merge parent owns
                let child_own_names: HashSet<String> =
                    relation.owns.iter().map(|o| o.name.clone()).collect();
                for parent_own in &parent.owns {
                    if !child_own_names.contains(&parent_own.name) {
                        relation.owns.push(parent_own.clone());
                    }
                }

                // Merge parent plays
                let child_play_refs: HashSet<String> =
                    relation.plays.iter().map(|p| p.role_ref.clone()).collect();
                for parent_play in &parent.plays {
                    if !child_play_refs.contains(&parent_play.role_ref) {
                        relation.plays.push(parent_play.clone());
                    }
                }

                // Inherit roles — child may override parent roles via "as"
                let child_role_names: HashSet<String> =
                    relation.roles.iter().map(|r| r.name.clone()).collect();
                let overridden_parent_roles: HashSet<String> = relation
                    .roles
                    .iter()
                    .filter_map(|r| r.overrides.clone())
                    .collect();
                for parent_role in &parent.roles {
                    if !child_role_names.contains(&parent_role.name)
                        && !overridden_parent_roles.contains(&parent_role.name)
                    {
                        relation.roles.push(parent_role.clone());
                    }
                }

                // Prepend parent owns_order
                let child_order_set: HashSet<String> =
                    relation.owns_order.iter().cloned().collect();
                let parent_attrs: Vec<String> = parent
                    .owns_order
                    .iter()
                    .filter(|a| !child_order_set.contains(a.as_str()))
                    .cloned()
                    .collect();
                if !parent_attrs.is_empty() {
                    let mut new_order = parent_attrs;
                    new_order.append(&mut relation.owns_order);
                    relation.owns_order = new_order;
                }

                let after = (
                    relation.owns.len(),
                    relation.roles.len(),
                    relation.plays.len(),
                );
                if before != after {
                    changed = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cardinality_serde() {
        let card = Cardinality {
            min: 1,
            max: Some(3),
        };
        let json = serde_json::to_string(&card).unwrap();
        let card2: Cardinality = serde_json::from_str(&json).unwrap();
        assert_eq!(card, card2);
    }

    #[test]
    fn test_cardinality_unbounded() {
        let card = Cardinality {
            min: 0,
            max: None,
        };
        let json = serde_json::to_string(&card).unwrap();
        assert!(json.contains("null"));
        let card2: Cardinality = serde_json::from_str(&json).unwrap();
        assert_eq!(card, card2);
    }

    #[test]
    fn test_type_schema_new() {
        let schema = TypeSchema::new();
        assert!(schema.entities.is_empty());
        assert!(schema.relations.is_empty());
        assert!(schema.attributes.is_empty());
    }

    #[test]
    fn test_type_schema_json_round_trip() {
        let mut schema = TypeSchema::new();
        schema.attributes.insert(
            "name".to_string(),
            AttributeType {
                name: "name".to_string(),
                value_type: "string".to_string(),
                parent: None,
                is_abstract: false,
                is_independent: false,
                regex: None,
                allowed_values: None,
                range_min: None,
                range_max: None,
            },
        );
        schema.entities.insert(
            "person".to_string(),
            EntityType {
                name: "person".to_string(),
                parent: None,
                is_abstract: false,
                owns: vec![OwnedAttribute {
                    name: "name".to_string(),
                    is_key: true,
                    is_unique: false,
                    is_cascade: false,
                    subkey_group: None,
                    cardinality: None,
                }],
                owns_order: vec!["name".to_string()],
                plays: vec![],
            },
        );

        let json = schema.to_json().unwrap();
        let schema2 = TypeSchema::from_json(&json).unwrap();
        let json2 = schema2.to_json().unwrap();
        assert_eq!(json, json2);
    }

    #[test]
    fn test_is_abstract() {
        let mut schema = TypeSchema::new();
        schema.entities.insert(
            "animal".to_string(),
            EntityType {
                name: "animal".to_string(),
                parent: None,
                is_abstract: true,
                owns: vec![],
                owns_order: vec![],
                plays: vec![],
            },
        );
        schema.entities.insert(
            "dog".to_string(),
            EntityType {
                name: "dog".to_string(),
                parent: Some("animal".to_string()),
                is_abstract: false,
                owns: vec![],
                owns_order: vec![],
                plays: vec![],
            },
        );

        assert!(schema.is_abstract("animal"));
        assert!(!schema.is_abstract("dog"));
        assert!(!schema.is_abstract("nonexistent"));
    }

    #[test]
    fn test_get_all_owned_attributes() {
        let mut schema = TypeSchema::new();
        let name_attr = OwnedAttribute {
            name: "name".to_string(),
            is_key: true,
            is_unique: false,
            is_cascade: false,
            subkey_group: None,
            cardinality: None,
        };
        schema.entities.insert(
            "person".to_string(),
            EntityType {
                name: "person".to_string(),
                parent: None,
                is_abstract: false,
                owns: vec![name_attr],
                owns_order: vec!["name".to_string()],
                plays: vec![],
            },
        );

        let attrs = schema.get_all_owned_attributes("person");
        assert_eq!(attrs.len(), 1);
        assert_eq!(attrs[0].name, "name");
        assert!(attrs[0].is_key);

        let empty = schema.get_all_owned_attributes("nonexistent");
        assert!(empty.is_empty());
    }

    #[test]
    fn test_entity_inheritance_resolution() {
        let mut schema = TypeSchema::new();
        schema.entities.insert(
            "person".to_string(),
            EntityType {
                name: "person".to_string(),
                parent: None,
                is_abstract: false,
                owns: vec![OwnedAttribute {
                    name: "name".to_string(),
                    is_key: true,
                    is_unique: false,
                    is_cascade: false,
                    subkey_group: None,
                    cardinality: None,
                }],
                owns_order: vec!["name".to_string()],
                plays: vec![PlayedRole {
                    role_ref: "friendship:friend".to_string(),
                    cardinality: None,
                }],
            },
        );
        schema.entities.insert(
            "employee".to_string(),
            EntityType {
                name: "employee".to_string(),
                parent: Some("person".to_string()),
                is_abstract: false,
                owns: vec![OwnedAttribute {
                    name: "employee-id".to_string(),
                    is_key: false,
                    is_unique: true,
                    is_cascade: false,
                    subkey_group: None,
                    cardinality: None,
                }],
                owns_order: vec!["employee-id".to_string()],
                plays: vec![],
            },
        );

        schema.resolve_inheritance().unwrap();

        let employee = schema.get_entity("employee").unwrap();
        assert_eq!(employee.owns.len(), 2);
        assert_eq!(employee.plays.len(), 1);
        assert_eq!(employee.owns_order, vec!["name", "employee-id"]);
    }

    #[test]
    fn test_relation_inheritance_with_role_override() {
        let mut schema = TypeSchema::new();
        schema.relations.insert(
            "contribution".to_string(),
            RelationType {
                name: "contribution".to_string(),
                parent: None,
                is_abstract: true,
                roles: vec![
                    RoleSpec {
                        name: "contributor".to_string(),
                        overrides: None,
                        cardinality: None,
                        distinct: false,
                    },
                    RoleSpec {
                        name: "work".to_string(),
                        overrides: None,
                        cardinality: None,
                        distinct: false,
                    },
                ],
                owns: vec![],
                owns_order: vec![],
                plays: vec![],
            },
        );
        schema.relations.insert(
            "authoring".to_string(),
            RelationType {
                name: "authoring".to_string(),
                parent: Some("contribution".to_string()),
                is_abstract: false,
                roles: vec![RoleSpec {
                    name: "author".to_string(),
                    overrides: Some("contributor".to_string()),
                    cardinality: None,
                    distinct: false,
                }],
                owns: vec![],
                owns_order: vec![],
                plays: vec![],
            },
        );

        schema.resolve_inheritance().unwrap();

        let authoring = schema.get_relation("authoring").unwrap();
        // Should have "author" (overrides contributor) + inherited "work"
        assert_eq!(authoring.roles.len(), 2);
        let role_names: Vec<&str> = authoring.roles.iter().map(|r| r.name.as_str()).collect();
        assert!(role_names.contains(&"author"));
        assert!(role_names.contains(&"work"));
        assert!(!role_names.contains(&"contributor"));
    }

    #[test]
    fn test_cycle_detection() {
        let mut schema = TypeSchema::new();
        schema.entities.insert(
            "a".to_string(),
            EntityType {
                name: "a".to_string(),
                parent: Some("b".to_string()),
                is_abstract: false,
                owns: vec![],
                owns_order: vec![],
                plays: vec![],
            },
        );
        schema.entities.insert(
            "b".to_string(),
            EntityType {
                name: "b".to_string(),
                parent: Some("a".to_string()),
                is_abstract: false,
                owns: vec![],
                owns_order: vec![],
                plays: vec![],
            },
        );

        let result = schema.resolve_inheritance();
        assert!(result.is_err());
        match result.unwrap_err() {
            SchemaError::InheritanceCycle { .. } => {}
            other => panic!("Expected InheritanceCycle, got {:?}", other),
        }
    }
}
