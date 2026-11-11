//! Canonical sdk fingerprint inventory for catalog regeneration.

use std::collections::BTreeMap;
use std::env;
use std::fs::OpenOptions;
use std::io::Write;

use serde_json::json;
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::projection::{
    BindingTarget, CSymbolPrefix, ProjectionConfig, RuntimeProjection,
};
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
use type_bridge_schema_codegen::{CEmitter, PythonEmitter, RustEmitter, TypeScriptEmitter};

mod support;

const SOURCE: &str = include_str!("acceptance/schema.yaml");
const OUTPUT_ENV: &str = "TYPE_BRIDGE_SDK_FINGERPRINTS_OUTPUT";

fn projections() -> BTreeMap<&'static str, RuntimeProjection> {
    projections_for(SOURCE, support::TEST_PROFILE)
}

fn projections_for(
    source: &str,
    semantic_profile: &str,
) -> BTreeMap<&'static str, RuntimeProjection> {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("sdk-schema.yaml").expect("fixture document ID"),
        source,
    )])
    .expect("sdk schema parses");
    let declared = normalize_documents(&documents).expect("sdk schema normalizes");
    let profile = SemanticProfileId::new(semantic_profile).expect("test profile is valid");
    let resolved = resolve(&declared, &profile).expect("sdk schema resolves");

    let python = PythonEmitter::new();
    let typescript = TypeScriptEmitter::new();
    let rust = RustEmitter::new();
    let c = CEmitter::new();
    BTreeMap::from([
        (
            "python",
            project(
                &resolved,
                BindingTarget::Python,
                &ProjectionConfig::python(),
                &python.generator_handlers_for(&resolved),
                &python
                    .code_resources_for(&resolved)
                    .expect("Python resources hash"),
            )
            .expect("sdk schema projects to Python"),
        ),
        (
            "node",
            project(
                &resolved,
                BindingTarget::TypeScript,
                &ProjectionConfig::typescript(),
                &typescript.generator_handlers_for(&resolved),
                &typescript
                    .code_resources_for(&resolved)
                    .expect("TypeScript resources hash"),
            )
            .expect("sdk schema projects to TypeScript"),
        ),
        (
            "rust",
            project(
                &resolved,
                BindingTarget::Rust,
                &ProjectionConfig::rust(),
                &rust.generator_handlers_for(&resolved),
                &rust
                    .code_resources_for(&resolved)
                    .expect("Rust resources hash"),
            )
            .expect("sdk schema projects to Rust"),
        ),
        (
            "c",
            project(
                &resolved,
                BindingTarget::C,
                &ProjectionConfig::c(
                    CSymbolPrefix::new("fixture").expect("live C prefix is valid"),
                ),
                &c.generator_handlers_for(&resolved),
                &c.code_resources_for(&resolved).expect("C resources hash"),
            )
            .expect("sdk schema projects to C"),
        ),
    ])
}

#[test]
fn exact_sdk_fingerprint_inventory_is_reproducible() {
    let projections = projections();
    let first = projections.values().next().expect("four projections exist");
    assert!(
        projections
            .values()
            .all(|projection| projection.semantic_fingerprint() == first.semantic_fingerprint()),
        "all binding projections must retain one semantic fingerprint",
    );
    let projection_digests = projections
        .values()
        .map(|projection| {
            projection
                .projection_fingerprint()
                .as_fingerprint()
                .digest()
                .to_hex()
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(projection_digests.len(), projections.len());

    let inventory = json!({
        "format": "typebridge.sdk-fingerprint-inventory/v1",
        "semantic": first.semantic_fingerprint(),
        "projections": projections
            .iter()
            .map(|(binding, projection)| (*binding, projection.projection_fingerprint()))
            .collect::<BTreeMap<_, _>>(),
    });
    let bytes = to_canonical_json(&inventory).expect("fingerprint inventory encodes");
    if let Some(output) = env::var_os(OUTPUT_ENV) {
        let mut destination = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .unwrap_or_else(|error| panic!("{OUTPUT_ENV} destination is not new: {error}"));
        destination
            .write_all(&bytes)
            .expect("fingerprint inventory writes atomically to a new file");
        destination
            .sync_all()
            .expect("fingerprint inventory flushes");
    }
}

#[test]
fn sdk_plan_parity_profiles_use_current_emitter_resources() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut measured = serde_json::Map::new();
    let mut expected = serde_json::Map::new();
    for profile in ["typedb-3.12.1/v1", "typedb-3.13.6/v1"] {
        let mut suites = serde_json::Map::new();
        let mut pinned = serde_json::Map::new();
        for (suite, source) in [
            ("queries", SOURCE),
            (
                "models",
                include_str!("../../../../tests/contracts/sdk_conformance/models/schema.yaml"),
            ),
        ] {
            let projections = projections_for(source, profile);
            suites.insert(suite.to_owned(), json!({
                "semantic": projections["rust"].semantic_fingerprint(),
                "projections": projections.iter().map(|(binding, projection)|
                    (*binding, projection.projection_fingerprint())).collect::<BTreeMap<_, _>>(),
            }));
            let matrix: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join(format!(
                    "tests/contracts/sdk_conformance/{suite}/profiles.json"
                )))
                .unwrap(),
            )
            .unwrap();
            pinned.insert(
                suite.to_owned(),
                matrix["profiles"][profile]["expected_fingerprints"].clone(),
            );
        }
        measured.insert(profile.to_owned(), json!(suites));
        expected.insert(profile.to_owned(), json!(pinned));
    }
    if let Some(path) = env::var_os("TYPE_BRIDGE_PLAN_PARITY_FINGERPRINTS_OUTPUT") {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .unwrap();
        output
            .write_all(&to_canonical_json(&measured).unwrap())
            .unwrap();
    }
    assert_eq!(
        measured, expected,
        "active SDK fingerprints must equal their measured profile inventory"
    );
}
