//! Deferred transaction execution shared by one-shot and staged callers.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::backend::{
    AnswerLimits, BoundedAnswerLimits, BoxFuture, DriverBackend, GivenRowsSpec, QueryResult, TxType,
};
use super::eager::EagerTransaction;
use super::staged_proxy::ProxyStaging;
use super::transaction::OwnedTransaction;
use crate::_registry::DescriptorRegistry;
use crate::error::{ClassifiedCommitError, OrmError};
use crate::match_request::selected_result_executor::SelectedResultExecutor;
use crate::match_request::{
    MatchError, MatchErrorCategory, MatchErrorPathSegment, MatchExecutionLimits,
    ValidatedMatchRequest, ValidatedMatchResult,
};
pub use crate::query_plan_exec::{PlanAnswer, merge_plan_answers};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::failure::Failure;
use type_bridge_contract::query_plan::v3::QueryPlanV3;
use type_bridge_contract::query_remote_v3::{
    SdkDiagnosticProjectionV3, StagedPlanV3, StagedRequestV3,
};
use type_bridge_contract::sdk_diagnostic::{
    FailureContext, SdkExecutionDiagnostic, SdkProviderOperation,
};

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
    /// Collect diagnostic phase timestamps without a tracing subscriber.
    pub collect_phase_timestamps: bool,
    /// Explicit provider kind, or infer from the staged queries.
    pub kind: Option<TransactionKind>,
    /// Limits enforced while reading each answer. An absolute deadline supplied
    /// here is captured as a relative timeout when this transaction is created.
    pub answer_limits: AnswerLimits,
    /// Server-side transaction lifetime and schema-lock acquisition timeout.
    pub deadline: Option<Duration>,
    /// Close without committing, retaining answers only.
    pub never_commit: bool,
    /// Relative answer timeout refreshed at every commit.
    pub answer_timeout: Option<Duration>,
    /// Per-call timeout, inherited from the connection when unset.
    /// Typed plans default to 60000 ms; raw queries have no implicit deadline.
    /// An explicit unbounded value disables this deadline; commit is never interrupted.
    pub timeout: Option<crate::QueryTimeout>,
}
impl TransactionOptions {
    /// Enable diagnostic timeline collection for each commit.
    pub fn with_collect_phase_timestamps(mut self, collect: bool) -> Self {
        self.collect_phase_timestamps = collect;
        self
    }

    /// Configure execution duration independently of plan construction.
    pub fn with_timeout(mut self, timeout: crate::QueryTimeout) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Set an explicit kind.
    pub fn with_kind(mut self, kind: TransactionKind) -> Self {
        self.kind = Some(kind);
        self
    }
    /// Set answer limits.
    pub fn with_answer_limits(mut self, limits: impl Into<AnswerLimits>) -> Self {
        let mut limits = limits.into();
        if let Some(deadline) = limits.answer.deadline {
            let timeout = deadline.saturating_duration_since(Instant::now());
            self.answer_timeout = Some(
                self.answer_timeout
                    .map_or(timeout, |existing| existing.min(timeout)),
            );
        }
        limits.answer.deadline = None;
        self.answer_limits = limits;
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
    pub(crate) fingerprint: Option<String>,
    pub(crate) redacted_typeql: Option<String>,
    pub(crate) redacted_snapshot: Option<String>,
    pub(crate) snapshot: Option<(String, PlanSource)>,
    /// Given columns rendered into literal terms; explain and traces must redact them.
    pub value_bearing_inputs: std::collections::BTreeSet<String>,
    /// Value-free schema context supplied by the plan lowering layer.
    pub failure_context: FailureContext,
}
impl StagedQuery {
    /// Stage raw TypeQL with an explicit write declaration.
    pub fn new(typeql: impl Into<String>, writes: bool) -> Self {
        Self {
            typeql: typeql.into(),
            rows: None,
            writes,
            canonical: false,
            fingerprint: None,
            redacted_typeql: None,
            redacted_snapshot: None,
            snapshot: None,
            value_bearing_inputs: Default::default(),
            failure_context: FailureContext::default(),
        }
    }
    pub(crate) async fn execute(
        &self,
        ops: &mut dyn super::backend::TransactionOps,
        limits: AnswerLimits,
    ) -> Result<QueryResult, OrmError> {
        self.execute_inner(ops, limits, false, None)
            .await
            .map(|(answer, _)| answer)
    }

    pub(crate) async fn execute_counted(
        &self,
        ops: &mut dyn super::backend::TransactionOps,
        limits: AnswerLimits,
        plan: Option<&QueryPlanV3>,
    ) -> Result<(QueryResult, super::backend::BoundedAnswerStats), OrmError> {
        self.execute_inner(ops, limits, true, plan).await
    }

    /// Execute the shared provider program, retaining validated capture evidence.
    #[tracing::instrument(target = "type_bridge.plan", name = "type_bridge.plan.stage", skip_all, fields(fingerprint = self.fingerprint.as_deref().unwrap_or("")))]
    async fn execute_inner(
        &self,
        ops: &mut dyn super::backend::TransactionOps,
        mut limits: AnswerLimits,
        count: bool,
        validation: Option<&QueryPlanV3>,
    ) -> Result<(QueryResult, super::backend::BoundedAnswerStats), OrmError> {
        use tracing::Instrument;
        let mut meter = (count || self.snapshot.is_some())
            .then(|| crate::answer_meter::AnswerMeter::new(&limits.answer));
        let snapshot = if let Some((typeql, plan)) = &self.snapshot {
            let answer = if count && let Some(rows) = &self.rows {
                collect_counted_given(
                    ops,
                    typeql,
                    rows.clone(),
                    limits.clone(),
                    meter.as_mut().expect("snapshot meter"),
                    true,
                    validation,
                )
                .await?
            } else {
                ops
                .query_materialized_bounded(
                    typeql,
                    self.rows.clone(),
                    limits.clone(),
                    self.canonical,
                    self.redacted_snapshot.as_deref(),
                )
                .instrument(tracing::debug_span!(target: "type_bridge.io", "type_bridge.io.provider", index = 0, writes = false, bytes_sent = typeql.len()))
                .await?
            };
            let meter = meter.as_mut().expect("snapshot meter");
            if !(count && self.rows.is_some())
                && let QueryResult::Rows(items) | QueryResult::Documents(items) = &answer
            {
                for item in items {
                    meter.accept_value(item)?;
                }
            }
            limits.answer.max_items = limits.answer.max_items.saturating_sub(meter.items());
            limits.answer.max_bytes = limits.answer.max_bytes.saturating_sub(meter.bytes());
            crate::query_plan_exec::validate_snapshot(
                plan.plan().map_err(plan_answer_error)?,
                &answer,
            )
            .map_err(plan_answer_error)?;
            Some(answer)
        } else {
            None
        };
        let streamed = count && self.rows.is_some();
        let answer = if let Some(rows) = self.rows.as_ref().filter(|_| streamed) {
            collect_counted_given(
                ops,
                &self.typeql,
                rows.clone(),
                limits.clone(),
                meter.as_mut().expect("counted meter"),
                validation.map_or(!self.writes, |plan| {
                    // A separated pre-delete capture keeps the deletion plan's
                    // trusted fetch shape, but runs as a read-only statement.
                    (!self.writes && plan.fetch_document_shape().is_some())
                        || matches!(
                            plan.output(),
                            type_bridge_contract::query_plan::QueryOutput::Documents { .. }
                        )
                        || matches!(
                            plan.pipeline().last(),
                            Some(type_bridge_contract::query_plan::v3::Stage::Fetch(_))
                        )
                }),
                validation,
            )
            .await?
        } else {
            ops
            .query_materialized_bounded(
                &self.typeql,
                self.rows.clone(),
                limits,
                self.canonical,
                self.redacted_typeql.as_deref(),
            )
            .instrument(tracing::debug_span!(target: "type_bridge.io", "type_bridge.io.provider", index = usize::from(self.snapshot.is_some()), writes = self.writes, bytes_sent = self.typeql.len()))
            .await?
        };
        if !streamed
            && let Some(meter) = &mut meter
            && let QueryResult::Rows(items) | QueryResult::Documents(items) = &answer
        {
            for item in items {
                meter.accept_value(item)?;
            }
        }
        let stats = meter.map_or_else(super::backend::BoundedAnswerStats::default, |meter| {
            super::backend::BoundedAnswerStats {
                processed_items: meter.items(),
                response_bytes: meter.bytes(),
                stopped_early: false,
            }
        });
        Ok((snapshot.unwrap_or(answer), stats))
    }

    /// Attach schema identities from the lowered plan, never provider prose.
    pub fn with_failure_context(mut self, context: FailureContext) -> Self {
        self.failure_context = context;
        self
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
    /// The plan cannot lower to one provider query; nothing was staged.
    #[error("Plan cannot be staged: {}", .0.message())]
    InvalidPlan(Box<Diagnostic>),
    /// The transaction's explicit kind refuses this staged work, such as a
    /// typed read or plan in a `Schema` transaction; nothing was staged.
    #[error("Not allowed in this transaction kind: {}", .0.message())]
    NotAllowedInKind(Box<Diagnostic>),
    /// Every pending answer observes the same failed execution.
    #[error("Transaction execution failed{index}: {diagnostic}", index = format_op_index(*op_index), diagnostic = .diagnostic.message())]
    Failed {
        /// Canonical execution diagnostic retained for every pending handle.
        diagnostic: Box<SdkExecutionDiagnostic>,
        /// Failing statement index, or none for opening/commit.
        op_index: Option<usize>,
        /// Value-free error description.
        message: String,
        /// Structural provider failure with the staged operation index.
        failure: Option<Box<Failure>>,
        /// Schema identities from the staged item.
        context: Box<FailureContext>,
    },
    /// A proxy commit the server refused under its own stable code;
    /// nothing it staged applied.
    #[error("Transaction execution failed{index}: the server refused the staged transaction ({code})", index = format_op_index(*op_index), code = .diagnostic.code())]
    Refused {
        /// Failing statement index, or none for the whole request.
        op_index: Option<usize>,
        /// The server's refusal, under its stable code.
        diagnostic: Box<SdkExecutionDiagnostic>,
    },
}

async fn collect_counted_given(
    ops: &mut dyn super::backend::TransactionOps,
    typeql: &str,
    rows: GivenRowsSpec,
    limits: AnswerLimits,
    meter: &mut crate::answer_meter::AnswerMeter,
    documents: bool,
    validation: Option<&QueryPlanV3>,
) -> Result<QueryResult, OrmError> {
    use super::backend::{AnswerControl, AnswerItem};
    let mut answer = if documents {
        QueryResult::Documents(Vec::new())
    } else {
        QueryResult::Rows(Vec::new())
    };
    let mut deferred = None;
    let mut consumer = |item: AnswerItem| {
        meter.accept(&item)?;
        if let (Some(plan), AnswerItem::Document(document)) = (validation, &item)
            && let Err(error) = crate::query_plan_exec::remote_document(document, plan)
        {
            deferred.get_or_insert_with(|| crate::query_plan_exec::plan_diagnostic_error(error));
        }
        if deferred.is_some() {
            return Ok(AnswerControl::Continue);
        }
        match (&mut answer, item) {
            (QueryResult::Documents(values), AnswerItem::Document(value))
            | (QueryResult::Rows(values), AnswerItem::Row(value)) => values.push(value),
            _ => {
                return Err(crate::match_request::MatchError::new(
                    crate::match_request::MatchErrorCategory::ResultDecode,
                    "query_plan_batch_answer",
                    "Returning deletion received the wrong provider answer kind",
                )
                .into());
            }
        }
        Ok(AnswerControl::Continue)
    };
    let result = ops
        .query_v2_with_rows_bounded(typeql, rows, limits, &mut consumer)
        .await;
    if let Some(error) = deferred {
        return Err(error);
    }
    result?;
    Ok(answer)
}

fn format_op_index(index: Option<usize>) -> String {
    index
        .map(|index| format!(" at operation {index}"))
        .unwrap_or_default()
}

/// Commit failure retaining the released ORM/provider error without reclassification.
#[non_exhaustive]
pub struct CommitError {
    /// Failing query index, or none for opening/commit.
    pub op_index: Option<usize>,
    /// Original failure, including existing commit certainty.
    pub error: Box<ClassifiedCommitError>,
    /// Structural classification with the staged operation index.
    pub failure: Option<Box<Failure>>,
    /// Schema identities from the executing staged item.
    pub context: Box<FailureContext>,
    /// The signed server refusal of a proxy commit, kept under its own code.
    remote: Option<Box<SdkExecutionDiagnostic>>,
    source_error: Option<Box<crate::QueryTargetError>>,
}
impl std::fmt::Debug for CommitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CommitError")
            .field("op_index", &self.op_index)
            .field("error", &self.error)
            .field("failure", &self.failure)
            .field("context", &self.context)
            .field("remote", &self.remote)
            .field(
                "source_error",
                &self.source_error.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}
impl std::fmt::Display for CommitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(formatter)
    }
}
impl std::error::Error for CommitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(match &self.source_error {
            Some(error) => error.as_ref(),
            None => self.error.as_ref(),
        })
    }
}
impl CommitError {
    /// Attach a binding's original materialization error while retaining the
    /// classified commit outcome and diagnostic.
    /// The source occupies the caller-owned callback slot in the error's source chain.
    #[must_use]
    pub fn with_binding_source(mut self, source: Box<dyn std::error::Error + Send + Sync>) -> Self {
        self.source_error = Some(Box::new(crate::QueryTargetError::Callback(source)));
        self
    }

    pub(crate) fn map_remote_diagnostic(
        mut self,
        map: impl FnOnce(&SdkExecutionDiagnostic) -> Option<SdkExecutionDiagnostic>,
    ) -> Self {
        if let Some(remote) = &self.remote
            && let Some(mapped) = map(remote)
        {
            let mut mapped = mapped;
            for (name, value) in remote.details() {
                if !mapped.details().contains_key(name) {
                    let Ok(next) = mapped.try_with_detail(name.clone(), value.clone()) else {
                        return self;
                    };
                    mapped = next;
                }
            }
            if let Some(failure) = &self.failure {
                mapped = mapped.with_failure(failure, self.context.as_ref().clone());
            }
            self.remote = Some(Box::new(mapped));
        }
        self
    }

    fn new(error: ClassifiedCommitError, op_index: Option<usize>, context: FailureContext) -> Self {
        let failure = error.failure().map(|failure| {
            match op_index.and_then(|index| u32::try_from(index).ok()) {
                Some(index) => failure.with_op_index(index),
                None => failure,
            }
        });
        Self {
            op_index,
            error: Box::new(error),
            failure: failure.map(Box::new),
            context: Box::new(context),
            remote: None,
            source_error: None,
        }
    }
    pub(crate) fn target_before_dispatch(error: crate::QueryTargetError) -> Self {
        match error {
            error @ crate::QueryTargetError::Callback(_) => {
                let mut commit = Self::new(
                    OrmError::ExecutionDiagnostic(Box::new(
                        SdkExecutionDiagnostic::provider_failure(SdkProviderOperation::Connect),
                    ))
                    .into(),
                    None,
                    Default::default(),
                );
                commit.source_error = Some(Box::new(error));
                commit
            }
            crate::QueryTargetError::Direct(error) => {
                Self::new(error.into(), None, Default::default())
            }
            error => Self::new(
                OrmError::ExecutionDiagnostic(Box::new(
                    error.into_diagnostic(SdkProviderOperation::Connect),
                ))
                .into(),
                None,
                Default::default(),
            ),
        }
    }
    pub(crate) fn staging(error: PendingError) -> Self {
        let diagnostic = match &error {
            PendingError::InvalidPlan(diagnostic) => {
                Some(crate::execution_diagnostic::lower_remote_query_diagnostic(
                    diagnostic.as_ref().clone(),
                ))
            }
            _ => error.diagnostic().cloned(),
        };
        match diagnostic {
            Some(diagnostic) => Self::new(
                OrmError::ExecutionDiagnostic(Box::new(diagnostic)).into(),
                None,
                Default::default(),
            ),
            None => Self::lifecycle(error),
        }
    }
    fn unknown(message: &str) -> Self {
        let mut error = Self::new(
            ClassifiedCommitError::Driver {
                certainty: crate::error::CommitFailureCertainty::Unknown,
                message: message.to_owned(),
            },
            None,
            Default::default(),
        );
        error.failure = Some(Box::new(Failure::Indeterminate { code: None }));
        error
    }
    pub(crate) fn proxy_projection_failure(diagnostic: SdkExecutionDiagnostic) -> Self {
        let mut error = Self::unknown("proxy model projection failed after commit");
        error.source_error = Some(Box::new(crate::QueryTargetError::Interrupted(diagnostic)));
        error
    }
    fn lifecycle(error: PendingError) -> Self {
        Self::new(
            OrmError::Transaction(error.to_string()).into(),
            None,
            Default::default(),
        )
    }
    fn pending(&self) -> PendingError {
        if let Some(remote) = &self.remote {
            return PendingError::Refused {
                op_index: self.op_index,
                diagnostic: remote.clone(),
            };
        }
        PendingError::Failed {
            diagnostic: Box::new(self.diagnostic()),
            op_index: self.op_index,
            message: self.to_string(),
            failure: self.failure.clone(),
            context: self.context.clone(),
        }
    }
    /// Canonical diagnostic retaining structural staged index and schema context.
    #[must_use]
    pub fn diagnostic(&self) -> SdkExecutionDiagnostic {
        if let Some(remote) = &self.remote {
            return remote.as_ref().clone();
        }
        match &self.failure {
            Some(failure) => {
                SdkExecutionDiagnostic::from_failure(failure, self.context.as_ref().clone())
            }
            None => match self.error.as_ref() {
                ClassifiedCommitError::Orm(error) => {
                    crate::execution_diagnostic::lower_orm_error(error, SdkProviderOperation::Write)
                }
                error => crate::execution_diagnostic::lower_commit_error(error),
            },
        }
    }
}
impl PendingError {
    /// Canonical diagnostic for a resolved execution failure.
    #[must_use]
    pub fn diagnostic(&self) -> Option<&SdkExecutionDiagnostic> {
        match self {
            Self::Failed { diagnostic, .. } | Self::Refused { diagnostic, .. } => Some(diagnostic),
            _ => None,
        }
    }
    /// Structural failure observed by all pending answers in the failed stack.
    #[must_use]
    pub fn failure(&self) -> Option<&Failure> {
        match self {
            Self::Failed { failure, .. } => failure.as_deref(),
            Self::Refused { diagnostic, .. } => diagnostic.failure(),
            _ => None,
        }
    }
}

struct PendingState<T> {
    answer: Mutex<Result<Arc<T>, PendingError>>,
    correlation:
        std::sync::OnceLock<Option<Arc<type_bridge_contract::query_plan::v3::BulkCorrelation>>>,
}
impl<T> PendingState<T> {
    fn new() -> Self {
        Self {
            answer: Mutex::new(Err(PendingError::NotCommitted)),
            correlation: std::sync::OnceLock::new(),
        }
    }
    fn lock(
        &self,
    ) -> std::sync::LockResult<std::sync::MutexGuard<'_, Result<Arc<T>, PendingError>>> {
        self.answer.lock()
    }
}
type AnswerState<T> = Arc<PendingState<T>>;
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
    /// Retained correlation proof from this answer's existing validation pass.
    pub fn correlation(
        &self,
    ) -> Result<Option<&type_bridge_contract::query_plan::v3::BulkCorrelation>, PendingError> {
        self.get()?;
        Ok(self
            .state
            .correlation
            .get()
            .and_then(|proof| proof.as_deref()))
    }
    /// Read the answer, or its current lifecycle/execution failure.
    pub fn get(&self) -> Result<Arc<T>, PendingError> {
        self.state.lock().unwrap().clone()
    }
}
fn close_if_pending<T>(state: &AnswerState<T>) {
    let mut state = state.lock().unwrap();
    if matches!(*state, Err(PendingError::NotCommitted)) {
        *state = Err(PendingError::Closed);
    }
}

/// A crate-private read program run on the commit's provider transaction.
///
/// A program owns what it reads, so its later statements may consume its
/// earlier answers: a paged read's rematch consumes the selection's roots,
/// which fixed staged queries cannot express. It receives the commit's
/// bounded answer limits (row/byte caps and the commit deadline) and must
/// read under them, as fixed staged queries do.
#[cfg(test)]
pub(crate) type Program<T> = Box<
    dyn for<'t> FnOnce(
            &'t mut OwnedTransaction,
            BoundedAnswerLimits,
        ) -> BoxFuture<'t, Result<T, OrmError>>
        + Send,
>;

