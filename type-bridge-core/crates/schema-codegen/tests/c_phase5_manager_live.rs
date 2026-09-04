#![cfg(unix)]

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::projection::{BindingTarget, CSymbolPrefix, ProjectionConfig};
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema::{
    BUILTIN_SCHEMA_CAPABILITY_IDS, ManagedDeltaContext, SchemaDocumentSet, build_schema_authority,
    normalize_documents, project, resolve,
};
use type_bridge_schema_codegen::{CEmitter, GeneratedPackage};

const SOURCE: &str =
    include_str!("../../../../tests/contracts/sdk_conformance/workforce-v3/schema-v3.yaml");
const PROVIDER: &str =
    include_str!("../../../../tests/contracts/sdk_conformance/workforce-v3/provider-3.12.1-v3.tql");
const CONSUMER: &str = include_str!("c_phase5_manager_live/consumer.c");
const WRONG_OWNER_COMPILE_FAIL: &str =
    include_str!("c_phase5_manager_live/wrong_owner_compile_fail.c");
const WRONG_SCALAR_COMPILE_FAIL: &str =
    include_str!("c_phase5_manager_live/wrong_scalar_compile_fail.c");
const SETUP: &str = include_str!("c_phase5_manager_live/setup.rs");
const SETUP_LOCK: &[u8] = include_bytes!("c_phase5_manager_live/setup-Cargo.lock");

const REPORT_FORMAT: &str = "typebridge.phase5-manager-filter-live-report/v1";
const SEMANTIC_PROFILE: &str = "typedb-3.12.1/v1";
const FACT_PREFIX: &str = "TYPE_BRIDGE_C_PHASE5_MANAGER_LIVE_FACT\t";
const REPORT_ENV: &str = "TYPE_BRIDGE_PHASE5_MANAGER_LIVE_REPORT";
const ADDRESS_ENV: &str = "TYPE_BRIDGE_PHASE5_MANAGER_LIVE_ADDRESS";
const HTTP_PORT_ENV: &str = "TYPE_BRIDGE_PHASE5_MANAGER_LIVE_HTTP_PORT";
const DATABASE_ENV: &str = "TYPE_BRIDGE_PHASE5_MANAGER_LIVE_DATABASE";
const DATABASE_CREATED_MARKER: &str = "Phase-5 C manager live database created";
const MAX_AUTHORITY_BYTES: u64 = 1024 * 1024;
const MAX_REPORT_BYTES: usize = 256 * 1024;
const MAX_PRODUCER_BYTES: usize = 1024 * 1024;
const AUTHORITIES: [(&str, &str, &str); 3] = [
    (
        "journey",
        "tests/contracts/sdk_conformance/workforce-v3/journey-v3.json",
        "912130753fe7938a38c054cff16e202b312551a6aa65265148628bfbd2abbbef",
    ),
    (
        "provider",
        "tests/contracts/sdk_conformance/workforce-v3/provider-3.12.1-v3.tql",
        "af61aaece22d666e19d2c1357f8ebc39b8cbcf4bff7b98a19a56daf171a1faba",
    ),
    (
        "schema",
        "tests/contracts/sdk_conformance/workforce-v3/schema-v3.yaml",
        "74b9161e3d8fd70a1f22c3a8b7fc70a823b18cb07973064e2e10ce0940f76f1f",
    ),
];

static STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Stage(PathBuf);

impl Stage {
    fn new() -> Self {
        let sequence = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "typebridge-c-phase5-manager-live-{}-{sequence}",
            std::process::id(),
        ));
        fs::create_dir(&path).expect("unique C Phase-5 manager stage creates");
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

struct EmittedFixture {
    local: GeneratedPackage,
    foreign: GeneratedPackage,
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("schema-codegen lives beneath repository root")
        .to_path_buf()
}

fn authority_for(
    source: &str,
    document: &str,
    scope: &str,
) -> (
    type_bridge_schema::ResolvedSchema,
    type_bridge_schema::VerifiedSchemaAuthority,
) {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new(document).expect("document ID is valid"),
        source,
    )])
    .expect("Phase-5 schema parses");
    let declared = normalize_documents(&documents).expect("Phase-5 schema normalizes");
    let profile = SemanticProfileId::new(SEMANTIC_PROFILE).expect("profile is valid");
    let resolved = resolve(&declared, &profile).expect("Phase-5 schema resolves");
    let capabilities: CapabilitySet = BUILTIN_SCHEMA_CAPABILITY_IDS
        .iter()
        .map(|id| CapabilityId::new(*id).expect("built-in capability ID is valid"))
        .collect();
    let context = ManagedDeltaContext::new(
        ManagedScopeId::new(scope).expect("managed scope is valid"),
        profile,
        capabilities,
    );
    let authority = build_schema_authority(&declared, declared.required_capabilities(), &context)
        .expect("Phase-5 schema authority builds");
    (resolved, authority)
}

