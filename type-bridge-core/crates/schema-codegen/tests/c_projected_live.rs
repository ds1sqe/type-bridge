#![cfg(unix)]

#[path = "support/c_provider.rs"]
mod c_provider;
use c_provider::{IsolatedDatabase, SETUP_LOCK};

use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
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
    include_str!("../../../../tests/contracts/sdk_conformance/sdk-v3/schema-v3.yaml");
const PROVIDER_SCHEMA: &str =
    include_str!("../../../../tests/contracts/sdk_conformance/sdk-v3/provider-3.12.1-v3.tql");
const CONSUMER: &str = include_str!("c_projected_live/consumer.c");

const REPORT_FORMAT: &str = "typebridge.projected-live-report/v1";
const SEMANTIC_PROFILE: &str = "typedb-3.12.1/v1";
const FACT_PREFIX: &str = "TYPE_BRIDGE_C_PROJECTED_LIVE_FACT\t";
const REPORT_ENV: &str = "TYPE_BRIDGE_PROJECTED_LIVE_REPORT";
const ADDRESS_ENV: &str = "TYPE_BRIDGE_PROJECTED_LIVE_ADDRESS";
const HTTP_PORT_ENV: &str = "TYPE_BRIDGE_PROJECTED_LIVE_HTTP_PORT";
const DATABASE_ENV: &str = "TYPE_BRIDGE_PROJECTED_LIVE_DATABASE";
const MAX_AUTHORITY_BYTES: u64 = 1024 * 1024;
const MAX_REPORT_BYTES: usize = 256 * 1024;
const OBSERVATIONS: [&str; 5] = [
    "canonical_scalar_values",
    "cleanup",
    "inherited_plain_activity_role_lifecycle",
    "integer_key_polymorphic_optional_role",
    "relation_as_player",
];
const AUTHORITIES: [(&str, &str, &str); 3] = [
    (
        "journey",
        "tests/contracts/sdk_conformance/sdk-v3/journey-v3.json",
        "c5679b428c22e2bff7989f6d674cde3780b752a40f6bbce6ccb3aa451797b1c0",
    ),
    (
        "provider",
        "tests/contracts/sdk_conformance/sdk-v3/provider-3.12.1-v3.tql",
        "af61aaece22d666e19d2c1357f8ebc39b8cbcf4bff7b98a19a56daf171a1faba",
    ),
    (
        "schema",
        "tests/contracts/sdk_conformance/sdk-v3/schema-v3.yaml",
        "74b9161e3d8fd70a1f22c3a8b7fc70a823b18cb07973064e2e10ce0940f76f1f",
    ),
];

static STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Stage(PathBuf);

impl Stage {
    fn new() -> Self {
        let sequence = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "typebridge-c-projected-live-{}-{sequence}",
            std::process::id(),
        ));
        fs::create_dir(&path).expect("unique C Projected live stage creates");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("C Projected live stage removes");
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
        .expect("schema-codegen lives beneath the repository root")
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
        DocumentId::new(document).expect("Projected document ID is valid"),
        source,
    )])
    .expect("Projected schema parses");
    let declared = normalize_documents(&documents).expect("Projected schema normalizes");
    let profile = SemanticProfileId::new(SEMANTIC_PROFILE).expect("Projected profile is valid");
    let resolved = resolve(&declared, &profile).expect("Projected schema resolves");
    let capabilities: CapabilitySet = BUILTIN_SCHEMA_CAPABILITY_IDS
        .iter()
        .map(|id| CapabilityId::new(*id).expect("built-in capability ID is valid"))
        .collect();
    let context = ManagedDeltaContext::new(
        ManagedScopeId::new(scope).expect("Projected scope is valid"),
        profile,
        capabilities,
    );
    let authority = build_schema_authority(&declared, declared.required_capabilities(), &context)
        .expect("Projected schema authority builds");
    (resolved, authority)
}

