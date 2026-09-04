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

const PRODUCER: &str = include_str!("rust_acceptance/phase5_manager_live.rs");
const CONSUMER_LOCK: &[u8] = include_bytes!("rust_acceptance/phase5-manager-live-Cargo.lock");
const OUTPUT_ENV: &str = "TYPE_BRIDGE_PHASE5_MANAGER_LIVE_REPORT";
const ADDRESS_ENV: &str = "TYPE_BRIDGE_PHASE5_MANAGER_LIVE_ADDRESS";
const DATABASE_ENV: &str = "TYPE_BRIDGE_PHASE5_MANAGER_LIVE_DATABASE";
const HTTP_PORT_ENV: &str = "TYPE_BRIDGE_PHASE5_MANAGER_LIVE_HTTP_PORT";
const ROOT_ENV: &str = "TYPE_BRIDGE_PHASE5_MANAGER_REPOSITORY_ROOT";
const MAX_PRODUCER_BYTES: usize = 1024 * 1024;

static STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static CARGO_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Stage(PathBuf);

impl Stage {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time follows Unix epoch")
            .as_nanos();
        let sequence = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "typebridge-rust-phase5-manager-live-{}-{nonce}-{sequence}",
            std::process::id(),
        ));
        fs::create_dir(&path).expect("unique Rust Phase-5 stage creates");
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
        .expect("schema-codegen has crates parent")
        .parent()
        .expect("crates has core parent")
        .parent()
        .expect("core has repository parent")
        .canonicalize()
        .expect("repository root canonicalizes")
}

fn workforce_source() -> String {
    fs::read_to_string(
        repository().join("tests/contracts/sdk_conformance/workforce-v3/schema-v3.yaml"),
    )
    .expect("Workforce V3 schema reads")
}

fn foreign_workforce_source() -> String {
    let source = workforce_source();
    let needle = "      foo__bar: { card: { min: 0, max: 1 } }";
    assert_eq!(source.matches(needle).count(), 1, "foo__bar fact is unique");
    let foreign = source.replacen(needle, "      foo__bar: { card: { min: 0, max: 2 } }", 1);
    assert_ne!(source, foreign, "foreign foo__bar token metadata differs");
    foreign
}

fn project_from_source(source: &str, document: &str) -> RuntimeProjection {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new(document).expect("static document ID is canonical"),
        source,
    )])
    .expect("Phase-5 schema parses");
    let declared = normalize_documents(&documents).expect("Phase-5 schema normalizes");
    let resolved = resolve(
        &declared,
        &SemanticProfileId::new("typedb-3.12.1/v1").expect("profile is canonical"),
    )
    .expect("Phase-5 schema resolves");
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
    .expect("Phase-5 Rust projection builds")
}

fn emit_from_source(source: &str, document: &str) -> GeneratedPackage {
    RustEmitter::new()
        .emit(
            &project_from_source(source, document),
            &support::authority(source),
        )
        .expect("Phase-5 Rust package emits")
}

fn write_package(package: &GeneratedPackage, root: &Path) {
    for (relative, bytes) in package.files() {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("generated path has parent"))
            .expect("generated parent creates");
        fs::write(path, bytes).expect("generated file writes");
    }
}

fn write_foreign_package(package: &GeneratedPackage, root: &Path) {
    write_package(package, root);
    let manifest = root.join("Cargo.toml");
    let source = fs::read_to_string(&manifest).expect("foreign manifest reads");
    let needle = "name = \"type-bridge-generated-schema\"";
    assert_eq!(source.matches(needle).count(), 1, "package name is unique");
    fs::write(
        manifest,
        source.replacen(needle, "name = \"type-bridge-generated-schema-foreign\"", 1),
    )
    .expect("foreign package manifest receives an unambiguous Cargo name");
}

fn escaped_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "\\\\")
}

fn write_consumer(root: &Path) {
    let manifest_directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let crates = manifest_directory
        .parent()
        .expect("schema-codegen has crates parent");
    let rust_path = escaped_path(&crates.join("rust"));
    let orm_path = escaped_path(&crates.join("orm"));
    fs::create_dir_all(root.join("src")).expect("consumer source directory creates");
    fs::write(
        root.join("Cargo.toml"),
        format!(
            r#"[package]
name = "rust-phase5-manager-live"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
generated = {{ package = "type-bridge-generated-schema", path = "../generated" }}
generated-foreign = {{ package = "type-bridge-generated-schema-foreign", path = "../generated-foreign" }}
serde_json = "1"
sha2 = "0.10"
tokio = {{ version = "1", features = ["macros", "rt-multi-thread"] }}
type-bridge = {{ path = "{rust_path}", default-features = false, features = ["band8", "band9"] }}
type-bridge-orm = {{ path = "{orm_path}", default-features = false, features = ["band8", "band9"] }}

[patch.crates-io]
type-bridge = {{ path = "{rust_path}" }}
type-bridge-orm = {{ path = "{orm_path}" }}

[workspace]
"#,
        ),
    )
    .expect("consumer manifest writes");
    fs::write(root.join("src/main.rs"), PRODUCER).expect("consumer source writes");
    fs::write(root.join("Cargo.lock"), CONSUMER_LOCK).expect("consumer lockfile is staged");
}

