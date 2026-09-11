use std::env;
use std::fs;
use std::fs::File;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig};
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema::{
    SchemaDocumentSet, encode_schema_authority, normalize_documents, project, resolve,
};
use type_bridge_schema_codegen::RustEmitter;

mod support;

const SCHEMA: &str = include_str!("acceptance/schema.yaml");
const SCHEMA_3_11: &str = include_str!("acceptance/schema-3.11.5.yaml");
const PROVIDER_SCHEMA: &str = include_str!("acceptance/provider-3.12.1.tql");
const PROVIDER_SCHEMA_3_11: &str = include_str!("acceptance/provider-3.11.5.tql");
const INTERNAL_FIXTURE: &str = include_str!("rust_projection_live/internal_fixture.rs");
const CONSUMER: &str = include_str!("rust_projection_live/consumer.rs");
const CONSUMER_LOCK: &[u8] = include_bytes!("rust_projection_live/consumer-Cargo.lock");
const FIXTURE_LOCK: &[u8] = include_bytes!("../../../../tests/support/provider/Cargo.lock");
const SDK_MANIFEST: &[u8] =
    include_bytes!("../../../../tests/contracts/sdk_conformance/manifest-v1.json");
const SDK_CATALOG: &[u8] =
    include_bytes!("../../../../tests/contracts/sdk_conformance/sdk-v1/catalog-v1.json");
const SDK_JOURNEY: &[u8] =
    include_bytes!("../../../../tests/contracts/sdk_conformance/sdk-v1/journey-v1.json");
const SDK_V2_JOURNEY: &[u8] =
    include_bytes!("../../../../tests/contracts/sdk_conformance/sdk-v2/journey-v2.json");
const SDK_V3_SCHEMA: &str =
    include_str!("../../../../tests/contracts/sdk_conformance/sdk-v3/schema-v3.yaml");
const SDK_V3_PROVIDER_SCHEMA: &str =
    include_str!("../../../../tests/contracts/sdk_conformance/sdk-v3/provider-3.12.1-v3.tql");
const SDK_V3_CATALOG: &[u8] =
    include_bytes!("../../../../tests/contracts/sdk_conformance/sdk-v3/catalog-v3.json");
const SDK_V3_JOURNEY: &[u8] =
    include_bytes!("../../../../tests/contracts/sdk_conformance/sdk-v3/journey-v3.json");
const SDK_V2_CATALOG_RELATIVE: &str = "tests/contracts/sdk_conformance/sdk-v2/catalog-v2.json";
const SDK_PROFILE: &str = "typedb-3.12.1/v1";
const CONSUMER_TESTS: [&str; 12] = [
    "generated_sdk_report_journeys",
    "generated_schema_handshake_and_tokens",
    "generated_entity_crud_batches_and_scalar_domains",
    "generated_inheritance_exact_and_subtype_reads",
    "generated_relation_query_and_remote_lifecycle",
    "generated_integer_keys_and_polymorphic_role_parity",
    "generated_plain_inherited_abstract_role_parity",
    "generated_unkeyed_entity_iid_lifecycle_and_singular_query",
    "generated_lifecycle_hooks_and_atomic_mutation_batches",
    "generated_write_transaction_commit_rollback_and_drop",
    "generated_data_model_runtime_v3_live",
    "generated_canonical_serialization_v5_live",
];

struct Stage(PathBuf);

impl Stage {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time follows the Unix epoch")
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "type-bridge-rust-projection-live-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("live acceptance stage is created");
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

struct ServerProcess {
    child: Child,
    container_name: Option<String>,
    log: PathBuf,
}

impl ServerProcess {
    fn wait_until_ready(&mut self, port: u16) {
        let deadline = Instant::now() + Duration::from_secs(300);
        while Instant::now() < deadline {
            if let Some(status) = self
                .child
                .try_wait()
                .expect("V2 smoke server status is readable")
            {
                panic!(
                    "V2 smoke server exited early with {status}\n{}",
                    fs::read_to_string(&self.log).unwrap_or_default()
                );
            }
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return;
            }
            thread::sleep(Duration::from_millis(200));
        }
        panic!(
            "V2 smoke server did not become reachable\n{}",
            fs::read_to_string(&self.log).unwrap_or_default()
        );
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        if let Some(container_name) = self.container_name.take() {
            let _ = Command::new("docker")
                .args(["rm", "--force", &container_name])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn toml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(ALPHABET[(first >> 2) as usize] as char);
        output.push(ALPHABET[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            output.push(ALPHABET[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char);
        } else {
            output.push('=');
        }
        if chunk.len() > 2 {
            output.push(ALPHABET[(third & 0x3f) as usize] as char);
        } else {
            output.push('=');
        }
    }
    output
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .expect("loopback port allocation succeeds")
        .local_addr()
        .expect("loopback local address is readable")
        .port()
}

fn manifest_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "\\\\")
}

fn requested_sdk_report(profile_name: &str) -> Option<PathBuf> {
    let raw = env::var_os("TYPE_BRIDGE_SDK_REPORT")?;
    let raw = raw
        .into_string()
        .expect("TYPE_BRIDGE_SDK_REPORT must be UTF-8");
    assert!(
        raw.len() <= 4096,
        "TYPE_BRIDGE_SDK_REPORT exceeds 4096 UTF-8 bytes"
    );
    assert_eq!(
        profile_name, SDK_PROFILE,
        "sdk reports are frozen to {SDK_PROFILE}"
    );
    let path = PathBuf::from(raw);
    assert!(
        path.is_absolute(),
        "TYPE_BRIDGE_SDK_REPORT must be an absolute path"
    );
    let parent = path
        .parent()
        .expect("TYPE_BRIDGE_SDK_REPORT must have a parent directory");
    let metadata =
        fs::symlink_metadata(parent).expect("TYPE_BRIDGE_SDK_REPORT parent must already exist");
    assert!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "TYPE_BRIDGE_SDK_REPORT parent must be a non-symlink directory"
    );
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => panic!("TYPE_BRIDGE_SDK_REPORT target must not already exist"),
        Err(error) => panic!("TYPE_BRIDGE_SDK_REPORT target is not inspectable: {error}"),
    }
    Some(path)
}

