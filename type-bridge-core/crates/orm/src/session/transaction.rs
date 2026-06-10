//! Transaction wrapper for TypeDB operations.

use super::backend::{QueryResult, TransactionOps, TxType};
use crate::error::{OrmError, Result};

/// A single TypeDB transaction.
///
/// Write and schema transactions must be explicitly committed via
/// [`commit`](Self::commit). Read transactions do not need commit.
pub struct Transaction {
    inner: Option<Box<dyn TransactionOps>>,
    tx_type: TxType,
}

impl Transaction {
    pub(crate) fn new(inner: Box<dyn TransactionOps>, tx_type: TxType) -> Self {
        Self {
            inner: Some(inner),
            tx_type,
        }
    }

    /// Execute a TypeQL query within this transaction.
    pub async fn query(&mut self, typeql: &str) -> Result<QueryResult> {
        let tx = self
            .inner
            .as_mut()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.query(typeql).await
    }

    /// Commit this transaction.
    pub async fn commit(&mut self) -> Result<()> {
        let mut tx = self
            .inner
            .take()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.commit().await
    }

    /// Roll back this transaction.
    pub async fn rollback(&mut self) -> Result<()> {
        let mut tx = self
            .inner
            .take()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.rollback().await
    }

    /// Close this transaction without committing.
    pub async fn close(&mut self) -> Result<()> {
        let Some(mut tx) = self.inner.take() else {
            return Ok(());
        };
        tx.close().await
    }

    /// The transaction type.
    pub fn tx_type(&self) -> TxType {
        self.tx_type
    }
}
