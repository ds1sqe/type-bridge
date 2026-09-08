use serde_json::{Value, json};
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::schema::{DocumentId, SchemaFact};
use type_bridge_core_lib::_bindgen::{BindgenOptions, TargetLanguage};
use type_bridge_schema_compat::{
    GENERATED_DECLARED_DESCRIPTOR_PATH, generate_package_with_declared_descriptors,
    generated_descriptors_to_declared, typeql_to_declared, typeql_to_generated_descriptors,
};

fn source() -> Value {
    json!({
        "provenance": "direct",
        "byte_start": 0,
        "byte_end": 1,
        "line": 1,
        "column": 1,
        "end_line": 1,
        "end_column": 2
    })
}

fn card(min: u64, max: u64) -> Value {
    json!({
        "kind": "cardinality",
        "min": min.to_string(),
        "max": max.to_string()
    })
}

fn overlap_descriptor() -> Value {
    json!({
        "format": "typebridge.generated-descriptors/v1",
        "snapshot_kind": "declared",
        "closed_world": true,
        "unsupported_constructs": [],
        "attributes": [
            { "label": "identifier", "value_type": "string", "source": source() },
            { "label": "age", "value_type": "long", "source": source() }
        ],
        "entities": [
            {
                "label": "party",
                "is_abstract": true,
                "owns": [
                    { "attribute": "identifier", "key": true, "source": source() }
                ],
                "source": source()
            },
            {
                "label": "person",
                "parent": "party",
                "owns": [
                    { "attribute": "age", "source": source() }
                ],
                "source": source()
            }
        ],
        "relations": [
            {
                "label": "membership",
                "relates": [
                    { "role": "member", "card": card(1, 2), "source": source() }
                ],
                "source": source()
            },
            {
                "label": "audit",
                "relates": [
                    { "role": "record", "source": source() }
                ],
                "source": source()
            }
        ],
        "plays": [
            {
                "player": "person",
                "relation": "membership",
                "role": "member",
                "card": card(0, 1),
                "source": source()
            },
            {
                "player": "audit",
                "relation": "membership",
                "role": "member",
                "card": card(0, 2),
                "source": source()
            }
        ]
    })
}

fn adapt(value: &Value) -> Result<type_bridge_contract::schema::DeclaredSchema, String> {
    let bytes = to_canonical_json(value).map_err(|error| error.to_string())?;
    generated_descriptors_to_declared(
        DocumentId::new("generated/descriptors.json").unwrap(),
        &bytes,
    )
    .map_err(|error| error.to_string())
}

#[test]
fn generated_direct_descriptors_match_typeql_declared_identity() {
    let generated = adapt(&overlap_descriptor()).expect("generated descriptors adapt");
    let typeql = typeql_to_declared(
        DocumentId::new("schema/main.tql").unwrap(),
        r#"define
attribute identifier, value string;
attribute age, value integer;
entity party @abstract, owns identifier @key;
entity person sub party, owns age, plays membership:member @card(0..1);
relation membership, relates member @card(1..2);
relation audit, relates record, plays membership:member @card(0..2);
"#,
    )
    .expect("overlap TypeQL adapts");

    assert_eq!(
        generated.declared_identity_fingerprint(),
        typeql.declared_identity_fingerprint(),
    );
    assert_eq!(
        generated
            .facts()
            .filter(|fact| matches!(fact, SchemaFact::Plays(_)))
            .count(),
        2,
    );
}

