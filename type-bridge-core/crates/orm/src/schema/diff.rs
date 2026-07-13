//! Schema diff computation and change tracking.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::attribute::ValueType;

use super::annotations;
use super::info::{AttributeSchemaEntry, OwnedAttributeEntry, SchemaInfo};

/// Severity assigned to a schema change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeCategory {
    /// Backwards-compatible change.
    Safe,
    /// Change that may require data/backfill review.
    Warning,
    /// Change that can reject existing data or remove schema/data.
    Breaking,
}

/// One classified schema change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifiedChange {
    /// Human-readable description.
    pub description: String,
    /// Change severity.
    pub category: ChangeCategory,
    /// Suggested operator action.
    pub recommendation: String,
}

/// A `@meta` annotation map, keyed by meta key.
type MetaMap = BTreeMap<String, String>;

/// An `@doc` change as `(old, new)`, each side optional.
type DocChange = Option<(Option<String>, Option<String>)>;

/// A `@meta` map change as `(old, new)`.
type MetaChange = Option<(MetaMap, MetaMap)>;

/// Changes detected for an entity type.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EntityChanges {
    /// Attributes added in the new schema.
    pub added_attributes: Vec<String>,
    /// Attributes removed in the new schema.
    pub removed_attributes: Vec<String>,
    /// Attributes with changed flags (name, old flags, new flags).
    pub modified_attributes: Vec<(String, String, String)>,
    /// Abstract flag changed: (old, new).
    pub abstract_changed: Option<(bool, bool)>,
    /// Parent type changed: (old, new).
    pub parent_changed: Option<(Option<String>, Option<String>)>,
    /// Type-level `@doc` annotation changed: (old, new). TypeDB 3.12+.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_changed: DocChange,
    /// Type-level `@meta` annotations changed: (old, new). TypeDB 3.12+.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta_changed: MetaChange,
}

/// Changes detected for a relation type.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RelationChanges {
    /// Attributes added in the new schema.
    pub added_attributes: Vec<String>,
    /// Attributes removed in the new schema.
    pub removed_attributes: Vec<String>,
    /// Attributes with changed flags.
    pub modified_attributes: Vec<(String, String, String)>,
    /// Roles added in the new schema.
    pub added_roles: Vec<String>,
    /// Roles removed in the new schema.
    pub removed_roles: Vec<String>,
    /// Player types added to or removed from existing roles.
    pub modified_role_players: Vec<RolePlayerChange>,
    /// Cardinality changed on existing roles.
    pub modified_role_cardinality: Vec<RoleCardinalityChange>,
    /// `@doc`/`@meta` changed on existing roles. TypeDB 3.12+.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modified_role_annotations: Vec<RoleAnnotationChange>,
    /// Abstract flag changed: (old, new).
    pub abstract_changed: Option<(bool, bool)>,
    /// Parent type changed: (old, new).
    pub parent_changed: Option<(Option<String>, Option<String>)>,
    /// Type-level `@doc` annotation changed: (old, new). TypeDB 3.12+.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_changed: DocChange,
    /// Type-level `@meta` annotations changed: (old, new). TypeDB 3.12+.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta_changed: MetaChange,
}

/// An optional list-of-strings constraint value (absent when unconstrained).
type OptStringList = Option<Vec<String>>;

/// A range constraint as `(min, max)`, each bound optional.
type RangeBounds = (Option<String>, Option<String>);

/// An optional range constraint value (absent when unconstrained).
type OptRange = Option<RangeBounds>;

/// Changes detected for a standalone attribute type definition.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttributeTypeChanges {
    /// Value type changed: (old, new).
    pub value_type_changed: Option<(ValueType, ValueType)>,
    /// Parent attribute type changed: (old, new).
    pub parent_changed: Option<(Option<String>, Option<String>)>,
    /// Abstract flag changed: (old, new).
    pub abstract_changed: Option<(bool, bool)>,
    /// Independent flag changed: (old, new).
    pub independent_changed: Option<(bool, bool)>,
    /// Regex constraint changed: (old, new).
    pub regex_changed: Option<(Option<String>, Option<String>)>,
    /// Allowed values constraint changed: (old, new).
    pub allowed_values_changed: Option<(OptStringList, OptStringList)>,
    /// Range constraint changed: (old, new).
    pub range_changed: Option<(OptRange, OptRange)>,
    /// Type-level `@doc` annotation changed: (old, new). TypeDB 3.12+.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_changed: DocChange,
    /// Type-level `@meta` annotations changed: (old, new). TypeDB 3.12+.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta_changed: MetaChange,
}

impl AttributeTypeChanges {
    fn has_changes(&self) -> bool {
        self.value_type_changed.is_some()
            || self.parent_changed.is_some()
            || self.abstract_changed.is_some()
            || self.independent_changed.is_some()
            || self.regex_changed.is_some()
            || self.allowed_values_changed.is_some()
            || self.range_changed.is_some()
            || self.doc_changed.is_some()
            || self.meta_changed.is_some()
    }

    /// Whether the only changes are `@doc`/`@meta` annotations.
    pub fn is_annotation_only(&self) -> bool {
        self.has_changes()
            && self.value_type_changed.is_none()
            && self.parent_changed.is_none()
            && self.abstract_changed.is_none()
            && self.independent_changed.is_none()
            && self.regex_changed.is_none()
            && self.allowed_values_changed.is_none()
            && self.range_changed.is_none()
    }
}

/// Change in the player types accepted by one role.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RolePlayerChange {
    /// Role name.
    pub role_name: String,
    /// Player types added in the new schema.
    pub added_player_types: Vec<String>,
    /// Player types removed in the new schema.
    pub removed_player_types: Vec<String>,
}

/// Change in one role's `@doc`/`@meta` annotations. TypeDB 3.12+.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleAnnotationChange {
    /// Role name.
    pub role_name: String,
    /// `@doc` annotation changed: (old, new).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_changed: DocChange,
    /// `@meta` annotations changed: (old, new).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta_changed: MetaChange,
}

/// Change in one role's cardinality.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleCardinalityChange {
    /// Role name.
    pub role_name: String,
    /// Old `(min, max)` cardinality, or no explicit constraint.
    pub old_cardinality: Option<(u32, Option<u32>)>,
    /// New `(min, max)` cardinality, or no explicit constraint.
    pub new_cardinality: Option<(u32, Option<u32>)>,
}

/// Diff between two schema versions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchemaDiff {
    /// Entity types added in the new schema.
    pub added_entities: Vec<String>,
    /// Entity types removed in the new schema.
    pub removed_entities: Vec<String>,
    /// Entity types with changes.
    pub modified_entities: BTreeMap<String, EntityChanges>,

    /// Relation types added.
    pub added_relations: Vec<String>,
    /// Relation types removed.
    pub removed_relations: Vec<String>,
    /// Relation types with changes.
    pub modified_relations: BTreeMap<String, RelationChanges>,

    /// Attribute types added.
    pub added_attributes: Vec<String>,
    /// Attribute types removed.
    pub removed_attributes: Vec<String>,
    /// Attribute types with changed definitions.
    #[serde(default)]
    pub modified_attributes: BTreeMap<String, AttributeTypeChanges>,
}

