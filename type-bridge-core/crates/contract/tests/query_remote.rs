#[path = "support/plan_examples.rs"]
#[allow(dead_code)]
mod examples;
#[path = "support/plan.rs"]
#[allow(dead_code)]
mod support;

use std::collections::BTreeMap;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde_json::{Value, json};
use type_bridge_contract::capability::CapabilityId;
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::limits::QueryLimits;
use type_bridge_contract::query_plan::{Operation, QueryPlan, query_plan_capability_vocabulary};
use type_bridge_contract::query_remote::*;
use type_bridge_contract::sdk_diagnostic::{
    SdkCommitFailureOutcome, SdkDiagnosticCode, SdkExecutionDiagnostic, SdkQueryDiagnosticCategory,
};

struct Keys(SigningKey);
impl Keys {
    fn new() -> Self {
        Self(SigningKey::from_bytes(&[7; 32]))
    }
}
impl RemoteReplySigner for Keys {
    fn public_key(&self) -> RemoteSigningPublicKey {
        RemoteSigningPublicKey::from_bytes(self.0.verifying_key().to_bytes())
    }
    fn sign(&self, digest: &RemoteReplySigningDigest) -> RemoteReplySignature {
        RemoteReplySignature::from_bytes(self.0.sign(digest.as_bytes()).to_bytes())
    }
}
impl RemoteReplyVerifier for Keys {
    fn verify(
        &self,
        key: RemoteSigningPublicKey,
        digest: &RemoteReplySigningDigest,
        signature: &RemoteReplySignature,
    ) -> bool {
        VerifyingKey::from_bytes(key.as_bytes()).is_ok_and(|key| {
            key.verify_strict(
                digest.as_bytes(),
                &Signature::from_bytes(signature.as_bytes()),
            )
            .is_ok()
        })
    }
}
fn advertisement() -> RemoteCapabilities {
    // Freeze the advertisement used by the released signed fixtures.
    let mut caps: type_bridge_contract::capability::CapabilitySet =
        query_plan_capability_vocabulary()
            .into_iter()
            .filter(|cap| {
                !matches!(
                    cap.as_str(),
                    "query.write.delete-snapshot"
                        | "query.pipeline.rematch"
                        | "query.expr.iid"
                        | "query.input.wide-strings"
                        | "query.fetch.scalar-fields"
                        | "query.fetch.role-lists"
                        | "query.pipeline.write-projection"
                        | "query.pattern.nested-iid"
                        | "query.pattern.negated-given-iid"
                        | "query.write.put-subtypes"
                        | "query.write.optional-attach"
                        | "query.write.attach-key"
                )
            })
            .collect();
    for cap in [CAP_QUERY_REMOTE_STAGED, CAP_QUERY_REMOTE_WRITE] {
        caps.insert(CapabilityId::new(cap).unwrap());
    }
    RemoteCapabilities::new(
        caps,
        RemoteExecutorBinding::new("fixture-executor-0001", "fixture-epoch-0000001").unwrap(),
        Keys::new().public_key(),
    )
    .with_query_limits(type_bridge_contract::limits::QueryLimits::CANONICAL)
    .unwrap()
}
fn entry(plan: QueryPlan) -> StagedPlan {
    let rows = plan.given_rows().unwrap_or(&[]).to_vec();
    StagedPlan::new(&plan, rows, &support::schema()).unwrap()
}
fn spec() -> StagedRequestSpec {
    StagedRequestSpec {
        nonce: "fixture-nonce-0000000000000001".into(),
        kind: None,
        limits: StagedLimits::default(),
        plans: vec![
            entry(examples::purchase()),
            entry(examples::batch()),
            entry(
                examples::expression_plan(type_bridge_contract::query_plan::Expr::Operand(
                    support::literal(2),
                ))
                .with_operation(Operation::Count, &support::schema())
                .unwrap(),
            ),
        ],
        prepared_at_unix_ms: 1_000_000,
    }
}
fn request() -> StagedRequest {
    StagedRequest::new(spec(), &support::schema(), &advertisement()).unwrap()
}
fn answers() -> Vec<StagedAnswer> {
    vec![
        StagedAnswer::Rows {
            rows: vec![vec![RemoteValue::Thing {
                iid: "0x01".into(),
                type_id: type_bridge_contract::id::TypeId::new(
                    type_bridge_contract::id::TypeKind::Relation,
                    "purchase",
                )
                .unwrap(),
            }]],
        },
        StagedAnswer::Matched { value: 2 },
        StagedAnswer::Count { value: 1 },
    ]
}
fn mutate_request(
    f: impl FnOnce(&mut Value),
) -> Result<StagedRequest, type_bridge_contract::diagnostic::Diagnostic> {
    let mut value: Value = serde_json::from_slice(&request().encode().unwrap()).unwrap();
    f(&mut value);
    StagedRequest::decode(
        &to_canonical_json(&value).unwrap(),
        &support::schema(),
        &advertisement(),
    )
}