fn emitted_fixture() -> EmittedFixture {
    let foreign_source = SOURCE.replace(
        "card: { min: 0, max: 3 }\n        ordered: true\n        distinct: true\n      score:",
        "card: { min: 0, max: 2 }\n        ordered: true\n        distinct: true\n      score:",
    );
    assert_ne!(foreign_source, SOURCE, "foreign C schema mutation applies");
    let (local_schema, local_authority) = authority_for(SOURCE, "sdk-v3.yaml", "sdk-v3-c-live");
    let (foreign_schema, foreign_authority) = authority_for(
        &foreign_source,
        "sdk-v3-foreign.yaml",
        "sdk-v3-c-live-foreign",
    );
    let emitter = CEmitter::new();
    let emit = |schema: &type_bridge_schema::ResolvedSchema,
                authority: &type_bridge_schema::VerifiedSchemaAuthority,
                prefix: &str| {
        let handlers = emitter.generator_handlers_for(schema);
        let resources = emitter
            .code_resources_for(schema)
            .expect("Projected C resources hash");
        let projection = project(
            schema,
            BindingTarget::C,
            &ProjectionConfig::c(CSymbolPrefix::new(prefix).expect("C prefix is valid")),
            &handlers,
            &resources,
        )
        .expect("Sdk V3 projects to C");
        assert_eq!(
            projection.semantic_fingerprint(),
            schema.semantic_fingerprint(),
            "generated C package retains its resolved semantic authority",
        );
        (
            projection.projection_fingerprint().clone(),
            emitter
                .emit(&projection, authority)
                .expect("Sdk V3 C package emits"),
        )
    };
    let (local_fingerprint, local) = emit(&local_schema, &local_authority, "projected");
    let (foreign_fingerprint, foreign) =
        emit(&foreign_schema, &foreign_authority, "projected_foreign");
    assert_ne!(
        local_fingerprint, foreign_fingerprint,
        "foreign-compatible package must retain distinct generated authority",
    );
    EmittedFixture { local, foreign }
}

fn write_package(package: &GeneratedPackage, root: &Path) {
    for (relative, contents) in package.files() {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("generated path has a parent"))
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
    let executable = env::current_exe().expect("current test executable is available");
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
        .expect("build type-bridge-c's shared library before the C live producer")
}

fn compile_consumer(
    compiler: &str,
    stage: &Path,
    local: &Path,
    foreign: &Path,
    library: Option<&Path>,
) -> PathBuf {
    let source = stage.join("projected-live-consumer.c");
    fs::write(&source, CONSUMER).expect("C Projected live consumer stages");
    let output = stage.join(format!("projected-live-{compiler}"));
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
        let directory = library.parent().expect("shared library has a parent");
        command
            .arg("-L")
            .arg(directory)
            .arg("-ltype_bridge_c")
            .arg(format!("-Wl,-rpath,{}", directory.display()))
            .arg("-o")
            .arg(&output);
    } else {
        command.args(["-fsyntax-only"]);
    }
    let compiled = command
        .output()
        .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
    assert!(
        compiled.status.success(),
        "{compiler} rejected the strict C17 Projected live producer:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&compiled.stdout),
        String::from_utf8_lossy(&compiled.stderr),
    );
    output
}

fn required_environment(name: &str) -> String {
    env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| panic!("{name} must be configured and non-empty"))
}

fn exact_http_port(value: &str) -> u16 {
    assert!(
        !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()),
        "{HTTP_PORT_ENV} must be an ASCII integer",
    );
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .unwrap_or_else(|| panic!("{HTTP_PORT_ENV} must be in 1..65535"))
}

fn report_destination() -> PathBuf {
    let path = PathBuf::from(required_environment(REPORT_ENV));
    assert!(path.is_absolute(), "{REPORT_ENV} must be an absolute path");
    let parent = path.parent().expect("report path has a parent");
    let metadata = fs::symlink_metadata(parent).expect("report parent is inspectable");
    assert!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "report parent must be a real directory",
    );
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => panic!("{REPORT_ENV} destination cannot be inspected: {error}"),
        Ok(_) => panic!("{REPORT_ENV} destination must not already exist"),
    }
    path
}

fn parse_observations(stdout: &[u8]) -> BTreeMap<String, Value> {
    let text = std::str::from_utf8(stdout).expect("C Projected live facts are UTF-8");
    let mut observations = BTreeMap::new();
    for line in text.lines() {
        let Some(payload) = line.strip_prefix(FACT_PREFIX) else {
            continue;
        };
        let (name, raw) = payload
            .split_once('\t')
            .expect("C Projected live fact has a name and value");
        assert!(
            OBSERVATIONS.contains(&name),
            "unexpected C Projected live fact {name}",
        );
        let value: Value = serde_json::from_str(raw).expect("C live fact is valid JSON");
        assert!(value.is_object(), "C live fact must be an object");
        assert_eq!(
            to_canonical_json(&value).expect("C live fact canonicalizes"),
            raw.as_bytes(),
            "C live fact {name} is not canonical JSON",
        );
        assert!(
            observations.insert(name.to_owned(), value).is_none(),
            "C live fact {name} was duplicated",
        );
    }
    assert_eq!(
        observations.keys().map(String::as_str).collect::<Vec<_>>(),
        OBSERVATIONS,
        "C Projected live observation ledger is incomplete",
    );
    observations
}

