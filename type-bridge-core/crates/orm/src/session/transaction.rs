//! Transaction wrapper for TypeDB operations.

use super::backend::{
    AnswerConsumer, BoundedAnswerLimits, BoundedAnswerStats, GivenRowsSpec, QueryResult,
    TransactionOps, TxType,
};
use crate::error::{ClassifiedCommitError, Result};
use type_bridge_core_lib::ast::{
    TypedFetchRows, TypedHydrateThings, TypedPageRematch, TypedRootScan,
};

/// A single TypeDB transaction.
///
/// Write and schema transactions must be explicitly committed via
/// [`commit`](Self::commit). Read transactions do not need commit.
pub struct OwnedTransaction {
    inner: super::eager::EagerTransaction,
}

impl OwnedTransaction {
    pub(crate) fn from_eager(inner: super::eager::EagerTransaction) -> Self {
        Self { inner }
    }

    /// Execute a TypeQL query within this transaction.
    pub async fn query(&mut self, typeql: &str) -> Result<QueryResult> {
        self.inner.query(typeql).await
    }

    /// Hand the provider transaction back to its owner, or `None` when this
    /// transaction already consumed it by committing or closing.
    pub(crate) fn into_provider(self) -> Option<Box<dyn TransactionOps>> {
        self.inner.into_provider()
    }

    /// Borrow the provider boundary without transferring transaction lifecycle ownership.
    pub(crate) fn provider_mut(&mut self) -> Result<&mut (dyn TransactionOps + 'static)> {
        self.inner.provider_mut()
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
        self.inner.query_with_rows(typeql, rows).await
    }

    /// Execute one internal typed selected-row statement with bounded reading.
    pub(crate) async fn query_typed_bounded(
        &mut self,
        query: &TypedFetchRows,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats> {
        self.inner
            .query_typed_bounded(query, limits, consumer)
            .await
    }

    pub(crate) fn supports_exactly_one_tuple_proof(&self) -> Result<bool> {
        self.inner.supports_exactly_one_tuple_proof()
    }

    /// Execute one internal distinct selected-tuple identity scan.
    pub(crate) async fn query_tuple_typed_bounded(
        &mut self,
        query: &TypedFetchRows,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats> {
        self.inner
            .query_tuple_typed_bounded(query, limits, consumer)
            .await
    }

    /// Execute one internal complete batched selected-thing hydration.
    pub(crate) async fn hydrate_typed_bounded(
        &mut self,
        query: &TypedHydrateThings,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats> {
        self.inner
            .hydrate_typed_bounded(query, limits, consumer)
            .await
    }

    /// Execute one internal typed distinct-root stream.
    pub(crate) async fn query_root_typed_bounded(
        &mut self,
        query: &TypedRootScan,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats> {
        self.inner
            .query_root_typed_bounded(query, limits, consumer)
            .await
    }

    /// Execute one internal exact batched page re-match/hydration.
    pub(crate) async fn rematch_page_typed_bounded(
        &mut self,
        query: &TypedPageRematch,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats> {
        self.inner
            .rematch_page_typed_bounded(query, limits, consumer)
            .await
    }

    /// Commit this transaction.
    pub async fn commit(&mut self) -> Result<()> {
        self.inner.commit().await
    }

    /// Commit while retaining provider durability certainty when available.
    ///
    /// Only known pre-commit rejection codes prove that no write committed;
    /// other server failures can still have an unknown durability outcome.
    /// Recovery-aware callers can inspect
    /// [`ClassifiedCommitError::commit_failure_certainty`]. Ordinary callers
    /// should continue to use [`Self::commit`], whose error surface is
    /// unchanged from the released API.
    pub async fn commit_classified(&mut self) -> std::result::Result<(), ClassifiedCommitError> {
        self.inner.commit_classified().await
    }

    /// Roll back writes and keep this transaction usable.
    pub async fn rollback(&mut self) -> Result<()> {
        self.inner.rollback().await
    }

    /// Close this transaction without committing.
    pub async fn close(&mut self) -> Result<()> {
        self.inner.close().await
    }

    /// The transaction type.
    pub fn tx_type(&self) -> TxType {
        self.inner.tx_type()
    }
}

/// Released eager transaction name, retained until 3.0.
#[deprecated(
    since = "2.3.0",
    note = "Use OwnedTransaction; staged Transaction takes the session-level name in 3.0"
)]
pub type Transaction = OwnedTransaction;
