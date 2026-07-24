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
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use type_bridge_core_lib::version as core_version;
use type_bridge_core_lib::version::DEFAULT_HTTP_PORT;

#[cfg(feature = "band8")]
use type_bridge_typedb_driver_b8 as driver_b8;
#[cfg(feature = "band8")]
use type_bridge_typedb_driver_b8::answer::QueryAnswer as B8QueryAnswer;
#[cfg(feature = "band8")]
use type_bridge_typedb_driver_b8::{
    Addresses, Credentials as B8Credentials, DriverOptions, DriverTlsConfig,
    TransactionOptions as B8TransactionOptions, TransactionType as B8TransactionType,
    TypeDBDriver as B8Driver,
};

#[cfg(feature = "band7")]
use type_bridge_typedb_driver_b7 as driver_b7;
#[cfg(feature = "band7")]
use type_bridge_typedb_driver_b7::answer::QueryAnswer as B7QueryAnswer;
#[cfg(feature = "band7")]
use type_bridge_typedb_driver_b7::{
    Credentials as B7Credentials, DriverOptions as B7DriverOptions,
    TransactionOptions as B7TransactionOptions, TransactionType as B7TransactionType,
    TypeDBDriver as B7Driver,
};

#[cfg(feature = "band9")]
use typedb_driver as driver_b9;
#[cfg(feature = "band9")]
use typedb_driver::answer::QueryAnswer as B9QueryAnswer;
#[cfg(feature = "band9")]
use typedb_driver::{
    Addresses as B9Addresses, Credentials as B9Credentials, DriverOptions as B9DriverOptions,
    DriverTlsConfig as B9DriverTlsConfig, TransactionOptions as B9TransactionOptions,
    TransactionType as B9TransactionType, TypeDBDriver as B9Driver,
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

/// A classified result available to recovery-aware internal commit callers.
///
/// [`RuntimeTransaction::commit`] retains the released [`RuntimeError`]
/// surface. Callers that need durability certainty can opt into
/// [`RuntimeTransaction::commit_classified`] without adding a variant to that
/// released, exhaustively matchable enum.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeCommitError {
    /// A transaction lifecycle error before the driver attempted a commit.
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    /// A driver commit failure with explicit durability certainty.
    #[error("Transaction error: Commit failed: {message}")]
    Driver {
        /// Whether the failed response proves that the commit was aborted.
        certainty: CommitFailureCertainty,
        /// The original driver error text.
        message: String,
    },
}

impl RuntimeCommitError {
    /// Convert to the released transaction-error surface.
    #[must_use]
    pub fn into_runtime_error(self) -> RuntimeError {
        match self {
            Self::Runtime(error) => error,
            Self::Driver { message, .. } => {
                RuntimeError::Transaction(format!("Commit failed: {message}"))
            }
        }
    }
}

fn commit_failure(
    certainty: CommitFailureCertainty,
    message: impl Into<String>,
) -> RuntimeCommitError {
    RuntimeCommitError::Driver {
        certainty,
        message: message.into(),
    }
}

#[cfg(feature = "band7")]
fn band7_commit_failure(error: driver_b7::Error) -> RuntimeCommitError {
    let certainty = if matches!(&error, driver_b7::Error::Server(_)) {
        CommitFailureCertainty::DefinitelyAborted
    } else {
        CommitFailureCertainty::Unknown
    };
    commit_failure(certainty, error.to_string())
}

#[cfg(feature = "band8")]
fn band8_commit_failure(error: driver_b8::Error) -> RuntimeCommitError {
    let certainty = if matches!(&error, driver_b8::Error::Server(_)) {
        CommitFailureCertainty::DefinitelyAborted
    } else {
        CommitFailureCertainty::Unknown
    };
    commit_failure(certainty, error.to_string())
}

#[cfg(feature = "band9")]
fn band9_commit_failure(error: driver_b9::Error) -> RuntimeCommitError {
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
                &error,
                RuntimeCommitError::Driver {
                    certainty: actual,
                    ..
                } if *actual == certainty
            ));
            assert!(matches!(
                error.into_runtime_error(),
                RuntimeError::Transaction(message)
                    if message == "Commit failed: driver response"
            ));
        }
    }

    #[cfg(feature = "band7")]
    #[test]
    fn band7_opaque_commit_failure_is_unknown() {
        let error = band7_commit_failure(driver_b7::Error::Other("transport".into()));
        assert!(matches!(
            error,
            RuntimeCommitError::Driver {
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
            RuntimeCommitError::Driver {
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
            RuntimeCommitError::Driver {
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

    fn is_unbounded(&self) -> bool {
        self.max_items == u64::MAX && self.max_bytes == u64::MAX && self.deadline.is_none()
    }
}

/// V2-only runtime limits layered over the released answer-limit shape.
///
/// The wrapper keeps [`RuntimeAnswerLimits`] exhaustively constructible by
/// released 1.5.x callers while carrying the additive document-list budget.
#[derive(Debug, Clone)]
pub struct QueryV2RuntimeAnswerLimits {
    /// Released item, byte, deadline, and cancellation limits.
    pub answer: RuntimeAnswerLimits,
    /// Maximum aggregate list members converted across fetched documents.
    pub max_collection_members: u64,
}

impl Default for QueryV2RuntimeAnswerLimits {
    fn default() -> Self {
        Self {
            answer: RuntimeAnswerLimits {
                max_items: 100_000,
                max_bytes: 64 * 1024 * 1024,
                deadline: None,
                cancellation: RuntimeAnswerCancellation::default(),
            },
            max_collection_members: 65_536,
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
/// Released temporal variants retain their ISO-8601 text representation.
/// Additive V2 variants carry exact components when text alone would erase
/// provider semantics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GivenValue {
    /// Explicit absence for an optional `given` cell.
    Empty,
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
    /// TypeQL `datetime-tz`, with a fixed offset.
    DatetimeTz(String),
    /// Exact V2 TypeQL `datetime-tz`, retaining an optional authored IANA zone
    /// and its already-validated effective offset.
    DatetimeTzExact {
        /// Canonical local datetime.
        local: String,
        /// Authored IANA name, or `None` for UTC/fixed-offset values.
        named_zone: Option<String>,
        /// Effective offset selected for this local value.
        effective_offset_seconds: i32,
    },
    /// TypeQL fixed-point `decimal`.
    Decimal(String),
    /// Exact non-negative TypeDB duration components.
    Duration {
        /// Calendar months.
        months: u32,
        /// Calendar days.
        days: u32,
        /// Absolute nanoseconds.
        nanos: u64,
    },
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

/// The compile-pinned band-8 driver fork version resolved in `Cargo.lock`.
///
/// This constant records the exact driver version this runtime crate was built
/// against.  It is the factual counterpart to the policy window declared in
/// `type_bridge_core_lib::version`: the window says which versions are allowed;
/// this constant says which version is currently in use.
///
/// When `type-bridge-typedb-driver-b8` is refreshed, update this constant to
/// match the new `Cargo.lock` entry — the `tests::cargo_lock_pin`
/// test will catch any divergence.
pub const PINNED_DRIVER_VERSION: &str = "3.11.5";

/// The compile-pinned version of the renamed band-7 driver package
/// (`type-bridge-typedb-driver-b7`) this runtime crate was built against.
///
/// Mirrors [`PINNED_DRIVER_VERSION`] for the band-8 driver fork.  When
/// the band-7 package is refreshed, update this constant to match the new
/// `Cargo.lock` entry — the `tests::cargo_lock_pin_b7` test will
/// catch any divergence.
pub const PINNED_DRIVER_VERSION_B7: &str = "3.8.1";

/// The compile-pinned version of the upstream band-9 `typedb-driver` crate
/// this runtime crate was built against.
///
/// Mirrors [`PINNED_DRIVER_VERSION`] for the band-8 driver fork.  When
/// the band-9 dependency is refreshed, update this constant to match the new
/// `Cargo.lock` entry — the `tests::cargo_lock_pin_b9` test will
/// catch any divergence.
pub const PINNED_DRIVER_VERSION_B9: &str = "3.12.1";

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
/// `[(7, "3.8.1"), (8, "3.11.5"), (9, "3.12.1")]`.
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

/// Canonical TLS policy for secure TypeDB connections.
///
/// The released [`ConnectOptions::tls`] Boolean is retained as a compatibility
/// adapter.  New callers use this type so a custom root can never be confused
/// with either plaintext or native-root TLS.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TlsMode {
    /// Connect over plaintext HTTP and gRPC.
    #[default]
    Disabled,
    /// Connect over TLS using the operating system's native trust roots.
    NativeRoots,
    /// Connect over TLS using only the PEM certificate bundle at this path.
    CustomRootCa(PathBuf),
}

impl TlsMode {
    /// Return whether this mode requires TLS on every transport.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        !matches!(self, Self::Disabled)
    }
}

/// Connection options for the typed TLS transport surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecureConnectOptions {
    /// Port of the TypeDB HTTP API used for the connect-time version probe.
    pub http_port: u16,
    /// TLS policy shared by the HTTP probe and every gRPC driver attempt.
    pub tls_mode: TlsMode,
    /// Exact server version supplied by the caller.
    ///
    /// When set, the HTTP probe is skipped, but `tls_mode` still applies to
    /// the selected gRPC driver.
    pub server_version: Option<core_version::Version>,
}

impl Default for SecureConnectOptions {
    fn default() -> Self {
        Self {
            http_port: DEFAULT_HTTP_PORT,
            tls_mode: TlsMode::Disabled,
            server_version: None,
        }
    }
}

impl SecureConnectOptions {
    /// Validate and lower this transport policy without constructing a host.
    ///
    /// This runs the exact shared custom-root and compiled-driver-band
    /// validation used by every secure connect/lifecycle entry point, then
    /// discards the provider options. It performs no HTTP request, gRPC host
    /// construction, credential use, or database operation, so orchestration
    /// layers can fail malformed trust material before resolving secrets.
    pub fn validate_transport(&self) -> SecureResult<()> {
        self.prepare_transport().map(|_| ())
    }

    /// Resolve and retain this transport policy for several lifecycle calls.
    ///
    /// This hidden additive seam exists for orchestrators that must finish all
    /// trust-material I/O before resolving credentials. Clones share the same
    /// captured custom-root snapshot and already-lowered band configurations;
    /// they never reopen [`TlsMode::CustomRootCa`]'s configured path.
    #[doc(hidden)]
    pub fn prepare_transport(&self) -> SecureResult<PreparedSecureConnectOptions> {
        Ok(PreparedSecureConnectOptions {
            http_port: self.http_port,
            server_version: self.server_version,
            resolved_tls: ResolvedTlsMode::from_configured_path(self.tls_mode.clone())?,
        })
    }

    /// Prepare a path that already carries workspace/server confinement
    /// provenance without following any component alias.
    ///
    /// Only confinement authorities may use this hidden entry point. Ordinary
    /// callers must use [`Self::prepare_transport`], which accepts normal OS
    /// path aliases before capturing the physical file.
    #[doc(hidden)]
    pub fn prepare_transport_from_validated_physical_path(
        &self,
    ) -> SecureResult<PreparedSecureConnectOptions> {
        Ok(PreparedSecureConnectOptions {
            http_port: self.http_port,
            server_version: self.server_version,
            resolved_tls: ResolvedTlsMode::from_validated_physical_path(self.tls_mode.clone())?,
        })
    }

    /// Prepare custom-root bytes already captured through a configuration
    /// authority without reopening the diagnostic [`TlsMode`] path.
    ///
    /// Only confinement authorities may use this hidden entry point. The
    /// captured bytes are still parsed and lowered by the shared TLS engine,
    /// and every driver band receives the resulting private snapshot.
    #[doc(hidden)]
    pub fn prepare_transport_from_captured_custom_root(
        &self,
        bytes: Arc<[u8]>,
    ) -> SecureResult<PreparedSecureConnectOptions> {
        Ok(PreparedSecureConnectOptions {
            http_port: self.http_port,
            server_version: self.server_version,
            resolved_tls: ResolvedTlsMode::from_captured_custom_root(self.tls_mode.clone(), bytes)?,
        })
    }
}

impl From<ConnectOptions> for SecureConnectOptions {
    fn from(value: ConnectOptions) -> Self {
        Self {
            http_port: value.http_port,
            tls_mode: if value.tls {
                TlsMode::NativeRoots
            } else {
                TlsMode::Disabled
            },
            server_version: value.server_version,
        }
    }
}

/// Error returned by typed secure connection and lifecycle entry points.
///
/// This additive type keeps TLS configuration failures matchable without
/// adding a variant to the released, exhaustively matchable [`RuntimeError`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SecureConnectError {
    /// Custom or native trust material failed validation before network I/O.
    #[error(transparent)]
    TlsConfiguration(#[from] core_version::TlsConfigurationError),
    /// A driver band could not lower the already validated TLS policy.
    #[error(
        "TLS configuration error [tls_driver_lowering_failed]: TypeDB driver band {band} rejected the TLS policy"
    )]
    DriverTlsConfiguration {
        /// Driver protocol band that rejected the configuration.
        band: u8,
    },
    /// The connection reached the ordinary version gate or driver runtime.
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}

impl SecureConnectError {
    /// Return the stable TLS configuration code, or `None` for an ordinary
    /// version/connection/runtime failure.
    #[must_use]
    pub fn configuration_code(&self) -> Option<&'static str> {
        match self {
            Self::TlsConfiguration(error) => Some(error.code()),
            Self::DriverTlsConfiguration { .. } => Some("tls_driver_lowering_failed"),
            Self::Runtime(_) => None,
        }
    }

    /// Return a diagnostic that is safe to expose after credentials have
    /// been resolved.
    ///
    /// TLS diagnostics are reconstructed from typed codes without retaining
    /// configured paths. Version diagnostics are retained only for variants
    /// whose fields are closed version/band data. Probe, parse, connection,
    /// query, and transaction failures may contain provider-controlled text
    /// and therefore return `None`.
    #[must_use]
    pub fn credential_safe_diagnostic(&self) -> Option<String> {
        match self {
            Self::TlsConfiguration(error) => Some(format!(
                "TLS policy preparation failed [{}]; inspect the configured trust material",
                error.code()
            )),
            Self::DriverTlsConfiguration { band } => Some(format!(
                "TLS policy lowering failed [tls_driver_lowering_failed] for TypeDB driver band {band}"
            )),
            Self::Runtime(RuntimeError::UnsupportedVersion(
                error @ (core_version::VersionError::Unsupported { .. }
                | core_version::VersionError::BandMismatch { .. }
                | core_version::VersionError::EmbeddedUnavailable { .. }
                | core_version::VersionError::FeatureUnsupported { .. }),
            )) => Some(format!("Unsupported version: {error}")),
            Self::Runtime(
                RuntimeError::UnsupportedVersion(
                    core_version::VersionError::Probe(_) | core_version::VersionError::Parse(_),
                )
                | RuntimeError::Connection(_)
                | RuntimeError::QueryExecution(_)
                | RuntimeError::Transaction(_)
                | RuntimeError::ResourceLimit { .. }
                | RuntimeError::AnswerConsumer,
            ) => None,
        }
    }

    /// Collapse an additive secure error onto the released runtime surface.
    #[must_use]
    pub fn into_runtime_error(self) -> RuntimeError {
        match self {
            Self::Runtime(error) => error,
            other => RuntimeError::Connection(other.to_string()),
        }
    }
}

const TRACE_CODE_DRIVER_DROP_CLOSE_FAILED: &str = "typedb_runtime_driver_drop_close_failed";
const TRACE_CODE_VERSION_GATE_PASSED: &str = "typedb_runtime_version_gate_passed";
#[cfg(all(feature = "band8", feature = "band9"))]
const TRACE_CODE_BAND9_UPGRADE_SUCCEEDED: &str = "typedb_runtime_band9_upgrade_succeeded";
#[cfg(all(feature = "band8", feature = "band9"))]
const TRACE_CODE_BAND9_UPGRADE_FAILED: &str = "typedb_runtime_band9_upgrade_failed";
#[cfg(feature = "band8")]
const TRACE_CODE_BAND8_FALLBACK_CONNECTED: &str = "typedb_runtime_band8_fallback_connected";
#[cfg(feature = "band7")]
const TRACE_CODE_BAND7_FALLBACK_CONNECTED: &str = "typedb_runtime_band7_fallback_connected";
const TRACE_CODE_CONNECTED: &str = "typedb_runtime_connected";

fn runtime_trace_failure_code(error: &RuntimeError) -> &'static str {
    match error {
        RuntimeError::UnsupportedVersion(_) => "typedb_runtime_unsupported_version",
        RuntimeError::Connection(_) => "typedb_runtime_connection_failed",
        RuntimeError::QueryExecution(_) => "typedb_runtime_query_failed",
        RuntimeError::Transaction(_) => "typedb_runtime_transaction_failed",
        RuntimeError::ResourceLimit { .. } => "typedb_runtime_resource_limit",
        RuntimeError::AnswerConsumer => "typedb_runtime_answer_consumer_failed",
    }
}

#[cfg(all(feature = "band8", feature = "band9"))]
fn secure_trace_failure_code(error: &SecureConnectError) -> &'static str {
    match error {
        SecureConnectError::TlsConfiguration(error) => error.code(),
        SecureConnectError::DriverTlsConfiguration { .. } => "tls_driver_lowering_failed",
        SecureConnectError::Runtime(error) => runtime_trace_failure_code(error),
    }
}

