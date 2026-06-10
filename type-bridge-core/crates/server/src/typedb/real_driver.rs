use std::future::Future;
use std::pin::Pin;

use futures::TryStreamExt;
use type_bridge_core_lib::version;
use typedb_driver::answer::QueryAnswer;
use typedb_driver::{Credentials, DriverOptions, Transaction, TransactionType, TypeDBDriver};

use super::backend::{DriverBackend, QueryResultKind, TransactionOps};
use super::client::concept_to_json;
use crate::config::TypeDBSection;
use crate::error::PipelineError;

// Compile-time-pinned typedb-driver version consumed by this crate.
// Cargo.lock resolves "typedb-driver = { version = "3", ... }" to this exact
// release; update when the lock pin changes.  The orm crate carries an
// equivalent const; we declare independently here so server never grows an
// orm dependency, and the `tests::cargo_lock_pin` below keeps this copy
// honest against the lock.
const PINNED_DRIVER_VERSION: &str = "3.8.1";

/// Real TypeDB driver backend wrapping `TypeDBDriver`.
pub(crate) struct RealTypeDBBackend {
    driver: TypeDBDriver,
}

impl RealTypeDBBackend {
    /// Connect to a TypeDB server using the provided configuration.
    ///
    /// Before constructing the driver the version gate runs:
    /// 1. Parse the compile-pinned driver version.
    /// 2. Probe the server's HTTP `/v1/version` endpoint (blocking, off-thread).
    /// 3. Call `core::check_supported` — fail fast if out-of-window or
    ///    cross-band.  No transaction is opened on an incompatible server.
    pub async fn connect(config: &TypeDBSection) -> Result<Self, PipelineError> {
        // --- version gate (fail-fast before driver construction) ---
        // The literal is controlled by this crate; a parse failure is a
        // programming error, not a runtime one (mirrors the orm crate).
        let driver_ver: version::Version = PINNED_DRIVER_VERSION
            .parse()
            .expect("PINNED_DRIVER_VERSION is not a valid version string — update the constant");

        let address = config.address.clone();
        let http_port = config.http_port;
        let server_ver = tokio::task::spawn_blocking(move || {
            version::server_version(&address, http_port, false)
        })
        .await
        .map_err(|e| PipelineError::Connection(format!("version probe task panicked: {e}")))?
        .map_err(PipelineError::UnsupportedVersion)?;

        version::check_supported(&driver_ver, &server_ver)
            .map_err(PipelineError::UnsupportedVersion)?;

        // --- driver construction (only reached when versions are compatible) ---
        let driver = TypeDBDriver::new(
            &config.address,
            Credentials::new(&config.username, &config.password),
            DriverOptions::new(false, None).map_err(|e| {
                PipelineError::Connection(format!("Failed to create driver options: {e}"))
            })?,
        )
        .await
        .map_err(|e| {
            PipelineError::Connection(format!(
                "Failed to connect to TypeDB at {}: {e}",
                config.address
            ))
        })?;

        tracing::info!(address = config.address.as_str(), "Connected to TypeDB");
        Ok(Self { driver })
    }
}

impl DriverBackend for RealTypeDBBackend {
    fn open_transaction(
        &self,
        database: &str,
        tx_type: TransactionType,
    ) -> Pin<Box<dyn Future<Output = Result<Box<dyn TransactionOps>, PipelineError>> + Send + '_>>
    {
        let db = database.to_string();
        Box::pin(async move {
            let transaction = self.driver.transaction(&db, tx_type).await.map_err(|e| {
                PipelineError::QueryExecution(format!("Failed to open transaction: {e}"))
            })?;
            Ok(Box::new(RealTransaction {
                transaction: Some(transaction),
            }) as Box<dyn TransactionOps>)
        })
    }

    fn is_open(&self) -> bool {
        self.driver.is_open()
    }
}

/// Real TypeDB transaction wrapping `Transaction`.
///
/// The `Option` allows `commit()` to take ownership (TypeDB's commit consumes self).
struct RealTransaction {
    transaction: Option<Transaction>,
}

impl TransactionOps for RealTransaction {
    fn query(
        &mut self,
        typeql: &str,
    ) -> Pin<Box<dyn Future<Output = Result<QueryResultKind, PipelineError>> + Send + '_>> {
        let tql = typeql.to_string();
        Box::pin(async move {
            let tx = self.transaction.as_ref().ok_or_else(|| {
                PipelineError::QueryExecution("Transaction already consumed".to_string())
            })?;
            let answer = tx.query(&tql).await.map_err(|e| {
                PipelineError::QueryExecution(format!("Query execution failed: {e}"))
            })?;

            match answer {
                QueryAnswer::Ok(_) => Ok(QueryResultKind::Ok),
                QueryAnswer::ConceptRowStream(_, stream) => {
                    let rows: Vec<_> = stream.try_collect().await.map_err(|e| {
                        PipelineError::QueryExecution(format!("Failed to collect rows: {e}"))
                    })?;

                    let json_rows: Vec<serde_json::Value> = rows
                        .iter()
                        .map(|row| {
                            let column_names = row.get_column_names();
                            let mut obj = serde_json::Map::new();
                            for (i, col) in column_names.iter().enumerate() {
                                let value = match row.row.get(i).and_then(|c| c.as_ref()) {
                                    Some(concept) => concept_to_json(concept),
                                    None => serde_json::Value::Null,
                                };
                                obj.insert(col.clone(), value);
                            }
                            serde_json::Value::Object(obj)
                        })
                        .collect();

                    Ok(QueryResultKind::Rows(json_rows))
                }
                QueryAnswer::ConceptDocumentStream(_, stream) => {
                    let docs: Vec<_> = stream.try_collect().await.map_err(|e| {
                        PipelineError::QueryExecution(format!("Failed to collect documents: {e}"))
                    })?;

                    let json_docs: Vec<serde_json::Value> = docs
                        .into_iter()
                        .map(|doc| {
                            let json = doc.into_json();
                            serde_json::to_value(&json).unwrap_or(serde_json::Value::Null)
                        })
                        .collect();

                    Ok(QueryResultKind::Documents(json_docs))
                }
            }
        })
    }

    fn commit(&mut self) -> Pin<Box<dyn Future<Output = Result<(), PipelineError>> + Send + '_>> {
        let transaction = self.transaction.take();
        Box::pin(async move {
            let tx = transaction.ok_or_else(|| {
                PipelineError::QueryExecution("Transaction already consumed".to_string())
            })?;
            tx.commit().await.map_err(|e| {
                PipelineError::QueryExecution(format!("Failed to commit transaction: {e}"))
            })
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::PINNED_DRIVER_VERSION;
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
                    .find_map(|line| line.trim().strip_prefix("version = \""))
                    .map(|rest| rest.trim_end_matches('"').to_string())
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
            Some(7),
            "pinned driver version {PINNED_DRIVER_VERSION} left protocol band 7; \
             review the gate expectations before accepting the bump"
        );
    }
}
