use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fmt;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors that can occur during schema parsing, validation, or inheritance resolution.
///
/// Each variant captures the specific context needed to produce a helpful error
/// message (e.g. source location for parse errors, the cycle participant for
/// inheritance cycles).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SchemaError {
    /// A syntax error encountered while parsing a TypeQL `define` block.
    ParseError {
        /// Human-readable description of what went wrong.
        message: String,
        /// 1-based line number where the error was detected.
        line: usize,
        /// 1-based column number where the error was detected.
        column: usize,
    },
    /// A cycle was detected in the `sub` (inheritance) chain of a type.
    InheritanceCycle {
        /// The name of the type involved in the cycle.
        type_name: String,
    },
    /// A type declares a parent (`sub`) that does not exist in the schema.
    UnknownParent {
        /// The child type that references a missing parent.
        child: String,
        /// The parent type name that could not be found.
        parent: String,
    },
    /// Two definitions with the same name and kind were found in the schema.
    DuplicateDefinition {
        /// The duplicated type name.
        name: String,
        /// The kind of type that was duplicated (e.g. `"entity"`, `"relation"`, `"attribute"`).
        kind: String,
    },
    /// A semantic validation error (e.g. invalid cardinality bounds, bad regex pattern).
    ValidationError {
        /// Human-readable description of the validation failure.
        message: String,
    },
}

/// Formats a [`SchemaError`] into a human-readable error message.
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
            SchemaError::ValidationError { message } => {
                write!(f, "Validation error: {}", message)
            }
        }
    }
}

/// Enables [`SchemaError`] to be used as a standard Rust error type.
impl std::error::Error for SchemaError {}

// ---------------------------------------------------------------------------
// Cardinality
// ---------------------------------------------------------------------------

/// Ownership or role-play cardinality constraint, corresponding to the
/// TypeQL `@card(min..max)` annotation.
///
/// Examples:
/// - `@card(1..1)` -- exactly one (default for most ownership)
/// - `@card(0..5)` -- zero to five
/// - `@card(1..)` -- one or more (unbounded upper bound)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cardinality {
    /// Minimum number of values or role-players required.
    pub min: u32,
    /// Maximum number of values or role-players allowed, or `None` for unbounded.
    pub max: Option<u32>,
}

// ---------------------------------------------------------------------------
// OwnedAttribute
// ---------------------------------------------------------------------------

/// An attribute ownership declaration on an entity or relation type.
///
/// Corresponds to a TypeQL `owns <attribute>` clause, optionally annotated
/// with `@key`, `@unique`, `@cascade`, `@subkey`, or `@card`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnedAttribute {
    /// The name of the owned attribute type (must exist in the schema's attributes map).
    pub name: String,
    /// Whether this attribute is annotated with `@key`, making it a unique identifier.
    pub is_key: bool,
    /// Whether this attribute is annotated with `@unique`.
    pub is_unique: bool,
    /// Whether deleting this owner should cascade-delete the attribute instance.
    pub is_cascade: bool,
    /// Optional `@subkey(<label>)` group identifier for composite key membership.
    pub subkey_group: Option<String>,
    /// Optional `@card(min..max)` cardinality constraint on the ownership.
    pub cardinality: Option<Cardinality>,
}

// ---------------------------------------------------------------------------
// PlayedRole
// ---------------------------------------------------------------------------

/// A role that an entity or relation type can play, corresponding to a
/// TypeQL `plays <relation>:<role>` clause.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayedRole {
    /// Fully-qualified role reference in `"<relation>:<role>"` format (e.g. `"friendship:friend"`).
    pub role_ref: String,
    /// Optional `@card(min..max)` cardinality constraint on playing this role.
    pub cardinality: Option<Cardinality>,
}

// ---------------------------------------------------------------------------
// RoleSpec
// ---------------------------------------------------------------------------

/// A role defined within a relation type, corresponding to a TypeQL
/// `relates <role>` clause.
///
/// Roles may override a parent relation's role via the `as` keyword
/// (e.g. `relates author as contributor`), carry a cardinality constraint,
/// or be marked `@distinct`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleSpec {
    /// The local name of the role (e.g. `"friend"`, `"author"`).
    pub name: String,
    /// If this role overrides a parent relation's role, the parent role name (e.g. `"contributor"`).
    pub overrides: Option<String>,
    /// Optional `@card(min..max)` cardinality constraint on the role.
    pub cardinality: Option<Cardinality>,
    /// Whether the role is annotated with `@distinct`, requiring unique role-players.
    pub distinct: bool,
}

