#[path = "support/plan_v3_examples.rs"]
#[allow(dead_code)]
mod examples;
#[path = "support/plan_v3.rs"]
#[allow(dead_code)]
mod support;

use std::collections::BTreeMap;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde_json::{Value, json};
use type_bridge_contract::capability::CapabilityId;
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::limits::QueryLimits;
use type_bridge_contract::query_plan::v3::{
    Operation, QueryPlanV3, query_plan_v3_capability_vocabulary,
};
use type_bridge_contract::query_remote::*;
use type_bridge_contract::query_remote_v3::*;
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
        query_plan_v3_capability_vocabulary()
            .into_iter()
            .filter(|cap| cap.as_str() != "query.write.delete-snapshot")
            .collect();
    for cap in [CAP_QUERY_REMOTE_STAGED_V3, CAP_QUERY_REMOTE_WRITE_V3] {
        caps.insert(CapabilityId::new(cap).unwrap());
    }
    RemoteCapabilities::new(
        caps,
        RemoteExecutorBinding::new("fixture-executor-0001", "fixture-epoch-0000001").unwrap(),
        Keys::new().public_key(),
    )
}
fn entry(plan: QueryPlanV3) -> StagedPlanV3 {
    let rows = plan.given_rows().unwrap_or(&[]).to_vec();
    StagedPlanV3::new(&plan, rows, &support::schema()).unwrap()
}
fn spec() -> StagedRequestSpecV3 {
    StagedRequestSpecV3 {
        nonce: "fixture-nonce-0000000000000001".into(),
        kind: None,
        limits: StagedLimitsV3::default(),
        plans: vec![
            entry(examples::purchase()),
            entry(examples::batch()),
            entry(
                examples::expression_plan(type_bridge_contract::query_plan::v3::Expr::Operand(
                    support::literal(2),
                ))
                .with_operation(Operation::Count, &support::schema())
                .unwrap(),
            ),
        ],
        prepared_at_unix_ms: 1_000_000,
    }
}
fn request() -> StagedRequestV3 {
    StagedRequestV3::new(spec(), &support::schema(), &advertisement()).unwrap()
}
fn answers() -> Vec<StagedAnswerV3> {
    vec![
        StagedAnswerV3::Rows {
            rows: vec![vec![RemoteValue::Thing {
                iid: "0x01".into(),
                type_id: type_bridge_contract::id::TypeId::new(
                    type_bridge_contract::id::TypeKind::Relation,
                    "purchase",
                )
                .unwrap(),
            }]],
        },
        StagedAnswerV3::Matched { value: 2 },
        StagedAnswerV3::Count { value: 1 },
    ]
}
fn mutate_request(
    f: impl FnOnce(&mut Value),
) -> Result<StagedRequestV3, type_bridge_contract::diagnostic::Diagnostic> {
    let mut value: Value = serde_json::from_slice(&request().encode().unwrap()).unwrap();
    f(&mut value);
    StagedRequestV3::decode(
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
        StagedRequestV3::decode(&bytes, &support::schema(), &advertisement()).unwrap(),
        request
    );
    assert_eq!(request.kind(), StagedKindV3::Write);
    assert_eq!(request.plans().len(), 3);
    assert!(request.validate_at(1_000_000).is_ok());
    assert!(request.validate_at(1_030_000).is_err());
    assert!(request.validate_at(900_000).is_err());
    for field in ["nonce", "kind", "prepared_at_unix_ms"] {
        let changed = mutate_request(|v| match field {
            "nonce" => v[field] = json!("another-nonce-00000000001"),
            "kind" => v[field] = json!("write"),
            _ => {
                v[field] = json!(1_000_001);
                v["expires_at_unix_ms"] = json!(1_030_001);
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
        StagedOutcomeV3::Committed,
        StagedOutcomeV3::Failed {
            diagnostic: SdkExecutionDiagnostic::commit_failure(SdkCommitFailureOutcome::Unknown),
        },
        StagedOutcomeV3::Failed {
            diagnostic: SdkExecutionDiagnostic::query_failure(
                SdkQueryDiagnosticCategory::Cardinality,
                SdkDiagnosticCode::new("query_rejected_cardinality").unwrap(),
            ),
        },
    ] {
        let committed = matches!(outcome, StagedOutcomeV3::Committed);
        let reply = StagedReplyV3::new(
            &request,
            outcome,
            if committed { answers() } else { vec![] },
        )
        .unwrap();
        let bytes = reply.encode_signed(&request, &ad, &keys).unwrap();
        let decoded =
            VerifiedStagedReplyV3::decode(&bytes, &request, &ad, keys.public_key(), &keys).unwrap();
        assert_eq!(decoded.committed(), committed);
        assert_eq!(
            decoded.encode_payload().unwrap(),
            reply.encode_payload().unwrap()
        );
        assert_eq!(decoded.diagnostic().is_some(), !committed);
    }
    assert!(
        StagedReplyV3::new(
            &request,
            StagedOutcomeV3::Failed {
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
    let reply = StagedReplyV3::new(&request, StagedOutcomeV3::Committed, answers()).unwrap();
    let bytes = reply.encode_signed(&request, &ad, &keys).unwrap();
    let mut changed: Value = serde_json::from_slice(&bytes).unwrap();
    changed["payload"]["outcome"] =
        json!({"kind":"failed","diagnostic":{"message":"SECRET_SENTINEL"}});
    let error = VerifiedStagedReplyV3::decode(
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
            VerifiedStagedReplyV3::decode(
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
        VerifiedStagedReplyV3::decode(
            &bytes,
            &request,
            &ad,
            RemoteSigningPublicKey::from_bytes([0; 32]),
            &keys
        )
        .is_err()
    );
    let foreign = mutate_request(|v| v["nonce"] = json!("another-nonce-00000000001")).unwrap();
    assert!(
        VerifiedStagedReplyV3::decode(&bytes, &foreign, &ad, keys.public_key(), &keys).is_err()
    );
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
    assert!(StagedRequestV3::decode(&bytes, &support::schema(), &advertisement()).is_err());
}
#[test]
fn plan_count_and_separate_statement_limits_are_enforced() {
    let base = entry(examples::batch());
    for count in [64, 65] {
        let mut spec = spec();
        spec.plans = vec![base.clone(); count];
        let result = StagedRequestV3::new(spec, &support::schema(), &advertisement());
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
        StagedRequestV3::decode_with_limits(
            &request().encode().unwrap(),
            &support::schema(),
            &advertisement(),
            limits
        )
        .is_err()
    );
    let mut spec = spec();
    spec.plans[0] = entry(examples::expression_plan(
        type_bridge_contract::query_plan::v3::Expr::Operand(support::literal(1)),
    ));
    let write = StagedRequestV3::new(spec, &support::schema(), &advertisement()).unwrap();
    assert_eq!(write.kind(), StagedKindV3::Write);
}
#[test]
fn capability_admission_and_aggregate_answer_budgets_fail_closed() {
    let ad = RemoteCapabilities::new(
        query_plan_v3_capability_vocabulary(),
        advertisement().executor().clone(),
        Keys::new().public_key(),
    );
    assert!(StagedRequestV3::new(spec(), &support::schema(), &ad).is_err());
    let mut spec = spec();
    spec.limits.max_items = 2;
    let request = StagedRequestV3::new(spec, &support::schema(), &advertisement()).unwrap();
    assert!(StagedReplyV3::new(&request, StagedOutcomeV3::Committed, answers()).is_err());
    let mut spec = crate::spec();
    spec.limits.max_bytes = 1;
    let request = StagedRequestV3::new(spec, &support::schema(), &advertisement()).unwrap();
    assert!(
        StagedReplyV3::new(&request, StagedOutcomeV3::Committed, answers())
            .unwrap()
            .encode_signed(&request, &advertisement(), &Keys::new())
            .is_err()
    );
    let failed = StagedReplyV3::new(
        &request,
        StagedOutcomeV3::Failed {
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
        vec![StagedAnswerV3::Count { value: 1 }; 3],
        vec![
            StagedAnswerV3::Rows { rows: vec![vec![]] },
            StagedAnswerV3::Matched { value: 2 },
            StagedAnswerV3::Count { value: 1 },
        ],
        vec![
            StagedAnswerV3::Rows {
                rows: vec![vec![RemoteValue::Thing {
                    iid: "SECRET_SENTINEL".into(),
                    type_id: support::entity("account"),
                }]],
            },
            StagedAnswerV3::Matched { value: 2 },
            StagedAnswerV3::Count { value: 1 },
        ],
    ] {
        let error = StagedReplyV3::new(&request, StagedOutcomeV3::Committed, answers).unwrap_err();
        assert!(!format!("{error:?}").contains("SECRET_SENTINEL"));
    }
}
#[test]
fn staged_envelope_goldens_are_exact_and_sdk_readable() {
    let request = request();
    let ad = advertisement();
    let keys = Keys::new();
    let success = StagedReplyV3::new(&request, StagedOutcomeV3::Committed, answers()).unwrap();
    let failure = StagedReplyV3::new(
        &request,
        StagedOutcomeV3::Failed {
            diagnostic: SdkExecutionDiagnostic::commit_failure(SdkCommitFailureOutcome::Unknown),
        },
        vec![],
    )
    .unwrap();
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/query-remote-v3");
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
    let document_request = StagedRequestV3::new(document_spec, &support::schema(), &ad).unwrap();
    let (_, mut entities) = fetch_documents(false);
    let mut second = entities[0].clone();
    second.insert(
        "_iid".into(),
        RemoteFieldValue::Scalar {
            value: type_bridge_contract::value::CanonicalValue::String(
                type_bridge_contract::value::CanonicalString::new("0x03").unwrap(),
            ),
        },
    );
    entities.push(second);
    let (_, relations) = fetch_documents(true);
    let documents = StagedReplyV3::new(
        &document_request,
        StagedOutcomeV3::Committed,
        vec![
            StagedAnswerV3::Documents {
                documents: entities,
            },
            StagedAnswerV3::Documents {
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
    if std::env::var_os("TYPE_BRIDGE_UPDATE_REMOTE_V3_FIXTURES").is_some() {
        std::fs::create_dir_all(&root).unwrap();
    }
    for (name, bytes) in fixtures {
        if std::env::var_os("TYPE_BRIDGE_UPDATE_REMOTE_V3_FIXTURES").is_some() {
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
    let tight = StagedRequestV3::new(spec, &support::schema(), &advertisement()).unwrap();
    let normal = request();
    let signed = StagedReplyV3::new(&normal, StagedOutcomeV3::Committed, answers())
        .unwrap()
        .encode_signed(&normal, &advertisement(), &Keys::new())
        .unwrap();
    let mut wire: Value = serde_json::from_slice(&signed).unwrap();
    // The injected verifier isolates budget ordering; genuine Ed25519 is
    // covered above. Malformed owned evidence must not be interpreted first.
    wire["payload"]["request"] = json!(tight.fingerprint().unwrap().digest().to_hex());
    wire["payload"]["answers"] = json!("not-an-answer-array");
    let error = VerifiedStagedReplyV3::decode(
        &to_canonical_json(&wire).unwrap(),
        &tight,
        &advertisement(),
        Keys::new().public_key(),
        &Accept,
    )
    .unwrap_err();
    assert_eq!(error.code().as_str(), "query_remote_v3_reply_bytes");
    wire["payload"]["outcome"] =
        json!({"kind":"failed","diagnostic":{"message":"x".repeat(65_537)}});
    wire["payload"]["answers"] = json!([]);
    let error = VerifiedStagedReplyV3::decode(
        &to_canonical_json(&wire).unwrap(),
        &tight,
        &advertisement(),
        Keys::new().public_key(),
        &Accept,
    )
    .unwrap_err();
    assert_eq!(error.code().as_str(), "query_remote_v3_diagnostic_limit");
    wire["payload"]["outcome"] = json!({"kind":"failed"});
    let error = VerifiedStagedReplyV3::decode(
        &to_canonical_json(&wire).unwrap(),
        &tight,
        &advertisement(),
        Keys::new().public_key(),
        &Accept,
    )
    .unwrap_err();
    assert_eq!(error.code().as_str(), "query_remote_v3_reply_shape");
}

#[test]
fn read_kind_and_template_rows_remain_explicit() {
    let mut spec = spec();
    let read = spec.plans.pop().unwrap();
    spec.plans = vec![read];
    let read = StagedRequestV3::new(spec, &support::schema(), &advertisement()).unwrap();
    assert_eq!(read.kind(), StagedKindV3::Read);
    let bytes = read.encode().unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(value.get("kind").is_none());
    let batch = examples::batch();
    let rows = batch.given_rows().unwrap().to_vec();
    let mut wire: Value = serde_json::from_slice(&batch.canonical_bytes().unwrap()).unwrap();
    wire.as_object_mut().unwrap().remove("given_rows");
    let template = type_bridge_contract::query_plan::v3::decode_query_plan_v3(
        &to_canonical_json(&wire).unwrap(),
        &support::schema(),
    )
    .unwrap();
    assert!(StagedPlanV3::new(&template, vec![], &support::schema()).is_err());
    let bound = StagedPlanV3::new(&template, rows.clone(), &support::schema()).unwrap();
    assert!(bound.plan().given_rows().is_none());
    assert_eq!(
        bound.bound_plan(&support::schema()).unwrap().given_rows(),
        Some(rows.as_slice())
    );
}

#[test]
fn opaque_plan_bytes_do_not_inherit_the_scalar_string_ceiling() {
    use type_bridge_contract::query_plan::QueryOperand;
    use type_bridge_contract::query_plan::v3::Expr;
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
    let request = StagedRequestV3::new(spec, &support::schema(), &advertisement()).unwrap();
    let bytes = request.encode().unwrap();
    assert!(bytes.len() > 2 * type_bridge_contract::limits::MAX_CANONICAL_STRING_BYTES);
    assert_eq!(
        StagedRequestV3::decode(&bytes, &support::schema(), &advertisement()).unwrap(),
        request
    );
    let mut tight = QueryLimits::CANONICAL;
    tight.codec.max_string_bytes = 512 * 1024;
    assert!(
        StagedRequestV3::decode_with_limits(&bytes, &support::schema(), &advertisement(), tight)
            .is_err()
    );
}

#[test]
fn staged_requests_admit_fetch_and_attach_capabilities() {
    for plan in [examples::fetch_written(false), examples::attach()] {
        let mut request = spec();
        request.plans = vec![entry(plan)];
        StagedRequestV3::new(request, &support::schema(), &advertisement()).unwrap();
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

fn fetch_documents(relation: bool) -> (StagedRequestV3, Vec<BTreeMap<String, RemoteFieldValue>>) {
    let mut spec = spec();
    spec.plans = vec![entry(examples::fetch_written(relation))];
    let request = StagedRequestV3::new(spec, &support::schema(), &advertisement()).unwrap();
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
                "id":{"kind":"scalar","value":{"kind":"string","value":"alice"}},
                "score":{"kind":"scalar","value":{"kind":"long","value":"7"}}
            }}
        })
    };
    (request, vec![serde_json::from_value(document).unwrap()])
}

#[test]
fn fetch_documents_validate_before_signing_and_after_signature_verification() {
    for relation in [false, true] {
        let (request, documents) = fetch_documents(relation);
        let reply = StagedReplyV3::new(
            &request,
            StagedOutcomeV3::Committed,
            vec![StagedAnswerV3::Documents { documents }],
        )
        .unwrap();
        let keys = Keys::new();
        let bytes = reply
            .encode_signed(&request, &advertisement(), &keys)
            .unwrap();
        VerifiedStagedReplyV3::decode(&bytes, &request, &advertisement(), keys.public_key(), &keys)
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
        let error = StagedReplyV3::new(
            &request,
            StagedOutcomeV3::Committed,
            vec![StagedAnswerV3::Documents { documents }],
        )
        .unwrap_err();
        assert_eq!(
            error.code().as_str(),
            format!("query_remote_v3_document_{code}")
        );
    }
    let (request, mut documents) = fetch_documents(true);
    documents[0].remove("_role_0_iid");
    let error = StagedReplyV3::new(
        &request,
        StagedOutcomeV3::Committed,
        vec![StagedAnswerV3::Documents { documents }],
    )
    .unwrap_err();
    assert_eq!(error.code().as_str(), "query_remote_v3_document_role");
}

#[test]
fn fetch_document_optional_roles_require_complete_evidence() {
    use type_bridge_contract::query_plan::v3::{DocumentRole, Stage, ThingDocument};
    let plan = QueryPlanV3::new(
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
        "query_remote_v3_document_role"
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
        "query_remote_v3_document_role"
    );
}

#[test]
fn fetch_document_inherited_attributes_require_an_admitted_exact_type() {
    use type_bridge_contract::query_plan::v3::{Stage, ThingDocument};
    let plan = QueryPlanV3::new(
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
        "query_remote_v3_document_type"
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
        "query_remote_v3_document_value"
    );
}

#[test]
fn fetch_documents_distinguish_scalar_and_collection_ownerships() {
    use type_bridge_contract::codec::FormatVersion;
    use type_bridge_contract::query_plan::v3::{Stage, ThingDocument};
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
    let plan = QueryPlanV3::new(
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
        "query_remote_v3_document_value"
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
        "query_remote_v3_document_value"
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
    );
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/query-remote-v3");
    let mut fixtures = BTreeMap::new();
    fixtures.insert("advertisement-delete-snapshot.json", ad.encode().unwrap());
    for relation in [false, true] {
        let make_spec = || {
            let mut spec = spec();
            spec.plans = vec![entry(examples::delete_returning(relation))];
            spec
        };
        assert_eq!(
            StagedRequestV3::new(make_spec(), &support::schema(), &old)
                .unwrap_err()
                .code()
                .as_str(),
            "unsupported_required_capability"
        );
        let request = StagedRequestV3::new(make_spec(), &support::schema(), &ad).unwrap();
        let (_, documents) = fetch_documents(relation);
        let reply = StagedReplyV3::new(
            &request,
            StagedOutcomeV3::Committed,
            vec![StagedAnswerV3::Documents { documents }],
        )
        .unwrap();
        let bytes = reply.encode_signed(&request, &ad, &Keys::new()).unwrap();
        VerifiedStagedReplyV3::decode(
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
        if std::env::var_os("TYPE_BRIDGE_UPDATE_REMOTE_V3_FIXTURES").is_some() {
            std::fs::write(root.join(name), &bytes).unwrap();
        }
        assert_eq!(std::fs::read(root.join(name)).unwrap(), bytes);
    }
}
