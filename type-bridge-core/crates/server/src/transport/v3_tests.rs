// `/v3/staged` route tests over the V2 route fixtures (mock backend, no
// provider). `include!`d into a child of the V2 test module so they share its
// private backend, authority and pipeline fixtures.

use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::query_plan::QueryOperand;
use type_bridge_contract::query_plan::v3::{
    InsertThing, Operation, PlanSpec, QueryPlanV3, Stage, WriteReturns,
};
use type_bridge_contract::query_remote_v3::{
    CAP_QUERY_REMOTE_IDEMPOTENCY_V3, CAP_QUERY_REMOTE_STAGED_V3, CAP_QUERY_REMOTE_WRITE_V3,
    IdempotencyKey, StagedAnswerV3, StagedLimitsV3, StagedPlanV3,
    StagedRequestSpecV3, StagedRequestV3, VerifiedStagedReplyV3,
};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_contract::value::{CanonicalString, CanonicalValue};

use super::*;
use crate::interceptor::StagedWriteAuthority;
use crate::transport::v3::{STAGED_WRITE_NOT_AUTHORIZED, staged_advertisement};

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
    let authority = authority_fixture(false);
    let declared = authority.declared.clone();
    let metrics = Arc::new(BackendMetrics::default());
    let database = Database::with_backend(
        Box::new(CountingBackend::new([], Arc::clone(&metrics))),
        "server-v3-route",
    );
    let query = V2QueryState::new_query_only(
        CapabilitySet::new(),
        QueryV2AnswerLimits::default(),
        database,
        authority.declared,
        authority.delta_context,
        authority.managed,
        authority.resolved,
    )
    .expect("executor advertisement is canonical")
    .with_unauthenticated_staged_writes(allow_unauthenticated_writes);
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

/// A policy that authenticated its caller and vouches for staged writes.
struct VouchingPolicy;

impl Interceptor for VouchingPolicy {
    fn name(&self) -> &str {
        "vouching-policy"
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

/// A response policy that withholds every reply while `withhold` is set.
#[derive(Default)]
struct WithholdsReplies {
    withhold: AtomicBool,
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
        _outcome: &'a V2PolicyOutcome<'a>,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
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
    assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 1);
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
    let fixture = v3_fixture(default_pipeline(), true);
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
    assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.metrics.query_executions.load(Ordering::SeqCst), 1);

    // A read keeps its mapping diagnostic.
    let fixture = v3_fixture(default_pipeline(), true);
    *fixture.metrics.rows.lock().expect("rows lock") = vec![serde_json::json!({})];
    let request = staged(&fixture, &[rows_plan(&fixture.declared)], None, TEST_NONCE);
    let reply = verify(&fixture, &request, &post(&fixture, &request).await);
    assert_eq!(failure_code(&reply), "query_remote_v3_answer_concept");
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
