//! Run one unit test alone in a child process of this test binary.
//!
//! The cyclic collector's enable flag and GIL ownership are interpreter-wide.
//! Batch tests toggle the flag and many tests hold the GIL, so a test that
//! asserts either one cannot share a process with the parallel test threads.

use std::process::Command;

/// Names the one test a child process runs, so the child executes the body
/// instead of spawning again.
const ISOLATED_TEST_ENV: &str = "TYPE_BRIDGE_CORE_ISOLATED_TEST";

/// Run `body` in a child process that executes only this test.
///
/// Pass `module_path!()` and the test function's name. The parent re-runs the
/// current test binary filtered to that exact test on one test thread and
/// fails unless the child reports exactly one passing test; the child runs
/// `body`.
pub(crate) fn run_isolated(module: &str, test: &str, body: impl FnOnce()) {
    // libtest names tests by their path inside the crate.
    let module = module.split_once("::").map_or("", |(_, path)| path);
    let name = format!("{module}::{test}");
    if std::env::var(ISOLATED_TEST_ENV).is_ok_and(|isolated| isolated == name) {
        body();
        return;
    }

    let binary = std::env::current_exe().expect("the test binary should know its own path");
    let output = Command::new(binary)
        .args([name.as_str(), "--exact", "--test-threads=1", "--nocapture"])
        .env(ISOLATED_TEST_ENV, &name)
        .output()
        .expect("the isolated test process should start");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("test result: ok. 1 passed;"),
        "isolated test `{name}` failed ({}):\n--- stdout\n{stdout}\n--- stderr\n{stderr}",
        output.status,
    );
}
