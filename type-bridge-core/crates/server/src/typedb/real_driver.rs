//! Real TypeDB backend using the `typedb-driver` crate.
//!
//! This module is only compiled when the `typedb` feature is enabled.

#[cfg(not(any(feature = "band7", feature = "band8")))]
compile_error!(
    "type-bridge-server: the `typedb` machinery requires at least one band feature; enable `band7` and/or `band8` (both are default)"
);

use futures::TryStreamExt;
use type_bridge_core_lib::version as core_version;

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

use super::backend::{BoxFuture, DriverBackend, QueryResultKind, TransactionOps, TransactionType};
#[cfg(feature = "band7")]
use super::client::concept_to_json_b7;
#[cfg(feature = "band8")]
use super::client::concept_to_json_b8;
use crate::config::TypeDBSection;
use crate::error::PipelineError;

/// The compile-pinned `typedb-driver` crate version resolved in `Cargo.lock`.
///
/// This constant records the exact driver version the server crate was built
/// against.  It is the factual counterpart to the policy window declared in
/// `type_bridge_core_lib::version`: the window says which versions are allowed;
/// this constant says which version is currently in use.
///
/// When the `typedb-driver` dependency is bumped, update this constant to
/// match the new `Cargo.lock` entry — the `tests::cargo_lock_pin` test will
/// catch any divergence.
///
/// Declared independently of the orm crate so server never grows an orm
/// dependency (see comment in the original server `real_driver.rs`).
pub const PINNED_DRIVER_VERSION: &str = "3.11.5";

/// The compile-pinned version of the vendored band-7 driver fork
/// (`type-bridge-typedb-driver-b7`) this server crate was built against.
///
/// Mirrors [`PINNED_DRIVER_VERSION`] for the band-8 upstream driver.  When
/// the band-7 fork is refreshed, update this constant to match the new
/// `Cargo.lock` entry — the `tests::cargo_lock_pin_b7` test will catch any
/// divergence.
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

/// Band-tagged driver handle.  Private to this module; no driver type escapes.
enum DriverHandle {
    #[cfg(feature = "band7")]
    B7(B7Driver),
    #[cfg(feature = "band8")]
    B8(B8Driver),
}

/// Real TypeDB backend wrapping a band-tagged [`DriverHandle`].
pub(crate) struct RealTypeDBBackend {
    driver: DriverHandle,
}

/// Version-gated [`DriverHandle`] constructor.
///
/// Before allocating any gRPC connection, this helper:
///
/// 1. Probes the TypeDB HTTP version endpoint (port configured in
///    `TypeDBSection`, plain HTTP — TLS is not plumbed in this crate today).
/// 2. Runs the embedded-runtime gate [`core_version::check_server_supported`]
///    against [`EMBEDDED_BANDS`] — the server is served when it is in-window
///    and its band is one this build embedded.
/// 3. Only on success, constructs and returns a live [`DriverHandle`] for the
///    server's band.
///
/// Any version incompatibility surfaces as [`PipelineError::UnsupportedVersion`]
/// **before** the gRPC handshake — fail fast, never silently.
async fn gated_driver(config: &TypeDBSection) -> Result<DriverHandle, PipelineError> {
    // Probe the server version over HTTP (blocking I/O; offload to a
    // dedicated thread so we don't block the async executor).
    let address_owned = config.address.clone();
    let http_port = config.http_port;
    let server_version = tokio::task::spawn_blocking(move || {
        core_version::server_version(&address_owned, http_port, false)
    })
    .await
    .map_err(|e| PipelineError::Connection(format!("version probe task panicked: {e}")))?
    .map_err(PipelineError::UnsupportedVersion)?;

    // Embedded-runtime gate: accept the server when it is in-window and its
    // band is one this build embedded.  Unlike the installed-driver gate
    // (check_supported, band equality), the embedded runtime carries every
    // compiled-in band, so a band-7 server is now served, not rejected.  After
    // this passes, band(&server_version) is guaranteed ∈ EMBEDDED_BANDS.
    core_version::check_server_supported(&server_version, EMBEDDED_BANDS)
        .map_err(PipelineError::UnsupportedVersion)?;

    let band = core_version::band(&server_version);

    tracing::debug!(
        address = config.address.as_str(),
        band = ?band,
        server_version = %server_version,
        "Embedded version gate passed"
    );

    #[cfg(feature = "band7")]
    if band == Some(7) {
        // Band-7 driver takes a raw address string and a two-argument
        // DriverOptions::new(tls_enabled, ca_path).  TLS is disabled.
        let opts = B7DriverOptions::new(false, None)
            .map_err(|e| PipelineError::Connection(format!("Band-7 driver options error: {e}")))?;
        let driver =
            B7Driver::new(&config.address, B7Credentials::new(&config.username, &config.password), opts)
                .await
                .map_err(|e| {
                    PipelineError::Connection(format!(
                        "Failed to connect to TypeDB at {}: {e}",
                        config.address
                    ))
                })?;
        return Ok(DriverHandle::B7(driver));
    }

    // Every band that is not 7 is band 8 here: the gate already rejected any
    // band outside EMBEDDED_BANDS, so a non-band-7 server is necessarily a
    // band-8 server this build embedded.
    #[cfg(feature = "band8")]
    {
        // TLS is not plumbed in this crate today; the band-8 driver's
        // `DriverTlsConfig::default()` would ENABLE it, so disable explicitly.
        let addresses = Addresses::try_from_address_str(&config.address).map_err(|e| {
            PipelineError::Connection(format!("Invalid TypeDB address {}: {e}", config.address))
        })?;
        let driver = B8Driver::new(
            addresses,
            B8Credentials::new(&config.username, &config.password),
            DriverOptions::new(DriverTlsConfig::disabled()),
        )
        .await
        .map_err(|e| {
            PipelineError::Connection(format!(
                "Failed to connect to TypeDB at {}: {e}",
                config.address
            ))
        })?;
        Ok(DriverHandle::B8(driver))
    }

    // Unreachable by invariant: with band8 compiled out, EMBEDDED_BANDS cannot
    // contain 8, so check_server_supported already rejected every non-band-7
    // server above.  The arm exists only to keep the cfg-complementary tail
    // total; it is never entered.
    #[cfg(not(feature = "band8"))]
    Err(PipelineError::Connection(format!(
        "No compiled driver band supports the detected server band ({band:?})"
    )))
}

