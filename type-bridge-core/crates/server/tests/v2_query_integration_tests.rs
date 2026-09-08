//! V2 envelope endpoints beside the retained V1 surface.
//!
//! The V2 route test executes against a live TypeDB (TYPEDB_ADDRESS /
//! TYPEDB_HTTP_PORT); run it explicitly with `-- --ignored`.

mod support;

use std::collections::BTreeMap;
use std::fs::File;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde::Serialize;
use tower::ServiceExt;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
use type_bridge_contract::query_plan::{
    OrderDirection, OrderTerm, QueryInvocation, QueryOperation, QueryOutput, QueryPattern,
    QueryPlan, ReadStage, query_plan_v2_capability_vocabulary,
};
use type_bridge_contract::query_remote::{RemoteCapabilities, RemoteLimits};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, OwnsFact, OwnsFactId, SchemaFact, SourceSpan, SourcedSchemaFact,
    TypeFact, ValueFact, ValueFactId,
};
use type_bridge_contract::schema_delta::ManagedSchemaState;
use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};
use type_bridge_orm::TxType;
use type_bridge_orm::query_v2::{QueryRowValue, QueryV2Outcome};
use type_bridge_orm::query_v2_remote::{decode_remote_outcome, encode_remote_request};
use type_bridge_orm::session::backend::QueryV2AnswerLimits;
use type_bridge_orm::session::database::Database;
use type_bridge_orm::session::real_driver::{
    ConnectOptions, SecureConnectOptions, delete_database_secure, ensure_database_exists,
};
use type_bridge_query::{MigrationAssertionValidationContext, ValidatedQuery, validate_query_plan};
use type_bridge_schema::{
    ManagedDeltaContext, ResolvedSchema, build_schema_authority, encode_schema_authority,
    managed_schema_state, resolve,
};
use type_bridge_server::transport::v2::{V2QueryState, create_router_with_v2};

use support::{MockExecutor, make_pipeline};

fn binding(id: u16, variable: &str) -> AssertionBinding {
    AssertionBinding::new(
        BindingId::new(id).expect("binding id"),
        QueryVariable::new(variable).expect("variable"),
    )
}

fn binding_id(id: u16) -> BindingId {
    BindingId::new(id).expect("binding id")
}

static DATABASE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct LiveQueryFixture {
    address: String,
    username: String,
    password: String,
    http_port: u16,
    database_name: String,
    database: Database,
    declared: DeclaredSchema,
    delta_context: ManagedDeltaContext,
    managed: ManagedSchemaState,
    resolved: ResolvedSchema,
    plan: QueryPlan,
    validated: ValidatedQuery,
    invocation: QueryInvocation,
}

async fn live_query_fixture(test_name: &str) -> LiveQueryFixture {
    let address = std::env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1730".into());
    let username = std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".into());
    let password = std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".into());
    let http_port = std::env::var("TYPEDB_HTTP_PORT")
        .unwrap_or_else(|_| "8000".into())
        .parse::<u16>()
        .expect("TYPEDB_HTTP_PORT is a u16");
    let sequence = DATABASE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let database_name = format!("tb_server_v2_{test_name}_{}_{sequence}", std::process::id());
    let connect_options = ConnectOptions {
        http_port,
        ..ConnectOptions::default()
    };
    ensure_database_exists(
        &address,
        &database_name,
        &username,
        &password,
        connect_options,
    )
    .await
    .expect("database exists");
    let database = Database::connect_with_options(
        &address,
        &database_name,
        &username,
        &password,
        connect_options,
    )
    .await
    .expect("connected database");

    let person = TypeId::new(TypeKind::Entity, "server-v2-person").unwrap();
    let name = AttributeId::new("server-v2-name").unwrap();
    database
        .execute_raw(
            &format!(
                "define\n\
                 attribute {name}, value string;\n\
                 entity {person}, owns {name};",
                name = name.label(),
                person = person.label(),
            ),
            TxType::Schema,
        )
        .await
        .expect("schema definition");
    database
        .execute_raw(
            &format!(
                "insert $a isa {person}, has {name} \"ada\"; \
                 $b isa {person}, has {name} \"bob\";",
                person = person.label(),
                name = name.label(),
            ),
            TxType::Write,
        )
        .await
        .expect("data insertion");

    let facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).unwrap()),
        SchemaFact::Type(
            TypeFact::new(TypeId::new(TypeKind::Attribute, name.label().as_str()).unwrap())
                .unwrap(),
        ),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), name.clone()).unwrap(),
        )),
    ];
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).unwrap();
        let line = u32::try_from(index + 1).unwrap();
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("server-v2-query").unwrap(),
                byte,
                byte + 1,
                line,
                1,
                line,
                2,
            )
            .unwrap(),
        )
    });
    let declared =
        DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced).unwrap();
    let server_version = database
        .server_version()
        .expect("the live server version is observed through its configured HTTP port");
    let profile = SemanticProfileId::new(format!("typedb-{server_version}/v1")).unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    let delta_context = ManagedDeltaContext::new(
        ManagedScopeId::new(format!("server-v2-query-{test_name}-{sequence}")).unwrap(),
        profile,
        CapabilitySet::new(),
    );
    let managed = managed_schema_state(&declared, &delta_context).unwrap();

    let plan = QueryPlan::new(
        vec![binding(0, "person"), binding(1, "name")],
        Vec::new(),
        vec![
            ReadStage::Match {
                patterns: vec![
                    QueryPattern::Isa {
                        binding: binding_id(0),
                        include_subtypes: true,
                        type_id: person,
                    },
                    QueryPattern::Has {
                        attribute: binding_id(1),
                        attribute_id: name,
                        owner: binding_id(0),
                    },
                ],
            },
            ReadStage::Sort {
                terms: vec![OrderTerm::new(binding_id(1), OrderDirection::Ascending)],
            },
        ],
        QueryOutput::Rows {
            columns: vec![binding_id(0), binding_id(1)],
        },
        managed.managed_semantic_schema().clone(),
    )
    .unwrap();
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &managed);
    let validated =
        validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL).unwrap();
    let invocation = QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).unwrap();

    LiveQueryFixture {
        address,
        username,
        password,
        http_port,
        database_name,
        database,
        declared,
        delta_context,
        managed,
        resolved,
        plan,
        validated,
        invocation,
    }
}