fn emitted_fixture() -> EmittedFixture {
    let needle = "      foo__bar: { card: { min: 0, max: 1 } }";
    assert_eq!(SOURCE.matches(needle).count(), 1, "foo__bar fact is unique");
    let foreign_source = SOURCE.replacen(needle, "      foo__bar: { card: { min: 0, max: 2 } }", 1);
    let (local_schema, local_authority) =
        authority_for(SOURCE, "workforce-v3.yaml", "workforce-v3-c-phase5-manager");
    let (foreign_schema, foreign_authority) = authority_for(
        &foreign_source,
        "workforce-v3-foreign-foo-bar.yaml",
        "workforce-v3-c-phase5-manager-foreign",
    );
    let emitter = CEmitter::new();
    let emit = |schema: &type_bridge_schema::ResolvedSchema,
                authority: &type_bridge_schema::VerifiedSchemaAuthority,
                prefix: &str| {
        let resources = emitter
            .code_resources_for(schema)
            .expect("C resources hash");
        let projection = project(
            schema,
            BindingTarget::C,
            &ProjectionConfig::c(CSymbolPrefix::new(prefix).expect("C prefix is valid")),
            &emitter.generator_handlers_for(schema),
            &resources,
        )
        .expect("Workforce V3 projects to C");
        (
            projection.projection_fingerprint().clone(),
            emitter
                .emit(&projection, authority)
                .expect("Workforce V3 C package emits"),
        )
    };
    let (local_fingerprint, local) = emit(&local_schema, &local_authority, "phase5");
    let (foreign_fingerprint, foreign) =
        emit(&foreign_schema, &foreign_authority, "phase5_foreign");
    assert_ne!(
        local_fingerprint, foreign_fingerprint,
        "foreign selected-field metadata retains distinct authority",
    );
    EmittedFixture { local, foreign }
}

fn write_package(package: &GeneratedPackage, root: &Path) {
    for (relative, contents) in package.files() {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("generated path has parent"))
            .expect("generated package directory creates");
        fs::write(path, contents).expect("generated package file writes");
    }
}

fn command_exists(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn runtime_include() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../c/include")
        .canonicalize()
        .expect("C runtime headers exist")
}

fn native_library() -> PathBuf {
    let executable = env::current_exe().expect("current executable is available");
    let filename = format!(
        "{}type_bridge_c{}",
        env::consts::DLL_PREFIX,
        env::consts::DLL_SUFFIX,
    );
    executable
        .ancestors()
        .flat_map(|directory| {
            [
                directory.join(&filename),
                directory.join("deps").join(&filename),
            ]
        })
        .find(|candidate| candidate.is_file())
        .expect("build type-bridge-c shared library before C live producer")
}

fn compile_consumer(
    compiler: &str,
    stage: &Path,
    local: &Path,
    foreign: &Path,
    library: Option<&Path>,
) -> PathBuf {
    let source = stage.join("phase5-manager-live-consumer.c");
    fs::write(&source, CONSUMER).expect("C Phase-5 manager consumer stages");
    let output = stage.join(format!("phase5-manager-live-{compiler}"));
    let mut command = Command::new(compiler);
    command
        .args([
            "-std=c17",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic-errors",
        ])
        .arg("-I")
        .arg(runtime_include())
        .arg("-I")
        .arg(local.join("include"))
        .arg("-I")
        .arg(foreign.join("include"))
        .arg(local.join("src/models.c"))
        .arg(foreign.join("src/models.c"))
        .arg(&source);
    if let Some(library) = library {
        let directory = library.parent().expect("shared library has parent");
        command
            .arg("-L")
            .arg(directory)
            .arg("-ltype_bridge_c")
            .arg(format!("-Wl,-rpath,{}", directory.display()))
            .arg("-o")
            .arg(&output);
    } else {
        command.arg("-fsyntax-only");
    }
    let compiled = command
        .output()
        .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
    assert!(
        compiled.status.success(),
        "{compiler} rejected strict C17 Phase-5 manager producer:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&compiled.stdout),
        String::from_utf8_lossy(&compiled.stderr),
    );
    output
}

