//! The committed SDK administration history remains replayable through the shipped CLI.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest as _, Sha256};

mod support;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/contracts/sdk_conformance/administration/workspace")
}

fn run(arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_type-bridge"))
        .current_dir(fixture())
        .args(arguments)
        .output()
        .expect("the shipped CLI runs against SDK administration")
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
        "administration/0001_initial  safety=Conditional  reversible=true\n\
         administration/0002_expand-display-name  safety=Additive  reversible=true\n\
         administration/0003_backfill-display-name  safety=BackfillRequired  reversible=true\n\
         administration/0004_contract-legacy-name  safety=Destructive  reversible=true\n",
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
    assert_eq!(canonical.len(), 48_958);
    let digest = Sha256::digest(&canonical)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(
        digest,
        "a087c2de09f1fd186321f97c05f43911fa99f1af3fc29383cc8f6536116f9b7f",
    );
}

#[test]
fn generated_rust_public_facade_observes_the_administration_catalog() {
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
            "type-bridge = { version = \"=2.2.6\", default-features = false }",
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
    let target = support::cargo_target();
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
        "a34672f796977f0b3e01e16540c1b33957c405ad76034f9e05849d4d35be3a59",
    );
    assert_eq!(
        fields[2],
        "administration/0001_initial,administration/0002_expand-display-name,administration/0003_backfill-display-name,administration/0004_contract-legacy-name",
    );
    assert_eq!(
        fields[3],
        "administration/0004_contract-legacy-name,administration/0003_backfill-display-name,administration/0002_expand-display-name,administration/0001_initial",
    );
    assert_eq!(fields[4], "1");
}

#[test]
fn per_run_copy_on_a_newer_profile_bridges_the_committed_history() {
    let committed = fs::read(fixture().join("typebridge.yaml")).expect("manifest reads");
    let temporary = tempfile::tempdir().expect("isolated workspace creates");
    support::prepare_administration_workspace_for(temporary.path(), "typedb-3.13.6/v1");
    assert_eq!(
        fs::read(fixture().join("typebridge.yaml")).expect("manifest reads"),
        committed,
        "the committed workspace stays byte-identical",
    );
    let plan = Command::new(env!("CARGO_BIN_EXE_type-bridge"))
        .current_dir(temporary.path())
        .args(["migration", "plan"])
        .output()
        .expect("migration plan runs");
    assert!(
        plan.status.success(),
        "migration plan failed: {}",
        String::from_utf8_lossy(&plan.stderr),
    );
    let stdout = String::from_utf8(plan.stdout).expect("plan output is UTF-8");
    assert!(
        stdout.ends_with(
            "administration/0004_contract-legacy-name  safety=Destructive  reversible=true\n\
             administration/0005_profile_upgrade_typedb_3_13_6  safety=FormalOnly  reversible=false\n"
        ),
        "{stdout}"
    );
}
