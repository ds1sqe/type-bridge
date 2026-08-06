use std::collections::BTreeSet;

use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::projection::{BindingTarget, CodeResourceDigest, ProjectionConfig};
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
use type_bridge_schema_codegen::TypeScriptEmitter;

fn projected(
    source: &str,
    resources: &[CodeResourceDigest],
) -> type_bridge_contract::projection::RuntimeProjection {
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("typescript-emitter.yaml").unwrap(), source)])
            .unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    project(
        &resolved,
        BindingTarget::TypeScript,
        &ProjectionConfig::typescript(),
        &TypeScriptEmitter::new().generator_handlers(),
        resources,
    )
    .unwrap()
}

#[test]
fn emits_exact_deterministic_es_module_package() {
    let emitter = TypeScriptEmitter::new();
    let resources = emitter.code_resources().unwrap();
    let projection = projected(include_str!("acceptance/schema.yaml"), &resources);
    let first = emitter.emit(&projection).unwrap();
    let second = emitter.emit(&projection).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        first
            .files()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "package.json",
            "src/functions.ts",
            "src/index.ts",
            "src/models.ts",
            "src/runtime.ts",
            "src/schema.ts",
            "src/structs.ts",
            "tsconfig.json",
        ])
    );
    let models = String::from_utf8(first.get("src/models.ts").unwrap().to_vec()).unwrap();
    assert!(
        models.find("export let Person:").unwrap() < models.find("Person = defineModel").unwrap()
    );
    assert!(models.contains("Link one dependency component"));
    assert!(models.contains("readonly value: string;"));
    assert!(models.contains("export type IdentifierCreate = string;"));
    assert!(models.contains("export type ScoreCreate = bigint;"));
    assert!(models.contains("valueType: \"long\","));
    let runtime = String::from_utf8(first.get("src/runtime.ts").unwrap().to_vec()).unwrap();
    assert!(runtime.contains("readonly iid: string | null;"));
    assert!(runtime.contains("HYDRATE_COMPLETE_BRAND"));
    assert!(runtime.contains("ProjectedModelManager"));
    let index = String::from_utf8(first.get("src/index.ts").unwrap().to_vec()).unwrap();
    assert!(index.contains("__installRuntimeProjectionPackage"));
    assert!(index.contains("RUNTIME_PROJECTION_JSON"));
    let schema = String::from_utf8(first.get("src/schema.ts").unwrap().to_vec()).unwrap();
    assert!(schema.contains("export const playsPersonMembershipMember"));
    assert!(schema.contains("export const playsRobotMembershipMember"));
    assert_ne!(
        schema.find("export const playsPersonMembershipMember"),
        schema.find("export const playsRobotMembershipMember")
    );
    let package_json = String::from_utf8(first.get("package.json").unwrap().to_vec()).unwrap();
    assert!(package_json.contains("\"type\": \"module\""));
    assert!(package_json.contains("\"@type-bridge/node\": \"^2.1.0\""));
    assert!(
        String::from_utf8(first.get("tsconfig.json").unwrap().to_vec())
            .unwrap()
            .contains("\"module\": \"NodeNext\"")
    );
}

#[test]
fn emits_safely_escaped_type_and_direct_sub_documentation() {
    let emitter = TypeScriptEmitter::new();
    let resources = emitter.code_resources().unwrap();
    let projection = projected(
        r#"format: typebridge.schema/v2
entities:
  actor: {}
  person:
    doc: |-
      Type "doc".
      closing */ kept
    sub:
      type: actor
      doc: |-
        Edge 'doc' \ path
        closes */ safely
"#,
        &resources,
    );
    let package = emitter.emit(&projection).unwrap();
    let models = std::str::from_utf8(package.get("src/models.ts").unwrap()).unwrap();
    let documentation = "/**\n * Type \"doc\".\n * closing *\\/ kept\n * \n * Direct subtype of `actor`:\n * Edge 'doc' \\ path\n * closes *\\/ safely\n */\n";

    assert!(models.contains(&format!(
        "{documentation}export interface Person extends CompleteFacet<"
    )));
    assert!(models.contains(&format!(
        "{documentation}export interface PersonRef extends ReferenceFacet<"
    )));
    assert!(!models.contains("closing */ kept\n *"));
}

#[test]
fn rejects_projection_without_exact_resource_evidence() {
    let projection = projected(include_str!("acceptance/schema.yaml"), &[]);
    let error = TypeScriptEmitter::new().emit(&projection).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("typescript_emitter_evidence_mismatch")
    );
}

#[test]
fn rejects_schema_name_colliding_with_runtime_export() {
    let emitter = TypeScriptEmitter::new();
    let resources = emitter.code_resources().unwrap();
    let projection = projected(
        "format: typebridge.schema/v2\nentities:\n  cardinality: {}\n",
        &resources,
    );
    let error = emitter.emit(&projection).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("typescript_emitter_name_collision")
    );
    assert!(error.to_string().contains("Cardinality"));
}
