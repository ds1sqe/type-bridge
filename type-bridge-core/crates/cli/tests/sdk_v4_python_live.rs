//! Exact-live generated Python Sdk V4 report evidence.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const CONSUMER: &str = include_str!("sdk_v4_python_live/consumer.py");

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

#[test]
#[ignore = "requires an isolated exact TypeDB 3.12.3 server and maturin"]
fn generated_python_emits_complete_v4_report_on_3_12_3() {
    let temporary = tempfile::tempdir().expect("isolated workspace creates");
    copy_tree(&fixture(), temporary.path());
    let generation = Command::new(env!("CARGO_BIN_EXE_type-bridge"))
        .current_dir(temporary.path())
        .args(["schema", "generate"])
        .output()
        .expect("schema generation runs");
    assert!(
        generation.status.success(),
        "{}",
        String::from_utf8_lossy(&generation.stderr)
    );
    fs::rename(
        temporary.path().join("generated/python"),
        temporary.path().join("sdk_v4_generated"),
    )
    .expect("generated Python package receives an importable nominal name");

    let environment = temporary.path().join("venv");
    let venv = Command::new("python3")
        .args(["-m", "venv"])
        .arg(&environment)
        .output()
        .expect("isolated Python environment creates");
    assert!(
        venv.status.success(),
        "{}",
        String::from_utf8_lossy(&venv.stderr)
    );
    let python = environment.join("bin/python");
    let dependencies = Command::new(&python)
        .args([
            "-m",
            "pip",
            "install",
            "--quiet",
            "pydantic>=2.12.4",
            "isodate==0.7.2",
            "jinja2>=3.1.0",
            "typer>=0.15.0",
            "typing-extensions>=4.12",
        ])
        .output()
        .expect("Python facade dependencies install");
    assert!(
        dependencies.status.success(),
        "{}",
        String::from_utf8_lossy(&dependencies.stderr)
    );
    let bin = environment.join("bin");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let root = repository_root().join("type-bridge-core");
    let installation = Command::new("maturin")
        .current_dir(&root)
        .env("VIRTUAL_ENV", &environment)
        .env("PATH", path)
        .env("PYO3_PYTHON", &python)
        .args(["develop", "--quiet", "--manifest-path"])
        .arg(root.join("crates/python/Cargo.toml"))
        .args(["--features", "pyo3/extension-module"])
        .output()
        .expect("in-tree Python extension installs");
    assert!(
        installation.status.success(),
        "{}",
        String::from_utf8_lossy(&installation.stderr)
    );

    let consumer = temporary.path().join("consumer.py");
    fs::write(&consumer, CONSUMER).expect("consumer writes");
    let database = format!("sdk_v4_python_{}", std::process::id());
    let python_path = format!(
        "{}:{}",
        temporary.path().display(),
        repository_root().display()
    );
    let output = Command::new(&python)
        .current_dir(temporary.path())
        .env("PYTHONPATH", python_path)
        .env("TYPE_BRIDGE_SDK_V4_DATABASE", database)
        .envs(std::env::vars().filter(|(name, _)| name.starts_with("TYPEDB_")))
        .arg(&consumer)
        .output()
        .expect("generated Python producer runs");
    assert!(
        output.status.success(),
        "producer failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let observation: Value = serde_json::from_slice(&output.stdout).expect("producer emits JSON");
    let root = repository_root();
    let report = json!({
        "format": "typebridge.sdk-conformance-report/v4",
        "binding": "python",
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
    let report_path = temporary.path().join("python-sdk-v4-report.json");
    fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    let validation = Command::new(&python)
        .args([
            "-c",
            "import importlib.util,json,pathlib,sys; root=pathlib.Path(sys.argv[1]); spec=importlib.util.spec_from_file_location('v4',root/'scripts/ci/compare_sdk_conformance_v4.py'); mod=importlib.util.module_from_spec(spec); sys.modules[spec.name]=mod; spec.loader.exec_module(mod); report=json.loads(pathlib.Path(sys.argv[2]).read_text()); contracts=mod.load_contracts(root); mod._validate_source_identity(report['manifest'],mod.MANIFEST_RELATIVE,root,'manifest'); mod._validate_source_identity(report['catalog'],mod.CATALOG_RELATIVE,root,'catalog'); mod._validate_source_identity(report['journey'],mod.JOURNEY_RELATIVE,root,'journey'); mod._validate_report(report,contracts.observation_refs)",
            root.to_str().unwrap(),
            report_path.to_str().unwrap(),
        ])
        .output()
        .expect("V4 comparator runs");
    assert!(
        validation.status.success(),
        "{}",
        String::from_utf8_lossy(&validation.stderr)
    );
    assert_eq!(report["results"][2]["observation"]["forward_changed"], 2);
    assert_eq!(
        report["results"][3]["observation"]["catalog_fingerprint"],
        "efa3249f25fb2decaa0fef24c99d3a9884942d2e7926b3545955eb21eee7accb"
    );
    if let Some(directory) = std::env::var_os("TYPE_BRIDGE_SDK_V4_REPORT_DIR") {
        fs::copy(
            &report_path,
            PathBuf::from(directory).join("python-sdk-v4-report.json"),
        )
        .expect("validated Python report publishes to the fan-in directory");
    }
}
