use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig};
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
use type_bridge_schema_codegen::{GeneratedPackage, RustEmitter};

const POSITIVE: &str = include_str!("rust_acceptance/positive.rs");
const NEGATIVE: &str = include_str!("rust_acceptance/negative.rs");

fn emit() -> GeneratedPackage {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("rust-acceptance.yaml").unwrap(),
        include_str!("acceptance/schema.yaml"),
    )])
    .unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let resolved = resolve(
        &declared,
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
    let emitter = RustEmitter::new();
    let resources = emitter.code_resources().unwrap();
    let projection = project(
        &resolved,
        BindingTarget::Rust,
        &ProjectionConfig::rust(),
        &emitter.generator_handlers(),
        &resources,
    )
    .unwrap();
    emitter.emit(&projection).unwrap()
}

fn write_package(package: &GeneratedPackage, root: &Path) {
    for (relative, bytes) in package.files() {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
}

fn write_consumer(root: &Path, name: &str, source: &str) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[dependencies]\ngenerated = {{ package = \"type-bridge-generated-schema\", path = \"../generated\" }}\n\n[workspace]\n"
        ),
    ).unwrap();
    fs::write(root.join("src/main.rs"), source).unwrap();
}

fn cargo(arguments: &[&str], target: &Path) -> Output {
    let executable = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    Command::new(executable)
        .args(arguments)
        .env("CARGO_TARGET_DIR", target)
        .output()
        .unwrap()
}

#[test]
fn generated_rust_crate_compiles_rejects_invalid_types_and_runs() {
    let stage = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/schema-codegen-rust-acceptance");
    match fs::remove_dir_all(&stage) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("acceptance stage cleanup failed: {error}"),
    }
    let generated = stage.join("generated");
    let positive = stage.join("positive");
    let negative = stage.join("negative");
    write_package(&emit(), &generated);
    write_consumer(&positive, "rust-projection-positive", POSITIVE);
    write_consumer(&negative, "rust-projection-negative", NEGATIVE);

    let positive_manifest = positive.join("Cargo.toml");
    let positive_output = cargo(
        &[
            "run",
            "--quiet",
            "--offline",
            "--manifest-path",
            positive_manifest.to_str().unwrap(),
        ],
        &stage.join("positive-target"),
    );
    assert!(
        positive_output.status.success(),
        "positive generated consumer failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&positive_output.stdout),
        String::from_utf8_lossy(&positive_output.stderr),
    );

    let negative_manifest = negative.join("Cargo.toml");
    let negative_output = cargo(
        &[
            "check",
            "--quiet",
            "--offline",
            "--manifest-path",
            negative_manifest.to_str().unwrap(),
        ],
        &stage.join("negative-target"),
    );
    assert!(
        !negative_output.status.success(),
        "negative generated consumer unexpectedly compiled"
    );
    let stderr = String::from_utf8_lossy(&negative_output.stderr);
    assert!(
        stderr.contains("mismatched types"),
        "negative failure was not a type mismatch:\n{stderr}"
    );
    assert!(
        stderr.contains("EventRef"),
        "negative failure omitted the distinct reference type:\n{stderr}"
    );
    assert!(
        stderr.contains("RoleToken"),
        "negative failure omitted the owner-branded role token:\n{stderr}"
    );
}
