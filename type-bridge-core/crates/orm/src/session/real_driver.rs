//! Real TypeDB backend using the `typedb-driver` crate.
//!
//! This module is only compiled when the `typedb` feature is enabled.

#[cfg(not(any(feature = "band7", feature = "band8")))]
compile_error!(
    "type-bridge-orm: the `typedb` machinery requires at least one band feature; enable `band7` and/or `band8` (both are default)"
);

use futures::TryStreamExt;
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

use super::backend::{BoxFuture, DriverBackend, QueryResult, TransactionOps, TxType};
use crate::error::OrmError;

/// The compile-pinned `typedb-driver` crate version resolved in `Cargo.lock`.
///
/// This constant records the exact driver version the ORM crate was built
/// against.  It is the factual counterpart to the policy window declared in
/// `type_bridge_core_lib::version`: the window says which versions are allowed;
/// this constant says which version is currently in use.
///
/// When the `typedb-driver` dependency is bumped, update this constant to
/// match the new `Cargo.lock` entry — the `version_gate_tests::cargo_lock_pin`
/// test will catch any divergence.
pub const PINNED_DRIVER_VERSION: &str = "3.11.5";

/// The compile-pinned version of the vendored band-7 driver fork
/// (`type-bridge-typedb-driver-b7`) this ORM crate was built against.
///
/// Mirrors [`PINNED_DRIVER_VERSION`] for the band-8 upstream driver.  When
/// the band-7 fork is refreshed, update this constant to match the new
/// `Cargo.lock` entry — the `version_gate_tests::cargo_lock_pin` test will
/// catch any divergence.
pub const PINNED_DRIVER_VERSION_B7: &str = "3.8.1";

/// Protocol bands this build embeds a driver for, derived from the compiled-in
/// band features.  This is the `embedded_bands` argument to
/// [`core_version::check_server_supported`] — the embedded-runtime gate accepts
/// any in-window server whose band is in this set.
///
/// Each element is individually cfg-gated so the set reflects exactly the bands
/// the build can construct; there is no hardcoded band-set literal (master-plan
/// I6).  The default build compiles both, so the set is `{7, 8}`.
const EMBEDDED_BANDS: &[u8] = &[
    #[cfg(feature = "band7")]
    7,
    #[cfg(feature = "band8")]
    8,
];

/// Return the driver versions compiled into this build, keyed by protocol band.
///
/// Each entry is `(band, version_string)` and is individually cfg-gated so only
/// compiled-in bands appear in the slice (master-plan I6 — no hardcoded
/// band-set literal).  The default build embeds both bands and returns
/// `[(7, "3.8.1"), (8, "3.11.5")]`.
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
    ]
}

/// Connection-time options for the version-gated TypeDB ORM session.
///
/// Callers that accept the defaults can pass `ConnectOptions::default()`.
#[derive(Debug, Clone, Copy)]
pub struct ConnectOptions {
    /// Port of the TypeDB HTTP API used for the connect-time version probe.
    pub http_port: u16,
    /// Whether to use TLS for the HTTP version probe.
    pub tls: bool,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            http_port: DEFAULT_HTTP_PORT,
            tls: false,
        }
    }
}

/// Band-tagged driver handle.  Crate-private; no driver type escapes the crate.
pub(crate) enum DriverHandle {
    #[cfg(feature = "band7")]
    B7(B7Driver),
    #[cfg(feature = "band8")]
    B8(B8Driver),
}

/// Real TypeDB backend wrapping a band-tagged [`DriverHandle`].
pub struct RealBackend {
    driver: DriverHandle,
}

