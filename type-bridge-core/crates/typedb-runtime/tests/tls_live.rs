//! Opt-in live proofs for custom-root and native-root TLS across HTTP
//! discovery, gRPC fallback, and the selected TypeDB driver band.

use std::future::Future;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use type_bridge_typedb_runtime::{
    RuntimeAnswerCancellation, RuntimeAnswerControl, RuntimeAnswerLimits, RuntimeError,
    SecureConnectError, SecureConnectOptions, SecureResult, TlsMode, TxType, TypeDBRuntime,
    database_exists_secure, delete_database_secure, embedded_driver_versions,
    ensure_database_exists_secure,
};

#[derive(Clone, Debug)]
struct ExpectedTopology {
    server_version: String,
    driver_band: u8,
    driver_version: String,
}

#[derive(Clone, Debug)]
struct LiveTlsContext {
    address: String,
    username: String,
    password: String,
    http_port: u16,
    root_ca: PathBuf,
    expected: Option<ExpectedTopology>,
    native_roots: bool,
}

impl LiveTlsContext {
    fn custom_root_options(&self) -> SecureConnectOptions {
        SecureConnectOptions {
            http_port: self.http_port,
            tls_mode: TlsMode::CustomRootCa(self.root_ca.clone()),
            server_version: None,
        }
    }

    fn native_root_options(&self) -> SecureConnectOptions {
        SecureConnectOptions {
            http_port: self.http_port,
            tls_mode: TlsMode::NativeRoots,
            server_version: None,
        }
    }

    fn has_loopback_address(&self) -> bool {
        self.address.starts_with("127.0.0.1:") || self.address.starts_with("localhost:")
    }
}

fn required_environment(name: &str, required: bool) -> Option<String> {
    match std::env::var(name) {
        Ok(value) => Some(value),
        Err(_) if required => panic!("{name} is required by TYPE_BRIDGE_TLS_LIVE_REQUIRED=1"),
        Err(_) => None,
    }
}

fn live_tls_context() -> Option<LiveTlsContext> {
    let required = std::env::var("TYPE_BRIDGE_TLS_LIVE_REQUIRED").as_deref() == Ok("1");
    let address = required_environment("TYPEDB_TLS_ADDRESS", required)?;
    let root_ca = PathBuf::from(required_environment("TYPEDB_TLS_ROOT_CA", required)?);
    let http_port = required_environment("TYPEDB_TLS_HTTP_PORT", required)?
        .parse::<u16>()
        .expect("TYPEDB_TLS_HTTP_PORT must be a u16");
    let username = std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned());
    let password = std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned());

    let expected_server = required_environment("TYPE_BRIDGE_TLS_EXPECTED_SERVER_VERSION", required);
    let expected_band = required_environment("TYPE_BRIDGE_TLS_EXPECTED_DRIVER_BAND", required);
    let expected_driver = required_environment("TYPE_BRIDGE_TLS_EXPECTED_DRIVER_VERSION", required);
    let expected = match (expected_server, expected_band, expected_driver) {
        (Some(server_version), Some(driver_band), Some(driver_version)) => Some(ExpectedTopology {
            server_version,
            driver_band: driver_band
                .parse::<u8>()
                .expect("TYPE_BRIDGE_TLS_EXPECTED_DRIVER_BAND must be a u8"),
            driver_version,
        }),
        (None, None, None) => None,
        _ => panic!(
            "TYPE_BRIDGE_TLS_EXPECTED_SERVER_VERSION, \
             TYPE_BRIDGE_TLS_EXPECTED_DRIVER_BAND, and \
             TYPE_BRIDGE_TLS_EXPECTED_DRIVER_VERSION must be supplied together"
        ),
    };

    let native_roots = std::env::var("TYPE_BRIDGE_TLS_NATIVE_ROOTS").as_deref() == Ok("1");
    assert!(
        !required || native_roots,
        "the required CI lane must set TYPE_BRIDGE_TLS_NATIVE_ROOTS=1"
    );
    assert!(
        !required || address.starts_with("127.0.0.1:"),
        "the required CI lane must use a loopback TLS endpoint so the plaintext trap is local"
    );

    Some(LiveTlsContext {
        address,
        username,
        password,
        http_port,
        root_ca,
        expected,
        native_roots,
    })
}

fn unique_database(prefix: &str) -> String {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time follows the Unix epoch")
        .as_nanos();
    format!("{prefix}_{}_{}", std::process::id(), suffix)
}

