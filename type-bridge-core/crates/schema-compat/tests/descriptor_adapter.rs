use serde_json::{Value, json};
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::schema::{AnnotationKindId, CollectionMode, DocumentId, SchemaFact};
use type_bridge_core_lib::_bindgen::{BindgenOptions, TargetLanguage};
use type_bridge_schema_compat::{
    GENERATED_DECLARED_DESCRIPTOR_PATH, GENERATED_DECLARED_DESCRIPTOR_V1,
    GENERATED_DECLARED_DESCRIPTOR_V2, generate_package_with_declared_descriptors,
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

#[test]
fn legacy_unordered_descriptor_bytes_remain_exact_v1() {
    let snapshot = typeql_to_generated_descriptors(
        DocumentId::new("legacy.tql").unwrap(),
        "define attribute name, value string; entity person, owns name;",
    )
    .expect("legacy unordered descriptor emits");
    assert_eq!(
        snapshot,
        r#"{"attributes":[{"doc":null,"is_abstract":false,"is_independent":false,"label":"name","meta":{},"parent":null,"range":null,"regex":null,"source":{"byte_end":35,"byte_start":7,"column":8,"document":"legacy.tql","end_column":36,"end_line":1,"line":1,"provenance":"direct"},"value_type":"string","values":null}],"closed_world":true,"entities":[{"doc":null,"is_abstract":false,"label":"person","meta":{},"owns":[{"attribute":"name","card":null,"doc":null,"key":false,"meta":{},"source":{"byte_end":61,"byte_start":52,"column":53,"document":"legacy.tql","end_column":62,"end_line":1,"line":1,"provenance":"direct"},"unique":false}],"parent":null,"source":{"byte_end":61,"byte_start":37,"column":38,"document":"legacy.tql","end_column":62,"end_line":1,"line":1,"provenance":"direct"}}],"format":"typebridge.generated-descriptors/v1","plays":[],"relations":[],"snapshot_kind":"declared","unsupported_constructs":[]}"#,
    );
    assert_eq!(
        serde_json::from_str::<Value>(&snapshot).unwrap()["format"],
        GENERATED_DECLARED_DESCRIPTOR_V1
    );
    assert!(!snapshot.contains("collection_mode"));
    assert!(!snapshot.contains("distinct"));
}

#[test]
fn ordered_and_distinct_select_v2_and_round_trip_exact_semantics() {
    let document = DocumentId::new("ordered.tql").unwrap();
    let source = "define\nattribute tag, value string;\nentity person, owns tag[] @distinct;\nrelation team, relates member[] @distinct;\n";
    let expected = typeql_to_declared(document.clone(), source).unwrap();
    let snapshot = typeql_to_generated_descriptors(document, source).unwrap();
    let value: Value = serde_json::from_str(&snapshot).unwrap();
    assert_eq!(value["format"], GENERATED_DECLARED_DESCRIPTOR_V2);
    assert_eq!(value["closed_world"], true);
    assert_eq!(value["unsupported_constructs"], json!([]));
    assert_eq!(
        value["entities"][0]["owns"][0]["collection_mode"],
        "ordered_list"
    );
    assert_eq!(value["entities"][0]["owns"][0]["distinct"], true);
    assert_eq!(
        value["relations"][0]["relates"][0]["collection_mode"],
        "ordered_list"
    );
    assert_eq!(value["relations"][0]["relates"][0]["distinct"], true);

    let rebuilt = generated_descriptors_to_declared(
        DocumentId::new("generated/declared-schema.json").unwrap(),
        snapshot.as_bytes(),
    )
    .unwrap();
    assert_eq!(
        rebuilt.declared_identity_fingerprint(),
        expected.declared_identity_fingerprint()
    );
    assert_eq!(
        rebuilt
            .facts()
            .filter(|fact| matches!(fact, SchemaFact::Owns(owns) if owns.collection_mode() == CollectionMode::OrderedList))
            .count(),
        1,
    );
    assert_eq!(
        rebuilt
            .facts()
            .filter(|fact| matches!(fact, SchemaFact::Relates(relates) if relates.collection_mode() == CollectionMode::OrderedList))
            .count(),
        1,
    );
    assert_eq!(
        rebuilt
            .facts()
            .filter(|fact| matches!(fact, SchemaFact::Annotation(annotation) if annotation.id().kind() == &AnnotationKindId::Distinct))
            .count(),
        2,
    );

    let package = generate_package_with_declared_descriptors(
        source,
        TargetLanguage::Python,
        &BindgenOptions::default(),
    )
    .expect("ordered package emits its successor descriptor");
    let attached = package
        .file(GENERATED_DECLARED_DESCRIPTOR_PATH)
        .expect("successor descriptor retains the frozen package path");
    assert_eq!(
        serde_json::from_str::<Value>(&attached.contents).unwrap()["format"],
        GENERATED_DECLARED_DESCRIPTOR_V2,
    );
    assert_eq!(
        package
            .files
            .iter()
            .filter(|file| file.path == GENERATED_DECLARED_DESCRIPTOR_PATH)
            .count(),
        1,
    );
}

#[test]
fn v2_defaults_are_accepted_only_inside_a_material_ordered_snapshot() {
    let snapshot = typeql_to_generated_descriptors(
        DocumentId::new("ordered-defaults.tql").unwrap(),
        "define attribute name, value string; attribute tag, value string; entity person, owns tag[], owns name;",
    )
    .unwrap();
    let mut descriptor = serde_json::from_str::<Value>(&snapshot).unwrap();
    let owns = descriptor["entities"][0]["owns"].as_array().unwrap();
    let ordered_index = owns
        .iter()
        .position(|owns| owns["attribute"] == "tag")
        .unwrap();
    let unordered_index = owns
        .iter()
        .position(|owns| owns["attribute"] == "name")
        .unwrap();
    assert_eq!(owns[ordered_index]["collection_mode"], "ordered_list");
    assert!(owns[unordered_index].get("collection_mode").is_none());
    assert!(owns[unordered_index].get("distinct").is_none());

    let canonical = to_canonical_json(&descriptor).unwrap();
    let declared = generated_descriptors_to_declared(
        DocumentId::new("generated/descriptors.json").unwrap(),
        &canonical,
    )
    .expect("v2 defaults omit unordered and false on the other members");
    assert_eq!(
        declared
            .facts()
            .filter(|fact| matches!(fact, SchemaFact::Owns(owns) if owns.collection_mode() == CollectionMode::OrderedList))
            .count(),
        1,
    );

    descriptor["entities"][0]["owns"][unordered_index]["collection_mode"] = json!("unordered");
    let explicit_default = to_canonical_json(&descriptor).unwrap();
    let diagnostics = generated_descriptors_to_declared(
        DocumentId::new("generated/descriptors.json").unwrap(),
        &explicit_default,
    )
    .expect_err("explicit unordered must fail the v2 rebuild-byte check");
    assert_eq!(
        diagnostics
            .iter()
            .next()
            .unwrap()
            .diagnostic()
            .code()
            .as_str(),
        "non_canonical_generated_descriptor",
    );

    let mut unordered_v2 = serde_json::from_str::<Value>(&snapshot).unwrap();
    unordered_v2["entities"][0]["owns"][ordered_index]
        .as_object_mut()
        .unwrap()
        .remove("collection_mode");
    let bytes = to_canonical_json(&unordered_v2).unwrap();
    let diagnostics = generated_descriptors_to_declared(
        DocumentId::new("generated/descriptors.json").unwrap(),
        &bytes,
    )
    .expect_err("an all-unordered snapshot must select v1");
    assert_eq!(
        diagnostics
            .iter()
            .next()
            .unwrap()
            .diagnostic()
            .code()
            .as_str(),
        "non_canonical_generated_descriptor",
    );
}
