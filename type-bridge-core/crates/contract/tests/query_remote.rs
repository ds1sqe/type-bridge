use serde_json::Value;
use type_bridge_contract::query_plan::v3::query_plan_v3_capability_vocabulary;
use type_bridge_contract::query_remote::{
    MAX_REMOTE_DEADLINE_MS, RemoteCapabilities, RemoteExecutorBinding, RemoteReplySignature,
    RemoteReplySigner, RemoteReplySigningDigest, RemoteSigningPublicKey, checked_remote_deadline,
    checked_remote_limit,
};
fn executor(identity: &str, epoch: &str) -> RemoteExecutorBinding {
    RemoteExecutorBinding::new(identity, epoch).expect("executor binding")
}

#[derive(Clone, Copy)]
struct TestSigner;

impl RemoteReplySigner for TestSigner {
    fn public_key(&self) -> RemoteSigningPublicKey {
        RemoteSigningPublicKey::from_bytes([7; 32])
    }

    fn sign(&self, digest: &RemoteReplySigningDigest) -> RemoteReplySignature {
        let mut signature = [0_u8; 64];
        signature[..32].copy_from_slice(digest.as_bytes());
        signature[32..].copy_from_slice(digest.as_bytes());
        RemoteReplySignature::from_bytes(signature)
    }
}

fn advertisement() -> RemoteCapabilities {
    RemoteCapabilities::new(
        query_plan_v3_capability_vocabulary(),
        executor("contract-executor-identity", "contract-executor-epoch"),
        TestSigner.public_key(),
    )
}

#[test]
fn executor_components_and_advertisements_are_canonical_and_bounded() {
    assert_eq!(
        RemoteExecutorBinding::new("short", "also-short")
            .expect_err("unsafe executor binding")
            .code()
            .as_str(),
        "query_remote_executor_invalid",
    );
    let advertisement = advertisement();
    let bytes = advertisement.encode().expect("advertisement");
    assert_eq!(
        RemoteCapabilities::decode(&bytes).expect("canonical advertisement"),
        advertisement,
    );
    assert_eq!(
        advertisement.reply_key_id(),
        type_bridge_contract::query_remote::RemoteSigningKeyId::for_public_key(
            advertisement.reply_key()
        ),
    );
    let mut mismatched_key_id: Value = serde_json::from_slice(&bytes).expect("advertisement JSON");
    mismatched_key_id["reply_key_id"] = Value::String("00".repeat(32));
    assert_eq!(
        RemoteCapabilities::decode(
            &serde_json::to_vec(&mismatched_key_id).expect("mismatched advertisement")
        )
        .expect_err("key identity must match the exact advertised key")
        .code()
        .as_str(),
        "query_remote_signature_invalid",
    );
    let mut changed = advertisement.clone();
    let same = advertisement.fingerprint().expect("fingerprint");
    assert_eq!(
        same,
        changed.fingerprint().expect("deterministic fingerprint")
    );
    changed = RemoteCapabilities::new(
        query_plan_v3_capability_vocabulary(),
        executor("contract-executor-identity", "contract-different-epoch"),
        TestSigner.public_key(),
    );
    assert_ne!(same, changed.fingerprint().expect("epoch fingerprint"));
}

