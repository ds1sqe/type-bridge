//! Instruction counts for `serde.rs`. Each group and bench id matches a
//! Criterion id (`<group>/<bench>`, a `/` inside the Criterion group written
//! as `_`), and the inputs come from `common/mod.rs`. Inputs are built
//! outside the measured function and returned from it, so neither
//! construction nor drop is counted.

mod common;

use std::hint::black_box;

use gungraun::{library_benchmark, library_benchmark_group, main};
use type_bridge_orm::_attribute::ValueType;
use type_bridge_orm::_schema::info::SchemaInfo;
use type_bridge_orm::expr::{Agg, Expr, SortDir};
use type_bridge_orm::value::AttributeValue;

#[library_benchmark]
#[bench::string_roundtrip(common::string_value())]
#[bench::long_roundtrip(common::long_value())]
#[bench::double_roundtrip(common::double_value())]
#[bench::boolean_roundtrip(common::boolean_value())]
#[bench::date_roundtrip(common::date_value())]
#[bench::datetime_roundtrip(common::datetime_value())]
fn attribute_value(value: AttributeValue) -> (AttributeValue, AttributeValue) {
    let json = serde_json::to_string(black_box(&value)).unwrap();
    let decoded: AttributeValue = serde_json::from_str(&json).unwrap();
    (value, decoded)
}

#[library_benchmark]
#[bench::value_type_batch_roundtrip(common::value_types())]
fn value_types(types: [ValueType; 9]) -> [ValueType; 9] {
    for vt in black_box(&types) {
        let json = serde_json::to_string(vt).unwrap();
        let _: ValueType = black_box(serde_json::from_str(&json).unwrap());
    }
    types
}

#[library_benchmark]
#[bench::small_serialize(common::make_small_schema_info())]
#[bench::large_serialize(common::make_large_schema_info())]
fn schema_info_serialize(info: SchemaInfo) -> (SchemaInfo, String) {
    let json = serde_json::to_string(black_box(&info)).unwrap();
    (info, json)
}

#[library_benchmark]
#[bench::small_deserialize(serde_json::to_string(&common::make_small_schema_info()).unwrap())]
#[bench::large_deserialize(serde_json::to_string(&common::make_large_schema_info()).unwrap())]
fn schema_info_deserialize(json: String) -> (String, SchemaInfo) {
    let info = serde_json::from_str(black_box(&json)).unwrap();
    (json, info)
}

#[library_benchmark]
#[bench::deep_serialize(common::deep_expr())]
fn expr_serialize(expr: Expr) -> (Expr, String) {
    let json = serde_json::to_string(black_box(&expr)).unwrap();
    (expr, json)
}

#[library_benchmark]
#[bench::deep_deserialize(serde_json::to_string(&common::deep_expr()).unwrap())]
fn expr_deserialize(json: String) -> (String, Expr) {
    let expr = serde_json::from_str(black_box(&json)).unwrap();
    (json, expr)
}

#[library_benchmark]
#[bench::agg_roundtrip(common::aggs())]
fn aggs(aggs: Vec<Agg>) -> (Vec<Agg>, Vec<Agg>) {
    let json = serde_json::to_string(black_box(&aggs)).unwrap();
    let decoded: Vec<Agg> = serde_json::from_str(&json).unwrap();
    (aggs, decoded)
}

#[library_benchmark]
#[bench::sort_dir_roundtrip(common::sort_dirs())]
fn sort_dirs(dirs: Vec<SortDir>) -> (Vec<SortDir>, Vec<SortDir>) {
    let json = serde_json::to_string(black_box(&dirs)).unwrap();
    let decoded: Vec<SortDir> = serde_json::from_str(&json).unwrap();
    (dirs, decoded)
}

library_benchmark_group!(name = serde_attribute_value, benchmarks = [attribute_value]);
library_benchmark_group!(name = serde, benchmarks = [value_types]);
library_benchmark_group!(
    name = serde_schema_info,
    benchmarks = [schema_info_serialize, schema_info_deserialize]
);
library_benchmark_group!(
    name = serde_expr,
    benchmarks = [expr_serialize, expr_deserialize, aggs, sort_dirs]
);

main!(
    library_benchmark_groups = serde_attribute_value,
    serde,
    serde_schema_info,
    serde_expr
);