#[test]
fn request_round_trip_lifetime_kind_and_fingerprint_bind_every_field() {
    let request = request();
    let bytes = request.encode().unwrap();
    assert_eq!(
        StagedRequest::decode(&bytes, &support::schema(), &advertisement()).unwrap(),
        request
    );
    assert_eq!(request.kind(), StagedKind::Write);
    assert_eq!(request.plans().len(), 3);
    assert!(request.validate_at(1_000_000).is_ok());
    assert!(request.validate_at(1_060_000).is_err());
    assert!(request.validate_at(900_000).is_err());
    for field in ["nonce", "kind", "prepared_at_unix_ms"] {
        let changed = mutate_request(|v| match field {
            "nonce" => v[field] = json!("another-nonce-00000000001"),
            "kind" => v[field] = json!("write"),
            _ => {
                v[field] = json!(1_000_001);
                v["expires_at_unix_ms"] = json!(1_060_001);
            }
        })
        .unwrap();
        assert_ne!(
            changed.fingerprint().unwrap(),
            request.fingerprint().unwrap()
        );
    }
}
#[test]
fn real_ed25519_sign_verify_and_all_or_nothing_outcomes() {
    let request = request();
    let ad = advertisement();
    let keys = Keys::new();
    for outcome in [
        StagedOutcome::Committed,
        StagedOutcome::Failed {
            diagnostic: SdkExecutionDiagnostic::commit_failure(SdkCommitFailureOutcome::Unknown),
        },
        StagedOutcome::Failed {
            diagnostic: SdkExecutionDiagnostic::query_failure(
                SdkQueryDiagnosticCategory::Cardinality,
                SdkDiagnosticCode::new("query_rejected_cardinality").unwrap(),
            ),
        },
    ] {
        let committed = matches!(outcome, StagedOutcome::Committed);
        let reply = StagedReply::new(
            &request,
            outcome,
            if committed { answers() } else { vec![] },
        )
        .unwrap();
        let bytes = reply.encode_signed(&request, &ad, &keys).unwrap();
        let decoded =
            VerifiedStagedReply::decode(&bytes, &request, &ad, keys.public_key(), &keys).unwrap();
        assert_eq!(decoded.committed(), committed);
        assert_eq!(
            decoded.encode_payload().unwrap(),
            reply.encode_payload().unwrap()
        );
        assert_eq!(decoded.diagnostic().is_some(), !committed);
    }
    assert!(
        StagedReply::new(
            &request,
            StagedOutcome::Failed {
                diagnostic: SdkExecutionDiagnostic::commit_failure(
                    SdkCommitFailureOutcome::Unknown
                )
            },
            answers()
        )
        .is_err()
    );
}
#[test]
fn tampering_wrong_key_and_foreign_request_fail_before_projection() {
    let request = request();
    let ad = advertisement();
    let keys = Keys::new();
    let reply = StagedReply::new(&request, StagedOutcome::Committed, answers()).unwrap();
    let bytes = reply.encode_signed(&request, &ad, &keys).unwrap();
    let mut changed: Value = serde_json::from_slice(&bytes).unwrap();
    changed["payload"]["outcome"] =
        json!({"kind":"failed","diagnostic":{"message":"SECRET_SENTINEL"}});
    let error = VerifiedStagedReply::decode(
        &to_canonical_json(&changed).unwrap(),
        &request,
        &ad,
        keys.public_key(),
        &keys,
    )
    .unwrap_err();
    assert_eq!(error.code().as_str(), "query_remote_signature_invalid");
    assert!(!format!("{error:?}").contains("SECRET_SENTINEL"));
    for (outer, field, value) in [
        (false, "request", json!("00".repeat(32))),
        (false, "nonce", json!("changed-nonce")),
        (false, "answers", json!([])),
        (true, "advertisement", json!("00".repeat(32))),
    ] {
        let mut changed: Value = serde_json::from_slice(&bytes).unwrap();
        if outer {
            changed[field] = value;
        } else {
            changed["payload"][field] = value;
        }
        assert!(
            VerifiedStagedReply::decode(
                &to_canonical_json(&changed).unwrap(),
                &request,
                &ad,
                keys.public_key(),
                &keys,
            )
            .is_err(),
            "tampered {field} was accepted"
        );
    }

    assert!(
        VerifiedStagedReply::decode(
            &bytes,
            &request,
            &ad,
            RemoteSigningPublicKey::from_bytes([0; 32]),
            &keys
        )
        .is_err()
    );
    let foreign = mutate_request(|v| v["nonce"] = json!("another-nonce-00000000001")).unwrap();
    assert!(VerifiedStagedReply::decode(&bytes, &foreign, &ad, keys.public_key(), &keys).is_err());
}
#[test]
fn request_refuses_unknown_fields_bad_times_kinds_and_keys() {
    for change in [
        json!({"extra":true}),
        json!({"format":"typebridge.query-remote-request/v2"}),
        json!({"nonce":"SECRET_SENTINEL!"}),
        json!({"expires_at_unix_ms":1_500_000}),
        json!({"kind":"read"}),
        json!({"kind":"schema"}),
        json!({"plans":[]}),
    ] {
        let error = mutate_request(|v| {
            v.as_object_mut()
                .unwrap()
                .extend(change.as_object().unwrap().clone())
        })
        .unwrap_err();
        assert!(!format!("{error:?}").contains("SECRET_SENTINEL"));
    }
    let mut bytes = request().encode().unwrap();
    bytes.push(b'\n');
    assert!(StagedRequest::decode(&bytes, &support::schema(), &advertisement()).is_err());
}
#[test]
fn plan_count_and_separate_statement_limits_are_enforced() {
    let base = entry(examples::batch());
    for count in [64, 65] {
        let mut spec = spec();
        spec.plans = vec![base.clone(); count];
        let result = StagedRequest::new(spec, &support::schema(), &advertisement());
        assert_eq!(result.is_ok(), count == 64);
    }
    for field in [
        "max_staged_plans",
        "max_write_statements",
        "max_read_statements",
    ] {
        let result = mutate_request(|v| v["limits"][field] = json!(1));
        assert_eq!(result.is_ok(), field == "max_read_statements");
    }
    for field in [
        "max_staged_plans",
        "max_write_statements",
        "max_read_statements",
        "max_bytes",
        "max_items",
        "deadline_ms",
    ] {
        assert!(mutate_request(|v| v["limits"][field] = json!(0)).is_err());
    }
}
#[test]
fn embedded_plan_rows_and_receiver_limits_are_revalidated() {
    assert!(mutate_request(|v| v["plans"][0]["plan_bytes"] = json!("7b7d")).is_err());
    assert!(
        mutate_request(|v| v["plans"][0]["rows"][0][0] = json!({"kind":"long","value":"42"}))
            .is_err()
    );
    let mut limits = QueryLimits::CANONICAL;
    limits.structural.bindings = 1;
    assert!(
        StagedRequest::decode_with_limits(
            &request().encode().unwrap(),
            &support::schema(),
            &advertisement(),
            limits
        )
        .is_err()
    );
    let mut spec = spec();
    spec.plans[0] = entry(examples::expression_plan(
        type_bridge_contract::query_plan::Expr::Operand(support::literal(1)),
    ));
    let write = StagedRequest::new(spec, &support::schema(), &advertisement()).unwrap();
    assert_eq!(write.kind(), StagedKind::Write);
}
#[test]
fn capability_admission_and_aggregate_answer_budgets_fail_closed() {
    let ad = RemoteCapabilities::new(
        query_plan_capability_vocabulary(),
        advertisement().executor().clone(),
        Keys::new().public_key(),
    );
    assert!(StagedRequest::new(spec(), &support::schema(), &ad).is_err());
    let mut spec = spec();
    spec.limits.max_items = 2;
    let request = StagedRequest::new(spec, &support::schema(), &advertisement()).unwrap();
    let refusal = StagedReply::new(&request, StagedOutcome::Committed, answers()).unwrap_err();
    assert_eq!(refusal.code().as_str(), "query_remote_answer_items");
    assert_eq!(
        refusal.details()["maximum"],
        type_bridge_contract::diagnostic::DiagnosticDetailValue::Long(2)
    );
    assert_eq!(
        refusal.details()["actual"],
        type_bridge_contract::diagnostic::DiagnosticDetailValue::Long(3)
    );
    let mut spec = crate::spec();
    spec.limits.max_bytes = 1;
    let request = StagedRequest::new(spec, &support::schema(), &advertisement()).unwrap();
    assert!(
        StagedReply::new(&request, StagedOutcome::Committed, answers())
            .unwrap()
            .encode_signed(&request, &advertisement(), &Keys::new())
            .is_err()
    );
    let failed = StagedReply::new(
        &request,
        StagedOutcome::Failed {
            diagnostic: SdkExecutionDiagnostic::commit_failure(SdkCommitFailureOutcome::Unknown),
        },
        vec![],
    )
    .unwrap();
    assert!(
        failed
            .encode_signed(&request, &advertisement(), &Keys::new())
            .is_ok()
    );
}
#[test]
fn answer_count_shape_kind_and_instance_evidence_bind_the_stack() {
    let request = request();
    for answers in [
        vec![],
        vec![StagedAnswer::Count { value: 1 }; 3],
        vec![
            StagedAnswer::Rows { rows: vec![vec![]] },
            StagedAnswer::Matched { value: 2 },
            StagedAnswer::Count { value: 1 },
        ],
        vec![
            StagedAnswer::Rows {
                rows: vec![vec![RemoteValue::Thing {
                    iid: "SECRET_SENTINEL".into(),
                    type_id: support::entity("account"),
                }]],
            },
            StagedAnswer::Matched { value: 2 },
            StagedAnswer::Count { value: 1 },
        ],
    ] {
        let error = StagedReply::new(&request, StagedOutcome::Committed, answers).unwrap_err();
        assert!(!format!("{error:?}").contains("SECRET_SENTINEL"));
    }
}
#[test]
fn staged_envelope_goldens_are_exact_and_sdk_readable() {
    let request = request();
    let ad = advertisement();
    let keys = Keys::new();
    let success = StagedReply::new(&request, StagedOutcome::Committed, answers()).unwrap();
    let failure = StagedReply::new(
        &request,
        StagedOutcome::Failed {
            diagnostic: SdkExecutionDiagnostic::commit_failure(SdkCommitFailureOutcome::Unknown),
        },
        vec![],
    )
    .unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/query-remote");
    let mut fixtures = std::collections::BTreeMap::from([
        ("advertisement.json", ad.encode().unwrap()),
        ("request.json", request.encode().unwrap()),
        (
            "reply-committed.json",
            success.encode_signed(&request, &ad, &keys).unwrap(),
        ),
        (
            "reply-failed.json",
            failure.encode_signed(&request, &ad, &keys).unwrap(),
        ),
    ]);
    fixtures.insert(
        "request-fingerprint.json",
        to_canonical_json(&request.fingerprint().unwrap()).unwrap(),
    );
    let mut document_spec = spec();
    document_spec.plans = vec![
        entry(examples::fetch_written(false)),
        entry(examples::fetch_written(true)),
    ];
    let document_request = StagedRequest::new(document_spec, &support::schema(), &ad).unwrap();
    let (_, entities) = fetch_documents(false);
    let (_, relations) = fetch_documents(true);
    let documents = StagedReply::new(
        &document_request,
        StagedOutcome::Committed,
        vec![
            StagedAnswer::Documents {
                documents: entities,
            },
            StagedAnswer::Documents {
                documents: relations,
            },
        ],
    )
    .unwrap();
    fixtures.insert("request-documents.json", document_request.encode().unwrap());
    fixtures.insert(
        "reply-documents.json",
        documents
            .encode_signed(&document_request, &ad, &keys)
            .unwrap(),
    );
    if std::env::var_os("TYPE_BRIDGE_UPDATE_REMOTE_FIXTURES").is_some() {
        std::fs::create_dir_all(&root).unwrap();
    }
    for (name, bytes) in fixtures {
        if std::env::var_os("TYPE_BRIDGE_UPDATE_REMOTE_FIXTURES").is_some() {
            std::fs::write(root.join(name), &bytes).unwrap();
        }
        assert_eq!(std::fs::read(root.join(name)).unwrap(), bytes);
    }
}

