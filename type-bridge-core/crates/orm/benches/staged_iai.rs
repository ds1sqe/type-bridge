//! Instruction counts for `staged.rs`; see `serde_iai.rs` for the
//! conventions.

mod common;

use std::hint::black_box;

use gungraun::{library_benchmark, library_benchmark_group};

#[library_benchmark]
#[bench::mixed_commit(common::staged_commit())]
fn staged_commit(staged: common::StagedCommit) -> (common::StagedCommit, usize) {
    let answers = black_box(&staged).run();
    (staged, answers)
}

library_benchmark_group!(name = staged, benchmarks = [staged_commit]);

common::gungraun_main!(library_benchmark_groups = staged);
