//! Offline capability-negotiation coverage: authority construction under
//! declared required capabilities, client-side advertisement preflight,
//! and executor admission of multi-row transport requirements.

use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::limits::MAX_REMOTE_ENVELOPE_BYTES;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
use type_bridge_contract::query_plan::{
    InputColumn, InputColumnId, InputRow, QueryInvocation, QueryOperation, QueryOutput,
    QueryPattern, QueryPlan, ReadStage,
};
use type_bridge_contract::query_remote::{
    MAX_REMOTE_CLOCK_SKEW_MS, RemoteCapabilities, RemoteExecutorBinding, RemoteValue,
};
use type_bridge_contract::query_remote_v2::{
    RemoteLimitsV2, RemoteOutcomeV2, RemoteQueryRequestV2, RemoteQueryResponseV2,
    RemoteReplyDecodeLimitsV2, RemoteRequestFingerprintV2, RemoteResultKindV2,
    query_remote_v2_required_capabilities,
};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, OwnsFact, OwnsFactId, SchemaFact, SourceSpan, SourcedSchemaFact,
    TypeFact, ValueFact, ValueFactId, encode_declared_schema,
};
use type_bridge_contract::value::{
    CanonicalString, CanonicalValue, ValueTypeTag, ValueTypeTag as Tag,
};
use type_bridge_contract::{query_given_rows_capability, query_plan_capability_vocabulary};
use type_bridge_orm::query_v2_prepared::{
    QueryAuthority, decode_remote_capabilities, prepare_remote_query_v2,
};
use type_bridge_orm::query_v2_remote::{
    RemoteReplySigningKey, decode_remote_outcome_v2, encode_remote_request_v2,
    encode_remote_request_v2_at, preflight_remote_request_v2, preflight_remote_request_v2_at,
};
use type_bridge_orm::session::backend::AnswerLimits;
use type_bridge_query::MigrationAssertionValidationContext;
use type_bridge_schema::{ManagedDeltaContext, managed_schema_state, resolve};

const SCOPE: &str = "query-v2-capabilities";
const PROFILE: &str = "typedb-3.12.1/v1";

fn reply_signer() -> RemoteReplySigningKey {
    RemoteReplySigningKey::from_secret_bytes([0x17; 32])
}

fn type_id(kind: TypeKind, label: &str) -> TypeId {
    TypeId::new(kind, label).expect("fixture type")
}

fn binding(id: u16, variable: &str) -> AssertionBinding {
    AssertionBinding::new(
        BindingId::new(id).expect("binding id"),
        QueryVariable::new(variable).expect("variable"),
    )
}

fn binding_id(id: u16) -> BindingId {
    BindingId::new(id).expect("binding id")
}

fn declared_schema(required: CapabilitySet) -> DeclaredSchema {
    let person = type_id(TypeKind::Entity, "person");
    let name = AttributeId::new("name").expect("attribute");
    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).expect("type fact")),
        SchemaFact::Type(TypeFact::new(type_id(TypeKind::Attribute, "name")).expect("type fact")),
        SchemaFact::Value(ValueFact::new(ValueFactId::new(name.clone()), Tag::String)),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person, name).expect("owns id"),
        )),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).expect("byte");
        let line = u32::try_from(index + 1).expect("line");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-v2-capabilities-fixture").expect("document"),
                byte,
                byte + 1,
                line,
                1,
                line,
                2,
            )
            .expect("span"),
        )
    });
    DeclaredSchema::from_facts(FormatVersion::V1, required, sourced).expect("declared schema")
}

