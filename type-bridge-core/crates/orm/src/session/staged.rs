//! Deferred transaction execution shared by one-shot and staged callers.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::backend::{
    BoundedAnswerLimits, BoxFuture, DriverBackend, GivenRowsSpec, QueryResult, TxType,
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
pub use crate::query_plan_exec::{PlanAnswer, staged_documents_unsupported};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::failure::Failure;
use type_bridge_contract::query_plan::v3::{Operation, QueryPlanV3};
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
            failure_context: FailureContext::default(),
        }
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
    #[error("Transaction execution failed{index}: {message}", index = format_op_index(*op_index))]
    Failed {
        /// Failing statement index, or none for opening/commit.
        op_index: Option<usize>,
        /// Value-free error description.
        message: String,
        /// Structural provider failure with the staged operation index.
        failure: Option<Box<Failure>>,
        /// Schema identities from the staged item.
        context: Box<FailureContext>,
    },
    /// A proxy commit the server refused under its own stable code. The
    /// refusal may have followed an earlier attempt that committed, so
    /// its outcome stays unknown until a retry replays a verified reply.
    #[error("Transaction execution failed{index}: the server refused the staged transaction ({code})", index = format_op_index(*op_index), code = .diagnostic.code())]
    Refused {
        /// Failing statement index, or none for the whole request.
        op_index: Option<usize>,
        /// The server's refusal, under its stable code.
        diagnostic: Box<SdkExecutionDiagnostic>,
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
#[non_exhaustive]
pub struct CommitError {
    /// Failing query index, or none for opening/commit.
    pub op_index: Option<usize>,
    /// Original failure, including existing commit certainty.
    #[source]
    pub error: Box<ClassifiedCommitError>,
    /// Structural classification with the staged operation index.
    pub failure: Option<Box<Failure>>,
    /// Schema identities from the executing staged item.
    pub context: Box<FailureContext>,
    /// The signed server refusal of a proxy commit, kept under its own code.
    remote: Option<Box<SdkExecutionDiagnostic>>,
}
impl CommitError {
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
        }
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
    /// Structural failure observed by all pending answers in the failed stack.
    #[must_use]
    pub fn failure(&self) -> Option<&Failure> {
        match self {
            Self::Failed { failure, .. } => failure.as_deref(),
            _ => None,
        }
    }
}

type AnswerState<T> = Arc<Mutex<Result<Arc<T>, PendingError>>>;
/// An answer made visible only after its whole execution succeeds.
///
/// On a proxy transaction a handle can read `Failed` with an unknown
/// outcome (or `Refused`) after `commit`, then resolve `Ok` after a
/// [`Transaction::commit_retry`] replays the committed reply.
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
    fn publish(&mut self);
    fn fail(&self, error: PendingError);
    fn close_if_pending(&self);
}
struct ProgramSlot<T> {
    state: AnswerState<T>,
    // Only commit's exclusive borrow takes it; the mutex supplies `Sync`.
    program: Mutex<Option<Program<T>>>,
    answer: Option<T>,
}
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

