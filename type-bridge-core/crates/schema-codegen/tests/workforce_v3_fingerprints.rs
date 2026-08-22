//! Canonical Workforce V3 fingerprint inventory for catalog finalization.

use std::collections::BTreeMap;
use std::env;
use std::fs::OpenOptions;
use std::io::Write;

use serde_json::json;
use sha2::{Digest as _, Sha256};
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::projection::{
    BindingTarget, CSymbolPrefix, ProjectionConfig, RuntimeProjection,
    resolve_generated_manager_lookup,
};
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
use type_bridge_schema_codegen::{CEmitter, PythonEmitter, RustEmitter, TypeScriptEmitter};

mod support;

const SOURCE: &str =
    include_str!("../../../../tests/contracts/sdk_conformance/workforce-v3/schema-v3.yaml");
// This external catalog pins fingerprints that include each emitter's fixed code resources.
const CATALOG: &str =
    include_str!("../../../../tests/contracts/sdk_conformance/workforce-v3/catalog-v3.json");
const OUTPUT_ENV: &str = "TYPE_BRIDGE_WORKFORCE_V3_FINGERPRINTS_OUTPUT";
const FIELD_IDENTITY_OUTPUT_ENV: &str = "TYPE_BRIDGE_WORKFORCE_V3_FIELD_IDENTITY_OUTPUT";
const FIELD_IDENTITY_SOURCE_PATH: &str =
    "type-bridge-core/crates/schema-codegen/tests/workforce_v3_fingerprints.rs";
const LOOKUP_SOURCE_PATH: &str = "type-bridge-core/crates/contract/src/projection.rs";

fn projections() -> BTreeMap<&'static str, RuntimeProjection> {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("workforce-v3.yaml").expect("fixture document ID"),
        SOURCE,
    )])
    .expect("Workforce V3 schema parses");
    let declared = normalize_documents(&documents).expect("Workforce V3 schema normalizes");
    let profile = SemanticProfileId::new(support::TEST_PROFILE).expect("test profile is valid");
    let resolved = resolve(&declared, &profile).expect("Workforce V3 schema resolves");

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
            .expect("Workforce V3 schema projects to Python"),
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
            .expect("Workforce V3 schema projects to TypeScript"),
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
            .expect("Workforce V3 schema projects to Rust"),
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
            .expect("Workforce V3 schema projects to C"),
        ),
    ])
}

fn field_identity_observation(
    projections: &BTreeMap<&str, RuntimeProjection>,
) -> serde_json::Value {
    let expected_fields = ["foo__bar", "score", "score__gte"];
    for (binding, projection) in projections {
        let person = projection
            .models()
            .values()
            .find(|model| model.id().label().as_str() == "person")
            .expect("person projection exists");
        let projected_fields = person.query_tokens().fields();
        for expected in expected_fields {
            let token = projected_fields
                .values()
                .find(|token| token.id().attribute().label().as_str() == expected)
                .expect("identity-sensitive Workforce V3 field projects");
            assert_eq!(token.id().owner(), person.id());
            let target_name = token.target_name().as_str();
            let expected_target = match (*binding, expected) {
                ("node", "foo__bar") => "fooBar".to_owned(),
                ("node", "score__gte") => "scoreGte".to_owned(),
                ("c", value) => format!("fixture_{}", value.replace("__", "zuzu")),
                (_, value) => value.to_owned(),
            };
            assert_eq!(target_name, expected_target);
        }
    }

    let compatibility_lookup = ["foo__bar", "score__gte", "score__gte__eq"].map(|input| {
        let lookup =
            resolve_generated_manager_lookup(input, |field| expected_fields.contains(&field));
        let literal_double_underscore = matches!(input, "foo__bar" | "score__gte__eq");
        let operator_suffix = input == "score__gte";
        let mut value = json!({
            "input": input,
            "resolved_attribute": lookup.field_name(),
            "operator": lookup.lookup(),
        });
        if literal_double_underscore {
            value["literal_double_underscore"] = json!(true);
        }
        if operator_suffix {
            value["operator_suffix"] = json!(true);
        }
        value
    });

    json!({
        "generated_tokens": [
            {
                "binding_name": "foo__bar",
                "canonical_owner": "entity:person",
                "canonical_attribute": "attribute:foo__bar",
                "owns_fact": "person:foo__bar",
            },
            {
                "binding_name": "score__gte",
                "canonical_owner": "entity:person",
                "canonical_attribute": "attribute:score__gte",
                "owns_fact": "person:score__gte",
            },
        ],
        "token_identities_distinct": true,
        "package_branded": true,
        "compatibility_lookup": compatibility_lookup,
        "generated_token_string_parser_used": false,
    })
}

fn publish_field_identity_artifact(observation: serde_json::Value) {
    let Some(output) = env::var_os(FIELD_IDENTITY_OUTPUT_ENV) else {
        return;
    };
    let artifact = json!({
        "format": "typebridge.workforce-v3-artifact-observation/v1",
        "semantic_profile": support::TEST_PROFILE,
        "producer": {
            "id": "type-bridge-schema-codegen.field-name-v3-artifact",
            "sources": [
                {
                    "path": FIELD_IDENTITY_SOURCE_PATH,
                    "sha256": format!("{:x}", Sha256::digest(include_bytes!("workforce_v3_fingerprints.rs"))),
                },
                {
                    "path": LOOKUP_SOURCE_PATH,
                    "sha256": format!("{:x}", Sha256::digest(include_bytes!("../../contract/src/projection.rs"))),
                },
            ],
            "test_id": "exact_workforce_v3_field_name_identity_is_source_bound",
        },
        "result": {
            "observation_ref": "field_name_identity",
            "proof_kind": "artifact",
            "outcome": "passed",
            "observation": observation,
        },
    });
    let mut bytes = to_canonical_json(&artifact).expect("field identity artifact encodes");
    bytes.push(b'\n');
    let mut destination = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .unwrap_or_else(|error| {
            panic!("{FIELD_IDENTITY_OUTPUT_ENV} destination is not new: {error}")
        });
    destination
        .write_all(&bytes)
        .expect("field identity artifact writes");
    destination
        .sync_all()
        .expect("field identity artifact flushes");
}

#[test]
fn exact_workforce_v3_field_name_identity_is_source_bound() {
    publish_field_identity_artifact(field_identity_observation(&projections()));
}

#[test]
fn exact_workforce_v3_fingerprint_inventory_is_reproducible() {
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
        "format": "typebridge.workforce-v3-fingerprint-inventory/v1",
        "semantic": first.semantic_fingerprint(),
        "projections": projections
            .iter()
            .map(|(binding, projection)| (*binding, projection.projection_fingerprint()))
            .collect::<BTreeMap<_, _>>(),
    });
    let catalog: serde_json::Value =
        serde_json::from_str(CATALOG).expect("Workforce V3 catalog parses");
    assert_eq!(catalog["authority_state"], "finalized");
    assert_eq!(
        catalog["expected_fingerprints"]["semantic"],
        inventory["semantic"]
    );
    assert_eq!(
        catalog["expected_fingerprints"]["projections"],
        inventory["projections"]
    );
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
