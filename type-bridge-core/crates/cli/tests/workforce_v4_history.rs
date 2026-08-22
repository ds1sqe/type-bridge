//! The committed Workforce V4 history remains replayable through the shipped CLI.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/contracts/sdk_conformance/workforce-v4/workspace")
}

fn run(arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_type-bridge"))
        .current_dir(fixture())
        .args(arguments)
        .output()
        .expect("the shipped CLI runs against Workforce V4")
}

#[test]
fn committed_expand_backfill_contract_history_replays_offline() {
    let check = run(&["schema", "check"]);
    assert!(
        check.status.success(),
        "schema check failed: {}",
        String::from_utf8_lossy(&check.stderr),
    );

    let plan = run(&["migration", "plan"]);
    assert!(
        plan.status.success(),
        "migration plan failed: {}",
        String::from_utf8_lossy(&plan.stderr),
    );
    assert_eq!(
        String::from_utf8(plan.stdout).expect("plan output is UTF-8"),
        "workforcev4/0001_initial  safety=Conditional  reversible=true\n\
         workforcev4/0002_expand-display-name  safety=Additive  reversible=true\n\
         workforcev4/0003_backfill-display-name  safety=BackfillRequired  reversible=true\n\
         workforcev4/0004_contract-legacy-name  safety=Destructive  reversible=true\n",
    );
}
