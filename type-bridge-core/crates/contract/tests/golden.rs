use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::{CanonicalizationVersion, Fingerprint, FingerprintDomain};
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::value::{CanonicalValue, Cardinality};

#[test]
fn phase1_foundation_has_byte_exact_golden_shapes() {
    let long = CanonicalValue::Long(9_007_199_254_740_993);
    let long_bytes = to_canonical_json(&long).unwrap();
    assert_eq!(long_bytes, br#"{"kind":"long","value":"9007199254740993"}"#);

    let type_id = TypeId::new(TypeKind::Entity, "person").unwrap();
    assert_eq!(to_canonical_json(&type_id).unwrap(), br#"{"kind":"entity","label":"person"}"#);

    let capabilities = CapabilitySet::from_iter([
        CapabilityId::new("schema.annotations").unwrap(),
        CapabilityId::new("query.given-multi-row").unwrap(),
    ]);
    assert_eq!(to_canonical_json(&capabilities).unwrap(), br#"["query.given-multi-row","schema.annotations"]"#);

    let cardinality = Cardinality::new(0, None).unwrap();
    assert_eq!(to_canonical_json(&cardinality).unwrap(), br#"{"kind":"cardinality","max":"unbounded","min":"0"}"#);

    let fingerprint = Fingerprint::compute(
        FingerprintDomain::new("test.value").unwrap(),
        CanonicalizationVersion::new("typebridge.canonical-json/v1").unwrap(),
        None,
        &long_bytes,
    );
    assert_eq!(fingerprint.digest().to_hex(), "cbe437dc731095f176ab19a4494c0ee53e491bded9a50627208a9bf022576ce9");
    assert_eq!(
        to_canonical_json(&fingerprint).unwrap(),
        br#"{"algorithm":"sha256","canonicalization":"typebridge.canonical-json/v1","digest":"cbe437dc731095f176ab19a4494c0ee53e491bded9a50627208a9bf022576ce9","domain":"test.value"}"#,
    );
}