impl SchemaDiff {
    /// Whether any changes were detected.
    pub fn has_changes(&self) -> bool {
        !self.added_entities.is_empty()
            || !self.removed_entities.is_empty()
            || !self.modified_entities.is_empty()
            || !self.added_relations.is_empty()
            || !self.removed_relations.is_empty()
            || !self.modified_relations.is_empty()
            || !self.added_attributes.is_empty()
            || !self.removed_attributes.is_empty()
            || !self.modified_attributes.is_empty()
    }

    /// Whether any breaking changes were detected.
    pub fn has_breaking_changes(&self) -> bool {
        self.classify()
            .iter()
            .any(|change| change.category == ChangeCategory::Breaking)
    }

    /// Classify all changes by migration severity.
    pub fn classify(&self) -> Vec<ClassifiedChange> {
        let mut changes = Vec::new();

        for entity in &self.added_entities {
            changes.push(classified(
                ChangeCategory::Safe,
                format!("Add entity: {entity}"),
                "No action needed",
            ));
        }
        for entity in &self.removed_entities {
            changes.push(classified(
                ChangeCategory::Breaking,
                format!("Remove entity: {entity}"),
                format!("Delete all '{entity}' instances before removing the type"),
            ));
        }

        for (entity, entity_changes) in &self.modified_entities {
            for attr_name in &entity_changes.added_attributes {
                changes.push(classified(
                    ChangeCategory::Warning,
                    format!("Add attribute '{attr_name}' to entity '{entity}'"),
                    "Existing instances will need values for this attribute",
                ));
            }
            for attr_name in &entity_changes.removed_attributes {
                changes.push(classified(
                    ChangeCategory::Breaking,
                    format!("Remove attribute '{attr_name}' from entity '{entity}'"),
                    format!("Remove '{attr_name}' values from all '{entity}' instances first"),
                ));
            }
            for (attr_name, old_flags, new_flags) in &entity_changes.modified_attributes {
                let category = classify_attribute_flag_change(old_flags, new_flags);
                changes.push(classified(
                    category,
                    format!(
                        "Modify attribute flags for '{attr_name}' on entity '{entity}': {old_flags} -> {new_flags}"
                    ),
                    "Review cardinality/constraint changes for compatibility",
                ));
            }
            if let Some((old, new)) = entity_changes.abstract_changed {
                changes.push(classified(
                    ChangeCategory::Breaking,
                    format!("Modify abstract flag for entity '{entity}': {old} -> {new}"),
                    "Review abstract type changes for compatibility",
                ));
            }
            if let Some((old, new)) = &entity_changes.parent_changed {
                changes.push(classified(
                    ChangeCategory::Breaking,
                    format!(
                        "Modify parent for entity '{entity}': {} -> {}",
                        option_label(old),
                        option_label(new)
                    ),
                    "Review type hierarchy changes for compatibility",
                ));
            }
            if entity_changes.doc_changed.is_some() || entity_changes.meta_changed.is_some() {
                changes.push(classified(
                    ChangeCategory::Safe,
                    format!("Modify doc/meta annotations for entity '{entity}'"),
                    "No action needed - documentation metadata only",
                ));
            }
        }

        for relation in &self.added_relations {
            changes.push(classified(
                ChangeCategory::Safe,
                format!("Add relation: {relation}"),
                "No action needed",
            ));
        }
        for relation in &self.removed_relations {
            changes.push(classified(
                ChangeCategory::Breaking,
                format!("Remove relation: {relation}"),
                format!("Delete all '{relation}' instances before removing the type"),
            ));
        }

        for (relation, relation_changes) in &self.modified_relations {
            for role_name in &relation_changes.added_roles {
                changes.push(classified(
                    ChangeCategory::Safe,
                    format!("Add role '{role_name}' to relation '{relation}'"),
                    "No action needed",
                ));
            }
            for role_name in &relation_changes.removed_roles {
                changes.push(classified(
                    ChangeCategory::Breaking,
                    format!("Remove role '{role_name}' from relation '{relation}'"),
                    format!(
                        "Update all '{relation}' relations to remove '{role_name}' role players before removing the role"
                    ),
                ));
            }
            for player_change in &relation_changes.modified_role_players {
                for player_type in &player_change.added_player_types {
                    changes.push(classified(
                        ChangeCategory::Safe,
                        format!(
                            "Add player type '{player_type}' to role '{}' in relation '{relation}'",
                            player_change.role_name
                        ),
                        "No action needed - role can now accept more entity types",
                    ));
                }
                for player_type in &player_change.removed_player_types {
                    changes.push(classified(
                        ChangeCategory::Breaking,
                        format!(
                            "Remove player type '{player_type}' from role '{}' in relation '{relation}'",
                            player_change.role_name
                        ),
                        format!(
                            "Update all '{relation}' relations where '{player_type}' plays '{}' before removing the player type",
                            player_change.role_name
                        ),
                    ));
                }
            }
            for cardinality_change in &relation_changes.modified_role_cardinality {
                changes.extend(classify_cardinality_change(relation, cardinality_change));
            }
            for annotation_change in &relation_changes.modified_role_annotations {
                changes.push(classified(
                    ChangeCategory::Safe,
                    format!(
                        "Modify doc/meta annotations for role '{}' in relation '{relation}'",
                        annotation_change.role_name
                    ),
                    "No action needed - documentation metadata only",
                ));
            }
            for attr_name in &relation_changes.added_attributes {
                changes.push(classified(
                    ChangeCategory::Warning,
                    format!("Add attribute '{attr_name}' to relation '{relation}'"),
                    "Existing relations may need values for this attribute",
                ));
            }
            for attr_name in &relation_changes.removed_attributes {
                changes.push(classified(
                    ChangeCategory::Breaking,
                    format!("Remove attribute '{attr_name}' from relation '{relation}'"),
                    format!("Remove '{attr_name}' values from all '{relation}' relations first"),
                ));
            }
            for (attr_name, old_flags, new_flags) in &relation_changes.modified_attributes {
                let category = classify_attribute_flag_change(old_flags, new_flags);
                changes.push(classified(
                    category,
                    format!(
                        "Modify attribute flags for '{attr_name}' on relation '{relation}': {old_flags} -> {new_flags}"
                    ),
                    "Review cardinality/constraint changes for compatibility",
                ));
            }
            if let Some((old, new)) = relation_changes.abstract_changed {
                changes.push(classified(
                    ChangeCategory::Breaking,
                    format!("Modify abstract flag for relation '{relation}': {old} -> {new}"),
                    "Review abstract relation changes for compatibility",
                ));
            }
            if let Some((old, new)) = &relation_changes.parent_changed {
                changes.push(classified(
                    ChangeCategory::Breaking,
                    format!(
                        "Modify parent for relation '{relation}': {} -> {}",
                        option_label(old),
                        option_label(new)
                    ),
                    "Review relation hierarchy changes for compatibility",
                ));
            }
            if relation_changes.doc_changed.is_some() || relation_changes.meta_changed.is_some() {
                changes.push(classified(
                    ChangeCategory::Safe,
                    format!("Modify doc/meta annotations for relation '{relation}'"),
                    "No action needed - documentation metadata only",
                ));
            }
        }

        for attr in &self.added_attributes {
            changes.push(classified(
                ChangeCategory::Safe,
                format!("Add attribute type: {attr}"),
                "No action needed",
            ));
        }
        for attr in &self.removed_attributes {
            changes.push(classified(
                ChangeCategory::Breaking,
                format!("Remove attribute type: {attr}"),
                format!("Remove all ownership and instances of '{attr}' before removing the attribute type"),
            ));
        }
        for (attr, attr_changes) in &self.modified_attributes {
            if attr_changes.is_annotation_only() {
                changes.push(classified(
                    ChangeCategory::Safe,
                    format!("Modify doc/meta annotations for attribute type '{attr}'"),
                    "No action needed - documentation metadata only",
                ));
            } else {
                changes.push(classified(
                    ChangeCategory::Breaking,
                    format!(
                        "Modify attribute type '{attr}': {}",
                        attribute_type_change_summary(attr_changes)
                    ),
                    "Review existing data before changing attribute type constraints",
                ));
            }
        }

        changes
    }

