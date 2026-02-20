use criterion::{black_box, criterion_group, criterion_main, Criterion};
use serde_json::json;
use type_bridge_core_lib::ast::{Constraint, Pattern, Statement, Value, LiteralValue, RolePlayer};
use type_bridge_core_lib::validation::ValidationEngine;

fn bench_validate_single_name(c: &mut Criterion) {
    let engine = ValidationEngine::new();

    c.bench_function("validate_type_name/simple", |b| {
        b.iter(|| engine.validate_type_name(black_box("person"), black_box("entity")))
    });
}

fn bench_validate_long_name(c: &mut Criterion) {
    let engine = ValidationEngine::new();
    let long_name = "a".repeat(100) + "-type";

    c.bench_function("validate_type_name/long_100ch", |b| {
        b.iter(|| engine.validate_type_name(black_box(&long_name), black_box("entity")))
    });
}

fn bench_validate_unicode_name(c: &mut Criterion) {
    let engine = ValidationEngine::new();

    c.bench_function("validate_type_name/unicode", |b| {
        b.iter(|| engine.validate_type_name(black_box("\u{00e9}l\u{00e8}ve"), black_box("entity")))
    });
}

fn bench_validate_reserved_word(c: &mut Criterion) {
    let engine = ValidationEngine::new();

    c.bench_function("validate_type_name/reserved_word", |b| {
        b.iter(|| engine.validate_type_name(black_box("match"), black_box("entity")))
    });
}

fn bench_validate_batch_names(c: &mut Criterion) {
    let engine = ValidationEngine::new();
    let mut names: Vec<String> = Vec::with_capacity(1000);
    for i in 0..400 {
        names.push(format!("entity-type-{}", i));
    }
    for i in 0..300 {
        names.push(format!("my-long-attribute-name-for-testing-{}", i));
    }
    for i in 0..200 {
        names.push(format!("relation-{}-data", i));
    }
    for i in 0..100 {
        names.push(format!("\u{00e9}l\u{00e8}ve-{}", i));
    }

    c.bench_function("validate_type_name/batch_1000", |b| {
        b.iter(|| {
            for name in &names {
                let _ = engine.validate_type_name(black_box(name), black_box("entity"));
            }
        })
    });
}

fn bench_validate_pattern_simple(c: &mut Criterion) {
    let engine = ValidationEngine::new();
    let pattern = Pattern::Entity {
        variable: "$p".to_string(),
        type_name: "person".to_string(),
        constraints: vec![
            Constraint::Has {
                attr_name: "name".to_string(),
                value: Value::Literal(LiteralValue {
                    value: json!("Alice"),
                    value_type: "string".to_string(),
                }),
            },
        ],
        is_strict: false,
    };

    c.bench_function("validate_pattern/simple_entity", |b| {
        b.iter(|| engine.validate_pattern(black_box(&pattern)))
    });
}

fn bench_validate_pattern_complex(c: &mut Criterion) {
    let engine = ValidationEngine::new();
    let pattern = Pattern::Or(vec![
        vec![
            Pattern::Entity {
                variable: "$p".to_string(),
                type_name: "person".to_string(),
                constraints: vec![
                    Constraint::Has {
                        attr_name: "name".to_string(),
                        value: Value::Literal(LiteralValue {
                            value: json!("Alice"),
                            value_type: "string".to_string(),
                        }),
                    },
                    Constraint::Has {
                        attr_name: "age".to_string(),
                        value: Value::Literal(LiteralValue {
                            value: json!(30),
                            value_type: "long".to_string(),
                        }),
                    },
                ],
                is_strict: false,
            },
            Pattern::Relation {
                variable: "$r".to_string(),
                type_name: "employment".to_string(),
                role_players: vec![
                    RolePlayer { role: "employee".to_string(), player_var: "$p".to_string() },
                    RolePlayer { role: "employer".to_string(), player_var: "$c".to_string() },
                ],
                constraints: vec![],
            },
        ],
        vec![
            Pattern::Not(vec![
                Pattern::Entity {
                    variable: "$p".to_string(),
                    type_name: "retired-person".to_string(),
                    constraints: vec![],
                    is_strict: true,
                },
            ]),
        ],
    ]);

    c.bench_function("validate_pattern/complex_nested", |b| {
        b.iter(|| engine.validate_pattern(black_box(&pattern)))
    });
}

// ---------------------------------------------------------------------------
// Variable name validation
// ---------------------------------------------------------------------------

fn bench_validate_variable_name_valid(c: &mut Criterion) {
    let engine = ValidationEngine::new();
    c.bench_function("validate_variable_name/valid", |b| {
        b.iter(|| engine.validate_variable_name(black_box("$person"), black_box("entity"), black_box("")))
    });
}

