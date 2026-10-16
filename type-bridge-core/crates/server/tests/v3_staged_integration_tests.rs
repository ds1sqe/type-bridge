//! `/v3/staged` against a live TypeDB 3.13.6 (TYPEDB_ADDRESS /
//! TYPEDB_HTTP_PORT); run it explicitly with `-- --ignored`.
//!
//! The scenarios are the one-query race programs shared with the ORM's
//! live plan execution test, executed through the proxy and directly.

#![allow(deprecated)] // `Interceptor::on_request` still takes legacy AST clauses.

#[path = "../../orm/tests/support/plan_v3_race.rs"]
mod race;

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::query_plan::InputRow;
use type_bridge_contract::query_plan::v3::{Operation, QueryPlanV3};
use type_bridge_contract::query_remote::RemoteCapabilities;
use type_bridge_contract::query_remote_v3::{
    IdempotencyKey, StagedAnswerV3, StagedLimitsV3, StagedPlanV3, StagedRequestSpecV3,
    StagedRequestV3, VerifiedStagedReplyV3,
};
use type_bridge_contract::schema::DeclaredSchema;
use type_bridge_contract::sdk_diagnostic::SdkCommitFailureOutcome;
use type_bridge_contract::value::{CanonicalString, CanonicalValue};
use type_bridge_core_lib::version::semantic_profile_id;
use type_bridge_orm::TxType;
use type_bridge_orm::query_v2_remote::Ed25519RemoteReplyVerifier;
use type_bridge_orm::session::backend::{QueryResult, QueryV2AnswerLimits};
use type_bridge_orm::session::database::Database;
use type_bridge_orm::session::real_driver::ConnectOptions;
use type_bridge_orm::session::staged::TransactionOptions;
use type_bridge_schema::{ManagedDeltaContext, managed_schema_state, resolve};
use type_bridge_server::interceptor::{
    InterceptError, Interceptor, RequestContext, StagedWriteAuthority, V2PolicyOutcome,
    V3CommitOutcome, V3PolicyPlan,
};
use type_bridge_server::pipeline::PipelineBuilder;
use type_bridge_server::transport::v2::{V2QueryState, create_router_with_v2};

mod support;
use support::MockExecutor;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

type CommitRecord = (
    bool,
    String,
    Option<SdkCommitFailureOutcome>,
    Option<String>,
);

/// A transport policy: optionally vouches for writes, refuses plans
/// requiring one capability, withholds the next reply once armed, and
/// records every commit outcome.
#[derive(Default)]
struct LivePolicy {
    vouch: bool,
    refuse: Option<&'static str>,
    withhold_next_reply: Arc<AtomicBool>,
    commits: Arc<Mutex<Vec<CommitRecord>>>,
}

impl Interceptor for LivePolicy {
    fn name(&self) -> &str {
        "live-staged-policy"
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

    /// A vouching policy authenticates its caller: idempotency retention is
    /// charged to that identity.
    fn on_v2_transport<'a>(
        &'a self,
        ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        if self.vouch {
            ctx.client_id = "live-staged-client".to_owned();
        }
        Box::pin(async { Ok(()) })
    }

    fn on_v3_write_authority<'a>(
        &'a self,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<StagedWriteAuthority, InterceptError>> + Send + 'a>>
    {
        let vouch = self.vouch;
        Box::pin(async move {
            if vouch {
                Ok(StagedWriteAuthority::Authenticated)
            } else {
                Ok(StagedWriteAuthority::Abstain)
            }
        })
    }

    fn on_v3_plan<'a>(
        &'a self,
        _index: usize,
        plan: &'a V3PolicyPlan<'a>,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
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

    fn on_v2_response<'a>(
        &'a self,
        _outcome: &'a V2PolicyOutcome<'a>,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        let withhold = self.withhold_next_reply.swap(false, Ordering::SeqCst);
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

    fn on_v3_commit<'a>(
        &'a self,
        outcome: &'a V3CommitOutcome<'a>,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        self.commits.lock().expect("commits").push((
            outcome.committed(),
            outcome.code().to_owned(),
            outcome.certainty(),
            outcome.failure().and_then(|f| f.code()).map(str::to_owned),
        ));
        Box::pin(async { Ok(()) })
    }
}

