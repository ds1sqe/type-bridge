//! TypeQL `define` block generator from [`SchemaInfo`].

use std::collections::{BTreeMap, BTreeSet, HashSet};

use super::info::{EntitySchemaEntry, RelationSchemaEntry, SchemaInfo};

/// Generate a TypeQL `define` block from the given schema info.
///
/// Output is deterministic (alphabetically sorted) and produces syntax like:
/// ```text
/// define
///
/// attribute name, value string;
/// attribute age, value long;
///
/// entity person,
///     owns name @key,
///     owns age;
///
/// relation employment,
///     relates employee,
///     relates employer,
///     owns position;
///
/// person plays employment:employee;
/// company plays employment:employer;
/// ```
pub fn generate_define_block(info: &SchemaInfo) -> String {
    let mut lines = Vec::new();
    lines.push("define".to_string());
    lines.push(String::new());

    // 1. Attribute definitions (sorted)
    let attr_names: BTreeSet<&str> = info.attributes.keys().map(|s| s.as_str()).collect();
    for attr_name in &attr_names {
        let attr = &info.attributes[*attr_name];
        lines.push(format!(
            "attribute {}, value {};",
            attr.attr_name, attr.value_type
        ));
    }
    if !attr_names.is_empty() {
        lines.push(String::new());
    }

    // 2. Entity definitions (topologically sorted: parents before children)
    let entity_order = topological_sort(&info.entities, |e| e.parent_type.as_deref());
    for entity_name in &entity_order {
        let entity = &info.entities[entity_name.as_str()];

        // Determine parent's owned attribute names to skip inherited attrs
        let parent_attr_names: HashSet<&str> = entity
            .parent_type
            .as_ref()
            .and_then(|p| info.entities.get(p))
            .map(|parent| {
                parent
                    .owned_attributes
                    .iter()
                    .map(|a| a.attr_name.as_str())
                    .collect()
            })
            .unwrap_or_default();

        // Build owns clauses (only non-inherited attributes)
        let mut parts = Vec::new();
        for attr in &entity.owned_attributes {
            if parent_attr_names.contains(attr.attr_name.as_str()) {
                continue;
            }
            let flags = attr.flags_string();
            if flags.is_empty() {
                parts.push(format!("    owns {}", attr.attr_name));
            } else {
                parts.push(format!("    owns {} {}", attr.attr_name, flags));
            }
        }

        // Build header: entity <name> [sub <parent>] [@abstract]
        let header = build_entity_header(entity);

        if parts.is_empty() {
            lines.push(format!("{header};"));
        } else {
            lines.push(format!("{header},"));
            let last = parts.len() - 1;
            for (i, part) in parts.iter().enumerate() {
                if i == last {
                    lines.push(format!("{part};"));
                } else {
                    lines.push(format!("{part},"));
                }
            }
        }
    }
    if !entity_order.is_empty() {
        lines.push(String::new());
    }

    // 3. Relation definitions (topologically sorted)
    let relation_order = topological_sort(&info.relations, |r| r.parent_type.as_deref());

    // Collect plays clauses: (player_type, relation:role)
    let mut plays_clauses: Vec<(String, String)> = Vec::new();

    for relation_name in &relation_order {
        let relation = &info.relations[relation_name.as_str()];

        // Determine parent's attribute and role names
        let parent_attr_names: HashSet<&str> = relation
            .parent_type
            .as_ref()
            .and_then(|p| info.relations.get(p))
            .map(|parent| {
                parent
                    .owned_attributes
                    .iter()
                    .map(|a| a.attr_name.as_str())
                    .collect()
            })
            .unwrap_or_default();

        let parent_role_names: HashSet<&str> = relation
            .parent_type
            .as_ref()
            .and_then(|p| info.relations.get(p))
            .map(|parent| {
                parent
                    .roles
                    .iter()
                    .map(|r| r.role_name.as_str())
                    .collect()
            })
            .unwrap_or_default();

        let mut parts = Vec::new();

        // Deduplicate roles by name, skip inherited roles
        let mut seen_roles = BTreeSet::new();
        for role in &relation.roles {
            if parent_role_names.contains(role.role_name.as_str()) {
                // Still collect plays clause for inherited roles
                plays_clauses.push((
                    role.player_type_name.clone(),
                    format!("{}:{}", relation.type_name, role.role_name),
                ));
                continue;
            }
            if seen_roles.insert(&role.role_name) {
                parts.push(format!("    relates {}", role.role_name));
            }
            plays_clauses.push((
                role.player_type_name.clone(),
                format!("{}:{}", relation.type_name, role.role_name),
            ));
        }

        for attr in &relation.owned_attributes {
            if parent_attr_names.contains(attr.attr_name.as_str()) {
                continue;
            }
            let flags = attr.flags_string();
            if flags.is_empty() {
                parts.push(format!("    owns {}", attr.attr_name));
            } else {
                parts.push(format!("    owns {} {}", attr.attr_name, flags));
            }
        }

        // Build header: relation <name> [sub <parent>] [@abstract]
        let header = build_relation_header(relation);

        if parts.is_empty() {
            lines.push(format!("{header};"));
        } else {
            lines.push(format!("{header},"));
            let last = parts.len() - 1;
            for (i, part) in parts.iter().enumerate() {
                if i == last {
                    lines.push(format!("{part};"));
                } else {
                    lines.push(format!("{part},"));
                }
            }
        }
    }

    // 4. Plays clauses (sorted by player type, then role)
    if !plays_clauses.is_empty() {
        lines.push(String::new());
        plays_clauses.sort();
        plays_clauses.dedup();
        for (player, role_ref) in &plays_clauses {
            lines.push(format!("{player} plays {role_ref};"));
        }
    }

    lines.join("\n")
}