fn bench_validate_variable_name_invalid(c: &mut Criterion) {
    let engine = ValidationEngine::new();
    c.bench_function("validate_variable_name/invalid_no_dollar", |b| {
        b.iter(|| engine.validate_variable_name(black_box("person"), black_box("entity"), black_box("")))
    });
}

// ---------------------------------------------------------------------------
// Multi-context type validation
// ---------------------------------------------------------------------------

fn bench_validate_type_name_relation(c: &mut Criterion) {
    let engine = ValidationEngine::new();
    c.bench_function("validate_type_name/relation_context", |b| {
        b.iter(|| engine.validate_type_name(black_box("employment"), black_box("relation")))
    });
}

fn bench_validate_type_name_attribute(c: &mut Criterion) {
    let engine = ValidationEngine::new();
    c.bench_function("validate_type_name/attribute_context", |b| {
        b.iter(|| engine.validate_type_name(black_box("first-name"), black_box("attribute")))
    });
}

fn bench_validate_type_name_role(c: &mut Criterion) {
    let engine = ValidationEngine::new();
    c.bench_function("validate_type_name/role_context", |b| {
        b.iter(|| engine.validate_type_name(black_box("employee"), black_box("role")))
    });
}

// ---------------------------------------------------------------------------
// Invalid name rejection
// ---------------------------------------------------------------------------

fn bench_validate_type_name_invalid_digit(c: &mut Criterion) {
    let engine = ValidationEngine::new();
    c.bench_function("validate_type_name/invalid_digit_start", |b| {
        b.iter(|| engine.validate_type_name(black_box("1st-entity"), black_box("entity")))
    });
}

// ---------------------------------------------------------------------------
// Large batch: 5000 names
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Statement validation
// ---------------------------------------------------------------------------

fn bench_validate_statement_simple(c: &mut Criterion) {
    let engine = ValidationEngine::new();
    let statement = Statement::Isa {
        variable: "$p".to_string(),
        type_name: "person".to_string(),
    };

    c.bench_function("validate_statement/simple_isa", |b| {
        b.iter(|| engine.validate_statement(black_box(&statement)))
    });
}

fn bench_validate_statement_relation(c: &mut Criterion) {
    let engine = ValidationEngine::new();
    let statement = Statement::Relation {
        variable: "$rel".to_string(),
        type_name: "employment".to_string(),
        role_players: vec![
            RolePlayer { role: "employee".to_string(), player_var: "$p".to_string() },
            RolePlayer { role: "employer".to_string(), player_var: "$c".to_string() },
        ],
        include_variable: true,
        attributes: vec![
            Statement::Has {
                subject_var: "$rel".to_string(),
                attr_name: "start-date".to_string(),
                value: Value::Literal(LiteralValue {
                    value: serde_json::json!("2024-01-15"),
                    value_type: "date".to_string(),
                }),
            },
            Statement::Has {
                subject_var: "$rel".to_string(),
                attr_name: "salary".to_string(),
                value: Value::Literal(LiteralValue {
                    value: serde_json::json!(95000),
                    value_type: "long".to_string(),
                }),
            },
        ],
    };

    c.bench_function("validate_statement/relation_with_attrs", |b| {
        b.iter(|| engine.validate_statement(black_box(&statement)))
    });
}

// ---------------------------------------------------------------------------
// Large batch: 5000 names
// ---------------------------------------------------------------------------

fn bench_validate_batch_5000(c: &mut Criterion) {
    let engine = ValidationEngine::new();
    let mut names: Vec<String> = Vec::with_capacity(5000);
    for i in 0..2000 {
        names.push(format!("entity-type-{}", i));
    }
    for i in 0..1500 {
        names.push(format!("my-long-attribute-name-for-testing-{}", i));
    }
    for i in 0..1000 {
        names.push(format!("relation-{}-data", i));
    }
    for i in 0..500 {
        names.push(format!("\u{00e9}l\u{00e8}ve-{}", i));
    }

    c.bench_function("validate_type_name/batch_5000", |b| {
        b.iter(|| {
            for name in &names {
                let _ = engine.validate_type_name(black_box(name), black_box("entity"));
            }
        })
    });
}

criterion_group!(
    benches,
    bench_validate_single_name,
    bench_validate_long_name,
    bench_validate_unicode_name,
    bench_validate_reserved_word,
    bench_validate_batch_names,
    bench_validate_pattern_simple,
    bench_validate_pattern_complex,
    bench_validate_variable_name_valid,
    bench_validate_variable_name_invalid,
    bench_validate_type_name_relation,
    bench_validate_type_name_attribute,
    bench_validate_type_name_role,
    bench_validate_type_name_invalid_digit,
    bench_validate_statement_simple,
    bench_validate_statement_relation,
    bench_validate_batch_5000,
);
criterion_main!(benches);