// ---------------------------------------------------------------------------
// AttributeType
// ---------------------------------------------------------------------------

/// A TypeDB attribute type definition.
///
/// Attribute types hold the value-type information (e.g. `string`, `long`,
/// `double`) and optional constraints such as `@regex`, `@values`, and
/// `@range`. They may also form an inheritance hierarchy via `sub`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttributeType {
    /// The attribute type name (e.g. `"name"`, `"email"`, `"age"`).
    pub name: String,
    /// The TypeDB value type (e.g. `"string"`, `"long"`, `"double"`, `"boolean"`, `"datetime"`).
    pub value_type: String,
    /// Optional parent attribute type name for inheritance (`sub` clause).
    pub parent: Option<String>,
    /// Whether the attribute type is declared `@abstract`.
    pub is_abstract: bool,
    /// Whether the attribute type is declared `@independent` (can exist without an owner).
    pub is_independent: bool,
    /// Optional `@regex` pattern constraining string attribute values.
    pub regex: Option<String>,
    /// Optional `@values` enumeration constraining allowed attribute values.
    pub allowed_values: Option<Vec<String>>,
    /// Optional lower bound of a `@range` constraint (inclusive), as a string literal.
    pub range_min: Option<String>,
    /// Optional upper bound of a `@range` constraint (inclusive), as a string literal.
    pub range_max: Option<String>,
}

// ---------------------------------------------------------------------------
// EntityType
// ---------------------------------------------------------------------------

/// A TypeDB entity type definition.
///
/// Entities are independent objects in the TypeDB type system. They can own
/// attributes, play roles in relations, and form an inheritance hierarchy
/// via the `sub` keyword.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityType {
    /// The entity type name (e.g. `"person"`, `"company"`).
    pub name: String,
    /// Optional parent entity type name for inheritance (`sub` clause).
    pub parent: Option<String>,
    /// Whether the entity type is declared `@abstract`.
    pub is_abstract: bool,
    /// Attributes owned by this entity type (includes inherited attributes after resolution).
    pub owns: Vec<OwnedAttribute>,
    /// Attribute names in declaration order (parent attributes first after inheritance resolution).
    pub owns_order: Vec<String>,
    /// Roles this entity type can play (includes inherited roles after resolution).
    pub plays: Vec<PlayedRole>,
}

// ---------------------------------------------------------------------------
// RelationType
// ---------------------------------------------------------------------------

/// A TypeDB relation type definition.
///
/// Relations connect entities (and other relations) via named roles. Like
/// entities, they can own attributes, play roles, and participate in an
/// inheritance hierarchy. Child relations may override parent roles using
/// the `as` keyword.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationType {
    /// The relation type name (e.g. `"friendship"`, `"employment"`).
    pub name: String,
    /// Optional parent relation type name for inheritance (`sub` clause).
    pub parent: Option<String>,
    /// Whether the relation type is declared `@abstract`.
    pub is_abstract: bool,
    /// Roles defined by this relation (includes inherited roles after resolution).
    pub roles: Vec<RoleSpec>,
    /// Attributes owned by this relation type (includes inherited attributes after resolution).
    pub owns: Vec<OwnedAttribute>,
    /// Attribute names in declaration order (parent attributes first after inheritance resolution).
    pub owns_order: Vec<String>,
    /// Roles this relation type can play (includes inherited roles after resolution).
    pub plays: Vec<PlayedRole>,
}

// ---------------------------------------------------------------------------
// TypeSchema
// ---------------------------------------------------------------------------

/// The complete parsed TypeDB schema containing all attribute, entity, and
/// relation type definitions.
///
/// This is the main entry point for working with a TypeDB schema in Rust.
/// Create one via [`TypeSchema::from_typeql()`] to parse a TypeQL `define`
/// block, which performs parsing, validation, and inheritance resolution in
/// one step. Alternatively, build one programmatically and call
/// `resolve_inheritance()` yourself.
///
/// After construction, use the lookup methods (`get_entity`, `get_relation`,
/// `get_attribute`) and the inheritance-aware accessors
/// (`get_all_owned_attributes`, `get_all_plays_roles`, `get_all_relates`)
/// to inspect the resolved type system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeSchema {
    /// All entity type definitions, keyed by type name. Sorted alphabetically (BTreeMap).
    pub entities: BTreeMap<String, EntityType>,
    /// All relation type definitions, keyed by type name. Sorted alphabetically (BTreeMap).
    pub relations: BTreeMap<String, RelationType>,
    /// All attribute type definitions, keyed by type name. Sorted alphabetically (BTreeMap).
    pub attributes: BTreeMap<String, AttributeType>,
}

