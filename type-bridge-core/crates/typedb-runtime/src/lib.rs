//! Shared TypeDB driver runtime.
//!
//! This crate owns all direct TypeDB driver interaction so higher-level crates
//! can share version gating, gRPC fallback, database lifecycle, transactions,
//! and JSON conversion without depending on each other.

#[cfg(not(any(feature = "band7", feature = "band8", feature = "band9")))]
compile_error!(
    "type-bridge-typedb-runtime requires at least one band feature; enable `band7`, `band8`, and/or `band9` (all are default)"
);

use std::future::Future;
use std::pin::Pin;
use std::time::Instant;

use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use type_bridge_core_lib::version as core_version;
use type_bridge_core_lib::version::DEFAULT_HTTP_PORT;

#[cfg(feature = "band8")]
use typedb_driver as driver_b8;
#[cfg(feature = "band8")]
use typedb_driver::answer::QueryAnswer as B8QueryAnswer;
#[cfg(feature = "band8")]
use typedb_driver::{
    Addresses, Credentials as B8Credentials, DriverOptions, DriverTlsConfig,
    TransactionType as B8TransactionType, TypeDBDriver as B8Driver,
};

#[cfg(feature = "band7")]
use type_bridge_typedb_driver_b7 as driver_b7;
#[cfg(feature = "band7")]
use type_bridge_typedb_driver_b7::answer::QueryAnswer as B7QueryAnswer;
#[cfg(feature = "band7")]
use type_bridge_typedb_driver_b7::{
    Credentials as B7Credentials, DriverOptions as B7DriverOptions,
    TransactionType as B7TransactionType, TypeDBDriver as B7Driver,
};

#[cfg(feature = "band9")]
use type_bridge_typedb_driver_b9 as driver_b9;
#[cfg(feature = "band9")]
use type_bridge_typedb_driver_b9::answer::QueryAnswer as B9QueryAnswer;
#[cfg(feature = "band9")]
use type_bridge_typedb_driver_b9::{
    Addresses as B9Addresses, Credentials as B9Credentials, DriverOptions as B9DriverOptions,
    DriverTlsConfig as B9DriverTlsConfig, TransactionType as B9TransactionType,
    TypeDBDriver as B9Driver,
};

/// Boxed future returned by async runtime methods.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// How confidently a failed commit can be classified without observing server state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitFailureCertainty {
    /// The server rejected the commit and the transaction did not commit.
    DefinitelyAborted,
    /// The client cannot determine whether the transaction committed.
    Unknown,
}

/// Runtime error type for direct TypeDB driver operations.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// The detected TypeDB server version lies outside the supported window or
    /// its protocol band is not compiled into this runtime.
    #[error("Unsupported version: {0}")]
    UnsupportedVersion(#[from] core_version::VersionError),

    /// Connection to TypeDB failed.
    #[error("Connection error: {0}")]
    Connection(String),

    /// Query execution failed.
    #[error("Query execution error: {0}")]
    QueryExecution(String),

    /// Transaction lifecycle failed.
    #[error("Transaction error: {0}")]
    Transaction(String),

    /// A driver commit failure with an explicit durability certainty.
    #[error("Transaction error: Commit failed: {message}")]
    Commit {
        /// Whether the failed response proves that the commit was aborted.
        certainty: CommitFailureCertainty,
        /// The original driver error text.
        message: String,
    },

    /// A bounded answer ceiling was exceeded while reading the driver stream.
    #[error("Resource limit [{code}]: {message}")]
    ResourceLimit {
        /// Stable resource code.
        code: &'static str,
        /// Human-readable diagnostic.
        message: &'static str,
    },

    /// The higher-level answer decoder rejected one streamed item.
    #[error("Answer consumer rejected a streamed provider item")]
    AnswerConsumer,
}

fn commit_failure(
    certainty: CommitFailureCertainty,
    message: impl Into<String>,
) -> RuntimeError {
    RuntimeError::Commit {
        certainty,
        message: message.into(),
    }
}

#[cfg(feature = "band7")]
fn band7_commit_failure(error: driver_b7::Error) -> RuntimeError {
    let certainty = if matches!(&error, driver_b7::Error::Server(_)) {
        CommitFailureCertainty::DefinitelyAborted
    } else {
        CommitFailureCertainty::Unknown
    };
    commit_failure(certainty, error.to_string())
}

#[cfg(feature = "band8")]
fn band8_commit_failure(error: driver_b8::Error) -> RuntimeError {
    let certainty = if matches!(&error, driver_b8::Error::Server(_)) {
        CommitFailureCertainty::DefinitelyAborted
    } else {
        CommitFailureCertainty::Unknown
    };
    commit_failure(certainty, error.to_string())
}

#[cfg(feature = "band9")]
fn band9_commit_failure(error: driver_b9::Error) -> RuntimeError {
    let certainty = if matches!(&error, driver_b9::Error::Server(_)) {
        CommitFailureCertainty::DefinitelyAborted
    } else {
        CommitFailureCertainty::Unknown
    };
    commit_failure(certainty, error.to_string())
}

#[cfg(test)]
mod commit_failure_tests {
    use super::*;

    #[test]
    fn typed_commit_failure_preserves_the_legacy_display() {
        for certainty in [
            CommitFailureCertainty::DefinitelyAborted,
            CommitFailureCertainty::Unknown,
        ] {
            let error = commit_failure(certainty, "driver response");
            assert_eq!(
                error.to_string(),
                "Transaction error: Commit failed: driver response"
            );
            assert!(matches!(
                error,
                RuntimeError::Commit {
                    certainty: actual,
                    ..
                } if actual == certainty
            ));
        }
    }

    #[cfg(feature = "band7")]
    #[test]
    fn band7_opaque_commit_failure_is_unknown() {
        let error = band7_commit_failure(driver_b7::Error::Other("transport".into()));
        assert!(matches!(
            error,
            RuntimeError::Commit {
                certainty: CommitFailureCertainty::Unknown,
                ..
            }
        ));
    }

    #[cfg(feature = "band8")]
    #[test]
    fn band8_opaque_commit_failure_is_unknown() {
        let error = band8_commit_failure(driver_b8::Error::Other("transport".into()));
        assert!(matches!(
            error,
            RuntimeError::Commit {
                certainty: CommitFailureCertainty::Unknown,
                ..
            }
        ));
    }

    #[cfg(feature = "band9")]
    #[test]
    fn band9_opaque_commit_failure_is_unknown() {
        let error = band9_commit_failure(driver_b9::Error::Other("transport".into()));
        assert!(matches!(
            error,
            RuntimeError::Commit {
                certainty: CommitFailureCertainty::Unknown,
                ..
            }
        ));
    }
}

/// Convenience result alias for runtime operations.
pub type Result<T> = std::result::Result<T, RuntimeError>;

/// Result of a query execution after conversion from TypeDB stream types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueryResult {
    /// No data returned.
    Ok,
    /// Document results from a fetch clause.
    Documents(Vec<serde_json::Value>),
    /// Row results from a match/reduce/concept query.
    Rows(Vec<serde_json::Value>),
}

/// One item read from a TypeDB concept answer stream.
#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeAnswerItem {
    /// Concept row converted to JSON.
    Row(serde_json::Value),
    /// Concept document converted to JSON.
    Document(serde_json::Value),
}

/// Provider answer kind, retained even for an empty stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeAnswerKind {
    /// Statement returned no data stream.
    Ok,
    /// Concept-row stream.
    Rows,
    /// Concept-document stream.
    Documents,
}

/// Whether the answer consumer needs another driver item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeAnswerControl {
    /// Continue reading.
    Continue,
    /// Stop before polling the stream again.
    Stop,
}

/// Cooperative cancellation shared with the ORM answer policy.
#[derive(Debug, Clone)]
pub struct RuntimeAnswerCancellation {
    cancelled: watch::Sender<bool>,
}

impl Default for RuntimeAnswerCancellation {
    fn default() -> Self {
        let (cancelled, _) = watch::channel(false);
        Self { cancelled }
    }
}

impl RuntimeAnswerCancellation {
    /// Construct from a shared wakeable cancellation state.
    pub fn from_shared(cancelled: watch::Sender<bool>) -> Self {
        Self { cancelled }
    }

    /// Request cancellation.
    pub fn cancel(&self) {
        self.cancelled.send_replace(true);
    }

    /// Return whether cancellation was requested.
    pub fn is_cancelled(&self) -> bool {
        *self.cancelled.borrow()
    }

    async fn cancelled(&self) {
        let mut receiver = self.cancelled.subscribe();
        if *receiver.borrow_and_update() {
            return;
        }
        while receiver.changed().await.is_ok() {
            if *receiver.borrow_and_update() {
                return;
            }
        }
    }
}

/// Hard limits checked while polling a real driver answer stream.
#[derive(Debug, Clone)]
pub struct RuntimeAnswerLimits {
    /// Maximum processed rows/documents.
    pub max_items: u64,
    /// Maximum encoded JSON response bytes.
    pub max_bytes: u64,
    /// Optional transaction deadline.
    pub deadline: Option<Instant>,
    /// Cooperative cancellation signal.
    pub cancellation: RuntimeAnswerCancellation,
}

impl RuntimeAnswerLimits {
    fn unbounded() -> Self {
        Self {
            max_items: u64::MAX,
            max_bytes: u64::MAX,
            deadline: None,
            cancellation: RuntimeAnswerCancellation::default(),
        }
    }
}

/// Bounded stream counters returned without materializing all items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeAnswerStats {
    /// Answer kind, including empty streams.
    pub kind: RuntimeAnswerKind,
    /// Items processed.
    pub processed_items: u64,
    /// Encoded response bytes processed.
    pub response_bytes: u64,
    /// Whether the consumer stopped early.
    pub stopped_early: bool,
}

impl RuntimeAnswerStats {
    fn new(kind: RuntimeAnswerKind) -> Self {
        Self {
            kind,
            processed_items: 0,
            response_bytes: 0,
            stopped_early: false,
        }
    }
}

/// Transaction type for TypeDB operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxType {
    /// Read-only transaction.
    Read,
    /// Read-write transaction.
    Write,
    /// Schema transaction.
    Schema,
}

/// A single value bound to a `given` variable, band-agnostic and serializable.
///
/// Temporal variants carry ISO-8601 text and are parsed when lowering onto
/// the band-9 driver — symmetric with [`QueryResult`], which renders temporal
/// values back to strings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GivenValue {
    /// TypeQL `boolean`.
    Boolean(bool),
    /// TypeQL `integer`.
    Integer(i64),
    /// TypeQL `double`.
    Double(f64),
    /// TypeQL `string`.
    String(String),
    /// TypeQL `date`, ISO-8601 (`YYYY-MM-DD`).
    Date(String),
    /// TypeQL `datetime`, ISO-8601 without offset.
    Datetime(String),
    /// TypeQL `datetime-tz`, RFC 3339 with offset.
    DatetimeTz(String),
}

/// Input rows for a `given`-stage query: a variable header plus value rows.
///
/// Every row must have exactly one entry per header variable, in header
/// order. Rows travel through the driver API, never the query string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GivenRowsSpec {
    /// Variable names, without the `$` sigil, in column order.
    pub variables: Vec<String>,
    /// Value rows; each inner vec is one input row in header order.
    pub rows: Vec<Vec<GivenValue>>,
}

/// The compile-pinned `typedb-driver` crate version resolved in `Cargo.lock`.
///
/// This constant records the exact driver version this runtime crate was built
/// against.  It is the factual counterpart to the policy window declared in
/// `type_bridge_core_lib::version`: the window says which versions are allowed;
/// this constant says which version is currently in use.
///
/// When the `typedb-driver` dependency is bumped, update this constant to
/// match the new `Cargo.lock` entry — the `tests::cargo_lock_pin`
/// test will catch any divergence.
pub const PINNED_DRIVER_VERSION: &str = "3.11.5";