/// A program with its answer type erased, so one stack holds every kind.
/// `Sync` keeps [`Transaction`] shareable as it was before programs.
trait StagedProgram: Send + Sync {
    /// Run once and keep the answer until the commit succeeds.
    fn run<'t>(
        &'t mut self,
        transaction: &'t mut OwnedTransaction,
        limits: BoundedAnswerLimits,
    ) -> BoxFuture<'t, Result<(), OrmError>>;
    fn remote_limits(&self) -> Option<&MatchExecutionLimits> {
        None
    }
    fn prepare_remote(
        &mut self,
        _schema: &type_bridge_contract::schema::DeclaredSchema,
        _bounds: &BoundedAnswerLimits,
    ) -> Result<(Vec<StagedPlanV3>, crate::QueryExecutionResourceLimits), OrmError> {
        Err(OrmError::Transaction(
            "this staged program cannot run through a proxy".into(),
        ))
    }
    fn resolve_remote(
        &mut self,
        _plans: &mut dyn Iterator<Item = StagedPlanV3>,
        _answers: &[type_bridge_contract::query_remote_v3::StagedAnswerV3],
    ) -> Result<(), OrmError> {
        Err(OrmError::Transaction(
            "this staged program cannot resolve proxy answers".into(),
        ))
    }
    fn publish(&mut self);
    fn fail(&self, error: PendingError);
    fn close_if_pending(&self);
}
#[cfg(test)]
struct ProgramSlot<T> {
    state: AnswerState<T>,
    // Only commit's exclusive borrow takes it; the mutex supplies `Sync`.
    program: Mutex<Option<Program<T>>>,
    answer: Option<T>,
}
#[cfg(test)]
impl<T: Send + Sync + 'static> StagedProgram for ProgramSlot<T> {
    fn run<'t>(
        &'t mut self,
        transaction: &'t mut OwnedTransaction,
        limits: BoundedAnswerLimits,
    ) -> BoxFuture<'t, Result<(), OrmError>> {
        let program = self.program.get_mut().unwrap().take();
        Box::pin(async move {
            let program = program
                .ok_or_else(|| OrmError::Transaction("Staged program already ran".into()))?;
            self.answer = Some(program(transaction, limits).await?);
            Ok(())
        })
    }
    fn publish(&mut self) {
        if let Some(answer) = self.answer.take() {
            *self.state.lock().unwrap() = Ok(Arc::new(answer));
        }
    }
    fn fail(&self, error: PendingError) {
        *self.state.lock().unwrap() = Err(error);
    }
    fn close_if_pending(&self) {
        close_if_pending(&self.state);
    }
}

struct MatchProgram {
    state: AnswerState<ValidatedMatchResult>,
    input: Option<(
        crate::error::Result<DescriptorRegistry>,
        Arc<ValidatedMatchRequest>,
    )>,
    remote: Option<(
        DescriptorRegistry,
        Arc<ValidatedMatchRequest>,
        crate::match_request::plan_adapter::AdaptedMatchRequest,
        crate::QueryExecutionResourceLimits,
    )>,
    limits: MatchExecutionLimits,
    capabilities: crate::match_request::CapabilitySet,
    supports_given: bool,
    answer: Option<ValidatedMatchResult>,
}
impl StagedProgram for MatchProgram {
    fn remote_limits(&self) -> Option<&MatchExecutionLimits> {
        Some(&self.limits)
    }
    fn run<'t>(
        &'t mut self,
        transaction: &'t mut OwnedTransaction,
        bounds: BoundedAnswerLimits,
    ) -> BoxFuture<'t, Result<(), OrmError>> {
        let input = self.input.take();
        Box::pin(async move {
            let (registry, request) = input
                .ok_or_else(|| OrmError::Transaction("staged model program already ran".into()))?;
            let registry = registry?;
            let limits = self.limits.clone().bounded_by(&bounds);
            self.answer = Some(
                SelectedResultExecutor::new(&registry, self.capabilities.clone(), limits)
                    .execute_compatible_staged(transaction, &request, self.supports_given)
                    .await?,
            );
            Ok(())
        })
    }
    fn prepare_remote(
        &mut self,
        schema: &type_bridge_contract::schema::DeclaredSchema,
        _bounds: &BoundedAnswerLimits,
    ) -> Result<(Vec<StagedPlanV3>, crate::QueryExecutionResourceLimits), OrmError> {
        if let Some(deadline) = self.limits.deadline() {
            self.limits.capture_deadline(deadline);
        }
        let (registry, request) = self
            .input
            .take()
            .ok_or_else(|| OrmError::Transaction("staged model program already ran".into()))?;
        let registry = registry?;
        let executor =
            SelectedResultExecutor::new(&registry, self.capabilities.clone(), self.limits.clone());
        let mut stack = executor.adapt_owned(&request, self.supports_given)?;
        let policy = self.limits.policy();
        let plans = stack
            .plans
            .iter()
            .map(|plan| {
                StagedPlanV3::new(plan, plan.given_rows().unwrap_or_default().to_vec(), schema)
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(plan_answer_error)?;
        stack.plans.clear();
        self.remote = Some((registry, request, stack, policy));
        Ok((plans, policy))
    }
    fn resolve_remote(
        &mut self,
        plans: &mut dyn Iterator<Item = StagedPlanV3>,
        answers: &[type_bridge_contract::query_remote_v3::StagedAnswerV3],
    ) -> Result<(), OrmError> {
        let (registry, request, stack, policy) = self
            .remote
            .as_mut()
            .ok_or_else(|| OrmError::Transaction("staged model program was not prepared".into()))?;
        SelectedResultExecutor::new(registry, self.capabilities.clone(), self.limits.clone())
            .check_lifecycle_budget()?;
        stack
            .plans
            .extend(plans.take(answers.len()).map(StagedPlanV3::into_plan));
        let answers = answers
            .iter()
            .zip(&stack.plans)
            .map(|(answer, plan)| PlanAnswer::from_staged_v3(answer, plan))
            .collect::<Result<Vec<_>, _>>()
            .map_err(plan_answer_error)?;
        self.answer = Some(
            crate::match_request::result_validation::validated_match_result_from_plan_stack(
                registry,
                request,
                stack,
                &answers,
                policy.for_adapter_statements(stack.plans.len()),
            )?,
        );
        Ok(())
    }
    fn publish(&mut self) {
        if let Some(answer) = self.answer.take() {
            *self.state.lock().unwrap() = Ok(Arc::new(answer));
        }
    }
    fn fail(&self, error: PendingError) {
        *self.state.lock().unwrap() = Err(error);
    }
    fn close_if_pending(&self) {
        close_if_pending(&self.state);
    }
}

pub(crate) trait SharedPlanSource: Send + Sync {
    fn plans(&self) -> &[QueryPlanV3];
}
impl<T: AsRef<[QueryPlanV3]> + Send + Sync> SharedPlanSource for T {
    fn plans(&self) -> &[QueryPlanV3] {
        self.as_ref()
    }
}
#[derive(Clone)]
pub(crate) enum PlanSource {
    Owned(Arc<QueryPlanV3>),
    Shared {
        source: Arc<dyn SharedPlanSource>,
        index: usize,
        identity: usize,
    },
}
impl std::fmt::Debug for PlanSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CanonicalPlanSource([REDACTED])")
    }
}
impl PlanSource {
    pub(crate) fn plan(&self) -> Result<&QueryPlanV3, Diagnostic> {
        match self {
            Self::Owned(plan) => Ok(plan),
            Self::Shared {
                source,
                index,
                identity,
            } => source
                .plans()
                .get(*index)
                .filter(|plan| std::ptr::from_ref(*plan) as usize == *identity)
                .ok_or_else(|| proxy_refusal("shared canonical plan source changed after staging")),
        }
    }
}

/// Where one staged item's answer goes, and how it resolves.
enum Slot {
    Query(AnswerState<QueryResult>),
    Plan {
        state: AnswerState<PlanAnswer>,
        plan: PlanSource,
    },
    Program(Box<dyn StagedProgram>),
}
/// An answer resolved during commit, published only after commit succeeds.
enum Resolved {
    Query(QueryResult),
    Plan {
        answer: Arc<PlanAnswer>,
        wire: Arc<type_bridge_contract::query_remote_v3::StagedAnswerV3>,
        correlation: Option<Arc<type_bridge_contract::query_plan::v3::BulkCorrelation>>,
    },
    /// The program holds its own answer.
    Program,
}
impl Slot {
    fn fail(&self, error: PendingError) {
        match self {
            Self::Query(state) => *state.lock().unwrap() = Err(error),
            Self::Plan { state, .. } => *state.lock().unwrap() = Err(error),
            Self::Program(program) => program.fail(error),
        }
    }
    fn close_if_pending(&self) {
        match self {
            Self::Query(state) => close_if_pending(state),
            Self::Plan { state, .. } => close_if_pending(state),
            Self::Program(program) => program.close_if_pending(),
        }
    }
    fn resolve(&self, answer: QueryResult) -> Result<Resolved, Diagnostic> {
        match self {
            Self::Query(_) => Ok(Resolved::Query(answer)),
            Self::Plan { plan, .. } => {
                let plan = plan.plan()?;
                let span = tracing::debug_span!(target: "type_bridge.plan", "type_bridge.plan.answer", fingerprint = tracing::field::Empty);
                if !span.is_disabled() {
                    span.record("fingerprint", plan.shape_fingerprint()?.digest().to_hex());
                }
                let _span = span.entered();
                let result = crate::query_plan_exec::resolve_plan_with_wire(plan, answer);
                tracing::debug!(target: "type_bridge.plan", outcome = if result.is_ok() { "ok" } else { "error" }, "plan answer resolved");
                result.map(|resolved| Resolved::Plan {
                    answer: Arc::new(resolved.answer),
                    wire: Arc::new(resolved.wire),
                    correlation: resolved.correlation,
                })
            }
            Self::Program(_) => unreachable!("programs resolve their own answers"),
        }
    }
    fn publish(&mut self, answer: Resolved) {
        match (self, answer) {
            (Self::Query(state), Resolved::Query(answer)) => {
                *state.lock().unwrap() = Ok(Arc::new(answer));
            }
            (
                Self::Plan { state, .. },
                Resolved::Plan {
                    answer,
                    correlation,
                    ..
                },
            ) => {
                let _ = state.correlation.set(correlation);
                *state.lock().unwrap() = Ok(answer);
            }
            (Self::Program(program), Resolved::Program) => program.publish(),
            _ => unreachable!("answers resolve through their own slot"),
        }
    }
}
/// One staged unit: a fixed query, or a program (no query) run on the
/// provider transaction itself.
struct Item {
    query: Option<StagedQuery>,
    /// The validated plan a proxy commit sends instead of `query`.
    remote: Option<StagedPlanV3>,
    deferred: Option<Box<DeferredPlan>>,
    slot: Slot,
}
struct DeferredPlan {
    schema: Arc<type_bridge_contract::schema::DeclaredSchema>,
    source_base: usize,
    local_index: usize,
}
impl Item {
    fn writes(&self) -> bool {
        self.query.as_ref().is_some_and(|query| query.writes)
            || self
                .remote
                .as_ref()
                .is_some_and(|entry| entry.plan().writes())
    }
    fn bind_source(&mut self, answers: &[Resolved], supports_given: bool) -> Result<(), OrmError> {
        let Some(deferred) = &self.deferred else {
            return Ok(());
        };
        let entry = self.remote.as_ref().expect("deferred canonical entry");
        let index = deferred.source_base + entry.result_refs()[0].source_plan as usize;
        let Some(Resolved::Plan { wire, .. }) = answers.get(index) else {
            return Err(result_ref_error(proxy_refusal(
                "result reference source is not a preceding plan answer",
            )));
        };
        let bound = entry
            .bind_source_result(wire, &deferred.schema)
            .map_err(result_ref_error)?;
        let source = PlanSource::Owned(Arc::new(bound));
        let query = if supports_given {
            crate::query_plan_exec::staged_query_with_source(&source, true)
        } else {
            crate::query_plan_exec::staged_literal_query(source.plan().map_err(result_ref_error)?)
        }
        .map_err(result_ref_error)?;
        let Slot::Plan { plan, .. } = &mut self.slot else {
            unreachable!("deferred item is a plan")
        };
        *plan = source;
        self.query = Some(query);
        Ok(())
    }
    fn failure_context(&self) -> FailureContext {
        self.query
            .as_ref()
            .map(|query| query.failure_context.clone())
            .unwrap_or_default()
    }
}
trait FetchGroupSource: SharedPlanSource {
    fn fetch(&self) -> &crate::match_request::PlanFetchStack;
}
impl<T> FetchGroupSource for T
where
    T: AsRef<[QueryPlanV3]> + AsRef<crate::match_request::PlanFetchStack> + Send + Sync,
{
    fn fetch(&self) -> &crate::match_request::PlanFetchStack {
        self.as_ref()
    }
}
struct FetchGroup {
    source: Arc<dyn FetchGroupSource>,
    identity: usize,
    registry: Arc<DescriptorRegistry>,
    range: std::ops::Range<usize>,
    limits: crate::QueryExecutionResourceLimits,
    state: AnswerState<crate::match_request::OwnedPlanFetchResult>,
    answer: Option<Arc<crate::match_request::OwnedPlanFetchResult>>,
}
impl FetchGroup {
    fn stack(&self) -> Result<&crate::match_request::PlanFetchStack, Diagnostic> {
        let stack = self.source.fetch();
        if std::ptr::from_ref(stack) as usize != self.identity
            || !std::ptr::eq(stack.plans(), self.source.plans())
        {
            return Err(proxy_refusal(
                "shared model fetch source changed after staging",
            ));
        }
        Ok(stack)
    }
}
struct Execution(Vec<Item>, Vec<FetchGroup>);
impl Execution {
    fn fail(&self, error: PendingError) {
        for item in &self.0 {
            item.slot.fail(error.clone());
        }
        for group in &self.1 {
            *group.state.lock().unwrap() = Err(error.clone());
        }
    }
    fn resolve_groups(&mut self, answers: &[Resolved]) -> Result<(), (usize, OrmError)> {
        for group in &mut self.1 {
            let evidence = answers[group.range.clone()]
                .iter()
                .map(|answer| {
                    let Resolved::Plan {
                        answer,
                        wire,
                        correlation,
                    } = answer
                    else {
                        unreachable!("fetch groups stage only canonical plans")
                    };
                    (answer, wire, correlation)
                })
                .collect::<Vec<_>>();
            let stack = group
                .stack()
                .map_err(|error| (group.range.start, plan_answer_error(error)))?;
            let proof = stack
                .resolve_group(
                    &group.registry,
                    &evidence
                        .iter()
                        .map(|(answer, _, _)| Arc::clone(answer))
                        .collect::<Vec<_>>(),
                    &evidence
                        .iter()
                        .map(|(_, wire, _)| Arc::clone(wire))
                        .collect::<Vec<_>>(),
                    &evidence
                        .iter()
                        .map(|(_, _, proof)| proof.as_deref())
                        .collect::<Vec<_>>(),
                    stack.execution_limits(group.limits),
                )
                .map_err(|error| (group.range.start, OrmError::from(error)))?;
            group.answer = Some(Arc::new(proof));
        }
        Ok(())
    }
    fn publish_groups(&mut self) {
        for group in &mut self.1 {
            if let Some(answer) = group.answer.take() {
                *group.state.lock().unwrap() = Ok(answer);
            }
        }
    }
}
impl Drop for Execution {
    fn drop(&mut self) {
        // Also resolves handles when a commit future is cancelled.
        for item in &self.0 {
            item.slot.close_if_pending();
        }
        for group in &self.1 {
            close_if_pending(&group.state);
        }
    }
}

/// Keep a malformed plan answer's structured code in the commit error.
pub(crate) fn plan_answer_error(diagnostic: Diagnostic) -> OrmError {
    MatchError::new(
        MatchErrorCategory::ResultDecode,
        diagnostic.code().as_str(),
        diagnostic.message(),
    )
    .at(MatchErrorPathSegment::Result)
    .into()
}

fn result_ref_error(diagnostic: Diagnostic) -> OrmError {
    crate::match_request::selected_result_executor::adapter_match_error(diagnostic).into()
}

/// Race a multi-statement read against the commit's cancellation and deadline.
pub(crate) async fn bounded<T>(
    limits: &BoundedAnswerLimits,
    future: impl std::future::Future<Output = Result<T, OrmError>>,
) -> Result<T, OrmError> {
    let cancelled = || {
        super::backend::cancelled_error(
            "provider_cancelled",
            "provider answer processing was cancelled",
        )
    };
    let expired = || {
        super::backend::resource_error(
            "transaction_deadline_exceeded",
            "provider transaction deadline expired",
        )
    };
    super::backend::BoundedAnswerReader::new(limits.clone()).check_before_read()?;
    let deadline = async {
        match limits.deadline {
            Some(deadline) => {
                tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
            }
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        biased;
        result = future => result,
        () = limits.cancellation.cancelled() => Err(cancelled()),
        () = deadline => Err(expired()),
    }
}

/// Refusal of work the proxy route cannot carry.
pub(crate) fn proxy_refusal(message: &'static str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::UnsupportedCapability,
        DiagnosticCode::new("staged_proxy_unsupported")
            .expect("the static proxy code is canonical"),
        message,
    )
}

/// Type a signed staged failure like the direct commit would: the released
/// commit codes keep their certainty and the provider code (`STC2`) its
/// typed failure; any other code stays a transaction failure under its code.
fn constrain_match_policy(
    current: &mut Option<crate::QueryExecutionResourceLimits>,
    origins: &mut std::collections::BTreeMap<&'static str, (&'static str, Option<usize>)>,
    candidate: crate::QueryExecutionResourceLimits,
    scope: &'static str,
    program: Option<usize>,
) {
    let previous = *current;
    let next = previous.map_or(candidate, |policy| policy.constrained_by(candidate));
    for field in type_bridge_contract::limits::ANSWER_LIMIT_FIELDS {
        let changed = previous.is_none_or(|policy| {
            if field.name == "statements" {
                policy.statements != next.statements
            } else {
                policy.get(field.name) != next.get(field.name)
            }
        });
        if changed {
            origins.insert(field.name, (scope, program));
        }
    }
    *current = Some(next);
}

fn annotate_match_limit(
    error: &mut CommitError,
    code: &str,
    policy: crate::QueryExecutionResourceLimits,
    origins: &std::collections::BTreeMap<&'static str, (&'static str, Option<usize>)>,
) {
    use type_bridge_contract::sdk_diagnostic::{
        SdkDiagnosticDetailValue, SdkDiagnosticName, SdkQueryDiagnosticIdentity,
    };
    let Some((limit, maximum, source)) =
        policy.refusal_details(crate::query_execution_limits::AnswerTransport::Proxy, code)
    else {
        return;
    };
    let Some((scope, program)) = origins.get(limit) else {
        return;
    };
    let mut diagnostic = error.diagnostic();
    for (name, value) in [
        (
            "limit",
            SdkDiagnosticDetailValue::QueryIdentity(
                SdkQueryDiagnosticIdentity::new(limit).expect("static limit"),
            ),
        ),
        ("maximum", SdkDiagnosticDetailValue::Count(maximum)),
        (
            "source",
            SdkDiagnosticDetailValue::QueryIdentity(
                SdkQueryDiagnosticIdentity::new(source).expect("static source"),
            ),
        ),
        (
            "limit_scope",
            SdkDiagnosticDetailValue::QueryIdentity(
                SdkQueryDiagnosticIdentity::new(*scope).expect("static scope"),
            ),
        ),
    ] {
        if let Ok(next) = diagnostic
            .clone()
            .try_with_detail(SdkDiagnosticName::new(name).expect("static detail"), value)
        {
            diagnostic = next;
        }
    }
    if let Some(index) = program
        && let Ok(next) = diagnostic.clone().try_with_detail(
            SdkDiagnosticName::new("program_index").expect("static detail"),
            SdkDiagnosticDetailValue::Count(*index as u64),
        )
    {
        diagnostic = next;
    }
    error.remote = Some(Box::new(diagnostic));
}

pub(crate) fn proxy_failure(projection: &SdkDiagnosticProjectionV3) -> CommitError {
    let json = projection.host_json().unwrap_or_default();
    let field = |key: &str| json.get(key).and_then(serde_json::Value::as_str);
    // The server signs `commit_outcome_unknown` for anything that may have
    // committed, so every other signed failure applied nothing.
    let unknown = projection.code() == "commit_outcome_unknown";
    let definite = !unknown;
    let error = ClassifiedCommitError::Driver {
        certainty: if definite {
            crate::error::CommitFailureCertainty::DefinitelyAborted
        } else {
            crate::error::CommitFailureCertainty::Unknown
        },
        // Driver errors are public through CommitError::Display, too.
        message: match projection.code() {
            "commit_outcome_unknown" => "The server could not establish the staged commit outcome",
            "commit_definitely_aborted" => "The server aborted the staged transaction",
            _ => "The server refused the staged transaction",
        }
        .to_owned(),
    };
    let op_index = json
        .get("path")
        .and_then(serde_json::Value::as_array)
        .and_then(|path| path.iter().find_map(serde_json::Value::as_u64))
        .and_then(|index| usize::try_from(index).ok());
    let mut commit = CommitError::new(error, op_index, Default::default());
    if !unknown && projection.code() != "commit_definitely_aborted" {
        commit.remote = remote_refusal(projection.code(), field("sdk_category")).map(Box::new);
    }
    // Retain the server's clock and phase facts instead of reconstructing a
    // client-side timeline when the authenticated failure crosses the proxy.
    if let Some(remote) = commit.remote.as_mut()
        && let Ok(typed) = projection.to_query_diagnostic()
    {
        for (name, value) in typed.details() {
            if remote.details().contains_key(name) {
                continue;
            }
            **remote = remote
                .as_ref()
                .clone()
                .try_with_detail(name.clone(), value.clone())
                .expect("verified server details fit the diagnostic budget");
        }
    }
    // The signed details carry the failure the server classified, so a
    // refusal is typed exactly as the direct commit would be; the refusal
    // keeps its own code and category.
    if let Some(failure) = projection.failure() {
        if let Some(remote) = commit.remote.take() {
            commit.remote = Some(Box::new(remote.with_failure(&failure, Default::default())));
        }
        commit.failure = Some(Box::new(failure));
    }
    commit
}