// Hosted runners can take slightly more than ten seconds to receive TypeDB's
// close acknowledgement under a fully concurrent matrix. Keep every live
// stage bounded while leaving enough headroom to distinguish load from a hang.
const LIVE_TLS_STAGE_TIMEOUT: Duration = Duration::from_secs(30);
const FORCE_CLOSE_RELEASE_TIMEOUT: Duration = Duration::from_secs(5);
const FORCE_CLOSE_RELEASE_RETRY_INTERVAL: Duration = Duration::from_millis(25);
const DATABASE_IN_USE_DIAGNOSTIC: &str = "Cannot delete database since it is in use";

async fn await_live_tls_stage<T>(stage: &'static str, future: impl Future<Output = T>) -> T {
    tokio::time::timeout(LIVE_TLS_STAGE_TIMEOUT, future)
        .await
        .unwrap_or_else(|_| {
            panic!(
                "live TLS stage `{stage}` timed out after {} seconds",
                LIVE_TLS_STAGE_TIMEOUT.as_secs()
            )
        })
}

fn database_release_is_pending_after_force_close(error: &SecureConnectError) -> bool {
    matches!(
        error,
        SecureConnectError::Runtime(RuntimeError::Connection(message))
            if message.contains("[DBD2]")
                && message.contains(DATABASE_IN_USE_DIAGNOSTIC)
                && message.contains("[SRV13]")
    )
}

async fn delete_owned_database_after_force_close(
    context: &LiveTlsContext,
    database: &str,
) -> SecureResult<()> {
    let deadline = tokio::time::Instant::now() + FORCE_CLOSE_RELEASE_TIMEOUT;
    loop {
        let result = tokio::time::timeout_at(
            deadline,
            delete_database_secure(
                &context.address,
                database,
                &context.username,
                &context.password,
                context.custom_root_options(),
            ),
        )
        .await
        .map_err(|_| {
            SecureConnectError::Runtime(RuntimeError::Connection(format!(
                "Database delete did not complete within {} seconds after force-close",
                FORCE_CLOSE_RELEASE_TIMEOUT.as_secs()
            )))
        })?;
        match result {
            Err(error) if database_release_is_pending_after_force_close(&error) => {
                // The official driver acknowledges local shutdown dispatch,
                // not the server's observation of the closed transport. This
                // fixture owns the unique database, so only the exact
                // DBD2/in-use/SRV13 diagnostic is safe to classify as release
                // propagation.
                // This poll is not evidence for issue #196's upstream-removal
                // gate, which still requires release without a downstream
                // retry.
                let retry_at = (tokio::time::Instant::now() + FORCE_CLOSE_RELEASE_RETRY_INTERVAL)
                    .min(deadline);
                tokio::time::sleep_until(retry_at).await;
            }
            result => return result,
        }
    }
}

#[test]
fn force_close_release_retry_classification_is_narrow() {
    let pending = SecureConnectError::Runtime(RuntimeError::Connection(
        "Database delete failed: [DBD2] Cannot delete database since it is in use. Caused: [SRV13]"
            .to_owned(),
    ));
    let wrong_diagnostic = SecureConnectError::Runtime(RuntimeError::Connection(
        "Database delete failed: [DBD2] permission denied. Caused: [SRV13]".to_owned(),
    ));
    let wrong_variant = SecureConnectError::Runtime(RuntimeError::Transaction(
        "[DBD2] Cannot delete database since it is in use. [SRV13]".to_owned(),
    ));
    let missing_server_code = SecureConnectError::Runtime(RuntimeError::Connection(
        "[DBD2] Cannot delete database since it is in use.".to_owned(),
    ));
    let missing_database_code = SecureConnectError::Runtime(RuntimeError::Connection(
        "Cannot delete database since it is in use. [SRV13]".to_owned(),
    ));

    assert!(database_release_is_pending_after_force_close(&pending));
    assert!(!database_release_is_pending_after_force_close(
        &wrong_diagnostic
    ));
    assert!(!database_release_is_pending_after_force_close(
        &wrong_variant
    ));
    assert!(!database_release_is_pending_after_force_close(
        &missing_server_code
    ));
    assert!(!database_release_is_pending_after_force_close(
        &missing_database_code
    ));
}

