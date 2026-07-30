//! Transaction wrapper for TypeDB operations.

use super::backend::{
    AnswerConsumer, BoundedAnswerLimits, BoundedAnswerStats, GivenRowsSpec, QueryResult,
    TransactionOps, TxType,
};
use crate::error::{ClassifiedCommitError, OrmError, Result};
use type_bridge_core_lib::ast::{
    TypedFetchRows, TypedHydrateThings, TypedPageRematch, TypedRootScan,
};
use type_bridge_core_lib::version::Version;

/// A single TypeDB transaction.
///
/// Write and schema transactions must be explicitly committed via
/// [`commit`](Self::commit). Read transactions do not need commit.
pub struct Transaction {
    inner: Option<Box<dyn TransactionOps>>,
    tx_type: TxType,
    server_version: Option<Version>,
}

impl Transaction {
    pub(crate) fn new(
        inner: Box<dyn TransactionOps>,
        tx_type: TxType,
        server_version: Option<Version>,
    ) -> Self {
        Self {
            inner: Some(inner),
            tx_type,
            server_version,
        }
    }

    /// Execute a TypeQL query within this transaction.
    pub async fn query(&mut self, typeql: &str) -> Result<QueryResult> {
        self.check_schema_annotation_support(typeql)?;
        let tx = self
            .inner
            .as_mut()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.query(typeql).await
    }

    /// Borrow the provider boundary without transferring transaction lifecycle ownership.
    pub(crate) fn provider_mut(&mut self) -> Result<&mut (dyn TransactionOps + 'static)> {
        self.inner
            .as_mut()
            .map(Box::as_mut)
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))
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
        self.check_schema_annotation_support(typeql)?;
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

    pub(crate) fn supports_exactly_one_tuple_proof(&self) -> Result<bool> {
        let tx = self
            .inner
            .as_ref()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        Ok(tx.supports_exactly_one_tuple_proof())
    }

    /// Execute one internal distinct selected-tuple identity scan.
    pub(crate) async fn query_tuple_typed_bounded(
        &mut self,
        query: &TypedFetchRows,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats> {
        let tx = self
            .inner
            .as_mut()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.query_tuple_typed_bounded(query, limits, consumer).await
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

    /// Commit while retaining provider durability certainty when available.
    ///
    /// Recovery-aware callers can inspect
    /// [`ClassifiedCommitError::commit_failure_certainty`]. Ordinary callers
    /// should continue to use [`Self::commit`], whose error surface is
    /// unchanged from the released API.
    pub async fn commit_classified(&mut self) -> std::result::Result<(), ClassifiedCommitError> {
        let mut tx = self.inner.take().ok_or_else(|| {
            ClassifiedCommitError::from(OrmError::Transaction(
                "Transaction already consumed".into(),
            ))
        })?;
        tx.commit_classified().await
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

    fn check_schema_annotation_support(&self, typeql: &str) -> Result<()> {
        if self.tx_type == TxType::Schema {
            crate::schema::annotations::check_schema_annotation_support(
                typeql,
                self.server_version,
            )?;
        }
        Ok(())
    }
}
