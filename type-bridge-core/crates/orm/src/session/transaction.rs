//! Transaction wrapper for TypeDB operations.

use super::backend::{GivenRowsSpec, QueryResult, TransactionOps, TxType};
use crate::error::{ClassifiedCommitError, Result};

/// A single TypeDB transaction.
///
/// Write and schema transactions must be explicitly committed via
/// [`commit`](Self::commit). Read transactions do not need commit.
pub struct OwnedTransaction {
    inner: super::eager::EagerTransaction,
}

impl OwnedTransaction {
    /// Lend this exact provider transaction to eager typed managers and raw
    /// queries. The scope owns invalidation; borrowed contexts cannot commit,
    /// roll back or close. Call `finish` before resuming engine execution.
    #[doc(hidden)]
    pub fn borrow_context<'a>(
        &'a mut self,
        database: &super::database::Database,
    ) -> BorrowedTransactionContext<'a> {
        let context = database
            .context_from_owned(self.inner.take_for_borrow())
            .borrowed();
        BorrowedTransactionContext {
            owner: self,
            context,
            finished: false,
        }
    }

    pub(crate) fn from_eager(inner: super::eager::EagerTransaction) -> Self {
        Self { inner }
    }

    /// Execute read plans on Read, Write or Schema transactions, and write
    /// plans on Write or Schema transactions, without changing ownership.
    pub async fn execute_plans(
        &mut self,
        plans: &[type_bridge_contract::query_plan::QueryPlan],
        limits: &crate::QueryExecutionResourceLimits,
        cancellation: &super::backend::AnswerCancellation,
        deadline: crate::QueryExecutionDeadline,
    ) -> Result<Vec<super::staged::PlanAnswer>> {
        self.inner
            .execute_plans(plans, limits, cancellation, deadline)
            .await
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

/// Engine-owned scope for a callback's temporary transaction context.
#[doc(hidden)]
pub struct BorrowedTransactionContext<'a> {
    owner: &'a mut OwnedTransaction,
    context: super::context::TransactionContext,
    finished: bool,
}
impl BorrowedTransactionContext<'_> {
    /// Shared execution only; lifecycle calls are refused by the context.
    pub fn context(&self) -> &super::context::TransactionContext {
        &self.context
    }

    /// Invalidate all callback handles and return the same transaction to its owner.
    pub async fn finish(mut self) -> Result<()> {
        self.owner.inner = self.context.detach_borrowed().await?;
        self.finished = true;
        Ok(())
    }
}
impl Drop for BorrowedTransactionContext<'_> {
    fn drop(&mut self) {
        if !self.finished {
            if let Some(mut transaction) = self.context.detach_borrowed_now() {
                transaction.latch_rollback_only();
                self.owner.inner = transaction;
            } else {
                self.context.abandon_borrowed();
            }
        }
    }
}
