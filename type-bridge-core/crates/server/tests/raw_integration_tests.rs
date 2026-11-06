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
    unvouched: bool,
}
impl Interceptor for Host {
    fn name(&self) -> &str {
        "raw-live-host"
    }
    fn supports_v3(&self) -> bool {
        true
    }
    fn on_v3_write_authority<'a>(
        &'a self,
        _ctx: &'a RequestContext,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        type_bridge_server::interceptor::StagedWriteAuthority,
                        InterceptError,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            Ok(if self.unvouched {
                type_bridge_server::interceptor::StagedWriteAuthority::Abstain
            } else {
                type_bridge_server::interceptor::StagedWriteAuthority::Authenticated
            })
        })
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
    let query = Arc::new(query);
    let unvouched = PipelineBuilder::new()
        .with_default_database(&name)
        .with_interceptor(Host {
            unvouched: true,
            ..Default::default()
        })
        .build()
        .unwrap();
    let unvouched = create_v3_router(Arc::new(unvouched), Arc::clone(&query));
    for (text, kind) in [
        (
            "insert $p isa raw-person, has raw-name \"unvouched\";",
            "write",
        ),
        ("undefine entity raw-person;", "schema"),
    ] {
        let (status, body) = raw(&unvouched, text, kind).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"error":"staged_write_not_authorized"})
        );
    }
    let bounded = direct
        .execute_raw_with_options(
            &name,
            "insert $p isa raw-person, has raw-name \"byte-limit-rolled-back\";",
            TxType::Write,
            type_bridge_typedb_runtime::raw::RawExecutionOptions {
                deadline: Some(std::time::Instant::now() + Duration::from_secs(2)),
                max_answer_bytes: Some(1),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(bounded.code, "raw_answer_byte_limit");
    assert_eq!(bounded.commit_outcome, None);
    let mut held = direct
        .open_transaction(&name, TxType::Schema)
        .await
        .unwrap();
    held.query_raw("define entity raw-lock-holder;")
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let timed = direct
        .execute_raw_with_options(
            &name,
            "insert $p isa raw-person, has raw-name \"deadline-rolled-back\";",
            TxType::Write,
            type_bridge_typedb_runtime::raw::RawExecutionOptions {
                deadline: Some(started + Duration::from_millis(25)),
                max_answer_bytes: Some(type_bridge_contract::limits::MAX_REMOTE_ENVELOPE_BYTES),
            },
        )
        .await
        .unwrap_err();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "raw open ignored its deadline"
    );
    assert_eq!(timed.commit_outcome, None);
    // The provider's millisecond timeout can win the same absolute deadline.
    // Preserve its TXN1 diagnostic instead of rewriting it as a local timeout.
    assert!(
        matches!(
            timed.code.as_str(),
            "transaction_deadline_exceeded" | "TXN1"
        ),
        "unexpected timeout diagnostic: {timed}"
    );
    println!("bounded raw open refused with {}", timed.code);
    held.close().await.unwrap();
    let unchanged = direct
        .execute_raw(
            &name,
            "match $p isa raw-person; reduce $n = count;",
            TxType::Read,
        )
        .await
        .unwrap();
    let unchanged: Value = serde_json::from_slice(&unchanged).unwrap();
    assert_eq!(unchanged["answers"][0]["data"]["n"]["value"], 1);
    let router = create_v3_router(Arc::new(pipeline), query);
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
    let direct_error = direct
        .execute_raw(&name, "this is not typeql;", TxType::Read)
        .await
        .unwrap_err();
    assert_eq!(
        error["typedb"],
        json!({"code":direct_error.code,"message":direct_error.message})
    );
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
    let missing = direct
        .execute_raw(&name, "match $x isa entity;", TxType::Read)
        .await
        .unwrap_err();
    assert_ne!(missing.code, "runtime_error");
    let (status, missing_proxy) = raw(&router, "match $x isa entity;", "read").await;
    assert!(status.is_client_error() || status.is_server_error());
    let missing_proxy: Value = serde_json::from_slice(&missing_proxy).unwrap();
    assert_eq!(
        missing_proxy["typedb"],
        json!({"code":missing.code,"message":missing.message})
    );
    println!(
        "raw HTTP parity: 4 query kinds/shapes; refusal, audit, error-code passthrough; database {name}"
    );
}

use std::fs::File;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct ProductionServer {
    child: Option<Child>,
    container_name: Option<String>,
    stdout_path: std::path::PathBuf,
    stderr_path: std::path::PathBuf,
}

