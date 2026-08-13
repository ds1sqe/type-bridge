use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, RuntimeProjection};
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
use type_bridge_schema_codegen::{GeneratedPackage, RustEmitter};

mod support;

const PRODUCER: &str = include_str!("rust_acceptance/phase2_live.rs");
const OUTPUT_ENV: &str = "TYPE_BRIDGE_PHASE2_LIVE_REPORT";
const ADDRESS_ENV: &str = "TYPE_BRIDGE_PHASE2_LIVE_ADDRESS";
const DATABASE_ENV: &str = "TYPE_BRIDGE_PHASE2_LIVE_DATABASE";
const HTTP_PORT_ENV: &str = "TYPE_BRIDGE_PHASE2_LIVE_HTTP_PORT";
const ROOT_ENV: &str = "TYPE_BRIDGE_PHASE2_REPOSITORY_ROOT";
const MAX_PRODUCER_BYTES: usize = 1024 * 1024;
const FORBIDDEN_PRODUCER_MARKERS: [&str; 5] = [
    "compare_phase2_projection_live",
    "compare_phase2_projection_parity",
    "expected_observations",
    "expected_report",
    "load_contract",
];
static STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static CARGO_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Stage(PathBuf);

impl Stage {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time follows the Unix epoch")
            .as_nanos();
        let sequence = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "type-bridge-rust-phase2-live-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("Rust Phase-2 live stage is created");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("schema-codegen crate has the crates directory")
        .parent()
        .expect("crates directory has the core workspace")
        .parent()
        .expect("core workspace has the repository")
        .canonicalize()
        .expect("repository root canonicalizes")
}

fn project_from_source(source: &str) -> RuntimeProjection {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("rust-phase2-live.yaml").expect("static document ID is canonical"),
        source,
    )])
    .expect("Phase-2 live schema parses");
    let declared = normalize_documents(&documents).expect("Phase-2 live schema normalizes");
    let resolved = resolve(
        &declared,
        &SemanticProfileId::new("typedb-3.12.1/v1").expect("static profile is canonical"),
    )
    .expect("Phase-2 live schema resolves");
    let emitter = RustEmitter::new();
    let resources = emitter
        .code_resources_for(&resolved)
        .expect("Rust code resources resolve");
    project(
        &resolved,
        BindingTarget::Rust,
        &ProjectionConfig::rust(),
        &emitter.generator_handlers_for(&resolved),
        &resources,
    )
    .expect("Phase-2 live Rust projection builds")
}

fn emit_from_source(source: &str) -> GeneratedPackage {
    RustEmitter::new()
        .emit(&project_from_source(source), &support::authority(source))
        .expect("Phase-2 live Rust package emits")
}

fn write_package(package: &GeneratedPackage, root: &Path) {
    for (relative, bytes) in package.files() {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("generated file has a parent"))
            .expect("generated parent directory is created");
        fs::write(path, bytes).expect("generated file is written");
    }
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
name = "rust-phase2-projection-live"
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
}

fn cargo_target() -> PathBuf {
    env::var_os("ACCEPTANCE_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("schema-codegen crate has the crates directory")
                .parent()
                .expect("crates directory has the core workspace")
                .join("target/tmp_acceptance_target")
        })
}

fn cargo(arguments: &[&str]) -> Output {
    let _guard = CARGO_MUTEX.lock().expect("acceptance cargo mutex locks");
    Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(arguments)
        .env("CARGO_TARGET_DIR", cargo_target())
        .output()
        .expect("cargo command starts")
}

fn run_live_consumer(manifest: &Path, report: &Path) -> Output {
    let _guard = CARGO_MUTEX.lock().expect("acceptance cargo mutex locks");
    Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args([
            "run",
            "--offline",
            "--quiet",
            "--manifest-path",
            manifest.to_str().expect("consumer manifest is UTF-8"),
        ])
        .env("CARGO_TARGET_DIR", cargo_target())
        .env(OUTPUT_ENV, report)
        .env(ROOT_ENV, repository())
        .output()
        .expect("live consumer starts")
}

