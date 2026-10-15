#![cfg(unix)]

mod support;
use support::sdk_profile_matrix::{self, SdkProfile};
use support::{
    Stage, command_exists, compile_c_consumer, native_library, repository_root, runtime_include,
    write_package,
};

#[path = "support/c_provider.rs"]
mod c_provider;
use c_provider::{EXPECTED_SERVER_VERSION_ENV, IsolatedDatabase, SETUP_LOCK, profiled_authority};

use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::projection::{BindingTarget, CSymbolPrefix, ProjectionConfig};
use type_bridge_schema::project;
use type_bridge_schema_codegen::{CEmitter, GeneratedPackage};

const SOURCE: &str = include_str!("../../../../tests/contracts/sdk_conformance/models/schema.yaml");
const PROVIDER: &str =
    include_str!("../../../../tests/contracts/sdk_conformance/models/provider-3.12.1.tql");
const CONSUMER: &str = include_str!("c_manager_live/consumer.c");
const WRONG_OWNER_COMPILE_FAIL: &str = include_str!("c_manager_live/wrong_owner_compile_fail.c");
const WRONG_SCALAR_COMPILE_FAIL: &str = include_str!("c_manager_live/wrong_scalar_compile_fail.c");

const REPORT_FORMAT: &str = "typebridge.manager-filter-live-report/v1";
const FACT_PREFIX: &str = "TYPE_BRIDGE_C_MANAGER_LIVE_FACT\t";
const REPORT_ENV: &str = "TYPE_BRIDGE_MANAGER_LIVE_REPORT";
const ADDRESS_ENV: &str = "TYPE_BRIDGE_MANAGER_LIVE_ADDRESS";
const HTTP_PORT_ENV: &str = "TYPE_BRIDGE_MANAGER_LIVE_HTTP_PORT";
const DATABASE_ENV: &str = "TYPE_BRIDGE_MANAGER_LIVE_DATABASE";
const MAX_AUTHORITY_BYTES: u64 = 1024 * 1024;
const MAX_REPORT_BYTES: usize = 256 * 1024;
const MAX_PRODUCER_BYTES: usize = 1024 * 1024;
const AUTHORITIES: [(&str, &str, &str); 3] = [
    (
        "journey",
        "tests/contracts/sdk_conformance/models/journey.json",
        "d0153b47c42f771cae1668fcfdb0e3d9d7c4e3ee4d793177751e1b657fe37e66",
    ),
    (
        "provider",
        "tests/contracts/sdk_conformance/models/provider-3.12.1.tql",
        "af61aaece22d666e19d2c1357f8ebc39b8cbcf4bff7b98a19a56daf171a1faba",
    ),
    (
        "schema",
        "tests/contracts/sdk_conformance/models/schema.yaml",
        "74b9161e3d8fd70a1f22c3a8b7fc70a823b18cb07973064e2e10ce0940f76f1f",
    ),
];

struct EmittedFixture {
    local: GeneratedPackage,
    foreign: GeneratedPackage,
}

fn emitted_fixture(profile: &SdkProfile) -> EmittedFixture {
    let needle = "      foo__bar: { card: { min: 0, max: 1 } }";
    assert_eq!(SOURCE.matches(needle).count(), 1, "foo__bar fact is unique");
    let foreign_source = SOURCE.replacen(needle, "      foo__bar: { card: { min: 0, max: 2 } }", 1);
    let (local_schema, local_authority) = profiled_authority(
        SOURCE,
        "models.yaml",
        "models-c-manager",
        &profile.semantic_profile,
    );
    let (foreign_schema, foreign_authority) = profiled_authority(
        &foreign_source,
        "models-foreign-foo-bar.yaml",
        "models-c-manager-foreign",
        &profile.semantic_profile,
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
        .expect("Models projects to C");
        (
            projection.projection_fingerprint().clone(),
            emitter
                .emit(&projection, authority)
                .expect("Models C package emits"),
        )
    };
    let (local_fingerprint, local) = emit(&local_schema, &local_authority, "manager");
    let (foreign_fingerprint, foreign) =
        emit(&foreign_schema, &foreign_authority, "manager_foreign");
    assert_ne!(
        local_fingerprint, foreign_fingerprint,
        "foreign selected-field metadata retains distinct authority",
    );
    EmittedFixture { local, foreign }
}