#[test]
fn authenticated_payload_preflights_budgets_before_owned_evidence() {
    struct Accept;
    impl RemoteReplyVerifier for Accept {
        fn verify(
            &self,
            _: RemoteSigningPublicKey,
            _: &RemoteReplySigningDigest,
            _: &RemoteReplySignature,
        ) -> bool {
            true
        }
    }
    let mut spec = spec();
    spec.limits.max_bytes = 1;
    let tight = StagedRequest::new(spec, &support::schema(), &advertisement()).unwrap();
    let normal = request();
    let signed = StagedReply::new(&normal, StagedOutcome::Committed, answers())
        .unwrap()
        .encode_signed(&normal, &advertisement(), &Keys::new())
        .unwrap();
    let mut wire: Value = serde_json::from_slice(&signed).unwrap();
    // The injected verifier isolates budget ordering; genuine Ed25519 is
    // covered above. Malformed owned evidence must not be interpreted first.
    wire["payload"]["request"] = json!(tight.fingerprint().unwrap().digest().to_hex());
    wire["payload"]["answers"] = json!("not-an-answer-array");
    let error = VerifiedStagedReply::decode(
        &to_canonical_json(&wire).unwrap(),
        &tight,
        &advertisement(),
        Keys::new().public_key(),
        &Accept,
    )
    .unwrap_err();
    assert_eq!(error.code().as_str(), "query_remote_reply_bytes");
    wire["payload"]["outcome"] =
        json!({"kind":"failed","diagnostic":{"message":"x".repeat(65_537)}});
    wire["payload"]["answers"] = json!([]);
    let error = VerifiedStagedReply::decode(
        &to_canonical_json(&wire).unwrap(),
        &tight,
        &advertisement(),
        Keys::new().public_key(),
        &Accept,
    )
    .unwrap_err();
    assert_eq!(error.code().as_str(), "query_remote_diagnostic_limit");
    wire["payload"]["outcome"] = json!({"kind":"failed"});
    let error = VerifiedStagedReply::decode(
        &to_canonical_json(&wire).unwrap(),
        &tight,
        &advertisement(),
        Keys::new().public_key(),
        &Accept,
    )
    .unwrap_err();
    assert_eq!(error.code().as_str(), "query_remote_reply_shape");
}

#[test]
fn read_kind_and_template_rows_remain_explicit() {
    let mut spec = spec();
    let read = spec.plans.pop().unwrap();
    spec.plans = vec![read];
    let read = StagedRequest::new(spec, &support::schema(), &advertisement()).unwrap();
    assert_eq!(read.kind(), StagedKind::Read);
    let bytes = read.encode().unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(value.get("kind").is_none());
    let batch = examples::batch();
    let rows = batch.given_rows().unwrap().to_vec();
    let mut wire: Value = serde_json::from_slice(&batch.canonical_bytes().unwrap()).unwrap();
    wire.as_object_mut().unwrap().remove("given_rows");
    let template = type_bridge_contract::query_plan::decode_query_plan(
        &to_canonical_json(&wire).unwrap(),
        &support::schema(),
    )
    .unwrap();
    assert!(StagedPlan::new(&template, vec![], &support::schema()).is_err());
    let bound = StagedPlan::new(&template, rows.clone(), &support::schema()).unwrap();
    assert!(bound.plan().given_rows().is_none());
    assert_eq!(
        bound.bound_plan(&support::schema()).unwrap().given_rows(),
        Some(rows.as_slice())
    );
}

#[test]
fn opaque_plan_bytes_do_not_inherit_the_scalar_string_ceiling() {
    use type_bridge_contract::query_plan::Expr;
    use type_bridge_contract::query_plan::QueryOperand;
    use type_bridge_contract::value::{CanonicalString, CanonicalValue};
    let large = examples::expression_plan(Expr::Operand(QueryOperand::Literal {
        value: CanonicalValue::String(
            CanonicalString::new(
                "x".repeat(type_bridge_contract::limits::MAX_CANONICAL_STRING_BYTES),
            )
            .unwrap(),
        ),
    }));
    let mut spec = spec();
    spec.plans = vec![entry(large)];
    let request = StagedRequest::new(spec, &support::schema(), &advertisement()).unwrap();
    let bytes = request.encode().unwrap();
    assert!(bytes.len() > 2 * type_bridge_contract::limits::MAX_CANONICAL_STRING_BYTES);
    assert_eq!(
        StagedRequest::decode(&bytes, &support::schema(), &advertisement()).unwrap(),
        request
    );
    let mut tight = QueryLimits::CANONICAL;
    tight.codec.max_string_bytes = 512 * 1024;
    assert!(
        StagedRequest::decode_with_limits(&bytes, &support::schema(), &advertisement(), tight)
            .is_err()
    );
}

#[test]
fn staged_requests_admit_fetch_and_attach_capabilities() {
    for plan in [examples::fetch_written(false), examples::attach()] {
        let mut request = spec();
        request.plans = vec![entry(plan)];
        StagedRequest::new(request, &support::schema(), &advertisement()).unwrap();
    }
    let mut unknown = type_bridge_contract::capability::CapabilitySet::new();
    unknown.insert(CapabilityId::new("query.fetch.future").unwrap());
    assert_eq!(
        unknown
            .ensure_supported_by(advertisement().capabilities())
            .unwrap_err()
            .code()
            .as_str(),
        "unsupported_required_capability"
    );
}

fn fetch_documents(relation: bool) -> (StagedRequest, Vec<BTreeMap<String, RemoteFieldValue>>) {
    let mut spec = spec();
    spec.plans = vec![entry(examples::fetch_written(relation))];
    let request = StagedRequest::new(spec, &support::schema(), &advertisement()).unwrap();
    let document = if relation {
        json!({
            "_iid": {"kind":"scalar","value":{"kind":"string","value":"0x02"}},
            "_type": {"kind":"scalar","value":{"kind":"string","value":"membership"}},
            "attributes": {"kind":"document","fields":{}},
            "_role_0_iid": {"kind":"scalar","value":{"kind":"string","value":"0x01"}},
            "_role_0_type": {"kind":"scalar","value":{"kind":"string","value":"account"}},
            "_role_0_attributes": {"kind":"document","fields":{
                "id":{"kind":"scalar","value":{"kind":"string","value":"alice"}}
            }}
        })
    } else {
        json!({
            "_iid": {"kind":"scalar","value":{"kind":"string","value":"0x01"}},
            "_type": {"kind":"scalar","value":{"kind":"string","value":"account"}},
            "attributes": {"kind":"document","fields":{
                "id":{"kind":"scalar","value":{"kind":"string","value":"account-b"}},
                "score":{"kind":"scalar","value":{"kind":"long","value":"1"}}
            }}
        })
    };
    (request, vec![serde_json::from_value(document).unwrap()])
}

#[test]
fn fetch_documents_validate_before_signing_and_after_signature_verification() {
    for relation in [false, true] {
        let (request, documents) = fetch_documents(relation);
        let reply = StagedReply::new(
            &request,
            StagedOutcome::Committed,
            vec![StagedAnswer::Documents { documents }],
        )
        .unwrap();
        let keys = Keys::new();
        let bytes = reply
            .encode_signed(&request, &advertisement(), &keys)
            .unwrap();
        VerifiedStagedReply::decode(&bytes, &request, &advertisement(), keys.public_key(), &keys)
            .unwrap();
    }
}

#[test]
fn fetch_document_refusals_name_the_invalid_evidence() {
    for (field, replacement, code) in [
        ("extra", json!({"kind":"absent"}), "key"),
        (
            "_type",
            json!({"kind":"scalar","value":{"kind":"string","value":"other"}}),
            "type",
        ),
        (
            "attributes",
            json!({"kind":"document","fields":{"foreign":{"kind":"list","values":[]}}}),
            "attribute",
        ),
        (
            "attributes",
            json!({"kind":"document","fields":{"id":{"kind":"scalar","value":{"kind":"long","value":"3"}}}}),
            "value",
        ),
        ("_iid", json!({"kind":"absent"}), "value"),
    ] {
        let (request, mut documents) = fetch_documents(false);
        documents[0].insert(field.into(), serde_json::from_value(replacement).unwrap());
        let error = StagedReply::new(
            &request,
            StagedOutcome::Committed,
            vec![StagedAnswer::Documents { documents }],
        )
        .unwrap_err();
        assert_eq!(
            error.code().as_str(),
            format!("query_remote_document_{code}")
        );
    }
    let (request, mut documents) = fetch_documents(true);
    documents[0].remove("_role_0_iid");
    let error = StagedReply::new(
        &request,
        StagedOutcome::Committed,
        vec![StagedAnswer::Documents { documents }],
    )
    .unwrap_err();
    assert_eq!(error.code().as_str(), "query_remote_document_role");
}

