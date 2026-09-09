use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use type_bridge_contract::projection::RuntimeProjection;
use type_bridge_schema_codegen::{GeneratedPackage, RustEmitter};

mod support;
use support::{Stage, cargo_with_env as cargo, repository_root as repository, write_package};

const PRODUCER: &str = include_str!("rust_acceptance/projected_live.rs");
const OUTPUT_ENV: &str = "TYPE_BRIDGE_PROJECTED_LIVE_REPORT";
const ADDRESS_ENV: &str = "TYPE_BRIDGE_PROJECTED_LIVE_ADDRESS";
const DATABASE_ENV: &str = "TYPE_BRIDGE_PROJECTED_LIVE_DATABASE";
const HTTP_PORT_ENV: &str = "TYPE_BRIDGE_PROJECTED_LIVE_HTTP_PORT";
const ROOT_ENV: &str = "TYPE_BRIDGE_PROJECTED_REPOSITORY_ROOT";
const MAX_PRODUCER_BYTES: usize = 1024 * 1024;
const FORBIDDEN_PRODUCER_MARKERS: [&str; 5] = [
    "compare_projected_live",
    "compare_projected_parity",
    "expected_observations",
    "expected_report",
    "load_contract",
];

fn project_from_source(source: &str) -> RuntimeProjection {
    support::rust_projection(source, "rust-projected-live.yaml")
}

fn emit_from_source(source: &str) -> GeneratedPackage {
    RustEmitter::new()
        .emit(&project_from_source(source), &support::authority(source))
        .expect("Projected live Rust package emits")
}

fn write_consumer(root: &Path) {
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("schema-codegen crate has the crates directory")
        .to_owned();
    let rust_path = crates.join("rust").to_string_lossy().replace('\\', "\\\\");
    let orm_path = crates.join("orm").to_string_lossy().replace('\\', "\\\\");
    fs::create_dir_all(root.join("src")).expect("consumer source directory is created");
    fs::write(
        root.join("Cargo.toml"),
        format!(
            r#"[package]
name = "rust-projected-projection-live"
version = "0.0.0"
edition = "2024"

[dependencies]
generated = {{ package = "type-bridge-generated-schema", path = "../generated" }}
serde_json = "1"
sha2 = "0.10"
tokio = {{ version = "1", features = ["macros", "rt-multi-thread"] }}
type-bridge = {{ path = "{rust_path}", default-features = false, features = ["band8", "band9"] }}
type-bridge-orm = {{ path = "{orm_path}", default-features = false, features = ["band8", "band9"] }}

[patch.crates-io]
type-bridge = {{ path = "{rust_path}" }}
type-bridge-orm = {{ path = "{orm_path}" }}

[workspace]
"#
        ),
    )
    .expect("consumer manifest is written");
    fs::write(root.join("src/main.rs"), PRODUCER).expect("consumer source is written");
    fs::write(
        root.join("Cargo.lock"),
        support::locks::Consumer::ProjectedLive.lock(),
    )
    .expect("consumer lockfile is staged");
}

fn sdk_source() -> String {
    fs::read_to_string(repository().join("tests/contracts/sdk_conformance/sdk-v3/schema-v3.yaml"))
        .expect("Sdk V3 schema reads")
}

fn foreign_sdk_source() -> String {
    let source = sdk_source();
    let foreign = source.replacen(
        "member: { card: { min: 0, max: 2 }, doc: membership player }",
        "member: { card: { min: 0, max: 3 }, doc: membership player }",
        1,
    );
    assert_ne!(source, foreign, "foreign schema mutation is applied once");
    foreign
}

#[test]
fn rust_projected_live_producer_is_bounded_and_expectation_independent() {
    assert!(PRODUCER.len() <= MAX_PRODUCER_BYTES);
    for marker in FORBIDDEN_PRODUCER_MARKERS {
        assert!(
            !PRODUCER.contains(marker),
            "producer contains forbidden comparator marker {marker}"
        );
    }
    for required in [
        OUTPUT_ENV,
        ADDRESS_ENV,
        DATABASE_ENV,
        HTTP_PORT_ENV,
        ROOT_ENV,
    ] {
        assert!(PRODUCER.contains(required), "producer omits {required}");
    }
    assert!(PRODUCER.contains("GeneratedDatabase<AppSchema>"));
    assert!(PRODUCER.contains("entities::<Person>()"));
    assert!(PRODUCER.contains("relations::<Interaction>()"));
    assert!(PRODUCER.contains("relations::<PlainActivity>()"));
    assert!(PRODUCER.contains("relations::<Container>()"));
    assert!(PRODUCER.contains("TxType::Schema"));
    assert_eq!(PRODUCER.matches("execute_raw(").count(), 1);
    assert!(!PRODUCER.contains("Aliases::new"));
    assert!(!PRODUCER.contains("NetworkLinkCreate"));
}