fn assert_expected_topology(runtime: &TypeDBRuntime, context: &LiveTlsContext) {
    let Some(expected) = &context.expected else {
        return;
    };

    assert!(
        embedded_driver_versions().iter().any(|(band, version)| {
            *band == expected.driver_band && *version == expected.driver_version
        }),
        "expected driver band {} at {}, embedded graph is {:?}",
        expected.driver_band,
        expected.driver_version,
        embedded_driver_versions()
    );

    assert_eq!(
        runtime.server_version().map(|version| version.to_string()),
        Some(expected.server_version.clone()),
        "the live server version must match the CI topology"
    );

    assert_eq!(
        runtime.supports_given_rows(),
        expected.driver_band == 9,
        "only the active official band-9 driver can transport given rows"
    );
}

/// A local endpoint that records whether the version probe speaks TLS or
/// plaintext. It closes TLS ClientHello connections immediately, forcing the
/// runtime onto its gRPC fallback. A plaintext request is recorded as a test
/// failure even if later fallback logic could otherwise recover.
struct HttpProbeTrap {
    port: u16,
    stop: Arc<AtomicBool>,
    saw_tls_client_hello: Arc<AtomicBool>,
    saw_plaintext_get: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl HttpProbeTrap {
    fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind HTTP probe trap");
        let port = listener.local_addr().expect("probe trap address").port();
        listener
            .set_nonblocking(true)
            .expect("make HTTP probe trap nonblocking");

        let stop = Arc::new(AtomicBool::new(false));
        let saw_tls_client_hello = Arc::new(AtomicBool::new(false));
        let saw_plaintext_get = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker_saw_tls = Arc::clone(&saw_tls_client_hello);
        let worker_saw_plaintext = Arc::clone(&saw_plaintext_get);
        let worker = thread::spawn(move || {
            while !worker_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _peer)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .expect("set probe trap read timeout");
                        let mut bytes = [0_u8; 4096];
                        if let Ok(read) = stream.read(&mut bytes)
                            && read > 0
                        {
                            if bytes[0] == 0x16 {
                                worker_saw_tls.store(true, Ordering::Release);
                            }
                            if bytes[0] == b'G' {
                                worker_saw_plaintext.store(true, Ordering::Release);
                                let _ = stream.write_all(
                                    b"HTTP/1.1 400 Bad Request\r\n\
                                      Content-Length: 0\r\n\
                                      Connection: close\r\n\r\n",
                                );
                            }
                        }
                        let _ = stream.shutdown(Shutdown::Both);
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("HTTP probe trap accept failed: {error}"),
                }
            }
        });

        Self {
            port,
            stop,
            saw_tls_client_hello,
            saw_plaintext_get,
            worker: Some(worker),
        }
    }

    fn finish(mut self) -> (bool, bool) {
        self.stop.store(true, Ordering::Release);
        self.worker
            .take()
            .expect("probe trap worker exists")
            .join()
            .expect("probe trap worker did not panic");
        (
            self.saw_tls_client_hello.load(Ordering::Acquire),
            self.saw_plaintext_get.load(Ordering::Acquire),
        )
    }
}

impl Drop for HttpProbeTrap {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn custom_root_http_discovery_and_grpc_lifecycle_live() {
    let Some(context) = live_tls_context() else {
        eprintln!(
            "skipping live TLS transport test: set TYPEDB_TLS_ADDRESS, \
             TYPEDB_TLS_HTTP_PORT, and TYPEDB_TLS_ROOT_CA"
        );
        return;
    };
    let database = unique_database("tb_tls_runtime");
    let options = context.custom_root_options();

    ensure_database_exists_secure(
        &context.address,
        &database,
        &context.username,
        &context.password,
        options.clone(),
    )
    .await
    .expect("custom-root lifecycle creates a database over TLS");
    assert!(
        database_exists_secure(
            &context.address,
            &database,
            &context.username,
            &context.password,
            options.clone(),
        )
        .await
        .expect("custom-root lifecycle checks a database over TLS")
    );

    let runtime = TypeDBRuntime::connect_secure(
        &context.address,
        &context.username,
        &context.password,
        options.clone(),
    )
    .await
    .expect("custom-root HTTP discovery and gRPC connection both succeed");
    assert_expected_topology(&runtime, &context);
    drop(runtime);

