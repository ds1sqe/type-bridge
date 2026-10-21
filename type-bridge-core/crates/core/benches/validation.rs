#![allow(deprecated)] // Legacy node bridges are removed in 3.0.
mod common;

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use type_bridge_core_lib::validation::ValidationEngine;

fn bench_validate_single_name(c: &mut Criterion) {
    let engine = ValidationEngine::new();

    c.bench_function("validate_type_name/simple", |b| {
        b.iter(|| engine.validate_type_name(black_box("person"), black_box("entity")))
    });
}

fn bench_validate_long_name(c: &mut Criterion) {
    let engine = ValidationEngine::new();
    let long_name = common::long_type_name();

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
    let names = common::batch_type_names(1);

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
    let pattern = common::simple_entity_pattern();

    c.bench_function("validate_pattern/simple_entity", |b| {
        b.iter(|| engine.validate_pattern(black_box(&pattern)))
    });
}

fn bench_validate_pattern_complex(c: &mut Criterion) {
    let engine = ValidationEngine::new();
    let pattern = common::complex_nested_pattern();

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
        b.iter(|| {
            engine.validate_variable_name(black_box("$person"), black_box("entity"), black_box(""))
        })
    });
}

fn bench_validate_variable_name_invalid(c: &mut Criterion) {
    let engine = ValidationEngine::new();
    c.bench_function("validate_variable_name/invalid_no_dollar", |b| {
        b.iter(|| {
            engine.validate_variable_name(black_box("person"), black_box("entity"), black_box(""))
        })
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
// Statement validation
// ---------------------------------------------------------------------------

fn bench_validate_statement_simple(c: &mut Criterion) {
    let engine = ValidationEngine::new();
    let statement = common::simple_isa_statement();

    c.bench_function("validate_statement/simple_isa", |b| {
        b.iter(|| engine.validate_statement(black_box(&statement)))
    });
}

fn bench_validate_statement_relation(c: &mut Criterion) {
    let engine = ValidationEngine::new();
    let statement = common::relation_with_attrs_statement();

    c.bench_function("validate_statement/relation_with_attrs", |b| {
        b.iter(|| engine.validate_statement(black_box(&statement)))
    });
}

// ---------------------------------------------------------------------------
// Large batch: 5000 names
// ---------------------------------------------------------------------------

fn bench_validate_batch_5000(c: &mut Criterion) {
    let engine = ValidationEngine::new();
    let names = common::batch_type_names(5);

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