#[test]
fn generated_rust_projected_live_producer_compiles_and_rejects_foreign_shape_before_io() {
    let stage = Stage::new();
    let generated = stage.path().join("generated");
    let consumer = stage.path().join("consumer");
    write_package(&emit_from_source(&sdk_source()), &generated);
    write_consumer(&consumer);
    let manifest = consumer.join("Cargo.toml");
    let checked = cargo(
        &[
            "clippy",
            "--locked",
            "--offline",
            "--quiet",
            "--manifest-path",
            manifest.to_str().expect("consumer manifest is UTF-8"),
            "--",
            "-D",
            "warnings",
        ],
        &[],
    );
    assert!(
        checked.status.success(),
        "local live producer failed to compile\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&checked.stdout),
        String::from_utf8_lossy(&checked.stderr),
    );

    fs::remove_dir_all(&generated).expect("local generated package is removed");
    write_package(&emit_from_source(&foreign_sdk_source()), &generated);
    let rejected = cargo(
        &[
            "run",
            "--locked",
            "--offline",
            "--quiet",
            "--manifest-path",
            manifest.to_str().expect("consumer manifest is UTF-8"),
        ],
        &[],
    );
    assert!(
        !rejected.status.success(),
        "foreign package unexpectedly ran"
    );
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        stderr.contains("generated_package_mismatch")
            && stderr.contains("exact Sdk V3 semantic projection"),
        "foreign rejection was not the semantic package fence:\n{stderr}"
    );
}

#[test]
fn rust_projected_live_dependency_graph_is_frozen() {
    let lock = support::locks::Consumer::ProjectedLive.lock();
    assert_eq!(
        lock.matches("name = \"rust-projected-projection-live\"")
            .count(),
        1
    );
    assert!(lock.contains("name = \"tinyvec\"\nversion = \"1.12.0\""));
    assert!(!lock.contains("name = \"tinyvec\"\nversion = \"1.13.0\""));
}

#[test]
fn generated_rust_projected_live_producer_runs_when_explicitly_configured() {
    let Some(report_value) = env::var_os(OUTPUT_ENV) else {
        return;
    };
    for required in [ADDRESS_ENV, DATABASE_ENV, HTTP_PORT_ENV] {
        assert!(
            env::var_os(required).is_some(),
            "live run requires {required}"
        );
    }
    let report = PathBuf::from(report_value);
    assert!(
        report.is_absolute(),
        "external report path must be absolute"
    );
    assert!(!report.exists(), "external report path must not exist");

    let stage = Stage::new();
    let generated = stage.path().join("generated");
    let consumer = stage.path().join("consumer");
    write_package(&emit_from_source(&sdk_source()), &generated);
    write_consumer(&consumer);
    let manifest = consumer.join("Cargo.toml");
    let output = cargo(
        &[
            "run",
            "--locked",
            "--offline",
            "--quiet",
            "--manifest-path",
            manifest.to_str().expect("consumer manifest is UTF-8"),
        ],
        &[
            (OUTPUT_ENV, report.as_os_str()),
            (ROOT_ENV, repository().as_os_str()),
        ],
    );
    assert!(
        output.status.success(),
        "Rust Projected live producer failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let metadata = fs::symlink_metadata(&report).expect("live report exists");
    assert!(metadata.is_file() && !metadata.file_type().is_symlink());
    assert!(metadata.len() <= 256 * 1024);
    let payload = fs::read(&report).expect("live report reads");
    assert_eq!(payload.last(), Some(&b'\n'));

    let duplicate = cargo(
        &[
            "run",
            "--locked",
            "--offline",
            "--quiet",
            "--manifest-path",
            manifest.to_str().expect("consumer manifest is UTF-8"),
        ],
        &[
            (OUTPUT_ENV, report.as_os_str()),
            (ROOT_ENV, repository().as_os_str()),
        ],
    );
    assert!(
        !duplicate.status.success(),
        "duplicate live producer unexpectedly replaced its report"
    );
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("output_exists"));
    assert_eq!(
        fs::read(&report).expect("live report remains readable"),
        payload,
        "failed create-new publication changed the live report"
    );

    let comparator = repository().join("scripts/ci/compare_projected_live.py");
    let producer_path = repository()
        .join("type-bridge-core/crates/schema-codegen/tests/rust_acceptance/projected_live.rs");
    let verified = Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util,pathlib,sys; p=pathlib.Path(sys.argv[1]); s=importlib.util.spec_from_file_location('projected_live_contract',p); m=importlib.util.module_from_spec(s); sys.modules[s.name]=m; s.loader.exec_module(m); m.validate_producer_source(pathlib.Path(sys.argv[2]).read_bytes()); binding,_=m._load_report(pathlib.Path(sys.argv[3]),m.load_contract()); assert binding=='rust'",
        )
        .arg(comparator)
        .arg(producer_path)
        .arg(&report)
        .output()
        .expect("live report verifier starts");
    assert!(
        verified.status.success(),
        "Rust Projected live report failed exact validation:\n{}\nreport:\n{}",
        String::from_utf8_lossy(&verified.stderr),
        String::from_utf8_lossy(&payload),
    );
}
