//! Exact-live generated Rust administration/migration evidence for Workforce V4.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const CONSUMER: &str = include_str!("workforce_v4_rust_live/consumer.rs");

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/contracts/sdk_conformance/workforce-v4/workspace")
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("destination creates");
    for entry in fs::read_dir(source).expect("source directory reads") {
        let entry = entry.expect("source entry reads");
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("source type reads").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("source file copies");
        }
    }
}

#[test]
#[ignore = "requires an isolated exact TypeDB 3.12.3 server"]
fn generated_rust_observes_v4_administration_controls_and_lifecycle_on_3_12_3() {
    let temporary = tempfile::tempdir().expect("isolated workspace creates");
    copy_tree(&fixture(), temporary.path());
    let generation = Command::new(env!("CARGO_BIN_EXE_type-bridge"))
        .current_dir(temporary.path())
        .args(["schema", "generate"])
        .output()
        .expect("schema generation runs");
    assert!(
        generation.status.success(),
        "generation failed: {}",
        String::from_utf8_lossy(&generation.stderr),
    );

    let generated = temporary.path().join("generated/rust");
    let generated_manifest = generated.join("Cargo.toml");
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../rust")
        .canonicalize()
        .expect("in-tree Rust runtime resolves");
    let manifest = fs::read_to_string(&generated_manifest).expect("generated manifest reads");
    fs::write(
        &generated_manifest,
        manifest.replace(
            "type-bridge = { version = \"=2.1.0\", default-features = false }",
            &format!(
                "type-bridge = {{ path = {:?}, default-features = false, features = [\"typedb\"] }}",
                runtime
            ),
        ),
    )
    .expect("generated package binds the exact in-tree runtime");

    let consumer = temporary.path().join("rust-live-consumer");
    fs::create_dir_all(consumer.join("src")).expect("consumer source directory creates");
    fs::write(consumer.join("src/main.rs"), CONSUMER).expect("consumer source writes");
    fs::write(
        consumer.join("Cargo.toml"),
        format!(
            "[package]\nname = \"workforce-v4-rust-live\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\
             [dependencies]\ntype-bridge-generated-schema = {{ path = {:?} }}\ntype-bridge = {{ path = {:?}, features = [\"typedb\"] }}\nserde_json = \"1\"\ntokio = {{ version = \"1\", features = [\"macros\", \"rt-multi-thread\"] }}\n[workspace]\n",
            generated,
            runtime,
        ),
    )
    .expect("consumer manifest writes");
    let database = format!("workforce_v4_rust_{}", std::process::id());
    let output = Command::new("cargo")
        .current_dir(&consumer)
        .env(
            "CARGO_TARGET_DIR",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"),
        )
        .env("TYPE_BRIDGE_WORKFORCE_V4_DATABASE", database)
        .envs(std::env::vars().filter(|(name, _)| name.starts_with("TYPEDB_")))
        .args(["run", "--quiet"])
        .output()
        .expect("generated Rust live producer runs");
    assert!(
        output.status.success(),
        "producer failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let observation: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("producer emits one JSON observation");
    assert_eq!(observation["administration"]["create"], "created");
    assert_eq!(
        observation["cancellation"]["code"],
        "migration_execution_cancelled"
    );
    assert_eq!(
        observation["resource_limits"]["code"], "migration_execution_group_limit",
        "{observation}"
    );
    assert_eq!(observation["cleanup"]["managed_database_absent"], true);
    assert_eq!(observation["cleanup"]["journal_database_absent"], true);
}
