// `/v3/staged` route tests over the V2 route fixtures (mock backend, no
// provider). `include!`d into a child of the V2 test module so they share its
// private backend, authority and pipeline fixtures.

use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::query_plan::{InputColumn, InputColumnId, InputRow, QueryOperand};
use type_bridge_contract::query_plan::v3::{
    DeleteItem, InsertThing, Operation, PlanSpec, QueryPlanV3, Stage, WriteReturns,
};
use type_bridge_contract::query_remote_v3::{
    CAP_QUERY_REMOTE_IDEMPOTENCY_V3, CAP_QUERY_REMOTE_STAGED_V3, CAP_QUERY_REMOTE_WRITE_V3,
    IdempotencyKey, StagedAnswerV3, StagedKindV3, StagedLimitsV3, StagedPlanV3,
    StagedRequestSpecV3, StagedRequestV3, VerifiedStagedReplyV3,
};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::sdk_diagnostic::SdkCommitFailureOutcome;
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_contract::value::{CanonicalString, CanonicalValue};

use super::*;
use crate::interceptor::{StagedWriteAuthority, V3CommitOutcome, V3PolicyPlan};
use crate::idempotency::MAX_IDEMPOTENCY_TTL;
use crate::transport::v3::{
    STAGED_IDEMPOTENCY_NOT_AUTHORIZED, STAGED_WRITE_NOT_AUTHORIZED, staged_advertisement,
};
use type_bridge_contract::query_remote::MAX_REMOTE_DEADLINE_MS;

struct V3Fixture {
    advertisement: RemoteCapabilities,
    declared: DeclaredSchema,
    metrics: Arc<BackendMetrics>,
    query: Arc<V2QueryState>,
    router: Router,
}

fn v3_fixture(pipeline: QueryPipeline, allow_unauthenticated_writes: bool) -> V3Fixture {
    timed_v3_fixture(pipeline, allow_unauthenticated_writes, None)
}

fn timed_v3_fixture(
    pipeline: QueryPipeline,
    allow_unauthenticated_writes: bool,
    timeout: Option<Duration>,
) -> V3Fixture {
    configured_v3_fixture(
        pipeline,
        allow_unauthenticated_writes,
        timeout,
        QueryV2AnswerLimits::default(),
        None,
    )
}

fn configured_v3_fixture(
    pipeline: QueryPipeline,
    allow_unauthenticated_writes: bool,
    timeout: Option<Duration>,
    ceilings: QueryV2AnswerLimits,
    store: Option<IdempotencyConfig>,
) -> V3Fixture {
    let authority = authority_fixture(false);
    let declared = authority.declared.clone();
    let metrics = Arc::new(BackendMetrics::default());
    let database = Database::with_backend(
        Box::new(CountingBackend::new([], Arc::clone(&metrics))),
        "server-v3-route",
    );
    let query = V2QueryState::new_query_only(
        CapabilitySet::new(),
        ceilings,
        database,
        authority.declared,
        authority.delta_context,
        authority.managed,
        authority.resolved,
    )
    .expect("executor advertisement is canonical")
    .with_unauthenticated_staged_writes(allow_unauthenticated_writes);
    let query = match store {
        Some(config) => query.with_idempotency_store(Arc::new(
            IdempotencyStore::new(config).expect("idempotency configuration"),
        )),
        None => query,
    };
    let query = Arc::new(match timeout {
        Some(timeout) => query.with_execution_timeout(timeout),
        None => query,
    });
    let advertisement = staged_advertisement(&query.advertisement).expect("staged advertisement");
    let router = create_router_with_v2(Arc::new(pipeline), Arc::clone(&query));
    V3Fixture {
        advertisement,
        declared,
        metrics,
        query,
        router,
    }
}

/// The client identity the vouching test policies authenticate.
const VOUCHED_CLIENT: &str = "vouched-client";

/// A policy that authenticated its caller and vouches for staged writes.
struct VouchingPolicy;

impl Interceptor for VouchingPolicy {
    fn name(&self) -> &str {
        "vouching-policy"
    }

    fn on_v2_transport<'a>(
        &'a self,
        ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        ctx.client_id = VOUCHED_CLIENT.to_owned();
        Box::pin(async { Ok(()) })
    }

    fn on_request<'a>(
        &'a self,
        clauses: Vec<type_bridge_core_lib::ast::Clause>,
        _ctx: &'a mut RequestContext,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Vec<type_bridge_core_lib::ast::Clause>, InterceptError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move { Ok(clauses) })
    }

    fn supports_v2(&self) -> bool {
        true
    }

    fn supports_v3(&self) -> bool {
        true
    }

    fn on_v3_write_authority<'a>(
        &'a self,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<StagedWriteAuthority, InterceptError>> + Send + 'a>>
    {
        Box::pin(async { Ok(StagedWriteAuthority::Authenticated) })
    }
}

fn vouching_pipeline() -> QueryPipeline {
    PipelineBuilder::new(MockExecutor::new())
        .with_default_database("server-v2-route")
        .with_interceptor(VouchingPolicy)
        .build()
        .expect("query pipeline")
}

fn person() -> TypeId {
    TypeId::new(TypeKind::Entity, "transport-person").expect("type")
}

fn spec(operation: Operation, pipeline: Vec<Stage>, columns: Vec<BindingId>) -> PlanSpec {
    PlanSpec {
        bindings: vec![AssertionBinding::new(
            BindingId::new(0).expect("binding"),
            QueryVariable::new("person").expect("variable"),
        )],
        inputs: vec![],
        given_rows: None,
        operation,
        pipeline,
        output: QueryOutput::Rows { columns },
        managed_semantics: ManagedSemanticSchemaFingerprint::compute(
            SemanticProfileId::new("typedb-3.12.1/v1").expect("profile"),
            b"server-v3-route",
        )
        .expect("semantics"),
        compatibility: None,
        functions: vec![],
    }
}

fn insert_plan(declared: &DeclaredSchema) -> QueryPlanV3 {
    insert_returning(declared, WriteReturns::Matched, vec![])
}

fn insert_returning(
    declared: &DeclaredSchema,
    returns: WriteReturns,
    columns: Vec<BindingId>,
) -> QueryPlanV3 {
    let name = AttributeId::new("transport-name").expect("attribute");
    let thing = InsertThing {
        binding: BindingId::new(0).expect("binding"),
        type_id: person(),
        fields: vec![(
            OwnsFactId::new(person(), name).expect("owns"),
            QueryOperand::Literal {
                value: CanonicalValue::String(CanonicalString::new("ada").expect("text")),
            },
        )],
        players: vec![],
    };
    let operation = Operation::Write { returns };
    let pipeline = vec![Stage::Insert {
        things: vec![thing],
    }];
    QueryPlanV3::new(spec(operation, pipeline, columns), declared).expect("insert plan")
}

