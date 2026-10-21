//! Instruction counts for `validation.rs`. Each group and bench id matches
//! a Criterion id (`<group>/<bench>`), and the inputs come from
//! `common/mod.rs`. Inputs are built outside the measured function and
//! returned from it, so neither construction nor drop is counted.

#![allow(deprecated)] // Legacy node bridges are removed in 3.0.
mod common;

use std::hint::black_box;

use gungraun::{library_benchmark, library_benchmark_group};
use type_bridge_core_lib::ast::{Pattern, Statement};
use type_bridge_core_lib::validation::{ValidationEngine, ValidationError, ValidationResult};

#[library_benchmark]
#[bench::simple(ValidationEngine::new(), "person".to_owned(), "entity")]
#[bench::long_100ch(ValidationEngine::new(), common::long_type_name(), "entity")]
#[bench::unicode(ValidationEngine::new(), "\u{00e9}l\u{00e8}ve".to_owned(), "entity")]
#[bench::reserved_word(ValidationEngine::new(), "match".to_owned(), "entity")]
#[bench::relation_context(ValidationEngine::new(), "employment".to_owned(), "relation")]
#[bench::attribute_context(ValidationEngine::new(), "first-name".to_owned(), "attribute")]
#[bench::role_context(ValidationEngine::new(), "employee".to_owned(), "role")]
#[bench::invalid_digit_start(ValidationEngine::new(), "1st-entity".to_owned(), "entity")]
fn type_name(
    engine: ValidationEngine,
    name: String,
    context: &str,
) -> (ValidationEngine, String, ValidationResult) {
    let result = engine.validate_type_name(black_box(&name), black_box(context));
    (engine, name, result)
}

#[library_benchmark]
#[bench::batch_1000(ValidationEngine::new(), common::batch_type_names(1))]
#[bench::batch_5000(ValidationEngine::new(), common::batch_type_names(5))]
fn type_name_batch(
    engine: ValidationEngine,
    names: Vec<String>,
) -> (ValidationEngine, Vec<String>) {
    for name in &names {
        let _ = black_box(engine.validate_type_name(black_box(name), black_box("entity")));
    }
    (engine, names)
}

#[library_benchmark]
#[bench::simple_entity(ValidationEngine::new(), common::simple_entity_pattern())]
#[bench::complex_nested(ValidationEngine::new(), common::complex_nested_pattern())]
fn pattern(
    engine: ValidationEngine,
    pattern: Pattern,
) -> (ValidationEngine, Pattern, ValidationResult) {
    let result = engine.validate_pattern(black_box(&pattern));
    (engine, pattern, result)
}

#[library_benchmark]
#[bench::valid(ValidationEngine::new(), "$person")]
#[bench::invalid_no_dollar(ValidationEngine::new(), "person")]
fn variable_name(engine: ValidationEngine, name: &str) -> (ValidationEngine, Vec<ValidationError>) {
    let result = engine.validate_variable_name(black_box(name), black_box("entity"), black_box(""));
    (engine, result)
}

#[library_benchmark]
#[bench::simple_isa(ValidationEngine::new(), common::simple_isa_statement())]
#[bench::relation_with_attrs(ValidationEngine::new(), common::relation_with_attrs_statement())]
fn statement(
    engine: ValidationEngine,
    statement: Statement,
) -> (ValidationEngine, Statement, ValidationResult) {
    let result = engine.validate_statement(black_box(&statement));
    (engine, statement, result)
}

library_benchmark_group!(
    name = validate_type_name,
    benchmarks = [type_name, type_name_batch]
);
library_benchmark_group!(name = validate_pattern, benchmarks = [pattern]);
library_benchmark_group!(name = validate_variable_name, benchmarks = [variable_name]);
library_benchmark_group!(name = validate_statement, benchmarks = [statement]);

common::gungraun_main!(
    library_benchmark_groups = validate_type_name,
    validate_pattern,
    validate_variable_name,
    validate_statement
);