/// The compile-pinned version of the vendored band-7 driver fork
/// (`type-bridge-typedb-driver-b7`) this runtime crate was built against.
///
/// Mirrors [`PINNED_DRIVER_VERSION`] for the band-8 upstream driver.  When
/// the band-7 fork is refreshed, update this constant to match the new
/// `Cargo.lock` entry — the `tests::cargo_lock_pin_b7` test will
/// catch any divergence.
pub const PINNED_DRIVER_VERSION_B7: &str = "3.8.1";

/// The compile-pinned version of the vendored band-9 driver fork
/// (`type-bridge-typedb-driver-b9`) this runtime crate was built against.
///
/// Mirrors [`PINNED_DRIVER_VERSION`] for the band-8 upstream driver.  When
/// the band-9 fork is refreshed, update this constant to match the new
/// `Cargo.lock` entry — the `tests::cargo_lock_pin_b9` test will
/// catch any divergence.
pub const PINNED_DRIVER_VERSION_B9: &str = "3.12.0";

/// Protocol bands this build embeds a driver for, derived from the compiled-in
/// band features.  This is the `embedded_bands` argument to
/// [`core_version::check_server_supported`] — the embedded-runtime gate accepts
/// any in-window server whose band is in this set.
///
/// Each element is individually cfg-gated so the set reflects exactly the bands
/// the build can construct; there is no hardcoded band-set literal (master-plan
/// I6).  The default build compiles all three, so the set is `{7, 8, 9}`.
const EMBEDDED_BANDS: &[u8] = &[
    #[cfg(feature = "band7")]
    7,
    #[cfg(feature = "band8")]
    8,
    #[cfg(feature = "band9")]
    9,
];

/// Return the driver versions compiled into this build, keyed by protocol band.
///
/// Each entry is `(band, version_string)` and is individually cfg-gated so only
/// compiled-in bands appear in the slice (master-plan I6 — no hardcoded
/// band-set literal).  The default build embeds all three bands and returns
/// `[(7, "3.8.1"), (8, "3.11.5"), (9, "3.12.0")]`.
///
/// This is the Rust-side source of truth for the Python `embedded_driver_versions()`
/// binding — `crates/python/src/version.rs` wraps this function and exposes it
/// as a Python dict `{int: str}`.
pub fn embedded_driver_versions() -> &'static [(u8, &'static str)] {
    &[
        #[cfg(feature = "band7")]
        (7, PINNED_DRIVER_VERSION_B7),
        #[cfg(feature = "band8")]
        (8, PINNED_DRIVER_VERSION),
        #[cfg(feature = "band9")]
        (9, PINNED_DRIVER_VERSION_B9),
    ]
}

/// Connection-time options for the version-gated TypeDB runtime.
///
/// Callers that accept the defaults can pass `ConnectOptions::default()`.
#[derive(Debug, Clone, Copy)]
pub struct ConnectOptions {
    /// Port of the TypeDB HTTP API used for the connect-time version probe.
    pub http_port: u16,
    /// Whether to use TLS for the HTTP version probe.
    pub tls: bool,
    /// Exact server version supplied by the caller.
    ///
    /// When set, the connect gate validates this version and skips the HTTP
    /// `/v1/version` probe. This is the supported path for gRPC-only TypeDB
    /// deployments where the HTTP API is disabled or unreachable.
    pub server_version: Option<core_version::Version>,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            http_port: DEFAULT_HTTP_PORT,
            tls: false,
            server_version: None,
        }
    }
}

/// Band-tagged driver handle.  Crate-private; no driver type escapes the crate.
pub(crate) enum DriverHandle {
    #[cfg(feature = "band7")]
    B7(B7Driver),
    #[cfg(feature = "band8")]
    B8(B8Driver),
    #[cfg(feature = "band9")]
    B9(B9Driver),
}

/// Real TypeDB backend wrapping a band-tagged [`DriverHandle`].
pub struct TypeDBRuntime {
    driver: DriverHandle,
    /// The server version the connect gate detected, when it could.
    ///
    /// `Some` on the exact-version, HTTP-probe, and band-8 gRPC-fallback
    /// paths; `None` only on the band-7 gRPC fallback, where the server
    /// does not report its version.
    server_version: Option<core_version::Version>,
}

/// Version-gated [`DriverHandle`] constructor with an injectable probe.
///
/// When `options.server_version` is set, the supplied exact version is validated
/// and the `probe` closure is not called. Otherwise, the `probe` closure is
/// called with `(address, http_port, tls)` and must return the server version or
/// a [`core_version::VersionError`]. If that HTTP probe fails, the constructor
/// falls back to gRPC driver negotiation: band 8 first, then band 7. Production
/// code passes [`core_version::server_version`]; tests inject a recording
/// closure to exercise the gate without a live server.
pub(crate) async fn gated_driver_with_probe<F>(
    address: &str,
    username: &str,
    password: &str,
    options: ConnectOptions,
    probe: F,
) -> Result<(DriverHandle, Option<core_version::Version>)>
where
    F: FnOnce(
            &str,
            u16,
            bool,
        ) -> std::result::Result<core_version::Version, core_version::VersionError>
        + Send
        + 'static,
{
    if let Some(server_version) = options.server_version {
        let driver = driver_for_server_version(address, username, password, server_version).await?;
        return Ok((driver, Some(server_version)));
    }

    // Probe the server version over HTTP (blocking I/O; offload to a dedicated
    // thread so we don't block the async executor).
    let address_owned = address.to_string();
    let http_port = options.http_port;
    let tls = options.tls;
    match tokio::task::spawn_blocking(move || probe(&address_owned, http_port, tls))
        .await
        .map_err(|e| RuntimeError::Connection(format!("Version probe task panicked: {e}")))?
    {
        Ok(server_version) => {
            let driver =
                driver_for_server_version(address, username, password, server_version).await?;
            Ok((driver, Some(server_version)))
        }
        Err(http_error) => grpc_fallback_driver(address, username, password, http_error).await,
    }
}

fn validate_server_band(server_version: &core_version::Version) -> Result<u8> {
    // Embedded-runtime gate: accept the server when it is in-window and any
    // band it accepts is one this build embedded.  Unlike the installed-driver
    // gate (check_supported), the embedded runtime carries every compiled-in
    // band. A band-7 server is therefore served, while a confirmed 3.12
    // server normally negotiates native band 9; its band-8 acceptance remains
    // available to discovery/fallback paths. After this passes, negotiation
    // over EMBEDDED_BANDS is guaranteed to yield a band.
    core_version::check_server_supported(server_version, EMBEDDED_BANDS)
        .map_err(RuntimeError::UnsupportedVersion)?;

    Ok(
        core_version::negotiate_server_band(server_version, EMBEDDED_BANDS)
            .expect("check_server_supported accepted a server with no negotiable band"),
    )
}

async fn driver_for_server_version(
    address: &str,
    username: &str,
    password: &str,
    server_version: core_version::Version,
) -> Result<DriverHandle> {
    let band = validate_server_band(&server_version)?;

    tracing::debug!(
        address,
        band = ?band,
        server_version = %server_version,
        "Embedded version gate passed"
    );

    #[cfg(feature = "band7")]
    if band == 7 {
        return connect_band7_driver(address, username, password)
            .await
            .map(DriverHandle::B7);
    }

    #[cfg(feature = "band8")]
    if band == 8 {
        return connect_band8_driver(address, username, password)
            .await
            .map(DriverHandle::B8);
    }

    #[cfg(feature = "band9")]
    if band == 9 {
        return connect_band9_driver(address, username, password)
            .await
            .map(DriverHandle::B9);
    }

    // Unreachable by invariant: the gate already rejected any server whose
    // accepted-band set does not intersect EMBEDDED_BANDS, and negotiation
    // only yields embedded bands, each of which returned above.  The arm
    // exists only to keep the tail total; it is never entered.
    Err(RuntimeError::Connection(format!(
        "No compiled driver band supports the negotiated server band ({band})"
    )))
}

#[cfg(feature = "band7")]
async fn connect_band7_driver(address: &str, username: &str, password: &str) -> Result<B7Driver> {
    // Band-7 driver takes a raw address string and a two-argument
    // DriverOptions::new(tls_enabled, ca_path).  TLS is disabled.
    let opts = B7DriverOptions::new(false, None)
        .map_err(|e| RuntimeError::Connection(format!("Band-7 driver options error: {e}")))?;
    B7Driver::new(address, B7Credentials::new(username, password), opts)
        .await
        .map_err(|e| RuntimeError::Connection(format!("Failed to connect to {address}: {e}")))
}

#[cfg(feature = "band8")]
async fn connect_band8_driver(address: &str, username: &str, password: &str) -> Result<B8Driver> {
    // TLS is not plumbed in this crate today; the band-8 driver's
    // `DriverTlsConfig::default()` would ENABLE it, so disable explicitly.
    let addresses = Addresses::try_from_address_str(address)
        .map_err(|e| RuntimeError::Connection(format!("Invalid TypeDB address {address}: {e}")))?;
    B8Driver::new(
        addresses,
        B8Credentials::new(username, password),
        DriverOptions::new(DriverTlsConfig::disabled()),
    )
    .await
    .map_err(|e| RuntimeError::Connection(format!("Failed to connect to {address}: {e}")))
}

#[cfg(feature = "band9")]
async fn connect_band9_driver(address: &str, username: &str, password: &str) -> Result<B9Driver> {
    // Same construction shape as band 8; TLS disabled explicitly for the
    // same reason (`DriverTlsConfig::default()` would enable it).
    let addresses = B9Addresses::try_from_address_str(address)
        .map_err(|e| RuntimeError::Connection(format!("Invalid TypeDB address {address}: {e}")))?;
    B9Driver::new(
        addresses,
        B9Credentials::new(username, password),
        B9DriverOptions::new(B9DriverTlsConfig::disabled()),
    )
    .await
    .map_err(|e| RuntimeError::Connection(format!("Failed to connect to {address}: {e}")))
}