#[test]
fn fetch_document_optional_roles_require_complete_evidence() {
    use type_bridge_contract::query_plan::{DocumentRole, Stage, ThingDocument};
    let plan = QueryPlan::new(
        support::spec(
            vec![
                Stage::Match {
                    patterns: vec![support::isa(0, support::relation()).into()],
                },
                Stage::Fetch(ThingDocument::new(
                    support::id(0),
                    vec![DocumentRole::new(support::role(), false)],
                )),
            ],
            vec![],
        ),
        &support::schema(),
    )
    .unwrap();
    let shape = plan.fetch_document_shape().unwrap();
    let (_, mut documents) = fetch_documents(true);
    let document = &mut documents[0];
    shape.validate(document).unwrap();
    for suffix in ["iid", "type", "attributes"] {
        document.remove(&format!("_role_0_{suffix}"));
    }
    shape.validate(document).unwrap();
    document.insert("_role_0_iid".into(), RemoteFieldValue::Absent);
    assert_eq!(
        shape.validate(document).unwrap_err().code().as_str(),
        "query_remote_document_role"
    );
    for suffix in ["iid", "type", "attributes"] {
        document.insert(format!("_role_0_{suffix}"), RemoteFieldValue::Absent);
    }
    shape.validate(document).unwrap();
    document.insert(
        "_role_0_type".into(),
        serde_json::from_value(json!({
            "kind":"scalar", "value":{"kind":"string", "value":"account"}
        }))
        .unwrap(),
    );
    assert_eq!(
        shape.validate(document).unwrap_err().code().as_str(),
        "query_remote_document_role"
    );
}

#[test]
fn fetch_document_inherited_attributes_require_an_admitted_exact_type() {
    use type_bridge_contract::query_plan::{Stage, ThingDocument};
    let plan = QueryPlan::new(
        support::spec(
            vec![
                Stage::Match {
                    patterns: vec![support::isa(0, support::entity("child_account")).into()],
                },
                Stage::Fetch(ThingDocument::new(support::id(0), vec![])),
            ],
            vec![],
        ),
        &support::schema(),
    )
    .unwrap();
    let shape = plan.fetch_document_shape().unwrap();
    let (_, mut documents) = fetch_documents(false);
    let document = &mut documents[0];
    assert_eq!(
        shape.validate(document).unwrap_err().code().as_str(),
        "query_remote_document_type"
    );
    document.insert(
        "_type".into(),
        serde_json::from_value(json!({
            "kind":"scalar", "value":{"kind":"string", "value":"child_account"}
        }))
        .unwrap(),
    );
    shape.validate(document).unwrap();
    let RemoteFieldValue::Document { fields } = document.get_mut("attributes").unwrap() else {
        panic!()
    };
    fields.remove("id");
    assert_eq!(
        shape.validate(document).unwrap_err().code().as_str(),
        "query_remote_document_value"
    );
}

#[test]
fn fetch_documents_distinguish_scalar_and_collection_ownerships() {
    use type_bridge_contract::codec::FormatVersion;
    use type_bridge_contract::query_plan::{Stage, ThingDocument};
    use type_bridge_contract::schema::*;
    use type_bridge_contract::value::Cardinality;
    let mut facts = support::schema().facts().cloned().collect::<Vec<_>>();
    facts.push(SchemaFact::Annotation(
        AnnotationFact::new(
            AnnotationFactId::new(
                AnnotationSubjectId::Owns(support::field("score")),
                AnnotationKindId::Card,
            ),
            SchemaAnnotationValue::Cardinality(Cardinality::new(0, Some(2)).unwrap()),
        )
        .unwrap(),
    ));
    let schema = DeclaredSchema::from_facts(
        FormatVersion::V1,
        Default::default(),
        facts.into_iter().enumerate().map(|(index, fact)| {
            SourcedSchemaFact::new(
                fact,
                SourceSpan::new(
                    DocumentId::new("fetch.yaml").unwrap(),
                    index as u64,
                    index as u64 + 1,
                    index as u32 + 1,
                    1,
                    index as u32 + 1,
                    2,
                )
                .unwrap(),
            )
        }),
    )
    .unwrap();
    let plan = QueryPlan::new(
        support::spec(
            vec![
                support::matched(),
                Stage::Fetch(ThingDocument::new(support::id(0), vec![])),
            ],
            vec![],
        ),
        &schema,
    )
    .unwrap();
    let shape = plan.fetch_document_shape().unwrap();
    let (_, mut documents) = fetch_documents(false);
    assert_eq!(
        shape.validate(&documents[0]).unwrap_err().code().as_str(),
        "query_remote_document_value"
    );
    let RemoteFieldValue::Document { fields } = documents[0].get_mut("attributes").unwrap() else {
        panic!()
    };
    fields.insert("score".into(), serde_json::from_value(json!({"kind":"list", "values":[{"kind":"long","value":"7"},{"kind":"long","value":"8"}]})).unwrap());
    shape.validate(&documents[0]).unwrap();
    let RemoteFieldValue::Document { fields } = documents[0].get_mut("attributes").unwrap() else {
        panic!()
    };
    let RemoteFieldValue::List { values } = fields.get_mut("score").unwrap() else {
        panic!()
    };
    values.push(type_bridge_contract::value::CanonicalValue::Long(9));
    assert_eq!(
        shape.validate(&documents[0]).unwrap_err().code().as_str(),
        "query_remote_document_value"
    );
}

#[test]
fn deletion_snapshots_require_capability_and_verify_signed_pre_images() {
    let old = advertisement();
    let mut caps = old.capabilities().clone();
    caps.insert(CapabilityId::new("query.write.delete-snapshot").unwrap());
    let ad = RemoteCapabilities::new(
        caps,
        RemoteExecutorBinding::new("fixture-executor-0001", "fixture-epoch-0000001").unwrap(),
        Keys::new().public_key(),
    )
    .with_query_limits(type_bridge_contract::limits::QueryLimits::CANONICAL)
    .unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/query-remote");
    let mut fixtures = BTreeMap::new();
    fixtures.insert("advertisement-delete-snapshot.json", ad.encode().unwrap());
    for relation in [false, true] {
        let make_spec = || {
            let mut spec = spec();
            spec.plans = vec![entry(examples::delete_returning(relation))];
            spec
        };
        assert_eq!(
            StagedRequest::new(make_spec(), &support::schema(), &old)
                .unwrap_err()
                .code()
                .as_str(),
            "unsupported_required_capability"
        );
        let request = StagedRequest::new(make_spec(), &support::schema(), &ad).unwrap();
        let (_, documents) = fetch_documents(relation);
        let reply = StagedReply::new(
            &request,
            StagedOutcome::Committed,
            vec![StagedAnswer::Documents { documents }],
        )
        .unwrap();
        let bytes = reply.encode_signed(&request, &ad, &Keys::new()).unwrap();
        VerifiedStagedReply::decode(
            &bytes,
            &request,
            &ad,
            Keys::new().public_key(),
            &Keys::new(),
        )
        .unwrap();
        if relation {
            fixtures.insert("request-delete-snapshot.json", request.encode().unwrap());
            fixtures.insert("reply-delete-snapshot.json", bytes);
        }
    }
    for (name, bytes) in fixtures {
        if std::env::var_os("TYPE_BRIDGE_UPDATE_REMOTE_FIXTURES").is_some() {
            std::fs::write(root.join(name), &bytes).unwrap();
        }
        assert_eq!(std::fs::read(root.join(name)).unwrap(), bytes);
    }
}

#[test]
fn deletion_snapshot_capture_consumes_the_read_statement_budget() {
    let schema = support::schema();
    let read = examples::expression_plan(type_bridge_contract::query_plan::Expr::Operand(
        support::literal(2),
    ))
    .with_operation(Operation::Count, &schema)
    .unwrap();
    let snapshot = examples::delete_returning(false);
    assert_eq!(read.statement_counts(), (1, 0));
    assert_eq!(examples::purchase().statement_counts(), (0, 1));
    assert_eq!(examples::batch().statement_counts(), (0, 1));
    assert_eq!(snapshot.statement_counts(), (1, 1));
    let old = advertisement();
    let mut caps = old.capabilities().clone();
    caps.insert(CapabilityId::new("query.write.delete-snapshot").unwrap());
    let ad = RemoteCapabilities::new(caps, old.executor().clone(), Keys::new().public_key());
    for count in [2, 3] {
        let mut spec = spec();
        spec.limits.max_read_statements = 3;
        spec.plans = vec![entry(read.clone()); count];
        spec.plans.push(entry(snapshot.clone()));
        let result = StagedRequest::new(spec, &schema, &ad);
        if count == 2 {
            assert!(result.is_ok());
        } else {
            assert_eq!(
                result.unwrap_err().code().as_str(),
                "query_remote_statement_limit"
            );
        }
    }
}

