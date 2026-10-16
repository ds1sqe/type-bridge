mod common;

use criterion::{Criterion, black_box, criterion_group, criterion_main};

use type_bridge_orm::_attribute::ValueType;
use type_bridge_orm::_schema::info::SchemaInfo;
use type_bridge_orm::expr::{Agg, Expr, SortDir};
use type_bridge_orm::value::AttributeValue;

fn bench_attribute_value_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("serde/attribute_value");

    let string_val = common::string_value();
    group.bench_function("string_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(black_box(&string_val)).unwrap();
            let _: AttributeValue = serde_json::from_str(&json).unwrap();
        })
    });

    let long_val = common::long_value();
    group.bench_function("long_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(black_box(&long_val)).unwrap();
            let _: AttributeValue = serde_json::from_str(&json).unwrap();
        })
    });

    let double_val = common::double_value();
    group.bench_function("double_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(black_box(&double_val)).unwrap();
            let _: AttributeValue = serde_json::from_str(&json).unwrap();
        })
    });

    let bool_val = common::boolean_value();
    group.bench_function("boolean_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(black_box(&bool_val)).unwrap();
            let _: AttributeValue = serde_json::from_str(&json).unwrap();
        })
    });

    let date_val = common::date_value();
    group.bench_function("date_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(black_box(&date_val)).unwrap();
            let _: AttributeValue = serde_json::from_str(&json).unwrap();
        })
    });

    let datetime_val = common::datetime_value();
    group.bench_function("datetime_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(black_box(&datetime_val)).unwrap();
            let _: AttributeValue = serde_json::from_str(&json).unwrap();
        })
    });

    group.finish();
}

fn bench_value_type_roundtrip(c: &mut Criterion) {
    let types = common::value_types();

    c.bench_function("serde/value_type_batch_roundtrip", |b| {
        b.iter(|| {
            for vt in black_box(&types) {
                let json = serde_json::to_string(vt).unwrap();
                let _: ValueType = serde_json::from_str(&json).unwrap();
            }
        })
    });
}

fn bench_schema_info_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("serde/schema_info");

    let small = common::make_small_schema_info();
    let small_json = serde_json::to_string(&small).unwrap();
    group.bench_function("small_serialize", |b| {
        b.iter(|| serde_json::to_string(black_box(&small)).unwrap())
    });
    group.bench_function("small_deserialize", |b| {
        b.iter(|| {
            let _: SchemaInfo = serde_json::from_str(black_box(&small_json)).unwrap();
        })
    });

    let large = common::make_large_schema_info();
    let large_json = serde_json::to_string(&large).unwrap();
    group.bench_function("large_serialize", |b| {
        b.iter(|| serde_json::to_string(black_box(&large)).unwrap())
    });
    group.bench_function("large_deserialize", |b| {
        b.iter(|| {
            let _: SchemaInfo = serde_json::from_str(black_box(&large_json)).unwrap();
        })
    });

    group.finish();
}

fn bench_expr_roundtrip(c: &mut Criterion) {
    let deep_expr = common::deep_expr();
    let deep_json = serde_json::to_string(&deep_expr).unwrap();

    let mut group = c.benchmark_group("serde/expr");
    group.bench_function("deep_serialize", |b| {
        b.iter(|| serde_json::to_string(black_box(&deep_expr)).unwrap())
    });
    group.bench_function("deep_deserialize", |b| {
        b.iter(|| {
            let _: Expr = serde_json::from_str(black_box(&deep_json)).unwrap();
        })
    });

    // Agg round-trip
    let aggs = common::aggs();
    group.bench_function("agg_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(black_box(&aggs)).unwrap();
            let _: Vec<Agg> = serde_json::from_str(&json).unwrap();
        })
    });

    // SortDir round-trip
    let dirs = common::sort_dirs();
    group.bench_function("sort_dir_roundtrip", |b| {
        b.iter(|| {
            let json = serde_json::to_string(black_box(&dirs)).unwrap();
            let _: Vec<SortDir> = serde_json::from_str(&json).unwrap();
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_attribute_value_roundtrip,
    bench_value_type_roundtrip,
    bench_schema_info_roundtrip,
    bench_expr_roundtrip,
);
criterion_main!(benches);