async fn grpc_fallback_driver(
    address: &str,
    username: &str,
    password: &str,
    http_error: core_version::VersionError,
) -> Result<(DriverHandle, Option<core_version::Version>)> {
    #[cfg(not(any(feature = "band7", feature = "band8")))]
    let _ = (address, username, password);
    let mut failures = vec![format!("HTTP version probe failed: {http_error}")];

    // Band 9 is deliberately NOT probed blindly: a band-9 connection attempt
    // against a 3.11 server crashes the server outright (measured live on
    // 3.11.5).  The fallback therefore discovers the server through band 8 —
    // accepted by every band-{8,9} server — and only upgrades to the native
    // band-9 protocol once the reported version proves the server accepts it.
    #[cfg(feature = "band8")]
    {
        match connect_band8_driver(address, username, password).await {
            Ok(driver) => match classify_band8_grpc_version(
                address,
                driver
                    .server_version()
                    .await
                    .map(|reported| reported.version().to_owned())
                    .map_err(|error| error.to_string()),
            )? {
                Band8GrpcVersion::Validated(server_version) => {
                    // Prefer the server's native band when this build embeds
                    // it. The authoritative band-8 version round trip makes a
                    // band-9 attempt safe; probing band 9 before this point can
                    // crash a 3.11 server.
                    #[cfg(feature = "band9")]
                    if core_version::negotiate_server_band(&server_version, EMBEDDED_BANDS)
                        == Some(9)
                    {
                        match connect_band9_driver(address, username, password).await {
                            Ok(b9_driver) => {
                                tracing::debug!(
                                    address,
                                    server_version = %server_version,
                                    "Connected through gRPC band-8 fallback, upgraded to native band 9"
                                );
                                return Ok((DriverHandle::B9(b9_driver), Some(server_version)));
                            }
                            Err(error) => {
                                tracing::warn!(
                                    address,
                                    server_version = %server_version,
                                    %error,
                                    "Band-9 upgrade failed after band-8 fallback; staying on band 8"
                                );
                            }
                        }
                    }
                    tracing::debug!(
                        address,
                        server_version = %server_version,
                        "Connected through gRPC band-8 fallback after HTTP version probe failed"
                    );
                    return Ok((DriverHandle::B8(driver), Some(server_version)));
                }
                Band8GrpcVersion::RetryableFailure(failure) => failures.push(failure),
            },
            Err(error) => failures.push(format!("band-8 gRPC attempt failed: {error}")),
        }
    }

    #[cfg(not(feature = "band8"))]
    failures.push("band-8 gRPC attempt skipped: band8 feature is not compiled in".to_string());

    #[cfg(feature = "band7")]
    {
        match connect_band7_driver(address, username, password).await {
            Ok(driver) => {
                tracing::warn!(
                    address,
                    "Connected through gRPC band-7 fallback after HTTP version probe failed; \
                     exact server version is unavailable on band 7, so use server_version=... \
                     for strict gRPC-only version validation"
                );
                return Ok((DriverHandle::B7(driver), None));
            }
            Err(error) => failures.push(format!("band-7 gRPC attempt failed: {error}")),
        }
    }

    #[cfg(not(feature = "band7"))]
    failures.push("band-7 gRPC attempt skipped: band7 feature is not compiled in".to_string());

    Err(RuntimeError::UnsupportedVersion(
        core_version::VersionError::Probe(format!(
            "HTTP version probe and gRPC fallback both failed: {}",
            failures.join("; ")
        )),
    ))
}

#[cfg(feature = "band8")]
#[derive(Debug)]
enum Band8GrpcVersion {
    Validated(core_version::Version),
    RetryableFailure(String),
}

#[cfg(feature = "band8")]
fn classify_band8_grpc_version(
    address: &str,
    reported: std::result::Result<String, String>,
) -> Result<Band8GrpcVersion> {
    let reported = match reported {
        Ok(reported) => reported,
        Err(error) => {
            // TypeDB's band-8 Driver::new is lazy. Against a reachable band-7
            // server it can return Ok before server_version performs the first
            // protocol round trip. Treat only that inability to validate the
            // candidate connection as retryable so band 7 still gets a chance.
            return Ok(Band8GrpcVersion::RetryableFailure(format!(
                "band-8 gRPC version validation failed after connect to {address}: {error}"
            )));
        }
    };

    // Once the server has authoritatively reported a version, parse and gate
    // failures are terminal. Falling through to band 7 here could silently
    // accept an unsupported server and discard the exact rejection evidence.
    let server_version = reported
        .parse::<core_version::Version>()
        .map_err(RuntimeError::UnsupportedVersion)?;
    validate_server_band(&server_version)?;
    if !core_version::server_accepted_bands(&server_version).contains(&8) {
        return Err(RuntimeError::UnsupportedVersion(
            core_version::VersionError::Probe(format!(
                "band-8 gRPC connection reported server version {server_version}, \
                 which does not accept band 8"
            )),
        ));
    }

    Ok(Band8GrpcVersion::Validated(server_version))
}

/// Version-gated [`DriverHandle`] constructor.
///
/// Thin wrapper over [`gated_driver_with_probe`] that passes the real
/// [`core_version::server_version`] HTTP probe.
async fn gated_driver(
    address: &str,
    username: &str,
    password: &str,
    options: ConnectOptions,
) -> Result<(DriverHandle, Option<core_version::Version>)> {
    gated_driver_with_probe(
        address,
        username,
        password,
        options,
        core_version::server_version,
    )
    .await
}

impl TypeDBRuntime {
    /// Connect to a TypeDB server.
    ///
    /// Validates the supplied server version, or probes via HTTP before opening
    /// any gRPC connection when no version is supplied.
    /// Returns [`RuntimeError::UnsupportedVersion`] when the server is outside the
    /// supported window or on a different protocol band than the driver.
    pub async fn connect(
        address: &str,
        username: &str,
        password: &str,
        options: ConnectOptions,
    ) -> Result<Self> {
        let (driver, server_version) = gated_driver(address, username, password, options).await?;
        tracing::info!(address, "Connected to TypeDB");
        Ok(Self {
            driver,
            server_version,
        })
    }

    /// The server version detected by the connect gate, when known.
    ///
    /// `None` only when the connection was established through the band-7
    /// gRPC fallback, which cannot report the server version.
    pub fn server_version(&self) -> Option<core_version::Version> {
        self.server_version
    }

    /// Return whether the connection actually negotiated a driver that can
    /// transport `given` input rows.
    ///
    /// This is deliberately separate from the reported server version. A
    /// 3.12 server may accept the band-8 discovery connection when a later
    /// band-9 upgrade fails; advertising `given` in that state would select an
    /// execution path the active driver cannot carry.
    pub fn supports_given_rows(&self) -> bool {
        match &self.driver {
            #[cfg(feature = "band7")]
            DriverHandle::B7(_) => false,
            #[cfg(feature = "band8")]
            DriverHandle::B8(_) => false,
            #[cfg(feature = "band9")]
            DriverHandle::B9(_) => true,
        }
    }
}

/// Ensure a TypeDB database exists, creating it if absent.
///
/// Validates the supplied server version, or probes via HTTP before opening any
/// gRPC connection when no version is supplied.
/// Returns [`RuntimeError::UnsupportedVersion`] when the server is outside the
/// supported window or on a different protocol band than the driver.
///
/// On all other TypeDB failures (including unreachable server) the error is
/// returned so callers can treat it as a hard failure rather than silently
/// skipping.
pub async fn ensure_database_exists(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
    options: ConnectOptions,
) -> Result<()> {
    let (driver, _server_version) = gated_driver(address, username, password, options).await?;

    match driver {
        #[cfg(feature = "band7")]
        DriverHandle::B7(d) => {
            let databases = d.databases();
            let exists = databases
                .contains(database)
                .await
                .map_err(|e| RuntimeError::Connection(format!("Database lookup failed: {e}")))?;
            if !exists {
                databases.create(database).await.map_err(|e| {
                    RuntimeError::Connection(format!("Database create failed: {e}"))
                })?;
            }
        }
        #[cfg(feature = "band8")]
        DriverHandle::B8(d) => {
            let databases = d.databases();
            let exists = databases
                .contains(database)
                .await
                .map_err(|e| RuntimeError::Connection(format!("Database lookup failed: {e}")))?;
            if !exists {
                databases.create(database).await.map_err(|e| {
                    RuntimeError::Connection(format!("Database create failed: {e}"))
                })?;
            }
        }
        #[cfg(feature = "band9")]
        DriverHandle::B9(d) => {
            let databases = d.databases();
            let exists = databases
                .contains(database)
                .await
                .map_err(|e| RuntimeError::Connection(format!("Database lookup failed: {e}")))?;
            if !exists {
                databases.create(database).await.map_err(|e| {
                    RuntimeError::Connection(format!("Database create failed: {e}"))
                })?;
            }
        }
    }

    Ok(())
}

impl TypeDBRuntime {
    /// Open a TypeDB transaction against `database`.
    pub fn open_transaction(
        &self,
        database: &str,
        tx_type: TxType,
    ) -> BoxFuture<'_, Result<RuntimeTransaction>> {
        let db = database.to_string();
        Box::pin(async move {
            match &self.driver {
                #[cfg(feature = "band7")]
                DriverHandle::B7(d) => {
                    let typedb_tx_type = match tx_type {
                        TxType::Read => B7TransactionType::Read,
                        TxType::Write => B7TransactionType::Write,
                        TxType::Schema => B7TransactionType::Schema,
                    };
                    let transaction = d.transaction(&db, typedb_tx_type).await.map_err(|e| {
                        RuntimeError::Transaction(format!("Failed to open transaction: {e}"))
                    })?;
                    Ok(RuntimeTransaction {
                        inner: RuntimeTransactionInner::B7(Some(transaction)),
                    })
                }
                #[cfg(feature = "band8")]
                DriverHandle::B8(d) => {
                    let typedb_tx_type = match tx_type {
                        TxType::Read => B8TransactionType::Read,
                        TxType::Write => B8TransactionType::Write,
                        TxType::Schema => B8TransactionType::Schema,
                    };
                    let transaction = d.transaction(&db, typedb_tx_type).await.map_err(|e| {
                        RuntimeError::Transaction(format!("Failed to open transaction: {e}"))
                    })?;
                    Ok(RuntimeTransaction {
                        inner: RuntimeTransactionInner::B8(Some(transaction)),
                    })
                }
                #[cfg(feature = "band9")]
                DriverHandle::B9(d) => {
                    let typedb_tx_type = match tx_type {
                        TxType::Read => B9TransactionType::Read,
                        TxType::Write => B9TransactionType::Write,
                        TxType::Schema => B9TransactionType::Schema,
                    };
                    let transaction = d.transaction(&db, typedb_tx_type).await.map_err(|e| {
                        RuntimeError::Transaction(format!("Failed to open transaction: {e}"))
                    })?;
                    Ok(RuntimeTransaction {
                        inner: RuntimeTransactionInner::B9(Some(transaction)),
                    })
                }
            }
        })
    }

    /// Check whether the underlying driver is open.
    pub fn is_open(&self) -> bool {
        match &self.driver {
            #[cfg(feature = "band7")]
            DriverHandle::B7(d) => d.is_open(),
            #[cfg(feature = "band8")]
            DriverHandle::B8(d) => d.is_open(),
            #[cfg(feature = "band9")]
            DriverHandle::B9(d) => d.is_open(),
        }
    }

    /// Check whether a database exists.
    pub fn database_exists(&self, database: &str) -> BoxFuture<'_, Result<bool>> {
        let database = database.to_string();
        Box::pin(async move {
            match &self.driver {
                #[cfg(feature = "band7")]
                DriverHandle::B7(d) => {
                    d.databases().contains(database).await.map_err(|e| {
                        RuntimeError::Connection(format!("Database lookup failed: {e}"))
                    })
                }
                #[cfg(feature = "band8")]
                DriverHandle::B8(d) => {
                    d.databases().contains(database).await.map_err(|e| {
                        RuntimeError::Connection(format!("Database lookup failed: {e}"))
                    })
                }
                #[cfg(feature = "band9")]
                DriverHandle::B9(d) => {
                    d.databases().contains(database).await.map_err(|e| {
                        RuntimeError::Connection(format!("Database lookup failed: {e}"))
                    })
                }
            }
        })
    }

    /// Create a database.
    pub fn create_database(&self, database: &str) -> BoxFuture<'_, Result<()>> {
        let database = database.to_string();
        Box::pin(async move {
            match &self.driver {
                #[cfg(feature = "band7")]
                DriverHandle::B7(d) => {
                    d.databases().create(database).await.map_err(|e| {
                        RuntimeError::Connection(format!("Database create failed: {e}"))
                    })
                }
                #[cfg(feature = "band8")]
                DriverHandle::B8(d) => {
                    d.databases().create(database).await.map_err(|e| {
                        RuntimeError::Connection(format!("Database create failed: {e}"))
                    })
                }
                #[cfg(feature = "band9")]
                DriverHandle::B9(d) => {
                    d.databases().create(database).await.map_err(|e| {
                        RuntimeError::Connection(format!("Database create failed: {e}"))
                    })
                }
            }
        })
    }

    /// Delete a database.
    pub fn delete_database(&self, database: &str) -> BoxFuture<'_, Result<()>> {
        let database = database.to_string();
        Box::pin(async move {
            match &self.driver {
                #[cfg(feature = "band7")]
                DriverHandle::B7(d) => {
                    let db = d.databases().get(&database).await.map_err(|e| {
                        RuntimeError::Connection(format!("Database lookup failed: {e}"))
                    })?;
                    db.delete().await.map_err(|e| {
                        RuntimeError::Connection(format!("Database delete failed: {e}"))
                    })
                }
                #[cfg(feature = "band8")]
                DriverHandle::B8(d) => {
                    let db = d.databases().get(database).await.map_err(|e| {
                        RuntimeError::Connection(format!("Database lookup failed: {e}"))
                    })?;
                    db.delete().await.map_err(|e| {
                        RuntimeError::Connection(format!("Database delete failed: {e}"))
                    })
                }
                #[cfg(feature = "band9")]
                DriverHandle::B9(d) => {
                    let db = d.databases().get(database).await.map_err(|e| {
                        RuntimeError::Connection(format!("Database lookup failed: {e}"))
                    })?;
                    db.delete().await.map_err(|e| {
                        RuntimeError::Connection(format!("Database delete failed: {e}"))
                    })
                }
            }
        })
    }

    /// Export a database schema as TypeQL text.
    pub fn schema_text(&self, database: &str) -> BoxFuture<'_, Result<String>> {
        let database = database.to_string();
        Box::pin(async move {
            match &self.driver {
                #[cfg(feature = "band7")]
                DriverHandle::B7(d) => {
                    let db = d.databases().get(&database).await.map_err(|e| {
                        RuntimeError::Connection(format!("Database lookup failed: {e}"))
                    })?;
                    db.schema()
                        .await
                        .map_err(|e| RuntimeError::Connection(format!("Schema export failed: {e}")))
                }
                #[cfg(feature = "band8")]
                DriverHandle::B8(d) => {
                    let db = d.databases().get(&database).await.map_err(|e| {
                        RuntimeError::Connection(format!("Database lookup failed: {e}"))
                    })?;
                    db.schema()
                        .await
                        .map_err(|e| RuntimeError::Connection(format!("Schema export failed: {e}")))
                }
                #[cfg(feature = "band9")]
                DriverHandle::B9(d) => {
                    let db = d.databases().get(&database).await.map_err(|e| {
                        RuntimeError::Connection(format!("Database lookup failed: {e}"))
                    })?;
                    db.schema()
                        .await
                        .map_err(|e| RuntimeError::Connection(format!("Schema export failed: {e}")))
                }
            }
        })
    }
}