fn assert_wrong_owner_compile_negative(compiler: &str, stage: &Path, local: &Path) {
    let source = stage.join(format!("phase5-wrong-owner-compile-fail-{compiler}.c"));
    fs::write(&source, WRONG_OWNER_COMPILE_FAIL)
        .expect("C Phase-5 wrong-owner compile-negative stages");
    let rejected = Command::new(compiler)
        .args([
            "-std=c17",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic-errors",
            "-fsyntax-only",
        ])
        .arg("-I")
        .arg(runtime_include())
        .arg("-I")
        .arg(local.join("include"))
        .arg(&source)
        .output()
        .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        !rejected.status.success(),
        "{compiler} accepted the generated C wrong-owner call:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&rejected.stdout),
        stderr,
    );
    assert!(
        stderr.contains("incompatible-pointer-types"),
        "{compiler} rejected the wrong-owner call for an unexpected reason:\n{stderr}",
    );
}

fn assert_manager_value_compile_negative(
    compiler: &str,
    stage: &Path,
    local: &Path,
    foreign: &Path,
    label: &str,
    fixture: &str,
) {
    let source = stage.join(format!("phase5-{label}-compile-fail-{compiler}.c"));
    fs::write(&source, fixture).expect("C Phase-5 manager value compile-negative stages");
    let rejected = Command::new(compiler)
        .args([
            "-std=c17",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic-errors",
            "-fsyntax-only",
        ])
        .arg("-I")
        .arg(runtime_include())
        .arg("-I")
        .arg(local.join("include"))
        .arg("-I")
        .arg(foreign.join("include"))
        .arg(&source)
        .output()
        .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        !rejected.status.success(),
        "{compiler} accepted the generated C {label} call:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&rejected.stdout),
        stderr,
    );
    assert!(
        stderr.contains("incompatible-pointer-types")
            && stderr.contains("phase5_person_manager_filter_foozuzubar_eq"),
        "{compiler} rejected the {label} call for an unexpected reason:\n{stderr}",
    );
}

fn required_environment(name: &str) -> String {
    env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| panic!("{name} must be configured and non-empty"))
}

fn report_destination() -> PathBuf {
    let path = PathBuf::from(required_environment(REPORT_ENV));
    assert!(path.is_absolute(), "{REPORT_ENV} must be absolute");
    let parent = path.parent().expect("report has parent");
    let metadata = fs::symlink_metadata(parent).expect("report parent is inspectable");
    assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => panic!("report destination cannot be inspected: {error}"),
        Ok(_) => panic!("report destination must not exist"),
    }
    path
}

fn parse_observation(stdout: &[u8]) -> Value {
    let text = std::str::from_utf8(stdout).expect("C manager facts are UTF-8");
    let facts = text
        .lines()
        .filter_map(|line| line.strip_prefix(FACT_PREFIX))
        .collect::<Vec<_>>();
    assert_eq!(facts.len(), 1, "C manager emits exactly one observation");
    let value: Value = serde_json::from_str(facts[0]).expect("C manager fact is JSON");
    assert!(value.is_object(), "C manager observation is an object");
    assert_eq!(
        to_canonical_json(&value).expect("C observation canonicalizes"),
        facts[0].as_bytes(),
        "C manager fact is canonical JSON",
    );
    value
}

fn report_authority(root: &Path) -> BTreeMap<&'static str, Value> {
    AUTHORITIES
        .into_iter()
        .map(|(name, relative, expected)| {
            let path = root.join(relative);
            let metadata = fs::symlink_metadata(&path).expect("authority is inspectable");
            assert!(metadata.is_file() && !metadata.file_type().is_symlink());
            assert!(metadata.len() <= MAX_AUTHORITY_BYTES);
            let bytes = fs::read(path).expect("authority reads");
            let actual = format!("{:x}", Sha256::digest(&bytes));
            assert_eq!(actual, expected, "{name} authority hash drifted");
            (name, json!({"path": relative, "sha256": actual}))
        })
        .collect()
}

