#![allow(deprecated)] // Legacy node bridges are removed in 11.30 / 3.0.
mod common;

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use type_bridge_core_lib::compiler::QueryCompiler;

fn bench_compile_simple_match(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = common::make_simple_match();

    c.bench_function("compile/simple_match", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

fn bench_compile_match_with_constraints(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = common::make_match_with_constraints();

    c.bench_function("compile/match_with_constraints", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

fn bench_compile_complex_query(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = common::make_complex_query();

    c.bench_function("compile/complex_10_patterns", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

fn bench_compile_batch(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clauses = common::make_batch_clauses();

    c.bench_function("compile/batch_50_clauses", |b| {
        b.iter(|| compiler.compile(black_box(&clauses)))
    });
}

fn bench_compile_relation_insert(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = common::make_relation_insert();

    c.bench_function("compile/relation_insert", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

fn bench_compile_reduce(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = common::make_reduce_with_groupby();
    c.bench_function("compile/reduce_with_groupby", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

fn bench_compile_long_query(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = common::make_long_query();

    c.bench_function("compile/long_30_patterns", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

fn bench_compile_deeply_nested(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = common::make_deeply_nested();

    c.bench_function("compile/deeply_nested_3_levels", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

fn bench_compile_graph_traversal(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = common::make_graph_traversal();

    c.bench_function("compile/graph_traversal_8_hops", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

fn bench_compile_heavy_insert(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = common::make_heavy_insert();

    c.bench_function("compile/heavy_insert_100x6", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

fn bench_compile_large_batch(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clauses = common::make_large_batch();

    c.bench_function("compile/large_batch_200_clauses", |b| {
        b.iter(|| compiler.compile(black_box(&clauses)))
    });
}

fn bench_compile_arithmetic(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = common::make_arithmetic_match();
    c.bench_function("compile/arithmetic_match", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

fn bench_compile_nested_arithmetic(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = common::make_nested_arithmetic();
    c.bench_function("compile/nested_arithmetic_4_levels", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

// ---------------------------------------------------------------------------
// MatchLetClause compilation
// ---------------------------------------------------------------------------

fn bench_compile_match_let(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = common::make_match_let();
    c.bench_function("compile/match_let_3_assignments", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

// ---------------------------------------------------------------------------
// FetchVariable compilation
// ---------------------------------------------------------------------------

fn bench_compile_fetch_variable(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = common::make_fetch_variable();
    c.bench_function("compile/fetch_variable", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

// ---------------------------------------------------------------------------
// IsaConstraint compilation
// ---------------------------------------------------------------------------

fn bench_compile_isa_constraint(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = common::make_isa_constraint();
    c.bench_function("compile/isa_constraint", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

// ---------------------------------------------------------------------------
// Standalone clause types
// ---------------------------------------------------------------------------

fn bench_compile_standalone_insert(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = common::make_standalone_insert();
    c.bench_function("compile/standalone_insert_5x4", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

fn bench_compile_standalone_delete(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = common::make_standalone_delete();
    c.bench_function("compile/standalone_delete_10", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

fn bench_compile_standalone_update(c: &mut Criterion) {
    let compiler = QueryCompiler::new();
    let clause = common::make_standalone_update();
    c.bench_function("compile/standalone_update_20", |b| {
        b.iter(|| compiler.compile_clause(black_box(&clause)))
    });
}

criterion_group!(
    benches,
    bench_compile_simple_match,
    bench_compile_match_with_constraints,
    bench_compile_complex_query,
    bench_compile_batch,
    bench_compile_relation_insert,
    bench_compile_reduce,
    bench_compile_long_query,
    bench_compile_deeply_nested,
    bench_compile_graph_traversal,
    bench_compile_heavy_insert,
    bench_compile_large_batch,
    bench_compile_arithmetic,
    bench_compile_nested_arithmetic,
    bench_compile_match_let,
    bench_compile_fetch_variable,
    bench_compile_isa_constraint,
    bench_compile_standalone_insert,
    bench_compile_standalone_delete,
    bench_compile_standalone_update,
);
criterion_main!(benches);
