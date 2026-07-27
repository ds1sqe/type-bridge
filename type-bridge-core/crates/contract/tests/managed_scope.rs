use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::{
    ManagedScopeBinding, ManagedScopeId, SemanticProfileFingerprint,
    exclusive_managed_scope_profile_bytes, semantic_profile_canonical_bytes,
};
use type_bridge_contract::semantic_profile::SemanticProfile;

#[test]
fn exclusive_managed_scope_profile_has_fixed_bytes_binding_and_digest() {
    let bytes = exclusive_managed_scope_profile_bytes().unwrap();
    assert_eq!(
        bytes,
        br#"{"internal_facts":"excluded","non_internal_facts":"managed","profile_id":"typebridge.managed-scope/exclusive/v1"}"#,
    );

    let binding =
        ManagedScopeBinding::exclusive(ManagedScopeId::new("example-schema").unwrap()).unwrap();
    assert_eq!(
        binding
            .profile()
            .fingerprint()
            .as_fingerprint()
            .digest()
            .to_hex(),
        "833c78025a775fdad6803a4f399a9edc4c7dd6b79fdb2efd3f48b6e1f751cf74",
    );
    assert_eq!(
        to_canonical_json(&binding).unwrap(),
        br#"{"id":"example-schema","profile":{"fingerprint":{"algorithm":"sha256","canonicalization":"typebridge.managed-scope-profile/v1","digest":"833c78025a775fdad6803a4f399a9edc4c7dd6b79fdb2efd3f48b6e1f751cf74","domain":"typebridge.schema.managed-scope-profile"},"id":"typebridge.managed-scope/exclusive/v1"}}"#,
    );
}

#[test]
fn semantic_profiles_have_fixed_content_bytes_and_separate_digests() {
    let profile_id = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
    let profile = SemanticProfile::resolve(&profile_id).unwrap();
    assert_eq!(
        semantic_profile_canonical_bytes(&profile).unwrap(),
        br#"{"id":"typedb-3.12.1/v1","key_owns_default":{"kind":"cardinality","max":"1","min":"1"},"owns_default":{"kind":"cardinality","max":"1","min":"0"},"plays_default":{"kind":"cardinality","max":"unbounded","min":"0"},"relates_default":{"kind":"cardinality","max":"1","min":"0"}}"#,
    );
    let fingerprint = SemanticProfileFingerprint::compute(&profile).unwrap();
    assert_eq!(
        fingerprint.as_fingerprint().digest().to_hex(),
        "ac7e4d41e9123690a302ca8d25ae20dd6fb44a6cde078d2ac155d2b2056d7308",
    );

    let previous_id = SemanticProfileId::new("typedb-3.11.5/v1").unwrap();
    let previous = SemanticProfile::resolve(&previous_id).unwrap();
    let previous = SemanticProfileFingerprint::compute(&previous).unwrap();
    assert_eq!(
        previous.as_fingerprint().digest().to_hex(),
        "1c299a36c24aa21cd7989d254dc09fc6d8913776b08f9bb71db44f54520920d4",
    );
    assert_ne!(fingerprint, previous);
}
