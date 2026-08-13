//! Canonical workforce fingerprint inventory for catalog regeneration.

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
const OUTPUT_ENV: &str = "TYPE_BRIDGE_WORKFORCE_FINGERPRINTS_OUTPUT";

fn projections() -> BTreeMap<&'static str, RuntimeProjection> {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("workforce-schema.yaml").expect("fixture document ID"),
        SOURCE,
    )])
    .expect("workforce schema parses");
    let declared = normalize_documents(&documents).expect("workforce schema normalizes");
    let profile = SemanticProfileId::new(support::TEST_PROFILE).expect("test profile is valid");
    let resolved = resolve(&declared, &profile).expect("workforce schema resolves");

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
            .expect("workforce schema projects to Python"),
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
            .expect("workforce schema projects to TypeScript"),
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
            .expect("workforce schema projects to Rust"),
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
            .expect("workforce schema projects to C"),
        ),
    ])
}

#[test]
fn exact_workforce_fingerprint_inventory_is_reproducible() {
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
        "format": "typebridge.workforce-fingerprint-inventory/v1",
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
