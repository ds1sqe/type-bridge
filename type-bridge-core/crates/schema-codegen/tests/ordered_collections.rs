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
        BTreeSet::from([
            "typebridge.generator.python.runtime-source".to_owned(),
            "typebridge.generator.python.runtime-stub".to_owned(),
        ])
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
    assert!(typescript_models.contains("defineOrderedModel as defineModel"));
    assert!(typescript_models.contains("type OrderedModelToken as ModelToken"));
    assert!(typescript_models.contains("\"kind\":\"distinct\""));
    assert!(typescript_models.contains("readonly (Tag)[]"));
    assert!(typescript_runtime.contains("readonly collection_mode?: \"ordered_list\""));
    assert!(typescript_runtime.contains("TYPE_BRIDGE_ORDERED_COLLECTION_RESOURCE_VERSION = 4"));
    assert!(typescript_runtime.contains("export type ProjectedBatchUpdate<Complete>"));
    assert!(typescript_runtime.contains("export interface OrderedProjectedModelManager<"));
    assert!(typescript_runtime.contains("export interface ProjectedModelFilter<"));
    assert!(typescript_runtime.contains("export type ProjectedManagerComparison ="));
    assert!(typescript_runtime.contains("export type OrderedModelToken<"));
    assert!(typescript_runtime.contains("native.insertManyProjected<Complete>"));
    assert!(typescript_runtime.contains("native.putManyProjected<Complete>"));
    assert!(typescript_runtime.contains("native.updateManyProjected<Complete>"));
    assert!(typescript_runtime.contains("native.deleteManyProjected("));
    assert!(
        typescript_runtime.contains("projectedBatchMaterializer: materializeOrderedProjectedBatch")
    );
    assert!(typescript_runtime.contains("descriptors[HYDRATE_COMPLETE_BRAND]"));
    assert!(typescript_runtime.contains("requireProjection().validateThingJson("));
    let typescript_index =
        std::str::from_utf8(typescript_package.get("src/index.ts").unwrap()).unwrap();
    assert!(typescript_index.contains("__installOrderedRuntimeProjectionPackage("));
    let projection_install = typescript_runtime
        .rfind("const projection = installRuntimeProjection({")
        .unwrap();
    let authority_install = typescript_runtime
        .rfind("const authority = installGeneratedSchemaAuthority({")
        .unwrap();
    assert!(projection_install < authority_install);

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
    assert!(rust_runtime.contains("const _: u16 = 3;"));
    assert!(rust_create.contains("__tb_validate_generated_create"));
    let rust_read = std::str::from_utf8(rust_package.get("src/read.rs").unwrap()).unwrap();
    assert!(rust_read.contains("__tb_validate_generated_hydration"));

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
        BTreeSet::from([
            "typebridge.generator.c.cmake-package-config-template".to_owned(),
            "typebridge.generator.c.cmake-template".to_owned(),
            "typebridge.generator.c.pkg-config-template".to_owned(),
        ])
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
    let c_package_config =
        std::str::from_utf8(c_package.get("ordered_codegenConfig.cmake.in").unwrap()).unwrap();
    let c_pkg_config =
        std::str::from_utf8(c_package.get("ordered_codegen.pc.in").unwrap()).unwrap();
    assert!(c_header.contains("ordered_codegen_person_tag_count"));
    assert!(c_header.contains("ordered_codegen_membership_member_count"));
    assert!(c_header.contains("#include <typebridge/type_bridge_abi_1_5.h>"));
    assert!(c_header.contains("ordered_codegen_schema_package_open_v2("));
    assert!(contains_hex_bytes(c_source, b"ordered_list"));
    assert!(contains_hex_bytes(c_source, b"distinct"));
    assert!(
        c_source
            .contains("sizeof(type_bridge_schema_package_chunked_descriptor_v1_t),\n  1u,\n  5u,")
    );
    assert!(c_source.contains("type_bridge_schema_package_open_chunked_v2("));
    assert!(c_cmake.starts_with("# TypeBridge ordered-collection generator resource v3\n"));
    assert!(c_cmake.contains("find_package(TypeBridge 1.5 CONFIG REQUIRED)"));
    assert!(c_package_config.contains("find_dependency(TypeBridge 1.5 CONFIG)"));
    assert!(c_pkg_config.contains("Requires: type-bridge >= 1.5.0, type-bridge < 2.0.0"));
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
    for path in [
        "__init__.py",
        "_authority.py",
        "_models.py",
        "_query.py",
        "_runtime.py",
    ] {
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
fn ordered_typescript_runtime_names_are_reserved_only_for_successor_packages() {
    let emitter = TypeScriptEmitter::new();
    for (label, target_name) in [
        ("ordered-model-token", "OrderedModelToken"),
        (
            "ordered-projected-model-manager",
            "OrderedProjectedModelManager",
        ),
        ("projected-batch-update", "ProjectedBatchUpdate"),
        ("projected-manager-comparison", "ProjectedManagerComparison"),
        ("projected-model-filter", "ProjectedModelFilter"),
    ] {
        let ordered_source = ORDERED_SOURCE.replace("  person:", &format!("  {label}:"));
        let (ordered_schema, ordered_authority) = resolved(&ordered_source);
        let ordered_projection = projection(
            &ordered_schema,
            BindingTarget::TypeScript,
            &ProjectionConfig::typescript(),
            &emitter.generator_handlers_for(&ordered_schema),
            &emitter.code_resources_for(&ordered_schema).unwrap(),
        );
        let error = emitter
            .emit(&ordered_projection, &ordered_authority)
            .unwrap_err();
        assert_eq!(error.code().as_str(), "typescript_emitter_name_collision");
        assert!(error.to_string().contains(target_name));

        let unordered_source = UNORDERED_SOURCE.replace("  person:", &format!("  {label}:"));
        let (unordered_schema, unordered_authority) = resolved(&unordered_source);
        let unordered_projection = projection(
            &unordered_schema,
            BindingTarget::TypeScript,
            &ProjectionConfig::typescript(),
            &emitter.generator_handlers_for(&unordered_schema),
            &emitter.code_resources_for(&unordered_schema).unwrap(),
        );
        let package = emitter
            .emit(&unordered_projection, &unordered_authority)
            .unwrap();
        let models = std::str::from_utf8(package.get("src/models.ts").unwrap()).unwrap();
        assert!(models.contains(&format!("export interface {target_name}")));
    }

    const COLLIDING_FUNCTION: &str = r#"
functions:
  define-ordered-model:
    parameters:
      - { name: person, type: person }
    returns: { scalar: string }
    body:
      typeql: |-
        match
          $person has tag $tag-attribute;
          let $tag = $tag-attribute;
        return first $tag;
"#;
    let ordered_source = format!("{ORDERED_SOURCE}{COLLIDING_FUNCTION}");
    let (ordered_schema, ordered_authority) = resolved(&ordered_source);
    let ordered_projection = projection(
        &ordered_schema,
        BindingTarget::TypeScript,
        &ProjectionConfig::typescript(),
        &emitter.generator_handlers_for(&ordered_schema),
        &emitter.code_resources_for(&ordered_schema).unwrap(),
    );
    let error = emitter
        .emit(&ordered_projection, &ordered_authority)
        .unwrap_err();
    assert_eq!(error.code().as_str(), "typescript_emitter_name_collision");
    assert!(error.to_string().contains("defineOrderedModel"));

    let unordered_source = format!("{UNORDERED_SOURCE}{COLLIDING_FUNCTION}");
    let (unordered_schema, unordered_authority) = resolved(&unordered_source);
    let unordered_projection = projection(
        &unordered_schema,
        BindingTarget::TypeScript,
        &ProjectionConfig::typescript(),
        &emitter.generator_handlers_for(&unordered_schema),
        &emitter.code_resources_for(&unordered_schema).unwrap(),
    );
    let package = emitter
        .emit(&unordered_projection, &unordered_authority)
        .unwrap();
    let functions = std::str::from_utf8(package.get("src/functions.ts").unwrap()).unwrap();
    assert!(functions.contains("export function defineOrderedModel"));
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
    let typescript_models =
        std::str::from_utf8(typescript_package.get("src/models.ts").unwrap()).unwrap();
    assert!(typescript_models.contains("\n  defineModel,\n"));
    assert!(!typescript_models.contains("defineOrderedModel"));
    assert!(typescript_models.contains("\n  type ModelToken,\n"));
    assert!(!typescript_models.contains("OrderedModelToken"));
    let typescript_runtime =
        std::str::from_utf8(typescript_package.get("src/runtime.ts").unwrap()).unwrap();
    assert!(!typescript_runtime.contains("ProjectedBatchUpdate"));
    assert!(!typescript_runtime.contains("OrderedProjectedModelManager"));
    assert!(!typescript_runtime.contains("ProjectedManagerComparison"));
    assert!(!typescript_runtime.contains("ProjectedModelFilter"));
    assert!(!typescript_runtime.contains("OrderedModelToken"));
    assert!(!typescript_runtime.contains("insertManyProjected"));
    assert!(!typescript_runtime.contains("putManyProjected"));
    assert!(!typescript_runtime.contains("updateManyProjected"));
    assert!(!typescript_runtime.contains("deleteManyProjected"));
    assert!(!typescript_runtime.contains("projectedBatchMaterializer"));
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

#[test]
#[ignore = "requires a built Node native addon and the TypeScript toolchain"]
fn ordered_typescript_hydration_uses_common_projected_validation() {
    const SOURCE: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  tag:
    value:
      type: string
      regex: "^[a-z]+$"
  occurred-on: { value: date }
  observed-at: { value: datetime }
  reported-at: { value: datetime-tz }
entities:
  record:
    owns:
      identifier: { key: true }
      tag:
        card: { min: 0, max: 4 }
        ordered: true
        distinct: true
  actor:
    abstract: true
    owns:
      tag:
        card: { min: 0, max: 4 }
        ordered: true
        distinct: true
  person:
    sub: actor
  group:
    sub: actor
  temporal-record:
    owns:
      occurred-on: { key: true }
      observed-at: { card: 1 }
      reported-at: { card: 1 }
relations:
  membership-base:
    abstract: true
    relates:
      participant:
        card: { min: 0, max: 4 }
        ordered: true
        distinct: true
  membership:
    sub: membership-base
  activity-link:
    relates:
      subject:
        card: { min: 0, max: 4 }
        ordered: true
        distinct: true
  temporal-link:
    relates:
      subject: { card: 1 }
  record-link:
    relates:
      subject: { card: 1 }
plays:
  record:
    record-link:
      subject: { card: { min: 0, max: 1 } }
  person:
    membership-base:
      participant: { card: { min: 0, max: 4 } }
  group:
    membership-base:
      participant: { card: { min: 0, max: 4 } }
  membership:
    activity-link:
      subject: { card: { min: 0, max: 4 } }
  temporal-record:
    temporal-link:
      subject: { card: { min: 0, max: 1 } }
"#;

    let (schema, authority) = resolved(SOURCE);
    let emitter = TypeScriptEmitter::new();
    let runtime_projection = projection(
        &schema,
        BindingTarget::TypeScript,
        &ProjectionConfig::typescript(),
        &emitter.generator_handlers_for(&schema),
        &emitter.code_resources_for(&schema).unwrap(),
    );
    let package = emitter.emit(&runtime_projection, &authority).unwrap();
    let foreign_source = SOURCE.replace("relations:\n", "  foreign-marker: {}\nrelations:\n");
    let (foreign_schema, foreign_authority) = resolved(&foreign_source);
    let foreign_projection = projection(
        &foreign_schema,
        BindingTarget::TypeScript,
        &ProjectionConfig::typescript(),
        &emitter.generator_handlers_for(&foreign_schema),
        &emitter.code_resources_for(&foreign_schema).unwrap(),
    );
    let foreign_package = emitter
        .emit(&foreign_projection, &foreign_authority)
        .unwrap();
    let stage = Stage::new();
    let generated = stage.path().join("generated");
    let foreign = stage.path().join("foreign");
    write_package(&package, &generated);
    write_package(&foreign_package, &foreign);
    for root in [&generated, &foreign] {
        let runtime_path = root.join("src/runtime.ts");
        let mut runtime_source = fs::read_to_string(&runtime_path).unwrap();
        let materializer_declaration = "const materializeOrderedProjectedBatch: NonNullable<";
        assert_eq!(runtime_source.matches(materializer_declaration).count(), 1);
        runtime_source = runtime_source.replacen(
            materializer_declaration,
            "const __testOrderedBatchMaterializeBase: NonNullable<",
            1,
        );
        let install_anchor =
            "/** @internal Install authority-backed evidence for an ordered generated package. */";
        assert_eq!(runtime_source.matches(install_anchor).count(), 1);
        runtime_source = runtime_source.replacen(
            install_anchor,
            r#"
let __testOrderedBatchFault = "none";
let __testOrderedBatchFaultOrdinal = -1;
let __testOrderedBatchInterceptor:
  | ((ordinal: number, value: object) => void)
  | null = null;
const __testOrderedBatchCaptured: object[] = [];
const materializeOrderedProjectedBatch: typeof __testOrderedBatchMaterializeBase = (
  typeKey,
  ordinal,
  json,
  authority,
): any => {
  const value = __testOrderedBatchMaterializeBase(
    typeKey,
    ordinal,
    json,
    authority,
  );
  __testOrderedBatchCaptured.push(value);
  __testOrderedBatchInterceptor?.(ordinal, value);
  if (ordinal === __testOrderedBatchFaultOrdinal) {
    if (__testOrderedBatchFault === "throw") {
      throw new TypeError("injected ordered batch materializer failure");
    }
    if (__testOrderedBatchFault === "null") {
      return null;
    }
    if (__testOrderedBatchFault === "primitive") {
      return 7;
    }
  }
  return value;
};

export function __testConfigureOrderedBatchMaterializer(
  fault: string,
  ordinal: number,
  interceptor: ((ordinal: number, value: object) => void) | null = null,
): void {
  if (!["none", "throw", "null", "primitive"].includes(fault)) {
    throw new TypeError("unknown ordered batch materializer fault");
  }
  __testOrderedBatchFault = fault;
  __testOrderedBatchFaultOrdinal = ordinal;
  __testOrderedBatchInterceptor = interceptor;
  __testOrderedBatchCaptured.length = 0;
}

export function __testCapturedOrderedBatchValues(): readonly object[] {
  return Object.freeze([...__testOrderedBatchCaptured]);
}

/** @internal Install authority-backed evidence for an ordered generated package. */"#,
            1,
        );
        runtime_source.push_str(
            r#"

// Test-only access to the module-private successor registry. This is appended
// only to the staged package and is never part of an emitted resource.
export function __testOrderedHydrateEnvelope(
  envelope: OrderedProjectedEnvelope,
): unknown {
  return hydrateOrderedProjectedEnvelope(envelope);
}
export function __testOrderedFacadeProof(value: object): unknown {
  return orderedFacadeProofs.get(value) ?? null;
}
export function __testOrderedRoleProofs(
  typeKey: string,
  value: unknown,
): readonly unknown[] {
  return orderedRoleProofs(typeKey, value);
}
export function __testOrderedNativeManager(
  typeKey: string,
  connection: RuntimeProjectionConnection,
): NativeProjectedManager {
  return requireProjection().manager(typeKey, connection);
}
export function __testOrderedNativeCreate(
  typeKey: string,
  value: unknown,
): { readonly json: string; readonly proofs: readonly unknown[] } {
  return {
    json: JSON.stringify(lowerOrderedProjectedValue(value)),
    proofs: orderedRoleProofs(typeKey, value),
  };
}
export function __testOrderedBatchMaterialize(
  typeKey: string,
  ordinal: number,
  json: string,
  authority: object,
): object {
  return __testOrderedBatchMaterializeBase(
    typeKey,
    ordinal,
    json,
    authority as Parameters<typeof materializeOrderedProjectedBatch>[3],
  );
}
"#,
        );
        fs::write(runtime_path, runtime_source).unwrap();
    }

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
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            node_package
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("target")
        });
    let native_library = if cfg!(target_os = "windows") {
        "type_bridge_node.dll"
    } else if cfg!(target_os = "macos") {
        "libtype_bridge_node.dylib"
    } else {
        "libtype_bridge_node.so"
    };
    let adapter_artifact = target.join("debug").join(native_library);
    assert!(
        adapter_artifact.is_file(),
        "ordered Node hydration acceptance requires the debug contract-test-adapter artifact"
    );
    fs::copy(
        adapter_artifact,
        installed_node.join("type_bridge_node.node"),
    )
    .unwrap();

    for root in [&generated, &foreign] {
        checked_command(
            Command::new(node_package.join("node_modules/.bin/tsc"))
                .arg("--project")
                .arg(root.join("tsconfig.json")),
            "ordered generated TypeScript hydration package compile",
        );
    }

    fs::write(
        stage.path().join("hydrate.mjs"),
        r#"import assert from "node:assert/strict";
import { createRequire } from "node:module";
import * as Local from "./generated/dist/index.js";
import * as Foreign from "./foreign/dist/index.js";
import * as Runtime from "./generated/dist/runtime.js";
import * as ForeignRuntime from "./foreign/dist/runtime.js";

const require = createRequire(import.meta.url);
const Native = require("./node_modules/@type-bridge/node/type_bridge_node.node");
const Handles = require("./node_modules/@type-bridge/node/dist/runtime-handles.js");

assert.equal("NodeProjectedFacadeProof" in Native, false);
assert.equal(
  Object.keys(Native).some((name) => name.includes("FacadeProof")),
  false,
);

const fixture = (responses, authority = null, commitFailure = null) => {
  const recording = commitFailure === null
    ? new Native.__ProjectionRecordingFixture(JSON.stringify(responses), authority)
    : new Native.__ProjectionRecordingFixture(
        JSON.stringify(responses),
        authority,
        commitFailure,
      );
  const database = recording.takeDatabase();
  const connection = Object.freeze({});
  Handles.registerRustDatabaseHandle(connection, database);
  return { recording, database, connection };
};
const counters = ({ recording }) => JSON.parse(recording.countersJson());
const zeroCounters = {
  opens: [], queries: [], commits: 0, rollbacks: 0, closes: 0,
};
const documents = (...values) => ({ Documents: values });
const iidDocument = (iid) => documents({ iid });
const personDocument = (iid, tag = "sourcetag") => ({
  _iid: iid,
  _type: "person",
  attributes: { tag: [{ value: tag }] },
});
const membershipDocument = (iid, playerIid) => ({
  _iid: iid,
  _type: "membership",
  attributes: {},
  role_players: [{
    role_name: "participant",
    player_iid: playerIid,
    player_type_name: "person",
    attributes: { tag: [{ value: "sourcetag" }] },
  }],
});
const activityLinkDocument = (iid, playerIid) => ({
  _iid: iid,
  _type: "activity-link",
  attributes: {},
  role_players: [{
    role_name: "subject",
    player_iid: playerIid,
    player_type_name: "membership",
    attributes: {},
  }],
});
const recordDocument = (iid, identifier, tags) => ({
  _iid: iid,
  _type: "record",
  attributes: {
    identifier: [{ value: identifier }],
    tag: tags.map((value) => ({ value })),
  },
});
const batchRecordResponses = () => [
  documents(
    { ordinal: 2, iid: "0xd2" },
    { ordinal: 0, iid: "0xd0" },
    { ordinal: 1, iid: "0xd1" },
  ),
  documents(
    { ordinal: 1, ...recordDocument("0xd1", "provider-1", ["normalizedone"]) },
    { ordinal: 2, ...recordDocument("0xd2", "provider-2", ["normalizedtwo"]) },
    { ordinal: 0, ...recordDocument("0xd0", "provider-0", ["normalizedzero"]) },
  ),
];
const temporalRecordDocument = (ordinal, iid) => ({
  ordinal,
  _iid: iid,
  _type: "temporal-record",
  attributes: {
    "occurred-on": [{ value: "+10000-01-02" }],
    "observed-at": [{ value: "-9999-01-02T03:04:05" }],
    "reported-at": [{ value: "-9999-01-02T03:04:05Z" }],
  },
});
const temporalLinkDocument = (iid, playerIid) => ({
  _iid: iid,
  _type: "temporal-link",
  attributes: {},
  role_players: [{
    role: "temporal-link:subject",
    iid: playerIid,
    type_name: "temporal-record",
    attributes: {
      "occurred-on": [{ value: "+10000-01-02" }],
      "observed-at": [{ value: "-9999-01-02T03:04:05" }],
      "reported-at": [{ value: "-9999-01-02T03:04:05Z" }],
    },
  }],
});

const diagnostic = (error) => {
  assert.ok(error instanceof Error);
  return JSON.parse(error.message);
};
const rejects = (operation, code, memberKind) => {
  assert.throws(operation, (error) => {
    const value = diagnostic(error);
    assert.equal(value.sdkCategory, "integrity");
    assert.equal(value.code, code);
    assert.equal(value.path[0].kind, "type");
    if (memberKind !== undefined) assert.equal(value.path[1].kind, memberKind);
    return true;
  });
};
const hydrate = (token, iid, input) => {
  const symbol = Object.getOwnPropertySymbols(token).find(
    (candidate) => candidate.description === "typebridge.hydrate-complete",
  );
  assert.notEqual(symbol, undefined);
  const materialize = token[symbol];
  assert.equal(typeof materialize, "function");
  return materialize(iid, input);
};

const first = Local.Tag.create("first");
const second = Local.Tag.create("second");
rejects(
  () => hydrate(Local.Tag, null, "INVALID"),
  "regex_constraint_violation",
);
const person = hydrate(Local.Person, "0xa1", { tag: [first, second] });
assert.deepEqual(person.tag.map((value) => value.value), ["first", "second"]);
rejects(
  () => hydrate(Local.Person, "0xa2", { tag: [first, first] }),
  "ordered_distinct_duplicate",
  "field",
);

const group = hydrate(Local.Group, "0xa3", { tag: [Local.Tag.create("group")] });
const membership = hydrate(Local.Membership, "0xb1", { participant: [person, group] });
assert.deepEqual(membership.participant.map((value) => value.iid), ["0xa1", "0xa3"]);
assert.equal(group.iid, "0xa3");
rejects(
  () => hydrate(Local.Membership, "0xb2", { participant: [person, person] }),
  "ordered_distinct_duplicate",
  "role",
);
rejects(
  () => hydrate(Local.Person, "person-a5", { tag: [] }),
  "noncanonical_hydrated_iid",
  "argument",
);

const link = hydrate(Local.ActivityLink, "0xc1", {
  subject: [Local.Membership.reference("0xb1", {})],
});
assert.equal(link.subject[0].iid, "0xb1");
rejects(
  () => hydrate(Local.ActivityLink, "0xc2", {
    subject: [Local.Membership.reference("membership-bad", {})],
  }),
  "noncanonical_iid",
  "role",
);

const foreignPerson = hydrate(Foreign.Person, "0xf1", {
  tag: [Foreign.Tag.create("foreign")],
});
rejects(
  () => hydrate(Local.Membership, "0xf2", { participant: [foreignPerson] }),
  "generated_token_package_mismatch",
  "role",
);

const recordBatchInputs = () => ["alpha", "beta", "gamma"].map((tag, ordinal) => Local.Record.create({
  identifier: Local.Identifier.create(`client-${ordinal}`),
  tag: [Local.Tag.create(tag)],
}));
const rejectsRecordProofWithoutIo = (value, target) => {
  assert.throws(
    () => Local.RecordLink.manager(target.connection).insert(
      Local.RecordLink.create({ subject: value }),
    ),
    (error) => error instanceof Error && error.message ===
      "projected batch proof is not active for this installed package",
  );
  assert.deepEqual(counters(target), zeroCounters);
};
const rowAtFailure = fixture([]);
const rawRecordManager = Runtime.__testOrderedNativeManager(
  Local.Record.typeKey,
  rowAtFailure.connection,
);
const hostileRowError = new Error("hostile rowAt failure");
hostileRowError.cause = hostileRowError;
Object.defineProperty(hostileRowError, "toString", {
  value: () => {
    throw new Error("rowAt error must not be coerced");
  },
});
let failedRowAtCalls = 0;
assert.throws(
  () => rawRecordManager.insertManyProjected(3, () => {
    failedRowAtCalls += 1;
    throw hostileRowError;
  }),
  (error) => error === hostileRowError,
);
assert.equal(failedRowAtCalls, 1);
assert.deepEqual(counters(rowAtFailure), zeroCounters);

for (const fault of ["throw", "null", "primitive"]) {
  for (const faultOrdinal of [0, 1, 2]) {
    const batchFailure = fixture(batchRecordResponses());
    const proofTarget = fixture([]);
    const callbackOrdinals = [];
    Runtime.__testConfigureOrderedBatchMaterializer(
      fault,
      faultOrdinal,
      (ordinal, value) => {
        callbackOrdinals.push(ordinal);
        rejectsRecordProofWithoutIo(value, proofTarget);
      },
    );
    assert.throws(
      () => Local.Record.manager(batchFailure.connection).insertMany(
        recordBatchInputs(),
      ),
      (error) => {
        const value = diagnostic(error);
        assert.equal(value.sdkCategory, "integrity");
        assert.equal(value.code, "generated_model_materialization_failed");
        assert.deepEqual(value.path, [
          { kind: "argument", value: "rows" },
          { kind: "index", value: faultOrdinal },
        ]);
        return true;
      },
    );
    assert.deepEqual(
      callbackOrdinals,
      Array.from({ length: faultOrdinal + 1 }, (_, index) => index),
    );
    const captured = Runtime.__testCapturedOrderedBatchValues();
    assert.equal(Object.isFrozen(captured), true);
    assert.equal(captured.length, faultOrdinal + 1);
    assert.deepEqual(
      captured.map((value) => value.identifier.value),
      callbackOrdinals.map((ordinal) => `provider-${ordinal}`),
    );
    for (const value of captured) {
      rejectsRecordProofWithoutIo(value, proofTarget);
    }
    const state = counters(batchFailure);
    assert.deepEqual(state.opens, ["write"]);
    assert.equal(state.queries.length, 2);
    assert.equal(state.givenRows.length, 2);
    assert.equal(state.commits, 0);
    assert.equal(state.rollbacks, 1);
    assert.equal(state.closes, 0);
  }
}

for (const commitFailure of ["definitely_aborted", "unknown"]) {
  const failedCommit = fixture(batchRecordResponses(), null, commitFailure);
  const proofTarget = fixture([]);
  const callbackOrdinals = [];
  Runtime.__testConfigureOrderedBatchMaterializer(
    "none",
    -1,
    (ordinal, value) => {
      callbackOrdinals.push(ordinal);
      rejectsRecordProofWithoutIo(value, proofTarget);
    },
  );
  assert.throws(() => Local.Record.manager(failedCommit.connection).insertMany(
    recordBatchInputs(),
  ));
  assert.deepEqual(callbackOrdinals, [0, 1, 2]);
  const captured = Runtime.__testCapturedOrderedBatchValues();
  assert.equal(captured.length, 3);
  for (const value of captured) {
    rejectsRecordProofWithoutIo(value, proofTarget);
  }
  const state = counters(failedCommit);
  assert.deepEqual(state.opens, ["write"]);
  assert.equal(state.queries.length, 2);
  assert.equal(state.givenRows.length, 2);
  assert.equal(state.commits, 1);
}
Runtime.__testConfigureOrderedBatchMaterializer("none", -1);

const nativeRoundTrip = fixture([
  iidDocument("0xd1"),
  documents(recordDocument("0xd1", "provider-insert", ["inserted", "normalized"])),
  documents(),
  iidDocument("0xd2"),
  documents(recordDocument("0xd2", "provider-put", ["put", "normalized"])),
  "Ok",
  documents(recordDocument("0xd2", "provider-update", ["updated", "normalized"])),
  documents(recordDocument("0xd2", "provider-get", ["read", "normalized"])),
]);
const recordManager = Local.Record.manager(nativeRoundTrip.connection);
const insertedRecord = recordManager.insert(Local.Record.create({
  identifier: Local.Identifier.create("client-insert"),
  tag: [Local.Tag.create("client")],
}));
assert.equal(insertedRecord.iid, "0xd1");
assert.equal(insertedRecord.identifier.value, "provider-insert");
assert.deepEqual(insertedRecord.tag.map((value) => value.value), ["inserted", "normalized"]);
const putRecord = recordManager.put(Local.Record.create({
  identifier: Local.Identifier.create("client-put"),
  tag: [Local.Tag.create("client")],
}));
assert.equal(putRecord.identifier.value, "provider-put");
const updatedRecord = recordManager.update("0xd2", Local.Record.create({
  identifier: Local.Identifier.create("client-update"),
  tag: [Local.Tag.create("client")],
}));
assert.equal(updatedRecord.identifier.value, "provider-update");
const readRecord = recordManager.getByIid("0xd2");
assert.equal(readRecord.identifier.value, "provider-get");
assert.deepEqual(readRecord.tag.map((value) => value.value), ["read", "normalized"]);
assert.deepEqual(counters(nativeRoundTrip), {
  opens: ["write", "write", "write", "read"],
  queries: counters(nativeRoundTrip).queries,
  commits: 3,
  rollbacks: 0,
  closes: 1,
});
assert.equal(counters(nativeRoundTrip).queries.length, 8);

const sharedAuthority = Native.__newProjectionRecordingAuthority();
const source = fixture([documents(personDocument("0xa7"))], sharedAuthority);
const exactPerson = Local.Person.manager(source.connection).getByIid("0xa7");
assert.equal(exactPerson.iid, "0xa7");
const exactPersonProof = Runtime.__testOrderedFacadeProof(exactPerson);
assert.notEqual(exactPersonProof, null);

const sameEntity = fixture([
  iidDocument("0xb7"),
  documents(membershipDocument("0xb7", "0xa7")),
], sharedAuthority);
const sameMembership = Local.Membership.manager(sameEntity.connection).insert(
  Local.Membership.create({ participant: [exactPerson] }),
);
assert.equal(sameMembership.participant[0].iid, "0xa7");
assert.equal(counters(sameEntity).commits, 1);

const foreignEntity = fixture([]);
assert.throws(() => Local.Membership.manager(foreignEntity.connection).insert(
  Local.Membership.create({ participant: [exactPerson] }),
), (error) => diagnostic(error).code === "reference_database_mismatch");
assert.deepEqual(counters(foreignEntity), zeroCounters);

const membershipSource = fixture([
  documents(membershipDocument("0xb8", "0xa7")),
], sharedAuthority);
const exactMembership = Local.Membership.manager(membershipSource.connection).getByIid("0xb8");
assert.equal(exactMembership.iid, "0xb8");
const sameRelation = fixture([
  iidDocument("0xb8"),
  iidDocument("0xc8"),
  documents(activityLinkDocument("0xc8", "0xb8")),
], sharedAuthority);
const sameLink = Local.ActivityLink.manager(sameRelation.connection).insert(
  Local.ActivityLink.create({ subject: [exactMembership] }),
);
assert.equal(sameLink.subject[0].iid, "0xb8");
assert.equal(counters(sameRelation).queries.length, 3);
const foreignRelation = fixture([]);
assert.throws(() => Local.ActivityLink.manager(foreignRelation.connection).insert(
  Local.ActivityLink.create({ subject: [exactMembership] }),
), (error) => diagnostic(error).code === "reference_database_mismatch");
assert.deepEqual(counters(foreignRelation), zeroCounters);

const nativeForeign = fixture([]);
const foreignNativeManager = ForeignRuntime.__testOrderedNativeManager(
  Foreign.Membership.typeKey,
  nativeForeign.connection,
);
const localPersonWire = Runtime.__testOrderedNativeCreate(
  Local.Membership.typeKey,
  Local.Membership.create({ participant: [exactPerson] }),
);
assert.throws(() => foreignNativeManager.insertProjected(
  localPersonWire.json,
  localPersonWire.proofs,
));
assert.deepEqual(counters(nativeForeign), zeroCounters);

const rawSource = fixture([documents(personDocument("0xa9"))], sharedAuthority);
const rawPersonManager = Runtime.__testOrderedNativeManager(
  Local.Person.typeKey,
  rawSource.connection,
);
const rawEnvelope = rawPersonManager.getByIidProjected("0xa9");
const rawPerson = Runtime.__testOrderedHydrateEnvelope(rawEnvelope);
const rawProof = Runtime.__testOrderedFacadeProof(rawPerson);
assert.notEqual(rawProof, null);
const proofTarget = fixture([]);
const rawMembershipManager = Runtime.__testOrderedNativeManager(
  Local.Membership.typeKey,
  proofTarget.connection,
);
const rawMembershipWire = Runtime.__testOrderedNativeCreate(
  Local.Membership.typeKey,
  Local.Membership.create({
    participant: [rawPerson],
  }),
).json;
const malformedProofs = [
  rawEnvelope,
  {},
  Object.create(null),
  JSON.parse(JSON.stringify(rawProof)),
  Native.__newProjectionRecordingAuthority(),
];
for (const candidate of malformedProofs) {
  assert.throws(() => rawMembershipManager.insertProjected(
    rawMembershipWire,
    [candidate],
  ));
  assert.deepEqual(counters(proofTarget), zeroCounters);
}
assert.throws(() => rawMembershipManager.insertProjected(
  rawMembershipWire,
  [new Proxy(rawProof, {})],
));
assert.deepEqual(counters(proofTarget), zeroCounters);
let clonedProof;
let cloneFailed = false;
try {
  clonedProof = structuredClone(rawProof);
} catch (error) {
  assert.ok(error instanceof Error);
  cloneFailed = true;
}
if (!cloneFailed) {
  assert.throws(() => rawMembershipManager.insertProjected(
    rawMembershipWire,
    [clonedProof],
  ));
}
assert.deepEqual(counters(proofTarget), zeroCounters);

assert.throws(() => new Native.NodeProjectedModelManager());
assert.throws(() => new Native.NodeProjectedValueEnvelope());

const mismatchedWire = JSON.parse(rawMembershipWire);
mismatchedWire.values.participant[0].iid = "0xaa";
assert.throws(() => rawMembershipManager.insertProjected(
  JSON.stringify(mismatchedWire),
  [rawProof],
), (error) => diagnostic(error).code === "malformed_projected_create");
assert.deepEqual(counters(proofTarget), zeroCounters);

const managerReceivers = [
  {},
  Object.create(Native.NodeProjectedModelManager.prototype),
  Object.create(rawMembershipManager),
  new Proxy(rawMembershipManager, {}),
  rawEnvelope,
];
for (const receiver of managerReceivers) {
  assert.throws(() =>
    Native.NodeProjectedModelManager.prototype.getByIidProjected.call(receiver, "0xa9"),
  );
  assert.deepEqual(counters(proofTarget), zeroCounters);
}
const envelopeReceivers = [
  {},
  Object.create(Native.NodeProjectedValueEnvelope.prototype),
  Object.create(rawEnvelope),
  new Proxy(rawEnvelope, {}),
  rawMembershipManager,
];
for (const receiver of envelopeReceivers) {
  assert.throws(() =>
    Native.NodeProjectedValueEnvelope.prototype.rootProof.call(receiver),
  );
}

const exactPersonProxy = new Proxy(exactPerson, {});
const exactPersonCopy = Object.create(
  Object.getPrototypeOf(exactPerson),
  Object.getOwnPropertyDescriptors(exactPerson),
);
assert.deepEqual(
  Runtime.__testOrderedRoleProofs(
    Local.Membership.typeKey,
    Local.Membership.create({ participant: [exactPersonProxy] }),
  ),
  [null],
);
assert.deepEqual(
  Runtime.__testOrderedRoleProofs(
    Local.Membership.typeKey,
    Local.Membership.create({ participant: [exactPersonCopy] }),
  ),
  [null],
);
for (const unboundPlayer of [exactPersonProxy, exactPersonCopy]) {
  const unboundTarget = fixture([
    iidDocument("0xbe"),
    documents(membershipDocument("0xbe", "0xa7")),
  ]);
  const created = Local.Membership.manager(unboundTarget.connection).insert(
    Local.Membership.create({ participant: [unboundPlayer] }),
  );
  assert.equal(created.iid, "0xbe");
  assert.equal(counters(unboundTarget).queries.length, 2);
  assert.equal(counters(unboundTarget).commits, 1);
}

const rootProof = Object.freeze({ marker: "private-root-proof" });
const playerProof = Object.freeze({ marker: "private-player-proof" });
const projected = Runtime.__testOrderedHydrateEnvelope({
  json: JSON.stringify({
    typeKey: Local.Membership.typeKey,
    form: "complete",
    iid: "0xb9",
    value: null,
    values: {
      participant: [{
        typeKey: Local.Person.typeKey,
        form: "complete",
        iid: "0xa9",
        value: null,
        values: { tag: [] },
      }],
    },
  }),
  rootProof: () => rootProof,
  roleProof: (roleName, playerIndex) => {
    assert.equal(roleName, "participant");
    assert.equal(playerIndex, 0);
    return playerProof;
  },
});
const projectedPlayer = projected.participant[0];
assert.equal(Runtime.__testOrderedFacadeProof(projected), rootProof);
assert.equal(Runtime.__testOrderedFacadeProof(projectedPlayer), playerProof);
assert.equal(JSON.stringify(projected).includes("private-"), false);
assert.equal(
  Reflect.ownKeys(projectedPlayer).some(
    (key) => projectedPlayer[key] === playerProof,
  ),
  false,
);
assert.equal(Object.isFrozen(projected), true);
assert.equal(Object.isFrozen(projectedPlayer), true);
assert.throws(() => {
  projectedPlayer.iid = "0xchanged";
}, TypeError);
const lookalike = Object.create(
  Object.getPrototypeOf(projectedPlayer),
  Object.getOwnPropertyDescriptors(projectedPlayer),
);
assert.notEqual(lookalike, projectedPlayer);
assert.deepEqual(lookalike, projectedPlayer);
assert.equal(Runtime.__testOrderedFacadeProof(lookalike), null);
assert.deepEqual(
  Runtime.__testOrderedRoleProofs(
    Local.Membership.typeKey,
    Local.Membership.create({ participant: [projectedPlayer] }),
  ),
  [playerProof],
);
assert.deepEqual(
  Runtime.__testOrderedRoleProofs(
    Local.Membership.typeKey,
    Local.Membership.create({ participant: [lookalike] }),
  ),
  [null],
);
let failedRootProofCalls = 0;
assert.throws(
  () => Runtime.__testOrderedHydrateEnvelope({
    json: JSON.stringify({
      typeKey: Local.Membership.typeKey,
      form: "complete",
      iid: "0xba",
      value: null,
      values: {
        participant: [{
          typeKey: Local.Person.typeKey,
          form: "complete",
          iid: "0xaa",
          value: null,
          values: { tag: [] },
        }],
      },
    }),
    rootProof: () => {
      failedRootProofCalls += 1;
      return rootProof;
    },
    roleProof: () => {
      throw new TypeError("proof lookup failed");
    },
  }),
  /proof lookup failed/,
);
assert.equal(failedRootProofCalls, 0);

const batchAuthority = Object.freeze({ marker: "private-batch-authority" });
const batchProjected = Runtime.__testOrderedBatchMaterialize(
  Local.Membership.typeKey,
  7,
  JSON.stringify({
    typeKey: Local.Membership.typeKey,
    form: "complete",
    iid: "0xbb",
    value: null,
        values: {
          participant: [{
            typeKey: Local.Person.typeKey,
            form: "complete",
            iid: "0xab",
            value: null,
            values: {
              tag: [{
                typeKey: Local.Tag.typeKey,
                form: "complete",
                iid: null,
                value: { valueType: "string", value: "batchtag" },
                values: {},
              }],
            },
          }],
        },
  }),
  batchAuthority,
);
const batchRootProof = Runtime.__testOrderedFacadeProof(batchProjected);
const batchRoleProof = Runtime.__testOrderedFacadeProof(
  batchProjected.participant[0],
);
assert.deepEqual(batchRootProof, {
  kind: "root",
  authority: batchAuthority,
  row: 7,
});
assert.deepEqual(batchRoleProof, {
  kind: "role",
  authority: batchAuthority,
  row: 7,
  roleName: "participant",
  playerIndex: 0,
});
assert.equal(Object.isFrozen(batchRootProof), true);
assert.equal(Object.isFrozen(batchRoleProof), true);
assert.equal(batchProjected.participant[0].tag[0].value, "batchtag");
assert.deepEqual(
  Runtime.__testOrderedRoleProofs(
    Local.Membership.typeKey,
    Local.Membership.create({ participant: [batchProjected.participant[0]] }),
  ),
  [batchRoleProof],
);

const temporalRecord = Runtime.__testOrderedBatchMaterialize(
  Local.TemporalRecord.typeKey,
  9,
  JSON.stringify({
    typeKey: Local.TemporalRecord.typeKey,
    form: "complete",
    iid: "0xdc",
    value: null,
    values: {
      occurredOn: {
        typeKey: Local.OccurredOn.typeKey,
        form: "complete",
        iid: null,
        value: { valueType: "date", value: "+010000-01-02" },
        values: {},
      },
      observedAt: {
        typeKey: Local.ObservedAt.typeKey,
        form: "complete",
        iid: null,
        value: {
          valueType: "datetime",
          value: "-009999-01-02T03:04:05",
        },
        values: {},
      },
      reportedAt: {
        typeKey: Local.ReportedAt.typeKey,
        form: "complete",
        iid: null,
        value: {
          valueType: "datetime_tz",
          value: "-009999-01-02T03:04:05Z",
        },
        values: {},
      },
    },
  }),
  batchAuthority,
);
assert.equal(temporalRecord.occurredOn.value.toISOString(), "+010000-01-02T00:00:00.000Z");
assert.equal(temporalRecord.observedAt.value.toISOString(), "-009999-01-02T03:04:05.000Z");
assert.equal(temporalRecord.reportedAt.value.toISOString(), "-009999-01-02T03:04:05.000Z");
const temporalRecordInput = Runtime.__testOrderedNativeCreate(
  Local.TemporalRecord.typeKey,
  temporalRecord,
);
const temporalRecordWire = JSON.parse(temporalRecordInput.json);
assert.equal(temporalRecordWire.values.occurredOn.value.value, "+10000-01-02");
assert.equal(
  temporalRecordWire.values.observedAt.value.value,
  "-9999-01-02T03:04:05",
);
assert.equal(
  temporalRecordWire.values.reportedAt.value.value,
  "-9999-01-02T03:04:05Z",
);
const temporalLinkInput = Local.TemporalLink.create({ subject: temporalRecord });
assert.deepEqual(
  Runtime.__testOrderedRoleProofs(
    Local.TemporalLink.typeKey,
    temporalLinkInput,
  ),
  [Runtime.__testOrderedFacadeProof(temporalRecord)],
);
const temporalLinkWire = JSON.parse(
  Runtime.__testOrderedNativeCreate(
    Local.TemporalLink.typeKey,
    temporalLinkInput,
  ).json,
);
assert.equal(
  temporalLinkWire.values.subject.values.occurredOn.value.value,
  "+10000-01-02",
);
const negativeSmallTemporalRecord = Runtime.__testOrderedBatchMaterialize(
  Local.TemporalRecord.typeKey,
  10,
  JSON.stringify({
    ...temporalRecordWire,
    iid: "0xdd",
    values: {
      occurredOn: {
        ...temporalRecordWire.values.occurredOn,
        value: { valueType: "date", value: "-000001-01-02" },
      },
      observedAt: {
        ...temporalRecordWire.values.observedAt,
        value: {
          valueType: "datetime",
          value: "-000001-01-02T03:04:05",
        },
      },
      reportedAt: {
        ...temporalRecordWire.values.reportedAt,
        value: {
          valueType: "datetime_tz",
          value: "-000001-01-02T03:04:05Z",
        },
      },
    },
  }),
  batchAuthority,
);
const negativeSmallWire = JSON.parse(
  Runtime.__testOrderedNativeCreate(
    Local.TemporalRecord.typeKey,
    negativeSmallTemporalRecord,
  ).json,
);
assert.equal(negativeSmallWire.values.occurredOn.value.value, "-0001-01-02");
assert.equal(
  negativeSmallWire.values.observedAt.value.value,
  "-0001-01-02T03:04:05",
);
assert.equal(
  negativeSmallWire.values.reportedAt.value.value,
  "-0001-01-02T03:04:05Z",
);

const temporalDatabaseAuthority = Native.__newProjectionRecordingAuthority();
const temporalBatch = fixture([
  documents({ ordinal: 0, iid: "0xde" }),
  documents(temporalRecordDocument(0, "0xde")),
], temporalDatabaseAuthority);
Runtime.__testConfigureOrderedBatchMaterializer("none", -1);
const temporalNativeManager = Runtime.__testOrderedNativeManager(
  Local.TemporalRecord.typeKey,
  temporalBatch.connection,
);
let temporalRowAtCalls = 0;
const temporalBatchValues = temporalNativeManager.insertManyProjected(
  1,
  (ordinal) => {
    temporalRowAtCalls += 1;
    assert.equal(ordinal, 0);
    return {
      instanceJson: JSON.stringify({ ...temporalRecordWire, iid: null }),
      proofs: [],
    };
  },
);
assert.equal(temporalRowAtCalls, 1);
assert.equal(Object.isFrozen(temporalBatchValues), true);
assert.equal(temporalBatchValues.length, 1);
const activeTemporalRecord = temporalBatchValues[0];
assert.equal(activeTemporalRecord.iid, "0xde");
assert.equal(
  activeTemporalRecord.occurredOn.value.toISOString(),
  "+010000-01-02T00:00:00.000Z",
);
const activeTemporalProof = Runtime.__testOrderedFacadeProof(activeTemporalRecord);
assert.equal(activeTemporalProof.kind, "root");
assert.equal(activeTemporalProof.row, 0);
const temporalLinkTarget = fixture([
  documents({
    kind: 1,
    ordinal: 0,
    reference_ordinal: 0,
    iid: "0xde",
    type: "temporal-record",
  }),
  documents({ ordinal: 0, iid: "0xdf" }),
  documents({ ordinal: 0, ...temporalLinkDocument("0xdf", "0xde") }),
], temporalDatabaseAuthority);
const [activeTemporalLink] = Local.TemporalLink.manager(
  temporalLinkTarget.connection,
).insertMany([Local.TemporalLink.create({ subject: activeTemporalRecord })]);
assert.equal(activeTemporalLink.iid, "0xdf");
assert.equal(activeTemporalLink.subject.iid, "0xde");
assert.equal(
  activeTemporalLink.subject.occurredOn.value.toISOString(),
  "+010000-01-02T00:00:00.000Z",
);
assert.equal(counters(temporalBatch).commits, 1);
assert.equal(counters(temporalLinkTarget).commits, 1);

Runtime.__testConfigureOrderedBatchMaterializer("none", -1);
const abortedTemporalLinkBatch = fixture([
  documents({
    kind: 1,
    ordinal: 0,
    reference_ordinal: 0,
    iid: "0xde",
    type: "temporal-record",
  }),
  documents({ ordinal: 0, iid: "0xe0" }),
  documents({ ordinal: 0, ...temporalLinkDocument("0xe0", "0xde") }),
], temporalDatabaseAuthority, "definitely_aborted");
assert.throws(() => Local.TemporalLink.manager(
  abortedTemporalLinkBatch.connection,
).insertMany([Local.TemporalLink.create({ subject: activeTemporalRecord })]));
const [abortedTemporalLink] = Runtime.__testCapturedOrderedBatchValues();
const abortedTemporalRoleProof = Runtime.__testOrderedFacadeProof(
  abortedTemporalLink.subject,
);
assert.equal(abortedTemporalRoleProof.kind, "role");
assert.equal(abortedTemporalRoleProof.row, 0);
assert.equal(abortedTemporalRoleProof.roleName, "subject");
assert.equal(abortedTemporalRoleProof.playerIndex, 0);
const abortedTemporalRoleTarget = fixture([], temporalDatabaseAuthority);
assert.throws(
  () => Local.TemporalLink.manager(
    abortedTemporalRoleTarget.connection,
  ).insert(Local.TemporalLink.create({ subject: abortedTemporalLink.subject })),
  (error) => error instanceof Error && error.message ===
    "projected batch proof is not active for this installed package",
);
assert.deepEqual(counters(abortedTemporalRoleTarget), zeroCounters);
const abortedTemporalLinkState = counters(abortedTemporalLinkBatch);
assert.equal(abortedTemporalLinkState.queries.length, 3);
assert.equal(abortedTemporalLinkState.givenRows.length, 3);
assert.equal(abortedTemporalLinkState.commits, 1);

const materializeMalformedBatch = (typeKey, wire) =>
  Runtime.__testOrderedBatchMaterialize(
    typeKey,
    8,
    JSON.stringify(wire),
    batchAuthority,
  );
const identifierWire = {
  typeKey: Local.Identifier.typeKey,
  form: "complete",
  iid: null,
  value: { valueType: "string", value: "identifier" },
  values: {},
};
const personWire = {
  typeKey: Local.Person.typeKey,
  form: "complete",
  iid: "0xac",
  value: null,
  values: { tag: [] },
};
assert.throws(
  () => materializeMalformedBatch(Local.Membership.typeKey, {
    typeKey: Local.Membership.typeKey,
    form: "complete",
    iid: null,
    value: null,
    values: { participant: [] },
  }),
  /native complete thing wire has no IID/,
);
assert.throws(
  () => materializeMalformedBatch(Local.Membership.typeKey, {
    typeKey: Local.Membership.typeKey,
    form: "complete",
    iid: "0xbc",
    value: null,
    values: { participant: [], extra: null },
  }),
  /native projected wire has an inexact member set/,
);
assert.throws(
  () => materializeMalformedBatch(Local.Membership.typeKey, {
    typeKey: Local.Membership.typeKey,
    form: "complete",
    iid: "0xbd",
    value: null,
    values: { participant: personWire },
  }),
  /participant must be a sequence/,
);
assert.throws(
  () => materializeMalformedBatch(Local.Membership.typeKey, {
    typeKey: Local.Membership.typeKey,
    form: "complete",
    iid: "0xbe",
    value: null,
    values: { participant: [identifierWire] },
  }),
  /participant has an incompatible projected model form/,
);
assert.throws(
  () => materializeMalformedBatch(Local.Person.typeKey, {
    typeKey: Local.Person.typeKey,
    form: "complete",
    iid: "0xad",
    value: null,
    values: { tag: [identifierWire] },
  }),
  /tag is not the field's exact attribute value/,
);
"#,
    )
    .unwrap();
    checked_command(
        Command::new("node").arg(stage.path().join("hydrate.mjs")),
        "ordered generated TypeScript common-validation hydration",
    );
}