impl ProductionServer {
    fn spawn(config_path: &std::path::Path, log_directory: &std::path::Path) -> Self {
        let stdout_path = log_directory.join("type-bridge-server.stdout.log");
        let stderr_path = log_directory.join("type-bridge-server.stderr.log");
        let stdout = File::create(&stdout_path).expect("create production-server stdout capture");
        let stderr = File::create(&stderr_path).expect("create production-server stderr capture");
        let image = std::env::var("TYPE_BRIDGE_SERVER_IMAGE").ok();
        let container_name = image.as_ref().map(|_| {
            format!(
                "type-bridge-server-oci-{}-{}",
                std::process::id(),
                staged_live::SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            )
        });
        let mut command = if let Some(image) = image {
            let mount = format!(
                "type=bind,src={},dst={},readonly",
                log_directory.display(),
                log_directory.display()
            );
            let mut command = Command::new("docker");
            command.args([
                "run",
                "--rm",
                "--name",
                container_name
                    .as_deref()
                    .expect("container image always has a name"),
                "--network",
                "host",
                "--read-only",
                "--cap-drop",
                "ALL",
                "--security-opt",
                "no-new-privileges:true",
                "--user",
                "10001:10001",
                "--mount",
                &mount,
            ]);
            if let Ok(platform) = std::env::var("TYPE_BRIDGE_SERVER_PLATFORM") {
                command.args(["--platform", &platform]);
            }
            command.arg(image).arg("--config").arg(config_path);
            command
        } else {
            let mut command = Command::new(env!("CARGO_BIN_EXE_type-bridge-server"));
            command.arg("--config").arg(config_path);
            command
        };
        let child = command
            .env_remove("RUST_LOG")
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("spawn the production type-bridge-server process");
        Self {
            child: Some(child),
            container_name,
            stdout_path,
            stderr_path,
        }
    }

    fn exited(&mut self) -> Option<std::process::ExitStatus> {
        self.child
            .as_mut()
            .expect("production server child remains owned")
            .try_wait()
            .expect("inspect production server process")
    }

    fn diagnostics(&self) -> String {
        let stdout = std::fs::read_to_string(&self.stdout_path).unwrap_or_default();
        let stderr = std::fs::read_to_string(&self.stderr_path).unwrap_or_default();
        format!("stdout:\n{stdout}\nstderr:\n{stderr}")
    }

