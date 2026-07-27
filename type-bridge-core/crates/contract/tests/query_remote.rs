use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;
use sha2::{Digest, Sha256};
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::limits::{MAX_CANONICAL_DEPTH, MAX_REMOTE_ENVELOPE_BYTES};
use type_bridge_contract::migration_assertion::{
    AssertionBinding, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::query_plan::{
    InputColumn, InputColumnId, InputRow, QueryInvocation, QueryOperand, QueryOperation,
    QueryOutput, QueryPattern, QueryPlan, QueryPlanFingerprint, ReadStage,
};
use type_bridge_contract::query_remote::{
    DEFAULT_REMOTE_DEADLINE_MS, MAX_REMOTE_CLOCK_SKEW_MS, MAX_REMOTE_DEADLINE_MS,
    RemoteCapabilities, RemoteExecutorBinding, RemoteFieldValue, RemoteLimits, RemoteOutcome,
    RemoteOutcomeShape, RemoteQueryFailure, RemoteQueryRequest, RemoteQueryResponse, RemoteReply,
    RemoteReplyDecodeLimits, RemoteReplySignature, RemoteReplySigner, RemoteReplySigningDigest,
    RemoteReplyVerifier, RemoteRequestFingerprint, RemoteSigningPublicKey, RemoteValue,
    checked_remote_deadline, checked_remote_limit, decode_remote_reply,
    decode_signed_remote_failure,
};
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_contract::temporal::{CanonicalDateTimeTz, TimeZoneDesignator};
use type_bridge_contract::value::{CanonicalString, CanonicalValue, ValueTypeTag};
use type_bridge_contract::{query_given_rows_capability, query_plan_capability_vocabulary};

fn binding(id: u16, variable: &str) -> AssertionBinding {
    AssertionBinding::new(
        BindingId::new(id).expect("binding id"),
        QueryVariable::new(variable).expect("query variable"),
    )
}

fn binding_id(id: u16) -> BindingId {
    BindingId::new(id).expect("binding id")
}

fn managed_semantics() -> ManagedSemanticSchemaFingerprint {
    ManagedSemanticSchemaFingerprint::compute(
        SemanticProfileId::new("typedb-3.12.1/v1").expect("semantic profile"),
        b"query-remote-managed-fixture",
    )
    .expect("managed semantic fingerprint")
}

fn person_name_patterns() -> Vec<QueryPattern> {
    vec![
        QueryPattern::Isa {
            binding: binding_id(0),
            include_subtypes: true,
            type_id: TypeId::new(TypeKind::Entity, "person").expect("type id"),
        },
        QueryPattern::Has {
            attribute: binding_id(1),
            attribute_id: AttributeId::new("name").expect("attribute id"),
            owner: binding_id(0),
        },
    ]
}

fn limits() -> RemoteLimits {
    RemoteLimits {
        deadline_ms: None,
        max_bytes: 1 << 20,
        max_items: 100,
        max_collection_members: 1 << 16,
    }
}

const NONCE: &str = "remote-nonce-0123456789abcdef";
const NOW_MS: u64 = 1_800_000_000_000;

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

impl RemoteReplyVerifier for TestSigner {
    fn verify(
        &self,
        key: RemoteSigningPublicKey,
        digest: &RemoteReplySigningDigest,
        signature: &RemoteReplySignature,
    ) -> bool {
        key == self.public_key() && *signature == self.sign(digest)
    }
}

#[derive(Default)]
struct CountingVerifier {
    calls: AtomicUsize,
}

impl CountingVerifier {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl RemoteReplyVerifier for CountingVerifier {
    fn verify(
        &self,
        key: RemoteSigningPublicKey,
        digest: &RemoteReplySigningDigest,
        signature: &RemoteReplySignature,
    ) -> bool {
        self.calls.fetch_add(1, Ordering::SeqCst);
        TestSigner.verify(key, digest, signature)
    }
}

fn advertisement() -> RemoteCapabilities {
    RemoteCapabilities::new(
        query_plan_capability_vocabulary(),
        executor("contract-executor-identity", "contract-executor-epoch"),
        TestSigner.public_key(),
    )
}

fn decode_count_reply(
    bytes: &[u8],
    plan: &QueryPlanFingerprint,
    request: &RemoteRequestFingerprint,
    advertisement: &RemoteCapabilities,
) -> Result<type_bridge_contract::query_remote::RemoteReply, Diagnostic> {
    decode_remote_reply(
        bytes,
        NONCE,
        plan,
        request,
        &advertisement
            .fingerprint()
            .expect("advertisement fingerprint"),
        advertisement.reply_key(),
        RemoteReplyDecodeLimits {
            shape: RemoteOutcomeShape::Count,
            max_bytes: u64::try_from(MAX_REMOTE_ENVELOPE_BYTES).expect("wire ceiling"),
            max_items: 100,
            max_collection_members: 100,
        },
        &TestSigner,
    )
}

fn sign_raw_payload(template: &[u8], payload: &[u8]) -> Vec<u8> {
    let outer: Value = serde_json::from_slice(template).expect("signed template");
    let advertisement = outer["advertisement"].as_str().expect("advertisement");
    let format = outer["format"].as_str().expect("format");
    let key = outer["key"].as_str().expect("key");
    let key_id = outer["key_id"].as_str().expect("key id");
    let prefix = format!(
        "{{\"advertisement\":\"{advertisement}\",\"format\":\"{format}\",\"key\":\"{key}\",\"key_id\":\"{key_id}\",\"payload\":"
    );
    let mut hasher = Sha256::new();
    hasher
        .update(type_bridge_contract::query_remote::QUERY_REMOTE_REPLY_SIGNATURE_DOMAIN.as_bytes());
    hasher.update([0]);
    hasher.update(prefix.as_bytes());
    hasher.update(payload);
    hasher.update(b"}");
    let digest: [u8; 32] = hasher.finalize().into();
    let mut signature = [0_u8; 64];
    signature[..32].copy_from_slice(&digest);
    signature[32..].copy_from_slice(&digest);
    let signature = signature
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let suffix = format!(",\"signature\":\"{signature}\"}}");
    let mut encoded = Vec::with_capacity(prefix.len() + payload.len() + suffix.len());
    encoded.extend_from_slice(prefix.as_bytes());
    encoded.extend_from_slice(payload);
    encoded.extend_from_slice(suffix.as_bytes());
    encoded
}

/// A plan whose canonical bytes exceed the 1 MiB per-string ceiling.
///
/// Each literal stays far below the per-string limit; only the whole
/// document is large, exactly the shape the plan contract's 16 MiB
/// artifact limit admits.
fn oversized_plan() -> QueryPlan {
    let mut patterns = person_name_patterns();
    for label in ["x", "y", "z"] {
        patterns.push(QueryPattern::Value {
            comparator: ValueComparator::NotEqual,
            left: QueryOperand::Binding {
                binding: binding_id(1),
            },
            right: QueryOperand::Literal {
                value: CanonicalValue::String(
                    CanonicalString::new(label.repeat(600 * 1024)).expect("canonical literal"),
                ),
            },
        });
    }
    QueryPlan::new(
        vec![binding(0, "person"), binding(1, "name")],
        Vec::new(),
        vec![ReadStage::Match { patterns }],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        managed_semantics(),
    )
    .expect("oversized plan")
}

fn input_plan() -> QueryPlan {
    let mut patterns = person_name_patterns();
    patterns.push(QueryPattern::Value {
        comparator: ValueComparator::Equal,
        left: QueryOperand::Binding {
            binding: binding_id(1),
        },
        right: QueryOperand::Input {
            column: InputColumnId::new(0),
        },
    });
    QueryPlan::new(
        vec![binding(0, "person"), binding(1, "name")],
        vec![InputColumn::new(
            InputColumnId::new(0),
            QueryVariable::new("wanted_name").expect("input name"),
            ValueTypeTag::String,
            false,
        )],
        vec![ReadStage::Match { patterns }],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        managed_semantics(),
    )
    .expect("input plan")
}

fn nested_value_plan(negation_levels: usize) -> QueryPlan {
    let mut nested = QueryPattern::Value {
        comparator: ValueComparator::Equal,
        left: QueryOperand::Binding {
            binding: binding_id(0),
        },
        right: QueryOperand::Literal {
            value: CanonicalValue::String(
                CanonicalString::new("depth-boundary").expect("canonical literal"),
            ),
        },
    };
    for _ in 0..negation_levels {
        nested = QueryPattern::Not {
            patterns: vec![nested],
        };
    }
    QueryPlan::new(
        vec![binding(0, "person")],
        Vec::new(),
        vec![ReadStage::Match {
            patterns: vec![person_name_patterns()[0].clone(), nested],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0)],
        },
        managed_semantics(),
    )
    .expect("structurally valid nested plan")
}

fn json_depth(value: &Value) -> usize {
    match value {
        Value::Array(values) => 1 + values.iter().map(json_depth).max().unwrap_or(0),
        Value::Object(values) => 1 + values.values().map(json_depth).max().unwrap_or(0),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => 1,
    }
}

fn string_row(value: &str) -> InputRow {
    InputRow::new(vec![Some(CanonicalValue::String(
        CanonicalString::new(value).expect("canonical string"),
    ))])
}

#[test]
fn canonical_depth_boundary_plan_rides_one_request_framing_level() {
    let plan = nested_value_plan(28);
    let plan_bytes = plan.canonical_bytes().expect("depth-boundary plan bytes");
    let plan_json: Value = serde_json::from_slice(&plan_bytes).expect("plan JSON");
    assert_eq!(json_depth(&plan_json), MAX_CANONICAL_DEPTH);

    let invocation =
        QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).expect("invocation");
    let request = RemoteQueryRequest::new(
        &plan,
        &invocation,
        &advertisement(),
        limits(),
        NONCE,
        NOW_MS,
    )
    .expect("request");
    let request_bytes = request
        .encode()
        .expect("request framing adds one admitted depth level");
    let request_json: Value = serde_json::from_slice(&request_bytes).expect("request JSON");
    assert_eq!(json_depth(&request_json), MAX_CANONICAL_DEPTH + 1);
    let decoded = RemoteQueryRequest::decode(&request_bytes).expect("request decodes");
    assert_eq!(decoded.plan().expect("embedded plan revalidates"), plan);

    let beyond = nested_value_plan(29);
    let error = beyond
        .canonical_bytes()
        .expect_err("standalone plan beyond the canonical depth still fails locally");
    assert_eq!(error.code().as_str(), "canonical_json_too_deep");
}