#[test]
fn optional_answer_dimensions_round_trip_and_require_receiver_capability() {
    let old = advertisement();
    let mut capabilities = old.capabilities().clone();
    capabilities.insert(CapabilityId::new(CAP_QUERY_REMOTE_ANSWER_DIMENSIONS).unwrap());
    let current = RemoteCapabilities::new(capabilities, old.executor().clone(), old.reply_key())
        .with_query_limits(old.query_limits())
        .unwrap();
    let mut legacy_spec = spec();
    let mut spec = spec();
    spec.limits.max_graph_nodes = Some(2);
    spec.limits.max_attribute_values = Some(3);
    spec.limits.max_collection_members = Some(4);
    spec.limits.max_role_players = Some(0);

    legacy_spec.limits = spec.limits;
    assert_eq!(
        StagedRequest::new(legacy_spec, &support::schema(), &old)
            .unwrap_err()
            .code()
            .as_str(),
        "unsupported_required_capability"
    );
    let request = StagedRequest::new(spec, &support::schema(), &current).unwrap();
    let bytes = request.encode().unwrap();
    assert_eq!(
        StagedRequest::decode(&bytes, &support::schema(), &current).unwrap(),
        request
    );
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/query-remote");
    for (name, fixture) in [
        (
            "advertisement-answer-dimensions.json",
            current.encode().unwrap(),
        ),
        ("request-answer-dimensions.json", bytes.clone()),
    ] {
        let path = root.join(name);
        if std::env::var_os("TYPE_BRIDGE_UPDATE_REMOTE_FIXTURES").is_some() {
            std::fs::write(&path, &fixture).unwrap();
        }
        assert_eq!(std::fs::read(path).unwrap(), fixture);
    }

    // Released receivers use deny_unknown_fields and refuse these additions.
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct ReleasedLimits {
        deadline_ms: Option<u64>,
        max_bytes: u64,
        max_items: u64,
        max_staged_plans: u32,
        max_read_statements: u32,
        max_write_statements: u32,
    }
    let wire: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(serde_json::from_value::<ReleasedLimits>(wire["limits"].clone()).is_err());
}

#[test]
fn signed_fetch_evidence_cannot_exceed_requested_semantic_dimensions() {
    let old = advertisement();
    let mut capabilities = old.capabilities().clone();
    capabilities.insert(CapabilityId::new(CAP_QUERY_REMOTE_ANSWER_DIMENSIONS).unwrap());
    let current = RemoteCapabilities::new(capabilities, old.executor().clone(), old.reply_key())
        .with_query_limits(old.query_limits())
        .unwrap();
    let (_, documents) = fetch_documents(true);
    assert_eq!(
        StagedAnswer::Documents {
            documents: documents.clone()
        }
        .resource_counts(),
        [2, 1, 2, 1]
    );
    let mut repeated = documents.clone();
    repeated.extend(documents.clone());
    assert_eq!(
        StagedAnswer::Documents {
            documents: repeated
        }
        .resource_counts(),
        [2, 1, 2, 1]
    );
    for index in 0..4 {
        let mut spec = spec();
        spec.plans = vec![entry(examples::fetch_written(true))];
        match index {
            0 => spec.limits.max_graph_nodes = Some(1),
            1 => spec.limits.max_attribute_values = Some(0),
            2 => spec.limits.max_collection_members = Some(1),
            _ => spec.limits.max_role_players = Some(0),
        }
        let request = StagedRequest::new(spec, &support::schema(), &current).unwrap();
        let error = StagedReply::new(
            &request,
            StagedOutcome::Committed,
            vec![StagedAnswer::Documents {
                documents: documents.clone(),
            }],
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "query_remote_answer_dimensions");
    }
}

#[test]
fn read_rematch_is_fenced_by_receiver_capability() {
    let old = advertisement();
    let request_spec = || {
        let mut spec = spec();
        spec.plans = vec![entry(examples::rematch_window())];
        spec
    };
    assert_eq!(
        StagedRequest::new(request_spec(), &support::schema(), &old)
            .unwrap_err()
            .code()
            .as_str(),
        "unsupported_required_capability"
    );
    let mut capabilities = old.capabilities().clone();
    capabilities.insert(CapabilityId::new("query.pipeline.rematch").unwrap());
    let current = RemoteCapabilities::new(capabilities, old.executor().clone(), old.reply_key())
        .with_query_limits(old.query_limits())
        .unwrap();
    let request = StagedRequest::new(request_spec(), &support::schema(), &current).unwrap();
    assert_eq!(
        StagedRequest::decode(&request.encode().unwrap(), &support::schema(), &current).unwrap(),
        request
    );
}

#[test]
fn iid_expression_requires_receiver_capability() {
    let old = advertisement();
    let request_spec = || {
        let mut spec = spec();
        spec.plans = vec![entry(examples::iid_expression())];
        spec
    };
    assert_eq!(
        StagedRequest::new(request_spec(), &support::schema(), &old)
            .unwrap_err()
            .code()
            .as_str(),
        "unsupported_required_capability"
    );
    let mut capabilities = old.capabilities().clone();
    capabilities.insert(CapabilityId::new("query.expr.iid").unwrap());
    let current = RemoteCapabilities::new(capabilities, old.executor().clone(), old.reply_key())
        .with_query_limits(old.query_limits())
        .unwrap();
    let request = StagedRequest::new(request_spec(), &support::schema(), &current).unwrap();
    assert_eq!(
        StagedRequest::decode(&request.encode().unwrap(), &support::schema(), &current).unwrap(),
        request
    );
}

#[test]
fn wide_scalars_require_explicit_caller_and_receiver_limits_and_capability() {
    let mut invalid_limits = QueryLimits::CANONICAL;
    invalid_limits.codec.max_depth = 127;
    assert_eq!(
        StagedRequest::new_with_limits(
            spec(),
            &support::schema(),
            &advertisement(),
            invalid_limits
        )
        .unwrap_err()
        .code()
        .as_str(),
        "query_remote_query_limits"
    );
    use type_bridge_contract::query_plan::{Expr, Pattern, QueryOperand, Stage};
    use type_bridge_contract::value::{CanonicalString, CanonicalValue};
    let text = "x".repeat(1024 * 1024 + 1);
    let limits =
        QueryLimits::from_overrides([("canonical_string_bytes", 2 * 1024 * 1024)]).unwrap();
    assert!(CanonicalString::new(&text).is_err());
    let value =
        CanonicalValue::String(CanonicalString::new_with_limits(text, limits.codec).unwrap());
    let plan_spec = support::spec(
        vec![Stage::Match {
            patterns: vec![Pattern::Let {
                binding: support::id(0),
                expr: Expr::Operand(QueryOperand::Literal {
                    value: value.clone(),
                }),
            }],
        }],
        vec![0],
    );
    assert!(QueryPlan::new(plan_spec.clone(), &support::schema()).is_err());
    let plan = QueryPlan::new_with_limits(plan_spec, &support::schema(), limits).unwrap();
    assert!(
        plan.required_capabilities()
            .iter()
            .any(|cap| cap.as_str() == "query.input.wide-strings")
    );
    let mut thing = support::thing(0);
    thing.fields[0].1 = QueryOperand::Literal {
        value: value.clone(),
    };
    let write = QueryPlan::new_with_limits(
        support::spec(
            vec![Stage::Insert {
                things: vec![thing],
            }],
            vec![0],
        ),
        &support::schema(),
        limits,
    )
    .unwrap();
    assert_eq!(
        type_bridge_contract::query_plan::decode_query_plan_with_limits(
            &write.canonical_bytes().unwrap(),
            &support::schema(),
            limits,
        )
        .unwrap(),
        write
    );
    let bytes = plan.canonical_bytes().unwrap();
    assert!(
        type_bridge_contract::query_plan::decode_query_plan(&bytes, &support::schema()).is_err()
    );
    assert_eq!(
        type_bridge_contract::query_plan::decode_query_plan_with_limits(
            &bytes,
            &support::schema(),
            limits
        )
        .unwrap(),
        plan
    );
    // The frozen scalar decoder keeps its default ceiling.
    assert!(
        serde_json::from_value::<CanonicalValue>(serde_json::to_value(&value).unwrap()).is_err()
    );
    let request_spec = || {
        let mut request_spec = spec();
        request_spec.plans = vec![entry(plan.clone())];
        request_spec
    };
    assert!(
        StagedRequest::new_with_limits(
            request_spec(),
            &support::schema(),
            &advertisement(),
            limits
        )
        .is_err()
    );
    let old = advertisement();
    let mut capabilities = old.capabilities().clone();
    capabilities.insert(CapabilityId::new("query.input.wide-strings").unwrap());
    let ad = RemoteCapabilities::new(capabilities, old.executor().clone(), old.reply_key())
        .with_query_limits(limits)
        .unwrap();
    assert!(StagedRequest::new(request_spec(), &support::schema(), &ad).is_err());
    let request =
        StagedRequest::new_with_limits(request_spec(), &support::schema(), &ad, limits).unwrap();
    let bytes = request.encode().unwrap();
    assert!(StagedRequest::decode(&bytes, &support::schema(), &ad).is_err());
    assert_eq!(
        StagedRequest::decode_with_limits(&bytes, &support::schema(), &ad, limits).unwrap(),
        request
    );
    let mut small_spec = spec();
    small_spec.plans = vec![entry(examples::expression_plan(Expr::Operand(
        support::string("small"),
    )))];
    let small_request = StagedRequest::new(small_spec, &support::schema(), &ad).unwrap();
    assert!(
        StagedReply::new(
            &small_request,
            StagedOutcome::Committed,
            vec![StagedAnswer::Rows {
                rows: vec![vec![RemoteValue::Value {
                    value: value.clone()
                }]]
            }],
        )
        .is_err()
    );
    let reply = StagedReply::new(
        &request,
        StagedOutcome::Committed,
        vec![StagedAnswer::Rows {
            rows: vec![vec![RemoteValue::Value { value }]],
        }],
    )
    .unwrap();
    let bytes = reply.encode_signed(&request, &ad, &Keys::new()).unwrap();
    VerifiedStagedReply::decode(
        &bytes,
        &request,
        &ad,
        Keys::new().public_key(),
        &Keys::new(),
    )
    .unwrap();
}