fn requested_sdk_v2_report(profile_name: &str) -> Option<PathBuf> {
    let raw = env::var_os("TYPE_BRIDGE_SDK_REPORT_V2")?;
    let raw = raw
        .into_string()
        .expect("TYPE_BRIDGE_SDK_REPORT_V2 must be UTF-8");
    assert!(
        raw.len() <= 4096,
        "TYPE_BRIDGE_SDK_REPORT_V2 exceeds 4096 UTF-8 bytes"
    );
    assert_eq!(
        profile_name, SDK_PROFILE,
        "sdk-v2 reports are frozen to {SDK_PROFILE}"
    );
    let path = PathBuf::from(raw);
    assert!(
        path.is_absolute(),
        "TYPE_BRIDGE_SDK_REPORT_V2 must be an absolute path"
    );
    let parent = path
        .parent()
        .expect("TYPE_BRIDGE_SDK_REPORT_V2 must have a parent directory");
    let metadata =
        fs::symlink_metadata(parent).expect("TYPE_BRIDGE_SDK_REPORT_V2 parent must already exist");
    assert!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "TYPE_BRIDGE_SDK_REPORT_V2 parent must be a non-symlink directory"
    );
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => panic!("TYPE_BRIDGE_SDK_REPORT_V2 target must not already exist"),
        Err(error) => {
            panic!("TYPE_BRIDGE_SDK_REPORT_V2 target is not inspectable: {error}")
        }
    }
    Some(path)
}