fn rows_plan(declared: &DeclaredSchema) -> QueryPlanV3 {
    let binding = BindingId::new(0).expect("binding");
    let pattern = QueryPattern::Isa {
        binding,
        include_subtypes: false,
        type_id: person(),
    };
    let pipeline = vec![Stage::Match {
        patterns: vec![pattern.into()],
    }];
    QueryPlanV3::new(spec(Operation::Rows, pipeline, vec![binding]), declared).expect("rows plan")
}

fn count_plan(declared: &DeclaredSchema) -> QueryPlanV3 {
    rows_plan(declared)
        .with_operation(Operation::Count, declared)
        .expect("count plan")
}

fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
    )
    .expect("millis")
}

fn staged(
    fixture: &V3Fixture,
    plans: &[QueryPlanV3],
    key: Option<u8>,
    nonce: &str,
) -> StagedRequestV3 {
    let spec = StagedRequestSpecV3 {
        nonce: nonce.into(),
        idempotency_key: key.map(|byte| IdempotencyKey::new(vec![byte; 16]).expect("key")),
        kind: None,
        limits: StagedLimitsV3::default(),
        plans: plans
            .iter()
            .map(|plan| StagedPlanV3::new(plan, vec![], &fixture.declared).expect("entry"))
            .collect(),
        prepared_at_unix_ms: now_ms(),
    };
    StagedRequestV3::new(spec, &fixture.declared, &fixture.advertisement).expect("request")
}

async fn call(router: &Router, method: &str, uri: &str, body: Vec<u8>) -> Vec<u8> {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("request"),
        )
        .await
        .expect("route response");
    assert_eq!(response.status(), StatusCode::OK);
    response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec()
}

async fn post(fixture: &V3Fixture, request: &StagedRequestV3) -> Vec<u8> {
    let body = request.encode().expect("request bytes");
    call(&fixture.router, "POST", "/v3/staged", body).await
}

fn verify(fixture: &V3Fixture, request: &StagedRequestV3, reply: &[u8]) -> VerifiedStagedReplyV3 {
    VerifiedStagedReplyV3::decode(
        reply,
        request,
        &fixture.advertisement,
        fixture.query.signer.public_key(),
        &Ed25519RemoteReplyVerifier,
    )
    .expect("signed staged reply verifies")
}

fn failure_code(reply: &VerifiedStagedReplyV3) -> &str {
    reply.diagnostic().expect("failed reply").code()
}

#[tokio::test]
async fn staged_capabilities_extend_without_changing_the_v2_advertisement() {
    let fixture = v3_fixture(default_pipeline(), false);
    let v3 = call(&fixture.router, "GET", "/v3/capabilities", Vec::new()).await;
    let v3 = RemoteCapabilities::decode(&v3).expect("staged advertisement");
    assert_eq!(v3, fixture.advertisement);
    for capability in [
        CAP_QUERY_REMOTE_STAGED_V3,
        CAP_QUERY_REMOTE_WRITE_V3,
        CAP_QUERY_REMOTE_IDEMPOTENCY_V3,
    ] {
        assert!(
            v3.capabilities()
                .contains(&CapabilityId::new(capability).expect("capability"))
        );
    }
    assert_eq!(v3.executor(), fixture.query.advertisement.executor());
    let v2 = fixture.query.advertisement.capabilities();
    assert!(!v2.contains(&CapabilityId::new(CAP_QUERY_REMOTE_STAGED_V3).expect("capability")));
}

#[tokio::test]
async fn staged_read_commits_and_signs_its_reply() {
    let fixture = v3_fixture(default_pipeline(), false);
    let request = staged(&fixture, &[count_plan(&fixture.declared)], None, TEST_NONCE);
    let reply = verify(&fixture, &request, &post(&fixture, &request).await);
    assert!(reply.committed());
    assert_eq!(reply.answers(), [StagedAnswerV3::Count { value: 0 }]);
    assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 1);

    // An unkeyed request replays its nonce only once.
    let replay = verify(&fixture, &request, &post(&fixture, &request).await);
    assert_eq!(failure_code(&replay), "query_remote_replay");
    assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unauthenticated_staged_write_is_refused_before_anything_opens() {
    let fixture = v3_fixture(default_pipeline(), false);
    let request = staged(&fixture, &[insert_plan(&fixture.declared)], None, TEST_NONCE);
    let reply = verify(&fixture, &request, &post(&fixture, &request).await);
    assert!(!reply.committed());
    assert_eq!(failure_code(&reply), STAGED_WRITE_NOT_AUTHORIZED);
    assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.metrics.schema_exports.load(Ordering::SeqCst), 0);
}

/// Code of a refusal signed before any request was decoded.
fn unbound_code(fixture: &V3Fixture, bytes: &[u8]) -> String {
    decode_signed_remote_failure_v2(
        bytes,
        &fixture.advertisement.fingerprint().expect("fingerprint"),
        fixture.advertisement.reply_key(),
        u64::try_from(type_bridge_contract::limits::MAX_REMOTE_ENVELOPE_BYTES)
            .expect("wire ceiling"),
        &Ed25519RemoteReplyVerifier,
    )
    .expect("unbound failure envelope")
    .diagnostic()
    .expect("failure diagnostic")
    .code()
    .as_str()
    .to_owned()
}

#[derive(Clone, Copy)]
enum WriteVouch {
    Vouch,
    Fail,
    Hang,
}

/// A V2 request policy that refuses every typed V2 plan, optionally
/// declaring V3 coverage, and answers staged write admission per `vouch`.
struct StagedGate {
    supports_v3: bool,
    vouch: WriteVouch,
    transports: AtomicUsize,
}

impl StagedGate {
    fn new(supports_v3: bool, vouch: WriteVouch) -> Arc<Self> {
        Arc::new(Self {
            supports_v3,
            vouch,
            transports: AtomicUsize::new(0),
        })
    }
}

impl Interceptor for Arc<StagedGate> {
    fn name(&self) -> &str {
        "staged-gate"
    }

