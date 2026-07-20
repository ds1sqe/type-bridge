use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema::{
    ManagedSchemaScope, SchemaDocumentSet, TYPEDB_3_12_1_TIMEZONE_POLICY_ID,
    canonical_managed_declared_identity_bytes, canonical_managed_semantic_schema_bytes,
    canonical_semantic_schema_bytes, managed_declared_identity_fingerprint,
    managed_semantic_schema_fingerprint, normalize_documents, semantic_schema_fingerprint,
};

fn document(source: &str) -> SchemaDocumentSet {
    SchemaDocumentSet::parse([(
        DocumentId::new("schema.yaml").expect("fixture document identifier is valid"),
        source,
    )])
    .expect("fixture YAML parses")
}

fn profile() -> SemanticProfileId {
    SemanticProfileId::new("typedb-3.12.1/v1").expect("fixture profile is valid")
}

#[test]
fn explicit_equal_cardinality_default_is_semantically_omitted() {
    let omitted = r#"format: typebridge.schema/v2
attributes:
  name: { value: string }
entities:
  person: { owns: [name] }
"#;
    let explicit = r#"format: typebridge.schema/v2
attributes:
  name: { value: string }
entities:
  person:
    owns:
      name:
        card: { min: 0, max: 1 }
"#;
    let omitted = normalize_documents(&document(omitted)).expect("omitted schema normalizes");
    let explicit = normalize_documents(&document(explicit)).expect("explicit schema normalizes");

    assert_ne!(
        omitted.declared_identity_fingerprint(),
        explicit.declared_identity_fingerprint()
    );
    assert_eq!(
        semantic_schema_fingerprint(&omitted, &profile()).expect("fingerprint computes"),
        semantic_schema_fingerprint(&explicit, &profile()).expect("fingerprint computes")
    );
}

#[test]
fn key_owns_materializes_exactly_one_in_semantic_bytes() {
    let source = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
entities:
  account:
    owns:
      identifier: { key: true }
"#;
    let declared = normalize_documents(&document(source)).expect("key schema normalizes");
    let bytes =
        canonical_semantic_schema_bytes(&declared, &profile()).expect("semantic bytes compute");
    let value: serde_json::Value = serde_json::from_slice(&bytes).expect("semantic bytes are JSON");
    let cardinality = value["facts"]
        .as_array()
        .expect("semantic facts are an array")
        .iter()
        .find(|fact| fact["value"]["kind"] == "materialized_cardinality")
        .expect("ownership cardinality is materialized");
    let cardinality = &cardinality["value"]["value"]["cardinality"];

    assert_eq!(cardinality["min"].as_str(), Some("1"));
    assert_eq!(cardinality["max"].as_str(), Some("1"));
}

#[test]
fn semantic_doc_changes_semantic_fingerprint() {
    let first = r#"format: typebridge.schema/v2
entities:
  person: { doc: "first" }
"#;
    let second = r#"format: typebridge.schema/v2
entities:
  person: { doc: "second" }
"#;
    let first = normalize_documents(&document(first)).expect("first schema normalizes");
    let second = normalize_documents(&document(second)).expect("second schema normalizes");
    assert_ne!(
        semantic_schema_fingerprint(&first, &profile()).expect("fingerprint computes"),
        semantic_schema_fingerprint(&second, &profile()).expect("fingerprint computes")
    );
}

#[test]
fn comments_change_document_but_not_semantic_fingerprint() {
    let plain_source = "format: typebridge.schema/v2\nentities:\n  person: {}\n";
    let commented_source =
        "# presentation only\nformat: typebridge.schema/v2\nentities:\n  person: {}\n";
    let plain_documents = document(plain_source);
    let commented_documents = document(commented_source);
    let id = DocumentId::new("schema.yaml").expect("fixture document identifier is valid");
    assert_ne!(
        plain_documents
            .get(&id)
            .expect("document exists")
            .fingerprint(),
        commented_documents
            .get(&id)
            .expect("document exists")
            .fingerprint()
    );

    let plain = normalize_documents(&plain_documents).expect("plain schema normalizes");
    let commented = normalize_documents(&commented_documents).expect("commented schema normalizes");
    assert_eq!(
        semantic_schema_fingerprint(&plain, &profile()).expect("fingerprint computes"),
        semantic_schema_fingerprint(&commented, &profile()).expect("fingerprint computes")
    );
}