fn authority(root: &Path) -> BTreeMap<&'static str, Value> {
    AUTHORITIES
        .into_iter()
        .map(|(name, relative, expected)| {
            let path = root.join(relative);
            let metadata = fs::symlink_metadata(&path)
                .unwrap_or_else(|error| panic!("{name} authority cannot be inspected: {error}"));
            assert!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "{name} authority must be a regular non-symlink file",
            );
            assert!(
                metadata.len() <= MAX_AUTHORITY_BYTES,
                "{name} authority exceeds its byte limit",
            );
            let bytes = fs::read(path).expect("Projected authority reads");
            assert!(bytes.len() as u64 <= MAX_AUTHORITY_BYTES);
            let actual = format!("{:x}", Sha256::digest(&bytes));
            assert_eq!(actual, expected, "{name} authority hash drifted");
            (
                name,
                json!({
                    "path": relative,
                    "sha256": actual,
                }),
            )
        })
        .collect()
}

fn publish_report(path: &Path, root: &Path, observations: BTreeMap<String, Value>) {
    let report = json!({
        "authority": authority(root),
        "binding": "c",
        "format": REPORT_FORMAT,
        "observations": observations,
        "semantic_profile": SEMANTIC_PROFILE,
    });
    let mut bytes = to_canonical_json(&report).expect("C live report canonicalizes");
    bytes.push(b'\n');
    assert!(
        bytes.len() <= MAX_REPORT_BYTES,
        "C live report exceeds its byte limit",
    );
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .expect("C live report creates without replacement");
    output.write_all(&bytes).expect("C live report writes");
    output.sync_all().expect("C live report synchronizes");
}

fn stage_setup(stage: &Path, environment: Vec<(String, String)>) -> IsolatedDatabase {
    c_provider::stage_setup(
        stage,
        environment,
        "TYPE_BRIDGE_PROJECTED_LIVE",
        PROVIDER_SCHEMA,
    )
}

#[test]
fn c_projected_live_setup_dependency_graph_is_frozen() {
    let lock = std::str::from_utf8(SETUP_LOCK).expect("setup lockfile is UTF-8");
    assert_eq!(
        lock.matches("name = \"type-bridge-test-provider\"").count(),
        1
    );
    assert!(lock.contains("name = \"tinyvec\"\nversion = \"1.12.0\""));
    assert!(!lock.contains("name = \"tinyvec\"\nversion = \"1.13.0\""));
}

#[test]
fn report_publisher_is_bounded_create_new_and_non_overwriting() {
    let stage = Stage::new();
    let destination = stage.path().join("report.json");
    let observations = OBSERVATIONS
        .into_iter()
        .map(|name| (name.to_owned(), json!({"observed": true})))
        .collect();
    publish_report(&destination, &repository_root(), observations);
    let bytes = fs::read(&destination).expect("published report reads");
    assert!(bytes.len() <= MAX_REPORT_BYTES);
    assert_eq!(bytes.last(), Some(&b'\n'));
    let observations = OBSERVATIONS
        .into_iter()
        .map(|name| (name.to_owned(), json!({"observed": true})))
        .collect();
    assert!(
        std::panic::catch_unwind(|| {
            publish_report(&destination, &repository_root(), observations);
        })
        .is_err(),
        "a pre-existing report must not be replaced",
    );
}

