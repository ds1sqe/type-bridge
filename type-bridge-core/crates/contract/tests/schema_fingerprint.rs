use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::schema::{
    ManagedDeclaredIdentityFingerprint, ManagedSemanticSchemaFingerprint,
    SemanticSchemaFingerprint,
};

fn profile() -> SemanticProfileId {
    SemanticProfileId::new("typedb-3.12.1/v1").expect("fixture profile is valid")
}

#[test]
fn schema_fingerprint_domains_are_not_interchangeable() {
    let bytes = br#"{"facts":[]}"#;
    let semantic = SemanticSchemaFingerprint::compute(profile(), bytes)
        .expect("semantic fingerprint computes");
    let managed_declared = ManagedDeclaredIdentityFingerprint::compute(bytes)
        .expect("managed declared fingerprint computes");
    let managed_semantic = ManagedSemanticSchemaFingerprint::compute(profile(), bytes)
        .expect("managed semantic fingerprint computes");

    assert_eq!(
        semantic.as_fingerprint().domain().as_str(),
        "typebridge.schema.semantic"
    );
    assert_eq!(
        managed_declared.as_fingerprint().domain().as_str(),
        "typebridge.schema.managed-declared-identity"
    );
    assert_eq!(
        managed_declared.as_fingerprint().canonicalization().as_str(),
        "typebridge.managed-declared/v1"
    );
    assert_eq!(
        managed_semantic.as_fingerprint().domain().as_str(),
        "typebridge.schema.managed-semantic"
    );
    assert_eq!(
        managed_semantic.as_fingerprint().canonicalization().as_str(),
        "typebridge.managed-semantic/v1"
    );
    assert_ne!(
        semantic.as_fingerprint().digest(),
        managed_semantic.as_fingerprint().digest()
    );
}

#[test]
fn semantic_profile_participates_in_the_digest() {
    let bytes = br#"{"facts":[]}"#;
    let first = SemanticSchemaFingerprint::compute(profile(), bytes)
        .expect("first fingerprint computes");
    let second = SemanticSchemaFingerprint::compute(
        SemanticProfileId::new("typedb-3.13.0/v1").expect("fixture profile is valid"),
        bytes,
    )
    .expect("second fingerprint computes");

    assert_ne!(
        first.as_fingerprint().digest(),
        second.as_fingerprint().digest()
    );
}