#[test]
fn ordered_rust_construction_and_hydration_use_common_projected_validation() {
    const SOURCE: &str = r#"format: typebridge.schema/v2
attributes:
  tag: { value: string }
entities:
  actor:
    abstract: true
    owns:
      tag:
        card: { min: 0, max: 4 }
        ordered: true
        distinct: true
  person:
    sub: actor
relations:
  membership-base:
    abstract: true
    relates:
      participant:
        card: { min: 0, max: 4 }
        ordered: true
        distinct: true
  membership:
    sub: membership-base
plays:
  person:
    membership-base:
      participant: { card: { min: 0, max: 4 } }
"#;

    let (schema, authority) = resolved(SOURCE);
    let emitter = RustEmitter::new();
    let runtime_projection = projection(
        &schema,
        BindingTarget::Rust,
        &ProjectionConfig::rust(),
        &emitter.generator_handlers_for(&schema),
        &emitter.code_resources_for(&schema).unwrap(),
    );
    let package = emitter.emit(&runtime_projection, &authority).unwrap();
    let stage = Stage::new();
    let generated = stage.path().join("generated");
    let consumer = stage.path().join("consumer");
    write_package(&package, &generated);
    fs::create_dir_all(consumer.join("src")).unwrap();

    let rust_sdk = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("rust");
    let rust_path = rust_sdk.to_string_lossy().replace('\\', "\\\\");
    fs::write(
        consumer.join("Cargo.toml"),
        format!(
            "[package]\nname=\"ordered-rust-validation\"\nversion=\"0.0.0\"\nedition=\"2024\"\npublish=false\n[dependencies]\ngenerated={{package=\"type-bridge-generated-schema\",path=\"../generated\",features=[\"test-harness\"]}}\ntype-bridge={{path=\"{rust_path}\",default-features=false,features=[\"test-harness\"]}}\n[patch.crates-io]\ntype-bridge={{path=\"{rust_path}\"}}\n[workspace]\n"
        ),
    )
    .unwrap();
    fs::write(
        consumer.join("src/main.rs"),
        r#"use generated::*;
use type_bridge::__codegen::{
    CanonicalDouble, HydratedPlayer, HydratedRow, IntoEncodedScalar,
    materialize_model_for_test,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let first = Tag::new("first")?;
    let second = Tag::new("second")?;
    let person = PersonCreate::new(vec![first.clone(), second.clone()])?;
    assert_eq!(
        person.tag().iter().map(|value| value.value().as_str()).collect::<Vec<_>>(),
        ["first", "second"],
    );

    let error = PersonCreate::new(vec![first.clone(), first.clone()]).unwrap_err();
    assert_eq!(error.code(), "ordered_distinct_duplicate");
    assert_eq!(error.field(), "type.tag[1]");

    let first_player = PersonRef::from_iid("0xa")?;
    let second_player = PersonRef::from_iid("0xb")?;
    let membership = MembershipCreate::new(vec![first_player.clone(), second_player])?;
    assert_eq!(
        membership
            .participant()
            .iter()
            .map(|reference| reference.iid())
            .collect::<Vec<_>>(),
        [Some("0xa"), Some("0xb")],
    );
    let error =
        MembershipCreate::new(vec![first_player.clone(), first_player]).unwrap_err();
    assert_eq!(error.code(), "ordered_distinct_duplicate");
    assert_eq!(error.field(), "type.participant[1].type");

    let error = CanonicalDouble::try_new(f64::INFINITY).unwrap_err();
    assert_eq!((error.code(), error.field()), ("noncanonical_double", ""));

    let duplicate_field_row = HydratedRow::new(
        Person::TYPE_ID_JSON,
        "0x1".to_owned(),
        vec![(
            PersonType::tag.owns_id_json(),
            vec![
                first.value().into_encoded_scalar(),
                first.value().into_encoded_scalar(),
            ],
        )],
        vec![],
    );
    let error = materialize_model_for_test::<Person>(&duplicate_field_row).unwrap_err();
    assert_eq!(error.code(), "ordered_distinct_duplicate");
    assert_eq!(error.field(), "type.tag[1]");

    let duplicate_shape_row = HydratedRow::new(
        Person::TYPE_ID_JSON,
        "0x1".to_owned(),
        vec![
            (PersonType::tag.owns_id_json(), vec![]),
            (PersonType::tag.owns_id_json(), vec![]),
        ],
        vec![],
    );
    let error = materialize_model_for_test::<Person>(&duplicate_shape_row).unwrap_err();
    assert_eq!(error.code(), "duplicate_scalar_evidence");
    assert_eq!(error.field(), "");

    let wrong_domain_row = HydratedRow::new(
        Person::TYPE_ID_JSON,
        "0x1".to_owned(),
        vec![(PersonType::tag.owns_id_json(), vec![1_i64.into_encoded_scalar()])],
        vec![],
    );
    let error = materialize_model_for_test::<Person>(&wrong_domain_row).unwrap_err();
    assert_eq!(error.code(), "wrong_scalar_domain");
    assert_eq!(error.field(), "tag[0]");

    let hydrated_player = HydratedPlayer::new(
        Person::TYPE_ID_JSON,
        Some("0xa".to_owned()),
        vec![],
    );
    let duplicate_role_row = HydratedRow::new(
        Membership::TYPE_ID_JSON,
        "0x2".to_owned(),
        vec![],
        vec![(
            MembershipType::participant.role_id_json(),
            vec![hydrated_player.clone(), hydrated_player],
        )],
    );
    let error = materialize_model_for_test::<Membership>(&duplicate_role_row).unwrap_err();
    assert_eq!(error.code(), "ordered_distinct_duplicate");
    assert_eq!(error.field(), "type.participant[1].type");

    let accepted_role_row = HydratedRow::new(
        Membership::TYPE_ID_JSON,
        "0x2".to_owned(),
        vec![],
        vec![(
            MembershipType::participant.role_id_json(),
            vec![
                HydratedPlayer::new(Person::TYPE_ID_JSON, Some("0xa".to_owned()), vec![]),
                HydratedPlayer::new(Person::TYPE_ID_JSON, Some("0xb".to_owned()), vec![]),
            ],
        )],
    );
    let accepted_membership: Membership = materialize_model_for_test(&accepted_role_row)?;
    assert_eq!(
        accepted_membership
            .participant()
            .iter()
            .map(|player| match player {
                MembershipParticipantPlayer::Person(reference) => reference.iid(),
            })
            .collect::<Vec<_>>(),
        [Some("0xa"), Some("0xb")],
    );

    let invalid_player_iid_row = HydratedRow::new(
        Membership::TYPE_ID_JSON,
        "0x2".to_owned(),
        vec![],
        vec![(
            MembershipType::participant.role_id_json(),
            vec![HydratedPlayer::new(
                Person::TYPE_ID_JSON,
                Some("person-a".to_owned()),
                vec![],
            )],
        )],
    );
    let error = materialize_model_for_test::<Membership>(&invalid_player_iid_row).unwrap_err();
    assert_eq!(error.code(), "noncanonical_iid");
    assert_eq!(error.field(), "type.iid");

    let accepted_row = HydratedRow::new(
        Person::TYPE_ID_JSON,
        "0x3".to_owned(),
        vec![(
            PersonType::tag.owns_id_json(),
            vec![
                first.value().into_encoded_scalar(),
                second.value().into_encoded_scalar(),
            ],
        )],
        vec![],
    );
    let accepted: Person = materialize_model_for_test(&accepted_row)?;
    assert_eq!(accepted.iid(), "0x3");
    assert_eq!(
        accepted.tag().iter().map(|value| value.value().as_str()).collect::<Vec<_>>(),
        ["first", "second"],
    );

    let noncanonical_iid_row = HydratedRow::new(
        Person::TYPE_ID_JSON,
        "person-3".to_owned(),
        vec![(
            PersonType::tag.owns_id_json(),
            vec![first.value().into_encoded_scalar()],
        )],
        vec![],
    );
    let error = materialize_model_for_test::<Person>(&noncanonical_iid_row).unwrap_err();
    assert_eq!(error.code(), "noncanonical_hydrated_iid");
    assert_eq!(error.field(), "type.iid");
    Ok(())
}
"#,
    )
    .unwrap();

    checked_command(
        Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .arg("run")
            .arg("--quiet")
            .arg("--offline")
            .arg("--manifest-path")
            .arg(consumer.join("Cargo.toml"))
            .env(
                "CARGO_TARGET_DIR",
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .unwrap()
                    .join("target/ordered-rust-phase2"),
            ),
        "ordered generated Rust common-validation consumer",
    );

    let foreign_generated = stage.path().join("foreign-generated");
    write_package(&package, &foreign_generated);
    let foreign_manifest = foreign_generated.join("Cargo.toml");
    let foreign_manifest_source = fs::read_to_string(&foreign_manifest).unwrap();
    fs::write(
        &foreign_manifest,
        foreign_manifest_source.replace(
            "name = \"type-bridge-generated-schema\"",
            "name = \"type-bridge-generated-schema-foreign\"",
        ),
    )
    .unwrap();
    let foreign_consumer = stage.path().join("foreign-consumer");
    fs::create_dir_all(foreign_consumer.join("src")).unwrap();
    fs::write(
        foreign_consumer.join("Cargo.toml"),
        format!(
            "[package]\nname=\"ordered-rust-foreign-fence\"\nversion=\"0.0.0\"\nedition=\"2024\"\npublish=false\n[dependencies]\nschema_a={{package=\"type-bridge-generated-schema\",path=\"../generated\"}}\nschema_b={{package=\"type-bridge-generated-schema-foreign\",path=\"../foreign-generated\"}}\ntype-bridge={{path=\"{rust_path}\",default-features=false}}\n[patch.crates-io]\ntype-bridge={{path=\"{rust_path}\"}}\n[workspace]\n"
        ),
    )
    .unwrap();
    fs::write(
        foreign_consumer.join("src/main.rs"),
        r#"fn foreign_package_value(player: schema_b::PersonRef) {
    let _ = schema_a::MembershipCreate::new(vec![player]);
}

fn main() {}
"#,
    )
    .unwrap();
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .arg("check")
        .arg("--quiet")
        .arg("--offline")
        .arg("--manifest-path")
        .arg(foreign_consumer.join("Cargo.toml"))
        .env(
            "CARGO_TARGET_DIR",
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("target/ordered-rust-phase2"),
        )
        .output()
        .unwrap();
    assert!(!output.status.success(), "foreign ordered package compiled");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("mismatched types")
            && stderr.contains("actually distinct types")
            && stderr.contains(
                "`PersonRef` is defined in crate `type_bridge_generated_schema_foreign`"
            )
            && stderr.contains(
                "`type_bridge_generated_schema::PersonRef` is defined in crate `type_bridge_generated_schema`"
            ),
        "foreign package rejection did not preserve nominal identities:\n{stderr}",
    );
}