    fn on_request<'a>(
        &'a self,
        clauses: Vec<type_bridge_core_lib::ast::Clause>,
        _ctx: &'a mut RequestContext,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Vec<type_bridge_core_lib::ast::Clause>, InterceptError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move { Ok(clauses) })
    }

    fn supports_v2(&self) -> bool {
        true
    }

    fn supports_v3(&self) -> bool {
        self.supports_v3
    }

    fn on_v2_transport<'a>(
        &'a self,
        _ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        self.transports.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }

    fn on_v2_request<'a>(
        &'a self,
        _request: &'a V2PolicyRequest<'a>,
        _ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        Box::pin(async {
            Err(InterceptError::AccessDenied {
                reason: "typed V2 plans are refused".into(),
            })
        })
    }

    fn on_v3_write_authority<'a>(
        &'a self,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<StagedWriteAuthority, InterceptError>> + Send + 'a>>
    {
        let vouch = self.vouch;
        Box::pin(async move {
            match vouch {
                WriteVouch::Vouch => Ok(StagedWriteAuthority::Authenticated),
                WriteVouch::Fail => Err(InterceptError::Internal("identity store down".into())),
                WriteVouch::Hang => std::future::pending().await,
            }
        })
    }
}

fn gate_pipeline(gate: &Arc<StagedGate>) -> QueryPipeline {
    PipelineBuilder::new(MockExecutor::new())
        .with_default_database("server-v2-route")
        .with_interceptor(Arc::clone(gate))
        .build()
        .expect("query pipeline")
}