fn trace_driver_drop_close_failure(error: &RuntimeError) {
    tracing::warn!(
        code = TRACE_CODE_DRIVER_DROP_CLOSE_FAILED,
        failure_code = runtime_trace_failure_code(error),
        "TypeDB driver cleanup failed during final runtime drop"
    );
}

fn trace_version_gate_passed(_address: &str, band: u8, server_version: core_version::Version) {
    tracing::debug!(
        code = TRACE_CODE_VERSION_GATE_PASSED,
        driver_band = band,
        %server_version,
        "TypeDB runtime version gate passed"
    );
}

#[cfg(all(feature = "band8", feature = "band9"))]
fn trace_band9_upgrade_succeeded(_address: &str, server_version: core_version::Version) {
    tracing::debug!(
        code = TRACE_CODE_BAND9_UPGRADE_SUCCEEDED,
        driver_band = 9_u8,
        %server_version,
        "TypeDB runtime upgraded its validated fallback connection"
    );
}

#[cfg(all(feature = "band8", feature = "band9"))]
fn trace_band9_upgrade_failed(
    _address: &str,
    server_version: core_version::Version,
    error: &SecureConnectError,
) {
    tracing::warn!(
        code = TRACE_CODE_BAND9_UPGRADE_FAILED,
        failure_code = secure_trace_failure_code(error),
        driver_band = 9_u8,
        %server_version,
        "TypeDB runtime could not upgrade its validated fallback connection"
    );
}

#[cfg(feature = "band8")]
fn trace_band8_fallback_connected(_address: &str, server_version: core_version::Version) {
    tracing::debug!(
        code = TRACE_CODE_BAND8_FALLBACK_CONNECTED,
        driver_band = 8_u8,
        %server_version,
        "TypeDB runtime retained its validated fallback connection"
    );
}

#[cfg(feature = "band7")]
fn trace_band7_fallback_connected(_address: &str) {
    tracing::warn!(
        code = TRACE_CODE_BAND7_FALLBACK_CONNECTED,
        driver_band = 7_u8,
        server_version_known = false,
        "TypeDB runtime connected through the legacy fallback; configure an exact server version for strict validation"
    );
}

fn trace_runtime_connected(
    _address: &str,
    driver_band: u8,
    server_version: Option<core_version::Version>,
) {
    match server_version {
        Some(server_version) => tracing::info!(
            code = TRACE_CODE_CONNECTED,
            driver_band,
            server_version_known = true,
            %server_version,
            "TypeDB runtime connected"
        ),
        None => tracing::info!(
            code = TRACE_CODE_CONNECTED,
            driver_band,
            server_version_known = false,
            "TypeDB runtime connected"
        ),
    }
}

/// Result alias for typed secure connection and lifecycle operations.
pub type SecureResult<T> = std::result::Result<T, SecureConnectError>;

/// TLS policy lowered eagerly for every compiled TypeDB driver band.
///
/// Constructing this value reads and validates a custom root exactly once
/// before the HTTP probe or any gRPC host is created. Every band is then
/// lowered from the retained file material, and HTTP receives the captured
/// bytes rather than the caller-controlled path. Every fallback receives this
/// same value, so an enabled request has no plaintext reconstruction path.
#[derive(Clone, Debug)]
struct ResolvedTlsMode {
    probe_mode: ResolvedTlsProbeMode,
    #[cfg(feature = "band7")]
    band7: B7DriverOptions,
    #[cfg(feature = "band8")]
    band8: DriverTlsConfig,
    #[cfg(feature = "band9")]
    band9: B9DriverTlsConfig,
}

#[derive(Clone, Debug)]
enum ResolvedTlsProbeMode {
    Disabled,
    NativeRoots,
    CustomRootCa(core_version::RetainedCustomRootCa),
}

/// A reusable, fully resolved secure transport policy.
///
/// This type is doc-hidden because ordinary callers should keep using
/// [`SecureConnectOptions`]. It is public only so orchestration crates can
/// prepare transport material once before credential resolution and reuse the
/// same immutable snapshot across database lifecycle and connection calls.
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct PreparedSecureConnectOptions {
    http_port: u16,
    server_version: Option<core_version::Version>,
    resolved_tls: ResolvedTlsMode,
}

impl ResolvedTlsProbeMode {
    #[cfg(any(test, feature = "band7"))]
    const fn is_enabled(&self) -> bool {
        !matches!(self, Self::Disabled)
    }
}

impl ResolvedTlsMode {
    fn from_captured_custom_root(mode: TlsMode, bytes: Arc<[u8]>) -> SecureResult<Self> {
        let material = match &mode {
            TlsMode::CustomRootCa(path) => {
                core_version::RetainedCustomRootCa::load_captured_bytes(path, bytes)?
            }
            TlsMode::Disabled | TlsMode::NativeRoots => {
                return Err(core_version::TlsConfigurationError::ClientConfiguration.into());
            }
        };
        Self::lower(mode, ResolvedTlsProbeMode::CustomRootCa(material))
    }

    fn from_validated_physical_path(mode: TlsMode) -> SecureResult<Self> {
        let probe_mode = match &mode {
            TlsMode::Disabled => ResolvedTlsProbeMode::Disabled,
            TlsMode::NativeRoots => ResolvedTlsProbeMode::NativeRoots,
            TlsMode::CustomRootCa(path) => {
                ResolvedTlsProbeMode::CustomRootCa(core_version::RetainedCustomRootCa::load(path)?)
            }
        };
        Self::lower(mode, probe_mode)
    }

    fn from_configured_path(mode: TlsMode) -> SecureResult<Self> {
        let probe_mode = match &mode {
            TlsMode::Disabled => ResolvedTlsProbeMode::Disabled,
            TlsMode::NativeRoots => ResolvedTlsProbeMode::NativeRoots,
            TlsMode::CustomRootCa(path) => ResolvedTlsProbeMode::CustomRootCa(
                core_version::RetainedCustomRootCa::load_configured_alias(path)?,
            ),
        };
        Self::lower(mode, probe_mode)
    }

    fn lower(mode: TlsMode, probe_mode: ResolvedTlsProbeMode) -> SecureResult<Self> {
        let custom_root = match &probe_mode {
            ResolvedTlsProbeMode::CustomRootCa(material) => Some(material),
            ResolvedTlsProbeMode::Disabled | ResolvedTlsProbeMode::NativeRoots => None,
        };

        if matches!(mode, TlsMode::CustomRootCa(_)) != custom_root.is_some() {
            return Err(core_version::TlsConfigurationError::ClientConfiguration.into());
        }

        #[cfg(feature = "band7")]
        let band7 = match &mode {
            TlsMode::Disabled => B7DriverOptions::new(false, None),
            TlsMode::NativeRoots => B7DriverOptions::new(true, None),
            TlsMode::CustomRootCa(_) => custom_root
                .ok_or(core_version::TlsConfigurationError::ClientConfiguration)?
                .with_driver_root_path(|path| B7DriverOptions::new(true, Some(path)))?,
        }
        .map_err(|_| SecureConnectError::DriverTlsConfiguration { band: 7 })?;

        #[cfg(feature = "band8")]
        let band8 = match &mode {
            TlsMode::Disabled => DriverTlsConfig::disabled(),
            TlsMode::NativeRoots => DriverTlsConfig::enabled_with_native_root_ca(),
            TlsMode::CustomRootCa(_) => custom_root
                .ok_or(core_version::TlsConfigurationError::ClientConfiguration)?
                .with_driver_root_path(DriverTlsConfig::enabled_with_root_ca)?
                .map_err(|_| SecureConnectError::DriverTlsConfiguration { band: 8 })?,
        };

        #[cfg(feature = "band9")]
        let band9 = match &mode {
            TlsMode::Disabled => B9DriverTlsConfig::disabled(),
            TlsMode::NativeRoots => B9DriverTlsConfig::enabled_with_native_root_ca(),
            TlsMode::CustomRootCa(_) => custom_root
                .ok_or(core_version::TlsConfigurationError::ClientConfiguration)?
                .with_driver_root_path(B9DriverTlsConfig::enabled_with_root_ca)?
                .map_err(|_| SecureConnectError::DriverTlsConfiguration { band: 9 })?,
        };

        Ok(Self {
            probe_mode,
            #[cfg(feature = "band7")]
            band7,
            #[cfg(feature = "band8")]
            band8,
            #[cfg(feature = "band9")]
            band9,
        })
    }
}

/// Band-tagged driver state. Crate-private; no driver type escapes the crate.
enum DriverHandleInner {
    #[cfg(feature = "band7")]
    B7(B7Driver),
    #[cfg(feature = "band8")]
    B8(B8Driver),
    #[cfg(feature = "band9")]
    B9(B9Driver),
}

fn contain_driver_shutdown(shutdown: impl FnOnce() -> Result<()>) -> Result<()> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(shutdown)) {
        Ok(result) => result,
        Err(_) => Err(RuntimeError::Connection(
            "Driver close failed: upstream driver panicked during shutdown".to_owned(),
        )),
    }
}

fn run_driver_shutdown(
    state: &AtomicU8,
    close_lock: &Mutex<()>,
    shutdown: impl FnOnce() -> Result<()>,
) -> Result<()> {
    // Serialize explicit binding close, cleanup retries, and final-lease drop.
    // Recovering a poisoned guard is preferable to leaking the driver's
    // background runtime if an unrelated close caller previously panicked.
    let _close_guard = match close_lock.lock() {
        Ok(close_guard) => close_guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    match state.load(AtomicOrdering::Acquire) {
        DRIVER_CLOSED => return Ok(()),
        DRIVER_OPEN => {
            // Publish the monotonic transition before invoking upstream
            // shutdown. New operations now fail immediately; active
            // operations are cancelled rather than drained, because waiting
            // for them can recreate the terminal-close liveness defect
            // tracked by #196.
            state.store(DRIVER_CLOSING, AtomicOrdering::Release);
        }
        DRIVER_CLOSING => {
            // A prior shutdown attempt failed or panicked. Keep admission
            // terminal, but retry cleanup: upstream 3.12.1 can return before
            // closing its background runtime when server cleanup fails.
        }
        _ => unreachable!("driver shutdown state is internal and validated"),
    }

    let result = contain_driver_shutdown(shutdown);
    if result.is_ok() {
        state.store(DRIVER_CLOSED, AtomicOrdering::Release);
    }
    result
}

/// Shared driver lifetime retained by every open transaction.
///
/// Automatic shutdown happens only when the final runtime/transaction lease is
/// released. Explicit connection close still calls [`Self::force_close`]
/// immediately. This distinction preserves the released behavior that a
/// transaction can outlive the database handle that opened it.
pub(crate) struct DriverHandle {
    inner: DriverHandleInner,
    state: AtomicU8,
    close_lock: Mutex<()>,
}

const DRIVER_OPEN: u8 = 0;
const DRIVER_CLOSING: u8 = 1;
const DRIVER_CLOSED: u8 = 2;

impl DriverHandle {
    #[cfg(feature = "band7")]
    fn band7(driver: B7Driver) -> Self {
        Self {
            inner: DriverHandleInner::B7(driver),
            state: AtomicU8::new(DRIVER_OPEN),
            close_lock: Mutex::new(()),
        }
    }

    #[cfg(feature = "band8")]
    fn band8(driver: B8Driver) -> Self {
        Self {
            inner: DriverHandleInner::B8(driver),
            state: AtomicU8::new(DRIVER_OPEN),
            close_lock: Mutex::new(()),
        }
    }

    #[cfg(feature = "band9")]
    fn band9(driver: B9Driver) -> Self {
        Self {
            inner: DriverHandleInner::B9(driver),
            state: AtomicU8::new(DRIVER_OPEN),
            close_lock: Mutex::new(()),
        }
    }

    fn inner(&self) -> &DriverHandleInner {
        &self.inner
    }

    fn band(&self) -> u8 {
        match &self.inner {
            #[cfg(feature = "band7")]
            DriverHandleInner::B7(_) => 7,
            #[cfg(feature = "band8")]
            DriverHandleInner::B8(_) => 8,
            #[cfg(feature = "band9")]
            DriverHandleInner::B9(_) => 9,
        }
    }

    fn ensure_open(&self) -> Result<()> {
        if self.state.load(AtomicOrdering::Acquire) == DRIVER_OPEN {
            Ok(())
        } else {
            Err(RuntimeError::Connection(
                "TypeDB driver connection is closed".to_owned(),
            ))
        }
    }

    fn shutdown_started(&self) -> bool {
        self.state.load(AtomicOrdering::Acquire) != DRIVER_OPEN
    }

    /// Make the selected driver terminal and dispatch upstream shutdown.
    ///
    /// A failed attempt leaves admission closed and permits a later cleanup
    /// retry; after successful dispatch, subsequent calls are no-ops. The
    /// upstream callback worker is joined only when the final driver lease is
    /// dropped.
    fn force_close(&self) -> Result<()> {
        run_driver_shutdown(&self.state, &self.close_lock, || match &self.inner {
            #[cfg(feature = "band7")]
            DriverHandleInner::B7(driver) => driver
                .force_close()
                .map_err(|error| RuntimeError::Connection(format!("Driver close failed: {error}"))),
            #[cfg(feature = "band8")]
            DriverHandleInner::B8(driver) => driver
                .force_close()
                .map_err(|error| RuntimeError::Connection(format!("Driver close failed: {error}"))),
            #[cfg(feature = "band9")]
            DriverHandleInner::B9(driver) => driver
                .force_close()
                .map_err(|error| RuntimeError::Connection(format!("Driver close failed: {error}"))),
        })
    }
}

impl Drop for DriverHandle {
    fn drop(&mut self) {
        // Temporary downstream lifecycle workaround and removal gate:
        // https://github.com/ds1sqe/type-bridge/issues/196
        //
        // The official driver exposes explicit shutdown because ordinary field
        // drop can leave background transaction workers alive long enough to
        // poison later driver lifecycles. The final shared lease closes while
        // every driver field is intact, without invalidating a transaction that
        // outlives its originating database handle.
        if let Err(error) = self.force_close() {
            trace_driver_drop_close_failure(&error);
        }
    }
}

/// Real TypeDB backend wrapping a band-tagged [`DriverHandle`].
pub struct TypeDBRuntime {
    driver: Arc<DriverHandle>,
    /// The server version the connect gate detected, when it could.
    ///
    /// `Some` on the exact-version, HTTP-probe, and band-8 gRPC-fallback
    /// paths; `None` only on the band-7 gRPC fallback, where the server
    /// does not report its version.
    server_version: Option<core_version::Version>,
}

impl TypeDBRuntime {
    /// Make the selected driver terminal and dispatch upstream shutdown.
    ///
    /// Bindings call this at their public connection-close boundary instead of
    /// relying on field drop ordering. The first call atomically prevents new
    /// operations, invalidates in-flight work through upstream shutdown, and
    /// leaves the runtime permanently unavailable even if shutdown reports an
    /// error. A later call retries incomplete upstream dispatch without
    /// reopening the connection; calls after successful dispatch are harmless.
    /// Final callback-worker release remains lease-aware and occurs when the
    /// last driver handle drops.
    pub fn force_close(&self) -> Result<()> {
        self.driver.force_close()
    }
}

/// Released Boolean-adapter gate with an injectable probe.
#[cfg(test)]
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
    gated_driver_secure_with_probe(
        address,
        username,
        password,
        options.into(),
        move |probe_address, http_port, tls_mode| {
            probe(probe_address, http_port, tls_mode.is_enabled())
                .map_err(core_version::VersionProbeError::Probe)
        },
    )
    .await
    .map_err(SecureConnectError::into_runtime_error)
}

