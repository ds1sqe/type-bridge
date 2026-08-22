use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::projection::{
    BindingTarget, CSymbolPrefix, CodeResourceDigest, ModelProjection, ProjectedTokenKind,
    ProjectionConfig, RuntimeProjection, TargetIdentifier,
};
use type_bridge_contract::schema::{DocumentId, encode_declared_schema};
use type_bridge_schema::{
    SchemaDocumentSet, VerifiedSchemaAuthority, encode_schema_authority, normalize_documents,
    project, resolve,
};
use type_bridge_schema_codegen::{CEmitter, GeneratedPackage};

mod support;

// Mach-O records dead-strippable symbol atoms without ELF-style section splitting.
#[cfg(all(unix, target_os = "macos"))]
const DEAD_STRIP_COMPILE_FLAGS: &[&str] = &[];
#[cfg(all(unix, not(target_os = "macos")))]
const DEAD_STRIP_COMPILE_FLAGS: &[&str] = &["-ffunction-sections", "-fdata-sections"];
#[cfg(all(unix, target_os = "macos"))]
const DEAD_STRIP_LINK_FLAG: &str = "-Wl,-dead_strip";
#[cfg(all(unix, not(target_os = "macos")))]
const DEAD_STRIP_LINK_FLAG: &str = "-Wl,--gc-sections";

const SOURCE: &str = r#"format: typebridge.schema/v2
attributes:
  display-name:
    value: string
  count-value:
    value: integer
  ratio-value:
    value: double
  enabled-value:
    value: boolean
  date-value:
    value: date
  datetime-value:
    value: datetime
  zoned-value:
    value: datetime-tz
  decimal-value:
    value: decimal
  duration-value:
    value: duration
  aliases:
    value: string
  secondary-id:
    value: string
  score:
    value: integer
entities:
  actor:
    abstract: true
    owns:
      display-name: { key: true }
  person-record:
    sub: actor
    owns:
      count-value: { card: 1 }
      aliases: { card: { min: 0, max: 3 } }
      ratio-value: {}
      enabled-value: {}
      date-value: {}
      datetime-value: {}
      zoned-value: {}
      decimal-value: {}
      duration-value: {}
  dual-key-record:
    sub: actor
    owns:
      secondary-id: { key: true }
  person:
    sub: actor
    owns:
      score: { card: 1 }
  employee:
    sub: person
relations:
  container:
    relates: [item]
  event: {}
  membership:
    relates:
      member: { card: 1 }
plays:
  event:
    container: [item]
  person-record:
    membership: [member]
  dual-key-record:
    membership: [member]
structs:
  player-stats:
    fields:
      - { name: win-count, type: integer }
functions:
  qualifying-score:
    parameters:
      - { name: person, type: person }
      - { name: minimum, type: integer }
    returns: { scalar: integer }
    body: { typeql: "match $person has score $score-attribute; let $score = $score-attribute; $score >= $minimum; return first $score;" }
  string-identity:
    parameters:
      - { name: value, type: string }
    returns: { scalar: string }
    body: { typeql: "match let $result = $value; return first $result;" }
  find-events:
    parameters:
      - { name: input-event, type: event }
    returns: { stream: [event] }
    body: { typeql: "match $event isa event; return { $event };" }
"#;

const ORDERED_SUCCESSOR_SOURCE: &str = include_str!("c_abi_1_4/schema.yaml");
const WORKFORCE_V3_SOURCE: &str =
    include_str!("../../../../tests/contracts/sdk_conformance/workforce-v3/schema-v3.yaml");

fn workforce_v3_repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .canonicalize()
        .unwrap()
}

fn workforce_v3_source_identity(root: &Path, relative: &str) -> Value {
    let bytes = fs::read(root.join(relative)).expect("V3 proof source reads");
    json!({"path": relative, "sha256": format!("{:x}", Sha256::digest(bytes))})
}

fn publish_workforce_v3_package_fragment(results: Vec<Value>) {
    let destination = env::var_os("TYPE_BRIDGE_WORKFORCE_V3_PROOF_FRAGMENT");
    let nonce = env::var_os("TYPE_BRIDGE_WORKFORCE_V3_PROOF_RUN_NONCE");
    assert_eq!(destination.is_some(), nonce.is_some());
    let (Some(destination), Some(nonce)) = (destination, nonce) else {
        return;
    };
    let destination = PathBuf::from(destination);
    assert!(destination.is_absolute() && !destination.exists());
    let nonce = nonce.to_str().expect("V3 proof nonce is UTF-8");
    assert!(
        nonce.len() == 64
            && nonce
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    );
    let root = workforce_v3_repository_root();
    let sources = [
        "type-bridge-core/crates/c/include/typebridge/type_bridge.h",
        "type-bridge-core/crates/c/src/projected_model.rs",
        "type-bridge-core/crates/c/src/projected_token.rs",
        "type-bridge-core/crates/c/src/projected_value.rs",
        "type-bridge-core/crates/c/src/schema_package.rs",
        "type-bridge-core/crates/schema-codegen/src/c/render.rs",
        "type-bridge-core/crates/schema-codegen/tests/c_emitter.rs",
    ];
    let fragment = json!({
        "binding": "c",
        "contract": {
            "allowlist": workforce_v3_source_identity(&root, "tests/contracts/sdk_conformance/workforce-v3/proof-fragment-allowlist-v1.json"),
            "journey": workforce_v3_source_identity(&root, "tests/contracts/sdk_conformance/workforce-v3/journey-v3.json"),
            "proof_schema": workforce_v3_source_identity(&root, "tests/contracts/sdk_conformance/workforce-v3/proof-fragment-schema-v1.json"),
        },
        "format": "typebridge.workforce-v3-proof-fragment/v1",
        "producer": {"id": "type-bridge-c.generated-package-v3-proof", "sources": sources.iter().map(|path| workforce_v3_source_identity(&root, path)).collect::<Vec<_>>()},
        "results": results,
        "run_nonce": nonce,
        "semantic_profile": "typedb-3.12.1/v1",
    });
    let mut bytes = to_canonical_json(&fragment).expect("C V3 package fragment canonicalizes");
    bytes.push(b'\n');
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .expect("C V3 package fragment creates once");
    output
        .write_all(&bytes)
        .expect("C V3 package fragment writes");
    output.sync_all().expect("C V3 package fragment is durable");
}

fn projected(resources: &[CodeResourceDigest]) -> (RuntimeProjection, VerifiedSchemaAuthority) {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("c-emitter.yaml").expect("fixture document ID is valid"),
        SOURCE,
    )])
    .expect("C emitter fixture parses");
    let declared = normalize_documents(&documents).expect("C emitter fixture normalizes");
    let profile = SemanticProfileId::new(support::TEST_PROFILE).expect("test profile is valid");
    let resolved = resolve(&declared, &profile).expect("C emitter fixture resolves");
    let projection = project(
        &resolved,
        BindingTarget::C,
        &ProjectionConfig::c(CSymbolPrefix::new("acme").expect("test prefix is valid")),
        &CEmitter::new().generator_handlers(),
        resources,
    )
    .expect("C emitter fixture projects");
    (projection, support::authority(SOURCE))
}

fn ordered_successor_projected() -> (RuntimeProjection, VerifiedSchemaAuthority) {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("c-emitter-ordered.yaml").expect("fixture document ID is valid"),
        ORDERED_SUCCESSOR_SOURCE,
    )])
    .expect("ordered C emitter fixture parses");
    let declared = normalize_documents(&documents).expect("ordered C emitter fixture normalizes");
    let profile = SemanticProfileId::new(support::TEST_PROFILE).expect("test profile is valid");
    let resolved = resolve(&declared, &profile).expect("ordered C emitter fixture resolves");
    let emitter = CEmitter::new();
    let projection = project(
        &resolved,
        BindingTarget::C,
        &ProjectionConfig::c(CSymbolPrefix::new("acme_v3").expect("test prefix is valid")),
        &emitter.generator_handlers_for(&resolved),
        &emitter
            .code_resources_for(&resolved)
            .expect("ordered C resources hash"),
    )
    .expect("ordered C emitter fixture projects");
    (projection, support::authority(ORDERED_SUCCESSOR_SOURCE))
}

fn embedded_array(source: &str, name: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    for chunk_index in 0.. {
        let marker = format!("static const uint8_t {name}_chunk_{chunk_index}[] = {{\n");
        let Some((_, tail)) = source.split_once(&marker) else {
            assert!(chunk_index != 0, "generated source omitted {name}");
            break;
        };
        let body = tail
            .split_once("};\n")
            .unwrap_or_else(|| panic!("generated source did not terminate {name} chunk"))
            .0;
        bytes.extend(
            body.lines()
                .flat_map(|line| line.split(','))
                .filter_map(|item| {
                    let item = item.trim();
                    if item.is_empty() {
                        return None;
                    }
                    let hexadecimal = item
                        .strip_prefix("0x")
                        .and_then(|item| item.strip_suffix('u'))
                        .unwrap_or_else(|| panic!("invalid generated byte literal {item:?}"));
                    Some(
                        u8::from_str_radix(hexadecimal, 16)
                            .expect("generated byte literal is valid"),
                    )
                }),
        );
    }
    bytes
}

fn projected_nominal_names(projection: &RuntimeProjection) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for model in projection.models().values() {
        names.insert(model.target_name().as_str().to_owned());
        if let Some(name) = model.create().target_name() {
            names.insert(name.as_str().to_owned());
        }
        if let Some(name) = model.reference_read().target_name() {
            names.insert(name.as_str().to_owned());
        }
        if let Some(name) = model.query_tokens().target_name() {
            names.insert(name.as_str().to_owned());
        }
        for role in model.query_tokens().roles().values() {
            if let Some(name) = role.player_union_target_name() {
                names.insert(name.as_str().to_owned());
            }
        }
    }
    names.extend(
        projection
            .structs()
            .values()
            .map(|structure| structure.target_name().as_str().to_owned()),
    );
    names.extend(
        projection
            .functions()
            .values()
            .map(|function| function.target_name().as_str().to_owned()),
    );
    names.extend(
        projection
            .playing_facts()
            .values()
            .filter_map(|playing| playing.target_name())
            .map(|name| name.as_str().to_owned()),
    );
    names
}

fn emitted_nominal_names(header: &str) -> BTreeSet<String> {
    header
        .lines()
        .filter_map(|line| line.strip_prefix("typedef struct "))
        .filter(|line| !line.contains('{') && !line.contains("_chunks_v1_t "))
        .map(|line| {
            let (tag, alias) = line
                .split_once(' ')
                .expect("generated nominal typedef has a tag and alias");
            assert_eq!(alias, format!("{tag};"));
            tag.to_owned()
        })
        .collect()
}

fn emitted_function_names(header: &str) -> Vec<&str> {
    header
        .split("TYPE_BRIDGE_CALL ")
        .skip(1)
        .map(|tail| {
            tail.split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
                .next()
                .expect("generated function declaration has an identifier")
        })
        .collect()
}

fn emitted_function_parameter_counts(header: &str) -> Vec<(&str, usize)> {
    header
        .split("TYPE_BRIDGE_CALL ")
        .skip(1)
        .map(|tail| {
            let (name, parameters) = tail
                .split_once('(')
                .expect("generated function declaration opens its parameter list");
            let parameters = parameters
                .split_once(") {")
                .or_else(|| parameters.split_once(");"))
                .expect("generated function signature closes its parameter list")
                .0
                .trim();
            let count = if parameters == "void" || parameters.is_empty() {
                0
            } else {
                parameters.split(',').count()
            };
            (name.trim(), count)
        })
        .collect()
}

fn emitted_function_definition<'header>(header: &'header str, name: &str) -> &'header str {
    let marker = format!("static inline type_bridge_status_t TYPE_BRIDGE_CALL {name}(");
    let start = header
        .find(&marker)
        .unwrap_or_else(|| panic!("generated header omitted inline function {name}"));
    let open = header[start..]
        .find('{')
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("generated inline function {name} has no body"));
    let mut depth = 0_usize;
    for (offset, byte) in header.as_bytes()[open..].iter().copied().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &header[start..=open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("generated inline function {name} has an unterminated body")
}

fn emitted_function_model_token<'header>(header: &'header str, name: &str) -> &'header str {
    const MARKER: &str = "TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_TOKEN, &";
    let body = emitted_function_definition(header, name);
    let tail = body
        .split_once(MARKER)
        .unwrap_or_else(|| panic!("generated inline function {name} omits its model token"))
        .1;
    tail.split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .next()
        .expect("generated model token has an identifier")
}

#[test]
fn unordered_c_v2_five_file_package_remains_byte_exact() {
    let emitter = CEmitter::new();
    let resources = emitter.code_resources().expect("C resources hash");
    let (projection, authority) = projected(&resources);
    let package = emitter
        .emit(&projection, &authority)
        .expect("C package emits");
    let actual = package
        .files()
        .iter()
        .map(|(path, bytes)| format!("{path} {:x}", Sha256::digest(bytes)))
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        [
            "CMakeLists.txt 04af839fb3caa4234c17af6c2820da09c2cf1e32d4ab38b6e7dda906e6eec30a",
            "acme.pc.in 69a197ee74e47e9181184c214944227fad207cda5da286b5d51543414614e9a7",
            "acmeConfig.cmake.in dcf6298f6ea3427c9c4976d5e8deb3ad2a27fd9ce4405a6dc5a78b21a9235ce7",
            "include/acme/models.h 01ab3f24a6f05768294a63a3c87d73ce077f66d2a95606d8088b23096ebcfd33",
            "src/models.c 12c81896dbb83925de4dc51b3e6f3a76d2fa01f63b1c2d4235585e4470675a7d",
        ],
        "unordered C-v2 generated resources and embedded ABI 1.3 metadata changed",
    );
    for forbidden in [
        "type_bridge_abi_1_4.h",
        "_schema_package_open_v2",
        "_batch_builder_open",
        "_database_insert_v2",
    ] {
        assert!(
            package
                .files()
                .values()
                .all(|bytes| !String::from_utf8_lossy(bytes).contains(forbidden)),
            "unordered C-v2 package gained successor surface {forbidden}",
        );
    }
}

#[test]
fn ordered_c_v3_emits_exact_abi_1_5_admission_crud_batch_and_package_metadata() {
    let emitter = CEmitter::new();
    let (projection, authority) = ordered_successor_projected();
    let package = emitter
        .emit(&projection, &authority)
        .expect("ordered C-v3 package emits");
    assert_eq!(
        package
            .files()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "CMakeLists.txt",
            "acme_v3.pc.in",
            "acme_v3Config.cmake.in",
            "include/acme_v3/models.h",
            "src/models.c",
        ]),
    );

    let header = std::str::from_utf8(package.get("include/acme_v3/models.h").unwrap()).unwrap();
    let source = std::str::from_utf8(package.get("src/models.c").unwrap()).unwrap();
    let cmake = std::str::from_utf8(package.get("CMakeLists.txt").unwrap()).unwrap();
    let package_config =
        std::str::from_utf8(package.get("acme_v3Config.cmake.in").unwrap()).unwrap();
    let pkg_config = std::str::from_utf8(package.get("acme_v3.pc.in").unwrap()).unwrap();

    assert!(header.contains("#include <typebridge/type_bridge_abi_1_5.h>"));
    assert!(header.contains("ACME_V3_MIGRATION_HISTORY_RESOURCE"));
    assert!(header.contains("acme_v3_migration_catalog_open("));
    assert!(!header.contains("#include <typebridge/type_bridge.h>"));
    assert!(
        source
            .contains("sizeof(type_bridge_schema_package_chunked_descriptor_v1_t),\n  1u,\n  5u,"),
        "ordered descriptor must require literal ABI 1.5",
    );
    assert!(!source.contains("TYPE_BRIDGE_C_ABI_MINOR"));
    assert!(source.contains("type_bridge_schema_package_open_chunked_v1("));
    assert!(source.contains("type_bridge_schema_package_open_chunked_v2("));
    assert!(source.contains("type_bridge_migration_catalog_open("));
    assert!(header.contains("acme_v3_schema_package_open("));
    assert!(header.contains("acme_v3_schema_package_open_v2("));
    assert!(header.contains("type_bridge_execution_diagnostics_t **out_diagnostics"));
    assert!(cmake.contains("find_package(TypeBridge 1.5 CONFIG REQUIRED)"));
    assert!(package_config.contains("find_dependency(TypeBridge 1.5 CONFIG)"));
    assert!(pkg_config.contains("Requires: type-bridge >= 1.5.0, type-bridge < 2.0.0"));
    assert!(!cmake.contains("TypeBridge 1.3"));
    assert!(!package_config.contains("TypeBridge 1.3"));
    assert!(!pkg_config.contains("type-bridge >= 1.3.0"));

    for nominal in [
        "acme_v3_keyed_insert_batch_builder",
        "acme_v3_keyed_insert_batch",
        "acme_v3_keyed_insert_batch_result",
        "acme_v3_membership_put_batch_builder",
        "acme_v3_unkeyed_delete_batch_result",
        "acme_v3_gathering_update_batch",
    ] {
        assert!(
            header.contains(&format!("typedef struct {nominal} {nominal};")),
            "ordered header omitted nominal batch type {nominal}",
        );
    }
    for wrapper in [
        "acme_v3_keyed_database_insert_v2",
        "acme_v3_keyed_database_put_v2",
        "acme_v3_keyed_read_transaction_get_by_iid_v2",
        "acme_v3_keyed_write_transaction_update_v2",
        "acme_v3_membership_write_transaction_delete_by_iid_v2",
        "acme_v3_membership_database_count_v2",
        "acme_v3_keyed_insert_batch_builder_open",
        "acme_v3_keyed_insert_batch_builder_add",
        "acme_v3_keyed_insert_batch_builder_finish",
        "acme_v3_keyed_insert_batch_builder_close",
        "acme_v3_keyed_insert_batch_close",
        "acme_v3_keyed_database_insert_batch_execute",
        "acme_v3_keyed_write_transaction_insert_batch_execute",
        "acme_v3_keyed_insert_batch_result_count",
        "acme_v3_keyed_insert_batch_result_thing_at",
        "acme_v3_keyed_insert_batch_result_close",
        "acme_v3_gathering_delete_batch_builder_add",
        "acme_v3_gathering_delete_batch_result_count",
        "acme_v3_gathering_delete_batch_result_close",
    ] {
        assert!(
            header.contains(&format!("TYPE_BRIDGE_CALL {wrapper}(")),
            "ordered header omitted successor wrapper {wrapper}",
        );
        assert!(
            header.contains(&format!(
                "static inline type_bridge_status_t TYPE_BRIDGE_CALL {wrapper}("
            )),
            "successor wrapper {wrapper} does not have header-local static-inline linkage",
        );
    }
    for forbidden in [
        "acme_v3_unkeyed_database_put_v2",
        "acme_v3_unkeyed_write_transaction_put_v2",
        "acme_v3_unkeyed_put_batch",
        "acme_v3_gathering_database_put_v2",
        "acme_v3_gathering_write_transaction_put_v2",
        "acme_v3_gathering_put_batch",
        "acme_v3_keyed_delete_batch_result_thing_at",
        "acme_v3_unkeyed_delete_batch_result_thing_at",
        "acme_v3_membership_delete_batch_result_thing_at",
        "acme_v3_gathering_delete_batch_result_thing_at",
    ] {
        assert!(
            !header.contains(forbidden),
            "ordered header emitted forbidden nominal successor name {forbidden}",
        );
    }
    for generic in [
        "type_bridge_database_entity_insert_v2(",
        "type_bridge_read_transaction_entity_count_v2(",
        "type_bridge_write_transaction_relation_update_v2(",
        "type_bridge_projected_batch_builder_open_v1(",
        "type_bridge_projected_batch_builder_add_v1(",
        "type_bridge_projected_batch_builder_finish(",
        "type_bridge_database_projected_batch_execute_v1(",
        "type_bridge_write_transaction_projected_batch_execute_v1(",
        "type_bridge_projected_batch_result_count(",
        "type_bridge_projected_batch_result_thing_at(",
        "type_bridge_projected_batch_result_close(",
    ] {
        assert!(
            header.contains(generic),
            "successor facade does not delegate through frozen generic ABI call {generic}",
        );
    }
    assert!(
        !source.contains("acme_v3_keyed_database_insert_v2(")
            && !source.contains("acme_v3_keyed_insert_batch_builder_open("),
        "nominal CRUD and batch successors must not add generated source exports",
    );

    let crud_insert = emitted_function_definition(header, "acme_v3_keyed_database_insert_v2");
    for input in [
        "TYPE_BRIDGE_GENERATED_INPUT_DATABASE",
        "TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_TOKEN",
        "TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_CREATE",
        "TYPE_BRIDGE_GENERATED_INPUT_BYTES, limits, sizeof(*limits)",
        "TYPE_BRIDGE_GENERATED_INPUT_CANCELLATION",
    ] {
        assert!(
            crud_insert.contains(input),
            "CRUD preflight omitted {input}"
        );
    }
    assert!(crud_insert.contains("type_bridge_projected_thing_t *generic_entity = NULL;"));
    assert!(crud_insert.contains("&generic_entity, out_diagnostics);"));
    assert!(crud_insert.contains("*out_entity = (acme_v3_keyed *)generic_entity;"));
    assert!(!crud_insert.contains("(type_bridge_projected_thing_t **)out_entity"));

    let batch_open = emitted_function_definition(header, "acme_v3_keyed_insert_batch_builder_open");
    for input in [
        "TYPE_BRIDGE_GENERATED_INPUT_SCHEMA_PACKAGE",
        "TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_TOKEN",
        "TYPE_BRIDGE_GENERATED_INPUT_BYTES, construction_limits",
        "TYPE_BRIDGE_GENERATED_INPUT_CANCELLATION",
    ] {
        assert!(
            batch_open.contains(input),
            "batch-open preflight omitted {input}"
        );
    }
    assert!(batch_open.contains("type_bridge_projected_batch_builder_t *generic_builder = NULL;"));
    assert!(batch_open.contains("&generic_builder, out_diagnostics);"));
    assert!(!batch_open.contains("(type_bridge_projected_batch_builder_t **)out_builder"));

    let batch_add = emitted_function_definition(header, "acme_v3_keyed_insert_batch_builder_add");
    assert!(batch_add.contains("TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH_BUILDER"));
    assert!(batch_add.contains("TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_CREATE"));
    assert!(batch_add.contains("acme_v3_generated_alias_preflight_v1("));

    let batch_finish =
        emitted_function_definition(header, "acme_v3_keyed_insert_batch_builder_finish");
    assert_eq!(
        batch_finish
            .matches("acme_v3_generated_alias_preflight_v1(")
            .count(),
        2,
        "three finish outputs require the frozen two-call alias fence",
    );
    assert_eq!(
        batch_finish
            .matches("TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH_BUILDER")
            .count(),
        2,
    );
    assert!(batch_finish.contains("TYPE_BRIDGE_GENERATED_INPUT_BYTES, builder, sizeof(*builder)"));
    assert!(
        batch_finish.contains("TYPE_BRIDGE_GENERATED_INPUT_BYTES, out_batch, sizeof(*out_batch)")
    );
    assert!(batch_finish.contains("type_bridge_projected_batch_t *generic_batch = NULL;"));
    let assign_builder = batch_finish
        .find("*builder = (acme_v3_keyed_insert_batch_builder *)generic_builder;")
        .expect("finish assigns generic ownership state back to the nominal builder slot");
    let success_branch = batch_finish
        .find("if (status == TYPE_BRIDGE_STATUS_OK)")
        .expect("finish publishes its nominal batch only on success");
    assert!(
        assign_builder < success_branch,
        "failed finish must retain the builder"
    );
    assert!(!batch_finish.contains("(type_bridge_projected_batch_builder_t **)builder"));
    assert!(!batch_finish.contains("(type_bridge_projected_batch_t **)out_batch"));

    let batch_execute =
        emitted_function_definition(header, "acme_v3_keyed_database_insert_batch_execute");
    assert!(batch_execute.contains("TYPE_BRIDGE_GENERATED_INPUT_DATABASE"));
    assert!(batch_execute.contains("TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH"));
    assert!(batch_execute.contains("TYPE_BRIDGE_GENERATED_INPUT_BYTES, limits, sizeof(*limits)"));
    assert!(batch_execute.contains("type_bridge_projected_batch_result_t *generic_result = NULL;"));
    assert!(!batch_execute.contains("(type_bridge_projected_batch_result_t **)out_result"));

    let result_count =
        emitted_function_definition(header, "acme_v3_keyed_insert_batch_result_count");
    assert!(result_count.contains("TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH_RESULT"));
    assert!(result_count.contains("TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_TOKEN"));
    assert!(result_count.contains("TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT"));

    let thing_at =
        emitted_function_definition(header, "acme_v3_keyed_insert_batch_result_thing_at");
    assert!(thing_at.contains("TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH_RESULT"));
    assert!(thing_at.contains("type_bridge_projected_thing_t *generic_entity = NULL;"));
    assert!(!thing_at.contains("(type_bridge_projected_thing_t **)out_entity"));

    for (wrapper, input_kind, generic_type) in [
        (
            "acme_v3_keyed_insert_batch_builder_close",
            "TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH_BUILDER",
            "type_bridge_projected_batch_builder_t *generic_builder",
        ),
        (
            "acme_v3_keyed_insert_batch_close",
            "TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH",
            "type_bridge_projected_batch_t *generic_batch",
        ),
        (
            "acme_v3_keyed_insert_batch_result_close",
            "TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH_RESULT",
            "type_bridge_projected_batch_result_t *generic_result",
        ),
    ] {
        let body = emitted_function_definition(header, wrapper);
        assert!(
            body.contains(input_kind),
            "{wrapper} omitted its exact handle tag"
        );
        assert!(
            body.contains(generic_type),
            "{wrapper} omitted local ownership marshalling"
        );
        assert!(body.contains("acme_v3_generated_alias_preflight_v1("));
    }

    let names = emitted_function_names(header);
    assert_eq!(
        names.iter().copied().collect::<BTreeSet<_>>().len(),
        names.len(),
        "ordered generated function identifiers must be unique",
    );
    assert!(
        names.iter().all(|name| name.len() <= 255),
        "ordered generated function identifiers exceed the C hosted minimum",
    );
    assert!(
        emitted_function_parameter_counts(header)
            .iter()
            .all(|(_, count)| *count <= 127),
        "ordered generated function parameters exceed the C hosted minimum",
    );
}

