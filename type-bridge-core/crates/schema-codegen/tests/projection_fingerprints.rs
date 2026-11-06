use serde_json::json;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig};
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
use type_bridge_schema_codegen::{PythonEmitter, TypeScriptEmitter};

/// Emit evidence through the native projection pipeline, never from pinned digests.
#[test]
#[ignore = "invoked by the projection fingerprint refresh tool"]
fn produce_python_node_projection_fingerprints() {
    let mut evidence = serde_json::Map::new();
    for profile in ["typedb-3.12.1/v1", "typedb-3.13.6/v1"] {
        let mut suites = serde_json::Map::new();
        for (suite, schema) in [
            ("queries", include_str!("acceptance/schema.yaml")),
            (
                "models",
                include_str!("../../../../tests/contracts/sdk_conformance/models/schema.yaml"),
            ),
        ] {
            let documents =
                SchemaDocumentSet::parse([(DocumentId::new("schema.yaml").unwrap(), schema)])
                    .unwrap();
            let declared = normalize_documents(&documents).unwrap();
            let resolved = resolve(&declared, &SemanticProfileId::new(profile).unwrap()).unwrap();
            let python = PythonEmitter::new();
            let node = TypeScriptEmitter::new();
            let python = project(
                &resolved,
                BindingTarget::Python,
                &ProjectionConfig::python(),
                &python.generator_handlers_for(&resolved),
                &python.code_resources_for(&resolved).unwrap(),
            )
            .unwrap();
            let node = project(
                &resolved,
                BindingTarget::TypeScript,
                &ProjectionConfig::typescript(),
                &node.generator_handlers_for(&resolved),
                &node.code_resources_for(&resolved).unwrap(),
            )
            .unwrap();
            suites.insert(suite.to_owned(), json!({"semantic": python.semantic_fingerprint(), "python": python.projection_fingerprint(), "node": node.projection_fingerprint()}));
        }
        evidence.insert(profile.to_owned(), suites.into());
    }
    let destination = std::env::var_os("TYPE_BRIDGE_C_PROJECTION_FINGERPRINT_OUTPUT")
        .expect("refresh tool supplies output path");
    std::fs::write(destination, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
}
