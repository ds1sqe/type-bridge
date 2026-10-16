//! Instruction counts for `schema.rs`; see `validation_iai.rs` for the
//! conventions.

#![allow(deprecated)] // Frozen released-schema recovery remains available until 3.0.
mod common;

use std::hint::black_box;

use gungraun::{library_benchmark, library_benchmark_group, main};
use type_bridge_core_lib::_schema::TypeSchema;

fn medium_schema() -> TypeSchema {
    TypeSchema::from_typeql(common::MEDIUM_SCHEMA).unwrap()
}

#[library_benchmark]
#[bench::parse_small(common::SMALL_SCHEMA.to_owned())]
#[bench::parse_medium(common::MEDIUM_SCHEMA.to_owned())]
#[bench::parse_large_120types(common::large_schema())]
#[bench::resolve_inheritance_depth10(common::inheritance_schema())]
fn parse(tql: String) -> (String, TypeSchema) {
    let schema = TypeSchema::from_typeql(black_box(&tql)).unwrap();
    (tql, schema)
}

#[library_benchmark]
#[bench::to_json(medium_schema())]
fn to_json(schema: TypeSchema) -> (TypeSchema, String) {
    let json = black_box(&schema).to_json().unwrap();
    (schema, json)
}

#[library_benchmark]
#[bench::from_json(medium_schema().to_json().unwrap())]
fn from_json(json: String) -> (String, TypeSchema) {
    let schema = TypeSchema::from_json(black_box(&json)).unwrap();
    (json, schema)
}

library_benchmark_group!(name = schema, benchmarks = [parse, to_json, from_json]);

main!(library_benchmark_groups = schema);