#[test]
fn advertisements_reject_non_exact_set_and_hex_encodings() {
    let bytes = advertisement().encode().expect("advertisement");

    let mut duplicated: Value = serde_json::from_slice(&bytes).expect("advertisement JSON");
    let capabilities = duplicated["capabilities"]
        .as_array_mut()
        .expect("capability array");
    capabilities.push(capabilities[0].clone());
    assert_eq!(
        RemoteCapabilities::decode(
            &serde_json::to_vec(&duplicated).expect("duplicate capability advertisement")
        )
        .expect_err("duplicate set members must not normalize on decode")
        .code()
        .as_str(),
        "query_remote_capabilities_wire_mismatch",
    );

    let mut reordered: Value = serde_json::from_slice(&bytes).expect("advertisement JSON");
    reordered["capabilities"]
        .as_array_mut()
        .expect("capability array")
        .reverse();
    assert_eq!(
        RemoteCapabilities::decode(
            &serde_json::to_vec(&reordered).expect("reordered capability advertisement")
        )
        .expect_err("set order must not normalize on decode")
        .code()
        .as_str(),
        "query_remote_capabilities_wire_mismatch",
    );

    let mut uppercase: Value = serde_json::from_slice(&bytes).expect("advertisement JSON");
    uppercase["reply_key"] = Value::String(
        uppercase["reply_key"]
            .as_str()
            .expect("reply key")
            .to_ascii_uppercase(),
    );
    uppercase["reply_key_id"] = Value::String(
        uppercase["reply_key_id"]
            .as_str()
            .expect("reply key identity")
            .to_ascii_uppercase(),
    );
    assert_eq!(
        RemoteCapabilities::decode(
            &serde_json::to_vec(&uppercase).expect("uppercase capability advertisement")
        )
        .expect_err("hex spelling must not normalize on decode")
        .code()
        .as_str(),
        "query_remote_capabilities_wire_mismatch",
    );
}

#[test]
fn limit_conversion_rejects_the_signed_and_oversized_ranges() {
    assert_eq!(checked_remote_limit(0).expect("zero"), 0);
    assert_eq!(
        checked_remote_limit(i128::from(u64::MAX)).expect("max"),
        u64::MAX
    );
    for invalid in [-1, i128::from(u64::MAX) + 1] {
        assert_eq!(
            checked_remote_limit(invalid)
                .expect_err("out of range")
                .code()
                .as_str(),
            "query_remote_limit_invalid"
        );
    }
    assert_eq!(checked_remote_deadline(None).expect("absent"), None);
    assert_eq!(
        checked_remote_deadline(Some(30_000)).expect("present"),
        Some(30_000)
    );
    assert_eq!(
        checked_remote_deadline(Some(-1))
            .expect_err("negative deadline")
            .code()
            .as_str(),
        "query_remote_limit_invalid"
    );
    assert_eq!(
        checked_remote_deadline(Some(i128::from(MAX_REMOTE_DEADLINE_MS) + 1))
            .expect_err("deadline above the supported Instant horizon")
            .code()
            .as_str(),
        "query_remote_deadline_limit",
    );
}

#[test]
fn advertised_answer_limits_are_omitted_at_default_and_canonical_otherwise() {
    use std::collections::BTreeMap;

    let default = advertisement();
    let default_bytes = default.encode().expect("default advertisement");
    assert!(default.answer_limits().is_empty());
    assert!(!String::from_utf8_lossy(&default_bytes).contains("answer_limits"));

    let ceilings = BTreeMap::from([
        ("items".to_owned(), 1_000_000_u64),
        ("bytes".to_owned(), 1_u64 << 30),
    ]);
    let advertised = advertisement()
        .with_answer_limits(ceilings.clone())
        .expect("admissible ceilings");
    let bytes = advertised.encode().expect("raised advertisement");
    let decoded = RemoteCapabilities::decode(&bytes).expect("canonical raised advertisement");
    assert_eq!(decoded, advertised);
    assert_eq!(decoded.answer_limits(), &ceilings);
    assert_ne!(
        decoded.fingerprint().expect("raised fingerprint"),
        default.fingerprint().expect("default fingerprint"),
        "a request cannot bind one advertisement and be admitted under another's ceilings"
    );

    let explicit = advertisement()
        .with_answer_limits(BTreeMap::from([("statements".to_owned(), 3)]))
        .unwrap();
    assert_eq!(
        RemoteCapabilities::decode(&explicit.encode().unwrap()).unwrap(),
        explicit
    );
    assert_eq!(explicit.answer_limits().get("statements"), Some(&3));

    // None and durations beyond the former 24-hour cap are canonical ceilings.
    for milliseconds in [0, 24 * 60 * 60 * 1000 + 1, u64::MAX] {
        let value = advertisement()
            .with_answer_limits(BTreeMap::from([(
                "timeout_milliseconds".to_owned(),
                milliseconds,
            )]))
            .unwrap();
        assert_eq!(
            RemoteCapabilities::decode(&value.encode().unwrap())
                .unwrap()
                .answer_limits()
                .get("timeout_milliseconds"),
            Some(&milliseconds)
        );
    }

    // An unknown name, a redundant default, and a value past its maximum.
    for refused in [("made_up", 1_u64), ("items", 65_536), ("statements", 4097)] {
        assert_eq!(
            advertisement()
                .with_answer_limits(BTreeMap::from([(refused.0.to_owned(), refused.1)]))
                .expect_err("inadmissible ceilings cannot be advertised")
                .code()
                .as_str(),
            "query_remote_capabilities_limits_invalid",
            "{refused:?}",
        );
    }
    let mut forged: Value = serde_json::from_slice(&bytes).expect("advertisement JSON");
    forged["answer_limits"]["made_up"] = Value::from(1);
    assert_eq!(
        RemoteCapabilities::decode(
            &type_bridge_contract::codec::to_canonical_json(&forged).expect("forged advertisement"),
        )
        .expect_err("forged ceilings are refused")
        .code()
        .as_str(),
        "query_remote_capabilities_limits_invalid",
    );
}

