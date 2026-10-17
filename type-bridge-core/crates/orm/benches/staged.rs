mod common;

use criterion::{Criterion, black_box, criterion_group, criterion_main};

fn bench_staged_commit(c: &mut Criterion) {
    let mut group = c.benchmark_group("staged");
    let staged = common::staged_commit();
    group.bench_function("mixed_commit", |b| b.iter(|| black_box(&staged).run()));
    group.finish();
}

criterion_group!(benches, bench_staged_commit);
criterion_main!(benches);