fn cargo_target() -> PathBuf {
    env::var_os("ACCEPTANCE_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("schema-codegen has crates parent")
                .parent()
                .expect("crates has core parent")
                .join("target/tmp_acceptance_target")
        })
}

fn cargo(arguments: &[&str], environment: &[(&str, &Path)]) -> Output {
    let _guard = CARGO_MUTEX.lock().expect("acceptance cargo mutex locks");
    let mut command = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command
        .args(arguments)
        .env("CARGO_TARGET_DIR", cargo_target());
    for (name, value) in environment {
        command.env(name, value);
    }
    command.output().expect("cargo command starts")
}

fn staged_consumer(stage: &Stage) -> PathBuf {
    let generated = stage.path().join("generated");
    let foreign = stage.path().join("generated-foreign");
    let consumer = stage.path().join("consumer");
    write_package(
        &emit_from_source(&workforce_source(), "workforce-v3.yaml"),
        &generated,
    );
    write_foreign_package(
        &emit_from_source(
            &foreign_workforce_source(),
            "workforce-v3-foreign-foo-bar.yaml",
        ),
        &foreign,
    );
    write_consumer(&consumer);
    consumer.join("Cargo.toml")
}

#[test]
fn rust_phase5_manager_live_producer_is_bounded_and_expectation_independent() {
    assert!(PRODUCER.len() <= MAX_PRODUCER_BYTES);
    for required in [
        OUTPUT_ENV,
        ADDRESS_ENV,
        DATABASE_ENV,
        HTTP_PORT_ENV,
        ROOT_ENV,
    ] {
        assert!(PRODUCER.contains(required), "producer omits {required}");
    }
    for required in [
        "ProjectedManagerComparison",
        "PersonType::foo__bar",
        "sdk_category()",
        "read.entities::<Person>()",
        "read.query()",
        "count_by(person)",
    ] {
        assert!(PRODUCER.contains(required), "producer omits {required}");
    }
}

#[test]
fn generated_rust_phase5_manager_live_producer_compiles() {
    let stage = Stage::new();
    let manifest = staged_consumer(&stage);
    let checked = cargo(
        &[
            "clippy",
            "--locked",
            "--offline",
            "--quiet",
            "--manifest-path",
            manifest.to_str().expect("manifest path is UTF-8"),
            "--",
            "-D",
            "warnings",
        ],
        &[],
    );
    assert!(
        checked.status.success(),
        "Rust Phase-5 producer failed strict compile\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&checked.stdout),
        String::from_utf8_lossy(&checked.stderr),
    );
}

#[test]
fn generated_rust_phase5_manager_live_runs_when_explicitly_configured() {
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
    assert!(report.is_absolute(), "report path must be absolute");
    assert!(!report.exists(), "report destination must be absent");

    let stage = Stage::new();
    let manifest = staged_consumer(&stage);
    let repository = repository();
    let output = cargo(
        &[
            "run",
            "--locked",
            "--offline",
            "--quiet",
            "--manifest-path",
            manifest.to_str().expect("manifest path is UTF-8"),
        ],
        &[(OUTPUT_ENV, &report), (ROOT_ENV, &repository)],
    );
    assert!(
        output.status.success(),
        "Rust Phase-5 live producer failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let metadata = fs::symlink_metadata(&report).expect("live report exists");
    assert!(metadata.is_file() && !metadata.file_type().is_symlink());
    assert!(metadata.len() <= 256 * 1024);
    assert_eq!(fs::read(report).expect("report reads").last(), Some(&b'\n'));
}

#[test]
fn rust_phase5_manager_live_dependency_graph_is_frozen() {
    let lock = std::str::from_utf8(CONSUMER_LOCK).expect("consumer lockfile is UTF-8");
    assert_eq!(
        lock.matches("name = \"rust-phase5-manager-live\"").count(),
        1
    );
    assert!(lock.contains("name = \"tinyvec\"\nversion = \"1.12.0\""));
    assert!(!lock.contains("name = \"tinyvec\"\nversion = \"1.13.0\""));
}