fn workforce_source() -> String {
    fs::read_to_string(
        repository().join("tests/contracts/sdk_conformance/workforce-v3/schema-v3.yaml"),
    )
    .expect("Workforce V3 schema reads")
}

fn foreign_workforce_source() -> String {
    let source = workforce_source();
    let foreign = source.replacen(
        "member: { card: { min: 0, max: 2 }, doc: membership player }",
        "member: { card: { min: 0, max: 3 }, doc: membership player }",
        1,
    );
    assert_ne!(source, foreign, "foreign schema mutation is applied once");
    foreign
}

#[test]
fn rust_phase2_live_producer_is_bounded_and_expectation_independent() {
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
fn generated_rust_phase2_live_producer_compiles_and_rejects_foreign_shape_before_io() {
    let stage = Stage::new();
    let generated = stage.path().join("generated");
    let consumer = stage.path().join("consumer");
    write_package(&emit_from_source(&workforce_source()), &generated);
    write_consumer(&consumer);
    let manifest = consumer.join("Cargo.toml");
    let checked = cargo(&[
        "clippy",
        "--offline",
        "--quiet",
        "--manifest-path",
        manifest.to_str().expect("consumer manifest is UTF-8"),
        "--",
        "-D",
        "warnings",
    ]);
    assert!(
        checked.status.success(),
        "local live producer failed to compile\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&checked.stdout),
        String::from_utf8_lossy(&checked.stderr),
    );

    fs::remove_dir_all(&generated).expect("local generated package is removed");
    write_package(&emit_from_source(&foreign_workforce_source()), &generated);
    let rejected = cargo(&[
        "run",
        "--offline",
        "--quiet",
        "--manifest-path",
        manifest.to_str().expect("consumer manifest is UTF-8"),
    ]);
    assert!(
        !rejected.status.success(),
        "foreign package unexpectedly ran"
    );
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        stderr.contains("generated_package_mismatch")
            && stderr.contains("exact Workforce V3 semantic projection"),
        "foreign rejection was not the semantic package fence:\n{stderr}"
    );
}

#[test]
fn generated_rust_phase2_live_producer_runs_when_explicitly_configured() {
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
    write_package(&emit_from_source(&workforce_source()), &generated);
    write_consumer(&consumer);
    let manifest = consumer.join("Cargo.toml");
    let output = run_live_consumer(&manifest, &report);
    assert!(
        output.status.success(),
        "Rust Phase-2 live producer failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let metadata = fs::symlink_metadata(&report).expect("live report exists");
    assert!(metadata.is_file() && !metadata.file_type().is_symlink());
    assert!(metadata.len() <= 256 * 1024);
    let payload = fs::read(&report).expect("live report reads");
    assert_eq!(payload.last(), Some(&b'\n'));

    let duplicate = run_live_consumer(&manifest, &report);
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

    let comparator = repository().join("scripts/ci/compare_phase2_projection_live.py");
    let producer_path = repository()
        .join("type-bridge-core/crates/schema-codegen/tests/rust_acceptance/phase2_live.rs");
    let verified = Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util,pathlib,sys; p=pathlib.Path(sys.argv[1]); s=importlib.util.spec_from_file_location('phase2_live_contract',p); m=importlib.util.module_from_spec(s); sys.modules[s.name]=m; s.loader.exec_module(m); m.validate_producer_source(pathlib.Path(sys.argv[2]).read_bytes()); binding,_=m._load_report(pathlib.Path(sys.argv[3]),m.load_contract()); assert binding=='rust'",
        )
        .arg(comparator)
        .arg(producer_path)
        .arg(&report)
        .output()
        .expect("live report verifier starts");
    assert!(
        verified.status.success(),
        "Rust Phase-2 live report failed exact validation:\n{}\nreport:\n{}",
        String::from_utf8_lossy(&verified.stderr),
        String::from_utf8_lossy(&payload),
    );
}