#[tokio::test]
async fn v2_request_policy_without_v3_coverage_refuses_every_staged_request() {
    let gate = StagedGate::new(false, WriteVouch::Vouch);
    let fixture = v3_fixture(gate_pipeline(&gate), false);
    let request = staged(&fixture, &[count_plan(&fixture.declared)], None, TEST_NONCE);
    let reply = post(&fixture, &request).await;
    assert_eq!(unbound_code(&fixture, &reply), "query_remote_policy_rejected");
    let capabilities = call(&fixture.router, "GET", "/v3/capabilities", Vec::new()).await;
    assert_eq!(
        unbound_code(&fixture, &capabilities),
        "query_remote_policy_rejected"
    );
    // Refused before the transport hook, the decode and any provider work.
    assert_eq!(gate.transports.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.metrics.schema_exports.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.metrics.query_executions.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn v2_request_policy_declaring_v3_coverage_is_admitted() {
    let gate = StagedGate::new(true, WriteVouch::Vouch);
    let fixture = v3_fixture(gate_pipeline(&gate), false);
    let request = staged(&fixture, &[count_plan(&fixture.declared)], None, TEST_NONCE);
    let reply = verify(&fixture, &request, &post(&fixture, &request).await);
    assert!(reply.committed());
    assert_eq!(gate.transports.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn failing_write_authority_refuses_before_anything_opens() {
    let gate = StagedGate::new(true, WriteVouch::Fail);
    let fixture = v3_fixture(gate_pipeline(&gate), false);
    let request = staged(&fixture, &[insert_plan(&fixture.declared)], None, TEST_NONCE);
    let reply = verify(&fixture, &request, &post(&fixture, &request).await);
    assert!(!reply.committed());
    assert_eq!(failure_code(&reply), "query_remote_policy_rejected");
    assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.metrics.schema_exports.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.metrics.query_executions.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn hanging_write_authority_times_out_before_anything_opens() {
    let gate = StagedGate::new(true, WriteVouch::Hang);
    let fixture = timed_v3_fixture(
        gate_pipeline(&gate),
        false,
        Some(Duration::from_millis(200)),
    );
    let request = staged(&fixture, &[insert_plan(&fixture.declared)], None, TEST_NONCE);
    let reply = verify(&fixture, &request, &post(&fixture, &request).await);
    assert!(!reply.committed());
    assert_eq!(failure_code(&reply), "transaction_deadline_exceeded");
    assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.metrics.schema_exports.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.metrics.query_executions.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn vouched_or_explicitly_allowed_staged_writes_commit() {
    for fixture in [
        v3_fixture(vouching_pipeline(), false),
        v3_fixture(default_pipeline(), true),
    ] {
        let plans = [insert_plan(&fixture.declared), count_plan(&fixture.declared)];
        let request = staged(&fixture, &plans, None, TEST_NONCE);
        let reply = verify(&fixture, &request, &post(&fixture, &request).await);
        assert!(reply.committed());
        assert_eq!(
            reply.answers(),
            [
                StagedAnswerV3::Matched { value: 0 },
                StagedAnswerV3::Count { value: 0 }
            ]
        );
        assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.metrics.query_executions.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn unsignable_reply_after_a_committed_write_reports_an_unknown_outcome() {
    let fixture = v3_fixture(default_pipeline(), true);
    // A budget below any signed reply: the write commits, then cannot sign.
    let spec = StagedRequestSpecV3 {
        nonce: TEST_NONCE.into(),
        idempotency_key: None,
        kind: None,
        limits: StagedLimitsV3 {
            max_bytes: 1,
            ..StagedLimitsV3::default()
        },
        plans: vec![
            StagedPlanV3::new(&insert_plan(&fixture.declared), vec![], &fixture.declared)
                .expect("entry"),
        ],
        prepared_at_unix_ms: now_ms(),
    };
    let request =
        StagedRequestV3::new(spec, &fixture.declared, &fixture.advertisement).expect("request");
    let reply = verify(&fixture, &request, &post(&fixture, &request).await);
    assert!(!reply.committed());
    assert_eq!(failure_code(&reply), "commit_outcome_unknown");
    assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.metrics.query_executions.load(Ordering::SeqCst), 1);
}

/// A response policy that withholds every reply while `withhold` is set,
/// and records every `(success, code)` it audits.
#[derive(Default)]
struct WithholdsReplies {
    withhold: AtomicBool,
    audited: Mutex<Vec<(bool, String)>>,
}

impl Interceptor for Arc<WithholdsReplies> {
    fn name(&self) -> &str {
        "withholds-replies"
    }

    fn on_request<'a>(
        &'a self,
        clauses: Vec<type_bridge_core_lib::ast::Clause>,
        _ctx: &'a mut RequestContext,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Vec<type_bridge_core_lib::ast::Clause>, InterceptError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move { Ok(clauses) })
    }

    fn supports_v2(&self) -> bool {
        true
    }

    fn supports_v3(&self) -> bool {
        true
    }

    fn on_v2_response<'a>(
        &'a self,
        outcome: &'a V2PolicyOutcome<'a>,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        self.audited
            .lock()
            .expect("audited")
            .push((outcome.success(), outcome.code().to_owned()));
        let withhold = self.withhold.load(Ordering::SeqCst);
        Box::pin(async move {
            if withhold {
                Err(InterceptError::AccessDenied {
                    reason: "audit sink unavailable".into(),
                })
            } else {
                Ok(())
            }
        })
    }
}

fn withholding_fixture() -> (Arc<WithholdsReplies>, V3Fixture) {
    let policy = Arc::new(WithholdsReplies::default());
    let pipeline = PipelineBuilder::new(MockExecutor::new())
        .with_default_database("server-v2-route")
        .with_interceptor(VouchingPolicy)
        .with_interceptor(Arc::clone(&policy))
        .build()
        .expect("query pipeline");
    (policy, v3_fixture(pipeline, true))
}

#[tokio::test]
async fn reply_withheld_after_a_committed_write_reports_an_unknown_outcome() {
    let (policy, fixture) = withholding_fixture();
    let request = staged(&fixture, &[insert_plan(&fixture.declared)], Some(9), TEST_NONCE);
    policy.withhold.store(true, Ordering::SeqCst);
    let first = post(&fixture, &request).await;
    policy.withhold.store(false, Ordering::SeqCst);
    let reply = verify(&fixture, &request, &first);
    // Committed but withheld: never the safe-to-retry policy refusal.
    assert!(!reply.committed());
    assert_eq!(failure_code(&reply), "commit_outcome_unknown");
    let projected = reply.diagnostic().expect("failed reply").host_json().expect("json");
    assert_eq!(projected["details"]["commit_outcome"]["value"], "unknown");
    // The keyed retry replays those exact bytes and commits nothing more.
    assert_eq!(post(&fixture, &request).await, first);
    assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.metrics.query_executions.load(Ordering::SeqCst), 1);

    // A withheld read stays the policy refusal.
    let (policy, fixture) = withholding_fixture();
    policy.withhold.store(true, Ordering::SeqCst);
    let request = staged(&fixture, &[count_plan(&fixture.declared)], None, TEST_NONCE);
    let reply = verify(&fixture, &request, &post(&fixture, &request).await);
    assert_eq!(failure_code(&reply), "query_remote_policy_rejected");
}

#[tokio::test]
async fn withheld_replay_of_a_committed_write_reports_an_unknown_outcome() {
    let (policy, fixture) = withholding_fixture();
    let request = staged(&fixture, &[insert_plan(&fixture.declared)], Some(10), TEST_NONCE);
    let committed = post(&fixture, &request).await;
    assert!(verify(&fixture, &request, &committed).committed());
    // The retained bytes are opaque, so a withheld write replay reads as
    // possibly committed, identically on every attempt.
    policy.withhold.store(true, Ordering::SeqCst);
    let withheld = post(&fixture, &request).await;
    let reply = verify(&fixture, &request, &withheld);
    assert!(!reply.committed());
    assert_eq!(failure_code(&reply), "commit_outcome_unknown");
    assert_eq!(post(&fixture, &request).await, withheld);
    // The retained committed reply was not overwritten.
    policy.withhold.store(false, Ordering::SeqCst);
    assert_eq!(post(&fixture, &request).await, committed);
    assert_eq!(fixture.metrics.commits.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.metrics.query_executions.load(Ordering::SeqCst), 1);

    // A withheld read replay stays the policy refusal.
    let (policy, fixture) = withholding_fixture();
    let request = staged(&fixture, &[count_plan(&fixture.declared)], Some(11), TEST_NONCE);
    assert!(verify(&fixture, &request, &post(&fixture, &request).await).committed());
    policy.withhold.store(true, Ordering::SeqCst);
    let reply = verify(&fixture, &request, &post(&fixture, &request).await);
    assert_eq!(failure_code(&reply), "query_remote_policy_rejected");
    assert_eq!(fixture.metrics.query_executions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unmappable_answers_after_a_committed_write_report_an_unknown_outcome() {
    let fixture = v3_fixture(vouching_pipeline(), false);
    // The provider row lacks the projected column: its answer resolves
    // before the commit but cannot map into a reply after it.
    *fixture.metrics.rows.lock().expect("rows lock") = vec![serde_json::json!({})];
    let binding = BindingId::new(0).expect("binding");
    let plan = insert_returning(&fixture.declared, WriteReturns::Rows, vec![binding]);
    let request = staged(&fixture, &[plan], Some(12), TEST_NONCE);
    let first = post(&fixture, &request).await;
    let reply = verify(&fixture, &request, &first);
    assert!(!reply.committed());
    assert_eq!(failure_code(&reply), "commit_outcome_unknown");
    // The keyed retry replays those exact bytes and commits nothing more.
    assert_eq!(post(&fixture, &request).await, first);
    assert_eq!(fixture.metrics.commits.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.metrics.query_executions.load(Ordering::SeqCst), 1);

    // A read keeps its mapping diagnostic.
    let fixture = v3_fixture(default_pipeline(), true);
    *fixture.metrics.rows.lock().expect("rows lock") = vec![serde_json::json!({})];
    let request = staged(&fixture, &[rows_plan(&fixture.declared)], None, TEST_NONCE);
    let reply = verify(&fixture, &request, &post(&fixture, &request).await);
    assert_eq!(failure_code(&reply), "query_remote_v3_answer_concept");
    assert_eq!(fixture.metrics.commits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn schema_kind_is_refused_before_anything_opens() {
    let fixture = v3_fixture(default_pipeline(), true);
    let request = staged(&fixture, &[count_plan(&fixture.declared)], None, TEST_NONCE);
    let mut wire: serde_json::Value =
        serde_json::from_slice(&request.encode().expect("bytes")).expect("json");
    wire["kind"] = serde_json::json!("schema");
    let body = to_canonical_json(&wire).expect("canonical bytes");
    let reply = call(&fixture.router, "POST", "/v3/staged", body).await;
    assert!(String::from_utf8_lossy(&reply).contains("query_remote_v3_kind"));
    assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn keyed_retry_after_a_dropped_reply_replays_identical_bytes_once() {
    let fixture = v3_fixture(vouching_pipeline(), false);
    let request = staged(&fixture, &[insert_plan(&fixture.declared)], Some(7), TEST_NONCE);
    // Fault injection: the first reply is lost on the way to the caller.
    let dropped = post(&fixture, &request).await;
    let retried = post(&fixture, &request).await;
    assert_eq!(retried, dropped);
    assert!(verify(&fixture, &request, &retried).committed());
    assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.metrics.query_executions.load(Ordering::SeqCst), 1);

    // The same key over different request bytes is refused, unexecuted.
    let other = staged(
        &fixture,
        &[insert_plan(&fixture.declared)],
        Some(7),
        "server-route-nonce-000002",
    );
    let reused = verify(&fixture, &other, &post(&fixture, &other).await);
    assert_eq!(failure_code(&reused), "idempotency_key_reused");
    assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 1);
}

/// What one plan policy saw: index, capabilities, types and row count.
type SeenPlan = (usize, Vec<String>, Vec<String>, Option<usize>);
/// What one commit observer saw: kind, committed, code, certainty, provider
/// code.
type SeenCommit = (
    StagedKindV3,
    bool,
    String,
    Option<SdkCommitFailureOutcome>,
    Option<String>,
);

/// A vouching policy recording plan and commit hooks; it refuses any plan
/// requiring `refuse`.
#[derive(Default)]
struct HookPolicy {
    refuse: Option<&'static str>,
    /// The commit observer never resolves.
    hang: bool,
    wants_rows: bool,
    plans: Mutex<Vec<SeenPlan>>,
    debug: Mutex<Vec<String>>,
    commits: Mutex<Vec<SeenCommit>>,
}

impl Interceptor for Arc<HookPolicy> {
    fn name(&self) -> &str {
        "hook-policy"
    }

    fn on_v2_transport<'a>(
        &'a self,
        ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        ctx.client_id = VOUCHED_CLIENT.to_owned();
        Box::pin(async { Ok(()) })
    }

    fn on_request<'a>(
        &'a self,
        clauses: Vec<type_bridge_core_lib::ast::Clause>,
        _ctx: &'a mut RequestContext,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Vec<type_bridge_core_lib::ast::Clause>, InterceptError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move { Ok(clauses) })
    }

    fn supports_v2(&self) -> bool {
        true
    }

    fn supports_v3(&self) -> bool {
        true
    }

    fn on_v3_write_authority<'a>(
        &'a self,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<StagedWriteAuthority, InterceptError>> + Send + 'a>>
    {
        Box::pin(async { Ok(StagedWriteAuthority::Authenticated) })
    }

    fn wants_v3_plan_rows(&self) -> bool {
        self.wants_rows
    }

    fn on_v3_plan<'a>(
        &'a self,
        index: usize,
        plan: &'a V3PolicyPlan<'a>,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        self.plans.lock().expect("plans").push((
            index,
            plan.capabilities().iter().map(|cap| cap.to_string()).collect(),
            plan.types().iter().map(|ty| ty.label().to_string()).collect(),
            plan.rows().map(<[InputRow]>::len),
        ));
        self.debug.lock().expect("debug").push(format!("{plan:?}"));
        let refused = self.refuse.is_some_and(|cap| plan.requires(cap));
        Box::pin(async move {
            if refused {
                Err(InterceptError::AccessDenied {
                    reason: "refused capability".into(),
                })
            } else {
                Ok(())
            }
        })
    }

    fn on_v3_commit<'a>(
        &'a self,
        outcome: &'a V3CommitOutcome<'a>,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        self.commits.lock().expect("commits").push((
            outcome.kind(),
            outcome.committed(),
            outcome.code().to_owned(),
            outcome.certainty(),
            outcome.failure().and_then(|f| f.code()).map(str::to_owned),
        ));
        if self.hang {
            return Box::pin(std::future::pending());
        }
        // Observer errors are logged only; they never change the reply.
        Box::pin(async {
            Err(InterceptError::Internal("observer failure is ignored".into()))
        })
    }
}

fn hook_pipeline(policies: &[&Arc<HookPolicy>]) -> QueryPipeline {
    policies
        .iter()
        .fold(
            PipelineBuilder::new(MockExecutor::new()).with_default_database("server-v2-route"),
            |builder, policy| builder.with_interceptor(Arc::clone(policy)),
        )
        .build()
        .expect("query pipeline")
}

fn delete_plan(declared: &DeclaredSchema) -> QueryPlanV3 {
    let binding = BindingId::new(0).expect("binding");
    let pattern = QueryPattern::Isa {
        binding,
        include_subtypes: false,
        type_id: person(),
    };
    let pipeline = vec![
        Stage::Match {
            patterns: vec![pattern.into()],
        },
        Stage::Delete {
            items: vec![DeleteItem::Thing { binding }],
        },
    ];
    let operation = Operation::Write {
        returns: WriteReturns::Matched,
    };
    QueryPlanV3::new(spec(operation, pipeline, vec![]), declared).expect("delete plan")
}

/// An insert whose name is an input column, staged with one bound row.
fn insert_input_entry(declared: &DeclaredSchema) -> StagedPlanV3 {
    let name = AttributeId::new("transport-name").expect("attribute");
    let column = InputColumnId::new(0);
    let thing = InsertThing {
        binding: BindingId::new(0).expect("binding"),
        type_id: person(),
        fields: vec![(
            OwnsFactId::new(person(), name).expect("owns"),
            QueryOperand::Input { column },
        )],
        players: vec![],
    };
    let mut spec = spec(
        Operation::Write {
            returns: WriteReturns::Matched,
        },
        vec![Stage::Insert {
            things: vec![thing],
        }],
        vec![],
    );
    spec.inputs = vec![InputColumn::new(
        column,
        QueryVariable::new("name").expect("variable"),
        ValueTypeTag::String,
        false,
    )];
    let plan = QueryPlanV3::new(spec, declared).expect("input insert plan");
    let row = InputRow::new(vec![Some(CanonicalValue::String(
        CanonicalString::new("grace").expect("text"),
    ))]);
    StagedPlanV3::new(&plan, vec![row], declared).expect("entry")
}

#[tokio::test]
async fn plan_policy_refuses_a_staged_delete_before_reservation_or_io() {
    let policy = Arc::new(HookPolicy {
        refuse: Some("query.write.delete"),
        ..HookPolicy::default()
    });
    let fixture = v3_fixture(hook_pipeline(&[&policy]), false);
    let plans = [
        insert_plan(&fixture.declared),
        delete_plan(&fixture.declared),
        count_plan(&fixture.declared),
    ];
    let request = staged(&fixture, &plans, Some(9), TEST_NONCE);
    let reply = post(&fixture, &request).await;
    let verified = verify(&fixture, &request, &reply);
    assert!(!verified.committed());
    assert_eq!(failure_code(&verified), "query_remote_policy_rejected");
    // The refused plan stops the stack: the count plan is never consulted.
    let seen = policy.plans.lock().expect("plans").clone();
    assert_eq!(seen.iter().map(|plan| plan.0).collect::<Vec<_>>(), [0, 1]);
    assert!(seen[1].1.contains(&"query.write.delete".to_owned()));
    // Refused before live-authority I/O, staging or commit: nothing ran,
    // so the commit observer has nothing to observe.
    assert_eq!(fixture.metrics.schema_exports.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.metrics.query_executions.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.metrics.commits.load(Ordering::SeqCst), 0);
    assert!(policy.commits.lock().expect("commits").is_empty());

    // Nothing was reserved: a retry is re-authorized, never replayed ...
    let retried = verify(&fixture, &request, &post(&fixture, &request).await);
    assert_eq!(failure_code(&retried), "query_remote_policy_rejected");
    assert_eq!(policy.plans.lock().expect("plans").len(), 4);
    // ... and the same key and nonce still admit a different request.
    let admitted = staged(&fixture, &plans[..1], Some(9), TEST_NONCE);
    assert!(verify(&fixture, &admitted, &post(&fixture, &admitted).await).committed());
    assert_eq!(fixture.metrics.commits.load(Ordering::SeqCst), 1);
    // Live authority export is the I/O the refusals above skipped.
    assert!(fixture.metrics.schema_exports.load(Ordering::SeqCst) > 0);
}

#[tokio::test]
async fn plan_policy_sees_capabilities_and_types_but_rows_only_on_request() {
    let blind = Arc::new(HookPolicy::default());
    let sighted = Arc::new(HookPolicy {
        refuse: Some("query.write.insert"),
        wants_rows: true,
        ..HookPolicy::default()
    });
    let fixture = v3_fixture(hook_pipeline(&[&blind, &sighted]), false);
    let spec = StagedRequestSpecV3 {
        nonce: TEST_NONCE.into(),
        idempotency_key: None,
        kind: None,
        limits: StagedLimitsV3::default(),
        plans: vec![insert_input_entry(&fixture.declared)],
        prepared_at_unix_ms: now_ms(),
    };
    let request =
        StagedRequestV3::new(spec, &fixture.declared, &fixture.advertisement).expect("request");
    let reply = verify(&fixture, &request, &post(&fixture, &request).await);
    assert_eq!(failure_code(&reply), "query_remote_policy_rejected");
    let (index, capabilities, types, rows) = blind.plans.lock().expect("plans")[0].clone();
    assert_eq!(index, 0);
    assert!(capabilities.contains(&"query.write.insert".to_owned()));
    assert_eq!(types, ["transport-person"]);
    assert_eq!(rows, None);
    assert_eq!(sighted.plans.lock().expect("plans")[0].3, Some(1));
    // Debug output stays value-free even for the policy that sees rows.
    for policy in [&blind, &sighted] {
        let debug = policy.debug.lock().expect("debug")[0].clone();
        assert!(debug.contains("rows: Some(1)"), "{debug}");
        assert!(debug.contains("query.write.insert"), "{debug}");
        assert!(!debug.contains("grace"), "{debug}");
    }
}

#[tokio::test]
async fn commit_observer_sees_commits_and_stc2_conflicts() {
    let policy = Arc::new(HookPolicy::default());
    let fixture = v3_fixture(hook_pipeline(&[&policy]), false);
    let plans = [insert_plan(&fixture.declared)];
    let request = staged(&fixture, &plans, None, TEST_NONCE);
    // The observer's own error never changes the committed reply.
    assert!(verify(&fixture, &request, &post(&fixture, &request).await).committed());
    assert_eq!(fixture.metrics.commits.load(Ordering::SeqCst), 1);

    fixture.metrics.fail_commit.store(true, Ordering::SeqCst);
    let request = staged(&fixture, &plans, None, "server-route-nonce-000002");
    let reply = verify(&fixture, &request, &post(&fixture, &request).await);
    assert!(!reply.committed());
    assert_eq!(failure_code(&reply), "commit_definitely_aborted");
    // A successfully closed read reports as committed under its own kind.
    fixture.metrics.fail_commit.store(false, Ordering::SeqCst);
    let read = staged(
        &fixture,
        &[count_plan(&fixture.declared)],
        None,
        "server-route-nonce-000003",
    );
    assert!(verify(&fixture, &read, &post(&fixture, &read).await).committed());
    let commits = policy.commits.lock().expect("commits").clone();
    assert_eq!(
        commits,
        [
            (StagedKindV3::Write, true, "ok".to_owned(), None, None),
            (
                StagedKindV3::Write,
                false,
                "commit_definitely_aborted".to_owned(),
                Some(SdkCommitFailureOutcome::DefinitelyAborted),
                Some("STC2".to_owned()),
            ),
            (StagedKindV3::Read, true, "ok".to_owned(), None, None),
        ]
    );
}

/// A request at `prepared_at` with `limits`, outside the default helper.
fn staged_with(
    fixture: &V3Fixture,
    plans: &[QueryPlanV3],
    key: Option<u8>,
    nonce: &str,
    limits: StagedLimitsV3,
    prepared_at_unix_ms: u64,
) -> StagedRequestV3 {
    let spec = StagedRequestSpecV3 {
        nonce: nonce.into(),
        idempotency_key: key.map(|byte| IdempotencyKey::new(vec![byte; 16]).expect("key")),
        kind: None,
        limits,
        plans: plans
            .iter()
            .map(|plan| StagedPlanV3::new(plan, vec![], &fixture.declared).expect("entry"))
            .collect(),
        prepared_at_unix_ms,
    };
    StagedRequestV3::new(spec, &fixture.declared, &fixture.advertisement).expect("request")
}

fn ceilings(max_items: u64, max_bytes: u64) -> QueryV2AnswerLimits {
    let mut ceilings = QueryV2AnswerLimits::default();
    ceilings.answer.max_items = max_items;
    ceilings.answer.max_bytes = max_bytes;
    ceilings
}

#[tokio::test]
async fn staged_reads_enforce_the_advertised_answer_ceilings() {
    let fixture = configured_v3_fixture(
        default_pipeline(),
        false,
        None,
        ceilings(2, 1 << 20),
        None,
    );
    let row = serde_json::json!({"person": {"kind": "entity", "type": "transport-person", "iid": "0x1e"}});
    *fixture.metrics.rows.lock().expect("rows lock") = vec![row; 3];
    // The request allows far more items than the server's ceiling.
    let request = staged(&fixture, &[rows_plan(&fixture.declared)], None, TEST_NONCE);
    assert!(request.limits().max_items > 2);
    let reply = verify(&fixture, &request, &post(&fixture, &request).await);
    assert!(!reply.committed());
    assert_eq!(failure_code(&reply), "processed_item_limit");

    // The request's own tighter budget binds below the ceiling too.
    let fixture = v3_fixture(default_pipeline(), false);
    *fixture.metrics.rows.lock().expect("rows lock") = vec![serde_json::json!({}); 3];
    let limits = StagedLimitsV3 {
        max_items: 2,
        ..StagedLimitsV3::default()
    };
    let plans = [rows_plan(&fixture.declared)];
    let request = staged_with(&fixture, &plans, None, TEST_NONCE, limits, now_ms());
    let reply = verify(&fixture, &request, &post(&fixture, &request).await);
    assert_eq!(failure_code(&reply), "processed_item_limit");
}

#[tokio::test]
async fn keyed_requests_need_a_vouched_identified_caller() {
    // Open unauthenticated writes do not open retention, for writes or reads.
    let fixture = v3_fixture(default_pipeline(), true);
    for plan in [insert_plan(&fixture.declared), count_plan(&fixture.declared)] {
        let request = staged(&fixture, &[plan], Some(1), TEST_NONCE);
        let reply = verify(&fixture, &request, &post(&fixture, &request).await);
        assert_eq!(failure_code(&reply), STAGED_IDEMPOTENCY_NOT_AUTHORIZED);
    }
    // A vouching policy that never identified its caller is not enough.
    let gate = StagedGate::new(true, WriteVouch::Vouch);
    let fixture = v3_fixture(gate_pipeline(&gate), false);
    let request = staged(&fixture, &[count_plan(&fixture.declared)], Some(2), TEST_NONCE);
    let reply = verify(&fixture, &request, &post(&fixture, &request).await);
    assert_eq!(failure_code(&reply), STAGED_IDEMPOTENCY_NOT_AUTHORIZED);
    assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 0);

    // An identified, vouched caller may key a read.
    let fixture = v3_fixture(vouching_pipeline(), false);
    let request = staged(&fixture, &[count_plan(&fixture.declared)], Some(3), TEST_NONCE);
    assert!(verify(&fixture, &request, &post(&fixture, &request).await).committed());
}

#[tokio::test]
async fn retention_is_sized_by_the_server_ceiling_and_bounded_per_client() {
    // A 1 MiB store fits a 512 KiB ceiling's allowance, though the request
    // asks for the full envelope.
    let store = IdempotencyConfig::new(4, 1 << 20, MAX_IDEMPOTENCY_TTL).with_client_share(1);
    let fixture = configured_v3_fixture(
        vouching_pipeline(),
        false,
        None,
        ceilings(100, 512 << 10),
        Some(store),
    );
    // Each completion releases its unused allowance for the next.
    for (key, nonce) in [(1, TEST_NONCE), (2, "server-route-nonce-000002")] {
        let request = staged(&fixture, &[insert_plan(&fixture.declared)], Some(key), nonce);
        assert!(verify(&fixture, &request, &post(&fixture, &request).await).committed());
    }

    // Store-wide capacity, then the per-client share, surface their codes.
    let store = IdempotencyConfig::new(1, 1 << 20, MAX_IDEMPOTENCY_TTL).with_client_share(1);
    let fixture = configured_v3_fixture(
        vouching_pipeline(),
        false,
        None,
        ceilings(100, 64 << 10),
        Some(store),
    );
    let plans = [count_plan(&fixture.declared)];
    let first = staged(&fixture, &plans, Some(1), TEST_NONCE);
    assert!(verify(&fixture, &first, &post(&fixture, &first).await).committed());
    let second = staged(&fixture, &plans, Some(2), "server-route-nonce-000002");
    let reply = verify(&fixture, &second, &post(&fixture, &second).await);
    assert_eq!(failure_code(&reply), "idempotency_capacity");

    let store = IdempotencyConfig::new(4, 1 << 20, MAX_IDEMPOTENCY_TTL).with_client_share(4);
    let fixture = configured_v3_fixture(
        vouching_pipeline(),
        false,
        None,
        ceilings(100, 64 << 10),
        Some(store),
    );
    let first = staged(&fixture, &plans, Some(1), TEST_NONCE);
    assert!(verify(&fixture, &first, &post(&fixture, &first).await).committed());
    let second = staged(&fixture, &plans, Some(2), "server-route-nonce-000002");
    let reply = verify(&fixture, &second, &post(&fixture, &second).await);
    assert_eq!(failure_code(&reply), "idempotency_client_capacity");
    assert_eq!(fixture.metrics.query_executions.load(Ordering::SeqCst), 1);
}

/// Poll `done` until it holds, failing after five seconds.
async fn eventually(mut done: impl FnMut() -> bool) {
    for _ in 0..500 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition never held");
}

#[tokio::test]
async fn client_disconnect_mid_commit_still_retains_and_observes_the_outcome() {
    let policy = Arc::new(HookPolicy::default());
    let fixture = v3_fixture(hook_pipeline(&[&policy]), false);
    let request = staged(&fixture, &[insert_plan(&fixture.declared)], Some(5), TEST_NONCE);
    let body = request.encode().expect("request bytes");
    fixture.metrics.hold_commit.store(true, Ordering::SeqCst);
    let router = fixture.router.clone();
    let handler = tokio::spawn(async move { call(&router, "POST", "/v3/staged", body).await });
    let metrics = Arc::clone(&fixture.metrics);
    eventually(|| metrics.query_executions.load(Ordering::SeqCst) == 1).await;

    // The commit is in flight: a retry waits, and the client goes away.
    let retry = verify(&fixture, &request, &post(&fixture, &request).await);
    assert_eq!(failure_code(&retry), "retry_later");
    handler.abort();
    assert!(handler.await.is_err_and(|error| error.is_cancelled()));
    fixture.metrics.release_commit.notify_one();
    eventually(|| policy.commits.lock().expect("commits").len() == 1).await;

    // The detached outcome was retained: the retry replays the commit.
    let mut replay = None;
    for _ in 0..500 {
        let bytes = post(&fixture, &request).await;
        let reply = verify(&fixture, &request, &bytes);
        if reply.committed() {
            replay = Some(bytes);
            break;
        }
        assert_eq!(failure_code(&reply), "retry_later");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let replay = replay.expect("the committed reply is retained");
    assert_eq!(post(&fixture, &request).await, replay);
    assert_eq!(fixture.metrics.commits.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.metrics.query_executions.load(Ordering::SeqCst), 1);
    let commits = policy.commits.lock().expect("commits").clone();
    assert_eq!(
        commits,
        [(StagedKindV3::Write, true, "ok".to_owned(), None, None)]
    );
}

#[tokio::test]
async fn hanging_commit_observer_times_out_without_blocking_the_reply() {
    let policy = Arc::new(HookPolicy {
        hang: true,
        ..HookPolicy::default()
    });
    let fixture = v3_fixture(hook_pipeline(&[&policy]), false);
    let request = staged(&fixture, &[insert_plan(&fixture.declared)], None, TEST_NONCE);
    let reply = tokio::time::timeout(Duration::from_secs(5), post(&fixture, &request))
        .await
        .expect("the reply does not wait for the observer");
    assert!(verify(&fixture, &request, &reply).committed());
    assert_eq!(policy.commits.lock().expect("commits").len(), 1);
}

#[tokio::test]
async fn withheld_keyed_read_is_retained_as_its_refusal() {
    let (policy, fixture) = withholding_fixture();
    let request = staged(&fixture, &[count_plan(&fixture.declared)], Some(13), TEST_NONCE);
    policy.withhold.store(true, Ordering::SeqCst);
    let refused = post(&fixture, &request).await;
    assert_eq!(
        failure_code(&verify(&fixture, &request, &refused)),
        "query_remote_policy_rejected"
    );
    // Once the policy relents, the retry still gets the refusal, never the
    // withheld answer, and nothing re-executes.
    policy.withhold.store(false, Ordering::SeqCst);
    assert_eq!(post(&fixture, &request).await, refused);
    assert_eq!(fixture.metrics.query_executions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn retained_failure_replays_with_its_failure_outcome() {
    let (policy, fixture) = withholding_fixture();
    fixture.metrics.fail_commit.store(true, Ordering::SeqCst);
    let request = staged(&fixture, &[insert_plan(&fixture.declared)], Some(14), TEST_NONCE);
    let failed = post(&fixture, &request).await;
    assert_eq!(
        failure_code(&verify(&fixture, &request, &failed)),
        "commit_definitely_aborted"
    );
    // The replay is the retained failure, and the response/audit policy
    // sees it as that failure, not as a success.
    fixture.metrics.fail_commit.store(false, Ordering::SeqCst);
    assert_eq!(post(&fixture, &request).await, failed);
    assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.metrics.commits.load(Ordering::SeqCst), 0);
    let failure = (false, "commit_definitely_aborted".to_owned());
    assert_eq!(
        *policy.audited.lock().expect("audited"),
        [failure.clone(), failure]
    );
}

#[tokio::test]
async fn skewed_keyed_request_is_retained_within_the_store_ttl() {
    let fixture = v3_fixture(vouching_pipeline(), false);
    // Prepared 50 s ahead (inside the skew allowance) with the longest
    // deadline: its lifetime exceeds the store's five-minute TTL.
    let limits = StagedLimitsV3 {
        deadline_ms: Some(MAX_REMOTE_DEADLINE_MS),
        ..StagedLimitsV3::default()
    };
    let plans = [insert_plan(&fixture.declared)];
    let request = staged_with(&fixture, &plans, Some(4), TEST_NONCE, limits, now_ms() + 50_000);
    let first = post(&fixture, &request).await;
    assert!(verify(&fixture, &request, &first).committed());
    assert_eq!(post(&fixture, &request).await, first);

    // Once a short retention lapses, the spent nonce still refuses a
    // re-execution, as an unknown outcome for a write.
    let store = IdempotencyConfig::new(16, 1 << 20, Duration::from_millis(150));
    let fixture = configured_v3_fixture(
        vouching_pipeline(),
        false,
        None,
        ceilings(100, 64 << 10),
        Some(store),
    );
    let plans = [insert_plan(&fixture.declared)];
    let request = staged(&fixture, &plans, Some(4), TEST_NONCE);
    assert!(verify(&fixture, &request, &post(&fixture, &request).await).committed());
    tokio::time::sleep(Duration::from_millis(200)).await;
    let lapsed = post(&fixture, &request).await;
    assert_eq!(
        failure_code(&verify(&fixture, &request, &lapsed)),
        "commit_outcome_unknown"
    );
    assert_eq!(post(&fixture, &request).await, lapsed);
    assert_eq!(fixture.metrics.commits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn malformed_staged_requests_are_refused_before_anything_opens() {
    let fixture = v3_fixture(default_pipeline(), false);
    // Documents are not yet carried by staged replies.
    let mut plan = spec(
        Operation::Rows,
        vec![Stage::Match {
            patterns: vec![
                QueryPattern::Isa {
                    binding: BindingId::new(0).expect("binding"),
                    include_subtypes: false,
                    type_id: person(),
                }
                .into(),
            ],
        }],
        vec![],
    );
    plan.output = QueryOutput::Documents {
        fields: vec![DocumentField::new(
            QueryVariable::new("person").expect("document key"),
            DocumentSource::Binding {
                binding: BindingId::new(0).expect("binding"),
            },
        )],
    };
    let plan = QueryPlanV3::new(plan, &fixture.declared).expect("documents plan");
    let request = staged(&fixture, &[plan], None, TEST_NONCE);
    let reply = verify(&fixture, &request, &post(&fixture, &request).await);
    assert_eq!(failure_code(&reply), "query_remote_v3_documents_unsupported");

    // An expired request.
    let plans = [count_plan(&fixture.declared)];
    let stale = staged_with(
        &fixture,
        &plans,
        None,
        TEST_NONCE,
        StagedLimitsV3::default(),
        now_ms() - 10 * 60 * 1_000,
    );
    let reply = verify(&fixture, &stale, &post(&fixture, &stale).await);
    assert_eq!(failure_code(&reply), "query_remote_v3_expired");

    // A body over the transport limit, refused before it decodes.
    let oversized = vec![b' '; fixture.query.body_limit_bytes() + 1];
    let reply = call(&fixture.router, "POST", "/v3/staged", oversized).await;
    assert_eq!(unbound_code(&fixture, &reply), "query_remote_envelope_oversized");
    assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 0);
}
