use serde_json::Value;
use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::{
    CanonicalizationVersion, Fingerprint, FingerprintDomain, SemanticProfileId,
};
use type_bridge_contract::limits::{MAX_CANONICAL_COLLECTION_LEN, MAX_CANONICAL_DEPTH};
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::schema::{DeclaredSchema, DocumentId, encode_declared_schema};
use type_bridge_schema::{
    MAX_SCHEMA_AUTHORITY_BYTES, ManagedDeltaContext, SCHEMA_AUTHORITY_FINGERPRINT_CANONICALIZATION,
    SCHEMA_AUTHORITY_FINGERPRINT_DOMAIN, SchemaAuthorityErrorCode, SchemaDocumentSet,
    build_schema_authority, decode_schema_authority, encode_schema_authority, normalize_documents,
    schema_authority_capability_vocabulary,
};

fn capabilities(ids: &[&str]) -> CapabilitySet {
    ids.iter()
        .map(|id| CapabilityId::new(*id).expect("test capability is canonical"))
        .collect()
}

fn declared_schema() -> DeclaredSchema {
    let source = r#"format: typebridge.schema/v2
capabilities:
  required: [schema.roles]
attributes:
  name: { value: string }
entities:
  person: { owns: [name] }
relations:
  membership: { relates: [member] }
"#;
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("schema/person.yaml").expect("fixture path is valid"),
        source,
    )])
    .expect("fresh Split-YAML document parses");
    normalize_documents(&documents).expect("fresh Split-YAML document normalizes")
}

fn fixture() -> (DeclaredSchema, CapabilitySet, ManagedDeltaContext) {
    let declared = declared_schema();
    let required = capabilities(&["schema.roles", "server.query-v2"]);
    let context = ManagedDeltaContext::new(
        ManagedScopeId::new("example-application").expect("scope is valid"),
        SemanticProfileId::new("typedb-3.12.1/v1").expect("profile is valid"),
        required.clone(),
    );
    (declared, required, context)
}

fn authority_value() -> (Value, CapabilitySet) {
    let (declared, required, context) = fixture();
    let authority =
        build_schema_authority(&declared, &required, &context).expect("fixture authority builds");
    (
        serde_json::from_slice(&encode_schema_authority(&authority))
            .expect("authority is canonical JSON"),
        required,
    )
}

fn resign(value: &mut Value) {
    let content = to_canonical_json(&value["content"]).expect("mutated content is canonicalizable");
    let fingerprint = Fingerprint::compute(
        FingerprintDomain::new(SCHEMA_AUTHORITY_FINGERPRINT_DOMAIN)
            .expect("authority fingerprint domain is valid"),
        CanonicalizationVersion::new(SCHEMA_AUTHORITY_FINGERPRINT_CANONICALIZATION)
            .expect("authority canonicalization is valid"),
        None,
        &content,
    );
    value["authority_fingerprint"] =
        serde_json::to_value(fingerprint).expect("fingerprint is JSON-representable");
}

fn canonical_bytes(value: &Value) -> Vec<u8> {
    to_canonical_json(value).expect("test mutation is canonicalizable")
}

fn assert_resigned_mutation_rejected(
    base: &Value,
    available: &CapabilitySet,
    mutate: impl FnOnce(&mut Value),
    expected: SchemaAuthorityErrorCode,
) {
    let mut value = base.clone();
    mutate(&mut value);
    resign(&mut value);
    let error = decode_schema_authority(&canonical_bytes(&value), available)
        .expect_err("resigned mutation must fail independent reconstruction");
    assert_eq!(error.code(), expected);
}

#[test]
fn split_yaml_authority_round_trips_without_source_access() {
    let (declared, required, context) = fixture();
    let authority = build_schema_authority(&declared, &required, &context)
        .expect("authority builds from the resolved declaration");
    let bytes = encode_schema_authority(&authority);
    let declared_bytes = encode_declared_schema(&declared).expect("declaration encodes");

    drop(declared);
    let decoded = decode_schema_authority(&bytes, context.available_capabilities())
        .expect("source-free authority decode succeeds");
    let rebuilt = build_schema_authority(
        decoded.declared_schema(),
        decoded.required_capabilities(),
        &ManagedDeltaContext::new(
            decoded.managed_scope().id().clone(),
            decoded.semantic_profile().id().clone(),
            context.available_capabilities().clone(),
        ),
    )
    .expect("decoded authority can rebuild itself");

    assert_eq!(
        encode_declared_schema(decoded.declared_schema()).unwrap(),
        declared_bytes,
    );
    assert_eq!(decoded.resolved_schema(), authority.resolved_schema());
    assert_eq!(decoded.managed_state(), authority.managed_state());
    assert_eq!(decoded.managed_scope(), authority.managed_scope());
    assert_eq!(decoded.semantic_profile(), authority.semantic_profile());
    assert_eq!(decoded.required_capabilities(), &required);
    assert_eq!(
        decoded.authority_fingerprint(),
        authority.authority_fingerprint()
    );
    assert_eq!(encode_schema_authority(&rebuilt), bytes);
    assert_eq!(
        authority.authority_fingerprint().digest().to_hex(),
        "a1cac0a9577e883b48ff33a2fb5790a799b676e6a65f2252f308987be0c0b49b"
    );
}