/// Keep a server refusal's stable code in the SDK diagnostic. Server text
/// stays out: SDK messages are implementation-owned static text.
fn remote_refusal(code: &str, sdk_category: Option<&str>) -> Option<SdkExecutionDiagnostic> {
    use type_bridge_contract::sdk_diagnostic::{SdkDiagnosticCode, SdkDiagnosticMessage};
    let code = SdkDiagnosticCode::new(code).ok()?;
    let message = SdkDiagnosticMessage::new("The server refused the staged transaction").ok()?;
    Some(match sdk_category {
        Some("invalid_input") => SdkExecutionDiagnostic::invalid_input(code, message),
        Some("unsupported_capability") => {
            SdkExecutionDiagnostic::unsupported_capability(code, message)
        }
        Some("resource_limit") => SdkExecutionDiagnostic::resource_limit(code, message),
        Some("integrity") => SdkExecutionDiagnostic::integrity(code, message),
        Some("cancelled") => SdkExecutionDiagnostic::query_failure(
            type_bridge_contract::sdk_diagnostic::SdkQueryDiagnosticCategory::Cancelled,
            code,
        ),
        _ => SdkExecutionDiagnostic::transaction_failure(code, message),
    })
}

fn provider_consumed() -> OrmError {
    OrmError::Transaction("Staged program consumed the provider transaction".into())
}