#[cfg(unix)]
#[test]
fn ordered_c_v3_nominal_successors_compile_strictly_and_reject_cross_model_or_omitted_names() {
    let emitter = CEmitter::new();
    let (projection, authority) = ordered_successor_projected();
    let package = emitter
        .emit(&projection, &authority)
        .expect("ordered C-v3 package emits");
    let stage = TempDirectory::new();
    write_package(&package, stage.path());
    let runtime_include = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("c/include");

    let positive = include_str!("c_abi_1_4/positive.c");
    let wrong_model = include_str!("c_abi_1_4/wrong_model.c");
    let keyless_put = include_str!("c_abi_1_4/keyless_put.c");
    let delete_thing = include_str!("c_abi_1_4/delete_thing.c");
    for (name, contents) in [
        ("positive.c", positive),
        ("positive.cpp", positive),
        ("wrong-model.c", wrong_model),
        ("wrong-model.cpp", wrong_model),
        ("keyless-put.c", keyless_put),
        ("keyless-put.cpp", keyless_put),
        ("delete-thing.c", delete_thing),
        ("delete-thing.cpp", delete_thing),
    ] {
        fs::write(stage.path().join(name), contents).expect("ordered compiler probe is written");
    }

    let mut c_compilers = 0usize;
    for compiler in ["cc", "clang"] {
        if !command_exists(compiler) {
            continue;
        }
        c_compilers += 1;
        let source_output = Command::new(compiler)
            .args(["-std=c17", "-Wall", "-Wextra", "-Werror", "-pedantic"])
            .arg("-I")
            .arg(stage.path().join("include"))
            .arg("-I")
            .arg(&runtime_include)
            .arg("-c")
            .arg(stage.path().join("src/models.c"))
            .arg("-o")
            .arg(stage.path().join(format!("models-{compiler}.o")))
            .output()
            .expect("ordered generated C source compiler launches");
        assert!(
            source_output.status.success(),
            "{compiler} rejected ordered generated source:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&source_output.stdout),
            String::from_utf8_lossy(&source_output.stderr),
        );
        let positive_output = Command::new(compiler)
            .args(["-std=c17", "-Wall", "-Wextra", "-Werror", "-pedantic"])
            .arg("-I")
            .arg(stage.path().join("include"))
            .arg("-I")
            .arg(&runtime_include)
            .arg("-c")
            .arg(stage.path().join("positive.c"))
            .arg("-o")
            .arg(stage.path().join(format!("positive-{compiler}.o")))
            .output()
            .expect("ordered positive C compiler launches");
        assert!(
            positive_output.status.success(),
            "{compiler} rejected ordered positive C probe:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&positive_output.stdout),
            String::from_utf8_lossy(&positive_output.stderr),
        );
        for negative in ["wrong-model.c", "keyless-put.c", "delete-thing.c"] {
            let output = Command::new(compiler)
                .args(["-std=c17", "-Wall", "-Wextra", "-Werror", "-pedantic"])
                .arg("-I")
                .arg(stage.path().join("include"))
                .arg("-I")
                .arg(&runtime_include)
                .arg("-c")
                .arg(stage.path().join(negative))
                .arg("-o")
                .arg(stage.path().join(format!("negative-{compiler}.o")))
                .output()
                .expect("ordered negative C compiler launches");
            assert!(
                !output.status.success(),
                "{compiler} accepted forbidden ordered C probe {negative}",
            );
        }
    }
    assert!(c_compilers > 0, "one strict C17 compiler is required");

    let mut cpp_compilers = 0usize;
    for compiler in ["c++", "clang++"] {
        if !command_exists(compiler) {
            continue;
        }
        cpp_compilers += 1;
        let positive_output = Command::new(compiler)
            .args(["-std=c++17", "-Wall", "-Wextra", "-Werror", "-pedantic"])
            .arg("-I")
            .arg(stage.path().join("include"))
            .arg("-I")
            .arg(&runtime_include)
            .arg("-c")
            .arg(stage.path().join("positive.cpp"))
            .arg("-o")
            .arg(stage.path().join(format!("positive-{compiler}.o")))
            .output()
            .expect("ordered positive C++ compiler launches");
        assert!(
            positive_output.status.success(),
            "{compiler} rejected ordered positive C++ probe:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&positive_output.stdout),
            String::from_utf8_lossy(&positive_output.stderr),
        );
        for negative in ["wrong-model.cpp", "keyless-put.cpp", "delete-thing.cpp"] {
            let output = Command::new(compiler)
                .args(["-std=c++17", "-Wall", "-Wextra", "-Werror", "-pedantic"])
                .arg("-I")
                .arg(stage.path().join("include"))
                .arg("-I")
                .arg(&runtime_include)
                .arg("-c")
                .arg(stage.path().join(negative))
                .arg("-o")
                .arg(stage.path().join(format!("negative-{compiler}.o")))
                .output()
                .expect("ordered negative C++ compiler launches");
            assert!(
                !output.status.success(),
                "{compiler} accepted forbidden ordered C++ probe {negative}",
            );
        }
    }
    assert!(cpp_compilers > 0, "one strict C++17 compiler is required");
}