#[test]
fn plans_beyond_the_string_ceiling_ride_the_request_envelope() {
    use type_bridge_contract::limits::{
        MAX_CANONICAL_BYTES, MAX_INPUT_BYTES, MAX_REMOTE_ENVELOPE_BYTES,
    };

    const {
        assert!(
            MAX_REMOTE_ENVELOPE_BYTES >= MAX_CANONICAL_BYTES + MAX_INPUT_BYTES + 1024 * 1024,
            "the owning envelope must budget independently for a maximal plan, input rows, and framing",
        );
    }
    let plan = oversized_plan();
    assert!(
        plan.canonical_bytes().expect("plan bytes").len() > 1024 * 1024,
        "fixture must exceed the per-string ceiling"
    );
    let invocation =
        QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).expect("invocation");

    let request = RemoteQueryRequest::new(
        &plan,
        &invocation,
        &advertisement(),
        limits(),
        NONCE,
        NOW_MS,
    )
    .expect("request envelope");
    let bytes = request.encode().expect("request bytes");
    let decoded = RemoteQueryRequest::decode(&bytes).expect("request decodes");
    assert_eq!(decoded.plan().expect("embedded plan decodes"), plan);
}

#[test]
fn requests_reject_nested_scalar_normalization() {
    let plan = input_plan();
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("ada")])
        .expect("invocation");
    let bytes = RemoteQueryRequest::new(
        &plan,
        &invocation,
        &advertisement(),
        limits(),
        NONCE,
        NOW_MS,
    )
    .expect("request")
    .encode()
    .expect("request bytes");
    let request: Value = serde_json::from_slice(&bytes).expect("request JSON");

    let mut unknown = request.clone();
    unknown["rows"][0][0]["unknown"] = Value::Null;
    let unknown = to_canonical_json(&unknown).expect("canonical unknown-field request");
    assert_eq!(
        RemoteQueryRequest::decode(&unknown)
            .expect_err("unknown CanonicalValue members must not disappear during decode")
            .code()
            .as_str(),
        "query_remote_request_wire_mismatch",
    );

    let mut normalizing = request;
    normalizing["rows"][0][0] = serde_json::json!({"kind": "decimal", "value": "+001.2300dec"});
    let normalizing = to_canonical_json(&normalizing).expect("canonical normalizing request");
    assert_eq!(
        RemoteQueryRequest::decode(&normalizing)
            .expect_err("normalizing scalar spellings must not change request identity")
            .code()
            .as_str(),
        "query_remote_request_wire_mismatch",
    );
}

