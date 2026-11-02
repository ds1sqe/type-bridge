//! Live raw transport parity with TypeDB's own HTTP query-answer encoding.

#[path = "support/staged_live.rs"]
mod staged_live;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_orm::session::backend::AnswerLimits;
use type_bridge_orm::session::staged::TransactionKind;
use type_bridge_schema::{ManagedDeltaContext, managed_schema_state};
use type_bridge_server::config::AuditLogConfig;
use type_bridge_server::interceptor::audit_log::AuditLogInterceptor;
use type_bridge_server::interceptor::{
    InterceptError, Interceptor, RequestContext, V3CommitOutcome,
};
use type_bridge_server::pipeline::PipelineBuilder;
use type_bridge_server::transport::staged_state::StagedQueryState;
use type_bridge_server::transport::v3::create_v3_router;
use type_bridge_typedb_runtime::{ConnectOptions, TxType, TypeDBRuntime};

#[derive(Default, Clone)]
struct Host {
    commits: Arc<Mutex<Vec<TransactionKind>>>,
}
impl Interceptor for Host {
    fn name(&self) -> &str {
        "raw-live-host"
    }
    fn supports_v3(&self) -> bool {
        true
    }
    fn on_v3_raw<'a>(
        &'a self,
        query: &'a str,
        _kind: TransactionKind,
        _ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        Box::pin(async move {
            if query.contains("refused") {
                Err(InterceptError::Refused {
                    code: "raw_live_refused",
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
        assert!(outcome.committed());
        self.commits.lock().unwrap().push(outcome.kind());
        Box::pin(async { Ok(()) })
    }
}
async fn runtime() -> TypeDBRuntime {
    TypeDBRuntime::connect(
        &std::env::var("TYPEDB_ADDRESS").unwrap(),
        &std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".into()),
        &std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".into()),
        ConnectOptions {
            http_port: std::env::var("TYPEDB_HTTP_PORT").unwrap().parse().unwrap(),
            ..Default::default()
        },
    )
    .await
    .unwrap()
}
async fn provider_http(
    client: &reqwest::Client,
    base: &str,
    token: Option<&str>,
    path: &str,
    body: Value,
) -> (u16, Value) {
    let mut request = client
        .post(format!("{base}{path}"))
        .header("content-type", "application/json")
        .body(body.to_string());
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    (
        status,
        serde_json::from_slice(&response.bytes().await.unwrap()).unwrap(),
    )
}
async fn raw(router: &axum::Router, query: &str, kind: &str) -> (StatusCode, Vec<u8>) {
    let expires_at_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 30_000;
    let body = json!({"query":query,"kind":kind,"nonce":format!("raw-live-{}",uuid::Uuid::new_v4()),"expires_at_unix_ms":expires_at_unix_ms});
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v3/raw")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    (
        response.status(),
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
}

#[tokio::test]
#[ignore = "requires live TypeDB and its HTTP API"]
async fn raw_answers_match_typedb_http_and_host_refusal_writes_nothing() {
    let name = staged_live::live_database::name("raw");
    let direct = runtime().await;
    direct.create_database(&name).await.unwrap();
    direct
        .execute_raw(
            &name,
            "define attribute raw-name, value string; entity raw-person, owns raw-name;",
            TxType::Schema,
        )
        .await
        .unwrap();
    direct
        .execute_raw(
            &name,
            "insert $p isa raw-person, has raw-name \"hello\";",
            TxType::Write,
        )
        .await
        .unwrap();
    let database = staged_live::connect(&name).await;
    let profile = SemanticProfileId::new(
        type_bridge_core_lib::version::semantic_profile_id(&database.server_version().unwrap())
            .unwrap(),
    )
    .unwrap();
    let declared = staged_live::race::schema();
    let context = ManagedDeltaContext::new(
        ManagedScopeId::new("raw-live").unwrap(),
        profile,
        CapabilitySet::new(),
    );
    let managed = managed_schema_state(&declared, &context).unwrap();
    // Raw deliberately does not compare live schema with generated authority.
    let query = StagedQueryState::new_query_only(
        CapabilitySet::new(),
        AnswerLimits::default(),
        database,
        declared,
        context,
        managed,
    )
    .unwrap()
    .with_raw_executor(runtime().await);
    let host = Host::default();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("raw.jsonl");
    let audit = AuditLogInterceptor::new(&AuditLogConfig {
        output: "file".into(),
        file_path: path.to_str().unwrap().into(),
    })
    .unwrap();
    let pipeline = PipelineBuilder::new()
        .with_default_database(&name)
        .with_interceptor(audit)
        .with_interceptor(host.clone())
        .build()
        .unwrap();
    let router = create_v3_router(Arc::new(pipeline), Arc::new(query));
    let client = reqwest::Client::new();
    let base = format!(
        "http://127.0.0.1:{}",
        std::env::var("TYPEDB_HTTP_PORT").unwrap()
    );
    let (status,signin)=provider_http(&client,&base,None,"/v1/signin",json!({"username":std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_|"admin".into()),"password":std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_|"password".into())})).await;
    assert_eq!(status, 200);
    let token = signin["token"].as_str().unwrap();
    let queries = [
        (
            "read",
            TxType::Read,
            "match $p isa raw-person, has raw-name $n; let $v = $n; select $p, $n, $v;",
        ),
        (
            "read",
            TxType::Read,
            "match $p isa raw-person; fetch { \"person\": { $p.* } };",
        ),
        ("write", TxType::Write, "insert $n isa raw-name \"added\";"),
        ("schema", TxType::Schema, "define entity raw-extra;"),
    ];
    for (kind, transaction_kind, query) in queries {
        let direct_answer = direct
            .execute_raw(&name, query, transaction_kind)
            .await
            .unwrap();
        let (status, proxy_answer) = raw(&router, query, kind).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{kind}: {}",
            String::from_utf8_lossy(&proxy_answer)
        );
        let (http_status,http_answer)=provider_http(&client,&base,Some(token),"/v1/query",json!({"databaseName":name,"transactionType":kind,"query":query,"queryOptions":{"includeQueryStructure":false}})).await;
        assert_eq!(http_status, 200, "{http_answer}");
        assert_eq!(
            serde_json::from_slice::<Value>(&direct_answer).unwrap(),
            http_answer,
            "direct {query}"
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&proxy_answer).unwrap(),
            http_answer,
            "proxy {query}"
        );
    }
    let before = direct
        .execute_raw(&name, queries[0].2, TxType::Read)
        .await
        .unwrap();
    let (status, body) = raw(
        &router,
        "insert $p isa raw-person, has raw-name \"refused\";",
        "write",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, br#"{"error":"raw_live_refused"}"#);
    assert_eq!(
        direct
            .execute_raw(&name, queries[0].2, TxType::Read)
            .await
            .unwrap(),
        before
    );
    let (status, error) = raw(&router, "this is not typeql;", "read").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let error: Value = serde_json::from_slice(&error).unwrap();
    let (_, http_error) = provider_http(
        &client,
        &base,
        Some(token),
        "/v1/query",
        json!({"databaseName":name,"transactionType":"read","query":"this is not typeql;"}),
    )
    .await;
    assert_eq!(error["error"], "typedb_error");
    // gRPC and HTTP add different transport frames to the same provider error.
    let direct_error = direct.execute_raw(&name, "this is not typeql;", TxType::Read).await.unwrap_err();
    assert_eq!(error["typedb"], json!({"code":direct_error.code,"message":direct_error.message}));
    assert_eq!(error["typedb"]["code"], http_error["code"]);
    assert_eq!(
        *host.commits.lock().unwrap(),
        [TransactionKind::Write, TransactionKind::Schema]
    );
    let audit = std::fs::read_to_string(path).unwrap();
    for (_, _, query) in queries {
        assert!(
            audit.lines().any(
                |line| serde_json::from_str::<Value>(line).unwrap()["compiled_typeql"] == query
            )
        );
    }
    direct.delete_database(&name).await.unwrap();
    println!(
        "raw HTTP parity: 4 query kinds/shapes; refusal, audit, error-code passthrough; database {name}"
    );
}
