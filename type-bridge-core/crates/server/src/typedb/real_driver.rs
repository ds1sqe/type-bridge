use std::future::Future;
use std::pin::Pin;

use futures::TryStreamExt;
use typedb_driver::answer::QueryAnswer;
use typedb_driver::{Credentials, DriverOptions, Transaction, TransactionType, TypeDBDriver};

use super::backend::{DriverBackend, QueryResultKind, TransactionOps};
use super::client::concept_to_json;
use crate::config::TypeDBSection;
use crate::error::PipelineError;

/// Real TypeDB driver backend wrapping `TypeDBDriver`.
pub(crate) struct RealTypeDBBackend {
    driver: TypeDBDriver,
}

impl RealTypeDBBackend {
    /// Connect to a TypeDB server using the provided configuration.
    pub async fn connect(config: &TypeDBSection) -> Result<Self, PipelineError> {
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
            let transaction = self
                .driver
                .transaction(&db, tx_type)
                .await
                .map_err(|e| {
                    PipelineError::QueryExecution(format!("Failed to open transaction: {e}"))
                })?;
            Ok(Box::new(RealTransaction { transaction: Some(transaction) }) as Box<dyn TransactionOps>)
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

    fn commit(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<(), PipelineError>> + Send + '_>> {
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