    /// Produce a human-readable summary of the changes.
    pub fn summary(&self) -> String {
        let mut lines = Vec::new();

        for name in &self.added_entities {
            lines.push(format!("+ entity {name}"));
        }
        for name in &self.removed_entities {
            lines.push(format!("- entity {name}"));
        }
        for (name, changes) in &self.modified_entities {
            if let Some((old, new)) = &changes.abstract_changed {
                lines.push(format!("~ entity {name}: abstract ({old} -> {new})"));
            }
            if let Some((old, new)) = &changes.parent_changed {
                let old_str = old.as_deref().unwrap_or("none");
                let new_str = new.as_deref().unwrap_or("none");
                lines.push(format!("~ entity {name}: parent ({old_str} -> {new_str})"));
            }
            for attr in &changes.added_attributes {
                lines.push(format!("~ entity {name}: + owns {attr}"));
            }
            for attr in &changes.removed_attributes {
                lines.push(format!("~ entity {name}: - owns {attr}"));
            }
            for (attr, old, new) in &changes.modified_attributes {
                lines.push(format!("~ entity {name}: ~ owns {attr} ({old} -> {new})"));
            }
            if changes.doc_changed.is_some() {
                lines.push(format!("~ entity {name}: doc"));
            }
            if changes.meta_changed.is_some() {
                lines.push(format!("~ entity {name}: meta"));
            }
        }

        for name in &self.added_relations {
            lines.push(format!("+ relation {name}"));
        }
        for name in &self.removed_relations {
            lines.push(format!("- relation {name}"));
        }
        for (name, changes) in &self.modified_relations {
            if let Some((old, new)) = &changes.abstract_changed {
                lines.push(format!("~ relation {name}: abstract ({old} -> {new})"));
            }
            if let Some((old, new)) = &changes.parent_changed {
                let old_str = old.as_deref().unwrap_or("none");
                let new_str = new.as_deref().unwrap_or("none");
                lines.push(format!(
                    "~ relation {name}: parent ({old_str} -> {new_str})"
                ));
            }
            for role in &changes.added_roles {
                lines.push(format!("~ relation {name}: + relates {role}"));
            }
            for role in &changes.removed_roles {
                lines.push(format!("~ relation {name}: - relates {role}"));
            }
            for player_change in &changes.modified_role_players {
                for player in &player_change.added_player_types {
                    lines.push(format!(
                        "~ relation {name}: ~ role {} + player {player}",
                        player_change.role_name
                    ));
                }
                for player in &player_change.removed_player_types {
                    lines.push(format!(
                        "~ relation {name}: ~ role {} - player {player}",
                        player_change.role_name
                    ));
                }
            }
            for cardinality_change in &changes.modified_role_cardinality {
                lines.push(format!(
                    "~ relation {name}: ~ role {} cardinality ({:?} -> {:?})",
                    cardinality_change.role_name,
                    cardinality_change.old_cardinality,
                    cardinality_change.new_cardinality
                ));
            }
            for annotation_change in &changes.modified_role_annotations {
                let mut parts = Vec::new();
                if annotation_change.doc_changed.is_some() {
                    parts.push("doc");
                }
                if annotation_change.meta_changed.is_some() {
                    parts.push("meta");
                }
                lines.push(format!(
                    "~ relation {name}: ~ role {} {}",
                    annotation_change.role_name,
                    parts.join(", ")
                ));
            }
            for attr in &changes.added_attributes {
                lines.push(format!("~ relation {name}: + owns {attr}"));
            }
            for attr in &changes.removed_attributes {
                lines.push(format!("~ relation {name}: - owns {attr}"));
            }
            if changes.doc_changed.is_some() {
                lines.push(format!("~ relation {name}: doc"));
            }
            if changes.meta_changed.is_some() {
                lines.push(format!("~ relation {name}: meta"));
            }
        }

        for name in &self.added_attributes {
            lines.push(format!("+ attribute {name}"));
        }
        for name in &self.removed_attributes {
            lines.push(format!("- attribute {name}"));
        }
        for (name, changes) in &self.modified_attributes {
            lines.push(format!(
                "~ attribute {name}: {}",
                attribute_type_change_summary(changes)
            ));
        }

        if lines.is_empty() {
            "No changes detected.".to_string()
        } else {
            lines.join("\n")
        }
    }

