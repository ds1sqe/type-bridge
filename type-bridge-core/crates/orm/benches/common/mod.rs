//! Bench inputs shared by the Criterion (wall time) and gungraun (instruction
//! count) benches, so both measure the same work under the same ids.
// Each bench binary compiles this module but uses only its own fixtures.
#![allow(dead_code)]

use type_bridge_orm::_attribute::ValueType;
use type_bridge_orm::_schema::info::*;
use type_bridge_orm::expr::{Agg, Expr, SortDir};
use type_bridge_orm::value::AttributeValue;

// ---------------------------------------------------------------------------
// Serde
// ---------------------------------------------------------------------------

pub fn string_value() -> AttributeValue {
    AttributeValue::String("hello world".to_string())
}

pub fn long_value() -> AttributeValue {
    AttributeValue::Long(42)
}

pub fn double_value() -> AttributeValue {
    AttributeValue::Double(2.78)
}

pub fn boolean_value() -> AttributeValue {
    AttributeValue::Boolean(true)
}

pub fn date_value() -> AttributeValue {
    AttributeValue::Date("2024-01-15".to_string())
}

pub fn datetime_value() -> AttributeValue {
    AttributeValue::DateTime("2024-01-15T10:30:00".to_string())
}

pub fn value_types() -> [ValueType; 9] {
    [
        ValueType::String,
        ValueType::Long,
        ValueType::Double,
        ValueType::Boolean,
        ValueType::Date,
        ValueType::DateTime,
        ValueType::DateTimeTz,
        ValueType::Decimal,
        ValueType::Duration,
    ]
}

