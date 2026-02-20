use std::collections::HashMap;

use criterion::{black_box, criterion_group, criterion_main, Criterion};

use type_bridge_core_lib::schema::TypeSchema;
use type_bridge_server::crud::builder;
use type_bridge_server::crud::types::*;

fn test_schema() -> TypeSchema {
    TypeSchema::from_typeql(
        r#"
        define
            attribute name, value string;
            attribute age, value long;
            attribute email, value string;
            attribute salary, value double;
            attribute active, value boolean;
            attribute start-date, value date;
            entity person,
                owns name @key,
                owns age,
                owns email,
                owns salary,
                owns active;
            entity company,
                owns name @key;
            relation employment,
                relates employee,
                relates employer,
                owns start-date;
        "#,
    )
    .unwrap()
}

fn make_attrs(count: usize) -> HashMap<String, AttributeValueSpec> {
    let attr_defs = [
        ("name", serde_json::json!("Alice"), "string"),
        ("age", serde_json::json!(30), "long"),
        ("email", serde_json::json!("alice@example.com"), "string"),
        ("salary", serde_json::json!(75000.0), "double"),
        ("active", serde_json::json!(true), "boolean"),
    ];
    let mut map = HashMap::new();
    for (name, value, vtype) in attr_defs.iter().take(count) {
        map.insert(
            name.to_string(),
            AttributeValueSpec {
                value: value.clone(),
                value_type: vtype.to_string(),
            },
        );
    }
    map
}

fn bench_entity_insert(c: &mut Criterion) {
    let schema = test_schema();
    let mut group = c.benchmark_group("crud/entity_insert");

    let attrs_3 = make_attrs(3);
    group.bench_function("3_attrs", |b| {
        b.iter(|| {
            builder::build_entity_insert(
                black_box("person"),
                black_box(&attrs_3),
                black_box(&schema),
            )
            .unwrap()
        })
    });

    let attrs_5 = make_attrs(5);
    group.bench_function("5_attrs", |b| {
        b.iter(|| {
            builder::build_entity_insert(
                black_box("person"),
                black_box(&attrs_5),
                black_box(&schema),
            )
            .unwrap()
        })
    });

    group.finish();
}

fn bench_entity_fetch(c: &mut Criterion) {
    let schema = test_schema();
    let mut group = c.benchmark_group("crud/entity_fetch");

    group.bench_function("no_filters", |b| {
        b.iter(|| {
            builder::build_entity_fetch(
                black_box("person"),
                black_box(&[]),
                black_box(&[]),
                black_box(None),
                black_box(None),
                black_box(&schema),
            )
            .unwrap()
        })
    });

    let filters = vec![
        FilterSpec {
            attr: "age".to_string(),
            op: ">=".to_string(),
            value: AttributeValueSpec {
                value: serde_json::json!(18),
                value_type: "long".to_string(),
            },
        },
        FilterSpec {
            attr: "active".to_string(),
            op: "==".to_string(),
            value: AttributeValueSpec {
                value: serde_json::json!(true),
                value_type: "boolean".to_string(),
            },
        },
        FilterSpec {
            attr: "name".to_string(),
            op: "!=".to_string(),
            value: AttributeValueSpec {
                value: serde_json::json!("deleted"),
                value_type: "string".to_string(),
            },
        },
    ];

    group.bench_function("3_filters", |b| {
        b.iter(|| {
            builder::build_entity_fetch(
                black_box("person"),
                black_box(&filters),
                black_box(&[]),
                black_box(Some(10)),
                black_box(Some(0)),
                black_box(&schema),
            )
            .unwrap()
        })
    });

    group.finish();
}

fn bench_entity_fetch_by_iid(c: &mut Criterion) {
    let schema = test_schema();
    c.bench_function("crud/entity_fetch_by_iid", |b| {
        b.iter(|| {
            builder::build_entity_fetch_by_iid(
                black_box("person"),
                black_box("0xabcdef1234567890"),
                black_box(&schema),
            )
            .unwrap()
        })
    });
}

fn bench_entity_update(c: &mut Criterion) {
    let schema = test_schema();
    let attrs = make_attrs(3);
    c.bench_function("crud/entity_update", |b| {
        b.iter(|| {
            builder::build_entity_update_by_iid(
                black_box("person"),
                black_box("0xabc123"),
                black_box(&attrs),
                black_box(&schema),
            )
            .unwrap()
        })
    });
}

fn bench_relation_insert(c: &mut Criterion) {
    let schema = test_schema();
    let mut group = c.benchmark_group("crud/relation_insert");

    let role_players_2 = vec![
        RolePlayerSpec {
            role: "employee".to_string(),
            entity_type: "person".to_string(),
            iid: None,
            key_attr: Some("name".to_string()),
            key_value: Some(AttributeValueSpec {
                value: serde_json::json!("Alice"),
                value_type: "string".to_string(),
            }),
        },
        RolePlayerSpec {
            role: "employer".to_string(),
            entity_type: "company".to_string(),
            iid: None,
            key_attr: Some("name".to_string()),
            key_value: Some(AttributeValueSpec {
                value: serde_json::json!("Acme"),
                value_type: "string".to_string(),
            }),
        },
    ];
    let attrs = HashMap::new();

    group.bench_function("2_role_players", |b| {
        b.iter(|| {
            builder::build_relation_insert(
                black_box("employment"),
                black_box(&role_players_2),
                black_box(&attrs),
                black_box(&schema),
            )
            .unwrap()
        })
    });

    let mut attrs_with_date = HashMap::new();
    attrs_with_date.insert(
        "start-date".to_string(),
        AttributeValueSpec {
            value: serde_json::json!("2024-01-01"),
            value_type: "date".to_string(),
        },
    );

    group.bench_function("2_role_players_with_attrs", |b| {
        b.iter(|| {
            builder::build_relation_insert(
                black_box("employment"),
                black_box(&role_players_2),
                black_box(&attrs_with_date),
                black_box(&schema),
            )
            .unwrap()
        })
    });

    group.finish();
}

fn bench_batch_inserts(c: &mut Criterion) {
    let schema = test_schema();
    let attrs = make_attrs(3);

    c.bench_function("crud/batch_10_entity_inserts", |b| {
        b.iter(|| {
            for _ in 0..10 {
                builder::build_entity_insert(
                    black_box("person"),
                    black_box(&attrs),
                    black_box(&schema),
                )
                .unwrap();
            }
        })
    });
}

criterion_group!(
    benches,
    bench_entity_insert,
    bench_entity_fetch,
    bench_entity_fetch_by_iid,
    bench_entity_update,
    bench_relation_insert,
    bench_batch_inserts,
);
criterion_main!(benches);
