//! Deferred transaction execution shared by one-shot and staged callers.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::backend::{BoundedAnswerLimits, DriverBackend, GivenRowsSpec, QueryResult, TxType};
use crate::error::{ClassifiedCommitError, OrmError};

/// Provider transaction kind. An omitted kind is inferred from staged writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionKind {
    /// Read-only transaction.
    Read,
    /// Data transaction.
    Write,
    /// Schema transaction.
    Schema,
}
impl From<TransactionKind> for TxType {
    fn from(kind: TransactionKind) -> Self {
        match kind {
            TransactionKind::Read => Self::Read,
            TransactionKind::Write => Self::Write,
            TransactionKind::Schema => Self::Schema,
        }
    }
}
impl From<TxType> for TransactionKind {
    fn from(kind: TxType) -> Self {
        match kind {
            TxType::Read => Self::Read,
            TxType::Write => Self::Write,
            TxType::Schema => Self::Schema,
        }
    }
}

/// Options for one deferred transaction's atomic execution units.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct TransactionOptions {
    /// Explicit provider kind, or infer from the staged queries.
    pub kind: Option<TransactionKind>,
    /// Limits enforced while reading each answer. An absolute deadline supplied
    /// here is captured as a relative timeout when this transaction is created.
    pub answer_limits: BoundedAnswerLimits,
    /// Server-side transaction lifetime and schema-lock acquisition timeout.
    pub deadline: Option<Duration>,
    /// Close without committing, retaining answers only.
    pub never_commit: bool,
    /// Relative answer timeout refreshed at every commit.
    pub answer_timeout: Option<Duration>,
}
impl TransactionOptions {
    /// Set an explicit kind.
    pub fn with_kind(mut self, kind: TransactionKind) -> Self {
        self.kind = Some(kind);
        self
    }
    /// Set answer limits.
    pub fn with_answer_limits(mut self, limits: BoundedAnswerLimits) -> Self {
        if let Some(deadline) = limits.deadline {
            let timeout = deadline.saturating_duration_since(Instant::now());
            self.answer_timeout = Some(
                self.answer_timeout
                    .map_or(timeout, |existing| existing.min(timeout)),
            );
        }
        self.answer_limits = BoundedAnswerLimits {
            deadline: None,
            ..limits
        };
        self
    }
    /// Set a relative local answer timeout for each atomic execution.
    pub fn with_answer_timeout(mut self, timeout: Duration) -> Self {
        self.answer_timeout = Some(timeout);
        self
    }
    /// Set the server-side lifetime.
    pub fn with_deadline(mut self, deadline: Duration) -> Self {
        self.deadline = Some(deadline);
        self
    }
    /// Close each execution without committing.
    pub fn with_never_commit(mut self, never_commit: bool) -> Self {
        self.never_commit = never_commit;
        self
    }
}

/// TypeQL and its transport inputs, until typed plans replace this adapter.
#[derive(Debug, Clone)]
pub struct StagedQuery {
    /// TypeQL statement.
    pub typeql: String,
    /// Optional band-9 given rows.
    pub rows: Option<GivenRowsSpec>,
    /// Whether the statement may write.
    pub writes: bool,
    pub(crate) canonical: bool,
}
impl StagedQuery {
    /// Stage raw TypeQL with an explicit write declaration.
    pub fn new(typeql: impl Into<String>, writes: bool) -> Self {
        Self {
            typeql: typeql.into(),
            rows: None,
            writes,
            canonical: false,
        }
    }
    /// Attach given-stage inputs.
    pub fn with_rows(mut self, rows: GivenRowsSpec) -> Self {
        self.rows = Some(rows);
        self
    }
    /// Use exact canonical scalar encoding for this query.
    pub fn canonical(mut self) -> Self {
        self.canonical = true;
        self
    }
}