    fn stop(&mut self) -> std::io::Result<()> {
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        if let Some(container_name) = self.container_name.take() {
            let stopped = Command::new("docker")
                .args(["stop", "--time", "5", &container_name])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?;
            if !stopped.success() {
                let _ = Command::new("docker")
                    .args(["rm", "--force", &container_name])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
        if child.try_wait()?.is_none() {
            child.kill()?;
        }
        child.wait()?;
        Ok(())
    }
}

impl Drop for ProductionServer {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn reserve_loopback_port() -> u16 {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .expect("reserve a loopback port for the production server");
    listener.local_addr().expect("reserved address").port()
}

async fn wait_for_production_health(
    server: &mut ProductionServer,
    client: &reqwest::Client,
    base_url: &str,
) -> serde_json::Value {
    let startup_timeout = std::env::var("TYPE_BRIDGE_SERVER_STARTUP_TIMEOUT_SECONDS")
        .ok()
        .map(|raw| {
            raw.parse::<u64>()
                .expect("TYPE_BRIDGE_SERVER_STARTUP_TIMEOUT_SECONDS is a u64")
        })
        .unwrap_or(30);
    let deadline = Instant::now() + Duration::from_secs(startup_timeout);
    loop {
        if let Some(status) = server.exited() {
            panic!(
                "production server exited before health became ready ({status}): {}",
                server.diagnostics()
            );
        }
        if let Ok(response) = client.get(format!("{base_url}/health")).send().await
            && response.status() == reqwest::StatusCode::OK
        {
            let body = response.bytes().await.expect("read health body");
            return serde_json::from_slice(&body).expect("health body is JSON");
        }
        assert!(
            Instant::now() < deadline,
            "production server health did not become ready in {startup_timeout} seconds: {}",
            server.diagnostics()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB; optionally tests TYPE_BRIDGE_SERVER_IMAGE"]
async fn production_binary_serves_staged_and_removed_routes() {
    use type_bridge_contract::query_plan::Operation;
    use type_bridge_contract::query_remote::RemoteCapabilities;
    use type_bridge_contract::query_remote::{
        StagedAnswer, StagedLimits, StagedPlan, StagedRequest, StagedRequestSpec,
        VerifiedStagedReply,
    };
    use type_bridge_orm::remote_signing::Ed25519RemoteReplyVerifier;
    use type_bridge_schema::{build_schema_authority, encode_schema_authority};

    let name = staged_live::live_database::name("binary");
    let direct = runtime().await;
    direct.create_database(&name).await.unwrap();
    let schema = staged_live::race::SCHEMA_TYPEQL
        .replace(" @key", "")
        .replace(" @card(0..1)", "");
    direct
        .execute_raw(&name, &schema, TxType::Schema)
        .await
        .unwrap();
    let database = staged_live::connect(&name).await;
    staged_live::seed(&database, "I1", "A1").await;
    let profile = SemanticProfileId::new(
        type_bridge_core_lib::version::semantic_profile_id(&database.server_version().unwrap())
            .unwrap(),
    )
    .unwrap();
    let declared = staged_live::race::schema();
    let context = ManagedDeltaContext::new(
        ManagedScopeId::new("binary-live").unwrap(),
        profile,
        CapabilitySet::new(),
    );
    let authority =
        build_schema_authority(&declared, declared.required_capabilities(), &context).unwrap();
    database.close().unwrap();
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The unprivileged OCI process must read this mounted fixture directory.
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let authority_path = directory.path().join("schema-authority.json");
    std::fs::write(&authority_path, encode_schema_authority(&authority)).unwrap();
    let port = std::env::var("SMOKE_PORT")
        .map(|port| port.parse::<u16>().unwrap())
        .unwrap_or_else(|_| reserve_loopback_port());
    let config = json!({"server":{"host":"127.0.0.1","port":port},"typedb":{"address":std::env::var("TYPEDB_ADDRESS").unwrap(),"database":name,"username":std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_|"admin".into()),"password":std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_|"password".into()),"http_port":std::env::var("TYPEDB_HTTP_PORT").unwrap().parse::<u16>().unwrap(),"tls":false},"logging":{"level":"warn","format":"text"},"staged":{"enabled":true,"schema_authority_file":authority_path.to_str().unwrap(),"authority_mode":"query_only"}});
    let config_path = directory.path().join("server.toml");
    std::fs::write(&config_path, toml::to_string(&config).unwrap()).unwrap();
    let mut server = ProductionServer::spawn(&config_path, directory.path());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let base = format!("http://127.0.0.1:{port}");
    assert_eq!(
        wait_for_production_health(&mut server, &client, &base).await,
        json!({"status":"ok","version":"1.5.11","typedb_connected":true})
    );
    let health = client
        .get(format!("{base}/health"))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(
        health.as_ref(),
        br#"{"status":"ok","version":"1.5.11","typedb_connected":true}"#
    );
    for path in [
        "/query",
        "/query/raw",
        "/query/validate",
        "/schema",
        "/v2/query",
        "/v2/capabilities",
    ] {
        let response = client
            .post(format!("{base}{path}"))
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
        assert_eq!(
            response.bytes().await.unwrap().as_ref(),
            br#"{"error":"route_removed","use":"/v3/staged"}"#
        );
    }
    let response = client
        .post(format!("{base}/v3/raw"))
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    assert_eq!(
        response.bytes().await.unwrap().as_ref(),
        br#"{"error":"raw_disabled"}"#
    );
    let advertisement = client
        .get(format!("{base}/v3/capabilities"))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let advertisement = RemoteCapabilities::decode(&advertisement).unwrap();
    assert!(
        !advertisement
            .capabilities()
            .iter()
            .any(|capability| capability.as_str() == "query.remote.raw")
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let request = StagedRequest::new(
        StagedRequestSpec {
            nonce: format!("binary-{}", uuid::Uuid::new_v4()),
            kind: None,
            limits: StagedLimits::default(),
            plans: vec![
                StagedPlan::new(
                    &staged_live::race::stock_read(Operation::Count),
                    vec![],
                    &declared,
                )
                .unwrap(),
            ],
            prepared_at_unix_ms: now,
        },
        &declared,
        &advertisement,
    )
    .unwrap();
    let response = client
        .post(format!("{base}/v3/staged"))
        .header("content-type", "application/json")
        .body(request.encode().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let reply = VerifiedStagedReply::decode(
        &response.bytes().await.unwrap(),
        &request,
        &advertisement,
        advertisement.reply_key(),
        &Ed25519RemoteReplyVerifier,
    )
    .unwrap();
    assert!(reply.committed(), "{:?}", reply.diagnostic());
    assert_eq!(reply.answers(), [StagedAnswer::Count { value: 1 }]);
    assert!(server.exited().is_none(), "{}", server.diagnostics());
    server.stop().unwrap();
    direct.delete_database(&name).await.unwrap();
}
