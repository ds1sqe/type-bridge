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

use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use type_bridge_core_lib::version as core_version;
use type_bridge_core_lib::version::DEFAULT_HTTP_PORT;

#[cfg(feature = "band8")]
use typedb_driver::answer::QueryAnswer as B8QueryAnswer;
#[cfg(feature = "band8")]
use typedb_driver::{
    Addresses, Credentials as B8Credentials, DriverOptions, DriverTlsConfig,
    TransactionType as B8TransactionType, TypeDBDriver as B8Driver,
};

#[cfg(feature = "band7")]
use type_bridge_typedb_driver_b7::answer::QueryAnswer as B7QueryAnswer;
#[cfg(feature = "band7")]
use type_bridge_typedb_driver_b7::{
    Credentials as B7Credentials, DriverOptions as B7DriverOptions,
    TransactionType as B7TransactionType, TypeDBDriver as B7Driver,
};

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
    // band, so a band-7 server is served, and a dual-band 3.12 server is
    // served through its band-8 acceptance.  After this passes, negotiation
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
    let mut failures = vec![format!("HTTP version probe failed: {http_error}")];

    #[cfg(feature = "band9")]
    {
        match connect_band9_driver(address, username, password).await {
            Ok(driver) => {
                let reported = driver.server_version().await.map_err(|e| {
                    RuntimeError::Connection(format!(
                        "Band-9 gRPC version validation failed after connect to {address}: {e}"
                    ))
                })?;
                let server_version = reported
                    .version()
                    .parse::<core_version::Version>()
                    .map_err(RuntimeError::UnsupportedVersion)?;
                validate_server_band(&server_version)?;
                if !core_version::server_accepted_bands(&server_version).contains(&9) {
                    return Err(RuntimeError::UnsupportedVersion(
                        core_version::VersionError::Probe(format!(
                            "band-9 gRPC connection reported server version {server_version}, \
                             which does not accept band 9"
                        )),
                    ));
                }
                tracing::debug!(
                    address,
                    server_version = %server_version,
                    "Connected through gRPC band-9 fallback after HTTP version probe failed"
                );
                return Ok((DriverHandle::B9(driver), Some(server_version)));
            }
            Err(error) => failures.push(format!("band-9 gRPC attempt failed: {error}")),
        }
    }

    #[cfg(not(feature = "band9"))]
    failures.push("band-9 gRPC attempt skipped: band9 feature is not compiled in".to_string());

    #[cfg(feature = "band8")]
    {
        match connect_band8_driver(address, username, password).await {
            Ok(driver) => {
                let reported = driver.server_version().await.map_err(|e| {
                    RuntimeError::Connection(format!(
                        "Band-8 gRPC version validation failed after connect to {address}: {e}"
                    ))
                })?;
                let server_version = reported
                    .version()
                    .parse::<core_version::Version>()
                    .map_err(RuntimeError::UnsupportedVersion)?;
                // Window + embedded-band gate, then confirm the server accepts
                // band 8.  The acceptance check (not negotiate == 8): a 3.12
                // server reached here negotiates its native band 9 when band9
                // is embedded, but this band-8 connection is still legitimate
                // through the server's band-8 backward acceptance.
                validate_server_band(&server_version)?;
                if !core_version::server_accepted_bands(&server_version).contains(&8) {
                    return Err(RuntimeError::UnsupportedVersion(
                        core_version::VersionError::Probe(format!(
                            "band-8 gRPC connection reported server version {server_version}, \
                             which does not accept band 8"
                        )),
                    ));
                }
                tracing::debug!(
                    address,
                    server_version = %server_version,
                    "Connected through gRPC band-8 fallback after HTTP version probe failed"
                );
                return Ok((DriverHandle::B8(driver), Some(server_version)));
            }
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

impl RuntimeTransaction {
    /// Execute TypeQL within this transaction.
    pub fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult>> {
        let tql = typeql.to_string();
        Box::pin(async move {
            match &self.inner {
                #[cfg(feature = "band7")]
                RuntimeTransactionInner::B7(opt) => {
                    let tx = opt.as_ref().ok_or_else(|| {
                        RuntimeError::Transaction("Transaction already consumed".into())
                    })?;
                    let answer = tx
                        .query(&tql)
                        .await
                        .map_err(|e| RuntimeError::QueryExecution(format!("{e}")))?;
                    match answer {
                        B7QueryAnswer::Ok(_) => Ok(QueryResult::Ok),
                        B7QueryAnswer::ConceptRowStream(_, stream) => {
                            let rows: Vec<_> = stream.try_collect().await.map_err(|e| {
                                RuntimeError::QueryExecution(format!("Row collect: {e}"))
                            })?;
                            let json_rows = rows
                                .iter()
                                .map(|row| {
                                    let mut obj = serde_json::Map::new();
                                    for (i, col) in row.get_column_names().iter().enumerate() {
                                        let value = row
                                            .row
                                            .get(i)
                                            .and_then(|c| c.as_ref())
                                            .map(concept_to_json_b7)
                                            .unwrap_or(serde_json::Value::Null);
                                        obj.insert(col.clone(), value);
                                    }
                                    serde_json::Value::Object(obj)
                                })
                                .collect();
                            Ok(QueryResult::Rows(json_rows))
                        }
                        B7QueryAnswer::ConceptDocumentStream(_, stream) => {
                            let docs: Vec<_> = stream.try_collect().await.map_err(|e| {
                                RuntimeError::QueryExecution(format!("Doc collect: {e}"))
                            })?;
                            let json_docs = docs
                                .into_iter()
                                .map(|doc| {
                                    serde_json::to_value(doc.into_json())
                                        .unwrap_or(serde_json::Value::Null)
                                })
                                .collect();
                            Ok(QueryResult::Documents(json_docs))
                        }
                    }
                }
                #[cfg(feature = "band8")]
                RuntimeTransactionInner::B8(opt) => {
                    let tx = opt.as_ref().ok_or_else(|| {
                        RuntimeError::Transaction("Transaction already consumed".into())
                    })?;
                    let answer = tx
                        .query(&tql)
                        .await
                        .map_err(|e| RuntimeError::QueryExecution(format!("{e}")))?;
                    match answer {
                        B8QueryAnswer::Ok(_) => Ok(QueryResult::Ok),
                        B8QueryAnswer::ConceptRowStream(_, stream) => {
                            let rows: Vec<_> = stream.try_collect().await.map_err(|e| {
                                RuntimeError::QueryExecution(format!("Row collect: {e}"))
                            })?;
                            let json_rows = rows
                                .iter()
                                .map(|row| {
                                    let mut obj = serde_json::Map::new();
                                    for (i, col) in row.get_column_names().iter().enumerate() {
                                        let value = row
                                            .row
                                            .get(i)
                                            .and_then(|c| c.as_ref())
                                            .map(concept_to_json_b8)
                                            .unwrap_or(serde_json::Value::Null);
                                        obj.insert(col.clone(), value);
                                    }
                                    serde_json::Value::Object(obj)
                                })
                                .collect();
                            Ok(QueryResult::Rows(json_rows))
                        }
                        B8QueryAnswer::ConceptDocumentStream(_, stream) => {
                            let docs: Vec<_> = stream.try_collect().await.map_err(|e| {
                                RuntimeError::QueryExecution(format!("Doc collect: {e}"))
                            })?;
                            let json_docs = docs
                                .into_iter()
                                .map(|doc| {
                                    serde_json::to_value(doc.into_json())
                                        .unwrap_or(serde_json::Value::Null)
                                })
                                .collect();
                            Ok(QueryResult::Documents(json_docs))
                        }
                    }
                }
                #[cfg(feature = "band9")]
                RuntimeTransactionInner::B9(opt) => {
                    let tx = opt.as_ref().ok_or_else(|| {
                        RuntimeError::Transaction("Transaction already consumed".into())
                    })?;
                    let answer = tx
                        .query(&tql)
                        .await
                        .map_err(|e| RuntimeError::QueryExecution(format!("{e}")))?;
                    convert_answer_b9(answer).await
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
        let tql = typeql.to_string();
        Box::pin(async move {
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
                    let answer = tx
                        .query_with_rows(&tql, given_rows)
                        .await
                        .map_err(|e| RuntimeError::QueryExecution(format!("{e}")))?;
                    convert_answer_b9(answer).await
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
                    t.commit()
                        .await
                        .map_err(|e| RuntimeError::Transaction(format!("Commit failed: {e}")))
                })
            }
            #[cfg(feature = "band8")]
            RuntimeTransactionInner::B8(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    let t = tx.ok_or_else(|| {
                        RuntimeError::Transaction("Transaction already consumed".into())
                    })?;
                    t.commit()
                        .await
                        .map_err(|e| RuntimeError::Transaction(format!("Commit failed: {e}")))
                })
            }
            #[cfg(feature = "band9")]
            RuntimeTransactionInner::B9(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    let t = tx.ok_or_else(|| {
                        RuntimeError::Transaction("Transaction already consumed".into())
                    })?;
                    t.commit()
                        .await
                        .map_err(|e| RuntimeError::Transaction(format!("Commit failed: {e}")))
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

/// Convert a band-7 TypeDB value to a JSON value.
#[cfg(feature = "band7")]
fn value_to_json_b7(value: &type_bridge_typedb_driver_b7::concept::Value) -> serde_json::Value {
    if let Some(b) = value.get_boolean() {
        return serde_json::Value::Bool(b);
    }
    if let Some(i) = value.get_integer() {
        return serde_json::json!(i);
    }
    if let Some(d) = value.get_double() {
        return serde_json::json!(d);
    }
    if let Some(s) = value.get_string() {
        return serde_json::Value::String(s.to_string());
    }
    if let Some(date) = value.get_date() {
        return serde_json::Value::String(date.to_string());
    }
    if let Some(dt) = value.get_datetime() {
        return serde_json::Value::String(dt.to_string());
    }
    if let Some(dt_tz) = value.get_datetime_tz() {
        return serde_json::Value::String(dt_tz.to_string());
    }
    if let Some(dec) = value.get_decimal() {
        return serde_json::Value::String(dec.to_string());
    }
    if let Some(dur) = value.get_duration() {
        return serde_json::Value::String(dur.to_string());
    }
    serde_json::Value::String(value.to_string())
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

/// Convert a band-8 TypeDB value to a JSON value.
#[cfg(feature = "band8")]
fn value_to_json_b8(value: &typedb_driver::concept::Value) -> serde_json::Value {
    if let Some(b) = value.get_boolean() {
        return serde_json::Value::Bool(b);
    }
    if let Some(i) = value.get_integer() {
        return serde_json::json!(i);
    }
    if let Some(d) = value.get_double() {
        return serde_json::json!(d);
    }
    if let Some(s) = value.get_string() {
        return serde_json::Value::String(s.to_string());
    }
    if let Some(date) = value.get_date() {
        return serde_json::Value::String(date.to_string());
    }
    if let Some(dt) = value.get_datetime() {
        return serde_json::Value::String(dt.to_string());
    }
    if let Some(dt_tz) = value.get_datetime_tz() {
        return serde_json::Value::String(dt_tz.to_string());
    }
    if let Some(dec) = value.get_decimal() {
        return serde_json::Value::String(dec.to_string());
    }
    if let Some(dur) = value.get_duration() {
        return serde_json::Value::String(dur.to_string());
    }
    serde_json::Value::String(value.to_string())
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

/// Convert a band-9 [`B9QueryAnswer`] into the runtime's [`QueryResult`].
///
/// Shared by the plain `query` arm and the given-stage `query_with_rows`
/// path — both produce the same driver answer type.
#[cfg(feature = "band9")]
async fn convert_answer_b9(answer: B9QueryAnswer) -> Result<QueryResult> {
    match answer {
        B9QueryAnswer::Ok(_) => Ok(QueryResult::Ok),
        B9QueryAnswer::ConceptRowStream(_, stream) => {
            let rows: Vec<_> = stream
                .try_collect()
                .await
                .map_err(|e| RuntimeError::QueryExecution(format!("Row collect: {e}")))?;
            let json_rows = rows
                .iter()
                .map(|row| {
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
                    serde_json::Value::Object(obj)
                })
                .collect();
            Ok(QueryResult::Rows(json_rows))
        }
        B9QueryAnswer::ConceptDocumentStream(_, stream) => {
            let docs: Vec<_> = stream
                .try_collect()
                .await
                .map_err(|e| RuntimeError::QueryExecution(format!("Doc collect: {e}")))?;
            let json_docs = docs
                .into_iter()
                .map(|doc| serde_json::to_value(doc.into_json()).unwrap_or(serde_json::Value::Null))
                .collect();
            Ok(QueryResult::Documents(json_docs))
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

/// Convert a band-9 TypeDB value to a JSON value.
#[cfg(feature = "band9")]
fn value_to_json_b9(value: &type_bridge_typedb_driver_b9::concept::Value) -> serde_json::Value {
    if let Some(b) = value.get_boolean() {
        return serde_json::Value::Bool(b);
    }
    if let Some(i) = value.get_integer() {
        return serde_json::json!(i);
    }
    if let Some(d) = value.get_double() {
        return serde_json::json!(d);
    }
    if let Some(s) = value.get_string() {
        return serde_json::Value::String(s.to_string());
    }
    if let Some(date) = value.get_date() {
        return serde_json::Value::String(date.to_string());
    }
    if let Some(dt) = value.get_datetime() {
        return serde_json::Value::String(dt.to_string());
    }
    if let Some(dt_tz) = value.get_datetime_tz() {
        return serde_json::Value::String(dt_tz.to_string());
    }
    if let Some(dec) = value.get_decimal() {
        return serde_json::Value::String(dec.to_string());
    }
    if let Some(dur) = value.get_duration() {
        return serde_json::Value::String(dur.to_string());
    }
    serde_json::Value::String(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

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
                assert!(msg.contains("band-9 gRPC attempt failed"), "{msg}");
                assert!(msg.contains("band-8 gRPC attempt failed"), "{msg}");
                assert!(msg.contains("band-7 gRPC attempt failed"), "{msg}");
            }
            Err(other) => panic!("expected aggregated version-probe failure, got {other}"),
            Ok(_) => panic!("expected aggregated version-probe failure, got successful connection"),
        }
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