/// Band-tagged transaction inner state.
///
/// The `Option` wrapper lets commit/close/rollback take ownership by value
/// (`.take()`) while the outer `&mut RuntimeTransaction` satisfies the trait's
/// `&mut self`.  A `None` after `.take()` marks the transaction as consumed.
enum RuntimeTransactionInner {
    #[cfg(feature = "band7")]
    B7(Option<type_bridge_typedb_driver_b7::Transaction>),
    #[cfg(feature = "band8")]
    B8(Option<typedb_driver::Transaction>),
    #[cfg(feature = "band9")]
    B9(Option<type_bridge_typedb_driver_b9::Transaction>),
}

/// Open TypeDB transaction owned by the shared runtime.
pub struct RuntimeTransaction {
    inner: RuntimeTransactionInner,
}

fn runtime_cancelled_error() -> RuntimeError {
    RuntimeError::ResourceLimit {
        code: "provider_cancelled",
        message: "provider answer processing was cancelled",
    }
}

fn runtime_deadline_error() -> RuntimeError {
    RuntimeError::ResourceLimit {
        code: "transaction_deadline_exceeded",
        message: "provider transaction deadline expired",
    }
}

fn runtime_check(limits: &RuntimeAnswerLimits) -> Result<()> {
    if limits.cancellation.is_cancelled() {
        return Err(runtime_cancelled_error());
    }
    if limits.deadline.is_some_and(|deadline| {
        tokio::time::Instant::now() >= tokio::time::Instant::from_std(deadline)
    }) {
        return Err(runtime_deadline_error());
    }
    Ok(())
}

async fn runtime_await<T>(
    future: impl Future<Output = Result<T>>,
    limits: &RuntimeAnswerLimits,
) -> Result<T> {
    runtime_check(limits)?;
    tokio::pin!(future);
    let cancellation = limits.cancellation.cancelled();
    tokio::pin!(cancellation);

    if let Some(deadline) = limits.deadline {
        let deadline = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline));
        tokio::pin!(deadline);
        tokio::select! {
            biased;
            result = &mut future => result,
            () = &mut cancellation => Err(runtime_cancelled_error()),
            () = &mut deadline => Err(runtime_deadline_error()),
        }
    } else {
        tokio::select! {
            biased;
            result = &mut future => result,
            () = &mut cancellation => Err(runtime_cancelled_error()),
        }
    }
}

fn runtime_accept(
    limits: &RuntimeAnswerLimits,
    stats: &mut RuntimeAnswerStats,
    item: RuntimeAnswerItem,
    consumer: &mut (dyn FnMut(RuntimeAnswerItem) -> Result<RuntimeAnswerControl> + Send),
) -> Result<RuntimeAnswerControl> {
    runtime_check(limits)?;
    let next_items = stats
        .processed_items
        .checked_add(1)
        .ok_or(RuntimeError::ResourceLimit {
            code: "processed_item_counter_overflow",
            message: "processed provider item counter overflowed",
        })?;
    if next_items > limits.max_items {
        return Err(RuntimeError::ResourceLimit {
            code: "processed_item_limit",
            message: "provider answer exceeded the processed-item ceiling",
        });
    }
    let value = match &item {
        RuntimeAnswerItem::Row(value) | RuntimeAnswerItem::Document(value) => value,
    };
    let encoded = u64::try_from(
        serde_json::to_vec(value)
            .map_err(|error| RuntimeError::QueryExecution(format!("Answer encode: {error}")))?
            .len(),
    )
    .map_err(|_| RuntimeError::ResourceLimit {
        code: "answer_byte_counter_overflow",
        message: "encoded provider answer length exceeds the counter range",
    })?;
    let next_bytes =
        stats
            .response_bytes
            .checked_add(encoded)
            .ok_or(RuntimeError::ResourceLimit {
                code: "answer_byte_counter_overflow",
                message: "provider answer byte counter overflowed",
            })?;
    if next_bytes > limits.max_bytes {
        return Err(RuntimeError::ResourceLimit {
            code: "response_byte_limit",
            message: "provider answer exceeded the response-byte ceiling",
        });
    }
    stats.processed_items = next_items;
    stats.response_bytes = next_bytes;
    let control = consumer(item);
    runtime_check(limits)?;
    let control = control?;
    if control == RuntimeAnswerControl::Stop {
        stats.stopped_early = true;
    }
    Ok(control)
}

async fn runtime_consume_stream<S>(
    mut stream: S,
    kind: RuntimeAnswerKind,
    limits: &RuntimeAnswerLimits,
    consumer: &mut (dyn FnMut(RuntimeAnswerItem) -> Result<RuntimeAnswerControl> + Send),
) -> Result<RuntimeAnswerStats>
where
    S: futures::TryStream<Ok = RuntimeAnswerItem, Error = RuntimeError> + Send + Unpin,
{
    let mut stats = RuntimeAnswerStats::new(kind);
    while let Some(item) = runtime_await(async { stream.try_next().await }, limits).await? {
        if runtime_accept(limits, &mut stats, item, consumer)? == RuntimeAnswerControl::Stop {
            break;
        }
    }
    Ok(stats)
}

fn materialize_runtime_answer(
    stats: RuntimeAnswerStats,
    items: Vec<RuntimeAnswerItem>,
) -> QueryResult {
    match stats.kind {
        RuntimeAnswerKind::Ok => QueryResult::Ok,
        RuntimeAnswerKind::Rows => QueryResult::Rows(
            items
                .into_iter()
                .filter_map(|item| match item {
                    RuntimeAnswerItem::Row(value) => Some(value),
                    RuntimeAnswerItem::Document(_) => None,
                })
                .collect(),
        ),
        RuntimeAnswerKind::Documents => QueryResult::Documents(
            items
                .into_iter()
                .filter_map(|item| match item {
                    RuntimeAnswerItem::Document(value) => Some(value),
                    RuntimeAnswerItem::Row(_) => None,
                })
                .collect(),
        ),
    }
}

impl RuntimeTransaction {
    /// Return whether this transaction's negotiated driver can transport
    /// `given` input rows.
    pub fn supports_given_rows(&self) -> bool {
        match &self.inner {
            #[cfg(feature = "band7")]
            RuntimeTransactionInner::B7(_) => false,
            #[cfg(feature = "band8")]
            RuntimeTransactionInner::B8(_) => false,
            #[cfg(feature = "band9")]
            RuntimeTransactionInner::B9(_) => true,
        }
    }

    /// Execute TypeQL within this transaction, materializing for legacy callers.
    ///
    /// New match executors use [`Self::query_bounded`] directly.
    pub fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult>> {
        let typeql = typeql.to_owned();
        Box::pin(async move {
            let mut items = Vec::new();
            let mut collect = |item| {
                items.push(item);
                Ok(RuntimeAnswerControl::Continue)
            };
            let stats = self
                .query_bounded(&typeql, RuntimeAnswerLimits::unbounded(), &mut collect)
                .await?;
            Ok(materialize_runtime_answer(stats, items))
        })
    }

