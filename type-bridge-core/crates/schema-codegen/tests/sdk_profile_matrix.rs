//! Reproduces the per-profile fingerprints in each SDK conformance profile matrix.
//!
//! Every `sdk-vN/profiles-vN.json` lists the identity a conforming report
//! carries on each supported TypeDB profile. The catalog's own top-level values
//! are the entry for the catalog's profile; the other entries are derived here
//! from the same fixture source projected under that profile.

use std::collections::BTreeMap;
use std::env;
use std::fs::OpenOptions;
use std::io::Write;

use serde_json::{Value, json};
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::projection::{
    BindingTarget, CSymbolPrefix, ProjectionConfig, RuntimeProjection,
};
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
use type_bridge_schema_codegen::{CEmitter, PythonEmitter, RustEmitter, TypeScriptEmitter};

const OUTPUT_ENV: &str = "TYPE_BRIDGE_SDK_PROFILE_MATRIX_OUTPUT";
const ACCEPTANCE_SOURCE: &str = include_str!("acceptance/schema.yaml");
const SDK_V3_SOURCE: &str =
    include_str!("../../../../tests/contracts/sdk_conformance/sdk-v3/schema-v3.yaml");

/// One conformance set: its catalog, its matrix, and the fixture source whose
/// projections the catalog pins (`None` when the set pins no fingerprints).
struct ConformanceSet {
    id: &'static str,
    catalog: &'static str,
    matrix: &'static str,
    source: Option<&'static str>,
}

const SETS: [ConformanceSet; 6] = [
    ConformanceSet {
        id: "sdk-v1",
        catalog: include_str!("../../../../tests/contracts/sdk_conformance/sdk-v1/catalog-v1.json"),
        matrix: include_str!("../../../../tests/contracts/sdk_conformance/sdk-v1/profiles-v1.json"),
        source: Some(ACCEPTANCE_SOURCE),
    },
    ConformanceSet {
        id: "sdk-v2",
        catalog: include_str!("../../../../tests/contracts/sdk_conformance/sdk-v2/catalog-v2.json"),
        matrix: include_str!("../../../../tests/contracts/sdk_conformance/sdk-v2/profiles-v2.json"),
        source: Some(ACCEPTANCE_SOURCE),
    },
    ConformanceSet {
        id: "sdk-v3",
        catalog: include_str!("../../../../tests/contracts/sdk_conformance/sdk-v3/catalog-v3.json"),
        matrix: include_str!("../../../../tests/contracts/sdk_conformance/sdk-v3/profiles-v3.json"),
        source: Some(SDK_V3_SOURCE),
    },
    ConformanceSet {
        id: "sdk-v4",
        catalog: include_str!("../../../../tests/contracts/sdk_conformance/sdk-v4/catalog-v4.json"),
        matrix: include_str!("../../../../tests/contracts/sdk_conformance/sdk-v4/profiles-v4.json"),
        source: None,
    },
    ConformanceSet {
        id: "sdk-v5",
        catalog: include_str!("../../../../tests/contracts/sdk_conformance/sdk-v5/catalog-v5.json"),
        matrix: include_str!("../../../../tests/contracts/sdk_conformance/sdk-v5/profiles-v5.json"),
        source: None,
    },
    ConformanceSet {
        id: "sdk-v6",
        catalog: include_str!("../../../../tests/contracts/sdk_conformance/sdk-v6/catalog-v6.json"),
        matrix: include_str!("../../../../tests/contracts/sdk_conformance/sdk-v6/profiles-v6.json"),
        source: None,
    },
];

fn projections(source: &str, profile: &str) -> BTreeMap<&'static str, RuntimeProjection> {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("fixture.yaml").expect("fixture document ID"),
        source,
    )])
    .expect("fixture schema parses");
    let declared = normalize_documents(&documents).expect("fixture schema normalizes");
    let profile = SemanticProfileId::new(profile).expect("matrix profile is valid");
    let resolved = resolve(&declared, &profile).expect("fixture schema resolves");

    let python = PythonEmitter::new();
    let typescript = TypeScriptEmitter::new();
    let rust = RustEmitter::new();
    let c = CEmitter::new();
    let c_config =
        ProjectionConfig::c(CSymbolPrefix::new("fixture").expect("live C prefix is valid"));
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
            .expect("fixture projects to Python"),
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
            .expect("fixture projects to TypeScript"),
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
            .expect("fixture projects to Rust"),
        ),
        (
            "c",
            project(
                &resolved,
                BindingTarget::C,
                &c_config,
                &c.generator_handlers_for(&resolved),
                &c.code_resources_for(&resolved).expect("C resources hash"),
            )
            .expect("fixture projects to C"),
        ),
    ])
}

/// The fingerprints a catalog with `bindings` pins for `source` under `profile`.
fn expected_fingerprints(source: &str, profile: &str, bindings: &[Value]) -> Value {
    let projections = projections(source, profile);
    let semantic = json!(projections["rust"].semantic_fingerprint());
    let pinned = bindings
        .iter()
        .map(|binding| {
            let binding = binding.as_str().expect("report binding is a string");
            (
                binding.to_owned(),
                json!(projections[binding].projection_fingerprint()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    json!({"semantic": semantic, "projections": pinned})
}

#[test]
fn every_profile_matrix_entry_reproduces_from_its_fixture_source() {
    let mut inventory = BTreeMap::new();
    for set in &SETS {
        let catalog: Value = serde_json::from_str(set.catalog).expect("catalog parses");
        let matrix: Value = serde_json::from_str(set.matrix).expect("matrix parses");
        assert_eq!(matrix["format"], "typebridge.sdk-profile-matrix/v1");
        assert_eq!(matrix["set"], set.id);
        let profiles = matrix["profiles"].as_object().expect("matrix profiles");
        let catalog_profile = catalog["fixture"]["semantic_profile"]
            .as_str()
            .expect("catalog profile");
        assert!(
            profiles.contains_key(catalog_profile),
            "{}: the catalog's own profile must be a matrix entry",
            set.id
        );
        let mut computed = BTreeMap::new();
        for (profile, entry) in profiles {
            let Some(source) = set.source else {
                assert!(entry.get("expected_fingerprints").is_none());
                continue;
            };
            let bindings = catalog["report_bindings"]
                .as_array()
                .or_else(|| catalog["bindings"].as_array())
                .expect("catalog lists its bindings");
            let expected = expected_fingerprints(source, profile, bindings);
            computed.insert(profile.clone(), expected.clone());
            if env::var_os(OUTPUT_ENV).is_none() {
                assert_eq!(
                    entry["expected_fingerprints"], expected,
                    "{} {profile} fingerprints are stale",
                    set.id
                );
            }
        }
        inventory.insert(set.id, computed);
    }
    if let Some(output) = env::var_os(OUTPUT_ENV) {
        let mut destination = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .unwrap_or_else(|error| panic!("{OUTPUT_ENV} destination is not new: {error}"));
        destination
            .write_all(&to_canonical_json(&json!(inventory)).expect("inventory encodes"))
            .expect("inventory writes");
    }
}