fn publish_report(path: &Path, root: &Path, observation: Value) {
    let report = json!({
        "authority": report_authority(root),
        "binding": "c",
        "format": REPORT_FORMAT,
        "observation": observation,
        "semantic_profile": SEMANTIC_PROFILE,
    });
    let mut bytes = to_canonical_json(&report).expect("C manager report canonicalizes");
    bytes.push(b'\n');
    assert!(bytes.len() <= MAX_REPORT_BYTES);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .expect("C manager report creates without replacement");
    output.write_all(&bytes).expect("C manager report writes");
    output.sync_all().expect("C manager report synchronizes");
}

fn manifest_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "\\\\")
}

struct IsolatedDatabase {
    cargo: OsString,
    manifest: PathBuf,
    target: PathBuf,
    environment: Vec<(String, String)>,
    active: bool,
}

impl IsolatedDatabase {
    fn run(&self, mode: &str) -> Output {
        let mut command = Command::new(&self.cargo);
        command
            .args(["run", "--locked", "--quiet", "--manifest-path"])
            .arg(&self.manifest)
            .arg("--")
            .arg(mode)
            .env("CARGO_TARGET_DIR", &self.target);
        for (name, value) in &self.environment {
            command.env(name, value);
        }
        command
            .output()
            .expect("C manager database command launches")
    }

    fn setup(&mut self) {
        let output = self.run("setup");
        let stdout = String::from_utf8_lossy(&output.stdout);
        self.active = stdout.lines().any(|line| line == DATABASE_CREATED_MARKER);
        assert!(
            output.status.success(),
            "C manager setup failed:\nstdout:\n{}\nstderr:\n{}",
            stdout,
            String::from_utf8_lossy(&output.stderr),
        );
        assert!(self.active, "C manager setup omitted creation marker");
    }

    fn cleanup_with_retries(&self) -> Output {
        let mut output = self.run("cleanup");
        for _ in 1..3 {
            if output.status.success() {
                break;
            }
            output = self.run("cleanup");
        }
        output
    }