fn assert_wrong_owner_compile_negative(compiler: &str, stage: &Path, local: &Path) {
    let source = stage.join(format!("manager-wrong-owner-compile-fail-{compiler}.c"));
    fs::write(&source, WRONG_OWNER_COMPILE_FAIL)
        .expect("C Manager wrong-owner compile-negative stages");
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
    let source = stage.join(format!("manager-{label}-compile-fail-{compiler}.c"));
    fs::write(&source, fixture).expect("C Manager value compile-negative stages");
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
            && stderr.contains("manager_person_manager_filter_foozuzubar_eq"),
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

fn publish_report(path: &Path, root: &Path, profile: &SdkProfile, observation: Value) {
    let report = json!({
        "authority": report_authority(root),
        "binding": "c",
        "format": REPORT_FORMAT,
        "observation": observation,
        "semantic_profile": profile.semantic_profile,
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

fn stage_setup(stage: &Path, environment: Vec<(String, String)>) -> IsolatedDatabase {
    c_provider::stage_setup(stage, environment, "TYPE_BRIDGE_MANAGER_LIVE", PROVIDER)
}

#[test]
fn c_manager_live_setup_dependency_graph_is_frozen() {
    let lock = std::str::from_utf8(SETUP_LOCK).expect("setup lockfile is UTF-8");
    assert_eq!(
        lock.matches("name = \"type-bridge-test-provider\"").count(),
        1
    );
    assert!(lock.contains("name = \"tinyvec\"\nversion = \"1.12.0\""));
    assert!(!lock.contains("name = \"tinyvec\"\nversion = \"1.13.0\""));
}

#[test]
fn c_manager_live_producer_is_bounded_and_expectation_independent() {
    assert!(CONSUMER.len() <= MAX_PRODUCER_BYTES);
    for required in [
        "manager_person_manager_filter_foozuzubar_eq",
        "manager_person_manager_filter_foozuzubar_gte",
        "manager_person_manager_read_transaction_all",
        "manager_first_requires_identity",
        "manager_read_transaction_query_execute_count",
    ] {
        assert!(CONSUMER.contains(required), "producer omits {required}");
    }
    assert!(CONSUMER.contains("TYPE_BRIDGE_STATUS_INVALID_ARGUMENT"));
    assert!(CONSUMER.contains("cross_owner_field"));
    assert!(CONSUMER.contains("c_query_nominal_contract_mismatch"));
    assert!(CONSUMER.contains("not rebranded as observed manager diagnostics"));
    assert!(
        WRONG_OWNER_COMPILE_FAIL.contains("manager_person_foozuzubar_query_field_from_exact(robot")
    );
    assert!(WRONG_SCALAR_COMPILE_FAIL.contains("const manager_identifier *value"));
    assert!(CONSUMER.contains("manager_person_manager_open(foreign_package"));
}

#[test]
fn strict_c17_manager_live_consumer_compiles() {
    let fixture = emitted_fixture(&sdk_profile_matrix::requested(3));
    let stage = Stage::new();
    let local = stage.path().join("local");
    let foreign = stage.path().join("foreign");
    write_package(&fixture.local, &local);
    write_package(&fixture.foreign, &foreign);
    let mut compiled = 0usize;
    for compiler in ["gcc", "clang"] {
        if command_exists(compiler) {
            compile_c_consumer(
                ("manager", CONSUMER),
                compiler,
                stage.path(),
                &local,
                &foreign,
                None,
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
            compiled += 1;
        }
    }
    assert!(
        compiled > 0,
        "no supported strict C17 compiler is available"
    );
}

#[test]
fn c_manager_live_runs_when_explicitly_configured() {
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
    let profile = sdk_profile_matrix::requested(3);
    let fixture = emitted_fixture(&profile);
    let stage = Stage::new();
    let local = stage.path().join("local");
    let foreign = stage.path().join("foreign");
    write_package(&fixture.local, &local);
    write_package(&fixture.foreign, &foreign);
    let compiler = ["clang", "gcc"]
        .into_iter()
        .find(|compiler| command_exists(compiler))
        .expect("a strict C17 compiler is available");
    let executable = compile_c_consumer(
        ("manager", CONSUMER),
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
            (
                EXPECTED_SERVER_VERSION_ENV.to_owned(),
                profile.live_server_version.clone(),
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
        "C Manager producer failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    publish_report(
        &report,
        &repository_root(),
        &profile,
        observation.expect("successful C producer has observation"),
    );
}