pub fn make_small_schema_info() -> SchemaInfo {
    let mut info = SchemaInfo::default();
    for i in 0..3 {
        let name = format!("entity_{i}");
        info.entities.insert(
            name.clone(),
            EntitySchemaEntry {
                type_name: name,
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![
                    OwnedAttributeEntry {
                        attr_name: "name".to_string(),
                        value_type: ValueType::String,
                        annotations: vec![],
                        is_ordered: false,
                        doc: None,
                        meta: Default::default(),
                    },
                    OwnedAttributeEntry {
                        attr_name: "age".to_string(),
                        value_type: ValueType::Long,
                        annotations: vec![],
                        is_ordered: false,
                        doc: None,
                        meta: Default::default(),
                    },
                ],
                plays_cardinalities: std::collections::BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );
    }
    for i in 0..2 {
        let name = format!("relation_{i}");
        info.relations.insert(
            name.clone(),
            RelationSchemaEntry {
                type_name: name,
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![RoleEntry {
                    role_name: "player".to_string(),
                    player_type_names: vec!["entity_0".to_string()],
                    ..Default::default()
                }],
                plays_cardinalities: std::collections::BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );
    }
    info
}

pub fn make_large_schema_info() -> SchemaInfo {
    let mut info = SchemaInfo::default();
    for i in 0..50 {
        let name = format!("entity_{i}");
        let attrs: Vec<OwnedAttributeEntry> = (0..3)
            .map(|j| OwnedAttributeEntry {
                attr_name: format!("attr_{j}"),
                value_type: if j % 2 == 0 {
                    ValueType::String
                } else {
                    ValueType::Long
                },
                annotations: vec![],
                is_ordered: false,
                doc: None,
                meta: Default::default(),
            })
            .collect();
        info.entities.insert(
            name.clone(),
            EntitySchemaEntry {
                type_name: name,
                is_abstract: i % 5 == 0,
                parent_type: if i > 0 {
                    Some(format!("entity_{}", i - 1))
                } else {
                    None
                },
                owned_attributes: attrs,
                plays_cardinalities: std::collections::BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );
    }
    for i in 0..20 {
        let name = format!("relation_{i}");
        info.relations.insert(
            name.clone(),
            RelationSchemaEntry {
                type_name: name,
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![
                    RoleEntry {
                        role_name: "player_a".to_string(),
                        player_type_names: vec!["entity_0".to_string()],
                        ..Default::default()
                    },
                    RoleEntry {
                        role_name: "player_b".to_string(),
                        player_type_names: vec!["entity_1".to_string()],
                        ..Default::default()
                    },
                ],
                plays_cardinalities: std::collections::BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );
    }
    info
}

/// An `And`/`Or` tree five levels deep.
pub fn deep_expr() -> Expr {
    build_nested(5)
}

fn build_nested(depth: usize) -> Expr {
    if depth == 0 {
        Expr::eq("name", AttributeValue::String("test".to_string()))
    } else {
        Expr::And(vec![
            build_nested(depth - 1),
            Expr::Or(vec![
                Expr::gte("age", AttributeValue::Long(18)),
                build_nested(depth - 1),
            ]),
        ])
    }
}

pub fn aggs() -> Vec<Agg> {
    vec![
        Agg::Count,
        Agg::Sum("salary".to_string()),
        Agg::Mean("age".to_string()),
    ]
}

pub fn sort_dirs() -> Vec<SortDir> {
    vec![SortDir::Asc, SortDir::Desc]
}

// ---------------------------------------------------------------------------
// Query building
// ---------------------------------------------------------------------------

pub fn make_schema_info_for_generation(entity_count: usize) -> SchemaInfo {
    let mut info = SchemaInfo::default();
    for i in 0..entity_count {
        let name = format!("entity-{i}");
        let attrs: Vec<OwnedAttributeEntry> = (0..3)
            .map(|j| OwnedAttributeEntry {
                attr_name: format!("attr-{j}"),
                value_type: match j % 3 {
                    0 => ValueType::String,
                    1 => ValueType::Long,
                    _ => ValueType::Boolean,
                },
                annotations: vec![],
                is_ordered: false,
                doc: None,
                meta: Default::default(),
            })
            .collect();

        for attr in &attrs {
            info.attributes
                .entry(attr.attr_name.clone())
                .or_insert_with(|| AttributeSchemaEntry::new(&attr.attr_name, attr.value_type));
        }

        info.entities.insert(
            name.clone(),
            EntitySchemaEntry {
                type_name: name,
                is_abstract: false,
                parent_type: None,
                owned_attributes: attrs,
                plays_cardinalities: std::collections::BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );
    }

    // Add some relations
    for i in 0..(entity_count / 3).max(1) {
        let name = format!("relation-{i}");
        info.relations.insert(
            name.clone(),
            RelationSchemaEntry {
                type_name: name,
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                roles: vec![
                    RoleEntry {
                        role_name: "player-a".to_string(),
                        player_type_names: vec!["entity-0".to_string()],
                        ..Default::default()
                    },
                    RoleEntry {
                        role_name: "player-b".to_string(),
                        player_type_names: vec![if entity_count > 1 {
                            "entity-1".to_string()
                        } else {
                            "entity-0".to_string()
                        }],
                        ..Default::default()
                    },
                ],
                plays_cardinalities: std::collections::BTreeMap::new(),
                doc: None,
                meta: Default::default(),
            },
        );
    }

    info
}

pub const SMALL_CODEGEN_SCHEMA: &str = r#"
        define
            attribute name, value string;
            attribute age, value long;
            attribute active, value boolean;
            entity person,
                owns name @key,
                owns age;
            entity company,
                owns name @key;
            relation employment,
                relates employee,
                relates employer;
    "#;

/// 15 attributes, 10 entities and 5 relations.
pub fn medium_codegen_schema() -> String {
    let mut medium_tql = String::from("define\n");
    for i in 0..15 {
        medium_tql.push_str(&format!("    attribute attr-{i}, value string;\n"));
    }
    for i in 0..10 {
        medium_tql.push_str(&format!(
            "    entity entity-{i},\n        owns attr-0,\n        owns attr-1;\n"
        ));
    }
    for i in 0..5 {
        medium_tql.push_str(&format!(
            "    relation rel-{i},\n        relates player-a,\n        relates player-b;\n"
        ));
    }
    medium_tql
}
