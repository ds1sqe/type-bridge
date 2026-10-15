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
    source: Option<&'static str>,
}

const SETS: [ConformanceSet; 6] = [
    ConformanceSet {
        source: Some(ACCEPTANCE_SOURCE),
    },
    ConformanceSet {
        source: Some(ACCEPTANCE_SOURCE),
    },
    ConformanceSet {
        source: Some(SDK_V3_SOURCE),
    },
    ConformanceSet { source: None },
    ConformanceSet { source: None },
    ConformanceSet { source: None },
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
    for (index, set) in SETS.iter().enumerate() {
        let root = producer_selection::repository_root();
        let paths = producer_selection::contract_paths(&root, (index + 1) as u8);
        let read = |kind: &str| {
            std::fs::read_to_string(root.join(paths[kind].as_str().expect("contract path")))
                .expect("contract readable")
        };
        let catalog: Value = serde_json::from_str(&read("catalog")).expect("catalog parses");
        let matrix: Value = serde_json::from_str(&read("matrix")).expect("matrix parses");
        assert_eq!(matrix["format"], "typebridge.sdk-profile-matrix/v1");
        let suite = paths["suite"].as_str().expect("suite name");
        assert_eq!(matrix["suite"], suite);
        let profiles = matrix["profiles"].as_object().expect("matrix profiles");
        let catalog_profile = catalog["fixture"]["semantic_profile"]
            .as_str()
            .expect("catalog profile");
        assert!(
            profiles.contains_key(catalog_profile),
            "{}: the catalog's own profile must be a matrix entry",
            suite
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
                    suite
                );
            }
        }
        inventory.insert(suite.to_owned(), computed);
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

#[path = "../../../../tests/support/sdk_profile_matrix.rs"]
#[allow(dead_code)]
mod producer_selection;

#[test]
fn producers_select_each_profiles_exact_live_server() {
    let root = producer_selection::repository_root();
    for set in 1..=6 {
        for (profile, server) in [
            ("typedb-3.12.1/v1", "3.12.3"),
            ("typedb-3.13.6/v1", "3.13.6"),
        ] {
            let entry = producer_selection::select_from(&root, set, profile);
            assert_eq!(entry.semantic_profile, profile, "sdk-v{set}");
            assert_eq!(entry.live_server_version, server, "sdk-v{set} {profile}");
            entry.require_server_version(Some(server));
        }
    }
}

#[test]
fn producers_default_to_the_catalog_profile() {
    let root = producer_selection::repository_root();
    for requested in [None, Some("")] {
        let entry = producer_selection::requested_from(&root, 1, requested);
        assert_eq!(entry.semantic_profile, "typedb-3.12.1/v1");
        assert_eq!(entry.live_server_version, "3.12.3");
    }
}

#[test]
#[should_panic(expected = "sdk-v2 has no matrix entry")]
fn producers_reject_an_unknown_profile() {
    let root = producer_selection::repository_root();
    producer_selection::requested_from(&root, 2, Some("typedb-9.9.9/v1"));
}

#[test]
#[should_panic(expected = "actual detected TypeDB server version")]
fn producers_reject_another_live_server() {
    let root = producer_selection::repository_root();
    producer_selection::select_from(&root, 4, "typedb-3.13.6/v1")
        .require_server_version(Some("3.12.3"));
}

#[test]
fn sdk_v2_fingerprints_name_the_selected_profile() {
    let root = producer_selection::repository_root();
    let entry = producer_selection::requested_from(&root, 2, Some("typedb-3.13.6/v1"));
    let fingerprints = &entry.expected_fingerprints;
    assert_eq!(
        fingerprints["semantic"]["semantic_profile"],
        "typedb-3.13.6/v1"
    );
    let projections = fingerprints["projections"]
        .as_object()
        .expect("sdk-v2 pins per-binding projections");
    assert_eq!(projections.len(), 4);
    for (binding, projection) in projections {
        assert_eq!(
            projection["semantic_profile"], "typedb-3.13.6/v1",
            "{binding}"
        );
    }
}
