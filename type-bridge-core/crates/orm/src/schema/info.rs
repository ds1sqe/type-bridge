//! Type-erased schema metadata containers.
//!
//! These types store metadata extracted from `TypeBridgeEntity` and
//! `TypeBridgeRelation` trait impls at registration time, enabling
//! runtime schema operations without generic type parameters.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use type_bridge_core_lib::schema as core_schema;

use crate::attribute::ValueType;
use crate::descriptor::{OwnedAttributeDescriptor, TypeDescriptor};
use crate::entity::Annotation;

use super::diff::SchemaDiff;
use super::error::SchemaError;
use super::generator;

/// Metadata for one owned attribute in a schema entry (owned `String` version).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnedAttributeEntry {
    /// Attribute type name.
    pub attr_name: String,
    /// Value type.
    pub value_type: ValueType,
    /// Ownership annotations.
    pub annotations: Vec<Annotation>,
    /// Whether this ownership is declared as an ordered list (`owns name[]`).
    ///
    /// Instance-level list semantics are engine-unimplemented (REP256); this field
    /// is a schema-emission marker only.
    #[serde(default)]
    pub is_ordered: bool,
}

impl OwnedAttributeEntry {
    /// Format the annotation flags as a TypeQL annotation string.
    ///
    /// Returns strings like `"@key"`, `"@unique"`, `"@distinct"`, `"@card(2..5)"`.
    pub fn flags_string(&self) -> String {
        let mut parts = Vec::new();
        for ann in &self.annotations {
            match ann {
                Annotation::Key => parts.push("@key".to_string()),
                Annotation::Unique => parts.push("@unique".to_string()),
                Annotation::Distinct => parts.push("@distinct".to_string()),
                Annotation::Card(min, max) => {
                    // TypeDB 3.x spells an unbounded upper bound as `@card(min..)`.
                    let max_str = match max {
                        Some(m) => m.to_string(),
                        None => String::new(),
                    };
                    parts.push(format!("@card({min}..{max_str})"));
                }
            }
        }
        parts.join(" ")
    }
}

/// Metadata for one role in a relation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleEntry {
    /// Role name (e.g. `"employee"`).
    pub role_name: String,
    /// Entity types that can play this role.
    pub player_type_names: Vec<String>,
    /// Optional role cardinality, where `None` max means unbounded.
    pub cardinality: Option<(u32, Option<u32>)>,
    /// Parent role name this role specializes, or `None` for plain roles.
    #[serde(default)]
    pub overrides: Option<String>,
    /// Whether this role carries a schema-level `@abstract` annotation.
    #[serde(default)]
    pub is_abstract: bool,
    /// Whether this role is declared as an ordered list (`relates name[]`).
    ///
    /// Instance-level list semantics are engine-unimplemented (REP256); this field
    /// is a schema-emission marker only.
    #[serde(default)]
    pub ordered: bool,
    /// Whether this role carries a schema-level `@distinct` annotation.
    ///
    /// Valid only when `ordered` is `true`.
    #[serde(default)]
    pub distinct: bool,
}

/// Schema entry for an entity type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntitySchemaEntry {
    /// Entity type name.
    pub type_name: String,
    /// Whether this entity is abstract.
    pub is_abstract: bool,
    /// Parent entity type name (for `sub` hierarchies).
    pub parent_type: Option<String>,
    /// Owned attributes.
    pub owned_attributes: Vec<OwnedAttributeEntry>,
    /// Plays-side cardinality overlay, keyed by role_ref `"{relation}:{role}"`.
    ///
    /// Distinct from relates-side `RoleEntry.cardinality`: this constrains
    /// relations-per-player, not players-per-relation. Populated from
    /// `PlayedRole.cardinality`; empty when no plays-card is declared.
    #[serde(default)]
    pub plays_cardinalities: BTreeMap<String, (u32, Option<u32>)>,
}

/// Schema entry for a relation type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Plays-side cardinality overlay, keyed by role_ref `"{relation}:{role}"`.
    ///
    /// Distinct from relates-side `RoleEntry.cardinality`: this constrains
    /// relations-per-player, not players-per-relation. Populated from
    /// `PlayedRole.cardinality`; empty when no plays-card is declared.
    #[serde(default)]
    pub plays_cardinalities: BTreeMap<String, (u32, Option<u32>)>,
}

/// Metadata for a standalone attribute type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttributeSchemaEntry {
    /// Attribute type name.
    pub attr_name: String,
    /// Value type.
    pub value_type: ValueType,
    /// Parent attribute type name (for `sub` hierarchies).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_type: Option<String>,
    /// Whether this attribute type is abstract.
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_abstract: bool,
    /// Whether this attribute type is independent.
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_independent: bool,
    /// Optional `@regex` pattern.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
    /// Optional `@values` allowlist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_values: Option<Vec<String>>,
    /// Optional `@range` bounds. `None` bound means open-ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<(Option<String>, Option<String>)>,
}

