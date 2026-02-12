use futures::{StreamExt, TryStreamExt};
use typedb_driver::{
    answer::QueryAnswer, concept::Concept, Credentials, DriverOptions, TransactionType, TypeDBDriver,
};

use crate::config::TypeDBSection;
use crate::error::PipelineError;
use crate::executor::QueryExecutor;

/// Wrapper around the TypeDB Rust driver providing a clean async API
/// for query execution and schema retrieval.
pub struct TypeDBClient {
    driver: TypeDBDriver,
}

impl TypeDBClient {
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
        .map_err(|e| PipelineError::Connection(format!("Failed to connect to TypeDB at {}: {e}", config.address)))?;

        tracing::info!(address = config.address.as_str(), "Connected to TypeDB");
        Ok(Self { driver })
    }

    /// Execute a TypeQL query and return results as JSON.
    ///
    /// For read transactions, the transaction is used directly.
    /// For write and schema transactions, the transaction is committed after execution.
    pub async fn execute(
        &self,
        database: &str,
        typeql: &str,
        tx_type: &str,
    ) -> Result<serde_json::Value, PipelineError> {
        let transaction_type = match tx_type {
            "read" => TransactionType::Read,
            "write" => TransactionType::Write,
            "schema" => TransactionType::Schema,
            other => {
                return Err(PipelineError::QueryExecution(format!(
                    "Unknown transaction type: {other}"
                )));
            }
        };

        let transaction = self
            .driver
            .transaction(database, transaction_type)
            .await
            .map_err(|e| {
                PipelineError::QueryExecution(format!("Failed to open transaction: {e}"))
            })?;

        let answer = transaction.query(typeql).await.map_err(|e| {
            PipelineError::QueryExecution(format!("Query execution failed: {e}"))
        })?;

        let results = match answer {
            QueryAnswer::Ok(_) => {
                // Write/schema confirmations - commit and return empty results
                if matches!(transaction_type, TransactionType::Write | TransactionType::Schema) {
                    transaction.commit().await.map_err(|e| {
                        PipelineError::QueryExecution(format!("Failed to commit transaction: {e}"))
                    })?;
                }
                serde_json::json!({ "ok": true })
            }
            QueryAnswer::ConceptRowStream(_, stream) => {
                let rows: Vec<_> = stream
                    .try_collect()
                    .await
                    .map_err(|e| {
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

                if matches!(transaction_type, TransactionType::Write | TransactionType::Schema) {
                    transaction.commit().await.map_err(|e| {
                        PipelineError::QueryExecution(format!("Failed to commit transaction: {e}"))
                    })?;
                }

                serde_json::Value::Array(json_rows)
            }
            QueryAnswer::ConceptDocumentStream(_, stream) => {
                let docs: Vec<_> = stream
                    .try_collect()
                    .await
                    .map_err(|e| {
                        PipelineError::QueryExecution(format!("Failed to collect documents: {e}"))
                    })?;

                let json_docs: Vec<serde_json::Value> = docs
                    .into_iter()
                    .map(|doc| {
                        let json = doc.into_json();
                        // Convert typedb_driver::JSON → serde_json::Value via serialization
                        serde_json::to_value(&json).unwrap_or(serde_json::Value::Null)
                    })
                    .collect();

                if matches!(transaction_type, TransactionType::Write | TransactionType::Schema) {
                    // Documents from write queries would already be committed
                    let _ = transaction.commit().await;
                }

                serde_json::Value::Array(json_docs)
            }
        };

        Ok(results)
    }

    /// Fetch the schema definition from a TypeDB database.
    #[allow(dead_code)] // public API for future schema-from-TypeDB feature
    pub async fn fetch_schema(&self, database: &str) -> Result<String, PipelineError> {
        let transaction = self
            .driver
            .transaction(database, TransactionType::Read)
            .await
            .map_err(|e| {
                PipelineError::QueryExecution(format!("Failed to open read transaction: {e}"))
            })?;

        // Query all type definitions
        let answer = transaction
            .query("match entity $x; fetch {};")
            .await
            .map_err(|e| {
                PipelineError::QueryExecution(format!("Schema query failed: {e}"))
            })?;

        // Collect results as a string representation
        let mut schema_parts = Vec::new();
        if answer.is_document_stream() {
            let mut stream = answer.into_documents();
            while let Some(Ok(doc)) = stream.next().await {
                schema_parts.push(doc.into_json().to_string());
            }
        }

        Ok(schema_parts.join("\n"))
    }

    /// Check if the driver connection is open.
    pub fn is_connected(&self) -> bool {
        self.driver.is_open()
    }
}

#[async_trait::async_trait]
impl QueryExecutor for TypeDBClient {
    async fn execute(
        &self,
        database: &str,
        typeql: &str,
        transaction_type: &str,
    ) -> Result<serde_json::Value, PipelineError> {
        self.execute(database, typeql, transaction_type).await
    }

    fn is_connected(&self) -> bool {
        self.is_connected()
    }
}

/// Convert a TypeDB Concept to a serde_json::Value.
fn concept_to_json(concept: &Concept) -> serde_json::Value {
    let mut obj = serde_json::Map::new();

    obj.insert(
        "category".to_string(),
        serde_json::Value::String(concept.get_category().name().to_string()),
    );
    obj.insert(
        "label".to_string(),
        serde_json::Value::String(concept.get_label().to_string()),
    );

    if let Some(iid) = concept.try_get_iid() {
        obj.insert(
            "iid".to_string(),
            serde_json::Value::String(iid.to_string()),
        );
    }

    if let Some(value) = concept.try_get_value() {
        obj.insert("value".to_string(), value_to_json(value));
    }

    if let Some(value_type) = concept.try_get_value_type() {
        obj.insert(
            "value_type".to_string(),
            serde_json::Value::String(value_type.name().to_string()),
        );
    }

    serde_json::Value::Object(obj)
}

/// Convert a TypeDB Value to a serde_json::Value.
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
    // Fallback: use Display representation
    serde_json::Value::String(value.to_string())
}