/// Provides a default empty [`TypeSchema`] by delegating to [`TypeSchema::new()`].
impl Default for TypeSchema {
    fn default() -> Self {
        Self::new()
    }
}

/// Core methods for constructing, querying, validating, and resolving a [`TypeSchema`].
impl TypeSchema {
    /// Creates an empty `TypeSchema` with no types defined.
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
        schema.validate()?;
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

    /// Check if a type name exists anywhere (entity, relation, or attribute).
    pub fn type_exists(&self, name: &str) -> bool {
        self.entities.contains_key(name)
            || self.relations.contains_key(name)
            || self.attributes.contains_key(name)
    }

    /// Get the kind of a type: `"entity"`, `"relation"`, `"attribute"`, or `None`.
    pub fn type_kind(&self, name: &str) -> Option<&'static str> {
        if self.entities.contains_key(name) {
            Some("entity")
        } else if self.relations.contains_key(name) {
            Some("relation")
        } else if self.attributes.contains_key(name) {
            Some("attribute")
        } else {
            None
        }
    }

    // -----------------------------------------------------------------------
    // Semantic validation
    // -----------------------------------------------------------------------

    fn validate(&self) -> Result<(), SchemaError> {
        self.validate_cardinalities()?;
        self.validate_regex_patterns()?;
        self.validate_values()?;
        self.validate_subkeys()?;
        Ok(())
    }

    /// Check all @card annotations for min > max.
    fn validate_cardinalities(&self) -> Result<(), SchemaError> {
        fn check_card(card: &Option<Cardinality>) -> Result<(), SchemaError> {
            if let Some(c) = card
                && let Some(max) = c.max
                && c.min > max
            {
                return Err(SchemaError::ValidationError {
                    message: format!(
                        "Invalid @card annotation: minimum ({}) cannot be greater \
                         than maximum ({}). Use '@card({}..{})' instead.",
                        c.min, max, max, c.min
                    ),
                });
            }
            Ok(())
        }

        for entity in self.entities.values() {
            for own in &entity.owns {
                check_card(&own.cardinality)?;
            }
            for play in &entity.plays {
                check_card(&play.cardinality)?;
            }
        }
        for relation in self.relations.values() {
            for own in &relation.owns {
                check_card(&own.cardinality)?;
            }
            for play in &relation.plays {
                check_card(&play.cardinality)?;
            }
            for role in &relation.roles {
                check_card(&role.cardinality)?;
            }
        }
        Ok(())
    }

    /// Check all @regex patterns are valid.
    fn validate_regex_patterns(&self) -> Result<(), SchemaError> {
        for attr in self.attributes.values() {
            if let Some(ref pattern) = attr.regex
                && let Err(e) = regex::Regex::new(pattern)
            {
                return Err(SchemaError::ValidationError {
                    message: format!(
                        "Invalid @regex pattern: '{}'. \
                         Must be a valid regular expression. Error: {}",
                        pattern, e
                    ),
                });
            }
        }
        Ok(())
    }

    /// Check all @values for duplicates.
    fn validate_values(&self) -> Result<(), SchemaError> {
        for attr in self.attributes.values() {
            if let Some(ref values) = attr.allowed_values {
                let mut seen = HashSet::new();
                let mut duplicates = Vec::new();
                for v in values {
                    if !seen.insert(v.as_str()) {
                        duplicates.push(v.clone());
                    }
                }
                if !duplicates.is_empty() {
                    return Err(SchemaError::ValidationError {
                        message: format!(
                            "Invalid @values annotation: duplicate values found: {:?}. \
                             Each value must be unique.",
                            duplicates
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    /// Check all @subkey identifiers are valid.
    fn validate_subkeys(&self) -> Result<(), SchemaError> {
        fn check_subkey(id: &str) -> Result<(), SchemaError> {
            if id.is_empty() {
                return Err(SchemaError::ValidationError {
                    message: "Invalid @subkey identifier: empty string.".to_string(),
                });
            }
            let first = id.chars().next().unwrap();
            if !unicode_ident::is_xid_start(first) && first != '_' {
                return Err(SchemaError::ValidationError {
                    message: format!(
                        "Invalid @subkey identifier: '{}'. \
                         Must start with a letter or underscore.",
                        id
                    ),
                });
            }
            for ch in id[first.len_utf8()..].chars() {
                if !unicode_ident::is_xid_continue(ch) && ch != '-' {
                    return Err(SchemaError::ValidationError {
                        message: format!(
                            "Invalid @subkey identifier: '{}'. \
                             Contains invalid character '{}'.",
                            id, ch
                        ),
                    });
                }
            }
            Ok(())
        }

        for entity in self.entities.values() {
            for own in &entity.owns {
                if let Some(ref id) = own.subkey_group {
                    check_subkey(id)?;
                }
            }
        }
        for relation in self.relations.values() {
            for own in &relation.owns {
                if let Some(ref id) = own.subkey_group {
                    check_subkey(id)?;
                }
            }
        }
        Ok(())
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

    // --- Validation tests ---

    #[test]
    fn test_validate_card_min_gt_max() {
        let result = TypeSchema::from_typeql(
            "define\nentity person, owns name @card(5..1);",
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("minimum (5)"), "expected min in msg: {}", msg);
        assert!(msg.contains("maximum (1)"), "expected max in msg: {}", msg);
    }

    #[test]
    fn test_validate_card_valid_passes() {
        let result = TypeSchema::from_typeql(
            "define\nentity person, owns name @card(1..5);",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_card_exact_passes() {
        let result = TypeSchema::from_typeql(
            "define\nentity person, owns name @card(3);",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_card_unbounded_passes() {
        let result = TypeSchema::from_typeql(
            "define\nentity person, owns name @card(1..);",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_regex_invalid() {
        let result = TypeSchema::from_typeql(
            "define\nattribute email, value string, @regex(\"[invalid(\");",
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("regex"), "expected 'regex' in msg: {}", msg);
    }

    #[test]
    fn test_validate_regex_valid_passes() {
        let result = TypeSchema::from_typeql(
            "define\nattribute email, value string, @regex(\"^[a-z]+@[a-z]+\\\\.[a-z]+$\");",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_values_duplicate() {
        let result = TypeSchema::from_typeql(
            "define\nattribute status, value string, @values(\"active\", \"inactive\", \"active\");",
        );
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("duplicate"), "expected 'duplicate' in msg: {}", msg);
    }

    #[test]
    fn test_validate_values_valid_passes() {
        let result = TypeSchema::from_typeql(
            "define\nattribute status, value string, @values(\"active\", \"inactive\");",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_subkey_invalid_start() {
        let mut schema = TypeSchema::new();
        schema.entities.insert(
            "person".to_string(),
            EntityType {
                name: "person".to_string(),
                parent: None,
                is_abstract: false,
                owns: vec![OwnedAttribute {
                    name: "name".to_string(),
                    is_key: false,
                    is_unique: false,
                    is_cascade: false,
                    subkey_group: Some("123abc".to_string()),
                    cardinality: None,
                }],
                owns_order: vec!["name".to_string()],
                plays: vec![],
            },
        );
        let result = schema.validate();
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("start with a letter"), "expected start msg: {}", msg);
    }

    #[test]
    fn test_validate_subkey_invalid_char() {
        let mut schema = TypeSchema::new();
        schema.entities.insert(
            "person".to_string(),
            EntityType {
                name: "person".to_string(),
                parent: None,
                is_abstract: false,
                owns: vec![OwnedAttribute {
                    name: "name".to_string(),
                    is_key: false,
                    is_unique: false,
                    is_cascade: false,
                    subkey_group: Some("invalid@char".to_string()),
                    cardinality: None,
                }],
                owns_order: vec!["name".to_string()],
                plays: vec![],
            },
        );
        let result = schema.validate();
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("invalid character"), "expected char msg: {}", msg);
    }

    #[test]
    fn test_validate_subkey_valid_passes() {
        let mut schema = TypeSchema::new();
        schema.entities.insert(
            "person".to_string(),
            EntityType {
                name: "person".to_string(),
                parent: None,
                is_abstract: false,
                owns: vec![OwnedAttribute {
                    name: "name".to_string(),
                    is_key: false,
                    is_unique: false,
                    is_cascade: false,
                    subkey_group: Some("user-id".to_string()),
                    cardinality: None,
                }],
                owns_order: vec!["name".to_string()],
                plays: vec![],
            },
        );
        assert!(schema.validate().is_ok());
    }
}