fn assert_names(outcome: &QueryV2Outcome) {
    let QueryV2Outcome::Rows(rows) = outcome else {
        panic!("rows outcome: {outcome:?}");
    };
    let names = rows
        .iter()
        .map(|row| match &row.values()[1] {
            QueryRowValue::Attribute {
                value: CanonicalValue::String(value),
                ..
            } => value.as_str().to_owned(),
            other => panic!("expected string names: {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["ada".to_owned(), "bob".to_owned()]);
}

#[derive(Serialize)]
struct ProductionServerConfig<'a> {
    server: ProductionListener,
    typedb: ProductionTypeDb<'a>,
    logging: ProductionLogging,
    v2: ProductionV2<'a>,
}

#[derive(Serialize)]
struct ProductionListener {
    host: &'static str,
    port: u16,
}

#[derive(Serialize)]
struct ProductionTypeDb<'a> {
    address: &'a str,
    database: &'a str,
    username: &'a str,
    password: &'a str,
    http_port: u16,
    tls: bool,
}

#[derive(Serialize)]
struct ProductionLogging {
    level: &'static str,
    format: &'static str,
}

#[derive(Serialize)]
struct ProductionV2<'a> {
    enabled: bool,
    schema_authority_file: &'a str,
    authority_mode: &'static str,
}

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
                DATABASE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
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
            let body = response.bytes().await.expect("read V1 health body");
            return serde_json::from_slice(&body).expect("V1 health body is JSON");
        }
        assert!(
            Instant::now() < deadline,
            "production server health did not become ready in {startup_timeout} seconds: {}",
            server.diagnostics()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

struct ProductionV1RouteSnapshot {
    method: reqwest::Method,
    path: &'static str,
    request_body: Option<&'static [u8]>,
    status: reqwest::StatusCode,
    response_body: &'static [u8],
}

async fn assert_production_v1_route_snapshot(
    client: &reqwest::Client,
    base_url: &str,
    snapshot: ProductionV1RouteSnapshot,
) {
    let method = snapshot.method.as_str().to_owned();
    let mut request = client.request(snapshot.method, format!("{base_url}{}", snapshot.path));
    if let Some(body) = snapshot.request_body {
        request = request
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);
    }
    let response = request.send().await.expect("production V1 response");
    assert_eq!(
        response.status(),
        snapshot.status,
        "{method} {} status",
        snapshot.path,
    );
    let mut actual_headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                value
                    .to_str()
                    .expect("production V1 response header is ASCII")
                    .to_owned(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let date = actual_headers
        .remove("date")
        .expect("production HTTP transport retains its released Date header");
    assert_eq!(date.len(), 29, "released Date header wire width");
    assert!(date.ends_with(" GMT"), "released Date header timezone");
    chrono::DateTime::parse_from_rfc2822(&date)
        .expect("production Date header remains an RFC 2822 timestamp");
    let expected_headers = BTreeMap::from([
        (
            "content-length".to_owned(),
            snapshot.response_body.len().to_string(),
        ),
        ("content-type".to_owned(), "application/json".to_owned()),
    ]);
    assert_eq!(
        actual_headers, expected_headers,
        "{method} {} released headers",
        snapshot.path,
    );
    let body = response.bytes().await.expect("production V1 response body");
    assert_eq!(
        body.as_ref(),
        snapshot.response_body,
        "{method} {} released body",
        snapshot.path,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn v2_envelope_endpoints_serve_beside_v1() {
    let LiveQueryFixture {
        database,
        declared,
        delta_context,
        managed,
        resolved,
        plan,
        validated,
        invocation,
        ..
    } = live_query_fixture("router").await;

    let state = Arc::new(
        V2QueryState::new_query_only(
            query_plan_v2_capability_vocabulary(),
            QueryV2AnswerLimits::default(),
            database,
            declared,
            delta_context,
            managed,
            resolved,
        )
        .expect("executor advertisement is canonical"),
    );
    let router = create_router_with_v2(Arc::new(make_pipeline(MockExecutor::new(), false)), state);

    // The retained V1 surface still answers.
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Negotiation: the executor advertises the complete V2 vocabulary.
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v2/capabilities")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let advertised = RemoteCapabilities::decode(&bytes).expect("advertisement");
    for capability in query_plan_v2_capability_vocabulary().iter() {
        assert!(advertised.capabilities().contains(capability));
    }
    for capability in plan.required_capabilities().iter() {
        assert!(advertised.capabilities().contains(capability));
    }

    // The envelope executes through the versioned endpoint.
    let nonce = "server-v2-nonce-0123456789";
    let limits = RemoteLimits {
        deadline_ms: Some(30_000),
        max_bytes: 1 << 20,
        max_items: 100,
        max_collection_members: 1 << 16,
    };
    let request = encode_remote_request(&validated, &invocation, &advertised, limits, nonce)
        .expect("request envelope");
    let expected_request =
        type_bridge_contract::query_remote::RemoteRequestFingerprint::compute(&request)
            .expect("request fingerprint");
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v2/query")
                .header("content-type", "application/json")
                .body(Body::from(request.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let advertisement_fingerprint = advertised.fingerprint().expect("advertisement fingerprint");
    let outcome = decode_remote_outcome(
        &bytes,
        &validated,
        QueryOperation::Rows,
        nonce,
        &expected_request,
        &advertisement_fingerprint,
        advertised.reply_key(),
        limits,
    )
    .expect("typed outcome");
    assert_names(&outcome);

    // The exact request is one-shot at the executor; replay rejects with a
    // failure bound to the original request before another query.
    let replay = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v2/query")
                .header("content-type", "application/json")
                .body(Body::from(request))
                .unwrap(),
        )
        .await
        .unwrap();
    let replay = replay.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        decode_remote_outcome(
            &replay,
            &validated,
            QueryOperation::Rows,
            nonce,
            &expected_request,
            &advertisement_fingerprint,
            advertised.reply_key(),
            limits,
        )
        .expect_err("exact replay must be rejected")
        .code()
        .as_str(),
        "query_remote_replay",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn production_binary_serves_v1_health_and_v2_query() {
    let LiveQueryFixture {
        address,
        username,
        password,
        http_port,
        database_name,
        database,
        declared,
        delta_context,
        plan,
        validated,
        invocation,
        ..
    } = live_query_fixture("production").await;
    let directory = tempfile::tempdir().expect("production-server test directory");
    let authority =
        build_schema_authority(&declared, declared.required_capabilities(), &delta_context)
            .expect("build source-free schema authority");
    let authority_path = directory.path().join("schema-authority.json");
    std::fs::write(&authority_path, encode_schema_authority(&authority))
        .expect("write canonical schema-authority fixture");

    // The production process must construct its own driver and live authority;
    // it cannot inherit the fixture's already-connected Database handle.
    database.close().expect("close fixture database connection");
    drop(database);

    let port = reserve_loopback_port();
    let schema_authority_file = authority_path
        .to_str()
        .expect("temporary schema-authority path is UTF-8");
    let config = ProductionServerConfig {
        server: ProductionListener {
            host: "127.0.0.1",
            port,
        },
        typedb: ProductionTypeDb {
            address: &address,
            database: &database_name,
            username: &username,
            password: &password,
            http_port,
            tls: false,
        },
        logging: ProductionLogging {
            level: "info",
            format: "text",
        },
        v2: ProductionV2 {
            enabled: true,
            schema_authority_file,
            authority_mode: "query_only",
        },
    };
    let config_path = directory.path().join("server.toml");
    std::fs::write(
        &config_path,
        toml::to_string(&config).expect("serialize production server config"),
    )
    .expect("write production server config");

    let mut server = ProductionServer::spawn(&config_path, directory.path());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .expect("bounded HTTP client");
    let base_url = format!("http://127.0.0.1:{port}");
    wait_for_production_health(&mut server, &client, &base_url).await;
    const QUERY_FAILURE: &[u8] = br#"{"status":"error","error":{"code":"QUERY_EXECUTION_ERROR","message":"Query execution error: Unknown transaction type: v1-wire-probe"}}"#;
    const SCHEMA_FAILURE: &[u8] = br#"{"status":"error","error":{"code":"SCHEMA_ERROR","message":"Schema error: No schema loaded"}}"#;
    let v1_snapshots = [
        ProductionV1RouteSnapshot {
            method: reqwest::Method::GET,
            path: "/health",
            request_body: None,
            status: reqwest::StatusCode::OK,
            response_body: br#"{"status":"ok","version":"1.5.11","typedb_connected":true}"#,
        },
        ProductionV1RouteSnapshot {
            method: reqwest::Method::POST,
            path: "/query",
            request_body: Some(br#"{"transaction_type":"v1-wire-probe","clauses":[]}"#),
            status: reqwest::StatusCode::BAD_REQUEST,
            response_body: QUERY_FAILURE,
        },
        ProductionV1RouteSnapshot {
            method: reqwest::Method::POST,
            path: "/query/raw",
            request_body: Some(
                br#"{"transaction_type":"v1-wire-probe","query":"match $p isa person; fetch { \"person\": { $p.* } };"}"#,
            ),
            status: reqwest::StatusCode::BAD_REQUEST,
            response_body: QUERY_FAILURE,
        },
        ProductionV1RouteSnapshot {
            method: reqwest::Method::POST,
            path: "/query/validate",
            request_body: Some(br#"{"clauses":[]}"#),
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            response_body: SCHEMA_FAILURE,
        },
        ProductionV1RouteSnapshot {
            method: reqwest::Method::GET,
            path: "/schema",
            request_body: None,
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            response_body: SCHEMA_FAILURE,
        },
    ];
    for snapshot in v1_snapshots {
        assert_production_v1_route_snapshot(&client, &base_url, snapshot).await;
    }

    let response = client
        .get(format!("{base_url}/v2/capabilities"))
        .header("authorization", "Bearer production-live-smoke")
        .send()
        .await
        .expect("production capabilities request");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let advertised = RemoteCapabilities::decode(
        &response
            .bytes()
            .await
            .expect("production capabilities body"),
    )
    .expect("production capability advertisement");
    for capability in query_plan_v2_capability_vocabulary().iter() {
        assert!(advertised.capabilities().contains(capability));
    }
    for capability in plan.required_capabilities().iter() {
        assert!(advertised.capabilities().contains(capability));
    }

    let nonce = "production-server-v2-nonce-0123456789";
    let limits = RemoteLimits {
        deadline_ms: Some(30_000),
        max_bytes: 1 << 20,
        max_items: 100,
        max_collection_members: 1 << 16,
    };
    let request = encode_remote_request(&validated, &invocation, &advertised, limits, nonce)
        .expect("production request envelope");
    let expected_request =
        type_bridge_contract::query_remote::RemoteRequestFingerprint::compute(&request)
            .expect("production request fingerprint");
    let response = client
        .post(format!("{base_url}/v2/query"))
        .header("content-type", "application/json")
        .header("authorization", "Bearer production-live-smoke")
        .body(request)
        .send()
        .await
        .expect("production V2 query request");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let response = response.bytes().await.expect("production V2 query body");
    let advertisement_fingerprint = advertised
        .fingerprint()
        .expect("production advertisement fingerprint");
    let outcome = decode_remote_outcome(
        &response,
        &validated,
        QueryOperation::Rows,
        nonce,
        &expected_request,
        &advertisement_fingerprint,
        advertised.reply_key(),
        limits,
    )
    .expect("nonce- and request-fingerprint-bound production response");
    assert_names(&outcome);
    assert!(
        server.exited().is_none(),
        "production server exited after its first V2 request: {}",
        server.diagnostics()
    );

    server
        .stop()
        .expect("terminate and reap production server process");
    delete_database_secure(
        &address,
        &database_name,
        &username,
        &password,
        SecureConnectOptions {
            http_port,
            ..SecureConnectOptions::default()
        },
    )
    .await
    .expect("delete production-server smoke database");
}