/// Lifecycle or execution failure observed through a pending answer.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum PendingError {
    /// Commit has not resolved this answer yet.
    #[error("Transaction has not committed")]
    NotCommitted,
    /// The logical transaction was closed or dropped.
    #[error("Transaction is closed")]
    Closed,
    /// Rollback discarded the staged query.
    #[error("Staged query was rolled back")]
    RolledBack,
    /// An explicit read transaction refuses writes.
    #[error("Read transaction cannot stage a write")]
    ReadOnly,
    /// Every pending answer observes the same failed execution.
    #[error("Transaction execution failed{index}: {message}", index = format_op_index(*op_index))]
    Failed {
        /// Failing statement index, or none for opening/commit.
        op_index: Option<usize>,
        /// Original error text.
        message: String,
    },
}

fn format_op_index(index: Option<usize>) -> String {
    index
        .map(|index| format!(" at operation {index}"))
        .unwrap_or_default()
}

/// Commit failure retaining the released ORM/provider error without reclassification.
#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub struct CommitError {
    /// Failing query index, or none for opening/commit.
    pub op_index: Option<usize>,
    /// Original failure, including existing commit certainty.
    #[source]
    pub error: ClassifiedCommitError,
}
impl CommitError {
    fn lifecycle(error: PendingError) -> Self {
        Self {
            op_index: None,
            error: OrmError::Transaction(error.to_string()).into(),
        }
    }
}

type AnswerState<T> = Arc<Mutex<Result<Arc<T>, PendingError>>>;
/// An answer made visible only after its whole execution succeeds.
pub struct Pending<T> {
    state: AnswerState<T>,
}
impl<T> std::fmt::Debug for Pending<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state.lock().unwrap();
        let status = match &*state {
            Ok(_) => "ready",
            Err(PendingError::NotCommitted) => "pending",
            Err(_) => "failed",
        };
        formatter
            .debug_struct("Pending")
            .field("state", &status)
            .finish_non_exhaustive()
    }
}
impl<T> Clone for Pending<T> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
        }
    }
}
impl<T> Pending<T> {
    /// Read the answer, or its current lifecycle/execution failure.
    pub fn get(&self) -> Result<Arc<T>, PendingError> {
        self.state.lock().unwrap().clone()
    }
}
struct Item {
    query: StagedQuery,
    state: AnswerState<QueryResult>,
}
struct Execution(Vec<Item>);
impl Execution {
    fn fail(&self, error: PendingError) {
        for item in &self.0 {
            *item.state.lock().unwrap() = Err(error.clone());
        }
    }
}
impl Drop for Execution {
    fn drop(&mut self) {
        // Also resolves handles when a commit future is cancelled.
        for item in &self.0 {
            let mut state = item.state.lock().unwrap();
            if matches!(*state, Err(PendingError::NotCommitted)) {
                *state = Err(PendingError::Closed);
            }
        }
    }
}

