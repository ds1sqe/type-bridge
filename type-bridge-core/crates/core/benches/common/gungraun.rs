//! Entry point for the gungraun (instruction count) benches of the core and
//! ORM crates; the ORM benches include this file by path.

/// Expands `gungraun::main!` behind a guard so the bench binary is a no-op
/// under `cargo test`, as Criterion benches are.
///
/// gungraun's own `main` always hands the benchmarks to an external
/// `gungraun-runner`, so `cargo test --all-targets` fails wherever the runner
/// is not installed. The guard runs it only when `cargo bench` started the
/// binary (cargo passes `--bench`) or when the runner re-executes it to
/// measure one benchmark (`--gungraun-run`).
#[allow(unused_macros)] // Criterion bench binaries compile it unused.
macro_rules! gungraun_main {
    ($($args:tt)*) => {
        fn main() {
            if std::env::args()
                .skip(1)
                .any(|arg| arg == "--bench" || arg == "--gungraun-run")
            {
                // Items in this block shadow the outer `main`, so the call
                // below enters gungraun's generated entry point.
                gungraun::main!($($args)*);
                main();
            }
        }
    };
}

pub(crate) use gungraun_main;