/// Version-gated secure driver constructor with an injectable typed probe.
///
/// Custom roots and all compiled driver TLS configurations are validated
/// before the exact-version fast path, HTTP probe, or any gRPC construction.
/// If the HTTP request itself fails, the same [`ResolvedTlsMode`] is retained
/// by every fallback and upgrade attempt.  TLS configuration failures are
/// terminal and never trigger a transport fallback.
async fn gated_driver_secure_with_probe<F>(
    address: &str,
    username: &str,
    password: &str,
    options: SecureConnectOptions,
    probe: F,
) -> SecureResult<(DriverHandle, Option<core_version::Version>)>
where
    F: FnOnce(
            &str,
            u16,
            &ResolvedTlsProbeMode,
        )
            -> std::result::Result<core_version::Version, core_version::VersionProbeError>
        + Send
        + 'static,
{
    let prepared = options.prepare_transport()?;
    gated_driver_prepared_with_probe(address, username, password, prepared, probe).await
}

async fn gated_driver_prepared_with_probe<F>(
    address: &str,
    username: &str,
    password: &str,
    options: PreparedSecureConnectOptions,
    probe: F,
) -> SecureResult<(DriverHandle, Option<core_version::Version>)>
where
    F: FnOnce(
            &str,
            u16,
            &ResolvedTlsProbeMode,
        )
            -> std::result::Result<core_version::Version, core_version::VersionProbeError>
        + Send
        + 'static,
{
    let PreparedSecureConnectOptions {
        http_port,
        server_version,
        resolved_tls,
    } = options;

    if let Some(server_version) = server_version {
        let driver =
            driver_for_server_version(address, username, password, server_version, &resolved_tls)
                .await?;
        return Ok((driver, Some(server_version)));
    }

    // Probe the server version over HTTP (blocking I/O; offload to a dedicated
    // thread so we don't block the async executor).
    let address_owned = address.to_string();
    let probe_mode = resolved_tls.probe_mode.clone();
    let probe_result =
        tokio::task::spawn_blocking(move || probe(&address_owned, http_port, &probe_mode))
            .await
            .map_err(|e| RuntimeError::Connection(format!("Version probe task panicked: {e}")))?;

    match probe_result {
        Ok(server_version) => {
            let driver = driver_for_server_version(
                address,
                username,
                password,
                server_version,
                &resolved_tls,
            )
            .await?;
            Ok((driver, Some(server_version)))
        }
        Err(core_version::VersionProbeError::Probe(http_error)) => {
            grpc_fallback_driver(address, username, password, http_error, &resolved_tls).await
        }
        Err(core_version::VersionProbeError::TlsConfiguration(error)) => {
            Err(SecureConnectError::TlsConfiguration(error))
        }
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
    tls: &ResolvedTlsMode,
) -> SecureResult<DriverHandle> {
    let band = validate_server_band(&server_version)?;

    trace_version_gate_passed(address, band, server_version);

    #[cfg(feature = "band7")]
    if band == 7 {
        return connect_band7_driver(address, username, password, tls)
            .await
            .map(DriverHandle::band7);
    }

    #[cfg(feature = "band8")]
    if band == 8 {
        return connect_band8_driver(address, username, password, tls)
            .await
            .map(DriverHandle::band8);
    }

    #[cfg(feature = "band9")]
    if band == 9 {
        return connect_band9_driver(address, username, password, tls)
            .await
            .map(DriverHandle::band9);
    }

    // Unreachable by invariant: the gate already rejected any server whose
    // accepted-band set does not intersect EMBEDDED_BANDS, and negotiation
    // only yields embedded bands, each of which returned above.  The arm
    // exists only to keep the tail total; it is never entered.
    Err(RuntimeError::Connection(format!(
        "No compiled driver band supports the negotiated server band ({band})"
    ))
    .into())
}

#[cfg(feature = "band7")]
async fn connect_band7_driver(
    address: &str,
    username: &str,
    password: &str,
    tls: &ResolvedTlsMode,
) -> SecureResult<B7Driver> {
    // The protocol-7 driver predates the typed TLS configuration used by the
    // later drivers and validates TLS twice: in DriverOptions and in the URI
    // scheme. Public TypeBridge addresses remain scheme-free, so lower the
    // already-resolved secure policy onto the URI only at this band boundary.
    // Explicit caller schemes are preserved so contradictions still fail in
    // the driver rather than being silently rewritten.
    let driver_address = band7_driver_address(address, tls.probe_mode.is_enabled());
    B7Driver::new(
        driver_address.as_ref(),
        B7Credentials::new(username, password),
        tls.band7.clone(),
    )
    .await
    .map_err(|e| RuntimeError::Connection(format!("Failed to connect to {address}: {e}")).into())
}

#[cfg(feature = "band7")]
fn band7_driver_address(address: &str, tls_enabled: bool) -> std::borrow::Cow<'_, str> {
    if tls_enabled && !address.contains("://") {
        format!("https://{address}").into()
    } else {
        address.into()
    }
}

#[cfg(feature = "band8")]
async fn connect_band8_driver(
    address: &str,
    username: &str,
    password: &str,
    tls: &ResolvedTlsMode,
) -> SecureResult<B8Driver> {
    let addresses = Addresses::try_from_address_str(address)
        .map_err(|e| RuntimeError::Connection(format!("Invalid TypeDB address {address}: {e}")))?;
    B8Driver::new(
        addresses,
        B8Credentials::new(username, password),
        DriverOptions::new(tls.band8.clone()),
    )
    .await
    .map_err(|e| RuntimeError::Connection(format!("Failed to connect to {address}: {e}")).into())
}

#[cfg(feature = "band9")]
async fn connect_band9_driver(
    address: &str,
    username: &str,
    password: &str,
    tls: &ResolvedTlsMode,
) -> SecureResult<B9Driver> {
    let addresses = B9Addresses::try_from_address_str(address)
        .map_err(|e| RuntimeError::Connection(format!("Invalid TypeDB address {address}: {e}")))?;
    B9Driver::new(
        addresses,
        B9Credentials::new(username, password),
        B9DriverOptions::new(tls.band9.clone()),
    )
    .await
    .map_err(|e| RuntimeError::Connection(format!("Failed to connect to {address}: {e}")).into())
}