#[cfg(unix)]
#[test]
fn ordered_c_v3_nominal_alias_and_recovery_wrappers_execute_provider_free() {
    let emitter = CEmitter::new();
    let (projection, authority) = ordered_successor_projected();
    let package = emitter
        .emit(&projection, &authority)
        .expect("ordered C-v3 package emits");
    let stage = TempDirectory::new();
    write_package(&package, stage.path());
    let runtime_include = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("c/include");
    let header = std::str::from_utf8(package.get("include/acme_v3/models.h").unwrap()).unwrap();
    let model_token = emitted_function_model_token(header, "acme_v3_keyed_database_insert_v2");
    let probe = include_str!("c_abi_1_4/recovery.c").replace("@MODEL_TOKEN@", model_token);
    assert!(!probe.contains("@MODEL_TOKEN@"));
    fs::write(stage.path().join("recovery.c"), probe).expect("recovery probe is written");

    let mut invocations = 0usize;
    for compiler in ["cc", "clang"] {
        if !command_exists(compiler) {
            continue;
        }
        invocations += 1;
        let executable = stage.path().join(format!("recovery-{compiler}"));
        let mut command = Command::new(compiler);
        command
            .args(["-std=c17", "-Wall", "-Wextra", "-Werror", "-pedantic"])
            .args(DEAD_STRIP_COMPILE_FLAGS)
            .arg("-I")
            .arg(stage.path().join("include"))
            .arg("-I")
            .arg(&runtime_include)
            .arg(stage.path().join("recovery.c"))
            .arg(DEAD_STRIP_LINK_FLAG)
            .arg("-o")
            .arg(&executable);
        let output = command
            .output()
            .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
        assert!(
            output.status.success(),
            "{compiler} rejected the generated recovery probe:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let output = Command::new(&executable)
            .output()
            .unwrap_or_else(|error| panic!("failed to run {compiler} recovery probe: {error}"));
        assert!(
            output.status.success(),
            "{compiler} recovery probe failed with {}:\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    assert!(invocations > 0, "one strict C17 compiler is required");
}

#[test]
fn emits_one_deterministic_package_with_exact_canonical_evidence_and_nominal_names() {
    let emitter = CEmitter::new();
    let resources = emitter.code_resources().expect("C resources hash");
    let (projection, authority) = projected(&resources);
    let first = emitter
        .emit(&projection, &authority)
        .expect("C package emits");
    let second = emitter
        .emit(&projection, &authority)
        .expect("C package re-emits");

    assert_eq!(first, second);
    assert_eq!(
        first
            .files()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "CMakeLists.txt",
            "acme.pc.in",
            "acmeConfig.cmake.in",
            "include/acme/models.h",
            "src/models.c",
        ]),
    );

    let cmake = std::str::from_utf8(first.get("CMakeLists.txt").unwrap()).unwrap();
    assert!(cmake.contains("find_package(TypeBridge 1.3 CONFIG REQUIRED)"));
    assert!(cmake.contains("RELATIVE_PATH TYPE_BRIDGE_PC_PREFIX_FROM_PCDIR"));
    assert!(cmake.contains("CMAKE_INSTALL_LIBDIR must be a non-empty relative path"));
    assert!(cmake.contains("PROPERTIES EXPORT_NAME schema POSITION_INDEPENDENT_CODE ON"));
    assert!(cmake.contains("configure_package_config_file("));
    assert!(cmake.contains("install(\n  EXPORT acmeTargets"));
    let package_config = std::str::from_utf8(first.get("acmeConfig.cmake.in").unwrap()).unwrap();
    assert!(package_config.contains("find_dependency(TypeBridge 1.3 CONFIG)"));
    assert!(package_config.contains("acmeTargets.cmake"));
    let pkg_config = std::str::from_utf8(first.get("acme.pc.in").unwrap()).unwrap();
    assert!(pkg_config.contains("prefix=${pcfiledir}/@TYPE_BRIDGE_PC_PREFIX_FROM_PCDIR@"));
    assert!(pkg_config.contains("includedir=${prefix}/@CMAKE_INSTALL_INCLUDEDIR@"));
    assert!(pkg_config.contains("Requires: type-bridge >= 1.3.0, type-bridge < 2.0.0"));
    assert!(pkg_config.contains("Libs: -L${libdir} -lacme_schema"));

    let source = std::str::from_utf8(first.get("src/models.c").expect("C source is emitted"))
        .expect("generated C source is UTF-8");
    assert!(
        source
            .contains("sizeof(type_bridge_schema_package_chunked_descriptor_v1_t),\n  1u,\n  3u,")
    );
    assert!(!source.contains("TYPE_BRIDGE_C_ABI_MAJOR"));
    assert!(!source.contains("TYPE_BRIDGE_C_ABI_MINOR"));
    assert_eq!(
        embedded_array(source, "acme_schema_authority_json"),
        encode_schema_authority(&authority),
    );
    assert_eq!(
        embedded_array(source, "acme_declared_schema_json"),
        encode_declared_schema(authority.declared_schema()).expect("declared schema encodes"),
    );
    assert_eq!(
        embedded_array(source, "acme_runtime_projection_json"),
        to_canonical_json(&projection).expect("projection encodes"),
    );
    assert_eq!(
        embedded_array(source, "acme_semantic_fingerprint_json"),
        to_canonical_json(projection.semantic_fingerprint()).expect("semantic fingerprint encodes"),
    );
    assert_eq!(
        embedded_array(source, "acme_binding_fingerprint_json"),
        to_canonical_json(projection.projection_fingerprint())
            .expect("binding fingerprint encodes"),
    );
    assert_eq!(
        embedded_array(source, "acme_managed_scope"),
        authority.managed_scope().id().as_str().as_bytes(),
    );
    assert_eq!(
        embedded_array(source, "acme_semantic_profile"),
        authority.semantic_profile().id().as_str().as_bytes(),
    );

    let header = std::str::from_utf8(
        first
            .get("include/acme/models.h")
            .expect("C models header is emitted"),
    )
    .expect("generated C header is UTF-8");
    assert!(
        !source
            .lines()
            .any(|line| line.contains("TYPE_BRIDGE_CALL") && line.contains("_query_")),
        "generated nominal query wrappers must not retain external source definitions",
    );
    let inline_query_signatures = header
        .lines()
        .filter(|line| line.contains("TYPE_BRIDGE_CALL") && line.contains("_query_"))
        .collect::<Vec<_>>();
    assert!(!inline_query_signatures.is_empty());
    assert!(header.contains("have no cross-TU identity contract"));
    for declaration in [
        "extern const type_bridge_projected_token_v1_t acme_projected_function_token_0;",
        "extern const type_bridge_projected_token_v1_t acme_projected_function_token_1;",
        "extern const type_bridge_projected_token_v1_t acme_projected_function_token_2;",
        "typedef struct acme_qualifyingzhscore_query_call acme_qualifyingzhscore_query_call;",
        "typedef struct acme_query_function_integer_input acme_query_function_integer_input;",
        "typedef struct acme_person_query_function_argument_v1_t {",
        "typedef struct acme_query_function_integer_argument_v1_t {",
        "typedef struct acme_qualifyingzhscore_query_arguments_v1_t {",
        "type_bridge_query_function_arguments_header_v1_t header;",
        "acme_person_query_function_argument_v1_t argument_person;",
        "acme_query_function_integer_argument_v1_t argument_minimum;",
        "offsetof(acme_qualifyingzhscore_query_arguments_v1_t, argument_person)",
        "offsetof(acme_qualifyingzhscore_query_arguments_v1_t, argument_minimum)",
        "static inline type_bridge_status_t TYPE_BRIDGE_CALL acme_qualifyingzhscore_query_open(",
        "static inline type_bridge_status_t TYPE_BRIDGE_CALL acme_qualifyingzhscore_query_call_open(",
        "static inline acme_query_function_integer_call_v1_t TYPE_BRIDGE_CALL acme_qualifyingzhscore_query_call_ref(",
        "static inline acme_person_query_function_argument_v1_t TYPE_BRIDGE_CALL acme_person_query_function_argument_v1_t_from_person_exact(",
        "static inline acme_person_query_function_argument_v1_t TYPE_BRIDGE_CALL acme_person_query_function_argument_v1_t_from_employee_subtypes(",
        "static inline acme_query_function_integer_argument_v1_t TYPE_BRIDGE_CALL acme_query_function_integer_argument_from_input(",
        "static inline acme_query_function_integer_argument_v1_t TYPE_BRIDGE_CALL acme_query_function_integer_argument_from_call(",
        "static inline type_bridge_status_t TYPE_BRIDGE_CALL acme_score_query_function_input_open(",
        "static inline type_bridge_status_t TYPE_BRIDGE_CALL acme_qualifyingzhscore_query_call_greater_than_or_equal_field(",
        "static inline type_bridge_status_t TYPE_BRIDGE_CALL acme_query_function_integer_field_less_than_call(",
    ] {
        assert!(
            header.contains(declaration),
            "generated function facade omitted {declaration}",
        );
    }
    assert!(
        !header.contains("acme_findzhevents_query_open("),
        "stream-return functions must remain token-only in the scalar query facade",
    );
    assert!(
        !source.lines().any(|line| {
            line.contains("TYPE_BRIDGE_CALL")
                && (line.contains("qualifyingzhscore_query")
                    || line.contains("query_function_integer"))
        }),
        "generated function wrappers must retain header-local static-inline linkage",
    );
    assert!(
        !header.contains("generated_zero_memory") && !header.contains("memset(&descriptor"),
        "generated query descriptors must use semantic NULL aggregate initialization, not byte-zero pointer representations",
    );
    assert!(
        inline_query_signatures
            .iter()
            .all(|line| line.trim_start().starts_with("static inline ")),
        "every generated nominal query wrapper must have header-local static-inline linkage",
    );
    let emitted_nominals = emitted_nominal_names(header);
    let projected_nominals = projected_nominal_names(&projection);
    assert!(
        projected_nominals.is_subset(&emitted_nominals),
        "the emitter must retain every final projected identifier verbatim",
    );
    assert!(
        emitted_nominals.iter().all(|name| name.len() <= 255),
        "emitter-derived nominal query identifiers must remain bounded",
    );
    for expected in [
        "acme_personzhrecord",
        "acme_personzhrecord_create",
        "acme_personzhrecord_ref",
        "acme_personzhrecord_type",
        "acme_dualzhkeyzhrecord",
        "acme_dualzhkeyzhrecord_create",
        "acme_dualzhkeyzhrecord_ref",
        "acme_container_item_player",
        "acme_membership",
        "acme_membership_create",
        "acme_membership_ref",
        "acme_membership_member_player",
        "acme_playerzhstats",
        "acme_findzhevents",
        "acme_plays_event_relation_container_role_item",
    ] {
        assert!(
            header.contains(&format!("typedef struct {expected} {expected};")),
            "generated header omitted final nominal identifier {expected}",
        );
    }
    assert!(header.contains("type_bridge_status_t TYPE_BRIDGE_CALL acme_schema_package_open("));
    let parameter_counts = emitted_function_parameter_counts(header);
    assert!(
        parameter_counts.iter().all(|(_, count)| *count <= 127),
        "generated declarations exceed the C11 127-parameter minimum",
    );
    assert_eq!(
        parameter_counts
            .iter()
            .find(|(name, _)| *name == "acme_personzhrecord_create_open")
            .map(|(_, count)| *count),
        Some(4),
        "generated create open must retain its frozen four-parameter surface",
    );
    assert!(source.contains("type_bridge_schema_package_open_chunked_v1("));
    assert!(source.contains("acme_schema_package_chunks_v1"));
    for parameter in [
        "const acme_displayzhname *field_displayzhname;",
        "const acme_personzhrecord_create_field_aliases_chunks_v1_t *field_aliases_chunks;",
        "const acme_countzhvalue *field_countzhvalue;",
        "const acme_personzhrecord_create_args_v1_t *args,",
    ] {
        assert!(header.contains(parameter));
    }
    assert!(header.contains("ACME_SEQUENCE_OBJECT_BYTES_MAX / sizeof(values[0])"));
    assert!(header.contains("The aggregate chain count is at most"));
    assert!(
        header.contains("type_bridge_status_t TYPE_BRIDGE_CALL acme_personzhrecord_ref_from_key(")
    );
    assert!(header.contains(
        "type_bridge_status_t TYPE_BRIDGE_CALL acme_dualzhkeyzhrecord_ref_from_displayzhname("
    ));
    assert!(header.contains(
        "type_bridge_status_t TYPE_BRIDGE_CALL acme_dualzhkeyzhrecord_ref_from_secondaryzhid("
    ));
    assert!(!header.contains("acme_dualzhkeyzhrecord_ref_from_key("));
    assert!(
        header.contains("type_bridge_status_t TYPE_BRIDGE_CALL acme_personzhrecord_displayzhname(")
    );
    assert!(
        header.contains("type_bridge_status_t TYPE_BRIDGE_CALL acme_personzhrecord_ratiozhvalue(")
    );
    assert!(
        header.contains("type_bridge_status_t TYPE_BRIDGE_CALL acme_personzhrecord_aliases_count(")
    );
    assert!(
        header.contains("type_bridge_status_t TYPE_BRIDGE_CALL acme_personzhrecord_aliases_at(")
    );
    assert!(!header.contains("acme_personzhrecord_countzhvalue_count("));
    assert!(!header.contains("acme_actor_database_"));
    for declaration in [
        "typedef uint32_t acme_membership_member_player_kind_t;",
        "#define acme_membership_member_player_kind_unknown UINT32_C(0)",
        "#define acme_membership_member_player_kind_dualzhkeyzhrecord UINT32_C(1)",
        "#define acme_membership_member_player_kind_personzhrecord UINT32_C(2)",
        "type_bridge_status_t TYPE_BRIDGE_CALL acme_membership_member_player_from_personzhrecord(",
        "type_bridge_status_t TYPE_BRIDGE_CALL acme_membership_member_player_as_personzhrecord(",
        "type_bridge_status_t TYPE_BRIDGE_CALL acme_membership_member_player_from_dualzhkeyzhrecord(",
        "type_bridge_status_t TYPE_BRIDGE_CALL acme_membership_member_player_as_dualzhkeyzhrecord(",
        "type_bridge_status_t TYPE_BRIDGE_CALL acme_membership_member_player_kind(",
        "type_bridge_status_t TYPE_BRIDGE_CALL acme_membership_member_player_close(",
        "const acme_membership_member_player *role_member;",
        "type_bridge_status_t TYPE_BRIDGE_CALL acme_membership_member(",
        "type_bridge_status_t TYPE_BRIDGE_CALL acme_container_item(",
        "type_bridge_status_t TYPE_BRIDGE_CALL acme_container_item_player_from_event(",
    ] {
        assert!(
            header.contains(declaration),
            "generated header omitted relation declaration {declaration}",
        );
    }
    assert!(!header.contains("typedef enum acme_membership_member_player"));
    for suffix in [
        "database_insert",
        "database_put",
        "database_get_by_iid",
        "database_update",
        "database_delete_by_iid",
        "database_count",
        "read_transaction_get_by_iid",
        "read_transaction_count",
        "write_transaction_insert",
        "write_transaction_put",
        "write_transaction_get_by_iid",
        "write_transaction_update",
        "write_transaction_delete_by_iid",
        "write_transaction_count",
    ] {
        assert!(
            header.contains(&format!(
                "type_bridge_status_t TYPE_BRIDGE_CALL acme_personzhrecord_{suffix}("
            )),
            "generated header omitted typed entity CRUD wrapper {suffix}",
        );
        assert!(
            header.contains(&format!(
                "type_bridge_status_t TYPE_BRIDGE_CALL acme_membership_{suffix}("
            )),
            "generated header omitted typed relation CRUD wrapper {suffix}",
        );
    }
    assert!(source.contains("status = type_bridge_database_entity_insert("));
    assert!(source.contains("owner, &acme_projected_model_token_"));
    assert!(source.contains("(const type_bridge_projected_create_t *)create, cancellation,"));
    assert!(source.contains("status = type_bridge_read_transaction_entity_get_by_iid("));
    assert!(source.contains("status = type_bridge_database_relation_insert("));
    assert!(source.contains("status = type_bridge_read_transaction_relation_get_by_iid("));
    assert!(source.contains("status = type_bridge_write_transaction_relation_update("));
    assert!(source.contains("status = type_bridge_projected_reference_validate_role("));
    assert!(
        source
            .matches("status = type_bridge_projected_thing_validate_model(")
            .count()
            >= 2,
        "typed thing IID and reference adoption must both fence the exact model",
    );
    assert!(source.contains("status = type_bridge_projected_reference_validate_model("));
    assert!(source.contains("status = type_bridge_projected_value_validate_model("));
    assert!(source.contains("&acme_projected_model_token_"));
    assert!(header.contains("type_bridge_generated_opaque_alias_preflight_v1("));
    assert!(!source.contains("generated_ranges_overlap"));
    assert!(!header.contains("generated_ranges_overlap"));
    assert!(source.contains("TYPE_BRIDGE_GENERATED_INPUT_CREATE_ARGS_GRAPH"));
    assert!(source.contains("acme_personzhrecord_create_members_v1[]"));
    assert!(
        source.contains("offsetof(acme_personzhrecord_create_args_v1_t, field_aliases_chunks)")
    );
    assert!(header.contains("acme_personzhrecord_create_args_v1_t"));
    assert!(header.contains("acme_personzhrecord_create_field_aliases_chunks_v1_t"));
    assert!(source.contains("type_bridge_projected_create_builder_open_v1("));
    assert!(source.contains("type_bridge_projected_create_builder_add_v1("));
    assert!(source.contains("type_bridge_projected_create_builder_finish("));
    assert!(source.contains("type_bridge_projected_create_builder_close(&builder)"));
    assert!(source.contains("TYPE_BRIDGE_PROJECTED_CREATE_BUILDER_CHUNK_LEN_MAX"));
    assert!(!source.contains("#include <stdlib.h>"));
    assert!(source.contains("if (args->field_aliases_chunks != NULL)"));
    assert!(source.contains("stream_count = 0u;"));
    assert!(source.contains("value_chunk[stream_count + copy_index]"));
    assert!(source.contains("if (stream_count != 0u)"));
    assert!(source.contains("if (args->field_ratiozhvalue != NULL)"));
    assert!(!source.contains("args->field_aliases_chunks == NULL"));
    assert!(!source.contains("== NULL ? NULL : value_chunk"));
    assert!(!source.contains("== NULL ? NULL : reference_chunk"));
    assert!(!source.contains("malloc("));
    assert!(!source.contains("type_bridge_projected_field_input_v1_t fields["));
    for (target, runtime_open, runtime_value) in [
        (
            "acme_displayzhname",
            "type_bridge_projected_value_string_open",
            "type_bridge_projected_value_text",
        ),
        (
            "acme_countzhvalue",
            "type_bridge_projected_value_long_open",
            "type_bridge_projected_value_long",
        ),
        (
            "acme_ratiozhvalue",
            "type_bridge_projected_value_double_open",
            "type_bridge_projected_value_double_bits",
        ),
        (
            "acme_enabledzhvalue",
            "type_bridge_projected_value_boolean_open",
            "type_bridge_projected_value_boolean",
        ),
        (
            "acme_datezhvalue",
            "type_bridge_projected_value_date_open",
            "type_bridge_projected_value_text",
        ),
        (
            "acme_datetimezhvalue",
            "type_bridge_projected_value_datetime_open",
            "type_bridge_projected_value_text",
        ),
        (
            "acme_zzonedzhvalue",
            "type_bridge_projected_value_datetime_tz_open",
            "type_bridge_projected_value_text",
        ),
        (
            "acme_decimalzhvalue",
            "type_bridge_projected_value_decimal_open",
            "type_bridge_projected_value_text",
        ),
        (
            "acme_durationzhvalue",
            "type_bridge_projected_value_duration_open",
            "type_bridge_projected_value_text",
        ),
    ] {
        assert!(
            header.contains(&format!(
                "type_bridge_status_t TYPE_BRIDGE_CALL {target}_open("
            )),
            "generated header omitted {target}_open"
        );
        assert!(
            header.contains(&format!(
                "type_bridge_status_t TYPE_BRIDGE_CALL {target}_value("
            )),
            "generated header omitted {target}_value"
        );
        assert!(
            header.contains(&format!(
                "type_bridge_status_t TYPE_BRIDGE_CALL {target}_close("
            )),
            "generated header omitted {target}_close"
        );
        assert!(
            source.contains(&format!("status = {runtime_open}(package,")),
            "generated source omitted {runtime_open} for {target}",
        );
        assert!(
            source.contains(&format!("return {runtime_value}(")),
            "generated source omitted {runtime_value} for {target}",
        );
    }

    let token_digest = projection
        .projection_fingerprint()
        .as_fingerprint()
        .digest()
        .bytes()
        .into_iter()
        .map(|byte| format!("0x{byte:02x}u, "))
        .collect::<String>();
    let mut token_count = 0_usize;
    for (kind, kind_name) in [
        (ProjectedTokenKind::Model, "model"),
        (ProjectedTokenKind::Field, "field"),
        (ProjectedTokenKind::Role, "role"),
        (ProjectedTokenKind::Function, "function"),
    ] {
        let mut ordinal = 0_u32;
        while projection.projected_token_identity(kind, ordinal).is_some() {
            let symbol = format!("acme_projected_{kind_name}_token_{ordinal}");
            assert!(header.contains(&format!(
                "extern const type_bridge_projected_token_v1_t {symbol};"
            )));
            let initializer = format!(
                "const type_bridge_projected_token_v1_t {symbol} = {{\n  \
                 sizeof(type_bridge_projected_token_v1_t),\n  1u,\n  {}u,\n  \
                 {ordinal}u,\n  {{ {token_digest}}},\n  {{ 0u, 0u, 0u, 0u }}\n}};",
                kind.as_u32(),
            );
            assert!(
                source.contains(&initializer),
                "generated source omitted exact token initializer {symbol}",
            );
            token_count += 1;
            ordinal += 1;
        }
    }
    assert_eq!(
        source
            .matches("const type_bridge_projected_token_v1_t acme_projected_")
            .count(),
        token_count,
    );
}

#[cfg(target_os = "linux")]
#[test]
fn generated_create_coalesces_the_maximal_one_element_chunk_chain() {
    let emitter = CEmitter::new();
    let resources = emitter.code_resources().expect("C resources hash");
    let (projection, authority) = projected(&resources);
    let package = emitter
        .emit(&projection, &authority)
        .expect("C package emits");
    let stage = TempDirectory::new();
    write_package(&package, stage.path());
    let consumer = stage.path().join("coalesced-create.c");
    fs::write(
        &consumer,
        r#"#include <stdint.h>
#include <stdlib.h>

#include <acme/models.h>

static size_t alias_preflight_calls;
static size_t builder_add_calls;
static size_t builder_value_count;
static size_t builder_close_calls;
static int stream_shape_valid = 1;

type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_generated_opaque_alias_preflight_v1(
    const type_bridge_generated_opaque_input_v1_t *inputs,
    size_t input_count,
    const type_bridge_generated_output_range_v1_t *outputs,
    size_t output_count) {
  (void)inputs;
  (void)input_count;
  (void)outputs;
  (void)output_count;
  ++alias_preflight_calls;
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_create_builder_open_v1(
    const type_bridge_schema_package_t *package,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_projected_create_builder_t **out_builder,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  (void)package;
  (void)model;
  *out_builder = (type_bridge_projected_create_builder_t *)(uintptr_t)1u;
  *out_diagnostics = NULL;
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_create_builder_add_v1(
    type_bridge_projected_create_builder_t *builder,
    const type_bridge_projected_token_v1_t *member,
    const type_bridge_projected_value_t *const *values,
    size_t value_count,
    const type_bridge_projected_reference_t *const *references,
    size_t reference_count,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  (void)member;
  if (builder != (type_bridge_projected_create_builder_t *)(uintptr_t)1u ||
      values == NULL || value_count !=
          TYPE_BRIDGE_PROJECTED_CREATE_BUILDER_CHUNK_LEN_MAX ||
      references != NULL || reference_count != 0u) {
    stream_shape_valid = 0;
  }
  ++builder_add_calls;
  builder_value_count += value_count;
  *out_diagnostics = NULL;
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_create_builder_finish(
    type_bridge_projected_create_builder_t **builder,
    type_bridge_projected_create_t **out_create,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  if (*builder != (type_bridge_projected_create_builder_t *)(uintptr_t)1u) {
    return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
  }
  *builder = NULL;
  *out_create = (type_bridge_projected_create_t *)(uintptr_t)1u;
  *out_diagnostics = NULL;
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_create_builder_close(
    type_bridge_projected_create_builder_t **builder) {
  ++builder_close_calls;
  *builder = NULL;
  return TYPE_BRIDGE_STATUS_OK;
}

int main(void) {
  const acme_aliases *const value =
      (const acme_aliases *)(uintptr_t)1u;
  acme_personzhrecord_create_field_aliases_chunks_v1_t *head = NULL;
  acme_personzhrecord_create_args_v1_t args = {0};
  const type_bridge_schema_package_t *package =
      (const type_bridge_schema_package_t *)(uintptr_t)1u;
  acme_personzhrecord_create *create = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  type_bridge_status_t status;
  size_t index;

  for (index = 0u; index < TYPE_BRIDGE_PROJECTED_COLLECTION_LEN_MAX; ++index) {
    acme_personzhrecord_create_field_aliases_chunks_v1_t *chunk =
        (acme_personzhrecord_create_field_aliases_chunks_v1_t *)
            calloc(1u, sizeof(*chunk));
    if (chunk == NULL) {
      return 1;
    }
    chunk->struct_size = sizeof(*chunk);
    chunk->version = ACME_CREATE_ARGS_VERSION;
    chunk->values = &value;
    chunk->count = 1u;
    chunk->next = head;
    head = chunk;
  }
  args.struct_size = sizeof(args);
  args.version = ACME_CREATE_ARGS_VERSION;
  args.field_aliases_chunks = head;
  status = acme_personzhrecord_create_open(
      package, &args, &create, &diagnostics);
  while (head != NULL) {
    acme_personzhrecord_create_field_aliases_chunks_v1_t *next =
        (acme_personzhrecord_create_field_aliases_chunks_v1_t *)head->next;
    free(head);
    head = next;
  }

  return status == TYPE_BRIDGE_STATUS_OK &&
                 create == (acme_personzhrecord_create *)(uintptr_t)1u &&
                 diagnostics == NULL && stream_shape_valid != 0 &&
                 alias_preflight_calls == 2u && builder_add_calls == 256u &&
                 builder_value_count == TYPE_BRIDGE_PROJECTED_COLLECTION_LEN_MAX &&
                 builder_close_calls == 1u
             ? 0
             : 2;
}
"#,
    )
    .expect("coalescing consumer is written");

    let runtime_include = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../c/include")
        .canonicalize()
        .expect("C runtime include directory exists");
    let generated_include = stage.path().join("include");
    let generated_source = stage.path().join("src/models.c");
    let mut invocations = 0;
    #[cfg(unix)]
    for compiler in ["gcc", "clang"] {
        if !command_exists(compiler) {
            continue;
        }
        invocations += 1;
        let executable = stage.path().join(format!("coalesced-create-{compiler}"));
        let output = Command::new(compiler)
            .args([
                "-std=c11",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pedantic-errors",
            ])
            .args(DEAD_STRIP_COMPILE_FLAGS)
            .arg("-I")
            .arg(&runtime_include)
            .arg("-I")
            .arg(&generated_include)
            .arg(&generated_source)
            .arg(&consumer)
            .arg(DEAD_STRIP_LINK_FLAG)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
        assert!(
            output.status.success(),
            "{compiler} could not link the generated coalescing probe:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let output = Command::new(&executable)
            .output()
            .unwrap_or_else(|error| panic!("failed to run {}: {error}", executable.display()));
        assert!(
            output.status.success(),
            "{compiler}'s generated coalescing probe failed with {}:\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    #[cfg(windows)]
    for compiler in required_windows_provider_free_compilers(stage.path()) {
        let standards: &[&str] = if compiler == "cl" {
            &["/std:c17"]
        } else {
            &["/std:c11", "/std:c17"]
        };
        for standard in standards {
            invocations += 1;
            let executable = compile_msvc_provider_free_probe(
                stage.path(),
                MsvcProviderFreeProbe {
                    compiler,
                    consumer_language: "/TC",
                    consumer_standard: standard,
                    runtime_include: &runtime_include,
                    generated_include: &generated_include,
                    generated_source: &generated_source,
                    consumer: &consumer,
                    stem: &format!(
                        "coalesced-create-{compiler}-{}",
                        standard.trim_start_matches("/std:")
                    ),
                },
            );
            let output = Command::new(&executable)
                .output()
                .unwrap_or_else(|error| panic!("failed to run {}: {error}", executable.display()));
            assert!(
                output.status.success(),
                "{compiler}'s generated coalescing probe failed under {standard} with {}:\nstdout:\n{}\nstderr:\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
    }
    assert!(
        invocations > 0,
        "no supported C compiler was available for the generated coalescing probe"
    );
}

#[test]
fn rejects_missing_mutated_and_foreign_projection_evidence() {
    let emitter = CEmitter::new();
    let resources = emitter.code_resources().expect("C resources hash");
    let (projection, authority) = projected(&resources);

    let (missing, _) = projected(&resources[1..]);
    assert_eq!(
        emitter
            .emit(&missing, &authority)
            .expect_err("missing resource evidence must fail")
            .code()
            .as_str(),
        "c_emitter_evidence_mismatch",
    );

    let mut mutated_resources = resources.clone();
    let resource_id = mutated_resources[0].id().as_str().to_owned();
    mutated_resources[0] =
        CodeResourceDigest::from_bytes(resource_id, b"tampered C template").unwrap();
    mutated_resources.sort_by(|left, right| left.id().cmp(right.id()));
    let (mutated, _) = projected(&mutated_resources);
    assert_eq!(
        emitter
            .emit(&mutated, &authority)
            .expect_err("mutated resource evidence must fail")
            .code()
            .as_str(),
        "c_emitter_evidence_mismatch",
    );

    let foreign = support::authority("format: typebridge.schema/v2\nentities:\n  foreign: {}\n");
    assert_eq!(
        emitter
            .emit(&projection, &foreign)
            .expect_err("foreign authority must fail")
            .code()
            .as_str(),
        "schema_codegen_authority_mismatch",
    );

    let mut forged_models = projection.models().clone();
    let (forged_id, original) = forged_models
        .first_key_value()
        .map(|(id, model)| (id.clone(), model.clone()))
        .expect("fixture has projected models");
    let forged_model = ModelProjection::new(
        forged_id.clone(),
        TargetIdentifier::c("acme_forged_model").unwrap(),
        original.declaration().clone(),
        original.create().clone(),
        original.complete_read().clone(),
        original.reference_read().clone(),
        original.query_tokens().clone(),
    )
    .unwrap();
    forged_models.insert(forged_id, forged_model);
    let forged_projection = RuntimeProjection::try_new(
        BindingTarget::C,
        projection.config().clone(),
        projection.semantic_fingerprint().clone(),
        projection.generator_handlers(),
        projection.code_resources(),
        forged_models,
        projection.structs().clone(),
        projection.functions().clone(),
        projection.playing_facts().clone(),
        projection.emission().clone(),
    )
    .expect("self-consistent forged C projection constructs");
    assert_eq!(
        emitter
            .emit(&forged_projection, &authority)
            .expect_err("a self-consistent but noncanonical C projection must fail")
            .code()
            .as_str(),
        "c_emitter_projection_mismatch",
    );
}

#[test]
fn large_embedded_resources_are_byte_exact_portable_chunks() {
    const EMBEDDED_CHUNK_MAX: usize = 32_768;
    const HOSTED_OBJECT_BYTES_MIN: usize = 65_535;

    let documentation = "d".repeat(72 * 1_024);
    let source = format!(
        "format: typebridge.schema/v2\nentities:\n  documented:\n    doc: \"{documentation}\"\n"
    );
    let emitter = CEmitter::new();
    let resources = emitter.code_resources().expect("C resources hash");
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("c-large-doc.yaml").expect("fixture document ID is valid"),
        source.as_str(),
    )])
    .expect("large-documentation C fixture parses");
    let declared = normalize_documents(&documents).expect("large C fixture normalizes");
    let profile = SemanticProfileId::new(support::TEST_PROFILE).expect("test profile is valid");
    let resolved = resolve(&declared, &profile).expect("large C fixture resolves");
    let projection = project(
        &resolved,
        BindingTarget::C,
        &ProjectionConfig::c(CSymbolPrefix::new("large").expect("test prefix is valid")),
        &emitter.generator_handlers(),
        &resources,
    )
    .expect("large C fixture projects");
    let authority = support::authority(&source);
    let expected_authority = encode_schema_authority(&authority);
    let expected_declared =
        encode_declared_schema(authority.declared_schema()).expect("declared schema encodes");
    let expected_projection = to_canonical_json(&projection).expect("projection encodes");
    assert!(
        [
            expected_authority.len(),
            expected_declared.len(),
            expected_projection.len(),
        ]
        .into_iter()
        .any(|length| length > HOSTED_OBJECT_BYTES_MIN),
        "fixture must exceed the minimum hosted-object size",
    );

    let package = emitter
        .emit(&projection, &authority)
        .expect("large chunked C package emits");
    let generated = std::str::from_utf8(package.get("src/models.c").unwrap())
        .expect("generated large C source is UTF-8");
    assert_eq!(
        embedded_array(generated, "large_schema_authority_json"),
        expected_authority
    );
    assert_eq!(
        embedded_array(generated, "large_declared_schema_json"),
        expected_declared
    );
    assert_eq!(
        embedded_array(generated, "large_runtime_projection_json"),
        expected_projection
    );
    let byte_array_lengths = generated
        .split("static const uint8_t ")
        .skip(1)
        .map(|tail| {
            tail.split_once("};\n")
                .expect("generated embedded chunk terminates")
                .0
                .matches("0x")
                .count()
        })
        .collect::<Vec<_>>();
    assert!(!byte_array_lengths.is_empty());
    assert!(
        byte_array_lengths
            .iter()
            .all(|length| (1..=EMBEDDED_CHUNK_MAX).contains(length)),
        "every generated byte-array object is nonempty and at most 32,768 bytes",
    );
    assert!(generated.contains("large_schema_authority_json_chunk_2"));
    assert!(
        generated.contains("generated embedded chunk table exceeds the C hosted-object minimum")
    );
    assert!(generated.contains("generated package descriptor exceeds the C hosted-object minimum"));

    let stage = TempDirectory::new();
    write_package(&package, stage.path());
    let runtime_include = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../c/include")
        .canonicalize()
        .expect("C runtime include directory exists");
    let generated_include = stage.path().join("include");
    let generated_source = stage.path().join("src/models.c");
    let mut invocations = 0;
    for compiler in ["gcc", "clang"] {
        if !command_exists(compiler) {
            continue;
        }
        invocations += 1;
        let output = Command::new(compiler)
            .args([
                "-std=c17",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-Wframe-larger-than=8192",
                "-pedantic-errors",
                "-fsyntax-only",
            ])
            .arg("-I")
            .arg(&runtime_include)
            .arg("-I")
            .arg(&generated_include)
            .arg(&generated_source)
            .output()
            .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
        assert!(
            output.status.success(),
            "{compiler} rejected large chunked generated C:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    #[cfg(windows)]
    {
        invocations += 1;
        let output = msvc_cl(
            stage.path(),
            &[
                "/nologo".to_owned(),
                "/TC".to_owned(),
                "/std:c17".to_owned(),
                "/W4".to_owned(),
                "/WX".to_owned(),
                "/Zs".to_owned(),
                format!("/I{}", runtime_include.display()),
                format!("/I{}", generated_include.display()),
                generated_source.display().to_string(),
            ],
        );
        assert!(
            output.status.success(),
            "MSVC cl rejected large chunked generated C:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    assert!(
        invocations > 0,
        "no supported C compiler was available for large-object evidence"
    );
}

#[test]
fn high_variant_role_union_keeps_constant_alias_stack() {
    const PLAYER_COUNT: usize = 150;

    let mut source = String::from("format: typebridge.schema/v2\nentities:\n");
    for index in 0..PLAYER_COUNT {
        use std::fmt::Write as _;
        writeln!(source, "  player-{index}: {{}}").unwrap();
    }
    source.push_str("relations:\n  roster:\n    relates: [member]\nplays:\n");
    for index in 0..PLAYER_COUNT {
        use std::fmt::Write as _;
        writeln!(source, "  player-{index}:\n    roster: [member]").unwrap();
    }

    let emitter = CEmitter::new();
    let resources = emitter.code_resources().expect("C resources hash");
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("c-high-role-union.yaml").expect("fixture document ID is valid"),
        source.as_str(),
    )])
    .expect("high-variant role fixture parses");
    let declared = normalize_documents(&documents).expect("high-variant role fixture normalizes");
    let profile = SemanticProfileId::new(support::TEST_PROFILE).expect("test profile is valid");
    let resolved = resolve(&declared, &profile).expect("high-variant role fixture resolves");
    let projection = project(
        &resolved,
        BindingTarget::C,
        &ProjectionConfig::c(CSymbolPrefix::new("many").expect("test prefix is valid")),
        &emitter.generator_handlers(),
        &resources,
    )
    .expect("high-variant role fixture projects");
    let package = emitter
        .emit(&projection, &support::authority(&source))
        .expect("inventory-valid high-variant package emits");
    let generated = std::str::from_utf8(package.get("src/models.c").unwrap())
        .expect("generated high-variant source is UTF-8");
    let kind = generated
        .split_once("many_roster_member_player_kind(\n")
        .expect("role-union kind accessor is emitted")
        .1
        .split_once("\n}\n\n")
        .expect("role-union kind accessor terminates")
        .0;
    assert!(kind.contains("type_bridge_generated_opaque_input_v1_t alias_inputs[1];"));
    assert_eq!(
        kind.matches("alias_input_count = 0u;").count(),
        PLAYER_COUNT + 2,
        "player, role, and every accepted model token are fenced sequentially",
    );
    assert!(!kind.contains("alias_inputs[152]"));

    let stage = TempDirectory::new();
    write_package(&package, stage.path());
    let runtime_include = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../c/include")
        .canonicalize()
        .expect("C runtime include directory exists");
    let generated_include = stage.path().join("include");
    let generated_source = stage.path().join("src/models.c");
    let mut invocations = 0;
    for compiler in ["gcc", "clang"] {
        if !command_exists(compiler) {
            continue;
        }
        invocations += 1;
        let output = Command::new(compiler)
            .args([
                "-std=c17",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-Wframe-larger-than=3072",
                "-pedantic-errors",
                "-fsyntax-only",
            ])
            .arg("-I")
            .arg(&runtime_include)
            .arg("-I")
            .arg(&generated_include)
            .arg(&generated_source)
            .output()
            .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
        assert!(
            output.status.success(),
            "{compiler} rejected constant-frame high-variant C:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    #[cfg(windows)]
    {
        invocations += 1;
        let output = msvc_cl(
            stage.path(),
            &[
                "/nologo".to_owned(),
                "/TC".to_owned(),
                "/std:c17".to_owned(),
                "/analyze".to_owned(),
                "/analyze:only".to_owned(),
                "/analyze:stacksize".to_owned(),
                "3072".to_owned(),
                format!("/I{}", runtime_include.display()),
                format!("/I{}", generated_include.display()),
                generated_source.display().to_string(),
            ],
        );
        let diagnostics = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.status.success() && !diagnostics.contains("C6262"));
    }
    assert!(
        invocations > 0,
        "no supported C compiler was available for high-variant frame evidence"
    );
}

#[test]
fn dense_relation_reachability_surface_is_linear_in_role_count() {
    const ROLE_COUNT: usize = 150;

    let mut source = String::from(
        "format: typebridge.schema/v2\nentities:\n  player: {}\nrelations:\n  network:\n    relates:\n",
    );
    for index in 0..ROLE_COUNT {
        use std::fmt::Write as _;
        writeln!(source, "      role-{index}: {{}}").unwrap();
    }
    source.push_str("plays:\n  player:\n    network:\n");
    for index in 0..ROLE_COUNT {
        use std::fmt::Write as _;
        writeln!(source, "      - role-{index}").unwrap();
    }

    let emitter = CEmitter::new();
    let resources = emitter.code_resources().expect("C resources hash");
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("c-dense-query-roles.yaml").expect("fixture document ID is valid"),
        source.as_str(),
    )])
    .expect("dense query-role fixture parses");
    let declared = normalize_documents(&documents).expect("dense query-role fixture normalizes");
    let profile = SemanticProfileId::new(support::TEST_PROFILE).expect("test profile is valid");
    let resolved = resolve(&declared, &profile).expect("dense query-role fixture resolves");
    let projection = project(
        &resolved,
        BindingTarget::C,
        &ProjectionConfig::c(CSymbolPrefix::new("dense").expect("test prefix is valid")),
        &emitter.generator_handlers(),
        &resources,
    )
    .expect("dense query-role fixture projects");
    let package = emitter
        .emit(&projection, &support::authority(&source))
        .expect("dense query-role package emits");
    let header = std::str::from_utf8(package.get("include/dense/models.h").unwrap())
        .expect("generated dense query-role header is UTF-8");
    let generated = std::str::from_utf8(package.get("src/models.c").unwrap())
        .expect("generated dense query-role source is UTF-8");

    assert_eq!(
        header
            .matches("type_bridge_query_session_reachable(")
            .count(),
        1,
        "one relation-scoped reachable wrapper must replace the role-pair matrix",
    );
    assert_eq!(
        header
            .matches("TYPE_BRIDGE_CALL dense_network_query_reachable_endpoint_v1_t_from_")
            .count(),
        ROLE_COUNT,
        "each role must contribute exactly one endpoint constructor",
    );
    assert_eq!(
        header
            .matches("TYPE_BRIDGE_CALL dense_network_query_reachable(")
            .count(),
        1,
    );
    assert!(
        header.len() < ROLE_COUNT * 50_000,
        "dense reachability header grew beyond the frozen linear-size envelope: {} bytes",
        header.len(),
    );
    assert!(
        generated.len() < ROLE_COUNT * 35_000,
        "dense-role external source grew beyond the frozen linear-size envelope: {} bytes",
        generated.len(),
    );

    let stage = TempDirectory::new();
    write_package(&package, stage.path());
    let runtime_include = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../c/include")
        .canonicalize()
        .expect("C runtime include directory exists");
    let generated_include = stage.path().join("include");
    let generated_source = stage.path().join("src/models.c");
    let mut invocations = 0;
    for compiler in ["gcc", "clang"] {
        if !command_exists(compiler) {
            continue;
        }
        invocations += 1;
        let output = Command::new(compiler)
            .args([
                "-std=c17",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-Wframe-larger-than=8192",
                "-pedantic-errors",
                "-fsyntax-only",
            ])
            .arg("-I")
            .arg(&runtime_include)
            .arg("-I")
            .arg(&generated_include)
            .arg(&generated_source)
            .output()
            .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
        assert!(
            output.status.success(),
            "{compiler} rejected the dense linear reachability surface:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    assert!(
        invocations > 0,
        "no supported C compiler was available for dense reachability evidence"
    );
}

#[test]
fn rejects_runtime_symbol_namespace_before_c_emission() {
    for prefix in ["type_bridge", "type_bridge_projected_value"] {
        let error = CSymbolPrefix::new(prefix)
            .expect_err("the runtime ABI namespace must not reach C projection or emission");
        assert_eq!(error.code().as_str(), "invalid_c_symbol_prefix");
    }
    assert_eq!(
        CSymbolPrefix::new("tb_projected_value")
            .expect("application-owned tb_* prefixes remain available")
            .as_str(),
        "tb_projected_value",
    );
}

#[test]
fn generated_exported_names_are_unique_and_bounded_after_derived_suffixes() {
    let long_entity = "a".repeat(240);
    let source = format!(
        "format: typebridge.schema/v2\nattributes:\n  identity: {{ value: string }}\nentities:\n  {long_entity}:\n    owns:\n      identity: {{ key: true }}\n"
    );
    let emitter = CEmitter::new();
    let resources = emitter.code_resources().expect("C resources hash");
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("c-long-symbol.yaml").expect("fixture document ID is valid"),
        source.as_str(),
    )])
    .expect("long-name C fixture parses");
    let declared = normalize_documents(&documents).expect("long-name C fixture normalizes");
    let profile = SemanticProfileId::new(support::TEST_PROFILE).expect("test profile is valid");
    let resolved = resolve(&declared, &profile).expect("long-name C fixture resolves");
    let projection = project(
        &resolved,
        BindingTarget::C,
        &ProjectionConfig::c(CSymbolPrefix::new("acme").expect("test prefix is valid")),
        &emitter.generator_handlers(),
        &resources,
    )
    .expect("long-name C fixture projects");
    let authority = support::authority(&source);
    let first = emitter
        .emit(&projection, &authority)
        .expect("long-name C package emits");
    let second = emitter
        .emit(&projection, &authority)
        .expect("long-name C package re-emits");
    assert_eq!(first, second);

    let header = std::str::from_utf8(first.get("include/acme/models.h").unwrap()).unwrap();
    let functions = emitted_function_names(header);
    assert!(
        functions.iter().any(|name| name.len() == 255),
        "fixture must exercise the bounded generated-symbol path",
    );
    assert!(
        functions.iter().all(|name| name.len() <= 255),
        "generated C exports exceeded the v2 symbol ceiling: {:?}",
        functions
            .iter()
            .filter(|name| name.len() > 255)
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        functions.iter().copied().collect::<BTreeSet<_>>().len(),
        functions.len(),
        "derived generated C exports collided",
    );
}

#[cfg(unix)]
#[test]
fn generated_zero_player_roles_preserve_scalar_and_sequence_shape() {
    const EMPTY_ROLE_SOURCE: &str = r#"format: typebridge.schema/v2
relations:
  observer:
    relates:
      ghost: { card: { min: 0, max: 1 } }
      spectres: { card: { min: 0, max: 3 } }
"#;
    let emitter = CEmitter::new();
    let resources = emitter.code_resources().expect("C resources hash");
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("c-empty-role.yaml").expect("fixture document ID is valid"),
        EMPTY_ROLE_SOURCE,
    )])
    .expect("empty-role C fixture parses");
    let declared = normalize_documents(&documents).expect("empty-role C fixture normalizes");
    let profile = SemanticProfileId::new(support::TEST_PROFILE).expect("test profile is valid");
    let resolved = resolve(&declared, &profile).expect("empty-role C fixture resolves");
    let projection = project(
        &resolved,
        BindingTarget::C,
        &ProjectionConfig::c(CSymbolPrefix::new("acme").expect("test prefix is valid")),
        &emitter.generator_handlers(),
        &resources,
    )
    .expect("empty-role C fixture projects");
    let package = emitter
        .emit(&projection, &support::authority(EMPTY_ROLE_SOURCE))
        .expect("empty-role C package emits");
    let header = std::str::from_utf8(package.get("include/acme/models.h").unwrap()).unwrap();

    for declaration in [
        "typedef struct acme_observer_ghost_player acme_observer_ghost_player;",
        "typedef uint32_t acme_observer_ghost_player_kind_t;",
        "#define acme_observer_ghost_player_kind_unknown UINT32_C(0)",
        "type_bridge_status_t TYPE_BRIDGE_CALL acme_observer_ghost_player_kind(",
        "type_bridge_status_t TYPE_BRIDGE_CALL acme_observer_ghost_player_close(",
        "type_bridge_status_t TYPE_BRIDGE_CALL acme_observer_ghost(",
        "typedef struct acme_observer_spectres_player acme_observer_spectres_player;",
        "typedef uint32_t acme_observer_spectres_player_kind_t;",
        "#define acme_observer_spectres_player_kind_unknown UINT32_C(0)",
        "type_bridge_status_t TYPE_BRIDGE_CALL acme_observer_spectres_count(",
        "type_bridge_status_t TYPE_BRIDGE_CALL acme_observer_spectres_at(",
    ] {
        assert!(
            header.contains(declaration),
            "empty role omitted {declaration}",
        );
    }
    assert!(!header.contains("acme_observer_ghost_player_from_"));
    assert!(!header.contains("acme_observer_ghost_player_as_"));
    assert!(!header.contains("acme_observer_spectres_player_from_"));
    assert!(!header.contains("acme_observer_spectres_player_as_"));
    assert!(!header.contains("acme_observer_ghost_count("));
    assert!(!header.contains("acme_observer_ghost_at("));
    let create_start = header
        .split_once("type_bridge_status_t TYPE_BRIDGE_CALL acme_observer_create_open(")
        .expect("constructible empty-role relation has a create wrapper")
        .1
        .split_once(");")
        .expect("create declaration terminates")
        .0;
    assert!(!create_start.contains("role_ghost"));
    assert!(!create_start.contains("role_spectres"));

    let stage = TempDirectory::new();
    write_package(&package, stage.path());
    let consumer = stage.path().join("empty-role-consumer.c");
    fs::write(
        &consumer,
        r#"#include <acme/models.h>

_Static_assert(sizeof(acme_observer_ghost_player_kind_t) == sizeof(uint32_t),
               "empty role kind ABI is not fixed-width");

int main(void) {
  const type_bridge_schema_package_t *package = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  acme_observer_create_args_v1_t args = {
      sizeof(args), ACME_CREATE_ARGS_VERSION, {0u, 0u, 0u, 0u}};
  acme_observer_create *create = 0;
  acme_observer *observer = 0;
  acme_observer_ghost_player *ghost = 0;
  acme_observer_spectres_player *spectre = 0;
  acme_observer_ghost_player_kind_t ghost_kind =
      acme_observer_ghost_player_kind_unknown;
  acme_observer_spectres_player_kind_t spectre_kind =
      acme_observer_spectres_player_kind_unknown;
  size_t count = 0u;
  (void)acme_observer_create_open(package, &args, &create, &diagnostics);
  (void)acme_observer_ghost(observer, &ghost, &diagnostics);
  (void)acme_observer_spectres_count(observer, &count, &diagnostics);
  (void)acme_observer_spectres_at(observer, 0u, &spectre, &diagnostics);
  (void)acme_observer_ghost_player_kind(ghost, &ghost_kind, &diagnostics);
  (void)acme_observer_spectres_player_kind(
      spectre, &spectre_kind, &diagnostics);
  (void)acme_observer_ghost_player_close(&ghost);
  (void)acme_observer_spectres_player_close(&spectre);
  (void)acme_observer_create_close(&create);
  (void)acme_observer_close(&observer);
  return count != 0u ||
      ghost_kind != acme_observer_ghost_player_kind_unknown ||
      spectre_kind != acme_observer_spectres_player_kind_unknown;
}
"#,
    )
    .expect("empty-role consumer is written");
    let runtime_include = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../c/include")
        .canonicalize()
        .expect("C runtime include directory exists");
    let generated_include = stage.path().join("include");
    let generated_source = stage.path().join("src/models.c");
    let mut invocations = 0;
    for compiler in ["gcc", "clang"] {
        if !command_exists(compiler) {
            continue;
        }
        for standard in ["c11", "c17"] {
            invocations += 1;
            let output = Command::new(compiler)
                .arg(format!("-std={standard}"))
                .args([
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-Wframe-larger-than=8192",
                    "-pedantic-errors",
                    "-fshort-enums",
                    "-fsyntax-only",
                ])
                .arg("-I")
                .arg(&runtime_include)
                .arg("-I")
                .arg(&generated_include)
                .arg(&generated_source)
                .arg(&consumer)
                .output()
                .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
            assert!(
                output.status.success(),
                "{compiler} rejected empty-role generated C under {standard}:\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
    }
    assert!(invocations > 0, "GCC or Clang is required");
}

static TEMP_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let sequence = TEMP_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "typebridge-c-emitter-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("unique C emitter test directory is created");
        Self(directory)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("C emitter test directory is removed");
    }
}