#[test]
fn signed_responses_reject_nested_scalar_normalization() {
    let plan = input_plan();
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("ada")])
        .expect("invocation");
    let advertisement = advertisement();
    let request =
        RemoteQueryRequest::new(&plan, &invocation, &advertisement, limits(), NONCE, NOW_MS)
            .expect("request");
    let request_fingerprint =
        RemoteRequestFingerprint::compute(&request.encode().expect("request bytes"))
            .expect("request fingerprint");
    let plan_fingerprint = QueryPlanFingerprint::compute(&plan).expect("plan fingerprint");
    let response = RemoteQueryResponse::new(
        NONCE,
        &plan_fingerprint,
        &request_fingerprint,
        RemoteOutcome::Rows {
            rows: vec![vec![RemoteValue::Value {
                value: CanonicalValue::String(
                    CanonicalString::new("ada").expect("canonical response value"),
                ),
            }]],
        },
    )
    .expect("response")
    .encode_signed(
        &advertisement
            .fingerprint()
            .expect("advertisement fingerprint"),
        &TestSigner,
    )
    .expect("signed response");
    let decode = |bytes: &[u8]| {
        decode_remote_reply(
            bytes,
            NONCE,
            &plan_fingerprint,
            &request_fingerprint,
            &advertisement
                .fingerprint()
                .expect("advertisement fingerprint"),
            advertisement.reply_key(),
            RemoteReplyDecodeLimits {
                shape: RemoteOutcomeShape::Rows { width: 1 },
                max_bytes: u64::try_from(MAX_REMOTE_ENVELOPE_BYTES).expect("wire ceiling"),
                max_items: 100,
                max_collection_members: 100,
            },
            &TestSigner,
        )
    };
    let outer: Value = serde_json::from_slice(&response).expect("response JSON");
    let payload = outer["payload"].clone();

    let mut unknown = payload.clone();
    unknown["outcome"]["rows"][0][0]["value"]["unknown"] = Value::Null;
    let unknown = to_canonical_json(&unknown).expect("canonical unknown-field response");
    let unknown = sign_raw_payload(&response, &unknown);
    assert_eq!(
        decode(&unknown)
            .expect_err("unknown CanonicalValue members must not disappear during decode")
            .code()
            .as_str(),
        "query_remote_response_wire_mismatch",
    );

    let mut normalizing = payload;
    normalizing["outcome"]["rows"][0][0]["value"] =
        serde_json::json!({"kind": "decimal", "value": "+001.2300dec"});
    let normalizing =
        to_canonical_json(&normalizing).expect("canonical normalizing response payload");
    let normalizing = sign_raw_payload(&response, &normalizing);
    assert_eq!(
        decode(&normalizing)
            .expect_err("normalizing scalar spellings must not change response identity")
            .code()
            .as_str(),
        "query_remote_response_wire_mismatch",
    );
}

#[test]
fn remote_failures_require_nonce_and_whole_request_binding() {
    let plan = input_plan();
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("ada")])
        .expect("invocation");
    let request = RemoteQueryRequest::new(
        &plan,
        &invocation,
        &advertisement(),
        limits(),
        NONCE,
        NOW_MS,
    )
    .expect("request");
    let request_fingerprint =
        RemoteRequestFingerprint::compute(&request.encode().expect("request bytes"))
            .expect("request fingerprint");
    let diagnostic = Diagnostic::new(
        DiagnosticCategory::Integrity,
        DiagnosticCode::new("provider_unavailable").expect("diagnostic code"),
        "provider unavailable",
    );

    let unbound = RemoteQueryFailure::new(None, &diagnostic);
    assert_eq!(
        unbound
            .verify_binding(NONCE, &request_fingerprint)
            .expect_err("missing nonce and request are never request-bound")
            .code()
            .as_str(),
        "query_remote_failure_unbound",
    );
    let nonce_only = RemoteQueryFailure::new(Some(NONCE.to_owned()), &diagnostic);
    assert_eq!(
        nonce_only
            .verify_binding(NONCE, &request_fingerprint)
            .expect_err("a nonce alone does not bind an invocation")
            .code()
            .as_str(),
        "query_remote_failure_unbound",
    );
    RemoteQueryFailure::bound(NONCE, &request_fingerprint, &diagnostic)
        .verify_binding(NONCE, &request_fingerprint)
        .expect("fully bound failure verifies");
}

