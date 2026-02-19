//! Schema diff computation and change tracking.

use std::collections::BTreeMap;

use super::info::{OwnedAttributeEntry, SchemaInfo};

/// Changes detected for an entity type.
#[derive(Debug, Clone, Default)]
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
}

/// Changes detected for a relation type.
#[derive(Debug, Clone, Default)]
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
    /// Abstract flag changed: (old, new).
    pub abstract_changed: Option<(bool, bool)>,
    /// Parent type changed: (old, new).
    pub parent_changed: Option<(Option<String>, Option<String>)>,
}

/// Diff between two schema versions.
#[derive(Debug, Clone, Default)]
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
    }

    /// Whether any breaking changes were detected (removals or modifications).
    pub fn has_breaking_changes(&self) -> bool {
        !self.removed_entities.is_empty()
            || !self.removed_relations.is_empty()
            || !self.removed_attributes.is_empty()
            || self.modified_entities.values().any(|c| {
                !c.removed_attributes.is_empty()
                    || !c.modified_attributes.is_empty()
                    || c.abstract_changed.is_some()
                    || c.parent_changed.is_some()
            })
            || self.modified_relations.values().any(|c| {
                !c.removed_attributes.is_empty()
                    || !c.modified_attributes.is_empty()
                    || !c.removed_roles.is_empty()
                    || c.abstract_changed.is_some()
                    || c.parent_changed.is_some()
            })
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
                lines.push(format!("~ relation {name}: parent ({old_str} -> {new_str})"));
            }
            for role in &changes.added_roles {
                lines.push(format!("~ relation {name}: + relates {role}"));
            }
            for role in &changes.removed_roles {
                lines.push(format!("~ relation {name}: - relates {role}"));
            }
            for attr in &changes.added_attributes {
                lines.push(format!("~ relation {name}: + owns {attr}"));
            }
            for attr in &changes.removed_attributes {
                lines.push(format!("~ relation {name}: - owns {attr}"));
            }
        }

        for name in &self.added_attributes {
            lines.push(format!("+ attribute {name}"));
        }
        for name in &self.removed_attributes {
            lines.push(format!("- attribute {name}"));
        }

        if lines.is_empty() {
            "No changes detected.".to_string()
        } else {
            lines.join("\n")
        }
    }

    /// Compute the diff between two schemas.
    pub(crate) fn compute(old: &SchemaInfo, new: &SchemaInfo) -> Self {
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
                let mut changes = diff_owned_attributes(
                    &old_entry.owned_attributes,
                    &new_entry.owned_attributes,
                );

                if old_entry.is_abstract != new_entry.is_abstract {
                    changes.abstract_changed =
                        Some((old_entry.is_abstract, new_entry.is_abstract));
                }
                if old_entry.parent_type != new_entry.parent_type {
                    changes.parent_changed = Some((
                        old_entry.parent_type.clone(),
                        new_entry.parent_type.clone(),
                    ));
                }

                if !changes.added_attributes.is_empty()
                    || !changes.removed_attributes.is_empty()
                    || !changes.modified_attributes.is_empty()
                    || changes.abstract_changed.is_some()
                    || changes.parent_changed.is_some()
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
                let attr_changes = diff_owned_attributes(
                    &old_entry.owned_attributes,
                    &new_entry.owned_attributes,
                );

                let old_roles: BTreeMap<&str, &str> = old_entry
                    .roles
                    .iter()
                    .map(|r| (r.role_name.as_str(), r.player_type_name.as_str()))
                    .collect();
                let new_roles: BTreeMap<&str, &str> = new_entry
                    .roles
                    .iter()
                    .map(|r| (r.role_name.as_str(), r.player_type_name.as_str()))
                    .collect();

                let mut added_roles = Vec::new();
                let mut removed_roles = Vec::new();

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

                let abstract_changed =
                    if old_entry.is_abstract != new_entry.is_abstract {
                        Some((old_entry.is_abstract, new_entry.is_abstract))
                    } else {
                        None
                    };
                let parent_changed = if old_entry.parent_type != new_entry.parent_type {
                    Some((
                        old_entry.parent_type.clone(),
                        new_entry.parent_type.clone(),
                    ))
                } else {
                    None
                };

                if !attr_changes.added_attributes.is_empty()
                    || !attr_changes.removed_attributes.is_empty()
                    || !attr_changes.modified_attributes.is_empty()
                    || !added_roles.is_empty()
                    || !removed_roles.is_empty()
                    || abstract_changed.is_some()
                    || parent_changed.is_some()
                {
                    diff.modified_relations.insert(
                        name.clone(),
                        RelationChanges {
                            added_attributes: attr_changes.added_attributes,
                            removed_attributes: attr_changes.removed_attributes,
                            modified_attributes: attr_changes.modified_attributes,
                            added_roles,
                            removed_roles,
                            abstract_changed,
                            parent_changed,
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

        diff
    }
}

/// Compare two lists of owned attributes and produce entity-level changes.
fn diff_owned_attributes(
    old_attrs: &[OwnedAttributeEntry],
    new_attrs: &[OwnedAttributeEntry],
) -> EntityChanges {
    let old_map: BTreeMap<&str, &OwnedAttributeEntry> =
        old_attrs.iter().map(|a| (a.attr_name.as_str(), a)).collect();
    let new_map: BTreeMap<&str, &OwnedAttributeEntry> =
        new_attrs.iter().map(|a| (a.attr_name.as_str(), a)).collect();

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

    fn make_attr(name: &str, vt: ValueType, annotations: Vec<Annotation>) -> OwnedAttributeEntry {
        OwnedAttributeEntry {
            attr_name: name.into(),
            value_type: vt,
            annotations,
        }
    }

    fn make_entity(name: &str, attrs: Vec<OwnedAttributeEntry>) -> EntitySchemaEntry {
        EntitySchemaEntry {
            type_name: name.into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: attrs,
        }
    }

    fn make_relation(
        name: &str,
        attrs: Vec<OwnedAttributeEntry>,
        roles: Vec<(&str, &str)>,
    ) -> RelationSchemaEntry {
        RelationSchemaEntry {
            type_name: name.into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: attrs,
            roles: roles
                .into_iter()
                .map(|(r, p)| RoleEntry {
                    role_name: r.into(),
                    player_type_name: p.into(),
                })
                .collect(),
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
            AttributeSchemaEntry {
                attr_name: "name".into(),
                value_type: ValueType::String,
            },
        );
        let diff = SchemaDiff::compute(&old, &new);
        assert!(diff.has_changes());
        assert_eq!(diff.added_attributes, vec!["name"]);
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
            },
        );

        let diff = SchemaDiff::compute(&old, &new);
        assert!(diff.has_changes());
        assert!(diff.has_breaking_changes());
        let changes = diff.modified_entities.get("dog").unwrap();
        assert_eq!(
            changes.parent_changed,
            Some((None, Some("animal".into())))
        );
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
            },
        );

        let diff = SchemaDiff::compute(&old, &new);
        assert!(diff.has_changes());
        assert!(diff.has_breaking_changes());
        let changes = diff.modified_relations.get("connection").unwrap();
        assert_eq!(changes.abstract_changed, Some((false, true)));
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
                    player_type_name: "person".into(),
                }],
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
}