/// A reusable logical transaction. Each commit is one atomic provider transaction.
/// Nothing opens until commit; drop discards staged work without committing.
pub struct Transaction {
    backend: Arc<dyn DriverBackend>,
    database: String,
    options: TransactionOptions,
    stack: Execution,
    closed: bool,
}
impl std::fmt::Debug for Transaction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Transaction")
            .field("kind", &self.options.kind)
            .field("staged_queries", &self.stack.0.len())
            .field("closed", &self.closed)
            .finish_non_exhaustive()
    }
}
impl Transaction {
    pub(crate) fn new(
        backend: Arc<dyn DriverBackend>,
        database: String,
        mut options: TransactionOptions,
    ) -> Self {
        if let Some(deadline) = options.answer_limits.deadline.take() {
            let timeout = deadline.saturating_duration_since(Instant::now());
            options.answer_timeout = Some(
                options
                    .answer_timeout
                    .map_or(timeout, |existing| existing.min(timeout)),
            );
        }
        Self {
            backend,
            database,
            options,
            stack: Execution(Vec::new()),
            closed: false,
        }
    }
    /// Queue one query without opening a provider transaction.
    pub fn stage(&mut self, query: StagedQuery) -> Result<Pending<QueryResult>, PendingError> {
        if self.closed {
            return Err(PendingError::Closed);
        }
        if query.writes && self.options.kind == Some(TransactionKind::Read) {
            return Err(PendingError::ReadOnly);
        }
        let state = Arc::new(Mutex::new(Err(PendingError::NotCommitted)));
        self.stack.0.push(Item {
            query,
            state: Arc::clone(&state),
        });
        Ok(Pending { state })
    }
    /// Fire queued queries in order and expose their answers after success only.
    pub async fn commit(&mut self) -> Result<(), CommitError> {
        if self.closed {
            return Err(CommitError::lifecycle(PendingError::Closed));
        }
        let execution = std::mem::replace(&mut self.stack, Execution(Vec::new()));
        if execution.0.is_empty() {
            return Ok(());
        }
        let kind = self.options.kind.unwrap_or_else(|| {
            if execution.0.iter().any(|i| i.query.writes) {
                TransactionKind::Write
            } else {
                TransactionKind::Read
            }
        });
        let timeout = match (self.options.deadline, self.options.answer_timeout) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        };
        let mut answer_limits = self.options.answer_limits.clone();
        let result = async {
            answer_limits.deadline = timeout
                .map(|timeout| {
                    Instant::now().checked_add(timeout).ok_or_else(|| {
                        CommitError::lifecycle(PendingError::Failed {
                            op_index: None,
                            message: "Transaction deadline exceeds the supported range".into(),
                        })
                    })
                })
                .transpose()?;
            let mut provider = self
                .backend
                .open_with_options(&self.database, kind.into(), &self.options)
                .await
                .map_err(|error| CommitError {
                    op_index: None,
                    error: error.into(),
                })?;
            let mut answers = Vec::with_capacity(execution.0.len());
            for (index, item) in execution.0.iter().enumerate() {
                let result = provider
                    .query_materialized_bounded(
                        &item.query.typeql,
                        item.query.rows.clone(),
                        answer_limits.clone(),
                        item.query.canonical,
                    )
                    .await;
                match result {
                    Ok(answer) => answers.push(answer),
                    Err(error) => {
                        let _ = provider.rollback().await;
                        let _ = provider.close().await;
                        return Err(CommitError {
                            op_index: Some(index),
                            error: error.into(),
                        });
                    }
                }
            }
            if self.options.never_commit || kind == TransactionKind::Read {
                provider.close().await.map_err(|error| CommitError {
                    op_index: None,
                    error: error.into(),
                })?;
            } else {
                let commit = provider.commit_classified().await;
                let close = provider.close().await;
                commit.map_err(|error| CommitError {
                    op_index: None,
                    error,
                })?;
                if close.is_err() {
                    tracing::warn!(code = "transaction_close_failed");
                }
            }
            Ok(answers)
        }
        .await;
        match result {
            Ok(answers) => {
                for (item, answer) in execution.0.iter().zip(answers) {
                    *item.state.lock().unwrap() = Ok(Arc::new(answer));
                }
                Ok(())
            }
            Err(error) => {
                execution.fail(PendingError::Failed {
                    op_index: error.op_index,
                    message: error.to_string(),
                });
                Err(error)
            }
        }
    }
    /// Discard queued work and retain the logical transaction for reuse.
    pub fn rollback(&mut self) -> Result<(), PendingError> {
        if self.closed {
            return Err(PendingError::Closed);
        }
        self.stack.fail(PendingError::RolledBack);
        self.stack.0.clear();
        Ok(())
    }
    /// Discard queued work and make this logical transaction terminal.
    pub fn close(&mut self) -> Result<(), PendingError> {
        if self.closed {
            return Err(PendingError::Closed);
        }
        self.stack.fail(PendingError::Closed);
        self.stack.0.clear();
        self.closed = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::backend::{BoxFuture, TransactionOps};
    use super::*;

    #[derive(Default)]
    struct State {
        events: Vec<String>,
        kinds: Vec<TxType>,
        fail_close: bool,
    }
    struct Fake(Arc<Mutex<State>>);
    impl DriverBackend for Fake {
        fn open_transaction(
            &self,
            _: &str,
            kind: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            self.0.lock().unwrap().kinds.push(kind);
            Box::pin(async { Ok(Box::new(Fake(Arc::clone(&self.0))) as Box<dyn TransactionOps>) })
        }
        fn is_open(&self) -> bool {
            true
        }
    }
    impl TransactionOps for Fake {
        fn query(&mut self, query: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            self.0.lock().unwrap().events.push(query.to_owned());
            let fail = query == "fail";
            Box::pin(async move {
                if fail {
                    Err(OrmError::QueryExecution("failure".into()))
                } else {
                    Ok(QueryResult::Rows(Vec::new()))
                }
            })
        }
        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.0.lock().unwrap().events.push("commit".into());
            Box::pin(async { Ok(()) })
        }
        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.0.lock().unwrap().events.push("rollback".into());
            Box::pin(async { Ok(()) })
        }
        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            let mut state = self.0.lock().unwrap();
            state.events.push("close".into());
            let fail = state.fail_close;
            Box::pin(async move {
                if fail {
                    Err(OrmError::QueryExecution("close failed".into()))
                } else {
                    Ok(())
                }
            })
        }
    }
    fn fixture(options: TransactionOptions) -> (Transaction, Arc<Mutex<State>>) {
        let state = Arc::new(Mutex::new(State::default()));
        (
            Transaction::new(Arc::new(Fake(Arc::clone(&state))), "test".into(), options),
            state,
        )
    }
    #[tokio::test]
    async fn successful_commit_resolves_answers_even_when_close_fails() {
        let (mut tx, state) = fixture(TransactionOptions::default());
        state.lock().unwrap().fail_close = true;
        let answer = tx.stage(StagedQuery::new("write", true)).unwrap();
        tx.commit().await.unwrap();
        assert!(answer.get().is_ok());
        assert_eq!(state.lock().unwrap().events, ["write", "commit", "close"]);
    }

    #[tokio::test]
    async fn ordered_deferred_execution_resolves_only_after_commit_and_reuses() {
        let (mut tx, state) = fixture(TransactionOptions::default());
        let a = tx.stage(StagedQuery::new("a", true)).unwrap();
        let b = tx.stage(StagedQuery::new("b", false)).unwrap();
        assert!(state.lock().unwrap().kinds.is_empty());
        assert!(matches!(a.get(), Err(PendingError::NotCommitted)));
        tx.commit().await.unwrap();
        assert!(matches!(&*a.get().unwrap(), QueryResult::Rows(rows) if rows.is_empty()));
        assert!(b.get().is_ok());
        assert_eq!(state.lock().unwrap().events, ["a", "b", "commit", "close"]);
        assert_eq!(state.lock().unwrap().kinds, [TxType::Write]);
        tx.commit().await.unwrap(); // empty commit opens nothing
        tx.stage(StagedQuery::new("read", false)).unwrap();
        tx.commit().await.unwrap();
        assert_eq!(state.lock().unwrap().kinds, [TxType::Write, TxType::Read]);
        assert_eq!(
            state.lock().unwrap().events,
            ["a", "b", "commit", "close", "read", "close"]
        );
    }
    #[tokio::test]
    async fn failure_stops_execution_and_fails_every_answer_at_the_same_index() {
        let (mut tx, state) = fixture(TransactionOptions::default());
        let a = tx.stage(StagedQuery::new("a", true)).unwrap();
        let b = tx.stage(StagedQuery::new("fail", true)).unwrap();
        let c = tx.stage(StagedQuery::new("later", true)).unwrap();
        assert_eq!(tx.commit().await.unwrap_err().op_index, Some(1));
        assert_eq!(a.get().unwrap_err(), b.get().unwrap_err());
        assert_eq!(b.get().unwrap_err(), c.get().unwrap_err());
        assert_eq!(
            state.lock().unwrap().events,
            ["a", "fail", "rollback", "close"]
        );
    }
    #[tokio::test]
    async fn never_commit_and_explicit_kinds() {
        let (mut tx, state) = fixture(
            TransactionOptions::default()
                .with_kind(TransactionKind::Schema)
                .with_never_commit(true),
        );
        tx.stage(StagedQuery::new("schema", true)).unwrap();
        tx.commit().await.unwrap();
        assert_eq!(state.lock().unwrap().events, ["schema", "close"]);
        assert_eq!(state.lock().unwrap().kinds, [TxType::Schema]);
        let (mut read, _) = fixture(TransactionOptions::default().with_kind(TransactionKind::Read));
        assert!(matches!(
            read.stage(StagedQuery::new("write", true)),
            Err(PendingError::ReadOnly)
        ));
    }
    #[tokio::test]
    async fn answer_timeout_refreshes_on_commit_reuse_and_debug_redacts_queries() {
        let limits = BoundedAnswerLimits {
            deadline: Some(Instant::now() + Duration::from_millis(40)),
            ..Default::default()
        };
        let (mut transaction, _) =
            fixture(TransactionOptions::default().with_answer_limits(limits));
        let answer = transaction
            .stage(StagedQuery::new("SECRET_QUERY", false))
            .unwrap();
        assert!(!format!("{transaction:?} {answer:?}").contains("SECRET_QUERY"));
        transaction.commit().await.unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
        transaction.stage(StagedQuery::new("reuse", false)).unwrap();
        transaction.commit().await.unwrap();
        assert_eq!(
            PendingError::Failed {
                op_index: Some(1),
                message: "failure".into()
            }
            .to_string(),
            "Transaction execution failed at operation 1: failure"
        );
    }

    #[tokio::test]
    async fn cancelled_commit_resolves_pending_answers() {
        struct Blocking;
        impl DriverBackend for Blocking {
            fn open_transaction(
                &self,
                _: &str,
                _: TxType,
            ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
                Box::pin(std::future::pending())
            }
            fn is_open(&self) -> bool {
                true
            }
        }
        let mut tx = Transaction::new(
            Arc::new(Blocking),
            "test".into(),
            TransactionOptions::default(),
        );
        let answer = tx.stage(StagedQuery::new("pending", false)).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(5), tx.commit())
                .await
                .is_err()
        );
        assert!(matches!(answer.get(), Err(PendingError::Closed)));
        tx.commit().await.unwrap();
    }

    #[tokio::test]
    async fn rollback_close_and_drop_resolve_discarded_answers() {
        let (mut tx, state) = fixture(TransactionOptions::default());
        let a = tx.stage(StagedQuery::new("discard", true)).unwrap();
        tx.rollback().unwrap();
        assert!(matches!(a.get(), Err(PendingError::RolledBack)));
        tx.stage(StagedQuery::new("keep", true)).unwrap();
        tx.commit().await.unwrap();
        let b = tx.stage(StagedQuery::new("discard", true)).unwrap();
        tx.close().unwrap();
        assert!(matches!(b.get(), Err(PendingError::Closed)));
        assert!(matches!(
            tx.stage(StagedQuery::new("closed", false)),
            Err(PendingError::Closed)
        ));
        assert!(tx.commit().await.is_err());
        assert_eq!(state.lock().unwrap().events, ["keep", "commit", "close"]);
        let (mut tx, _) = fixture(TransactionOptions::default());
        let pending = tx.stage(StagedQuery::new("drop", true)).unwrap();
        drop(tx);
        assert!(matches!(pending.get(), Err(PendingError::Closed)));
    }
}
