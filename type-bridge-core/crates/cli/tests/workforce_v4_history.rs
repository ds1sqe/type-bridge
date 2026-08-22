//! The committed Workforce V4 history remains replayable through the shipped CLI.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest as _, Sha256};

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

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("fixture destination creates");
    for entry in fs::read_dir(source).expect("fixture directory reads") {
        let entry = entry.expect("fixture entry reads");
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("fixture type reads").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("fixture file copies");
        }
    }
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

#[test]
fn all_four_generated_packages_embed_identical_canonical_history() {
    let temporary = tempfile::tempdir().expect("isolated workspace creates");
    copy_tree(&fixture(), temporary.path());
    let output = Command::new(env!("CARGO_BIN_EXE_type-bridge"))
        .current_dir(temporary.path())
        .args(["schema", "generate"])
        .output()
        .expect("schema generation runs");
    assert!(
        output.status.success(),
        "schema generation failed: {}",
        String::from_utf8_lossy(&output.stderr),
    );

    let mut canonical = None;
    for binding in ["python", "typescript", "rust", "c"] {
        let bytes = fs::read(
            temporary
                .path()
                .join("generated")
                .join(binding)
                .join("typebridge/migration-history.json"),
        )
        .unwrap_or_else(|error| panic!("{binding} history resource reads: {error}"));
        if let Some(expected) = &canonical {
            assert_eq!(&bytes, expected, "{binding} embedded different history");
        } else {
            canonical = Some(bytes);
        }
    }
    let canonical = canonical.expect("one canonical history resource");
    assert_eq!(canonical.len(), 48_902);
    let digest = Sha256::digest(&canonical)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(
        digest,
        "091cad62b2db770c886898101b281f327206da3285ad64239b53c9c6c2f0cfff",
    );
}
