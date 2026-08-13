//! Shared transaction context for grouping multiple operations.

use std::sync::Arc;

use tokio::sync::Mutex;

use super::backend::{
    AnswerConsumer, BoundedAnswerLimits, BoundedAnswerStats, GivenRowsSpec, QueryResult,
    TransactionOps, TxType,
};
use super::database::DatabaseExecutionIdentity;
use crate::_registry::DescriptorRegistry;
use crate::error::{ClassifiedCommitError, OrmError, Result};
use crate::match_request::selected_result_executor::SelectedResultExecutor;
use crate::match_request::{
    CapabilitySet, MatchExecutionLimits, ValidatedMatchRequest, ValidatedMatchResult,
};
use type_bridge_core_lib::ast::{
    TypedFetchRows, TypedHydrateThings, TypedPageRematch, TypedRootScan,
};
use type_bridge_core_lib::version::Version;

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
    server_version: Option<Version>,
    execution_identity: DatabaseExecutionIdentity,
}

impl TransactionContext {
    pub(crate) fn new(
        inner: Box<dyn TransactionOps>,
        tx_type: TxType,
        match_capabilities: CapabilitySet,
        server_version: Option<Version>,
        execution_identity: DatabaseExecutionIdentity,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Some(inner))),
            tx_type,
            match_capabilities,
            server_version,
            execution_identity,
        }
    }

    pub(crate) const fn execution_identity(&self) -> &DatabaseExecutionIdentity {
        &self.execution_identity
    }

    /// Execute a query on the shared transaction.
    pub async fn query(&self, typeql: &str) -> Result<QueryResult> {
        self.check_schema_annotation_support(typeql)?;
        let mut guard = self.inner.lock().await;
        let tx = guard
            .as_mut()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.query(typeql).await
    }

    /// Execute a canonical provider-answer query for binding-neutral CRUD.
    #[doc(hidden)]
    pub(crate) async fn query_canonical(&self, typeql: &str) -> Result<QueryResult> {
        self.check_schema_annotation_support(typeql)?;
        let mut guard = self.inner.lock().await;
        let tx = guard
            .as_mut()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.query_canonical(typeql).await
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
        self.check_schema_annotation_support(typeql)?;
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

    /// Return whether this borrowed transaction can transport canonical given rows.
    pub(crate) async fn supports_given_rows(&self) -> Result<bool> {
        let guard = self.inner.lock().await;
        let tx = guard
            .as_ref()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        Ok(tx.supports_given_rows())
    }

    /// Return whether both the borrowed transaction transport and its
    /// authoritatively negotiated server can execute a `given` stage.
    pub(crate) async fn supports_given_stage(&self) -> Result<bool> {
        use type_bridge_core_lib::version::{Feature, check_feature_supported};

        let server_supported = self
            .server_version
            .is_some_and(|server| check_feature_supported(Feature::GivenStage, &server).is_ok());
        let transport_supported = self.supports_given_rows().await?;
        Ok(server_supported && transport_supported)
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

    /// Commit while retaining provider durability certainty when available.
    ///
    /// This consumes the shared transaction exactly once, just like
    /// [`Self::commit`]. Recovery-aware binding runtimes can distinguish a
    /// provider-proven abort from an unknown commit outcome without exposing
    /// provider text across their public boundary.
    pub async fn commit_classified(&self) -> std::result::Result<(), ClassifiedCommitError> {
        let mut guard = self.inner.lock().await;
        let mut tx = guard.take().ok_or_else(|| {
            ClassifiedCommitError::from(OrmError::Transaction(
                "Transaction already consumed".into(),
            ))
        })?;
        tx.commit_classified().await
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

    fn check_schema_annotation_support(&self, typeql: &str) -> Result<()> {
        if self.tx_type == TxType::Schema {
            crate::_schema::annotations::check_schema_annotation_support(
                typeql,
                self.server_version,
            )?;
        }
        Ok(())
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
            server_version: self.server_version,
            execution_identity: self.execution_identity.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CommitFailureCertainty;
    use crate::session::backend::{BoxFuture, QueryResult};

    struct ClassifiedCommitTransaction {
        certainty: CommitFailureCertainty,
    }

    impl TransactionOps for ClassifiedCommitTransaction {
        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult>> {
            Box::pin(async { Ok(QueryResult::Ok) })
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { panic!("classified context commit used the lossy commit path") })
        }

        fn commit_classified(
            &mut self,
        ) -> BoxFuture<'_, std::result::Result<(), ClassifiedCommitError>> {
            let certainty = self.certainty;
            Box::pin(async move {
                Err(ClassifiedCommitError::Driver {
                    certainty,
                    message: "provider text retained inside the ORM only".into(),
                })
            })
        }

        fn rollback(&mut self) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn shared_context_preserves_definitely_aborted_commit_certainty() {
        let context = TransactionContext::new(
            Box::new(ClassifiedCommitTransaction {
                certainty: CommitFailureCertainty::DefinitelyAborted,
            }),
            TxType::Write,
            CapabilitySet::new(),
            None,
            DatabaseExecutionIdentity::isolated("commit-definitely-aborted"),
        );

        let error = context.commit_classified().await.unwrap_err();
        assert_eq!(
            error.commit_failure_certainty(),
            Some(CommitFailureCertainty::DefinitelyAborted)
        );
        assert!(matches!(
            context.commit_classified().await.unwrap_err(),
            ClassifiedCommitError::Orm(OrmError::Transaction(message))
                if message == "Transaction already consumed"
        ));
    }

    #[tokio::test]
    async fn shared_context_preserves_unknown_commit_certainty() {
        let context = TransactionContext::new(
            Box::new(ClassifiedCommitTransaction {
                certainty: CommitFailureCertainty::Unknown,
            }),
            TxType::Write,
            CapabilitySet::new(),
            None,
            DatabaseExecutionIdentity::isolated("commit-outcome-unknown"),
        );

        assert_eq!(
            context
                .commit_classified()
                .await
                .unwrap_err()
                .commit_failure_certainty(),
            Some(CommitFailureCertainty::Unknown)
        );
    }
}