fn connect_options() -> ConnectOptions {
    ConnectOptions {
        http_port: std::env::var("TYPEDB_HTTP_PORT")
            .unwrap_or_else(|_| "8000".into())
            .parse()
            .expect("TYPEDB_HTTP_PORT is a u16"),
        ..ConnectOptions::default()
    }
}

async fn connect(name: &str) -> Database {
    Database::connect_with_options(
        &std::env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1730".into()),
        name,
        &std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".into()),
        &std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".into()),
        connect_options(),
    )
    .await
    .expect("connected database")
}

/// A fresh database holding the race schema and one stocked item.
///
/// The declared authority carries no key or cardinality facts, so the live
/// schema omits those annotations to keep the live-authority check exact.
async fn seeded(test: &str) -> (String, Database) {
    let name = format!(
        "tb_server_v3_{test}_{}_{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let db = connect(&name).await;
    db.create_database().await.expect("database created");
    let schema = race::SCHEMA_TYPEQL
        .replace(" @key", "")
        .replace(" @card(0..1)", "");
    db.execute_raw(&schema, TxType::Schema)
        .await
        .expect("schema");
    seed(&db, "I1", "A1").await;
    (name, db)
}

async fn seed(db: &Database, item: &str, account: &str) {
    db.execute_raw(
        &format!(
            r#"insert $i isa item, has item-id "{item}", has stock 1, has price 40;
               $a isa account, has account-id "{account}", has balance 100;"#
        ),
        TxType::Write,
    )
    .await
    .expect("seed data");
}

struct Proxy {
    router: Router,
    advertisement: RemoteCapabilities,
    declared: DeclaredSchema,
    commits: Arc<Mutex<Vec<CommitRecord>>>,
}

async fn proxy(name: &str, policy: LivePolicy, allow_unauthenticated: bool) -> Proxy {
    let database = connect(name).await;
    let declared = race::schema();
    let profile = SemanticProfileId::new(
        semantic_profile_id(&database.server_version().expect("server version"))
            .expect("supported profile"),
    )
    .expect("profile");
    let resolved = resolve(&declared, &profile).expect("resolved");
    let delta_context = ManagedDeltaContext::new(
        ManagedScopeId::new(format!("server-v3-live-{name}")).expect("scope"),
        profile,
        CapabilitySet::new(),
    );
    let managed = managed_schema_state(&declared, &delta_context).expect("managed");
    let state = Arc::new(
        V2QueryState::new_query_only(
            CapabilitySet::new(),
            QueryV2AnswerLimits::default(),
            database,
            declared.clone(),
            delta_context,
            managed,
            resolved,
        )
        .expect("executor advertisement")
        .with_unauthenticated_staged_writes(allow_unauthenticated),
    );
    let commits = Arc::clone(&policy.commits);
    let pipeline = PipelineBuilder::new(MockExecutor::new())
        .with_default_database(name)
        .with_interceptor(policy)
        .build()
        .expect("pipeline");
    let router = create_router_with_v2(Arc::new(pipeline), state);
    let bytes = call(&router, "GET", "/v3/capabilities", Vec::new()).await;
    let advertisement = RemoteCapabilities::decode(&bytes).expect("v3 advertisement");
    Proxy {
        router,
        advertisement,
        declared,
        commits,
    }
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

fn text(value: &str) -> Option<CanonicalValue> {
    Some(CanonicalValue::String(
        CanonicalString::new(value).expect("text"),
    ))
}

/// A staged entry: the purchase template bound to one order, or a plan as is.
fn purchase_entry(proxy: &Proxy, order: &str, item: &str, account: &str) -> StagedPlanV3 {
    let row = InputRow::new(vec![text(order), text(item), text(account)]);
    StagedPlanV3::new(&race::purchase_template(), vec![row], &proxy.declared).expect("entry")
}

fn plain_entry(proxy: &Proxy, plan: &QueryPlanV3) -> StagedPlanV3 {
    StagedPlanV3::new(plan, vec![], &proxy.declared).expect("entry")
}

fn request(proxy: &Proxy, plans: Vec<StagedPlanV3>, key: Option<u8>) -> StagedRequestV3 {
    let nonce = format!(
        "tb-live-staged-nonce-{}-{:06}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let spec = StagedRequestSpecV3 {
        nonce,
        idempotency_key: key.map(|byte| IdempotencyKey::new(vec![byte; 16]).expect("key")),
        kind: None,
        limits: StagedLimitsV3::default(),
        plans,
        prepared_at_unix_ms: u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_millis(),
        )
        .expect("millis"),
    };
    StagedRequestV3::new(spec, &proxy.declared, &proxy.advertisement).expect("staged request")
}

async fn post(proxy: &Proxy, request: &StagedRequestV3) -> Vec<u8> {
    let body = request.encode().expect("request bytes");
    call(&proxy.router, "POST", "/v3/staged", body).await
}

fn verify(proxy: &Proxy, request: &StagedRequestV3, reply: &[u8]) -> VerifiedStagedReplyV3 {
    VerifiedStagedReplyV3::decode(
        reply,
        request,
        &proxy.advertisement,
        proxy.advertisement.reply_key(),
        &Ed25519RemoteReplyVerifier,
    )
    .expect("signed staged reply verifies")
}

fn failure_code(reply: &VerifiedStagedReplyV3) -> String {
    reply.diagnostic().expect("failed reply").code().to_owned()
}

async fn row_count(db: &Database, query: &str) -> usize {
    match db.execute_raw(query, TxType::Read).await.expect("read") {
        QueryResult::Rows(rows) => rows.len(),
        other => panic!("expected rows, got {other:?}"),
    }
}

/// Stock, balance and purchases of `I1` / `A1`, as one comparable state.
async fn state(db: &Database) -> (usize, usize, usize, usize) {
    (
        row_count(
            db,
            r#"match $i isa item, has item-id "I1", has stock 0; select $i;"#,
        )
        .await,
        row_count(
            db,
            r#"match $a isa account, has account-id "A1", has balance 60; select $a;"#,
        )
        .await,
        row_count(db, "match $p isa purchase; select $p;").await,
        row_count(db, "match $i isa item, has price $p; select $i;").await,
    )
}

async fn drop_database(db: Database) {
    db.delete_database().await.expect("database deleted");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn staged_purchase_matches_direct_transaction_answers_and_state() {
    let (_, direct) = seeded("direct").await;
    let (proxy_name, proxied) = seeded("proxy").await;
    let proxy = proxy(&proxy_name, LivePolicy::default(), true).await;

    // Two transactions, the same plans each way: the purchase that takes the
    // last item, then a refused second purchase with reads and a delete.
    let reads = [
        race::stock_read(Operation::Count),
        race::stock_read(Operation::Exists),
        race::drop_price(),
    ];
    let mut direct_answers = Vec::new();
    let mut tx = direct.transaction(TransactionOptions::default());
    let bought = tx
        .stage_plan(&race::purchase("O1", "I1", "A1"))
        .expect("stage");
    tx.commit().await.expect("commit");
    let first = race::purchase_template();
    direct_answers.push(
        bought
            .get()
            .expect("answer")
            .to_staged_v3(&first)
            .expect("v3"),
    );
    let refused = tx
        .stage_plan(&race::purchase("O2", "I1", "A1"))
        .expect("stage");
    let pending = reads
        .iter()
        .map(|plan| tx.stage_plan(plan).expect("stage"))
        .collect::<Vec<_>>();
    tx.commit().await.expect("commit");
    direct_answers.push(
        refused
            .get()
            .expect("answer")
            .to_staged_v3(&first)
            .expect("v3"),
    );
    for (answer, plan) in pending.iter().zip(&reads) {
        direct_answers.push(
            answer
                .get()
                .expect("answer")
                .to_staged_v3(plan)
                .expect("v3"),
        );
    }

    let mut proxy_answers: Vec<StagedAnswerV3> = Vec::new();
    let first_request = request(&proxy, vec![purchase_entry(&proxy, "O1", "I1", "A1")], None);
    let reply = verify(&proxy, &first_request, &post(&proxy, &first_request).await);
    assert!(reply.committed(), "{:?}", reply.diagnostic());
    proxy_answers.extend_from_slice(reply.answers());
    let mut entries = vec![purchase_entry(&proxy, "O2", "I1", "A1")];
    entries.extend(reads.iter().map(|plan| plain_entry(&proxy, plan)));
    let second_request = request(&proxy, entries, None);
    let reply = verify(
        &proxy,
        &second_request,
        &post(&proxy, &second_request).await,
    );
    assert!(reply.committed(), "{:?}", reply.diagnostic());
    proxy_answers.extend_from_slice(reply.answers());

    assert_eq!(
        direct_answers,
        [
            StagedAnswerV3::Matched { value: 1 },
            StagedAnswerV3::Matched { value: 0 },
            StagedAnswerV3::Count { value: 1 },
            StagedAnswerV3::Exists { value: true },
            StagedAnswerV3::Matched { value: 1 },
        ]
    );
    assert_eq!(proxy_answers, direct_answers);
    assert_eq!(state(&direct).await, (1, 1, 1, 0));
    assert_eq!(state(&proxied).await, state(&direct).await);

    drop(proxy);
    drop_database(direct).await;
    drop_database(proxied).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn unauthenticated_staged_write_is_refused_and_writes_nothing() {
    let (name, db) = seeded("unauthenticated").await;
    let proxy = proxy(&name, LivePolicy::default(), false).await;
    let staged = request(&proxy, vec![purchase_entry(&proxy, "O1", "I1", "A1")], None);
    let reply = verify(&proxy, &staged, &post(&proxy, &staged).await);
    assert!(!reply.committed());
    assert_eq!(failure_code(&reply), "staged_write_not_authorized");
    assert_eq!(state(&db).await, (0, 0, 0, 1));
    drop(proxy);
    drop_database(db).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn dropped_reply_retry_with_the_same_key_applies_once() {
    let (name, db) = seeded("idempotent").await;
    let policy = LivePolicy {
        vouch: true,
        ..LivePolicy::default()
    };
    let proxy = proxy(&name, policy, false).await;
    // A fresh stock so a second application would also match and write.
    db.execute_raw(
        r#"match $i isa item, has item-id "I1", has stock $s; update $i has stock 5;"#,
        TxType::Write,
    )
    .await
    .expect("restock");
    let staged = request(
        &proxy,
        vec![purchase_entry(&proxy, "O1", "I1", "A1")],
        Some(7),
    );
    // The first reply is lost on the way back; the client resends the bytes.
    let dropped = post(&proxy, &staged).await;
    let retried = post(&proxy, &staged).await;
    assert_eq!(
        retried, dropped,
        "the retry replays the retained signed bytes"
    );
    let reply = verify(&proxy, &staged, &retried);
    assert!(reply.committed(), "{:?}", reply.diagnostic());
    assert_eq!(reply.answers(), [StagedAnswerV3::Matched { value: 1 }]);
    assert_eq!(row_count(&db, "match $p isa purchase; select $p;").await, 1);
    assert_eq!(
        row_count(
            &db,
            r#"match $i isa item, has item-id "I1", has stock 4; select $i;"#
        )
        .await,
        1
    );
    // The commit hook ran once: the replay never re-executed.
    assert_eq!(proxy.commits.lock().expect("commits").len(), 1);
    drop(proxy);
    drop_database(db).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn reply_withheld_after_commit_reports_an_unknown_outcome_and_replays_it() {
    let (name, db) = seeded("withheld").await;
    let withhold = Arc::new(AtomicBool::new(false));
    let policy = LivePolicy {
        vouch: true,
        withhold_next_reply: Arc::clone(&withhold),
        ..LivePolicy::default()
    };
    let proxy = proxy(&name, policy, false).await;
    withhold.store(true, Ordering::SeqCst);
    let staged = request(
        &proxy,
        vec![purchase_entry(&proxy, "O1", "I1", "A1")],
        Some(9),
    );
    let first = post(&proxy, &staged).await;
    let reply = verify(&proxy, &staged, &first);
    // The purchase committed, so the withheld reply must not read as a
    // safe-to-retry policy refusal.
    assert!(!reply.committed());
    assert_eq!(failure_code(&reply), "commit_outcome_unknown");
    assert_eq!(row_count(&db, "match $p isa purchase; select $p;").await, 1);
    // The keyed retry replays the same bytes and applies nothing more.
    assert_eq!(post(&proxy, &staged).await, first);
    assert_eq!(row_count(&db, "match $p isa purchase; select $p;").await, 1);
    assert_eq!(proxy.commits.lock().expect("commits").len(), 1);

    // A committed reply delivered, then withheld on replay: still an
    // unknown outcome, identical on every withheld replay, and the retained
    // committed reply survives.
    seed(&db, "I2", "A2").await;
    let staged = request(
        &proxy,
        vec![purchase_entry(&proxy, "O2", "I2", "A2")],
        Some(10),
    );
    let committed = post(&proxy, &staged).await;
    assert!(verify(&proxy, &staged, &committed).committed());
    withhold.store(true, Ordering::SeqCst);
    let withheld = post(&proxy, &staged).await;
    assert_eq!(
        failure_code(&verify(&proxy, &staged, &withheld)),
        "commit_outcome_unknown"
    );
    withhold.store(true, Ordering::SeqCst);
    assert_eq!(post(&proxy, &staged).await, withheld);
    assert_eq!(post(&proxy, &staged).await, committed);
    assert_eq!(row_count(&db, "match $p isa purchase; select $p;").await, 2);
    assert_eq!(proxy.commits.lock().expect("commits").len(), 2);
    drop(proxy);
    drop_database(db).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn plan_policy_refuses_a_staged_delete_and_nothing_commits() {
    let (name, db) = seeded("policy").await;
    let policy = LivePolicy {
        vouch: true,
        refuse: Some("query.write.delete"),
        ..LivePolicy::default()
    };
    let proxy = proxy(&name, policy, false).await;
    // The purchase comes first; the delete refusal must still drop it.
    let entries = vec![
        purchase_entry(&proxy, "O1", "I1", "A1"),
        plain_entry(&proxy, &race::drop_price()),
    ];
    let staged = request(&proxy, entries, None);
    let reply = verify(&proxy, &staged, &post(&proxy, &staged).await);
    assert!(!reply.committed());
    assert_eq!(failure_code(&reply), "query_remote_policy_rejected");
    assert_eq!(state(&db).await, (0, 0, 0, 1));
    // Refused before anything executed, so the commit observer never runs.
    assert!(proxy.commits.lock().expect("commits").is_empty());
    drop(proxy);
    drop_database(db).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn commit_hook_observes_an_stc2_conflict_between_racing_staged_purchases() {
    let (name, db) = seeded("race").await;
    let policy = LivePolicy {
        vouch: true,
        ..LivePolicy::default()
    };
    let proxy = Arc::new(proxy(&name, policy, false).await);
    // Two purchases of the same last item race; when their snapshots
    // overlap, one commits and the other is rejected with STC2. Overlap is
    // timing dependent, so race fresh items until one round conflicts.
    for round in 0..40 {
        let (item, account) = (format!("R{round}"), format!("B{round}"));
        seed(&db, &item, &account).await;
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let racers = ["a", "b"].map(|side| {
            let proxy = Arc::clone(&proxy);
            let barrier = Arc::clone(&barrier);
            let order = format!("O{round}{side}");
            let (item, account) = (item.clone(), account.clone());
            tokio::spawn(async move {
                let staged = request(
                    &proxy,
                    vec![purchase_entry(&proxy, &order, &item, &account)],
                    None,
                );
                let body = staged.encode().expect("request bytes");
                barrier.wait().await;
                let reply = call(&proxy.router, "POST", "/v3/staged", body).await;
                verify(&proxy, &staged, &reply)
            })
        });
        let mut replies = Vec::new();
        for racer in racers {
            replies.push(racer.await.expect("racer"));
        }
        let bought = row_count(
            &db,
            &format!(r#"match $i isa item, has item-id "{item}"; $p isa purchase, links (item: $i); select $p;"#),
        )
        .await;
        assert_eq!(bought, 1, "exactly one racer buys the last item");
        let conflicted = replies.iter().filter(|reply| !reply.committed()).count();
        if conflicted == 0 {
            continue;
        }
        assert_eq!(conflicted, 1);
        let failed = replies
            .iter()
            .find(|reply| !reply.committed())
            .expect("failed");
        assert_eq!(failure_code(failed), "commit_definitely_aborted");
        let commits = proxy.commits.lock().expect("commits").clone();
        assert!(
            commits.contains(&(
                false,
                "commit_definitely_aborted".to_owned(),
                Some(SdkCommitFailureOutcome::DefinitelyAborted),
                Some("STC2".to_owned()),
            )),
            "{commits:?}"
        );
        drop(proxy);
        drop_database(db).await;
        return;
    }
    panic!("no racing round produced an STC2 conflict");
}
