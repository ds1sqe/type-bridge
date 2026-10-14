use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use type_bridge_schema_codegen::{GeneratedPackage, RustEmitter};

mod support;
use support::sdk_profile_matrix::{self, SdkProfile};
use support::{
    Stage, cargo_with_env as cargo, repository_root as repository, rust_projection_on,
    write_package,
};

const PRODUCER: &str = include_str!("rust_acceptance/manager_live.rs");
const OUTPUT_ENV: &str = "TYPE_BRIDGE_MANAGER_LIVE_REPORT";
const ADDRESS_ENV: &str = "TYPE_BRIDGE_MANAGER_LIVE_ADDRESS";
const DATABASE_ENV: &str = "TYPE_BRIDGE_MANAGER_LIVE_DATABASE";
const HTTP_PORT_ENV: &str = "TYPE_BRIDGE_MANAGER_LIVE_HTTP_PORT";
const ROOT_ENV: &str = "TYPE_BRIDGE_MANAGER_REPOSITORY_ROOT";
const PROFILE_ENV: &str = "TYPE_BRIDGE_MANAGER_LIVE_PROFILE";
const MAX_PRODUCER_BYTES: usize = 1024 * 1024;

fn sdk_source() -> String {
    fs::read_to_string(repository().join("tests/contracts/sdk_conformance/sdk-v3/schema-v3.yaml"))
        .expect("Sdk V3 schema reads")
}

fn foreign_sdk_source() -> String {
    let source = sdk_source();
    let needle = "      foo__bar: { card: { min: 0, max: 1 } }";
    assert_eq!(source.matches(needle).count(), 1, "foo__bar fact is unique");
    let foreign = source.replacen(needle, "      foo__bar: { card: { min: 0, max: 2 } }", 1);
    assert_ne!(source, foreign, "foreign foo__bar token metadata differs");
    foreign
}

fn emit_from_source(source: &str, document: &str, profile: &SdkProfile) -> GeneratedPackage {
    let semantic_profile = profile.semantic_profile.as_str();
    RustEmitter::new()
        .emit(
            &rust_projection_on(source, document, semantic_profile),
            &support::authority_with(source, support::TEST_SCOPE, semantic_profile),
        )
        .expect("Manager Rust package emits")
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
name = "rust-manager-live"
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
    fs::write(
        root.join("Cargo.lock"),
        support::locks::Consumer::ManagerLive.lock(),
    )
    .expect("consumer lockfile is staged");
}

fn staged_consumer(stage: &Stage, profile: &SdkProfile) -> PathBuf {
    let generated = stage.path().join("generated");
    let foreign = stage.path().join("generated-foreign");
    let consumer = stage.path().join("consumer");
    write_package(
        &emit_from_source(&sdk_source(), "sdk-v3.yaml", profile),
        &generated,
    );
    write_foreign_package(
        &emit_from_source(
            &foreign_sdk_source(),
            "sdk-v3-foreign-foo-bar.yaml",
            profile,
        ),
        &foreign,
    );
    write_consumer(&consumer);
    consumer.join("Cargo.toml")
}

#[test]
fn rust_manager_live_producer_is_bounded_and_expectation_independent() {
    assert!(PRODUCER.len() <= MAX_PRODUCER_BYTES);
    for required in [
        OUTPUT_ENV,
        ADDRESS_ENV,
        DATABASE_ENV,
        HTTP_PORT_ENV,
        ROOT_ENV,
        PROFILE_ENV,
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
fn generated_rust_manager_live_producer_compiles() {
    let stage = Stage::new();
    let manifest = staged_consumer(&stage, &sdk_profile_matrix::requested(3));
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
        "Rust Manager producer failed strict compile\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&checked.stdout),
        String::from_utf8_lossy(&checked.stderr),
    );
}

#[test]
fn generated_rust_manager_live_runs_when_explicitly_configured() {
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
    let profile = sdk_profile_matrix::requested(3);
    let profile_json = support::sdk_profile_entry_json(&profile);
    let manifest = staged_consumer(&stage, &profile);
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
        &[
            (OUTPUT_ENV, report.as_os_str()),
            (ROOT_ENV, repository.as_os_str()),
            (PROFILE_ENV, profile_json.as_ref()),
        ],
    );
    assert!(
        output.status.success(),
        "Rust Manager live producer failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let metadata = fs::symlink_metadata(&report).expect("live report exists");
    assert!(metadata.is_file() && !metadata.file_type().is_symlink());
    assert!(metadata.len() <= 256 * 1024);
    assert_eq!(fs::read(report).expect("report reads").last(), Some(&b'\n'));
}

#[test]
fn rust_manager_live_dependency_graph_is_frozen() {
    let lock = support::locks::Consumer::ManagerLive.lock();
    assert_eq!(lock.matches("name = \"rust-manager-live\"").count(), 1);
    assert!(lock.contains("name = \"tinyvec\"\nversion = \"1.12.0\""));
    assert!(!lock.contains("name = \"tinyvec\"\nversion = \"1.13.0\""));
}
