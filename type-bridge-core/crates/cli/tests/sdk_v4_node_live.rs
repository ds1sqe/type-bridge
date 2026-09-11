//! Exact-live generated Node Sdk V4 report evidence.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const CONSUMER: &str = include_str!("sdk_v4_node_live/consumer.mjs");

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/contracts/sdk_conformance/sdk-v4/workspace")
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("destination creates");
    for entry in fs::read_dir(source).expect("source reads") {
        let entry = entry.expect("source entry reads");
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("source type reads").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("source file copies");
        }
    }
}

fn source_identity(root: &Path, relative: &str) -> Value {
    let bytes = fs::read(root.join(relative)).expect("authority source reads");
    json!({"path": relative, "sha256": format!("{:x}", Sha256::digest(bytes))})
}

fn result(case: &str, capability: &str, proof: &str, observation: Value) -> Value {
    json!({"case_id": case, "capability_id": capability, "proof_kind": proof, "outcome": "passed", "observation": observation})
}

fn run(command: &mut Command, description: &str) {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{description} starts: {error}"));
    assert!(
        output.status.success(),
        "{description} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore = "requires an isolated exact TypeDB 3.12.3 server, Node, and npm"]
fn generated_node_emits_complete_v4_report_on_3_12_3() {
    let temporary = tempfile::tempdir().expect("isolated workspace creates");
    copy_tree(&fixture(), temporary.path());
    run(
        Command::new(env!("CARGO_BIN_EXE_type-bridge"))
            .current_dir(temporary.path())
            .args(["schema", "generate"]),
        "schema generation",
    );

    let root = repository_root();
    let node_package = root.join("type-bridge-core/crates/node");
    run(
        Command::new("npm")
            .current_dir(&node_package)
            .args(["run", "build"]),
        "in-tree Node package build",
    );
    let generated = temporary.path().join("generated/typescript");
    run(
        Command::new("npm")
            .current_dir(&generated)
            .args([
                "install",
                "--ignore-scripts",
                "--no-package-lock",
                "--no-audit",
                "--no-fund",
            ])
            .arg(&node_package),
        "generated package local dependency install",
    );
    run(
        Command::new("node")
            .arg(node_package.join("node_modules/typescript/bin/tsc"))
            .current_dir(&generated)
            .args(["-p", "tsconfig.json"]),
        "generated package build",
    );

    let consumer = generated.join("consumer.mjs");
    fs::write(&consumer, CONSUMER).expect("consumer writes");
    let database = format!("sdk_v4_node_{}", std::process::id());
    let output = Command::new("node")
        .current_dir(&generated)
        .env("TYPE_BRIDGE_SDK_V4_DATABASE", database)
        .envs(std::env::vars().filter(|(name, _)| name.starts_with("TYPEDB_")))
        .arg(&consumer)
        .output()
        .expect("generated Node producer runs");
    assert!(
        output.status.success(),
        "producer failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let observation: Value = serde_json::from_slice(&output.stdout).expect("producer emits JSON");
    let report = json!({
        "format": "typebridge.sdk-conformance-report/v4",
        "binding": "node",
        "manifest": source_identity(&root, "tests/contracts/sdk_conformance/manifest-v1.json"),
        "catalog": source_identity(&root, "tests/contracts/sdk_conformance/sdk-v4/catalog-v4.json"),
        "journey": source_identity(&root, "tests/contracts/sdk_conformance/sdk-v4/journey-v4.json"),
        "server_version": "3.12.3",
        "results": [
            result("sdk.runtime.database-administration", "runtime.database-administration", "direct_runtime", observation["administration"].clone()),
            result("sdk.migration.rollback", "migration.reverse-cli", "direct_runtime", observation["rollback"].clone()),
            result("sdk.migration.backfill", "migration.binding-neutral-backfill", "direct_runtime", observation["backfill"].clone()),
            result("sdk.migration.runtime-facade", "migration.sdk-runtime-facade", "direct_runtime", observation["runtime_facade"].clone()),
            result("sdk.runtime.cancellation", "runtime.cancellation", "direct_runtime", observation["cancellation"].clone()),
            result("sdk.runtime.timeout-resource-limits", "runtime.timeout-and-resource-limits", "direct_runtime", observation["resource_limits"].clone()),
            result("sdk.diagnostic.all-workflows", "diagnostic.all-workflows-structured", "diagnostic", observation["diagnostic"].clone()),
            result("sdk.runtime.explicit-close", "runtime.explicit-close", "lifecycle", observation["lifecycle"].clone()),
        ],
        "cleanup": observation["cleanup"].clone(),
    });
    let report_path = temporary.path().join("node-sdk-v4-report.json");
    fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    run(
        Command::new("python3").args([
            "-c",
            "import importlib.util,json,pathlib,sys; root=pathlib.Path(sys.argv[1]); spec=importlib.util.spec_from_file_location('v4',root/'scripts/ci/compare_sdk_conformance_v4.py'); mod=importlib.util.module_from_spec(spec); sys.modules[spec.name]=mod; spec.loader.exec_module(mod); report=json.loads(pathlib.Path(sys.argv[2]).read_text()); contracts=mod.load_contracts(root); mod._validate_source_identity(report['manifest'],mod.MANIFEST_RELATIVE,root,'manifest'); mod._validate_source_identity(report['catalog'],mod.CATALOG_RELATIVE,root,'catalog'); mod._validate_source_identity(report['journey'],mod.JOURNEY_RELATIVE,root,'journey'); mod._validate_report(report,contracts.observation_refs)",
            root.to_str().unwrap(),
            report_path.to_str().unwrap(),
        ]),
        "V4 comparator",
    );
    assert_eq!(report["results"][2]["observation"]["forward_changed"], 2);
    assert_eq!(
        report["results"][3]["observation"]["catalog_fingerprint"],
        "efa3249f25fb2decaa0fef24c99d3a9884942d2e7926b3545955eb21eee7accb"
    );
    if let Some(directory) = std::env::var_os("TYPE_BRIDGE_SDK_V4_REPORT_DIR") {
        fs::copy(
            &report_path,
            PathBuf::from(directory).join("node-sdk-v4-report.json"),
        )
        .expect("validated Node report publishes to the fan-in directory");
    }
}
