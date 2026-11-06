//! Internal eager execution for schema migrations and legacy helpers until 3.0.

use super::backend::{GivenRowsSpec, QueryResult, TransactionOps, TxType};
use crate::error::{ClassifiedCommitError, OrmError, Result};
use type_bridge_core_lib::version::Version;

/// A single TypeDB transaction.
///
/// Write and schema transactions must be explicitly committed via
/// [`commit`](Self::commit). Read transactions do not need commit.
pub struct EagerTransaction {
    inner: Option<Box<dyn TransactionOps>>,
    tx_type: TxType,
    server_version: Option<Version>,
    never_commit: bool,
    plan_rollback_only: bool,
}

impl EagerTransaction {
    pub(crate) fn new(
        inner: Box<dyn TransactionOps>,
        tx_type: TxType,
        server_version: Option<Version>,
    ) -> Self {
        Self {
            inner: Some(inner),
            tx_type,
            server_version,
            never_commit: false,
            plan_rollback_only: false,
        }
    }

    pub(crate) fn set_never_commit(&mut self, never_commit: bool) {
        self.never_commit = never_commit;
    }

    /// Open with the backend's certified schema-exclusion guarantee.
    pub(crate) async fn open_fenced(
        backend: &dyn super::backend::DriverBackend,
        database: &str,
        timeout: std::time::Duration,
        server_version: Option<Version>,
    ) -> Result<(Self, String)> {
        let options = super::staged::TransactionOptions::default()
            .with_deadline(timeout)
            .with_never_commit(true);
        let mut provider = backend
            .open_with_options(database, TxType::Write, &options)
            .await?;
        let schema = match backend.schema_text(database).await {
            Ok(schema) => schema,
            Err(error) => {
                let _ = provider.close().await;
                return Err(error);
            }
        };
        let mut transaction = Self::new(provider, TxType::Write, server_version);
        transaction.set_never_commit(true);
        Ok((transaction, schema))
    }

    /// Open immediately for schema migrations and legacy prerequisite reads.
    pub async fn open(
        database: &super::database::Database,
        kind: super::staged::TransactionKind,
        options: super::staged::TransactionOptions,
    ) -> Result<Self> {
        let provider = database
            .backend
            .open_with_options(database.database_name(), kind.into(), &options)
            .await?;
        let mut transaction = Self::new(provider, kind.into(), database.server_version());
        transaction.set_never_commit(options.never_commit);
        Ok(transaction)
    }

    fn check_plan_latch(&self) -> Result<()> {
        if self.plan_rollback_only {
            return Err(OrmError::Transaction("Transaction is rollback-only".into()));
        }
        Ok(())
    }

    /// Execute read plans on Read, Write or Schema transactions, and write
    /// plans on Write or Schema transactions, without changing ownership.
    pub async fn execute_plans(
        &mut self,
        plans: &[type_bridge_contract::query_plan::v3::QueryPlanV3],
        policy: &crate::QueryExecutionResourceLimits,
        cancellation: &super::backend::AnswerCancellation,
        deadline: crate::QueryExecutionDeadline,
    ) -> Result<Vec<super::staged::PlanAnswer>> {
        self.check_plan_latch()?;
        let writes = plans
            .iter()
            .any(type_bridge_contract::query_plan::v3::QueryPlanV3::writes);
        if writes && self.tx_type == TxType::Read {
            return Err(crate::match_request::MatchError::new(
                crate::match_request::MatchErrorCategory::InvalidPlan,
                "query_plan_read_only",
                "one-shot plan execution requires read-only plans",
            )
            .into());
        }
        let limits = crate::query_plan_exec::plan_answer_limits(*policy, cancellation, deadline)?;
        let provider = self
            .inner
            .as_mut()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        let given = provider.supports_given_rows();
        let mut queries = Vec::with_capacity(plans.len());
        for plan in plans {
            crate::query_plan_exec::check_provider(plan, self.server_version)
                .map_err(crate::query_plan_exec::plan_diagnostic_error)?;
            queries.push(
                if given {
                    crate::query_plan_exec::staged_query(plan, true)
                } else {
                    crate::query_plan_exec::staged_literal_query(plan)
                }
                .map_err(crate::query_plan_exec::plan_diagnostic_error)?,
            );
        }
        // This assignment survives a dropped execution future.
        self.plan_rollback_only = writes;
        let result = super::staged::bounded(
            &limits.answer,
            crate::query_plan_exec::execute_plans_counted::<true>(
                provider.as_mut(),
                given,
                plans,
                *policy,
                cancellation,
                deadline,
                Some(queries),
            ),
        )
        .await;
        if result.is_ok() {
            self.plan_rollback_only = false;
        }
        result.map(|(answers, _)| answers)
    }