    delete_database_secure(
        &context.address,
        &database,
        &context.username,
        &context.password,
        options,
    )
    .await
    .expect("custom-root lifecycle deletes the database over TLS");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn custom_root_raw_stop_requires_force_close_before_delete_live() {
    let Some(context) = live_tls_context() else {
        eprintln!(
            "skipping live TLS raw-stop cleanup test: set TYPEDB_TLS_ADDRESS, \
             TYPEDB_TLS_HTTP_PORT, and TYPEDB_TLS_ROOT_CA"
        );
        return;
    };
    let database = unique_database("tb_tls_terminal_close");
    let runtime = await_live_tls_stage(
        "raw-stop/connect",
        TypeDBRuntime::connect_secure(
            &context.address,
            &context.username,
            &context.password,
            context.custom_root_options(),
        ),
    )
    .await
    .expect("custom-root terminal-close runtime connects over TLS");
    assert_expected_topology(&runtime, &context);
    await_live_tls_stage(
        "raw-stop/create-database",
        runtime.create_database(&database),
    )
    .await
    .expect("terminal-close database is created over TLS");

    let mut schema = await_live_tls_stage(
        "raw-stop/open-schema",
        runtime.open_transaction(&database, TxType::Schema),
    )
    .await
    .expect("terminal-close schema transaction opens over TLS");
    await_live_tls_stage(
        "raw-stop/define-schema",
        schema.query("define entity tls-terminal-close-person;"),
    )
    .await
    .expect("terminal-close schema is defined over TLS");
    await_live_tls_stage("raw-stop/commit-schema", schema.commit())
        .await
        .expect("terminal-close schema commits over TLS");

    let mut write = await_live_tls_stage(
        "raw-stop/open-write",
        runtime.open_transaction(&database, TxType::Write),
    )
    .await
    .expect("terminal-close write transaction opens over TLS");
    await_live_tls_stage(
        "raw-stop/insert",
        write.query(
            "insert $first isa tls-terminal-close-person; \
             $second isa tls-terminal-close-person; \
             $third isa tls-terminal-close-person;",
        ),
    )
    .await
    .expect("terminal-close rows are inserted over TLS");
    await_live_tls_stage("raw-stop/commit-write", write.commit())
        .await
        .expect("terminal-close rows commit over TLS");

    let mut read = await_live_tls_stage(
        "raw-stop/open-read",
        runtime.open_transaction(&database, TxType::Read),
    )
    .await
    .expect("terminal-close read transaction opens over TLS");
    let mut stop_after_first = |_item| Ok(RuntimeAnswerControl::Stop);
    let stats = await_live_tls_stage(
        "raw-stop/query",
        read.query_bounded(
            "match $person isa tls-terminal-close-person; select $person;",
            RuntimeAnswerLimits {
                max_items: 3,
                max_bytes: 1024 * 1024,
                deadline: None,
                cancellation: RuntimeAnswerCancellation::default(),
            },
            &mut stop_after_first,
        ),
    )
    .await
    .expect("bounded TLS query delivers its first row");
    assert_eq!(stats.processed_items, 1);
    assert!(stats.stopped_early);
    // A raw `Stop` deliberately leaves a resumable driver stream. TypeDB
    // 3.12.1 does not acknowledge transaction close, so this low-level test
    // must not claim that close alone proves server-side release (issue #196).
    // Make the shared driver terminal first. RuntimeTransaction::close then
    // observes that shutdown has started and drops the driver transaction
    // locally instead of waiting forever for that absent acknowledgement.
    runtime
        .force_close()
        .expect("raw-stop TLS runtime connection force-closes");
    await_live_tls_stage("raw-stop/close-after-force-close", read.close())
        .await
        .expect("raw-stop transaction releases locally after force-close");
    drop(read);
    drop(runtime);

    await_live_tls_stage(
        "raw-stop/delete-after-force-close",
        delete_owned_database_after_force_close(&context, &database),
    )
    .await
    .expect("a fresh TLS connection deletes the database after force-close");
    assert!(
        !await_live_tls_stage(
            "raw-stop/verify-deleted",
            database_exists_secure(
                &context.address,
                &database,
                &context.username,
                &context.password,
                context.custom_root_options(),
            ),
        )
        .await
        .expect("TLS database lookup works after raw-stop cleanup")
    );
    await_live_tls_stage(
        "raw-stop/recreate",
        ensure_database_exists_secure(
            &context.address,
            &database,
            &context.username,
            &context.password,
            context.custom_root_options(),
        ),
    )
    .await
    .expect("the same TLS database name is immediately reusable");
    await_live_tls_stage(
        "raw-stop/final-delete",
        delete_database_secure(
            &context.address,
            &database,
            &context.username,
            &context.password,
            context.custom_root_options(),
        ),
    )
    .await
    .expect("TLS raw-stop fixture cleans up");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn https_failure_falls_back_to_secure_grpc_without_plaintext_live() {
    let Some(context) = live_tls_context() else {
        eprintln!("skipping live TLS gRPC fallback test: TLS endpoint variables are not set");
        return;
    };
    if !context.has_loopback_address() {
        eprintln!("skipping local plaintext trap for non-loopback TLS endpoint");
        return;
    }

    let trap = HttpProbeTrap::start();
    let mut options = context.custom_root_options();
    options.http_port = trap.port;
    let runtime = tokio::time::timeout(
        Duration::from_secs(30),
        TypeDBRuntime::connect_secure(
            &context.address,
            &context.username,
            &context.password,
            options,
        ),
    )
    .await
    .expect("secure gRPC fallback must be bounded")
    .expect("HTTPS failure falls back through TLS-enabled gRPC drivers");
    let (saw_tls_client_hello, saw_plaintext_get) = trap.finish();

    assert!(
        saw_tls_client_hello,
        "the failing HTTP discovery attempt must start with a TLS ClientHello"
    );
    assert!(
        !saw_plaintext_get,
        "an enabled HTTP discovery path must never retry a plaintext GET"
    );
    assert_expected_topology(&runtime, &context);
    assert!(
        !runtime
            .database_exists(&unique_database("tb_tls_fallback_absent"))
            .await
            .expect("the fallback driver performs a live TLS round trip"),
        "the fresh fallback probe database must not already exist"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_roots_cover_http_and_selected_grpc_band_live() {
    let Some(context) = live_tls_context() else {
        eprintln!("skipping native-root TLS test: TLS endpoint variables are not set");
        return;
    };
    if !context.native_roots {
        eprintln!(
            "skipping native-root TLS test: set TYPE_BRIDGE_TLS_NATIVE_ROOTS=1 \
             after configuring the fixture root as a native trust input"
        );
        return;
    }

    let runtime = TypeDBRuntime::connect_secure(
        &context.address,
        &context.username,
        &context.password,
        context.native_root_options(),
    )
    .await
    .expect("native roots trust the CI-installed fixture root for HTTP and gRPC");
    assert_expected_topology(&runtime, &context);
    assert!(
        !runtime
            .database_exists(&unique_database("tb_tls_native_absent"))
            .await
            .expect("native-root driver performs a live TLS round trip"),
        "the fresh native-root probe database must not already exist"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tls_only_grpc_endpoint_rejects_plaintext_live() {
    let Some(context) = live_tls_context() else {
        eprintln!("skipping plaintext rejection test: TLS endpoint variables are not set");
        return;
    };
    let Some(expected) = &context.expected else {
        eprintln!("skipping plaintext rejection test: expected live topology is not set");
        return;
    };
    let options = SecureConnectOptions {
        http_port: context.http_port,
        tls_mode: TlsMode::Disabled,
        server_version: Some(
            expected
                .server_version
                .parse()
                .expect("expected server version must parse"),
        ),
    };
    let database = unique_database("tb_plaintext_must_fail");

    let plaintext_round_trip_succeeded = tokio::time::timeout(Duration::from_secs(15), async {
        match TypeDBRuntime::connect_secure(
            &context.address,
            &context.username,
            &context.password,
            options,
        )
        .await
        {
            Ok(runtime) => runtime.database_exists(&database).await.is_ok(),
            Err(_) => false,
        }
    })
    .await
    .expect("plaintext rejection must be bounded");
    assert!(
        !plaintext_round_trip_succeeded,
        "the host endpoint used by the fallback proof must not accept plaintext gRPC"
    );
}

#[test]
fn custom_root_runtime_survives_separate_block_on_calls_live() {
    let Some(context) = live_tls_context() else {
        eprintln!(
            "skipping split-block_on TLS test: set TYPEDB_TLS_ADDRESS, \
             TYPEDB_TLS_HTTP_PORT, and TYPEDB_TLS_ROOT_CA"
        );
        return;
    };
    let database = unique_database("tb_tls_split_runtime");
    let runtime = tokio::runtime::Runtime::new().expect("Tokio runtime");
    let connected = runtime
        .block_on(TypeDBRuntime::connect_secure(
            &context.address,
            &context.username,
            &context.password,
            context.custom_root_options(),
        ))
        .expect("custom-root connection survives the first block_on boundary");
    runtime
        .block_on(connected.create_database(&database))
        .expect("a later block_on can create a database over the retained TLS driver");
    assert!(
        runtime
            .block_on(connected.database_exists(&database))
            .expect("a later block_on can query lifecycle state")
    );
    runtime
        .block_on(connected.delete_database(&database))
        .expect("a later block_on can delete the database");
}