#[test]
fn advertised_query_limits_are_omitted_at_default_and_canonical_otherwise() {
    use type_bridge_contract::limits::QueryLimits;

    let default = advertisement();
    let default_bytes = default.encode().expect("default advertisement");
    assert_eq!(default.query_limits(), QueryLimits::CANONICAL);
    // Advertising the defaults explicitly changes neither bytes nor identity.
    let explicit = advertisement()
        .with_query_limits(QueryLimits::CANONICAL)
        .expect("default limits");
    assert_eq!(explicit.encode().expect("explicit default"), default_bytes);
    assert!(!String::from_utf8_lossy(&default_bytes).contains("query_limits"));

    let mut raised = QueryLimits::CANONICAL;
    raised.codec.max_bytes = 64 * 1024 * 1024;
    raised.remote_request_bytes = 128 * 1024 * 1024;
    raised.structural.boolean_terms = 8;
    let advertised = advertisement()
        .with_query_limits(raised)
        .expect("admissible limits");
    let bytes = advertised.encode().expect("raised advertisement");
    let decoded = RemoteCapabilities::decode(&bytes).expect("canonical raised advertisement");
    assert_eq!(decoded, advertised);
    assert_eq!(decoded.query_limits(), raised);
    assert_ne!(
        decoded.fingerprint().expect("raised fingerprint"),
        default.fingerprint().expect("default fingerprint"),
        "a request cannot bind one advertisement and be admitted under another's limits"
    );

    let mut unsupported = QueryLimits::CANONICAL;
    unsupported.codec.max_depth = 1_000;
    assert_eq!(
        advertisement()
            .with_query_limits(unsupported)
            .expect_err("unsupported limits cannot be advertised")
            .code()
            .as_str(),
        "query_remote_capabilities_limits_invalid",
    );

    let forged = |mutate: fn(&mut Value)| {
        let mut wire: Value = serde_json::from_slice(&bytes).expect("advertisement JSON");
        mutate(&mut wire);
        // Canonical key order, so the forgery reaches the limits check under
        // every JSON backend instead of failing as non-canonical bytes.
        RemoteCapabilities::decode(
            &type_bridge_contract::codec::to_canonical_json(&wire).expect("forged advertisement"),
        )
        .expect_err("forged limits are refused")
        .code()
        .as_str()
        .to_owned()
    };
    // An unknown name, a redundant default, and an unsupported value.
    assert_eq!(
        forged(|wire| wire["query_limits"]["made_up"] = Value::from(1)),
        "query_remote_capabilities_limits_invalid",
    );
    assert_eq!(
        forged(|wire| wire["query_limits"]["bindings"] = Value::from(256)),
        "query_remote_capabilities_limits_invalid",
    );
    assert_eq!(
        forged(|wire| wire["query_limits"]["canonical_depth"] = Value::from(1_000)),
        "query_remote_capabilities_limits_invalid",
    );
    assert_eq!(
        forged(|wire| wire["query_limits"]["boolean_terms"] = Value::from(0)),
        "query_remote_capabilities_limits_invalid",
    );
}
