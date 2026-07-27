use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig};
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
use type_bridge_schema_codegen::RustEmitter;

const SCHEMA: &str = include_str!("acceptance/schema.yaml");
const PROVIDER_SCHEMA: &str = include_str!("acceptance/provider-3.12.1.tql");
const CONSUMER: &str = include_str!("rust_projection_live/consumer.rs");

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

fn manifest_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "\\\\")
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
    let package = emitter.emit(&projection).expect("Rust package emits");

    let stage = Stage::new();
    let generated = stage.path().join("generated");
    for (relative, bytes) in package.files() {
        let path = generated.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("generated parent directory is created");
        }
        fs::write(path, bytes).expect("generated file is written");
    }

    let consumer = stage.path().join("consumer");
    fs::create_dir_all(consumer.join("src")).expect("consumer source directory is created");
    fs::write(consumer.join("src/main.rs"), CONSUMER).expect("consumer source is written");
    fs::write(consumer.join("src/provider-3.12.1.tql"), PROVIDER_SCHEMA)
        .expect("provider fixture is written");

    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let crates_dir = crate_dir
        .parent()
        .expect("schema-codegen has a crates parent");
    let manifest = format!(
        r#"[package]
name = "type-bridge-rust-projection-live-consumer"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
type-bridge-generated-schema = {{ path = "{}" }}
type-bridge-contract = {{ path = "{}" }}
type-bridge-orm = {{ path = "{}" }}
tokio = {{ version = "1", features = ["macros", "rt-multi-thread"] }}

[workspace]
"#,
        manifest_path(&generated),
        manifest_path(&crates_dir.join("contract")),
        manifest_path(&crates_dir.join("orm")),
    );
    let consumer_manifest = consumer.join("Cargo.toml");
    fs::write(&consumer_manifest, manifest).expect("consumer manifest is written");

    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(&consumer_manifest)
        .env("CARGO_TARGET_DIR", stage.path().join("target"))
        .output()
        .expect("live generated Rust consumer starts");

    assert!(
        output.status.success(),
        "live generated Rust consumer failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