async fn grpc_fallback_driver(
    address: &str,
    username: &str,
    password: &str,
    http_error: core_version::VersionError,
    tls: &ResolvedTlsMode,
) -> SecureResult<(DriverHandle, Option<core_version::Version>)> {
    #[cfg(not(any(feature = "band7", feature = "band8")))]
    let _ = (address, username, password, tls);
    let mut failures = vec![format!("HTTP version probe failed: {http_error}")];

    // Band 9 is deliberately NOT probed blindly: a band-9 connection attempt
    // against a 3.11 server crashes the server outright (measured live on
    // 3.11.5).  The fallback therefore discovers the server through band 8 —
    // accepted by every band-{8,9} server — and only upgrades to the native
    // band-9 protocol once the reported version proves the server accepts it.
    #[cfg(feature = "band8")]
    {
        match connect_band8_driver(address, username, password, tls).await {
            Ok(driver) => {
                // Wrap the discovery candidate immediately so every error,
                // retry, and native-band upgrade path deterministically closes
                // it rather than relying on the upstream field-drop path.
                let driver = DriverHandle::band8(driver);
                let reported = match driver.inner() {
                    DriverHandleInner::B8(driver) => driver
                        .server_version()
                        .await
                        .map(|reported| reported.version().to_owned())
                        .map_err(|error| error.to_string()),
                    #[cfg(any(feature = "band7", feature = "band9"))]
                    _ => {
                        return Err(RuntimeError::Connection(
                            "Internal TypeDB driver-band mismatch".to_owned(),
                        )
                        .into());
                    }
                };
                let classification = match classify_band8_grpc_version(address, reported) {
                    Ok(classification) => classification,
                    Err(error) => {
                        driver.force_close().map_err(SecureConnectError::Runtime)?;
                        return Err(SecureConnectError::Runtime(error));
                    }
                };
                match classification {
                    Band8GrpcVersion::Validated(server_version) => {
                        // Prefer the server's native band when this build embeds
                        // it. The authoritative band-8 version round trip makes a
                        // band-9 attempt safe; probing band 9 before this point can
                        // crash a 3.11 server.
                        #[cfg(feature = "band9")]
                        if core_version::negotiate_server_band(&server_version, EMBEDDED_BANDS)
                            == Some(9)
                        {
                            match connect_band9_driver(address, username, password, tls).await {
                                Ok(b9_driver) => {
                                    let b9_driver = DriverHandle::band9(b9_driver);
                                    driver.force_close().map_err(SecureConnectError::Runtime)?;
                                    trace_band9_upgrade_succeeded(address, server_version);
                                    return Ok((b9_driver, Some(server_version)));
                                }
                                Err(error) => {
                                    trace_band9_upgrade_failed(address, server_version, &error);
                                }
                            }
                        }
                        trace_band8_fallback_connected(address, server_version);
                        return Ok((driver, Some(server_version)));
                    }
                    Band8GrpcVersion::RetryableFailure(failure) => {
                        driver.force_close().map_err(SecureConnectError::Runtime)?;
                        failures.push(failure);
                    }
                }
            }
            Err(error) => failures.push(format!("band-8 gRPC attempt failed: {error}")),
        }
    }

    #[cfg(not(feature = "band8"))]
    failures.push("band-8 gRPC attempt skipped: band8 feature is not compiled in".to_string());

    #[cfg(feature = "band7")]
    {
        match connect_band7_driver(address, username, password, tls).await {
            Ok(driver) => {
                trace_band7_fallback_connected(address);
                return Ok((DriverHandle::band7(driver), None));
            }
            Err(error) => failures.push(format!("band-7 gRPC attempt failed: {error}")),
        }
    }

    #[cfg(not(feature = "band7"))]
    failures.push("band-7 gRPC attempt skipped: band7 feature is not compiled in".to_string());

    Err(
        RuntimeError::UnsupportedVersion(core_version::VersionError::Probe(format!(
            "HTTP version probe and gRPC fallback both failed: {}",
            failures.join("; ")
        )))
        .into(),
    )
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

fn probe_server_version_for_tls_mode(
    address: &str,
    http_port: u16,
    tls_mode: &ResolvedTlsProbeMode,
) -> std::result::Result<core_version::Version, core_version::VersionProbeError> {
    match tls_mode {
        ResolvedTlsProbeMode::Disabled => {
            core_version::server_version_plaintext(address, http_port)
        }
        ResolvedTlsProbeMode::NativeRoots => {
            core_version::server_version_native_roots(address, http_port)
        }
        ResolvedTlsProbeMode::CustomRootCa(material) => {
            core_version::server_version_retained_custom_root_ca(address, http_port, material)
        }
    }
}

/// Version-gated released [`ConnectOptions`] adapter.
async fn gated_driver(
    address: &str,
    username: &str,
    password: &str,
    options: ConnectOptions,
) -> Result<(DriverHandle, Option<core_version::Version>)> {
    gated_driver_secure(address, username, password, options.into())
        .await
        .map_err(SecureConnectError::into_runtime_error)
}

/// Version-gated typed TLS constructor.
async fn gated_driver_secure(
    address: &str,
    username: &str,
    password: &str,
    options: SecureConnectOptions,
) -> SecureResult<(DriverHandle, Option<core_version::Version>)> {
    gated_driver_secure_with_probe(
        address,
        username,
        password,
        options,
        probe_server_version_for_tls_mode,
    )
    .await
}

async fn gated_driver_prepared_secure(
    address: &str,
    username: &str,
    password: &str,
    options: PreparedSecureConnectOptions,
) -> SecureResult<(DriverHandle, Option<core_version::Version>)> {
    gated_driver_prepared_with_probe(
        address,
        username,
        password,
        options,
        probe_server_version_for_tls_mode,
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
        trace_runtime_connected(address, driver.band(), server_version);
        Ok(Self {
            driver: Arc::new(driver),
            server_version,
        })
    }

    /// Connect using an explicit typed TLS policy.
    ///
    /// Custom trust material is validated before the HTTP version probe or
    /// any gRPC host is constructed.  An enabled mode remains enabled through
    /// HTTP failure, driver-band discovery, and native-band upgrade.
    pub async fn connect_secure(
        address: &str,
        username: &str,
        password: &str,
        options: SecureConnectOptions,
    ) -> SecureResult<Self> {
        let (driver, server_version) =
            gated_driver_secure(address, username, password, options).await?;
        trace_runtime_connected(address, driver.band(), server_version);
        Ok(Self {
            driver: Arc::new(driver),
            server_version,
        })
    }

    /// Connect with a transport policy that was already fully resolved.
    ///
    /// This hidden orchestration seam performs no trust-material path I/O;
    /// clones of `options` share the exact prepared custom-root snapshot.
    #[doc(hidden)]
    pub async fn connect_prepared_secure(
        address: &str,
        username: &str,
        password: &str,
        options: PreparedSecureConnectOptions,
    ) -> SecureResult<Self> {
        let (driver, server_version) =
            gated_driver_prepared_secure(address, username, password, options).await?;
        trace_runtime_connected(address, driver.band(), server_version);
        Ok(Self {
            driver: Arc::new(driver),
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
        match self.driver.inner() {
            #[cfg(feature = "band7")]
            DriverHandleInner::B7(_) => false,
            #[cfg(feature = "band8")]
            DriverHandleInner::B8(_) => false,
            #[cfg(feature = "band9")]
            DriverHandleInner::B9(_) => true,
        }
    }
}

fn runtime_error_diagnostic(error: RuntimeError) -> String {
    match error {
        RuntimeError::Connection(message) => message,
        other => other.to_string(),
    }
}

fn finish_one_shot_database_operation<T>(operation: Result<T>, close: Result<()>) -> Result<T> {
    match (operation, close) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(close_error)) => Err(close_error),
        (Err(operation_error), Ok(())) => Err(operation_error),
        (Err(operation_error), Err(close_error)) => Err(RuntimeError::Connection(format!(
            "{}; additionally, TypeDB driver cleanup failed: {}",
            runtime_error_diagnostic(operation_error),
            runtime_error_diagnostic(close_error),
        ))),
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
    ensure_database_exists_secure(address, database, username, password, options.into())
        .await
        .map_err(SecureConnectError::into_runtime_error)
}

/// Ensure a TypeDB database exists using an explicit typed TLS policy.
///
/// The same resolved policy is used for version discovery, gRPC fallback, and
/// the database lookup/create operations on the resulting driver.
pub async fn ensure_database_exists_secure(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
    options: SecureConnectOptions,
) -> SecureResult<()> {
    ensure_database_exists_prepared_secure(
        address,
        database,
        username,
        password,
        options.prepare_transport()?,
    )
    .await
}

/// Ensure a database exists using an already resolved transport snapshot.
#[doc(hidden)]
pub async fn ensure_database_exists_prepared_secure(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
    options: PreparedSecureConnectOptions,
) -> SecureResult<()> {
    let (driver, server_version) =
        gated_driver_prepared_secure(address, username, password, options).await?;
    let runtime = TypeDBRuntime {
        driver: Arc::new(driver),
        server_version,
    };
    let operation = async {
        if !runtime.database_exists(database).await? {
            runtime.create_database(database).await?;
        }
        Ok(())
    }
    .await;
    let close = runtime.force_close();
    finish_one_shot_database_operation(operation, close).map_err(SecureConnectError::Runtime)
}

/// Check whether a TypeDB database exists without creating it.
///
/// Same version gating as [`ensure_database_exists`]; the lookup itself is
/// read-only and never mutates server state.
pub async fn database_exists(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
    options: ConnectOptions,
) -> Result<bool> {
    database_exists_secure(address, database, username, password, options.into())
        .await
        .map_err(SecureConnectError::into_runtime_error)
}

/// Check whether a TypeDB database exists using an explicit typed TLS policy.
pub async fn database_exists_secure(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
    options: SecureConnectOptions,
) -> SecureResult<bool> {
    database_exists_prepared_secure(
        address,
        database,
        username,
        password,
        options.prepare_transport()?,
    )
    .await
}

/// Check for a database using an already resolved transport snapshot.
#[doc(hidden)]
pub async fn database_exists_prepared_secure(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
    options: PreparedSecureConnectOptions,
) -> SecureResult<bool> {
    let (driver, server_version) =
        gated_driver_prepared_secure(address, username, password, options).await?;
    let runtime = TypeDBRuntime {
        driver: Arc::new(driver),
        server_version,
    };
    let operation = runtime.database_exists(database).await;
    let close = runtime.force_close();
    finish_one_shot_database_operation(operation, close).map_err(SecureConnectError::Runtime)
}

/// Connect securely and delete a TypeDB database.
///
/// This lifecycle entry point exists for callers that do not otherwise need
/// to retain a [`TypeDBRuntime`].  Deleting through an already connected
/// runtime remains available as [`TypeDBRuntime::delete_database`].
pub async fn delete_database_secure(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
    options: SecureConnectOptions,
) -> SecureResult<()> {
    delete_database_prepared_secure(
        address,
        database,
        username,
        password,
        options.prepare_transport()?,
    )
    .await
}

/// Delete a database using an already resolved transport snapshot.
#[doc(hidden)]
pub async fn delete_database_prepared_secure(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
    options: PreparedSecureConnectOptions,
) -> SecureResult<()> {
    let runtime =
        TypeDBRuntime::connect_prepared_secure(address, username, password, options).await?;
    let operation = async {
        if runtime.database_exists(database).await? {
            runtime.delete_database(database).await?;
        }
        Ok(())
    }
    .await;
    let close = runtime.force_close();
    finish_one_shot_database_operation(operation, close).map_err(SecureConnectError::Runtime)
}

impl TypeDBRuntime {
    /// Open a TypeDB transaction against `database`.
    pub fn open_transaction(
        &self,
        database: &str,
        tx_type: TxType,
    ) -> BoxFuture<'_, Result<RuntimeTransaction>> {
        self.open_transaction_with_optional_timeout(database, tx_type, None)
    }

    /// Open a TypeDB transaction with an explicit server-side lifetime bound.
    ///
    /// Higher layers use this for transactions that hold schema exclusion: a
    /// lost client must not leave the server-side fence unbounded while the
    /// transport is still detecting terminal closure.
    pub fn open_transaction_with_timeout(
        &self,
        database: &str,
        tx_type: TxType,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<RuntimeTransaction>> {
        self.open_transaction_with_optional_timeout(database, tx_type, Some(timeout))
    }

    fn open_transaction_with_optional_timeout(
        &self,
        database: &str,
        tx_type: TxType,
        timeout: Option<Duration>,
    ) -> BoxFuture<'_, Result<RuntimeTransaction>> {
        let db = database.to_string();
        let driver_lease = Arc::clone(&self.driver);
        Box::pin(async move {
            driver_lease.ensure_open()?;
            match driver_lease.inner() {
                #[cfg(feature = "band7")]
                DriverHandleInner::B7(d) => {
                    let typedb_tx_type = match tx_type {
                        TxType::Read => B7TransactionType::Read,
                        TxType::Write => B7TransactionType::Write,
                        TxType::Schema => B7TransactionType::Schema,
                    };
                    let mut options = B7TransactionOptions::new();
                    if let Some(timeout) = timeout {
                        options = options
                            .transaction_timeout(timeout)
                            .schema_lock_acquire_timeout(timeout);
                    }
                    let transaction = d
                        .transaction_with_options(&db, typedb_tx_type, options)
                        .await
                        .map_err(|e| {
                            RuntimeError::Transaction(format!("Failed to open transaction: {e}"))
                        })?;
                    driver_lease.ensure_open()?;
                    Ok(RuntimeTransaction {
                        inner: RuntimeTransactionInner::B7(Some(transaction)),
                        driver_lease: Some(driver_lease),
                    })
                }
                #[cfg(feature = "band8")]
                DriverHandleInner::B8(d) => {
                    let typedb_tx_type = match tx_type {
                        TxType::Read => B8TransactionType::Read,
                        TxType::Write => B8TransactionType::Write,
                        TxType::Schema => B8TransactionType::Schema,
                    };
                    let mut options = B8TransactionOptions::new();
                    if let Some(timeout) = timeout {
                        options = options
                            .transaction_timeout(timeout)
                            .schema_lock_acquire_timeout(timeout);
                    }
                    let transaction = d
                        .transaction_with_options(&db, typedb_tx_type, options)
                        .await
                        .map_err(|e| {
                            RuntimeError::Transaction(format!("Failed to open transaction: {e}"))
                        })?;
                    driver_lease.ensure_open()?;
                    Ok(RuntimeTransaction {
                        inner: RuntimeTransactionInner::B8(Some(transaction)),
                        driver_lease: Some(driver_lease),
                    })
                }
                #[cfg(feature = "band9")]
                DriverHandleInner::B9(d) => {
                    let typedb_tx_type = match tx_type {
                        TxType::Read => B9TransactionType::Read,
                        TxType::Write => B9TransactionType::Write,
                        TxType::Schema => B9TransactionType::Schema,
                    };
                    let mut options = B9TransactionOptions::new();
                    if let Some(timeout) = timeout {
                        options = options
                            .transaction_timeout(timeout)
                            .schema_lock_acquire_timeout(timeout);
                    }
                    let transaction = d
                        .transaction_with_options(&db, typedb_tx_type, options)
                        .await
                        .map_err(|e| {
                            RuntimeError::Transaction(format!("Failed to open transaction: {e}"))
                        })?;
                    driver_lease.ensure_open()?;
                    Ok(RuntimeTransaction {
                        inner: RuntimeTransactionInner::B9(Some(transaction)),
                        driver_lease: Some(driver_lease),
                    })
                }
            }
        })
    }

    /// Check whether the underlying driver is open.
    pub fn is_open(&self) -> bool {
        if self.driver.shutdown_started() {
            return false;
        }
        match self.driver.inner() {
            #[cfg(feature = "band7")]
            DriverHandleInner::B7(d) => d.is_open(),
            #[cfg(feature = "band8")]
            DriverHandleInner::B8(d) => d.is_open(),
            #[cfg(feature = "band9")]
            DriverHandleInner::B9(d) => d.is_open(),
        }
    }

    /// Check whether a database exists.
    pub fn database_exists(&self, database: &str) -> BoxFuture<'_, Result<bool>> {
        let database = database.to_string();
        Box::pin(async move {
            self.driver.ensure_open()?;
            match self.driver.inner() {
                #[cfg(feature = "band7")]
                DriverHandleInner::B7(d) => {
                    d.databases().contains(database).await.map_err(|e| {
                        RuntimeError::Connection(format!("Database lookup failed: {e}"))
                    })
                }
                #[cfg(feature = "band8")]
                DriverHandleInner::B8(d) => {
                    d.databases().contains(database).await.map_err(|e| {
                        RuntimeError::Connection(format!("Database lookup failed: {e}"))
                    })
                }
                #[cfg(feature = "band9")]
                DriverHandleInner::B9(d) => {
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
            self.driver.ensure_open()?;
            match self.driver.inner() {
                #[cfg(feature = "band7")]
                DriverHandleInner::B7(d) => {
                    d.databases().create(database).await.map_err(|e| {
                        RuntimeError::Connection(format!("Database create failed: {e}"))
                    })
                }
                #[cfg(feature = "band8")]
                DriverHandleInner::B8(d) => {
                    d.databases().create(database).await.map_err(|e| {
                        RuntimeError::Connection(format!("Database create failed: {e}"))
                    })
                }
                #[cfg(feature = "band9")]
                DriverHandleInner::B9(d) => {
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
            self.driver.ensure_open()?;
            match self.driver.inner() {
                #[cfg(feature = "band7")]
                DriverHandleInner::B7(d) => {
                    let db = d.databases().get(&database).await.map_err(|e| {
                        RuntimeError::Connection(format!("Database lookup failed: {e}"))
                    })?;
                    db.delete().await.map_err(|e| {
                        RuntimeError::Connection(format!("Database delete failed: {e}"))
                    })
                }
                #[cfg(feature = "band8")]
                DriverHandleInner::B8(d) => {
                    let db = d.databases().get(database).await.map_err(|e| {
                        RuntimeError::Connection(format!("Database lookup failed: {e}"))
                    })?;
                    db.delete().await.map_err(|e| {
                        RuntimeError::Connection(format!("Database delete failed: {e}"))
                    })
                }
                #[cfg(feature = "band9")]
                DriverHandleInner::B9(d) => {
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
            self.driver.ensure_open()?;
            match self.driver.inner() {
                #[cfg(feature = "band7")]
                DriverHandleInner::B7(d) => {
                    let db = d.databases().get(&database).await.map_err(|e| {
                        RuntimeError::Connection(format!("Database lookup failed: {e}"))
                    })?;
                    db.schema()
                        .await
                        .map_err(|e| RuntimeError::Connection(format!("Schema export failed: {e}")))
                }
                #[cfg(feature = "band8")]
                DriverHandleInner::B8(d) => {
                    let db = d.databases().get(&database).await.map_err(|e| {
                        RuntimeError::Connection(format!("Database lookup failed: {e}"))
                    })?;
                    db.schema()
                        .await
                        .map_err(|e| RuntimeError::Connection(format!("Schema export failed: {e}")))
                }
                #[cfg(feature = "band9")]
                DriverHandleInner::B9(d) => {
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
    B8(Option<type_bridge_typedb_driver_b8::Transaction>),
    #[cfg(feature = "band9")]
    B9(Option<typedb_driver::Transaction>),
}

/// Open TypeDB transaction owned by the shared runtime.
pub struct RuntimeTransaction {
    inner: RuntimeTransactionInner,
    // Keep the selected driver alive until this transaction is released. Test
    // doubles use `None` because they contain no live upstream transaction.
    driver_lease: Option<Arc<DriverHandle>>,
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

#[cfg(any(feature = "band9", test))]
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
    fn ensure_driver_open(&self) -> Result<()> {
        match &self.driver_lease {
            Some(driver) => driver.ensure_open(),
            None => Ok(()),
        }
    }

    fn driver_shutdown_started(driver_lease: &Option<Arc<DriverHandle>>) -> bool {
        driver_lease
            .as_ref()
            .is_some_and(|driver| driver.shutdown_started())
    }

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
        self.query_bounded_with_temporal_encoding(
            typeql,
            QueryV2RuntimeAnswerLimits {
                answer: limits,
                max_collection_members: u64::MAX,
            },
            TemporalJsonEncoding::DriverDisplay,
            consumer,
        )
    }

    /// Execute V2 TypeQL with an additive document collection-member limit.
    pub fn query_v2_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        limits: QueryV2RuntimeAnswerLimits,
        consumer: &'a mut (dyn FnMut(RuntimeAnswerItem) -> Result<RuntimeAnswerControl> + Send),
    ) -> BoxFuture<'a, Result<RuntimeAnswerStats>> {
        self.query_bounded_with_temporal_encoding(
            typeql,
            limits,
            TemporalJsonEncoding::ExactV2,
            consumer,
        )
    }

    fn query_bounded_with_temporal_encoding<'a>(
        &'a mut self,
        typeql: &'a str,
        limits: QueryV2RuntimeAnswerLimits,
        temporal_encoding: TemporalJsonEncoding,
        consumer: &'a mut (dyn FnMut(RuntimeAnswerItem) -> Result<RuntimeAnswerControl> + Send),
    ) -> BoxFuture<'a, Result<RuntimeAnswerStats>> {
        let QueryV2RuntimeAnswerLimits {
            answer: limits,
            max_collection_members,
        } = limits;
        let tql = typeql.to_string();
        Box::pin(async move {
            self.ensure_driver_open()?;
            runtime_check(&limits)?;
            match &self.inner {
                #[cfg(feature = "band7")]
                RuntimeTransactionInner::B7(opt) => {
                    let tx = opt.as_ref().ok_or_else(|| {
                        RuntimeError::Transaction("Transaction already consumed".into())
                    })?;
                    let answer = runtime_await(
                        async {
                            let options = if limits.is_unbounded() {
                                driver_b7::QueryOptions::new()
                            } else {
                                driver_b7::QueryOptions::new().prefetch_size(1)
                            };
                            tx.query_with_options(&tql, options)
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
                                        .map(|concept| {
                                            concept_to_json_b7(concept, temporal_encoding)
                                        })
                                        .transpose()?
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
                            let mut collection_members = 0_u64;
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
                                let value = document_to_json_b7(
                                    &document,
                                    &mut collection_members,
                                    max_collection_members,
                                    temporal_encoding,
                                )?;
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
                            let options = if limits.is_unbounded() {
                                driver_b8::QueryOptions::new()
                            } else {
                                driver_b8::QueryOptions::new().prefetch_size(1)
                            };
                            tx.query_with_options(&tql, options)
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
                                        .map(|concept| {
                                            concept_to_json_b8(concept, temporal_encoding)
                                        })
                                        .transpose()?
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
                            let mut collection_members = 0_u64;
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
                                let value = document_to_json_b8(
                                    &document,
                                    &mut collection_members,
                                    max_collection_members,
                                    temporal_encoding,
                                )?;
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
                            let options = if limits.is_unbounded() {
                                driver_b9::QueryOptions::new()
                            } else {
                                driver_b9::QueryOptions::new().prefetch_size(1)
                            };
                            tx.query_with_options(&tql, options)
                                .await
                                .map_err(|e| RuntimeError::QueryExecution(format!("{e}")))
                        },
                        &limits,
                    )
                    .await?;
                    consume_answer_b9(
                        answer,
                        &limits,
                        max_collection_members,
                        temporal_encoding,
                        consumer,
                    )
                    .await
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
        self.query_with_rows_bounded_with_temporal_encoding(
            typeql,
            rows,
            QueryV2RuntimeAnswerLimits {
                answer: limits,
                max_collection_members: u64::MAX,
            },
            TemporalJsonEncoding::DriverDisplay,
            consumer,
        )
    }

    /// Execute a V2 `given` query with an additive collection-member limit.
    pub fn query_v2_with_rows_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        rows: GivenRowsSpec,
        limits: QueryV2RuntimeAnswerLimits,
        consumer: &'a mut (dyn FnMut(RuntimeAnswerItem) -> Result<RuntimeAnswerControl> + Send),
    ) -> BoxFuture<'a, Result<RuntimeAnswerStats>> {
        self.query_with_rows_bounded_with_temporal_encoding(
            typeql,
            rows,
            limits,
            TemporalJsonEncoding::ExactV2,
            consumer,
        )
    }

    fn query_with_rows_bounded_with_temporal_encoding<'a>(
        &'a mut self,
        typeql: &'a str,
        rows: GivenRowsSpec,
        limits: QueryV2RuntimeAnswerLimits,
        temporal_encoding: TemporalJsonEncoding,
        consumer: &'a mut (dyn FnMut(RuntimeAnswerItem) -> Result<RuntimeAnswerControl> + Send),
    ) -> BoxFuture<'a, Result<RuntimeAnswerStats>> {
        let QueryV2RuntimeAnswerLimits {
            answer: limits,
            max_collection_members,
        } = limits;
        let tql = typeql.to_owned();
        #[cfg(not(feature = "band9"))]
        let _ = (
            &tql,
            &rows,
            max_collection_members,
            temporal_encoding,
            &consumer,
        );
        Box::pin(async move {
            self.ensure_driver_open()?;
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
                            let options = if limits.is_unbounded() {
                                driver_b9::QueryOptions::new()
                            } else {
                                driver_b9::QueryOptions::new().prefetch_size(1)
                            };
                            tx.query_with_options_and_rows(&tql, options, Some(given_rows))
                                .await
                                .map_err(|e| RuntimeError::QueryExecution(format!("{e}")))
                        },
                        &limits,
                    )
                    .await?;
                    consume_answer_b9(
                        answer,
                        &limits,
                        max_collection_members,
                        temporal_encoding,
                        consumer,
                    )
                    .await
                }
            }
        })
    }

    /// Commit this transaction.
    pub fn commit(&mut self) -> BoxFuture<'_, Result<()>> {
        let commit = self.commit_classified();
        Box::pin(async move { commit.await.map_err(RuntimeCommitError::into_runtime_error) })
    }

    /// Commit this transaction while retaining driver durability certainty.
    pub fn commit_classified(
        &mut self,
    ) -> BoxFuture<'_, std::result::Result<(), RuntimeCommitError>> {
        // Take ownership out of the Option so the async block can move the
        // transaction by value into commit(self) — both bands consume self.
        let driver_lease = self.driver_lease.clone();
        match &mut self.inner {
            #[cfg(feature = "band7")]
            RuntimeTransactionInner::B7(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    if let Some(driver) = &driver_lease {
                        driver.ensure_open().map_err(RuntimeCommitError::Runtime)?;
                    }
                    let t = tx.ok_or_else(|| {
                        RuntimeCommitError::Runtime(RuntimeError::Transaction(
                            "Transaction already consumed".into(),
                        ))
                    })?;
                    t.commit().await.map_err(band7_commit_failure)
                })
            }
            #[cfg(feature = "band8")]
            RuntimeTransactionInner::B8(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    if let Some(driver) = &driver_lease {
                        driver.ensure_open().map_err(RuntimeCommitError::Runtime)?;
                    }
                    let t = tx.ok_or_else(|| {
                        RuntimeCommitError::Runtime(RuntimeError::Transaction(
                            "Transaction already consumed".into(),
                        ))
                    })?;
                    t.commit().await.map_err(band8_commit_failure)
                })
            }
            #[cfg(feature = "band9")]
            RuntimeTransactionInner::B9(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    if let Some(driver) = &driver_lease {
                        driver.ensure_open().map_err(RuntimeCommitError::Runtime)?;
                    }
                    let t = tx.ok_or_else(|| {
                        RuntimeCommitError::Runtime(RuntimeError::Transaction(
                            "Transaction already consumed".into(),
                        ))
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
        let driver_lease = self.driver_lease.clone();
        match &mut self.inner {
            #[cfg(feature = "band7")]
            RuntimeTransactionInner::B7(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    if let Some(driver) = &driver_lease {
                        driver.ensure_open()?;
                    }
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
                    if let Some(driver) = &driver_lease {
                        driver.ensure_open()?;
                    }
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
                    if let Some(driver) = &driver_lease {
                        driver.ensure_open()?;
                    }
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
        let driver_lease = self.driver_lease.clone();
        match &mut self.inner {
            #[cfg(feature = "band7")]
            RuntimeTransactionInner::B7(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    let Some(t) = tx else {
                        return Ok(());
                    };
                    if Self::driver_shutdown_started(&driver_lease) {
                        drop(t);
                        return Ok(());
                    }
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
                    if Self::driver_shutdown_started(&driver_lease) {
                        drop(t);
                        return Ok(());
                    }
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
                    if Self::driver_shutdown_started(&driver_lease) {
                        drop(t);
                        return Ok(());
                    }
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

fn runtime_document_member_error(message: &'static str) -> RuntimeError {
    RuntimeError::ResourceLimit {
        code: "query_v2_document_member_limit",
        message,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TemporalJsonEncoding {
    DriverDisplay,
    ExactV2,
}

fn checked_datetime_tz_local<Tz>(
    value: &chrono::DateTime<Tz>,
) -> Result<(chrono::NaiveDateTime, i32)>
where
    Tz: chrono::TimeZone,
    Tz::Offset: chrono::Offset,
{
    use chrono::Offset as _;

    let offset_seconds = value.offset().fix().local_minus_utc();
    let local = value
        .naive_utc()
        .checked_add_signed(chrono::TimeDelta::seconds(i64::from(offset_seconds)))
        .ok_or_else(|| {
            RuntimeError::QueryExecution(
                "provider datetime-tz local value is outside the supported range".to_owned(),
            )
        })?;
    Ok((local, offset_seconds))
}

fn append_exact_offset(rendered: &mut String, offset_seconds: i64) {
    use std::fmt::Write as _;

    if offset_seconds == 0 {
        rendered.push('Z');
        return;
    }
    let sign = if offset_seconds < 0 { '-' } else { '+' };
    let absolute = offset_seconds.unsigned_abs();
    let hours = absolute / 3_600;
    let minutes = (absolute % 3_600) / 60;
    let seconds = absolute % 60;
    write!(rendered, "{sign}{hours:02}:{minutes:02}")
        .expect("writing an offset to String cannot fail");
    if seconds != 0 {
        write!(rendered, ":{seconds:02}").expect("writing an offset to String cannot fail");
    }
}

fn exact_naive_datetime(value: &chrono::NaiveDateTime) -> String {
    use std::fmt::Write as _;

    use chrono::{Datelike as _, Timelike as _};

    let mut rendered = String::new();
    let year = value.year();
    match year {
        0..=9999 => write!(rendered, "{year:04}"),
        10_000.. => write!(rendered, "+{year}"),
        -9999..=-1 => write!(rendered, "-{:04}", -year),
        _ => write!(rendered, "{year}"),
    }
    .expect("writing a year to String cannot fail");
    write!(
        rendered,
        "-{:02}-{:02}T{:02}:{:02}:{:02}",
        value.month(),
        value.day(),
        value.hour(),
        value.minute(),
        value.second(),
    )
    .expect("writing datetime components to String cannot fail");
    let nanosecond = value.nanosecond();
    if nanosecond != 0 {
        let fraction = format!("{nanosecond:09}");
        write!(rendered, ".{}", fraction.trim_end_matches('0'))
            .expect("writing a datetime fraction to String cannot fail");
    }
    rendered
}

fn exact_duration(months: u32, days: u32, nanos: u64) -> String {
    use std::fmt::Write as _;

    let seconds = nanos / 1_000_000_000;
    let nanosecond = nanos % 1_000_000_000;
    let mut rendered = String::from("P");
    if months != 0 {
        write!(rendered, "{months}M").expect("writing duration months to String cannot fail");
    }
    if days != 0 {
        write!(rendered, "{days}D").expect("writing duration days to String cannot fail");
    }
    if seconds != 0 || nanosecond != 0 || (months == 0 && days == 0) {
        write!(rendered, "T{seconds}").expect("writing duration seconds to String cannot fail");
        if nanosecond != 0 {
            let fraction = format!("{nanosecond:09}");
            write!(rendered, ".{}", fraction.trim_end_matches('0'))
                .expect("writing a duration fraction to String cannot fail");
        }
        rendered.push('S');
    }
    rendered
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
        fn $document_fn(
            document: &$driver::answer::ConceptDocument,
            collection_members: &mut u64,
            max_collection_members: u64,
            temporal_encoding: TemporalJsonEncoding,
        ) -> Result<serde_json::Value> {
            document
                .root
                .as_ref()
                .map(|node| {
                    $node_fn(
                        node,
                        collection_members,
                        max_collection_members,
                        temporal_encoding,
                    )
                })
                .transpose()
                .map(|value| value.unwrap_or(serde_json::Value::Null))
        }

        #[cfg(feature = $feature)]
        fn $node_fn(
            node: &$driver::answer::concept_document::Node,
            collection_members: &mut u64,
            max_collection_members: u64,
            temporal_encoding: TemporalJsonEncoding,
        ) -> Result<serde_json::Value> {
            use $driver::answer::concept_document::Node;

            match node {
                Node::Map(map) => map
                    .iter()
                    .map(|(name, node)| {
                        $node_fn(
                            node,
                            collection_members,
                            max_collection_members,
                            temporal_encoding,
                        )
                        .map(|value| (name.clone(), value))
                    })
                    .collect::<Result<serde_json::Map<String, serde_json::Value>>>()
                    .map(serde_json::Value::Object),
                Node::List(list) => {
                    let members = u64::try_from(list.len()).map_err(|_| {
                        runtime_document_member_error(
                            "document list member count exceeds the supported counter range",
                        )
                    })?;
                    let next = collection_members.checked_add(members).ok_or_else(|| {
                        runtime_document_member_error("document list member counter overflowed")
                    })?;
                    if next > max_collection_members {
                        return Err(runtime_document_member_error(
                            "document lists exceed the aggregate member ceiling",
                        ));
                    }
                    *collection_members = next;
                    list.iter()
                        .map(|node| {
                            $node_fn(
                                node,
                                collection_members,
                                max_collection_members,
                                temporal_encoding,
                            )
                        })
                        .collect::<Result<Vec<_>>>()
                        .map(serde_json::Value::Array)
                }
                Node::Leaf(Some(leaf)) => $leaf_fn(leaf, temporal_encoding),
                Node::Leaf(None) => Ok(serde_json::Value::Null),
            }
        }

        #[cfg(feature = $feature)]
        fn $leaf_fn(
            leaf: &$driver::answer::concept_document::Leaf,
            temporal_encoding: TemporalJsonEncoding,
        ) -> Result<serde_json::Value> {
            use $driver::answer::concept_document::Leaf;
            use $driver::concept::Concept;

            match leaf {
                Leaf::Empty => Ok(serde_json::Value::Null),
                Leaf::Concept(concept) => match concept {
                    Concept::EntityType(_) => {
                        Ok(document_type_to_json("entity", concept.get_label()))
                    }
                    Concept::RelationType(_) => {
                        Ok(document_type_to_json("relation", concept.get_label()))
                    }
                    Concept::RoleType(_) => {
                        Ok(document_type_to_json("relation:role", concept.get_label()))
                    }
                    Concept::AttributeType(_) => {
                        let value_type = concept.try_get_value_type();
                        Ok(document_attribute_type_to_json(
                            concept.get_label(),
                            value_type.as_ref().map(|value_type| value_type.name()),
                        ))
                    }
                    Concept::Attribute(_) | Concept::Value(_) => {
                        let value = concept.try_get_value().ok_or_else(|| {
                            RuntimeError::QueryExecution(
                                "document value concept did not carry a value".to_owned(),
                            )
                        })?;
                        $value_fn(value, temporal_encoding)
                    }
                    Concept::Entity(_) | Concept::Relation(_) => Err(RuntimeError::QueryExecution(
                        "document response carried an unsupported thing instance".to_owned(),
                    )),
                },
                Leaf::ValueType(value_type) => {
                    Ok(serde_json::Value::String(value_type.name().to_owned()))
                }
                Leaf::Kind(kind) => Ok(serde_json::Value::String(kind.name().to_owned())),
            }
        }

        #[cfg(feature = $feature)]
        fn $value_fn(
            value: &$driver::concept::Value,
            temporal_encoding: TemporalJsonEncoding,
        ) -> Result<serde_json::Value> {
            use $driver::concept::Value;

            let converted = match value {
                Value::Boolean(value) => serde_json::Value::Bool(*value),
                Value::Integer(value) => serde_json::Value::from(*value),
                Value::Double(value) => serde_json::Value::from(*value),
                Value::String(value) => serde_json::Value::String(value.clone()),
                Value::Decimal(_) | Value::Date(_) => serde_json::Value::String(value.to_string()),
                Value::Datetime(datetime) => {
                    let rendered = match temporal_encoding {
                        TemporalJsonEncoding::DriverDisplay => value.to_string(),
                        TemporalJsonEncoding::ExactV2 => exact_naive_datetime(datetime),
                    };
                    serde_json::Value::String(rendered)
                }
                Value::Duration(duration) => {
                    let rendered = match temporal_encoding {
                        TemporalJsonEncoding::DriverDisplay => value.to_string(),
                        TemporalJsonEncoding::ExactV2 => {
                            exact_duration(duration.months, duration.days, duration.nanos)
                        }
                    };
                    serde_json::Value::String(rendered)
                }
                Value::DatetimeTZ(datetime_tz) => {
                    let (local, offset_seconds) = checked_datetime_tz_local(datetime_tz)?;
                    let rendered = match temporal_encoding {
                        TemporalJsonEncoding::DriverDisplay => value.to_string(),
                        TemporalJsonEncoding::ExactV2 => {
                            let mut rendered = exact_naive_datetime(&local);
                            append_exact_offset(&mut rendered, i64::from(offset_seconds));
                            if let $driver::concept::value::TimeZone::IANA(timezone) =
                                datetime_tz.timezone()
                            {
                                use std::fmt::Write as _;
                                write!(rendered, "[{}]", timezone.name())
                                    .expect("writing a timezone name to String cannot fail");
                            }
                            rendered
                        }
                    };
                    serde_json::Value::String(rendered)
                }
                Value::Struct(value, name) => {
                    let fields = value
                        .fields()
                        .iter()
                        .map(|(field, value)| -> Result<_> {
                            Ok((
                                field.clone(),
                                value
                                    .as_ref()
                                    .map(|value| $value_fn(value, temporal_encoding))
                                    .transpose()?
                                    .unwrap_or(serde_json::Value::Null),
                            ))
                        })
                        .collect::<Result<_>>()?;
                    serde_json::Value::Object(serde_json::Map::from_iter([(
                        name.clone(),
                        serde_json::Value::Object(fields),
                    )]))
                }
            };
            Ok(converted)
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
    temporal_encoding: TemporalJsonEncoding,
) -> Result<serde_json::Value> {
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
        obj.insert("value".into(), value_to_json_b7(value, temporal_encoding)?);
    }
    if let Some(vt) = concept.try_get_value_type() {
        obj.insert(
            "value_type".into(),
            serde_json::Value::String(vt.name().into()),
        );
    }
    Ok(serde_json::Value::Object(obj))
}

/// Convert a band-8 TypeDB concept to a JSON value.
///
/// Output shape is identical to [`concept_to_json_b7`] for all common concepts.
#[cfg(feature = "band8")]
fn concept_to_json_b8(
    concept: &type_bridge_typedb_driver_b8::concept::Concept,
    temporal_encoding: TemporalJsonEncoding,
) -> Result<serde_json::Value> {
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
        obj.insert("value".into(), value_to_json_b8(value, temporal_encoding)?);
    }
    if let Some(vt) = concept.try_get_value_type() {
        obj.insert(
            "value_type".into(),
            serde_json::Value::String(vt.name().into()),
        );
    }
    Ok(serde_json::Value::Object(obj))
}

/// Lower a portable [`GivenRowsSpec`] onto the band-9 driver's `GivenRows`.
///
/// Fails when a row's width does not match the header or a temporal string
/// does not parse — both are caller mistakes reported before any wire I/O.
#[cfg(feature = "band9")]
fn given_rows_b9(spec: GivenRowsSpec) -> Result<typedb_driver::given::GivenRows> {
    use typedb_driver::given::GivenRows;

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
fn given_entry_b9(value: GivenValue) -> Result<typedb_driver::given::GivenRowEntry> {
    use chrono::{NaiveDate, NaiveDateTime};
    use typedb_driver::concept::value::TimeZone as B9TimeZone;
    use typedb_driver::concept::value::{Decimal, Duration};
    use typedb_driver::given::GivenRowEntry;

    Ok(match value {
        GivenValue::Empty => GivenRowEntry::Empty,
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
            let dt = parse_given_datetime_tz(&s).map_err(|e| {
                RuntimeError::QueryExecution(format!("Invalid given datetime-tz {s:?}: {e}"))
            })?;
            let offset = *dt.offset();
            GivenRowEntry::from(dt.with_timezone(&B9TimeZone::Fixed(offset)))
        }
        GivenValue::DatetimeTzExact {
            local,
            named_zone,
            effective_offset_seconds,
        } => GivenRowEntry::from(exact_given_datetime_tz_b9(
            &local,
            named_zone.as_deref(),
            effective_offset_seconds,
        )?),
        GivenValue::Decimal(s) => {
            // The upstream parser parses the unsigned magnitude into `i64`,
            // so the otherwise-valid decimal minimum cannot pass through it.
            // Preserve that one boundary explicitly; every other canonical
            // decimal spelling is accepted by the driver parser.
            let decimal = if s == "-9223372036854775808" {
                Decimal::MIN
            } else {
                s.parse::<Decimal>().map_err(|e| {
                    RuntimeError::QueryExecution(format!("Invalid given decimal {s:?}: {e}"))
                })?
            };
            GivenRowEntry::from(decimal)
        }
        GivenValue::Duration {
            months,
            days,
            nanos,
        } => GivenRowEntry::from(Duration::new(months, days, nanos)),
    })
}

#[cfg(feature = "band9")]
fn exact_given_datetime_tz_b9(
    local: &str,
    named_zone: Option<&str>,
    effective_offset_seconds: i32,
) -> Result<chrono::DateTime<typedb_driver::concept::value::TimeZone>> {
    use chrono::{FixedOffset, LocalResult, NaiveDateTime, TimeZone as _};
    use typedb_driver::concept::value::TimeZone as B9TimeZone;

    let local = local.parse::<NaiveDateTime>().map_err(|error| {
        RuntimeError::QueryExecution(format!("Invalid given exact datetime-tz local: {error}"))
    })?;
    let matches_offset = |value: &chrono::DateTime<B9TimeZone>| {
        value
            .naive_local()
            .signed_duration_since(value.naive_utc())
            .num_seconds()
            == i64::from(effective_offset_seconds)
    };
    if let Some(name) = named_zone {
        let timezone = chrono_tz::Tz::from_str_insensitive(name)
            .map(B9TimeZone::IANA)
            .map_err(|_| {
                RuntimeError::QueryExecution(
                    "Invalid given exact datetime-tz named zone".to_owned(),
                )
            })?;
        let selected = match timezone.from_local_datetime(&local) {
            LocalResult::Single(value) if matches_offset(&value) => Some(value),
            LocalResult::Ambiguous(earlier, later) => [earlier, later]
                .into_iter()
                .find(|value| matches_offset(value)),
            LocalResult::Single(_) | LocalResult::None => None,
        };
        return selected.ok_or_else(|| {
            RuntimeError::QueryExecution(
                "Invalid given exact datetime-tz named-zone resolution".to_owned(),
            )
        });
    }

    let offset = FixedOffset::east_opt(effective_offset_seconds).ok_or_else(|| {
        RuntimeError::QueryExecution("Invalid given exact datetime-tz fixed offset".to_owned())
    })?;
    B9TimeZone::Fixed(offset)
        .from_local_datetime(&local)
        .single()
        .ok_or_else(|| {
            RuntimeError::QueryExecution(
                "Invalid given exact datetime-tz fixed resolution".to_owned(),
            )
        })
}

/// Parse the fixed-offset spelling used by the portable V2 contract.
///
/// Chrono's RFC-3339 parser deliberately accepts only four-digit years and
/// minute-resolution offsets. TypeDB's scalar domain is wider, so parse the
/// canonical local datetime and an optional seconds component separately.
#[cfg(feature = "band9")]
fn parse_given_datetime_tz(
    value: &str,
) -> std::result::Result<chrono::DateTime<chrono::FixedOffset>, &'static str> {
    use chrono::{FixedOffset, NaiveDateTime, TimeZone as _};

    let (local, offset_seconds) = if let Some(local) = value.strip_suffix('Z') {
        (local, 0)
    } else {
        let (local, offset) = [9_usize, 6]
            .into_iter()
            .find_map(|width| {
                let split = value.len().checked_sub(width)?;
                if !value.is_char_boundary(split) {
                    return None;
                }
                let (local, offset) = (&value[..split], &value[split..]);
                parse_fixed_offset(offset).map(|seconds| (local, seconds))
            })
            .ok_or("expected a Z or signed fixed offset")?;
        (local, offset)
    };
    let local = local
        .parse::<NaiveDateTime>()
        .map_err(|_| "invalid local datetime")?;
    let offset = FixedOffset::east_opt(offset_seconds).ok_or("fixed offset is out of range")?;
    offset
        .from_local_datetime(&local)
        .single()
        .ok_or("local datetime is out of range")
}

#[cfg(feature = "band9")]
fn parse_fixed_offset(value: &str) -> Option<i32> {
    let bytes = value.as_bytes();
    if !matches!(
        bytes,
        [b'+' | b'-', _, _, b':', _, _] | [b'+' | b'-', _, _, b':', _, _, b':', _, _]
    ) || !bytes
        .iter()
        .enumerate()
        .filter(|(index, _)| !matches!(index, 0 | 3 | 6))
        .all(|(_, byte)| byte.is_ascii_digit())
    {
        return None;
    }
    let component = |left: usize, right: usize| {
        std::str::from_utf8(&bytes[left..right])
            .ok()?
            .parse::<i32>()
            .ok()
    };
    let hours = component(1, 3)?;
    let minutes = component(4, 6)?;
    let seconds = if bytes.len() == 9 {
        component(7, 9)?
    } else {
        0
    };
    if hours > 23 || minutes > 59 || seconds > 59 {
        return None;
    }
    let magnitude = hours * 3_600 + minutes * 60 + seconds;
    Some(if bytes[0] == b'-' {
        -magnitude
    } else {
        magnitude
    })
}

/// Convert and consume a band-9 answer without materializing its stream.
#[cfg(feature = "band9")]
async fn consume_answer_b9(
    answer: B9QueryAnswer,
    limits: &RuntimeAnswerLimits,
    max_collection_members: u64,
    temporal_encoding: TemporalJsonEncoding,
    consumer: &mut (dyn FnMut(RuntimeAnswerItem) -> Result<RuntimeAnswerControl> + Send),
) -> Result<RuntimeAnswerStats> {
    match answer {
        B9QueryAnswer::Ok(_) => Ok(RuntimeAnswerStats::new(RuntimeAnswerKind::Ok)),
        B9QueryAnswer::ConceptRowStream(_, stream) => {
            let stream = stream
                .map_err(|error| RuntimeError::QueryExecution(format!("Row stream: {error}")))
                .and_then(move |row| {
                    let result = (|| -> Result<RuntimeAnswerItem> {
                        let mut obj = serde_json::Map::new();
                        for (i, col) in row.get_column_names().iter().enumerate() {
                            let value = row
                                .row
                                .get(i)
                                .and_then(|c| c.as_ref())
                                .map(|concept| concept_to_json_b9(concept, temporal_encoding))
                                .transpose()?
                                .unwrap_or(serde_json::Value::Null);
                            obj.insert(col.clone(), value);
                        }
                        Ok(RuntimeAnswerItem::Row(serde_json::Value::Object(obj)))
                    })();
                    futures::future::ready(result)
                });
            runtime_consume_stream(stream, RuntimeAnswerKind::Rows, limits, consumer).await
        }
        B9QueryAnswer::ConceptDocumentStream(_, stream) => {
            let mut collection_members = 0_u64;
            let stream = stream
                .map_err(|error| RuntimeError::QueryExecution(format!("Document stream: {error}")))
                .and_then(move |document| {
                    let result = document_to_json_b9(
                        &document,
                        &mut collection_members,
                        max_collection_members,
                        temporal_encoding,
                    )
                    .map(RuntimeAnswerItem::Document);
                    futures::future::ready(result)
                });
            runtime_consume_stream(stream, RuntimeAnswerKind::Documents, limits, consumer).await
        }
    }
}

/// Convert a band-9 TypeDB concept to a JSON value.
///
/// Output shape is identical to [`concept_to_json_b8`] for all common concepts.
#[cfg(feature = "band9")]
fn concept_to_json_b9(
    concept: &typedb_driver::concept::Concept,
    temporal_encoding: TemporalJsonEncoding,
) -> Result<serde_json::Value> {
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
        obj.insert("value".into(), value_to_json_b9(value, temporal_encoding)?);
    }
    if let Some(vt) = concept.try_get_value_type() {
        obj.insert(
            "value_type".into(),
            serde_json::Value::String(vt.name().into()),
        );
    }
    Ok(serde_json::Value::Object(obj))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    static NEXT_TLS_MATERIAL_TEST_ID: AtomicUsize = AtomicUsize::new(0);

    #[derive(Clone, Default)]
    struct TraceCapture {
        bytes: Arc<Mutex<Vec<u8>>>,
    }

    struct TraceCaptureWriter {
        bytes: Arc<Mutex<Vec<u8>>>,
    }

    impl io::Write for TraceCaptureWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.bytes
                .lock()
                .expect("trace capture lock")
                .extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for TraceCapture {
        type Writer = TraceCaptureWriter;

        fn make_writer(&'writer self) -> Self::Writer {
            TraceCaptureWriter {
                bytes: Arc::clone(&self.bytes),
            }
        }
    }

    fn capture_traces(emit: impl FnOnce()) -> String {
        let capture = TraceCapture::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_max_level(tracing::Level::TRACE)
            .with_writer(capture.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, emit);
        let bytes = capture.bytes.lock().expect("trace capture lock").clone();
        String::from_utf8(bytes).expect("tracing formatter emits UTF-8")
    }

    #[test]
    fn driver_drop_close_warning_drops_hostile_provider_text() {
        const PROVIDER_SENTINEL: &str = "TB_DROP_CLOSE_PROVIDER_SECRET";
        let error = RuntimeError::Connection(PROVIDER_SENTINEL.to_owned());
        let output = capture_traces(|| trace_driver_drop_close_failure(&error));

        assert!(
            output.contains(TRACE_CODE_DRIVER_DROP_CLOSE_FAILED),
            "{output}"
        );
        assert!(
            output.contains("typedb_runtime_connection_failed"),
            "{output}"
        );
        assert!(!output.contains(PROVIDER_SENTINEL), "{output}");
    }

    #[cfg(all(feature = "band8", feature = "band9"))]
    #[test]
    fn band9_upgrade_failure_warning_drops_hostile_provider_and_address_text() {
        const ADDRESS_SENTINEL: &str = "TB_BAND9_UPGRADE_ADDRESS_SECRET";
        const PROVIDER_SENTINEL: &str = "TB_BAND9_UPGRADE_PROVIDER_SECRET";
        let error =
            SecureConnectError::Runtime(RuntimeError::Connection(PROVIDER_SENTINEL.to_owned()));
        let output = capture_traces(|| {
            trace_band9_upgrade_failed(
                ADDRESS_SENTINEL,
                core_version::Version::new(3, 12, 1),
                &error,
            );
        });

        assert!(output.contains(TRACE_CODE_BAND9_UPGRADE_FAILED), "{output}");
        assert!(
            output.contains("typedb_runtime_connection_failed"),
            "{output}"
        );
        assert!(output.contains("3.12.1"), "{output}");
        assert!(!output.contains(ADDRESS_SENTINEL), "{output}");
        assert!(!output.contains(PROVIDER_SENTINEL), "{output}");
    }

    #[test]
    fn post_credential_connection_traces_drop_raw_address_identity() {
        const ADDRESS_SENTINEL: &str = "TB_POST_CREDENTIAL_ADDRESS_SECRET";
        let server_version = core_version::Version::new(3, 12, 1);
        let output = capture_traces(|| {
            trace_version_gate_passed(ADDRESS_SENTINEL, 9, server_version);
            #[cfg(all(feature = "band8", feature = "band9"))]
            trace_band9_upgrade_succeeded(ADDRESS_SENTINEL, server_version);
            #[cfg(feature = "band8")]
            trace_band8_fallback_connected(ADDRESS_SENTINEL, server_version);
            #[cfg(feature = "band7")]
            trace_band7_fallback_connected(ADDRESS_SENTINEL);
            trace_runtime_connected(ADDRESS_SENTINEL, 9, Some(server_version));
        });

        assert!(!output.contains(ADDRESS_SENTINEL), "{output}");
        assert!(output.contains(TRACE_CODE_VERSION_GATE_PASSED), "{output}");
        assert!(output.contains(TRACE_CODE_CONNECTED), "{output}");
        assert!(output.contains("3.12.1"), "{output}");
        #[cfg(all(feature = "band8", feature = "band9"))]
        assert!(
            output.contains(TRACE_CODE_BAND9_UPGRADE_SUCCEEDED),
            "{output}"
        );
        #[cfg(feature = "band8")]
        assert!(
            output.contains(TRACE_CODE_BAND8_FALLBACK_CONNECTED),
            "{output}"
        );
        #[cfg(feature = "band7")]
        assert!(
            output.contains(TRACE_CODE_BAND7_FALLBACK_CONNECTED),
            "{output}"
        );
    }

    #[test]
    fn credential_safe_secure_diagnostics_drop_provider_controlled_text() {
        const SENTINEL: &str = "TB_POST_CREDENTIAL_PROVIDER_SECRET";
        for error in [
            SecureConnectError::Runtime(RuntimeError::Connection(SENTINEL.to_owned())),
            SecureConnectError::Runtime(RuntimeError::UnsupportedVersion(
                core_version::VersionError::Probe(SENTINEL.to_owned()),
            )),
            SecureConnectError::Runtime(RuntimeError::UnsupportedVersion(
                core_version::VersionError::Parse(SENTINEL.to_owned()),
            )),
        ] {
            assert_eq!(error.credential_safe_diagnostic(), None);
        }

        let tls = SecureConnectError::TlsConfiguration(
            core_version::TlsConfigurationError::CustomRootCaUnreadable {
                path: std::path::PathBuf::from(SENTINEL),
            },
        )
        .credential_safe_diagnostic()
        .expect("typed TLS codes remain safe");
        assert!(tls.contains("tls_custom_root_ca_unreadable"), "{tls}");
        assert!(!tls.contains(SENTINEL), "{tls}");
    }

    #[test]
    fn credential_safe_secure_diagnostics_preserve_closed_version_data() {
        let error = SecureConnectError::Runtime(RuntimeError::UnsupportedVersion(
            core_version::VersionError::Unsupported {
                component: "server",
                found: core_version::Version::new(3, 13, 0),
            },
        ));
        let diagnostic = error
            .credential_safe_diagnostic()
            .expect("closed version diagnostics remain actionable");
        assert!(diagnostic.contains("server version 3.13.0"), "{diagnostic}");
        assert!(diagnostic.contains("3.8.0–3.12.x"), "{diagnostic}");
    }

    #[test]
    fn driver_shutdown_panics_are_contained_as_connection_errors() {
        let error = contain_driver_shutdown(|| panic!("simulated upstream shutdown panic"))
            .expect_err("shutdown panic must not cross the runtime boundary");

        assert!(matches!(
            error,
            RuntimeError::Connection(message)
                if message == "Driver close failed: upstream driver panicked during shutdown"
        ));
    }

    #[test]
    fn failed_driver_shutdown_is_retried_without_reopening_admission() {
        let state = AtomicU8::new(DRIVER_OPEN);
        let close_lock = Mutex::new(());

        for _ in 0..2 {
            let error = run_driver_shutdown(&state, &close_lock, || {
                Err(RuntimeError::Connection(
                    "Driver close failed: simulated incomplete cleanup".to_owned(),
                ))
            })
            .expect_err("incomplete cleanup must remain observable");
            assert_eq!(
                error.to_string(),
                "Connection error: Driver close failed: simulated incomplete cleanup"
            );
            assert_eq!(state.load(AtomicOrdering::Acquire), DRIVER_CLOSING);
        }

        run_driver_shutdown(&state, &close_lock, || Ok(()))
            .expect("a later close retries and completes cleanup");
        assert_eq!(state.load(AtomicOrdering::Acquire), DRIVER_CLOSED);

        run_driver_shutdown(&state, &close_lock, || {
            panic!("successful cleanup must make later close calls no-ops")
        })
        .expect("close remains idempotent after cleanup succeeds");
    }

    #[test]
    fn one_shot_database_operation_propagates_close_failure_after_success() {
        assert_eq!(
            finish_one_shot_database_operation(Ok(17_u8), Ok(())).unwrap(),
            17
        );

        let error = finish_one_shot_database_operation(
            Ok(17_u8),
            Err(RuntimeError::Connection("close diagnosis".to_owned())),
        )
        .expect_err("a successful operation must still report driver-close failure");
        assert!(matches!(
            error,
            RuntimeError::Connection(message) if message == "close diagnosis"
        ));
    }

    #[test]
    fn one_shot_database_operation_preserves_primary_failure_and_combines_close_failure() {
        let primary_only = finish_one_shot_database_operation::<()>(
            Err(RuntimeError::Connection("primary diagnosis".to_owned())),
            Ok(()),
        )
        .expect_err("the primary database-operation failure must be returned");
        assert!(matches!(
            primary_only,
            RuntimeError::Connection(message) if message == "primary diagnosis"
        ));

        let combined = finish_one_shot_database_operation::<()>(
            Err(RuntimeError::Connection("primary diagnosis".to_owned())),
            Err(RuntimeError::Connection("close diagnosis".to_owned())),
        )
        .expect_err("both failures must produce one deterministic diagnostic");
        assert!(matches!(
            combined,
            RuntimeError::Connection(message)
                if message
                    == "primary diagnosis; additionally, TypeDB driver cleanup failed: close diagnosis"
        ));
    }

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
                let mut collection_members = 0;
                assert_eq!(
                    $node_fn(
                        &integer,
                        &mut collection_members,
                        u64::MAX,
                        TemporalJsonEncoding::ExactV2,
                    )
                    .unwrap(),
                    serde_json::Value::from(LARGE_INTEGER),
                    "concept-document integers must not cross an f64 boundary"
                );

                let decimal = $decimal;
                let expected = decimal.to_string();
                let decimal =
                    Node::Leaf(Some(Leaf::Concept(Concept::Value(Value::Decimal(decimal)))));
                assert_eq!(
                    $node_fn(
                        &decimal,
                        &mut collection_members,
                        u64::MAX,
                        TemporalJsonEncoding::ExactV2,
                    )
                    .unwrap(),
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

    macro_rules! datetime_tz_evidence_regression {
        ($feature:literal, $name:ident, $driver:ident, $concept_fn:ident, $leaf_fn:ident) => {
            #[cfg(feature = $feature)]
            #[test]
            fn $name() {
                use chrono::{NaiveDateTime, TimeZone as _};
                use $driver::answer::concept_document::Leaf;
                use $driver::concept::value::TimeZone;
                use $driver::concept::{Concept, Value};

                let london = TimeZone::IANA(
                    "Europe/London"
                        .parse()
                        .expect("driver timezone database contains London"),
                );
                let overlap_utc = "2024-10-27T00:30:00"
                    .parse::<NaiveDateTime>()
                    .expect("UTC datetime");
                let named = Value::DatetimeTZ(london.from_utc_datetime(&overlap_utc));
                assert_eq!(
                    $concept_fn(
                        &Concept::Value(named.clone()),
                        TemporalJsonEncoding::ExactV2,
                    )
                    .expect("row concept")["value"],
                    serde_json::json!("2024-10-27T01:30:00+01:00[Europe/London]"),
                    "row evidence preserves both the IANA identity and overlap side",
                );
                assert_eq!(
                    $leaf_fn(
                        &Leaf::Concept(Concept::Value(named.clone())),
                        TemporalJsonEncoding::ExactV2,
                    )
                    .expect("document leaf"),
                    serde_json::json!("2024-10-27T01:30:00+01:00[Europe/London]"),
                    "document evidence uses the same exact datetime-tz bridge",
                );
                assert_eq!(
                    $concept_fn(&Concept::Value(named), TemporalJsonEncoding::DriverDisplay,)
                        .expect("row concept")["value"],
                    serde_json::json!("2024-10-27T01:30:00.000000000 Europe/London"),
                    "released conversion retains the upstream driver display",
                );

                let fixed = TimeZone::Fixed(
                    chrono::FixedOffset::east_opt(1_172).expect("second-resolution offset"),
                );
                let fixed_utc = "1900-01-01T11:40:28"
                    .parse::<NaiveDateTime>()
                    .expect("UTC datetime");
                let fixed = Value::DatetimeTZ(fixed.from_utc_datetime(&fixed_utc));
                assert_eq!(
                    $concept_fn(
                        &Concept::Value(fixed.clone()),
                        TemporalJsonEncoding::ExactV2,
                    )
                    .expect("row concept")["value"],
                    serde_json::json!("1900-01-01T12:00:00+00:19:32"),
                    "row evidence preserves fixed offset seconds",
                );
                assert_eq!(
                    $leaf_fn(
                        &Leaf::Concept(Concept::Value(fixed)),
                        TemporalJsonEncoding::ExactV2,
                    )
                    .expect("document leaf"),
                    serde_json::json!("1900-01-01T12:00:00+00:19:32"),
                    "document evidence preserves fixed offset seconds",
                );
            }
        };
    }

    datetime_tz_evidence_regression!(
        "band7",
        band7_rows_and_documents_preserve_exact_datetime_tz_evidence,
        driver_b7,
        concept_to_json_b7,
        document_leaf_to_json_b7
    );
    datetime_tz_evidence_regression!(
        "band8",
        band8_rows_and_documents_preserve_exact_datetime_tz_evidence,
        driver_b8,
        concept_to_json_b8,
        document_leaf_to_json_b8
    );
    datetime_tz_evidence_regression!(
        "band9",
        band9_rows_and_documents_preserve_exact_datetime_tz_evidence,
        driver_b9,
        concept_to_json_b9,
        document_leaf_to_json_b9
    );

    macro_rules! datetime_tz_local_range_regression {
        ($feature:literal, $name:ident, $driver:ident, $concept_fn:ident, $leaf_fn:ident) => {
            #[cfg(feature = $feature)]
            #[test]
            fn $name() {
                use chrono::{FixedOffset, NaiveDateTime, TimeZone as _};
                use $driver::answer::concept_document::Leaf;
                use $driver::concept::value::TimeZone;
                use $driver::concept::{Concept, Value};

                let assert_range_error = |error: RuntimeError| {
                    assert!(matches!(
                        error,
                        RuntimeError::QueryExecution(message)
                            if message
                                == "provider datetime-tz local value is outside the supported range"
                    ));
                };
                let invalid = [
                    (
                        FixedOffset::east_opt(1).expect("positive offset"),
                        NaiveDateTime::MAX,
                    ),
                    (
                        FixedOffset::west_opt(1).expect("negative offset"),
                        NaiveDateTime::MIN,
                    ),
                ];
                for (offset, utc) in invalid {
                    let timezone = TimeZone::Fixed(offset);
                    let value = Value::DatetimeTZ(timezone.from_utc_datetime(&utc));
                    for encoding in [
                        TemporalJsonEncoding::ExactV2,
                        TemporalJsonEncoding::DriverDisplay,
                    ] {
                        let row_error =
                            $concept_fn(&Concept::Value(value.clone()), encoding).expect_err(
                                "an unrepresentable provider local datetime must fail row conversion",
                            );
                        assert_range_error(row_error);

                        let document_error =
                            $leaf_fn(&Leaf::Concept(Concept::Value(value.clone())), encoding)
                                .expect_err(
                                    "an unrepresentable provider local datetime must fail document conversion",
                                );
                        assert_range_error(document_error);
                    }
                }

                let valid = [
                    (
                        FixedOffset::west_opt(1).expect("negative offset"),
                        NaiveDateTime::MAX,
                    ),
                    (
                        FixedOffset::east_opt(1).expect("positive offset"),
                        NaiveDateTime::MIN,
                    ),
                ];
                for (offset, utc) in valid {
                    let timezone = TimeZone::Fixed(offset);
                    let value = Value::DatetimeTZ(timezone.from_utc_datetime(&utc));
                    for encoding in [
                        TemporalJsonEncoding::ExactV2,
                        TemporalJsonEncoding::DriverDisplay,
                    ] {
                        let row = $concept_fn(&Concept::Value(value.clone()), encoding)
                            .expect("an inward offset must remain representable");
                        assert!(row["value"].is_string());

                        let document =
                            $leaf_fn(&Leaf::Concept(Concept::Value(value.clone())), encoding)
                                .expect("document conversion accepts an inward offset");
                        assert!(document.is_string());
                    }
                }
            }
        };
    }

    datetime_tz_local_range_regression!(
        "band7",
        band7_datetime_tz_local_range_errors_are_propagated,
        driver_b7,
        concept_to_json_b7,
        document_leaf_to_json_b7
    );
    datetime_tz_local_range_regression!(
        "band8",
        band8_datetime_tz_local_range_errors_are_propagated,
        driver_b8,
        concept_to_json_b8,
        document_leaf_to_json_b8
    );
    datetime_tz_local_range_regression!(
        "band9",
        band9_datetime_tz_local_range_errors_are_propagated,
        driver_b9,
        concept_to_json_b9,
        document_leaf_to_json_b9
    );

    macro_rules! exact_temporal_evidence_regression {
        ($feature:literal, $name:ident, $driver:ident, $concept_fn:ident, $leaf_fn:ident) => {
            #[cfg(feature = $feature)]
            #[test]
            fn $name() {
                use $driver::answer::concept_document::Leaf;
                use $driver::concept::value::Duration;
                use $driver::concept::{Concept, Value};

                let duration_cases = [
                    (Duration::new(0, 0, 0), "PT0S"),
                    (Duration::new(12, 0, 0), "P12M"),
                    (Duration::new(0, 0, 3_660_000_000_000), "PT3660S"),
                    (Duration::new(0, 0, 1_500_000_000), "PT1.5S"),
                    (
                        Duration::new(u32::MAX, u32::MAX, u64::MAX),
                        "P4294967295M4294967295DT18446744073.709551615S",
                    ),
                ];
                for (duration, expected) in duration_cases {
                    let value = Value::Duration(duration);
                    assert_eq!(
                        $concept_fn(
                            &Concept::Value(value.clone()),
                            TemporalJsonEncoding::ExactV2,
                        )
                        .expect("row concept")["value"],
                        serde_json::Value::String(expected.to_owned()),
                        "row evidence uses the contract's exact duration components",
                    );
                    assert_eq!(
                        $leaf_fn(
                            &Leaf::Concept(Concept::Value(value)),
                            TemporalJsonEncoding::ExactV2,
                        )
                        .expect("document leaf"),
                        serde_json::Value::String(expected.to_owned()),
                        "document evidence uses the same exact duration bridge",
                    );
                }

                let year = Value::Duration(Duration::new(12, 0, 0));
                assert_eq!(
                    $concept_fn(&Concept::Value(year), TemporalJsonEncoding::DriverDisplay,)
                        .expect("row concept")["value"],
                    serde_json::json!("P1Y"),
                    "released conversion retains the upstream duration display",
                );

                let datetime = chrono::NaiveDate::from_ymd_opt(2024, 1, 2)
                    .expect("date")
                    .and_hms_nano_opt(3, 4, 5, 500_000_000)
                    .expect("datetime");
                let datetime = Value::Datetime(datetime);
                assert_eq!(
                    $concept_fn(
                        &Concept::Value(datetime.clone()),
                        TemporalJsonEncoding::ExactV2,
                    )
                    .expect("row concept")["value"],
                    serde_json::json!("2024-01-02T03:04:05.5"),
                    "row evidence trims only insignificant datetime zeroes",
                );
                assert_eq!(
                    $leaf_fn(
                        &Leaf::Concept(Concept::Value(datetime.clone())),
                        TemporalJsonEncoding::ExactV2,
                    )
                    .expect("document leaf"),
                    serde_json::json!("2024-01-02T03:04:05.5"),
                    "document evidence uses the same exact datetime bridge",
                );
                assert_eq!(
                    $concept_fn(
                        &Concept::Value(datetime),
                        TemporalJsonEncoding::DriverDisplay,
                    )
                    .expect("row concept")["value"],
                    serde_json::json!("2024-01-02T03:04:05.500000000"),
                    "released conversion retains the upstream datetime display",
                );
            }
        };
    }

    exact_temporal_evidence_regression!(
        "band7",
        band7_rows_and_documents_preserve_exact_datetime_and_duration_evidence,
        driver_b7,
        concept_to_json_b7,
        document_leaf_to_json_b7
    );
    exact_temporal_evidence_regression!(
        "band8",
        band8_rows_and_documents_preserve_exact_datetime_and_duration_evidence,
        driver_b8,
        concept_to_json_b8,
        document_leaf_to_json_b8
    );
    exact_temporal_evidence_regression!(
        "band9",
        band9_rows_and_documents_preserve_exact_datetime_and_duration_evidence,
        driver_b9,
        concept_to_json_b9,
        document_leaf_to_json_b9
    );

    #[cfg(feature = "band9")]
    #[test]
    fn document_list_limit_rejects_before_json_collection() {
        use driver_b9::answer::concept_document::{Leaf, Node};
        let document = Node::List(vec![Node::Leaf(Some(Leaf::Empty)), Node::Leaf(None)]);
        let mut members = 0;
        let error =
            document_node_to_json_b9(&document, &mut members, 1, TemporalJsonEncoding::ExactV2)
                .expect_err("the list exceeds its conversion budget");
        assert!(matches!(
            error,
            RuntimeError::ResourceLimit {
                code: "query_v2_document_member_limit",
                ..
            }
        ));
        assert_eq!(members, 0, "rejected lists are not partially charged");
    }

    #[cfg(feature = "band9")]
    #[test]
    fn unexpected_document_thing_is_a_typed_error_not_a_panic() {
        use driver_b9::answer::concept_document::{Leaf, Node};
        use driver_b9::concept::{Concept, Entity};

        let document = Node::Leaf(Some(Leaf::Concept(Concept::Entity(Entity {
            iid: vec![1_u8].into(),
            type_: None,
        }))));
        let mut members = 0;
        let error = document_node_to_json_b9(
            &document,
            &mut members,
            u64::MAX,
            TemporalJsonEncoding::ExactV2,
        )
        .expect_err("thing instances are not valid fetch-document leaves");
        assert!(matches!(
            error,
            RuntimeError::QueryExecution(message)
                if message == "document response carried an unsupported thing instance"
        ));
    }

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
            driver_lease: None,
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
            driver_lease: None,
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
            driver_lease: None,
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

    #[test]
    fn released_boolean_options_map_to_the_typed_tls_truth_table() {
        let disabled = SecureConnectOptions::from(ConnectOptions {
            http_port: 8123,
            tls: false,
            server_version: Some(core_version::Version::new(3, 11, 5)),
        });
        assert_eq!(disabled.http_port, 8123);
        assert_eq!(disabled.tls_mode, TlsMode::Disabled);
        assert_eq!(
            disabled.server_version,
            Some(core_version::Version::new(3, 11, 5))
        );

        let native = SecureConnectOptions::from(ConnectOptions {
            http_port: 8124,
            tls: true,
            server_version: None,
        });
        assert_eq!(native.http_port, 8124);
        assert_eq!(native.tls_mode, TlsMode::NativeRoots);
        assert_eq!(native.server_version, None);
    }

    #[cfg(feature = "band7")]
    #[test]
    fn band7_tls_lowering_adds_only_the_required_https_scheme() {
        assert_eq!(
            band7_driver_address("db.example:1729", true),
            "https://db.example:1729"
        );
        assert_eq!(
            band7_driver_address("[::1]:1729", true),
            "https://[::1]:1729"
        );
        assert_eq!(
            band7_driver_address("db.example:1729", false),
            "db.example:1729"
        );
        assert_eq!(
            band7_driver_address("https://db.example:1729", true),
            "https://db.example:1729"
        );
        assert_eq!(
            band7_driver_address("http://db.example:1729", true),
            "http://db.example:1729",
            "explicit contradictory schemes remain errors at the driver boundary"
        );
    }

    #[test]
    fn every_enabled_fallback_band_lowers_to_tls_without_plaintext() {
        let disabled = ResolvedTlsMode::from_configured_path(TlsMode::Disabled).unwrap();
        assert!(!disabled.probe_mode.is_enabled());
        #[cfg(feature = "band7")]
        assert!(!disabled.band7.is_tls_enabled());
        #[cfg(feature = "band8")]
        assert!(!disabled.band8.is_enabled());
        #[cfg(feature = "band9")]
        assert!(!disabled.band9.is_enabled());

        let native = ResolvedTlsMode::from_configured_path(TlsMode::NativeRoots).unwrap();
        assert!(native.probe_mode.is_enabled());
        #[cfg(feature = "band7")]
        assert!(native.band7.is_tls_enabled());
        #[cfg(feature = "band8")]
        assert!(native.band8.is_enabled());
        #[cfg(feature = "band9")]
        assert!(native.band9.is_enabled());

        let root_ca = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/root-ca.pem");
        let custom =
            ResolvedTlsMode::from_configured_path(TlsMode::CustomRootCa(root_ca.clone())).unwrap();
        assert!(custom.probe_mode.is_enabled());
        #[cfg(any(feature = "band8", feature = "band9"))]
        let expected_root = std::fs::read(&root_ca).unwrap();
        #[cfg(feature = "band7")]
        assert!(custom.band7.is_tls_enabled());
        #[cfg(feature = "band8")]
        {
            assert!(custom.band8.is_enabled());
            assert_eq!(
                std::fs::read(custom.band8.root_ca_path().unwrap()).unwrap(),
                expected_root
            );
        }
        #[cfg(feature = "band9")]
        {
            assert!(custom.band9.is_enabled());
            assert_eq!(
                std::fs::read(custom.band9.root_ca_path().unwrap()).unwrap(),
                expected_root
            );
        }
    }

    #[test]
    fn every_band_lowers_from_retained_material_after_path_replacement() {
        let sequence = NEXT_TLS_MATERIAL_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "type-bridge-runtime-root-replacement-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        let configured = directory.join("root.pem");
        let moved = directory.join("loaded-root.pem");
        let original = include_bytes!("../tests/fixtures/root-ca.pem");
        std::fs::write(&configured, original).unwrap();

        let material = core_version::RetainedCustomRootCa::load(&configured).unwrap();
        match std::fs::rename(&configured, &moved) {
            Ok(()) => {
                std::fs::write(&configured, b"replacement is not a certificate\n").unwrap();
            }
            Err(_) => {
                // Windows' retained no-delete handle makes the replacement
                // itself fail closed. It must also prevent an in-place write.
                assert!(
                    std::fs::write(&configured, b"replacement is not a certificate\n").is_err()
                );
            }
        }

        let resolved = ResolvedTlsMode::lower(
            TlsMode::CustomRootCa(configured),
            ResolvedTlsProbeMode::CustomRootCa(material),
        )
        .expect("all bands must lower the retained original material");

        #[cfg(feature = "band8")]
        assert_eq!(
            std::fs::read(resolved.band8.root_ca_path().unwrap()).unwrap(),
            original
        );
        #[cfg(feature = "band9")]
        assert_eq!(
            std::fs::read(resolved.band9.root_ca_path().unwrap()).unwrap(),
            original
        );

        drop(resolved);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn every_band_lowers_from_retained_material_after_parent_swap() {
        let sequence = NEXT_TLS_MATERIAL_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "type-bridge-runtime-root-parent-swap-{}-{sequence}",
            std::process::id()
        ));
        let configured_parent = root.join("configured-parent");
        let moved_parent = root.join("loaded-parent");
        std::fs::create_dir_all(&configured_parent).unwrap();
        let configured = configured_parent.join("root.pem");
        let original = include_bytes!("../tests/fixtures/root-ca.pem");
        std::fs::write(&configured, original).unwrap();

        let material = core_version::RetainedCustomRootCa::load(&configured).unwrap();
        match std::fs::rename(&configured_parent, &moved_parent) {
            Ok(()) => {
                std::fs::create_dir(&configured_parent).unwrap();
                std::fs::write(&configured, b"replacement is not a certificate\n").unwrap();
            }
            Err(_) => {
                // Windows retains the parent without delete sharing, so a
                // namespace swap is rejected before any driver lowering.
                assert_eq!(std::fs::read(&configured).unwrap(), original);
            }
        }

        let resolved = ResolvedTlsMode::lower(
            TlsMode::CustomRootCa(configured),
            ResolvedTlsProbeMode::CustomRootCa(material),
        )
        .expect("all bands must lower the retained parent/file material");

        #[cfg(feature = "band8")]
        assert_eq!(
            std::fs::read(resolved.band8.root_ca_path().unwrap()).unwrap(),
            original
        );
        #[cfg(feature = "band9")]
        assert_eq!(
            std::fs::read(resolved.band9.root_ca_path().unwrap()).unwrap(),
            original
        );

        drop(resolved);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn every_band_lowers_from_snapshot_after_in_place_source_overwrite() {
        let sequence = NEXT_TLS_MATERIAL_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "type-bridge-runtime-root-overwrite-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        let configured = directory.join("root.pem");
        let original = include_bytes!("../tests/fixtures/root-ca.pem");
        std::fs::write(&configured, original).unwrap();

        let material = core_version::RetainedCustomRootCa::load(&configured).unwrap();
        match std::fs::write(&configured, b"overwritten source is not a certificate\n") {
            Ok(()) => assert_ne!(std::fs::read(&configured).unwrap(), original),
            Err(_) => assert_eq!(std::fs::read(&configured).unwrap(), original),
        }

        let resolved = ResolvedTlsMode::lower(
            TlsMode::CustomRootCa(configured),
            ResolvedTlsProbeMode::CustomRootCa(material),
        )
        .expect("all bands must lower the captured-byte snapshot");

        #[cfg(feature = "band8")]
        assert_eq!(
            std::fs::read(resolved.band8.root_ca_path().unwrap()).unwrap(),
            original
        );
        #[cfg(feature = "band9")]
        assert_eq!(
            std::fs::read(resolved.band9.root_ca_path().unwrap()).unwrap(),
            original
        );

        drop(resolved);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn prepared_transport_never_rereads_mutated_custom_root_path() {
        let sequence = NEXT_TLS_MATERIAL_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "type-bridge-runtime-root-prepared-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        let configured = directory.join("root.pem");
        let original = include_bytes!("../tests/fixtures/root-ca.pem");
        std::fs::write(&configured, original).unwrap();

        let prepared = SecureConnectOptions {
            http_port: 8123,
            tls_mode: TlsMode::CustomRootCa(configured.clone()),
            server_version: None,
        }
        .prepare_transport()
        .expect("prepare the complete custom-root transport");

        match std::fs::write(&configured, b"mutated after preparation\n") {
            Ok(()) => assert_ne!(std::fs::read(&configured).unwrap(), original),
            Err(_) => assert_eq!(std::fs::read(&configured).unwrap(), original),
        }

        #[cfg(feature = "band8")]
        assert_eq!(
            std::fs::read(prepared.resolved_tls.band8.root_ca_path().unwrap()).unwrap(),
            original
        );
        #[cfg(feature = "band9")]
        assert_eq!(
            std::fs::read(prepared.resolved_tls.band9.root_ca_path().unwrap()).unwrap(),
            original
        );

        let expected = original.to_vec();
        let error = gated_driver_prepared_with_probe(
            "host must not be constructed",
            "credential must not be used",
            "credential must not be used",
            prepared.clone(),
            move |_address, port, mode| {
                assert_eq!(port, 8123);
                let ResolvedTlsProbeMode::CustomRootCa(material) = mode else {
                    panic!("prepared probe must retain custom-root mode");
                };
                let bytes = material
                    .with_driver_root_path(|path| std::fs::read(path))
                    .unwrap()
                    .unwrap();
                assert_eq!(bytes, expected);
                Err(core_version::VersionProbeError::TlsConfiguration(
                    core_version::TlsConfigurationError::NativeRootsUnavailable,
                ))
            },
        )
        .await
        .err()
        .expect("injected terminal probe error prevents host construction");
        assert!(matches!(
            error,
            SecureConnectError::TlsConfiguration(
                core_version::TlsConfigurationError::NativeRootsUnavailable
            )
        ));

        drop(prepared);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn binding_transport_resolves_a_raw_alias_without_weakening_physical_paths() {
        use std::os::unix::fs::symlink;

        let sequence = NEXT_TLS_MATERIAL_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "type-bridge-runtime-binding-alias-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).expect("create binding-alias directory");
        let physical_parent = directory.join("physical");
        let alias_parent = directory.join("alias");
        std::fs::create_dir(&physical_parent).expect("create physical CA parent");
        std::fs::write(
            physical_parent.join("root.pem"),
            include_bytes!("../tests/fixtures/root-ca.pem"),
        )
        .expect("write binding CA");
        symlink(&physical_parent, &alias_parent).expect("create caller path alias");
        let configured = alias_parent.join("root.pem");
        let options = SecureConnectOptions {
            http_port: 8123,
            tls_mode: TlsMode::CustomRootCa(configured.clone()),
            server_version: None,
        };

        assert!(matches!(
            options.prepare_transport_from_validated_physical_path(),
            Err(SecureConnectError::TlsConfiguration(
                core_version::TlsConfigurationError::CustomRootCaUnreadable { path }
            )) if path == configured
        ));
        options
            .prepare_transport()
            .expect("raw binding preparation resolves one caller alias");
        std::fs::remove_dir_all(directory).expect("remove binding-alias directory");
    }

    #[test]
    fn public_transport_preflight_returns_typed_errors_without_a_host() {
        let missing =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/does-not-exist.pem");
        let options = SecureConnectOptions {
            http_port: 8123,
            tls_mode: TlsMode::CustomRootCa(missing.clone()),
            server_version: Some(core_version::Version::new(3, 11, 5)),
        };

        let error = options
            .validate_transport()
            .expect_err("preflight must validate trust material synchronously");
        assert_eq!(
            error.configuration_code(),
            Some("tls_custom_root_ca_unreadable")
        );
        assert!(matches!(
            error,
            SecureConnectError::TlsConfiguration(
                core_version::TlsConfigurationError::CustomRootCaUnreadable { path }
            ) if path == missing
        ));
    }

    #[tokio::test]
    async fn invalid_custom_root_is_rejected_before_exact_version_or_probe_io() {
        let probe_called = Arc::new(Mutex::new(false));
        let captured = Arc::clone(&probe_called);
        let missing =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/does-not-exist.pem");

        let error = gated_driver_secure_with_probe(
            "invalid host that must not be constructed",
            "admin",
            "password",
            SecureConnectOptions {
                http_port: 8123,
                tls_mode: TlsMode::CustomRootCa(missing.clone()),
                server_version: Some(core_version::Version::new(3, 11, 5)),
            },
            move |_address, _port, _mode| {
                *captured.lock().unwrap() = true;
                Ok(core_version::Version::new(3, 11, 5))
            },
        )
        .await
        .err()
        .expect("invalid custom root must fail before connection construction");

        assert!(!*probe_called.lock().unwrap());
        assert!(matches!(
            error,
            SecureConnectError::TlsConfiguration(
                core_version::TlsConfigurationError::CustomRootCaUnreadable { path }
            ) if path == missing
        ));
    }

    #[tokio::test]
    async fn enabled_probe_tls_failure_is_terminal_before_grpc_fallback() {
        let error = gated_driver_secure_with_probe(
            "127.0.0.1:1",
            "admin",
            "password",
            SecureConnectOptions {
                http_port: 8123,
                tls_mode: TlsMode::NativeRoots,
                server_version: None,
            },
            |_address, _port, mode| {
                assert!(matches!(mode, ResolvedTlsProbeMode::NativeRoots));
                Err(core_version::VersionProbeError::TlsConfiguration(
                    core_version::TlsConfigurationError::NativeRootsUnavailable,
                ))
            },
        )
        .await
        .err()
        .expect("TLS configuration failure must be terminal");

        assert!(matches!(
            error,
            SecureConnectError::TlsConfiguration(
                core_version::TlsConfigurationError::NativeRootsUnavailable
            )
        ));
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
            .find(|block| block.contains("name = \"type-bridge-typedb-driver-b8\""))
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
            .expect("type-bridge-typedb-driver-b8 entry not found in Cargo.lock");

        assert_eq!(
            lock_version, PINNED_DRIVER_VERSION,
            "Cargo.lock resolves type-bridge-typedb-driver-b8 {lock_version} but \
             PINNED_DRIVER_VERSION \
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
            GivenValue::Empty,
            GivenValue::Date("2026-07-13".into()),
            GivenValue::Datetime("2026-07-13T10:30:00".into()),
            GivenValue::DatetimeTz("2026-07-13T10:30:00+09:00".into()),
            GivenValue::DatetimeTz("2026-07-13T10:30:00+00:19:32".into()),
            GivenValue::DatetimeTzExact {
                local: "2024-10-27T01:30:00".into(),
                named_zone: Some("Europe/London".into()),
                effective_offset_seconds: 3_600,
            },
            GivenValue::DatetimeTzExact {
                local: "2024-07-01T12:00:00".into(),
                named_zone: Some("europe/amsterdam".into()),
                effective_offset_seconds: 7_200,
            },
            GivenValue::DatetimeTzExact {
                local: "1900-01-01T12:00:00".into(),
                named_zone: None,
                effective_offset_seconds: 1_172,
            },
            GivenValue::Decimal("12.30dec".into()),
            GivenValue::Decimal("-9223372036854775808".into()),
            GivenValue::Duration {
                months: u32::MAX,
                days: u32::MAX,
                nanos: u64::MAX,
            },
            GivenValue::Boolean(true),
            GivenValue::Double(1.5),
        ] {
            given_entry_b9(value.clone()).unwrap_or_else(|e| panic!("{value:?} must convert: {e}"));
        }
    }

    #[cfg(feature = "band9")]
    #[test]
    fn exact_given_entries_preserve_named_timezone_and_duration_components() {
        use typedb_driver::concept::Value;
        use typedb_driver::concept::value::TimeZone;
        use typedb_driver::given::GivenRowEntry;

        let named = given_entry_b9(GivenValue::DatetimeTzExact {
            local: "2024-10-27T01:30:00".into(),
            named_zone: Some("Europe/London".into()),
            effective_offset_seconds: 3_600,
        })
        .expect("explicit overlap side");
        let GivenRowEntry::Value(Value::DatetimeTZ(named)) = named else {
            panic!("datetime-tz given entry")
        };
        let TimeZone::IANA(timezone) = named.timezone() else {
            panic!("authored IANA identity must survive transport")
        };
        assert_eq!(timezone.name(), "Europe/London");
        assert_eq!(named.naive_local().to_string(), "2024-10-27 01:30:00");
        assert_eq!(
            named
                .naive_local()
                .signed_duration_since(named.naive_utc())
                .num_seconds(),
            3_600
        );

        let lower_case = given_entry_b9(GivenValue::DatetimeTzExact {
            local: "2024-07-01T12:00:00".into(),
            named_zone: Some("europe/amsterdam".into()),
            effective_offset_seconds: 7_200,
        })
        .expect("case-insensitive names admitted by schema also lower");
        let GivenRowEntry::Value(Value::DatetimeTZ(lower_case)) = lower_case else {
            panic!("datetime-tz given entry")
        };
        let TimeZone::IANA(timezone) = lower_case.timezone() else {
            panic!("IANA identity must survive transport")
        };
        assert_eq!(
            timezone.name(),
            "Europe/Amsterdam",
            "the provider canonicalizes accepted authored case"
        );

        let duration = given_entry_b9(GivenValue::Duration {
            months: u32::MAX,
            days: u32::MAX,
            nanos: u64::MAX,
        })
        .expect("duration boundary");
        let GivenRowEntry::Value(Value::Duration(duration)) = duration else {
            panic!("duration given entry")
        };
        assert_eq!(duration.months, u32::MAX);
        assert_eq!(duration.days, u32::MAX);
        assert_eq!(duration.nanos, u64::MAX);
    }

    #[cfg(feature = "band9")]
    #[test]
    fn given_entry_rejects_malformed_temporal() {
        for value in [
            GivenValue::Date("not-a-date".into()),
            GivenValue::Datetime("2026-13-45T99:00:00".into()),
            GivenValue::DatetimeTz("2026-07-13 10:30".into()),
            GivenValue::DatetimeTz("2026-07-13T10:30:00+24:00".into()),
            GivenValue::DatetimeTzExact {
                local: "2024-10-27T01:30:00".into(),
                named_zone: Some("Europe/London".into()),
                effective_offset_seconds: 7_200,
            },
            GivenValue::DatetimeTzExact {
                local: "2024-03-31T01:30:00".into(),
                named_zone: Some("Europe/London".into()),
                effective_offset_seconds: 3_600,
            },
            GivenValue::Decimal("1.00000000000000000000".into()),
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
            lock_version, PINNED_DRIVER_VERSION_B9,
            "Cargo.lock resolves typedb-driver {lock_version} but \
             PINNED_DRIVER_VERSION_B9 is {PINNED_DRIVER_VERSION_B9}; update the runtime constant"
        );

        let pinned: core_version::Version = PINNED_DRIVER_VERSION_B9.parse().unwrap();
        assert_eq!(
            core_version::band(&pinned),
            Some(9),
            "pinned band-9 driver version {PINNED_DRIVER_VERSION_B9} left protocol band 9; \
             review the gate expectations before accepting the bump"
        );
    }
}
