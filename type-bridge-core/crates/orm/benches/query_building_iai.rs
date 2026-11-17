//! Instruction counts for `query_building.rs`; see `serde_iai.rs` for the
//! conventions.

mod common;

use std::hint::black_box;

use gungraun::{library_benchmark, library_benchmark_group};
use type_bridge_orm::_schema::info::SchemaInfo;

#[library_benchmark]
#[bench::small_5_entities(common::make_schema_info_for_generation(5))]
#[bench::medium_10_entities(common::make_schema_info_for_generation(10))]
#[bench::large_30_entities(common::make_schema_info_for_generation(30))]
fn to_typeql(info: SchemaInfo) -> (SchemaInfo, String) {
    let typeql = black_box(&info).to_typeql().unwrap();
    (info, typeql)
}

library_benchmark_group!(name = query_schema_generate, benchmarks = [to_typeql]);

common::gungraun_main!(library_benchmark_groups = query_schema_generate);
