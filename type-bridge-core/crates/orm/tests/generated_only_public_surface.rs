//! External-consumer acceptance for the generated-only Rust cutover.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Stage(PathBuf);

impl Stage {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time follows the Unix epoch")
            .as_nanos();
        let sequence = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "type-bridge-generated-only-rust-surface-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("acceptance stage is created");
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

fn crates_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("ORM crate lives below the crates directory")
        .to_path_buf()
}

fn write_consumer(root: &Path, name: &str, source: &str) -> PathBuf {
    let crates = crates_root();
    fs::create_dir_all(root.join("src")).expect("consumer source directory is created");
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = {name:?}\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\n[dependencies]\ntype-bridge-orm = {{ path = {:?}, default-features = false }}\ntype-bridge-orm-derive = {{ path = {:?} }}\ntype-bridge-core-lib = {{ path = {:?} }}\n\n[workspace]\n",
            crates.join("orm"),
            crates.join("orm-derive"),
            crates.join("core"),
        ),
    )
    .expect("consumer manifest is written");
    fs::write(root.join("src/main.rs"), source).expect("consumer source is written");
    root.join("Cargo.toml")
}

fn cargo_check(manifest: &Path) -> Output {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    Command::new(cargo)
        .args(["check", "--quiet", "--offline", "--manifest-path"])
        .arg(manifest)
        .env(
            "CARGO_TARGET_DIR",
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/generated_only_public_surface"),
        )
        .output()
        .expect("external consumer cargo check starts")
}

fn assert_rejected(stage: &Stage, name: &str, source: &str, removed: &[&str]) {
    let manifest = write_consumer(&stage.path().join(name), name, source);
    let output = cargo_check(&manifest);
    assert!(
        !output.status.success(),
        "removed handwritten authoring surface unexpectedly compiled for {name}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unresolved import")
            || stderr.contains("could not find")
            || stderr.contains("failed to resolve"),
        "{name} failed for an unrelated reason:\n{stderr}"
    );
    for identity in removed {
        assert!(
            stderr.contains(identity),
            "{name} diagnostic omitted removed identity {identity}:\n{stderr}"
        );
    }
}

#[test]
fn handwritten_rust_authoring_paths_are_not_external_consumer_apis() {
    let stage = Stage::new();

    assert_rejected(
        &stage,
        "removed-orm-root-identities",
        r#"use type_bridge_orm::{
    Annotation, DeriveAttribute, DeriveEntity, DeriveRelation, DescriptorRegistry,
    DynamicEntityManager, DynamicRelationManager, EntityDescriptor, EntityManager,
    FieldRef, OwnedAttributeDescriptor, OwnedAttributeInfo, RelationDescriptor,
    RelationManager, RoleDescriptor, RoleInfo, RolePlayerFieldRef, RolePlayerRef,
    RoleRef, SchemaDiff, SchemaInfo, SchemaManager, TypeBridgeAttribute,
    TypeBridgeEntity, TypeBridgeRelation, TypeDescriptor, TypeDescriptorRef,
    define_attribute, include_schema,
};

fn main() {}
"#,
        &[
            "TypeBridgeAttribute",
            "TypeBridgeEntity",
            "TypeBridgeRelation",
            "DescriptorRegistry",
            "EntityDescriptor",
            "RelationDescriptor",
            "EntityManager",
            "RelationManager",
            "DynamicEntityManager",
            "DynamicRelationManager",
            "SchemaManager",
            "define_attribute",
            "include_schema",
        ],
    );

    assert_rejected(
        &stage,
        "removed-orm-module-paths",
        r#"use type_bridge_orm::{
    attribute, codegen, descriptor, dynamic, entity, field_ref, manager, registry,
    relation, schema,
};

fn main() {}
"#,
        &[
            "attribute",
            "codegen",
            "descriptor",
            "dynamic",
            "entity",
            "field_ref",
            "manager",
            "registry",
            "relation",
            "schema",
        ],
    );

    assert_rejected(
        &stage,
        "removed-derive-macros",
        r#"#[derive(type_bridge_orm_derive::TypeBridgeAttribute)]
struct Name(String);

#[derive(type_bridge_orm_derive::TypeBridgeEntity)]
struct Person;

#[derive(type_bridge_orm_derive::TypeBridgeRelation)]
struct Membership;

type_bridge_orm_derive::include_schema!("schema.tql");

fn main() {}
"#,
        &[
            "TypeBridgeAttribute",
            "TypeBridgeEntity",
            "TypeBridgeRelation",
            "include_schema",
        ],
    );

    assert_rejected(
        &stage,
        "removed-core-authoring-modules",
        "use type_bridge_core_lib::{bindgen, parser, schema};\nfn main() {}\n",
        &["bindgen", "parser", "schema"],
    );
}

#[test]
fn handwritten_typeql_codegen_binary_is_not_packaged() {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .output()
        .expect("cargo metadata starts");
    assert!(output.status.success());
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata is valid JSON");
    let package = metadata["packages"]
        .as_array()
        .and_then(|packages| {
            packages
                .iter()
                .find(|package| package["name"] == "type-bridge-orm")
        })
        .expect("ORM package exists in cargo metadata");
    let target_names = package["targets"]
        .as_array()
        .expect("package targets are an array")
        .iter()
        .filter_map(|target| target["name"].as_str())
        .collect::<Vec<_>>();
    assert!(
        !target_names.contains(&"type-bridge-codegen"),
        "removed TypeQL codegen binary remains packaged: {target_names:?}"
    );
}