#[test]
fn failed_setup_only_cleans_after_the_creation_marker() {
    use std::os::unix::fs::PermissionsExt;

    let stage = Stage::new();
    let fake_cargo = stage.path().join("fake-cargo");
    let log = stage.path().join("modes.log");
    fs::write(
        &fake_cargo,
        "#!/bin/sh\nfor argument do mode=$argument; done\nprintf '%s\\n' \"$mode\" >> \"$TYPE_BRIDGE_C_PROJECTED_LIVE_CLEANUP_LOG\"\nif test \"$mode\" = setup; then\n  if test \"${TYPE_BRIDGE_C_PROJECTED_LIVE_EMIT_CREATED:-}\" = 1; then\n    printf '%s\\n' 'C test database created'\n  fi\n  exit 23\nfi\n",
    )
    .expect("fake cargo command writes");
    let mut permissions = fs::metadata(&fake_cargo)
        .expect("fake cargo metadata reads")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_cargo, permissions).expect("fake cargo becomes executable");

    let failure = std::panic::catch_unwind(|| {
        let mut isolated = IsolatedDatabase {
            cargo: fake_cargo.clone().into_os_string(),
            manifest: stage.path().join("unused-Cargo.toml"),
            target: stage.path().join("unused-target"),
            environment: vec![
                (
                    "TYPE_BRIDGE_C_PROJECTED_LIVE_CLEANUP_LOG".to_owned(),
                    log.to_string_lossy().into_owned(),
                ),
                (
                    "TYPE_BRIDGE_C_PROJECTED_LIVE_EMIT_CREATED".to_owned(),
                    "1".to_owned(),
                ),
            ],
            active: false,
            env_prefix: "TYPE_BRIDGE_PROJECTED_LIVE",
        };
        isolated.setup();
    });
    assert!(failure.is_err());
    assert_eq!(
        fs::read_to_string(&log).expect("setup and cleanup modes log"),
        "setup\ncleanup\n",
    );

    let before_create_log = stage.path().join("before-create.log");
    let failure = std::panic::catch_unwind(|| {
        let mut isolated = IsolatedDatabase {
            cargo: fake_cargo.into_os_string(),
            manifest: stage.path().join("unused-before-Cargo.toml"),
            target: stage.path().join("unused-before-target"),
            environment: vec![(
                "TYPE_BRIDGE_C_PROJECTED_LIVE_CLEANUP_LOG".to_owned(),
                before_create_log.to_string_lossy().into_owned(),
            )],
            active: false,
            env_prefix: "TYPE_BRIDGE_PROJECTED_LIVE",
        };
        isolated.setup();
    });
    assert!(failure.is_err());
    assert_eq!(
        fs::read_to_string(before_create_log).expect("pre-create mode logs"),
        "setup\n",
        "an absent/pre-existing rejection must never trigger deletion",
    );
}

#[test]
fn strict_c17_projected_live_consumer_compiles_with_both_available_compilers() {
    let fixture = emitted_fixture();
    let stage = Stage::new();
    let local = stage.path().join("local");
    let foreign = stage.path().join("foreign");
    write_package(&fixture.local, &local);
    write_package(&fixture.foreign, &foreign);
    let mut count = 0;
    for compiler in ["gcc", "clang"] {
        if command_exists(compiler) {
            count += 1;
            compile_consumer(compiler, stage.path(), &local, &foreign, None);
        }
    }
    assert!(count > 0, "GCC or Clang is required for generated C checks");
}

#[test]
#[ignore = "requires an isolated exact TypeDB 3.12.1 server and C shared library"]
fn generated_c17_projected_live_subset_round_trips_exact_3_12_1() {
    let root = repository_root();
    let report = report_destination();
    let address = required_environment(ADDRESS_ENV);
    let http_port = required_environment(HTTP_PORT_ENV);
    exact_http_port(&http_port);
    let database = required_environment(DATABASE_ENV);
    let username = env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned());
    let password = env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned());
    let compiler = ["gcc", "clang"]
        .into_iter()
        .find(|compiler| command_exists(compiler))
        .expect("GCC or Clang is required for generated C live acceptance");
    let library = native_library();
    let fixture = emitted_fixture();
    let stage = Stage::new();
    let local = stage.path().join("local");
    let foreign = stage.path().join("foreign");
    write_package(&fixture.local, &local);
    write_package(&fixture.foreign, &foreign);
    let executable = compile_consumer(compiler, stage.path(), &local, &foreign, Some(&library));
    let environment = vec![
        (ADDRESS_ENV.to_owned(), address),
        (HTTP_PORT_ENV.to_owned(), http_port),
        (DATABASE_ENV.to_owned(), database),
        ("TYPEDB_USERNAME".to_owned(), username),
        ("TYPEDB_PASSWORD".to_owned(), password),
    ];
    let mut isolated = stage_setup(stage.path(), environment.clone());
    isolated.setup();

    let mut command = Command::new(executable);
    for (name, value) in &environment {
        command.env(name, value);
    }
    command.env_remove(REPORT_ENV);
    let output = command
        .output()
        .expect("C Projected live consumer launches");
    assert!(
        output.status.success(),
        "C Projected live consumer failed with {}:\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let observations = parse_observations(&output.stdout);
    isolated.cleanup();
    publish_report(&report, &root, observations);
}
