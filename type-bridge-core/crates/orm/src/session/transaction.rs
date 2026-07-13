//! Transaction wrapper for TypeDB operations.

use super::backend::{
    AnswerConsumer, BoundedAnswerLimits, BoundedAnswerStats, GivenRowsSpec, QueryResult,
    TransactionOps, TxType,
};
use crate::error::{OrmError, Result};
use type_bridge_core_lib::ast::{
    TypedFetchRows, TypedHydrateThings, TypedPageRematch, TypedRootScan,
};

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

    /// Execute a TypeQL query with `given`-stage input rows.
    ///
    /// Requires a band-9 (TypeDB 3.12+) connection; see
    /// [`Database::check_given_stage_support`](super::database::Database::check_given_stage_support).
    pub async fn query_with_rows(
        &mut self,
        typeql: &str,
        rows: GivenRowsSpec,
    ) -> Result<QueryResult> {
        let tx = self
            .inner
            .as_mut()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.query_with_rows(typeql, rows).await
    }

    /// Execute one internal typed selected-row statement with bounded reading.
    pub(crate) async fn query_typed_bounded(
        &mut self,
        query: &TypedFetchRows,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats> {
        let tx = self
            .inner
            .as_mut()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.query_typed_bounded(query, limits, consumer).await
    }

    /// Execute one internal complete batched selected-thing hydration.
    pub(crate) async fn hydrate_typed_bounded(
        &mut self,
        query: &TypedHydrateThings,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats> {
        let tx = self
            .inner
            .as_mut()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.hydrate_typed_bounded(query, limits, consumer).await
    }

    /// Execute one internal typed distinct-root stream.
    pub(crate) async fn query_root_typed_bounded(
        &mut self,
        query: &TypedRootScan,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats> {
        let tx = self
            .inner
            .as_mut()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.query_root_typed_bounded(query, limits, consumer).await
    }

    /// Execute one internal exact batched page re-match/hydration.
    pub(crate) async fn rematch_page_typed_bounded(
        &mut self,
        query: &TypedPageRematch,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats> {
        let tx = self
            .inner
            .as_mut()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.rematch_page_typed_bounded(query, limits, consumer).await
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
