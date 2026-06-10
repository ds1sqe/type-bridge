//! Real TypeDB backend using the `typedb-driver` crate.
//!
//! This module is only compiled when the `typedb` feature is enabled.

use futures::TryStreamExt;
use type_bridge_core_lib::version as core_version;
use typedb_driver::answer::QueryAnswer;
use typedb_driver::{
    Addresses, Credentials, DriverOptions, DriverTlsConfig, Transaction, TransactionType,
    TypeDBDriver,
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

/// Real TypeDB backend wrapping [`TypeDBDriver`].
pub struct RealBackend {
    driver: TypeDBDriver,
}

/// Version-gated [`TypeDBDriver`] constructor.
///
/// Before allocating any gRPC connection, this helper:
///
/// 1. Probes the TypeDB HTTP version endpoint (port 8000, plain HTTP — TLS is
///    not plumbed in this crate today).
/// 2. Runs [`core_version::check_supported`] against the compile-pinned driver
///    version ([`PINNED_DRIVER_VERSION`]) and the probed server version.
/// 3. Only on success, constructs and returns a live [`TypeDBDriver`].
///
/// Any version incompatibility surfaces as [`OrmError::UnsupportedVersion`]
/// **before** the gRPC handshake — fail fast, never silently.
async fn gated_driver(
    address: &str,
    username: &str,
    password: &str,
) -> Result<TypeDBDriver, OrmError> {
    // Parse the compile-pinned driver version.  The literal is controlled by
    // this crate; a parse failure is a programming error, not a runtime one.
    let driver_version: core_version::Version = PINNED_DRIVER_VERSION
        .parse()
        .expect("PINNED_DRIVER_VERSION is not a valid version string — update the constant");

    // Probe the server version over HTTP (blocking I/O; offload to a
    // dedicated thread so we don't block the async executor).
    let address_owned = address.to_string();
    let server_version = tokio::task::spawn_blocking(move || {
        core_version::server_version(&address_owned, 8000, false)
    })
    .await
    .map_err(|e| OrmError::Connection(format!("Version probe task panicked: {e}")))?
    .map_err(OrmError::UnsupportedVersion)?;

    // Gate: both endpoints must be in-window and on the same protocol band.
    core_version::check_supported(&driver_version, &server_version)
        .map_err(OrmError::UnsupportedVersion)?;

    tracing::debug!(
        address,
        driver_version = %driver_version,
        server_version = %server_version,
        "Version gate passed"
    );

    // TLS is not plumbed in this crate today; the band-8 driver's
    // `DriverTlsConfig::default()` would ENABLE it, so disable explicitly.
    let addresses = Addresses::try_from_address_str(address)
        .map_err(|e| OrmError::Connection(format!("Invalid TypeDB address {address}: {e}")))?;
    TypeDBDriver::new(
        addresses,
        Credentials::new(username, password),
        DriverOptions::new(DriverTlsConfig::disabled()),
    )
    .await
    .map_err(|e| OrmError::Connection(format!("Failed to connect to {address}: {e}")))
}

impl RealBackend {
    /// Connect to a TypeDB server.
    ///
    /// Probes the server version via HTTP before opening any gRPC connection.
    /// Returns [`OrmError::UnsupportedVersion`] when the server is outside the
    /// supported window or on a different protocol band than the driver.
    pub async fn connect(address: &str, username: &str, password: &str) -> Result<Self, OrmError> {
        let driver = gated_driver(address, username, password).await?;
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
) -> Result<(), OrmError> {
    let driver = gated_driver(address, username, password).await?;

    let databases = driver.databases();
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
            let typedb_tx_type = match tx_type {
                TxType::Read => TransactionType::Read,
                TxType::Write => TransactionType::Write,
                TxType::Schema => TransactionType::Schema,
            };
            let transaction = self
                .driver
                .transaction(&db, typedb_tx_type)
                .await
                .map_err(|e| OrmError::Transaction(format!("Failed to open transaction: {e}")))?;
            Ok(Box::new(RealTransaction {
                transaction: Some(transaction),
            }) as Box<dyn TransactionOps>)
        })
    }

    fn is_open(&self) -> bool {
        self.driver.is_open()
    }

    fn schema_text(&self, database: &str) -> BoxFuture<'_, Result<String, OrmError>> {
        let database = database.to_string();
        Box::pin(async move {
            let db = self
                .driver
                .databases()
                .get(&database)
                .await
                .map_err(|e| OrmError::Connection(format!("Database lookup failed: {e}")))?;
            db.schema()
                .await
                .map_err(|e| OrmError::Connection(format!("Schema export failed: {e}")))
        })
    }
}

struct RealTransaction {
    transaction: Option<Transaction>,
}

impl TransactionOps for RealTransaction {
    fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        let tql = typeql.to_string();
        Box::pin(async move {
            let tx = self
                .transaction
                .as_ref()
                .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
            let answer = tx
                .query(&tql)
                .await
                .map_err(|e| OrmError::QueryExecution(format!("{e}")))?;

            match answer {
                QueryAnswer::Ok(_) => Ok(QueryResult::Ok),
                QueryAnswer::ConceptRowStream(_, stream) => {
                    let rows: Vec<_> = stream
                        .try_collect()
                        .await
                        .map_err(|e| OrmError::QueryExecution(format!("Row collect: {e}")))?;
                    let json_rows = rows
                        .iter()
                        .map(|row| {
                            let mut obj = serde_json::Map::new();
                            for (i, col) in row.get_column_names().iter().enumerate() {
                                let value = row
                                    .row
                                    .get(i)
                                    .and_then(|c| c.as_ref())
                                    .map(concept_to_json)
                                    .unwrap_or(serde_json::Value::Null);
                                obj.insert(col.clone(), value);
                            }
                            serde_json::Value::Object(obj)
                        })
                        .collect();
                    Ok(QueryResult::Rows(json_rows))
                }
                QueryAnswer::ConceptDocumentStream(_, stream) => {
                    let docs: Vec<_> = stream
                        .try_collect()
                        .await
                        .map_err(|e| OrmError::QueryExecution(format!("Doc collect: {e}")))?;
                    let json_docs = docs
                        .into_iter()
                        .map(|doc| {
                            serde_json::to_value(doc.into_json()).unwrap_or(serde_json::Value::Null)
                        })
                        .collect();
                    Ok(QueryResult::Documents(json_docs))
                }
            }
        })
    }

    fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        let tx = self.transaction.take();
        Box::pin(async move {
            let t =
                tx.ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
            t.commit()
                .await
                .map_err(|e| OrmError::Transaction(format!("Commit failed: {e}")))
        })
    }

    fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        let tx = self.transaction.take();
        Box::pin(async move {
            let t =
                tx.ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
            t.rollback()
                .await
                .map_err(|e| OrmError::Transaction(format!("Rollback failed: {e}")))
        })
    }

    fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        let tx = self.transaction.take();
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

/// Convert a TypeDB concept to a JSON value.
fn concept_to_json(concept: &typedb_driver::concept::Concept) -> serde_json::Value {
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
        obj.insert("value".into(), value_to_json(value));
    }
    if let Some(vt) = concept.try_get_value_type() {
        obj.insert(
            "value_type".into(),
            serde_json::Value::String(vt.name().into()),
        );
    }
    serde_json::Value::Object(obj)
}

/// Convert a TypeDB value to a JSON value.
fn value_to_json(value: &typedb_driver::concept::Value) -> serde_json::Value {
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