#[test]
fn unbound_templates_require_wide_string_capability_for_invocation_rows() {
    use type_bridge_contract::migration_assertion::QueryVariable;
    use type_bridge_contract::query_plan::{
        Expr, InputColumn, InputColumnId, InputRow, Pattern, QueryOperand, Stage,
    };
    use type_bridge_contract::value::{CanonicalString, CanonicalValue, ValueTypeTag};
    let limits =
        QueryLimits::from_overrides([("canonical_string_bytes", 2 * 1024 * 1024)]).unwrap();
    let mut template = support::spec(
        vec![Stage::Match {
            patterns: vec![Pattern::Let {
                binding: support::id(0),
                expr: Expr::Operand(QueryOperand::Input {
                    column: InputColumnId::new(0),
                }),
            }],
        }],
        vec![0],
    );
    template.inputs = vec![InputColumn::new(
        InputColumnId::new(0),
        QueryVariable::new("input").unwrap(),
        ValueTypeTag::String,
        false,
    )];
    let template = QueryPlan::new_with_limits(template, &support::schema(), limits).unwrap();
    assert!(
        !template
            .required_capabilities()
            .iter()
            .any(|cap| cap.as_str() == "query.input.wide-strings")
    );
    let value = CanonicalValue::String(
        CanonicalString::new_with_limits("x".repeat(1024 * 1024 + 1), limits.codec).unwrap(),
    );
    let make_spec = || {
        let mut request = spec();
        request.plans = vec![
            StagedPlan::new(
                &template,
                vec![InputRow::new(vec![Some(value.clone())])],
                &support::schema(),
            )
            .unwrap(),
        ];
        request
    };
    let old = advertisement().with_query_limits(limits).unwrap();
    assert!(StagedRequest::new_with_limits(make_spec(), &support::schema(), &old, limits).is_err());
    let mut capabilities = old.capabilities().clone();
    capabilities.insert(CapabilityId::new("query.input.wide-strings").unwrap());
    let ad = RemoteCapabilities::new(capabilities, old.executor().clone(), old.reply_key())
        .with_query_limits(limits)
        .unwrap();
    let request =
        StagedRequest::new_with_limits(make_spec(), &support::schema(), &ad, limits).unwrap();
    let bytes = request.encode().unwrap();
    assert_eq!(
        StagedRequest::decode_with_limits(&bytes, &support::schema(), &ad, limits).unwrap(),
        request
    );
    let error = StagedRequest::decode(&bytes, &support::schema(), &ad).unwrap_err();
    assert_eq!(
        error.details().get("limit"),
        Some(
            &type_bridge_contract::diagnostic::DiagnosticDetailValue::Text(
                "canonical_string_bytes".into()
            )
        )
    );
}

#[test]
fn explicitly_advertised_statement_limits_remain_strict_at_the_default_value() {
    let make_spec = || {
        let mut request = spec();
        request.plans.push(entry(examples::batch()));
        request
    };
    StagedRequest::new(make_spec(), &support::schema(), &advertisement()).unwrap();
    let explicit = advertisement()
        .with_answer_limits(BTreeMap::from([("statements".to_owned(), 3)]))
        .unwrap();
    let error = StagedRequest::new(make_spec(), &support::schema(), &explicit).unwrap_err();
    assert_eq!(error.code().as_str(), "query_remote_statement_limit");
    assert_eq!(
        error.details().get("maximum"),
        Some(&type_bridge_contract::diagnostic::DiagnosticDetailValue::Long(3))
    );
}

#[test]
fn explicit_staged_answer_budgets_share_policy_maxima_without_changing_defaults() {
    let defaults = StagedLimits::default();
    assert_eq!(defaults.max_bytes, 32 * 1024 * 1024);
    assert_eq!(defaults.max_items, 65_536);
    for (bytes, items) in [(1 << 30, 1_000_000), (u64::MAX, u64::MAX)] {
        let limits = StagedLimits {
            max_bytes: bytes,
            max_items: items,
            ..defaults
        };
        assert_eq!(limits.validate().unwrap(), limits);
    }
    let wire = serde_json::to_string(&defaults).unwrap().replace(
        "\"max_bytes\":33554432",
        "\"max_bytes\":18446744073709551616",
    );
    assert!(serde_json::from_str::<StagedLimits>(&wire).is_err());
}

#[test]
fn staged_advertisements_state_native_columns_and_retain_explicit_lower_caps() {
    use type_bridge_contract::limits::{MAX_BINDINGS, QueryLimits};
    let old = advertisement();
    assert_eq!(old.query_limits(), QueryLimits::CANONICAL);
    let native = RemoteCapabilities::new(
        old.capabilities().clone(),
        old.executor().clone(),
        old.reply_key(),
    );
    assert_eq!(native.query_limits(), QueryLimits::PLAN);
    let json: serde_json::Value = serde_json::from_slice(&native.encode().unwrap()).unwrap();
    assert_eq!(json["query_limits"]["selected_slots"], MAX_BINDINGS);
    let lower = native.with_query_limits(QueryLimits::CANONICAL).unwrap();
    assert_eq!(lower.query_limits().structural.selected_slots, 16);
}

#[test]
fn typed_read_failure_projection_preserves_path_details_certainty_and_static_message() {
    use type_bridge_contract::sdk_diagnostic::{
        SdkDiagnosticDetailValue, SdkDiagnosticName, SdkDiagnosticPathSegment,
        SdkQueryDiagnosticCategory, SdkQueryDiagnosticIdentity,
    };
    let identity = |value| SdkQueryDiagnosticIdentity::new(value).unwrap();
    let diagnostic = SdkExecutionDiagnostic::query_failure(
        SdkQueryDiagnosticCategory::ResultDecode,
        SdkDiagnosticCode::new("remote_application_failure").unwrap(),
    )
    .try_at(SdkDiagnosticPathSegment::ContractField(identity("plan")))
    .unwrap()
    .try_at(SdkDiagnosticPathSegment::Index(2))
    .unwrap()
    .try_at(SdkDiagnosticPathSegment::ContractIdentity(identity(
        "person",
    )))
    .unwrap()
    .try_with_detail(
        SdkDiagnosticName::new("attempt").unwrap(),
        SdkDiagnosticDetailValue::Signed(-7),
    )
    .unwrap()
    .try_with_detail(
        SdkDiagnosticName::new("actual").unwrap(),
        SdkDiagnosticDetailValue::Count(u64::MAX),
    )
    .unwrap()
    .try_with_detail(
        SdkDiagnosticName::new("subject").unwrap(),
        SdkDiagnosticDetailValue::QueryIdentity(identity("person")),
    )
    .unwrap()
    .with_failure(
        &type_bridge_contract::failure::Failure::Indeterminate { code: None },
        Default::default(),
    );
    let expected = serde_json::to_value(diagnostic.host_json()).unwrap();
    let mut wire = expected.clone();
    wire["message"] = json!("provider secret must not escape");
    let projection: type_bridge_contract::query_remote::SdkDiagnosticProjection =
        serde_json::from_value(wire).unwrap();
    assert_eq!(
        serde_json::to_value(projection.to_query_diagnostic().unwrap().host_json()).unwrap(),
        expected
    );
}

#[test]
fn current_vocabulary_only_adds_explicitly_named_capabilities_to_frozen_advertisements() {
    let frozen =
        RemoteCapabilities::decode(include_bytes!("fixtures/query-remote/advertisement.json"))
            .unwrap();
    let mut current = query_plan_capability_vocabulary();
    for capability in [CAP_QUERY_REMOTE_STAGED, CAP_QUERY_REMOTE_WRITE] {
        current.insert(CapabilityId::new(capability).unwrap());
    }
    assert!(frozen.capabilities().missing_from(&current).is_empty());
    let expected: type_bridge_contract::capability::CapabilitySet = [
        "query.write.delete-snapshot",
        "query.pipeline.rematch",
        "query.expr.iid",
        "query.input.wide-strings",
        "query.fetch.scalar-fields",
        "query.fetch.role-lists",
        "query.pipeline.write-projection",
        "query.pattern.nested-iid",
        "query.pattern.negated-given-iid",
        "query.write.put-subtypes",
        "query.write.optional-attach",
        "query.write.attach-key",
    ]
    .into_iter()
    .map(|name| CapabilityId::new(name).unwrap())
    .collect();
    assert_eq!(current.missing_from(frozen.capabilities()), expected);
    assert_eq!(advertisement().encode().unwrap(), frozen.encode().unwrap());
}