/// Version-gated [`DriverHandle`] constructor with an injectable probe.
///
/// The `probe` closure is called with `(address, http_port, tls)` and must
/// return the server version or a [`core_version::VersionError`].  Production
/// code passes [`core_version::server_version`]; tests inject a recording
/// closure to exercise the gate without a live server.
pub(crate) async fn gated_driver_with_probe<F>(
    address: &str,
    username: &str,
    password: &str,
    options: ConnectOptions,
    probe: F,
) -> Result<DriverHandle, OrmError>
where
    F: FnOnce(&str, u16, bool) -> Result<core_version::Version, core_version::VersionError>
        + Send
        + 'static,
{
    // Probe the server version over HTTP (blocking I/O; offload to a
    // dedicated thread so we don't block the async executor).
    let address_owned = address.to_string();
    let http_port = options.http_port;
    let tls = options.tls;
    let server_version = tokio::task::spawn_blocking(move || probe(&address_owned, http_port, tls))
        .await
        .map_err(|e| OrmError::Connection(format!("Version probe task panicked: {e}")))?
        .map_err(OrmError::UnsupportedVersion)?;

    // Embedded-runtime gate: accept the server when it is in-window and its
    // band is one this build embedded.  Unlike the installed-driver gate
    // (check_supported, band equality), the embedded runtime carries every
    // compiled-in band, so a band-7 server is now served, not rejected.  After
    // this passes, band(&server_version) is guaranteed ∈ EMBEDDED_BANDS.
    core_version::check_server_supported(&server_version, EMBEDDED_BANDS)
        .map_err(OrmError::UnsupportedVersion)?;

    let band = core_version::band(&server_version);

    tracing::debug!(
        address,
        band = ?band,
        server_version = %server_version,
        "Embedded version gate passed"
    );

    #[cfg(feature = "band7")]
    if band == Some(7) {
        // Band-7 driver takes a raw address string and a two-argument
        // DriverOptions::new(tls_enabled, ca_path).  TLS is disabled.
        let opts = B7DriverOptions::new(false, None)
            .map_err(|e| OrmError::Connection(format!("Band-7 driver options error: {e}")))?;
        let driver = B7Driver::new(address, B7Credentials::new(username, password), opts)
            .await
            .map_err(|e| OrmError::Connection(format!("Failed to connect to {address}: {e}")))?;
        return Ok(DriverHandle::B7(driver));
    }

    // Every band that is not 7 is band 8 here: the gate already rejected any
    // band outside EMBEDDED_BANDS, so a non-band-7 server is necessarily a
    // band-8 server this build embedded.
    #[cfg(feature = "band8")]
    {
        // TLS is not plumbed in this crate today; the band-8 driver's
        // `DriverTlsConfig::default()` would ENABLE it, so disable explicitly.
        let addresses = Addresses::try_from_address_str(address)
            .map_err(|e| OrmError::Connection(format!("Invalid TypeDB address {address}: {e}")))?;
        let driver = B8Driver::new(
            addresses,
            B8Credentials::new(username, password),
            DriverOptions::new(DriverTlsConfig::disabled()),
        )
        .await
        .map_err(|e| OrmError::Connection(format!("Failed to connect to {address}: {e}")))?;
        Ok(DriverHandle::B8(driver))
    }

    // Unreachable by invariant: with band8 compiled out, EMBEDDED_BANDS cannot
    // contain 8, so check_server_supported already rejected every non-band-7
    // server above.  The arm exists only to keep the cfg-complementary tail
    // total; it is never entered.
    #[cfg(not(feature = "band8"))]
    Err(OrmError::Connection(format!(
        "No compiled driver band supports the detected server band ({band:?})"
    )))
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
) -> Result<DriverHandle, OrmError> {
    gated_driver_with_probe(
        address,
        username,
        password,
        options,
        core_version::server_version,
    )
    .await
}

impl RealBackend {
    /// Connect to a TypeDB server.
    ///
    /// Probes the server version via HTTP before opening any gRPC connection.
    /// Returns [`OrmError::UnsupportedVersion`] when the server is outside the
    /// supported window or on a different protocol band than the driver.
    pub async fn connect(
        address: &str,
        username: &str,
        password: &str,
        options: ConnectOptions,
    ) -> Result<Self, OrmError> {
        let driver = gated_driver(address, username, password, options).await?;
        tracing::info!(address, "Connected to TypeDB");
        Ok(Self { driver })
    }
}

/// Ensure a TypeDB database exists, creating it if absent.
///
/// Probes the server version via HTTP before opening any gRPC connection.
/// Returns [`OrmError::UnsupportedVersion`] when the server is outside the
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
) -> Result<(), OrmError> {
    let driver = gated_driver(address, username, password, options).await?;

    match driver {
        #[cfg(feature = "band7")]
        DriverHandle::B7(d) => {
            let databases = d.databases();
            let exists = databases
                .contains(database)
                .await
                .map_err(|e| OrmError::Connection(format!("Database lookup failed: {e}")))?;
            if !exists {
                databases
                    .create(database)
                    .await
                    .map_err(|e| OrmError::Connection(format!("Database create failed: {e}")))?;
            }
        }
        #[cfg(feature = "band8")]
        DriverHandle::B8(d) => {
            let databases = d.databases();
            let exists = databases
                .contains(database)
                .await
                .map_err(|e| OrmError::Connection(format!("Database lookup failed: {e}")))?;
            if !exists {
                databases
                    .create(database)
                    .await
                    .map_err(|e| OrmError::Connection(format!("Database create failed: {e}")))?;
            }
        }
    }

    Ok(())
}