fn requested_sdk_v3_supplement(profile_name: &str) -> Option<PathBuf> {
    let raw = env::var_os("TYPE_BRIDGE_SDK_V3_RUST_SUPPLEMENT")?;
    assert_eq!(profile_name, SDK_PROFILE);
    let path = PathBuf::from(raw);
    assert!(
        path.is_absolute(),
        "Sdk V3 supplement path must be absolute"
    );
    let parent = path.parent().expect("Sdk V3 supplement has a parent");
    let metadata =
        fs::symlink_metadata(parent).expect("Sdk V3 supplement parent must already exist");
    assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
    assert!(
        matches!(fs::symlink_metadata(&path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    );
    Some(path)
}

fn requested_sdk_v5_evidence(profile_name: &str) -> Option<PathBuf> {
    let raw = env::var_os("TYPE_BRIDGE_SDK_V5_RUST_EVIDENCE")?;
    assert_eq!(profile_name, SDK_PROFILE);
    let path = PathBuf::from(raw);
    assert!(path.is_absolute(), "Sdk V5 evidence path must be absolute");
    let parent = path.parent().expect("Sdk V5 evidence has a parent");
    let metadata = fs::symlink_metadata(parent).expect("Sdk V5 evidence parent must already exist");
    assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
    assert!(
        matches!(fs::symlink_metadata(&path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    );
    Some(path)
}

#[test]
fn external_consumer_remains_a_focused_public_api_suite() {
    fn consumer_test<'a>(source: &'a str, name: &str) -> &'a str {
        let marker = format!("#[tokio::test]\nasync fn {name}()");
        let start = source
            .find(&marker)
            .unwrap_or_else(|| panic!("external consumer test is missing: {name}"));
        let remaining = &source[start + marker.len()..];
        let end = remaining
            .find("\n#[tokio::test]")
            .unwrap_or(remaining.len());
        &remaining[..end]
    }

    assert!(!CONSUMER.contains("#[tokio::main]"));
    assert!(!CONSUMER.contains("fn run_sdk_v2_journey("));
    assert!(!CONSUMER.contains("Box::pin(run_sdk_v2_journey_inner(db))"));
    assert_eq!(
        CONSUMER.matches("#[tokio::test]").count(),
        CONSUMER_TESTS.len()
    );
    for test in CONSUMER_TESTS {
        assert!(
            CONSUMER.contains(&format!("async fn {test}()")),
            "external consumer test is missing: {test}"
        );
    }

    let relation = consumer_test(CONSUMER, "generated_relation_query_and_remote_lifecycle");
    assert!(relation.contains("F2C-03 public generated relation lifecycle: passed"));
    assert!(!relation.contains("TYPE_BRIDGE_SDK_REPORT"));
    assert!(!relation.contains("run_sdk_journey"));

    let reports = consumer_test(CONSUMER, "generated_sdk_report_journeys");
    for expression in [
        "env::var_os(\"TYPE_BRIDGE_SDK_REPORT\")",
        "env::var_os(\"TYPE_BRIDGE_SDK_REPORT_V2\")",
        "run_sdk_journey(&db).await",
        "run_sdk_v2_journey_inner(&db).await",
    ] {
        assert_eq!(
            reports.matches(expression).count(),
            1,
            "report test must consume exactly one {expression}"
        );
        assert_eq!(
            CONSUMER.matches(expression).count(),
            1,
            "report expression must occur in exactly one consumer test: {expression}"
        );
    }
    assert!(reports.contains("generated sdk report journeys: passed"));
}

#[test]
fn typedb_3_11_acceptance_is_the_same_application_without_3_12_docs() {
    fn strip_yaml_docs(source: &str) -> String {
        source
            .split_inclusive('\n')
            .map(|line| {
                let Some(start) = line.find(", doc: ") else {
                    return line.to_owned();
                };
                let end = line.rfind(" }").expect("inline YAML doc closes");
                format!("{}{}", &line[..start], &line[end..])
            })
            .collect()
    }

    fn strip_typeql_docs(source: &str) -> String {
        source
            .split_inclusive('\n')
            .map(|line| {
                let Some(start) = line.find(" @doc(\"") else {
                    return line.to_owned();
                };
                let relative_end = line[start..]
                    .find("\")")
                    .expect("TypeQL doc annotation closes");
                let end = start + relative_end + 2;
                format!("{}{}", &line[..start], &line[end..])
            })
            .collect()
    }

    assert_eq!(SCHEMA.matches(", doc: ").count(), 12);
    assert_eq!(PROVIDER_SCHEMA.matches(" @doc(\"").count(), 12);
    assert!(!SCHEMA_3_11.contains("doc:"));
    assert!(!PROVIDER_SCHEMA_3_11.contains("@doc"));
    assert_eq!(strip_yaml_docs(SCHEMA), SCHEMA_3_11);
    assert_eq!(strip_typeql_docs(PROVIDER_SCHEMA), PROVIDER_SCHEMA_3_11);
}

#[test]
#[ignore = "requires an isolated retained TypeDB server"]
fn generated_rust_projection_round_trips_exact_live_models() {
    let profile_name = env::var("TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE")
        .unwrap_or_else(|_| "typedb-3.12.1/v1".to_owned());
    let (schema, provider_schema) = match profile_name.as_str() {
        "typedb-3.11.5/v1" => (SCHEMA_3_11, PROVIDER_SCHEMA_3_11),
        "typedb-3.12.1/v1" => (SCHEMA, PROVIDER_SCHEMA),
        other => panic!("unsupported generated live semantic profile: {other}"),
    };
    let sdk_report = requested_sdk_report(&profile_name);
    let sdk_v2_report = requested_sdk_v2_report(&profile_name);
    let sdk_v3_supplement = requested_sdk_v3_supplement(&profile_name);
    let sdk_v5_evidence = requested_sdk_v5_evidence(&profile_name);
    assert!(
        (sdk_v3_supplement.is_none() && sdk_v5_evidence.is_none())
            || (sdk_report.is_none() && sdk_v2_report.is_none()),
        "Sdk V3/V5 use an isolated generated package and consumer run"
    );
    assert!(
        sdk_v3_supplement.is_none() || sdk_v5_evidence.is_none(),
        "Sdk V3 and V5 evidence use separate live runs"
    );
    let (schema, provider_schema) = if sdk_v3_supplement.is_some() || sdk_v5_evidence.is_some() {
        (SDK_V3_SCHEMA, SDK_V3_PROVIDER_SCHEMA)
    } else {
        (schema, provider_schema)
    };
    let sdk_v2_proof_fragments = sdk_v2_report.as_ref().map(|_| {
        let raw = env::var_os("TYPE_BRIDGE_SDK_V2_PROOF_FRAGMENTS")
            .expect("TYPE_BRIDGE_SDK_V2_PROOF_FRAGMENTS is required for a V2 report");
        support::sdk_v2_proof_paths(&raw)
            .expect("TYPE_BRIDGE_SDK_V2_PROOF_FRAGMENTS must be a valid path list")
    });
    let sdk_v2_proof_run_nonce = sdk_v2_report.as_ref().map(|_| {
        let nonce = env::var("TYPE_BRIDGE_SDK_V2_PROOF_RUN_NONCE")
            .expect("TYPE_BRIDGE_SDK_V2_PROOF_RUN_NONCE is required for a V2 report");
        assert!(
            nonce.len() == 64
                && nonce
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "TYPE_BRIDGE_SDK_V2_PROOF_RUN_NONCE must be 64 lowercase hexadecimal digits"
        );
        nonce
    });
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("schema-codegen lives beneath the repository root")
        .to_path_buf();
    let sdk_v2_validated_observations = sdk_v2_report.as_ref().map(|_| {
        let observations = support::validate_sdk_v2_proof_fragments(
            &repository_root,
            "rust",
            sdk_v2_proof_run_nonce
                .as_deref()
                .expect("sdk-v2 proof nonce was captured"),
            sdk_v2_proof_fragments
                .as_deref()
                .expect("sdk-v2 proof fragments were captured"),
        )
        .expect("Rust sdk-v2 proof fragments must validate");
        let serialized = observations
            .into_iter()
            .map(|((observation_ref, proof_kind), observation)| {
                (format!("{observation_ref}/{proof_kind}"), observation)
            })
            .collect::<std::collections::BTreeMap<String, Value>>();
        let encoded =
            serde_json::to_string(&serialized).expect("validated sdk-v2 observations serialize");
        assert!(
            encoded.len() <= 64 * 1024,
            "validated sdk-v2 observations exceed 64 KiB"
        );
        encoded
    });
    if let (Some(v1), Some(v2)) = (&sdk_report, &sdk_v2_report) {
        assert_ne!(v1, v2, "sdk-v1 and sdk-v2 reports require distinct paths");
    }
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("rust-projection-live.yaml").expect("document ID is valid"),
        schema,
    )])
    .expect("shared acceptance schema parses");
    let declared = normalize_documents(&documents).expect("acceptance schema normalizes");
    let profile = SemanticProfileId::new(&profile_name).expect("semantic profile is valid");
    let resolved = resolve(&declared, &profile).expect("acceptance schema resolves");
    let authority =
        support::authority_for_declared(&declared, "rust-projection-live", &profile_name);
    let authority_bytes = encode_schema_authority(&authority);
    let emitter = RustEmitter::new();
    let handlers = emitter.generator_handlers_for(&resolved);
    let resources = emitter
        .code_resources_for(&resolved)
        .expect("emitter resources hash");
    let projection = project(
        &resolved,
        BindingTarget::Rust,
        &ProjectionConfig::rust(),
        &handlers,
        &resources,
    )
    .expect("acceptance schema projects to Rust");
    let package = emitter
        .emit(&projection, &authority)
        .expect("Rust package emits");

    let stage = Stage::new();
    let sdk_files = sdk_report.as_ref().map(|_| {
        let directory = stage.path().join("sdk-v1");
        fs::create_dir_all(&directory).expect("sdk contract stage is created");
        let manifest = directory.join("manifest-v1.json");
        let catalog = directory.join("catalog-v1.json");
        let journey = directory.join("journey-v1.json");
        let schema = directory.join("schema.yaml");
        let provider_schema = directory.join("provider-3.12.1.tql");
        fs::write(&manifest, SDK_MANIFEST).expect("sdk manifest is staged");
        fs::write(&catalog, SDK_CATALOG).expect("sdk catalog is staged");
        fs::write(&journey, SDK_JOURNEY).expect("sdk journey is staged");
        fs::write(&schema, SCHEMA.as_bytes()).expect("sdk schema is staged");
        fs::write(&provider_schema, PROVIDER_SCHEMA.as_bytes())
            .expect("sdk provider schema is staged");
        (manifest, catalog, journey, schema, provider_schema)
    });
    let sdk_v2_files = sdk_v2_report.as_ref().map(|_| {
        let directory = stage.path().join("sdk-v2");
        fs::create_dir_all(&directory).expect("sdk-v2 contract stage is created");
        let manifest = directory.join("manifest-v1.json");
        let catalog = directory.join("catalog-v2.json");
        let journey = directory.join("journey-v2.json");
        let schema = directory.join("schema.yaml");
        let provider_schema = directory.join("provider-3.12.1.tql");
        let catalog_source = repository_root.join(SDK_V2_CATALOG_RELATIVE);
        let catalog_bytes = fs::read(&catalog_source).unwrap_or_else(|error| {
            panic!(
                "requested sdk-v2 report requires the frozen catalog at {}: {error}",
                catalog_source.display()
            )
        });
        fs::write(&manifest, SDK_MANIFEST).expect("sdk-v2 manifest is staged");
        fs::write(&catalog, catalog_bytes).expect("sdk-v2 catalog is staged");
        fs::write(&journey, SDK_V2_JOURNEY).expect("sdk-v2 journey is staged");
        fs::write(&schema, SCHEMA.as_bytes()).expect("sdk-v2 schema is staged");
        fs::write(&provider_schema, PROVIDER_SCHEMA.as_bytes())
            .expect("sdk-v2 provider schema is staged");
        (manifest, catalog, journey, schema, provider_schema)
    });
    let sdk_v3_files = sdk_v3_supplement.as_ref().map(|_| {
        let directory = stage.path().join("sdk-v3");
        fs::create_dir_all(&directory).expect("sdk-v3 contract stage is created");
        let catalog = directory.join("catalog-v3.json");
        let journey = directory.join("journey-v3.json");
        fs::write(&catalog, SDK_V3_CATALOG).expect("sdk-v3 catalog is staged");
        fs::write(&journey, SDK_V3_JOURNEY).expect("sdk-v3 journey is staged");
        (catalog, journey)
    });
    let generated = stage.path().join("generated");
    for (relative, bytes) in package.files() {
        let path = generated.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("generated parent directory is created");
        }
        fs::write(path, bytes).expect("generated file is written");
    }

    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let crates_dir = crate_dir
        .parent()
        .expect("schema-codegen has a crates parent");
    let target_dir = env::var_os("ACCEPTANCE_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| stage.path().join("target"));

    // Public consumer is staged, scanned, and compiled before either live subprocess.
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let consumer = stage.path().join("consumer");
    fs::create_dir_all(consumer.join("src")).expect("consumer staging directory is created");
    fs::write(consumer.join("src/lib.rs"), CONSUMER).expect("consumer source is staged");
    let preflight_manifest = format!(
        "[package]\nname=\"type-bridge-rust-projection-live-consumer\"\nversion=\"0.0.0\"\nedition=\"2024\"\npublish=false\n[dependencies]\ntype-bridge-generated-schema={{path=\"{}\"}}\ntype-bridge={{path=\"{}\"}}\ntokio={{version=\"1\",features=[\"macros\",\"rt-multi-thread\"]}}\nreqwest={{version=\"0.12\",default-features=false,features=[\"rustls-tls\"]}}\nserde_json=\"1\"\nsha2=\"0.10\"\n[patch.crates-io]\ntype-bridge={{path=\"{}\"}}\n[workspace]\n",
        manifest_path(&generated),
        manifest_path(&crates_dir.join("rust")),
        manifest_path(&crates_dir.join("rust"))
    );
    let consumer_manifest_path = consumer.join("Cargo.toml");
    fs::write(&consumer_manifest_path, preflight_manifest).expect("consumer manifest is staged");
    fs::write(consumer.join("Cargo.lock"), CONSUMER_LOCK).expect("consumer lockfile is staged");
    let generated_manifest =
        fs::read_to_string(generated.join("Cargo.toml")).expect("generated manifest is readable");
    let generated_deps = generated_manifest
        .split_once("[dependencies]")
        .unwrap()
        .1
        .split_once("[features]")
        .unwrap()
        .0
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    assert_eq!(generated_deps.len(), 1);
    assert!(
        generated_deps[0].trim_start().starts_with("type-bridge =")
            && generated_deps[0].contains("default-features = false")
    );
    let consumer_manifest_text =
        fs::read_to_string(&consumer_manifest_path).expect("consumer manifest readable");
    let consumer_deps = consumer_manifest_text
        .split_once("[dependencies]")
        .unwrap()
        .1
        .split_once("[patch.crates-io]")
        .unwrap()
        .0
        .lines()
        .filter_map(|line| line.split_once('=').map(|(key, _)| key.trim().to_owned()))
        .collect::<Vec<_>>();
    assert_eq!(
        consumer_deps,
        vec![
            "type-bridge-generated-schema",
            "type-bridge",
            "tokio",
            "reqwest",
            "serde_json",
            "sha2"
        ]
    );
    assert!(!consumer_manifest_text.contains("test-harness"));
    let consumer_source =
        fs::read_to_string(consumer.join("src/lib.rs")).expect("consumer source is readable");
    for forbidden in [
        "Dynamic",
        "AttributeValue",
        "HydratedRow",
        "HydrationCapability",
        "MaterializationCapability",
        "materialize_model",
        "TransactionContext",
        "type_bridge_orm",
        "type_bridge_contract",
        "type_bridge_schema",
        "type_bridge_query",
        "type_bridge_codegen",
        "type_bridge_provider",
        "type_bridge_driver",
        "type_bridge_transaction",
        "typedb_driver",
        "TypeQL",
        "execute_raw",
        "descriptor",
        "projection_descriptor",
        "InstalledRuntimeProjection",
        "RuntimeProjection",
        "runtime_projection",
        "projection_for",
        "match $",
        "insert $",
        "delete $",
        "test-harness",
        "test_harness",
    ] {
        assert!(
            !consumer_source.contains(forbidden),
            "forbidden consumer surface: {forbidden}"
        );
    }
    // The frozen consumer graph can contain versions absent from the workspace cache.
    // Fetch that exact graph before requiring the consumer compilation to be offline.
    let consumer_fetch = Command::new(&cargo)
        .args(["fetch", "--locked", "--manifest-path"])
        .arg(&consumer_manifest_path)
        .output()
        .expect("locked consumer dependency fetch starts");
    assert!(
        consumer_fetch.status.success(),
        "locked consumer dependency fetch failed\n{}",
        String::from_utf8_lossy(&consumer_fetch.stderr)
    );
    let consumer_check = Command::new(&cargo)
        .args([
            "check",
            "--tests",
            "--locked",
            "--offline",
            "--manifest-path",
        ])
        .arg(&consumer_manifest_path)
        .env("CARGO_TARGET_DIR", &target_dir)
        .output()
        .expect("preflight consumer check starts");
    assert!(
        consumer_check.status.success(),
        "preflight consumer check failed\n{}",
        String::from_utf8_lossy(&consumer_check.stderr)
    );
    if env::var("TYPE_BRIDGE_RUST_PROJECTION_PREFLIGHT_ONLY").as_deref() == Ok("1") {
        println!("generated Rust public consumer preflight: passed");
        return;
    }

    // 1. Prepare the provider schema through the retained raw execution seam.
    // Application CRUD/query evidence comes only from the generated consumer below.
    let fixture = stage.path().join("internal_fixture");
    fs::create_dir_all(fixture.join("src")).expect("fixture source directory is created");
    fs::write(fixture.join("src/main.rs"), INTERNAL_FIXTURE).expect("fixture source is written");
    fs::write(fixture.join("src/provider.tql"), provider_schema)
        .expect("provider fixture is written");

    let fixture_manifest = format!(
        r#"[package]
name = "type-bridge-test-provider"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
type-bridge-orm = {{ path = "{}" }}
tokio = {{ version = "1", features = ["macros", "rt-multi-thread"] }}

[workspace]
"#,
        manifest_path(&crates_dir.join("orm")),
    );
    let fixture_manifest_path = fixture.join("Cargo.toml");
    fs::write(&fixture_manifest_path, fixture_manifest).expect("fixture manifest is written");
    fs::write(fixture.join("Cargo.lock"), FIXTURE_LOCK).expect("fixture lockfile is staged");

    let fixture_output = Command::new(&cargo)
        .arg("run")
        .arg("--locked")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(&fixture_manifest_path)
        .env("CARGO_TARGET_DIR", &target_dir)
        .env("TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE", &profile_name)
        .output()
        .expect("provider schema setup starts");

    assert!(
        fixture_output.status.success(),
        "provider schema setup failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&fixture_output.stdout),
        String::from_utf8_lossy(&fixture_output.stderr),
    );
    assert!(
        String::from_utf8_lossy(&fixture_output.stdout)
            .contains("generated provider schema setup: passed")
    );

    let server_port = free_port();
    let server_log_path = stage.path().join("v2-smoke-server.log");
    let server_log = File::create(&server_log_path).expect("V2 server log is created");
    let server_error_log = server_log
        .try_clone()
        .expect("V2 server log handle is cloned");
    let core_dir = crates_dir.parent().expect("crates has a core parent");
    let image = env::var("TYPE_BRIDGE_SERVER_IMAGE").ok();
    let container_name = image
        .as_ref()
        .map(|_| format!("type-bridge-rust-projection-server-{}", std::process::id()));
    let mut server_command = if let Some(image) = image {
        assert_ne!(
            env::var("TYPE_BRIDGE_RUST_PROJECTION_TLS").as_deref(),
            Ok("1"),
            "exact production-image generated parity currently uses the plain isolated lane"
        );
        let authority_path = stage.path().join("schema-authority.json");
        fs::write(&authority_path, &authority_bytes).expect("schema authority is staged");
        let config_path = stage.path().join("server.toml");
        let address = env::var("TYPEDB_ADDRESS").expect("TYPEDB_ADDRESS is configured");
        let username = env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned());
        let password = env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned());
        let http_port = env::var("TYPEDB_HTTP_PORT").expect("TYPEDB_HTTP_PORT is configured");
        let database = env::var("TYPE_BRIDGE_RUST_PROJECTION_INTG_DATABASE")
            .expect("live database name is configured");
        fs::write(
            &config_path,
            format!(
                "[server]\nhost = \"127.0.0.1\"\nport = {server_port}\n\
                 [typedb]\naddress = {}\ndatabase = {}\nusername = {}\npassword = {}\n\
                 http_port = {http_port}\ntls = false\n\
                 [logging]\nlevel = \"info\"\nformat = \"text\"\n\
                 [v2]\nenabled = true\nschema_authority_file = {}\n\
                 authority_mode = \"query_only\"\n",
                toml_string(&address),
                toml_string(&database),
                toml_string(&username),
                toml_string(&password),
                toml_string(
                    authority_path
                        .to_str()
                        .expect("schema-authority path is UTF-8")
                ),
            ),
        )
        .expect("production server config is staged");
        let mount = format!(
            "type=bind,src={},dst={},readonly",
            stage.path().display(),
            stage.path().display()
        );
        let mut command = Command::new("docker");
        command.args([
            "run",
            "--rm",
            "--name",
            container_name
                .as_deref()
                .expect("container image always has a name"),
            "--network",
            "host",
            "--read-only",
            "--cap-drop",
            "ALL",
            "--security-opt",
            "no-new-privileges:true",
            "--user",
            "10001:10001",
            "--mount",
            &mount,
        ]);
        if let Ok(platform) = env::var("TYPE_BRIDGE_SERVER_PLATFORM") {
            command.args(["--platform", &platform]);
        }
        command.arg(image).arg("--config").arg(config_path);
        command
    } else {
        let mut command = Command::new(&cargo);
        command
            .args([
                "run",
                "--quiet",
                "-p",
                "type-bridge-server",
                "--features",
                "v2-query",
                "--example",
                "v2_smoke_server",
            ])
            .current_dir(core_dir)
            .env("CARGO_TARGET_DIR", &target_dir)
            .env(
                "SMOKE_TYPEDB_ADDRESS",
                env::var("TYPEDB_ADDRESS").expect("TYPEDB_ADDRESS is configured"),
            )
            .env(
                "SMOKE_TYPEDB_USERNAME",
                env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned()),
            )
            .env(
                "SMOKE_TYPEDB_PASSWORD",
                env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned()),
            )
            .env(
                "SMOKE_TYPEDB_HTTP_PORT",
                env::var("TYPEDB_HTTP_PORT").expect("TYPEDB_HTTP_PORT is configured"),
            )
            .env(
                "SMOKE_DATABASE",
                env::var("TYPE_BRIDGE_RUST_PROJECTION_INTG_DATABASE")
                    .expect("live database name is configured"),
            )
            .env("SMOKE_AUTHORITY_B64", base64(&authority_bytes))
            .env("SMOKE_PORT", server_port.to_string());
        if env::var("TYPE_BRIDGE_RUST_PROJECTION_TLS").as_deref() == Ok("1") {
            command.env("SMOKE_TYPEDB_TLS", "true").env(
                "SMOKE_TYPEDB_TLS_ROOT_CA",
                env::var_os("TYPEDB_TLS_ROOT_CA")
                    .expect("TYPEDB_TLS_ROOT_CA is configured for generated Rust TLS"),
            );
        }
        command
    };
    server_command
        .stdout(Stdio::from(server_log))
        .stderr(Stdio::from(server_error_log));
    let mut server = ServerProcess {
        child: server_command.spawn().expect("V2 smoke server starts"),
        container_name,
        log: server_log_path,
    };
    server.wait_until_ready(server_port);

    let mut consumer_command = Command::new(&cargo);
    consumer_command
        .arg("test")
        .arg("--locked")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(&consumer_manifest_path)
        .args(["--", "--test-threads=1", "--nocapture"])
        .env("CARGO_TARGET_DIR", &target_dir)
        .env(
            "TYPE_BRIDGE_REMOTE_URL",
            format!("http://127.0.0.1:{server_port}"),
        )
        .env("TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE", &profile_name)
        .env_remove("TYPE_BRIDGE_SDK_V2_PROOF_FRAGMENT")
        .env_remove("TYPE_BRIDGE_SDK_V2_PROOF_FRAGMENTS")
        .env_remove("TYPE_BRIDGE_SDK_V2_PROOF_RUN_NONCE")
        .env_remove("TYPE_BRIDGE_SDK_V2_VALIDATED_OBSERVATIONS");
    if sdk_v3_supplement.is_some() {
        consumer_command.arg("generated_data_model_runtime_v3_live");
    } else if sdk_v5_evidence.is_some() {
        consumer_command.arg("generated_canonical_serialization_v5_live");
    }
    if let (Some(report), Some((manifest, catalog, journey, schema, provider_schema))) =
        (&sdk_report, &sdk_files)
    {
        consumer_command
            .env("TYPE_BRIDGE_SDK_REPORT", report)
            .env("TYPE_BRIDGE_SDK_MANIFEST", manifest)
            .env("TYPE_BRIDGE_SDK_CATALOG", catalog)
            .env("TYPE_BRIDGE_SDK_JOURNEY", journey)
            .env("TYPE_BRIDGE_SDK_SCHEMA", schema)
            .env("TYPE_BRIDGE_SDK_PROVIDER_SCHEMA", provider_schema);
    }
    if let (Some(supplement), Some((catalog, journey))) = (&sdk_v3_supplement, &sdk_v3_files) {
        consumer_command
            .env("TYPE_BRIDGE_SDK_V3_RUST_SUPPLEMENT", supplement)
            .env("TYPE_BRIDGE_SDK_V3_CATALOG", catalog)
            .env("TYPE_BRIDGE_SDK_V3_JOURNEY", journey);
    }
    if let Some(evidence) = &sdk_v5_evidence {
        consumer_command.env("TYPE_BRIDGE_SDK_V5_RUST_EVIDENCE", evidence);
    }
    if let (Some(report), Some((manifest, catalog, journey, schema, provider_schema))) =
        (&sdk_v2_report, &sdk_v2_files)
    {
        consumer_command
            .env("TYPE_BRIDGE_SDK_REPORT_V2", report)
            .env("TYPE_BRIDGE_SDK_MANIFEST_V2", manifest)
            .env("TYPE_BRIDGE_SDK_CATALOG_V2", catalog)
            .env("TYPE_BRIDGE_SDK_JOURNEY_V2", journey)
            .env("TYPE_BRIDGE_SDK_SCHEMA_V2", schema)
            .env("TYPE_BRIDGE_SDK_PROVIDER_SCHEMA_V2", provider_schema)
            .env(
                "TYPE_BRIDGE_SDK_V2_VALIDATED_OBSERVATIONS",
                sdk_v2_validated_observations
                    .as_ref()
                    .expect("sdk-v2 observations were validated"),
            );
    }
    let consumer_output = consumer_command
        .output()
        .expect("dependency-isolated client consumer starts");

    assert!(
        consumer_output.status.success(),
        "dependency-isolated client consumer failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&consumer_output.stdout),
        String::from_utf8_lossy(&consumer_output.stderr),
    );
    let consumer_stdout = String::from_utf8_lossy(&consumer_output.stdout);
    let expected_consumer_tests = if sdk_v3_supplement.is_some() || sdk_v5_evidence.is_some() {
        1
    } else {
        CONSUMER_TESTS.len()
    };
    assert!(consumer_stdout.contains(&format!(
        "test result: ok. {expected_consumer_tests} passed; 0 failed"
    )));
    if sdk_v3_supplement.is_some() {
        assert!(consumer_stdout.contains("generated Sdk V3 Rust live supplement: passed"));
        assert!(
            sdk_v3_supplement
                .as_ref()
                .is_some_and(|path| path.is_file())
        );
        return;
    }
    if let Some(evidence_path) = &sdk_v5_evidence {
        assert!(consumer_stdout.contains("generated Sdk V5 Rust live evidence: passed"));
        assert!(evidence_path.is_file());
        let evidence: Value = serde_json::from_slice(
            fs::read(evidence_path)
                .expect("V5 evidence reads")
                .strip_suffix(b"\n")
                .expect("V5 evidence ends in one LF"),
        )
        .expect("V5 evidence parses");
        assert_eq!(
            evidence["format"],
            "typebridge.sdk-v5-live-codec-evidence/v1"
        );
        assert_eq!(evidence["binding"], "rust");
        assert_eq!(evidence["direct_remote_equal"], true);
        assert_eq!(
            evidence["detached_mutation_code"],
            "projected_snapshot_detached"
        );
        assert_eq!(evidence["remote_exchange_count"], 1);
        assert_eq!(evidence["rebound_mutation"], true);
        for field in ["entity_snapshot_b64", "relation_snapshot_b64"] {
            assert!(
                evidence[field]
                    .as_str()
                    .is_some_and(|value| !value.is_empty())
            );
        }
        return;
    }
    assert!(consumer_stdout.contains("public generated schema handshake and tokens: passed"));
    assert!(
        consumer_stdout
            .contains("public generated entity CRUD, batches, and scalar domains: passed")
    );
    assert!(consumer_stdout.contains("F2B-03 public generated entity lifecycle: passed"));
    assert!(consumer_stdout.contains("F2C-03 public generated relation lifecycle: passed"));
    assert!(consumer_stdout.contains("generated sdk report journeys: passed"));
    assert!(consumer_stdout.contains("F2D public write transaction lifecycle: passed"));
    assert!(
        consumer_stdout.contains("generated lifecycle hooks and atomic mutation batches: passed")
    );
    assert!(consumer_stdout.contains("F3 public generated query lifecycle: passed"));
    assert!(consumer_stdout.contains("F4 public selected/read/remote lifecycle: passed"));
    assert!(consumer_stdout.contains("F5 public relation parity and bounded reachability: passed"));
    assert!(consumer_stdout.contains("generated integer keys and polymorphic role parity: passed"));
    assert!(consumer_stdout.contains("generated plain-inherited abstract role parity: passed"));
    assert!(
        consumer_stdout
            .contains("generated unkeyed entity IID lifecycle and singular query: passed")
    );
    if let Some(report) = &sdk_report {
        assert!(
            report.is_file(),
            "requested generated Rust sdk report was not produced: {}",
            report.display()
        );
        let report_bytes = fs::read(report).expect("generated Rust report is readable");
        assert_eq!(
            report_bytes.last(),
            Some(&b'\n'),
            "generated Rust report must end in exactly one LF"
        );
        let canonical_bytes = &report_bytes[..report_bytes.len() - 1];
        assert!(
            !canonical_bytes.ends_with(b"\n"),
            "generated Rust report must end in exactly one LF"
        );
        let report_json: serde_json::Value =
            type_bridge_contract::codec::from_canonical_json(canonical_bytes)
                .expect("generated Rust report is compact canonical JSON");
        assert_eq!(report_json["binding"], "rust");
        assert_eq!(report_json["fixture"]["projection_target"], "rust");
        assert_eq!(
            report_json["results"]
                .as_array()
                .expect("generated Rust report results are an array")
                .len(),
            9
        );
        fn assert_no_runtime_identity(value: &serde_json::Value) {
            match value {
                serde_json::Value::Array(values) => {
                    for value in values {
                        assert_no_runtime_identity(value);
                    }
                }
                serde_json::Value::Object(values) => {
                    for (key, value) in values {
                        assert!(
                            !matches!(
                                key.as_str(),
                                "iid" | "database" | "address" | "port" | "runtime_identity"
                            ),
                            "generated Rust report leaked provider/runtime identity: {key}"
                        );
                        assert_no_runtime_identity(value);
                    }
                }
                _ => {}
            }
        }
        assert_no_runtime_identity(&report_json);
        assert!(
            fs::symlink_metadata(report)
                .expect("generated Rust report metadata is readable")
                .is_file(),
            "generated Rust report is not a regular file"
        );
    }
    if let Some(report) = &sdk_v2_report {
        assert!(
            report.is_file(),
            "requested generated Rust sdk-v2 report was not produced: {}",
            report.display()
        );
        let report_bytes = fs::read(report).expect("generated Rust sdk-v2 report is readable");
        assert_eq!(
            report_bytes.last(),
            Some(&b'\n'),
            "generated Rust sdk-v2 report must end in exactly one LF"
        );
        let canonical_bytes = &report_bytes[..report_bytes.len() - 1];
        assert!(
            !canonical_bytes.ends_with(b"\n"),
            "generated Rust sdk-v2 report must end in exactly one LF"
        );
        let report_json: serde_json::Value =
            type_bridge_contract::codec::from_canonical_json(canonical_bytes)
                .expect("generated Rust sdk-v2 report is compact canonical JSON");
        assert_eq!(
            report_json["format"],
            "typebridge.sdk-conformance-report/v2"
        );
        assert_eq!(report_json["binding"], "rust");
        assert_eq!(report_json["fixture"]["projection_target"], "rust");
        assert_eq!(
            report_json["results"]
                .as_array()
                .expect("generated Rust sdk-v2 report results are an array")
                .len(),
            34
        );
        fn assert_no_v2_runtime_identity(value: &serde_json::Value) {
            match value {
                serde_json::Value::Array(values) => {
                    for value in values {
                        assert_no_v2_runtime_identity(value);
                    }
                }
                serde_json::Value::Object(values) => {
                    for (key, value) in values {
                        assert!(
                            !matches!(
                                key.as_str(),
                                "iid" | "database" | "address" | "port" | "runtime_identity"
                            ),
                            "generated Rust sdk-v2 report leaked provider/runtime identity: {key}"
                        );
                        assert_no_v2_runtime_identity(value);
                    }
                }
                _ => {}
            }
        }
        assert_no_v2_runtime_identity(&report_json);
        assert!(
            fs::symlink_metadata(report)
                .expect("generated Rust sdk-v2 report metadata is readable")
                .is_file(),
            "generated Rust sdk-v2 report is not a regular file"
        );
    }
    println!("generated sdk report journeys: passed");
    println!("F2B-03 public generated entity lifecycle: passed");
    println!("F2C-03 public generated relation lifecycle: passed");
    println!("F2D public write transaction lifecycle: passed");
    println!("generated lifecycle hooks and atomic mutation batches: passed");
    println!("F3 public generated query lifecycle: passed");
    println!("F4 public selected/read/remote lifecycle: passed");
    println!("F5 public relation parity and bounded reachability: passed");
    println!("generated integer keys and polymorphic role parity: passed");
    println!("generated plain-inherited abstract role parity: passed");
    println!("generated unkeyed entity IID lifecycle and singular query: passed");
}

#[test]
fn generated_rust_projection_dependency_graphs_are_frozen() {
    let consumer = std::str::from_utf8(CONSUMER_LOCK).expect("consumer lockfile is UTF-8");
    assert_eq!(
        consumer
            .matches("name = \"type-bridge-rust-projection-live-consumer\"")
            .count(),
        1
    );
    assert!(consumer.contains("name = \"tinyvec\"\nversion = \"1.12.0\""));
    assert!(!consumer.contains("name = \"tinyvec\"\nversion = \"1.13.0\""));

    let fixture = std::str::from_utf8(FIXTURE_LOCK).expect("fixture lockfile is UTF-8");
    assert_eq!(
        fixture
            .matches("name = \"type-bridge-test-provider\"")
            .count(),
        1
    );
    assert!(fixture.contains("name = \"tinyvec\"\nversion = \"1.12.0\""));
    assert!(!fixture.contains("name = \"tinyvec\"\nversion = \"1.13.0\""));
}
