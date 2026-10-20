mod common;

use criterion::{Criterion, black_box, criterion_group, criterion_main};

use type_bridge_orm::_codegen::generator::generate_from_typeql;

fn bench_schema_generate_define(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/schema_generate");

    let small = common::make_schema_info_for_generation(5);
    group.bench_function("small_5_entities", |b| {
        b.iter(|| black_box(&small).to_typeql().unwrap())
    });

    let medium = common::make_schema_info_for_generation(10);
    group.bench_function("medium_10_entities", |b| {
        b.iter(|| black_box(&medium).to_typeql().unwrap())
    });

    let large = common::make_schema_info_for_generation(30);
    group.bench_function("large_30_entities", |b| {
        b.iter(|| black_box(&large).to_typeql().unwrap())
    });

    group.finish();
}

fn bench_codegen_from_typeql(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/codegen");

    group.bench_function("small_schema", |b| {
        b.iter(|| {
            generate_from_typeql(black_box(common::SMALL_CODEGEN_SCHEMA)).unwrap();
        })
    });

    let medium_tql = common::medium_codegen_schema();
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