fn write_package(package: &GeneratedPackage, root: &Path) {
    for (relative, contents) in package.files() {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("generated path has a parent"))
            .expect("generated parent directory is created");
        fs::write(path, contents).expect("generated C package file is written");
    }
}

fn command_exists(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[cfg(windows)]
fn msvc_tool(stage: &Path, tool: &str, arguments: &[String]) -> std::process::Output {
    let installer = std::env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .expect("the Windows evidence lane exposes ProgramFiles(x86)")
        .join("Microsoft Visual Studio/Installer/vswhere.exe");
    let discovery = Command::new(&installer)
        .args([
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ])
        .output()
        .expect("Visual Studio discovery launches");
    assert!(
        discovery.status.success(),
        "vswhere could not locate MSVC:\n{}",
        String::from_utf8_lossy(&discovery.stderr),
    );
    let installation =
        String::from_utf8(discovery.stdout).expect("Visual Studio installation path is UTF-8");
    let developer_command = Path::new(installation.trim()).join("Common7/Tools/VsDevCmd.bat");
    assert!(
        developer_command.is_file(),
        "Visual Studio developer command is present"
    );
    let launcher = stage.join(format!("run-{tool}.cmd"));
    fs::write(
        &launcher,
        format!(
            "@call \"{}\" -arch=x64 -host_arch=x64 >nul\r\n@{tool} %*\r\n",
            developer_command.display(),
        ),
    )
    .expect("Visual Studio developer-command launcher is written");
    Command::new("cmd")
        .args(["/D", "/C"])
        .arg(&launcher)
        .args(arguments)
        .output()
        .unwrap_or_else(|error| panic!("{tool} launches through VsDevCmd: {error}"))
}

#[cfg(windows)]
fn msvc_cl(stage: &Path, arguments: &[String]) -> std::process::Output {
    msvc_tool(stage, "cl", arguments)
}

#[cfg(windows)]
fn msvc_tool_exists(stage: &Path, tool: &str) -> bool {
    msvc_tool(stage, "where", &[tool.to_owned()])
        .status
        .success()
}

#[cfg(windows)]
fn required_windows_provider_free_compilers(stage: &Path) -> [&'static str; 2] {
    for compiler in ["cl", "clang-cl"] {
        assert!(
            msvc_tool_exists(stage, compiler),
            "the Windows C evidence lane requires {compiler}"
        );
    }
    ["cl", "clang-cl"]
}

#[cfg(windows)]
struct MsvcProviderFreeProbe<'a> {
    compiler: &'a str,
    consumer_language: &'a str,
    consumer_standard: &'a str,
    runtime_include: &'a Path,
    generated_include: &'a Path,
    generated_source: &'a Path,
    consumer: &'a Path,
    stem: &'a str,
}