/// Where one staged item's answer goes, and how it resolves.
enum Slot {
    Query(AnswerState<QueryResult>),
    Plan {
        state: AnswerState<PlanAnswer>,
        operation: Operation,
    },
    Program(Box<dyn StagedProgram>),
}
/// An answer resolved during commit, published only after commit succeeds.
enum Resolved {
    Query(QueryResult),
    Plan(PlanAnswer),
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
            Self::Plan { operation, .. } => {
                crate::query_plan_exec::resolve(*operation, answer).map(Resolved::Plan)
            }
            Self::Program(_) => unreachable!("programs resolve their own answers"),
        }
    }
    fn publish(&mut self, answer: Resolved) {
        match (self, answer) {
            (Self::Query(state), Resolved::Query(answer)) => {
                *state.lock().unwrap() = Ok(Arc::new(answer));
            }
            (Self::Plan { state, .. }, Resolved::Plan(answer)) => {
                *state.lock().unwrap() = Ok(Arc::new(answer));
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
    slot: Slot,
}
impl Item {
    fn writes(&self) -> bool {
        self.query.as_ref().is_some_and(|query| query.writes)
    }
    fn failure_context(&self) -> FailureContext {
        self.query
            .as_ref()
            .map(|query| query.failure_context.clone())
            .unwrap_or_default()
    }
}
struct Execution(Vec<Item>);
impl Execution {
    fn fail(&self, error: PendingError) {
        for item in &self.0 {
            item.slot.fail(error.clone());
        }
    }
}
impl Drop for Execution {
    fn drop(&mut self) {
        // Also resolves handles when a commit future is cancelled.
        for item in &self.0 {
            item.slot.close_if_pending();
        }
    }
}

/// Keep a malformed plan answer's structured code in the commit error.
fn plan_answer_error(diagnostic: Diagnostic) -> OrmError {
    MatchError::new(
        MatchErrorCategory::ResultDecode,
        diagnostic.code().as_str(),
        diagnostic.message(),
    )
    .at(MatchErrorPathSegment::Result)
    .into()
}

/// Race a multi-statement read against the commit's cancellation and deadline.
async fn bounded<T>(
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
    if limits.cancellation.is_cancelled() {
        return Err(cancelled());
    }
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
fn proxy_failure(projection: &SdkDiagnosticProjectionV3) -> CommitError {
    let json = projection.host_json().unwrap_or_default();
    let field = |key: &str| json.get(key).and_then(serde_json::Value::as_str);
    let message = field("message").unwrap_or_default();
    let provider = json
        .get("details")
        .and_then(|details| details.get("code"))
        .and_then(serde_json::Value::as_str);
    let rendered = match provider {
        Some(code) => format!("[{code}] {message}"),
        None => message.to_owned(),
    };
    // Every attempt carries a key, so a refusal may answer a retry of an
    // attempt that already committed (an in-flight window, a lost
    // reservation): only a definite commit outcome proves nothing applied.
    let definite = projection.code() == "commit_definitely_aborted";
    let error = ClassifiedCommitError::Driver {
        certainty: if definite {
            crate::error::CommitFailureCertainty::DefinitelyAborted
        } else {
            crate::error::CommitFailureCertainty::Unknown
        },
        message: if definite || projection.code() == "commit_outcome_unknown" {
            rendered
        } else {
            format!("[{}] {rendered}", projection.code())
        },
    };
    let op_index = json
        .get("path")
        .and_then(serde_json::Value::as_array)
        .and_then(|path| path.iter().find_map(serde_json::Value::as_u64))
        .and_then(|index| usize::try_from(index).ok());
    let mut commit = CommitError::new(error, op_index, Default::default());
    if !definite && projection.code() != "commit_outcome_unknown" {
        commit.remote = remote_refusal(projection.code(), field("sdk_category")).map(Box::new);
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
    closed: bool,
    /// The last proxy attempt whose outcome is unknown, kept for
    /// [`Self::commit_retry`].
    retry: Option<RetryAttempt>,
}
enum Backend {
    Local(Arc<dyn DriverBackend>),
    Proxy(Arc<ProxyStaging>),
}
struct RetryAttempt {
    request: StagedRequestV3,
    bytes: Vec<u8>,
    execution: Execution,
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
    /// one envelope with a fresh random 16-byte idempotency key;
    /// [`Self::commit_retry`] is the only way to resend a key.
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
            retry: None,
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
        let state = Arc::new(Mutex::new(Err(PendingError::NotCommitted)));
        self.stack.0.push(Item {
            query: Some(query),
            remote: None,
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
        self.admit(plan.writes())?;
        self.admit_typed()?;
        let invalid = |diagnostic| PendingError::InvalidPlan(Box::new(diagnostic));
        let (query, remote) = match &self.backend {
            Backend::Local(backend) => (
                Some(
                    crate::query_plan_exec::staged_query(
                        plan,
                        super::database::backend_supports_given_stage(backend.as_ref()),
                    )
                    .map_err(invalid)?,
                ),
                None,
            ),
            Backend::Proxy(proxy) => {
                let rows = plan.given_rows().unwrap_or_default().to_vec();
                (
                    None,
                    Some(StagedPlanV3::new(plan, rows, proxy.declared()).map_err(invalid)?),
                )
            }
        };
        let state = Arc::new(Mutex::new(Err(PendingError::NotCommitted)));
        self.stack.0.push(Item {
            query,
            remote,
            slot: Slot::Plan {
                state: Arc::clone(&state),
                operation: plan.operation(),
            },
        });
        Ok(Pending { state })
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
        // A snapshot failure surfaces at commit with its typed error, so
        // staging keeps one lifecycle error type.
        let backend = self.local()?;
        let registry = registry.owned_registry_snapshot();
        let capabilities = backend.match_capabilities();
        let supports_given_stage = super::database::backend_supports_given_stage(backend.as_ref());
        self.stage_program(Box::new(move |transaction, bounds| {
            Box::pin(async move {
                let registry = registry?;
                let limits = limits.bounded_by(&bounds);
                SelectedResultExecutor::new(&registry, capabilities, limits)
                    .execute_compatible_staged(transaction, &validated, supports_given_stage)
                    .await
            })
        }))
    }
    /// Queue one crate-private read program.
    pub(crate) fn stage_program<T: Send + Sync + 'static>(
        &mut self,
        program: Program<T>,
    ) -> Result<Pending<T>, PendingError> {
        self.admit(false)?;
        self.admit_typed()?;
        let state = Arc::new(Mutex::new(Err(PendingError::NotCommitted)));
        self.stack.0.push(Item {
            query: None,
            remote: None,
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
        if self.closed {
            return Err(CommitError::lifecycle(PendingError::Closed));
        }
        if self.stack.0.is_empty() && self.retry.is_some() {
            return Err(CommitError::lifecycle(PendingError::InvalidPlan(Box::new(
                proxy_refusal("a commit with an unknown outcome is pending; call commit_retry"),
            ))));
        }
        let mut execution = std::mem::replace(&mut self.stack, Execution(Vec::new()));
        if execution.0.is_empty() {
            return Ok(());
        }
        self.retry = None;
        let backend = match &self.backend {
            Backend::Local(backend) => Arc::clone(backend),
            Backend::Proxy(proxy) => return self.commit_proxy(Arc::clone(proxy), execution).await,
        };
        let kind = self.options.kind.unwrap_or_else(|| {
            if execution.0.iter().any(Item::writes) {
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
                            failure: None,
                            context: Default::default(),
                        })
                    })
                })
                .transpose()?;
            let opened = backend
                .open_with_options(&self.database, kind.into(), &self.options)
                .await
                .map_err(|error| CommitError::new(error.into(), None, Default::default()))?;
            // A program borrows the provider as an owned transaction and
            // hands it back; `None` means it was consumed.
            let mut provider = Some(opened);
            let server_version = backend.server_version();
            let mut answers = Vec::with_capacity(execution.0.len());
            for (index, item) in execution.0.iter_mut().enumerate() {
                let resolved = match (&item.query, provider.as_mut()) {
                    (_, None) => Err(provider_consumed()),
                    (Some(query), Some(ops)) => {
                        let result = ops
                            .query_materialized_bounded(
                                &query.typeql,
                                query.rows.clone(),
                                answer_limits.clone(),
                                query.canonical,
                            )
                            .await;
                        // Plan answers resolve before the provider commit, so a
                        // malformed answer aborts instead of surfacing after writes.
                        result
                            .and_then(|answer| item.slot.resolve(answer).map_err(plan_answer_error))
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
                };
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
            let provider = provider.as_mut().ok_or_else(|| {
                CommitError::new(provider_consumed().into(), None, Default::default())
            })?;
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
                Ok(())
            }
            Err(error) => {
                execution.fail(error.pending());
                Err(error)
            }
        }
    }
    /// Resend the last proxy commit with the same request bytes and
    /// idempotency key.
    ///
    /// Allowed only after a proxy commit whose outcome is unknown (a
    /// dropped reply, an unverifiable reply, or `commit_outcome_unknown`).
    /// The server replays its retained reply when the first attempt
    /// committed, so the stack applies at most once; the original pending
    /// answers resolve from that reply. Every other state, and a local
    /// transaction, refuses with a lifecycle error.
    pub async fn commit_retry(&mut self) -> Result<(), CommitError> {
        if self.closed {
            return Err(CommitError::lifecycle(PendingError::Closed));
        }
        let (Backend::Proxy(proxy), Some(attempt)) = (&self.backend, self.retry.take()) else {
            return Err(CommitError::lifecycle(PendingError::InvalidPlan(Box::new(
                proxy_refusal("no proxy commit with an unknown outcome to retry"),
            ))));
        };
        let proxy = Arc::clone(proxy);
        let timeout = self.commit_timeout();
        self.attempt(&proxy, attempt, timeout).await
    }
    /// Whether a proxy commit with an unknown outcome awaits
    /// [`Self::commit_retry`]: its writes may have applied, and only a
    /// retry with the same key can tell.
    #[must_use]
    pub fn has_unknown_outcome(&self) -> bool {
        self.retry.is_some()
    }
    async fn commit_proxy(
        &mut self,
        proxy: Arc<ProxyStaging>,
        execution: Execution,
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
        let timeout = self.commit_timeout();
        let deadline_ms =
            timeout.map(|timeout| u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX));
        let plans = execution
            .0
            .iter()
            .filter_map(|item| item.remote.clone())
            .collect();
        let built = proxy
            .request(plans, kind, deadline_ms)
            .and_then(|request| request.encode().map(|bytes| (request, bytes)));
        let (request, bytes) = match built {
            Ok(built) => built,
            Err(diagnostic) => {
                let error = CommitError::lifecycle(PendingError::InvalidPlan(Box::new(diagnostic)));
                execution.fail(error.pending());
                return Err(error);
            }
        };
        self.attempt(
            &proxy,
            RetryAttempt {
                request,
                bytes,
                execution,
            },
            timeout,
        )
        .await
    }
    fn commit_timeout(&self) -> Option<Duration> {
        match (self.options.deadline, self.options.answer_timeout) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        }
    }
    async fn attempt(
        &mut self,
        proxy: &ProxyStaging,
        attempt: RetryAttempt,
        timeout: Option<Duration>,
    ) -> Result<(), CommitError> {
        // Anything short of a verified definite outcome may have committed.
        let unknown = |message: &str| {
            CommitError::new(
                ClassifiedCommitError::Driver {
                    certainty: crate::error::CommitFailureCertainty::Unknown,
                    message: message.to_owned(),
                },
                None,
                Default::default(),
            )
        };
        // The attempt is retryable from before the first byte is sent: a
        // cancelled commit leaves its handles unknown and `commit_retry`
        // available.
        attempt
            .execution
            .fail(unknown("the proxy commit outcome is not known").pending());
        let bytes = attempt.bytes.clone();
        self.retry = Some(attempt);
        let sent = proxy.send(bytes, timeout).await;
        let attempt = self
            .retry
            .as_mut()
            .unwrap_or_else(|| unreachable!("held across send"));
        let outcome = match sent {
            Err(_) => Err(unknown("the proxy reply was not received")),
            Ok(reply) => match proxy.verify(&reply, &attempt.request) {
                Err(_) => Err(unknown("the proxy reply could not be verified")),
                Ok(verified) => match verified.diagnostic() {
                    Some(projection) => Err(proxy_failure(projection)),
                    None => attempt
                        .execution
                        .0
                        .iter()
                        .zip(verified.answers())
                        .enumerate()
                        .map(|(index, (item, answer))| {
                            let plan = item.remote.as_ref().expect("proxy items carry plans");
                            PlanAnswer::from_staged_v3(answer, plan.plan())
                                .map(Resolved::Plan)
                                .map_err(|diagnostic| {
                                    CommitError::new(
                                        plan_answer_error(diagnostic).into(),
                                        Some(index),
                                        Default::default(),
                                    )
                                })
                        })
                        .collect::<Result<Vec<_>, _>>(),
                },
            },
        };
        match outcome {
            Ok(answers) => {
                let mut attempt = self.retry.take().expect("held across send");
                for (item, answer) in attempt.execution.0.iter_mut().zip(answers) {
                    item.slot.publish(answer);
                }
                Ok(())
            }
            Err(error) => {
                attempt.execution.fail(error.pending());
                if error.error.commit_failure_certainty()
                    != Some(crate::error::CommitFailureCertainty::Unknown)
                {
                    self.retry = None;
                }
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
        self.retry = None;
        Ok(())
    }
    /// Discard queued work and make this logical transaction terminal.
    pub fn close(&mut self) -> Result<(), PendingError> {
        if self.closed {
            return Err(PendingError::Closed);
        }
        self.stack.fail(PendingError::Closed);
        self.stack.0.clear();
        self.retry = None;
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
        given: bool,
        rows: usize,
        answer_ok: bool,
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
        fn answer(&self) -> QueryResult {
            let state = self.0.lock().unwrap();
            if state.answer_ok {
                QueryResult::Ok
            } else {
                QueryResult::Rows(vec![serde_json::json!({}); state.rows])
            }
        }
    }
    impl TransactionOps for Fake {
        fn query(&mut self, query: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            self.0.lock().unwrap().events.push(query.to_owned());
            let fail = query == "fail";
            let typed = query == "typed-fail";
            let answer = self.answer();
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
            _: &str,
            rows: GivenRowsSpec,
        ) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            self.0
                .lock()
                .unwrap()
                .events
                .push(format!("given:{}", rows.rows.len()));
            let answer = self.answer();
            Box::pin(async move { Ok(answer) })
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
    async fn staged_plans_resolve_matched_and_rows_from_their_own_stream() {
        use crate::query_plan_exec::race;
        let (mut tx, state) = fixture(TransactionOptions::default());
        {
            let mut state = state.lock().unwrap();
            state.given = true;
            state.rows = 1;
        }
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