#[test]
fn signature_is_verified_before_binding_or_outcome_materialization() {
    let plan = input_plan();
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("ada")])
        .expect("invocation");
    let advertisement = advertisement();
    let request =
        RemoteQueryRequest::new(&plan, &invocation, &advertisement, limits(), NONCE, NOW_MS)
            .expect("request");
    let request_fingerprint =
        RemoteRequestFingerprint::compute(&request.encode().expect("request bytes"))
            .expect("request fingerprint");
    let plan_fingerprint = QueryPlanFingerprint::compute(&plan).expect("plan fingerprint");
    let response = RemoteQueryResponse::new(
        NONCE,
        &plan_fingerprint,
        &request_fingerprint,
        RemoteOutcome::Count { value: 1 },
    )
    .expect("response")
    .encode_signed(
        &advertisement
            .fingerprint()
            .expect("advertisement fingerprint"),
        &TestSigner,
    )
    .expect("response bytes");
    let mut forged: Value = serde_json::from_slice(&response).expect("response JSON");
    forged["payload"]["nonce"] = Value::String("foreign-nonce-0123456789abcdef".to_owned());
    forged["payload"]["outcome"] = Value::String("x".repeat(512 * 1024));
    let forged = serde_json::to_vec(&forged).expect("canonical forged signed response");

    assert_eq!(
        decode_count_reply(
            &forged,
            &plan_fingerprint,
            &request_fingerprint,
            &advertisement,
        )
        .expect_err("tampered payload must reject before binding or typed outcome decode")
        .code()
        .as_str(),
        "query_remote_signature_invalid",
    );

    let foreign = RemoteQueryResponse::new(
        "foreign-nonce-0123456789abcdef",
        &plan_fingerprint,
        &request_fingerprint,
        RemoteOutcome::Count { value: 1 },
    )
    .expect("foreign response")
    .encode_signed(
        &advertisement
            .fingerprint()
            .expect("advertisement fingerprint"),
        &TestSigner,
    )
    .expect("signed foreign response");
    assert_eq!(
        decode_count_reply(
            &foreign,
            &plan_fingerprint,
            &request_fingerprint,
            &advertisement,
        )
        .expect_err("an authenticated foreign binding remains rejected")
        .code()
        .as_str(),
        "query_remote_nonce_mismatch",
    );
}

#[test]
fn every_authenticated_outer_identity_field_is_signature_bound() {
    let plan = input_plan();
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("ada")])
        .expect("invocation");
    let advertisement = advertisement();
    let request =
        RemoteQueryRequest::new(&plan, &invocation, &advertisement, limits(), NONCE, NOW_MS)
            .expect("request");
    let request_fingerprint =
        RemoteRequestFingerprint::compute(&request.encode().expect("request bytes"))
            .expect("request fingerprint");
    let plan_fingerprint = QueryPlanFingerprint::compute(&plan).expect("plan fingerprint");
    let response = RemoteQueryResponse::new(
        NONCE,
        &plan_fingerprint,
        &request_fingerprint,
        RemoteOutcome::Count { value: 1 },
    )
    .expect("response")
    .encode_signed(
        &advertisement
            .fingerprint()
            .expect("advertisement fingerprint"),
        &TestSigner,
    )
    .expect("signed response");

    for field in ["advertisement", "key", "key_id", "signature"] {
        let mut forged: Value = serde_json::from_slice(&response).expect("response JSON");
        let value = forged[field].as_str().expect("signed string");
        let replacement = format!(
            "{}{}",
            if &value[..1] == "0" { "1" } else { "0" },
            &value[1..]
        );
        forged[field] = Value::String(replacement);
        let forged = serde_json::to_vec(&forged).expect("forged outer reply");
        assert_eq!(
            decode_count_reply(
                &forged,
                &plan_fingerprint,
                &request_fingerprint,
                &advertisement,
            )
            .expect_err("every fixed outer identity field is authenticated")
            .code()
            .as_str(),
            "query_remote_signature_invalid",
            "field {field}",
        );
    }
}

#[test]
fn caller_success_byte_budget_does_not_hide_authenticated_failures() {
    let plan = input_plan();
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("ada")])
        .expect("invocation");
    let advertisement = advertisement();
    let request =
        RemoteQueryRequest::new(&plan, &invocation, &advertisement, limits(), NONCE, NOW_MS)
            .expect("request");
    let request_fingerprint =
        RemoteRequestFingerprint::compute(&request.encode().expect("request bytes"))
            .expect("request fingerprint");
    let plan_fingerprint = QueryPlanFingerprint::compute(&plan).expect("plan fingerprint");
    let response = RemoteQueryResponse::new(
        NONCE,
        &plan_fingerprint,
        &request_fingerprint,
        RemoteOutcome::Count { value: 1 },
    )
    .expect("response")
    .encode_signed(
        &advertisement
            .fingerprint()
            .expect("advertisement fingerprint"),
        &TestSigner,
    )
    .expect("signed response");
    let verifier = CountingVerifier::default();
    let decode = |bytes: &[u8], max_bytes| {
        decode_remote_reply(
            bytes,
            NONCE,
            &plan_fingerprint,
            &request_fingerprint,
            &advertisement
                .fingerprint()
                .expect("advertisement fingerprint"),
            advertisement.reply_key(),
            RemoteReplyDecodeLimits {
                shape: RemoteOutcomeShape::Count,
                max_bytes,
                max_items: 100,
                max_collection_members: 100,
            },
            &verifier,
        )
    };
    assert_eq!(
        decode(
            &response,
            u64::try_from(response.len() - 1).expect("response length")
        )
        .expect_err("successful response exceeds caller data bytes")
        .code()
        .as_str(),
        "query_remote_response_oversized",
    );
    assert_eq!(
        verifier.calls(),
        1,
        "the signed reply kind is authenticated before applying the success-only budget",
    );

    let foreign_request = RemoteRequestFingerprint::compute(b"foreign remote request")
        .expect("foreign request fingerprint");
    let wrong_request = RemoteQueryResponse::new(
        NONCE,
        &plan_fingerprint,
        &foreign_request,
        RemoteOutcome::Count { value: 1 },
    )
    .expect("wrong-request response")
    .encode_signed(
        &advertisement
            .fingerprint()
            .expect("advertisement fingerprint"),
        &TestSigner,
    )
    .expect("signed wrong-request response");
    assert_eq!(
        decode(&wrong_request, 0)
            .expect_err("request correlation wins before the success-data budget")
            .code()
            .as_str(),
        "query_remote_request_mismatch",
    );
    assert_eq!(
        verifier.calls(),
        2,
        "trusted success evidence is correlated before applying its byte budget",
    );

    let mut forged: Value = serde_json::from_slice(&response).expect("response JSON");
    forged["signature"] = Value::String("00".repeat(64));
    let forged = serde_json::to_vec(&forged).expect("forged response");
    assert_eq!(
        decode(&forged, 0)
            .expect_err("forgery rejection wins before the success-data budget")
            .code()
            .as_str(),
        "query_remote_signature_invalid",
    );
    assert_eq!(
        verifier.calls(),
        3,
        "a forged reply cannot use a tiny budget to change authentication ordering",
    );

    let server_failure = Diagnostic::new(
        DiagnosticCategory::ResourceLimit,
        DiagnosticCode::new("query_remote_response_oversized").expect("diagnostic code"),
        "the typed response exceeds the effective byte budget",
    );
    let failure = RemoteQueryFailure::bound(NONCE, &request_fingerprint, &server_failure)
        .encode_signed(
            &advertisement
                .fingerprint()
                .expect("advertisement fingerprint"),
            &TestSigner,
        )
        .expect("signed bound failure");
    assert!(matches!(
        decode(&failure, 0).expect("zero success budget still admits a typed failure"),
        RemoteReply::Failure(_),
    ));
    assert_eq!(verifier.calls(), 4);

    assert_eq!(
        decode_signed_remote_failure(
            &response,
            &advertisement
                .fingerprint()
                .expect("advertisement fingerprint"),
            advertisement.reply_key(),
            u64::try_from(response.len() - 1).expect("response length"),
            &verifier,
        )
        .expect_err("uncorrelated failure decode shares the byte preflight")
        .code()
        .as_str(),
        "query_remote_response_oversized",
    );
    assert_eq!(
        verifier.calls(),
        4,
        "the explicit uncorrelated-failure budget still bypasses verification",
    );

    let over_wire_ceiling = vec![b'{'; MAX_REMOTE_ENVELOPE_BYTES + 1];
    assert_eq!(
        decode(&over_wire_ceiling, u64::MAX)
            .expect_err("the global wire ceiling is also checked before parsing")
            .code()
            .as_str(),
        "query_remote_envelope_too_large",
    );
    assert_eq!(
        verifier.calls(),
        4,
        "the global oversize path must also bypass the verifier"
    );
}

