use criterion::{Criterion, black_box, criterion_group, criterion_main};

use type_bridge_orm::_attribute::ValueType;
use type_bridge_orm::_schema::info::*;
use type_bridge_orm::expr::{Agg, Expr, SortDir};
use type_bridge_orm::value::AttributeValue;

fn bench_attribute_value_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("serde/attribute_value");

    let string_val = AttributeValue::String("hello world".to_string());
    group.bench_function("string_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(black_box(&string_val)).unwrap();
            let _: AttributeValue = serde_json::from_str(&json).unwrap();
        })
    });

    let long_val = AttributeValue::Long(42);
    group.bench_function("long_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(black_box(&long_val)).unwrap();
            let _: AttributeValue = serde_json::from_str(&json).unwrap();
        })
    });

    let double_val = AttributeValue::Double(2.78);
    group.bench_function("double_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(black_box(&double_val)).unwrap();
            let _: AttributeValue = serde_json::from_str(&json).unwrap();
        })
    });

    let bool_val = AttributeValue::Boolean(true);
    group.bench_function("boolean_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(black_box(&bool_val)).unwrap();
            let _: AttributeValue = serde_json::from_str(&json).unwrap();
        })
    });

    let date_val = AttributeValue::Date("2024-01-15".to_string());
    group.bench_function("date_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(black_box(&date_val)).unwrap();
            let _: AttributeValue = serde_json::from_str(&json).unwrap();
        })
    });

    let datetime_val = AttributeValue::DateTime("2024-01-15T10:30:00".to_string());
    group.bench_function("datetime_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(black_box(&datetime_val)).unwrap();
            let _: AttributeValue = serde_json::from_str(&json).unwrap();
        })
    });

    group.finish();
}

fn bench_value_type_roundtrip(c: &mut Criterion) {
    let types = [
        ValueType::String,
        ValueType::Long,
        ValueType::Double,
        ValueType::Boolean,
        ValueType::Date,
        ValueType::DateTime,
        ValueType::DateTimeTz,
        ValueType::Decimal,
        ValueType::Duration,
    ];

    c.bench_function("serde/value_type_batch_roundtrip", |b| {
        b.iter(|| {
            for vt in black_box(&types) {
                let json = serde_json::to_string(vt).unwrap();
                let _: ValueType = serde_json::from_str(&json).unwrap();
            }
        })
    });
}

fn make_small_schema_info() -> SchemaInfo {
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

fn make_large_schema_info() -> SchemaInfo {
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

fn bench_schema_info_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("serde/schema_info");

    let small = make_small_schema_info();
    let small_json = serde_json::to_string(&small).unwrap();
    group.bench_function("small_serialize", |b| {
        b.iter(|| serde_json::to_string(black_box(&small)).unwrap())
    });
    group.bench_function("small_deserialize", |b| {
        b.iter(|| {
            let _: SchemaInfo = serde_json::from_str(black_box(&small_json)).unwrap();
        })
    });

    let large = make_large_schema_info();
    let large_json = serde_json::to_string(&large).unwrap();
    group.bench_function("large_serialize", |b| {
        b.iter(|| serde_json::to_string(black_box(&large)).unwrap())
    });
    group.bench_function("large_deserialize", |b| {
        b.iter(|| {
            let _: SchemaInfo = serde_json::from_str(black_box(&large_json)).unwrap();
        })
    });

    group.finish();
}

fn bench_expr_roundtrip(c: &mut Criterion) {
    // Build a nested Expr tree
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

    let deep_expr = build_nested(5);
    let deep_json = serde_json::to_string(&deep_expr).unwrap();

    let mut group = c.benchmark_group("serde/expr");
    group.bench_function("deep_serialize", |b| {
        b.iter(|| serde_json::to_string(black_box(&deep_expr)).unwrap())
    });
    group.bench_function("deep_deserialize", |b| {
        b.iter(|| {
            let _: Expr = serde_json::from_str(black_box(&deep_json)).unwrap();
        })
    });

    // Agg round-trip
    let aggs = vec![
        Agg::Count,
        Agg::Sum("salary".to_string()),
        Agg::Mean("age".to_string()),
    ];
    let agg_json = serde_json::to_string(&aggs).unwrap();
    group.bench_function("agg_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(black_box(&aggs)).unwrap();
            let _: Vec<Agg> = serde_json::from_str(&json).unwrap();
        })
    });
    let _ = agg_json; // ensure it's used

    // SortDir round-trip
    let dirs = vec![SortDir::Asc, SortDir::Desc];
    group.bench_function("sort_dir_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(black_box(&dirs)).unwrap();
            let _: Vec<SortDir> = serde_json::from_str(&json).unwrap();
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_attribute_value_roundtrip,
    bench_value_type_roundtrip,
    bench_schema_info_roundtrip,
    bench_expr_roundtrip,
);
criterion_main!(benches);
