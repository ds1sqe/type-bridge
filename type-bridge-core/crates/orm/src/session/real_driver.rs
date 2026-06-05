//! Real TypeDB backend using the `typedb-driver` crate.
//!
//! This module is only compiled when the `typedb` feature is enabled.

use futures::TryStreamExt;
use typedb_driver::answer::QueryAnswer;
use typedb_driver::{Credentials, DriverOptions, Transaction, TransactionType, TypeDBDriver};

use super::backend::{BoxFuture, DriverBackend, QueryResult, TransactionOps, TxType};
use crate::error::OrmError;

/// Real TypeDB backend wrapping [`TypeDBDriver`].
pub struct RealBackend {
    driver: TypeDBDriver,
}

impl RealBackend {
    /// Connect to a TypeDB server.
    pub async fn connect(address: &str, username: &str, password: &str) -> Result<Self, OrmError> {
        let driver = TypeDBDriver::new(
            address,
            Credentials::new(username, password),
            DriverOptions::new(false, None)
                .map_err(|e| OrmError::Connection(format!("Driver options error: {e}")))?,
        )
        .await
        .map_err(|e| OrmError::Connection(format!("Failed to connect to {address}: {e}")))?;

        tracing::info!(address, "Connected to TypeDB");
        Ok(Self { driver })
    }
}

/// Ensure a TypeDB database exists, creating it if absent.
///
/// Connects with a standalone driver, checks whether the named database exists,
/// and creates it only when it does not.  Returns `Err` on any TypeDB failure
/// (including unreachable server) so callers can treat the error as a hard
/// failure rather than silently skipping.
pub async fn ensure_database_exists(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
) -> Result<(), OrmError> {
    let driver = TypeDBDriver::new(
        address,
        Credentials::new(username, password),
        DriverOptions::new(false, None)
            .map_err(|e| OrmError::Connection(format!("Driver options error: {e}")))?,
    )
    .await
    .map_err(|e| OrmError::Connection(format!("Failed to connect to {address}: {e}")))?;

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