impl RealTypeDBBackend {
    /// Connect to a TypeDB server using the provided configuration.
    ///
    /// Probes the server version via HTTP before opening any gRPC connection.
    /// Returns [`PipelineError::UnsupportedVersion`] when the server is outside
    /// the supported window or on a different protocol band than the driver.
    pub(crate) async fn connect(config: &TypeDBSection) -> Result<Self, PipelineError> {
        let driver = gated_driver(config).await?;
        tracing::info!(address = config.address.as_str(), "Connected to TypeDB");
        Ok(Self { driver })
    }
}

impl DriverBackend for RealTypeDBBackend {
    fn open_transaction(
        &self,
        database: &str,
        tx_type: TransactionType,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, PipelineError>> {
        let db = database.to_string();
        Box::pin(async move {
            match &self.driver {
                #[cfg(feature = "band7")]
                DriverHandle::B7(d) => {
                    let typedb_tx_type = match tx_type {
                        TransactionType::Read => B7TransactionType::Read,
                        TransactionType::Write => B7TransactionType::Write,
                        TransactionType::Schema => B7TransactionType::Schema,
                    };
                    let transaction = d
                        .transaction(&db, typedb_tx_type)
                        .await
                        .map_err(|e| {
                            PipelineError::QueryExecution(format!("Failed to open transaction: {e}"))
                        })?;
                    Ok(Box::new(RealTransaction {
                        inner: RealTransactionInner::B7(Some(transaction)),
                    }) as Box<dyn TransactionOps>)
                }
                #[cfg(feature = "band8")]
                DriverHandle::B8(d) => {
                    let typedb_tx_type = match tx_type {
                        TransactionType::Read => B8TransactionType::Read,
                        TransactionType::Write => B8TransactionType::Write,
                        TransactionType::Schema => B8TransactionType::Schema,
                    };
                    let transaction = d
                        .transaction(&db, typedb_tx_type)
                        .await
                        .map_err(|e| {
                            PipelineError::QueryExecution(format!("Failed to open transaction: {e}"))
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
}

/// Band-tagged transaction inner state.
///
/// The `Option` wrapper lets commit/close take ownership by value (`.take()`)
/// while the outer `&mut RealTransaction` satisfies the trait's `&mut self`.
/// A `None` after `.take()` marks the transaction as consumed.
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
    fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResultKind, PipelineError>> {
        let tql = typeql.to_string();
        Box::pin(async move {
            match &self.inner {
                #[cfg(feature = "band7")]
                RealTransactionInner::B7(opt) => {
                    let tx = opt.as_ref().ok_or_else(|| {
                        PipelineError::QueryExecution("Transaction already consumed".into())
                    })?;
                    let answer = tx
                        .query(&tql)
                        .await
                        .map_err(|e| PipelineError::QueryExecution(format!("{e}")))?;
                    match answer {
                        B7QueryAnswer::Ok(_) => Ok(QueryResultKind::Ok),
                        B7QueryAnswer::ConceptRowStream(_, stream) => {
                            let rows: Vec<_> = stream
                                .try_collect()
                                .await
                                .map_err(|e| PipelineError::QueryExecution(format!("Row collect: {e}")))?;
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
                            Ok(QueryResultKind::Rows(json_rows))
                        }
                        B7QueryAnswer::ConceptDocumentStream(_, stream) => {
                            let docs: Vec<_> = stream
                                .try_collect()
                                .await
                                .map_err(|e| PipelineError::QueryExecution(format!("Doc collect: {e}")))?;
                            let json_docs = docs
                                .into_iter()
                                .map(|doc| {
                                    serde_json::to_value(doc.into_json())
                                        .unwrap_or(serde_json::Value::Null)
                                })
                                .collect();
                            Ok(QueryResultKind::Documents(json_docs))
                        }
                    }
                }
                #[cfg(feature = "band8")]
                RealTransactionInner::B8(opt) => {
                    let tx = opt.as_ref().ok_or_else(|| {
                        PipelineError::QueryExecution("Transaction already consumed".into())
                    })?;
                    let answer = tx
                        .query(&tql)
                        .await
                        .map_err(|e| PipelineError::QueryExecution(format!("Query execution failed: {e}")))?;
                    match answer {
                        B8QueryAnswer::Ok(_) => Ok(QueryResultKind::Ok),
                        B8QueryAnswer::ConceptRowStream(_, stream) => {
                            let rows: Vec<_> = stream
                                .try_collect()
                                .await
                                .map_err(|e| PipelineError::QueryExecution(format!("Failed to collect rows: {e}")))?;
                            let json_rows = rows
                                .iter()
                                .map(|row| {
                                    let column_names = row.get_column_names();
                                    let mut obj = serde_json::Map::new();
                                    for (i, col) in column_names.iter().enumerate() {
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
                            Ok(QueryResultKind::Rows(json_rows))
                        }
                        B8QueryAnswer::ConceptDocumentStream(_, stream) => {
                            let docs: Vec<_> = stream
                                .try_collect()
                                .await
                                .map_err(|e| PipelineError::QueryExecution(format!("Failed to collect documents: {e}")))?;
                            let json_docs = docs
                                .into_iter()
                                .map(|doc| {
                                    let json = doc.into_json();
                                    serde_json::to_value(&json).unwrap_or(serde_json::Value::Null)
                                })
                                .collect();
                            Ok(QueryResultKind::Documents(json_docs))
                        }
                    }
                }
            }
        })
    }

    fn commit(&mut self) -> BoxFuture<'_, Result<(), PipelineError>> {
        // Take ownership out of the Option so the async block can move the
        // transaction by value into commit(self) — both bands consume self.
        match &mut self.inner {
            #[cfg(feature = "band7")]
            RealTransactionInner::B7(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    let t = tx.ok_or_else(|| {
                        PipelineError::QueryExecution("Transaction already consumed".into())
                    })?;
                    t.commit()
                        .await
                        .map_err(|e| PipelineError::QueryExecution(format!("Failed to commit transaction: {e}")))
                })
            }
            #[cfg(feature = "band8")]
            RealTransactionInner::B8(opt) => {
                let tx = opt.take();
                Box::pin(async move {
                    let t = tx.ok_or_else(|| {
                        PipelineError::QueryExecution("Transaction already consumed".into())
                    })?;
                    t.commit()
                        .await
                        .map_err(|e| PipelineError::QueryExecution(format!("Failed to commit transaction: {e}")))
                })
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{PINNED_DRIVER_VERSION, PINNED_DRIVER_VERSION_B7};
    use type_bridge_core_lib::version as core_version;

    /// Assert that this crate's `PINNED_DRIVER_VERSION` matches the
    /// `typedb-driver` entry in `Cargo.lock`, and stays in the expected
    /// protocol band.
    ///
    /// The constant is deliberately declared locally (no orm dependency), so
    /// it needs its own lock assertion — without it, a dependency bump could
    /// silently desynchronize the two crates' pinned facts.
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
                    .find(|l| l.trim_start().starts_with("version = "))
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
             is {PINNED_DRIVER_VERSION}; update the constant in crates/server/src/typedb/real_driver.rs"
        );

        let pinned: core_version::Version = PINNED_DRIVER_VERSION.parse().unwrap();
        assert_eq!(
            core_version::band(&pinned),
            Some(8),
            "pinned driver version {PINNED_DRIVER_VERSION} left protocol band 8; \
             review the gate expectations before accepting the bump"
        );
    }

    /// Assert that this crate's `PINNED_DRIVER_VERSION_B7` matches the
    /// `type-bridge-typedb-driver-b7` entry in `Cargo.lock`, and stays in the
    /// expected protocol band (7).
    ///
    /// The constant is declared independently so the server never grows an orm
    /// dependency.  Without this test, a fork refresh could silently
    /// desynchronize the two crates' pinned facts.
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
                    .find(|l| l.trim_start().starts_with("version = "))
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
             PINNED_DRIVER_VERSION_B7 is {PINNED_DRIVER_VERSION_B7}; \
             update the constant in crates/server/src/typedb/real_driver.rs"
        );

        let pinned: core_version::Version = PINNED_DRIVER_VERSION_B7.parse().unwrap();
        assert_eq!(
            core_version::band(&pinned),
            Some(7),
            "pinned band-7 fork version {PINNED_DRIVER_VERSION_B7} left protocol band 7; \
             review the gate expectations before accepting the bump"
        );
    }
}
