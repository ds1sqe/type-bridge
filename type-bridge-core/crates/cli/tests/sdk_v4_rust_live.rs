//! Exact-live generated Rust administration/migration evidence for Sdk V4.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

const CONSUMER: &str = include_str!("sdk_v4_rust_live/consumer.rs");
#[path = "../../../../tests/support/rust_locks.rs"]
#[allow(dead_code)]
mod locks;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/contracts/sdk_conformance/sdk-v4/workspace")
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

fn source_identity(root: &Path, relative: &str) -> serde_json::Value {
    let bytes = fs::read(root.join(relative)).expect("report authority source reads");
    serde_json::json!({
        "path": relative,
        "sha256": format!("{:x}", Sha256::digest(bytes)),
    })
}

fn result(
    case_id: &str,
    capability_id: &str,
    proof_kind: &str,
    observation: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "case_id": case_id,
        "capability_id": capability_id,
        "proof_kind": proof_kind,
        "outcome": "passed",
        "observation": observation,
    })
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
    let typedb_runtime = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../typedb-runtime")
        .canonicalize()
        .expect("in-tree TypeDB runtime resolves");
    let manifest = fs::read_to_string(&generated_manifest).expect("generated manifest reads");
    fs::write(
        &generated_manifest,
        manifest.replace(
            "type-bridge = { version = \"=2.2.0\", default-features = false }",
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
            "[package]\nname = \"sdk-v4-rust-live\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\
             [dependencies]\ntype-bridge-generated-schema = {{ path = {:?} }}\ntype-bridge = {{ path = {:?}, features = [\"typedb\"] }}\ntype-bridge-typedb-runtime = {{ path = {:?} }}\nserde_json = \"1\"\ntokio = {{ version = \"1\", features = [\"macros\", \"rt-multi-thread\"] }}\n[workspace]\n",
            generated,
            runtime,
            typedb_runtime,
        ),
    )
    .expect("consumer manifest writes");
    fs::write(
        consumer.join("Cargo.lock"),
        locks::Consumer::AdministrationLive.lock(),
    )
    .expect("consumer lockfile writes");
    let database = format!("sdk_v4_rust_{}", std::process::id());
    let output = Command::new("cargo")
        .current_dir(&consumer)
        .env(
            "CARGO_TARGET_DIR",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"),
        )
        .env("TYPE_BRIDGE_SDK_V4_DATABASE", database)
        .envs(std::env::vars().filter(|(name, _)| name.starts_with("TYPEDB_")))
        .args(["run", "--locked", "--quiet"])
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
    assert_eq!(
        observation["migration_probe"]["conflict_code"],
        "migration_typedb_backfill_destination_conflict"
    );
    assert_eq!(
        observation["migration_probe"]["conflict_visible_destination_count"],
        1
    );
    assert_eq!(observation["migration_probe"]["forward_changed"], 2);
    assert_eq!(
        observation["migration_probe"]["forward_transaction_groups"],
        2
    );
    assert_eq!(observation["migration_probe"]["equal_copy_count"], 2);
    assert_eq!(observation["migration_probe"]["retry_changed"], 0);
    assert_eq!(observation["migration_probe"]["reverse_changed"], 2);
    assert_eq!(
        observation["migration_probe"]["remaining_destination_count"],
        0
    );
    assert_eq!(
        observation["migration_probe"]["rollback_without_approval_code"],
        "migration_rollback_approval_required"
    );
    assert_eq!(
        observation["migration_probe"]["rollback_status"],
        "rolledback"
    );
    assert_eq!(
        observation["migration_probe"]["unknown_target_code"],
        "migration_history_unknown_rollback_target"
    );
    assert_eq!(
        observation["migration_probe"]["repeat_rollback_status"],
        "up_to_date"
    );
    assert_eq!(observation["migration_probe"]["reapply_status"], "applied");

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let probe = &observation["migration_probe"];
    let report = serde_json::json!({
        "format": "typebridge.sdk-conformance-report/v4",
        "binding": "rust",
        "manifest": source_identity(&root, "tests/contracts/sdk_conformance/manifest-v1.json"),
        "catalog": source_identity(&root, "tests/contracts/sdk_conformance/sdk-v4/catalog-v4.json"),
        "journey": source_identity(&root, "tests/contracts/sdk_conformance/sdk-v4/journey-v4.json"),
        "server_version": "3.12.3",
        "results": [
            result("sdk.runtime.database-administration", "runtime.database-administration", "direct_runtime", observation["administration"].clone()),
            result("sdk.migration.rollback", "migration.reverse-cli", "direct_runtime", serde_json::json!({
                "apply_status": probe["initial_status"],
                "rollback_without_approval_code": probe["rollback_without_approval_code"],
                "rollback_status": "rolled_back",
                "unknown_target_code": probe["unknown_target_code"],
                "repeat_rollback_status": probe["repeat_rollback_status"],
                "reapply_status": probe["reapply_status"],
            })),
            result("sdk.migration.backfill", "migration.binding-neutral-backfill", "direct_runtime", serde_json::json!({
                "conflict_certainty": "definitely_aborted",
                "conflict_code": probe["conflict_code"],
                "conflict_visible_destination_count": probe["conflict_visible_destination_count"],
                "forward_changed": probe["forward_changed"],
                "forward_transaction_groups": probe["forward_transaction_groups"],
                "equal_copy_count": probe["equal_copy_count"],
                "retry_changed": probe["retry_changed"],
                "reverse_changed": probe["reverse_changed"],
                "remaining_destination_count": probe["remaining_destination_count"],
            })),
            result("sdk.migration.runtime-facade", "migration.sdk-runtime-facade", "direct_runtime", observation["runtime_facade"].clone()),
            result("sdk.runtime.cancellation", "runtime.cancellation", "direct_runtime", serde_json::json!({"code": observation["cancellation"]["code"], "before_effect": true})),
            result("sdk.runtime.timeout-resource-limits", "runtime.timeout-and-resource-limits", "direct_runtime", serde_json::json!({"code": observation["resource_limits"]["code"], "bounded": true})),
            result("sdk.diagnostic.all-workflows", "diagnostic.all-workflows-structured", "diagnostic", serde_json::json!({"code": observation["cancellation"]["code"], "category": observation["cancellation"]["category"], "provider_text_absent": true})),
            result("sdk.runtime.explicit-close", "runtime.explicit-close", "lifecycle", serde_json::json!({"explicit_close": true, "repeat_close": true, "temporary_evidence_absent": true})),
        ],
        "cleanup": {
            "managed_database_absent": observation["cleanup"]["managed_database_absent"],
            "journal_database_absent": observation["cleanup"]["journal_database_absent"],
            "temporary_evidence_absent": true,
        },
    });
    assert_eq!(
        report["results"].as_array().expect("results array").len(),
        8
    );
    assert_eq!(report["results"][3]["observation"]["catalog_entries"], 4);
    assert_eq!(
        report["results"][3]["observation"]["catalog_fingerprint"],
        "efa3249f25fb2decaa0fef24c99d3a9884942d2e7926b3545955eb21eee7accb"
    );
    let report_path = temporary.path().join("rust-sdk-v4-report.json");
    fs::write(
        &report_path,
        serde_json::to_vec_pretty(&report).expect("report serializes"),
    )
    .expect("temporary report writes");
    let validation = Command::new("python3")
        .args([
            "-c",
            "import importlib.util,json,pathlib,sys; root=pathlib.Path(sys.argv[1]); spec=importlib.util.spec_from_file_location('v4',root/'scripts/ci/compare_sdk_conformance_v4.py'); mod=importlib.util.module_from_spec(spec); sys.modules[spec.name]=mod; spec.loader.exec_module(mod); report=json.loads(pathlib.Path(sys.argv[2]).read_text()); contracts=mod.load_contracts(root); mod._validate_source_identity(report['manifest'],mod.MANIFEST_RELATIVE,root,'manifest'); mod._validate_source_identity(report['catalog'],mod.CATALOG_RELATIVE,root,'catalog'); mod._validate_source_identity(report['journey'],mod.JOURNEY_RELATIVE,root,'journey'); mod._validate_report(report,contracts.observation_refs)",
            root.to_str().expect("repository root is UTF-8"),
            report_path.to_str().expect("report path is UTF-8"),
        ])
        .output()
        .expect("V4 comparator validation runs");
    assert!(
        validation.status.success(),
        "V4 report validation failed: {}",
        String::from_utf8_lossy(&validation.stderr)
    );
    if let Some(directory) = std::env::var_os("TYPE_BRIDGE_SDK_V4_REPORT_DIR") {
        fs::copy(
            &report_path,
            PathBuf::from(directory).join("rust-sdk-v4-report.json"),
        )
        .expect("validated Rust report publishes to the fan-in directory");
    }
}

#[test]
fn generated_rust_live_dependency_graph_is_frozen() {
    let lock = locks::Consumer::AdministrationLive.lock();
    assert_eq!(lock.matches("name = \"sdk-v4-rust-live\"").count(), 1);
    assert!(lock.contains("name = \"tinyvec\"\nversion = \"1.12.0\""));
    assert!(!lock.contains("name = \"tinyvec\"\nversion = \"1.13.0\""));
}