    /// Execute a TypeQL query within this transaction.
    pub async fn query(&mut self, typeql: &str) -> Result<QueryResult> {
        self.check_plan_latch()?;
        self.check_schema_annotation_support(typeql)?;
        let tx = self
            .inner
            .as_mut()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.query(typeql).await
    }

    /// Borrow the provider boundary without transferring transaction lifecycle ownership.
    pub(crate) fn provider_mut(&mut self) -> Result<&mut (dyn TransactionOps + 'static)> {
        self.check_plan_latch()?;
        self.inner
            .as_mut()
            .map(Box::as_mut)
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))
    }

    /// Hand the provider back without closing it; `None` once consumed.
    pub(crate) fn into_provider(mut self) -> Option<Box<dyn TransactionOps>> {
        self.inner.take()
    }

    pub(crate) fn provider_ref(&self) -> Result<&dyn TransactionOps> {
        self.inner
            .as_deref()
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
        self.check_plan_latch()?;
        self.check_schema_annotation_support(typeql)?;
        let tx = self
            .inner
            .as_mut()
            .ok_or_else(|| OrmError::Transaction("Transaction already consumed".into()))?;
        tx.query_with_rows(typeql, rows).await
    }

    /// Commit this transaction.
    pub async fn commit(&mut self) -> Result<()> {
        self.check_plan_latch()?;
        if self.never_commit {
            return self.close().await;
        }
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
        self.check_plan_latch()
            .map_err(ClassifiedCommitError::from)?;
        if self.never_commit {
            return self.close().await.map_err(Into::into);
        }
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
        match tx.rollback().await {
            Ok(()) => {
                self.inner = Some(tx);
                self.plan_rollback_only = false;
                Ok(())
            }
            Err(error) => {
                let _ = tx.close().await;
                Err(error)
            }
        }
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
            crate::_schema::annotations::check_schema_annotation_support(
                typeql,
                self.server_version,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::backend::{BoxFuture, DriverBackend};
    use crate::session::staged::TransactionOptions;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    struct FenceBackend {
        events: Arc<Mutex<Vec<&'static str>>>,
        fail_export: bool,
    }

    struct FenceTransaction(Arc<Mutex<Vec<&'static str>>>);

    impl TransactionOps for FenceTransaction {
        fn query(&mut self, _: &str) -> BoxFuture<'_, Result<QueryResult>> {
            Box::pin(async { Ok(QueryResult::Ok) })
        }
        fn commit(&mut self) -> BoxFuture<'_, Result<()>> {
            panic!("fenced transactions must never commit")
        }
        fn rollback(&mut self) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn close(&mut self) -> BoxFuture<'_, Result<()>> {
            self.0.lock().unwrap().push("close");
            Box::pin(async { Ok(()) })
        }
    }

    impl DriverBackend for FenceBackend {
        fn open_transaction(
            &self,
            _: &str,
            _: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>>> {
            panic!("fenced opens must retain transaction options")
        }
        fn open_with_options(
            &self,
            database: &str,
            kind: TxType,
            options: &TransactionOptions,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>>> {
            assert_eq!(database, "fenced");
            assert_eq!(kind, TxType::Write);
            assert_eq!(options.deadline, Some(Duration::from_secs(7)));
            assert!(options.never_commit);
            self.events.lock().unwrap().push("open");
            let transaction = FenceTransaction(Arc::clone(&self.events));
            Box::pin(async { Ok(Box::new(transaction) as Box<dyn TransactionOps>) })
        }
        fn schema_text(&self, _: &str) -> BoxFuture<'_, Result<String>> {
            self.events.lock().unwrap().push("export");
            Box::pin(async {
                if self.fail_export {
                    Err(OrmError::Connection("schema export failed".into()))
                } else {
                    Ok("define entity marker;".into())
                }
            })
        }
        fn is_open(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn fenced_open_keeps_options_and_never_commits() {
        let backend = FenceBackend {
            events: Arc::default(),
            fail_export: false,
        };
        let (mut transaction, schema) =
            EagerTransaction::open_fenced(&backend, "fenced", Duration::from_secs(7), None)
                .await
                .unwrap();
        assert_eq!(schema, "define entity marker;");
        assert_eq!(*backend.events.lock().unwrap(), ["open", "export"]);
        transaction.commit().await.unwrap();
        assert_eq!(*backend.events.lock().unwrap(), ["open", "export", "close"]);
    }

    #[tokio::test]
    async fn fenced_export_failure_closes_transaction() {
        let backend = FenceBackend {
            events: Arc::default(),
            fail_export: true,
        };
        let error = EagerTransaction::open_fenced(&backend, "fenced", Duration::from_secs(7), None)
            .await
            .err()
            .expect("export failed");
        assert!(error.to_string().contains("schema export failed"));
        assert_eq!(*backend.events.lock().unwrap(), ["open", "export", "close"]);
    }
}
