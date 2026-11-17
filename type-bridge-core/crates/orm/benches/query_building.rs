mod common;

use criterion::{Criterion, black_box, criterion_group, criterion_main};

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

criterion_group!(benches, bench_schema_generate_define);
criterion_main!(benches);