#[test]
fn effective_partial_and_unsupported_snapshots_fail_closed() {
    for (field, value, code) in [
        (
            "snapshot_kind",
            json!("effective"),
            "generated_descriptor_snapshot_not_declared",
        ),
        (
            "closed_world",
            json!(false),
            "generated_descriptor_snapshot_incomplete",
        ),
        (
            "unsupported_constructs",
            json!(["ordered_owns"]),
            "unsupported_generated_descriptor_construct",
        ),
    ] {
        let mut descriptor = overlap_descriptor();
        descriptor[field] = value;
        let bytes = to_canonical_json(&descriptor).unwrap();
        let diagnostics = generated_descriptors_to_declared(
            DocumentId::new("generated/descriptors.json").unwrap(),
            &bytes,
        )
        .expect_err("dishonest snapshot must fail");
        assert_eq!(
            diagnostics
                .iter()
                .next()
                .unwrap()
                .diagnostic()
                .code()
                .as_str(),
            code,
        );
    }
}

#[test]
fn non_direct_member_provenance_fails_before_fact_construction() {
    let mut descriptor = overlap_descriptor();
    descriptor["entities"][1]["owns"][0]["source"]["provenance"] = json!("effective");
    let bytes = to_canonical_json(&descriptor).unwrap();
    let diagnostics = generated_descriptors_to_declared(
        DocumentId::new("generated/descriptors.json").unwrap(),
        &bytes,
    )
    .expect_err("effective member provenance must fail");
    assert_eq!(
        diagnostics
            .iter()
            .next()
            .unwrap()
            .diagnostic()
            .code()
            .as_str(),
        "generated_descriptor_provenance_not_direct",
    );
}

#[test]
fn missing_explicit_playing_changes_identity_instead_of_being_inferred() {
    let complete = adapt(&overlap_descriptor()).unwrap();
    let mut incomplete = overlap_descriptor();
    incomplete["plays"].as_array_mut().unwrap().pop();
    let incomplete = adapt(&incomplete).unwrap();

    assert_ne!(
        complete.declared_identity_fingerprint(),
        incomplete.declared_identity_fingerprint(),
    );
}

fn generated_overlap_schema() -> &'static str {
    r#"define
attribute identifier, value string;
attribute age, value integer;
attribute standalone-tag, value string;
entity party @abstract, owns identifier @key;
entity person sub party,
    owns age,
    plays membership:member @card(0..1) @doc("person playing") @meta("owner", "person");
relation membership, relates member @card(1..2);
relation curated-membership sub membership, relates participant as member;
relation audit,
    relates record,
    plays membership:member @card(0..2) @doc("audit playing") @meta("owner", "audit");
"#
}

#[test]
fn generation_time_snapshot_and_native_package_share_declared_identity() {
    let source_document = DocumentId::new("schema/generated-overlap.tql").unwrap();
    let expected = typeql_to_declared(source_document.clone(), generated_overlap_schema())
        .expect("shared overlap TypeQL adapts");
    let snapshot = typeql_to_generated_descriptors(source_document, generated_overlap_schema())
        .expect("generation-time snapshot emits");
    let adapted = generated_descriptors_to_declared(
        DocumentId::new("generated/declared-schema.json").unwrap(),
        snapshot.as_bytes(),
    )
    .expect("emitted direct snapshot adapts");
    assert_eq!(
        adapted.declared_identity_fingerprint(),
        expected.declared_identity_fingerprint(),
    );
    assert!(snapshot.contains("standalone-tag"));
    assert!(snapshot.contains("curated-membership"));
    assert!(snapshot.contains("person playing"));
    assert!(snapshot.contains("audit playing"));

    let package = generate_package_with_declared_descriptors(
        generated_overlap_schema(),
        TargetLanguage::Python,
        &BindgenOptions::default(),
    )
    .expect("native package seam renders");
    let generated_snapshot = package
        .file(GENERATED_DECLARED_DESCRIPTOR_PATH)
        .expect("package exports the direct snapshot");
    let registry = package.file("registry.py").expect("Python registry exists");
    assert_eq!(
        generated_snapshot.contents,
        snapshot.replace("schema/generated-overlap.tql", "generated/schema.tql",)
    );
    assert!(
        registry
            .contents
            .contains("GENERATED_DECLARED_DESCRIPTORS_JSON")
    );
}