#[test]
fn shape_preflight_rejects_under_width_wrong_kind_and_malformed_documents() {
    let plan = input_plan();
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("ada")])
        .expect("invocation");
    let advertisement = advertisement();
    let request =
        RemoteQueryRequest::new(&plan, &invocation, &advertisement, limits(), NONCE, NOW_MS)
            .expect("request");
    let request_fingerprint =
        RemoteRequestFingerprint::compute(&request.encode().expect("request bytes"))
            .expect("request fingerprint");
    let plan_fingerprint = QueryPlanFingerprint::compute(&plan).expect("plan fingerprint");
    let sign = |outcome| {
        RemoteQueryResponse::new(NONCE, &plan_fingerprint, &request_fingerprint, outcome)
            .expect("response")
            .encode_signed(
                &advertisement
                    .fingerprint()
                    .expect("advertisement fingerprint"),
                &TestSigner,
            )
            .expect("signed response")
    };
    let decode_with_max_items = |bytes: &[u8], shape, max_items| {
        decode_remote_reply(
            bytes,
            NONCE,
            &plan_fingerprint,
            &request_fingerprint,
            &advertisement
                .fingerprint()
                .expect("advertisement fingerprint"),
            advertisement.reply_key(),
            RemoteReplyDecodeLimits {
                shape,
                max_bytes: u64::try_from(MAX_REMOTE_ENVELOPE_BYTES).expect("wire ceiling"),
                max_items,
                max_collection_members: 100,
            },
            &TestSigner,
        )
    };
    let decode = |bytes: &[u8], shape| decode_with_max_items(bytes, shape, 100);

    for (bytes, shape) in [
        (
            sign(RemoteOutcome::Rows { rows: vec![vec![]] }),
            RemoteOutcomeShape::Rows { width: 1 },
        ),
        (
            sign(RemoteOutcome::Documents {
                documents: vec![vec![]],
            }),
            RemoteOutcomeShape::Documents { width: 1 },
        ),
    ] {
        assert_eq!(
            decode(&bytes, shape)
                .expect_err("under-width evidence is rejected by the shape scan")
                .code()
                .as_str(),
            "query_remote_evidence_mismatch",
        );
    }

    let wrong_kind = sign(RemoteOutcome::Count { value: 1 });
    assert_eq!(
        decode(&wrong_kind, RemoteOutcomeShape::Rows { width: 1 })
            .expect_err("wrong outcome kind cannot bypass the shape scan")
            .code()
            .as_str(),
        "query_remote_outcome_mismatch",
    );

    let document = sign(RemoteOutcome::Documents {
        documents: vec![vec![RemoteFieldValue::List { values: vec![] }]],
    });
    let outer: Value = serde_json::from_slice(&document).expect("document outer");
    let payload = serde_json::to_string(&outer["payload"]).expect("document payload");
    let duplicated = payload.replace(
        "{\"kind\":\"list\",\"values\":[]}",
        "{\"kind\":\"list\",\"values\":[],\"values\":[]}",
    );
    assert_ne!(duplicated, payload, "fixture replacement must apply");
    let duplicated = sign_raw_payload(&document, duplicated.as_bytes());
    assert_eq!(
        decode(&duplicated, RemoteOutcomeShape::Documents { width: 1 })
            .expect_err("duplicate list payload is rejected before typed allocation")
            .code()
            .as_str(),
        "query_remote_evidence_mismatch",
    );

    for (name, malformed, expected) in [
        (
            "unknown document member",
            payload.replace(
                "{\"kind\":\"list\",\"values\":[]}",
                "{\"extra\":null,\"kind\":\"list\",\"values\":[]}",
            ),
            "query_remote_evidence_mismatch",
        ),
        (
            "duplicate document kind",
            payload.replace(
                "{\"kind\":\"list\",\"values\":[]}",
                "{\"kind\":\"list\",\"kind\":\"list\",\"values\":[]}",
            ),
            "query_remote_evidence_mismatch",
        ),
        (
            "unknown outcome member",
            payload.replace(
                "\"kind\":\"documents\"}",
                "\"extra\":null,\"kind\":\"documents\"}",
            ),
            "query_remote_outcome_mismatch",
        ),
        (
            "duplicate outcome member",
            payload.replace(
                "\"outcome\":{\"documents\":",
                "\"outcome\":{\"documents\":[],\"documents\":",
            ),
            "query_remote_outcome_mismatch",
        ),
    ] {
        assert_ne!(malformed, payload, "{name}: fixture replacement must apply");
        let malformed = sign_raw_payload(&document, malformed.as_bytes());
        assert_eq!(
            decode(&malformed, RemoteOutcomeShape::Documents { width: 1 })
                .expect_err(name)
                .code()
                .as_str(),
            expected,
            "{name}",
        );
    }

    let unknown_response_field = format!(
        "{},\"unknown\":null}}",
        payload.strip_suffix('}').expect("response payload object")
    );
    let duplicate_response_field =
        payload.replace(",\"request\":", ",\"request\":null,\"request\":");
    for (name, malformed, expected) in [
        (
            "unknown response field",
            unknown_response_field,
            "invalid_canonical_value",
        ),
        (
            "duplicate response field",
            duplicate_response_field,
            "query_remote_reply_malformed",
        ),
    ] {
        assert_ne!(malformed, payload, "{name}: fixture replacement must apply");
        let malformed = sign_raw_payload(&document, malformed.as_bytes());
        assert_eq!(
            decode(&malformed, RemoteOutcomeShape::Documents { width: 1 })
                .expect_err(name)
                .code()
                .as_str(),
            expected,
            "{name}",
        );
    }

    let count = sign(RemoteOutcome::Count { value: 1 });
    decode(&count, RemoteOutcomeShape::Count).expect("exact count shape");
    let count_at_limit = sign(RemoteOutcome::Count { value: 100 });
    decode(&count_at_limit, RemoteOutcomeShape::Count)
        .expect("a count equal to the caller item budget is accepted");
    let count_over_limit = sign(RemoteOutcome::Count { value: 101 });
    assert_eq!(
        decode(&count_over_limit, RemoteOutcomeShape::Count)
            .expect_err("a count above the caller item budget is rejected")
            .code()
            .as_str(),
        "query_remote_response_oversized",
    );
    let outer: Value = serde_json::from_slice(&count).expect("count outer");
    let count_payload = serde_json::to_string(&outer["payload"]).expect("count payload");
    for (name, malformed, expected) in [
        (
            "count value has the wrong scalar type",
            count_payload.replace("\"value\":1", "\"value\":true"),
            "query_remote_evidence_mismatch",
        ),
        (
            "count value is duplicated",
            count_payload.replace("\"value\":1", "\"value\":1,\"value\":1"),
            "query_remote_outcome_mismatch",
        ),
        (
            "count carries an unknown member",
            count_payload.replace("{\"kind\":\"count\"", "{\"extra\":null,\"kind\":\"count\""),
            "query_remote_outcome_mismatch",
        ),
        (
            "count omits its value",
            count_payload.replace(",\"value\":1", ""),
            "query_remote_outcome_mismatch",
        ),
        (
            "count claims the exists kind",
            count_payload.replace("\"kind\":\"count\"", "\"kind\":\"exists\""),
            "query_remote_outcome_mismatch",
        ),
    ] {
        assert_ne!(
            malformed, count_payload,
            "{name}: fixture replacement must apply"
        );
        let malformed = sign_raw_payload(&count, malformed.as_bytes());
        assert_eq!(
            decode(&malformed, RemoteOutcomeShape::Count)
                .expect_err(name)
                .code()
                .as_str(),
            expected,
            "{name}",
        );
    }

    let exists = sign(RemoteOutcome::Exists { value: true });
    decode(&exists, RemoteOutcomeShape::Exists).expect("exact exists shape");
    let absent = sign(RemoteOutcome::Exists { value: false });
    decode_with_max_items(&absent, RemoteOutcomeShape::Exists, 0)
        .expect("false existence evidence consumes no answer item");
    decode_with_max_items(&exists, RemoteOutcomeShape::Exists, 1)
        .expect("true existence evidence fits one answer item");
    assert_eq!(
        decode_with_max_items(&exists, RemoteOutcomeShape::Exists, 0)
            .expect_err("true existence evidence exceeds a zero-item budget")
            .code()
            .as_str(),
        "query_remote_response_oversized",
    );
    let outer: Value = serde_json::from_slice(&exists).expect("exists outer");
    let exists_payload = serde_json::to_string(&outer["payload"]).expect("exists payload");
    for (name, malformed, expected) in [
        (
            "exists value has the wrong scalar type",
            exists_payload.replace("\"value\":true", "\"value\":0"),
            "query_remote_evidence_mismatch",
        ),
        (
            "exists kind is duplicated",
            exists_payload.replace(
                "\"kind\":\"exists\"",
                "\"kind\":\"exists\",\"kind\":\"exists\"",
            ),
            "query_remote_outcome_mismatch",
        ),
    ] {
        assert_ne!(
            malformed, exists_payload,
            "{name}: fixture replacement must apply"
        );
        let malformed = sign_raw_payload(&exists, malformed.as_bytes());
        assert_eq!(
            decode(&malformed, RemoteOutcomeShape::Exists)
                .expect_err(name)
                .code()
                .as_str(),
            expected,
            "{name}",
        );
    }
}