    /// Compute the diff between two schemas.
    pub fn compute(old: &SchemaInfo, new: &SchemaInfo) -> Self {
        let mut diff = SchemaDiff::default();

        // Entities
        for name in new.entities.keys() {
            if !old.entities.contains_key(name) {
                diff.added_entities.push(name.clone());
            }
        }
        for name in old.entities.keys() {
            if !new.entities.contains_key(name) {
                diff.removed_entities.push(name.clone());
            }
        }
        for (name, new_entry) in &new.entities {
            if let Some(old_entry) = old.entities.get(name) {
                let mut changes =
                    diff_owned_attributes(&old_entry.owned_attributes, &new_entry.owned_attributes);

                if old_entry.is_abstract != new_entry.is_abstract {
                    changes.abstract_changed = Some((old_entry.is_abstract, new_entry.is_abstract));
                }
                if old_entry.parent_type != new_entry.parent_type {
                    changes.parent_changed =
                        Some((old_entry.parent_type.clone(), new_entry.parent_type.clone()));
                }
                changes.doc_changed = doc_change(&old_entry.doc, &new_entry.doc);
                changes.meta_changed = meta_change(&old_entry.meta, &new_entry.meta);

                if !changes.added_attributes.is_empty()
                    || !changes.removed_attributes.is_empty()
                    || !changes.modified_attributes.is_empty()
                    || changes.abstract_changed.is_some()
                    || changes.parent_changed.is_some()
                    || changes.doc_changed.is_some()
                    || changes.meta_changed.is_some()
                {
                    diff.modified_entities.insert(name.clone(), changes);
                }
            }
        }

        // Relations
        for name in new.relations.keys() {
            if !old.relations.contains_key(name) {
                diff.added_relations.push(name.clone());
            }
        }
        for name in old.relations.keys() {
            if !new.relations.contains_key(name) {
                diff.removed_relations.push(name.clone());
            }
        }
        for (name, new_entry) in &new.relations {
            if let Some(old_entry) = old.relations.get(name) {
                let attr_changes =
                    diff_owned_attributes(&old_entry.owned_attributes, &new_entry.owned_attributes);

                let old_roles: BTreeMap<&str, _> = old_entry
                    .roles
                    .iter()
                    .map(|r| (r.role_name.as_str(), r))
                    .collect();
                let new_roles: BTreeMap<&str, _> = new_entry
                    .roles
                    .iter()
                    .map(|r| (r.role_name.as_str(), r))
                    .collect();

                let mut added_roles = Vec::new();
                let mut removed_roles = Vec::new();
                let mut modified_role_players = Vec::new();
                let mut modified_role_cardinality = Vec::new();
                let mut modified_role_annotations = Vec::new();

                for role in new_roles.keys() {
                    if !old_roles.contains_key(role) {
                        added_roles.push(role.to_string());
                    }
                }
                for role in old_roles.keys() {
                    if !new_roles.contains_key(role) {
                        removed_roles.push(role.to_string());
                    }
                }
                for (role_name, old_role) in &old_roles {
                    if let Some(new_role) = new_roles.get(role_name) {
                        let old_players: BTreeSet<&str> = old_role
                            .player_type_names
                            .iter()
                            .map(String::as_str)
                            .collect();
                        let new_players: BTreeSet<&str> = new_role
                            .player_type_names
                            .iter()
                            .map(String::as_str)
                            .collect();

                        let added_player_types: Vec<String> = new_players
                            .difference(&old_players)
                            .map(|player| (*player).to_string())
                            .collect();
                        let removed_player_types: Vec<String> = old_players
                            .difference(&new_players)
                            .map(|player| (*player).to_string())
                            .collect();

                        if !added_player_types.is_empty() || !removed_player_types.is_empty() {
                            modified_role_players.push(RolePlayerChange {
                                role_name: (*role_name).to_string(),
                                added_player_types,
                                removed_player_types,
                            });
                        }

                        if old_role.cardinality != new_role.cardinality {
                            modified_role_cardinality.push(RoleCardinalityChange {
                                role_name: (*role_name).to_string(),
                                old_cardinality: old_role.cardinality,
                                new_cardinality: new_role.cardinality,
                            });
                        }

                        let role_doc_changed = doc_change(&old_role.doc, &new_role.doc);
                        let role_meta_changed = meta_change(&old_role.meta, &new_role.meta);
                        if role_doc_changed.is_some() || role_meta_changed.is_some() {
                            modified_role_annotations.push(RoleAnnotationChange {
                                role_name: (*role_name).to_string(),
                                doc_changed: role_doc_changed,
                                meta_changed: role_meta_changed,
                            });
                        }
                    }
                }

                let abstract_changed = if old_entry.is_abstract != new_entry.is_abstract {
                    Some((old_entry.is_abstract, new_entry.is_abstract))
                } else {
                    None
                };
                let parent_changed = if old_entry.parent_type != new_entry.parent_type {
                    Some((old_entry.parent_type.clone(), new_entry.parent_type.clone()))
                } else {
                    None
                };
                let doc_changed = doc_change(&old_entry.doc, &new_entry.doc);
                let meta_changed = meta_change(&old_entry.meta, &new_entry.meta);

                if !attr_changes.added_attributes.is_empty()
                    || !attr_changes.removed_attributes.is_empty()
                    || !attr_changes.modified_attributes.is_empty()
                    || !added_roles.is_empty()
                    || !removed_roles.is_empty()
                    || !modified_role_players.is_empty()
                    || !modified_role_cardinality.is_empty()
                    || !modified_role_annotations.is_empty()
                    || abstract_changed.is_some()
                    || parent_changed.is_some()
                    || doc_changed.is_some()
                    || meta_changed.is_some()
                {
                    diff.modified_relations.insert(
                        name.clone(),
                        RelationChanges {
                            added_attributes: attr_changes.added_attributes,
                            removed_attributes: attr_changes.removed_attributes,
                            modified_attributes: attr_changes.modified_attributes,
                            added_roles,
                            removed_roles,
                            modified_role_players,
                            modified_role_cardinality,
                            modified_role_annotations,
                            abstract_changed,
                            parent_changed,
                            doc_changed,
                            meta_changed,
                        },
                    );
                }
            }
        }

        // Attributes
        for name in new.attributes.keys() {
            if !old.attributes.contains_key(name) {
                diff.added_attributes.push(name.clone());
            }
        }
        for name in old.attributes.keys() {
            if !new.attributes.contains_key(name) {
                diff.removed_attributes.push(name.clone());
            }
        }
        for (name, new_entry) in &new.attributes {
            if let Some(old_entry) = old.attributes.get(name) {
                let changes = diff_attribute_type(old_entry, new_entry);
                if changes.has_changes() {
                    diff.modified_attributes.insert(name.clone(), changes);
                }
            }
        }

        diff
    }
}

fn attribute_type_change_summary(changes: &AttributeTypeChanges) -> String {
    let mut parts = Vec::new();
    if changes.value_type_changed.is_some() {
        parts.push("value type");
    }
    if changes.parent_changed.is_some() {
        parts.push("parent");
    }
    if changes.abstract_changed.is_some() {
        parts.push("abstract");
    }
    if changes.independent_changed.is_some() {
        parts.push("independent");
    }
    if changes.regex_changed.is_some() {
        parts.push("regex");
    }
    if changes.allowed_values_changed.is_some() {
        parts.push("values");
    }
    if changes.range_changed.is_some() {
        parts.push("range");
    }
    if changes.doc_changed.is_some() {
        parts.push("doc");
    }
    if changes.meta_changed.is_some() {
        parts.push("meta");
    }
    parts.join(", ")
}

fn classified(
    category: ChangeCategory,
    description: String,
    recommendation: impl Into<String>,
) -> ClassifiedChange {
    ClassifiedChange {
        description,
        category,
        recommendation: recommendation.into(),
    }
}

fn option_label(value: &Option<String>) -> &str {
    value.as_deref().unwrap_or("none")
}

fn classify_attribute_flag_change(old_flags: &str, new_flags: &str) -> ChangeCategory {
    // A change that only touches @doc/@meta is metadata-only.
    if annotations::constraint_part(old_flags) == annotations::constraint_part(new_flags) {
        return ChangeCategory::Safe;
    }
    let added_key = !old_flags.contains("@key") && new_flags.contains("@key");
    let added_unique = !old_flags.contains("@unique") && new_flags.contains("@unique");
    if added_key || added_unique {
        ChangeCategory::Breaking
    } else {
        ChangeCategory::Warning
    }
}

