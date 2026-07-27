//! Shared transaction context for grouping multiple operations.

use std::sync::Arc;

use tokio::sync::Mutex;

use super::backend::{
    AnswerConsumer, BoundedAnswerLimits, BoundedAnswerStats, GivenRowsSpec, QueryResult,
    TransactionOps, TxType,
};
use crate::error::{OrmError, Result};
use crate::match_request::selected_result_executor::SelectedResultExecutor;
use crate::match_request::{
    CapabilitySet, MatchExecutionLimits, ValidatedMatchRequest, ValidatedMatchResult,
};
use crate::registry::DescriptorRegistry;
use type_bridge_core_lib::ast::{
    TypedFetchRows, TypedHydrateThings, TypedPageRematch, TypedRootScan,
};

/// Shared transaction context for grouping multiple operations into
/// a single database transaction.
///
/// Cloneable via [`Arc`] — all clones share the same underlying
/// transaction. Call [`commit`](Self::commit) once when all operations
/// are complete.
pub struct TransactionContext {
    inner: Arc<Mutex<Option<Box<dyn TransactionOps>>>>,
    tx_type: TxType,
    match_capabilities: CapabilitySet,
}

impl TransactionContext {
    pub(crate) fn new(
        inner: Box<dyn TransactionOps>,
        tx_type: TxType,
        match_capabilities: CapabilitySet,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Some(inner))),
            tx_type,
            match_capabilities,
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

    /// Export the schema under this transaction's provider-side schema fence,
    /// when supported by the backend.
    pub(crate) async fn schema_snapshot(&self) -> Result<Option<String>> {
        let mut guard = self.inner.lock().await;
        let tx = guard
            .as_mut()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.schema_snapshot().await
    }

    /// Execute a `given`-stage query with input rows on the shared transaction.
    ///
    /// Requires a band-9 (TypeDB 3.12+) connection; see
    /// [`Database::check_given_stage_support`](super::database::Database::check_given_stage_support).
    pub async fn query_with_rows(&self, typeql: &str, rows: GivenRowsSpec) -> Result<QueryResult> {
        let mut guard = self.inner.lock().await;
        let tx = guard
            .as_mut()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.query_with_rows(typeql, rows).await
    }

    /// Execute one internal typed selected-row statement without consuming the
    /// caller-owned transaction context.
    pub(crate) async fn query_typed_bounded(
        &self,
        query: &TypedFetchRows,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats> {
        let mut guard = self.inner.lock().await;
        let tx = guard
            .as_mut()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.query_typed_bounded(query, limits, consumer).await
    }

    pub(crate) async fn supports_exactly_one_tuple_proof(&self) -> Result<bool> {
        let guard = self.inner.lock().await;
        let tx = guard
            .as_ref()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        Ok(tx.supports_exactly_one_tuple_proof())
    }

    /// Execute one distinct selected-tuple identity scan without consuming this context.
    pub(crate) async fn query_tuple_typed_bounded(
        &self,
        query: &TypedFetchRows,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats> {
        let mut guard = self.inner.lock().await;
        let tx = guard
            .as_mut()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.query_tuple_typed_bounded(query, limits, consumer).await
    }

    /// Execute one complete batched hydration without consuming this context.
    pub(crate) async fn hydrate_typed_bounded(
        &self,
        query: &TypedHydrateThings,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats> {
        let mut guard = self.inner.lock().await;
        let tx = guard
            .as_mut()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.hydrate_typed_bounded(query, limits, consumer).await
    }

    /// Execute one typed distinct-root stream without consuming this context.
    pub(crate) async fn query_root_typed_bounded(
        &self,
        query: &TypedRootScan,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats> {
        let mut guard = self.inner.lock().await;
        let tx = guard
            .as_mut()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.query_root_typed_bounded(query, limits, consumer).await
    }

    /// Execute one exact batched page re-match without consuming this context.
    pub(crate) async fn rematch_page_typed_bounded(
        &self,
        query: &TypedPageRematch,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats> {
        let mut guard = self.inner.lock().await;
        let tx = guard
            .as_mut()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.rematch_page_typed_bounded(query, limits, consumer).await
    }

    /// Commit the shared transaction.
    pub async fn commit(&self) -> Result<()> {
        let mut guard = self.inner.lock().await;
        let mut tx = guard
            .take()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.commit().await
    }

    /// Roll back the shared transaction.
    pub async fn rollback(&self) -> Result<()> {
        let mut guard = self.inner.lock().await;
        let mut tx = guard
            .take()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.rollback().await
    }

    /// Close the shared transaction without committing.
    pub async fn close(&self) -> Result<()> {
        let mut guard = self.inner.lock().await;
        let Some(mut tx) = guard.take() else {
            return Ok(());
        };
        tx.close().await
    }

    /// The transaction type.
    pub fn tx_type(&self) -> TxType {
        self.tx_type
    }

    /// Execute one validated selected-row request without consuming this read context.
    pub async fn execute_match(
        &self,
        registry: &DescriptorRegistry,
        validated: &ValidatedMatchRequest,
    ) -> Result<ValidatedMatchResult> {
        self.execute_match_with_limits(registry, validated, MatchExecutionLimits::default())
            .await
    }

    /// Execute one validated selected-row request with caller-tightened limits.
    pub async fn execute_match_with_limits(
        &self,
        registry: &DescriptorRegistry,
        validated: &ValidatedMatchRequest,
        limits: MatchExecutionLimits,
    ) -> Result<ValidatedMatchResult> {
        SelectedResultExecutor::new(registry, self.match_capabilities.clone(), limits)
            .execute_compatible_borrowed(self, validated)
            .await
    }
}

impl Clone for TransactionContext {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            tx_type: self.tx_type,
            match_capabilities: self.match_capabilities.clone(),
        }
    }
}