#[test]
fn unencodable_failures_use_a_nonempty_signed_bound_fallback() {
    let plan = input_plan();
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("ada")])
        .expect("invocation");
    let advertisement = advertisement();
    let request =
        RemoteQueryRequest::new(&plan, &invocation, &advertisement, limits(), NONCE, NOW_MS)
            .expect("request");
    let request_fingerprint =
        RemoteRequestFingerprint::compute(&request.encode().expect("request bytes"))
            .expect("request fingerprint");
    let plan_fingerprint = QueryPlanFingerprint::compute(&plan).expect("plan fingerprint");
    let oversized = Diagnostic::new(
        DiagnosticCategory::Integrity,
        DiagnosticCode::new("provider_unavailable").expect("diagnostic code"),
        "x".repeat(type_bridge_contract::limits::MAX_CANONICAL_STRING_BYTES + 1),
    );
    let bytes = RemoteQueryFailure::bound(NONCE, &request_fingerprint, &oversized)
        .encode_signed_or_fallback(
            &advertisement
                .fingerprint()
                .expect("advertisement fingerprint"),
            &TestSigner,
        );
    assert!(!bytes.is_empty());
    let reply = decode_remote_reply(
        &bytes,
        NONCE,
        &plan_fingerprint,
        &request_fingerprint,
        &advertisement
            .fingerprint()
            .expect("advertisement fingerprint"),
        advertisement.reply_key(),
        RemoteReplyDecodeLimits {
            shape: RemoteOutcomeShape::Count,
            max_bytes: u64::try_from(MAX_REMOTE_ENVELOPE_BYTES).expect("wire ceiling"),
            max_items: 100,
            max_collection_members: 100,
        },
        &TestSigner,
    )
    .expect("fallback is authenticated and bound");
    let RemoteReply::Failure(failure) = reply else {
        panic!("fallback must remain a failure");
    };
    assert_eq!(
        failure.diagnostic().expect("diagnostic").code().as_str(),
        "query_remote_internal_failure",
    );
}