fn classify_cardinality_change(
    relation_name: &str,
    change: &RoleCardinalityChange,
) -> Vec<ClassifiedChange> {
    let mut changes = Vec::new();
    let (old_min, old_max) = normalized_cardinality(change.old_cardinality);
    let (new_min, new_max) = normalized_cardinality(change.new_cardinality);

    if let Some(new_min) = new_min
        && old_min.is_none_or(|old_min| new_min > old_min)
    {
        changes.push(classified(
            ChangeCategory::Breaking,
            format!(
                "Increase minimum cardinality for role '{}' in relation '{relation_name}': {:?} -> {new_min}",
                change.role_name, old_min
            ),
            "Existing relations may violate new minimum constraint",
        ));
    }

    if let Some(new_max) = new_max
        && old_max.is_none_or(|old_max| new_max < old_max)
    {
        changes.push(classified(
            ChangeCategory::Breaking,
            format!(
                "Decrease maximum cardinality for role '{}' in relation '{relation_name}': {:?} -> {new_max}",
                change.role_name, old_max
            ),
            "Existing relations may violate new maximum constraint",
        ));
    }

    let min_relaxed = match (old_min, new_min) {
        (Some(old_min), Some(new_min)) => new_min < old_min,
        (Some(_), None) => true,
        _ => false,
    };
    let max_relaxed = match (old_max, new_max) {
        (Some(old_max), Some(new_max)) => new_max > old_max,
        (Some(_), None) => true,
        _ => false,
    };
    if min_relaxed || max_relaxed {
        changes.push(classified(
            ChangeCategory::Safe,
            format!(
                "Relax cardinality for role '{}' in relation '{relation_name}'",
                change.role_name
            ),
            "No action needed - constraint is relaxed",
        ));
    }

    changes
}

fn normalized_cardinality(cardinality: Option<(u32, Option<u32>)>) -> (Option<u32>, Option<u32>) {
    cardinality
        .map(|(min, max)| (Some(min), max))
        .unwrap_or((None, None))
}

fn diff_attribute_type(
    old_entry: &AttributeSchemaEntry,
    new_entry: &AttributeSchemaEntry,
) -> AttributeTypeChanges {
    let mut changes = AttributeTypeChanges::default();

    if old_entry.value_type != new_entry.value_type {
        changes.value_type_changed = Some((old_entry.value_type, new_entry.value_type));
    }
    if old_entry.parent_type != new_entry.parent_type {
        changes.parent_changed =
            Some((old_entry.parent_type.clone(), new_entry.parent_type.clone()));
    }
    if old_entry.is_abstract != new_entry.is_abstract {
        changes.abstract_changed = Some((old_entry.is_abstract, new_entry.is_abstract));
    }
    if old_entry.is_independent != new_entry.is_independent {
        changes.independent_changed = Some((old_entry.is_independent, new_entry.is_independent));
    }
    if old_entry.regex != new_entry.regex {
        changes.regex_changed = Some((old_entry.regex.clone(), new_entry.regex.clone()));
    }
    if old_entry.allowed_values != new_entry.allowed_values {
        changes.allowed_values_changed = Some((
            old_entry.allowed_values.clone(),
            new_entry.allowed_values.clone(),
        ));
    }
    if old_entry.range != new_entry.range {
        changes.range_changed = Some((old_entry.range.clone(), new_entry.range.clone()));
    }
    changes.doc_changed = doc_change(&old_entry.doc, &new_entry.doc);
    changes.meta_changed = meta_change(&old_entry.meta, &new_entry.meta);

    changes
}

/// Produce an `@doc` change pair when the values differ.
fn doc_change(old: &Option<String>, new: &Option<String>) -> DocChange {
    (old != new).then(|| (old.clone(), new.clone()))
}

/// Produce a `@meta` change pair when the maps differ.
fn meta_change(old: &MetaMap, new: &MetaMap) -> MetaChange {
    (old != new).then(|| (old.clone(), new.clone()))
}