#[test]
fn artifact_requirements_are_additive_but_fail_closed() {
    let (declared, required, context) = fixture();
    let authority = build_schema_authority(&declared, &required, &context)
        .expect("workspace-additive requirement is accepted");
    assert_eq!(authority.required_capabilities(), &required);
    assert_eq!(
        authority.managed_state().required_capabilities(),
        declared.required_capabilities(),
    );

    let missing_declared = capabilities(&["server.query-v2"]);
    let error = build_schema_authority(&declared, &missing_declared, &context)
        .expect_err("artifact cannot omit a declared requirement");
    assert_eq!(
        error.code(),
        SchemaAuthorityErrorCode::UnsupportedCapability
    );

    let unavailable_context = ManagedDeltaContext::new(
        context.scope_id().clone(),
        context.semantic_profile().clone(),
        capabilities(&["schema.roles"]),
    );
    let error = build_schema_authority(&declared, &required, &unavailable_context)
        .expect_err("artifact cannot claim an unavailable requirement");
    assert_eq!(
        error.code(),
        SchemaAuthorityErrorCode::UnsupportedCapability
    );

    let error = decode_schema_authority(
        &encode_schema_authority(&authority),
        &capabilities(&["schema.roles"]),
    )
    .expect_err("consumer must advertise every artifact requirement");
    assert_eq!(
        error.code(),
        SchemaAuthorityErrorCode::UnsupportedCapability
    );
}

#[test]
fn shared_consumers_accept_additive_workspace_execution_requirements() {
    let declared = declared_schema();
    let available = schema_authority_capability_vocabulary();
    let mut required = declared.required_capabilities().clone();
    required.insert(
        CapabilityId::new("schema.transition.define")
            .expect("additive execution capability is canonical"),
    );
    let context = ManagedDeltaContext::new(
        ManagedScopeId::new("additive-workspace").expect("scope is valid"),
        SemanticProfileId::new("typedb-3.12.1/v1").expect("profile is valid"),
        available.clone(),
    );

    let authority = build_schema_authority(&declared, &required, &context)
        .expect("shared authority vocabulary accepts the workspace requirement");
    let decoded = decode_schema_authority(&encode_schema_authority(&authority), &available)
        .expect("every generated and server consumer can decode the emitted authority");

    assert_eq!(decoded.required_capabilities(), &required);
}

#[test]
fn every_derived_authority_claim_is_reconstructed_after_resigning() {
    let (base, available) = authority_value();
    let stale = "0".repeat(64);

    assert_resigned_mutation_rejected(
        &base,
        &available,
        |value| value["content"]["declared_identity"]["digest"] = stale.clone().into(),
        SchemaAuthorityErrorCode::IntegrityMismatch,
    );
    assert_resigned_mutation_rejected(
        &base,
        &available,
        |value| value["content"]["declared_schema"]["required_capabilities"] = Value::Array(vec![]),
        SchemaAuthorityErrorCode::IntegrityMismatch,
    );
    assert_resigned_mutation_rejected(
        &base,
        &available,
        |value| {
            value["content"]["semantic_profile"]["fingerprint"]["digest"] = stale.clone().into()
        },
        SchemaAuthorityErrorCode::IntegrityMismatch,
    );
    assert_resigned_mutation_rejected(
        &base,
        &available,
        |value| value["content"]["semantic_profile"]["id"] = "typedb-3.11.5/v1".into(),
        SchemaAuthorityErrorCode::IntegrityMismatch,
    );
    assert_resigned_mutation_rejected(
        &base,
        &available,
        |value| value["content"]["semantic_schema"]["digest"] = stale.clone().into(),
        SchemaAuthorityErrorCode::IntegrityMismatch,
    );
    assert_resigned_mutation_rejected(
        &base,
        &available,
        |value| value["content"]["managed_scope"]["id"] = "other-application".into(),
        SchemaAuthorityErrorCode::IntegrityMismatch,
    );
    assert_resigned_mutation_rejected(
        &base,
        &available,
        |value| {
            value["content"]["managed_scope"]["profile"]["fingerprint"]["digest"] =
                stale.clone().into()
        },
        SchemaAuthorityErrorCode::IntegrityMismatch,
    );
    for field in [
        "declared_identity",
        "managed_declared_identity",
        "managed_semantic_schema",
    ] {
        assert_resigned_mutation_rejected(
            &base,
            &available,
            |value| value["content"]["managed_state"][field]["digest"] = stale.clone().into(),
            SchemaAuthorityErrorCode::IntegrityMismatch,
        );
    }
    assert_resigned_mutation_rejected(
        &base,
        &available,
        |value| {
            value["content"]["managed_state"]["selection"]
                .as_array_mut()
                .expect("selection is an array")
                .pop();
        },
        SchemaAuthorityErrorCode::IntegrityMismatch,
    );
    assert_resigned_mutation_rejected(
        &base,
        &available,
        |value| {
            value["content"]["required_capabilities"] =
                Value::Array(vec![Value::String("server.query-v2".to_owned())]);
        },
        SchemaAuthorityErrorCode::UnsupportedCapability,
    );
}