    fn cleanup(&mut self) {
        let output = self.cleanup_with_retries();
        assert!(
            output.status.success(),
            "C manager cleanup failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        self.active = false;
    }
}

impl Drop for IsolatedDatabase {
    fn drop(&mut self) {
        if self.active {
            let _ = self.cleanup_with_retries();
        }
    }
}

fn stage_setup(stage: &Path, environment: Vec<(String, String)>) -> IsolatedDatabase {
    let root = stage.join("setup");
    fs::create_dir_all(root.join("c_phase5_manager_live")).expect("setup source directory creates");
    fs::create_dir_all(root.join("workforce-v3")).expect("setup fixture directory creates");
    fs::write(root.join("c_phase5_manager_live/setup.rs"), SETUP).expect("setup source writes");
    fs::write(root.join("workforce-v3/provider.tql"), PROVIDER).expect("provider schema writes");
    let orm = Path::new(env!("CARGO_MANIFEST_DIR")).join("../orm");
    let manifest = root.join("Cargo.toml");
    fs::write(
        &manifest,
        format!(
            "[package]\nname = \"type-bridge-c-phase5-manager-live-setup\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\n[[bin]]\nname = \"setup\"\npath = \"c_phase5_manager_live/setup.rs\"\n\n[dependencies]\ntype-bridge-orm = {{ path = \"{}\" }}\ntokio = {{ version = \"1\", features = [\"macros\", \"rt-multi-thread\"] }}\n\n[workspace]\n",
            manifest_path(&orm),
        ),
    )
    .expect("setup manifest writes");
    fs::write(root.join("Cargo.lock"), SETUP_LOCK).expect("setup lockfile is staged");
    IsolatedDatabase {
        cargo: env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")),
        manifest,
        target: env::var_os("ACCEPTANCE_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| stage.join("target")),
        environment,
        active: false,
    }
}

#[test]
fn c_phase5_manager_live_setup_dependency_graph_is_frozen() {
    let lock = std::str::from_utf8(SETUP_LOCK).expect("setup lockfile is UTF-8");
    assert_eq!(
        lock.matches("name = \"type-bridge-c-phase5-manager-live-setup\"")
            .count(),
        1
    );
    assert!(lock.contains("name = \"tinyvec\"\nversion = \"1.12.0\""));
    assert!(!lock.contains("name = \"tinyvec\"\nversion = \"1.13.0\""));
}

#[test]
fn c_phase5_manager_live_producer_is_bounded_and_expectation_independent() {
    assert!(CONSUMER.len() <= MAX_PRODUCER_BYTES);
    for required in [
        "phase5_person_manager_filter_foozuzubar_eq",
        "phase5_person_manager_filter_foozuzubar_gte",
        "phase5_person_manager_read_transaction_all",
        "manager_first_requires_identity",
        "phase5_read_transaction_query_execute_count",
    ] {
        assert!(CONSUMER.contains(required), "producer omits {required}");
    }
    assert!(CONSUMER.contains("TYPE_BRIDGE_STATUS_INVALID_ARGUMENT"));
    assert!(CONSUMER.contains("cross_owner_field"));
    assert!(CONSUMER.contains("c_query_nominal_contract_mismatch"));
    assert!(CONSUMER.contains("not rebranded as observed manager diagnostics"));
    assert!(
        WRONG_OWNER_COMPILE_FAIL.contains("phase5_person_foozuzubar_query_field_from_exact(robot")
    );
    assert!(WRONG_SCALAR_COMPILE_FAIL.contains("const phase5_identifier *value"));
    assert!(CONSUMER.contains("phase5_person_manager_open(foreign_package"));
}

#[test]
fn strict_c17_phase5_manager_live_consumer_compiles() {
    let fixture = emitted_fixture();
    let stage = Stage::new();
    let local = stage.path().join("local");
    let foreign = stage.path().join("foreign");
    write_package(&fixture.local, &local);
    write_package(&fixture.foreign, &foreign);
    let mut compiled = 0usize;
    for compiler in ["gcc", "clang"] {
        if command_exists(compiler) {
            compile_consumer(compiler, stage.path(), &local, &foreign, None);
            assert_wrong_owner_compile_negative(compiler, stage.path(), &local);
            assert_manager_value_compile_negative(
                compiler,
                stage.path(),
                &local,
                &foreign,
                "wrong-scalar",
                WRONG_SCALAR_COMPILE_FAIL,
            );
            compiled += 1;
        }
    }
    assert!(
        compiled > 0,
        "no supported strict C17 compiler is available"
    );
}

#[test]
fn c_phase5_manager_live_runs_when_explicitly_configured() {
    let Some(_) = env::var_os(REPORT_ENV) else {
        return;
    };
    for required in [ADDRESS_ENV, HTTP_PORT_ENV, DATABASE_ENV] {
        assert!(
            env::var_os(required).is_some(),
            "live run requires {required}"
        );
    }
    let report = report_destination();
    let fixture = emitted_fixture();
    let stage = Stage::new();
    let local = stage.path().join("local");
    let foreign = stage.path().join("foreign");
    write_package(&fixture.local, &local);
    write_package(&fixture.foreign, &foreign);
    let compiler = ["clang", "gcc"]
        .into_iter()
        .find(|compiler| command_exists(compiler))
        .expect("a strict C17 compiler is available");
    let executable = compile_consumer(
        compiler,
        stage.path(),
        &local,
        &foreign,
        Some(&native_library()),
    );
    assert_wrong_owner_compile_negative(compiler, stage.path(), &local);
    assert_manager_value_compile_negative(
        compiler,
        stage.path(),
        &local,
        &foreign,
        "wrong-scalar",
        WRONG_SCALAR_COMPILE_FAIL,
    );
    let environment = [ADDRESS_ENV, HTTP_PORT_ENV, DATABASE_ENV]
        .into_iter()
        .map(|name| (name.to_owned(), required_environment(name)))
        .chain([
            (
                "TYPEDB_USERNAME".to_owned(),
                env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned()),
            ),
            (
                "TYPEDB_PASSWORD".to_owned(),
                env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned()),
            ),
        ])
        .collect::<Vec<_>>();
    let mut database = stage_setup(stage.path(), environment.clone());
    database.setup();
    let mut command = Command::new(executable);
    for (name, value) in &environment {
        command.env(name, value);
    }
    let output = command.output().expect("C manager producer launches");
    let primary = output.status.success();
    let observation = if primary {
        Some(parse_observation(&output.stdout))
    } else {
        None
    };
    database.cleanup();
    assert!(
        primary,
        "C Phase-5 manager producer failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    publish_report(
        &report,
        &repository_root(),
        observation.expect("successful C producer has observation"),
    );
}