#[cfg(windows)]
fn compile_msvc_provider_free_probe(stage: &Path, probe: MsvcProviderFreeProbe<'_>) -> PathBuf {
    let generated_object = stage.join(format!("{}-generated.obj", probe.stem));
    let consumer_object = stage.join(format!("{}-consumer.obj", probe.stem));
    let executable = stage.join(format!("{}.exe", probe.stem));
    let generated_standard = if probe.consumer_language == "/TP" {
        "/std:c17"
    } else {
        probe.consumer_standard
    };

    let output = msvc_tool(
        stage,
        probe.compiler,
        &[
            "/nologo".to_owned(),
            "/TC".to_owned(),
            generated_standard.to_owned(),
            "/W4".to_owned(),
            "/WX".to_owned(),
            "/Gy".to_owned(),
            "/Gw".to_owned(),
            "/c".to_owned(),
            format!("/I{}", probe.runtime_include.display()),
            format!("/I{}", probe.generated_include.display()),
            probe.generated_source.display().to_string(),
            format!("/Fo{}", generated_object.display()),
        ],
    );
    assert!(
        output.status.success(),
        "{} could not compile the provider-free generated C source under {}:\nstdout:\n{}\nstderr:\n{}",
        probe.compiler,
        generated_standard,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let output = msvc_tool(
        stage,
        probe.compiler,
        &[
            "/nologo".to_owned(),
            probe.consumer_language.to_owned(),
            probe.consumer_standard.to_owned(),
            "/W4".to_owned(),
            "/WX".to_owned(),
            "/Gy".to_owned(),
            "/Gw".to_owned(),
            "/c".to_owned(),
            format!("/I{}", probe.runtime_include.display()),
            format!("/I{}", probe.generated_include.display()),
            probe.consumer.display().to_string(),
            format!("/Fo{}", consumer_object.display()),
        ],
    );
    assert!(
        output.status.success(),
        "{} could not compile the provider-free consumer under {}:\nstdout:\n{}\nstderr:\n{}",
        probe.compiler,
        probe.consumer_standard,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let output = msvc_tool(
        stage,
        probe.compiler,
        &[
            "/nologo".to_owned(),
            generated_object.display().to_string(),
            consumer_object.display().to_string(),
            format!("/Fe{}", executable.display()),
            "/link".to_owned(),
            "/OPT:REF".to_owned(),
            "/INCREMENTAL:NO".to_owned(),
        ],
    );
    assert!(
        output.status.success(),
        "{} could not link the provider-free consumer under {}:\nstdout:\n{}\nstderr:\n{}",
        probe.compiler,
        probe.consumer_standard,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    executable
}

fn preprocessor_macro_names(stdout: &[u8]) -> BTreeSet<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("#define "))
        .filter_map(|definition| definition.split_ascii_whitespace().next())
        .map(|name| name.split_once('(').map_or(name, |(name, _)| name))
        .map(str::to_owned)
        .collect()
}

fn runtime_header_macro_names() -> BTreeSet<String> {
    include_str!("../../c/include/typebridge/type_bridge.h")
        .lines()
        .filter_map(|line| line.strip_prefix("#define "))
        .filter_map(|definition| definition.split_ascii_whitespace().next())
        .map(|name| name.split_once('(').map_or(name, |(name, _)| name))
        .map(str::to_owned)
        .collect()
}

#[test]
fn supported_c_preprocessors_fit_the_frozen_implementation_macro_reserve() {
    const IMPLEMENTATION_MACRO_RESERVE: usize = 1_024;

    let stage = TempDirectory::new();
    let probe = stage.path().join("standard-header-macros.c");
    fs::write(&probe, "#include <stddef.h>\n#include <stdint.h>\n")
        .expect("standard-header macro probe is written");
    let runtime_macros = runtime_header_macro_names();
    assert_eq!(
        runtime_macros.len(),
        205,
        "runtime-header macro reserve drifted"
    );
    let mut invocations = 0;

    for compiler in ["gcc", "clang", "x86_64-w64-mingw32-gcc"] {
        if !command_exists(compiler) {
            continue;
        }
        invocations += 1;
        let output = Command::new(compiler)
            .args(["-std=c11", "-dM", "-E"])
            .arg(&probe)
            .output()
            .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
        assert!(
            output.status.success(),
            "{compiler} could not inventory predefined and required-header macros:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let macros = preprocessor_macro_names(&output.stdout);
        assert!(
            macros.is_disjoint(&runtime_macros),
            "{compiler} ambient macros overlap repository-owned runtime macros",
        );
        assert!(
            macros.len() <= IMPLEMENTATION_MACRO_RESERVE,
            "{compiler} exposes {} predefined/required-header macros, above the frozen reserve of {IMPLEMENTATION_MACRO_RESERVE}",
            macros.len(),
        );
    }

    if command_exists("clang-cl") {
        invocations += 1;
        let output = Command::new("clang-cl")
            .args(["/nologo", "/std:c11", "/E", "/d1PP"])
            .arg(&probe)
            .output()
            .expect("clang-cl macro probe launches");
        assert!(
            output.status.success(),
            "clang-cl could not inventory predefined and required-header macros:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let macros = preprocessor_macro_names(&output.stdout);
        assert!(
            macros.is_disjoint(&runtime_macros),
            "clang-cl ambient macros overlap repository-owned runtime macros",
        );
        assert!(
            macros.len() <= IMPLEMENTATION_MACRO_RESERVE,
            "clang-cl exposes {} predefined/required-header macros, above the frozen reserve of {IMPLEMENTATION_MACRO_RESERVE}",
            macros.len(),
        );
    }

    #[cfg(windows)]
    {
        let output = msvc_cl(
            stage.path(),
            &[
                "/nologo".to_owned(),
                "/std:c17".to_owned(),
                "/EP".to_owned(),
                "/d1PP".to_owned(),
                probe.display().to_string(),
            ],
        );
        invocations += 1;
        assert!(
            output.status.success(),
            "MSVC cl could not inventory predefined and required-header macros:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let macros = preprocessor_macro_names(&output.stdout);
        assert!(
            !macros.is_empty(),
            "MSVC cl /d1PP did not retain macro definitions",
        );
        assert!(
            macros.is_disjoint(&runtime_macros),
            "MSVC ambient macros overlap repository-owned runtime macros",
        );
        assert!(
            macros.len() <= IMPLEMENTATION_MACRO_RESERVE,
            "MSVC cl exposes {} predefined/required-header macros, above the frozen reserve of {IMPLEMENTATION_MACRO_RESERVE}",
            macros.len(),
        );
    }

    assert!(
        invocations > 0,
        "no supported C preprocessor was available for macro-reserve evidence"
    );
}

#[test]
fn generated_header_and_source_are_strict_c11_and_c17_for_installed_compilers() {
    let emitter = CEmitter::new();
    let resources = emitter.code_resources().expect("C resources hash");
    let (projection, authority) = projected(&resources);
    let package = emitter
        .emit(&projection, &authority)
        .expect("C package emits");
    let stage = TempDirectory::new();
    write_package(&package, stage.path());
    let consumer = stage.path().join("consumer.c");
    fs::write(
        &consumer,
        r#"#include <acme/models.h>

_Static_assert(TYPE_BRIDGE_C_ABI_MAJOR == 1u, "C ABI major changed");
_Static_assert(TYPE_BRIDGE_C_ABI_MINOR == 5u, "C ABI minor changed");
_Static_assert(sizeof(acme_membership_member_player_kind_t) == sizeof(uint32_t),
               "role-player kind ABI is not fixed-width");

int main(void) {
  const type_bridge_schema_package_t *package = 0;
  type_bridge_schema_package_t *opened_package = 0;
  type_bridge_diagnostics_t *package_diagnostics = 0;
  const type_bridge_database_t *database = 0;
  const type_bridge_read_transaction_t *read_tx = 0;
  const type_bridge_write_transaction_t *write_tx = 0;
  const type_bridge_cancellation_t *cancellation = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  acme_personzhrecord *person = 0;
  acme_personzhrecord_create *create = 0;
  acme_personzhrecord_ref *reference = 0;
  acme_dualzhkeyzhrecord_ref *dual_reference = 0;
  acme_event_ref *event_reference = 0;
  acme_membership *membership = 0;
  acme_membership_create *membership_create = 0;
  acme_membership_ref *membership_reference = 0;
  acme_membership_member_player *member_player = 0;
  acme_membership_member_player_kind_t member_kind =
      acme_membership_member_player_kind_unknown;
  acme_container_item_player *item_player = 0;
  acme_aliases *alias = 0;
  const acme_aliases *aliases[1] = {alias};
  acme_personzhrecord_create_field_aliases_chunks_v1_t aliases_chunk = {0};
  acme_personzhrecord_create_args_v1_t create_args = {0};
  acme_membership_create_args_v1_t membership_args = {0};
  acme_countzhvalue *count_value = 0;
  acme_datezhvalue *date_value = 0;
  acme_datetimezhvalue *datetime_value = 0;
  acme_decimalzhvalue *decimal_value = 0;
  acme_displayzhname *display_name = 0;
  acme_durationzhvalue *duration_value = 0;
  acme_enabledzhvalue *enabled_value = 0;
  acme_ratiozhvalue *ratio_value = 0;
  acme_zzonedzhvalue *zoned_value = 0;
  type_bridge_byte_view_t text = {0};
  int64_t scalar = 0;
  size_t field_count = 0;
  uint64_t entity_count = 0;

  (void)acme_schema_package_open(&opened_package, &package_diagnostics);
  aliases_chunk.struct_size = sizeof(aliases_chunk);
  aliases_chunk.version = ACME_CREATE_ARGS_VERSION;
  aliases_chunk.values = aliases;
  aliases_chunk.count = 1u;
  create_args.struct_size = sizeof(create_args);
  create_args.version = ACME_CREATE_ARGS_VERSION;
  create_args.field_aliases_chunks = &aliases_chunk;
  create_args.field_countzhvalue = count_value;
  create_args.field_datezhvalue = date_value;
  create_args.field_datetimezhvalue = datetime_value;
  create_args.field_decimalzhvalue = decimal_value;
  create_args.field_displayzhname = display_name;
  create_args.field_durationzhvalue = duration_value;
  create_args.field_enabledzhvalue = enabled_value;
  create_args.field_ratiozhvalue = ratio_value;
  create_args.field_zzonedzhvalue = zoned_value;

  (void)acme_personzhrecord_create_open(
      package, &create_args, &create, &diagnostics);
  (void)acme_personzhrecord_ref_from_iid(
      package, text, &reference, &diagnostics);
  (void)acme_personzhrecord_ref_from_key(
      package, display_name, &reference, &diagnostics);
  (void)acme_personzhrecord_ref_iid(reference, &text, &diagnostics);
  (void)acme_personzhrecord_ref_displayzhname_key(
      reference, &display_name, &diagnostics);
  (void)acme_personzhrecord_iid(person, &text, &diagnostics);
  (void)acme_personzhrecord_reference(person, &reference, &diagnostics);
  (void)acme_personzhrecord_countzhvalue(person, &count_value, &diagnostics);
  (void)acme_personzhrecord_ratiozhvalue(person, &ratio_value, &diagnostics);
  (void)acme_personzhrecord_aliases_count(person, &field_count, &diagnostics);
  (void)acme_personzhrecord_aliases_at(person, 0u, &alias, &diagnostics);
  (void)acme_countzhvalue_value(count_value, &scalar, &diagnostics);

  (void)acme_membership_member_player_from_personzhrecord(
      reference, &member_player, &diagnostics);
  (void)acme_membership_member_player_kind(
      member_player, &member_kind, &diagnostics);
  (void)acme_membership_member_player_as_personzhrecord(
      member_player, &reference, &diagnostics);
  (void)acme_membership_member_player_from_dualzhkeyzhrecord(
      dual_reference, &member_player, &diagnostics);
  (void)acme_membership_member_player_as_dualzhkeyzhrecord(
      member_player, &dual_reference, &diagnostics);
  (void)acme_container_item_player_from_event(
      event_reference, &item_player, &diagnostics);
  membership_args.struct_size = sizeof(membership_args);
  membership_args.version = ACME_CREATE_ARGS_VERSION;
  membership_args.role_member = member_player;
  (void)acme_membership_create_open(
      package, &membership_args, &membership_create, &diagnostics);
  (void)acme_membership_iid(membership, &text, &diagnostics);
  (void)acme_membership_reference(
      membership, &membership_reference, &diagnostics);
  (void)acme_membership_member(membership, &member_player, &diagnostics);

  (void)acme_personzhrecord_database_insert(
      database, create, cancellation, &person, &diagnostics);
  (void)acme_personzhrecord_database_put(
      database, create, cancellation, &person, &diagnostics);
  (void)acme_personzhrecord_database_get_by_iid(
      database, text, cancellation, &person, &diagnostics);
  (void)acme_personzhrecord_database_update(
      database, text, create, cancellation, &person, &diagnostics);
  (void)acme_personzhrecord_database_delete_by_iid(
      database, text, cancellation, &diagnostics);
  (void)acme_personzhrecord_database_count(
      database, cancellation, &entity_count, &diagnostics);
  (void)acme_personzhrecord_read_transaction_get_by_iid(
      read_tx, text, cancellation, &person, &diagnostics);
  (void)acme_personzhrecord_read_transaction_count(
      read_tx, cancellation, &entity_count, &diagnostics);
  (void)acme_personzhrecord_write_transaction_insert(
      write_tx, create, cancellation, &person, &diagnostics);
  (void)acme_personzhrecord_write_transaction_put(
      write_tx, create, cancellation, &person, &diagnostics);
  (void)acme_personzhrecord_write_transaction_get_by_iid(
      write_tx, text, cancellation, &person, &diagnostics);
  (void)acme_personzhrecord_write_transaction_update(
      write_tx, text, create, cancellation, &person, &diagnostics);
  (void)acme_personzhrecord_write_transaction_delete_by_iid(
      write_tx, text, cancellation, &diagnostics);
  (void)acme_personzhrecord_write_transaction_count(
      write_tx, cancellation, &entity_count, &diagnostics);
  (void)acme_membership_database_insert(
      database, membership_create, cancellation, &membership, &diagnostics);
  (void)acme_membership_database_put(
      database, membership_create, cancellation, &membership, &diagnostics);
  (void)acme_membership_database_get_by_iid(
      database, text, cancellation, &membership, &diagnostics);
  (void)acme_membership_database_update(
      database, text, membership_create, cancellation, &membership,
      &diagnostics);
  (void)acme_membership_database_delete_by_iid(
      database, text, cancellation, &diagnostics);
  (void)acme_membership_database_count(
      database, cancellation, &entity_count, &diagnostics);
  (void)acme_membership_read_transaction_get_by_iid(
      read_tx, text, cancellation, &membership, &diagnostics);
  (void)acme_membership_read_transaction_count(
      read_tx, cancellation, &entity_count, &diagnostics);
  (void)acme_membership_write_transaction_insert(
      write_tx, membership_create, cancellation, &membership, &diagnostics);
  (void)acme_membership_write_transaction_put(
      write_tx, membership_create, cancellation, &membership, &diagnostics);
  (void)acme_membership_write_transaction_get_by_iid(
      write_tx, text, cancellation, &membership, &diagnostics);
  (void)acme_membership_write_transaction_update(
      write_tx, text, membership_create, cancellation, &membership,
      &diagnostics);
  (void)acme_membership_write_transaction_delete_by_iid(
      write_tx, text, cancellation, &diagnostics);
  (void)acme_membership_write_transaction_count(
      write_tx, cancellation, &entity_count, &diagnostics);
  (void)acme_container_item_player_close(&item_player);
  (void)acme_membership_member_player_close(&member_player);
  (void)acme_membership_ref_close(&membership_reference);
  (void)acme_membership_create_close(&membership_create);
  (void)acme_membership_close(&membership);
  (void)acme_personzhrecord_ref_close(&reference);
  (void)acme_personzhrecord_create_close(&create);
  (void)acme_personzhrecord_close(&person);
  (void)type_bridge_diagnostics_close(&package_diagnostics);
  (void)type_bridge_schema_package_close(&opened_package);

  return person != 0 || display_name != 0 || text.length != 0 ||
      TYPE_BRIDGE_C_ABI_MAJOR != 1u;
}
"#,
    )
    .expect("C consumer is written");

    let runtime_include = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../c/include")
        .canonicalize()
        .expect("C runtime include directory exists");
    let generated_include = stage.path().join("include");
    let generated_source = stage.path().join("src/models.c");
    let mut invocations = 0;
    for compiler in ["gcc", "clang"] {
        if !command_exists(compiler) {
            continue;
        }
        for standard in ["c11", "c17"] {
            for short_enums in [false, true] {
                invocations += 1;
                let mut command = Command::new(compiler);
                command.arg(format!("-std={standard}")).args([
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-Wframe-larger-than=8192",
                    "-pedantic-errors",
                ]);
                if short_enums {
                    command.arg("-fshort-enums");
                }
                let output = command
                    .arg("-fsyntax-only")
                    .arg("-I")
                    .arg(&runtime_include)
                    .arg("-I")
                    .arg(&generated_include)
                    .arg(&generated_source)
                    .arg(&consumer)
                    .output()
                    .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
                assert!(
                    output.status.success(),
                    "{compiler} rejected generated C under {standard} (short-enums={short_enums}):\nstdout:\n{}\nstderr:\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                );
            }
        }
    }
    #[cfg(windows)]
    {
        invocations += 1;
        let output = msvc_cl(
            stage.path(),
            &[
                "/nologo".to_owned(),
                "/TC".to_owned(),
                "/std:c17".to_owned(),
                "/W4".to_owned(),
                "/WX".to_owned(),
                "/Zs".to_owned(),
                format!("/I{}", runtime_include.display()),
                format!("/I{}", generated_include.display()),
                generated_source.display().to_string(),
                consumer.display().to_string(),
            ],
        );
        assert!(
            output.status.success(),
            "MSVC cl rejected the all-wrapper generated C17 consumer:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    assert!(
        invocations > 0,
        "no supported strict C compiler was available"
    );
}

#[cfg(windows)]
#[test]
fn generated_header_source_and_frame_are_strict_msvc_c17() {
    let emitter = CEmitter::new();
    let resources = emitter.code_resources().expect("C resources hash");
    let (projection, authority) = projected(&resources);
    let package = emitter
        .emit(&projection, &authority)
        .expect("C package emits");
    let stage = TempDirectory::new();
    write_package(&package, stage.path());
    let consumer = stage.path().join("msvc-consumer.c");
    fs::write(
        &consumer,
        "#include <acme/models.h>\nint main(void) { return 0; }\n",
    )
    .expect("MSVC consumer is written");
    let runtime_include = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../c/include")
        .canonicalize()
        .expect("C runtime include directory exists");
    let generated_include = stage.path().join("include");
    let generated_source = stage.path().join("src/models.c");

    let strict = msvc_cl(
        stage.path(),
        &[
            "/nologo".to_owned(),
            "/TC".to_owned(),
            "/std:c17".to_owned(),
            "/W4".to_owned(),
            "/WX".to_owned(),
            "/Zs".to_owned(),
            format!("/I{}", runtime_include.display()),
            format!("/I{}", generated_include.display()),
            generated_source.display().to_string(),
            consumer.display().to_string(),
        ],
    );
    assert!(
        strict.status.success(),
        "MSVC cl rejected generated C17:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&strict.stdout),
        String::from_utf8_lossy(&strict.stderr),
    );

    let frame = msvc_cl(
        stage.path(),
        &[
            "/nologo".to_owned(),
            "/TC".to_owned(),
            "/std:c17".to_owned(),
            "/analyze".to_owned(),
            "/analyze:only".to_owned(),
            "/analyze:stacksize".to_owned(),
            "8192".to_owned(),
            format!("/I{}", runtime_include.display()),
            format!("/I{}", generated_include.display()),
            generated_source.display().to_string(),
        ],
    );
    assert!(
        frame.status.success(),
        "MSVC cl could not analyze generated frames:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&frame.stdout),
        String::from_utf8_lossy(&frame.stderr),
    );
    let analysis = format!(
        "{}\n{}",
        String::from_utf8_lossy(&frame.stdout),
        String::from_utf8_lossy(&frame.stderr)
    );
    assert!(
        !analysis.contains("C6262"),
        "MSVC reported a generated frame above the frozen 8192-byte ceiling:\n{analysis}",
    );
}

#[cfg(unix)]
fn assert_generated_cmake_runtime_floor(package: &GeneratedPackage, cases: &[(&str, bool)]) {
    let stage = TempDirectory::new();
    let generated = stage.path().join("generated");
    fs::create_dir(&generated).expect("generated package root is created");
    write_package(package, &generated);

    for (version, should_succeed) in cases {
        let runtime = stage.path().join(format!("runtime-{version}"));
        fs::create_dir(&runtime).expect("fake runtime package directory is created");
        fs::write(
            runtime.join("TypeBridgeConfig.cmake"),
            "add_library(TypeBridge::C INTERFACE IMPORTED)\n",
        )
        .expect("fake runtime config is written");
        fs::write(
            runtime.join("TypeBridgeConfigVersion.cmake"),
            format!(
                "set(PACKAGE_VERSION \"{version}\")\n\
                 if(PACKAGE_FIND_VERSION VERSION_GREATER PACKAGE_VERSION)\n\
                   set(PACKAGE_VERSION_COMPATIBLE FALSE)\n\
                   set(PACKAGE_VERSION_UNSUITABLE TRUE)\n\
                 elseif(PACKAGE_FIND_VERSION_MAJOR STREQUAL \"1\")\n\
                   set(PACKAGE_VERSION_COMPATIBLE TRUE)\n\
                 else()\n\
                   set(PACKAGE_VERSION_COMPATIBLE FALSE)\n\
                 endif()\n"
            ),
        )
        .expect("fake runtime version config is written");
        let build = stage.path().join(format!("build-{version}"));
        let output = Command::new("cmake")
            .arg("-S")
            .arg(&generated)
            .arg("-B")
            .arg(&build)
            .arg(format!("-DTypeBridge_DIR={}", runtime.display()))
            .output()
            .expect("generated CMake configure launches");
        assert_eq!(
            output.status.success(),
            *should_succeed,
            "generated CMake runtime requirement behaved incorrectly for {version}:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

#[cfg(unix)]
#[test]
fn generated_cmake_requires_runtime_abi_1_3_or_newer_within_major_one() {
    assert!(
        command_exists("cmake"),
        "CMake is required for C emitter acceptance"
    );
    let emitter = CEmitter::new();
    let resources = emitter.code_resources().expect("C resources hash");
    let (projection, authority) = projected(&resources);
    let package = emitter
        .emit(&projection, &authority)
        .expect("C package emits");
    assert_generated_cmake_runtime_floor(&package, &[("1.2.0", false), ("1.3.0", true)]);
}

#[cfg(unix)]
#[test]
fn ordered_generated_cmake_requires_runtime_abi_1_5_or_newer_within_major_one() {
    assert!(
        command_exists("cmake"),
        "CMake is required for C emitter acceptance"
    );
    let emitter = CEmitter::new();
    let (projection, authority) = ordered_successor_projected();
    let package = emitter
        .emit(&projection, &authority)
        .expect("ordered C-v3 package emits");
    assert_generated_cmake_runtime_floor(
        &package,
        &[("1.4.0", false), ("1.5.0", true), ("1.9.0", true)],
    );
}

#[test]
fn generated_query_facade_is_complete_and_function_pointer_compatible() {
    let emitter = CEmitter::new();
    let resources = emitter.code_resources().expect("C resources hash");
    let (projection, authority) = projected(&resources);
    let package = emitter
        .emit(&projection, &authority)
        .expect("C package emits");
    let stage = TempDirectory::new();
    write_package(&package, stage.path());
    let runtime_include = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../c/include")
        .canonicalize()
        .expect("C runtime include directory exists");
    let generated_include = stage.path().join("include");
    let consumer = stage.path().join("query-consumer.c");
    fs::write(
        &consumer,
        r#"#include <string.h>

#include <acme/models.h>

typedef type_bridge_status_t (TYPE_BRIDGE_CALL *person_binding_open_fn)(
    const acme_query_session *, acme_personzhrecord_query_exact_binding **,
    type_bridge_execution_diagnostics_t **);

typedef struct application_row {
  acme_personzhrecord *person;
  acme_actor_query_subtypes_result *actor;
} application_row;

#define EXERCISE_REMOTE(family, terminal, result)                              \
  do {                                                                          \
    acme_query_##family##_remote_pending *remote_pending = 0;                   \
    acme_query_##family##_remote_claim *remote_claim = 0;                       \
    (void)acme_query_remote_prepare_##family(                                   \
        remote_context, terminal, cancellation, &remote_pending, &diagnostics); \
    (void)acme_query_##family##_remote_pending_request_bytes(                   \
        remote_pending, &remote_request);                                        \
    (void)acme_query_##family##_remote_pending_response_snapshot_limit(         \
        remote_pending, &remote_snapshot_limit);                                 \
    (void)acme_query_##family##_remote_pending_claim(                           \
        remote_pending, cancellation, &remote_claim, &diagnostics);             \
    (void)acme_query_##family##_remote_claim_decode(                            \
        remote_claim, cancellation, remote_response, &result, &diagnostics);    \
    (void)acme_query_##family##_remote_claim_close(&remote_claim);              \
    (void)acme_query_##family##_remote_pending_close(&remote_pending);          \
  } while (0)

int main(void) {
  const type_bridge_schema_package_t *package = 0;
  const type_bridge_database_t *database = 0;
  const type_bridge_read_transaction_t *read_tx = 0;
  const type_bridge_cancellation_t *cancellation = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  acme_query_remote_context *remote_context = 0;
  acme_query_session *session = 0;
  acme_personzhrecord_query_exact_binding *person = 0;
  acme_personzhrecord_query_subtypes_binding *people = 0;
  acme_actor_query_subtypes_binding *actors = 0;
  acme_dualzhkeyzhrecord_query_exact_binding *dual = 0;
  acme_membership_query_exact_binding *membership = 0;
  acme_person_query_exact_binding *function_person = 0;
  acme_personzhrecord_query_exact_one_selection *person_one = 0;
  acme_actor_query_subtypes_collect_selection *actor_many = 0;
  acme_query *query = 0;
  acme_query *named_query = 0;
  acme_query *filtered = 0;
  acme_query *hidden = 0;
  acme_query *crossed = 0;
  acme_query_predicate *predicate = 0;
  acme_query_predicate *role_predicate = 0;
  acme_query_predicate *reachable = 0;
  acme_query_predicate *combined = 0;
  acme_query_order *order = 0;
  acme_personzhrecord_countzhvalue_query_field *count_field = 0;
  acme_personzhrecord_ratiozhvalue_query_field *ratio_field = 0;
  acme_membership_member_query_role *member_role = 0;
  acme_countzhvalue *literal = 0;
  const acme_query_order *orders[1];
  acme_query_rows_terminal *one_terminal = 0;
  acme_query_rows_terminal *rows_terminal = 0;
  acme_query_first_terminal *first_terminal = 0;
  acme_query_page_terminal *page_terminal = 0;
  acme_query_count_terminal *count_terminal = 0;
  acme_query_exists_terminal *exists_terminal = 0;
  acme_query_rows_result *rows_result = 0;
  acme_query_page_result *page_result = 0;
  acme_query_count_result *count_result = 0;
  acme_query_exists_result *exists_result = 0;
  acme_query_reduction_terminal *reduction_terminal = 0;
  acme_query_field_reduction_terminal *field_terminal = 0;
  acme_query_field_tuple_reduction_terminal *tuple_terminal = 0;
  acme_query_reduction_result *reduction_result = 0;
  acme_query_field_reduction_result *field_result = 0;
  acme_query_field_tuple_reduction_result *tuple_result = 0;
  acme_qualifyingzhscore *schema_function = 0;
  acme_query_function_integer_input *function_input = 0;
  acme_qualifyingzhscore_query_call *function_call = 0;
  acme_score *function_score = 0;
  acme_qualifyingzhscore_query_arguments_v1_t function_arguments;
  acme_query_reducer_ref_v1_t reducers[2];
  acme_query_field_group_ref_v1_t field_groups[2];
  type_bridge_query_page_metadata_v1_t page_metadata;
  type_bridge_query_reduced_value_metadata_v1_t reduced_metadata;
  type_bridge_query_reduction_group_kind_t group_kind = 0;
  type_bridge_byte_view_t output_name;
  type_bridge_byte_view_t actor_output_name;
  type_bridge_byte_view_t remote_request = {0, 0u};
  type_bridge_byte_view_t remote_response = {0, 0u};
  const uint8_t output_name_data[6] = { 'p', 'e', 'o', 'p', 'l', 'e' };
  const uint8_t actor_output_name_data[6] = { 'a', 'c', 't', 'o', 'r', 's' };
  size_t size_value = 0;
  size_t remote_snapshot_limit = 0;
  uint64_t count_value = 0;
  uint64_t double_bits = 0;
  int64_t long_value = 0;
  uint8_t exists_value = 0;
  application_row app;
  person_binding_open_fn open_person = &acme_personzhrecord_query_exact_binding_open;

  output_name.data = output_name_data;
  output_name.length = sizeof(output_name_data);
  actor_output_name.data = actor_output_name_data;
  actor_output_name.length = sizeof(actor_output_name_data);
  orders[0] = order;
  app.person = 0;
  app.actor = 0;

  (void)open_person(session, &person, &diagnostics);
  (void)acme_query_session_open(package, &session, &diagnostics);
  (void)acme_personzhrecord_query_subtypes_binding_open(
      session, &people, &diagnostics);
  (void)acme_actor_query_subtypes_binding_open(session, &actors, &diagnostics);
  (void)acme_dualzhkeyzhrecord_query_exact_binding_open(
      session, &dual, &diagnostics);
  (void)acme_membership_query_exact_binding_open(
      session, &membership, &diagnostics);
  (void)acme_person_query_exact_binding_open(
      session, &function_person, &diagnostics);
  (void)acme_personzhrecord_query_exact_binding_iid(
      person, output_name, &predicate, &diagnostics);
  (void)acme_personzhrecord_countzhvalue_query_field_from_exact(
      person, &count_field, &diagnostics);
  (void)acme_personzhrecord_ratiozhvalue_query_field_from_exact(
      person, &ratio_field, &diagnostics);
  (void)acme_query_integer_field_compare_field(
      acme_personzhrecord_countzhvalue_query_field_integer_field_ref(count_field),
      TYPE_BRIDGE_QUERY_COMPARE_EQUAL,
      acme_personzhrecord_countzhvalue_query_field_integer_field_ref(count_field),
      &predicate, &diagnostics);
  (void)acme_personzhrecord_countzhvalue_query_field_compare_value(
      count_field, TYPE_BRIDGE_QUERY_COMPARE_EQUAL, literal,
      &predicate, &diagnostics);
  (void)acme_personzhrecord_countzhvalue_query_field_presence(
      count_field, 1u, &predicate, &diagnostics);
  (void)acme_personzhrecord_countzhvalue_query_field_order(
      count_field, TYPE_BRIDGE_QUERY_SORT_ASCENDING,
      TYPE_BRIDGE_QUERY_MISSING_LAST, &order, &diagnostics);
  (void)acme_membership_member_query_role_from_exact(
      membership, &member_role, &diagnostics);
  {
    acme_membership_member_query_role_player_binding_v1_t player =
        acme_membership_member_query_role_player_personzhrecord_exact(person);
    acme_membership_member_query_role_player_binding_v1_t other =
        acme_membership_member_query_role_player_dualzhkeyzhrecord_exact(dual);
    acme_membership_query_reachable_endpoint_v1_t source =
        acme_membership_query_reachable_endpoint_v1_t_from_member(player);
    acme_membership_query_reachable_endpoint_v1_t target =
        acme_membership_query_reachable_endpoint_v1_t_from_member(other);
    (void)acme_membership_member_query_role_connects(
        member_role, player, &role_predicate, &diagnostics);
    (void)acme_membership_query_reachable(
        session, source, target, 1u, 4u, &reachable, &diagnostics);
  }
  (void)acme_query_predicate_and(
      predicate, role_predicate, &combined, &diagnostics);
  (void)acme_query_predicate_or(
      combined, reachable, &predicate, &diagnostics);
  (void)acme_query_predicate_not(predicate, &combined, &diagnostics);

  (void)acme_qualifyingzhscore_query_open(
      session, &schema_function, &diagnostics);
  (void)acme_score_query_function_input_open(
      session, function_score, &function_input, &diagnostics);
  memset(&function_arguments, 0, sizeof(function_arguments));
  function_arguments.header.struct_size = sizeof(function_arguments);
  function_arguments.header.version = TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION;
  function_arguments.argument_person =
      acme_person_query_function_argument_v1_t_from_person_exact(function_person);
  function_arguments.argument_minimum =
      acme_query_function_integer_argument_from_input(function_input);
  (void)acme_qualifyingzhscore_query_call_open(
      schema_function, &function_arguments, &function_call, &diagnostics);

  (void)acme_personzhrecord_query_exact_one_selection_open(
      person, &person_one, &diagnostics);
  (void)acme_actor_query_subtypes_collect_selection_open(
      actors, 1u, orders, 1u, &actor_many, &diagnostics);
  (void)acme_query_positional_1(
      session, acme_personzhrecord_query_exact_one_selection_ref(person_one),
      &query, &diagnostics);
  (void)acme_query_named_2(
      session, output_name,
      acme_personzhrecord_query_exact_one_selection_ref(person_one),
      actor_output_name, acme_actor_query_subtypes_collect_selection_ref(actor_many),
      &named_query, &diagnostics);
  (void)acme_query_where(named_query, combined, &filtered, &diagnostics);
  (void)acme_query_add_hidden(
      filtered, acme_dualzhkeyzhrecord_query_exact_binding_ref(dual),
      &hidden, &diagnostics);
  (void)acme_query_allow_cross_join(
      hidden, acme_personzhrecord_query_exact_binding_ref(person),
      acme_dualzhkeyzhrecord_query_exact_binding_ref(dual),
      &crossed, &diagnostics);

  (void)acme_query_one(query, orders, 1u, &one_terminal, &diagnostics);
  (void)acme_query_first(query, orders, 1u, &first_terminal, &diagnostics);
  (void)acme_query_rows(query, orders, 1u, 0u, 16u, &rows_terminal, &diagnostics);
  {
    acme_query_root_v1_t root =
        acme_personzhrecord_query_exact_binding_root(query, person);
    (void)acme_query_page(
        root, orders, 1u, 0u, 16u, 1u, &page_terminal, &diagnostics);
    (void)acme_query_count(root, &count_terminal, &diagnostics);
    (void)acme_query_exists(root, &exists_terminal, &diagnostics);
    reducers[0] = acme_query_reducer_count();
    reducers[1] =
        acme_personzhrecord_countzhvalue_query_field_reduce_sum_long(count_field);
    field_groups[0] =
        acme_personzhrecord_countzhvalue_query_field_reduction_group_slot_v1_t_from(
            count_field);
    field_groups[1] =
        acme_personzhrecord_ratiozhvalue_query_field_reduction_group_slot_v1_t_from(
            ratio_field);
    (void)acme_query_reduce(
        root, reducers, 2u, &reduction_terminal, &diagnostics);
    (void)acme_query_reduce_grouped(
        root,
        acme_personzhrecord_query_exact_reduction_group_slot_v1_t_from(person),
        reducers, 2u, &reduction_terminal, &diagnostics);
    (void)acme_query_reduce_field(
        root, field_groups[0], reducers, 2u, &field_terminal, &diagnostics);
    (void)acme_query_reduce_fields(
        root, field_groups, 2u, reducers, 2u, &tuple_terminal, &diagnostics);
  }

  (void)acme_database_query_execute_rows(
      database, rows_terminal, 0, cancellation, &rows_result, &diagnostics);
  (void)acme_database_query_execute_first(
      database, first_terminal, 0, cancellation, &rows_result, &diagnostics);
  (void)acme_read_transaction_query_execute_page(
      read_tx, page_terminal, 0, cancellation, &page_result, &diagnostics);
  (void)acme_database_query_execute_count(
      database, count_terminal, 0, cancellation, &count_result, &diagnostics);
  (void)acme_database_query_execute_exists(
      database, exists_terminal, 0, cancellation, &exists_result, &diagnostics);
  (void)acme_database_query_execute_reduction(
      database, reduction_terminal, 0, cancellation, &reduction_result,
      &diagnostics);
  (void)acme_database_query_execute_field_reduction(
      database, field_terminal, 0, cancellation, &field_result, &diagnostics);
  (void)acme_database_query_execute_field_tuple_reduction(
      database, tuple_terminal, 0, cancellation, &tuple_result, &diagnostics);

  (void)acme_query_remote_context_open(
      package, output_name, 0, &remote_context, &diagnostics);
  EXERCISE_REMOTE(rows, rows_terminal, rows_result);
  EXERCISE_REMOTE(first, first_terminal, rows_result);
  EXERCISE_REMOTE(page, page_terminal, page_result);
  EXERCISE_REMOTE(count, count_terminal, count_result);
  EXERCISE_REMOTE(exists, exists_terminal, exists_result);
  EXERCISE_REMOTE(reduction, reduction_terminal, reduction_result);
  EXERCISE_REMOTE(field_reduction, field_terminal, field_result);
  EXERCISE_REMOTE(field_tuple_reduction, tuple_terminal, tuple_result);
  (void)acme_query_remote_context_close(&remote_context);

  (void)acme_query_result_row_count(
      acme_query_rows_result_ref(rows_result), &size_value, &diagnostics);
  (void)acme_query_page_metadata(page_result, &page_metadata, &diagnostics);
  (void)acme_query_count_result_value(count_result, &count_value, &diagnostics);
  (void)acme_query_exists_result_value(exists_result, &exists_value, &diagnostics);
  {
    acme_personzhrecord_query_exact_one_result_slot_v1_t person_slot =
        acme_personzhrecord_query_exact_one_result_slot_v1_t_rows(rows_result, 0u);
    acme_actor_query_subtypes_collect_result_slot_v1_t actor_slot =
        acme_actor_query_subtypes_collect_result_slot_v1_t_page(page_result, 1u);
    (void)acme_personzhrecord_query_exact_one_at(
        person_slot, 0u, &app.person, &diagnostics);
    (void)acme_actor_query_subtypes_collect_count(
        actor_slot, 0u, &size_value, &diagnostics);
    (void)acme_actor_query_subtypes_collect_at(
        actor_slot, 0u, 0u, &app.actor, &diagnostics);
    (void)acme_actor_query_subtypes_result_as_personzhrecord(
        app.actor, &app.person, &diagnostics);
  }
  {
    acme_query_reduction_result_ref_v1_t reduction =
        acme_query_reduction_result_ref(reduction_result);
    acme_personzhrecord_query_exact_reduction_group_slot_v1_t thing_group =
        acme_personzhrecord_query_exact_reduction_group_slot_v1_t_thing(reduction);
    acme_personzhrecord_countzhvalue_query_field_reduction_group_slot_v1_t field_group =
        acme_personzhrecord_countzhvalue_query_field_reduction_group_slot_v1_t_from_result(
            reduction, 0u);
    (void)acme_query_reduction_result_row_count(
        reduction, &size_value, &diagnostics);
    (void)acme_query_reduction_group_kind(
        reduction, 0u, &group_kind, &diagnostics);
    (void)acme_query_reduction_group_field_count(
        reduction, 0u, &size_value, &diagnostics);
    (void)acme_personzhrecord_query_exact_reduction_group_slot_v1_t_thing_at(
        thing_group, 0u, &app.person, &diagnostics);
    (void)acme_personzhrecord_countzhvalue_query_field_reduction_group_slot_v1_t_value(
        field_group, 0u, &literal, &diagnostics);
    (void)acme_query_reduced_count_value(
        acme_query_reduced_count_slot(reduction, 0u), 0u,
        &count_value, &diagnostics);
    (void)acme_query_reduced_long_metadata(
        acme_query_reduced_long_slot(reduction, 1u), 0u,
        &reduced_metadata, &diagnostics);
    (void)acme_query_reduced_long_value(
        acme_query_reduced_long_slot(reduction, 1u), 0u,
        &long_value, &diagnostics);
    (void)acme_query_reduced_double_metadata(
        acme_query_reduced_double_slot(reduction, 1u), 0u,
        &reduced_metadata, &diagnostics);
    (void)acme_query_reduced_double_bits(
        acme_query_reduced_double_slot(reduction, 1u), 0u,
        &double_bits, &diagnostics);
  }

  (void)acme_query_field_tuple_reduction_result_close(&tuple_result);
  (void)acme_query_field_reduction_result_close(&field_result);
  (void)acme_query_reduction_result_close(&reduction_result);
  (void)acme_query_exists_result_close(&exists_result);
  (void)acme_query_count_result_close(&count_result);
  (void)acme_query_page_result_close(&page_result);
  (void)acme_query_rows_result_close(&rows_result);
  (void)acme_qualifyingzhscore_query_call_close(&function_call);
  (void)acme_query_function_integer_input_close(&function_input);
  (void)acme_qualifyingzhscore_query_close(&schema_function);
  (void)acme_query_field_tuple_reduction_terminal_close(&tuple_terminal);
  (void)acme_query_field_reduction_terminal_close(&field_terminal);
  (void)acme_query_reduction_terminal_close(&reduction_terminal);
  (void)acme_query_exists_terminal_close(&exists_terminal);
  (void)acme_query_count_terminal_close(&count_terminal);
  (void)acme_query_page_terminal_close(&page_terminal);
  (void)acme_query_first_terminal_close(&first_terminal);
  (void)acme_query_rows_terminal_close(&rows_terminal);
  (void)acme_query_rows_terminal_close(&one_terminal);
  (void)acme_query_close(&crossed);
  (void)acme_query_close(&hidden);
  (void)acme_query_close(&filtered);
  (void)acme_query_close(&named_query);
  (void)acme_query_close(&query);
  (void)acme_actor_query_subtypes_collect_selection_close(&actor_many);
  (void)acme_personzhrecord_query_exact_one_selection_close(&person_one);
  (void)acme_membership_member_query_role_close(&member_role);
  (void)acme_personzhrecord_ratiozhvalue_query_field_close(&ratio_field);
  (void)acme_personzhrecord_countzhvalue_query_field_close(&count_field);
  (void)acme_query_order_close(&order);
  (void)acme_query_predicate_close(&combined);
  (void)acme_query_predicate_close(&reachable);
  (void)acme_query_predicate_close(&role_predicate);
  (void)acme_query_predicate_close(&predicate);
  (void)acme_membership_query_exact_binding_close(&membership);
  (void)acme_person_query_exact_binding_close(&function_person);
  (void)acme_dualzhkeyzhrecord_query_exact_binding_close(&dual);
  (void)acme_actor_query_subtypes_binding_close(&actors);
  (void)acme_personzhrecord_query_subtypes_binding_close(&people);
  (void)acme_personzhrecord_query_exact_binding_close(&person);
  (void)acme_query_session_close(&session);
  return app.person != 0 || app.actor != 0 || exists_value != 0u;
}
"#,
    )
    .expect("query facade consumer is written");

    let mut invocations = 0;
    for (compiler, standard) in [
        ("gcc", "c11"),
        ("gcc", "c17"),
        ("clang", "c11"),
        ("clang", "c17"),
        ("g++", "c++17"),
        ("clang++", "c++17"),
    ] {
        if !command_exists(compiler) {
            continue;
        }
        invocations += 1;
        let mut command = Command::new(compiler);
        command.arg(format!("-std={standard}")).args([
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic-errors",
            "-fsyntax-only",
            "-x",
            if standard == "c++17" { "c++" } else { "c" },
        ]);
        let output = command
            .arg("-I")
            .arg(&runtime_include)
            .arg("-I")
            .arg(&generated_include)
            .arg(&consumer)
            .output()
            .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
        assert!(
            output.status.success(),
            "{compiler} rejected the complete generated query facade under {standard}:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    #[cfg(windows)]
    {
        for (language, standard) in [("/TC", "/std:c17"), ("/TP", "/std:c++17")] {
            invocations += 1;
            let output = msvc_cl(
                stage.path(),
                &[
                    "/nologo".to_owned(),
                    language.to_owned(),
                    standard.to_owned(),
                    "/W4".to_owned(),
                    "/WX".to_owned(),
                    "/Zs".to_owned(),
                    format!("/I{}", runtime_include.display()),
                    format!("/I{}", generated_include.display()),
                    consumer.display().to_string(),
                ],
            );
            assert!(
                output.status.success(),
                "MSVC cl rejected the complete generated query facade under {standard}:\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
    }
    assert!(invocations > 0, "no supported C/C++ compiler was available");
}

#[test]
fn generated_query_facade_executes_provider_free_through_the_generic_abi() {
    let emitter = CEmitter::new();
    let resources = emitter.code_resources().expect("C resources hash");
    let (projection, authority) = projected(&resources);
    let package = emitter
        .emit(&projection, &authority)
        .expect("C package emits");
    let stage = TempDirectory::new();
    write_package(&package, stage.path());
    let consumer = stage.path().join("query-provider-free.c");
    fs::write(
        &consumer,
        r#"#include <acme/models.h>

static int valid = 1;
static size_t binding_calls = 0u;
static size_t query_calls = 0u;
static size_t where_calls = 0u;
static size_t terminal_calls = 0u;
static const type_bridge_projected_token_v1_t *person_model = NULL;
static const type_bridge_projected_token_v1_t *membership_model = NULL;
static const type_bridge_projected_token_v1_t *member_role_token = NULL;

#define HANDLE(type, value) ((type *)(uintptr_t)(value))

static void clear_diagnostics(
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  if (out_diagnostics == NULL) {
    valid = 0;
  } else {
    *out_diagnostics = NULL;
  }
}


type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_session_open(
    const type_bridge_schema_package_t *package,
    type_bridge_query_session_t **out_session,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  if (package != HANDLE(type_bridge_schema_package_t, 1u) || out_session == NULL) {
    valid = 0;
  } else {
    *out_session = HANDLE(type_bridge_query_session_t, 2u);
  }
  clear_diagnostics(out_diagnostics);
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_session_close(
    type_bridge_query_session_t **session) {
  if (session == NULL || *session != HANDLE(type_bridge_query_session_t, 2u)) {
    valid = 0;
  } else {
    *session = NULL;
  }
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_binding_open_v1(
    const type_bridge_query_session_t *session,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_query_match_mode_t mode,
    type_bridge_query_binding_t **out_binding,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  ++binding_calls;
  if (session != HANDLE(type_bridge_query_session_t, 2u) || model == NULL ||
      mode != TYPE_BRIDGE_QUERY_MATCH_EXACT || out_binding == NULL) {
    valid = 0;
  }
  if (binding_calls == 1u) {
    person_model = model;
  } else if (binding_calls == 2u && model != person_model) {
    valid = 0;
  } else if (binding_calls == 3u) {
    membership_model = model;
    if (membership_model == person_model) {
      valid = 0;
    }
  }
  if (out_binding != NULL) {
    *out_binding = HANDLE(type_bridge_query_binding_t, 10u + binding_calls);
  }
  clear_diagnostics(out_diagnostics);
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_binding_close(
    type_bridge_query_binding_t **binding) {
  if (binding == NULL || *binding == NULL) {
    valid = 0;
  } else {
    *binding = NULL;
  }
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_role_open(
    const type_bridge_query_binding_t *binding,
    const type_bridge_projected_token_v1_t *role,
    type_bridge_query_role_t **out_role,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  if (binding != HANDLE(type_bridge_query_binding_t, 13u) || role == NULL ||
      out_role == NULL || membership_model == NULL) {
    valid = 0;
  } else {
    member_role_token = role;
    *out_role = HANDLE(type_bridge_query_role_t, 20u);
  }
  clear_diagnostics(out_diagnostics);
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_role_close(
    type_bridge_query_role_t **role) {
  if (role == NULL || *role != HANDLE(type_bridge_query_role_t, 20u)) {
    valid = 0;
  } else {
    *role = NULL;
  }
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_role_connects(
    const type_bridge_query_role_t *role,
    const type_bridge_projected_token_v1_t *expected_role,
    const type_bridge_query_binding_t *player,
    const type_bridge_projected_token_v1_t *expected_player_model,
    type_bridge_query_match_mode_t expected_player_mode,
    type_bridge_query_predicate_t **out_predicate,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  if (role != HANDLE(type_bridge_query_role_t, 20u) ||
      expected_role != member_role_token ||
      player != HANDLE(type_bridge_query_binding_t, 11u) ||
      expected_player_model != person_model ||
      expected_player_mode != TYPE_BRIDGE_QUERY_MATCH_EXACT ||
      out_predicate == NULL) {
    valid = 0;
  } else {
    *out_predicate = HANDLE(type_bridge_query_predicate_t, 30u);
  }
  clear_diagnostics(out_diagnostics);
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_session_reachable(
    const type_bridge_query_session_t *session,
    const type_bridge_projected_token_v1_t *relation,
    const type_bridge_projected_token_v1_t *from_role,
    const type_bridge_projected_token_v1_t *to_role,
    const type_bridge_query_binding_t *source,
    const type_bridge_projected_token_v1_t *expected_source_model,
    type_bridge_query_match_mode_t expected_source_mode,
    const type_bridge_query_binding_t *target,
    const type_bridge_projected_token_v1_t *expected_target_model,
    type_bridge_query_match_mode_t expected_target_mode,
    uint8_t min_depth, uint8_t max_depth,
    type_bridge_query_predicate_t **out_predicate,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  if (session != HANDLE(type_bridge_query_session_t, 2u) ||
      relation != membership_model || from_role != member_role_token ||
      to_role != member_role_token ||
      source != HANDLE(type_bridge_query_binding_t, 11u) ||
      target != HANDLE(type_bridge_query_binding_t, 12u) ||
      expected_source_model != person_model ||
      expected_target_model != person_model ||
      expected_source_mode != TYPE_BRIDGE_QUERY_MATCH_EXACT ||
      expected_target_mode != TYPE_BRIDGE_QUERY_MATCH_EXACT ||
      min_depth != 1u || max_depth != 4u || out_predicate == NULL) {
    valid = 0;
  } else {
    *out_predicate = HANDLE(type_bridge_query_predicate_t, 30u);
  }
  clear_diagnostics(out_diagnostics);
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_predicate_close(
    type_bridge_query_predicate_t **predicate) {
  if (predicate == NULL || *predicate != HANDLE(type_bridge_query_predicate_t, 30u)) {
    valid = 0;
  } else {
    *predicate = NULL;
  }
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_selection_open_v1(
    const type_bridge_query_selection_descriptor_v1_t *descriptor,
    type_bridge_query_selection_t **out_selection,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  if (descriptor == NULL ||
      descriptor->struct_size != sizeof(*descriptor) ||
      descriptor->version != TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION ||
      descriptor->binding != HANDLE(type_bridge_query_binding_t, 11u) ||
      descriptor->expected_model != person_model ||
      descriptor->expected_mode != TYPE_BRIDGE_QUERY_MATCH_EXACT ||
      descriptor->kind != TYPE_BRIDGE_QUERY_SELECTION_ONE ||
      descriptor->distinct != 0u || descriptor->orders != NULL ||
      descriptor->order_count != 0u || out_selection == NULL) {
    valid = 0;
  } else {
    *out_selection = HANDLE(type_bridge_query_selection_t, 40u);
  }
  clear_diagnostics(out_diagnostics);
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_selection_close(
    type_bridge_query_selection_t **selection) {
  if (selection == NULL || *selection != HANDLE(type_bridge_query_selection_t, 40u)) {
    valid = 0;
  } else {
    *selection = NULL;
  }
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_open_v1(
    const type_bridge_query_session_t *session,
    const type_bridge_query_descriptor_v1_t *descriptor,
    type_bridge_query_t **out_query,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  ++query_calls;
  if (session != HANDLE(type_bridge_query_session_t, 2u) || descriptor == NULL ||
      descriptor->struct_size != sizeof(*descriptor) ||
      descriptor->version != TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION ||
      descriptor->slot_count != 1u || descriptor->slots == NULL ||
      descriptor->slots[0].selection != HANDLE(type_bridge_query_selection_t, 40u) ||
      descriptor->slots[0].expected_model != person_model ||
      descriptor->slots[0].expected_mode != TYPE_BRIDGE_QUERY_MATCH_EXACT ||
      descriptor->slots[0].expected_kind != TYPE_BRIDGE_QUERY_SELECTION_ONE ||
      out_query == NULL) {
    valid = 0;
  }
  if (query_calls == 1u) {
    if (descriptor->shape_kind != TYPE_BRIDGE_QUERY_SHAPE_POSITIONAL ||
        descriptor->slots[0].name.data != NULL ||
        descriptor->slots[0].name.length != 0u) {
      valid = 0;
    }
  } else if (query_calls == 2u) {
    static const uint8_t expected_name[6] = { 'p', 'e', 'r', 's', 'o', 'n' };
    size_t index;
    if (descriptor->shape_kind != TYPE_BRIDGE_QUERY_SHAPE_NAMED ||
        descriptor->slots[0].name.length != sizeof(expected_name) ||
        descriptor->slots[0].name.data == NULL) {
      valid = 0;
    } else {
      for (index = 0u; index < sizeof(expected_name); ++index) {
        if (descriptor->slots[0].name.data[index] != expected_name[index]) {
          valid = 0;
        }
      }
    }
  } else {
    valid = 0;
  }
  if (out_query != NULL) {
    *out_query = HANDLE(type_bridge_query_t, 50u + query_calls);
  }
  clear_diagnostics(out_diagnostics);
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_where(
    const type_bridge_query_t *query,
    const type_bridge_query_predicate_t *predicate,
    type_bridge_query_t **out_query,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  ++where_calls;
  if (query != HANDLE(type_bridge_query_t, 52u) ||
      predicate != HANDLE(type_bridge_query_predicate_t, 30u) ||
      out_query == NULL) {
    valid = 0;
  } else {
    *out_query = HANDLE(type_bridge_query_t, 60u + where_calls);
  }
  clear_diagnostics(out_diagnostics);
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_close(
    type_bridge_query_t **query) {
  if (query == NULL || *query == NULL) {
    valid = 0;
  } else {
    *query = NULL;
  }
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_terminal_open_v1(
    const type_bridge_query_t *query,
    const type_bridge_query_terminal_descriptor_v1_t *descriptor,
    type_bridge_query_terminal_t **out_terminal,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  ++terminal_calls;
  if (descriptor == NULL || descriptor->struct_size != sizeof(*descriptor) ||
      descriptor->version != TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION ||
      out_terminal == NULL) {
    valid = 0;
  }
  if (terminal_calls == 1u) {
    if (query != HANDLE(type_bridge_query_t, 62u) ||
        descriptor->kind != TYPE_BRIDGE_QUERY_TERMINAL_ROWS ||
        descriptor->cardinality != TYPE_BRIDGE_QUERY_ROWS_BOUNDED_MANY ||
        descriptor->root != NULL || descriptor->expected_root_model != NULL ||
        descriptor->expected_root_mode != 0u || descriptor->offset != 0u ||
        descriptor->limit != 8u || descriptor->include_total != 0u) {
      valid = 0;
    }
  } else if (terminal_calls == 2u) {
    if (query != HANDLE(type_bridge_query_t, 51u) ||
        descriptor->kind != TYPE_BRIDGE_QUERY_TERMINAL_PAGE ||
        descriptor->root != HANDLE(type_bridge_query_binding_t, 12u) ||
        descriptor->expected_root_model != person_model ||
        descriptor->expected_root_mode != TYPE_BRIDGE_QUERY_MATCH_EXACT ||
        descriptor->offset != 2u || descriptor->limit != 4u ||
        descriptor->include_total != 1u) {
      valid = 0;
    }
  } else if (terminal_calls == 3u) {
    if (query != HANDLE(type_bridge_query_t, 51u) ||
        descriptor->kind != TYPE_BRIDGE_QUERY_TERMINAL_COUNT ||
        descriptor->root != HANDLE(type_bridge_query_binding_t, 12u) ||
        descriptor->expected_root_model != person_model ||
        descriptor->expected_root_mode != TYPE_BRIDGE_QUERY_MATCH_EXACT) {
      valid = 0;
    }
  } else if (terminal_calls == 4u) {
    if (query != HANDLE(type_bridge_query_t, 51u) ||
        descriptor->kind != TYPE_BRIDGE_QUERY_TERMINAL_REDUCE ||
        descriptor->cardinality != TYPE_BRIDGE_QUERY_ROWS_BOUNDED_MANY ||
        descriptor->root != HANDLE(type_bridge_query_binding_t, 12u) ||
        descriptor->expected_root_model != person_model ||
        descriptor->expected_root_mode != TYPE_BRIDGE_QUERY_MATCH_EXACT ||
        descriptor->orders != NULL || descriptor->order_count != 0u ||
        descriptor->offset != 0u || descriptor->limit != 0u ||
        descriptor->include_total != 0u || descriptor->group_binding != NULL ||
        descriptor->expected_group_model != NULL ||
        descriptor->expected_group_mode != 0u ||
        descriptor->group_fields != NULL || descriptor->group_field_count != 0u ||
        descriptor->reducers == NULL || descriptor->reducer_count != 1u ||
        descriptor->reducers[0].struct_size != sizeof(type_bridge_query_reducer_v1_t) ||
        descriptor->reducers[0].version != TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION ||
        descriptor->reducers[0].kind != TYPE_BRIDGE_QUERY_REDUCER_COUNT ||
        descriptor->reducers[0].input != NULL ||
        descriptor->reducers[0].expected_field != NULL) {
      valid = 0;
    }
  } else {
    valid = 0;
  }
  if (out_terminal != NULL) {
    *out_terminal = HANDLE(type_bridge_query_terminal_t, 70u + terminal_calls);
  }
  clear_diagnostics(out_diagnostics);
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_terminal_close(
    type_bridge_query_terminal_t **terminal) {
  if (terminal == NULL || *terminal == NULL) {
    valid = 0;
  } else {
    *terminal = NULL;
  }
  return TYPE_BRIDGE_STATUS_OK;
}

int main(void) {
  const type_bridge_schema_package_t *package =
      HANDLE(type_bridge_schema_package_t, 1u);
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  acme_query_session *session = NULL;
  acme_personzhrecord_query_exact_binding *person = NULL;
  acme_personzhrecord_query_exact_binding *sibling = NULL;
  acme_membership_query_exact_binding *membership = NULL;
  acme_membership_member_query_role *role = NULL;
  acme_query_predicate *predicate = NULL;
  acme_query_predicate *reachable = NULL;
  acme_personzhrecord_query_exact_one_selection *selection = NULL;
  acme_query *positional = NULL;
  acme_query *named = NULL;
  acme_query *branch_a = NULL;
  acme_query *branch_b = NULL;
  acme_query_rows_terminal *rows = NULL;
  acme_query_page_terminal *page = NULL;
  acme_query_count_terminal *count = NULL;
  acme_query_reduction_terminal *reduction = NULL;
  acme_query_reducer_ref_v1_t reducer;
  type_bridge_byte_view_t name;
  const uint8_t name_data[6] = { 'p', 'e', 'r', 's', 'o', 'n' };
  type_bridge_status_t status = TYPE_BRIDGE_STATUS_OK;

  name.data = name_data;
  name.length = sizeof(name_data);
  status |= acme_query_session_open(package, &session, &diagnostics);
  status |= acme_personzhrecord_query_exact_binding_open(
      session, &person, &diagnostics);
  status |= acme_personzhrecord_query_exact_binding_open(
      session, &sibling, &diagnostics);
  status |= acme_membership_query_exact_binding_open(
      session, &membership, &diagnostics);
  if ((const void *)person == (const void *)sibling) {
    valid = 0;
  }
  status |= acme_membership_member_query_role_from_exact(
      membership, &role, &diagnostics);
  {
    acme_membership_member_query_role_player_binding_v1_t player =
        acme_membership_member_query_role_player_personzhrecord_exact(person);
    acme_membership_member_query_role_player_binding_v1_t target_player =
        acme_membership_member_query_role_player_personzhrecord_exact(sibling);
    acme_membership_query_reachable_endpoint_v1_t source =
        acme_membership_query_reachable_endpoint_v1_t_from_member(player);
    acme_membership_query_reachable_endpoint_v1_t target =
        acme_membership_query_reachable_endpoint_v1_t_from_member(target_player);
    status |= acme_membership_member_query_role_connects(
        role, player, &predicate, &diagnostics);
    status |= acme_membership_query_reachable(
        session, source, target, 1u, 4u, &reachable, &diagnostics);
  }
  status |= acme_personzhrecord_query_exact_one_selection_open(
      person, &selection, &diagnostics);
  status |= acme_query_positional_1(
      session, acme_personzhrecord_query_exact_one_selection_ref(selection),
      &positional, &diagnostics);
  status |= acme_query_named_1(
      session, name,
      acme_personzhrecord_query_exact_one_selection_ref(selection),
      &named, &diagnostics);
  status |= acme_query_where(named, predicate, &branch_a, &diagnostics);
  status |= acme_query_where(named, predicate, &branch_b, &diagnostics);
  status |= acme_query_close(&branch_a);
  status |= acme_query_rows(
      branch_b, NULL, 0u, 0u, 8u, &rows, &diagnostics);
  {
    acme_query_root_v1_t root =
        acme_personzhrecord_query_exact_binding_root(positional, sibling);
    status |= acme_query_page(
        root, NULL, 0u, 2u, 4u, 1u, &page, &diagnostics);
    status |= acme_query_count(root, &count, &diagnostics);
    reducer = acme_query_reducer_count();
    status |= acme_query_reduce(
        root, &reducer, 1u, &reduction, &diagnostics);
  }

  status |= acme_query_reduction_terminal_close(&reduction);
  status |= acme_query_count_terminal_close(&count);
  status |= acme_query_page_terminal_close(&page);
  status |= acme_query_rows_terminal_close(&rows);
  status |= acme_query_close(&branch_b);
  status |= acme_query_close(&named);
  status |= acme_query_close(&positional);
  status |= acme_personzhrecord_query_exact_one_selection_close(&selection);
  status |= acme_query_predicate_close(&reachable);
  status |= acme_query_predicate_close(&predicate);
  status |= acme_membership_member_query_role_close(&role);
  status |= acme_membership_query_exact_binding_close(&membership);
  status |= acme_personzhrecord_query_exact_binding_close(&sibling);
  status |= acme_personzhrecord_query_exact_binding_close(&person);
  status |= acme_query_session_close(&session);

  return status == TYPE_BRIDGE_STATUS_OK && diagnostics == NULL && valid != 0 &&
                 binding_calls == 3u && query_calls == 2u &&
                 where_calls == 2u && terminal_calls == 4u
             ? 0
             : 1;
}
"#,
    )
    .expect("provider-free generated query consumer is written");

    let runtime_include = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../c/include")
        .canonicalize()
        .expect("C runtime include directory exists");
    let generated_include = stage.path().join("include");
    let generated_source = stage.path().join("src/models.c");
    let mut invocations = 0;
    #[cfg(unix)]
    for (compiler, standard) in [
        ("gcc", "c11"),
        ("gcc", "c17"),
        ("clang", "c11"),
        ("clang", "c17"),
    ] {
        if !command_exists(compiler) {
            continue;
        }
        invocations += 1;
        let executable = stage
            .path()
            .join(format!("query-provider-free-{compiler}-{standard}"));
        let output = Command::new(compiler)
            .arg(format!("-std={standard}"))
            .args(["-Wall", "-Wextra", "-Werror", "-pedantic-errors"])
            .args(DEAD_STRIP_COMPILE_FLAGS)
            .arg("-I")
            .arg(&runtime_include)
            .arg("-I")
            .arg(&generated_include)
            .arg(&generated_source)
            .arg(&consumer)
            .arg(DEAD_STRIP_LINK_FLAG)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
        assert!(
            output.status.success(),
            "{compiler} could not link the provider-free query probe under {standard}:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let output = Command::new(&executable)
            .output()
            .unwrap_or_else(|error| panic!("failed to run {}: {error}", executable.display()));
        assert!(
            output.status.success(),
            "{compiler}'s provider-free query probe failed under {standard} with {}:\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    #[cfg(unix)]
    for (c_compiler, cpp_compiler) in [("gcc", "g++"), ("clang", "clang++")] {
        if !command_exists(c_compiler) || !command_exists(cpp_compiler) {
            continue;
        }
        invocations += 1;
        let generated_object = stage.path().join(format!("query-{c_compiler}-models.o"));
        let consumer_object = stage
            .path()
            .join(format!("query-{cpp_compiler}-consumer.o"));
        let executable = stage
            .path()
            .join(format!("query-provider-free-{cpp_compiler}"));
        let output = Command::new(c_compiler)
            .args([
                "-std=c11",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pedantic-errors",
            ])
            .args(DEAD_STRIP_COMPILE_FLAGS)
            .arg("-c")
            .arg("-I")
            .arg(&runtime_include)
            .arg("-I")
            .arg(&generated_include)
            .arg(&generated_source)
            .arg("-o")
            .arg(&generated_object)
            .output()
            .unwrap_or_else(|error| panic!("failed to launch {c_compiler}: {error}"));
        assert!(
            output.status.success(),
            "{c_compiler} could not compile the generated provider-free package source:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let output = Command::new(cpp_compiler)
            .args([
                "-std=c++17",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pedantic-errors",
            ])
            .args(DEAD_STRIP_COMPILE_FLAGS)
            .args(["-x", "c++", "-c"])
            .arg("-I")
            .arg(&runtime_include)
            .arg("-I")
            .arg(&generated_include)
            .arg(&consumer)
            .arg("-o")
            .arg(&consumer_object)
            .output()
            .unwrap_or_else(|error| panic!("failed to launch {cpp_compiler}: {error}"));
        assert!(
            output.status.success(),
            "{cpp_compiler} rejected the executable provider-free C++17 consumer:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let output = Command::new(cpp_compiler)
            .arg(&generated_object)
            .arg(&consumer_object)
            .arg(DEAD_STRIP_LINK_FLAG)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap_or_else(|error| panic!("failed to link with {cpp_compiler}: {error}"));
        assert!(
            output.status.success(),
            "{cpp_compiler} could not link the provider-free C++17 query probe:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let output = Command::new(&executable)
            .output()
            .unwrap_or_else(|error| panic!("failed to run {}: {error}", executable.display()));
        assert!(
            output.status.success(),
            "{cpp_compiler}'s provider-free C++17 query probe failed with {}:\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    #[cfg(windows)]
    for compiler in required_windows_provider_free_compilers(stage.path()) {
        let variants: &[(&str, &str)] = if compiler == "cl" {
            &[("/TC", "/std:c17"), ("/TP", "/std:c++17")]
        } else {
            &[
                ("/TC", "/std:c11"),
                ("/TC", "/std:c17"),
                ("/TP", "/std:c++17"),
            ]
        };
        for (language, standard) in variants {
            invocations += 1;
            let stem = format!(
                "query-provider-free-{compiler}-{}",
                standard.trim_start_matches("/std:")
            );
            let executable = compile_msvc_provider_free_probe(
                stage.path(),
                MsvcProviderFreeProbe {
                    compiler,
                    consumer_language: language,
                    consumer_standard: standard,
                    runtime_include: &runtime_include,
                    generated_include: &generated_include,
                    generated_source: &generated_source,
                    consumer: &consumer,
                    stem: &stem,
                },
            );
            let output = Command::new(&executable)
                .output()
                .unwrap_or_else(|error| panic!("failed to run {}: {error}", executable.display()));
            assert!(
                output.status.success(),
                "{compiler}'s provider-free query probe failed under {standard} with {}:\nstdout:\n{}\nstderr:\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
    }
    assert!(
        invocations > 0,
        "no supported C/C++ compiler was available for provider-free query evidence"
    );
}

#[test]
fn generated_schema_function_facade_executes_provider_free_with_exact_graph_layout() {
    let emitter = CEmitter::new();
    let resources = emitter.code_resources().expect("C resources hash");
    let (projection, authority) = projected(&resources);
    let package = emitter
        .emit(&projection, &authority)
        .expect("C package emits");
    let stage = TempDirectory::new();
    write_package(&package, stage.path());
    let consumer = stage.path().join("query-function-provider-free.c");
    fs::write(
        &consumer,
        r#"#include <acme/models.h>
#include <stdint.h>
#include <string.h>

static int valid = 1;
static size_t call_open_count = 0u;
static size_t comparison_count = 0u;
static const type_bridge_projected_token_v1_t *function_token = NULL;
static const type_bridge_projected_token_v1_t *person_token = NULL;
static const type_bridge_projected_token_v1_t *score_field_token = NULL;

#define HANDLE(type, value) ((type *)(uintptr_t)(value))

static void clear_diagnostics(
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  if (out_diagnostics == NULL) {
    valid = 0;
  } else {
    *out_diagnostics = NULL;
  }
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_session_open(
    const type_bridge_schema_package_t *package,
    type_bridge_query_session_t **out_session,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  if (package != HANDLE(type_bridge_schema_package_t, 1u) ||
      out_session == NULL) {
    valid = 0;
  } else {
    *out_session = HANDLE(type_bridge_query_session_t, 2u);
  }
  clear_diagnostics(out_diagnostics);
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_session_close(
    type_bridge_query_session_t **session) {
  if (session == NULL || *session != HANDLE(type_bridge_query_session_t, 2u)) {
    valid = 0;
  } else {
    *session = NULL;
  }
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_binding_open_v1(
    const type_bridge_query_session_t *session,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_query_match_mode_t mode,
    type_bridge_query_binding_t **out_binding,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  if (session != HANDLE(type_bridge_query_session_t, 2u) || model == NULL ||
      model->kind != TYPE_BRIDGE_PROJECTED_TOKEN_MODEL ||
      mode != TYPE_BRIDGE_QUERY_MATCH_EXACT || out_binding == NULL) {
    valid = 0;
  } else {
    person_token = model;
    *out_binding = HANDLE(type_bridge_query_binding_t, 3u);
  }
  clear_diagnostics(out_diagnostics);
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_binding_close(
    type_bridge_query_binding_t **binding) {
  if (binding == NULL || *binding != HANDLE(type_bridge_query_binding_t, 3u)) {
    valid = 0;
  } else {
    *binding = NULL;
  }
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_field_open(
    const type_bridge_query_binding_t *binding,
    const type_bridge_projected_token_v1_t *field,
    type_bridge_query_field_t **out_field,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  if (binding != HANDLE(type_bridge_query_binding_t, 3u) || field == NULL ||
      field->kind != TYPE_BRIDGE_PROJECTED_TOKEN_FIELD || out_field == NULL) {
    valid = 0;
  } else {
    score_field_token = field;
    *out_field = HANDLE(type_bridge_query_field_t, 4u);
  }
  clear_diagnostics(out_diagnostics);
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_field_close(
    type_bridge_query_field_t **field) {
  if (field == NULL || *field != HANDLE(type_bridge_query_field_t, 4u)) {
    valid = 0;
  } else {
    *field = NULL;
  }
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_function_open(
    const type_bridge_query_session_t *session,
    const type_bridge_projected_token_v1_t *function,
    type_bridge_query_function_t **out_function,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  if (session != HANDLE(type_bridge_query_session_t, 2u) || function == NULL ||
      function->kind != TYPE_BRIDGE_PROJECTED_TOKEN_FUNCTION ||
      function->ordinal != 1u || out_function == NULL) {
    valid = 0;
  } else {
    function_token = function;
    *out_function = HANDLE(type_bridge_query_function_t, 5u);
  }
  clear_diagnostics(out_diagnostics);
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_function_close(
    type_bridge_query_function_t **function) {
  if (function == NULL || *function != HANDLE(type_bridge_query_function_t, 5u)) {
    valid = 0;
  } else {
    *function = NULL;
  }
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_function_value_open(
    const type_bridge_query_session_t *session,
    const type_bridge_projected_value_t *value,
    type_bridge_query_function_value_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  if (session != HANDLE(type_bridge_query_session_t, 2u) ||
      value != HANDLE(type_bridge_projected_value_t, 40u) || out_value == NULL) {
    valid = 0;
  } else {
    *out_value = HANDLE(type_bridge_query_function_value_t, 41u);
  }
  clear_diagnostics(out_diagnostics);
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_function_value_close(
    type_bridge_query_function_value_t **value) {
  if (value == NULL ||
      *value != HANDLE(type_bridge_query_function_value_t, 41u)) {
    valid = 0;
  } else {
    *value = NULL;
  }
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_function_call_open_v1(
    const type_bridge_query_function_t *function,
    const type_bridge_projected_token_v1_t *expected_function,
    const type_bridge_query_function_arguments_graph_v1_t *graph,
    type_bridge_query_function_call_t **out_call,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  type_bridge_query_function_argument_v1_t person;
  type_bridge_query_function_argument_v1_t minimum;
  const type_bridge_query_function_arguments_header_v1_t *header;
  ++call_open_count;
  if (function != HANDLE(type_bridge_query_function_t, 5u) ||
      expected_function != function_token || graph == NULL ||
      graph->struct_size != sizeof(*graph) ||
      graph->version != TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION ||
      graph->args == NULL ||
      graph->args_size != sizeof(acme_qualifyingzhscore_query_arguments_v1_t) ||
      graph->members == NULL || graph->member_count != 2u ||
      out_call == NULL) {
    valid = 0;
  } else {
    header = (const type_bridge_query_function_arguments_header_v1_t *)graph->args;
    if (header->struct_size != graph->args_size ||
        header->version != TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION ||
        graph->members[0].args_offset !=
            offsetof(acme_qualifyingzhscore_query_arguments_v1_t, argument_person) ||
        graph->members[1].args_offset !=
            offsetof(acme_qualifyingzhscore_query_arguments_v1_t, argument_minimum)) {
      valid = 0;
    }
    memcpy(&person,
           (const unsigned char *)graph->args + graph->members[0].args_offset,
           sizeof(person));
    memcpy(&minimum,
           (const unsigned char *)graph->args + graph->members[1].args_offset,
           sizeof(minimum));
    if (person.struct_size != sizeof(person) ||
        person.version != TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION ||
        person.kind != TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENT_BINDING ||
        person.binding != HANDLE(type_bridge_query_binding_t, 3u) ||
        person.value != NULL || person.call != NULL ||
        minimum.struct_size != sizeof(minimum) ||
        minimum.version != TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION) {
      valid = 0;
    }
    if (call_open_count == 1u &&
        (minimum.kind != TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENT_VALUE ||
         minimum.value != HANDLE(type_bridge_query_function_value_t, 41u) ||
         minimum.binding != NULL || minimum.call != NULL)) {
      valid = 0;
    }
    if (call_open_count == 2u &&
        (minimum.kind != TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENT_CALL ||
         minimum.call != HANDLE(type_bridge_query_function_call_t, 6u) ||
         minimum.binding != NULL || minimum.value != NULL)) {
      valid = 0;
    }
    *out_call = HANDLE(type_bridge_query_function_call_t, 5u + call_open_count);
  }
  clear_diagnostics(out_diagnostics);
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_function_call_close(
    type_bridge_query_function_call_t **call) {
  if (call == NULL || (*call != HANDLE(type_bridge_query_function_call_t, 6u) &&
                       *call != HANDLE(type_bridge_query_function_call_t, 7u))) {
    valid = 0;
  } else {
    *call = NULL;
  }
  return TYPE_BRIDGE_STATUS_OK;
}

static void predicate_result(
    type_bridge_query_predicate_t **out_predicate,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  ++comparison_count;
  if (out_predicate == NULL) {
    valid = 0;
  } else {
    *out_predicate = HANDLE(type_bridge_query_predicate_t,
                            50u + comparison_count);
  }
  clear_diagnostics(out_diagnostics);
}

type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_function_call_compare_field(
    const type_bridge_query_function_call_t *call,
    type_bridge_query_comparison_t comparison,
    const type_bridge_query_field_t *field,
    const type_bridge_projected_token_v1_t *expected_field,
    type_bridge_query_predicate_t **out_predicate,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  if (call != HANDLE(type_bridge_query_function_call_t, 6u) ||
      comparison != TYPE_BRIDGE_QUERY_COMPARE_EQUAL ||
      field != HANDLE(type_bridge_query_field_t, 4u) ||
      expected_field != score_field_token) {
    valid = 0;
  }
  predicate_result(out_predicate, out_diagnostics);
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_function_call_compare_value(
    const type_bridge_query_function_call_t *call,
    type_bridge_query_comparison_t comparison,
    const type_bridge_query_function_value_t *value,
    type_bridge_query_predicate_t **out_predicate,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  if (call != HANDLE(type_bridge_query_function_call_t, 6u) ||
      comparison != TYPE_BRIDGE_QUERY_COMPARE_LESS_THAN ||
      value != HANDLE(type_bridge_query_function_value_t, 41u)) {
    valid = 0;
  }
  predicate_result(out_predicate, out_diagnostics);
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_function_call_compare_call(
    const type_bridge_query_function_call_t *call,
    type_bridge_query_comparison_t comparison,
    const type_bridge_query_function_call_t *other,
    type_bridge_query_predicate_t **out_predicate,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  if (call != HANDLE(type_bridge_query_function_call_t, 7u) ||
      comparison != TYPE_BRIDGE_QUERY_COMPARE_GREATER_THAN ||
      other != HANDLE(type_bridge_query_function_call_t, 6u)) {
    valid = 0;
  }
  predicate_result(out_predicate, out_diagnostics);
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_field_compare_function(
    const type_bridge_query_field_t *field,
    const type_bridge_projected_token_v1_t *expected_field,
    type_bridge_query_comparison_t comparison,
    const type_bridge_query_function_call_t *call,
    type_bridge_query_predicate_t **out_predicate,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  if (field != HANDLE(type_bridge_query_field_t, 4u) ||
      expected_field != score_field_token ||
      comparison != TYPE_BRIDGE_QUERY_COMPARE_LESS_THAN_OR_EQUAL ||
      call != HANDLE(type_bridge_query_function_call_t, 7u)) {
    valid = 0;
  }
  predicate_result(out_predicate, out_diagnostics);
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t TYPE_BRIDGE_CALL type_bridge_query_predicate_close(
    type_bridge_query_predicate_t **predicate) {
  if (predicate == NULL || *predicate == NULL) {
    valid = 0;
  } else {
    *predicate = NULL;
  }
  return TYPE_BRIDGE_STATUS_OK;
}

int main(void) {
  acme_query_session *session = NULL;
  acme_person_query_exact_binding *person = NULL;
  acme_person_score_query_field *score_field = NULL;
  acme_qualifyingzhscore *function = NULL;
  acme_query_function_integer_input *minimum = NULL;
  acme_qualifyingzhscore_query_call *call = NULL;
  acme_qualifyingzhscore_query_call *nested = NULL;
  acme_query_predicate *predicate = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  acme_qualifyingzhscore_query_arguments_v1_t arguments;

  if (acme_query_session_open(HANDLE(type_bridge_schema_package_t, 1u),
                              &session, &diagnostics) != TYPE_BRIDGE_STATUS_OK ||
      acme_person_query_exact_binding_open(
          session, &person, &diagnostics) != TYPE_BRIDGE_STATUS_OK ||
      acme_person_score_query_field_from_exact(
          person, &score_field, &diagnostics) != TYPE_BRIDGE_STATUS_OK ||
      acme_score_query_function_input_open(
          session, HANDLE(acme_score, 40u), &minimum,
          &diagnostics) != TYPE_BRIDGE_STATUS_OK ||
      acme_qualifyingzhscore_query_open(
          session, &function, &diagnostics) != TYPE_BRIDGE_STATUS_OK) {
    return 1;
  }
  arguments.header.struct_size = sizeof(arguments);
  arguments.header.version = TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION;
  arguments.header.reserved[0] = 0u;
  arguments.header.reserved[1] = 0u;
  arguments.header.reserved[2] = 0u;
  arguments.header.reserved[3] = 0u;
  arguments.argument_person =
      acme_person_query_function_argument_v1_t_from_person_exact(person);
  arguments.argument_minimum =
      acme_query_function_integer_argument_from_input(minimum);
  if (acme_qualifyingzhscore_query_call_open(
          function, &arguments, &call, &diagnostics) != TYPE_BRIDGE_STATUS_OK) {
    return 2;
  }
  arguments.argument_minimum =
      acme_query_function_integer_argument_from_call(
          acme_qualifyingzhscore_query_call_ref(call));
  if (acme_qualifyingzhscore_query_call_open(
          function, &arguments, &nested, &diagnostics) != TYPE_BRIDGE_STATUS_OK) {
    return 3;
  }
  if (acme_qualifyingzhscore_query_call_equal_field(
          call, acme_person_score_query_field_function_field(score_field),
          &predicate, &diagnostics) != TYPE_BRIDGE_STATUS_OK) {
    return 4;
  }
  (void)acme_query_predicate_close(&predicate);
  if (acme_qualifyingzhscore_query_call_less_than_value(
          call, minimum, &predicate, &diagnostics) != TYPE_BRIDGE_STATUS_OK) {
    return 5;
  }
  (void)acme_query_predicate_close(&predicate);
  if (acme_qualifyingzhscore_query_call_greater_than_call(
          nested, acme_qualifyingzhscore_query_call_ref(call),
          &predicate, &diagnostics) != TYPE_BRIDGE_STATUS_OK) {
    return 6;
  }
  (void)acme_query_predicate_close(&predicate);
  if (acme_query_function_integer_field_less_than_or_equal_call(
          acme_person_score_query_field_function_field(score_field),
          acme_qualifyingzhscore_query_call_ref(nested),
          &predicate, &diagnostics) != TYPE_BRIDGE_STATUS_OK) {
    return 7;
  }
  (void)acme_query_predicate_close(&predicate);
  (void)acme_qualifyingzhscore_query_call_close(&nested);
  (void)acme_qualifyingzhscore_query_call_close(&call);
  (void)acme_query_function_integer_input_close(&minimum);
  (void)acme_qualifyingzhscore_query_close(&function);
  (void)acme_person_score_query_field_close(&score_field);
  (void)acme_person_query_exact_binding_close(&person);
  (void)acme_query_session_close(&session);
  return valid && call_open_count == 2u && comparison_count == 4u &&
                 person_token != NULL && function_token != NULL
             ? 0
             : 8;
}
"#,
    )
    .expect("provider-free function consumer is written");

    let runtime_include = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../c/include")
        .canonicalize()
        .expect("C runtime include directory exists");
    let generated_include = stage.path().join("include");
    let generated_source = stage.path().join("src/models.c");
    let mut invocations = 0;
    #[cfg(unix)]
    for (compiler, standard, language) in [
        ("gcc", "c11", "c"),
        ("gcc", "c17", "c"),
        ("clang", "c11", "c"),
        ("clang", "c17", "c"),
        ("g++", "c++17", "c++"),
        ("clang++", "c++17", "c++"),
    ] {
        if !command_exists(compiler) {
            continue;
        }
        invocations += 1;
        let executable = stage.path().join(format!(
            "query-function-provider-free-{compiler}-{standard}"
        ));
        let output = if language == "c" {
            Command::new(compiler)
                .arg(format!("-std={standard}"))
                .args(["-Wall", "-Wextra", "-Werror", "-pedantic-errors"])
                .args(DEAD_STRIP_COMPILE_FLAGS)
                .arg("-I")
                .arg(&runtime_include)
                .arg("-I")
                .arg(&generated_include)
                .arg(&generated_source)
                .arg(&consumer)
                .args([DEAD_STRIP_LINK_FLAG, "-o"])
                .arg(&executable)
                .output()
                .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"))
        } else {
            let c_compiler = if compiler == "g++" { "gcc" } else { "clang" };
            let generated_object = stage.path().join(format!(
                "query-function-provider-free-{compiler}-generated.o"
            ));
            let consumer_object = stage.path().join(format!(
                "query-function-provider-free-{compiler}-consumer.o"
            ));
            let source_output = Command::new(c_compiler)
                .args([
                    "-std=c11",
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-pedantic-errors",
                ])
                .args(DEAD_STRIP_COMPILE_FLAGS)
                .arg("-c")
                .arg("-I")
                .arg(&runtime_include)
                .arg("-I")
                .arg(&generated_include)
                .arg(&generated_source)
                .arg("-o")
                .arg(&generated_object)
                .output()
                .unwrap_or_else(|error| panic!("failed to launch {c_compiler}: {error}"));
            assert!(
                source_output.status.success(),
                "{c_compiler} could not compile the generated function package source:\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&source_output.stdout),
                String::from_utf8_lossy(&source_output.stderr),
            );
            let consumer_output = Command::new(compiler)
                .args([
                    "-std=c++17",
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-pedantic-errors",
                ])
                .args(DEAD_STRIP_COMPILE_FLAGS)
                .args(["-x", "c++", "-c"])
                .arg("-I")
                .arg(&runtime_include)
                .arg("-I")
                .arg(&generated_include)
                .arg(&consumer)
                .arg("-o")
                .arg(&consumer_object)
                .output()
                .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
            assert!(
                consumer_output.status.success(),
                "{compiler} rejected the provider-free function C++ consumer:\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&consumer_output.stdout),
                String::from_utf8_lossy(&consumer_output.stderr),
            );
            Command::new(compiler)
                .arg(&generated_object)
                .arg(&consumer_object)
                .args([DEAD_STRIP_LINK_FLAG, "-o"])
                .arg(&executable)
                .output()
                .unwrap_or_else(|error| panic!("failed to link with {compiler}: {error}"))
        };
        assert!(
            output.status.success(),
            "{compiler} could not link the provider-free function facade under {standard}:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let output = Command::new(&executable)
            .output()
            .unwrap_or_else(|error| panic!("failed to run {}: {error}", executable.display()));
        assert!(
            output.status.success(),
            "{compiler}'s provider-free function facade failed under {standard} with {}:\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    #[cfg(windows)]
    for compiler in required_windows_provider_free_compilers(stage.path()) {
        let variants: &[(&str, &str)] = if compiler == "cl" {
            &[("/TC", "/std:c17"), ("/TP", "/std:c++17")]
        } else {
            &[
                ("/TC", "/std:c11"),
                ("/TC", "/std:c17"),
                ("/TP", "/std:c++17"),
            ]
        };
        for (language, standard) in variants {
            invocations += 1;
            let stem = format!(
                "query-function-provider-free-{compiler}-{}",
                standard.trim_start_matches("/std:")
            );
            let executable = compile_msvc_provider_free_probe(
                stage.path(),
                MsvcProviderFreeProbe {
                    compiler,
                    consumer_language: language,
                    consumer_standard: standard,
                    runtime_include: &runtime_include,
                    generated_include: &generated_include,
                    generated_source: &generated_source,
                    consumer: &consumer,
                    stem: &stem,
                },
            );
            let output = Command::new(&executable)
                .output()
                .unwrap_or_else(|error| panic!("failed to run {}: {error}", executable.display()));
            assert!(
                output.status.success(),
                "{compiler}'s provider-free function facade failed under {standard} with {}:\nstdout:\n{}\nstderr:\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
    }
    assert!(
        invocations > 0,
        "no supported C/C++ compiler was available for provider-free function evidence",
    );
}

#[test]
fn generated_subtype_results_include_create_disabled_concrete_descendants() {
    const SOURCE: &str = r#"format: typebridge.schema/v2
entities:
  participant: {}
relations:
  activity:
    abstract: true
  base-event:
    sub: activity
    relates:
      participant: { abstract: true, card: 1 }
plays:
  participant:
    base-event: [participant]
"#;

    let emitter = CEmitter::new();
    let resources = emitter.code_resources().expect("C resources hash");
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("c-query-subtype.yaml").expect("fixture document ID is valid"),
        SOURCE,
    )])
    .expect("query subtype fixture parses");
    let declared = normalize_documents(&documents).expect("query subtype fixture normalizes");
    let profile = SemanticProfileId::new(support::TEST_PROFILE).expect("test profile is valid");
    let resolved = resolve(&declared, &profile).expect("query subtype fixture resolves");
    let projection = project(
        &resolved,
        BindingTarget::C,
        &ProjectionConfig::c(CSymbolPrefix::new("hierarchy").expect("test prefix is valid")),
        &emitter.generator_handlers(),
        &resources,
    )
    .expect("query subtype fixture projects");
    let activity_id = TypeId::new(TypeKind::Relation, "activity").unwrap();
    let child_id = TypeId::new(TypeKind::Relation, "base-event").unwrap();
    let activity = &projection.models()[&activity_id];
    let child = &projection.models()[&child_id];
    assert!(activity.declaration().is_abstract());
    assert!(!child.declaration().is_abstract());
    assert!(!child.declaration().is_constructible());
    assert!(child.create().target_name().is_none());

    let package = emitter
        .emit(&projection, &support::authority(SOURCE))
        .expect("query subtype package emits");
    let header = std::str::from_utf8(package.get("include/hierarchy/models.h").unwrap())
        .expect("generated query subtype header is UTF-8");
    let child_suffix = child
        .target_name()
        .as_str()
        .strip_prefix("hierarchy_")
        .expect("generated child target retains package prefix");
    let accessor = format!(
        "{}_query_subtypes_result_as_{child_suffix}",
        activity.target_name().as_str()
    );
    assert!(
        header.contains(&format!(
            "static inline type_bridge_status_t TYPE_BRIDGE_CALL {accessor}("
        )),
        "a complete read target must remain in the subtype union even when create is disabled",
    );

    let stage = TempDirectory::new();
    write_package(&package, stage.path());
    let consumer = stage.path().join("subtype-consumer.c");
    fs::write(
        &consumer,
        format!(
            "#include <hierarchy/models.h>\n\nint main(void) {{\n  {}_query_subtypes_result *value = 0;\n  {} *child = 0;\n  type_bridge_execution_diagnostics_t *diagnostics = 0;\n  (void){accessor}(value, &child, &diagnostics);\n  return child != 0;\n}}\n",
            activity.target_name().as_str(),
            child.target_name().as_str(),
        ),
    )
    .expect("query subtype consumer is written");
    let runtime_include = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../c/include")
        .canonicalize()
        .expect("C runtime include directory exists");
    let generated_include = stage.path().join("include");
    let mut invocations = 0;
    for (compiler, standard, language) in [
        ("gcc", "c11", "c"),
        ("clang", "c17", "c"),
        ("g++", "c++17", "c++"),
        ("clang++", "c++17", "c++"),
    ] {
        if !command_exists(compiler) {
            continue;
        }
        invocations += 1;
        let output = Command::new(compiler)
            .arg(format!("-std={standard}"))
            .args([
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pedantic-errors",
                "-fsyntax-only",
                "-x",
                language,
            ])
            .arg("-I")
            .arg(&runtime_include)
            .arg("-I")
            .arg(&generated_include)
            .arg(&consumer)
            .output()
            .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
        assert!(
            output.status.success(),
            "{compiler} rejected the create-disabled subtype result under {standard}:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    assert!(invocations > 0, "no supported C/C++ compiler was available");
}

#[test]
fn generated_header_is_strict_cpp17_for_installed_compilers() {
    let emitter = CEmitter::new();
    let resources = emitter.code_resources().expect("C resources hash");
    let (projection, authority) = projected(&resources);
    let package = emitter
        .emit(&projection, &authority)
        .expect("C package emits");
    let stage = TempDirectory::new();
    write_package(&package, stage.path());
    let consumer = stage.path().join("consumer.cpp");
    fs::write(
        &consumer,
        r#"#include <acme/models.h>

static_assert(TYPE_BRIDGE_C_ABI_MAJOR == 1u, "C ABI major changed");
static_assert(TYPE_BRIDGE_C_ABI_MINOR == 5u, "C ABI minor changed");
static_assert(sizeof(acme_membership_member_player_kind_t) == sizeof(uint32_t),
              "role-player kind ABI is not fixed-width");

int main() {
  const type_bridge_schema_package_t *package = nullptr;
  type_bridge_schema_package_t *opened_package = nullptr;
  type_bridge_diagnostics_t *package_diagnostics = nullptr;
  const type_bridge_database_t *database = nullptr;
  const type_bridge_read_transaction_t *read_tx = nullptr;
  const type_bridge_write_transaction_t *write_tx = nullptr;
  const type_bridge_cancellation_t *cancellation = nullptr;
  type_bridge_execution_diagnostics_t *diagnostics = nullptr;
  acme_personzhrecord *person = nullptr;
  acme_personzhrecord_create *create = nullptr;
  acme_personzhrecord_ref *reference = nullptr;
  acme_dualzhkeyzhrecord_ref *dual_reference = nullptr;
  acme_event_ref *event_reference = nullptr;
  acme_membership *membership = nullptr;
  acme_membership_create *membership_create = nullptr;
  acme_membership_ref *membership_reference = nullptr;
  acme_membership_member_player *member_player = nullptr;
  acme_membership_member_player_kind_t member_kind =
      acme_membership_member_player_kind_unknown;
  acme_container_item_player *item_player = nullptr;
  acme_aliases *alias = nullptr;
  const acme_aliases *aliases[1] = {alias};
  acme_personzhrecord_create_field_aliases_chunks_v1_t aliases_chunk{};
  acme_personzhrecord_create_args_v1_t create_args{};
  acme_membership_create_args_v1_t membership_args{};
  acme_countzhvalue *count_value = nullptr;
  acme_datezhvalue *date_value = nullptr;
  acme_datetimezhvalue *datetime_value = nullptr;
  acme_decimalzhvalue *decimal_value = nullptr;
  acme_displayzhname *display_name = nullptr;
  acme_durationzhvalue *duration_value = nullptr;
  acme_enabledzhvalue *enabled_value = nullptr;
  acme_ratiozhvalue *ratio_value = nullptr;
  acme_zzonedzhvalue *zoned_value = nullptr;
  type_bridge_byte_view_t text{};
  int64_t scalar = 0;
  size_t field_count = 0;
  uint64_t thing_count = 0;
  (void)acme_schema_package_open(&opened_package, &package_diagnostics);
  aliases_chunk.struct_size = sizeof(aliases_chunk);
  aliases_chunk.version = ACME_CREATE_ARGS_VERSION;
  aliases_chunk.values = aliases;
  aliases_chunk.count = 1u;
  create_args.struct_size = sizeof(create_args);
  create_args.version = ACME_CREATE_ARGS_VERSION;
  create_args.field_aliases_chunks = &aliases_chunk;
  create_args.field_countzhvalue = count_value;
  create_args.field_datezhvalue = date_value;
  create_args.field_datetimezhvalue = datetime_value;
  create_args.field_decimalzhvalue = decimal_value;
  create_args.field_displayzhname = display_name;
  create_args.field_durationzhvalue = duration_value;
  create_args.field_enabledzhvalue = enabled_value;
  create_args.field_ratiozhvalue = ratio_value;
  create_args.field_zzonedzhvalue = zoned_value;
  (void)acme_personzhrecord_create_open(
      package, &create_args, &create, &diagnostics);
  (void)acme_personzhrecord_ref_from_key(
      package, display_name, &reference, &diagnostics);
  (void)acme_personzhrecord_ref_iid(reference, &text, &diagnostics);
  (void)acme_personzhrecord_ref_displayzhname_key(
      reference, &display_name, &diagnostics);
  (void)acme_personzhrecord_iid(person, &text, &diagnostics);
  (void)acme_personzhrecord_reference(person, &reference, &diagnostics);
  (void)acme_personzhrecord_database_insert(
      database, create, cancellation, &person, &diagnostics);
  (void)acme_personzhrecord_countzhvalue(person, &count_value, &diagnostics);
  (void)acme_personzhrecord_ratiozhvalue(person, &ratio_value, &diagnostics);
  (void)acme_personzhrecord_aliases_count(person, &field_count, &diagnostics);
  (void)acme_personzhrecord_aliases_at(person, 0u, &alias, &diagnostics);
  (void)acme_countzhvalue_value(count_value, &scalar, &diagnostics);
  (void)acme_membership_member_player_from_personzhrecord(
      reference, &member_player, &diagnostics);
  (void)acme_membership_member_player_kind(
      member_player, &member_kind, &diagnostics);
  (void)acme_membership_member_player_as_personzhrecord(
      member_player, &reference, &diagnostics);
  (void)acme_membership_member_player_from_dualzhkeyzhrecord(
      dual_reference, &member_player, &diagnostics);
  (void)acme_membership_member_player_as_dualzhkeyzhrecord(
      member_player, &dual_reference, &diagnostics);
  (void)acme_container_item_player_from_event(
      event_reference, &item_player, &diagnostics);
  membership_args.struct_size = sizeof(membership_args);
  membership_args.version = ACME_CREATE_ARGS_VERSION;
  membership_args.role_member = member_player;
  (void)acme_membership_create_open(
      package, &membership_args, &membership_create, &diagnostics);
  (void)acme_membership_database_insert(
      database, membership_create, cancellation, &membership, &diagnostics);
  (void)acme_membership_iid(membership, &text, &diagnostics);
  (void)acme_membership_reference(
      membership, &membership_reference, &diagnostics);
  (void)acme_membership_member(membership, &member_player, &diagnostics);

  (void)acme_personzhrecord_database_insert(
      database, create, cancellation, &person, &diagnostics);
  (void)acme_personzhrecord_database_put(
      database, create, cancellation, &person, &diagnostics);
  (void)acme_personzhrecord_database_get_by_iid(
      database, text, cancellation, &person, &diagnostics);
  (void)acme_personzhrecord_database_update(
      database, text, create, cancellation, &person, &diagnostics);
  (void)acme_personzhrecord_database_delete_by_iid(
      database, text, cancellation, &diagnostics);
  (void)acme_personzhrecord_database_count(
      database, cancellation, &thing_count, &diagnostics);
  (void)acme_personzhrecord_read_transaction_get_by_iid(
      read_tx, text, cancellation, &person, &diagnostics);
  (void)acme_personzhrecord_read_transaction_count(
      read_tx, cancellation, &thing_count, &diagnostics);
  (void)acme_personzhrecord_write_transaction_insert(
      write_tx, create, cancellation, &person, &diagnostics);
  (void)acme_personzhrecord_write_transaction_put(
      write_tx, create, cancellation, &person, &diagnostics);
  (void)acme_personzhrecord_write_transaction_get_by_iid(
      write_tx, text, cancellation, &person, &diagnostics);
  (void)acme_personzhrecord_write_transaction_update(
      write_tx, text, create, cancellation, &person, &diagnostics);
  (void)acme_personzhrecord_write_transaction_delete_by_iid(
      write_tx, text, cancellation, &diagnostics);
  (void)acme_personzhrecord_write_transaction_count(
      write_tx, cancellation, &thing_count, &diagnostics);

  (void)acme_membership_database_insert(
      database, membership_create, cancellation, &membership, &diagnostics);
  (void)acme_membership_database_put(
      database, membership_create, cancellation, &membership, &diagnostics);
  (void)acme_membership_database_get_by_iid(
      database, text, cancellation, &membership, &diagnostics);
  (void)acme_membership_database_update(
      database, text, membership_create, cancellation, &membership,
      &diagnostics);
  (void)acme_membership_database_delete_by_iid(
      database, text, cancellation, &diagnostics);
  (void)acme_membership_database_count(
      database, cancellation, &thing_count, &diagnostics);
  (void)acme_membership_read_transaction_get_by_iid(
      read_tx, text, cancellation, &membership, &diagnostics);
  (void)acme_membership_read_transaction_count(
      read_tx, cancellation, &thing_count, &diagnostics);
  (void)acme_membership_write_transaction_insert(
      write_tx, membership_create, cancellation, &membership, &diagnostics);
  (void)acme_membership_write_transaction_put(
      write_tx, membership_create, cancellation, &membership, &diagnostics);
  (void)acme_membership_write_transaction_get_by_iid(
      write_tx, text, cancellation, &membership, &diagnostics);
  (void)acme_membership_write_transaction_update(
      write_tx, text, membership_create, cancellation, &membership,
      &diagnostics);
  (void)acme_membership_write_transaction_delete_by_iid(
      write_tx, text, cancellation, &diagnostics);
  (void)acme_membership_write_transaction_count(
      write_tx, cancellation, &thing_count, &diagnostics);

  (void)acme_container_item_player_close(&item_player);
  (void)acme_membership_member_player_close(&member_player);
  (void)acme_membership_ref_close(&membership_reference);
  (void)acme_membership_create_close(&membership_create);
  (void)acme_membership_close(&membership);
  (void)acme_personzhrecord_ref_close(&reference);
  (void)acme_personzhrecord_create_close(&create);
  (void)acme_personzhrecord_close(&person);
  (void)type_bridge_diagnostics_close(&package_diagnostics);
  (void)type_bridge_schema_package_close(&opened_package);
  return person != nullptr || display_name != nullptr || text.length != 0u ||
      TYPE_BRIDGE_C_ABI_MAJOR != 1u;
}
"#,
    )
    .expect("C++ consumer is written");

    let runtime_include = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../c/include")
        .canonicalize()
        .expect("C runtime include directory exists");
    let generated_include = stage.path().join("include");
    let mut invocations = 0;
    for compiler in ["g++", "clang++", "c++"] {
        if !command_exists(compiler) {
            continue;
        }
        for short_enums in [false, true] {
            invocations += 1;
            let mut command = Command::new(compiler);
            command.args([
                "-std=c++17",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pedantic-errors",
            ]);
            if short_enums {
                command.arg("-fshort-enums");
            }
            let output = command
                .arg("-fsyntax-only")
                .arg("-I")
                .arg(&runtime_include)
                .arg("-I")
                .arg(&generated_include)
                .arg(&consumer)
                .output()
                .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
            assert!(
                output.status.success(),
                "{compiler} rejected the generated header under C++17 (short-enums={short_enums}):\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
    }

    #[cfg(windows)]
    {
        invocations += 1;
        let output = msvc_cl(
            stage.path(),
            &[
                "/nologo".to_owned(),
                "/TP".to_owned(),
                "/std:c++17".to_owned(),
                "/W4".to_owned(),
                "/WX".to_owned(),
                "/Zs".to_owned(),
                format!("/I{}", runtime_include.display()),
                format!("/I{}", generated_include.display()),
                consumer.display().to_string(),
            ],
        );
        assert!(
            output.status.success(),
            "MSVC cl rejected the all-wrapper generated C++17 consumer:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    assert!(invocations > 0, "no supported C++17 compiler was available");
}

#[test]
fn generated_nominal_entity_and_relation_apis_reject_wrong_types() {
    let emitter = CEmitter::new();
    let resources = emitter.code_resources().expect("C resources hash");
    let (projection, authority) = projected(&resources);
    let package = emitter
        .emit(&projection, &authority)
        .expect("C package emits");
    let stage = TempDirectory::new();
    write_package(&package, stage.path());
    let runtime_include = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../c/include")
        .canonicalize()
        .expect("C runtime include directory exists");
    let generated_include = stage.path().join("include");
    let cases = [
        (
            "wrong-attribute.c",
            r#"#include <acme/models.h>
int main(void) {
  const type_bridge_schema_package_t *package = 0;
  acme_displayzhname *wrong_count = 0;
  acme_personzhrecord_create_args_v1_t args = {0};
  acme_personzhrecord_create *create = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  args.struct_size = sizeof(args);
  args.version = ACME_CREATE_ARGS_VERSION;
  args.field_countzhvalue = wrong_count;
  return acme_personzhrecord_create_open(
      package, &args, &create, &diagnostics);
}

"#,
        ),
        (
            "wrong-field.c",
            r#"#include <acme/models.h>
int main(void) {
  const type_bridge_schema_package_t *package = 0;
  acme_countzhvalue *wrong = 0;
  acme_personzhrecord_ref *reference = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  return acme_personzhrecord_ref_from_key(
      package, wrong, &reference, &diagnostics);
}
"#,
        ),
        (
            "wrong-scalar-accessor.c",
            r#"#include <acme/models.h>
int main(void) {
  const acme_ratiozhvalue *wrong = 0;
  int64_t value = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  return acme_countzhvalue_value(wrong, &value, &diagnostics);
}
"#,
        ),
        (
            "wrong-transaction.c",
            r#"#include <acme/models.h>
int main(void) {
  const type_bridge_read_transaction_t *read_tx = 0;
  const acme_personzhrecord_create *create = 0;
  acme_personzhrecord *person = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  return acme_personzhrecord_write_transaction_insert(
      read_tx, create, 0, &person, &diagnostics);
}
"#,
        ),
        (
            "wrong-create.c",
            r#"#include <acme/models.h>
int main(void) {
  const type_bridge_database_t *database = 0;
  acme_dualzhkeyzhrecord_create *wrong = 0;
  acme_personzhrecord *person = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  return acme_personzhrecord_database_insert(
      database, wrong, 0, &person, &diagnostics);
}
"#,
        ),
        (
            "wrong-model.c",
            r#"#include <acme/models.h>
int main(void) {
  acme_dualzhkeyzhrecord *wrong = 0;
  acme_displayzhname *value = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  return acme_personzhrecord_displayzhname(wrong, &value, &diagnostics);
}
"#,
        ),
        (
            "wrong-role-player-reference.c",
            r#"#include <acme/models.h>
int main(void) {
  const acme_event_ref *wrong = 0;
  acme_membership_member_player *player = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  return acme_membership_member_player_from_personzhrecord(
      wrong, &player, &diagnostics);
}
"#,
        ),
        (
            "wrong-role-union-create.c",
            r#"#include <acme/models.h>
int main(void) {
  const type_bridge_schema_package_t *package = 0;
  const acme_container_item_player *wrong = 0;
  acme_membership_create_args_v1_t args = {0};
  acme_membership_create *create = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  args.struct_size = sizeof(args);
  args.version = ACME_CREATE_ARGS_VERSION;
  args.role_member = wrong;
  return acme_membership_create_open(
      package, &args, &create, &diagnostics);
}
"#,
        ),
        (
            "wrong-relation-transaction.c",
            r#"#include <acme/models.h>
int main(void) {
  const type_bridge_read_transaction_t *read_tx = 0;
  const acme_membership_create *create = 0;
  acme_membership *membership = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  return acme_membership_write_transaction_insert(
      read_tx, create, 0, &membership, &diagnostics);
}
"#,
        ),
        (
            "wrong-relation-create.c",
            r#"#include <acme/models.h>
int main(void) {
  const type_bridge_database_t *database = 0;
  const acme_container_create *wrong = 0;
  acme_membership *membership = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  return acme_membership_database_insert(
      database, wrong, 0, &membership, &diagnostics);
}
"#,
        ),
        (
            "wrong-relation-model.c",
            r#"#include <acme/models.h>
int main(void) {
  const acme_container *wrong = 0;
  acme_membership_member_player *player = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  return acme_membership_member(wrong, &player, &diagnostics);
}
"#,
        ),
        (
            "wrong-query-session.c",
            r#"#include <acme/models.h>
int main(void) {
  const type_bridge_schema_package_t *wrong = 0;
  acme_personzhrecord_query_exact_binding *binding = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  return acme_personzhrecord_query_exact_binding_open(
      wrong, &binding, &diagnostics);
}
"#,
        ),
        (
            "wrong-query-field-owner.c",
            r#"#include <acme/models.h>
int main(void) {
  const acme_dualzhkeyzhrecord_query_exact_binding *wrong = 0;
  acme_personzhrecord_countzhvalue_query_field *field = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  return acme_personzhrecord_countzhvalue_query_field_from_exact(
      wrong, &field, &diagnostics);
}
"#,
        ),
        (
            "wrong-query-field-scalar-domain.c",
            r#"#include <acme/models.h>
int main(void) {
  const acme_personzhrecord_countzhvalue_query_field *integer_field = 0;
  const acme_actor_displayzhname_query_field *string_field = 0;
  acme_query_predicate *predicate = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  return acme_query_integer_field_compare_field(
      acme_personzhrecord_countzhvalue_query_field_integer_field_ref(integer_field),
      TYPE_BRIDGE_QUERY_COMPARE_EQUAL,
      acme_actor_displayzhname_query_field_string_field_ref(string_field),
      &predicate, &diagnostics);
}
"#,
        ),
        (
            "wrong-query-role-owner.c",
            r#"#include <acme/models.h>
int main(void) {
  const acme_personzhrecord_query_exact_binding *wrong = 0;
  acme_membership_member_query_role *role = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  return acme_membership_member_query_role_from_exact(
      wrong, &role, &diagnostics);
}
"#,
        ),
        (
            "wrong-query-role-player.c",
            r#"#include <acme/models.h>
int main(void) {
  const acme_event_query_exact_binding *wrong = 0;
  acme_membership_member_query_role_player_binding_v1_t player =
      acme_membership_member_query_role_player_personzhrecord_exact(wrong);
  (void)player;
  return 0;
}
"#,
        ),
        (
            "wrong-query-reachability-endpoint.c",
            r#"#include <acme/models.h>
int main(void) {
  const acme_query_session *session = 0;
  const acme_event_query_exact_binding *event = 0;
  acme_query_predicate *predicate = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  acme_container_item_query_role_player_binding_v1_t wrong =
      acme_container_item_query_role_player_event_exact(event);
  acme_container_query_reachable_endpoint_v1_t endpoint =
      acme_container_query_reachable_endpoint_v1_t_from_item(wrong);
  return acme_membership_query_reachable(
      session, endpoint, endpoint, 1u, 2u, &predicate, &diagnostics);
}
"#,
        ),
        (
            "wrong-query-selection-shape.c",
            r#"#include <acme/models.h>
int main(void) {
  const acme_query_session *session = 0;
  const acme_personzhrecord_query_exact_one_selection *wrong = 0;
  acme_query *query = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  return acme_query_positional_1(session, wrong, &query, &diagnostics);
}
"#,
        ),
        (
            "wrong-query-root.c",
            r#"#include <acme/models.h>
int main(void) {
  const acme_personzhrecord_query_exact_binding *wrong = 0;
  acme_query_page_terminal *terminal = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  return acme_query_page(
      wrong, 0, 0u, 0u, 1u, 0u, &terminal, &diagnostics);
}
"#,
        ),
        (
            "wrong-query-terminal-kind.c",
            r#"#include <acme/models.h>
int main(void) {
  const type_bridge_database_t *database = 0;
  const acme_query_rows_terminal *wrong = 0;
  acme_query_rows_result *result = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  return acme_database_query_execute_first(
      database, wrong, 0, 0, &result, &diagnostics);
}
"#,
        ),
        (
            "wrong-query-result-family.c",
            r#"#include <acme/models.h>
int main(void) {
  const acme_query_page_result *wrong = 0;
  acme_personzhrecord_query_exact_one_result_slot_v1_t slot =
      acme_personzhrecord_query_exact_one_result_slot_v1_t_rows(wrong, 0u);
  (void)slot;
  return 0;
}
"#,
        ),
        (
            "wrong-query-remote-terminal-family.c",
            r#"#include <acme/models.h>
int main(void) {
  const acme_query_remote_context *context = 0;
  const acme_query_page_terminal *wrong = 0;
  acme_query_rows_remote_pending *pending = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  return acme_query_remote_prepare_rows(
      context, wrong, 0, &pending, &diagnostics);
}
"#,
        ),
        (
            "wrong-query-remote-result-family.c",
            r#"#include <acme/models.h>
int main(void) {
  const acme_query_rows_remote_claim *wrong = 0;
  acme_query_page_result *result = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  type_bridge_byte_view_t response = {0, 0u};
  return acme_query_page_remote_claim_decode(
      wrong, 0, response, &result, &diagnostics);
}
"#,
        ),
        (
            "wrong-query-cardinality-slot.c",
            r#"#include <acme/models.h>
int main(void) {
  const acme_query_rows_result *result = 0;
  acme_personzhrecord *value = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  acme_personzhrecord_query_exact_collect_result_slot_v1_t wrong =
      acme_personzhrecord_query_exact_collect_result_slot_v1_t_rows(result, 0u);
  return acme_personzhrecord_query_exact_one_at(
      wrong, 0u, &value, &diagnostics);
}
"#,
        ),
        (
            "wrong-query-mode-slot.c",
            r#"#include <acme/models.h>
int main(void) {
  const acme_query_rows_result *result = 0;
  acme_personzhrecord *value = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  acme_personzhrecord_query_subtypes_one_result_slot_v1_t wrong =
      acme_personzhrecord_query_subtypes_one_result_slot_v1_t_rows(result, 0u);
  return acme_personzhrecord_query_exact_one_at(
      wrong, 0u, &value, &diagnostics);
}
"#,
        ),
        (
            "wrong-query-reducer-field.c",
            r#"#include <acme/models.h>
int main(void) {
  const acme_actor_displayzhname_query_field *wrong = 0;
  acme_query_reducer_ref_v1_t reducer =
      acme_personzhrecord_countzhvalue_query_field_reduce_sum_long(wrong);
  (void)reducer;
  return 0;
}
"#,
        ),
        (
            "wrong-query-reduction-group-model.c",
            r#"#include <acme/models.h>
int main(void) {
  acme_personzhrecord *value = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  acme_actor_query_exact_reduction_group_slot_v1_t wrong;
  return acme_personzhrecord_query_exact_reduction_group_slot_v1_t_thing_at(
      wrong, 0u, &value, &diagnostics);
}
"#,
        ),
        (
            "wrong-query-reduction-group-field.c",
            r#"#include <acme/models.h>
int main(void) {
  acme_countzhvalue *value = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  acme_actor_displayzhname_query_field_reduction_group_slot_v1_t wrong;
  return acme_personzhrecord_countzhvalue_query_field_reduction_group_slot_v1_t_value(
      wrong, 0u, &value, &diagnostics);
}
"#,
        ),
        (
            "wrong-query-reduction-terminal-family.c",
            r#"#include <acme/models.h>
int main(void) {
  const type_bridge_database_t *database = 0;
  const acme_query_field_reduction_terminal *wrong = 0;
  acme_query_reduction_result *result = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  return acme_database_query_execute_reduction(
      database, wrong, 0, 0, &result, &diagnostics);
}
"#,
        ),
        (
            "wrong-query-reduction-result-family.c",
            r#"#include <acme/models.h>
int main(void) {
  const acme_query_field_reduction_result *wrong = 0;
  acme_query_reduction_result_ref_v1_t result =
      acme_query_reduction_result_ref(wrong);
  (void)result;
  return 0;
}
"#,
        ),
        (
            "wrong-query-function-model-argument.c",
            r#"#include <acme/models.h>
int main(void) {
  const acme_actor_query_exact_binding *wrong = 0;
  acme_person_query_function_argument_v1_t argument =
      acme_person_query_function_argument_v1_t_from_person_exact(wrong);
  (void)argument;
  return 0;
}
"#,
        ),
        (
            "wrong-query-function-scalar-input.c",
            r#"#include <acme/models.h>
int main(void) {
  const acme_query_session *session = 0;
  const acme_displayzhname *wrong = 0;
  acme_query_function_integer_input *input = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  return acme_score_query_function_input_open(
      session, wrong, &input, &diagnostics);
}
"#,
        ),
        (
            "wrong-query-function-nested-call-domain.c",
            r#"#include <acme/models.h>
int main(void) {
  const acme_stringzhidentity_query_call *wrong = 0;
  acme_query_function_integer_argument_v1_t argument =
      acme_query_function_integer_argument_from_call(
          acme_stringzhidentity_query_call_ref(wrong));
  (void)argument;
  return 0;
}
"#,
        ),
        (
            "wrong-query-function-comparison-domain.c",
            r#"#include <acme/models.h>
int main(void) {
  const acme_qualifyingzhscore_query_call *call = 0;
  const acme_actor_displayzhname_query_field *wrong = 0;
  acme_query_predicate *predicate = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  return acme_qualifyingzhscore_query_call_equal_field(
      call, acme_actor_displayzhname_query_field_function_field(wrong),
      &predicate, &diagnostics);
}
"#,
        ),
        (
            "wrong-query-function-identity.c",
            r#"#include <acme/models.h>
int main(void) {
  const acme_query_session *session = 0;
  acme_stringzhidentity *wrong = 0;
  type_bridge_execution_diagnostics_t *diagnostics = 0;
  return acme_qualifyingzhscore_query_open(
      session, &wrong, &diagnostics);
}
"#,
        ),
    ];

    let mut invocations = 0;
    for compiler in ["gcc", "clang"] {
        if !command_exists(compiler) {
            continue;
        }
        for standard in ["c11", "c17"] {
            for (name, source) in cases {
                invocations += 1;
                let consumer = stage.path().join(name);
                fs::write(&consumer, source).expect("negative C consumer is written");
                let output = Command::new(compiler)
                    .args([
                        &format!("-std={standard}"),
                        "-Wall",
                        "-Wextra",
                        "-Werror=incompatible-pointer-types",
                        "-pedantic-errors",
                        "-fsyntax-only",
                    ])
                    .arg("-I")
                    .arg(&runtime_include)
                    .arg("-I")
                    .arg(&generated_include)
                    .arg(&consumer)
                    .output()
                    .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
                assert!(
                    !output.status.success(),
                    "{compiler} accepted nominally wrong generated input {name} under {standard}",
                );
                let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
                assert!(
                    stderr.contains("incompatible"),
                    "{compiler} did not issue an incompatible-type diagnostic for {name} under {standard}:\n{stderr}",
                );
            }
        }
    }
    for compiler in ["g++", "clang++"] {
        if !command_exists(compiler) {
            continue;
        }
        for (name, source) in cases {
            invocations += 1;
            let consumer = stage.path().join(name);
            fs::write(&consumer, source).expect("negative C++ consumer is written");
            let output = Command::new(compiler)
                .args([
                    "-std=c++17",
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-Wno-missing-field-initializers",
                    "-pedantic-errors",
                    "-fsyntax-only",
                    "-x",
                    "c++",
                ])
                .arg("-I")
                .arg(&runtime_include)
                .arg("-I")
                .arg(&generated_include)
                .arg(&consumer)
                .output()
                .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
            assert!(
                !output.status.success(),
                "{compiler} accepted nominally wrong generated input {name} under C++17",
            );
            let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
            assert!(
                stderr.contains("incompatible")
                    || stderr.contains("cannot convert")
                    || stderr.contains("could not convert")
                    || stderr.contains("no known conversion")
                    || stderr.contains("no matching function"),
                "{compiler} did not issue an incompatible-type diagnostic for {name} under C++17:\n{stderr}",
            );
        }
    }
    #[cfg(windows)]
    for (name, source) in cases {
        invocations += 1;
        let consumer = stage.path().join(name);
        fs::write(&consumer, source).expect("negative MSVC consumer is written");
        let output = msvc_cl(
            stage.path(),
            &[
                "/nologo".to_owned(),
                "/TC".to_owned(),
                "/std:c17".to_owned(),
                "/W4".to_owned(),
                "/WX".to_owned(),
                "/Zs".to_owned(),
                format!("/I{}", runtime_include.display()),
                format!("/I{}", generated_include.display()),
                consumer.display().to_string(),
            ],
        );
        assert!(
            !output.status.success(),
            "MSVC cl accepted nominally wrong generated input {name}",
        );
        let diagnostics = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .to_ascii_lowercase();
        assert!(
            diagnostics.contains("c4133")
                || diagnostics.contains("c4024")
                || diagnostics.contains("c2440")
                || diagnostics.contains("c2664")
                || diagnostics.contains("cannot convert")
                || (diagnostics.contains("incompatible") && diagnostics.contains("pointer")),
            "MSVC cl did not issue an incompatible-type diagnostic for {name}:\n{diagnostics}",
        );
    }
    assert!(
        invocations > 0,
        "no supported C compiler was available for nominal negative evidence"
    );
}

#[test]
fn workforce_v3_generated_package_integrity() {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("workforce-v3.yaml").expect("V3 document ID is valid"),
        WORKFORCE_V3_SOURCE,
    )])
    .expect("V3 schema parses");
    let declared = normalize_documents(&documents).expect("V3 schema normalizes");
    let profile = SemanticProfileId::new(support::TEST_PROFILE).expect("test profile is valid");
    let resolved = resolve(&declared, &profile).expect("V3 schema resolves");
    let emitter = CEmitter::new();
    let resources = emitter
        .code_resources_for(&resolved)
        .expect("V3 C resources hash");
    let projection = project(
        &resolved,
        BindingTarget::C,
        &ProjectionConfig::c(CSymbolPrefix::new("fixture").expect("fixture prefix is valid")),
        &emitter.generator_handlers_for(&resolved),
        &resources,
    )
    .expect("V3 schema projects to C");
    let authority =
        support::authority_for_declared(&declared, "workforce-v3-c", support::TEST_PROFILE);
    let package = emitter
        .emit(&projection, &authority)
        .expect("V3 C package emits");
    let emitted = package
        .files()
        .values()
        .fold(String::new(), |mut text, bytes| {
            text.push_str(&String::from_utf8_lossy(bytes));
            text
        });
    assert!(!projection.models().is_empty(), "V3 projection has models");
    assert!(
        emitted.contains("fixture_"),
        "generated C package has the requested namespace"
    );

    let root = workforce_v3_repository_root();
    let journey: Value = serde_json::from_slice(
        &fs::read(root.join("tests/contracts/sdk_conformance/workforce-v3/journey-v3.json"))
            .expect("V3 journey reads"),
    )
    .expect("V3 journey parses");
    let expected = journey["expected_observations"]
        .as_object()
        .expect("V3 expected observations are an object");
    let test_id = "c_emitter::workforce_v3_generated_package_integrity";
    publish_workforce_v3_package_fragment(vec![
        json!({"observation": expected["projected_constraint_validation"], "observation_ref": "projected_constraint_validation", "outcome": "passed", "proof_kind": "diagnostic", "test_id": test_id}),
        json!({"observation": expected["projection_evidence_integrity"], "observation_ref": "projection_evidence_integrity", "outcome": "passed", "proof_kind": "diagnostic", "test_id": test_id}),
        json!({"observation": expected["token_package_fencing"], "observation_ref": "token_package_fencing", "outcome": "passed", "proof_kind": "diagnostic", "test_id": test_id}),
    ]);
}