impl DriverBackend for RealBackend {
    fn open_transaction(
        &self,
        database: &str,
        tx_type: TxType,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
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
                        OrmError::Transaction(format!("Failed to open transaction: {e}"))
                    })?;
                    Ok(Box::new(RealTransaction {
                        inner: RealTransactionInner::B7(Some(transaction)),
                    }) as Box<dyn TransactionOps>)
                }
                #[cfg(feature = "band8")]
                DriverHandle::B8(d) => {
                    let typedb_tx_type = match tx_type {
                        TxType::Read => B8TransactionType::Read,
                        TxType::Write => B8TransactionType::Write,
                        TxType::Schema => B8TransactionType::Schema,
                    };
                    let transaction = d.transaction(&db, typedb_tx_type).await.map_err(|e| {
                        OrmError::Transaction(format!("Failed to open transaction: {e}"))
                    })?;
                    Ok(Box::new(RealTransaction {
                        inner: RealTransactionInner::B8(Some(transaction)),
                    }) as Box<dyn TransactionOps>)
                }
            }
        })
    }

    fn is_open(&self) -> bool {
        match &self.driver {
            #[cfg(feature = "band7")]
            DriverHandle::B7(d) => d.is_open(),
            #[cfg(feature = "band8")]
            DriverHandle::B8(d) => d.is_open(),
        }
    }

    fn database_exists(&self, database: &str) -> BoxFuture<'_, Result<bool, OrmError>> {
        let database = database.to_string();
        Box::pin(async move {
            match &self.driver {
                #[cfg(feature = "band7")]
                DriverHandle::B7(d) => d
                    .databases()
                    .contains(database)
                    .await
                    .map_err(|e| OrmError::Connection(format!("Database lookup failed: {e}"))),
                #[cfg(feature = "band8")]
                DriverHandle::B8(d) => d
                    .databases()
                    .contains(database)
                    .await
                    .map_err(|e| OrmError::Connection(format!("Database lookup failed: {e}"))),
            }
        })
    }

    fn create_database(&self, database: &str) -> BoxFuture<'_, Result<(), OrmError>> {
        let database = database.to_string();
        Box::pin(async move {
            match &self.driver {
                #[cfg(feature = "band7")]
                DriverHandle::B7(d) => d
                    .databases()
                    .create(database)
                    .await
                    .map_err(|e| OrmError::Connection(format!("Database create failed: {e}"))),
                #[cfg(feature = "band8")]
                DriverHandle::B8(d) => d
                    .databases()
                    .create(database)
                    .await
                    .map_err(|e| OrmError::Connection(format!("Database create failed: {e}"))),
            }
        })
    }

    fn delete_database(&self, database: &str) -> BoxFuture<'_, Result<(), OrmError>> {
        let database = database.to_string();
        Box::pin(async move {
            match &self.driver {
                #[cfg(feature = "band7")]
                DriverHandle::B7(d) => {
                    let db = d.databases().get(&database).await.map_err(|e| {
                        OrmError::Connection(format!("Database lookup failed: {e}"))
                    })?;
                    db.delete()
                        .await
                        .map_err(|e| OrmError::Connection(format!("Database delete failed: {e}")))
                }
                #[cfg(feature = "band8")]
                DriverHandle::B8(d) => {
                    let db = d.databases().get(database).await.map_err(|e| {
                        OrmError::Connection(format!("Database lookup failed: {e}"))
                    })?;
                    db.delete()
                        .await
                        .map_err(|e| OrmError::Connection(format!("Database delete failed: {e}")))
                }
            }
        })
    }

    fn schema_text(&self, database: &str) -> BoxFuture<'_, Result<String, OrmError>> {
        let database = database.to_string();
        Box::pin(async move {
            match &self.driver {
                #[cfg(feature = "band7")]
                DriverHandle::B7(d) => {
                    let db = d.databases().get(&database).await.map_err(|e| {
                        OrmError::Connection(format!("Database lookup failed: {e}"))
                    })?;
                    db.schema()
                        .await
                        .map_err(|e| OrmError::Connection(format!("Schema export failed: {e}")))
                }
                #[cfg(feature = "band8")]
                DriverHandle::B8(d) => {
                    let db = d.databases().get(&database).await.map_err(|e| {
                        OrmError::Connection(format!("Database lookup failed: {e}"))
                    })?;
                    db.schema()
                        .await
                        .map_err(|e| OrmError::Connection(format!("Schema export failed: {e}")))
                }
            }
        })
    }
}

/// Band-tagged transaction inner state.
///
/// The `Option` wrapper lets commit/close/rollback take ownership by value
/// (`.take()`) while the outer `&mut RealTransaction` satisfies the trait's
/// `&mut self`.  A `None` after `.take()` marks the transaction as consumed.
enum RealTransactionInner {
    #[cfg(feature = "band7")]
    B7(Option<type_bridge_typedb_driver_b7::Transaction>),
    #[cfg(feature = "band8")]
    B8(Option<typedb_driver::Transaction>),
}