/// Build the entity header line: `entity <name> [sub <parent>] [@abstract]`.
fn build_entity_header(entity: &EntitySchemaEntry) -> String {
    let mut header = format!("entity {}", entity.type_name);
    if let Some(ref parent) = entity.parent_type {
        header.push_str(&format!(" sub {parent}"));
    }
    if entity.is_abstract {
        header.push_str(" @abstract");
    }
    header
}

/// Build the relation header line: `relation <name> [sub <parent>] [@abstract]`.
fn build_relation_header(relation: &RelationSchemaEntry) -> String {
    let mut header = format!("relation {}", relation.type_name);
    if let Some(ref parent) = relation.parent_type {
        header.push_str(&format!(" sub {parent}"));
    }
    if relation.is_abstract {
        header.push_str(" @abstract");
    }
    header
}

/// Topological sort: parents before children, deterministic order within each level.
fn topological_sort<T, F>(map: &BTreeMap<String, T>, get_parent: F) -> Vec<String>
where
    F: Fn(&T) -> Option<&str>,
{
    let mut result = Vec::new();
    let mut visited = HashSet::new();

    fn visit<T2, F2>(
        name: &str,
        map: &BTreeMap<String, T2>,
        get_parent: &F2,
        visited: &mut HashSet<String>,
        result: &mut Vec<String>,
    ) where
        F2: Fn(&T2) -> Option<&str>,
    {
        if visited.contains(name) {
            return;
        }
        visited.insert(name.to_string());
        if let Some(entry) = map.get(name)
            && let Some(parent) = get_parent(entry)
        {
            visit(parent, map, get_parent, visited, result);
        }
        result.push(name.to_string());
    }

    // BTreeMap keys are already sorted alphabetically
    for name in map.keys() {
        visit(name, map, &get_parent, &mut visited, &mut result);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attribute::ValueType;
    use crate::entity::Annotation;
    use crate::schema::info::*;

    #[test]
    fn empty_schema_produces_define_only() {
        let info = SchemaInfo::default();
        let result = generate_define_block(&info);
        assert_eq!(result, "define\n");
    }

    #[test]
    fn generates_attribute_definitions() {
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

        let result = generate_define_block(&info);
        assert!(result.contains("attribute age, value long;"));
        assert!(result.contains("attribute name, value string;"));
        // age before name (alphabetical)
        let age_pos = result.find("attribute age").unwrap();
        let name_pos = result.find("attribute name").unwrap();
        assert!(age_pos < name_pos);
    }

    #[test]
    fn generates_entity_with_key() {
        let mut info = SchemaInfo::default();
        info.attributes.insert(
            "name".into(),
            AttributeSchemaEntry {
                attr_name: "name".into(),
                value_type: ValueType::String,
            },
        );
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
                }],
            },
        );

        let result = generate_define_block(&info);
        assert!(result.contains("entity person,"));
        assert!(result.contains("    owns name @key;"));
    }

    #[test]
    fn generates_entity_with_multiple_attrs() {
        let mut info = SchemaInfo::default();
        info.entities.insert(
            "person".into(),
            EntitySchemaEntry {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![
                    OwnedAttributeEntry {
                        attr_name: "name".into(),
                        value_type: ValueType::String,
                        annotations: vec![Annotation::Key],
                    },
                    OwnedAttributeEntry {
                        attr_name: "age".into(),
                        value_type: ValueType::Long,
                        annotations: vec![],
                    },
                ],
            },
        );

        let result = generate_define_block(&info);
        assert!(result.contains("    owns name @key,"));
        assert!(result.contains("    owns age;"));
    }

    #[test]
    fn generates_relation_with_roles() {
        let mut info = SchemaInfo::default();
        info.relations.insert(
            "employment".into(),
            RelationSchemaEntry {
                type_name: "employment".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeEntry {
                    attr_name: "position".into(),
                    value_type: ValueType::String,
                    annotations: vec![],
                }],
                roles: vec![
                    RoleEntry {
                        role_name: "employee".into(),
                        player_type_name: "person".into(),
                    },
                    RoleEntry {
                        role_name: "employer".into(),
                        player_type_name: "company".into(),
                    },
                ],
            },
        );

        let result = generate_define_block(&info);
        assert!(result.contains("relation employment,"));
        assert!(result.contains("    relates employee,"));
        assert!(result.contains("    relates employer,"));
        assert!(result.contains("    owns position;"));
        assert!(result.contains("company plays employment:employer;"));
        assert!(result.contains("person plays employment:employee;"));
    }

    #[test]
    fn generates_cardinality_annotation() {
        let mut info = SchemaInfo::default();
        info.entities.insert(
            "person".into(),
            EntitySchemaEntry {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeEntry {
                    attr_name: "tag".into(),
                    value_type: ValueType::String,
                    annotations: vec![Annotation::Card(2, Some(5))],
                }],
            },
        );

        let result = generate_define_block(&info);
        assert!(result.contains("    owns tag @card(2..5);"));
    }

    #[test]
    fn generates_unique_annotation() {
        let mut info = SchemaInfo::default();
        info.entities.insert(
            "person".into(),
            EntitySchemaEntry {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeEntry {
                    attr_name: "email".into(),
                    value_type: ValueType::String,
                    annotations: vec![Annotation::Unique],
                }],
            },
        );

        let result = generate_define_block(&info);
        assert!(result.contains("    owns email @unique;"));
    }

    #[test]
    fn generates_abstract_entity() {
        let mut info = SchemaInfo::default();
        info.entities.insert(
            "animal".into(),
            EntitySchemaEntry {
                type_name: "animal".into(),
                is_abstract: true,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeEntry {
                    attr_name: "name".into(),
                    value_type: ValueType::String,
                    annotations: vec![Annotation::Key],
                }],
            },
        );

        let result = generate_define_block(&info);
        assert!(
            result.contains("entity animal @abstract,"),
            "should have @abstract: {result}"
        );
        assert!(result.contains("    owns name @key;"));
    }

    #[test]
    fn generates_sub_clause() {
        let mut info = SchemaInfo::default();
        info.entities.insert(
            "animal".into(),
            EntitySchemaEntry {
                type_name: "animal".into(),
                is_abstract: true,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeEntry {
                    attr_name: "name".into(),
                    value_type: ValueType::String,
                    annotations: vec![Annotation::Key],
                }],
            },
        );
        info.entities.insert(
            "dog".into(),
            EntitySchemaEntry {
                type_name: "dog".into(),
                is_abstract: false,
                parent_type: Some("animal".into()),
                owned_attributes: vec![
                    OwnedAttributeEntry {
                        attr_name: "name".into(),
                        value_type: ValueType::String,
                        annotations: vec![Annotation::Key],
                    },
                    OwnedAttributeEntry {
                        attr_name: "breed".into(),
                        value_type: ValueType::String,
                        annotations: vec![],
                    },
                ],
            },
        );

        let result = generate_define_block(&info);
        assert!(
            result.contains("entity dog sub animal,"),
            "should have sub clause: {result}"
        );
    }

    #[test]
    fn topological_sort_parents_before_children() {
        let mut info = SchemaInfo::default();
        // Insert child first alphabetically (dog before animal is not the case,
        // but let's use "cat" which comes before "mammal")
        info.entities.insert(
            "cat".into(),
            EntitySchemaEntry {
                type_name: "cat".into(),
                is_abstract: false,
                parent_type: Some("mammal".into()),
                owned_attributes: vec![],
            },
        );
        info.entities.insert(
            "mammal".into(),
            EntitySchemaEntry {
                type_name: "mammal".into(),
                is_abstract: true,
                parent_type: None,
                owned_attributes: vec![],
            },
        );

        let result = generate_define_block(&info);
        let mammal_pos = result.find("entity mammal").unwrap();
        let cat_pos = result.find("entity cat").unwrap();
        assert!(
            mammal_pos < cat_pos,
            "parent (mammal) should appear before child (cat): {result}"
        );
    }

    #[test]
    fn skips_inherited_attributes() {
        let mut info = SchemaInfo::default();
        info.entities.insert(
            "animal".into(),
            EntitySchemaEntry {
                type_name: "animal".into(),
                is_abstract: true,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeEntry {
                    attr_name: "name".into(),
                    value_type: ValueType::String,
                    annotations: vec![Annotation::Key],
                }],
            },
        );
        info.entities.insert(
            "dog".into(),
            EntitySchemaEntry {
                type_name: "dog".into(),
                is_abstract: false,
                parent_type: Some("animal".into()),
                owned_attributes: vec![
                    OwnedAttributeEntry {
                        attr_name: "name".into(),
                        value_type: ValueType::String,
                        annotations: vec![Annotation::Key],
                    },
                    OwnedAttributeEntry {
                        attr_name: "breed".into(),
                        value_type: ValueType::String,
                        annotations: vec![],
                    },
                ],
            },
        );

        let result = generate_define_block(&info);
        // "dog" section should have "owns breed" but NOT "owns name" (inherited)
        let dog_section_start = result.find("entity dog").unwrap();
        let dog_section = &result[dog_section_start..];
        // Find end of dog section (next blank line or end)
        let dog_section_end = dog_section.find("\n\n").unwrap_or(dog_section.len());
        let dog_section = &dog_section[..dog_section_end];

        assert!(
            dog_section.contains("owns breed"),
            "dog should own breed: {dog_section}"
        );
        assert!(
            !dog_section.contains("owns name"),
            "dog should NOT re-emit inherited name: {dog_section}"
        );
    }

    #[test]
    fn generates_abstract_relation() {
        let mut info = SchemaInfo::default();
        info.relations.insert(
            "connection".into(),
            RelationSchemaEntry {
                type_name: "connection".into(),
                is_abstract: true,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![RoleEntry {
                    role_name: "source".into(),
                    player_type_name: "node".into(),
                }],
            },
        );

        let result = generate_define_block(&info);
        assert!(
            result.contains("relation connection @abstract,"),
            "should have @abstract: {result}"
        );
    }

    #[test]
    fn generates_relation_sub_clause() {
        let mut info = SchemaInfo::default();
        info.relations.insert(
            "connection".into(),
            RelationSchemaEntry {
                type_name: "connection".into(),
                is_abstract: true,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![],
            },
        );
        info.relations.insert(
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

        let result = generate_define_block(&info);
        assert!(
            result.contains("relation employment sub connection,"),
            "should have sub clause: {result}"
        );
    }

    #[test]
    fn deduplicates_relation_roles() {
        let mut info = SchemaInfo::default();
        info.relations.insert(
            "friendship".into(),
            RelationSchemaEntry {
                type_name: "friendship".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![
                    RoleEntry {
                        role_name: "friend".into(),
                        player_type_name: "person".into(),
                    },
                    RoleEntry {
                        role_name: "friend".into(),
                        player_type_name: "person".into(),
                    },
                ],
            },
        );

        let result = generate_define_block(&info);
        // "relates friend" should appear only once
        let count = result.matches("relates friend").count();
        assert_eq!(count, 1, "should deduplicate role names: {result}");
    }
}