fn plan_with_input_v2(
    managed: &type_bridge_contract::schema_delta::ManagedSchemaState,
) -> QueryPlan {
    QueryPlan::new_v2(
        vec![binding(0, "person"), binding(1, "name")],
        vec![InputColumn::new(
            InputColumnId::new(0),
            QueryVariable::new("wanted_name").expect("input name"),
            ValueTypeTag::String,
            false,
        )],
        vec![ReadStage::Match {
            patterns: vec![
                QueryPattern::Isa {
                    binding: binding_id(0),
                    include_subtypes: true,
                    type_id: type_id(TypeKind::Entity, "person"),
                },
                QueryPattern::Has {
                    attribute: binding_id(1),
                    attribute_id: AttributeId::new("name").expect("attribute"),
                    owner: binding_id(0),
                },
            ],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .expect("V2 plan")
}

fn plan_with_typed_input(
    managed: &type_bridge_contract::schema_delta::ManagedSchemaState,
    value_type: ValueTypeTag,
    optional: bool,
) -> QueryPlan {
    QueryPlan::new(
        vec![binding(0, "person"), binding(1, "name")],
        vec![InputColumn::new(
            InputColumnId::new(0),
            QueryVariable::new("wanted_name").expect("input name"),
            value_type,
            optional,
        )],
        vec![ReadStage::Match {
            patterns: vec![
                QueryPattern::Isa {
                    binding: binding_id(0),
                    include_subtypes: true,
                    type_id: type_id(TypeKind::Entity, "person"),
                },
                QueryPattern::Has {
                    attribute: binding_id(1),
                    attribute_id: AttributeId::new("name").expect("attribute"),
                    owner: binding_id(0),
                },
            ],
        }],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .expect("plan")
}

fn invocation_json(rows: &[&str]) -> String {
    let rows: Vec<_> = rows
        .iter()
        .map(|value| vec![serde_json::json!({"kind": "string", "value": value})])
        .collect();
    serde_json::json!({"operation": "rows", "rows": rows}).to_string()
}

fn remote_capabilities_v2(with_given_rows: bool) -> RemoteCapabilities {
    let mut capabilities = query_plan_capability_vocabulary();
    for capability in query_remote_v2_required_capabilities(false) {
        capabilities.insert(capability);
    }
    if with_given_rows {
        capabilities.insert(query_given_rows_capability());
    }
    RemoteCapabilities::new(
        capabilities,
        RemoteExecutorBinding::new(
            "orm-capabilities-v2-executor",
            "orm-capabilities-v2-epoch-0001",
        )
        .expect("executor binding"),
        reply_signer().public_key(),
    )
}

fn advertisement(with_given_rows: bool) -> Vec<u8> {
    remote_capabilities_v2(with_given_rows)
        .encode()
        .expect("advertisement bytes")
}

fn limits_v2() -> RemoteLimitsV2 {
    RemoteLimitsV2 {
        deadline_ms: None,
        max_bytes: 1 << 20,
        max_items: 100,
        max_collection_members: 1 << 16,
        max_graph_nodes: 1 << 16,
        max_attribute_values: 1 << 16,
        max_role_players: 1 << 16,
        max_statements: 3,
    }
}

struct Fixture {
    authority: QueryAuthority,
    plan_bytes: Vec<u8>,
}

fn fixture_v2(required: CapabilitySet) -> Fixture {
    let declared = declared_schema(required);
    let bytes = encode_declared_schema(&declared).expect("declared bytes");
    let authority =
        QueryAuthority::from_declared_bytes(&bytes, SCOPE, PROFILE).expect("authority builds");
    let profile = SemanticProfileId::new(PROFILE).expect("profile");
    let managed = managed_schema_state(
        &declared,
        &ManagedDeltaContext::new(
            ManagedScopeId::new(SCOPE).expect("scope"),
            profile,
            declared.required_capabilities().clone(),
        ),
    )
    .expect("managed state");
    let plan_bytes = plan_with_input_v2(&managed)
        .canonical_bytes()
        .expect("V2 plan bytes");
    Fixture {
        authority,
        plan_bytes,
    }
}

#[test]
fn authorities_build_for_schemas_with_nonempty_required_capabilities() {
    let mut required = CapabilitySet::new();
    required.insert(CapabilityId::new("schema.roles").expect("capability"));
    // The declared artifact is the caller's authority: its own required
    // capabilities are available during resolution, exactly as on the
    // server.
    fixture_v2(required);
}

#[test]
fn v2_preparation_decodes_one_reply_and_replays_the_second() {
    let v2 = fixture_v2(CapabilitySet::new());
    let advertisement = remote_capabilities_v2(true);
    let pending = prepare_remote_query_v2(
        &v2.authority,
        &v2.plan_bytes,
        &invocation_json(&["ada"]),
        &advertisement.encode().expect("V2 advertisement"),
        limits_v2(),
    )
    .expect("V2 pending request");
    let request_envelope =
        RemoteQueryRequestV2::decode(pending.request_bytes()).expect("V2 request");
    let request =
        RemoteRequestFingerprintV2::compute(pending.request_bytes()).expect("request fingerprint");
    let plan = type_bridge_contract::query_plan::decode_query_plan(&v2.plan_bytes)
        .expect("V2 plan decodes");
    let response = RemoteQueryResponseV2::new(
        request_envelope.nonce(),
        &plan,
        &request,
        request_envelope.result_kind(),
        RemoteOutcomeV2::Rows { rows: Vec::new() },
    )
    .expect("V2 response")
    .encode_signed(
        &advertisement
            .fingerprint()
            .expect("advertisement fingerprint"),
        &reply_signer(),
    )
    .expect("signed V2 response");
    assert_eq!(
        pending.decode_reply(&response).expect("first reply"),
        r#"{"kind":"rows","rows":[]}"#
    );
    assert_eq!(
        pending
            .decode_reply(&response)
            .expect_err("second V2 reply is replayed")
            .code()
            .as_str(),
        "query_remote_v2_reply_replayed"
    );
}

#[test]
fn reply_claim_rejects_a_forged_signer_before_outcome_materialization() {
    let fixture = fixture_v2(CapabilitySet::new());
    let advertisement = remote_capabilities_v2(true);
    let pending = prepare_remote_query_v2(
        &fixture.authority,
        &fixture.plan_bytes,
        &invocation_json(&["ada"]),
        &advertisement.encode().expect("V2 advertisement"),
        limits_v2(),
    )
    .expect("V2 pending request");
    let request_envelope =
        RemoteQueryRequestV2::decode(pending.request_bytes()).expect("V2 request");
    let request =
        RemoteRequestFingerprintV2::compute(pending.request_bytes()).expect("request fingerprint");
    let plan = type_bridge_contract::query_plan::decode_query_plan(&fixture.plan_bytes)
        .expect("V2 plan decodes");
    let response = RemoteQueryResponseV2::new(
        request_envelope.nonce(),
        &plan,
        &request,
        request_envelope.result_kind(),
        RemoteOutcomeV2::Rows {
            rows: vec![vec![RemoteValue::Absent, RemoteValue::Absent]],
        },
    )
    .expect("V2 response");
    let advertisement_fingerprint = advertisement
        .fingerprint()
        .expect("advertisement fingerprint");
    let forged = response
        .encode_signed(
            &advertisement_fingerprint,
            &RemoteReplySigningKey::from_secret_bytes([0x42; 32]),
        )
        .expect("forged V2 response");
    assert_eq!(
        pending
            .decode_reply(&forged)
            .expect_err("foreign signer cannot authenticate a V2 outcome")
            .code()
            .as_str(),
        "query_remote_signature_invalid"
    );

    let valid = response
        .encode_signed(&advertisement_fingerprint, &reply_signer())
        .expect("valid V2 response");
    assert_eq!(
        pending
            .decode_reply(&valid)
            .expect_err("a failed verification still consumes the one-shot claim")
            .code()
            .as_str(),
        "query_remote_v2_reply_replayed"
    );
}

#[test]
fn admission_rejects_expiry_and_restarted_epoch_before_provider_construction() {
    const NOW_MS: u64 = 1_800_000_000_000;
    let declared = declared_schema(CapabilitySet::new());
    let profile = SemanticProfileId::new(PROFILE).expect("profile");
    let managed = managed_schema_state(
        &declared,
        &ManagedDeltaContext::new(
            ManagedScopeId::new(SCOPE).expect("scope"),
            profile.clone(),
            CapabilitySet::new(),
        ),
    )
    .expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");
    let context = MigrationAssertionValidationContext::new(&resolved, &managed);
    let plan = plan_with_input_v2(&managed);
    let validated = type_bridge_query::validate_query_plan(
        &plan,
        &context,
        type_bridge_contract::limits::StructuralLimits::CANONICAL,
    )
    .expect("validated V2 plan");
    let invocation = QueryInvocation::new(
        &plan,
        QueryOperation::Rows,
        vec![InputRow::new(vec![Some(CanonicalValue::String(
            CanonicalString::new("ada").expect("input"),
        ))])],
    )
    .expect("invocation");
    let advertisement = remote_capabilities_v2(true);
    let mut bounded = limits_v2();
    bounded.deadline_ms = Some(1_000);
    let request = type_bridge_orm::query_v2_remote::encode_remote_request_v2_at(
        &validated,
        &invocation,
        &advertisement,
        bounded,
        "absolute-v2-expiry-nonce-0001",
        NOW_MS,
    )
    .expect("V2 request");

    preflight_remote_request_v2_at(
        &request,
        &context,
        &advertisement,
        AnswerLimits::default(),
        NOW_MS,
    )
    .map(|_| ())
    .expect("live V2 request admits");
    let skewed = preflight_remote_request_v2_at(
        &request,
        &context,
        &advertisement,
        AnswerLimits::default(),
        NOW_MS - MAX_REMOTE_CLOCK_SKEW_MS,
    )
    .expect("maximum documented skew is admitted");
    assert_eq!(
        skewed
            .replay_until()
            .duration_since(skewed.deadline().expect("execution deadline")),
        std::time::Duration::from_millis(MAX_REMOTE_CLOCK_SKEW_MS),
        "replay retention reaches absolute expiry without granting execution time",
    );
    let expired = preflight_remote_request_v2_at(
        &request,
        &context,
        &advertisement,
        AnswerLimits::default(),
        NOW_MS + 1_000,
    )
    .map(|_| ())
    .expect_err("expired V2 request rejects");
    assert_eq!(expired.diagnostic_code(), "query_remote_v2_request_expired");

    let restarted = RemoteCapabilities::new(
        advertisement.capabilities().clone(),
        RemoteExecutorBinding::new(
            "orm-capabilities-v2-executor",
            "orm-capabilities-v2-restarted",
        )
        .expect("restarted epoch"),
        advertisement.reply_key(),
    );
    let mismatch = preflight_remote_request_v2_at(
        &request,
        &context,
        &restarted,
        AnswerLimits::default(),
        NOW_MS,
    )
    .map(|_| ())
    .expect_err("captured V2 request cannot cross epoch");
    assert_eq!(
        mismatch.diagnostic_code(),
        "query_remote_v2_advertisement_mismatch"
    );
}

#[test]
fn encode_refuses_executors_that_do_not_advertise_the_plan() {
    let fixture = fixture_v2(CapabilitySet::new());
    // The executor speaks the V2 envelope but advertises none of the plan
    // vocabulary: the plan itself is what must be refused.
    let mut envelope_only = CapabilitySet::new();
    for capability in query_remote_v2_required_capabilities(false) {
        envelope_only.insert(capability);
    }
    let starved = RemoteCapabilities::new(
        envelope_only,
        RemoteExecutorBinding::new(
            "orm-capabilities-v2-executor",
            "orm-capabilities-v2-epoch-0001",
        )
        .expect("executor binding"),
        reply_signer().public_key(),
    )
    .encode()
    .expect("starved advertisement");
    let error = prepare_remote_query_v2(
        &fixture.authority,
        &fixture.plan_bytes,
        &invocation_json(&["ada"]),
        &starved,
        limits_v2(),
    )
    .expect_err("plan must be refused before any bytes exist");
    assert_eq!(error.code().as_str(), "unsupported_required_capability");
}

#[test]
fn encode_refuses_multi_row_batches_without_the_given_transport() {
    let fixture = fixture_v2(CapabilitySet::new());
    let error = prepare_remote_query_v2(
        &fixture.authority,
        &fixture.plan_bytes,
        &invocation_json(&["ada", "bob"]),
        &advertisement(false),
        limits_v2(),
    )
    .expect_err("multi-row batch must be refused without given transport");
    assert_eq!(error.code().as_str(), "unsupported_required_capability");

    prepare_remote_query_v2(
        &fixture.authority,
        &fixture.plan_bytes,
        &invocation_json(&["ada", "bob"]),
        &advertisement(true),
        limits_v2(),
    )
    .expect("advertised given transport admits the batch");
}

#[test]
fn preflight_rejects_multi_row_batches_the_executor_cannot_transport() {
    let declared = declared_schema(CapabilitySet::new());
    let bytes = encode_declared_schema(&declared).expect("declared bytes");
    let authority =
        QueryAuthority::from_declared_bytes(&bytes, SCOPE, PROFILE).expect("authority builds");
    let profile = SemanticProfileId::new(PROFILE).expect("profile");
    let managed = managed_schema_state(
        &declared,
        &ManagedDeltaContext::new(
            ManagedScopeId::new(SCOPE).expect("scope"),
            profile.clone(),
            CapabilitySet::new(),
        ),
    )
    .expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");
    let plan = plan_with_input_v2(&managed);
    let plan_bytes = plan.canonical_bytes().expect("plan bytes");

    let pending = prepare_remote_query_v2(
        &authority,
        &plan_bytes,
        &invocation_json(&["ada", "bob"]),
        &advertisement(true),
        limits_v2(),
    )
    .expect("client with a truthful advertisement encodes");

    // A forged client can bind a request to the exact no-given advertisement
    // without performing client preflight. The executor must independently
    // reject its multi-row transport before any provider resource exists.
    let context = MigrationAssertionValidationContext::new(&resolved, &managed);
    let without_given = remote_capabilities_v2(false);
    let validated = type_bridge_query::validate_query_plan(
        &plan,
        &context,
        type_bridge_contract::limits::StructuralLimits::CANONICAL,
    )
    .expect("validated plan");
    let invocation = QueryInvocation::new(
        &plan,
        QueryOperation::Rows,
        vec![InputRow::new(vec![Some(CanonicalValue::String(
            CanonicalString::new("ada").expect("input"),
        ))])],
    )
    .expect("invocation");
    let single_row = encode_remote_request_v2(
        &validated,
        &invocation,
        &without_given,
        limits_v2(),
        "forged-client-nonce-00000001",
    )
    .expect("single-row request needs no given transport");
    // The contract constructor refuses the batch itself, so the forgery
    // splices a second given row into the single-row envelope bytes.
    let mut forged: serde_json::Value = serde_json::from_slice(&single_row).expect("request JSON");
    let rows = forged["rows"].as_array_mut().expect("given rows");
    let second = rows[0].clone();
    rows.push(second);
    let forged = serde_json::to_vec(&forged).expect("forged request bytes");
    let rejection =
        preflight_remote_request_v2(&forged, &context, &without_given, AnswerLimits::default())
            .map(|_| ())
            .expect_err("admission must fail without the transport capability");
    assert_eq!(
        rejection.diagnostic_code(),
        "unsupported_required_capability"
    );
    let envelope = rejection.into_failure_envelope(
        &without_given
            .fingerprint()
            .expect("advertisement fingerprint"),
        &reply_signer(),
    );
    let body = String::from_utf8(envelope).expect("failure envelope is JSON");
    assert!(
        body.contains("unsupported_required_capability") && body.contains("query.input.given-rows"),
        "{body}"
    );

    let advertised = remote_capabilities_v2(true);
    preflight_remote_request_v2(
        pending.request_bytes(),
        &context,
        &advertised,
        AnswerLimits::default(),
    )
    .map(|_| ())
    .expect("advertised transport admits the request");
}

#[test]
fn temporal_batches_admit_and_provider_invalid_values_reject_on_both_remote_preflights() {
    const NOW: u64 = 1_800_000_000_000;

    let declared = declared_schema(CapabilitySet::new());
    let profile = SemanticProfileId::new(PROFILE).expect("profile");
    let managed = managed_schema_state(
        &declared,
        &ManagedDeltaContext::new(
            ManagedScopeId::new(SCOPE).expect("scope"),
            profile.clone(),
            CapabilitySet::new(),
        ),
    )
    .expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");
    let context = MigrationAssertionValidationContext::new(&resolved, &managed);
    let advertised = remote_capabilities_v2(true);

    let temporal_plan = plan_with_typed_input(&managed, ValueTypeTag::DateTimeTz, false);
    let temporal_validated = type_bridge_query::validate_query_plan(
        &temporal_plan,
        &context,
        type_bridge_contract::limits::StructuralLimits::CANONICAL,
    )
    .expect("validated temporal plan");
    let local = "2026-07-13T10:30:00"
        .parse::<type_bridge_contract::temporal::CanonicalDateTime>()
        .expect("datetime");
    let temporal = CanonicalValue::DateTimeTz(
        type_bridge_contract::temporal::CanonicalDateTimeTz::new_named_resolved(
            local,
            "Europe/Amsterdam",
            7_200,
        )
        .expect("resolved datetime-tz"),
    );
    let temporal_invocation = QueryInvocation::new(
        &temporal_plan,
        QueryOperation::Rows,
        vec![
            InputRow::new(vec![Some(temporal.clone())]),
            InputRow::new(vec![Some(temporal)]),
        ],
    )
    .expect("temporal invocation");
    let temporal_request = encode_remote_request_v2_at(
        &temporal_validated,
        &temporal_invocation,
        &advertised,
        limits_v2(),
        "temporal-preflight-nonce-000001",
        NOW,
    )
    .expect("client preflight admits datetime-tz given rows");
    preflight_remote_request_v2_at(
        &temporal_request,
        &context,
        &advertised,
        AnswerLimits::default(),
        NOW,
    )
    .map(|_| ())
    .expect("executor preflight admits datetime-tz given rows");

    let duration_plan = plan_with_typed_input(&managed, ValueTypeTag::Duration, false);
    let duration_validated = type_bridge_query::validate_query_plan(
        &duration_plan,
        &context,
        type_bridge_contract::limits::StructuralLimits::CANONICAL,
    )
    .expect("validated duration plan");
    let duration = CanonicalValue::Duration("P1DT2S".parse().expect("duration"));
    let duration_invocation = QueryInvocation::new(
        &duration_plan,
        QueryOperation::Rows,
        vec![
            InputRow::new(vec![Some(duration.clone())]),
            InputRow::new(vec![Some(duration)]),
        ],
    )
    .expect("portable duration invocation");
    let duration_request = encode_remote_request_v2_at(
        &duration_validated,
        &duration_invocation,
        &advertised,
        limits_v2(),
        "duration-valid-nonce-00000001",
        NOW,
    )
    .expect("client preflight admits exact-component duration given rows");
    preflight_remote_request_v2_at(
        &duration_request,
        &context,
        &advertised,
        AnswerLimits::default(),
        NOW,
    )
    .map(|_| ())
    .expect("executor preflight admits exact-component duration given rows");

    let invalid_duration = CanonicalValue::Duration("-P1D".parse().expect("negative duration"));
    let invalid_invocation = QueryInvocation::new(
        &duration_plan,
        QueryOperation::Rows,
        vec![InputRow::new(vec![Some(invalid_duration)])],
    )
    .expect("contract-valid duration invocation");
    let client_error = encode_remote_request_v2_at(
        &duration_validated,
        &invalid_invocation,
        &advertised,
        limits_v2(),
        "duration-client-nonce-0000001",
        NOW,
    )
    .expect_err("client rejects a provider-invalid duration before request construction");
    assert_eq!(
        client_error.code().as_str(),
        "provider_duration_out_of_range"
    );

    // A forged client can bypass ORM preflight by constructing the lower
    // contract envelope directly. Executor admission must independently make
    // the same rejection before a transaction or provider exists.
    let forged = RemoteQueryRequestV2::new(
        &duration_plan,
        &invalid_invocation,
        RemoteResultKindV2::Rows,
        &advertised,
        limits_v2(),
        "duration-server-nonce-0000001",
        NOW,
    )
    .expect("contract request")
    .encode()
    .expect("request bytes");
    let rejection = preflight_remote_request_v2_at(
        &forged,
        &context,
        &advertised,
        AnswerLimits::default(),
        NOW,
    )
    .map(|_| ())
    .expect_err("executor rejects a provider-invalid duration during admission");
    assert_eq!(
        rejection.diagnostic_code(),
        "provider_duration_out_of_range"
    );
}

#[test]
fn advertisements_decode_into_sorted_capability_ids() {
    let decoded = decode_remote_capabilities(&advertisement(true)).expect("advertisement decodes");
    assert!(decoded.contains(&"query.input.given-rows".to_owned()));
    assert!(decoded.contains(&"query.remote.envelope-v2".to_owned()));
    let mut sorted = decoded.clone();
    sorted.sort();
    assert_eq!(decoded, sorted);
    assert!(decode_remote_capabilities(b"not an advertisement").is_err());
}

fn reply_decode_limits(limits: RemoteLimitsV2) -> RemoteReplyDecodeLimitsV2 {
    RemoteReplyDecodeLimitsV2 {
        max_bytes: limits.max_bytes,
        max_items: limits.max_items,
        max_collection_members: limits.max_collection_members,
        max_graph_nodes: limits.max_graph_nodes,
        max_attribute_values: limits.max_attribute_values,
        max_role_players: limits.max_role_players,
    }
}

#[test]
fn request_bound_scalar_evidence_cannot_exceed_the_caller_item_budget() {
    let declared = declared_schema(CapabilitySet::new());
    let profile = SemanticProfileId::new(PROFILE).expect("profile");
    let managed = managed_schema_state(
        &declared,
        &ManagedDeltaContext::new(
            ManagedScopeId::new(SCOPE).expect("scope"),
            profile.clone(),
            CapabilitySet::new(),
        ),
    )
    .expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");
    let plan = plan_with_input_v2(&managed);
    let validated = type_bridge_query::validate_query_plan(
        &plan,
        &MigrationAssertionValidationContext::new(&resolved, &managed),
        type_bridge_contract::limits::StructuralLimits::CANONICAL,
    )
    .expect("validated plan");
    let input = || {
        InputRow::new(vec![Some(CanonicalValue::String(
            CanonicalString::new("ada").expect("input"),
        ))])
    };
    let advertisement = remote_capabilities_v2(true);
    let advertisement_fingerprint = advertisement
        .fingerprint()
        .expect("advertisement fingerprint");
    let signer = reply_signer();

    let mut count_limits = limits_v2();
    count_limits.max_items = 1;
    let count_nonce = "forged-count-nonce-00000001";
    let count_invocation =
        QueryInvocation::new(&plan, QueryOperation::Count, vec![input()]).expect("invocation");
    let count_request = encode_remote_request_v2(
        &validated,
        &count_invocation,
        &advertisement,
        count_limits,
        count_nonce,
    )
    .expect("request");
    let count_envelope = RemoteQueryRequestV2::decode(&count_request).expect("count envelope");
    let count_request =
        RemoteRequestFingerprintV2::compute(&count_request).expect("request fingerprint");
    let count_response = RemoteQueryResponseV2::new(
        count_nonce,
        &plan,
        &count_request,
        RemoteResultKindV2::Count,
        RemoteOutcomeV2::Count { value: 2 },
    )
    .expect("bound forged response")
    .encode_signed(&advertisement_fingerprint, &signer)
    .expect("response bytes");
    let error = decode_remote_outcome_v2(
        &count_response,
        &count_envelope,
        &validated,
        &count_request,
        &advertisement_fingerprint,
        advertisement.reply_key(),
        reply_decode_limits(count_limits),
    )
    .expect_err("count beyond the bound cannot construct an outcome");
    assert_eq!(error.code().as_str(), "query_remote_v2_item_limit");

    let mut exists_limits = limits_v2();
    exists_limits.max_items = 0;
    let exists_nonce = "forged-exists-nonce-0000001";
    let exists_invocation =
        QueryInvocation::new(&plan, QueryOperation::Exists, vec![input()]).expect("invocation");
    let exists_request = encode_remote_request_v2(
        &validated,
        &exists_invocation,
        &advertisement,
        exists_limits,
        exists_nonce,
    )
    .expect("request");
    let exists_envelope = RemoteQueryRequestV2::decode(&exists_request).expect("exists envelope");
    let exists_request =
        RemoteRequestFingerprintV2::compute(&exists_request).expect("request fingerprint");
    let exists_response = RemoteQueryResponseV2::new(
        exists_nonce,
        &plan,
        &exists_request,
        RemoteResultKindV2::Exists,
        RemoteOutcomeV2::Exists { value: true },
    )
    .expect("bound forged response")
    .encode_signed(&advertisement_fingerprint, &signer)
    .expect("response bytes");
    let error = decode_remote_outcome_v2(
        &exists_response,
        &exists_envelope,
        &validated,
        &exists_request,
        &advertisement_fingerprint,
        advertisement.reply_key(),
        reply_decode_limits(exists_limits),
    )
    .expect_err("true existence exceeds a zero-item budget");
    assert_eq!(error.code().as_str(), "query_remote_v2_item_limit");
}

#[test]
fn one_shot_claim_authenticates_before_applying_a_tiny_success_budget() {
    let fixture = fixture_v2(CapabilitySet::new());
    let advertisement = remote_capabilities_v2(true);
    let mut tiny = limits_v2();
    tiny.max_bytes = 1;
    let pending = prepare_remote_query_v2(
        &fixture.authority,
        &fixture.plan_bytes,
        &invocation_json(&["ada"]),
        &advertisement.encode().expect("advertisement bytes"),
        tiny,
    )
    .expect("pending request");
    let plan = type_bridge_contract::query_plan::decode_query_plan(&fixture.plan_bytes)
        .expect("plan decodes");
    let request_envelope =
        RemoteQueryRequestV2::decode(pending.request_bytes()).expect("request envelope");
    let request =
        RemoteRequestFingerprintV2::compute(pending.request_bytes()).expect("request fingerprint");
    let response = RemoteQueryResponseV2::new(
        request_envelope.nonce(),
        &plan,
        &request,
        request_envelope.result_kind(),
        RemoteOutcomeV2::Rows { rows: Vec::new() },
    )
    .expect("response")
    .encode_signed(
        &advertisement
            .fingerprint()
            .expect("advertisement fingerprint"),
        &reply_signer(),
    )
    .expect("signed response");

    let claim = pending.claim_reply().expect("first claim");
    assert_eq!(
        claim.response_snapshot_limit(),
        MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1),
        "the immutable snapshot preserves the protocol hard-ceiling verdict before authentication",
    );
    assert_eq!(
        claim
            .decode(&response)
            .expect_err("the authenticated success exceeds the tiny caller budget")
            .code()
            .as_str(),
        // The signed outer reply envelope is shared by every reply format, so
        // its byte-budget verdict keeps the unversioned code.
        "query_remote_response_oversized",
    );
    assert_eq!(
        pending
            .claim_reply()
            .expect_err("the losing claim remains zero-copy")
            .code()
            .as_str(),
        "query_remote_v2_reply_replayed",
    );
}

#[test]
fn pending_and_claimed_debug_redact_request_bound_state() {
    const SENTINEL: &str = "debug-secret-invocation-value-7f13";

    let fixture = fixture_v2(CapabilitySet::new());
    let advertisement = remote_capabilities_v2(true);
    let pending = prepare_remote_query_v2(
        &fixture.authority,
        &fixture.plan_bytes,
        &invocation_json(&[SENTINEL]),
        &advertisement.encode().expect("advertisement bytes"),
        limits_v2(),
    )
    .expect("pending request");
    let request_envelope =
        RemoteQueryRequestV2::decode(pending.request_bytes()).expect("request envelope");
    let nonce = request_envelope.nonce();
    let raw_request =
        std::str::from_utf8(pending.request_bytes()).expect("canonical request is UTF-8 JSON");

    let pending_debug = format!("{pending:?}");
    assert!(pending_debug.contains("<redacted>"));
    assert!(!pending_debug.contains(SENTINEL));
    assert!(!pending_debug.contains(nonce));
    assert!(!pending_debug.contains(raw_request));
    assert!(!pending_debug.contains("wanted_name"));
    assert!(!pending_debug.contains("person"));

    let claim = pending.claim_reply().expect("first claim");
    let claimed_debug = format!("{claim:?}");
    assert!(claimed_debug.contains("<redacted>"));
    assert!(!claimed_debug.contains(SENTINEL));
    assert!(!claimed_debug.contains(nonce));
    assert!(!claimed_debug.contains(raw_request));
    assert!(!claimed_debug.contains("wanted_name"));
    assert!(!claimed_debug.contains("person"));
}

#[test]
fn remote_row_evidence_rejects_noncanonical_thing_iids() {
    let declared = declared_schema(CapabilitySet::new());
    let profile = SemanticProfileId::new(PROFILE).expect("profile");
    let managed = managed_schema_state(
        &declared,
        &ManagedDeltaContext::new(
            ManagedScopeId::new(SCOPE).expect("scope"),
            profile.clone(),
            CapabilitySet::new(),
        ),
    )
    .expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");
    let plan = plan_with_input_v2(&managed);
    let validated = type_bridge_query::validate_query_plan(
        &plan,
        &MigrationAssertionValidationContext::new(&resolved, &managed),
        type_bridge_contract::limits::StructuralLimits::CANONICAL,
    )
    .expect("validated plan");
    let invocation = QueryInvocation::new(
        &plan,
        QueryOperation::Rows,
        vec![InputRow::new(vec![Some(CanonicalValue::String(
            CanonicalString::new("ada").expect("input"),
        ))])],
    )
    .expect("invocation");
    let advertisement = remote_capabilities_v2(true);
    let nonce = "malformed-iid-nonce-00000001";
    let response_limits = limits_v2();
    let request_bytes = encode_remote_request_v2(
        &validated,
        &invocation,
        &advertisement,
        response_limits,
        nonce,
    )
    .expect("request");
    let request_envelope = RemoteQueryRequestV2::decode(&request_bytes).expect("request envelope");
    let request = RemoteRequestFingerprintV2::compute(&request_bytes).expect("request fingerprint");
    let person = type_id(TypeKind::Entity, "person");
    let name = type_id(TypeKind::Attribute, "name");
    let oversized = format!(
        "0x{}",
        "a".repeat(type_bridge_contract::id::MAX_THING_IID_HEX_DIGITS + 1)
    );

    for malformed in ["0x1; delete $x;", oversized.as_str()] {
        // The V2 response constructor is the evidence gate: a malformed Thing
        // IID never becomes a signable outcome, so no decode path exists.
        let error = RemoteQueryResponseV2::new(
            nonce,
            &plan,
            &request,
            request_envelope.result_kind(),
            RemoteOutcomeV2::Rows {
                rows: vec![vec![
                    RemoteValue::Thing {
                        iid: malformed.to_owned(),
                        type_id: person.clone(),
                    },
                    RemoteValue::Attribute {
                        type_id: name.clone(),
                        value: CanonicalValue::String(
                            CanonicalString::new("ada").expect("name value"),
                        ),
                    },
                ]],
            },
        )
        .expect_err("malformed Thing IID cannot construct a remote typed row");
        assert_eq!(error.code().as_str(), "query_remote_v2_evidence_mismatch");
    }
}