    /// Execute TypeQL and enforce bounds while polling the driver stream.
    pub fn query_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        limits: RuntimeAnswerLimits,
        consumer: &'a mut (dyn FnMut(RuntimeAnswerItem) -> Result<RuntimeAnswerControl> + Send),
    ) -> BoxFuture<'a, Result<RuntimeAnswerStats>> {
        let tql = typeql.to_string();
        Box::pin(async move {
            runtime_check(&limits)?;
            match &self.inner {
                #[cfg(feature = "band7")]
                RuntimeTransactionInner::B7(opt) => {
                    let tx = opt.as_ref().ok_or_else(|| {
                        RuntimeError::Transaction("Transaction already consumed".into())
                    })?;
                    let answer = runtime_await(
                        async {
                            tx.query(&tql)
                                .await
                                .map_err(|e| RuntimeError::QueryExecution(format!("{e}")))
                        },
                        &limits,
                    )
                    .await?;
                    match answer {
                        B7QueryAnswer::Ok(_) => Ok(RuntimeAnswerStats::new(RuntimeAnswerKind::Ok)),
                        B7QueryAnswer::ConceptRowStream(_, mut stream) => {
                            let mut stats = RuntimeAnswerStats::new(RuntimeAnswerKind::Rows);
                            while let Some(row) = runtime_await(
                                async {
                                    stream.try_next().await.map_err(|error| {
                                        RuntimeError::QueryExecution(format!("Row stream: {error}"))
                                    })
                                },
                                &limits,
                            )
                            .await?
                            {
                                let mut object = serde_json::Map::new();
                                for (index, column) in row.get_column_names().iter().enumerate() {
                                    let value = row
                                        .row
                                        .get(index)
                                        .and_then(|concept| concept.as_ref())
                                        .map(concept_to_json_b7)
                                        .unwrap_or(serde_json::Value::Null);
                                    object.insert(column.clone(), value);
                                }
                                if runtime_accept(
                                    &limits,
                                    &mut stats,
                                    RuntimeAnswerItem::Row(serde_json::Value::Object(object)),
                                    consumer,
                                )? == RuntimeAnswerControl::Stop
                                {
                                    break;
                                }
                            }
                            Ok(stats)
                        }
                        B7QueryAnswer::ConceptDocumentStream(_, mut stream) => {
                            let mut stats = RuntimeAnswerStats::new(RuntimeAnswerKind::Documents);
                            while let Some(document) = runtime_await(
                                async {
                                    stream.try_next().await.map_err(|error| {
                                        RuntimeError::QueryExecution(format!(
                                            "Document stream: {error}"
                                        ))
                                    })
                                },
                                &limits,
                            )
                            .await?
                            {
                                let value = document_to_json_b7(&document);
                                if runtime_accept(
                                    &limits,
                                    &mut stats,
                                    RuntimeAnswerItem::Document(value),
                                    consumer,
                                )? == RuntimeAnswerControl::Stop
                                {
                                    break;
                                }
                            }
                            Ok(stats)
                        }
                    }
                }
                #[cfg(feature = "band8")]
                RuntimeTransactionInner::B8(opt) => {
                    let tx = opt.as_ref().ok_or_else(|| {
                        RuntimeError::Transaction("Transaction already consumed".into())
                    })?;
                    let answer = runtime_await(
                        async {
                            tx.query(&tql)
                                .await
                                .map_err(|e| RuntimeError::QueryExecution(format!("{e}")))
                        },
                        &limits,
                    )
                    .await?;
                    match answer {
                        B8QueryAnswer::Ok(_) => Ok(RuntimeAnswerStats::new(RuntimeAnswerKind::Ok)),
                        B8QueryAnswer::ConceptRowStream(_, mut stream) => {
                            let mut stats = RuntimeAnswerStats::new(RuntimeAnswerKind::Rows);
                            while let Some(row) = runtime_await(
                                async {
                                    stream.try_next().await.map_err(|error| {
                                        RuntimeError::QueryExecution(format!("Row stream: {error}"))
                                    })
                                },
                                &limits,
                            )
                            .await?
                            {
                                let mut object = serde_json::Map::new();
                                for (index, column) in row.get_column_names().iter().enumerate() {
                                    let value = row
                                        .row
                                        .get(index)
                                        .and_then(|concept| concept.as_ref())
                                        .map(concept_to_json_b8)
                                        .unwrap_or(serde_json::Value::Null);
                                    object.insert(column.clone(), value);
                                }
                                if runtime_accept(
                                    &limits,
                                    &mut stats,
                                    RuntimeAnswerItem::Row(serde_json::Value::Object(object)),
                                    consumer,
                                )? == RuntimeAnswerControl::Stop
                                {
                                    break;
                                }
                            }
                            Ok(stats)
                        }
                        B8QueryAnswer::ConceptDocumentStream(_, mut stream) => {
                            let mut stats = RuntimeAnswerStats::new(RuntimeAnswerKind::Documents);
                            while let Some(document) = runtime_await(
                                async {
                                    stream.try_next().await.map_err(|error| {
                                        RuntimeError::QueryExecution(format!(
                                            "Document stream: {error}"
                                        ))
                                    })
                                },
                                &limits,
                            )
                            .await?
                            {
                                let value = document_to_json_b8(&document);
                                if runtime_accept(
                                    &limits,
                                    &mut stats,
                                    RuntimeAnswerItem::Document(value),
                                    consumer,
                                )? == RuntimeAnswerControl::Stop
                                {
                                    break;
                                }
                            }
                            Ok(stats)
                        }
                    }
                }
                #[cfg(feature = "band9")]
                RuntimeTransactionInner::B9(opt) => {
                    let tx = opt.as_ref().ok_or_else(|| {
                        RuntimeError::Transaction("Transaction already consumed".into())
                    })?;
                    let answer = runtime_await(
                        async {
                            tx.query(&tql)
                                .await
                                .map_err(|e| RuntimeError::QueryExecution(format!("{e}")))
                        },
                        &limits,
                    )
                    .await?;
                    consume_answer_b9(answer, &limits, consumer).await
                }
            }
        })
    }

    /// Execute TypeQL with `given`-stage input rows within this transaction.
    ///
    /// Rows travel through the driver API, not the query string, so this
    /// path is only available on the band-9 (TypeDB 3.12+) driver. On a
    /// band-7 or band-8 connection this returns an actionable error —
    /// callers that can fall back to per-row queries should consult the
    /// detected server version before choosing this path.
    pub fn query_with_rows(
        &mut self,
        typeql: &str,
        rows: GivenRowsSpec,
    ) -> BoxFuture<'_, Result<QueryResult>> {
        let typeql = typeql.to_owned();
        Box::pin(async move {
            let mut items = Vec::new();
            let mut collect = |item| {
                items.push(item);
                Ok(RuntimeAnswerControl::Continue)
            };
            let stats = self
                .query_with_rows_bounded(
                    &typeql,
                    rows,
                    RuntimeAnswerLimits::unbounded(),
                    &mut collect,
                )
                .await?;
            Ok(materialize_runtime_answer(stats, items))
        })
    }

    /// Execute TypeQL with `given` rows and enforce bounds while polling the
    /// driver answer stream.
    pub fn query_with_rows_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        rows: GivenRowsSpec,
        limits: RuntimeAnswerLimits,
        consumer: &'a mut (dyn FnMut(RuntimeAnswerItem) -> Result<RuntimeAnswerControl> + Send),
    ) -> BoxFuture<'a, Result<RuntimeAnswerStats>> {
        let tql = typeql.to_owned();
        #[cfg(not(feature = "band9"))]
        let _ = (&tql, &rows, &consumer);
        Box::pin(async move {
            runtime_check(&limits)?;
            match &self.inner {
                #[cfg(feature = "band7")]
                RuntimeTransactionInner::B7(_) => Err(RuntimeError::QueryExecution(
                    "given-stage parameterized queries require the band-9 driver \
                     (TypeDB 3.12+); this connection negotiated band 7"
                        .into(),
                )),
                #[cfg(feature = "band8")]
                RuntimeTransactionInner::B8(_) => Err(RuntimeError::QueryExecution(
                    "given-stage parameterized queries require the band-9 driver \
                     (TypeDB 3.12+); this connection negotiated band 8"
                        .into(),
                )),
                #[cfg(feature = "band9")]
                RuntimeTransactionInner::B9(opt) => {
                    let tx = opt.as_ref().ok_or_else(|| {
                        RuntimeError::Transaction("Transaction already consumed".into())
                    })?;
                    let given_rows = given_rows_b9(rows)?;
                    let answer = runtime_await(
                        async {
                            tx.query_with_rows(&tql, given_rows)
                                .await
                                .map_err(|e| RuntimeError::QueryExecution(format!("{e}")))
                        },
                        &limits,
                    )
                    .await?;
                    consume_answer_b9(answer, &limits, consumer).await
                }
            }
        })
    }

    /// Commit this transaction.
    pub fn commit(&mut self) -> BoxFuture<'_, Result<()>> {
        // Take ownership out of the Option so the async block can move the
        // transaction by value into commit(self) — both bands consume self.
        match &mut self.inner {
            #[cfg(feature = "band7")]
            RuntimeTransactionInner::B7(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    let t = tx.ok_or_else(|| {
                        RuntimeError::Transaction("Transaction already consumed".into())
                    })?;
                    t.commit().await.map_err(band7_commit_failure)
                })
            }
            #[cfg(feature = "band8")]
            RuntimeTransactionInner::B8(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    let t = tx.ok_or_else(|| {
                        RuntimeError::Transaction("Transaction already consumed".into())
                    })?;
                    t.commit().await.map_err(band8_commit_failure)
                })
            }
            #[cfg(feature = "band9")]
            RuntimeTransactionInner::B9(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    let t = tx.ok_or_else(|| {
                        RuntimeError::Transaction("Transaction already consumed".into())
                    })?;
                    t.commit().await.map_err(band9_commit_failure)
                })
            }
        }
    }

    /// Roll back this transaction.
    pub fn rollback(&mut self) -> BoxFuture<'_, Result<()>> {
        // Both bands expose rollback(&self), so we can move the value out of
        // the Option and call rollback on the owned T (the compiler
        // auto-borrows through the owned value for &self methods).
        match &mut self.inner {
            #[cfg(feature = "band7")]
            RuntimeTransactionInner::B7(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    let t = tx.ok_or_else(|| {
                        RuntimeError::Transaction("Transaction already consumed".into())
                    })?;
                    t.rollback()
                        .await
                        .map_err(|e| RuntimeError::Transaction(format!("Rollback failed: {e}")))
                })
            }
            #[cfg(feature = "band8")]
            RuntimeTransactionInner::B8(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    let t = tx.ok_or_else(|| {
                        RuntimeError::Transaction("Transaction already consumed".into())
                    })?;
                    t.rollback()
                        .await
                        .map_err(|e| RuntimeError::Transaction(format!("Rollback failed: {e}")))
                })
            }
            #[cfg(feature = "band9")]
            RuntimeTransactionInner::B9(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    let t = tx.ok_or_else(|| {
                        RuntimeError::Transaction("Transaction already consumed".into())
                    })?;
                    t.rollback()
                        .await
                        .map_err(|e| RuntimeError::Transaction(format!("Rollback failed: {e}")))
                })
            }
        }
    }

    /// Close this transaction without committing.
    pub fn close(&mut self) -> BoxFuture<'_, Result<()>> {
        match &mut self.inner {
            #[cfg(feature = "band7")]
            RuntimeTransactionInner::B7(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    let Some(t) = tx else {
                        return Ok(());
                    };
                    t.close()
                        .await
                        .map_err(|e| RuntimeError::Transaction(format!("Close failed: {e}")))
                })
            }
            #[cfg(feature = "band8")]
            RuntimeTransactionInner::B8(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    let Some(t) = tx else {
                        return Ok(());
                    };
                    t.close()
                        .await
                        .map_err(|e| RuntimeError::Transaction(format!("Close failed: {e}")))
                })
            }
            #[cfg(feature = "band9")]
            RuntimeTransactionInner::B9(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    let Some(t) = tx else {
                        return Ok(());
                    };
                    t.close()
                        .await
                        .map_err(|e| RuntimeError::Transaction(format!("Close failed: {e}")))
                })
            }
        }
    }
}

fn document_type_to_json(kind: &str, label: &str) -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::from_iter([
        (
            "kind".to_owned(),
            serde_json::Value::String(kind.to_owned()),
        ),
        (
            "label".to_owned(),
            serde_json::Value::String(label.to_owned()),
        ),
    ]))
}

fn document_attribute_type_to_json(label: &str, value_type: Option<&str>) -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::from_iter([
        (
            "kind".to_owned(),
            serde_json::Value::String("attribute".to_owned()),
        ),
        (
            "label".to_owned(),
            serde_json::Value::String(label.to_owned()),
        ),
        (
            "valueType".to_owned(),
            serde_json::Value::String(value_type.unwrap_or("none").to_owned()),
        ),
    ]))
}

