//! The live `/v3/staged` proxy fixture over TypeDB 3.13.6 (TYPEDB_ADDRESS /
//! TYPEDB_HTTP_PORT): seeded race databases, a recording transport policy
//! and an in-process proxy router. Shared by the server's staged live tests
//! and the Rust SDK's live plan-builder tests, so the server's own tests
//! never depend on the SDK crate.

#![allow(deprecated)] // `Interceptor::on_request` still takes legacy AST clauses.
#![allow(dead_code)]

#[path = "../../../orm/tests/support/plan_v3_race.rs"]
pub mod race;

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
use type_bridge_contract::query_remote::RemoteCapabilities;
use type_bridge_contract::schema::DeclaredSchema;
use type_bridge_contract::sdk_diagnostic::SdkCommitFailureOutcome;
use type_bridge_core_lib::version::semantic_profile_id;
use type_bridge_orm::TxType;
use type_bridge_orm::session::backend::{QueryResult, QueryV2AnswerLimits};
use type_bridge_orm::session::database::Database;
use type_bridge_orm::session::real_driver::ConnectOptions;
use type_bridge_schema::{ManagedDeltaContext, managed_schema_state, resolve};
use type_bridge_server::error::PipelineError;
use type_bridge_server::executor::QueryExecutor;
use type_bridge_server::interceptor::{
    InterceptError, Interceptor, RequestContext, StagedWriteAuthority, V2PolicyOutcome,
    V3CommitOutcome, V3PolicyPlan,
};
use type_bridge_server::pipeline::PipelineBuilder;
use type_bridge_server::transport::v2::{V2QueryState, create_router_with_v2};
use type_bridge_server::transport::v3::{StagedExecutor, StagedOutcome};

pub static SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub type CommitRecord = (
    bool,
    String,
    Option<SdkCommitFailureOutcome>,
    Option<String>,
);

/// A transport policy: optionally vouches for writes, refuses plans
/// requiring one capability, withholds the next reply once armed, and
/// records every commit outcome.
#[derive(Default)]
pub struct LivePolicy {
    pub vouch: bool,
    pub refuse: Option<&'static str>,
    pub withhold_next_reply: Arc<AtomicBool>,
    pub commits: Arc<Mutex<Vec<CommitRecord>>>,
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

    /// A vouching policy authenticates its caller.
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

/// The v1 executor behind the pipeline; the staged routes run on the
/// `V2QueryState` database instead, so it only reports a connection.
struct UnusedV1Executor;

impl QueryExecutor for UnusedV1Executor {
    fn execute<'a>(
        &'a self,
        _database: &'a str,
        _typeql: &'a str,
        _transaction_type: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, PipelineError>> + Send + 'a>> {
        Box::pin(async {
            Err(PipelineError::QueryExecution(
                "the staged live fixture serves no v1 queries".to_owned(),
            ))
        })
    }

    fn is_connected(&self) -> bool {
        true
    }
}

pub fn connect_options() -> ConnectOptions {
    ConnectOptions {
        http_port: std::env::var("TYPEDB_HTTP_PORT")
            .unwrap_or_else(|_| "8000".into())
            .parse()
            .expect("TYPEDB_HTTP_PORT is a u16"),
        ..ConnectOptions::default()
    }
}

pub async fn connect(name: &str) -> Database {
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

/// Whether the live server takes `given` rows, which every staged purchase
/// plan binds. TypeDB 3.11 does not, so those tests skip there with a reason
/// instead of failing with `query_plan_given_transport_unsupported`; test.sh
/// runs them only on the lanes that support given rows.
pub async fn given_rows_supported(test: &str) -> bool {
    let db = connect("tb_server_v3_given_rows_probe").await;
    let supported = db.supports_given_stage();
    if !supported {
        eprintln!(
            "skipping {test}: TypeDB {} does not support given rows (3.12+)",
            db.server_version()
                .map_or_else(|| "of unknown version".to_owned(), |v| v.to_string())
        );
    }
    db.close().expect("probe connection closes");
    supported
}

/// A fresh database holding the race schema and one stocked item.
///
/// The declared authority carries no key or cardinality facts, so the live
/// schema omits those annotations to keep the live-authority check exact.
pub async fn seeded(test: &str) -> (String, Database) {
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

pub async fn seed(db: &Database, item: &str, account: &str) {
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

pub struct Proxy {
    pub router: Router,
    pub advertisement: RemoteCapabilities,
    pub declared: DeclaredSchema,
    pub commits: Arc<Mutex<Vec<CommitRecord>>>,
    /// The library entry over the same pipeline and state as the router.
    pub executor: Arc<StagedExecutor>,
}

impl Proxy {
    /// Run signed request bytes through the library entry as an embedding
    /// host would: admit the caller through the pipeline, then execute.
    pub async fn execute_direct(&self, body: Vec<u8>) -> StagedOutcome {
        let pipeline = self.executor.pipeline();
        let metadata = std::collections::HashMap::from([
            ("transport".to_owned(), serde_json::json!("v3")),
            ("v3_endpoint".to_owned(), serde_json::json!("staged")),
        ]);
        let mut ctx = pipeline.v3_request_context(metadata).expect("v3 context");
        pipeline
            .begin_v2_request(&mut ctx)
            .await
            .expect("transport admission");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        self.executor
            .execute_staged_request(ctx, body.into(), deadline)
            .await
    }
}

pub async fn proxy(name: &str, policy: LivePolicy, allow_unauthenticated: bool) -> Proxy {
    let scope = format!("server-v3-live-{name}");
    proxy_with(name, policy, allow_unauthenticated, race::schema(), &scope).await
}

/// A proxy over `name` serving `declared` under managed `scope`.
pub async fn proxy_with(
    name: &str,
    policy: LivePolicy,
    allow_unauthenticated: bool,
    declared: DeclaredSchema,
    scope: &str,
) -> Proxy {
    let database = connect(name).await;
    let profile = SemanticProfileId::new(
        semantic_profile_id(&database.server_version().expect("server version"))
            .expect("supported profile"),
    )
    .expect("profile");
    let resolved = resolve(&declared, &profile).expect("resolved");
    let delta_context = ManagedDeltaContext::new(
        ManagedScopeId::new(scope).expect("scope"),
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
    let pipeline = PipelineBuilder::new(UnusedV1Executor)
        .with_default_database(name)
        .with_interceptor(policy)
        .build()
        .expect("pipeline");
    let pipeline = Arc::new(pipeline);
    let executor = Arc::new(StagedExecutor::new(
        Arc::clone(&pipeline),
        Arc::clone(&state),
    ));
    let router = create_router_with_v2(pipeline, state);
    let bytes = call(&router, "GET", "/v3/capabilities", Vec::new()).await;
    let advertisement = RemoteCapabilities::decode(&bytes).expect("v3 advertisement");
    Proxy {
        router,
        advertisement,
        declared,
        commits,
        executor,
    }
}

pub async fn call(router: &Router, method: &str, uri: &str, body: Vec<u8>) -> Vec<u8> {
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

pub async fn row_count(db: &Database, query: &str) -> usize {
    match db.execute_raw(query, TxType::Read).await.expect("read") {
        QueryResult::Rows(rows) => rows.len(),
        other => panic!("expected rows, got {other:?}"),
    }
}

/// Stock, balance and purchases of `I1` / `A1`, as one comparable state.
pub async fn state(db: &Database) -> (usize, usize, usize, usize) {
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

pub async fn drop_database(db: Database) {
    db.delete_database().await.expect("database deleted");
}
