//! Opt-in raw SDK parity and lifecycle checks against a real TypeDB and HTTP proxy.
#![cfg(feature = "typedb")]

#[path = "../../server/tests/support/staged_live.rs"]
mod staged_live;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use type_bridge::raw::TransactionKind;
use type_bridge::{ConnectionOptions, Database, ProxyConnectionPolicy, ProxyDatabase};
use type_bridge_contract::{
    capability::CapabilitySet, fingerprint::SemanticProfileId, managed_scope::ManagedScopeId,
};
use type_bridge_orm::{
    ProxyEndpoint, TxType,
    session::backend::{AnswerLimits, QueryResult},
};
use type_bridge_schema::{ManagedDeltaContext, managed_schema_state};
use type_bridge_server::interceptor::{InterceptError, Interceptor, RequestContext};
use type_bridge_server::pipeline::PipelineBuilder;
use type_bridge_server::transport::{staged_state::StagedQueryState, v3::create_v3_router};
use type_bridge_server::{
    config::{OutboundTlsMode, SecureTypeDBSection, TypeDBSection},
    typedb::TypeDBClient,
};

struct CountRaw(Arc<AtomicUsize>);
impl Interceptor for CountRaw {
    fn name(&self) -> &str {
        "raw-sdk-counter"
    }
    fn supports_v3(&self) -> bool {
        true
    }
    fn on_v3_transport<'a>(
        &'a self,
        ctx: &'a mut RequestContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), InterceptError>> + Send + 'a>>
    {
        if ctx
            .metadata
            .get("v3_endpoint")
            .and_then(serde_json::Value::as_str)
            == Some("raw")
        {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
        Box::pin(async { Ok(()) })
    }
}

struct Server {
    task: tokio::task::JoinHandle<()>,
    url: String,
    requests: Arc<AtomicUsize>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn server(database: &str, port: u16, enabled: bool) -> Server {
    let config = SecureTypeDBSection::new(
        TypeDBSection {
            address: std::env::var("TYPEDB_ADDRESS").unwrap(),
            database: database.into(),
            username: "admin".into(),
            password: "password".into(),
            http_port: std::env::var("TYPEDB_HTTP_PORT").unwrap().parse().unwrap(),
            server_version: None,
        },
        OutboundTlsMode::Disabled,
    );
    let transport = TypeDBClient::prepare_secure_transport(&config).unwrap();
    let db = transport.connect_database().await.unwrap();
    let profile = SemanticProfileId::new(
        type_bridge_core_lib::version::semantic_profile_id(&db.server_version().unwrap()).unwrap(),
    )
    .unwrap();
    let declared = staged_live::race::schema();
    let context = ManagedDeltaContext::new(
        ManagedScopeId::new("raw-sdk-live").unwrap(),
        profile,
        CapabilitySet::new(),
    );
    let managed = managed_schema_state(&declared, &context).unwrap();
    let mut state = StagedQueryState::new_query_only(
        CapabilitySet::new(),
        AnswerLimits::default(),
        db,
        declared,
        context,
        managed,
    )
    .unwrap()
    .with_unauthenticated_staged_writes(true);
    if enabled {
        state = state.with_raw_executor(transport.connect_raw_runtime().await.unwrap());
    }
    let requests = Arc::new(AtomicUsize::new(0));
    let pipeline = PipelineBuilder::new()
        .with_default_database(database)
        .with_interceptor(CountRaw(requests.clone()))
        .build()
        .unwrap();
    let router = create_v3_router(Arc::new(pipeline), Arc::new(state));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Server {
        task,
        url: format!("http://127.0.0.1:{port}"),
        requests,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires explicit disposable TypeDB database and two owned proxy ports"]
async fn raw_pair_matches_driver_answers_and_retains_atomicity() {
    let name = std::env::var("TYPE_BRIDGE_RAW_TEST_DATABASE").expect("explicit database");
    let port: u16 = std::env::var("TYPE_BRIDGE_RAW_TEST_PORT")
        .expect("explicit port")
        .parse()
        .unwrap();
    let orm = staged_live::connect(&name).await;
    assert!(!orm.database_exists().await.unwrap());
    orm.create_database().await.unwrap();
    let database = Database::connect(
        ConnectionOptions::new(std::env::var("TYPEDB_ADDRESS").unwrap(), &name)
            .http_port(std::env::var("TYPEDB_HTTP_PORT").unwrap().parse().unwrap()),
    )
    .await
    .unwrap();
    let schema = database
        .execute_query(
            "define entity raw-atomic; entity raw-oracle;",
            TransactionKind::Schema,
        )
        .await
        .unwrap();
    assert_eq!(schema["answerType"], "ok");
    assert_eq!(schema["queryType"], "schema");

    // The retained backend's ordinary document path independently reads the driver's
    // documents, without the raw HTTP-answer encoder used by the SDK pair.
    let insert = "insert $x isa raw-oracle; fetch { 'value': 7 };";
    let QueryResult::Documents(driver_insert) =
        orm.execute_raw(insert, TxType::Write).await.unwrap()
    else {
        panic!("driver document answer")
    };
    let inserted = database
        .execute_query(insert, TransactionKind::Write)
        .await
        .unwrap();
    assert_eq!(inserted["answers"], serde_json::json!(driver_insert));
    let fetch = "match $x isa raw-oracle; fetch { 'value': 7 };";
    let QueryResult::Documents(driver_fetch) = orm.execute_raw(fetch, TxType::Read).await.unwrap()
    else {
        panic!("driver document answer")
    };
    assert_eq!(
        database
            .execute_query(fetch, TransactionKind::Read)
            .await
            .unwrap()["answers"],
        serde_json::json!(driver_fetch)
    );

    let mut transaction = database
        .raw_transaction(TransactionKind::Write)
        .await
        .unwrap();
    for _ in 0..2 {
        transaction
            .execute_query("insert $x isa raw-atomic;")
            .await
            .unwrap();
    }
    let fetch_atomic = "match $x isa raw-atomic; fetch { 'value': 1 };";
    assert_eq!(
        transaction.execute_query(fetch_atomic).await.unwrap()["answers"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    transaction.rollback().await.unwrap();
    assert_eq!(
        database
            .execute_query(fetch_atomic, TransactionKind::Read)
            .await
            .unwrap()["answers"],
        serde_json::json!([])
    );

    let enabled = server(&name, port, true).await;
    let proxy =
        ProxyDatabase::connect(ProxyConnectionPolicy::new(ProxyEndpoint::new(&enabled.url)))
            .await
            .unwrap();
    assert_eq!(
        proxy
            .execute_query(fetch, TransactionKind::Read)
            .await
            .unwrap()["answers"],
        serde_json::json!(driver_fetch)
    );
    assert_eq!(
        proxy
            .execute_query(insert, TransactionKind::Write)
            .await
            .unwrap()["answers"],
        serde_json::json!(driver_insert)
    );
    let schema = proxy
        .execute_query("define entity raw-schema-proxy;", TransactionKind::Schema)
        .await
        .unwrap();
    assert_eq!(schema["queryType"], "schema");
    let before = enabled.requests.load(Ordering::SeqCst);
    let mut transaction = proxy.raw_transaction(TransactionKind::Write).await.unwrap();
    transaction
        .execute_query("insert $x isa raw-atomic;")
        .await
        .unwrap();
    assert_eq!(enabled.requests.load(Ordering::SeqCst), before + 1);
    assert_eq!(
        transaction
            .execute_query("insert $x isa raw-atomic;")
            .await
            .unwrap_err()
            .code,
        "raw_transaction_single_query"
    );
    transaction.commit().await.unwrap();
    assert_eq!(
        transaction.rollback().await.unwrap_err().code,
        "raw_transaction_already_committed"
    );
    assert_eq!(enabled.requests.load(Ordering::SeqCst), before + 1);
    assert_eq!(
        database
            .execute_query(fetch_atomic, TransactionKind::Read)
            .await
            .unwrap()["answers"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let mut read = proxy.raw_transaction(TransactionKind::Read).await.unwrap();
    read.execute_query(fetch_atomic).await.unwrap();
    read.commit().await.unwrap();
    read.rollback().await.unwrap();

    let disabled = server(&name, port + 1, false).await;
    let refused = ProxyDatabase::connect(ProxyConnectionPolicy::new(ProxyEndpoint::new(
        &disabled.url,
    )))
    .await
    .unwrap();
    for kind in [
        TransactionKind::Read,
        TransactionKind::Write,
        TransactionKind::Schema,
    ] {
        assert_eq!(
            refused
                .execute_query("unchecked text", kind)
                .await
                .unwrap_err()
                .code,
            "raw_disabled"
        );
        let mut transaction = refused.raw_transaction(kind).await.unwrap();
        assert_eq!(
            transaction
                .execute_query("unchecked text")
                .await
                .unwrap_err()
                .code,
            "raw_disabled"
        );
        assert_eq!(
            transaction.execute_query("second").await.unwrap_err().code,
            "raw_transaction_single_query"
        );
    }
    drop(disabled);
    drop(enabled);
    database.close().unwrap();
    orm.delete_database().await.unwrap();
    assert!(!orm.database_exists().await.unwrap());
}