#[test]
fn execution_deadline_is_independent_of_finite_request_freshness() {
    let mut spec = spec();
    spec.limits.deadline_ms = None;
    let issued = spec.prepared_at_unix_ms;
    let request = StagedRequest::new(spec, &support::schema(), &advertisement()).unwrap();
    assert_eq!(request.limits().deadline_ms, None);
    assert_eq!(request.expires_at_unix_ms(), issued + 60_000);
    assert!(request.validate_at(issued + 59_999).is_ok());
    assert_eq!(
        request
            .validate_at(issued + 60_000)
            .unwrap_err()
            .code()
            .as_str(),
        "query_remote_expired"
    );
    let bytes = request.encode().unwrap();
    assert_eq!(
        StagedRequest::decode(&bytes, &support::schema(), &advertisement()).unwrap(),
        request
    );
    for (field, value) in [
        ("expires_at_unix_ms", Value::Null),
        ("expires_at_unix_ms", json!(issued + 60_001)),
        ("prepared_at_unix_ms", json!(issued + 1)),
    ] {
        let mut wire: Value = serde_json::from_slice(&bytes).unwrap();
        wire[field] = value;
        assert!(
            StagedRequest::decode(
                &to_canonical_json(&wire).unwrap(),
                &support::schema(),
                &advertisement()
            )
            .is_err()
        );
    }
    for value in [json!(0), json!(-1), json!(1.5), json!("none")] {
        let mut wire: Value = serde_json::from_slice(&bytes).unwrap();
        wire["limits"]["deadline_ms"] = value;
        assert!(
            StagedRequest::decode(
                &to_canonical_json(&wire).unwrap(),
                &support::schema(),
                &advertisement()
            )
            .is_err()
        );
    }
}

#[test]
fn admission_window_is_configurable_and_server_ceiling_wins() {
    let advertisement = advertisement()
        .with_admission_window_milliseconds(17)
        .unwrap();
    let mut spec = spec();
    spec.limits.admission_window_milliseconds = 100;
    spec.limits.deadline_ms = Some(86_400_001);
    let issued = spec.prepared_at_unix_ms;
    let request = StagedRequest::new(spec, &support::schema(), &advertisement).unwrap();
    assert_eq!(request.limits().admission_window_milliseconds, 17);
    assert_eq!(request.expires_at_unix_ms(), issued + 17);
    assert_eq!(request.limits().deadline_ms, Some(86_400_001));
    assert!(
        advertisement
            .clone()
            .with_admission_window_milliseconds(0)
            .is_err()
    );
    assert_eq!(
        RemoteCapabilities::decode(&advertisement.encode().unwrap())
            .unwrap()
            .admission_window_milliseconds(),
        17
    );
}

#[test]
fn execution_timeline_is_diagnostic_only_and_round_trips() {
    let timeline: type_bridge_contract::execution_timeline::ExecutionTimeline =
        serde_json::from_value(serde_json::json!({
            "anchor_unix_milliseconds": 1000,
            "stamps": [
                {"phase": "fired", "offset_milliseconds": 0},
                {"phase": "failed", "offset_milliseconds": 5, "failing_phase": "fired"}
            ]
        }))
        .unwrap();
    let diagnostic =
        type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic::internal_failure()
            .try_with_detail(
                type_bridge_contract::sdk_diagnostic::SdkDiagnosticName::new("execution_timeline")
                    .unwrap(),
                type_bridge_contract::sdk_diagnostic::SdkDiagnosticDetailValue::ExecutionTimeline(
                    timeline.clone(),
                ),
            )
            .unwrap();
    let request = request();
    let reply = StagedReply::new(&request, StagedOutcome::Failed { diagnostic }, vec![]).unwrap();
    let payload: Value = serde_json::from_slice(&reply.encode_payload().unwrap()).unwrap();
    assert_eq!(payload["answers"], json!([]));
    let keys = Keys::new();
    let ad = advertisement();
    let bytes = reply.encode_signed(&request, &ad, &keys).unwrap();
    let decoded =
        VerifiedStagedReply::decode(&bytes, &request, &ad, keys.public_key(), &keys).unwrap();
    assert_eq!(
        decoded.encode_payload().unwrap(),
        reply.encode_payload().unwrap()
    );
    let reconstructed = decoded.diagnostic().unwrap().to_query_diagnostic().unwrap();
    assert_eq!(
        serde_json::to_value(reconstructed.host_json()).unwrap()["details"]["execution_timeline"]["value"],
        serde_json::to_value(timeline).unwrap()
    );
}

#[test]
fn maximum_execution_timeline_passes_signed_reply_admission() {
    use type_bridge_contract::execution_timeline::{
        ExecutionPhase, ExecutionPhaseStamp, ExecutionTimeline,
    };
    let maximum = type_bridge_contract::limits::MAX_STAGED_STATEMENTS;
    let request = mutate_request(|value| {
        value["limits"]["max_read_statements"] = json!(maximum);
        value["limits"]["max_write_statements"] = json!(maximum);
    })
    .unwrap();
    let mut stamps = vec![];
    for (offset, phase) in [
        ExecutionPhase::Fired,
        ExecutionPhase::Accepted,
        ExecutionPhase::ExecutionStarted,
    ]
    .into_iter()
    .enumerate()
    {
        stamps.push(ExecutionPhaseStamp {
            phase,
            offset_milliseconds: offset as u64,
            statement_index: None,
            failing_phase: None,
        });
    }
    for index in 0..maximum * 2 {
        stamps.push(ExecutionPhaseStamp {
            phase: ExecutionPhase::DbWrite,
            offset_milliseconds: 3 + u64::from(index),
            statement_index: Some(index),
            failing_phase: None,
        });
    }
    stamps.push(ExecutionPhaseStamp {
        phase: ExecutionPhase::Failed,
        offset_milliseconds: 3 + u64::from(maximum * 2),
        statement_index: None,
        failing_phase: Some(ExecutionPhase::DbWrite),
    });
    let timeline = ExecutionTimeline::new(1000, stamps).unwrap();
    let diagnostic = SdkExecutionDiagnostic::internal_failure()
        .try_with_detail(
            type_bridge_contract::sdk_diagnostic::SdkDiagnosticName::new("execution_timeline")
                .unwrap(),
            type_bridge_contract::sdk_diagnostic::SdkDiagnosticDetailValue::ExecutionTimeline(
                timeline,
            ),
        )
        .unwrap();
    let reply = StagedReply::new(&request, StagedOutcome::Failed { diagnostic }, vec![]).unwrap();
    let keys = Keys::new();
    let ad = advertisement();
    let bytes = reply.encode_signed(&request, &ad, &keys).unwrap();
    assert!(bytes.len() > type_bridge_contract::limits::MAX_DIAGNOSTIC_BYTES);
    let decoded =
        VerifiedStagedReply::decode(&bytes, &request, &ad, keys.public_key(), &keys).unwrap();
    assert_eq!(
        decoded.encode_payload().unwrap(),
        reply.encode_payload().unwrap()
    );
}

#[test]
fn phase_collection_is_optional_in_the_request_wire() {
    let plain: Value = serde_json::from_slice(&request().encode().unwrap()).unwrap();
    assert!(plain["limits"].get("collect_phase_timestamps").is_none());
    let enabled =
        mutate_request(|value| value["limits"]["collect_phase_timestamps"] = json!(true)).unwrap();
    assert!(enabled.limits().collect_phase_timestamps);
    let decoded = StagedRequest::decode(
        &enabled.encode().unwrap(),
        &support::schema(),
        &advertisement(),
    )
    .unwrap();
    assert!(decoded.limits().collect_phase_timestamps);
    assert!(
        mutate_request(|value| value["limits"]["collect_phase_timestamps"] = json!(1)).is_err()
    );
}

