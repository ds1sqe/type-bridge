//! Instruction counts for `query_building.rs`; see `serde_iai.rs` for the
//! conventions.

mod common;

use std::hint::black_box;

use gungraun::{library_benchmark, library_benchmark_group};
use type_bridge_orm::_codegen::generator::{GeneratedModels, generate_from_typeql};
use type_bridge_orm::_schema::info::SchemaInfo;

#[library_benchmark]
#[bench::small_5_entities(common::make_schema_info_for_generation(5))]
#[bench::medium_10_entities(common::make_schema_info_for_generation(10))]
#[bench::large_30_entities(common::make_schema_info_for_generation(30))]
fn to_typeql(info: SchemaInfo) -> (SchemaInfo, String) {
    let typeql = black_box(&info).to_typeql().unwrap();
    (info, typeql)
}

#[library_benchmark]
#[bench::small_schema(common::SMALL_CODEGEN_SCHEMA.to_owned())]
#[bench::medium_schema(common::medium_codegen_schema())]
fn codegen(tql: String) -> (String, GeneratedModels) {
    let models = generate_from_typeql(black_box(&tql)).unwrap();
    (tql, models)
}

library_benchmark_group!(name = query_schema_generate, benchmarks = [to_typeql]);
library_benchmark_group!(name = query_codegen, benchmarks = [codegen]);

common::gungraun_main!(
    library_benchmark_groups = query_schema_generate,
    query_codegen
);