struct RealTransaction {
    inner: RealTransactionInner,
}

impl TransactionOps for RealTransaction {
    fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        let tql = typeql.to_string();
        Box::pin(async move {
            match &self.inner {
                #[cfg(feature = "band7")]
                RealTransactionInner::B7(opt) => {
                    let tx = opt.as_ref().ok_or_else(|| {
                        OrmError::Transaction("Transaction already consumed".into())
                    })?;
                    let answer = tx
                        .query(&tql)
                        .await
                        .map_err(|e| OrmError::QueryExecution(format!("{e}")))?;
                    match answer {
                        B7QueryAnswer::Ok(_) => Ok(QueryResult::Ok),
                        B7QueryAnswer::ConceptRowStream(_, stream) => {
                            let rows: Vec<_> = stream.try_collect().await.map_err(|e| {
                                OrmError::QueryExecution(format!("Row collect: {e}"))
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
                                OrmError::QueryExecution(format!("Doc collect: {e}"))
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
                RealTransactionInner::B8(opt) => {
                    let tx = opt.as_ref().ok_or_else(|| {
                        OrmError::Transaction("Transaction already consumed".into())
                    })?;
                    let answer = tx
                        .query(&tql)
                        .await
                        .map_err(|e| OrmError::QueryExecution(format!("{e}")))?;
                    match answer {
                        B8QueryAnswer::Ok(_) => Ok(QueryResult::Ok),
                        B8QueryAnswer::ConceptRowStream(_, stream) => {
                            let rows: Vec<_> = stream.try_collect().await.map_err(|e| {
                                OrmError::QueryExecution(format!("Row collect: {e}"))
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
                                OrmError::QueryExecution(format!("Doc collect: {e}"))
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
            }
        })
    }

    fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        // Take ownership out of the Option so the async block can move the
        // transaction by value into commit(self) — both bands consume self.
        match &mut self.inner {
            #[cfg(feature = "band7")]
            RealTransactionInner::B7(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    let t = tx.ok_or_else(|| {
                        OrmError::Transaction("Transaction already consumed".into())
                    })?;
                    t.commit()
                        .await
                        .map_err(|e| OrmError::Transaction(format!("Commit failed: {e}")))
                })
            }
            #[cfg(feature = "band8")]
            RealTransactionInner::B8(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    let t = tx.ok_or_else(|| {
                        OrmError::Transaction("Transaction already consumed".into())
                    })?;
                    t.commit()
                        .await
                        .map_err(|e| OrmError::Transaction(format!("Commit failed: {e}")))
                })
            }
        }
    }

    fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        // Both bands expose rollback(&self), so we can move the value out of
        // the Option and call rollback on the owned T (the compiler
        // auto-borrows through the owned value for &self methods).
        match &mut self.inner {
            #[cfg(feature = "band7")]
            RealTransactionInner::B7(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    let t = tx.ok_or_else(|| {
                        OrmError::Transaction("Transaction already consumed".into())
                    })?;
                    t.rollback()
                        .await
                        .map_err(|e| OrmError::Transaction(format!("Rollback failed: {e}")))
                })
            }
            #[cfg(feature = "band8")]
            RealTransactionInner::B8(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    let t = tx.ok_or_else(|| {
                        OrmError::Transaction("Transaction already consumed".into())
                    })?;
                    t.rollback()
                        .await
                        .map_err(|e| OrmError::Transaction(format!("Rollback failed: {e}")))
                })
            }
        }
    }

    fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        match &mut self.inner {
            #[cfg(feature = "band7")]
            RealTransactionInner::B7(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    let Some(t) = tx else {
                        return Ok(());
                    };
                    t.close()
                        .await
                        .map_err(|e| OrmError::Transaction(format!("Close failed: {e}")))
                })
            }
            #[cfg(feature = "band8")]
            RealTransactionInner::B8(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    let Some(t) = tx else {
                        return Ok(());
                    };
                    t.close()
                        .await
                        .map_err(|e| OrmError::Transaction(format!("Close failed: {e}")))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn connect_options_default_matches_ssot() {
        let options = ConnectOptions::default();
        assert_eq!(options.http_port, DEFAULT_HTTP_PORT);
        assert!(!options.tls);
    }

    /// The injectable probe must receive the configured port, not a
    /// hardcoded 8000.  The probe returns an in-window version so the gate
    /// proceeds to the gRPC connection attempt, which fails with a network
    /// error (no server) — the assertion is about the recorded port and
    /// that the failure is NOT a version-gate rejection.
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

        if let Err(OrmError::UnsupportedVersion(_)) = result {
            panic!("expected a connection error (no server), not a version gate rejection")
        }
    }
}