#[test]
fn outer_fingerprint_unknown_fields_and_typed_normalization_fail_closed() {
    let (base, available) = authority_value();
    let mut stale_fingerprint = base.clone();
    stale_fingerprint["content"]["managed_scope"]["id"] = "other-application".into();
    let error = decode_schema_authority(&canonical_bytes(&stale_fingerprint), &available)
        .expect_err("unresigned mutation is rejected");
    assert_eq!(error.code(), SchemaAuthorityErrorCode::IntegrityMismatch);

    let mut unknown_root = base.clone();
    unknown_root["unknown"] = Value::Bool(true);
    let error = decode_schema_authority(&canonical_bytes(&unknown_root), &available)
        .expect_err("unknown root field is rejected");
    assert_eq!(error.code(), SchemaAuthorityErrorCode::Contract);

    assert_resigned_mutation_rejected(
        &base,
        &available,
        |value| value["content"]["semantic_profile"]["unknown"] = Value::Bool(true),
        SchemaAuthorityErrorCode::Contract,
    );
    assert_resigned_mutation_rejected(
        &base,
        &available,
        |value| value["content"]["declared_schema"]["unknown"] = Value::Bool(true),
        SchemaAuthorityErrorCode::Contract,
    );
    assert_resigned_mutation_rejected(
        &base,
        &available,
        |value| value["content"]["managed_state"]["unknown"] = Value::Bool(true),
        SchemaAuthorityErrorCode::IntegrityMismatch,
    );

    let mut duplicate_capability = base.clone();
    duplicate_capability["content"]["required_capabilities"]
        .as_array_mut()
        .expect("capabilities are an array")
        .push(Value::String("schema.roles".to_owned()));
    resign(&mut duplicate_capability);
    let error = decode_schema_authority(&canonical_bytes(&duplicate_capability), &available)
        .expect_err("typed canonical normalization rejects duplicate set entries");
    assert_eq!(error.code(), SchemaAuthorityErrorCode::Contract);
}

#[test]
fn versions_canonical_bytes_and_shared_structural_limits_fail_closed() {
    let (base, available) = authority_value();
    for (field, value) in [
        (
            "authority_version",
            Value::String("typebridge.schema-authority/v2".to_owned()),
        ),
        ("codec_version", Value::from(2)),
        ("schema_ir_version", Value::from(2)),
    ] {
        let mut mutated = base.clone();
        mutated["content"][field] = value;
        resign(&mut mutated);
        let error = decode_schema_authority(&canonical_bytes(&mutated), &available)
            .expect_err("unsupported version is rejected");
        assert_eq!(error.code(), SchemaAuthorityErrorCode::UnsupportedVersion);
    }

    assert_resigned_mutation_rejected(
        &base,
        &available,
        |value| value["content"]["semantic_profile"]["id"] = "typedb-3.13.0/v1".into(),
        SchemaAuthorityErrorCode::UnsupportedCapability,
    );

    let mut non_canonical = canonical_bytes(&base);
    non_canonical.push(b'\n');
    let error = decode_schema_authority(&non_canonical, &available)
        .expect_err("non-canonical JSON is rejected");
    assert_eq!(error.code(), SchemaAuthorityErrorCode::Contract);

    let oversized = vec![b' '; MAX_SCHEMA_AUTHORITY_BYTES + 1];
    let error = decode_schema_authority(&oversized, &available)
        .expect_err("oversized authority is rejected before parsing");
    assert_eq!(error.code(), SchemaAuthorityErrorCode::ResourceLimit);

    let too_deep = format!(
        "{}0{}",
        "[".repeat(MAX_CANONICAL_DEPTH + 1),
        "]".repeat(MAX_CANONICAL_DEPTH + 1)
    );
    let error = decode_schema_authority(too_deep.as_bytes(), &available)
        .expect_err("over-deep authority is rejected before reconstruction");
    assert_eq!(error.code(), SchemaAuthorityErrorCode::ResourceLimit);

    let too_many = format!(
        "[{}]",
        vec!["0"; MAX_CANONICAL_COLLECTION_LEN + 1].join(",")
    );
    let error = decode_schema_authority(too_many.as_bytes(), &available)
        .expect_err("oversized collection is rejected before reconstruction");
    assert_eq!(error.code(), SchemaAuthorityErrorCode::ResourceLimit);
}
