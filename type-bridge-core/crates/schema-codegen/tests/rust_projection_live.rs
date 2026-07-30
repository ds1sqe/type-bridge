use std::env;
use std::fs;
use std::fs::File;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig};
use type_bridge_contract::schema::{DocumentId, encode_declared_schema};
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
use type_bridge_schema_codegen::RustEmitter;

const SCHEMA: &str = include_str!("acceptance/schema.yaml");
const PROVIDER_SCHEMA: &str = include_str!("acceptance/provider-3.12.1.tql");
const INTERNAL_FIXTURE: &str = include_str!("rust_projection_live/internal_fixture.rs");
const CONSUMER: &str = include_str!("rust_projection_live/consumer.rs");
const CONSUMER_TESTS: [&str; 5] = [
    "generated_schema_handshake_and_tokens",
    "generated_entity_crud_batches_and_scalar_domains",
    "generated_inheritance_exact_and_subtype_reads",
    "generated_relation_query_and_remote_lifecycle",
    "generated_write_transaction_commit_rollback_and_drop",
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

#[test]
fn external_consumer_remains_a_focused_public_api_suite() {
    assert!(!CONSUMER.contains("#[tokio::main]"));
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
}

#[test]
#[ignore = "requires isolated TypeDB 3.12.1"]
fn generated_rust_projection_round_trips_exact_live_models() {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("rust-projection-live.yaml").expect("document ID is valid"),
        SCHEMA,
    )])
    .expect("shared acceptance schema parses");
    let declared = normalize_documents(&documents).expect("acceptance schema normalizes");
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").expect("semantic profile is valid");
    let resolved = resolve(&declared, &profile).expect("acceptance schema resolves");
    let emitter = RustEmitter::new();
    let handlers = emitter.generator_handlers();
    let resources = emitter.code_resources().expect("emitter resources hash");
    let projection = project(
        &resolved,
        BindingTarget::Rust,
        &ProjectionConfig::rust(),
        &handlers,
        &resources,
    )
    .expect("acceptance schema projects to Rust");
    let package = emitter
        .emit_with_declared_schema(&projection, &declared)
        .expect("Rust package emits");

    let stage = Stage::new();
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
        "[package]\nname=\"type-bridge-rust-projection-live-consumer\"\nversion=\"0.0.0\"\nedition=\"2024\"\npublish=false\n[dependencies]\ntype-bridge-generated-schema={{path=\"{}\"}}\ntype-bridge={{path=\"{}\"}}\ntokio={{version=\"1\",features=[\"macros\",\"rt-multi-thread\"]}}\nreqwest={{version=\"0.12\",default-features=false,features=[\"rustls-tls\"]}}\n[patch.crates-io]\ntype-bridge={{path=\"{}\"}}\n[workspace]\n",
        manifest_path(&generated),
        manifest_path(&crates_dir.join("rust")),
        manifest_path(&crates_dir.join("rust"))
    );
    let consumer_manifest_path = consumer.join("Cargo.toml");
    fs::write(&consumer_manifest_path, preflight_manifest).expect("consumer manifest is staged");
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
            "reqwest"
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
    let consumer_check = Command::new(&cargo)
        .args(["check", "--tests", "--offline", "--manifest-path"])
        .arg(&consumer_manifest_path)
        .env("CARGO_TARGET_DIR", &target_dir)
        .output()
        .expect("preflight consumer check starts");
    assert!(
        consumer_check.status.success(),
        "preflight consumer check failed\n{}",
        String::from_utf8_lossy(&consumer_check.stderr)
    );

    // 1. Run internal engine/projection fixture for dynamic map CRUD live coverage
    let fixture = stage.path().join("internal_fixture");
    fs::create_dir_all(fixture.join("src")).expect("fixture source directory is created");
    fs::write(fixture.join("src/main.rs"), INTERNAL_FIXTURE).expect("fixture source is written");
    fs::write(fixture.join("src/provider-3.12.1.tql"), PROVIDER_SCHEMA)
        .expect("provider fixture is written");

    let fixture_manifest = format!(
        r#"[package]
name = "type-bridge-rust-projection-live-fixture"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
type-bridge-generated-schema = {{ path = "{}", features = ["test-harness"] }}
type-bridge-contract = {{ path = "{}" }}
type-bridge-orm = {{ path = "{}" }}
tokio = {{ version = "1", features = ["macros", "rt-multi-thread"] }}

[patch.crates-io]
type-bridge = {{ path = "{}" }}

[workspace]
"#,
        manifest_path(&generated),
        manifest_path(&crates_dir.join("contract")),
        manifest_path(&crates_dir.join("orm")),
        manifest_path(&crates_dir.join("rust")),
    );
    let fixture_manifest_path = fixture.join("Cargo.toml");
    fs::write(&fixture_manifest_path, fixture_manifest).expect("fixture manifest is written");

    let fixture_output = Command::new(&cargo)
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(&fixture_manifest_path)
        .env("CARGO_TARGET_DIR", &target_dir)
        .output()
        .expect("internal projection fixture starts");

    assert!(
        fixture_output.status.success(),
        "internal projection fixture failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&fixture_output.stdout),
        String::from_utf8_lossy(&fixture_output.stderr),
    );
    assert!(
        String::from_utf8_lossy(&fixture_output.stdout)
            .contains("F2B-03 internal dynamic regression: passed")
    );
    assert!(
        String::from_utf8_lossy(&fixture_output.stdout)
            .contains("TypeDB 3.12 annotation export: passed")
    );
    println!("F2B-03 internal dynamic regression: passed");

    let server_port = free_port();
    let server_log_path = stage.path().join("v2-smoke-server.log");
    let server_log = File::create(&server_log_path).expect("V2 server log is created");
    let server_error_log = server_log
        .try_clone()
        .expect("V2 server log handle is cloned");
    let core_dir = crates_dir.parent().expect("crates has a core parent");
    let declared_bytes =
        encode_declared_schema(&declared).expect("declared schema encodes canonically");
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
        let declared_path = stage.path().join("declared-schema.json");
        fs::write(&declared_path, &declared_bytes).expect("declared schema is staged");
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
                 [v2]\nenabled = true\ndeclared_schema_file = {}\n\
                 scope = \"rust-projection-live\"\nprofile = \"typedb-3.12.1/v1\"\n\
                 authority_mode = \"query_only\"\n",
                toml_string(&address),
                toml_string(&database),
                toml_string(&username),
                toml_string(&password),
                toml_string(
                    declared_path
                        .to_str()
                        .expect("declared-schema path is UTF-8")
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
            .env("SMOKE_DECLARED_B64", base64(&declared_bytes))
            .env("SMOKE_SCOPE", "rust-projection-live")
            .env("SMOKE_PROFILE", "typedb-3.12.1/v1")
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

    let consumer_output = Command::new(&cargo)
        .arg("test")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(&consumer_manifest_path)
        .args(["--", "--test-threads=1", "--nocapture"])
        .env("CARGO_TARGET_DIR", &target_dir)
        .env(
            "TYPE_BRIDGE_REMOTE_URL",
            format!("http://127.0.0.1:{server_port}"),
        )
        .output()
        .expect("dependency-isolated client consumer starts");

    assert!(
        consumer_output.status.success(),
        "dependency-isolated client consumer failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&consumer_output.stdout),
        String::from_utf8_lossy(&consumer_output.stderr),
    );
    let consumer_stdout = String::from_utf8_lossy(&consumer_output.stdout);
    assert!(consumer_stdout.contains("test result: ok. 5 passed; 0 failed"));
    assert!(consumer_stdout.contains("public generated schema handshake and tokens: passed"));
    assert!(
        consumer_stdout
            .contains("public generated entity CRUD, batches, and scalar domains: passed")
    );
    assert!(consumer_stdout.contains("F2B-03 public generated entity lifecycle: passed"));
    assert!(consumer_stdout.contains("F2C-03 public generated relation lifecycle: passed"));
    assert!(consumer_stdout.contains("F2D public write transaction lifecycle: passed"));
    assert!(consumer_stdout.contains("F3 public generated query lifecycle: passed"));
    assert!(consumer_stdout.contains("F4 public selected/read/remote lifecycle: passed"));
    assert!(consumer_stdout.contains("F5 public relation parity and bounded reachability: passed"));
    println!("F2B-03 public generated entity lifecycle: passed");
    println!("F2C-03 public generated relation lifecycle: passed");
    println!("F2D public write transaction lifecycle: passed");
    println!("F3 public generated query lifecycle: passed");
    println!("F4 public selected/read/remote lifecycle: passed");
    println!("F5 public relation parity and bounded reachability: passed");
}
