#![deny(missing_docs)]
//! Staged transactions: [`Database::transaction`](crate::Database::transaction)
//! and [`ProxyDatabase::transaction`](crate::ProxyDatabase::transaction).
//!
//! Nothing reaches TypeDB until [`Transaction::commit`]: commit opens one
//! provider transaction, runs every staged plan in order and commits. A
//! failing plan stops the commit and nothing is stored. Each staged plan's
//! [`Pending`] answer resolves only after a successful commit.

use std::marker::PhantomData;

pub use crate::plan::Pending;
pub use type_bridge_orm::session::staged::{
    CommitError, PendingError, PlanAnswer, TransactionKind, TransactionOptions,
};

use crate::plan::{Plan, PlanResult, StageSchema};
use crate::schema::Schema;

/// A reusable staged transaction over plans of schema `S`.
///
/// - [`Self::stage`] queues a plan and returns its pending answer.
/// - [`Self::commit`] runs the queue as one atomic provider transaction. A
///   `Read` transaction is closed instead of committed.
/// - [`Self::rollback`] discards the queue and keeps the transaction
///   usable; [`Self::close`] ends it.
///
/// The kind comes from [`TransactionOptions::kind`], or is inferred as
/// `Write` when a staged plan writes. A `Read` transaction refuses write
/// plans and a `Schema` transaction refuses every plan, both at stage time;
/// schema changes go through migrations. Dropping the transaction discards
/// its queue without committing.
pub struct Transaction<S: Schema> {
    inner: type_bridge_orm::session::staged::Transaction,
    origin: type_bridge_orm::ProjectedQueryOrigin,
    marker: PhantomData<fn() -> S>,
}

impl<S: Schema> std::fmt::Debug for Transaction<S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner.fmt(formatter)
    }
}

impl<S: Schema> Transaction<S> {
    pub(crate) fn new(inner: type_bridge_orm::session::staged::Transaction) -> Self {
        Self {
            inner,
            origin: type_bridge_orm::ProjectedQueryOrigin::remote_unbound(),
            marker: PhantomData,
        }
    }

    pub(crate) fn with_origin(mut self, origin: type_bridge_orm::ProjectedQueryOrigin) -> Self {
        self.origin = origin;
        self
    }

    /// Queue `plan` without opening a provider transaction.
    ///
    /// # Errors
    ///
    /// Returns [`PendingError::ReadOnly`] for a write plan in a `Read`
    /// transaction, [`PendingError::NotAllowedInKind`] in a `Schema`
    /// transaction, [`PendingError::InvalidPlan`] when the plan cannot run
    /// on this backend, and [`PendingError::Closed`] after [`Self::close`].
    ///
    /// A branded plan cannot cross generated schema packages:
    ///
    /// ```compile_fail
    /// use type_bridge::schema::{Schema, sealed};
    /// use type_bridge::plan::{Plan, StageSchema};
    /// use type_bridge::staged::{PlanAnswer, Transaction};
    /// struct A;
    /// struct B;
    /// impl sealed::Sealed for A {}
    /// impl sealed::Sealed for B {}
    /// impl Schema for A {}
    /// impl Schema for B {}
    /// impl StageSchema<A> for A {}
    /// impl StageSchema<B> for B {}
    /// fn reject(tx: &mut Transaction<A>, plan: &Plan<PlanAnswer, B>) {
    ///     tx.stage(plan).unwrap();
    /// }
    /// ```
    pub fn stage<R: PlanResult, P: StageSchema<S>>(
        &mut self,
        plan: &Plan<R, P>,
    ) -> Result<Pending<R>, PendingError> {
        plan.inner().map_err(|_| {
            PendingError::InvalidPlan(Box::new(type_bridge_contract::diagnostic::Diagnostic::new(
                type_bridge_contract::diagnostic::DiagnosticCategory::InvalidContract,
                type_bridge_contract::diagnostic::DiagnosticCode::new("plan_context_missing")
                    .expect("static plan diagnostic code"),
                "finish the recording before staging its held result",
            )))
        })?;
        plan.stage(&mut self.inner, self.origin.clone())
    }

    /// Run every staged plan in order as one provider transaction and
    /// resolve their answers. An empty queue commits nothing.
    ///
    /// # Errors
    ///
    /// Returns the failing plan's index and its typed failure; nothing was
    /// stored unless the failure says the outcome is unknown, as with a
    /// lost proxy reply; then the caller reads the database to find out.
    pub async fn commit(&mut self) -> Result<(), CommitError> {
        self.inner.commit().await
    }

    /// Discard the staged plans and keep the transaction usable. Their
    /// pending answers read [`PendingError::RolledBack`].
    ///
    /// # Errors
    ///
    /// Returns [`PendingError::Closed`] after [`Self::close`].
    pub fn rollback(&mut self) -> Result<(), PendingError> {
        self.inner.rollback()
    }

    /// Discard the staged plans and end the transaction.
    ///
    /// # Errors
    ///
    /// Returns [`PendingError::Closed`] when already closed.
    pub fn close(&mut self) -> Result<(), PendingError> {
        self.inner.close()
    }
}

/// A commit failure as an SDK [`Error`](crate::Error), keeping its canonical
/// diagnostic code, the failing plan index and the typed
/// [`failure`](crate::Error::failure).
impl From<CommitError> for crate::Error {
    fn from(error: CommitError) -> Self {
        Self::from_sdk_execution(error.diagnostic(), crate::ModelValidationPhase::Input)
    }
}

/// A staging or pending-answer failure as an SDK [`Error`](crate::Error).
impl From<PendingError> for crate::Error {
    fn from(error: PendingError) -> Self {
        match error {
            PendingError::Failed { diagnostic, .. } => {
                Self::from_sdk_execution(*diagnostic, crate::ModelValidationPhase::Input)
            }
            PendingError::Refused { diagnostic, .. } => {
                Self::from_sdk_execution(*diagnostic, crate::ModelValidationPhase::Input)
            }
            PendingError::InvalidPlan(diagnostic) | PendingError::NotAllowedInKind(diagnostic) => {
                Self::from_contract_diagnostic(*diagnostic)
            }
            error => Self::Other {
                message: error.to_string(),
                source: Some(Box::new(error)),
            },
        }
    }
}

#[cfg(test)]
mod document_answers {
    use super::PlanAnswer;

    #[test]
    fn documents_are_part_of_the_public_answer_enum() {
        let answer = PlanAnswer::Documents(vec![serde_json::json!({
            "_iid": "0x01", "_type": "person", "attributes": {"name": ["Ada"]}
        })]);
        match answer {
            PlanAnswer::Documents(documents) => assert_eq!(documents[0]["_type"], "person"),
            other => panic!("expected documents, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod pending_diagnostic_redaction_tests {
    use super::*;
    #[test]
    fn pending_failure_uses_the_canonical_diagnostic_without_server_text() {
        let error = PendingError::Failed {
            diagnostic: Box::new(
                type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic::commit_failure(
                    type_bridge_contract::sdk_diagnostic::SdkCommitFailureOutcome::Unknown,
                ),
            ),
            op_index: None,
            message: "signed-server-secret".into(),
            failure: Some(Box::new(
                type_bridge_contract::failure::Failure::Indeterminate { code: None },
            )),
            context: Default::default(),
        };
        assert!(!error.to_string().contains("signed-server-secret"));
        let surfaced = crate::Error::from(error);
        assert!(!surfaced.to_string().contains("signed-server-secret"));
        assert!(!format!("{surfaced:?}").contains("signed-server-secret"));
    }
}
