use criterion::{Criterion, black_box, criterion_group, criterion_main};

use type_bridge_orm::_attribute::ValueType;
use type_bridge_orm::_codegen::generator::generate_from_typeql;
use type_bridge_orm::_schema::info::*;

fn make_schema_info_for_generation(entity_count: usize) -> SchemaInfo {
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

fn bench_schema_generate_define(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/schema_generate");

    let small = make_schema_info_for_generation(5);
    group.bench_function("small_5_entities", |b| {
        b.iter(|| black_box(&small).to_typeql().unwrap())
    });

    let medium = make_schema_info_for_generation(10);
    group.bench_function("medium_10_entities", |b| {
        b.iter(|| black_box(&medium).to_typeql().unwrap())
    });

    let large = make_schema_info_for_generation(30);
    group.bench_function("large_30_entities", |b| {
        b.iter(|| black_box(&large).to_typeql().unwrap())
    });

    group.finish();
}

fn bench_codegen_from_typeql(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/codegen");

    let small_tql = r#"
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

    group.bench_function("small_schema", |b| {
        b.iter(|| {
            generate_from_typeql(black_box(small_tql)).unwrap();
        })
    });

    // Build medium schema (20 types)
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

    group.bench_function("medium_schema", |b| {
        b.iter(|| {
            generate_from_typeql(black_box(&medium_tql)).unwrap();
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_schema_generate_define,
    bench_codegen_from_typeql
);
criterion_main!(benches);
