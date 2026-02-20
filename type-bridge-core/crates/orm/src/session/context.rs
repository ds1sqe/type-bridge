//! Shared transaction context for grouping multiple operations.

use std::sync::Arc;

use tokio::sync::Mutex;

use super::backend::{QueryResult, TransactionOps, TxType};
use crate::error::{OrmError, Result};

/// Shared transaction context for grouping multiple operations into
/// a single database transaction.
///
/// Cloneable via [`Arc`] — all clones share the same underlying
/// transaction. Call [`commit`](Self::commit) once when all operations
/// are complete.
pub struct TransactionContext {
    inner: Arc<Mutex<Option<Box<dyn TransactionOps>>>>,
    tx_type: TxType,
}

impl TransactionContext {
    pub(crate) fn new(inner: Box<dyn TransactionOps>, tx_type: TxType) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Some(inner))),
            tx_type,
        }
    }

    /// Execute a query on the shared transaction.
    pub async fn query(&self, typeql: &str) -> Result<QueryResult> {
        let mut guard = self.inner.lock().await;
        let tx = guard
            .as_mut()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.query(typeql).await
    }

    /// Commit the shared transaction.
    pub async fn commit(&self) -> Result<()> {
        let mut guard = self.inner.lock().await;
        let tx = guard
            .as_mut()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.commit().await
    }

    /// The transaction type.
    pub fn tx_type(&self) -> TxType {
        self.tx_type
    }
}

impl Clone for TransactionContext {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            tx_type: self.tx_type,
        }
    }
}