/// A reusable logical transaction. Each commit is one atomic provider transaction.
/// Nothing opens until commit; drop discards staged work without committing.
pub struct Transaction {
    backend: Backend,
    database: String,
    options: TransactionOptions,
    stack: Execution,
    empty: Vec<(AnswerState<PlanAnswer>, Arc<PlanAnswer>)>,
    closed: bool,
}
enum Backend {
    Local(Arc<dyn DriverBackend>),
    Proxy(Arc<ProxyStaging>),
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
        options: TransactionOptions,
    ) -> Self {
        Self::with_backend(Backend::Local(backend), database, options)
    }
    /// A transaction whose commit is one signed `query-remote/v3` exchange.
    pub(crate) fn new_proxy(proxy: ProxyStaging, options: TransactionOptions) -> Self {
        Self::with_backend(Backend::Proxy(Arc::new(proxy)), String::new(), options)
    }
    /// A transaction over a caller-owned proxy exchange: `exchange` sends
    /// the exact request bytes to `/v3/staged` and returns the exact reply
    /// bytes; `staged_advertisement` is the `/v3/capabilities` body and
    /// `declared` the schema the plans validate under.
    ///
    /// Only [`Self::stage_plan`] stages on it. Each [`Self::commit`] sends
    /// one envelope with a fresh nonce; a commit whose reply is lost has an
    /// unknown outcome, and the caller checks the database.
    ///
    /// # Errors
    ///
    /// Returns the decode failure of the advertisement.
    pub fn over_proxy(
        declared: type_bridge_contract::schema::DeclaredSchema,
        staged_advertisement: &[u8],
        exchange: super::StagedExchangeFn,
        options: TransactionOptions,
    ) -> Result<Self, Diagnostic> {
        let limits = crate::query_execution_limits::QueryExecutionResourceLimits::default();
        let proxy =
            ProxyStaging::new(None, Some(exchange), staged_advertisement, declared, limits)?;
        Ok(Self::new_proxy(proxy, options))
    }
    fn with_backend(backend: Backend, database: String, mut options: TransactionOptions) -> Self {
        if let Some(deadline) = options.answer_limits.answer.deadline.take() {
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
            stack: Execution(Vec::new(), Vec::new()),
            empty: Vec::new(),
            closed: false,
        }
    }
    fn local(&self) -> Result<&Arc<dyn DriverBackend>, PendingError> {
        match &self.backend {
            Backend::Local(backend) => Ok(backend),
            Backend::Proxy(_) => Err(PendingError::InvalidPlan(Box::new(proxy_refusal(
                "only plans stage on a proxy transaction",
            )))),
        }
    }
    /// Queue one query without opening a provider transaction.
    ///
    /// A proxy transaction refuses raw queries: only plans travel.
    pub fn stage(&mut self, query: StagedQuery) -> Result<Pending<QueryResult>, PendingError> {
        self.admit(query.writes)?;
        self.local()?;
        let state = Arc::new(PendingState::new());
        self.stack.0.push(Item {
            query: Some(query),
            remote: None,
            deferred: None,
            slot: Slot::Query(Arc::clone(&state)),
        });
        Ok(Pending { state })
    }
    /// Queue one validated `query-plan/v3` program as one provider query.
    ///
    /// The plan lowers now, so an unrepresentable plan fails here and stages
    /// nothing. Its answer resolves at commit from the pipeline's own answer
    /// stream: `Matched` counts the rows a write produced, `Rows` carries
    /// the projection appended to the same pipeline. A plan that writes
    /// makes an inferred kind `Write`.
    ///
    /// A `Schema` transaction refuses plans; the schema transaction stays
    /// in eager mode.
    pub fn stage_plan(&mut self, plan: &QueryPlanV3) -> Result<Pending<PlanAnswer>, PendingError> {
        self.stage_plan_source(PlanSource::Owned(Arc::new(plan.clone())))
    }

    /// Retain one chunk of a shared canonical plan source without copying it.
    ///
    /// The source can be one plan or a generated plan batch implementing
    /// `AsRef<[QueryPlanV3]>`; erasing its type retains the same Arc allocation.
    pub fn stage_shared_plan<T>(
        &mut self,
        source: Arc<T>,
        index: usize,
    ) -> Result<Pending<PlanAnswer>, PendingError>
    where
        T: AsRef<[QueryPlanV3]> + Send + Sync + 'static,
    {
        let identity = source
            .as_ref()
            .as_ref()
            .get(index)
            .map(|plan| std::ptr::from_ref(plan) as usize)
            .ok_or_else(|| {
                PendingError::InvalidPlan(Box::new(proxy_refusal(
                    "shared plan chunk index is outside its canonical source",
                )))
            })?;
        self.stage_plan_source(PlanSource::Shared {
            source,
            index,
            identity,
        })
    }

    /// Stage a complete model projection on the existing canonical plan path.
    ///
    /// Direct and borrowed targets resolve the proof before provider commit;
    /// proof failure rolls back prior writes. A proxy resolves after its verified
    /// atomic reply; proof failure leaves every pending outcome unknown.
    /// The source retains its original Arc allocation and must expose the stack's
    /// exact plan slice through both AsRef implementations.
    pub fn stage_plan_fetch<T>(
        &mut self,
        source: Arc<T>,
        registry: Arc<DescriptorRegistry>,
        limits: crate::QueryExecutionResourceLimits,
    ) -> Result<Pending<crate::match_request::OwnedPlanFetchResult>, PendingError>
    where
        T: AsRef<[QueryPlanV3]>
            + AsRef<crate::match_request::PlanFetchStack>
            + Send
            + Sync
            + 'static,
    {
        let stack = <T as AsRef<crate::match_request::PlanFetchStack>>::as_ref(source.as_ref());
        let plans = <T as AsRef<[QueryPlanV3]>>::as_ref(source.as_ref());
        if !std::ptr::eq(plans, stack.plans()) {
            return Err(PendingError::InvalidPlan(Box::new(proxy_refusal(
                "fetch group must retain its canonical plan slice",
            ))));
        }
        self.admit(stack.writes())?;
        self.admit_typed()?;
        let execution_limits = stack.execution_limits(limits);
        let need = plans
            .iter()
            .map(|plan| {
                let (read, write) = plan.statement_counts();
                read + write
            })
            .sum::<usize>();
        if need > execution_limits.statement_limit() as usize {
            return Err(PendingError::InvalidPlan(Box::new(Diagnostic::new(
                DiagnosticCategory::ResourceLimit,
                DiagnosticCode::new("statement_count_limit").expect("static diagnostic code"),
                "model fetch group exceeds its statement ceiling",
            ))));
        }
        let identity = std::ptr::from_ref(stack) as usize;
        let start = self.stack.0.len();
        for index in 0..plans.len() {
            if let Err(error) = self.stage_shared_plan(Arc::clone(&source), index) {
                self.stack.0.truncate(start);
                return Err(error);
            }
        }
        let state = Arc::new(PendingState::new());
        self.stack.1.push(FetchGroup {
            source,
            identity,
            registry,
            range: start..self.stack.0.len(),
            limits,
            state: Arc::clone(&state),
            answer: None,
        });
        Ok(Pending { state })
    }

    fn stage_plan_source(
        &mut self,
        source: PlanSource,
    ) -> Result<Pending<PlanAnswer>, PendingError> {
        self.stage_plan_source_with_entry(source, None)
    }

    fn stage_plan_source_with_entry(
        &mut self,
        source: PlanSource,
        remote_entry: Option<StagedPlanV3>,
    ) -> Result<Pending<PlanAnswer>, PendingError> {
        let plan = source
            .plan()
            .map_err(|d| PendingError::InvalidPlan(Box::new(d)))?;
        let _performance = tracing::info_span!("tb.orm.plan").entered();
        let stage = tracing::debug_span!(target: "type_bridge.plan", "type_bridge.plan.stage", fingerprint = tracing::field::Empty, writes = plan.writes(), stages = plan.pipeline().len(), outcome = "refused");
        if !stage.is_disabled() {
            let fingerprint = plan
                .shape_fingerprint()
                .map_err(|d| PendingError::InvalidPlan(Box::new(d)))?
                .digest()
                .to_hex();
            stage.record("fingerprint", &fingerprint);
        }
        let _stage = stage.entered();
        self.admit(plan.writes())?;
        self.admit_typed()?;
        let invalid = |diagnostic| PendingError::InvalidPlan(Box::new(diagnostic));
        let (query, remote) = match &self.backend {
            Backend::Local(backend) => {
                crate::query_plan_exec::check_provider(plan, backend.server_version())
                    .map_err(invalid)?;
                (
                    Some(
                        if remote_entry.is_some()
                            && !super::database::backend_supports_given_stage(backend.as_ref())
                        {
                            crate::query_plan_exec::staged_literal_query(plan)
                        } else {
                            crate::query_plan_exec::staged_query_with_source(
                                &source,
                                super::database::backend_supports_given_stage(backend.as_ref()),
                            )
                        }
                        .map_err(invalid)?,
                    ),
                    None,
                )
            }
            Backend::Proxy(proxy) => {
                let entry = match remote_entry {
                    Some(entry) => entry,
                    None => StagedPlanV3::new(
                        plan,
                        plan.given_rows().unwrap_or_default().to_vec(),
                        proxy.declared(),
                    )
                    .map_err(invalid)?,
                };
                (None, Some(entry))
            }
        };
        let state = Arc::new(PendingState::new());
        self.stack.0.push(Item {
            query,
            remote,
            deferred: None,
            slot: Slot::Plan {
                state: Arc::clone(&state),
                plan: source,
            },
        });
        tracing::Span::current().record("outcome", "ok");
        Ok(Pending { state })
    }

    /// Queue an authored static stack, including block-local IID dependencies.
    ///
    /// All graph and provider admission checks finish before anything opens.
    /// Source indexes are relative to this block; proxy framing rebases them
    /// when earlier staged operations expand to several wire plans. Referenced
    /// values bind inside the transaction, and answers publish only on commit.
    pub fn stage_plan_stack(
        &mut self,
        entries: Vec<StagedPlanV3>,
        schema: Arc<type_bridge_contract::schema::DeclaredSchema>,
    ) -> Result<Vec<Pending<PlanAnswer>>, PendingError> {
        self.admit(false)?;
        self.admit_typed()?;
        type_bridge_contract::query_remote_v3::validate_staged_result_refs_v3(&entries, &schema)
            .map_err(|error| PendingError::InvalidPlan(Box::new(error)))?;
        let start = self.stack.0.len();
        let mut pending = Vec::with_capacity(entries.len());
        for (local_index, entry) in entries.into_iter().enumerate() {
            let staged = if entry.result_refs().is_empty() {
                entry
                    .bound_plan(&schema)
                    .map_err(|error| PendingError::InvalidPlan(Box::new(error)))
                    .and_then(|plan| {
                        self.stage_plan_source_with_entry(
                            PlanSource::Owned(Arc::new(plan)),
                            Some(entry),
                        )
                    })
            } else {
                self.stage_deferred_plan(entry, Arc::clone(&schema), start, local_index)
            };
            match staged {
                Ok(answer) => pending.push(answer),
                Err(error) => {
                    self.stack.0.truncate(start);
                    return Err(error);
                }
            }
        }
        Ok(pending)
    }

    fn stage_deferred_plan(
        &mut self,
        entry: StagedPlanV3,
        schema: Arc<type_bridge_contract::schema::DeclaredSchema>,
        source_base: usize,
        local_index: usize,
    ) -> Result<Pending<PlanAnswer>, PendingError> {
        self.admit(entry.plan().writes())?;
        self.admit_typed()?;
        if let Backend::Local(backend) = &self.backend {
            crate::query_plan_exec::check_provider(entry.plan(), backend.server_version())
                .map_err(|error| PendingError::InvalidPlan(Box::new(error)))?;
        }
        let state = Arc::new(PendingState::new());
        let plan = PlanSource::Owned(Arc::new(entry.plan().clone()));
        self.stack.0.push(Item {
            query: None,
            remote: Some(entry),
            deferred: Some(Box::new(DeferredPlan {
                schema,
                source_base,
                local_index,
            })),
            slot: Slot::Plan {
                state: Arc::clone(&state),
                plan,
            },
        });
        Ok(Pending { state })
    }

    /// Stage one authored call's plans atomically. An empty bulk uses a local
    /// result barrier, with no provider query or proxy request. The source Arc
    /// is retained directly; empty_documents selects the empty bulk answer kind.
    pub fn stage_plans<T>(
        &mut self,
        source: Arc<T>,
        empty_documents: bool,
    ) -> Result<Vec<Pending<PlanAnswer>>, PendingError>
    where
        T: AsRef<[QueryPlanV3]> + Send + Sync + 'static,
    {
        let count = source.as_ref().as_ref().len();
        if count == 0 {
            self.admit(true)?;
            self.admit_typed()?;
            let state = Arc::new(PendingState::new());
            self.empty.push((
                Arc::clone(&state),
                Arc::new(if empty_documents {
                    PlanAnswer::Documents(vec![])
                } else {
                    PlanAnswer::Matched(0)
                }),
            ));
            return Ok(vec![Pending { state }]);
        }
        let start = self.stack.0.len();
        let mut pending = Vec::with_capacity(count);
        for index in 0..count {
            match self.stage_shared_plan(Arc::clone(&source), index) {
                Ok(answer) => pending.push(answer),
                Err(error) => {
                    self.stack.0.truncate(start);
                    return Err(error);
                }
            }
        }
        Ok(pending)
    }
    /// Queue one validated selected-row read, run at commit on the commit's
    /// provider transaction.
    ///
    /// A `Write` transaction allows it: commit is the only executor, so
    /// no mutation can interleave a paged read's selection, total and
    /// rematch, and the answer resolves only after the commit succeeds.
    /// It sees the writes staged before it. A `Schema` transaction refuses
    /// it. Execution failures abort the commit and keep their typed
    /// [`OrmError`] in [`CommitError::error`].
    pub fn stage_match(
        &mut self,
        registry: &DescriptorRegistry,
        validated: ValidatedMatchRequest,
    ) -> Result<Pending<ValidatedMatchResult>, PendingError> {
        self.stage_match_with_limits(registry, validated, MatchExecutionLimits::default())
    }
    /// [`Self::stage_match`] with caller-tightened limits.
    pub fn stage_match_with_limits(
        &mut self,
        registry: &DescriptorRegistry,
        validated: ValidatedMatchRequest,
        limits: MatchExecutionLimits,
    ) -> Result<Pending<ValidatedMatchResult>, PendingError> {
        self.stage_shared_match_with_limits(registry, Arc::new(validated), limits)
    }

    /// Queue a typed read while sharing its immutable request proof with the
    /// result decoder. The request is not rebuilt or revalidated in a binding.
    pub fn stage_shared_match_with_limits(
        &mut self,
        registry: &DescriptorRegistry,
        validated: Arc<ValidatedMatchRequest>,
        limits: MatchExecutionLimits,
    ) -> Result<Pending<ValidatedMatchResult>, PendingError> {
        self.admit(false)?;
        self.admit_typed()?;
        let (capabilities, supports_given) = match &self.backend {
            Backend::Local(backend) => (
                backend.match_capabilities(),
                super::database::backend_supports_given_stage(backend.as_ref()),
            ),
            Backend::Proxy(_) => (
                crate::match_request::Capability::ALL.into_iter().collect(),
                true,
            ),
        };
        let state = Arc::new(PendingState::new());
        self.stack.0.push(Item {
            query: None,
            remote: None,
            deferred: None,
            slot: Slot::Program(Box::new(MatchProgram {
                state: Arc::clone(&state),
                input: Some((registry.owned_registry_snapshot(), validated)),
                remote: None,
                limits,
                capabilities,
                supports_given,
                answer: None,
            })),
        });
        Ok(Pending { state })
    }

    /// Queue one crate-private read program.
    #[cfg(test)]
    pub(crate) fn stage_program<T: Send + Sync + 'static>(
        &mut self,
        program: Program<T>,
    ) -> Result<Pending<T>, PendingError> {
        self.admit(false)?;
        self.admit_typed()?;
        let state = Arc::new(PendingState::new());
        self.stack.0.push(Item {
            query: None,
            remote: None,
            deferred: None,
            slot: Slot::Program(Box::new(ProgramSlot {
                state: Arc::clone(&state),
                program: Mutex::new(Some(program)),
                answer: None,
            })),
        });
        Ok(Pending { state })
    }
    /// Refuse typed work in an explicit `Schema` transaction, reusing the
    /// released borrowed-target code.
    fn admit_typed(&self) -> Result<(), PendingError> {
        if self.options.kind == Some(TransactionKind::Schema) {
            return Err(PendingError::NotAllowedInKind(Box::new(Diagnostic::new(
                DiagnosticCategory::InvalidContract,
                DiagnosticCode::new("borrowed_target_not_read_only")
                    .expect("the static borrowed-target code is canonical"),
                "typed reads and plans cannot be staged in a schema transaction",
            ))));
        }
        Ok(())
    }
    fn admit(&self, writes: bool) -> Result<(), PendingError> {
        if self.closed {
            return Err(PendingError::Closed);
        }
        if writes && self.options.kind == Some(TransactionKind::Read) {
            return Err(PendingError::ReadOnly);
        }
        Ok(())
    }
    /// Fire queued queries in order and expose their answers after success only.
    pub async fn commit(&mut self) -> Result<(), CommitError> {
        self.commit_with_deadline(crate::QueryExecutionDeadline::from_timeout(
            crate::QueryTimeout::NONE,
        ))
        .await
    }

    /// Commit within an already-running call's absolute deadline.
    ///
    /// Time spent preparing or staging after that deadline was captured counts
    /// against the same call. The given deadline can tighten the transaction's
    /// configured timeout but cannot extend it. A dispatched provider commit
    /// always runs to its actual outcome.
    pub async fn commit_with_deadline(
        &mut self,
        deadline: crate::QueryExecutionDeadline,
    ) -> Result<(), CommitError> {
        self.commit_controlled(deadline, None).await
    }

    pub(crate) async fn commit_with_policy(
        &mut self,
        deadline: crate::QueryExecutionDeadline,
        limits: &MatchExecutionLimits,
    ) -> Result<(), CommitError> {
        self.commit_controlled(deadline, Some(limits)).await
    }

    async fn commit_controlled(
        &mut self,
        deadline: crate::QueryExecutionDeadline,
        limits: Option<&MatchExecutionLimits>,
    ) -> Result<(), CommitError> {
        use tracing::Instrument;
        let deadline = self.commit_deadline(deadline)?;
        let span = tracing::debug_span!(target: "type_bridge.plan", "type_bridge.plan.commit", plans = self.stack.0.len(), nonce = tracing::field::Empty, outcome = tracing::field::Empty);
        let collect = self.options.collect_phase_timestamps
            || limits.is_some_and(|limits| limits.policy().collect_phase_timestamps)
            || self.stack.0.iter().any(|item| match &item.slot {
                Slot::Program(program) => program
                    .remote_limits()
                    .is_some_and(|limits| limits.policy().collect_phase_timestamps),
                _ => false,
            });
        let (mut result, phases) = crate::execution_phases::observe_result(
            deadline.started(),
            collect,
            self.commit_inner(deadline, limits),
        )
        .instrument(span.clone())
        .await;
        if let (Err(error), Some(phases)) = (&mut result, phases) {
            let original = error.diagnostic();
            let diagnostic = phases.diagnostic(original.clone());
            if diagnostic != original {
                error.remote = Some(Box::new(diagnostic));
            }
        }
        span.record("outcome", if result.is_ok() { "ok" } else { "error" });
        result
    }

    async fn commit_inner(
        &mut self,
        deadline: crate::QueryExecutionDeadline,
        limits: Option<&MatchExecutionLimits>,
    ) -> Result<(), CommitError> {
        let empty = std::mem::take(&mut self.empty);
        let result = self.commit_work(deadline, limits).await;
        for (state, answer) in empty {
            *state.lock().unwrap() = match &result {
                Ok(()) => Ok(answer),
                Err(error) => Err(error.pending()),
            };
        }
        result
    }

    async fn commit_work(
        &mut self,
        deadline: crate::QueryExecutionDeadline,
        limits: Option<&MatchExecutionLimits>,
    ) -> Result<(), CommitError> {
        if self.closed {
            return Err(CommitError::lifecycle(PendingError::Closed));
        }
        let mut execution = std::mem::replace(&mut self.stack, Execution(Vec::new(), Vec::new()));
        if execution.0.is_empty() {
            match execution.resolve_groups(&[]) {
                Ok(()) => {
                    execution.publish_groups();
                    return Ok(());
                }
                Err((index, error)) => {
                    let error = CommitError::new(error.into(), Some(index), Default::default());
                    execution.fail(error.pending());
                    return Err(error);
                }
            }
        }
        for (index, item) in execution.0.iter().enumerate() {
            if let Slot::Plan { plan, .. } = &item.slot
                && let Err(diagnostic) = plan.plan()
            {
                let error = CommitError::new(
                    plan_answer_error(diagnostic).into(),
                    Some(index),
                    Default::default(),
                );
                execution.fail(error.pending());
                return Err(error);
            }
        }
        for group in &execution.1 {
            if let Err(diagnostic) = group.stack() {
                let error = CommitError::new(
                    plan_answer_error(diagnostic).into(),
                    Some(group.range.start),
                    Default::default(),
                );
                execution.fail(error.pending());
                return Err(error);
            }
        }
        let backend = match &self.backend {
            Backend::Local(backend) => Arc::clone(backend),
            Backend::Proxy(proxy) => {
                return self
                    .commit_proxy(Arc::clone(proxy), execution, deadline, limits)
                    .await;
            }
        };
        let kind = self.options.kind.unwrap_or_else(|| {
            if execution.0.iter().any(Item::writes) {
                TransactionKind::Write
            } else {
                TransactionKind::Read
            }
        });
        let mut answer_limits = self.options.answer_limits.answer.clone();
        let result = async {
            answer_limits.deadline = deadline.instant();
            let opened = bounded(&answer_limits, async {
                backend.open_with_options(&self.database, kind.into(), &self.options).await
            })
            .await
            .map_err(|error| CommitError::new(error.into(), None, Default::default()))?;
            // A program borrows the provider as an owned transaction and
            // hands it back; `None` means it was consumed.
            crate::execution_phases::record(type_bridge_contract::execution_timeline::ExecutionPhase::Accepted);
            crate::execution_phases::record(type_bridge_contract::execution_timeline::ExecutionPhase::ExecutionStarted);
            let mut provider = Some(opened);
            let server_version = backend.server_version();
            let supports_given = super::database::backend_supports_given_stage(backend.as_ref());
            let mut answers = Vec::with_capacity(execution.0.len());
            let mut plan_meter = crate::answer_meter::PlanMeter::new(&self.options.answer_limits);
            for (index, item) in execution.0.iter_mut().enumerate() {
                let prepared = item.bind_source(&answers, supports_given);
                let resolved = match prepared { Err(error) => Err(error), Ok(()) => match (&item.query, provider.as_mut()) {
                    (_, None) => Err(provider_consumed()),
                    (Some(query), Some(ops)) => {
                        let result = query
                            .execute(
                                ops.as_mut(),
                                AnswerLimits {
                                    answer: answer_limits.clone(),
                                    ..self.options.answer_limits.clone()
                                },
                            )
                            .await;
                        // Plan answers resolve before the provider commit, so a
                        // malformed answer aborts instead of surfacing after writes.
                        result.and_then(|answer| {
                            let _answer = tracing::debug_span!(target: "type_bridge.plan", "type_bridge.plan.answer", fingerprint = query.fingerprint.as_deref().unwrap_or("")).entered();
                            if matches!(item.slot, Slot::Plan { .. }) {
                                crate::answer_meter::meter_result(&answer, &answer_limits)?;
                            }
                            let resolved = item.slot.resolve(answer).map_err(plan_answer_error)?;
                            if let Resolved::Plan { wire, .. } = &resolved {
                                plan_meter.accept(wire)?;
                            }
                            Ok(resolved)
                        })
                    }
                    (None, Some(_)) => {
                        let Slot::Program(program) = &mut item.slot else {
                            unreachable!("only programs stage without a query")
                        };
                        let ops = provider.take().expect("the provider was just matched");
                        let mut owned = OwnedTransaction::from_eager(EagerTransaction::new(
                            ops,
                            kind.into(),
                            server_version,
                        ));
                        // The program's own budget reports precise limits; this
                        // race keeps the commit's cancellation and deadline
                        // binding between its statements too.
                        let ran = bounded(
                            &answer_limits,
                            program.run(&mut owned, answer_limits.clone()),
                        )
                        .await;
                        provider = owned.into_provider();
                        ran.map(|()| Resolved::Program)
                    }
                }};
                match resolved {
                    Ok(answer) => answers.push(answer),
                    Err(error) => {
                        if let Some(ops) = provider.as_mut() {
                            if kind != TransactionKind::Read {
                                let _ = ops.rollback().await;
                            }
                            let _ = ops.close().await;
                        }
                        return Err(CommitError::new(
                            error.into(),
                            Some(index),
                            item.failure_context(),
                        ));
                    }
                }
            }
            if let Err((index, error)) = execution.resolve_groups(&answers) {
                if let Some(ops) = provider.as_mut() {
                    if kind != TransactionKind::Read { let _ = ops.rollback().await; }
                    let _ = ops.close().await;
                }
                return Err(CommitError::new(error.into(), Some(index), Default::default()));
            }
            let provider = provider.as_mut().ok_or_else(|| {
                CommitError::new(provider_consumed().into(), None, Default::default())
            })?;
            crate::execution_phases::record(type_bridge_contract::execution_timeline::ExecutionPhase::BeforeCommit);
            if let Err(error) = super::backend::BoundedAnswerReader::new(answer_limits.clone()).check_before_read() {
                if kind != TransactionKind::Read { let _ = provider.rollback().await; }
                let _ = provider.close().await;
                return Err(CommitError::new(error.into(), None, Default::default()));
            }
            if self.options.never_commit || kind == TransactionKind::Read {
                provider
                    .close()
                    .await
                    .map_err(|error| CommitError::new(error.into(), None, Default::default()))?;
            } else {
                let commit = provider.commit_classified().await;
                let close = provider.close().await;
                commit.map_err(|error| CommitError::new(error, None, Default::default()))?;
                if close.is_err() {
                    tracing::warn!(code = "transaction_close_failed");
                }
            }
            Ok(answers)
        }
        .await;
        match result {
            Ok(answers) => {
                for (item, answer) in execution.0.iter_mut().zip(answers) {
                    item.slot.publish(answer);
                }
                execution.publish_groups();
                Ok(())
            }
            Err(error) => {
                execution.fail(error.pending());
                Err(error)
            }
        }
    }
    async fn commit_proxy(
        &mut self,
        proxy: Arc<ProxyStaging>,
        mut execution: Execution,
        deadline: crate::QueryExecutionDeadline,
        limits: Option<&MatchExecutionLimits>,
    ) -> Result<(), CommitError> {
        let kind = self.options.kind.unwrap_or_else(|| {
            if execution.0.iter().any(|item| {
                item.remote
                    .as_ref()
                    .is_some_and(|plan| plan.plan().writes())
            }) {
                TransactionKind::Write
            } else {
                TransactionKind::Read
            }
        });
        let timeout = deadline.instant().map(|instant| {
            instant.saturating_duration_since(tokio::time::Instant::now().into_std())
        });
        let mut plans = Vec::new();
        let mut counts = Vec::with_capacity(execution.0.len());
        let mut policy = None;
        let mut origins = std::collections::BTreeMap::new();
        if let Some(limits) = limits {
            constrain_match_policy(
                &mut policy,
                &mut origins,
                limits.policy(),
                "program",
                Some(0),
            );
        }
        for group in &execution.1 {
            constrain_match_policy(
                &mut policy,
                &mut origins,
                group.limits,
                "program",
                Some(group.range.start),
            );
        }
        for (index, item) in execution.0.iter_mut().enumerate() {
            match &mut item.slot {
                Slot::Program(program) => {
                    match program
                        .prepare_remote(proxy.declared(), &self.options.answer_limits.answer)
                    {
                        Ok((remote, limits)) => {
                            counts.push(remote.len());
                            plans.extend(remote);
                            constrain_match_policy(
                                &mut policy,
                                &mut origins,
                                limits,
                                "program",
                                Some(index),
                            );
                        }
                        Err(error) => {
                            let code = match &error {
                                OrmError::Match(error) => Some(error.code().as_str().to_owned()),
                                _ => None,
                            };
                            let mut error =
                                CommitError::new(error.into(), Some(index), Default::default());
                            if let (Some(code), Some(limits)) = (code, program.remote_limits()) {
                                let mut origins = std::collections::BTreeMap::new();
                                let mut policy = None;
                                constrain_match_policy(
                                    &mut policy,
                                    &mut origins,
                                    limits.policy(),
                                    "program",
                                    Some(index),
                                );
                                annotate_match_limit(&mut error, &code, limits.policy(), &origins);
                            }
                            execution.fail(error.pending());
                            return Err(error);
                        }
                    }
                }
                _ => {
                    counts.push(1);
                    let entry = item.remote.take().expect("proxy plan entry");
                    let entry = if let Some(deferred) = &item.deferred {
                        let base = plans.len() - deferred.local_index;
                        match u32::try_from(base)
                            .map_err(|_| proxy_refusal("result reference block offset is invalid"))
                            .and_then(|base| entry.rebase_result_refs(base))
                        {
                            Ok(entry) => entry,
                            Err(diagnostic) => {
                                let error = CommitError::lifecycle(PendingError::InvalidPlan(
                                    Box::new(diagnostic),
                                ));
                                execution.fail(error.pending());
                                return Err(error);
                            }
                        }
                    } else {
                        entry
                    };
                    plans.push(entry);
                }
            }
        }
        if policy.is_some() {
            let connection = match proxy.match_limits() {
                Ok(connection) => connection,
                Err(diagnostic) => {
                    let pending = PendingError::InvalidPlan(Box::new(diagnostic));
                    execution.fail(pending.clone());
                    return Err(CommitError::lifecycle(pending));
                }
            };
            constrain_match_policy(&mut policy, &mut origins, connection, "connection", None);
            let bounds = &self.options.answer_limits;
            let transaction = crate::QueryExecutionResourceLimits {
                collect_phase_timestamps: false,
                timeout_milliseconds: timeout.map_or(crate::QueryTimeout::NONE, |value| {
                    crate::QueryTimeout::from_milliseconds(
                        u64::try_from(value.as_millis()).unwrap_or(u64::MAX),
                    )
                }),
                items: bounds.answer.max_items,
                bytes: bounds.answer.max_bytes,
                graph_nodes: bounds.max_graph_nodes,
                attribute_values: bounds.max_attribute_values,
                collection_members: bounds.max_collection_members,
                role_players: bounds.max_role_players,
                statements: None,
            };
            constrain_match_policy(&mut policy, &mut origins, transaction, "transaction", None);
            let need = plans
                .iter()
                .map(|entry| {
                    let (read, write) = entry.plan().statement_counts();
                    read + write
                })
                .sum();
            if let Some(cap) = policy.and_then(|policy| policy.statements)
                && need > cap as usize
            {
                let mut error = CommitError::new(
                    OrmError::from(MatchError::new(
                        MatchErrorCategory::ResourceLimit,
                        "statement_count_limit",
                        "staged model stack exceeds its aggregate statement ceiling",
                    ))
                    .into(),
                    None,
                    Default::default(),
                );
                annotate_match_limit(
                    &mut error,
                    "statement_count_limit",
                    policy.expect("match policy"),
                    &origins,
                );
                execution.fail(error.pending());
                return Err(error);
            }
            policy = policy.map(|policy| policy.for_adapter_statements(need));
        }
        if let Err(diagnostic) = deadline.check(&self.options.answer_limits.answer.cancellation) {
            let error = CommitError::new(
                OrmError::ExecutionDiagnostic(Box::new(diagnostic)).into(),
                None,
                Default::default(),
            );
            execution.fail(error.pending());
            return Err(error);
        }
        let timeout = deadline.instant().map(|instant| {
            instant.saturating_duration_since(tokio::time::Instant::now().into_std())
        });
        let timeout = if policy.is_some() {
            let now = tokio::time::Instant::now().into_std();
            let program = execution
                .0
                .iter()
                .filter_map(|item| match &item.slot {
                    Slot::Program(program) => program
                        .remote_limits()
                        .and_then(MatchExecutionLimits::deadline),
                    _ => None,
                })
                .min()
                .map(|deadline| deadline.saturating_duration_since(now));
            match (timeout, program) {
                (Some(left), Some(right)) => Some(left.min(right)),
                (left, right) => left.or(right),
            }
        } else {
            timeout
        };
        let deadline_ms = timeout.map(|timeout| {
            u64::try_from(timeout.as_millis())
                .unwrap_or(u64::MAX)
                .max(1)
        });
        let built = proxy
            .request_with_policy(plans, kind, deadline_ms, policy)
            .and_then(|request| request.encode().map(|bytes| (request, bytes)));
        let (request, bytes) = match built {
            Ok(built) => built,
            Err(diagnostic) => {
                let error = CommitError::lifecycle(PendingError::InvalidPlan(Box::new(diagnostic)));
                execution.fail(error.pending());
                return Err(error);
            }
        };
        let span = tracing::Span::current();
        if !span.is_disabled() {
            span.record("nonce", request.nonce());
        }
        Self::attempt(
            &proxy,
            (request, bytes),
            execution,
            timeout,
            counts,
            policy,
            origins,
            limits,
        )
        .await
    }
    fn commit_deadline(
        &self,
        given: crate::QueryExecutionDeadline,
    ) -> Result<crate::QueryExecutionDeadline, CommitError> {
        let Some(timeout) = self.commit_timeout() else {
            return Ok(given);
        };
        let deadline = given.started().checked_add(timeout).ok_or_else(|| {
            CommitError::lifecycle(PendingError::Failed {
                diagnostic: Box::new(SdkExecutionDiagnostic::internal_failure()),
                op_index: None,
                message: "Transaction deadline exceeds the supported range".into(),
                failure: None,
                context: Default::default(),
            })
        })?;
        Ok(given.with_ceiling(deadline))
    }

    fn commit_timeout(&self) -> Option<Duration> {
        let timeout = self
            .options
            .timeout
            .unwrap_or_else(|| {
                if self.options.answer_timeout.is_some()
                    || !self
                        .stack
                        .0
                        .iter()
                        .any(|item| matches!(item.slot, Slot::Plan { .. } | Slot::Program(_)))
                {
                    crate::QueryTimeout::NONE
                } else {
                    crate::QueryTimeout::default()
                }
            })
            .milliseconds()
            .map(Duration::from_millis);
        [timeout, self.options.deadline, self.options.answer_timeout]
            .into_iter()
            .flatten()
            .min()
    }
    #[allow(clippy::too_many_arguments)]
    async fn attempt(
        proxy: &ProxyStaging,
        encoded: (StagedRequestV3, Vec<u8>),
        mut execution: Execution,
        timeout: Option<Duration>,
        counts: Vec<usize>,
        policy: Option<crate::QueryExecutionResourceLimits>,
        origins: std::collections::BTreeMap<&'static str, (&'static str, Option<usize>)>,
        limits: Option<&MatchExecutionLimits>,
    ) -> Result<(), CommitError> {
        let (request, bytes) = encoded;
        // Anything short of a verified reply may have committed.
        let unknown = CommitError::unknown;
        // Unknown from before the first byte is sent: a cancelled commit
        // leaves its handles unknown.
        execution.fail(unknown("the proxy commit outcome is not known").pending());
        let sent = {
            let mut cancellations = limits
                .into_iter()
                .chain(execution.0.iter().filter_map(|item| match &item.slot {
                    Slot::Program(program) => program.remote_limits(),
                    _ => None,
                }))
                .map(|limits| Box::pin(limits.cancellation().cancelled()))
                .collect::<Vec<_>>();
            let cancelled = std::future::poll_fn(|context| {
                for cancellation in &mut cancellations {
                    if std::future::Future::poll(cancellation.as_mut(), context).is_ready() {
                        return std::task::Poll::Ready(());
                    }
                }
                std::task::Poll::Pending
            });
            let expired = async {
                match (policy, timeout) {
                    (Some(_), Some(timeout)) => tokio::time::sleep(timeout).await,
                    _ => std::future::pending::<()>().await,
                }
            };
            tokio::select! {
                biased;
                () = cancelled => Err(crate::QueryTargetError::Interrupted(SdkExecutionDiagnostic::internal_failure())),
                () = expired => Err(crate::QueryTargetError::Interrupted(SdkExecutionDiagnostic::internal_failure())),
                result = proxy.send(bytes, timeout) => result,
            }
        };
        let outcome = match sent {
            Err(cause) => {
                let mut error = unknown("the proxy reply was not received");
                error.source_error = Some(Box::new(cause));
                Err(error)
            }
            Ok(reply) => match proxy.verify(&reply, &request) {
                Err(_) => Err(unknown("the proxy reply could not be verified")),
                Ok(verified) => match verified.diagnostic() {
                    Some(projection) => {
                        let mut failure = proxy_failure(projection);
                        if let Some(policy) = policy {
                            annotate_match_limit(&mut failure, projection.code(), policy, &origins);
                        }
                        Err(failure)
                    }
                    None => {
                        let mut entries = request.into_plans().into_iter();
                        let mut cursor = 0;
                        let mut resolved = Vec::with_capacity(execution.0.len());
                        let mut error = None;
                        for (index, (item, count)) in execution.0.iter_mut().zip(counts).enumerate()
                        {
                            let Some(answers) = verified.answers().get(cursor..cursor + count)
                            else {
                                error = Some(unknown("proxy model answer ranges are invalid"));
                                break;
                            };
                            cursor += count;
                            let result = match &mut item.slot {
                                Slot::Program(program) => program
                                    .resolve_remote(&mut entries, answers)
                                    .map(|()| Resolved::Program),
                                Slot::Plan { plan, .. } => {
                                    let entry = entries.next().expect("verified plan entry");
                                    let bound = if entry.result_refs().is_empty() {
                                        Ok(())
                                    } else {
                                        entry
                                            .bind_results(
                                                &verified.answers()[..cursor - count],
                                                proxy.declared(),
                                            )
                                            .map(|bound| *plan = PlanSource::Owned(Arc::new(bound)))
                                    };
                                    bound
                                        .and_then(|()| plan.plan())
                                        .and_then(|plan| {
                                            PlanAnswer::from_verified_staged_v3(&answers[0], plan)
                                        })
                                        .map(|answer| Resolved::Plan {
                                            answer: Arc::new(answer),
                                            wire: Arc::new(answers[0].clone()),
                                            correlation: verified.correlations()[cursor - count]
                                                .clone(),
                                        })
                                        .map_err(plan_answer_error)
                                }
                                Slot::Query(_) => unreachable!("proxy stages canonical plans"),
                            };
                            match result {
                                Ok(answer) => resolved.push(answer),
                                Err(_) => {
                                    let _ = index;
                                    error =
                                        Some(unknown("proxy model projection failed after commit"));
                                    break;
                                }
                            }
                        }
                        match error {
                            Some(error) => Err(error),
                            None => execution
                                .resolve_groups(&resolved)
                                .map(|()| resolved)
                                .map_err(|_| unknown("proxy model projection failed after commit")),
                        }
                    }
                },
            },
        };
        let outcome = outcome.and_then(|answers| {
            if limits.is_some_and(|limits| {
                limits.cancellation().is_cancelled()
                    || limits
                        .deadline()
                        .is_some_and(|deadline| tokio::time::Instant::now().into_std() >= deadline)
            }) {
                Err(unknown(
                    "proxy answer processing exceeded the invocation budget",
                ))
            } else {
                Ok(answers)
            }
        });
        match outcome {
            Ok(answers) => {
                for (item, answer) in execution.0.iter_mut().zip(answers) {
                    item.slot.publish(answer);
                }
                execution.publish_groups();
                Ok(())
            }
            Err(error) => {
                execution.fail(error.pending());
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
        self.stack.1.clear();
        for (state, _) in self.empty.drain(..) {
            *state.lock().unwrap() = Err(PendingError::RolledBack);
        }
        Ok(())
    }
    /// Discard queued work and make this logical transaction terminal.
    pub fn close(&mut self) -> Result<(), PendingError> {
        if self.closed {
            return Err(PendingError::Closed);
        }
        self.stack.fail(PendingError::Closed);
        self.stack.0.clear();
        self.stack.1.clear();
        for (state, _) in self.empty.drain(..) {
            *state.lock().unwrap() = Err(PendingError::Closed);
        }
        self.closed = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::backend::{BoxFuture, TransactionOps};
    use super::*;
    use type_bridge_contract::query_plan::v3::Operation;

    fn signed(diagnostic: &SdkExecutionDiagnostic) -> SdkDiagnosticProjectionV3 {
        let host = serde_json::to_value(diagnostic.host_json()).expect("host json");
        serde_json::from_value(host).expect("a valid projection")
    }

    #[test]
    fn proxy_refusals_keep_the_typed_failure_the_server_signed() {
        use type_bridge_contract::failure::{Phase, Reason};
        for failure in [
            Failure::Rejected {
                phase: Phase::Query,
                reason: Reason::Unique,
                op_index: Some(0),
            },
            Failure::Rejected {
                phase: Phase::Commit,
                reason: Reason::Cardinality,
                op_index: None,
            },
        ] {
            let projection = signed(&SdkExecutionDiagnostic::from_failure(
                &failure,
                Default::default(),
            ));
            let commit = proxy_failure(&projection);
            assert_eq!(commit.failure.as_deref(), Some(&failure));
            let diagnostic = commit.diagnostic();
            assert_eq!(diagnostic.failure(), Some(&failure));
            assert_eq!(diagnostic.code().as_str(), projection.code());
            assert_eq!(commit.pending().failure(), Some(&failure));
        }
        // A refusal whose details name no failure stays code-only.
        let plain = signed(&SdkExecutionDiagnostic::provider_failure(
            SdkProviderOperation::Write,
        ));
        let commit = proxy_failure(&plain);
        assert!(commit.failure.is_none());
        assert!(commit.diagnostic().failure().is_none());
        assert_eq!(
            commit.diagnostic().code().as_str(),
            "provider_operation_failed"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn proxy_deadline_keeps_the_server_phase_and_clock() {
        let (diagnostic, _) =
            crate::execution_phases::observe(tokio::time::Instant::now().into_std(), true, async {
                crate::execution_phases::received();
                tokio::time::advance(Duration::from_millis(7)).await;
                crate::execution_phases::annotate(
                    crate::QueryExecutionDeadline::expired()
                        .check(&Default::default())
                        .unwrap_err(),
                )
            })
            .await;
        let mapped = proxy_failure(&signed(&diagnostic)).diagnostic();
        assert_eq!(mapped.code(), diagnostic.code());
        assert_eq!(mapped.details(), diagnostic.details());
        let json = serde_json::to_value(mapped.host_json()).unwrap();
        assert_eq!(json["details"]["execution_phase"]["value"], "accepted");
        assert_eq!(json["details"]["elapsed_milliseconds"]["value"], "7");
    }

    #[test]
    fn proxy_cancellation_keeps_its_typed_category_and_code() {
        use type_bridge_contract::sdk_diagnostic::SdkDiagnosticCategory;
        let projection = signed(&SdkExecutionDiagnostic::data_operation_cancelled());
        let commit = proxy_failure(&projection);
        for diagnostic in [
            commit.diagnostic(),
            commit.pending().diagnostic().unwrap().clone(),
        ] {
            assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Cancelled);
            assert_eq!(diagnostic.code().as_str(), "provider_cancelled");
        }
    }

    #[test]
    fn signed_proxy_failure_messages_never_reach_public_errors() {
        use type_bridge_contract::sdk_diagnostic::{SdkDiagnosticCode, SdkDiagnosticMessage};
        for code in [
            "provider_operation_failed",
            "commit_definitely_aborted",
            "commit_outcome_unknown",
        ] {
            let diagnostic = SdkExecutionDiagnostic::transaction_failure(
                SdkDiagnosticCode::new(code).unwrap(),
                SdkDiagnosticMessage::new("signed-server-secret").unwrap(),
            );
            let projection = signed(&diagnostic);
            let error = proxy_failure(&projection);
            for rendered in [
                error.to_string(),
                format!("{:?}", error),
                error.pending().to_string(),
                serde_json::to_string(&error.pending().diagnostic().unwrap().host_json()).unwrap(),
                serde_json::to_string(&error.diagnostic().host_json()).unwrap(),
            ] {
                assert!(!rendered.contains("signed-server-secret"), "{rendered}");
            }
            assert_eq!(error.diagnostic().code().as_str(), code);
        }
    }

    fn proxy_match_fixture(
        options: TransactionOptions,
        bad_projection: bool,
        pending_reply: Option<Arc<tokio::sync::Notify>>,
    ) -> (Transaction, DescriptorRegistry, Arc<Mutex<usize>>) {
        proxy_match_fixture_with_transport(options, bad_projection, pending_reply, None)
    }

    fn proxy_match_fixture_with_transport(
        options: TransactionOptions,
        bad_projection: bool,
        pending_reply: Option<Arc<tokio::sync::Notify>>,
        transport: Option<Arc<dyn super::super::staged_proxy::StagedTransport>>,
    ) -> (Transaction, DescriptorRegistry, Arc<Mutex<usize>>) {
        use type_bridge_contract::query_remote::{RemoteCapabilities, RemoteExecutorBinding};
        use type_bridge_contract::query_remote_v3::{
            StagedAnswerV3, StagedOutcomeV3, StagedReplyV3, StagedRequestV3,
        };
        let registry = DescriptorRegistry::new();
        registry
            .register_entity(crate::_descriptor::EntityDescriptor {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        let authority =
            crate::match_request::descriptor_plan::MatchRequestAdapterAuthority::from_registry(
                &registry,
            )
            .unwrap();
        let schema = authority.declared().clone();
        let mut capabilities =
            type_bridge_contract::query_plan::v3::query_plan_v3_capability_vocabulary();
        for name in [
            type_bridge_contract::query_remote_v3::CAP_QUERY_REMOTE_STAGED_V3,
            type_bridge_contract::query_remote_v3::CAP_QUERY_REMOTE_WRITE_V3,
            type_bridge_contract::query_remote_v3::CAP_QUERY_REMOTE_ANSWER_DIMENSIONS,
        ] {
            capabilities.insert(type_bridge_contract::capability::CapabilityId::new(name).unwrap());
        }
        let signer =
            Arc::new(crate::query_v2_remote::RemoteReplySigningKey::from_secret_bytes([0x45; 32]));
        let advertisement = RemoteCapabilities::new(
            capabilities,
            RemoteExecutorBinding::new("proxy-match-tests", "epoch-00000000001").unwrap(),
            signer.public_key(),
        );
        let ad_bytes = advertisement.encode().unwrap();
        let committed = Arc::new(Mutex::new(0));
        let observed = Arc::clone(&committed);
        let captured_schema = schema.clone();
        let exchange: super::super::staged_proxy::StagedExchangeFn = Arc::new(move |bytes| {
            let schema = captured_schema.clone();
            let ad = advertisement.clone();
            let signer = Arc::clone(&signer);
            let committed = Arc::clone(&observed);
            let pending_reply = pending_reply.clone();
            Box::pin(async move {
                if let Some(entered) = pending_reply {
                    *committed.lock().unwrap() += 1;
                    entered.notify_one();
                    std::future::pending::<()>().await;
                }
                // This closure simulates a server; count only client verification spans.
                let _server_tracing = tracing::dispatcher::set_default(&tracing::Dispatch::none());
                let request = StagedRequestV3::decode(&bytes, &schema, &ad).unwrap();
                let (outcome, answers) = if request.plans().len() as u64
                    > request.limits().max_items
                {
                    (StagedOutcomeV3::Failed { diagnostic:SdkExecutionDiagnostic::query_failure(type_bridge_contract::sdk_diagnostic::SdkQueryDiagnosticCategory::ResourceLimit,type_bridge_contract::sdk_diagnostic::SdkDiagnosticCode::new("processed_item_limit").unwrap()) },vec![])
                } else {
                    *committed.lock().unwrap() += 1;
                    (
                        StagedOutcomeV3::Committed,
                        request
                            .plans()
                            .iter()
                            .map(|entry| if entry.plan().fetch_document_shape().is_some() {
                                PlanAnswer::Documents(vec![serde_json::json!({"_iid":"0x01", "_type":"person", "attributes":{}})])
                                    .to_staged_v3(entry.plan()).unwrap()
                            } else { StagedAnswerV3::Rows {
                                rows: vec![vec![
                                    type_bridge_contract::query_remote::RemoteValue::Value {
                                        value: type_bridge_contract::value::CanonicalValue::Long(
                                            if bad_projection { -1 } else { 7 },
                                        ),
                                    },
                                ]],
                            } })
                            .collect(),
                    )
                };
                Ok(StagedReplyV3::new(&request, outcome, answers)
                    .unwrap()
                    .encode_signed(&request, &ad, signer.as_ref())
                    .unwrap())
            })
        });
        let proxy = super::super::staged_proxy::ProxyStaging::new(
            None,
            Some(exchange),
            &ad_bytes,
            schema,
            crate::QueryExecutionResourceLimits::default_for(
                crate::query_execution_limits::AnswerTransport::Proxy,
            ),
        )
        .unwrap();
        let proxy = match transport {
            Some(transport) => ProxyStaging::with_route(
                super::super::staged_proxy::Route::Transport(transport),
                &ad_bytes,
                Arc::new(proxy.declared().clone()),
                crate::QueryExecutionResourceLimits::default_for(
                    crate::query_execution_limits::AnswerTransport::Proxy,
                ),
            )
            .unwrap(),
            None => proxy,
        };
        (Transaction::new_proxy(proxy, options), registry, committed)
    }
    fn proxy_count_request(registry: &DescriptorRegistry) -> ValidatedMatchRequest {
        use crate::match_request::*;
        validate_match_request(
            registry,
            MatchRequest::v1(
                MatchPlan {
                    bindings: vec![MatchBinding {
                        id: BindingId::new(0),
                        descriptor: DescriptorId::new("entity:person"),
                        thing_kind: ThingKind::Entity,
                        match_mode: MatchMode::Exact,
                    }],
                    predicate: None,
                    allowed_cross_joins: Default::default(),
                },
                MatchOperation::CountBy {
                    root: BindingId::new(0),
                },
            ),
        )
        .unwrap()
    }
    #[tokio::test]
    async fn proxy_match_aggregate_limit_names_the_tighter_program_or_transaction() {
        for transaction_bound in [false, true] {
            let options = if transaction_bound {
                TransactionOptions::default().with_answer_limits(BoundedAnswerLimits {
                    max_items: 1,
                    ..Default::default()
                })
            } else {
                TransactionOptions::default()
            };
            let (mut tx, registry, committed) = proxy_match_fixture(options, false, None);
            let limits = |items| {
                MatchExecutionLimits::from_policy(
                    crate::QueryExecutionResourceLimits {
                        items,
                        ..Default::default()
                    },
                    super::super::backend::AnswerCancellation::default(),
                )
            };
            let first = tx
                .stage_match_with_limits(&registry, proxy_count_request(&registry), limits(20))
                .unwrap();
            let second = tx
                .stage_match_with_limits(
                    &registry,
                    proxy_count_request(&registry),
                    limits(if transaction_bound { 10 } else { 1 }),
                )
                .unwrap();
            let error = tx.commit().await.unwrap_err();
            assert_eq!(*committed.lock().unwrap(), 0);
            let host = serde_json::to_value(error.diagnostic().host_json()).unwrap();
            assert_eq!(host["details"]["limit"]["value"], "items");
            assert_eq!(
                host["details"]["limit_scope"]["value"],
                if transaction_bound {
                    "transaction"
                } else {
                    "program"
                }
            );
            if !transaction_bound {
                assert_eq!(host["details"]["program_index"]["value"], "1");
            }
            assert!(first.get().is_err());
            assert!(second.get().is_err());
        }
    }
    #[tokio::test]
    async fn proxy_match_publishes_all_or_keeps_every_projection_unknown() {
        for bad in [false, true] {
            let (mut tx, registry, committed) =
                proxy_match_fixture(TransactionOptions::default(), bad, None);
            let first = tx
                .stage_match(&registry, proxy_count_request(&registry))
                .unwrap();
            let second = tx
                .stage_match(&registry, proxy_count_request(&registry))
                .unwrap();
            assert!(matches!(first.get(), Err(PendingError::NotCommitted)));
            let outcome = tx.commit().await;
            assert_eq!(*committed.lock().unwrap(), 1);
            if bad {
                assert!(outcome.is_err());
                assert!(matches!(first.get(), Err(PendingError::Failed { .. })));
                assert!(matches!(second.get(), Err(PendingError::Failed { .. })));
                assert_eq!(
                    outcome.unwrap_err().diagnostic().code().as_str(),
                    "commit_outcome_unknown"
                );
            } else {
                outcome.unwrap();
                assert!(first.get().is_ok());
                assert!(second.get().is_ok());
            }
        }
    }

    #[tokio::test]
    async fn proxy_commit_keeps_the_application_cause_and_unknown_outcome() {
        struct Fails;
        impl super::super::staged_proxy::StagedTransport for Fails {
            fn staged_capabilities(
                &self,
            ) -> BoxFuture<'_, Result<Vec<u8>, crate::QueryTargetError>> {
                panic!("fixture already has its advertisement")
            }
            fn exchange_staged<'a>(
                &'a self,
                _: &'a [u8],
            ) -> BoxFuture<'a, Result<Vec<u8>, crate::QueryTargetError>> {
                Box::pin(async {
                    Err(crate::QueryTargetError::Callback(Box::new(
                        std::io::Error::other("application-secret"),
                    )))
                })
            }
        }
        let (mut tx, registry, _) = proxy_match_fixture_with_transport(
            TransactionOptions::default(),
            false,
            None,
            Some(Arc::new(Fails)),
        );
        let pending = tx
            .stage_match(&registry, proxy_count_request(&registry))
            .unwrap();
        let error = tx.commit().await.unwrap_err();
        assert_eq!(error.diagnostic().code().as_str(), "commit_outcome_unknown");
        assert!(matches!(
            error.error.as_ref(),
            ClassifiedCommitError::Driver {
                certainty: crate::error::CommitFailureCertainty::Unknown,
                ..
            }
        ));
        assert!(matches!(
            pending.get().unwrap_err().failure(),
            Some(Failure::Indeterminate { .. })
        ));
        assert!(!format!("{error:?} {error}").contains("application-secret"));
        let target = std::error::Error::source(&error)
            .unwrap()
            .downcast_ref::<crate::QueryTargetError>()
            .unwrap();
        let cause = std::error::Error::source(target)
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap();
        assert_eq!(cause.to_string(), "application-secret");
    }

    #[tokio::test(start_paused = true)]
    async fn proxy_match_cancel_and_absolute_deadline_interrupt_a_pending_reply_as_unknown() {
        for cancel in [true, false] {
            let entered = Arc::new(tokio::sync::Notify::new());
            let (mut tx, registry, committed) = proxy_match_fixture(
                TransactionOptions::default(),
                false,
                Some(Arc::clone(&entered)),
            );
            let cancellation = super::super::backend::AnswerCancellation::default();
            let limits = MatchExecutionLimits::from_policy(
                crate::QueryExecutionResourceLimits {
                    timeout_milliseconds: crate::QueryTimeout::from_milliseconds(5000),
                    ..Default::default()
                },
                cancellation.clone(),
            )
            .with_absolute_deadline(crate::QueryExecutionDeadline::from_instant(
                (tokio::time::Instant::now() + Duration::from_secs(5)).into_std(),
            ));
            let first = tx
                .stage_match_with_limits(&registry, proxy_count_request(&registry), limits)
                .unwrap();
            let second = tx
                .stage_match(&registry, proxy_count_request(&registry))
                .unwrap();
            let execution = tokio::spawn(async move { tx.commit().await });
            entered.notified().await;
            assert_eq!(*committed.lock().unwrap(), 1);
            if cancel {
                cancellation.cancel();
            } else {
                tokio::time::advance(Duration::from_secs(5)).await;
            }
            tokio::task::yield_now().await;
            assert!(execution.is_finished());
            let error = execution.await.unwrap().unwrap_err();
            assert!(matches!(
                error.error.as_ref(),
                ClassifiedCommitError::Driver {
                    certainty: crate::error::CommitFailureCertainty::Unknown,
                    ..
                }
            ));
            for pending in [first, second] {
                assert!(matches!(pending.get(), Err(PendingError::Failed { .. })));
                assert!(matches!(
                    pending.get().unwrap_err().failure(),
                    Some(Failure::Indeterminate { .. })
                ));
            }
        }
    }

    #[tokio::test]
    async fn proxy_match_aggregate_statement_refusal_names_its_program_before_send() {
        let (mut tx, registry, committed) =
            proxy_match_fixture(TransactionOptions::default(), false, None);
        let limits = MatchExecutionLimits::from_policy(
            crate::QueryExecutionResourceLimits {
                statements: Some(1),
                ..Default::default()
            },
            super::super::backend::AnswerCancellation::default(),
        );
        let first = tx
            .stage_match_with_limits(&registry, proxy_count_request(&registry), limits)
            .unwrap();
        let second = tx
            .stage_match(&registry, proxy_count_request(&registry))
            .unwrap();
        let error = tx.commit().await.unwrap_err();
        assert_eq!(*committed.lock().unwrap(), 0);
        let host = serde_json::to_value(error.diagnostic().host_json()).unwrap();
        assert_eq!(host["code"], "statement_count_limit");
        assert_eq!(host["details"]["limit_scope"]["value"], "program");
        assert_eq!(host["details"]["program_index"]["value"], "0");
        assert!(first.get().is_err());
        assert!(second.get().is_err());
    }

    #[derive(Default)]
    struct State {
        events: Vec<String>,
        pending_writes: usize,
        visible_writes: usize,
        kinds: Vec<TxType>,
        fail_close: bool,
        given: bool,
        rows: usize,
        row_values: Option<Vec<serde_json::Value>>,
        answer_ok: bool,
        snapshot_documents: Option<Vec<serde_json::Value>>,
        snapshot_batches: std::collections::VecDeque<Vec<serde_json::Value>>,
        fail_delete: bool,
        given_payloads: Vec<GivenRowsSpec>,
        given_queries: Vec<String>,
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
        fn supports_given_rows(&self) -> bool {
            self.0.lock().unwrap().given
        }
        fn server_version(&self) -> Option<type_bridge_core_lib::version::Version> {
            "3.13.6".parse().ok()
        }
    }
    impl Fake {
        fn record_write(&self, query: &str) {
            if query == "earlier-write" || query.split_whitespace().any(|part| part == "update") {
                self.0.lock().unwrap().pending_writes += 1;
            }
        }
        fn documents(&self) -> Option<QueryResult> {
            let mut state = self.0.lock().unwrap();
            state
                .snapshot_batches
                .pop_front()
                .or_else(|| state.snapshot_documents.clone())
                .map(QueryResult::Documents)
        }
        fn answer(&self) -> QueryResult {
            let state = self.0.lock().unwrap();
            if state.answer_ok {
                QueryResult::Ok
            } else {
                QueryResult::Rows(
                    state
                        .row_values
                        .clone()
                        .unwrap_or_else(|| vec![serde_json::json!({}); state.rows]),
                )
            }
        }
    }
    impl TransactionOps for Fake {
        fn query(&mut self, query: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            self.0.lock().unwrap().events.push(query.to_owned());
            self.record_write(query);
            let fail =
                query == "fail" || (self.0.lock().unwrap().fail_delete && query.contains("delete"));
            let typed = query == "typed-fail";
            let answer = if query.contains("fetch") {
                self.documents()
            } else {
                None
            }
            .unwrap_or_else(|| self.answer());
            Box::pin(async move {
                if typed {
                    Err(OrmError::QueryExecution(
                        "[CNT9] TypeDB query rejected: unique constraint".into(),
                    ))
                } else if fail {
                    Err(OrmError::QueryExecution("failure".into()))
                } else {
                    Ok(answer)
                }
            })
        }
        fn query_with_rows(
            &mut self,
            typeql: &str,
            rows: GivenRowsSpec,
        ) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            self.record_write(typeql);
            self.0.lock().unwrap().given_payloads.push(rows.clone());
            self.0.lock().unwrap().given_queries.push(typeql.to_owned());
            self.0
                .lock()
                .unwrap()
                .events
                .push(format!("given:{}", rows.rows.len()));
            let captured = if typeql.contains("fetch") {
                self.documents()
            } else {
                None
            };
            let answer = captured.unwrap_or_else(|| self.answer());
            Box::pin(async move { Ok(answer) })
        }
        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            let mut state = self.0.lock().unwrap();
            state.events.push("commit".into());
            let pending = state.pending_writes;
            state.visible_writes += pending;
            state.pending_writes = 0;
            Box::pin(async { Ok(()) })
        }
        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            let mut state = self.0.lock().unwrap();
            state.events.push("rollback".into());
            state.pending_writes = 0;
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
    #[test]
    fn implicit_timeout_applies_to_typed_plans_but_not_raw_queries() {
        let (mut raw, _) = fixture(TransactionOptions::default());
        raw.stage(StagedQuery::new("raw read", false)).unwrap();
        assert_eq!(raw.commit_timeout(), None);
        let (mut typed, _) = fixture(TransactionOptions::default());
        typed
            .stage_plan(&crate::query_plan_exec::race::touch())
            .unwrap();
        assert_eq!(typed.commit_timeout(), Some(Duration::from_secs(60)));
    }

    #[tokio::test(start_paused = true)]
    async fn absolute_commit_deadline_counts_time_spent_staging() {
        let call = crate::QueryExecutionDeadline::from_timeout_milliseconds(10);
        let (mut transaction, state) =
            fixture(TransactionOptions::default().with_timeout(crate::QueryTimeout::NONE));
        let pending = transaction.stage(StagedQuery::new("write", true)).unwrap();
        tokio::time::advance(Duration::from_millis(11)).await;
        let error = transaction.commit_with_deadline(call).await.unwrap_err();
        assert_eq!(
            error.diagnostic().code().as_str(),
            "transaction_deadline_exceeded"
        );
        assert!(pending.get().is_err());
        assert!(
            state.lock().unwrap().events.is_empty(),
            "expired call must not open or write"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn absolute_commit_deadline_only_tightens_and_none_is_unbounded() {
        let now = tokio::time::Instant::now().into_std();
        let (transaction, _) = fixture(
            TransactionOptions::default().with_timeout(crate::QueryTimeout::from_milliseconds(10)),
        );
        let looser = crate::QueryExecutionDeadline::from_timeout_milliseconds(100);
        assert_eq!(
            transaction.commit_deadline(looser).unwrap().instant(),
            Some(now + Duration::from_millis(10))
        );
        let tighter = crate::QueryExecutionDeadline::from_timeout_milliseconds(1);
        assert_eq!(
            transaction.commit_deadline(tighter).unwrap().instant(),
            tighter.instant()
        );
        let none = crate::QueryExecutionDeadline::from_timeout(crate::QueryTimeout::NONE);
        assert!(
            transaction
                .commit_deadline(none)
                .unwrap()
                .instant()
                .is_some()
        );
        let (unbounded, _) =
            fixture(TransactionOptions::default().with_timeout(crate::QueryTimeout::NONE));
        assert_eq!(unbounded.commit_deadline(none).unwrap().instant(), None);
        assert_eq!(
            unbounded.commit_deadline(tighter).unwrap().instant(),
            tighter.instant()
        );
    }

    fn fixture(options: TransactionOptions) -> (Transaction, Arc<Mutex<State>>) {
        let state = Arc::new(Mutex::new(State::default()));
        (
            Transaction::new(Arc::new(Fake(Arc::clone(&state))), "test".into(), options),
            state,
        )
    }
    fn iid_reference_stack(
        rows: usize,
    ) -> (
        Vec<StagedPlanV3>,
        Arc<type_bridge_contract::schema::DeclaredSchema>,
    ) {
        use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
        use type_bridge_contract::migration_assertion::{
            AssertionBinding, BindingId, QueryVariable,
        };
        use type_bridge_contract::query_plan::v3::{
            Operation, PlanSpec, QueryPlanV3, Set, Stage, WriteReturns,
        };
        use type_bridge_contract::query_plan::{
            InputColumn, InputColumnId, InputRow, QueryOperand, QueryOutput, QueryPattern,
        };
        use type_bridge_contract::query_remote_v3::StagedResultRefV3;
        use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};
        let schema = Arc::new(crate::query_plan_exec::race::schema());
        let account = TypeId::new(TypeKind::Entity, "account").unwrap();
        let root = BindingId::new(0).unwrap();
        let mut spec = PlanSpec {
            bindings: vec![AssertionBinding::new(
                root,
                QueryVariable::new("root").unwrap(),
            )],
            inputs: vec![],
            given_rows: None,
            operation: Operation::Rows,
            pipeline: vec![Stage::Match {
                patterns: vec![
                    QueryPattern::Isa {
                        binding: root,
                        type_id: account.clone(),
                        include_subtypes: false,
                    }
                    .into(),
                ],
            }],
            output: QueryOutput::Rows {
                columns: vec![root],
            },
            managed_semantics: crate::query_plan_exec::race::touch()
                .managed_semantics()
                .clone(),
            compatibility: None,
            functions: vec![],
        };
        let source = QueryPlanV3::new(spec.clone(), &schema).unwrap();
        spec.inputs = vec![InputColumn::new(
            InputColumnId::new(0),
            QueryVariable::new("target_iid").unwrap(),
            ValueTypeTag::String,
            false,
        )];
        let Stage::Match { patterns } = &mut spec.pipeline[0] else {
            unreachable!()
        };
        patterns.push(
            QueryPattern::Iid {
                binding: root,
                iid: QueryOperand::Input {
                    column: InputColumnId::new(0),
                },
            }
            .into(),
        );
        spec.pipeline.push(Stage::Update {
            sets: vec![Set::Field {
                owner: root,
                field: type_bridge_contract::schema::OwnsFactId::new(
                    account.clone(),
                    AttributeId::new("balance").unwrap(),
                )
                .unwrap(),
                operand: QueryOperand::Literal {
                    value: CanonicalValue::Long(7),
                },
            }],
        });
        spec.operation = Operation::Write {
            returns: WriteReturns::Rows,
        };
        let target = QueryPlanV3::new(spec, &schema).unwrap();
        (
            vec![
                StagedPlanV3::new(&source, vec![], &schema).unwrap(),
                StagedPlanV3::new_with_result_refs(
                    &target,
                    vec![InputRow::new(vec![None]); rows],
                    vec![StagedResultRefV3 {
                        input: InputColumnId::new(0),
                        source_plan: 0,
                        source_column: 0,
                        expected_types: vec![account],
                    }],
                    &schema,
                )
                .unwrap(),
            ],
            schema,
        )
    }

    fn iid_reference_answer(iids: &[&str]) -> Vec<serde_json::Value> {
        iids.iter().map(|iid| serde_json::json!({"root": type_bridge_contract::query_remote::RemoteValue::Thing {
            iid: (*iid).into(), type_id: type_bridge_contract::id::TypeId::new(type_bridge_contract::id::TypeKind::Entity, "account").unwrap(),
        }})).collect()
    }

    #[tokio::test]
    async fn staged_iid_refs_bind_broadcasts_and_zips_on_the_existing_commit_path() {
        for given in [false, true] {
            for (ids, templates) in [(vec!["0x10"], 3), (vec!["0x10", "0x20"], 2)] {
                let (mut tx, state) = fixture(Default::default());
                {
                    let mut state = state.lock().unwrap();
                    state.given = given;
                    state.row_values = Some(iid_reference_answer(&ids));
                }
                let (stack, schema) = iid_reference_stack(templates);
                let pending = tx.stage_plan_stack(stack, schema).unwrap();
                assert!(
                    pending
                        .iter()
                        .all(|answer| matches!(answer.get(), Err(PendingError::NotCommitted)))
                );
                tx.commit().await.unwrap();
                assert!(pending.iter().all(|answer| answer.get().is_ok()));
                let state = state.lock().unwrap();
                assert_eq!(state.kinds, [TxType::Write]);
                assert_eq!((state.visible_writes, state.pending_writes), (1, 0));
                if given {
                    let expected = (0..templates)
                        .map(|index| {
                            vec![super::super::backend::GivenValue::String(
                                ids[if ids.len() == 1 { 0 } else { index }].into(),
                            )]
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(state.given_payloads[0].rows, expected);
                } else {
                    assert!(state.given_payloads.is_empty());
                    for iid in ids {
                        assert!(state.events[1].contains(iid));
                    }
                }
                assert_eq!(&state.events[state.events.len() - 2..], ["commit", "close"]);
            }
        }
    }

    #[tokio::test]
    async fn a_missing_iid_ref_rolls_back_prior_writes_before_target_dispatch() {
        let (mut tx, state) = fixture(Default::default());
        state.lock().unwrap().given = true;
        tx.stage(StagedQuery::new("earlier-write", true)).unwrap();
        let (stack, schema) = iid_reference_stack(1);
        let pending = tx.stage_plan_stack(stack, schema).unwrap();
        let error = tx.commit().await.unwrap_err();
        assert_eq!(error.op_index, Some(2));
        assert_eq!(
            error.diagnostic().code().as_str(),
            "query_stage_result_ref_missing"
        );
        assert!(pending.iter().all(|answer| answer.get().is_err()));
        let state = state.lock().unwrap();
        assert!(state.given_payloads.is_empty());
        assert_eq!((state.visible_writes, state.pending_writes), (0, 0));
        assert_eq!(
            &state.events[state.events.len() - 2..],
            ["rollback", "close"]
        );
    }

    #[tokio::test]
    async fn a_later_failure_never_publishes_an_iid_ref_write() {
        let (mut tx, state) = fixture(Default::default());
        {
            let mut state = state.lock().unwrap();
            state.given = true;
            state.row_values = Some(iid_reference_answer(&["0x10"]));
        }
        let (stack, schema) = iid_reference_stack(1);
        let pending = tx.stage_plan_stack(stack, schema).unwrap();
        tx.stage(StagedQuery::new("fail", true)).unwrap();
        assert_eq!(tx.commit().await.unwrap_err().op_index, Some(2));
        assert!(pending.iter().all(|answer| answer.get().is_err()));
        let state = state.lock().unwrap();
        assert_eq!((state.visible_writes, state.pending_writes), (0, 0));
        assert_eq!(
            &state.events[state.events.len() - 3..],
            ["fail", "rollback", "close"]
        );
    }

    #[tokio::test]
    async fn proxy_iid_refs_preserve_authored_bytes_and_rebase_after_earlier_plans() {
        use type_bridge_contract::query_remote::{
            RemoteCapabilities, RemoteExecutorBinding, RemoteValue,
        };
        use type_bridge_contract::query_remote_v3::{
            CAP_QUERY_REMOTE_STAGED_V3, CAP_QUERY_REMOTE_WRITE_V3, CAP_QUERY_STAGE_RESULT_REF,
            StagedAnswerV3, StagedOutcomeV3, StagedReplyV3, StagedRequestSpecV3,
        };
        for prefix in [false, true] {
            let (stack, schema) = iid_reference_stack(3);
            let mut expected = Vec::new();
            if prefix {
                expected.push(stack[0].clone());
            }
            expected.extend(
                stack
                    .iter()
                    .cloned()
                    .map(|entry| entry.rebase_result_refs(u32::from(prefix)).unwrap()),
            );
            let mut capabilities =
                type_bridge_contract::query_plan::v3::query_plan_v3_capability_vocabulary();
            for cap in [
                CAP_QUERY_REMOTE_STAGED_V3,
                CAP_QUERY_REMOTE_WRITE_V3,
                CAP_QUERY_STAGE_RESULT_REF,
            ] {
                capabilities
                    .insert(type_bridge_contract::capability::CapabilityId::new(cap).unwrap());
            }
            let signer = Arc::new(
                crate::query_v2_remote::RemoteReplySigningKey::from_secret_bytes([0x46; 32]),
            );
            let advertisement = RemoteCapabilities::new(
                capabilities,
                RemoteExecutorBinding::new("proxy-iid-ref-tests", "epoch-00000000001").unwrap(),
                signer.public_key(),
            )
            .with_query_limits(type_bridge_contract::limits::QueryLimits::V3)
            .unwrap();
            let ad_bytes = advertisement.encode().unwrap();
            let calls = Arc::new(Mutex::new(0));
            let recorded = Arc::clone(&calls);
            let captured_schema = Arc::clone(&schema);
            let exchange: super::super::staged_proxy::StagedExchangeFn = Arc::new(move |bytes| {
                let schema = Arc::clone(&captured_schema);
                let ad = advertisement.clone();
                let signer = Arc::clone(&signer);
                let expected = expected.clone();
                let recorded = Arc::clone(&recorded);
                Box::pin(async move {
                    *recorded.lock().unwrap() += 1;
                    let request = StagedRequestV3::decode(&bytes, &schema, &ad).unwrap();
                    let manual = StagedRequestV3::new(
                        StagedRequestSpecV3 {
                            nonce: request.nonce().to_owned(),
                            kind: Some(request.kind()),
                            limits: request.limits(),
                            prepared_at_unix_ms: request.expires_at_unix_ms()
                                - request.limits().admission_window_milliseconds,
                            plans: expected,
                        },
                        &schema,
                        &ad,
                    )
                    .unwrap();
                    assert_eq!(bytes, manual.encode().unwrap());
                    assert_eq!(
                        request.plans().last().unwrap().result_refs()[0].source_plan,
                        u32::from(prefix)
                    );
                    assert!(
                        request
                            .plans()
                            .last()
                            .unwrap()
                            .rows()
                            .iter()
                            .all(|row| row.values()[0].is_none())
                    );
                    let answers = request
                        .plans()
                        .iter()
                        .map(|_| StagedAnswerV3::Rows {
                            rows: vec![vec![RemoteValue::Thing {
                                iid: "0x10".into(),
                                type_id: type_bridge_contract::id::TypeId::new(
                                    type_bridge_contract::id::TypeKind::Entity,
                                    "account",
                                )
                                .unwrap(),
                            }]],
                        })
                        .collect();
                    Ok(
                        StagedReplyV3::new(&request, StagedOutcomeV3::Committed, answers)
                            .unwrap()
                            .encode_signed(&request, &ad, signer.as_ref())
                            .unwrap(),
                    )
                })
            });
            let mut tx = Transaction::over_proxy(
                schema.as_ref().clone(),
                &ad_bytes,
                exchange,
                Default::default(),
            )
            .unwrap();
            let earlier = prefix.then(|| tx.stage_plan(stack[0].plan()).unwrap());
            let pending = tx.stage_plan_stack(stack, schema).unwrap();
            assert!(
                pending
                    .iter()
                    .all(|answer| matches!(answer.get(), Err(PendingError::NotCommitted)))
            );
            tx.commit().await.unwrap();
            assert_eq!(*calls.lock().unwrap(), 1);
            assert!(pending.iter().all(|answer| answer.get().is_ok()));
            if let Some(earlier) = earlier {
                assert!(earlier.get().is_ok());
            }
        }
    }

    #[test]
    fn invalid_iid_ref_graph_never_opens_or_leaves_a_partial_staged_block() {
        let (mut tx, state) = fixture(Default::default());
        let (mut stack, schema) = iid_reference_stack(1);
        let invalid = stack.pop().unwrap().rebase_result_refs(1).unwrap();
        stack.push(invalid);
        let error = tx.stage_plan_stack(stack, schema).err().unwrap();
        let PendingError::InvalidPlan(diagnostic) = error else {
            panic!("invalid result reference graph")
        };
        assert_eq!(diagnostic.code().as_str(), "query_stage_result_ref_order");
        assert!(tx.stack.0.is_empty());
        assert!(state.lock().unwrap().events.is_empty());
        assert!(state.lock().unwrap().kinds.is_empty());
    }

    fn model_fetch_stack(
        registry: &DescriptorRegistry,
    ) -> Arc<crate::match_request::PlanFetchStack> {
        use type_bridge_contract::id::{TypeId, TypeKind};
        use type_bridge_contract::query_plan::v3::PlanBuilder;
        let authority =
            crate::match_request::descriptor_plan::MatchRequestAdapterAuthority::from_registry(
                registry,
            )
            .unwrap();
        let mut builder = PlanBuilder::new();
        let thing = builder.exact(TypeId::new(TypeKind::Entity, "person").unwrap());
        builder.fetch(thing, vec![]);
        let plan = builder
            .build(
                authority.declared(),
                authority
                    .context()
                    .managed_state()
                    .managed_semantic_schema()
                    .clone(),
            )
            .unwrap();
        Arc::new(
            crate::match_request::build_plan_fetch_stack(
                registry,
                &plan,
                &[(
                    thing,
                    crate::match_request::DescriptorId::new("entity:person"),
                )],
            )
            .unwrap(),
        )
    }

    #[tokio::test]
    async fn fetch_groups_resolve_before_commit_and_publish_one_owned_proof() {
        for fail in [false, true] {
            let registry = Arc::new(DescriptorRegistry::new());
            registry
                .register_entity(crate::_descriptor::EntityDescriptor {
                    type_name: "person".into(),
                    is_abstract: false,
                    parent_type: None,
                    owned_attributes: vec![],
                    doc: None,
                    meta: Default::default(),
                })
                .unwrap();
            let source = model_fetch_stack(&registry);
            let (mut tx, state) = fixture(TransactionOptions::default());
            state.lock().unwrap().snapshot_documents = Some(vec![
                serde_json::json!({"_iid":"0x01", "_type":"person", "attributes":{}}),
            ]);
            let write = tx.stage(StagedQuery::new("earlier-write", true)).unwrap();
            let first = tx
                .stage_plan_fetch(
                    Arc::clone(&source),
                    Arc::clone(&registry),
                    Default::default(),
                )
                .unwrap();
            let second = tx
                .stage_plan_fetch(
                    Arc::clone(&source),
                    Arc::clone(&registry),
                    crate::QueryExecutionResourceLimits {
                        items: if fail { 0 } else { 10 },
                        ..Default::default()
                    },
                )
                .unwrap();
            for item in &tx.stack.0[1..] {
                let Slot::Plan { plan, .. } = &item.slot else {
                    panic!("plan slot")
                };
                assert!(std::ptr::eq(plan.plan().unwrap(), &source.plans()[0]));
            }
            crate::query_plan_exec::take_document_validation_count();
            assert!(matches!(first.get(), Err(PendingError::NotCommitted)));
            let outcome = tx.commit().await;
            assert_eq!(crate::query_plan_exec::take_document_validation_count(), 2);
            let events = state.lock().unwrap().events.clone();
            if fail {
                assert_eq!(
                    outcome.unwrap_err().diagnostic().code().as_str(),
                    "processed_item_limit"
                );
                assert_eq!(&events[3..], ["rollback", "close"]);
                for pending in [&first, &second] {
                    let error = pending.get().err().unwrap();
                    assert_eq!(
                        error.diagnostic().unwrap().code().as_str(),
                        "processed_item_limit"
                    );
                }
                assert!(write.get().is_err());
            } else {
                outcome.unwrap();
                assert_eq!(&events[3..], ["commit", "close"]);
                assert!(write.get().is_ok());
                let proof = first.get().unwrap();
                assert!(Arc::ptr_eq(&proof, &first.get().unwrap()));
                assert_eq!(proof.model_rows(), &[0]);
                assert_eq!(
                    proof.model_ordinal(
                        0,
                        type_bridge_contract::migration_assertion::BindingId::new(0).unwrap()
                    ),
                    Some((0, 0))
                );
                assert!(second.get().is_ok());
                tx.commit().await.unwrap();
                assert!(Arc::ptr_eq(&proof, &first.get().unwrap()));
            }
        }
    }

    #[tokio::test]
    async fn bulk_correlation_runs_once_per_answer_for_direct_and_verified_proxy_replies() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tracing::instrument::WithSubscriber;
        use type_bridge_contract::id::{TypeId, TypeKind};
        use type_bridge_contract::query_plan::v3::PlanBuilder;
        for proxy in [false, true] {
            let (remote, registry, _) = proxy_match_fixture(Default::default(), false, None);
            let authority =
                crate::match_request::descriptor_plan::MatchRequestAdapterAuthority::from_registry(
                    &registry,
                )
                .unwrap();
            let mut builder = PlanBuilder::new();
            let person = builder.insert(TypeId::new(TypeKind::Entity, "person").unwrap());
            builder
                .bind_model_rows(person, vec![vec![], vec![]])
                .fetch_held(person, vec![]);
            let plans = Arc::new(
                builder
                    .build_plans(
                        authority.declared(),
                        authority
                            .context()
                            .managed_state()
                            .managed_semantic_schema()
                            .clone(),
                        false,
                        type_bridge_contract::split::SplitLimits::CANONICAL,
                    )
                    .unwrap(),
            );
            assert_eq!(plans.plans().len(), 2);
            let mut tx = if proxy {
                remote
            } else {
                let (tx, state) = fixture(Default::default());
                state.lock().unwrap().snapshot_documents = Some(vec![
                    serde_json::json!({"_iid":"0x01", "_type":"person", "attributes":{}}),
                ]);
                tx
            };
            let pending = tx.stage_plans(Arc::clone(&plans), true).unwrap();
            let count = Arc::new(AtomicUsize::new(0));
            let observed = Arc::clone(&count);
            let dispatch = crate::debug::dispatch(Arc::new(move |event| {
                if event.name == "type_bridge.plan.correlate" {
                    observed.fetch_add(1, Ordering::Relaxed);
                }
            }));
            tx.commit().with_subscriber(dispatch.clone()).await.unwrap();
            assert_eq!(count.load(Ordering::Relaxed), 2, "proxy={proxy}");
            let answers = pending
                .iter()
                .map(Pending::get)
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            let correlations = pending
                .iter()
                .map(Pending::correlation)
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert!(correlations.iter().all(Option::is_some));
            let merged = tracing::dispatcher::with_default(&dispatch, || {
                merge_plan_answers(&plans, &answers, &correlations)
            })
            .unwrap();
            assert!(
                matches!(merged.as_ref(), PlanAnswer::Documents(documents) if documents.len() == 2)
            );
            assert_eq!(
                count.load(Ordering::Relaxed),
                2,
                "merging repeated correlation, proxy={proxy}"
            );
        }
    }

    #[tokio::test]
    async fn bulk_fetch_group_correlates_once_before_commit_and_empty_bulk_uses_no_provider() {
        use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
        use type_bridge_contract::query_plan::v3::PlanBuilder;
        use type_bridge_contract::schema::OwnsFactId;
        use type_bridge_contract::value::{CanonicalString, CanonicalValue};
        for empty in [false, true] {
            let (registry, _) =
                crate::query_v2_adapter_tests::generic_model_selection(&[0, 1], false);
            let authority =
                crate::match_request::descriptor_plan::MatchRequestAdapterAuthority::from_registry(
                    &registry,
                )
                .unwrap();
            let mut builder = PlanBuilder::new();
            let type_id = TypeId::new(TypeKind::Entity, "person").unwrap();
            let thing = builder.insert(type_id.clone());
            let field = OwnsFactId::new(type_id, AttributeId::new("person-name").unwrap()).unwrap();
            let names = if empty {
                vec![]
            } else {
                vec!["Alice", "Bob", "Carol"]
            };
            builder.bind_model_rows(
                thing,
                names
                    .iter()
                    .map(|name| {
                        vec![(
                            field.clone(),
                            CanonicalValue::String(CanonicalString::new(*name).unwrap()),
                        )]
                    })
                    .collect(),
            );
            builder.fetch_held(thing, vec![]);
            let built = builder
                .build_plans(
                    authority.declared(),
                    authority
                        .context()
                        .managed_state()
                        .managed_semantic_schema()
                        .clone(),
                    false,
                    type_bridge_contract::split::SplitLimits::new(2, usize::MAX),
                )
                .unwrap();
            let source = Arc::new(
                crate::match_request::build_bulk_plan_fetch_stack(
                    &registry,
                    built,
                    (
                        thing,
                        crate::match_request::DescriptorId::new("entity:person"),
                    ),
                )
                .unwrap(),
            );
            assert_eq!(source.plans().len(), if empty { 0 } else { 2 });
            let (mut tx, state) = fixture(Default::default());
            state.lock().unwrap().given = true;
            let document = |iid, name| serde_json::json!({"_iid":iid,"_type":"person","attributes":{"person-name":name}});
            state.lock().unwrap().snapshot_batches = [
                vec![document("0x02", "Bob"), document("0x01", "Alice")],
                vec![document("0x03", "Carol")],
            ]
            .into();
            let pending = tx
                .stage_plan_fetch(source, registry, Default::default())
                .unwrap();
            assert!(matches!(pending.get(), Err(PendingError::NotCommitted)));
            crate::query_plan_exec::take_document_validation_count();
            tx.commit().await.unwrap();
            assert_eq!(
                crate::query_plan_exec::take_document_validation_count(),
                names.len()
            );
            let result = pending.get().unwrap();
            let PlanAnswer::Documents(documents) = result.selection() else {
                panic!("model documents")
            };
            assert_eq!(documents.len(), names.len());
            assert_eq!(result.model_rows().len(), names.len());
            for (document, name) in documents.iter().zip(names) {
                assert_eq!(document["attributes"]["person-name"], name);
            }
            if empty {
                assert!(state.lock().unwrap().events.is_empty());
            } else {
                assert_eq!(&state.lock().unwrap().events[2..], ["commit", "close"]);
            }
        }
    }

    #[tokio::test]
    async fn authored_ordered_model_fetch_refuses_population_before_commit_and_rolls_back_all_writes()
     {
        use crate::_descriptor::{OwnedAttributeDescriptor, RoleDescriptor, TypeDescriptor};
        use crate::match_request::{DescriptorId, MatchResult, SlotValue};
        use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
        use type_bridge_contract::query_plan::QueryOperand;
        use type_bridge_contract::query_plan::v3::{DocumentRole, PlanBuilder};
        use type_bridge_contract::schema::OwnsFactId;
        use type_bridge_contract::value::CanonicalValue;
        for ordered in [false, true] {
            for populated in [false, true] {
                let (original, _) =
                    crate::query_v2_adapter_tests::generic_model_selection(&[0], false);
                let registry = Arc::new(DescriptorRegistry::new());
                for descriptor in original.snapshot() {
                    match descriptor {
                        TypeDescriptor::Entity(entity) => {
                            registry.register_entity(entity).unwrap();
                        }
                        TypeDescriptor::Relation(mut relation) => {
                            if relation.type_name == "employment" {
                                relation.roles[0].ordered = ordered;
                                relation.roles.push(RoleDescriptor {
                                    role_name: "anchor".into(),
                                    player_type_names: vec!["person".into()],
                                    cardinality: Some((1, Some(1))),
                                    ..Default::default()
                                });
                                relation.owned_attributes.push(OwnedAttributeDescriptor {
                                    field_name: "status".into(),
                                    attr_name: "status".into(),
                                    value_type: crate::ValueType::Long,
                                    annotations: vec![],
                                    is_optional: true,
                                    is_ordered: false,
                                    doc: None,
                                    meta: Default::default(),
                                });
                            }
                            registry.register_relation(relation).unwrap();
                        }
                    }
                }
                let authority = crate::match_request::descriptor_plan::MatchRequestAdapterAuthority::from_registry(&registry).unwrap();
                let employment = TypeId::new(TypeKind::Relation, "employment").unwrap();
                let mut builder = PlanBuilder::new();
                let thing = builder.exact(employment.clone());
                builder.update(
                    thing,
                    OwnsFactId::new(employment, AttributeId::new("status").unwrap()).unwrap(),
                    QueryOperand::Literal {
                        value: CanonicalValue::Long(2),
                    },
                );
                builder.fetch_held(
                    thing,
                    ["anchor", "worker"]
                        .into_iter()
                        .map(|name| {
                            DocumentRole::new(RoleId::new("employment", name).unwrap(), false)
                        })
                        .collect(),
                );
                let plan = builder
                    .build(
                        authority.declared(),
                        authority
                            .context()
                            .managed_state()
                            .managed_semantic_schema()
                            .clone(),
                    )
                    .unwrap();
                plan.validate_model_projection().unwrap();
                assert!(plan.writes());
                let source = Arc::new(
                    crate::match_request::build_plan_fetch_stack(
                        &registry,
                        &plan,
                        &[(thing, DescriptorId::new("relation:employment"))],
                    )
                    .unwrap(),
                );
                assert_eq!(source.plans().len(), 1);
                let (mut tx, state) = fixture(Default::default());
                let mut document = serde_json::json!({
                    "_iid":"0x20", "_type":"employment", "attributes":{"status":2},
                    "_role_0_iid":"0x01", "_role_0_type":"person", "_role_0_attributes":{"person-name":"Alice"},
                    "_role_1_iid":null, "_role_1_type":null, "_role_1_attributes":null,
                });
                if populated {
                    document["_role_1_iid"] = serde_json::json!("0x02");
                    document["_role_1_type"] = serde_json::json!("person");
                    document["_role_1_attributes"] = serde_json::json!({"person-name":"Bob"});
                }
                state.lock().unwrap().snapshot_documents = Some(vec![document]);
                let earlier = tx.stage(StagedQuery::new("earlier-write", true)).unwrap();
                let pending = tx
                    .stage_plan_fetch(source, registry, Default::default())
                    .unwrap();
                assert!(matches!(pending.get(), Err(PendingError::NotCommitted)));
                let outcome = tx.commit().await;
                let recorded = state.lock().unwrap();
                assert_eq!(recorded.kinds, [TxType::Write]);
                assert_eq!(recorded.pending_writes, 0);
                if ordered && populated {
                    let error = outcome.unwrap_err();
                    assert_eq!(
                        error.diagnostic().code().as_str(),
                        "query_plan_hydration_ordered_role"
                    );
                    assert_eq!(recorded.visible_writes, 0);
                    assert!(!recorded.events.iter().any(|event| event == "commit"));
                    assert_eq!(
                        &recorded.events[recorded.events.len() - 2..],
                        ["rollback", "close"]
                    );
                    assert!(earlier.get().is_err());
                    assert_eq!(
                        pending
                            .get()
                            .err()
                            .unwrap()
                            .diagnostic()
                            .unwrap()
                            .code()
                            .as_str(),
                        "query_plan_hydration_ordered_role"
                    );
                } else {
                    outcome.unwrap();
                    assert_eq!(recorded.visible_writes, 2);
                    assert_eq!(
                        &recorded.events[recorded.events.len() - 2..],
                        ["commit", "close"]
                    );
                    earlier.get().unwrap();
                    let result = pending.get().unwrap();
                    let MatchResult::Rows { rows } =
                        result.result().for_request(result.request()).unwrap()
                    else {
                        panic!("model rows")
                    };
                    let SlotValue::One(model) = &rows[0].slots()[0] else {
                        panic!("one model")
                    };
                    let workers = model
                        .roles()
                        .iter()
                        .find(|role| role.role().name == "worker")
                        .unwrap()
                        .players();
                    assert_eq!(workers.len(), usize::from(populated));
                }
            }
        }
    }

    #[tokio::test]
    async fn mixed_fetch_group_seals_duplicates_and_rejects_unmatched_documents_before_commit() {
        use type_bridge_contract::id::{TypeId, TypeKind};
        use type_bridge_contract::migration_assertion::BindingId;
        use type_bridge_contract::query_remote::RemoteValue;
        use type_bridge_contract::value::CanonicalValue;
        for fail in [false, true] {
            let (registry, plan) =
                crate::query_v2_adapter_tests::generic_model_selection(&[0, 1], false);
            let source = Arc::new(
                crate::match_request::build_plan_fetch_stack(
                    &registry,
                    &plan,
                    &[(
                        BindingId::new(0).unwrap(),
                        crate::match_request::DescriptorId::new("entity:person"),
                    )],
                )
                .unwrap(),
            );
            assert!(source.plans().len() > 1);
            let (mut tx, state) = fixture(Default::default());
            let row = serde_json::json!({
                "b0": RemoteValue::Thing { iid: "0x01".into(), type_id: TypeId::new(TypeKind::Entity, "person").unwrap() },
                "b1": RemoteValue::Value { value: CanonicalValue::Long(7) },
            });
            state.lock().unwrap().row_values = Some(vec![row.clone(), row]);
            state.lock().unwrap().snapshot_documents = Some(vec![serde_json::json!({
                "_iid": if fail { "0x02" } else { "0x01" }, "_type":"person", "attributes":{"person-name":"Alice"}
            })]);
            let write = tx.stage(StagedQuery::new("earlier-write", true)).unwrap();
            let pending = tx
                .stage_plan_fetch(source, registry, Default::default())
                .unwrap();
            crate::query_plan_exec::take_document_validation_count();
            let outcome = tx.commit().await;
            assert_eq!(crate::query_plan_exec::take_document_validation_count(), 1);
            let events = state.lock().unwrap().events.clone();
            if fail {
                let error = outcome.unwrap_err();
                assert_eq!(
                    error.diagnostic().code().as_str(),
                    "selected_root_set_mismatch"
                );
                assert_eq!(&events[events.len() - 2..], ["rollback", "close"]);
                assert!(write.get().is_err());
                assert_eq!(
                    pending
                        .get()
                        .err()
                        .unwrap()
                        .diagnostic()
                        .unwrap()
                        .code()
                        .as_str(),
                    "selected_root_set_mismatch"
                );
            } else {
                outcome.unwrap();
                assert_eq!(&events[events.len() - 2..], ["commit", "close"]);
                let result = pending.get().unwrap();
                assert_eq!(result.model_rows(), &[0, 0]);
                assert_eq!(result.columns().len(), 2);
                assert!(matches!(
                    result.cell(1, BindingId::new(0).unwrap()),
                    Some(crate::match_request::PlanFetchCell::Model { row: 0, slot: 0 })
                ));
                assert!(matches!(
                    result.cell(1, BindingId::new(1).unwrap()),
                    Some(crate::match_request::PlanFetchCell::Scalar(
                        CanonicalValue::Long(7)
                    ))
                ));
                assert!(result.cell(2, BindingId::new(1).unwrap()).is_none());

                assert_eq!(
                    result.model_ordinal(1, BindingId::new(0).unwrap()),
                    Some((0, 0))
                );
                assert!(matches!(result.selection(), PlanAnswer::Rows(rows) if rows.len() == 2));
            }
        }
    }

    #[tokio::test]
    async fn proxy_fetch_group_limits_reach_the_adapter_before_commit() {
        let (mut tx, registry, committed) = proxy_match_fixture(Default::default(), false, None);
        let registry = Arc::new(registry);
        let source = model_fetch_stack(&registry);
        let first = tx
            .stage_plan_fetch(
                Arc::clone(&source),
                Arc::clone(&registry),
                Default::default(),
            )
            .unwrap();
        let pending = tx
            .stage_plan_fetch(
                source,
                registry,
                crate::QueryExecutionResourceLimits {
                    items: 1,
                    ..Default::default()
                },
            )
            .unwrap();
        let error = tx.commit().await.unwrap_err();
        assert_eq!(error.diagnostic().code().as_str(), "processed_item_limit");
        assert_eq!(*committed.lock().unwrap(), 0);
        assert!(first.get().is_err());
        assert_eq!(
            pending
                .get()
                .err()
                .unwrap()
                .diagnostic()
                .unwrap()
                .code()
                .as_str(),
            "processed_item_limit"
        );
    }

    #[tokio::test]
    async fn proxy_fetch_group_default_statements_do_not_cap_the_whole_transaction() {
        for statements in [None, Some(3)] {
            let (mut tx, registry, committed) =
                proxy_match_fixture(Default::default(), false, None);
            let registry = Arc::new(registry);
            let source = model_fetch_stack(&registry);
            let pending = (0..4)
                .map(|_| {
                    tx.stage_plan_fetch(
                        Arc::clone(&source),
                        Arc::clone(&registry),
                        crate::QueryExecutionResourceLimits {
                            statements,
                            ..Default::default()
                        },
                    )
                    .unwrap()
                })
                .collect::<Vec<_>>();
            let outcome = tx.commit().await;
            if statements.is_none() {
                outcome.unwrap();
                assert_eq!(*committed.lock().unwrap(), 1);
                for answer in pending {
                    assert_eq!(answer.get().unwrap().model_rows(), &[0]);
                }
            } else {
                assert_eq!(
                    outcome.unwrap_err().diagnostic().code().as_str(),
                    "statement_count_limit"
                );
                assert_eq!(*committed.lock().unwrap(), 0);
                for answer in pending {
                    assert_eq!(
                        answer
                            .get()
                            .err()
                            .unwrap()
                            .diagnostic()
                            .unwrap()
                            .code()
                            .as_str(),
                        "statement_count_limit"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn proxy_fetch_groups_publish_together_or_leave_all_pending_unknown() {
        for fail in [false, true] {
            let (mut tx, registry, committed) = proxy_match_fixture(Default::default(), fail, None);
            let registry = Arc::new(registry);
            let source = model_fetch_stack(&registry);
            let ordinary = tx
                .stage_match(&registry, proxy_count_request(&registry))
                .unwrap();
            let first = tx
                .stage_plan_fetch(
                    Arc::clone(&source),
                    Arc::clone(&registry),
                    Default::default(),
                )
                .unwrap();
            let second = tx
                .stage_plan_fetch(
                    source,
                    registry,
                    crate::QueryExecutionResourceLimits {
                        items: 10,
                        ..Default::default()
                    },
                )
                .unwrap();
            let outcome = tx.commit().await;
            assert_eq!(*committed.lock().unwrap(), 1);
            if fail {
                assert_eq!(
                    outcome.unwrap_err().diagnostic().code().as_str(),
                    "commit_outcome_unknown"
                );
                assert_eq!(
                    ordinary
                        .get()
                        .err()
                        .unwrap()
                        .diagnostic()
                        .unwrap()
                        .code()
                        .as_str(),
                    "commit_outcome_unknown"
                );
                for pending in [&first, &second] {
                    assert_eq!(
                        pending
                            .get()
                            .err()
                            .unwrap()
                            .diagnostic()
                            .unwrap()
                            .code()
                            .as_str(),
                        "commit_outcome_unknown"
                    );
                }
            } else {
                outcome.unwrap();
                assert!(ordinary.get().is_ok());
                let proof = first.get().unwrap();
                assert!(Arc::ptr_eq(&proof, &first.get().unwrap()));
                assert_eq!(proof.model_rows(), &[0]);
                assert!(second.get().is_ok());
            }
        }
    }

    #[test]
    fn fetch_group_rollback_close_and_drop_resolve_every_pending() {
        for action in [0, 1, 2] {
            let registry = Arc::new(DescriptorRegistry::new());
            registry
                .register_entity(crate::_descriptor::EntityDescriptor {
                    type_name: "person".into(),
                    is_abstract: false,
                    parent_type: None,
                    owned_attributes: vec![],
                    doc: None,
                    meta: Default::default(),
                })
                .unwrap();
            let source = model_fetch_stack(&registry);
            let (mut tx, state) = fixture(Default::default());
            let pending = tx
                .stage_plan_fetch(source, registry, Default::default())
                .unwrap();
            match action {
                0 => tx.rollback().unwrap(),
                1 => tx.close().unwrap(),
                _ => drop(tx),
            }
            assert!(matches!(
                pending.get(),
                Err(PendingError::RolledBack | PendingError::Closed)
            ));
            assert!(state.lock().unwrap().events.is_empty());
        }
    }

    #[test]
    fn shared_plan_stage_retains_the_original_batch_allocation() {
        let (mut tx, state) = fixture(TransactionOptions::default());
        let source = Arc::new(vec![crate::query_plan_exec::race::stock_read(
            type_bridge_contract::query_plan::v3::Operation::Rows,
        )]);
        tx.stage_shared_plan(Arc::clone(&source), 0).unwrap();
        assert_eq!(Arc::strong_count(&source), 2);
        let Slot::Plan { plan, .. } = &tx.stack.0[0].slot else {
            panic!("plan slot");
        };
        assert!(std::ptr::eq(plan.plan().unwrap(), &source[0]));
        assert!(state.lock().unwrap().events.is_empty());
        assert!(matches!(
            tx.stage_shared_plan(Arc::clone(&source), 1),
            Err(PendingError::InvalidPlan(_))
        ));
        assert_eq!(Arc::strong_count(&source), 2);
    }

    #[test]
    fn authored_batch_retains_the_original_source_for_every_chunk() {
        let (mut tx, state) = fixture(TransactionOptions::default());
        let source = Arc::new(vec![
            crate::query_plan_exec::race::stock_read(
                type_bridge_contract::query_plan::v3::Operation::Rows,
            ),
            crate::query_plan_exec::race::stock_read(
                type_bridge_contract::query_plan::v3::Operation::Count,
            ),
        ]);
        assert_eq!(tx.stage_plans(Arc::clone(&source), false).unwrap().len(), 2);
        assert_eq!(Arc::strong_count(&source), 3);
        for (index, item) in tx.stack.0.iter().enumerate() {
            let Slot::Plan { plan, .. } = &item.slot else {
                panic!("plan slot");
            };
            assert!(std::ptr::eq(plan.plan().unwrap(), &source[index]));
        }
        assert!(state.lock().unwrap().events.is_empty());
    }

    #[test]
    fn shared_snapshot_stage_reuses_the_original_plan_for_capture_validation() {
        let (mut tx, state) = fixture(TransactionOptions::default());
        let source = Arc::new(crate::query_plan_exec::race::delete_doctors_returning());
        tx.stage_shared_plan(Arc::clone(&source), 0).unwrap();
        let item = &tx.stack.0[0];
        let (_, capture) = item.query.as_ref().unwrap().snapshot.as_ref().unwrap();
        assert!(std::ptr::eq(capture.plan().unwrap(), source.as_ref()));
        let Slot::Plan { plan, .. } = &item.slot else {
            panic!("plan slot");
        };
        assert!(std::ptr::eq(plan.plan().unwrap(), source.as_ref()));
        assert_eq!(Arc::strong_count(&source), 3);
        assert!(state.lock().unwrap().events.is_empty());
    }

    #[tokio::test]
    async fn shared_plan_source_changes_refuse_before_opening_the_provider() {
        struct Switching {
            changed: std::sync::atomic::AtomicBool,
            original: Vec<QueryPlanV3>,
        }
        impl AsRef<[QueryPlanV3]> for Switching {
            fn as_ref(&self) -> &[QueryPlanV3] {
                if self.changed.load(std::sync::atomic::Ordering::SeqCst) {
                    &[]
                } else {
                    &self.original
                }
            }
        }
        let (mut tx, state) = fixture(TransactionOptions::default());
        let source = Arc::new(Switching {
            changed: std::sync::atomic::AtomicBool::new(false),
            original: vec![crate::query_plan_exec::race::stock_read(
                type_bridge_contract::query_plan::v3::Operation::Rows,
            )],
        });
        let pending = tx.stage_shared_plan(Arc::clone(&source), 0).unwrap();
        source
            .changed
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(tx.commit().await.is_err());
        assert!(pending.get().is_err());
        assert!(state.lock().unwrap().events.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn interrupted_write_diagnostics_keep_phases_and_roll_back() {
        for collect in [false, true] {
            for cancel in [false, true] {
                let cancellation = super::super::backend::AnswerCancellation::default();
                let trigger = cancellation.clone();
                let (mut tx, state) = fixture(
                    TransactionOptions::default()
                        .with_timeout(crate::QueryTimeout::from_milliseconds(20))
                        .with_collect_phase_timestamps(collect)
                        .with_answer_limits(BoundedAnswerLimits {
                            cancellation,
                            ..Default::default()
                        }),
                );
                let first = tx.stage(StagedQuery::new("earlier-write", true)).unwrap();
                let stalled: Program<()> = Box::new(move |_, _| {
                    if cancel {
                        trigger.cancel();
                    }
                    Box::pin(std::future::pending::<Result<(), OrmError>>())
                });
                tx.stage_program(stalled).unwrap();
                let error = tx.commit().await.unwrap_err();
                let host = serde_json::to_value(error.diagnostic().host_json()).unwrap();
                assert_eq!(
                    host["code"],
                    if cancel {
                        "provider_cancelled"
                    } else {
                        "transaction_deadline_exceeded"
                    }
                );
                assert_eq!(host["details"]["execution_phase"]["value"], "db_write");
                assert_eq!(
                    host["details"]["elapsed_milliseconds"]["value"],
                    if cancel { "0" } else { "20" }
                );
                let timeline = host["details"].get("execution_timeline");
                assert_eq!(timeline.is_some(), collect);
                if let Some(timeline) = timeline {
                    let stamps = timeline["value"]["stamps"].as_array().unwrap();
                    assert_eq!(
                        stamps
                            .iter()
                            .map(|stamp| stamp["phase"].as_str().unwrap())
                            .collect::<Vec<_>>(),
                        [
                            "fired",
                            "accepted",
                            "execution_started",
                            "db_write",
                            "failed"
                        ]
                    );
                    assert_eq!(stamps[3]["statement_index"], 0);
                    assert_eq!(stamps[4]["failing_phase"], "db_write");
                    assert_eq!(
                        stamps[4]["offset_milliseconds"],
                        if cancel { 0 } else { 20 }
                    );
                }
                let pending = first.get().unwrap_err();
                let pending_host =
                    serde_json::to_value(pending.diagnostic().unwrap().host_json()).unwrap();
                assert_eq!(
                    pending_host["details"]["execution_phase"]["value"],
                    "db_write"
                );
                let state = state.lock().unwrap();
                assert_eq!(state.events, ["earlier-write", "rollback", "close"]);
                assert_eq!(state.pending_writes, 0);
                assert_eq!(state.visible_writes, 0);
            }
        }
    }

    #[tokio::test]
    async fn plan_trace_records_seams_and_metering_without_given_values() {
        use tracing::instrument::WithSubscriber;
        let records = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&records);
        let dispatch = crate::debug::dispatch(Arc::new(move |event| {
            captured.lock().unwrap().push(event.clone())
        }));
        async {
            let (mut tx, state) = fixture(TransactionOptions::default());
            state.lock().unwrap().given = true;
            state.lock().unwrap().rows = 2;
            for order in ["TRACE_SECRET", "OTHER_SECRET"] {
                let plan = crate::query_plan_exec::race::purchase(order, "I1", "A1");
                tx.stage_plan(&plan).unwrap();
                tx.commit().await.unwrap();
            }
        }
        .with_subscriber(dispatch)
        .await;
        let records = records.lock().unwrap();
        let phases = records
            .iter()
            .filter(|record| record.name == "type_bridge.plan.phase")
            .collect::<Vec<_>>();
        assert_eq!(phases.len(), 12);
        for call in phases.chunks(6) {
            assert_eq!(
                call.iter()
                    .map(|record| record.fields["phase"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                [
                    "fired",
                    "accepted",
                    "execution_started",
                    "db_write",
                    "before_commit",
                    "committed"
                ]
            );
            assert_eq!(call[3].fields["statement_index"], serde_json::json!(0));
            let offsets = call
                .iter()
                .map(|record| record.fields["offset_milliseconds"].as_u64().unwrap())
                .collect::<Vec<_>>();
            assert!(offsets.windows(2).all(|pair| pair[0] <= pair[1]));
        }
        let expected = crate::query_plan_exec::race::purchase("TRACE_SECRET", "I1", "A1")
            .shape_fingerprint()
            .unwrap()
            .digest()
            .to_hex();
        for record in records.iter() {
            assert!(!record.fields.contains_key("request_fingerprint"));
            if let Some(fingerprint) = record.fields.get("fingerprint") {
                assert_eq!(fingerprint, &serde_json::json!(expected));
            }
        }
        for name in ["lower", "stage", "commit", "answer"] {
            assert!(
                records
                    .iter()
                    .any(|record| record.name == format!("type_bridge.plan.{name}")),
                "missing {name}"
            );
        }
        assert!(
            records
                .iter()
                .any(|record| record.name == "type_bridge.io.provider")
        );
        assert!(records.iter().any(|record| record.fields.get("items")
            == Some(&serde_json::json!(2))
            && record.fields.get("bytes") == Some(&serde_json::json!(4))));
        assert!(
            !serde_json::to_string(&*records)
                .unwrap()
                .contains("TRACE_SECRET")
        );
    }

    #[tokio::test]
    async fn explained_statements_are_the_recorded_provider_program() {
        for snapshot in [false, true] {
            let (mut tx, state) = fixture(TransactionOptions::default());
            state.lock().unwrap().given = true;
            state.lock().unwrap().snapshot_documents = Some(vec![]);
            let plan = if snapshot {
                crate::query_plan_exec::race::delete_named_doctors_returning(&["EXPLAIN_SECRET"])
            } else {
                crate::query_plan_exec::race::purchase("EXPLAIN_SECRET", "I1", "A1")
            };
            let explanation = crate::plan_explain::explain(&plan).unwrap();
            assert!(
                !String::from_utf8(explanation.canonical_bytes().unwrap())
                    .unwrap()
                    .contains("EXPLAIN_SECRET")
            );
            tx.stage_plan(&plan).unwrap();
            tx.commit().await.unwrap();
            let state = state.lock().unwrap();
            assert_eq!(explanation.statements, state.given_queries);
            assert_eq!(explanation.read_statements, usize::from(snapshot));
            assert_eq!(explanation.write_statements, 1);
        }
    }

    #[tokio::test]
    async fn explained_regex_statements_equal_the_redacted_provider_program() {
        use type_bridge_contract::migration_assertion::ValueComparator;
        for comparator in [
            ValueComparator::StartsWith,
            ValueComparator::EndsWith,
            ValueComparator::Like,
        ] {
            let (mut tx, state) = fixture(TransactionOptions::default());
            state.lock().unwrap().given = true;
            state.lock().unwrap().snapshot_documents = Some(vec![]);
            let plan = crate::query_plan_exec::race::string_read(
                comparator,
                &["REGEX_SECRET_A", "REGEX_SECRET_B"],
            );
            let explanation = crate::plan_explain::explain(&plan).unwrap();
            tx.stage_plan(&plan).unwrap();
            tx.commit().await.unwrap();
            let recorded = &state.lock().unwrap().given_queries;
            let redacted: Vec<_> = recorded
                .iter()
                .map(|statement| {
                    statement
                        .replace("REGEX_SECRET_A", "<input:needle>")
                        .replace("REGEX_SECRET_B", "<input:needle>")
                })
                .collect();
            assert_eq!(explanation.statements, redacted);
        }
    }

    fn empty_bulk(documents: bool) -> type_bridge_contract::query_plan::v3::BuiltPlans {
        use type_bridge_contract::id::{TypeId, TypeKind};
        use type_bridge_contract::query_plan::v3::PlanBuilder;
        let mut builder = PlanBuilder::new();
        let thing = builder.insert(TypeId::new(TypeKind::Entity, "doctor").unwrap());
        builder.bind_model_rows(thing, vec![]);
        if documents {
            builder.fetch_held(thing, vec![]);
        }
        builder
            .build_plans(
                &crate::query_plan_exec::race::schema(),
                crate::query_plan_exec::race::purchase_template()
                    .managed_semantics()
                    .clone(),
                false,
                type_bridge_contract::split::SplitLimits::CANONICAL,
            )
            .unwrap()
    }

    #[tokio::test]
    async fn empty_bulk_resolves_after_commit_without_provider_work() {
        let (mut tx, state) = fixture(TransactionOptions::default());
        let pending = tx
            .stage_plans(Arc::new(empty_bulk(true)), true)
            .unwrap()
            .remove(0);
        assert!(matches!(pending.get(), Err(PendingError::NotCommitted)));
        assert_eq!(tx.stack.0.len(), 0);
        tx.commit().await.unwrap();
        assert_eq!(*pending.get().unwrap(), PlanAnswer::Documents(vec![]));
        assert!(state.lock().unwrap().events.is_empty());
        assert!(state.lock().unwrap().kinds.is_empty());
        let rolled = tx
            .stage_plans(Arc::new(empty_bulk(false)), false)
            .unwrap()
            .remove(0);
        tx.rollback().unwrap();
        assert!(matches!(rolled.get(), Err(PendingError::RolledBack)));
        let failed = tx
            .stage_plans(Arc::new(empty_bulk(true)), true)
            .unwrap()
            .remove(0);
        tx.stage(StagedQuery::new("fail", true)).unwrap();
        assert!(tx.commit().await.is_err());
        assert!(matches!(failed.get(), Err(PendingError::Failed { .. })));
        let closed = tx
            .stage_plans(Arc::new(empty_bulk(true)), true)
            .unwrap()
            .remove(0);
        tx.close().unwrap();
        assert!(matches!(closed.get(), Err(PendingError::Closed)));
    }
    #[tokio::test]
    async fn deletion_snapshot_reuses_identical_given_rows_without_interpolation() {
        let (mut tx, state) = fixture(TransactionOptions::default());
        state.lock().unwrap().given = true;
        state.lock().unwrap().snapshot_documents = Some(vec![]);
        let plan =
            crate::query_plan_exec::race::delete_named_doctors_returning(&["SECRET_A", "SECRET_B"]);
        let query = crate::query_plan_exec::staged_query(&plan, true).unwrap();
        assert!(!query.typeql.contains("SECRET"));
        assert!(!query.snapshot.as_ref().unwrap().0.contains("SECRET"));
        tx.stage_plan(&plan).unwrap();
        tx.commit().await.unwrap();
        let state = state.lock().unwrap();
        assert_eq!(state.given_payloads.len(), 2);
        assert_eq!(state.given_payloads[0], state.given_payloads[1]);
        assert_eq!(state.given_payloads[0].rows.len(), 2);
    }

    #[tokio::test]
    async fn deletion_snapshots_execute_before_delete_and_fail_atomically() {
        use crate::query_plan_exec::race;
        for (fail_delete, malformed) in [(false, false), (true, false), (false, true)] {
            let (mut tx, state) = fixture(TransactionOptions::default());
            let documents = vec![if malformed {
                serde_json::json!({})
            } else {
                serde_json::json!({"_iid":"0x01", "_type":"doctor", "attributes":{"name":"alice", "on-call":true}})
            }];
            state.lock().unwrap().snapshot_documents = Some(documents.clone());
            state.lock().unwrap().fail_delete = fail_delete;
            let pending = tx.stage_plan(&race::delete_doctors_returning()).unwrap();
            assert!(matches!(pending.get(), Err(PendingError::NotCommitted)));
            let result = tx.commit().await;
            let events = &state.lock().unwrap().events;
            assert!(events[0].contains("fetch"));
            if malformed {
                assert!(result.is_err());
                assert_eq!(&events[1..], ["rollback", "close"]);
            } else {
                assert!(events[1].contains("delete"));
                if fail_delete {
                    assert!(result.is_err());
                    assert_eq!(&events[2..], ["rollback", "close"]);
                } else {
                    result.unwrap();
                    assert_eq!(&*pending.get().unwrap(), &PlanAnswer::Documents(documents));
                    assert_eq!(&events[2..], ["commit", "close"]);
                }
            }
            if fail_delete || malformed {
                assert!(matches!(pending.get(), Err(PendingError::Failed { .. })));
            }
        }
    }

    #[tokio::test]
    async fn staged_plans_resolve_matched_and_rows_from_their_own_stream() {
        use crate::query_plan_exec::race;
        let (mut tx, state) = fixture(TransactionOptions::default());
        {
            let mut state = state.lock().unwrap();
            state.given = true;
            state.rows = 1;
        }
        state.lock().unwrap().row_values = Some(vec![serde_json::json!({
            "doctor": type_bridge_contract::query_remote::RemoteValue::Thing {
                iid: "0x01".into(),
                type_id: type_bridge_contract::id::TypeId::new(type_bridge_contract::id::TypeKind::Entity, "doctor").unwrap(),
            }
        })]);
        let purchase = tx.stage_plan(&race::purchase("O1", "I1", "A1")).unwrap();
        let touched = tx.stage_plan(&race::touch_rows()).unwrap();
        let count = tx.stage_plan(&race::stock_read(Operation::Count)).unwrap();
        assert!(matches!(purchase.get(), Err(PendingError::NotCommitted)));
        tx.commit().await.unwrap();
        assert_eq!(*purchase.get().unwrap(), PlanAnswer::Matched(1));
        assert!(matches!(&*touched.get().unwrap(), PlanAnswer::Rows(rows) if rows.len() == 1));
        assert_eq!(*count.get().unwrap(), PlanAnswer::Count(1));
        let state = state.lock().unwrap();
        assert_eq!(state.kinds, [TxType::Write]);
        assert_eq!(state.events[0], "given:1");
        assert_eq!(state.events[3..], ["commit", "close"]);
    }

    #[tokio::test]
    async fn read_only_templates_and_unsupported_given_refuse_plans_at_stage() {
        use crate::query_plan_exec::race;
        let (mut read, _) = fixture(TransactionOptions::default().with_kind(TransactionKind::Read));
        assert!(matches!(
            read.stage_plan(&race::touch()),
            Err(PendingError::ReadOnly)
        ));
        read.stage_plan(&race::stock_read(Operation::Exists))
            .unwrap();
        let (mut tx, state) = fixture(TransactionOptions::default());
        let plan = race::purchase("O1", "I1", "A1");
        let Err(PendingError::InvalidPlan(diagnostic)) = tx.stage_plan(&plan) else {
            panic!("given transport is required");
        };
        assert_eq!(
            diagnostic.code().as_str(),
            "query_plan_given_transport_unsupported"
        );
        state.lock().unwrap().given = true;
        assert!(matches!(
            tx.stage_plan(&race::purchase_template()),
            Err(PendingError::InvalidPlan(_))
        ));
        assert!(format!("{:?}", tx.stage_plan(&plan).unwrap()).contains("pending"));
    }

    #[tokio::test]
    async fn a_malformed_plan_answer_aborts_before_commit() {
        use crate::query_plan_exec::race;
        let (mut tx, state) = fixture(TransactionOptions::default());
        state.lock().unwrap().answer_ok = true;
        let pending = tx.stage_plan(&race::touch()).unwrap();
        let error = tx.commit().await.unwrap_err();
        assert_eq!(error.op_index, Some(0));
        assert_eq!(
            error.diagnostic().code().to_string(),
            "query_plan_answer_shape"
        );
        assert!(matches!(pending.get(), Err(PendingError::Failed { .. })));
        assert_eq!(state.lock().unwrap().events[1..], ["rollback", "close"]);
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
    #[cfg(feature = "typedb")]
    #[tokio::test]
    async fn structural_failure_keeps_index_context_and_pending_answer_agreement() {
        use type_bridge_contract::id::{TypeId, TypeKind};
        let (mut tx, _) = fixture(TransactionOptions::default());
        tx.stage(StagedQuery::new("first", true)).unwrap();
        let context =
            FailureContext::new(Some(TypeId::new(TypeKind::Entity, "item").unwrap()), None);
        let pending = tx
            .stage(StagedQuery::new("typed-fail", true).with_failure_context(context.clone()))
            .unwrap();
        let error = tx.commit().await.unwrap_err();
        assert_eq!(error.failure.as_ref().unwrap().host_json()["op_index"], 1);
        assert_eq!(*error.context, context);
        assert_eq!(
            error.diagnostic().code().as_str(),
            "provider_operation_failed"
        );
        assert_eq!(
            pending.get().unwrap_err().failure(),
            error.failure.as_deref()
        );
        assert_eq!(
            error.error.failure().unwrap().host_json()["op_index"],
            serde_json::Value::Null
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
                diagnostic: Box::new(SdkExecutionDiagnostic::transaction_failure(
                    type_bridge_contract::sdk_diagnostic::SdkDiagnosticCode::new("test_failure")
                        .unwrap(),
                    type_bridge_contract::sdk_diagnostic::SdkDiagnosticMessage::new("failure")
                        .unwrap(),
                )),
                op_index: Some(1),
                message: "failure".into(),
                failure: None,
                context: Default::default(),
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

    /// A program that issues one statement on the provider and returns the
    /// rows it saw, standing in for a paged read's selection and rematch.
    fn probe(statement: &'static str) -> Program<usize> {
        Box::new(move |transaction, _| {
            Box::pin(async move {
                match transaction.query(statement).await? {
                    QueryResult::Rows(rows) => Ok(rows.len()),
                    _ => Ok(0),
                }
            })
        })
    }

    #[test]
    fn transactions_stay_send_and_sync_with_programs() {
        fn shareable<T: Send + Sync>() {}
        shareable::<Transaction>();
    }

    #[tokio::test]
    async fn programs_staged_in_write_see_earlier_writes_and_resolve_after_commit() {
        let (mut tx, state) =
            fixture(TransactionOptions::default().with_kind(TransactionKind::Write));
        state.lock().unwrap().rows = 2;
        let before = tx.stage_program(probe("read-1")).unwrap();
        tx.stage(StagedQuery::new("write", true)).unwrap();
        let after = tx.stage_program(probe("read-2")).unwrap();
        assert!(matches!(after.get(), Err(PendingError::NotCommitted)));
        tx.commit().await.unwrap();
        assert_eq!(*before.get().unwrap(), 2);
        assert_eq!(*after.get().unwrap(), 2);
        let state = state.lock().unwrap();
        assert_eq!(state.kinds, [TxType::Write]);
        assert_eq!(
            state.events,
            ["read-1", "write", "read-2", "commit", "close"]
        );
    }

    #[tokio::test]
    async fn a_failed_program_aborts_the_commit_at_its_index() {
        let (mut tx, state) = fixture(TransactionOptions::default());
        tx.stage(StagedQuery::new("write", true)).unwrap();
        let read = tx.stage_program(probe("fail")).unwrap();
        let error = tx.commit().await.unwrap_err();
        assert_eq!(error.op_index, Some(1));
        assert!(matches!(
            read.get(),
            Err(PendingError::Failed {
                op_index: Some(1),
                ..
            })
        ));
        assert_eq!(
            state.lock().unwrap().events,
            ["write", "fail", "rollback", "close"]
        );
    }

    #[tokio::test]
    async fn programs_read_under_the_commit_caps_deadline_and_cancellation() {
        let caps = BoundedAnswerLimits {
            max_items: 7,
            max_bytes: 4096,
            ..BoundedAnswerLimits::default()
        };
        let (mut tx, _) = fixture(
            TransactionOptions::default()
                .with_answer_limits(caps.clone())
                .with_answer_timeout(Duration::from_secs(30)),
        );
        let seen = tx
            .stage_program(Box::new(|_, limits| {
                Box::pin(async move { Ok((limits.max_items, limits.max_bytes, limits.deadline)) })
            }))
            .unwrap();
        tx.commit().await.unwrap();
        let (items, bytes, deadline) = *seen.get().unwrap();
        assert_eq!((items, bytes), (7, 4096));
        assert!(deadline.is_some_and(|deadline| deadline > Instant::now()));

        // A program stalled between statements fails at the commit deadline.
        let (mut tx, state) = fixture(
            TransactionOptions::default()
                .with_kind(TransactionKind::Write)
                .with_answer_timeout(Duration::from_millis(20)),
        );
        let stalled: Program<()> =
            Box::new(|_, _| Box::pin(std::future::pending::<Result<(), OrmError>>()));
        let stalled = tx.stage_program(stalled).unwrap();
        let error = tx.commit().await.unwrap_err();
        assert_eq!(error.op_index, Some(0));
        assert!(error.to_string().contains("deadline"));
        assert!(matches!(stalled.get(), Err(PendingError::Failed { .. })));
        assert_eq!(state.lock().unwrap().events, ["rollback", "close"]);

        // Cancellation of the transaction's answers stops a program too.
        let cancellation = super::super::backend::AnswerCancellation::default();
        let (mut tx, _) = fixture(TransactionOptions::default().with_answer_limits(
            BoundedAnswerLimits {
                cancellation: cancellation.clone(),
                ..BoundedAnswerLimits::default()
            },
        ));
        let stalled: Program<()> =
            Box::new(|_, _| Box::pin(std::future::pending::<Result<(), OrmError>>()));
        tx.stage_program(stalled).unwrap();
        let commit = tx.commit();
        cancellation.cancel();
        assert!(commit.await.unwrap_err().to_string().contains("cancelled"));
    }

    #[tokio::test]
    async fn schema_transactions_refuse_typed_reads_and_plans_with_the_released_code() {
        use crate::query_plan_exec::race;
        let (mut tx, state) =
            fixture(TransactionOptions::default().with_kind(TransactionKind::Schema));
        state.lock().unwrap().given = true;
        for refused in [
            tx.stage_program(probe("read")).map(|_| ()),
            tx.stage_plan(&race::stock_read(Operation::Count))
                .map(|_| ()),
        ] {
            let Err(PendingError::NotAllowedInKind(diagnostic)) = refused else {
                panic!("schema transactions keep eager mode");
            };
            assert_eq!(diagnostic.code().as_str(), "borrowed_target_not_read_only");
        }
        // Raw statements, such as schema definitions, still stage.
        tx.stage(StagedQuery::new("define", false)).unwrap();
        tx.commit().await.unwrap();
        assert_eq!(state.lock().unwrap().events, ["define", "commit", "close"]);
    }
}
