use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::projection::{
    BindingTarget, CSymbolPrefix, CodeResourceDigest, ProjectionConfig, ProjectionHandler,
    RuntimeProjection,
};
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema::{
    ResolvedSchema, SchemaDocumentSet, VerifiedSchemaAuthority, normalize_documents, project,
    resolve,
};
use type_bridge_schema_codegen::{
    CEmitter, PythonEmitter, RustEmitter, TypeScriptEmitter, verify_projection_evidence,
};

mod support;

static STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Stage(PathBuf);

impl Stage {
    fn new() -> Self {
        let sequence = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "typebridge-ordered-four-target-{}-{sequence}",
            std::process::id(),
        ));
        fs::create_dir(&path).expect("unique ordered-collection stage creates");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("ordered-collection stage removes");
    }
}

const UNORDERED_SOURCE: &str = r#"format: typebridge.schema/v2
attributes:
  tag: { value: string }
entities:
  person:
    owns:
      tag: { card: 1 }
relations:
  membership:
    relates:
      member: { card: 1 }
plays:
  person:
    membership:
      member: { card: 1 }
"#;

const ORDERED_SOURCE: &str = r#"format: typebridge.schema/v2
attributes:
  tag: { value: string }
entities:
  person:
    owns:
      tag:
        card: 1
        ordered: true
        distinct: true
relations:
  membership:
    relates:
      member:
        card: 1
        ordered: true
        distinct: true
plays:
  person:
    membership:
      member: { card: 1 }
"#;

fn resolved(source: &str) -> (ResolvedSchema, VerifiedSchemaAuthority) {
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("ordered-codegen.yaml").unwrap(), source)])
            .unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
    (
        resolve(&declared, &profile).unwrap(),
        support::authority(source),
    )
}

fn resource_ids(resources: &[CodeResourceDigest]) -> BTreeSet<&str> {
    resources
        .iter()
        .map(|resource| resource.id().as_str())
        .collect()
}

fn changed_resource_ids(
    legacy: &[CodeResourceDigest],
    successor: &[CodeResourceDigest],
) -> BTreeSet<String> {
    successor
        .iter()
        .filter(|resource| {
            legacy
                .iter()
                .find(|candidate| candidate.id() == resource.id())
                != Some(*resource)
        })
        .map(|resource| resource.id().as_str().to_owned())
        .collect()
}

fn handler_version(handlers: &[ProjectionHandler]) -> u16 {
    assert_eq!(handlers.len(), 1);
    handlers[0].version().get()
}

fn contains_hex_bytes(source: &str, needle: &[u8]) -> bool {
    let encoded = needle
        .iter()
        .map(|byte| format!("0x{byte:02x}u, "))
        .collect::<String>();
    source.contains(&encoded)
}

fn projection(
    schema: &ResolvedSchema,
    target: BindingTarget,
    config: &ProjectionConfig,
    handlers: &[ProjectionHandler],
    resources: &[CodeResourceDigest],
) -> RuntimeProjection {
    project(schema, target, config, handlers, resources).unwrap()
}

fn write_package(package: &type_bridge_schema_codegen::GeneratedPackage, root: &Path) {
    for (relative, bytes) in package.files() {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("generated path has a parent")).unwrap();
        fs::write(path, bytes).unwrap();
    }
}

