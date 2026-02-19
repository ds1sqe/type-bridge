//! Primary database connection handle.

use super::backend::{DriverBackend, QueryResult, TxType};
use super::context::TransactionContext;
use super::transaction::Transaction;
use crate::error::Result;

/// Primary connection handle wrapping a TypeDB driver.
///
/// Provides methods to create transactions and execute raw queries.
/// Use [`EntityManager`](crate::manager::EntityManager) for typed CRUD.
pub struct Database {
    backend: Box<dyn DriverBackend>,
    database_name: String,
}

impl Database {
    /// Create a Database with a custom backend (for testing).
    pub fn with_backend(
        backend: Box<dyn DriverBackend>,
        database_name: impl Into<String>,
    ) -> Self {
        Self {
            backend,
            database_name: database_name.into(),
        }
    }

    /// Connect to a TypeDB server.
    #[cfg(feature = "typedb")]
    pub async fn connect(
        address: &str,
        database: &str,
        username: &str,
        password: &str,
    ) -> Result<Self> {
        let backend = super::real_driver::RealBackend::connect(address, username, password).await?;
        Ok(Self {
            backend: Box::new(backend),
            database_name: database.to_string(),
        })
    }

    /// Open a read transaction.
    pub async fn read_transaction(&self) -> Result<Transaction> {
        let tx = self
            .backend
            .open_transaction(&self.database_name, TxType::Read)
            .await?;
        Ok(Transaction::new(tx, TxType::Read))
    }

    /// Open a write transaction.
    pub async fn write_transaction(&self) -> Result<Transaction> {
        let tx = self
            .backend
            .open_transaction(&self.database_name, TxType::Write)
            .await?;
        Ok(Transaction::new(tx, TxType::Write))
    }

    /// Create a shared [`TransactionContext`] for grouping operations.
    pub async fn transaction_context(&self, tx_type: TxType) -> Result<TransactionContext> {
        let tx = self
            .backend
            .open_transaction(&self.database_name, tx_type)
            .await?;
        Ok(TransactionContext::new(tx, tx_type))
    }

    /// Get the database name.
    pub fn database_name(&self) -> &str {
        &self.database_name
    }

    /// Check if the underlying connection is alive.
    pub fn is_connected(&self) -> bool {
        self.backend.is_open()
    }

    /// Execute a raw TypeQL query, auto-managing the transaction lifecycle.
    ///
    /// Opens a new transaction, executes the query, and commits if the
    /// transaction type is `Write` or `Schema`.
    #[tracing::instrument(skip(self, typeql), fields(db = %self.database_name))]
    pub async fn execute_raw(&self, typeql: &str, tx_type: TxType) -> Result<QueryResult> {
        let mut tx = self
            .backend
            .open_transaction(&self.database_name, tx_type)
            .await?;
        let result = tx.query(typeql).await?;
        if matches!(tx_type, TxType::Write | TxType::Schema) {
            tx.commit().await?;
        }
        Ok(result)
    }
}
