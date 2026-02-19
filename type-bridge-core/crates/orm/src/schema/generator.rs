//! TypeQL `define` block generator from [`SchemaInfo`].

use std::collections::BTreeSet;

use super::info::SchemaInfo;

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

    // 2. Entity definitions (sorted)
    let entity_names: BTreeSet<&str> = info.entities.keys().map(|s| s.as_str()).collect();
    for entity_name in &entity_names {
        let entity = &info.entities[*entity_name];
        let mut parts = Vec::new();

        for attr in &entity.owned_attributes {
            let flags = attr.flags_string();
            if flags.is_empty() {
                parts.push(format!("    owns {}", attr.attr_name));
            } else {
                parts.push(format!("    owns {} {}", attr.attr_name, flags));
            }
        }

        if parts.is_empty() {
            lines.push(format!("entity {};", entity.type_name));
        } else {
            lines.push(format!("entity {},", entity.type_name));
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
    if !entity_names.is_empty() {
        lines.push(String::new());
    }

    // 3. Relation definitions (sorted)
    let relation_names: BTreeSet<&str> = info.relations.keys().map(|s| s.as_str()).collect();

    // Collect plays clauses: (player_type, relation:role)
    let mut plays_clauses: Vec<(String, String)> = Vec::new();

    for relation_name in &relation_names {
        let relation = &info.relations[*relation_name];
        let mut parts = Vec::new();

        // Deduplicate roles by name
        let mut seen_roles = BTreeSet::new();
        for role in &relation.roles {
            if seen_roles.insert(&role.role_name) {
                parts.push(format!("    relates {}", role.role_name));
            }
            plays_clauses.push((
                role.player_type_name.clone(),
                format!("{}:{}", relation.type_name, role.role_name),
            ));
        }

        for attr in &relation.owned_attributes {
            let flags = attr.flags_string();
            if flags.is_empty() {
                parts.push(format!("    owns {}", attr.attr_name));
            } else {
                parts.push(format!("    owns {} {}", attr.attr_name, flags));
            }
        }

        if parts.is_empty() {
            lines.push(format!("relation {};", relation.type_name));
        } else {
            lines.push(format!("relation {},", relation.type_name));
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
    fn deduplicates_relation_roles() {
        let mut info = SchemaInfo::default();
        info.relations.insert(
            "friendship".into(),
            RelationSchemaEntry {
                type_name: "friendship".into(),
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