macro_rules! define_driver_json_conversion {
    (
        $feature:literal,
        $driver:ident,
        $document_fn:ident,
        $node_fn:ident,
        $leaf_fn:ident,
        $value_fn:ident
    ) => {
        #[cfg(feature = $feature)]
        fn $document_fn(document: &$driver::answer::ConceptDocument) -> serde_json::Value {
            document
                .root
                .as_ref()
                .map($node_fn)
                .unwrap_or(serde_json::Value::Null)
        }

        #[cfg(feature = $feature)]
        fn $node_fn(node: &$driver::answer::concept_document::Node) -> serde_json::Value {
            use $driver::answer::concept_document::Node;

            match node {
                Node::Map(map) => serde_json::Value::Object(
                    map.iter()
                        .map(|(name, node)| (name.clone(), $node_fn(node)))
                        .collect(),
                ),
                Node::List(list) => serde_json::Value::Array(list.iter().map($node_fn).collect()),
                Node::Leaf(Some(leaf)) => $leaf_fn(leaf),
                Node::Leaf(None) => serde_json::Value::Null,
            }
        }

        #[cfg(feature = $feature)]
        fn $leaf_fn(leaf: &$driver::answer::concept_document::Leaf) -> serde_json::Value {
            use $driver::answer::concept_document::Leaf;
            use $driver::concept::Concept;

            match leaf {
                Leaf::Empty => serde_json::Value::Null,
                Leaf::Concept(concept) => match concept {
                    Concept::EntityType(_) => document_type_to_json("entity", concept.get_label()),
                    Concept::RelationType(_) => {
                        document_type_to_json("relation", concept.get_label())
                    }
                    Concept::RoleType(_) => {
                        document_type_to_json("relation:role", concept.get_label())
                    }
                    Concept::AttributeType(_) => {
                        let value_type = concept.try_get_value_type();
                        document_attribute_type_to_json(
                            concept.get_label(),
                            value_type.as_ref().map(|value_type| value_type.name()),
                        )
                    }
                    Concept::Attribute(_) | Concept::Value(_) => {
                        $value_fn(concept.try_get_value().expect("value concept has a value"))
                    }
                    concept @ (Concept::Entity(_) | Concept::Relation(_)) => {
                        unreachable!(
                            "unexpected concept encountered in fetch response: {:?}",
                            concept
                        )
                    }
                },
                Leaf::ValueType(value_type) => {
                    serde_json::Value::String(value_type.name().to_owned())
                }
                Leaf::Kind(kind) => serde_json::Value::String(kind.name().to_owned()),
            }
        }

        #[cfg(feature = $feature)]
        fn $value_fn(value: &$driver::concept::Value) -> serde_json::Value {
            use $driver::concept::Value;

            match value {
                Value::Boolean(value) => serde_json::Value::Bool(*value),
                Value::Integer(value) => serde_json::Value::from(*value),
                Value::Double(value) => serde_json::Value::from(*value),
                Value::String(value) => serde_json::Value::String(value.clone()),
                Value::Decimal(_)
                | Value::Date(_)
                | Value::Datetime(_)
                | Value::DatetimeTZ(_)
                | Value::Duration(_) => serde_json::Value::String(value.to_string()),
                Value::Struct(value, name) => {
                    let fields = value
                        .fields()
                        .iter()
                        .map(|(field, value)| {
                            (
                                field.clone(),
                                value
                                    .as_ref()
                                    .map($value_fn)
                                    .unwrap_or(serde_json::Value::Null),
                            )
                        })
                        .collect();
                    serde_json::Value::Object(serde_json::Map::from_iter([(
                        name.clone(),
                        serde_json::Value::Object(fields),
                    )]))
                }
            }
        }
    };
}

define_driver_json_conversion!(
    "band7",
    driver_b7,
    document_to_json_b7,
    document_node_to_json_b7,
    document_leaf_to_json_b7,
    value_to_json_b7
);
define_driver_json_conversion!(
    "band8",
    driver_b8,
    document_to_json_b8,
    document_node_to_json_b8,
    document_leaf_to_json_b8,
    value_to_json_b8
);
define_driver_json_conversion!(
    "band9",
    driver_b9,
    document_to_json_b9,
    document_node_to_json_b9,
    document_leaf_to_json_b9,
    value_to_json_b9
);

/// Convert a band-7 TypeDB concept to a JSON value.
///
/// Output shape is identical to [`concept_to_json_b8`] for all common concepts.
#[cfg(feature = "band7")]
fn concept_to_json_b7(
    concept: &type_bridge_typedb_driver_b7::concept::Concept,
) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert(
        "category".into(),
        serde_json::Value::String(concept.get_category().name().into()),
    );
    obj.insert(
        "label".into(),
        serde_json::Value::String(concept.get_label().into()),
    );
    if let Some(iid) = concept.try_get_iid() {
        obj.insert("iid".into(), serde_json::Value::String(iid.to_string()));
    }
    if let Some(value) = concept.try_get_value() {
        obj.insert("value".into(), value_to_json_b7(value));
    }
    if let Some(vt) = concept.try_get_value_type() {
        obj.insert(
            "value_type".into(),
            serde_json::Value::String(vt.name().into()),
        );
    }
    serde_json::Value::Object(obj)
}

/// Convert a band-8 TypeDB concept to a JSON value.
///
/// Output shape is identical to [`concept_to_json_b7`] for all common concepts.
#[cfg(feature = "band8")]
fn concept_to_json_b8(concept: &typedb_driver::concept::Concept) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert(
        "category".into(),
        serde_json::Value::String(concept.get_category().name().into()),
    );
    obj.insert(
        "label".into(),
        serde_json::Value::String(concept.get_label().into()),
    );
    if let Some(iid) = concept.try_get_iid() {
        obj.insert("iid".into(), serde_json::Value::String(iid.to_string()));
    }
    if let Some(value) = concept.try_get_value() {
        obj.insert("value".into(), value_to_json_b8(value));
    }
    if let Some(vt) = concept.try_get_value_type() {
        obj.insert(
            "value_type".into(),
            serde_json::Value::String(vt.name().into()),
        );
    }
    serde_json::Value::Object(obj)
}

/// Lower a portable [`GivenRowsSpec`] onto the band-9 driver's `GivenRows`.
///
/// Fails when a row's width does not match the header or a temporal string
/// does not parse — both are caller mistakes reported before any wire I/O.
#[cfg(feature = "band9")]
fn given_rows_b9(spec: GivenRowsSpec) -> Result<type_bridge_typedb_driver_b9::given::GivenRows> {
    use type_bridge_typedb_driver_b9::given::GivenRows;

    let mut given = GivenRows::new(spec.variables, spec.rows.len());
    for row in spec.rows {
        let entries = row
            .into_iter()
            .map(given_entry_b9)
            .collect::<Result<Vec<_>>>()?;
        given
            .push_row(entries)
            .map_err(|e| RuntimeError::QueryExecution(format!("Invalid given row: {e}")))?;
    }
    Ok(given)
}

/// Convert one portable [`GivenValue`] to a band-9 driver row entry.
#[cfg(feature = "band9")]
fn given_entry_b9(value: GivenValue) -> Result<type_bridge_typedb_driver_b9::given::GivenRowEntry> {
    use chrono::{DateTime, NaiveDate, NaiveDateTime};
    use type_bridge_typedb_driver_b9::concept::value::TimeZone as B9TimeZone;
    use type_bridge_typedb_driver_b9::given::GivenRowEntry;

    Ok(match value {
        GivenValue::Boolean(b) => GivenRowEntry::from(b),
        GivenValue::Integer(i) => GivenRowEntry::from(i),
        GivenValue::Double(d) => GivenRowEntry::from(d),
        GivenValue::String(s) => GivenRowEntry::from(s),
        GivenValue::Date(s) => {
            let date = s.parse::<NaiveDate>().map_err(|e| {
                RuntimeError::QueryExecution(format!("Invalid given date {s:?}: {e}"))
            })?;
            GivenRowEntry::from(date)
        }
        GivenValue::Datetime(s) => {
            let dt = s.parse::<NaiveDateTime>().map_err(|e| {
                RuntimeError::QueryExecution(format!("Invalid given datetime {s:?}: {e}"))
            })?;
            GivenRowEntry::from(dt)
        }
        GivenValue::DatetimeTz(s) => {
            let dt = DateTime::parse_from_rfc3339(&s).map_err(|e| {
                RuntimeError::QueryExecution(format!("Invalid given datetime-tz {s:?}: {e}"))
            })?;
            let offset = *dt.offset();
            GivenRowEntry::from(dt.with_timezone(&B9TimeZone::Fixed(offset)))
        }
    })
}

/// Convert and consume a band-9 answer without materializing its stream.
#[cfg(feature = "band9")]
async fn consume_answer_b9(
    answer: B9QueryAnswer,
    limits: &RuntimeAnswerLimits,
    consumer: &mut (dyn FnMut(RuntimeAnswerItem) -> Result<RuntimeAnswerControl> + Send),
) -> Result<RuntimeAnswerStats> {
    match answer {
        B9QueryAnswer::Ok(_) => Ok(RuntimeAnswerStats::new(RuntimeAnswerKind::Ok)),
        B9QueryAnswer::ConceptRowStream(_, stream) => {
            let stream = stream
                .map_ok(|row| {
                    let mut obj = serde_json::Map::new();
                    for (i, col) in row.get_column_names().iter().enumerate() {
                        let value = row
                            .row
                            .get(i)
                            .and_then(|c| c.as_ref())
                            .map(concept_to_json_b9)
                            .unwrap_or(serde_json::Value::Null);
                        obj.insert(col.clone(), value);
                    }
                    RuntimeAnswerItem::Row(serde_json::Value::Object(obj))
                })
                .map_err(|error| RuntimeError::QueryExecution(format!("Row stream: {error}")));
            runtime_consume_stream(stream, RuntimeAnswerKind::Rows, limits, consumer).await
        }
        B9QueryAnswer::ConceptDocumentStream(_, stream) => {
            let stream = stream
                .map_ok(|document| RuntimeAnswerItem::Document(document_to_json_b9(&document)))
                .map_err(|error| RuntimeError::QueryExecution(format!("Document stream: {error}")));
            runtime_consume_stream(stream, RuntimeAnswerKind::Documents, limits, consumer).await
        }
    }
}

