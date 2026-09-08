//! The committed Sdk V4 history remains replayable through the shipped CLI.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest as _, Sha256};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/contracts/sdk_conformance/sdk-v4/workspace")
}

fn run(arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_type-bridge"))
        .current_dir(fixture())
        .args(arguments)
        .output()
        .expect("the shipped CLI runs against Sdk V4")
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
        "sdkv4/0001_initial  safety=Conditional  reversible=true\n\
         sdkv4/0002_expand-display-name  safety=Additive  reversible=true\n\
         sdkv4/0003_backfill-display-name  safety=BackfillRequired  reversible=true\n\
         sdkv4/0004_contract-legacy-name  safety=Destructive  reversible=true\n",
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
    assert_eq!(canonical.len(), 48_758);
    let digest = Sha256::digest(&canonical)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(
        digest,
        "54a97d4d43f81bf1a7af65f0683bab571dafaa0efc6b8a1fe2e7dc0a1914000a",
    );
}

#[test]
fn generated_rust_public_facade_observes_the_v4_catalog() {
    let temporary = tempfile::tempdir().expect("isolated workspace creates");
    copy_tree(&fixture(), temporary.path());
    let generation = Command::new(env!("CARGO_BIN_EXE_type-bridge"))
        .current_dir(temporary.path())
        .args(["schema", "generate"])
        .output()
        .expect("schema generation runs");
    assert!(generation.status.success());

    let package = temporary.path().join("generated/rust");
    let manifest = package.join("Cargo.toml");
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../rust")
        .canonicalize()
        .expect("local Rust runtime resolves");
    let source = fs::read_to_string(&manifest).expect("generated manifest reads");
    fs::write(
        &manifest,
        source.replace(
            "type-bridge = { version = \"=2.2.0\", default-features = false }",
            &format!(
                "type-bridge = {{ path = {:?}, default-features = false }}",
                runtime
            ),
        ),
    )
    .expect("consumer binds the in-tree public runtime");
    fs::create_dir_all(package.join("src/bin")).expect("consumer bin directory creates");
    fs::write(
        package.join("src/bin/observe.rs"),
        r#"use type_bridge_generated_schema::open_migration_catalog;

fn main() {
    let catalog = open_migration_catalog().expect("generated catalog opens");
    let applied = (0..catalog.len())
        .map(|index| catalog.entry(index).expect("catalog entry").id().clone())
        .collect::<Vec<_>>();
    let apply = catalog.preview_apply(Vec::new(), None).expect("apply preview");
    let rollback = catalog
        .preview_rollback(applied.clone(), applied)
        .expect("rollback preview");
    let apply_order = (0..apply.len())
        .map(|index| {
            let entry = apply.entry(index).expect("apply entry");
            format!("{}/{}", entry.id().app_label().as_str(), entry.id().name().as_str())
        })
        .collect::<Vec<_>>();
    let rollback_order = (0..rollback.len())
        .map(|index| {
            let entry = rollback.entry(index).expect("rollback entry");
            format!("{}/{}", entry.id().app_label().as_str(), entry.id().name().as_str())
        })
        .collect::<Vec<_>>();
    let backfills = (0..apply.len())
        .map(|index| apply.entry(index).expect("apply entry").backfill_count())
        .sum::<usize>();
    println!(
        "{}|{}|{}|{}|{}",
        catalog.len(),
        catalog.fingerprint().digest().to_hex(),
        apply_order.join(","),
        rollback_order.join(","),
        backfills,
    );
}
"#,
    )
    .expect("source-bound producer writes");
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    let observation = Command::new("cargo")
        .current_dir(&package)
        .env("CARGO_TARGET_DIR", target)
        .args(["run", "--quiet", "--bin", "observe"])
        .output()
        .expect("generated Rust producer runs");
    assert!(
        observation.status.success(),
        "producer failed: {}",
        String::from_utf8_lossy(&observation.stderr),
    );
    let fields = String::from_utf8(observation.stdout)
        .expect("observation is UTF-8")
        .trim()
        .split('|')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(fields.len(), 5);
    assert_eq!(fields[0], "4");
    assert_eq!(
        fields[1],
        "efa3249f25fb2decaa0fef24c99d3a9884942d2e7926b3545955eb21eee7accb",
    );
    assert_eq!(
        fields[2],
        "sdkv4/0001_initial,sdkv4/0002_expand-display-name,sdkv4/0003_backfill-display-name,sdkv4/0004_contract-legacy-name",
    );
    assert_eq!(
        fields[3],
        "sdkv4/0004_contract-legacy-name,sdkv4/0003_backfill-display-name,sdkv4/0002_expand-display-name,sdkv4/0001_initial",
    );
    assert_eq!(fields[4], "1");
}
