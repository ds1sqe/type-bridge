use std::collections::BTreeSet;

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
use type_bridge_schema_codegen::{CEmitter, PythonEmitter, RustEmitter, TypeScriptEmitter};

mod support;

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
    assert!(python_runtime.contains("_TYPE_BRIDGE_ORDERED_COLLECTION_RESOURCE_VERSION = 2"));

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