#[test]
fn managed_fingerprints_bind_scope_profile_and_semantic_profile() {
    let source = "format: typebridge.schema/v2\nentities:\n  person: {}\n";
    let declared = normalize_documents(&document(source)).expect("schema normalizes");
    let first_scope =
        ManagedSchemaScope::bind_exclusive(ManagedScopeId::new("first-schema").unwrap(), &declared)
            .unwrap();
    let second_scope = ManagedSchemaScope::bind_exclusive(
        ManagedScopeId::new("second-schema").unwrap(),
        &declared,
    )
    .unwrap();

    assert_ne!(
        managed_declared_identity_fingerprint(&declared, &first_scope).unwrap(),
        managed_declared_identity_fingerprint(&declared, &second_scope).unwrap(),
    );
    assert_ne!(
        managed_semantic_schema_fingerprint(&declared, &profile(), &first_scope).unwrap(),
        managed_semantic_schema_fingerprint(&declared, &profile(), &second_scope).unwrap(),
    );

    let previous_profile = SemanticProfileId::new("typedb-3.11.5/v1").unwrap();
    assert_ne!(
        managed_semantic_schema_fingerprint(&declared, &profile(), &first_scope).unwrap(),
        managed_semantic_schema_fingerprint(&declared, &previous_profile, &first_scope).unwrap(),
    );
}

#[test]
fn exclusive_binding_selects_every_direct_declared_fact() {
    let source = r#"format: typebridge.schema/v2
attributes:
  name: { value: string }
entities:
  person: { owns: [name] }
"#;
    let declared = normalize_documents(&document(source)).expect("schema normalizes");
    let scope = ManagedSchemaScope::bind_exclusive(
        ManagedScopeId::new("complete-schema").unwrap(),
        &declared,
    )
    .unwrap();

    assert_eq!(scope.selection().len(), declared.facts().len());
    assert!(
        declared
            .facts()
            .all(|fact| scope.selection().contains(&fact.id()))
    );
}

#[test]
fn managed_schema_preimages_and_fingerprints_have_fixed_goldens() {
    let source = "format: typebridge.schema/v2\nentities:\n  person: {}\n";
    let declared = normalize_documents(&document(source)).expect("schema normalizes");
    let scope = ManagedSchemaScope::bind_exclusive(
        ManagedScopeId::new("example-schema").unwrap(),
        &declared,
    )
    .unwrap();

    let declared_bytes = canonical_managed_declared_identity_bytes(&declared, &scope).unwrap();
    assert_eq!(
        declared_bytes,
        br#"{"facts":[{"kind":"type","value":{"id":{"kind":"entity","label":"person"}}}],"format_version":1,"managed_scope":{"id":"example-schema","profile":{"fingerprint":{"algorithm":"sha256","canonicalization":"typebridge.managed-scope-profile/v1","digest":"833c78025a775fdad6803a4f399a9edc4c7dd6b79fdb2efd3f48b6e1f751cf74","domain":"typebridge.schema.managed-scope-profile"},"id":"typebridge.managed-scope/exclusive/v1"}},"required_capabilities":[]}"#,
    );
    assert_eq!(
        managed_declared_identity_fingerprint(&declared, &scope)
            .unwrap()
            .as_fingerprint()
            .digest()
            .to_hex(),
        "7cb0058ce4bc5b85e2c6d69084bc9dd9cf5110f0453ce5be52b2019f773cbbe4",
    );

    let semantic_bytes =
        canonical_managed_semantic_schema_bytes(&declared, &profile(), &scope).unwrap();
    assert_eq!(
        semantic_bytes,
        br#"{"facts":[{"id":{"kind":"type","value":{"kind":"entity","label":"person"}},"value":{"kind":"direct","value":{"kind":"type","value":{"id":{"kind":"entity","label":"person"}}}}}],"format_version":1,"managed_scope":{"id":"example-schema","profile":{"fingerprint":{"algorithm":"sha256","canonicalization":"typebridge.managed-scope-profile/v1","digest":"833c78025a775fdad6803a4f399a9edc4c7dd6b79fdb2efd3f48b6e1f751cf74","domain":"typebridge.schema.managed-scope-profile"},"id":"typebridge.managed-scope/exclusive/v1"}},"required_capabilities":[],"semantic_profile":"typedb-3.12.1/v1","semantic_profile_fingerprint":{"algorithm":"sha256","canonicalization":"typebridge.semantic-profile/v1","digest":"ac7e4d41e9123690a302ca8d25ae20dd6fb44a6cde078d2ac155d2b2056d7308","domain":"typebridge.schema.semantic-profile"},"timezone_policy":"typedb-iana-2024a/v1"}"#,
    );
    assert_eq!(
        managed_semantic_schema_fingerprint(&declared, &profile(), &scope)
            .unwrap()
            .as_fingerprint()
            .digest()
            .to_hex(),
        "68fce8298d9dfe8b861e3e99b8f6a32ec7636486b033b94de27f2cad58a76c62",
    );
}