/// Convert a band-9 TypeDB concept to a JSON value.
///
/// Output shape is identical to [`concept_to_json_b8`] for all common concepts.
#[cfg(feature = "band9")]
fn concept_to_json_b9(
    concept: &type_bridge_typedb_driver_b9::concept::Concept,
) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert(
        "category".into(),
        serde_json::Value::String(concept.get_category().name().into()),
    );
    obj.insert(
        "label".into(),
        serde_json::Value::String(concept.get_label().into()),
    );
    if let Some(iid) = concept.try_get_iid() {
        obj.insert("iid".into(), serde_json::Value::String(iid.to_string()));
    }
    if let Some(value) = concept.try_get_value() {
        obj.insert("value".into(), value_to_json_b9(value));
    }
    if let Some(vt) = concept.try_get_value_type() {
        obj.insert(
            "value_type".into(),
            serde_json::Value::String(vt.name().into()),
        );
    }
    serde_json::Value::Object(obj)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    macro_rules! document_scalar_regression {
        ($feature:literal, $name:ident, $driver:ident, $node_fn:ident, $decimal:expr) => {
            #[cfg(feature = $feature)]
            #[test]
            fn $name() {
                use $driver::answer::concept_document::{Leaf, Node};
                use $driver::concept::{Concept, Value};

                const LARGE_INTEGER: i64 = 9_007_199_254_740_993;
                let integer = Node::Leaf(Some(Leaf::Concept(Concept::Value(Value::Integer(
                    LARGE_INTEGER,
                )))));
                assert_eq!(
                    $node_fn(&integer),
                    serde_json::Value::from(LARGE_INTEGER),
                    "concept-document integers must not cross an f64 boundary"
                );

                let decimal = $decimal;
                let expected = decimal.to_string();
                let decimal =
                    Node::Leaf(Some(Leaf::Concept(Concept::Value(Value::Decimal(decimal)))));
                assert_eq!(
                    $node_fn(&decimal),
                    serde_json::Value::String(expected),
                    "concept-document decimals must remain lossless strings"
                );
            }
        };
    }

    document_scalar_regression!(
        "band7",
        band7_document_scalars_are_lossless,
        driver_b7,
        document_node_to_json_b7,
        driver_b7::concept::value::Decimal::new(1234, 5_600_000_000_000_000_000)
    );
    document_scalar_regression!(
        "band8",
        band8_document_scalars_are_lossless,
        driver_b8,
        document_node_to_json_b8,
        driver_b8::concept::value::Decimal::new(1234, 5_600_000_000_000_000_000)
    );
    document_scalar_regression!(
        "band9",
        band9_document_scalars_are_lossless,
        driver_b9,
        document_node_to_json_b9,
        driver_b9::concept::value::Decimal::from_parts(1234, 5_600_000_000_000_000_000)
    );

    async fn assert_pending_runtime_await_hits_deadline() {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let limits = RuntimeAnswerLimits {
            max_items: 10,
            max_bytes: 4096,
            deadline: Some(
                (tokio::time::Instant::now() + std::time::Duration::from_secs(5)).into_std(),
            ),
            cancellation: RuntimeAnswerCancellation::default(),
        };
        let execution = tokio::spawn(async move {
            runtime_await(
                async move {
                    let _ = started_tx.send(());
                    std::future::pending::<Result<()>>().await
                },
                &limits,
            )
            .await
        });

        started_rx.await.unwrap();
        tokio::time::advance(std::time::Duration::from_secs(5)).await;
        tokio::task::yield_now().await;
        assert!(execution.is_finished());
        assert!(matches!(
            execution.await.unwrap().unwrap_err(),
            RuntimeError::ResourceLimit {
                code: "transaction_deadline_exceeded",
                ..
            }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn runtime_deadline_interrupts_pending_query_await() {
        assert_pending_runtime_await_hits_deadline().await;
    }

    #[tokio::test(start_paused = true)]
    async fn runtime_deadline_interrupts_pending_stream_poll() {
        assert_pending_runtime_await_hits_deadline().await;
    }

    #[tokio::test]
    async fn runtime_cancellation_after_poll_start_interrupts_pending_await() {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let cancellation = RuntimeAnswerCancellation::default();
        let trigger = cancellation.clone();
        let limits = RuntimeAnswerLimits {
            max_items: 10,
            max_bytes: 4096,
            deadline: None,
            cancellation,
        };
        let execution = tokio::spawn(async move {
            runtime_await(
                async move {
                    let _ = started_tx.send(());
                    std::future::pending::<Result<()>>().await
                },
                &limits,
            )
            .await
        });

        started_rx.await.unwrap();
        trigger.cancel();
        tokio::task::yield_now().await;
        assert!(execution.is_finished());
        assert!(matches!(
            execution.await.unwrap().unwrap_err(),
            RuntimeError::ResourceLimit {
                code: "provider_cancelled",
                ..
            }
        ));
    }

    #[test]
    fn bounded_runtime_reader_stops_and_enforces_limits_before_consumer() {
        let limits = RuntimeAnswerLimits {
            max_items: 1,
            max_bytes: 64,
            deadline: None,
            cancellation: RuntimeAnswerCancellation::default(),
        };
        let mut stats = RuntimeAnswerStats::new(RuntimeAnswerKind::Rows);
        let mut accepted = 0;
        let mut stop = |_item| {
            accepted += 1;
            Ok(RuntimeAnswerControl::Stop)
        };
        assert_eq!(
            runtime_accept(
                &limits,
                &mut stats,
                RuntimeAnswerItem::Row(serde_json::json!({"v": 1})),
                &mut stop,
            )
            .unwrap(),
            RuntimeAnswerControl::Stop
        );
        assert_eq!(accepted, 1);
        assert!(stats.stopped_early);

        let mut never_called =
            |_item| -> Result<RuntimeAnswerControl> { panic!("over-limit item reached consumer") };
        let error = runtime_accept(
            &limits,
            &mut stats,
            RuntimeAnswerItem::Row(serde_json::json!({"v": 2})),
            &mut never_called,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            RuntimeError::ResourceLimit {
                code: "processed_item_limit",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn bounded_runtime_stream_stops_before_polling_another_item() {
        let polls = Arc::new(AtomicUsize::new(0));
        let observed_polls = Arc::clone(&polls);
        let mut emitted = false;
        let stream = futures::stream::poll_fn(move |_context| {
            observed_polls.fetch_add(1, Ordering::SeqCst);
            assert!(!emitted, "stream was polled after the consumer stopped");
            emitted = true;
            std::task::Poll::Ready(Some(Ok::<_, RuntimeError>(RuntimeAnswerItem::Row(
                serde_json::json!({"v": 1}),
            ))))
        });
        let limits = RuntimeAnswerLimits::unbounded();
        let mut consumer = |_item| Ok(RuntimeAnswerControl::Stop);

        let stats = runtime_consume_stream(stream, RuntimeAnswerKind::Rows, &limits, &mut consumer)
            .await
            .unwrap();

        assert_eq!(polls.load(Ordering::SeqCst), 1);
        assert_eq!(stats.processed_items, 1);
        assert!(stats.stopped_early);
    }

    #[tokio::test]
    async fn bounded_runtime_stream_rejects_an_over_limit_item_before_consumer() {
        let stream = futures::stream::iter([
            Ok(RuntimeAnswerItem::Row(serde_json::json!({"v": 1}))),
            Ok(RuntimeAnswerItem::Row(serde_json::json!({"v": 2}))),
        ]);
        let limits = RuntimeAnswerLimits {
            max_items: 1,
            max_bytes: u64::MAX,
            deadline: None,
            cancellation: RuntimeAnswerCancellation::default(),
        };
        let mut accepted = 0;
        let mut consumer = |_item| {
            accepted += 1;
            Ok(RuntimeAnswerControl::Continue)
        };

        let error = runtime_consume_stream(stream, RuntimeAnswerKind::Rows, &limits, &mut consumer)
            .await
            .unwrap_err();

        assert_eq!(accepted, 1);
        assert!(matches!(
            error,
            RuntimeError::ResourceLimit {
                code: "processed_item_limit",
                ..
            }
        ));
    }

    #[cfg(any(feature = "band7", feature = "band8"))]
    fn empty_given_rows() -> GivenRowsSpec {
        GivenRowsSpec {
            variables: vec!["value".into()],
            rows: Vec::new(),
        }
    }

    #[cfg(feature = "band7")]
    #[tokio::test]
    async fn band7_transaction_rejects_bounded_given_rows_actionably() {
        let mut transaction = RuntimeTransaction {
            inner: RuntimeTransactionInner::B7(None),
        };
        let mut consumer = |_item| Ok(RuntimeAnswerControl::Continue);

        assert!(!transaction.supports_given_rows());
        let error = transaction
            .query_with_rows_bounded(
                "given $value: string; match $x isa thing;",
                empty_given_rows(),
                RuntimeAnswerLimits::unbounded(),
                &mut consumer,
            )
            .await
            .unwrap_err();

        assert!(
            matches!(&error, RuntimeError::QueryExecution(message) if message.contains("band-9 driver") && message.contains("band 7")),
            "unexpected error: {error}"
        );
    }

    #[cfg(feature = "band8")]
    #[tokio::test]
    async fn band8_transaction_rejects_bounded_given_rows_actionably() {
        let mut transaction = RuntimeTransaction {
            inner: RuntimeTransactionInner::B8(None),
        };
        let mut consumer = |_item| Ok(RuntimeAnswerControl::Continue);

        assert!(!transaction.supports_given_rows());
        let error = transaction
            .query_with_rows_bounded(
                "given $value: string; match $x isa thing;",
                empty_given_rows(),
                RuntimeAnswerLimits::unbounded(),
                &mut consumer,
            )
            .await
            .unwrap_err();

        assert!(
            matches!(&error, RuntimeError::QueryExecution(message) if message.contains("band-9 driver") && message.contains("band 8")),
            "unexpected error: {error}"
        );
    }

    #[cfg(feature = "band9")]
    #[test]
    fn band9_transaction_reports_given_row_transport_support() {
        let transaction = RuntimeTransaction {
            inner: RuntimeTransactionInner::B9(None),
        };

        assert!(transaction.supports_given_rows());
    }

    #[test]
    fn bounded_runtime_reader_rechecks_cancellation_after_consumer_work() {
        let cancellation = RuntimeAnswerCancellation::default();
        let trigger = cancellation.clone();
        let limits = RuntimeAnswerLimits {
            max_items: 1,
            max_bytes: 64,
            deadline: None,
            cancellation,
        };
        let mut stats = RuntimeAnswerStats::new(RuntimeAnswerKind::Rows);
        let mut consumer = move |_item| {
            trigger.cancel();
            Ok(RuntimeAnswerControl::Continue)
        };

        let error = runtime_accept(
            &limits,
            &mut stats,
            RuntimeAnswerItem::Row(serde_json::json!({"v": 1})),
            &mut consumer,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            RuntimeError::ResourceLimit {
                code: "provider_cancelled",
                ..
            }
        ));

        let cancellation = RuntimeAnswerCancellation::default();
        let trigger = cancellation.clone();
        let limits = RuntimeAnswerLimits {
            max_items: 1,
            max_bytes: 64,
            deadline: None,
            cancellation,
        };
        let mut stats = RuntimeAnswerStats::new(RuntimeAnswerKind::Rows);
        let mut failing_consumer = move |_item| -> Result<RuntimeAnswerControl> {
            trigger.cancel();
            Err(RuntimeError::AnswerConsumer)
        };
        let error = runtime_accept(
            &limits,
            &mut stats,
            RuntimeAnswerItem::Row(serde_json::json!({"v": 1})),
            &mut failing_consumer,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            RuntimeError::ResourceLimit {
                code: "provider_cancelled",
                ..
            }
        ));
    }

    #[test]
    fn connect_options_default_matches_ssot() {
        let options = ConnectOptions::default();
        assert_eq!(options.http_port, DEFAULT_HTTP_PORT);
        assert!(!options.tls);
        assert_eq!(options.server_version, None);
    }

    /// The injectable probe must receive the configured port, not a
    /// hardcoded 8000.  The probe returns an in-window version so the gate
    /// proceeds to the gRPC connection attempt, which fails with a network
    /// error (no server) — the assertion is about the recorded port and
    /// that the HTTP failure fallback path was not involved.
    #[tokio::test]
    async fn gated_driver_probe_receives_configured_port() {
        let recorded_port: Arc<Mutex<Option<u16>>> = Arc::new(Mutex::new(None));
        let captured = Arc::clone(&recorded_port);

        let result = gated_driver_with_probe(
            "localhost:1729",
            "admin",
            "password",
            ConnectOptions {
                http_port: 9123,
                tls: false,
                server_version: None,
            },
            move |_addr, port, _tls| {
                *captured.lock().unwrap() = Some(port);
                Ok(core_version::Version::new(3, 8, 3))
            },
        )
        .await;

        let observed = recorded_port.lock().unwrap().expect("probe was not called");
        assert_eq!(
            observed, 9123,
            "probe must receive the configured http_port (9123), got {observed}"
        );

        if let Err(RuntimeError::UnsupportedVersion(_)) = result {
            panic!("expected a connection error (no server), not a version gate rejection")
        }
    }

    /// When HTTP version detection fails, the constructor attempts gRPC band 8
    /// and then band 7. If neither can connect, the error should preserve all
    /// attempted paths instead of surfacing only the HTTP failure.
    #[tokio::test]
    async fn gated_driver_http_failure_reports_grpc_fallback_failures() {
        let result = gated_driver_with_probe(
            "127.0.0.1:1",
            "admin",
            "password",
            ConnectOptions {
                http_port: 9123,
                tls: false,
                server_version: None,
            },
            move |_addr, _port, _tls| {
                Err(core_version::VersionError::Probe(
                    "HTTP endpoint unavailable".to_string(),
                ))
            },
        )
        .await;

        match result {
            Err(RuntimeError::UnsupportedVersion(err)) => {
                let msg = err.to_string();
                assert!(msg.contains("HTTP endpoint unavailable"), "{msg}");
                assert!(msg.contains("band-8 gRPC attempt failed"), "{msg}");
                assert!(msg.contains("band-7 gRPC attempt failed"), "{msg}");
                // Band 9 must never be probed blindly: a band-9 connect
                // attempt crashes a 3.11 server (see grpc_fallback_driver).
                assert!(!msg.contains("band-9 gRPC attempt"), "{msg}");
            }
            Err(other) => panic!("expected aggregated version-probe failure, got {other}"),
            Ok(_) => panic!("expected aggregated version-probe failure, got successful connection"),
        }
    }

    #[cfg(feature = "band8")]
    #[test]
    fn band8_lazy_validation_failure_is_retryable() {
        let result = classify_band8_grpc_version(
            "localhost:1729",
            Err("protocol handshake failed".to_string()),
        )
        .expect("transport validation failure should remain retryable");

        match result {
            Band8GrpcVersion::RetryableFailure(failure) => {
                assert!(failure.contains("localhost:1729"), "{failure}");
                assert!(failure.contains("protocol handshake failed"), "{failure}");
            }
            Band8GrpcVersion::Validated(version) => {
                panic!("failed lazy connection unexpectedly validated as {version}")
            }
        }
    }

    #[cfg(feature = "band8")]
    #[test]
    fn band8_reported_versions_are_authoritative() {
        let unsupported = classify_band8_grpc_version("localhost:1729", Ok("3.7.3".to_string()))
            .expect_err("reported below-window version must be terminal");
        assert!(matches!(
            unsupported,
            RuntimeError::UnsupportedVersion(core_version::VersionError::Unsupported {
                component: "server",
                found: core_version::Version {
                    major: 3,
                    minor: 7,
                    patch: 3,
                },
            })
        ));

        let wrong_band = classify_band8_grpc_version("localhost:1729", Ok("3.10.4".to_string()))
            .expect_err("reported band-7 version must not silently fall through");
        match wrong_band {
            RuntimeError::UnsupportedVersion(error) => {
                assert!(
                    error
                        .to_string()
                        .contains("server version 3.10.4, which does not accept band 8")
                );
            }
            other => panic!("expected authoritative version rejection, got {other}"),
        }

        let malformed =
            classify_band8_grpc_version("localhost:1729", Ok("not-a-version".to_string()))
                .expect_err("malformed reported version must be terminal");
        assert!(matches!(
            malformed,
            RuntimeError::UnsupportedVersion(core_version::VersionError::Parse(_))
        ));

        let validated = classify_band8_grpc_version("localhost:1729", Ok("3.11.5".to_string()))
            .expect("reported band-8 version should validate");
        assert!(matches!(
            validated,
            Band8GrpcVersion::Validated(core_version::Version {
                major: 3,
                minor: 11,
                patch: 5,
            })
        ));

        let backward_compatible =
            classify_band8_grpc_version("localhost:1729", Ok("3.12.0".to_string()))
                .expect("reported band-9 server accepts the discovery band");
        assert!(matches!(
            backward_compatible,
            Band8GrpcVersion::Validated(core_version::Version {
                major: 3,
                minor: 12,
                patch: 0,
            })
        ));
    }

    /// A caller-supplied exact server version is the gRPC-only escape hatch: it
    /// must bypass the HTTP probe but still flow through the normal embedded
    /// runtime gate before driver construction.
    #[tokio::test]
    async fn gated_driver_pinned_version_skips_probe() {
        let probe_called = Arc::new(Mutex::new(false));
        let captured = Arc::clone(&probe_called);

        let result = gated_driver_with_probe(
            "localhost:1729",
            "admin",
            "password",
            ConnectOptions {
                http_port: 9123,
                tls: false,
                server_version: Some(core_version::Version::new(3, 8, 3)),
            },
            move |_addr, _port, _tls| {
                *captured.lock().unwrap() = true;
                Ok(core_version::Version::new(3, 11, 5))
            },
        )
        .await;

        assert!(
            !*probe_called.lock().unwrap(),
            "pinned server_version must skip the HTTP probe"
        );

        if let Err(RuntimeError::UnsupportedVersion(_)) = result {
            panic!("expected a connection error (no server), not a version gate rejection")
        }
    }

    /// Pinned versions still use the exact semantic version gate. This prevents
    /// a gRPC-only band-7 path from silently accepting unsupported 3.7 servers.
    #[tokio::test]
    async fn gated_driver_rejects_unsupported_pinned_version_without_probe() {
        let probe_called = Arc::new(Mutex::new(false));
        let captured = Arc::clone(&probe_called);

        let result = gated_driver_with_probe(
            "localhost:1729",
            "admin",
            "password",
            ConnectOptions {
                http_port: 9123,
                tls: false,
                server_version: Some(core_version::Version::new(3, 7, 3)),
            },
            move |_addr, _port, _tls| {
                *captured.lock().unwrap() = true;
                Ok(core_version::Version::new(3, 8, 3))
            },
        )
        .await;

        assert!(
            !*probe_called.lock().unwrap(),
            "unsupported pinned server_version must skip the HTTP probe"
        );

        match result {
            Err(RuntimeError::UnsupportedVersion(err)) => {
                assert!(
                    err.to_string().contains("3.7.3"),
                    "error should name rejected version: {err}"
                );
            }
            Err(other) => panic!("expected unsupported-version rejection for 3.7.3, got {other}"),
            Ok(_) => panic!(
                "expected unsupported-version rejection for 3.7.3, got successful connection"
            ),
        }
    }

    #[test]
    fn cargo_lock_pin() {
        let lock_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock");
        let lock_contents = std::fs::read_to_string(lock_path)
            .expect("Cargo.lock not found relative to crate root");

        let lock_version = lock_contents
            .split("[[package]]")
            .find(|block| block.contains("name = \"typedb-driver\""))
            .and_then(|block| {
                block
                    .lines()
                    .find(|line| line.trim_start().starts_with("version = "))
            })
            .and_then(|line| {
                let start = line.find('"')? + 1;
                let end = line.rfind('"')?;
                Some(&line[start..end])
            })
            .expect("typedb-driver entry not found in Cargo.lock");

        assert_eq!(
            lock_version, PINNED_DRIVER_VERSION,
            "Cargo.lock resolves typedb-driver {lock_version} but PINNED_DRIVER_VERSION \
             is {PINNED_DRIVER_VERSION}; update the runtime constant"
        );

        let pinned: core_version::Version = PINNED_DRIVER_VERSION.parse().unwrap();
        assert_eq!(
            core_version::band(&pinned),
            Some(8),
            "pinned driver version {PINNED_DRIVER_VERSION} left protocol band 8; \
             review the gate expectations before accepting the bump"
        );
    }

    #[test]
    fn cargo_lock_pin_b7() {
        let lock_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock");
        let lock_contents = std::fs::read_to_string(lock_path)
            .expect("Cargo.lock not found relative to crate root");

        let lock_version = lock_contents
            .split("[[package]]")
            .find(|block| block.contains("name = \"type-bridge-typedb-driver-b7\""))
            .and_then(|block| {
                block
                    .lines()
                    .find(|line| line.trim_start().starts_with("version = "))
            })
            .and_then(|line| {
                let start = line.find('"')? + 1;
                let end = line.rfind('"')?;
                Some(&line[start..end])
            })
            .expect("type-bridge-typedb-driver-b7 entry not found in Cargo.lock");

        assert_eq!(
            lock_version, PINNED_DRIVER_VERSION_B7,
            "Cargo.lock resolves type-bridge-typedb-driver-b7 {lock_version} but \
             PINNED_DRIVER_VERSION_B7 is {PINNED_DRIVER_VERSION_B7}; update the runtime constant"
        );

        let pinned: core_version::Version = PINNED_DRIVER_VERSION_B7.parse().unwrap();
        assert_eq!(
            core_version::band(&pinned),
            Some(7),
            "pinned band-7 fork version {PINNED_DRIVER_VERSION_B7} left protocol band 7; \
             review the gate expectations before accepting the bump"
        );
    }

    /// Portable given-rows lower onto the band-9 driver structures, including
    /// temporal string parsing; malformed rows fail before any wire I/O.
    #[cfg(feature = "band9")]
    #[test]
    fn given_rows_lowering() {
        let spec = GivenRowsSpec {
            variables: vec!["n".into(), "a".into()],
            rows: vec![
                vec![GivenValue::String("alice".into()), GivenValue::Integer(28)],
                vec![GivenValue::String("bob".into()), GivenValue::Integer(26)],
            ],
        };
        let rows = given_rows_b9(spec).expect("valid spec must lower");
        let (header, rows) = rows.into_parts();
        assert_eq!(header.width(), 2);
        assert_eq!(rows.len(), 2);
    }

    #[cfg(feature = "band9")]
    #[test]
    fn given_rows_lowering_rejects_width_mismatch() {
        let spec = GivenRowsSpec {
            variables: vec!["n".into(), "a".into()],
            rows: vec![vec![GivenValue::String("alice".into())]],
        };
        let err = given_rows_b9(spec).expect_err("short row must be rejected");
        assert!(matches!(err, RuntimeError::QueryExecution(_)), "{err}");
    }

    #[cfg(feature = "band9")]
    #[test]
    fn given_entry_temporal_parsing() {
        for value in [
            GivenValue::Date("2026-07-13".into()),
            GivenValue::Datetime("2026-07-13T10:30:00".into()),
            GivenValue::DatetimeTz("2026-07-13T10:30:00+09:00".into()),
            GivenValue::Boolean(true),
            GivenValue::Double(1.5),
        ] {
            given_entry_b9(value.clone()).unwrap_or_else(|e| panic!("{value:?} must convert: {e}"));
        }
    }

    #[cfg(feature = "band9")]
    #[test]
    fn given_entry_rejects_malformed_temporal() {
        for value in [
            GivenValue::Date("not-a-date".into()),
            GivenValue::Datetime("2026-13-45T99:00:00".into()),
            GivenValue::DatetimeTz("2026-07-13 10:30".into()),
        ] {
            let err = given_entry_b9(value.clone())
                .expect_err("malformed temporal string must be rejected");
            assert!(
                err.to_string().contains("Invalid given"),
                "{value:?}: {err}"
            );
        }
    }

    #[test]
    fn cargo_lock_pin_b9() {
        let lock_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock");
        let lock_contents = std::fs::read_to_string(lock_path)
            .expect("Cargo.lock not found relative to crate root");

        let lock_version = lock_contents
            .split("[[package]]")
            .find(|block| block.contains("name = \"type-bridge-typedb-driver-b9\""))
            .and_then(|block| {
                block
                    .lines()
                    .find(|line| line.trim_start().starts_with("version = "))
            })
            .and_then(|line| {
                let start = line.find('"')? + 1;
                let end = line.rfind('"')?;
                Some(&line[start..end])
            })
            .expect("type-bridge-typedb-driver-b9 entry not found in Cargo.lock");

        assert_eq!(
            lock_version, PINNED_DRIVER_VERSION_B9,
            "Cargo.lock resolves type-bridge-typedb-driver-b9 {lock_version} but \
             PINNED_DRIVER_VERSION_B9 is {PINNED_DRIVER_VERSION_B9}; update the runtime constant"
        );

        let pinned: core_version::Version = PINNED_DRIVER_VERSION_B9.parse().unwrap();
        assert_eq!(
            core_version::band(&pinned),
            Some(9),
            "pinned band-9 fork version {PINNED_DRIVER_VERSION_B9} left protocol band 9; \
             review the gate expectations before accepting the bump"
        );
    }
}