#[test]
fn bound_noncanonical_replies_keep_the_canonical_codec_diagnostic() {
    let plan = input_plan();
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("ada")])
        .expect("invocation");
    let advertisement = advertisement();
    let request =
        RemoteQueryRequest::new(&plan, &invocation, &advertisement, limits(), NONCE, NOW_MS)
            .expect("request");
    let request_fingerprint =
        RemoteRequestFingerprint::compute(&request.encode().expect("request bytes"))
            .expect("request fingerprint");
    let plan_fingerprint = QueryPlanFingerprint::compute(&plan).expect("plan fingerprint");
    let mut response = RemoteQueryResponse::new(
        NONCE,
        &plan_fingerprint,
        &request_fingerprint,
        RemoteOutcome::Count { value: 1 },
    )
    .expect("response")
    .encode_signed(
        &advertisement
            .fingerprint()
            .expect("advertisement fingerprint"),
        &TestSigner,
    )
    .expect("response bytes");
    response.push(b'\n');

    assert_eq!(
        decode_count_reply(
            &response,
            &plan_fingerprint,
            &request_fingerprint,
            &advertisement,
        )
        .expect_err("bound non-canonical bytes must reach the canonical decoder")
        .code()
        .as_str(),
        "non_canonical_json",
    );
}

#[test]
fn string_embedded_plans_are_rejected() {
    let plan = input_plan();
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("ada")])
        .expect("invocation");
    let bytes = RemoteQueryRequest::new(
        &plan,
        &invocation,
        &advertisement(),
        limits(),
        NONCE,
        NOW_MS,
    )
    .expect("request envelope")
    .encode()
    .expect("request bytes");

    // Re-embed the plan in the retired string form and re-encode
    // canonically; the envelope decodes but the plan accessor must
    // fail closed instead of accepting the old wire shape.
    let mut envelope: Value = serde_json::from_slice(&bytes).expect("envelope JSON");
    let plan_text = serde_json::to_string(&envelope["plan"]).expect("plan text");
    envelope["plan"] = Value::String(plan_text);
    let stringified = serde_json::to_vec(&envelope).expect("stringified envelope");

    let decoded = RemoteQueryRequest::decode(&stringified).expect("envelope still decodes");
    assert!(decoded.plan().is_err(), "string plan must not decode");
}

#[test]
fn request_binds_exact_executor_epoch_and_absolute_expiry() {
    let plan = input_plan();
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("ada")])
        .expect("invocation");
    let mut bounded = limits();
    bounded.deadline_ms = Some(30_000);
    let advertisement = advertisement();
    let request =
        RemoteQueryRequest::new(&plan, &invocation, &advertisement, bounded, NONCE, NOW_MS)
            .expect("request");

    assert!(
        request
            .binds_advertisement(&advertisement)
            .expect("binding")
    );
    let restarted = RemoteCapabilities::new(
        query_plan_capability_vocabulary(),
        executor("contract-executor-identity", "contract-restarted-epoch"),
        TestSigner.public_key(),
    );
    assert!(!request.binds_advertisement(&restarted).expect("binding"));
    assert_eq!(request.expires_at_unix_ms(), NOW_MS + 30_000);
    assert_eq!(
        request
            .remaining_lifetime_ms(NOW_MS + 10_000)
            .expect("remaining"),
        20_000,
    );
    assert_eq!(
        request
            .remaining_lifetime_ms(NOW_MS + 30_000)
            .expect_err("expiry is exclusive")
            .code()
            .as_str(),
        "query_remote_request_expired",
    );
}

#[test]
fn absolute_expiry_must_equal_the_declared_or_default_deadline() {
    let plan = input_plan();
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("ada")])
        .expect("invocation");
    let default_request = RemoteQueryRequest::new(
        &plan,
        &invocation,
        &advertisement(),
        limits(),
        NONCE,
        NOW_MS,
    )
    .expect("default-bounded request");
    assert_eq!(
        default_request.expires_at_unix_ms(),
        NOW_MS + DEFAULT_REMOTE_DEADLINE_MS,
        "an omitted deadline has a short deterministic replay lifetime",
    );

    let default_bytes = default_request.encode().expect("default request bytes");
    RemoteQueryRequest::decode(&default_bytes).expect("matching default timestamps decode");

    let mut forged_declared: Value = serde_json::from_slice(&default_bytes).expect("request JSON");
    forged_declared["limits"]["deadline_ms"] = Value::from(1_u64);
    let forged_declared = serde_json::to_vec(&forged_declared).expect("forged request bytes");
    assert_eq!(
        RemoteQueryRequest::decode(&forged_declared)
            .expect_err("a one-millisecond declaration cannot retain the default expiry")
            .code()
            .as_str(),
        "query_remote_time_invalid",
    );

    let mut forged_default: Value = serde_json::from_slice(&default_bytes).expect("request JSON");
    forged_default["expires_at_unix_ms"] = Value::from(NOW_MS + MAX_REMOTE_DEADLINE_MS);
    let forged_default = serde_json::to_vec(&forged_default).expect("forged request bytes");
    assert_eq!(
        RemoteQueryRequest::decode(&forged_default)
            .expect_err("an omitted deadline cannot claim the protocol maximum")
            .code()
            .as_str(),
        "query_remote_time_invalid",
    );
}