#[test]
fn missing_player_proof_cell_keeps_coordinates_in_signed_failure() {
    use type_bridge_contract::diagnostic::DiagnosticDetailValue;
    use type_bridge_contract::migration_assertion::QueryVariable;
    use type_bridge_contract::query_plan::{Expr, Pattern, Stage};
    use type_bridge_contract::query_plan::{
        InputColumn, InputColumnId, InputRow, QueryOperand, QueryPattern,
    };
    use type_bridge_contract::sdk_diagnostic::{SdkDiagnosticDetailValue, SdkDiagnosticName};
    use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};

    let schema = support::schema();
    let proof = QueryPlan::new(
        support::spec(
            vec![Stage::Match {
                patterns: vec![
                    Pattern::Let {
                        binding: support::id(2),
                        expr: Expr::Operand(support::literal(0)),
                    },
                    QueryPattern::Try {
                        patterns: vec![support::isa(0, support::entity("account"))],
                    }
                    .into(),
                    QueryPattern::Try {
                        patterns: vec![support::isa(1, support::entity("account"))],
                    }
                    .into(),
                ],
            }],
            vec![2, 0, 1],
        ),
        &schema,
    )
    .unwrap();
    let proof = StagedPlan::new(&proof, vec![], &schema).unwrap();
    let mut target = support::spec(
        vec![Stage::Match {
            patterns: vec![
                support::isa(0, support::entity("account")).into(),
                QueryPattern::Iid {
                    binding: support::id(0),
                    iid: QueryOperand::Input {
                        column: InputColumnId::new(0),
                    },
                }
                .into(),
            ],
        }],
        vec![0],
    );
    target.inputs = (0..2)
        .map(|index| {
            InputColumn::new(
                InputColumnId::new(index),
                QueryVariable::new(format!("player{index}")).unwrap(),
                ValueTypeTag::String,
                false,
            )
        })
        .collect();
    let target = QueryPlan::new(target, &schema).unwrap();
    let target = StagedPlan::new_with_result_refs(
        &target,
        vec![InputRow::new(vec![None, None])],
        (0..2)
            .map(|index| StagedResultRef {
                input: InputColumnId::new(index),
                source_plan: 0,
                source_column: index + 1,
                expected_types: vec![support::entity("account")],
            })
            .collect(),
        &schema,
    )
    .unwrap();
    let answer = StagedAnswer::Rows {
        rows: vec![vec![
            RemoteValue::Value {
                value: CanonicalValue::Long(0),
            },
            RemoteValue::Thing {
                iid: "0x10".into(),
                type_id: support::entity("account"),
            },
            RemoteValue::Absent,
        ]],
    };
    answer.validate_for(proof.plan()).unwrap();
    let failure = target.bind_results(&[answer], &schema).unwrap_err();
    assert_eq!(failure.code().as_str(), "query_stage_result_ref_missing");
    for (name, expected) in [
        ("source_plan", 0),
        ("source_row", 0),
        ("source_column", 2),
        ("input_column", 1),
    ] {
        assert_eq!(
            failure.details().get(name),
            Some(&DiagnosticDetailValue::Long(expected))
        );
    }
    let empty = target
        .bind_results(&[StagedAnswer::Rows { rows: vec![] }], &schema)
        .unwrap_err();
    assert_eq!(
        empty.details().get("source_column"),
        Some(&DiagnosticDetailValue::Long(1))
    );
    assert_eq!(
        empty.details().get("input_column"),
        Some(&DiagnosticDetailValue::Long(0))
    );
    assert!(!empty.details().contains_key("source_row"));

    let mut diagnostic = SdkExecutionDiagnostic::query_failure(
        SdkQueryDiagnosticCategory::ResultDecode,
        SdkDiagnosticCode::new(failure.code().as_str()).unwrap(),
    );
    for (name, value) in failure.details() {
        let DiagnosticDetailValue::Long(value) = value else {
            panic!("proof coordinates are integers")
        };
        diagnostic = diagnostic
            .try_with_detail(
                SdkDiagnosticName::new(name).unwrap(),
                SdkDiagnosticDetailValue::Signed(*value),
            )
            .unwrap();
    }
    let keys = Keys::new();
    let base = advertisement();
    let mut capabilities = base.capabilities().clone();
    capabilities.insert(CapabilityId::new(CAP_QUERY_STAGE_RESULT_REF).unwrap());
    for entry in [&proof, &target] {
        for capability in entry.plan().required_capabilities().iter() {
            capabilities.insert(capability.clone());
        }
    }
    let advertisement =
        RemoteCapabilities::new(capabilities, base.executor().clone(), keys.public_key())
            .with_query_limits(base.query_limits())
            .unwrap();
    let request = StagedRequest::new(
        StagedRequestSpec {
            nonce: "missing-proof-cell-0000001".into(),
            kind: None,
            limits: StagedLimits::default(),
            plans: vec![proof, target],
            prepared_at_unix_ms: 1_000_000,
        },
        &schema,
        &advertisement,
    )
    .unwrap();
    let expected_diagnostic = diagnostic.clone();
    let reply = StagedReply::new(&request, StagedOutcome::Failed { diagnostic }, vec![]).unwrap();
    let bytes = reply
        .encode_signed(&request, &advertisement, &keys)
        .unwrap();
    let decoded =
        VerifiedStagedReply::decode(&bytes, &request, &advertisement, keys.public_key(), &keys)
            .unwrap();
    assert!(!decoded.committed());
    assert_eq!(
        decoded.encode_payload().unwrap(),
        reply.encode_payload().unwrap()
    );
    let round_trip = decoded.diagnostic().unwrap().to_query_diagnostic().unwrap();
    assert_eq!(round_trip.code().as_str(), failure.code().as_str());
    assert_eq!(round_trip, expected_diagnostic);
}

#[test]
fn pre_request_failure_preserves_diagnostics_and_authenticates() {
    use type_bridge_contract::diagnostic::{
        Diagnostic, DiagnosticCategory, DiagnosticCode, DiagnosticPath, DiagnosticPathSegment,
    };
    let diagnostic = Diagnostic::new(
        DiagnosticCategory::ResourceLimit,
        DiagnosticCode::new("staged_admission_full").unwrap(),
        "queue is full",
    )
    .with_detail("capacity", 3_i64)
    .with_path(DiagnosticPath::from_segments(vec![
        DiagnosticPathSegment::Field("plans".into()),
    ]));
    let nonce = "staged_failure_nonce_0001";
    let failure = StagedFailure::new(Some(nonce.into()), &diagnostic).unwrap();
    let keys = Keys::new();
    let advertisement = advertisement().fingerprint().unwrap();
    let bytes = failure.encode_signed(&advertisement, &keys).unwrap();
    let decoded =
        decode_signed_staged_failure(&bytes, &advertisement, keys.public_key(), u64::MAX, &keys)
            .unwrap();
    assert_eq!(decoded.diagnostic().unwrap(), diagnostic);
    assert_eq!(decoded.nonce(), Some(nonce));
    assert!(
        decode_signed_staged_failure(&bytes, &advertisement, keys.public_key(), 1, &keys).is_err()
    );
    let wrong = Keys(SigningKey::from_bytes(&[8; 32]));
    assert!(
        decode_signed_staged_failure(&bytes, &advertisement, wrong.public_key(), u64::MAX, &keys)
            .is_err()
    );
    let other_advertisement = RemoteCapabilities::new(
        Default::default(),
        RemoteExecutorBinding::new("fixture-executor-0002", "fixture-epoch-0000002").unwrap(),
        keys.public_key(),
    )
    .fingerprint()
    .unwrap();
    assert!(
        decode_signed_staged_failure(
            &bytes,
            &other_advertisement,
            keys.public_key(),
            u64::MAX,
            &keys
        )
        .is_err()
    );
}

#[test]
fn pre_request_failure_rejects_invalid_payloads_and_fallback_is_bounded() {
    use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
    let diagnostic = Diagnostic::new(
        DiagnosticCategory::Integrity,
        DiagnosticCode::new("staged_refused").unwrap(),
        "refused",
    );
    let failure = StagedFailure::new(None, &diagnostic).unwrap();
    let value = serde_json::to_value(&failure).unwrap();
    for (field, replacement) in [
        ("format", json!("typebridge.query-remote-failure/v2")),
        ("request", json!("a".repeat(64))),
        ("nonce", json!("bad nonce")),
        ("extra", json!(true)),
        (
            "message",
            json!("x".repeat(type_bridge_contract::limits::MAX_DIAGNOSTIC_BYTES)),
        ),
    ] {
        let mut invalid = value.clone();
        invalid[field] = replacement;
        assert!(
            StagedFailure::decode_payload(&to_canonical_json(&invalid).unwrap()).is_err(),
            "{field}"
        );
    }
    assert!(StagedFailure::new(Some("short".into()), &diagnostic).is_err());
    let mut oversized = value;
    oversized["message"] = json!("x".repeat(type_bridge_contract::limits::MAX_DIAGNOSTIC_BYTES));
    let failure: StagedFailure = serde_json::from_value(oversized).unwrap();
    let keys = Keys::new();
    let advertisement = advertisement().fingerprint().unwrap();
    assert!(failure.encode_signed(&advertisement, &keys).is_err());
    let bytes = failure.encode_signed_or_fallback(&advertisement, &keys);
    let fallback =
        decode_signed_staged_failure(&bytes, &advertisement, keys.public_key(), u64::MAX, &keys)
            .unwrap();
    assert_eq!(
        fallback.diagnostic().unwrap().code().as_str(),
        "query_remote_internal_failure"
    );
}