fn checked_command(command: &mut Command, description: &str) -> Output {
    let output = command.output().unwrap_or_else(|error| {
        panic!("{description} did not launch: {error}");
    });
    assert!(
        output.status.success(),
        "{description} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    output
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn assert_exact_and_legacy_evidence(
    schema: &ResolvedSchema,
    authority: &VerifiedSchemaAuthority,
    target: BindingTarget,
    config: ProjectionConfig,
    exact: (Vec<ProjectionHandler>, Vec<CodeResourceDigest>),
    legacy: (Vec<ProjectionHandler>, Vec<CodeResourceDigest>),
) {
    let (exact_handlers, exact_resources) = exact;
    let (legacy_handlers, legacy_resources) = legacy;
    let exact = projection(schema, target, &config, &exact_handlers, &exact_resources);
    verify_projection_evidence(authority, &exact).unwrap();

    let legacy = projection(schema, target, &config, &legacy_handlers, &legacy_resources);
    assert_eq!(
        verify_projection_evidence(authority, &legacy)
            .unwrap_err()
            .code()
            .as_str(),
        "schema_codegen_projection_evidence_mismatch",
    );

    let mut forged_resources = exact_resources;
    let forged_id = forged_resources[0].id().as_str().to_owned();
    forged_resources[0] = CodeResourceDigest::from_bytes(forged_id, b"forged resource").unwrap();
    let forged = projection(schema, target, &config, &exact_handlers, &forged_resources);
    assert_eq!(
        verify_projection_evidence(authority, &forged)
            .unwrap_err()
            .code()
            .as_str(),
        "schema_codegen_projection_evidence_mismatch",
    );
}

#[test]
fn ordered_projection_selects_successor_evidence_and_descriptors_in_all_bindings() {
    let (schema, authority) = resolved(ORDERED_SOURCE);

    let python = PythonEmitter::new();
    let python_handlers = python.generator_handlers_for(&schema);
    let python_resources = python.code_resources_for(&schema).unwrap();
    let python_legacy_resources = python.code_resources().unwrap();
    assert_eq!(handler_version(&python_handlers), 2);
    assert_eq!(
        resource_ids(&python_resources),
        resource_ids(&python_legacy_resources)
    );
    assert_eq!(
        changed_resource_ids(&python_legacy_resources, &python_resources),
        BTreeSet::from(["typebridge.generator.python.runtime-source".to_owned()])
    );
    let python_projection = projection(
        &schema,
        BindingTarget::Python,
        &ProjectionConfig::python(),
        &python_handlers,
        &python_resources,
    );
    let python_package = python.emit(&python_projection, &authority).unwrap();
    let python_models = std::str::from_utf8(python_package.get("_models.py").unwrap()).unwrap();
    let python_stub = std::str::from_utf8(python_package.get("_models.pyi").unwrap()).unwrap();
    let python_runtime = std::str::from_utf8(python_package.get("_runtime.py").unwrap()).unwrap();
    assert!(python_models.contains("\\\"collection_mode\\\":\\\"ordered_list\\\""));
    assert!(python_models.contains("\\\"kind\\\":\\\"distinct\\\""));
    assert!(python_stub.contains("Sequence[Tag]"));
    assert!(python_stub.contains("tuple[Tag, ...]"));
    assert!(python_runtime.contains("_TYPE_BRIDGE_ORDERED_COLLECTION_RESOURCE_VERSION = 3"));
    assert!(python_runtime.contains("class _WholeCreateProjectedField"));
    assert!(python_runtime.contains("class _WholeCreateProjectedModelManager"));
    assert!(python_models.contains("_install_runtime_projection("));
    assert!(python_runtime.contains("def _install_runtime_projection_with_authority("));
    assert!(python_runtime.contains("from ._authority import SCHEMA_AUTHORITY_BYTES"));
    assert!(python_runtime.contains(
        "globals()[\"install_runtime_projection\"] = _install_runtime_projection_with_authority"
    ));

    let typescript = TypeScriptEmitter::new();
    let typescript_handlers = typescript.generator_handlers_for(&schema);
    let typescript_resources = typescript.code_resources_for(&schema).unwrap();
    let typescript_legacy_resources = typescript.code_resources().unwrap();
    assert_eq!(handler_version(&typescript_handlers), 2);
    assert_eq!(
        resource_ids(&typescript_resources),
        resource_ids(&typescript_legacy_resources)
    );
    assert_eq!(
        changed_resource_ids(&typescript_legacy_resources, &typescript_resources),
        BTreeSet::from(["typebridge.generator.typescript.runtime-source".to_owned()])
    );
    let typescript_projection = projection(
        &schema,
        BindingTarget::TypeScript,
        &ProjectionConfig::typescript(),
        &typescript_handlers,
        &typescript_resources,
    );
    let typescript_package = typescript.emit(&typescript_projection, &authority).unwrap();
    let typescript_models =
        std::str::from_utf8(typescript_package.get("src/models.ts").unwrap()).unwrap();
    let typescript_runtime =
        std::str::from_utf8(typescript_package.get("src/runtime.ts").unwrap()).unwrap();
    assert!(typescript_models.contains("\"collection_mode\":\"ordered_list\""));
    assert!(typescript_models.contains("\"kind\":\"distinct\""));
    assert!(typescript_models.contains("readonly (Tag)[]"));
    assert!(typescript_runtime.contains("readonly collection_mode?: \"ordered_list\""));
    assert!(typescript_runtime.contains("TYPE_BRIDGE_ORDERED_COLLECTION_RESOURCE_VERSION = 2"));
    assert!(
        std::str::from_utf8(typescript_package.get("src/index.ts").unwrap())
            .unwrap()
            .contains("__installOrderedRuntimeProjectionPackage(")
    );

    let rust = RustEmitter::new();
    let rust_handlers = rust.generator_handlers_for(&schema);
    let rust_resources = rust.code_resources_for(&schema).unwrap();
    let rust_legacy_resources = rust.code_resources().unwrap();
    assert_eq!(handler_version(&rust_handlers), 2);
    assert_eq!(
        resource_ids(&rust_resources),
        resource_ids(&rust_legacy_resources)
    );
    assert_eq!(
        changed_resource_ids(&rust_legacy_resources, &rust_resources),
        BTreeSet::from(["typebridge.generator.rust.runtime-source".to_owned()])
    );
    let rust_projection = projection(
        &schema,
        BindingTarget::Rust,
        &ProjectionConfig::rust(),
        &rust_handlers,
        &rust_resources,
    );
    let rust_package = rust.emit(&rust_projection, &authority).unwrap();
    let rust_tokens = std::str::from_utf8(rust_package.get("src/tokens.rs").unwrap()).unwrap();
    let rust_create = std::str::from_utf8(rust_package.get("src/create.rs").unwrap()).unwrap();
    let rust_runtime = std::str::from_utf8(rust_package.get("src/runtime.rs").unwrap()).unwrap();
    assert!(rust_tokens.contains("\\\"collection_mode\\\":\\\"ordered_list\\\""));
    assert!(rust_tokens.contains("\\\"kind\\\":\\\"distinct\\\""));
    assert!(rust_create.contains("Vec<Tag>"));
    assert!(rust_runtime.contains("Successor resource for ordered collection projections"));
    assert!(rust_runtime.contains("const _: u16 = 2;"));

    let c = CEmitter::new();
    let c_handlers = c.generator_handlers_for(&schema);
    let c_resources = c.code_resources_for(&schema).unwrap();
    let c_legacy_resources = c.code_resources().unwrap();
    assert_eq!(handler_version(&c_handlers), 3);
    assert_eq!(
        resource_ids(&c_resources),
        resource_ids(&c_legacy_resources)
    );
    assert_eq!(
        changed_resource_ids(&c_legacy_resources, &c_resources),
        BTreeSet::from(["typebridge.generator.c.cmake-template".to_owned()])
    );
    let c_projection = projection(
        &schema,
        BindingTarget::C,
        &ProjectionConfig::c(CSymbolPrefix::new("ordered_codegen").unwrap()),
        &c_handlers,
        &c_resources,
    );
    let c_package = c.emit(&c_projection, &authority).unwrap();
    let c_header =
        std::str::from_utf8(c_package.get("include/ordered_codegen/models.h").unwrap()).unwrap();
    let c_source = std::str::from_utf8(c_package.get("src/models.c").unwrap()).unwrap();
    let c_cmake = std::str::from_utf8(c_package.get("CMakeLists.txt").unwrap()).unwrap();
    assert!(c_header.contains("ordered_codegen_person_tag_count"));
    assert!(c_header.contains("ordered_codegen_membership_member_count"));
    assert!(contains_hex_bytes(c_source, b"ordered_list"));
    assert!(contains_hex_bytes(c_source, b"distinct"));
    assert!(c_cmake.starts_with("# TypeBridge ordered-collection generator resource v3\n"));
}

#[test]
fn shared_package_gate_rejects_legacy_and_forged_ordered_evidence_in_all_bindings() {
    let (schema, authority) = resolved(ORDERED_SOURCE);

    let python = PythonEmitter::new();
    assert_exact_and_legacy_evidence(
        &schema,
        &authority,
        BindingTarget::Python,
        ProjectionConfig::python(),
        (
            python.generator_handlers_for(&schema),
            python.code_resources_for(&schema).unwrap(),
        ),
        (
            python.generator_handlers(),
            python.code_resources().unwrap(),
        ),
    );

    let typescript = TypeScriptEmitter::new();
    assert_exact_and_legacy_evidence(
        &schema,
        &authority,
        BindingTarget::TypeScript,
        ProjectionConfig::typescript(),
        (
            typescript.generator_handlers_for(&schema),
            typescript.code_resources_for(&schema).unwrap(),
        ),
        (
            typescript.generator_handlers(),
            typescript.code_resources().unwrap(),
        ),
    );

    let rust = RustEmitter::new();
    assert_exact_and_legacy_evidence(
        &schema,
        &authority,
        BindingTarget::Rust,
        ProjectionConfig::rust(),
        (
            rust.generator_handlers_for(&schema),
            rust.code_resources_for(&schema).unwrap(),
        ),
        (rust.generator_handlers(), rust.code_resources().unwrap()),
    );

    let c = CEmitter::new();
    assert_exact_and_legacy_evidence(
        &schema,
        &authority,
        BindingTarget::C,
        ProjectionConfig::c(CSymbolPrefix::new("ordered_evidence").unwrap()),
        (
            c.generator_handlers_for(&schema),
            c.code_resources_for(&schema).unwrap(),
        ),
        (c.generator_handlers(), c.code_resources().unwrap()),
    );
}

#[test]
#[ignore = "requires the combined Python, TypeScript, Rust, and C compiler toolchain"]
fn ordered_generated_packages_pass_all_four_language_compilers() {
    let (schema, authority) = resolved(ORDERED_SOURCE);
    let stage = Stage::new();

    let python = PythonEmitter::new();
    let python_projection = projection(
        &schema,
        BindingTarget::Python,
        &ProjectionConfig::python(),
        &python.generator_handlers_for(&schema),
        &python.code_resources_for(&schema).unwrap(),
    );
    let python_root = stage.path().join("python");
    write_package(
        &python.emit(&python_projection, &authority).unwrap(),
        &python_root,
    );
    let mut python_command = Command::new("python3");
    python_command.arg("-m").arg("py_compile");
    for path in ["_authority.py", "_models.py", "_query.py", "_runtime.py"] {
        python_command.arg(python_root.join(path));
    }
    checked_command(
        &mut python_command,
        "ordered generated Python syntax compile",
    );

    let typescript = TypeScriptEmitter::new();
    let typescript_projection = projection(
        &schema,
        BindingTarget::TypeScript,
        &ProjectionConfig::typescript(),
        &typescript.generator_handlers_for(&schema),
        &typescript.code_resources_for(&schema).unwrap(),
    );
    let typescript_root = stage.path().join("typescript");
    write_package(
        &typescript.emit(&typescript_projection, &authority).unwrap(),
        &typescript_root,
    );
    let node_package = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("node");
    let installed_node = stage
        .path()
        .join("node_modules")
        .join("@type-bridge")
        .join("node");
    fs::create_dir_all(&installed_node).unwrap();
    fs::copy(
        node_package.join("package.json"),
        installed_node.join("package.json"),
    )
    .unwrap();
    copy_tree(&node_package.join("dist"), &installed_node.join("dist"));
    checked_command(
        Command::new(node_package.join("node_modules/.bin/tsc"))
            .arg("--project")
            .arg(typescript_root.join("tsconfig.json")),
        "ordered generated TypeScript compile",
    );

    let rust = RustEmitter::new();
    let rust_projection = projection(
        &schema,
        BindingTarget::Rust,
        &ProjectionConfig::rust(),
        &rust.generator_handlers_for(&schema),
        &rust.code_resources_for(&schema).unwrap(),
    );
    let rust_root = stage.path().join("rust");
    write_package(
        &rust.emit(&rust_projection, &authority).unwrap(),
        &rust_root,
    );
    let rust_sdk = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("rust");
    let patch = format!(
        "patch.crates-io.type-bridge.path=\"{}\"",
        rust_sdk.to_string_lossy().replace('\\', "\\\\"),
    );
    checked_command(
        Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .arg("check")
            .arg("--quiet")
            .arg("--offline")
            .arg("--manifest-path")
            .arg(rust_root.join("Cargo.toml"))
            .arg("--config")
            .arg(patch)
            .env(
                "CARGO_TARGET_DIR",
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .join("target/ordered-four-target"),
            ),
        "ordered generated Rust compile",
    );

    let c = CEmitter::new();
    let c_projection = projection(
        &schema,
        BindingTarget::C,
        &ProjectionConfig::c(CSymbolPrefix::new("ordered_codegen").unwrap()),
        &c.generator_handlers_for(&schema),
        &c.code_resources_for(&schema).unwrap(),
    );
    let c_root = stage.path().join("c");
    write_package(&c.emit(&c_projection, &authority).unwrap(), &c_root);
    let c_runtime_include = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("c/include");
    checked_command(
        Command::new("cc")
            .args(["-std=c17", "-Wall", "-Wextra", "-Werror", "-pedantic"])
            .arg("-I")
            .arg(c_root.join("include"))
            .arg("-I")
            .arg(c_runtime_include)
            .arg("-c")
            .arg(c_root.join("src/models.c"))
            .arg("-o")
            .arg(c_root.join("models.o")),
        "ordered generated C17 compile",
    );
}

#[test]
fn unordered_schema_aware_evidence_and_fixed_resources_remain_exactly_legacy() {
    let (schema, authority) = resolved(UNORDERED_SOURCE);

    let python = PythonEmitter::new();
    assert_eq!(
        python.generator_handlers_for(&schema),
        python.generator_handlers()
    );
    assert_eq!(
        python.code_resources_for(&schema).unwrap(),
        python.code_resources().unwrap()
    );
    let python_projection = projection(
        &schema,
        BindingTarget::Python,
        &ProjectionConfig::python(),
        &python.generator_handlers_for(&schema),
        &python.code_resources_for(&schema).unwrap(),
    );
    let python_package = python.emit(&python_projection, &authority).unwrap();
    assert_eq!(
        python_package.get("_runtime.py").unwrap(),
        include_bytes!("../src/python/runtime.py")
    );
    assert_eq!(
        python_package.get("_runtime.pyi").unwrap(),
        include_bytes!("../src/python/runtime.pyi")
    );
    let python_models = std::str::from_utf8(python_package.get("_models.py").unwrap()).unwrap();
    assert!(python_models.contains("_install_runtime_projection("));
    assert!(!python_models.contains("_SCHEMA_AUTHORITY_BYTES"));

    let typescript = TypeScriptEmitter::new();
    assert_eq!(
        typescript.generator_handlers_for(&schema),
        typescript.generator_handlers()
    );
    assert_eq!(
        typescript.code_resources_for(&schema).unwrap(),
        typescript.code_resources().unwrap()
    );
    let typescript_projection = projection(
        &schema,
        BindingTarget::TypeScript,
        &ProjectionConfig::typescript(),
        &typescript.generator_handlers_for(&schema),
        &typescript.code_resources_for(&schema).unwrap(),
    );
    let typescript_package = typescript.emit(&typescript_projection, &authority).unwrap();
    assert_eq!(
        typescript_package.get("src/runtime.ts").unwrap(),
        include_bytes!("../src/typescript/runtime.ts")
    );
    let typescript_index =
        std::str::from_utf8(typescript_package.get("src/index.ts").unwrap()).unwrap();
    assert!(typescript_index.contains("__installRuntimeProjectionPackage("));
    assert!(!typescript_index.contains("__installOrderedRuntimeProjectionPackage("));

    let rust = RustEmitter::new();
    assert_eq!(
        rust.generator_handlers_for(&schema),
        rust.generator_handlers()
    );
    assert_eq!(
        rust.code_resources_for(&schema).unwrap(),
        rust.code_resources().unwrap()
    );
    let rust_projection = projection(
        &schema,
        BindingTarget::Rust,
        &ProjectionConfig::rust(),
        &rust.generator_handlers_for(&schema),
        &rust.code_resources_for(&schema).unwrap(),
    );
    let rust_package = rust.emit(&rust_projection, &authority).unwrap();
    assert_eq!(
        rust_package.get("src/runtime.rs").unwrap(),
        include_bytes!("../src/rust/runtime.rs")
    );

    let c = CEmitter::new();
    assert_eq!(c.generator_handlers_for(&schema), c.generator_handlers());
    assert_eq!(
        c.code_resources_for(&schema).unwrap(),
        c.code_resources().unwrap()
    );
    let c_projection = projection(
        &schema,
        BindingTarget::C,
        &ProjectionConfig::c(CSymbolPrefix::new("legacy_codegen").unwrap()),
        &c.generator_handlers_for(&schema),
        &c.code_resources_for(&schema).unwrap(),
    );
    let c_package = c.emit(&c_projection, &authority).unwrap();
    let expected_cmake = std::str::from_utf8(include_bytes!("../src/c/CMakeLists.txt.in"))
        .unwrap()
        .replace("@TYPE_BRIDGE_C_SYMBOL_PREFIX@", "legacy_codegen");
    assert_eq!(
        c_package.get("CMakeLists.txt").unwrap(),
        expected_cmake.as_bytes()
    );
}