impl AttributeSchemaEntry {
    /// Build an unconstrained attribute schema entry.
    pub fn new(attr_name: impl Into<String>, value_type: ValueType) -> Self {
        Self {
            attr_name: attr_name.into(),
            value_type,
            parent_type: None,
            is_abstract: false,
            is_independent: false,
            regex: None,
            allowed_values: None,
            range: None,
        }
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Complete schema information extracted from registered models.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaInfo {
    /// Registered entity types, keyed by type name.
    pub entities: BTreeMap<String, EntitySchemaEntry>,
    /// Registered relation types, keyed by type name.
    pub relations: BTreeMap<String, RelationSchemaEntry>,
    /// All attribute types referenced by entities and relations.
    pub attributes: BTreeMap<String, AttributeSchemaEntry>,
}

impl SchemaInfo {
    /// Parse an exported TypeDB `define` schema into migration-facing schema IR.
    ///
    /// This is used for live introspection because TypeDB's schema export
    /// preserves annotations that are not currently queryable through schema
    /// match clauses.
    pub fn from_typeql(input: &str) -> Result<Self, SchemaError> {
        if input.trim() == "define" {
            return Ok(Self::default());
        }

        let schema = core_schema::TypeSchema::from_typeql(input).map_err(|error| {
            SchemaError::Validation {
                message: error.to_string(),
            }
        })?;
        Ok(Self::from_type_schema(&schema))
    }

    /// Bridge the parser-facing TypeQL schema into the migration-facing schema IR.
    pub fn from_type_schema(schema: &core_schema::TypeSchema) -> Self {
        let mut info = Self::default();

        for (attr_name, attr) in &schema.attributes {
            info.attributes.insert(
                attr_name.clone(),
                AttributeSchemaEntry {
                    attr_name: attr_name.clone(),
                    value_type: value_type_from_typeql(&attr.value_type),
                    parent_type: attr.parent.clone(),
                    is_abstract: attr.is_abstract,
                    is_independent: attr.is_independent,
                    regex: attr.regex.clone(),
                    allowed_values: attr.allowed_values.clone(),
                    range: match (&attr.range_min, &attr.range_max) {
                        (None, None) => None,
                        (min, max) => Some((min.clone(), max.clone())),
                    },
                },
            );
        }

        let PlaysData {
            role_players,
            plays_cards,
        } = plays_data_from_schema(schema);

        for (entity_name, entity) in &schema.entities {
            let plays_cardinalities = plays_cards.get(entity_name).cloned().unwrap_or_default();
            info.entities.insert(
                entity_name.clone(),
                EntitySchemaEntry {
                    type_name: entity_name.clone(),
                    is_abstract: entity.is_abstract,
                    parent_type: entity.parent.clone(),
                    owned_attributes: owned_attribute_entries_from_typeql(
                        &entity.owns,
                        &info.attributes,
                    ),
                    plays_cardinalities,
                },
            );
        }

        for (relation_name, relation) in &schema.relations {
            let roles = relation
                .roles
                .iter()
                .map(|role| {
                    let player_type_names = role_players
                        .get(&(relation_name.clone(), role.name.clone()))
                        .map(|players| players.iter().cloned().collect())
                        .unwrap_or_default();
                    RoleEntry {
                        role_name: role.name.clone(),
                        player_type_names,
                        cardinality: role.cardinality.as_ref().map(cardinality_tuple),
                        overrides: role.overrides.clone(),
                        is_abstract: role.is_abstract,
                        ordered: role.ordered,
                        distinct: role.distinct,
                    }
                })
                .collect();

            let plays_cardinalities = plays_cards.get(relation_name).cloned().unwrap_or_default();

            info.relations.insert(
                relation_name.clone(),
                RelationSchemaEntry {
                    type_name: relation_name.clone(),
                    is_abstract: relation.is_abstract,
                    parent_type: relation.parent.clone(),
                    owned_attributes: owned_attribute_entries_from_typeql(
                        &relation.owns,
                        &info.attributes,
                    ),
                    roles,
                    plays_cardinalities,
                },
            );
        }

        info
    }

    /// Bridge the CRUD-facing descriptor IR into the migration-facing schema IR.
    ///
    /// Descriptors are what Python models register for CRUD; the diff and
    /// breaking-change engine reads `SchemaInfo`. This constructor is the only
    /// crossing between the two — there is no second schema engine. Relation
    /// roles keep their player set grouped because cardinality belongs to the
    /// role, not to each individual player type.
    ///
    /// After the initial entry build, two post-passes run:
    ///
    /// **Plays-cardinality overlay**: for every relation descriptor role that carries
    /// `plays_cardinality: Some(card)`, each named player type that is present in the
    /// built entries (entity or relation) receives a `"{relation_type_name}:{role_name}"`
    /// key in its `plays_cardinalities` map. This centralises the plays-card data so
    /// every registry consumer (sync, migration, node) gets define-correct plays-card
    /// without binding-side post-passes.
    ///
    /// **Foreign-parent nulling**: any entity or relation entry whose `parent_type`
    /// names a type absent from the built entries has its `parent_type` set to `None`.
    /// This is a DEFINE-EMISSION rule: `sub <unregistered>` cannot be emitted and
    /// deliberately discards the raw parent string. It is not a neutral normalization.
    pub fn from_descriptors(descriptors: &[TypeDescriptor]) -> Self {
        let mut info = Self::default();

        for descriptor in descriptors {
            match descriptor {
                TypeDescriptor::Entity(entity) => {
                    register_attribute_types(&mut info, &entity.owned_attributes);
                    info.entities.insert(
                        entity.type_name.clone(),
                        EntitySchemaEntry {
                            type_name: entity.type_name.clone(),
                            is_abstract: entity.is_abstract,
                            parent_type: entity.parent_type.clone(),
                            owned_attributes: owned_attribute_entries(&entity.owned_attributes),
                            plays_cardinalities: BTreeMap::new(),
                        },
                    );
                }
                TypeDescriptor::Relation(relation) => {
                    register_attribute_types(&mut info, &relation.owned_attributes);
                    let roles = relation
                        .roles
                        .iter()
                        .map(|role| RoleEntry {
                            role_name: role.role_name.clone(),
                            player_type_names: role.player_type_names.clone(),
                            cardinality: role.cardinality,
                            overrides: role.overrides.clone(),
                            is_abstract: role.is_abstract,
                            ordered: role.ordered,
                            distinct: role.distinct,
                        })
                        .collect();

                    info.relations.insert(
                        relation.type_name.clone(),
                        RelationSchemaEntry {
                            type_name: relation.type_name.clone(),
                            is_abstract: relation.is_abstract,
                            parent_type: relation.parent_type.clone(),
                            owned_attributes: owned_attribute_entries(&relation.owned_attributes),
                            roles,
                            plays_cardinalities: BTreeMap::new(),
                        },
                    );
                }
            }
        }

        // Plays-cardinality overlay pass: propagate descriptor-authored plays-card
        // values onto each listed player's entry. Collecting the overlays first avoids
        // simultaneous mutable borrows of `info.entities` and `info.relations`.
        let mut entity_overlays: BTreeMap<String, BTreeMap<String, (u32, Option<u32>)>> =
            BTreeMap::new();
        let mut relation_overlays: BTreeMap<String, BTreeMap<String, (u32, Option<u32>)>> =
            BTreeMap::new();

        for descriptor in descriptors {
            if let TypeDescriptor::Relation(relation) = descriptor {
                for role in &relation.roles {
                    if let Some(card) = role.plays_cardinality {
                        let role_ref = format!("{}:{}", relation.type_name, role.role_name);
                        for player in &role.player_type_names {
                            if info.entities.contains_key(player) {
                                entity_overlays
                                    .entry(player.clone())
                                    .or_default()
                                    .insert(role_ref.clone(), card);
                            } else if info.relations.contains_key(player) {
                                relation_overlays
                                    .entry(player.clone())
                                    .or_default()
                                    .insert(role_ref.clone(), card);
                            }
                            // Players absent from the snapshot are skipped; no panic.
                        }
                    }
                }
            }
        }

        for (player, overlays) in entity_overlays {
            if let Some(entry) = info.entities.get_mut(&player) {
                entry.plays_cardinalities.extend(overlays);
            }
        }
        for (player, overlays) in relation_overlays {
            if let Some(entry) = info.relations.get_mut(&player) {
                entry.plays_cardinalities.extend(overlays);
            }
        }

        // Foreign-parent nulling pass: discard parent references that name a type not
        // present in this snapshot. `sub <unregistered>` cannot be emitted in a define
        // block, so the raw parent string is deliberately dropped rather than propagated.
        let known: BTreeSet<String> = info
            .entities
            .keys()
            .chain(info.relations.keys())
            .cloned()
            .collect();

        for entry in info.entities.values_mut() {
            if entry
                .parent_type
                .as_ref()
                .is_some_and(|p| !known.contains(p))
            {
                entry.parent_type = None;
            }
        }
        for entry in info.relations.values_mut() {
            if entry
                .parent_type
                .as_ref()
                .is_some_and(|p| !known.contains(p))
            {
                entry.parent_type = None;
            }
        }

        info
    }

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

fn owned_attribute_entries_from_typeql(
    attrs: &[core_schema::OwnedAttribute],
    known_attrs: &BTreeMap<String, AttributeSchemaEntry>,
) -> Vec<OwnedAttributeEntry> {
    attrs
        .iter()
        .map(|attr| OwnedAttributeEntry {
            attr_name: attr.name.clone(),
            value_type: known_attrs
                .get(&attr.name)
                .map(|entry| entry.value_type)
                .unwrap_or(ValueType::String),
            annotations: annotations_from_typeql(attr),
            is_ordered: attr.ordered,
        })
        .collect()
}

fn annotations_from_typeql(attr: &core_schema::OwnedAttribute) -> Vec<Annotation> {
    let mut annotations = Vec::new();
    if attr.is_key {
        annotations.push(Annotation::Key);
    }
    if attr.is_unique {
        annotations.push(Annotation::Unique);
    }
    if attr.distinct {
        annotations.push(Annotation::Distinct);
    }
    if let Some(cardinality) = &attr.cardinality {
        annotations.push(Annotation::Card(cardinality.min, cardinality.max));
    }
    annotations
}

/// `plays`-derived data, built in a single pass over the schema. Both maps come
/// from the same `PlayedRole` walk, so they are collected together rather than in
/// two separate traversals.
struct PlaysData {
    /// (relation, role) → player type names. Players are grouped because
    /// relates-side cardinality belongs to the role, not the individual player.
    role_players: BTreeMap<(String, String), BTreeSet<String>>,
    /// player type name → (role_ref → (min, max)). Only `@card`-annotated plays
    /// appear; bare plays are omitted so the overlay stays empty by default,
    /// preserving the bare `… plays r:role;` emission.
    plays_cards: BTreeMap<String, BTreeMap<String, (u32, Option<u32>)>>,
}

fn plays_data_from_schema(schema: &core_schema::TypeSchema) -> PlaysData {
    let mut data = PlaysData {
        role_players: BTreeMap::new(),
        plays_cards: BTreeMap::new(),
    };

    for (entity_name, entity) in &schema.entities {
        accumulate_plays(&mut data, entity_name, &entity.plays);
    }
    for (relation_name, relation) in &schema.relations {
        accumulate_plays(&mut data, relation_name, &relation.plays);
    }

    data
}

/// Fold one player type's `plays` clauses into both the role→players inversion
/// and the plays-card overlay in a single walk. A bare plays contributes only to
/// the inversion; a `@card`-annotated plays also seeds the overlay.
fn accumulate_plays(
    data: &mut PlaysData,
    player_type_name: &str,
    played_roles: &[core_schema::PlayedRole],
) {
    for played in played_roles {
        if let Some((relation_name, role_name)) = split_role_ref(&played.role_ref) {
            data.role_players
                .entry((relation_name.to_string(), role_name.to_string()))
                .or_default()
                .insert(player_type_name.to_string());
        }
        if let Some(cardinality) = &played.cardinality {
            data.plays_cards
                .entry(player_type_name.to_string())
                .or_default()
                .insert(played.role_ref.clone(), cardinality_tuple(cardinality));
        }
    }
}

fn split_role_ref(role_ref: &str) -> Option<(&str, &str)> {
    let (relation_name, role_name) = role_ref.split_once(':')?;
    if relation_name.is_empty() || role_name.is_empty() {
        return None;
    }
    Some((relation_name, role_name))
}

fn cardinality_tuple(cardinality: &core_schema::Cardinality) -> (u32, Option<u32>) {
    (cardinality.min, cardinality.max)
}

fn value_type_from_typeql(value_type: &str) -> ValueType {
    match value_type {
        "integer" => ValueType::Long,
        other => ValueType::parse(other).unwrap_or(ValueType::String),
    }
}

fn owned_attribute_entries(attributes: &[OwnedAttributeDescriptor]) -> Vec<OwnedAttributeEntry> {
    attributes
        .iter()
        .map(|attr| {
            // Python descriptors emit scalar optionality as `Annotation::Card(0, Some(1))`.
            // `is_optional` remains descriptor-only dynamic-manager metadata.
            OwnedAttributeEntry {
                attr_name: attr.attr_name.clone(),
                value_type: attr.value_type,
                annotations: attr.annotations.clone(),
                is_ordered: attr.is_ordered,
            }
        })
        .collect()
}

fn register_attribute_types(info: &mut SchemaInfo, attributes: &[OwnedAttributeDescriptor]) {
    for attr in attributes {
        info.attributes
            .entry(attr.attr_name.clone())
            .or_insert_with(|| AttributeSchemaEntry::new(attr.attr_name.clone(), attr.value_type));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{
        EntityDescriptor, OwnedAttributeDescriptor, RelationDescriptor, RoleDescriptor,
    };

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
                plays_cardinalities: BTreeMap::new(),
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
                plays_cardinalities: BTreeMap::new(),
            },
        );
        assert!(info.get_relation_by_name("employment").is_some());
    }

    #[test]
    fn validate_passes_for_consistent_attrs() {
        let mut info = SchemaInfo::default();
        info.attributes.insert(
            "name".into(),
            AttributeSchemaEntry::new("name", ValueType::String),
        );
        info.attributes.insert(
            "age".into(),
            AttributeSchemaEntry::new("age", ValueType::Long),
        );
        assert!(info.validate().is_ok());
    }

    #[test]
    fn flags_string_key() {
        let entry = OwnedAttributeEntry {
            attr_name: "name".into(),
            value_type: ValueType::String,
            annotations: vec![Annotation::Key],
            is_ordered: false,
        };
        assert_eq!(entry.flags_string(), "@key");
    }

    #[test]
    fn flags_string_card_bounded() {
        let entry = OwnedAttributeEntry {
            attr_name: "tag".into(),
            value_type: ValueType::String,
            annotations: vec![Annotation::Card(2, Some(5))],
            is_ordered: false,
        };
        assert_eq!(entry.flags_string(), "@card(2..5)");
    }

    #[test]
    fn flags_string_card_unbounded() {
        let entry = OwnedAttributeEntry {
            attr_name: "phone".into(),
            value_type: ValueType::String,
            annotations: vec![Annotation::Card(0, None)],
            is_ordered: false,
        };
        assert_eq!(entry.flags_string(), "@card(0..)");
    }

    #[test]
    fn flags_string_multiple() {
        let entry = OwnedAttributeEntry {
            attr_name: "email".into(),
            value_type: ValueType::String,
            annotations: vec![Annotation::Unique, Annotation::Card(1, Some(3))],
            is_ordered: false,
        };
        assert_eq!(entry.flags_string(), "@unique @card(1..3)");
    }

    #[test]
    fn schema_info_serde_roundtrip() {
        let mut info = SchemaInfo::default();
        info.entities.insert(
            "person".into(),
            EntitySchemaEntry {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeEntry {
                    attr_name: "name".into(),
                    value_type: ValueType::String,
                    annotations: vec![Annotation::Key],
                    is_ordered: false,
                }],
                plays_cardinalities: BTreeMap::new(),
            },
        );
        info.relations.insert(
            "employment".into(),
            RelationSchemaEntry {
                type_name: "employment".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![RoleEntry {
                    role_name: "employee".into(),
                    player_type_names: vec!["person".into()],
                    cardinality: None,
                    overrides: None,
                    is_abstract: false,
                    ..Default::default()
                }],
                plays_cardinalities: BTreeMap::new(),
            },
        );
        info.attributes.insert(
            "name".into(),
            AttributeSchemaEntry::new("name", ValueType::String),
        );

        let json = serde_json::to_string(&info).unwrap();
        let parsed: SchemaInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info.entities.len(), parsed.entities.len());
        assert_eq!(info.relations.len(), parsed.relations.len());
        assert_eq!(info.attributes.len(), parsed.attributes.len());
        assert_eq!(
            info.entities.get("person").unwrap().owned_attributes,
            parsed.entities.get("person").unwrap().owned_attributes,
        );
    }

    #[test]
    fn from_descriptors_lowers_entities_relations_roles_and_attribute_side_map() {
        let descriptors = vec![
            TypeDescriptor::Entity(EntityDescriptor {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: Some("thing".into()),
                owned_attributes: vec![OwnedAttributeDescriptor {
                    field_name: "name".into(),
                    attr_name: "name".into(),
                    value_type: ValueType::String,
                    annotations: vec![Annotation::Key],
                    is_optional: false,
                    is_ordered: false,
                }],
            }),
            TypeDescriptor::Relation(RelationDescriptor {
                type_name: "employment".into(),
                is_abstract: true,
                parent_type: Some("contract".into()),
                owned_attributes: vec![OwnedAttributeDescriptor {
                    field_name: "since".into(),
                    attr_name: "since".into(),
                    value_type: ValueType::Date,
                    annotations: vec![],
                    is_optional: false,
                    is_ordered: false,
                }],
                roles: vec![RoleDescriptor {
                    role_name: "participant".into(),
                    player_type_names: vec!["person".into(), "company".into()],
                    cardinality: Some((1, Some(2))),
                    overrides: None,
                    is_abstract: false,
                    ordered: false,
                    distinct: false,
                    plays_cardinality: None,
                }],
            }),
        ];

        let info = SchemaInfo::from_descriptors(&descriptors);

        // "thing" and "contract" are not in the snapshot, so foreign-parent nulling
        // clears both parent_type fields.
        assert_eq!(
            info.entities.get("person"),
            Some(&EntitySchemaEntry {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeEntry {
                    attr_name: "name".into(),
                    value_type: ValueType::String,
                    annotations: vec![Annotation::Key],
                    is_ordered: false,
                }],
                plays_cardinalities: BTreeMap::new(),
            })
        );
        assert_eq!(
            info.relations.get("employment"),
            Some(&RelationSchemaEntry {
                type_name: "employment".into(),
                is_abstract: true,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeEntry {
                    attr_name: "since".into(),
                    value_type: ValueType::Date,
                    annotations: vec![],
                    is_ordered: false,
                }],
                roles: vec![RoleEntry {
                    role_name: "participant".into(),
                    player_type_names: vec!["person".into(), "company".into()],
                    cardinality: Some((1, Some(2))),
                    overrides: None,
                    is_abstract: false,
                    ordered: false,
                    distinct: false,
                },],
                plays_cardinalities: BTreeMap::new(),
            })
        );
        assert_eq!(
            info.attributes.get("name"),
            Some(&AttributeSchemaEntry::new("name", ValueType::String))
        );
        assert_eq!(
            info.attributes.get("since"),
            Some(&AttributeSchemaEntry::new("since", ValueType::Date))
        );
    }

    #[test]
    fn from_descriptors_preserves_cardinality_annotation_for_optional_attributes() {
        let descriptors = vec![TypeDescriptor::Entity(EntityDescriptor {
            type_name: "person".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![OwnedAttributeDescriptor {
                field_name: "age".into(),
                attr_name: "age".into(),
                value_type: ValueType::Long,
                annotations: vec![Annotation::Card(0, Some(1))],
                is_optional: true,
                is_ordered: false,
            }],
        })];

        let info = SchemaInfo::from_descriptors(&descriptors);
        let attr = &info.entities["person"].owned_attributes[0];

        assert_eq!(attr.annotations, vec![Annotation::Card(0, Some(1))]);
        assert_eq!(attr.flags_string(), "@card(0..1)");
    }

    #[test]
    fn from_descriptors_empty_yields_empty_schema() {
        let info = SchemaInfo::from_descriptors(&[]);

        assert!(info.entities.is_empty());
        assert!(info.relations.is_empty());
        assert!(info.attributes.is_empty());
    }

    #[test]
    fn from_typeql_preserves_attribute_type_annotations() {
        let typeql = r#"
define

attribute base-token @abstract, value string;
attribute email sub base-token, value string @regex("^[a-z]+@[a-z]+\.[a-z]+$");
attribute status @independent, value string @values("active", "inactive");
attribute age, value integer @range(0..150);
"#;
        let info = SchemaInfo::from_typeql(typeql).expect("parse failed");

        let base = info
            .attributes
            .get("base-token")
            .expect("base-token missing");
        assert!(base.is_abstract);

        let email = info.attributes.get("email").expect("email missing");
        assert_eq!(email.parent_type.as_deref(), Some("base-token"));
        assert_eq!(email.regex.as_deref(), Some(r"^[a-z]+@[a-z]+\.[a-z]+$"));

        let status = info.attributes.get("status").expect("status missing");
        assert!(status.is_independent);
        assert_eq!(
            status.allowed_values,
            Some(vec!["active".into(), "inactive".into()])
        );

        let age = info.attributes.get("age").expect("age missing");
        assert_eq!(age.range, Some((Some("0".into()), Some("150".into()))));

        let emitted = info.to_typeql().expect("emit failed");
        assert!(emitted.contains(
            r#"attribute email sub base-token, value string @regex("^[a-z]+@[a-z]+\.[a-z]+$");"#
        ));
        assert!(emitted.contains(
            r#"attribute status @independent, value string @values("active", "inactive");"#
        ));
        assert!(emitted.contains("attribute age, value integer @range(0..150);"));
    }

    #[test]
    fn attribute_schema_entry_serde_defaults_preserve_old_json_shape() {
        let json = r#"{"attr_name":"name","value_type":"string"}"#;
        let entry: AttributeSchemaEntry = serde_json::from_str(json).expect("deserialize failed");

        assert_eq!(entry, AttributeSchemaEntry::new("name", ValueType::String));
        assert_eq!(
            serde_json::to_string(&entry).expect("serialize failed"),
            r#"{"attr_name":"name","value_type":"string"}"#
        );
    }

    /// `from_typeql` populates `plays_cardinalities` when a `@card` annotation
    /// is present on a plays clause.
    #[test]
    fn from_typeql_populates_plays_cardinalities() {
        // The parser merges entity blocks that share the same type name, so the
        // plays clause can live in a second entity block after the relation.
        let typeql = r#"
define

attribute allowed-val, value string;
attribute allowed-nm, value string;

entity tval, owns allowed-nm @key;
entity tattr, owns allowed-nm @key;

relation accepts, relates allowed-val, relates allowed-nm;

entity tval, plays accepts:allowed-val @card(0..1);
entity tattr, plays accepts:allowed-nm;
"#;
        let info = SchemaInfo::from_typeql(typeql).expect("parse failed");

        let tval = info.entities.get("tval").expect("tval missing");
        assert_eq!(
            tval.plays_cardinalities.get("accepts:allowed-val"),
            Some(&(0, Some(1))),
            "tval should have plays-card for accepts:allowed-val"
        );

        // tattr plays without @card → its map must be empty
        let tattr = info.entities.get("tattr").expect("tattr missing");
        assert!(
            tattr.plays_cardinalities.is_empty(),
            "tattr has no plays-card; map should be empty"
        );
    }

    /// Per-player boundary: two players of the same role, only one has plays-card.
    #[test]
    fn plays_cardinalities_per_player_boundary() {
        let typeql = r#"
define

attribute label, value string;

entity player-a, owns label @key;
entity player-b, owns label @key;

relation link, relates member;

entity player-a, plays link:member @card(1..1);
entity player-b, plays link:member;
"#;
        let info = SchemaInfo::from_typeql(typeql).expect("parse failed");

        let a = info.entities.get("player-a").expect("player-a missing");
        assert_eq!(
            a.plays_cardinalities.get("link:member"),
            Some(&(1, Some(1))),
            "player-a should have plays-card"
        );

        let b = info.entities.get("player-b").expect("player-b missing");
        assert!(
            !b.plays_cardinalities.contains_key("link:member"),
            "player-b should have no plays-card for link:member"
        );
    }

    /// Integration smoke: a plays-card authored as inline TypeQL survives the
    /// full parse → IR → emit boundary. The parser reads the inline
    /// `entity X, plays r:role` form (TypeDB's export grammar); the emitter
    /// renders the standalone `X plays r:role;` form (TypeDB's define grammar).
    /// This is the realistic introspect-then-re-emit flow, not byte idempotence
    /// — the two grammars are intentionally asymmetric, so
    /// `from_typeql(to_typeql(..))` is not a valid oracle.
    #[test]
    fn plays_card_survives_parse_to_emit_boundary() {
        let typeql = r#"
define

attribute label, value string;

entity holder, owns label @key;

relation owns-one, relates slot;

entity holder, plays owns-one:slot @card(0..1);
"#;
        let info = SchemaInfo::from_typeql(typeql).expect("parse failed");
        let emitted = info.to_typeql().expect("emit failed");
        assert!(
            emitted.contains("holder plays owns-one:slot @card(0..1);"),
            "plays-card must survive parse->IR->emit: {emitted}"
        );
    }

    /// Serde round-trip: a `SchemaInfo` carrying a `plays_cardinalities` entry
    /// serializes and deserializes correctly.
    #[test]
    fn plays_cardinalities_serde_roundtrip() {
        let mut cards = BTreeMap::new();
        cards.insert("accepts:allowed-value".to_string(), (0u32, Some(1u32)));

        let mut info = SchemaInfo::default();
        info.entities.insert(
            "tval".into(),
            EntitySchemaEntry {
                type_name: "tval".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                plays_cardinalities: cards.clone(),
            },
        );

        let json = serde_json::to_string(&info).expect("serialize failed");
        let parsed: SchemaInfo = serde_json::from_str(&json).expect("deserialize failed");

        assert_eq!(
            parsed.entities.get("tval").unwrap().plays_cardinalities,
            cards,
            "plays_cardinalities must round-trip through JSON"
        );
    }

    /// Serde back-compat: a JSON payload that omits `plays_cardinalities`
    /// deserializes successfully and defaults to an empty map.
    #[test]
    fn plays_cardinalities_serde_default_when_absent() {
        // JSON for an EntitySchemaEntry without the plays_cardinalities field
        let json = r#"{
            "entities": {
                "person": {
                    "type_name": "person",
                    "is_abstract": false,
                    "parent_type": null,
                    "owned_attributes": []
                }
            },
            "relations": {},
            "attributes": {}
        }"#;

        let info: SchemaInfo = serde_json::from_str(json).expect("deserialize failed");
        let person = info.entities.get("person").expect("person missing");
        assert!(
            person.plays_cardinalities.is_empty(),
            "missing plays_cardinalities field should default to empty map"
        );
    }

    // ---- plays_cardinality overlay and foreign-parent nulling tests ----

    fn make_entity(type_name: &str) -> TypeDescriptor {
        TypeDescriptor::Entity(EntityDescriptor {
            type_name: type_name.into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![],
        })
    }

    fn make_relation_with_roles(type_name: &str, roles: Vec<RoleDescriptor>) -> TypeDescriptor {
        TypeDescriptor::Relation(RelationDescriptor {
            type_name: type_name.into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![],
            roles,
        })
    }

    /// `plays_cardinality` on a role overlays onto an entity player.
    #[test]
    fn plays_cardinality_overlay_lands_on_entity_player() {
        let descriptors = vec![
            make_entity("person"),
            make_relation_with_roles(
                "employment",
                vec![RoleDescriptor {
                    role_name: "employee".into(),
                    player_type_names: vec!["person".into()],
                    plays_cardinality: Some((0, Some(1))),
                    ..Default::default()
                }],
            ),
        ];

        let info = SchemaInfo::from_descriptors(&descriptors);

        let person = info.entities.get("person").expect("person missing");
        assert_eq!(
            person.plays_cardinalities.get("employment:employee"),
            Some(&(0, Some(1))),
            "entity player must receive plays-card overlay"
        );
    }

    /// `plays_cardinality` on a role overlays onto a relation player (relation playing
    /// a role in another relation).
    #[test]
    fn plays_cardinality_overlay_lands_on_relation_player() {
        let descriptors = vec![
            make_relation_with_roles("inner-rel", vec![]),
            make_relation_with_roles(
                "outer-rel",
                vec![RoleDescriptor {
                    role_name: "container".into(),
                    player_type_names: vec!["inner-rel".into()],
                    plays_cardinality: Some((1, None)),
                    ..Default::default()
                }],
            ),
        ];

        let info = SchemaInfo::from_descriptors(&descriptors);

        let inner = info.relations.get("inner-rel").expect("inner-rel missing");
        assert_eq!(
            inner.plays_cardinalities.get("outer-rel:container"),
            Some(&(1, None)),
            "relation player must receive plays-card overlay"
        );
    }

    /// A role with multiple players: all present players receive the overlay.
    #[test]
    fn plays_cardinality_fans_out_to_all_present_players() {
        let descriptors = vec![
            make_entity("alpha"),
            make_entity("beta"),
            make_relation_with_roles(
                "link",
                vec![RoleDescriptor {
                    role_name: "member".into(),
                    player_type_names: vec!["alpha".into(), "beta".into()],
                    plays_cardinality: Some((0, Some(3))),
                    ..Default::default()
                }],
            ),
        ];

        let info = SchemaInfo::from_descriptors(&descriptors);

        assert_eq!(
            info.entities["alpha"]
                .plays_cardinalities
                .get("link:member"),
            Some(&(0, Some(3))),
        );
        assert_eq!(
            info.entities["beta"].plays_cardinalities.get("link:member"),
            Some(&(0, Some(3))),
        );
    }

    /// A player name listed in a role but absent from the snapshot is silently skipped.
    #[test]
    fn plays_cardinality_skips_absent_player_without_panic() {
        let descriptors = vec![make_relation_with_roles(
            "rel",
            vec![RoleDescriptor {
                role_name: "slot".into(),
                player_type_names: vec!["ghost".into()],
                plays_cardinality: Some((1, Some(1))),
                ..Default::default()
            }],
        )];

        // Must not panic; "ghost" is not in the snapshot.
        let info = SchemaInfo::from_descriptors(&descriptors);
        assert!(info.entities.is_empty());
    }

    /// A role with `plays_cardinality: None` produces no overlay entries.
    #[test]
    fn plays_cardinality_none_produces_no_overlay() {
        let descriptors = vec![
            make_entity("person"),
            make_relation_with_roles(
                "employment",
                vec![RoleDescriptor {
                    role_name: "employee".into(),
                    player_type_names: vec!["person".into()],
                    plays_cardinality: None,
                    ..Default::default()
                }],
            ),
        ];

        let info = SchemaInfo::from_descriptors(&descriptors);

        assert!(
            info.entities["person"].plays_cardinalities.is_empty(),
            "no plays-card overlay expected for None role"
        );
    }

    /// Subtype flattened inherited role: both the parent relation descriptor and the
    /// child relation descriptor list the same role carrying plays_cardinality. The
    /// player map gains both `Parent:role` and `Child:role` keys because each
    /// descriptor's listing produces its own key.
    #[test]
    fn plays_cardinality_subtype_flattened_role_produces_both_keys() {
        let descriptors = vec![
            make_entity("person"),
            // Parent relation descriptor lists the role.
            make_relation_with_roles(
                "base-employment",
                vec![RoleDescriptor {
                    role_name: "employee".into(),
                    player_type_names: vec!["person".into()],
                    plays_cardinality: Some((0, Some(5))),
                    ..Default::default()
                }],
            ),
            // Child relation descriptor flattens the inherited role with its own card.
            TypeDescriptor::Relation(RelationDescriptor {
                type_name: "sub-employment".into(),
                is_abstract: false,
                parent_type: Some("base-employment".into()),
                owned_attributes: vec![],
                roles: vec![RoleDescriptor {
                    role_name: "employee".into(),
                    player_type_names: vec!["person".into()],
                    plays_cardinality: Some((1, Some(2))),
                    ..Default::default()
                }],
            }),
        ];

        let info = SchemaInfo::from_descriptors(&descriptors);

        let person = info.entities.get("person").expect("person missing");
        assert_eq!(
            person.plays_cardinalities.get("base-employment:employee"),
            Some(&(0, Some(5))),
            "parent relation key must be present"
        );
        assert_eq!(
            person.plays_cardinalities.get("sub-employment:employee"),
            Some(&(1, Some(2))),
            "child relation key must be present"
        );
    }

    /// Foreign-parent nulling: an entity with a parent not in the snapshot gets
    /// `parent_type = None`; an entity with a registered parent keeps it.
    #[test]
    fn foreign_parent_nulled_registered_parent_kept() {
        let descriptors = vec![
            TypeDescriptor::Entity(EntityDescriptor {
                type_name: "root".into(),
                is_abstract: true,
                parent_type: None,
                owned_attributes: vec![],
            }),
            TypeDescriptor::Entity(EntityDescriptor {
                type_name: "child".into(),
                is_abstract: false,
                // "root" is registered — must be preserved.
                parent_type: Some("root".into()),
                owned_attributes: vec![],
            }),
            TypeDescriptor::Entity(EntityDescriptor {
                type_name: "orphan".into(),
                is_abstract: false,
                // "thing" is not in the snapshot — must be nulled.
                parent_type: Some("thing".into()),
                owned_attributes: vec![],
            }),
        ];

        let info = SchemaInfo::from_descriptors(&descriptors);

        assert_eq!(
            info.entities["child"].parent_type.as_deref(),
            Some("root"),
            "registered parent must be preserved"
        );
        assert_eq!(
            info.entities["orphan"].parent_type, None,
            "unregistered parent must be nulled"
        );
    }

    /// `RoleDescriptor` without `plays_cardinality` key deserializes with `None` default.
    #[test]
    fn role_descriptor_serde_plays_cardinality_defaults_to_none() {
        let json = r#"{
            "role_name": "participant",
            "player_type_names": ["person"],
            "cardinality": null
        }"#;
        let role: RoleDescriptor = serde_json::from_str(json).expect("deserialize failed");
        assert_eq!(
            role.plays_cardinality, None,
            "missing plays_cardinality must default to None"
        );
    }

    /// `RoleDescriptor` serializes with `"plays_cardinality":null` included.
    #[test]
    fn role_descriptor_serde_serializes_plays_cardinality_null() {
        let role = RoleDescriptor {
            role_name: "participant".into(),
            player_type_names: vec!["person".into()],
            plays_cardinality: None,
            ..Default::default()
        };
        let serialized = serde_json::to_string(&role).expect("serialize failed");
        assert!(
            serialized.contains("\"plays_cardinality\":null"),
            "serialized role must include plays_cardinality:null — got: {serialized}"
        );
    }
}