#[test]
fn malformed_future_and_expired_times_have_stable_rejections() {
    let plan = input_plan();
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("ada")])
        .expect("invocation");
    let mut bounded = limits();
    bounded.deadline_ms = Some(30_000);
    let bytes =
        RemoteQueryRequest::new(&plan, &invocation, &advertisement(), bounded, NONCE, NOW_MS)
            .expect("request")
            .encode()
            .expect("request bytes");

    let future = RemoteQueryRequest::decode(&bytes).expect("request");
    assert!(
        future
            .remaining_lifetime_ms(NOW_MS - MAX_REMOTE_CLOCK_SKEW_MS)
            .is_ok(),
        "the documented positive skew boundary is inclusive",
    );
    assert_eq!(
        future
            .remaining_lifetime_ms(NOW_MS - MAX_REMOTE_CLOCK_SKEW_MS)
            .expect("absolute replay horizon"),
        MAX_REMOTE_CLOCK_SKEW_MS + 30_000,
    );
    assert_eq!(
        future
            .remaining_execution_ms(NOW_MS - MAX_REMOTE_CLOCK_SKEW_MS)
            .expect("execution horizon"),
        30_000,
        "clock skew must not grant execution beyond the declared duration",
    );
    assert_eq!(
        future
            .remaining_lifetime_ms(NOW_MS - MAX_REMOTE_CLOCK_SKEW_MS - 1)
            .expect_err("future request")
            .code()
            .as_str(),
        "query_remote_time_future",
    );

    let mut malformed: Value = serde_json::from_slice(&bytes).expect("request JSON");
    malformed["expires_at_unix_ms"] = Value::from(NOW_MS - 1);
    let malformed = serde_json::to_vec(&malformed).expect("canonical JSON");
    assert_eq!(
        RemoteQueryRequest::decode(&malformed)
            .expect_err("expiry before preparation")
            .code()
            .as_str(),
        "query_remote_time_invalid",
    );

    let mut overlong: Value = serde_json::from_slice(&bytes).expect("request JSON");
    overlong["expires_at_unix_ms"] = Value::from(NOW_MS + MAX_REMOTE_DEADLINE_MS + 1);
    let overlong = serde_json::to_vec(&overlong).expect("canonical JSON");
    assert_eq!(
        RemoteQueryRequest::decode(&overlong)
            .expect_err("overlong lifetime")
            .code()
            .as_str(),
        "query_remote_time_invalid",
    );
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
        query_plan_capability_vocabulary(),
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
fn multi_row_invocations_require_the_given_transport_capability() {
    let plan = input_plan();
    let empty = QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("ada")])
        .expect("single-row invocation");
    assert!(empty.transport_capabilities().is_empty());

    let multi = QueryInvocation::new(
        &plan,
        QueryOperation::Rows,
        vec![string_row("ada"), string_row("bob")],
    )
    .expect("multi-row invocation");
    let capabilities = multi.transport_capabilities();
    assert_eq!(capabilities.len(), 1);
    assert!(capabilities.contains(&query_given_rows_capability()));
    // Plans never require the transport capability themselves, so it is
    // advertised separately from the plan vocabulary.
    assert!(!query_plan_capability_vocabulary().contains(&query_given_rows_capability()));
}

#[test]
fn single_row_optional_absence_requires_the_given_transport_capability() {
    let source = input_plan();
    let plan = QueryPlan::new(
        source.bindings().to_vec(),
        vec![InputColumn::new(
            InputColumnId::new(0),
            QueryVariable::new("wanted_name").expect("input name"),
            ValueTypeTag::String,
            true,
        )],
        source.pipeline().to_vec(),
        source.output().clone(),
        source.managed_semantics().clone(),
    )
    .expect("optional input plan");

    let present = QueryInvocation::new(&plan, QueryOperation::Rows, vec![string_row("ada")])
        .expect("present optional value");
    assert!(present.transport_capabilities().is_empty());

    let absent = QueryInvocation::new(&plan, QueryOperation::Rows, vec![InputRow::new(vec![None])])
        .expect("absent optional value");
    let capabilities = absent.transport_capabilities();
    assert_eq!(capabilities.len(), 1);
    assert!(capabilities.contains(&query_given_rows_capability()));
}

#[test]
fn single_row_datetime_tz_requires_the_exact_given_transport_capability() {
    let source = input_plan();
    let plan = QueryPlan::new(
        source.bindings().to_vec(),
        vec![InputColumn::new(
            InputColumnId::new(0),
            QueryVariable::new("zoned").expect("input name"),
            ValueTypeTag::DateTimeTz,
            false,
        )],
        source.pipeline().to_vec(),
        source.output().clone(),
        source.managed_semantics().clone(),
    )
    .expect("datetime-tz input plan");
    let value = CanonicalDateTimeTz::new_fixed(
        "1900-01-01T12:00:00".parse().expect("local datetime"),
        TimeZoneDesignator::OffsetSeconds(1_172),
    )
    .expect("second-resolution fixed offset");
    let invocation = QueryInvocation::new(
        &plan,
        QueryOperation::Rows,
        vec![InputRow::new(vec![Some(CanonicalValue::DateTimeTz(value))])],
    )
    .expect("single-row datetime-tz invocation");
    assert_eq!(invocation.transport_capabilities().len(), 1);
    assert!(
        invocation
            .transport_capabilities()
            .contains(&query_given_rows_capability())
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
