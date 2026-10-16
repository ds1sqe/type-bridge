mod common;

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use type_bridge_core_lib::_schema::TypeSchema;

fn bench_parse_small_schema(c: &mut Criterion) {
    c.bench_function("schema/parse_small", |b| {
        b.iter(|| TypeSchema::from_typeql(black_box(common::SMALL_SCHEMA)).unwrap())
    });
}

fn bench_parse_medium_schema(c: &mut Criterion) {
    c.bench_function("schema/parse_medium", |b| {
        b.iter(|| TypeSchema::from_typeql(black_box(common::MEDIUM_SCHEMA)).unwrap())
    });
}

fn bench_parse_large_schema(c: &mut Criterion) {
    let tql = common::large_schema();

    c.bench_function("schema/parse_large_120types", |b| {
        b.iter(|| TypeSchema::from_typeql(black_box(&tql)).unwrap())
    });
}

fn bench_resolve_inheritance(c: &mut Criterion) {
    let tql = common::inheritance_schema();

    c.bench_function("schema/resolve_inheritance_depth10", |b| {
        b.iter(|| TypeSchema::from_typeql(black_box(&tql)).unwrap())
    });
}

fn bench_json_round_trip(c: &mut Criterion) {
    let schema = TypeSchema::from_typeql(common::MEDIUM_SCHEMA).unwrap();
    let json = schema.to_json().unwrap();

    c.bench_function("schema/to_json", |b| {
        b.iter(|| black_box(&schema).to_json().unwrap())
    });

    c.bench_function("schema/from_json", |b| {
        b.iter(|| TypeSchema::from_json(black_box(&json)).unwrap())
    });
}

criterion_group!(
    benches,
    bench_parse_small_schema,
    bench_parse_medium_schema,
    bench_parse_large_schema,
    bench_resolve_inheritance,
    bench_json_round_trip,
);
criterion_main!(benches);