#[test]
fn struct_field_order_and_optionality_are_semantic() {
    let first = r#"format: typebridge.schema/v2
structs:
  sample:
    fields:
      - name: first
        type: string
      - name: second
        type: integer
"#;
    let reordered = r#"format: typebridge.schema/v2
structs:
  sample:
    fields:
      - name: second
        type: integer
      - name: first
        type: string
"#;
    let optional = r#"format: typebridge.schema/v2
structs:
  sample:
    fields:
      - name: first
        type: string
        optional: true
      - name: second
        type: integer
"#;

    let first = normalize_documents(&document(first)).unwrap();
    let reordered = normalize_documents(&document(reordered)).unwrap();
    let optional = normalize_documents(&document(optional)).unwrap();

    let first = semantic_schema_fingerprint(&first, &profile()).unwrap();
    let reordered = semantic_schema_fingerprint(&reordered, &profile()).unwrap();
    let optional = semantic_schema_fingerprint(&optional, &profile()).unwrap();

    assert_ne!(first, reordered);
    assert_ne!(first, optional);
}

#[test]
fn omitted_and_explicit_false_struct_optionality_are_semantically_equal() {
    let omitted = r#"format: typebridge.schema/v2
structs:
  sample:
    fields:
      - name: value
        type: string
"#;
    let explicit = r#"format: typebridge.schema/v2
structs:
  sample:
    fields:
      - name: value
        type: string
        optional: false
"#;

    let omitted = normalize_documents(&document(omitted)).unwrap();
    let explicit = normalize_documents(&document(explicit)).unwrap();

    assert_eq!(
        semantic_schema_fingerprint(&omitted, &profile()).unwrap(),
        semantic_schema_fingerprint(&explicit, &profile()).unwrap(),
    );
}

#[test]
fn timezone_representation_is_declared_but_utc_instant_is_semantic() {
    let named = r#"format: typebridge.schema/v2
attributes:
  observed:
    value:
      type: datetime-tz
      values: ["2024-01-15T12:00:00[Europe/London]"]
"#;
    let utc = r#"format: typebridge.schema/v2
attributes:
  observed:
    value:
      type: datetime-tz
      values: ["2024-01-15T12:00:00Z"]
"#;
    let named = normalize_documents(&document(named)).unwrap();
    let utc = normalize_documents(&document(utc)).unwrap();

    assert_ne!(
        named.declared_identity_fingerprint(),
        utc.declared_identity_fingerprint(),
    );
    assert_eq!(
        semantic_schema_fingerprint(&named, &profile()).unwrap(),
        semantic_schema_fingerprint(&utc, &profile()).unwrap(),
    );

    let bytes = canonical_semantic_schema_bytes(&named, &profile()).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains(TYPEDB_3_12_1_TIMEZONE_POLICY_ID));
    assert!(!text.contains("Europe/London"));
}

#[test]
fn schema_fingerprints_and_semantic_bytes_have_fixed_goldens() {
    let source = "format: typebridge.schema/v2\nattributes:\n  name: { value: string }\nentities:\n  person: { owns: [name] }\n";
    let declared = normalize_documents(&document(source)).expect("golden schema normalizes");
    let semantic =
        semantic_schema_fingerprint(&declared, &profile()).expect("fingerprint computes");
    let bytes = canonical_semantic_schema_bytes(&declared, &profile())
        .expect("canonical semantic bytes compute");

    assert_eq!(
        declared
            .declared_identity_fingerprint()
            .as_fingerprint()
            .digest()
            .to_hex(),
        "5624e642d504db2396a35ae200950cfd0c9d1a7be00b041b2fc49155cfe759bb"
    );
    assert_eq!(
        semantic.as_fingerprint().digest().to_hex(),
        "c06028d61573f712d711e593544505366dfa506715090124df8cb7ec9a529752"
    );
    assert_eq!(
        String::from_utf8(bytes).expect("canonical JSON is UTF-8"),
        r#"{"facts":[{"id":{"kind":"type","value":{"kind":"entity","label":"person"}},"value":{"kind":"direct","value":{"kind":"type","value":{"id":{"kind":"entity","label":"person"}}}}},{"id":{"kind":"type","value":{"kind":"attribute","label":"name"}},"value":{"kind":"direct","value":{"kind":"type","value":{"id":{"kind":"attribute","label":"name"}}}}},{"id":{"kind":"value","value":"name"},"value":{"kind":"direct","value":{"kind":"value","value":{"id":"name","value_type":"string"}}}},{"id":{"kind":"owns","value":{"attribute":"name","owner":{"kind":"entity","label":"person"}}},"value":{"kind":"direct","value":{"kind":"owns","value":{"id":{"attribute":"name","owner":{"kind":"entity","label":"person"}}}}}},{"id":{"kind":"annotation","value":{"kind":{"kind":"card"},"subject":{"kind":"owns","value":{"attribute":"name","owner":{"kind":"entity","label":"person"}}}}},"value":{"kind":"materialized_cardinality","value":{"cardinality":{"kind":"cardinality","max":"1","min":"0"},"subject":{"kind":"owns","value":{"attribute":"name","owner":{"kind":"entity","label":"person"}}}}}}],"format_version":1,"required_capabilities":[],"semantic_profile":"typedb-3.12.1/v1","timezone_policy":"typedb-iana-2024a/v1"}"#
    );
}