/// Compare two lists of owned attributes and produce entity-level changes.
fn diff_owned_attributes(
    old_attrs: &[OwnedAttributeEntry],
    new_attrs: &[OwnedAttributeEntry],
) -> EntityChanges {
    let old_map: BTreeMap<&str, &OwnedAttributeEntry> = old_attrs
        .iter()
        .map(|a| (a.attr_name.as_str(), a))
        .collect();
    let new_map: BTreeMap<&str, &OwnedAttributeEntry> = new_attrs
        .iter()
        .map(|a| (a.attr_name.as_str(), a))
        .collect();

    let mut changes = EntityChanges::default();

    for (name, new_entry) in &new_map {
        match old_map.get(name) {
            None => changes.added_attributes.push(name.to_string()),
            Some(old_entry) => {
                let old_flags = old_entry.flags_string();
                let new_flags = new_entry.flags_string();
                if old_flags != new_flags {
                    changes
                        .modified_attributes
                        .push((name.to_string(), old_flags, new_flags));
                }
            }
        }
    }

    for name in old_map.keys() {
        if !new_map.contains_key(name) {
            changes.removed_attributes.push(name.to_string());
        }
    }

    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attribute::ValueType;
    use crate::entity::Annotation;
    use crate::schema::info::*;

    type TestRoleSpec<'a> = (&'a str, Vec<&'a str>, Option<(u32, Option<u32>)>);

    fn make_attr(name: &str, vt: ValueType, annotations: Vec<Annotation>) -> OwnedAttributeEntry {
        OwnedAttributeEntry {
            attr_name: name.into(),
            value_type: vt,
            annotations,
            is_ordered: false,
            doc: None,
            meta: Default::default(),
        }
    }

    fn make_entity(name: &str, attrs: Vec<OwnedAttributeEntry>) -> EntitySchemaEntry {
        EntitySchemaEntry {
            type_name: name.into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: attrs,
            plays_cardinalities: BTreeMap::new(),
            doc: None,
            meta: Default::default(),
        }
    }

    fn make_relation(
        name: &str,
        attrs: Vec<OwnedAttributeEntry>,
        roles: Vec<(&str, &str)>,
    ) -> RelationSchemaEntry {
        let roles = roles
            .into_iter()
            .map(|(role_name, player_type_name)| (role_name, vec![player_type_name], None))
            .collect();
        make_relation_with_roles(name, attrs, roles)
    }

    fn make_relation_with_roles(
        name: &str,
        attrs: Vec<OwnedAttributeEntry>,
        roles: Vec<TestRoleSpec<'_>>,
    ) -> RelationSchemaEntry {
        RelationSchemaEntry {
            type_name: name.into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: attrs,
            roles: roles
                .into_iter()
                .map(|(r, players, cardinality)| RoleEntry {
                    role_name: r.into(),
                    player_type_names: players.into_iter().map(str::to_string).collect(),
                    cardinality,
                    ..Default::default()
                })
                .collect(),
            plays_cardinalities: BTreeMap::new(),
            doc: None,
            meta: Default::default(),
        }
    }

    #[test]
    fn no_changes_detected() {
        let old = SchemaInfo::default();
        let new = SchemaInfo::default();
        let diff = SchemaDiff::compute(&old, &new);
        assert!(!diff.has_changes());
        assert!(!diff.has_breaking_changes());
        assert_eq!(diff.summary(), "No changes detected.");
    }

    #[test]
    fn detect_added_entity() {
        let old = SchemaInfo::default();
        let mut new = SchemaInfo::default();
        new.entities.insert(
            "person".into(),
            make_entity("person", vec![make_attr("name", ValueType::String, vec![])]),
        );
        let diff = SchemaDiff::compute(&old, &new);
        assert!(diff.has_changes());
        assert!(!diff.has_breaking_changes());
        assert_eq!(diff.added_entities, vec!["person"]);
    }

    #[test]
    fn detect_removed_entity() {
        let mut old = SchemaInfo::default();
        old.entities
            .insert("person".into(), make_entity("person", vec![]));
        let new = SchemaInfo::default();
        let diff = SchemaDiff::compute(&old, &new);
        assert!(diff.has_changes());
        assert!(diff.has_breaking_changes());
        assert_eq!(diff.removed_entities, vec!["person"]);
    }

    #[test]
    fn detect_added_attribute_on_entity() {
        let mut old = SchemaInfo::default();
        old.entities.insert(
            "person".into(),
            make_entity("person", vec![make_attr("name", ValueType::String, vec![])]),
        );

        let mut new = SchemaInfo::default();
        new.entities.insert(
            "person".into(),
            make_entity(
                "person",
                vec![
                    make_attr("name", ValueType::String, vec![]),
                    make_attr("age", ValueType::Long, vec![]),
                ],
            ),
        );

        let diff = SchemaDiff::compute(&old, &new);
        assert!(diff.has_changes());
        let changes = diff.modified_entities.get("person").unwrap();
        assert_eq!(changes.added_attributes, vec!["age"]);
    }

    #[test]
    fn detect_removed_attribute_on_entity() {
        let mut old = SchemaInfo::default();
        old.entities.insert(
            "person".into(),
            make_entity(
                "person",
                vec![
                    make_attr("name", ValueType::String, vec![]),
                    make_attr("age", ValueType::Long, vec![]),
                ],
            ),
        );

        let mut new = SchemaInfo::default();
        new.entities.insert(
            "person".into(),
            make_entity("person", vec![make_attr("name", ValueType::String, vec![])]),
        );

        let diff = SchemaDiff::compute(&old, &new);
        assert!(diff.has_breaking_changes());
        let changes = diff.modified_entities.get("person").unwrap();
        assert_eq!(changes.removed_attributes, vec!["age"]);
    }

    #[test]
    fn detect_modified_annotation() {
        let mut old = SchemaInfo::default();
        old.entities.insert(
            "person".into(),
            make_entity("person", vec![make_attr("name", ValueType::String, vec![])]),
        );

        let mut new = SchemaInfo::default();
        new.entities.insert(
            "person".into(),
            make_entity(
                "person",
                vec![make_attr("name", ValueType::String, vec![Annotation::Key])],
            ),
        );

        let diff = SchemaDiff::compute(&old, &new);
        assert!(diff.has_breaking_changes());
        let changes = diff.modified_entities.get("person").unwrap();
        assert_eq!(changes.modified_attributes.len(), 1);
        assert_eq!(changes.modified_attributes[0].0, "name");
        assert_eq!(changes.modified_attributes[0].2, "@key");
    }

    #[test]
    fn detect_added_relation() {
        let old = SchemaInfo::default();
        let mut new = SchemaInfo::default();
        new.relations.insert(
            "employment".into(),
            make_relation("employment", vec![], vec![("employee", "person")]),
        );
        let diff = SchemaDiff::compute(&old, &new);
        assert!(diff.has_changes());
        assert_eq!(diff.added_relations, vec!["employment"]);
    }

    #[test]
    fn detect_removed_role() {
        let mut old = SchemaInfo::default();
        old.relations.insert(
            "employment".into(),
            make_relation(
                "employment",
                vec![],
                vec![("employee", "person"), ("employer", "company")],
            ),
        );

        let mut new = SchemaInfo::default();
        new.relations.insert(
            "employment".into(),
            make_relation("employment", vec![], vec![("employee", "person")]),
        );

        let diff = SchemaDiff::compute(&old, &new);
        assert!(diff.has_breaking_changes());
        let changes = diff.modified_relations.get("employment").unwrap();
        assert_eq!(changes.removed_roles, vec!["employer"]);
    }

    #[test]
    fn detect_added_attribute_type() {
        let old = SchemaInfo::default();
        let mut new = SchemaInfo::default();
        new.attributes.insert(
            "name".into(),
            AttributeSchemaEntry::new("name", ValueType::String),
        );
        let diff = SchemaDiff::compute(&old, &new);
        assert!(diff.has_changes());
        assert_eq!(diff.added_attributes, vec!["name"]);
    }

    #[test]
    fn detect_modified_attribute_type_constraints() {
        let mut old = SchemaInfo::default();
        old.attributes.insert(
            "email".into(),
            AttributeSchemaEntry::new("email", ValueType::String),
        );

        let mut constrained = AttributeSchemaEntry::new("email", ValueType::String);
        constrained.regex = Some(r"^[a-z]+$".into());
        constrained.range = Some((None, Some("255".into())));
        let mut new = SchemaInfo::default();
        new.attributes.insert("email".into(), constrained);

        let diff = SchemaDiff::compute(&old, &new);

        assert!(diff.has_changes());
        assert!(diff.has_breaking_changes());
        let changes = diff.modified_attributes.get("email").unwrap();
        assert_eq!(
            changes.regex_changed,
            Some((None, Some(r"^[a-z]+$".into())))
        );
        assert_eq!(
            changes.range_changed,
            Some((None, Some((None, Some("255".into())))))
        );
        assert!(diff.summary().contains("~ attribute email: regex, range"));
    }

    #[test]
    fn summary_is_readable() {
        let old = SchemaInfo::default();
        let mut new = SchemaInfo::default();
        new.entities
            .insert("person".into(), make_entity("person", vec![]));
        new.relations.insert(
            "employment".into(),
            make_relation("employment", vec![], vec![("employee", "person")]),
        );

        let diff = SchemaDiff::compute(&old, &new);
        let summary = diff.summary();
        assert!(summary.contains("+ entity person"));
        assert!(summary.contains("+ relation employment"));
    }

    #[test]
    fn identical_schemas_no_changes() {
        let mut old = SchemaInfo::default();
        old.entities.insert(
            "person".into(),
            make_entity(
                "person",
                vec![make_attr("name", ValueType::String, vec![Annotation::Key])],
            ),
        );
        let new = old.clone();
        let diff = SchemaDiff::compute(&old, &new);
        assert!(!diff.has_changes());
    }

    #[test]
    fn detect_abstract_changed_on_entity() {
        let mut old = SchemaInfo::default();
        old.entities
            .insert("animal".into(), make_entity("animal", vec![]));

        let mut new = SchemaInfo::default();
        new.entities.insert(
            "animal".into(),
            EntitySchemaEntry {
                type_name: "animal".into(),
                is_abstract: true,
                parent_type: None,
                owned_attributes: vec![],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let diff = SchemaDiff::compute(&old, &new);
        assert!(diff.has_changes());
        assert!(diff.has_breaking_changes());
        let changes = diff.modified_entities.get("animal").unwrap();
        assert_eq!(changes.abstract_changed, Some((false, true)));
        assert!(diff.summary().contains("abstract (false -> true)"));
    }

    #[test]
    fn detect_parent_changed_on_entity() {
        let mut old = SchemaInfo::default();
        old.entities
            .insert("dog".into(), make_entity("dog", vec![]));

        let mut new = SchemaInfo::default();
        new.entities.insert(
            "dog".into(),
            EntitySchemaEntry {
                type_name: "dog".into(),
                is_abstract: false,
                parent_type: Some("animal".into()),
                owned_attributes: vec![],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let diff = SchemaDiff::compute(&old, &new);
        assert!(diff.has_changes());
        assert!(diff.has_breaking_changes());
        let changes = diff.modified_entities.get("dog").unwrap();
        assert_eq!(changes.parent_changed, Some((None, Some("animal".into()))));
        assert!(diff.summary().contains("parent (none -> animal)"));
    }

    #[test]
    fn detect_abstract_changed_on_relation() {
        let mut old = SchemaInfo::default();
        old.relations.insert(
            "connection".into(),
            make_relation("connection", vec![], vec![]),
        );

        let mut new = SchemaInfo::default();
        new.relations.insert(
            "connection".into(),
            RelationSchemaEntry {
                type_name: "connection".into(),
                is_abstract: true,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let diff = SchemaDiff::compute(&old, &new);
        assert!(diff.has_changes());
        assert!(diff.has_breaking_changes());
        let changes = diff.modified_relations.get("connection").unwrap();
        assert_eq!(changes.abstract_changed, Some((false, true)));
    }

    #[test]
    fn schema_diff_serde_roundtrip() {
        let mut old = SchemaInfo::default();
        old.entities
            .insert("person".into(), make_entity("person", vec![]));
        let mut new = SchemaInfo::default();
        new.entities.insert(
            "person".into(),
            make_entity("person", vec![make_attr("age", ValueType::Long, vec![])]),
        );
        new.entities
            .insert("company".into(), make_entity("company", vec![]));

        let diff = SchemaDiff::compute(&old, &new);
        let json = serde_json::to_string(&diff).unwrap();
        let parsed: SchemaDiff = serde_json::from_str(&json).unwrap();
        assert_eq!(diff.added_entities, parsed.added_entities);
        assert_eq!(diff.modified_entities.len(), parsed.modified_entities.len());
    }

    #[test]
    fn detect_parent_changed_on_relation() {
        let mut old = SchemaInfo::default();
        old.relations.insert(
            "employment".into(),
            make_relation("employment", vec![], vec![("employee", "person")]),
        );

        let mut new = SchemaInfo::default();
        new.relations.insert(
            "employment".into(),
            RelationSchemaEntry {
                type_name: "employment".into(),
                is_abstract: false,
                parent_type: Some("connection".into()),
                owned_attributes: vec![],
                roles: vec![RoleEntry {
                    role_name: "employee".into(),
                    player_type_names: vec!["person".into()],
                    ..Default::default()
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );

        let diff = SchemaDiff::compute(&old, &new);
        assert!(diff.has_breaking_changes());
        let changes = diff.modified_relations.get("employment").unwrap();
        assert_eq!(
            changes.parent_changed,
            Some((None, Some("connection".into())))
        );
    }

    #[test]
    fn detect_role_player_changes_without_dropping_multi_player_roles() {
        let mut old = SchemaInfo::default();
        old.relations.insert(
            "employment".into(),
            make_relation_with_roles(
                "employment",
                vec![],
                vec![("participant", vec!["person", "company"], None)],
            ),
        );

        let mut new = SchemaInfo::default();
        new.relations.insert(
            "employment".into(),
            make_relation_with_roles(
                "employment",
                vec![],
                vec![("participant", vec!["person", "contractor"], None)],
            ),
        );

        let diff = SchemaDiff::compute(&old, &new);
        let changes = diff.modified_relations.get("employment").unwrap();
        assert!(changes.added_roles.is_empty());
        assert!(changes.removed_roles.is_empty());
        assert_eq!(
            changes.modified_role_players,
            vec![RolePlayerChange {
                role_name: "participant".into(),
                added_player_types: vec!["contractor".into()],
                removed_player_types: vec!["company".into()],
            }]
        );
        assert!(diff.has_breaking_changes());
    }

    #[test]
    fn detect_role_cardinality_changes() {
        let mut old = SchemaInfo::default();
        old.relations.insert(
            "employment".into(),
            make_relation_with_roles(
                "employment",
                vec![],
                vec![("employee", vec!["person"], Some((0, None)))],
            ),
        );

        let mut new = SchemaInfo::default();
        new.relations.insert(
            "employment".into(),
            make_relation_with_roles(
                "employment",
                vec![],
                vec![("employee", vec!["person"], Some((1, Some(1))))],
            ),
        );

        let diff = SchemaDiff::compute(&old, &new);
        let changes = diff.modified_relations.get("employment").unwrap();
        assert_eq!(
            changes.modified_role_cardinality,
            vec![RoleCardinalityChange {
                role_name: "employee".into(),
                old_cardinality: Some((0, None)),
                new_cardinality: Some((1, Some(1))),
            }]
        );
        assert!(diff.has_breaking_changes());
    }

    #[test]
    fn classify_matches_python_breaking_parity_matrix() {
        let diff = SchemaDiff {
            added_entities: vec!["person".into()],
            removed_entities: vec!["company".into()],
            added_relations: vec!["employment".into()],
            removed_relations: vec!["contract".into()],
            added_attributes: vec!["name".into()],
            removed_attributes: vec!["email".into()],
            modified_entities: BTreeMap::from([(
                "person".into(),
                EntityChanges {
                    added_attributes: vec!["age".into()],
                    removed_attributes: vec!["legacy".into()],
                    modified_attributes: vec![
                        ("name".into(), "".into(), "@key".into()),
                        ("tag".into(), "@card(0..1)".into(), "@card(1..1)".into()),
                    ],
                    abstract_changed: None,
                    parent_changed: None,
                    doc_changed: None,
                    meta_changed: None,
                },
            )]),
            modified_relations: BTreeMap::from([(
                "employment".into(),
                RelationChanges {
                    added_roles: vec!["manager".into()],
                    removed_roles: vec!["intern".into()],
                    modified_role_players: vec![RolePlayerChange {
                        role_name: "employee".into(),
                        added_player_types: vec!["contractor".into()],
                        removed_player_types: vec!["temp".into()],
                    }],
                    modified_role_cardinality: vec![
                        RoleCardinalityChange {
                            role_name: "employee".into(),
                            old_cardinality: Some((0, None)),
                            new_cardinality: Some((1, None)),
                        },
                        RoleCardinalityChange {
                            role_name: "employer".into(),
                            old_cardinality: Some((1, Some(1))),
                            new_cardinality: Some((0, None)),
                        },
                    ],
                    added_attributes: vec!["start-date".into()],
                    removed_attributes: vec!["end-date".into()],
                    modified_attributes: vec![(
                        "notes".into(),
                        "@card(0..1)".into(),
                        "@card(1..1)".into(),
                    )],
                    modified_role_annotations: vec![],
                    abstract_changed: None,
                    parent_changed: None,
                    doc_changed: None,
                    meta_changed: None,
                },
            )]),
            modified_attributes: BTreeMap::from([(
                "email".into(),
                AttributeTypeChanges {
                    regex_changed: Some((None, Some(r"^[a-z]+$".into()))),
                    ..Default::default()
                },
            )]),
        };

        let classified = diff.classify();

        assert!(
            classified
                .iter()
                .any(|change| change.category == ChangeCategory::Safe
                    && change.description.contains("Add entity"))
        );
        assert!(
            classified
                .iter()
                .any(|change| change.category == ChangeCategory::Warning
                    && change.description.contains("Add attribute 'age'"))
        );
        assert!(classified.iter().any(|change| {
            change.category == ChangeCategory::Breaking
                && change
                    .description
                    .contains("Modify attribute flags for 'name'")
        }));
        assert!(classified.iter().any(|change| {
            change.category == ChangeCategory::Warning
                && change
                    .description
                    .contains("Modify attribute flags for 'tag'")
        }));
        assert!(
            classified
                .iter()
                .any(|change| change.category == ChangeCategory::Safe
                    && change.description.contains("Add player type"))
        );
        assert!(classified.iter().any(|change| {
            change.category == ChangeCategory::Breaking
                && change.description.contains("Modify attribute type 'email'")
        }));
        assert!(
            classified
                .iter()
                .any(|change| change.category == ChangeCategory::Breaking
                    && change.description.contains("Remove player type"))
        );
        assert!(
            classified
                .iter()
                .any(|change| change.category == ChangeCategory::Breaking
                    && change.description.contains("Increase minimum"))
        );
        assert!(
            classified
                .iter()
                .any(|change| change.category == ChangeCategory::Safe
                    && change.description.contains("Relax cardinality"))
        );
        assert!(diff.has_breaking_changes());
    }

    fn meta(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn detect_entity_doc_meta_changes_as_safe() {
        let mut old = SchemaInfo::default();
        old.entities
            .insert("person".into(), make_entity("person", vec![]));

        let mut new = SchemaInfo::default();
        let mut person = make_entity("person", vec![]);
        person.doc = Some("A person.".into());
        person.meta = meta(&[("owner", "core")]);
        new.entities.insert("person".into(), person);

        let diff = SchemaDiff::compute(&old, &new);
        let changes = diff.modified_entities.get("person").unwrap();
        assert_eq!(changes.doc_changed, Some((None, Some("A person.".into()))));
        assert_eq!(
            changes.meta_changed,
            Some((meta(&[]), meta(&[("owner", "core")])))
        );
        assert!(!diff.has_breaking_changes());
        assert!(
            diff.classify()
                .iter()
                .all(|c| c.category == ChangeCategory::Safe)
        );
        assert!(diff.summary().contains("~ entity person: doc"));
        assert!(diff.summary().contains("~ entity person: meta"));
    }

    #[test]
    fn detect_relation_and_role_doc_meta_changes_as_safe() {
        let mut old = SchemaInfo::default();
        old.relations.insert(
            "employment".into(),
            make_relation("employment", vec![], vec![("employee", "person")]),
        );

        let mut new = SchemaInfo::default();
        let mut relation = make_relation("employment", vec![], vec![("employee", "person")]);
        relation.doc = Some("Employment relation.".into());
        relation.roles[0].doc = Some("The employed party.".into());
        relation.roles[0].meta = meta(&[("side", "a")]);
        new.relations.insert("employment".into(), relation);

        let diff = SchemaDiff::compute(&old, &new);
        let changes = diff.modified_relations.get("employment").unwrap();
        assert_eq!(
            changes.doc_changed,
            Some((None, Some("Employment relation.".into())))
        );
        assert_eq!(
            changes.modified_role_annotations,
            vec![RoleAnnotationChange {
                role_name: "employee".into(),
                doc_changed: Some((None, Some("The employed party.".into()))),
                meta_changed: Some((meta(&[]), meta(&[("side", "a")]))),
            }]
        );
        assert!(!diff.has_breaking_changes());
        assert!(
            diff.summary()
                .contains("~ relation employment: ~ role employee doc, meta")
        );
    }

    #[test]
    fn detect_attribute_type_doc_meta_changes_as_annotation_only() {
        let mut old = SchemaInfo::default();
        old.attributes.insert(
            "name".into(),
            AttributeSchemaEntry::new("name", ValueType::String),
        );

        let mut new = SchemaInfo::default();
        let mut entry = AttributeSchemaEntry::new("name", ValueType::String);
        entry.doc = Some("A name.".into());
        new.attributes.insert("name".into(), entry);

        let diff = SchemaDiff::compute(&old, &new);
        let changes = diff.modified_attributes.get("name").unwrap();
        assert!(changes.is_annotation_only());
        assert_eq!(changes.doc_changed, Some((None, Some("A name.".into()))));
        assert!(!diff.has_breaking_changes());
        assert!(diff.summary().contains("~ attribute name: doc"));
    }

    #[test]
    fn ownership_doc_meta_changes_ride_flag_string_triples() {
        let mut old_attr = make_attr("name", ValueType::String, vec![Annotation::Key]);
        old_attr.doc = Some("old ownership doc".into());
        let mut new_attr = make_attr("name", ValueType::String, vec![Annotation::Key]);
        new_attr.doc = Some("new ownership doc".into());
        new_attr.meta = meta(&[("column", "name")]);

        let mut old = SchemaInfo::default();
        old.entities
            .insert("person".into(), make_entity("person", vec![old_attr]));
        let mut new = SchemaInfo::default();
        new.entities
            .insert("person".into(), make_entity("person", vec![new_attr]));

        let diff = SchemaDiff::compute(&old, &new);
        let changes = diff.modified_entities.get("person").unwrap();
        assert_eq!(changes.modified_attributes.len(), 1);
        let (attr_name, old_flags, new_flags) = &changes.modified_attributes[0];
        assert_eq!(attr_name, "name");
        assert!(old_flags.contains("@doc(\"old ownership doc\")"));
        assert!(new_flags.contains("@doc(\"new ownership doc\")"));
        assert!(new_flags.contains("@meta(\"column\", \"name\")"));
        // A doc/meta-only flag change is metadata-only: Safe, not Warning.
        assert!(!diff.has_breaking_changes());
        assert!(
            diff.classify()
                .iter()
                .all(|c| c.category == ChangeCategory::Safe)
        );
    }

    #[test]
    fn constraint_flag_change_with_doc_still_warns() {
        assert_eq!(
            classify_attribute_flag_change(
                "@card(0..1) @doc(\"same\")",
                "@card(1..1) @doc(\"same\")"
            ),
            ChangeCategory::Warning
        );
        assert_eq!(
            classify_attribute_flag_change("@card(0..1)", "@card(0..1) @doc(\"added\")"),
            ChangeCategory::Safe
        );
    }
}
