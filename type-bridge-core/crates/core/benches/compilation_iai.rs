//! Instruction counts for `compilation.rs`; see `validation_iai.rs` for the
//! conventions.

mod common;

use std::hint::black_box;

use gungraun::{library_benchmark, library_benchmark_group, main};
use type_bridge_core_lib::ast::Clause;
use type_bridge_core_lib::compiler::QueryCompiler;

#[library_benchmark]
#[bench::simple_match(common::make_simple_match())]
#[bench::match_with_constraints(common::make_match_with_constraints())]
#[bench::complex_10_patterns(common::make_complex_query())]
#[bench::relation_insert(common::make_relation_insert())]
#[bench::reduce_with_groupby(common::make_reduce_with_groupby())]
#[bench::long_30_patterns(common::make_long_query())]
#[bench::deeply_nested_3_levels(common::make_deeply_nested())]
#[bench::graph_traversal_8_hops(common::make_graph_traversal())]
#[bench::heavy_insert_100x6(common::make_heavy_insert())]
#[bench::arithmetic_match(common::make_arithmetic_match())]
#[bench::nested_arithmetic_4_levels(common::make_nested_arithmetic())]
#[bench::match_let_3_assignments(common::make_match_let())]
#[bench::fetch_variable(common::make_fetch_variable())]
#[bench::isa_constraint(common::make_isa_constraint())]
#[bench::standalone_insert_5x4(common::make_standalone_insert())]
#[bench::standalone_delete_10(common::make_standalone_delete())]
#[bench::standalone_update_20(common::make_standalone_update())]
fn clause(clause: Clause) -> (Clause, String) {
    let typeql = QueryCompiler::new().compile_clause(black_box(&clause));
    (clause, typeql)
}

#[library_benchmark]
#[bench::batch_50_clauses(common::make_batch_clauses())]
#[bench::large_batch_200_clauses(common::make_large_batch())]
fn clauses(clauses: Vec<Clause>) -> (Vec<Clause>, String) {
    let typeql = QueryCompiler::new().compile(black_box(&clauses));
    (clauses, typeql)
}

library_benchmark_group!(name = compile, benchmarks = [clause, clauses]);

main!(library_benchmark_groups = compile);
