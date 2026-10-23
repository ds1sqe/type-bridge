//! Shared transaction context for grouping multiple operations.

use std::sync::Arc;

use tokio::sync::{Mutex, OwnedMutexGuard};

use super::backend::{
    AnswerConsumer, AnswerLimits, BoundedAnswerLimits, BoundedAnswerStats, GivenRowsSpec,
    QueryResult, TransactionOps, TxType,
};
use super::database::DatabaseExecutionIdentity;
use crate::_registry::DescriptorRegistry;
use crate::QueryExecutionResourceLimits;
use crate::error::{ClassifiedCommitError, OrmError, Result};
use crate::match_request::selected_result_executor::SelectedResultExecutor;
use crate::match_request::{
    CapabilitySet, MatchExecutionLimits, ValidatedMatchRequest, ValidatedMatchResult,
};
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic, SdkProviderOperation,
};
use type_bridge_core_lib::ast::{
    TypedFetchRows, TypedHydrateThings, TypedPageRematch, TypedRootScan,
};
use type_bridge_core_lib::version::Version;

/// Binding-neutral lifecycle state shared by every clone of a transaction context.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TransactionContextState {
    /// Provider work and lifecycle transitions are permitted.
    Active,
    /// Only rollback or close is permitted.
    RollbackOnly,
    /// Commit completed successfully.
    Committed,
    /// The provider proved that a failed commit did not take effect.
    DefinitelyAborted,
    /// A dispatched commit has an unknown durability outcome.
    CommitOutcomeUnknown,
    /// Rollback completed successfully.
    RolledBack,
    /// The provider resource is closed or otherwise no longer reusable.
    Closed,
}

enum CommitOnceError {
    RollbackOnly,
    /// The commit was refused before dispatch because the context no longer
    /// owns an open transaction; no provider outcome exists to classify.
    Consumed,
    Other(ClassifiedCommitError),
}

impl CommitOnceError {
    fn into_classified(self) -> ClassifiedCommitError {
        match self {
            Self::RollbackOnly => ClassifiedCommitError::from(OrmError::Transaction(
                "Transaction is rollback-only".into(),
            )),
            Self::Consumed => ClassifiedCommitError::from(consumed_error()),
            Self::Other(error) => error,
        }
    }
}

struct TransactionContextInner {
    transaction: Option<super::eager::EagerTransaction>,
    state: TransactionContextState,
    rollback_only_cause: Option<SdkExecutionDiagnostic>,
}

/// Shared transaction context for grouping multiple operations into
/// a single database transaction.
///
/// Cloneable via [`Arc`] — all clones share the same underlying
/// transaction. Call [`commit`](Self::commit) once when all operations
/// are complete.
pub struct TransactionContext {
    inner: Arc<Mutex<TransactionContextInner>>,
    tx_type: TxType,
    match_capabilities: CapabilitySet,
    server_version: Option<Version>,
    execution_identity: DatabaseExecutionIdentity,
    answer_limits: Option<QueryExecutionResourceLimits>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MutationLeasePhase {
    PreDispatch,
    PossibleEffect,
    Finished,
}

/// Private exclusive transaction lease for one common atomic mutation.
///
/// Holding the owned guard keeps ordinary queries and lifecycle transitions on
/// every context clone behind the same mutex for the whole multi-statement
/// operation. The type deliberately has no public re-export: its
/// consumers are the common projected batch executor and snapshot deletion.
pub(crate) struct TransactionContextMutationLease {
    inner: OwnedMutexGuard<TransactionContextInner>,
    phase: MutationLeasePhase,
    exact_failure: Option<SdkExecutionDiagnostic>,
}

impl TransactionContextMutationLease {
    fn transaction(&mut self) -> Result<&mut (dyn TransactionOps + 'static)> {
        active_transaction(&mut self.inner)
    }

    /// Execute a bounded scalar-`given` prerequisite while retaining the
    /// exclusive lifecycle fence. This does not arm rollback-only because no
    /// mutation has yet been dispatched.
    pub(crate) async fn query_v2_with_rows_bounded(
        &mut self,
        typeql: &str,
        rows: GivenRowsSpec,
        limits: AnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats> {
        if self.phase != MutationLeasePhase::PreDispatch {
            return Err(OrmError::Transaction(
                "Mutation prerequisites must precede mutation dispatch".into(),
            ));
        }
        self.transaction()?
            .query_v2_with_rows_bounded(typeql, rows, limits, consumer)
            .await
    }

    /// Arm the cancellation-safe poison latch synchronously, then dispatch one
    /// bounded mutation. Every later provider call remains inside the armed
    /// lease through complete hydration and output construction.
    pub(crate) async fn mutate_v2_with_rows_bounded(
        &mut self,
        typeql: &str,
        rows: GivenRowsSpec,
        limits: AnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats> {
        if self.phase == MutationLeasePhase::Finished {
            return Err(OrmError::Transaction(
                "Completed mutation lease cannot dispatch provider work".into(),
            ));
        }
        if self.phase == MutationLeasePhase::PreDispatch {
            self.phase = MutationLeasePhase::PossibleEffect;
        }
        self.transaction()?
            .query_v2_with_rows_bounded(typeql, rows, limits, consumer)
            .await
    }

    /// Retain the first exact redacted failure observed after mutation became
    /// possible. It supersedes the interruption fallback used only by Drop.
    pub(crate) fn record_failure(&mut self, diagnostic: &SdkExecutionDiagnostic) {
        if self.phase == MutationLeasePhase::PossibleEffect && self.exact_failure.is_none() {
            self.exact_failure = Some(diagnostic.clone());
        }
    }

    /// Mark a fully hydrated, fully reserved borrowed result reusable.
    pub(crate) fn complete_success(mut self) {
        self.phase = MutationLeasePhase::Finished;
    }
}

impl Drop for TransactionContextMutationLease {
    fn drop(&mut self) {
        if self.phase != MutationLeasePhase::PossibleEffect
            || self.inner.state != TransactionContextState::Active
        {
            return;
        }
        self.inner.state = TransactionContextState::RollbackOnly;
        self.inner.rollback_only_cause = Some(
            self.exact_failure
                .clone()
                .unwrap_or_else(SdkExecutionDiagnostic::internal_failure),
        );
    }
}

impl TransactionContext {
    pub(crate) fn new(
        inner: super::eager::EagerTransaction,
        tx_type: TxType,
        match_capabilities: CapabilitySet,
        server_version: Option<Version>,
        execution_identity: DatabaseExecutionIdentity,
        answer_limits: Option<QueryExecutionResourceLimits>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(TransactionContextInner {
                transaction: Some(inner),
                state: TransactionContextState::Active,
                rollback_only_cause: None,
            })),
            tx_type,
            match_capabilities,
            server_version,
            execution_identity,
            answer_limits,
        }
    }

    pub(crate) const fn execution_identity(&self) -> &DatabaseExecutionIdentity {
        &self.execution_identity
    }

    /// Return the immutable generated direct-connection answer ceiling.
    #[doc(hidden)]
    #[must_use]
    pub const fn answer_limits(&self) -> Option<QueryExecutionResourceLimits> {
        self.answer_limits
    }

    /// Acquire the private whole-mutation lifecycle fence.
    pub(crate) async fn acquire_mutation_lease(&self) -> Result<TransactionContextMutationLease> {
        if self.tx_type != TxType::Write {
            return Err(OrmError::Transaction(
                "Projected batch mutations require a write transaction".into(),
            ));
        }
        let mut inner = Arc::clone(&self.inner).lock_owned().await;
        if inner.state == TransactionContextState::RolledBack {
            inner.state = TransactionContextState::Active;
        }
        if inner.state != TransactionContextState::Active || inner.transaction.is_none() {
            return Err(consumed_error());
        }
        Ok(TransactionContextMutationLease {
            inner,
            phase: MutationLeasePhase::PreDispatch,
            exact_failure: None,
        })
    }

    /// Return the lifecycle state shared by all clones.
    #[doc(hidden)]
    pub async fn lifecycle_state(&self) -> TransactionContextState {
        self.inner.lock().await.state
    }

    /// Return the first redacted failure that made this transaction rollback-only.
    #[doc(hidden)]
    pub async fn rollback_only_cause(&self) -> Option<SdkExecutionDiagnostic> {
        self.inner.lock().await.rollback_only_cause.clone()
    }

    /// Make an active transaction rollback-only while retaining its first cause.
    ///
    /// The diagnostic is already a redacted, binding-neutral value and is cloned
    /// into shared state. Callers that invoke this after provider dispatch must
    /// already hold an executor-level exclusive lease spanning dispatch through
    /// this latch; calling it after an ordinary query returns is not race-safe
    /// against a competing clone that commits.
    #[doc(hidden)]
    pub async fn latch_rollback_only(&self, diagnostic: &SdkExecutionDiagnostic) {
        let mut inner = self.inner.lock().await;
        if inner.state == TransactionContextState::Active {
            inner.state = TransactionContextState::RollbackOnly;
            inner.rollback_only_cause = Some(diagnostic.clone());
        }
    }

    /// Execute a query on the shared transaction.
    pub async fn query(&self, typeql: &str) -> Result<QueryResult> {
        self.check_schema_annotation_support(typeql)?;
        let mut guard = self.inner.lock().await;
        let tx = active_transaction(&mut guard)?;
        tx.query(typeql).await
    }

    /// Execute a canonical provider-answer query for binding-neutral CRUD.
    #[doc(hidden)]
    pub(crate) async fn query_canonical(&self, typeql: &str) -> Result<QueryResult> {
        self.check_schema_annotation_support(typeql)?;
        let mut guard = self.inner.lock().await;
        let tx = active_transaction(&mut guard)?;
        tx.query_canonical(typeql).await
    }

    /// Execute one canonical V2 provider-answer query through the bounded
    /// streaming seam without consuming this transaction context.
    ///
    /// This is the read-only counterpart to the bounded methods on
    /// [`TransactionContextMutationLease`]. The caller owns the absolute
    /// deadline and cancellation token and must retain any cumulative
    /// statement/output accounting around this single provider call.
    #[doc(hidden)]
    pub(crate) async fn query_v2_bounded(
        &self,
        typeql: &str,
        limits: AnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats> {
        self.check_schema_annotation_support(typeql)?;
        let mut guard = self.inner.lock().await;
        let tx = active_transaction(&mut guard)?;
        tx.query_v2_bounded(typeql, limits, consumer).await
    }

    /// Lower one trusted v3 plan for this transaction without dispatching
    /// it. A failure here means nothing reached the provider.
    ///
    /// Helpers that still run inside an eager transaction use this and
    /// [`Self::dispatch_plan`] until they stage plans. Inputs travel as
    /// `given` rows, so the caller builds literal-only plans for connections
    /// without the `given` stage.
    pub(crate) async fn prepare_plan(
        &self,
        plan: &type_bridge_contract::query_plan::v3::QueryPlanV3,
    ) -> Result<crate::session::staged::StagedQuery> {
        let supports_given = self.supports_given_stage().await?;
        let query = crate::query_plan_exec::staged_query(plan, supports_given)
            .map_err(plan_diagnostic_error)?;
        self.check_schema_annotation_support(&query.typeql)?;
        Ok(query)
    }

    /// Dispatch one prepared plan query and return the provider answer.
    pub(crate) async fn dispatch_plan(
        &self,
        query: crate::session::staged::StagedQuery,
    ) -> Result<QueryResult> {
        if query.snapshot.is_some() {
            let mut lease = self.acquire_mutation_lease().await?;
            // Cancellation or failure anywhere in the two-statement operation
            // leaves the caller-owned transaction unable to commit.
            lease.phase = MutationLeasePhase::PossibleEffect;
            let result = query
                .execute(
                    lease.transaction()?,
                    super::backend::AnswerLimits::default(),
                )
                .await
                .and_then(|answer| {
                    crate::answer_meter::meter_result(&answer, &BoundedAnswerLimits::default())?;
                    Ok(answer)
                });
            return match result {
                Ok(answer) => {
                    lease.complete_success();
                    Ok(answer)
                }
                Err(error) => {
                    lease.record_failure(&crate::execution_diagnostic::lower_orm_error(
                        &error,
                        SdkProviderOperation::Write,
                    ));
                    Err(error)
                }
            };
        }
        let mut guard = self.inner.lock().await;
        let tx = active_transaction(&mut guard)?;
        let answer = tx
            .query_materialized_bounded(
                &query.typeql,
                query.rows,
                super::backend::AnswerLimits::default(),
                query.canonical,
            )
            .await?;
        crate::answer_meter::meter_result(&answer, &BoundedAnswerLimits::default())?;
        Ok(answer)
    }

    /// Capture and hydrate a legacy manager batch before any deletion, keeping
    /// the same exclusive transaction lease through all statements.
    pub(crate) async fn delete_captured<F>(
        &self,
        statements: &(String, String),
        iids: &[String],
        canonical: bool,
        mut hydrate: F,
    ) -> Result<Vec<serde_json::Value>>
    where
        F: FnMut(usize, Vec<serde_json::Value>) -> Result<Option<serde_json::Value>> + Send,
    {
        let mut limits = AnswerLimits::default();
        if let Some(policy) = self.answer_limits() {
            if policy.statements < 2 {
                return Err(crate::match_request::MatchError::new(
                    crate::match_request::MatchErrorCategory::ResourceLimit,
                    "statement_count_limit",
                    "Deletion snapshots exceed the configured statement ceiling",
                )
                .into());
            }
            limits.answer.max_items = policy.items;
            limits.answer.max_bytes = policy.bytes;
            limits.answer.deadline = Some(
                crate::query_execution_limits::QueryExecutionDeadline::for_limits(policy).instant(),
            );
            limits.max_collection_members = policy.collection_members;
        }
        let mut lease = self.acquire_mutation_lease().await?;
        lease.phase = MutationLeasePhase::PossibleEffect;
        let result = async {
            let mut meter = crate::answer_meter::AnswerMeter::new(&limits.answer);
            let mut snapshots = Vec::new();
            let mut snapshot_resources = [0_u64; 4];
            let policy = self.answer_limits().unwrap_or_default();
            let rows = GivenRowsSpec {
                variables: vec!["snapshot_iid".into()],
                rows: iids
                    .iter()
                    .map(|iid| vec![super::backend::GivenValue::String(iid.to_ascii_lowercase())])
                    .collect(),
            };
            let answer = lease
                .transaction()?
                .query_materialized_bounded(
                    &statements.0,
                    Some(rows.clone()),
                    limits.clone(),
                    canonical,
                )
                .await?;
            let QueryResult::Documents(documents) = answer else {
                return Err(OrmError::Hydration {
                    type_name: "delete_returning".into(),
                    message: "Snapshot capture did not return documents".into(),
                });
            };
            let positions: std::collections::HashMap<_, _> = iids
                .iter()
                .enumerate()
                .map(|(index, iid)| (iid.to_ascii_lowercase(), index))
                .collect();
            let mut grouped = vec![Vec::new(); iids.len()];
            for document in documents {
                meter.accept_value(&document)?;
                let position = document
                    .as_object()
                    .and_then(|object| {
                        crate::_manager::hydration::extract_scalar_string(object, "_iid")
                    })
                    .and_then(|iid| positions.get(&iid.to_ascii_lowercase()).copied())
                    .ok_or_else(|| OrmError::Hydration {
                        type_name: "delete_returning".into(),
                        message: "Snapshot capture returned an unrequested identity".into(),
                    })?;
                grouped[position].push(document);
            }
            for (index, documents) in grouped.into_iter().enumerate() {
                if let Some(snapshot) = hydrate(index, documents)? {
                    charge_deletion_snapshot(&snapshot, &mut snapshot_resources, policy)?;
                    snapshots.push(snapshot);
                }
            }
            let answer = lease
                .transaction()?
                .query_materialized_bounded(&statements.1, Some(rows), limits.clone(), canonical)
                .await?;
            match answer {
                QueryResult::Rows(rows) | QueryResult::Documents(rows) => {
                    for row in &rows {
                        meter.accept_value(row)?;
                    }
                }
                QueryResult::Ok => {}
            }
            Ok(snapshots)
        }
        .await;
        match result {
            Ok(snapshots) => {
                lease.complete_success();
                Ok(snapshots)
            }
            Err(error) => {
                lease.record_failure(&crate::execution_diagnostic::lower_orm_error(
                    &error,
                    SdkProviderOperation::Write,
                ));
                Err(error)
            }
        }
    }

    /// Resolve a dispatched plan answer.
    pub(crate) fn resolve_plan(
        plan: &type_bridge_contract::query_plan::v3::QueryPlanV3,
        answer: QueryResult,
    ) -> Result<crate::query_plan_exec::PlanAnswer> {
        crate::query_plan_exec::resolve_plan(plan, answer).map_err(plan_diagnostic_error)
    }

    /// Export the schema under this transaction's provider-side schema fence,
    /// when supported by the backend.
    pub(crate) async fn schema_snapshot(&self) -> Result<Option<String>> {
        let mut guard = self.inner.lock().await;
        let tx = active_transaction(&mut guard)?;
        tx.schema_snapshot().await
    }

    /// Execute a `given`-stage query with input rows on the shared transaction.
    ///
    /// Requires a band-9 (TypeDB 3.12+) connection; see
    /// [`Database::check_given_stage_support`](super::database::Database::check_given_stage_support).
    pub async fn query_with_rows(&self, typeql: &str, rows: GivenRowsSpec) -> Result<QueryResult> {
        self.check_schema_annotation_support(typeql)?;
        let mut guard = self.inner.lock().await;
        let tx = active_transaction(&mut guard)?;
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
        let tx = active_transaction(&mut guard)?;
        tx.query_typed_bounded(query, limits, consumer).await
    }

    /// Return whether this borrowed transaction can transport canonical given rows.
    pub(crate) async fn supports_given_rows(&self) -> Result<bool> {
        let guard = self.inner.lock().await;
        let tx = active_transaction_ref(&guard)?;
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
        let tx = active_transaction_ref(&guard)?;
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
        let tx = active_transaction(&mut guard)?;
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
        let tx = active_transaction(&mut guard)?;
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
        let tx = active_transaction(&mut guard)?;
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
        let tx = active_transaction(&mut guard)?;
        tx.rematch_page_typed_bounded(query, limits, consumer).await
    }

    /// Commit the shared transaction.
    pub async fn commit(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        let mut transaction = begin_commit(&mut inner)
            .map_err(CommitOnceError::into_classified)
            .map_err(ClassifiedCommitError::into_orm_error)?;
        let result = transaction.commit().await;
        inner.state = if result.is_ok() {
            TransactionContextState::Committed
        } else {
            TransactionContextState::CommitOutcomeUnknown
        };
        result
    }

    /// Commit while retaining provider durability certainty when available.
    ///
    /// This consumes the shared transaction exactly once, just like
    /// [`Self::commit`]. Recovery-aware binding runtimes can distinguish a
    /// provider-proven abort from an unknown commit outcome without exposing
    /// provider text across their public boundary.
    pub async fn commit_classified(&self) -> std::result::Result<(), ClassifiedCommitError> {
        self.commit_classified_once()
            .await
            .map_err(CommitOnceError::into_classified)
    }

    /// Commit with the exact redacted binding-neutral diagnostic contract.
    #[doc(hidden)]
    pub async fn commit_sdk(&self) -> std::result::Result<(), SdkExecutionDiagnostic> {
        self.commit_classified_once()
            .await
            .map_err(|error| match error {
                CommitOnceError::RollbackOnly => transaction_rollback_only_diagnostic(),
                CommitOnceError::Consumed => crate::execution_diagnostic::lower_execution_error(
                    consumed_error(),
                    SdkProviderOperation::Write,
                ),
                CommitOnceError::Other(error) => {
                    crate::execution_diagnostic::lower_commit_error(&error)
                }
            })
    }

    /// Roll back writes and retain this shared context for more work.
    pub async fn rollback(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        let mut transaction = match inner.state {
            TransactionContextState::RolledBack => return Ok(()),
            TransactionContextState::Active | TransactionContextState::RollbackOnly => {
                // A dispatched rollback owns the provider resource. If its
                // future is dropped or fails, that resource is not reusable.
                inner.state = TransactionContextState::Closed;
                inner.transaction.take().ok_or_else(consumed_error)?
            }
            TransactionContextState::Committed
            | TransactionContextState::DefinitelyAborted
            | TransactionContextState::CommitOutcomeUnknown
            | TransactionContextState::Closed => return Err(consumed_error()),
        };

        let result = transaction.rollback().await;
        match &result {
            Ok(()) => {
                inner.transaction = Some(transaction);
                inner.rollback_only_cause = None;
                inner.state = TransactionContextState::RolledBack;
            }
            Err(_) => {
                let _ = transaction.close().await;
                if inner.rollback_only_cause.is_none() {
                    inner.rollback_only_cause = Some(SdkExecutionDiagnostic::provider_failure(
                        SdkProviderOperation::Rollback,
                    ));
                }
            }
        }
        result
    }

    /// Close the shared transaction without committing.
    pub async fn close(&self) -> Result<()> {
        let transaction = {
            let mut inner = self.inner.lock().await;
            match inner.state {
                TransactionContextState::Active
                | TransactionContextState::RollbackOnly
                | TransactionContextState::RolledBack => {
                    inner.state = TransactionContextState::Closed;
                    inner.transaction.take()
                }
                TransactionContextState::Committed
                | TransactionContextState::DefinitelyAborted
                | TransactionContextState::CommitOutcomeUnknown
                | TransactionContextState::Closed => None,
            }
        };
        let Some(mut transaction) = transaction else {
            return Ok(());
        };
        transaction.close().await
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

    async fn commit_classified_once(&self) -> std::result::Result<(), CommitOnceError> {
        let mut inner = self.inner.lock().await;
        let mut transaction = begin_commit(&mut inner)?;

        let result = transaction.commit_classified().await;
        inner.state = match &result {
            Ok(()) => TransactionContextState::Committed,
            Err(ClassifiedCommitError::Driver {
                certainty: crate::error::CommitFailureCertainty::DefinitelyAborted,
                ..
            }) => TransactionContextState::DefinitelyAborted,
            Err(ClassifiedCommitError::Driver {
                certainty: crate::error::CommitFailureCertainty::Unknown,
                ..
            })
            | Err(ClassifiedCommitError::Orm(_)) => TransactionContextState::CommitOutcomeUnknown,
        };
        result.map_err(CommitOnceError::Other)
    }
}

fn begin_commit(
    inner: &mut TransactionContextInner,
) -> std::result::Result<super::eager::EagerTransaction, CommitOnceError> {
    match inner.state {
        TransactionContextState::Active | TransactionContextState::RolledBack => {
            let transaction = inner.transaction.take().ok_or(CommitOnceError::Consumed)?;
            // Commit dispatch is irreversible. If this future is dropped,
            // the provider outcome cannot safely be inferred.
            inner.state = TransactionContextState::CommitOutcomeUnknown;
            Ok(transaction)
        }
        TransactionContextState::RollbackOnly => Err(CommitOnceError::RollbackOnly),
        TransactionContextState::Committed
        | TransactionContextState::DefinitelyAborted
        | TransactionContextState::CommitOutcomeUnknown
        | TransactionContextState::Closed => Err(CommitOnceError::Consumed),
    }
}

// Count the coalesced snapshot, so Cartesian role evidence is not charged as
// multiple logical roots. Keep the same semantic dimensions as projected CRUD.
fn charge_deletion_snapshot(
    snapshot: &serde_json::Value,
    totals: &mut [u64; 4],
    policy: QueryExecutionResourceLimits,
) -> Result<()> {
    fn attribute_count(document: &serde_json::Value) -> u64 {
        let Some(object) = document.as_object() else {
            return 0;
        };
        let attributes = object
            .get("attributes")
            .and_then(serde_json::Value::as_object);
        attributes
            .unwrap_or(object)
            .iter()
            .filter(|(key, _)| {
                attributes.is_some()
                    || (!key.starts_with('_') && *key != "attributes" && *key != "role_players")
            })
            .fold(0_u64, |count, (_, value)| {
                count.saturating_add(value.as_array().map_or(1, |values| values.len() as u64))
            })
    }
    let players = snapshot
        .get("role_players")
        .and_then(serde_json::Value::as_array);
    let player_count = players.map_or(0, |players| players.len() as u64);
    let attributes = players
        .into_iter()
        .flatten()
        .fold(attribute_count(snapshot), |count, player| {
            count.saturating_add(attribute_count(player))
        });
    let counts = [
        1_u64.saturating_add(player_count),
        attributes,
        attributes.saturating_add(player_count),
        player_count,
    ];
    let dimensions = [
        ("graph_nodes", policy.graph_nodes),
        ("attribute_values", policy.attribute_values),
        ("collection_members", policy.collection_members),
        ("role_players", policy.role_players),
    ];
    for ((total, count), (dimension, maximum)) in totals.iter_mut().zip(counts).zip(dimensions) {
        *total = total.saturating_add(count);
        if *total > maximum {
            return Err(crate::match_request::MatchError::new(
                crate::match_request::MatchErrorCategory::ResourceLimit,
                "deletion_snapshot_resource_limit",
                "Deletion snapshots exceed the configured resource ceiling",
            )
            .with_detail("dimension", dimension)
            .with_detail("actual", *total)
            .with_detail("maximum", maximum)
            .into());
        }
    }
    Ok(())
}

fn active_transaction(
    inner: &mut TransactionContextInner,
) -> Result<&mut (dyn TransactionOps + 'static)> {
    if inner.state == TransactionContextState::RolledBack {
        inner.state = TransactionContextState::Active;
    }
    if inner.state != TransactionContextState::Active {
        return Err(consumed_error());
    }
    inner
        .transaction
        .as_mut()
        .ok_or_else(consumed_error)?
        .provider_mut()
}

fn active_transaction_ref(inner: &TransactionContextInner) -> Result<&dyn TransactionOps> {
    if !matches!(
        inner.state,
        TransactionContextState::Active | TransactionContextState::RolledBack
    ) {
        return Err(consumed_error());
    }
    inner
        .transaction
        .as_ref()
        .ok_or_else(consumed_error)?
        .provider_ref()
}

fn consumed_error() -> OrmError {
    OrmError::Transaction("Transaction already consumed".into())
}

fn transaction_rollback_only_diagnostic() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::transaction_failure(
        SdkDiagnosticCode::new("transaction_rollback_only")
            .expect("the rollback-only diagnostic code is canonical"),
        SdkDiagnosticMessage::new("The transaction is rollback-only and cannot be committed")
            .expect("the rollback-only diagnostic message is canonical"),
    )
}

impl Clone for TransactionContext {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            tx_type: self.tx_type,
            match_capabilities: self.match_capabilities.clone(),
            server_version: self.server_version,
            execution_identity: self.execution_identity.clone(),
            answer_limits: self.answer_limits,
        }
    }
}

/// A plan that cannot lower or answer is a contract failure, never provider
/// prose; it keeps its diagnostic code.
fn plan_diagnostic_error(diagnostic: type_bridge_contract::diagnostic::Diagnostic) -> OrmError {
    crate::match_request::MatchError::new(
        crate::match_request::MatchErrorCategory::ResultDecode,
        diagnostic.code().as_str(),
        diagnostic.message(),
    )
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CommitFailureCertainty;
    use crate::session::backend::{BoxFuture, QueryResult};
    use std::future::pending;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;
    use type_bridge_contract::sdk_diagnostic::{SdkCommitFailureOutcome, SdkDiagnosticCategory};

    #[derive(Clone, Copy)]
    enum CommitResult {
        Success,
        DefinitelyAborted,
        Unknown,
        LegacyFailure,
    }

    #[derive(Default)]
    struct Calls {
        queries: AtomicUsize,
        commits: AtomicUsize,
        classified_commits: AtomicUsize,
        rollbacks: AtomicUsize,
        closes: AtomicUsize,
    }

    #[derive(Default)]
    struct Gate {
        entered: Notify,
    }

    struct LifecycleTransaction {
        calls: Arc<Calls>,
        commit_result: CommitResult,
        rollback_fails: bool,
        close_fails: bool,
        commit_gate: Option<Arc<Gate>>,
        rollback_gate: Option<Arc<Gate>>,
        close_gate: Option<Arc<Gate>>,
        deletion_gate: Option<Arc<Gate>>,
    }

    impl TransactionOps for LifecycleTransaction {
        fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult>> {
            self.calls.queries.fetch_add(1, Ordering::SeqCst);
            let gate = self.deletion_gate.clone();
            let deleting = typeql.contains("delete");
            Box::pin(async move {
                if let Some(gate) = gate {
                    if deleting {
                        gate.entered.notify_one();
                        return pending().await;
                    }
                    return Ok(QueryResult::Documents(vec![serde_json::json!({
                        "_iid":"0x01", "_type":"doctor", "attributes":{"name":"alice", "on-call":true}
                    })]));
                }
                Ok(QueryResult::Ok)
            })
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<()>> {
            self.calls.commits.fetch_add(1, Ordering::SeqCst);
            let result = self.commit_result;
            let gate = self.commit_gate.clone();
            Box::pin(async move {
                if let Some(gate) = gate {
                    gate.entered.notify_one();
                    pending().await
                }
                match result {
                    CommitResult::Success => Ok(()),
                    CommitResult::LegacyFailure
                    | CommitResult::DefinitelyAborted
                    | CommitResult::Unknown => {
                        Err(OrmError::Transaction("private provider commit text".into()))
                    }
                }
            })
        }

        fn commit_classified(
            &mut self,
        ) -> BoxFuture<'_, std::result::Result<(), ClassifiedCommitError>> {
            self.calls.classified_commits.fetch_add(1, Ordering::SeqCst);
            let result = self.commit_result;
            let gate = self.commit_gate.clone();
            Box::pin(async move {
                if let Some(gate) = gate {
                    gate.entered.notify_one();
                    pending().await
                }
                match result {
                    CommitResult::Success => Ok(()),
                    CommitResult::DefinitelyAborted => Err(ClassifiedCommitError::Driver {
                        certainty: CommitFailureCertainty::DefinitelyAborted,
                        message: "private definitely-aborted text".into(),
                    }),
                    CommitResult::Unknown => Err(ClassifiedCommitError::Driver {
                        certainty: CommitFailureCertainty::Unknown,
                        message: "private unknown-outcome text".into(),
                    }),
                    CommitResult::LegacyFailure => Err(ClassifiedCommitError::from(
                        OrmError::Transaction("private provider commit text".into()),
                    )),
                }
            })
        }

        fn rollback(&mut self) -> BoxFuture<'_, Result<()>> {
            self.calls.rollbacks.fetch_add(1, Ordering::SeqCst);
            let fails = self.rollback_fails;
            let gate = self.rollback_gate.clone();
            Box::pin(async move {
                if let Some(gate) = gate {
                    gate.entered.notify_one();
                    pending().await
                }
                if fails {
                    Err(OrmError::Transaction("private rollback path secret".into()))
                } else {
                    Ok(())
                }
            })
        }

        fn close(&mut self) -> BoxFuture<'_, Result<()>> {
            self.calls.closes.fetch_add(1, Ordering::SeqCst);
            let fails = self.close_fails;
            let gate = self.close_gate.clone();
            Box::pin(async move {
                if let Some(gate) = gate {
                    gate.entered.notify_one();
                    pending().await
                }
                if fails {
                    Err(OrmError::Transaction("private close path secret".into()))
                } else {
                    Ok(())
                }
            })
        }
    }

    fn new_context(
        commit_result: CommitResult,
        rollback_fails: bool,
        close_fails: bool,
        commit_gate: Option<Arc<Gate>>,
        rollback_gate: Option<Arc<Gate>>,
        close_gate: Option<Arc<Gate>>,
    ) -> (TransactionContext, Arc<Calls>) {
        let calls = Arc::new(Calls::default());
        let context = TransactionContext::new(
            super::super::eager::EagerTransaction::new(
                Box::new(LifecycleTransaction {
                    calls: Arc::clone(&calls),
                    commit_result,
                    rollback_fails,
                    close_fails,
                    commit_gate,
                    rollback_gate,
                    close_gate,
                    deletion_gate: None,
                }),
                TxType::Write,
                None,
            ),
            TxType::Write,
            CapabilitySet::new(),
            None,
            DatabaseExecutionIdentity::isolated("transaction-lifecycle-test"),
            None,
        );
        (context, calls)
    }

    fn active_context() -> (TransactionContext, Arc<Calls>) {
        new_context(CommitResult::Success, false, false, None, None, None)
    }

    #[tokio::test]
    async fn clones_share_state_and_first_redacted_rollback_only_cause() {
        let (context, calls) = active_context();
        let clone = context.clone();
        let first = SdkExecutionDiagnostic::provider_failure(SdkProviderOperation::Write);
        let second = SdkExecutionDiagnostic::provider_failure(SdkProviderOperation::Read);

        clone.latch_rollback_only(&first).await;
        context.latch_rollback_only(&second).await;

        assert_eq!(
            context.lifecycle_state().await,
            TransactionContextState::RollbackOnly
        );
        assert_eq!(clone.rollback_only_cause().await, Some(first));
        assert!(matches!(
            context.query("match $x isa thing;").await,
            Err(OrmError::Transaction(_))
        ));
        assert_eq!(calls.queries.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn rollback_only_sdk_commit_is_exact_and_never_calls_provider() {
        let (context, calls) = active_context();
        context
            .latch_rollback_only(&SdkExecutionDiagnostic::internal_failure())
            .await;

        let diagnostic = context.commit_sdk().await.unwrap_err();
        assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Transaction);
        assert_eq!(diagnostic.code().as_str(), "transaction_rollback_only");
        assert_eq!(calls.commits.load(Ordering::SeqCst), 0);
        assert_eq!(calls.classified_commits.load(Ordering::SeqCst), 0);
        assert!(matches!(
            context.commit().await,
            Err(OrmError::Transaction(_))
        ));
        assert_eq!(
            context.lifecycle_state().await,
            TransactionContextState::RollbackOnly
        );
    }

    #[tokio::test]
    async fn second_sdk_commit_is_a_pre_dispatch_lifecycle_error() {
        let (context, calls) = active_context();
        context.commit_sdk().await.unwrap();

        // The second commit never reaches the provider, so it must not
        // claim an unknown durability outcome.
        let diagnostic = context.commit_sdk().await.unwrap_err();
        assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Transaction);
        assert_eq!(diagnostic.code().as_str(), "transaction_state_invalid");
        assert!(diagnostic.failure().is_none());
        assert_eq!(calls.classified_commits.load(Ordering::SeqCst), 1);
        assert_eq!(
            context.lifecycle_state().await,
            TransactionContextState::Committed
        );
        let OrmError::Transaction(message) = context.commit().await.unwrap_err() else {
            panic!("legacy double commit keeps the released transaction error");
        };
        assert_eq!(message, "Transaction already consumed");
    }

    #[tokio::test]
    async fn legacy_commit_uses_legacy_provider_seam_once() {
        let (context, calls) =
            new_context(CommitResult::LegacyFailure, false, false, None, None, None);

        assert!(matches!(
            context.commit().await,
            Err(OrmError::Transaction(_))
        ));
        assert_eq!(
            context.lifecycle_state().await,
            TransactionContextState::CommitOutcomeUnknown
        );
        assert_eq!(calls.commits.load(Ordering::SeqCst), 1);
        assert_eq!(calls.classified_commits.load(Ordering::SeqCst), 0);
        assert!(context.commit().await.is_err());
        assert_eq!(calls.commits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn classified_commit_records_every_terminal_outcome_once() {
        for (result, expected, expected_outcome) in [
            (
                CommitResult::Success,
                TransactionContextState::Committed,
                None,
            ),
            (
                CommitResult::DefinitelyAborted,
                TransactionContextState::DefinitelyAborted,
                Some(SdkCommitFailureOutcome::DefinitelyAborted),
            ),
            (
                CommitResult::Unknown,
                TransactionContextState::CommitOutcomeUnknown,
                Some(SdkCommitFailureOutcome::Unknown),
            ),
        ] {
            let (context, calls) = new_context(result, false, false, None, None, None);
            let commit = context.commit_classified().await;
            assert_eq!(context.lifecycle_state().await, expected);
            assert_eq!(
                commit
                    .as_ref()
                    .err()
                    .and_then(ClassifiedCommitError::commit_failure_certainty)
                    .map(|certainty| match certainty {
                        CommitFailureCertainty::DefinitelyAborted => {
                            SdkCommitFailureOutcome::DefinitelyAborted
                        }
                        CommitFailureCertainty::Unknown => SdkCommitFailureOutcome::Unknown,
                    }),
                expected_outcome
            );
            assert_eq!(calls.classified_commits.load(Ordering::SeqCst), 1);
            assert!(context.commit_classified().await.is_err());
            assert_eq!(calls.classified_commits.load(Ordering::SeqCst), 1);
            context.close().await.unwrap();
            assert_eq!(context.lifecycle_state().await, expected);
            assert_eq!(calls.closes.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn rollback_succeeds_from_active_or_rollback_only_and_is_idempotent() {
        for latch in [false, true] {
            let (context, calls) = active_context();
            if latch {
                context
                    .latch_rollback_only(&SdkExecutionDiagnostic::internal_failure())
                    .await;
            }
            context.rollback().await.unwrap();
            context.clone().rollback().await.unwrap();
            assert_eq!(
                context.lifecycle_state().await,
                TransactionContextState::RolledBack
            );
            assert_eq!(calls.rollbacks.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn rollback_then_query_then_commit_reuses_the_same_provider() {
        let (context, calls) = active_context();
        context
            .latch_rollback_only(&SdkExecutionDiagnostic::internal_failure())
            .await;
        context.rollback().await.unwrap();
        assert!(context.rollback_only_cause().await.is_none());
        context.query("match $x isa marker;").await.unwrap();
        assert_eq!(
            context.lifecycle_state().await,
            TransactionContextState::Active
        );
        context.commit().await.unwrap();
        assert_eq!(calls.rollbacks.load(Ordering::SeqCst), 1);
        assert_eq!(calls.queries.load(Ordering::SeqCst), 1);
        assert_eq!(calls.commits.load(Ordering::SeqCst), 1);
        assert_eq!(calls.closes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn failed_rollback_closes_provider_and_retains_the_first_redacted_cause() {
        let (context, calls) = new_context(CommitResult::Success, true, false, None, None, None);
        let first = SdkExecutionDiagnostic::provider_failure(SdkProviderOperation::Write);
        context.latch_rollback_only(&first).await;

        let error = context.rollback().await.unwrap_err();
        assert!(error.to_string().contains("private rollback path secret"));
        assert_eq!(
            context.lifecycle_state().await,
            TransactionContextState::Closed
        );
        assert_eq!(context.rollback_only_cause().await, Some(first));
        assert!(context.rollback().await.is_err());
        assert_eq!(calls.rollbacks.load(Ordering::SeqCst), 1);

        let (context, _) = new_context(CommitResult::Success, true, false, None, None, None);
        context.rollback().await.unwrap_err();
        let cause = context.rollback_only_cause().await.unwrap();
        assert_eq!(cause.code().as_str(), "provider_operation_failed");
        assert!(!format!("{cause:?}").contains("private rollback path secret"));
    }

    #[tokio::test]
    async fn close_dispatches_once_and_is_terminal_even_on_error() {
        for (latch, fails) in [(false, false), (true, false), (false, true)] {
            let (context, calls) =
                new_context(CommitResult::Success, false, fails, None, None, None);
            if latch {
                context
                    .latch_rollback_only(&SdkExecutionDiagnostic::internal_failure())
                    .await;
            }
            assert_eq!(context.close().await.is_err(), fails);
            context.close().await.unwrap();
            assert_eq!(
                context.lifecycle_state().await,
                TransactionContextState::Closed
            );
            assert_eq!(calls.closes.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn cancelled_lifecycle_awaits_leave_conservative_shared_states() {
        let commit_gate = Arc::new(Gate::default());
        let (context, calls) = new_context(
            CommitResult::Success,
            false,
            false,
            Some(Arc::clone(&commit_gate)),
            None,
            None,
        );
        let task = tokio::spawn({
            let context = context.clone();
            async move { context.commit_classified().await }
        });
        commit_gate.entered.notified().await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(
            context.lifecycle_state().await,
            TransactionContextState::CommitOutcomeUnknown
        );
        assert_eq!(calls.classified_commits.load(Ordering::SeqCst), 1);

        let rollback_gate = Arc::new(Gate::default());
        let (context, calls) = new_context(
            CommitResult::Success,
            false,
            false,
            None,
            Some(Arc::clone(&rollback_gate)),
            None,
        );
        let task = tokio::spawn({
            let context = context.clone();
            async move { context.rollback().await }
        });
        rollback_gate.entered.notified().await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(
            context.lifecycle_state().await,
            TransactionContextState::Closed
        );
        assert_eq!(calls.rollbacks.load(Ordering::SeqCst), 1);

        let close_gate = Arc::new(Gate::default());
        let (context, calls) = new_context(
            CommitResult::Success,
            false,
            false,
            None,
            None,
            Some(Arc::clone(&close_gate)),
        );
        let task = tokio::spawn({
            let context = context.clone();
            async move { context.close().await }
        });
        close_gate.entered.notified().await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(
            context.lifecycle_state().await,
            TransactionContextState::Closed
        );
        assert_eq!(calls.closes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancelled_deletion_snapshot_cannot_commit_its_borrowed_transaction() {
        let calls = Arc::new(Calls::default());
        let gate = Arc::new(Gate::default());
        let context = TransactionContext::new(
            super::super::eager::EagerTransaction::new(
                Box::new(LifecycleTransaction {
                    calls: Arc::clone(&calls),
                    commit_result: CommitResult::Success,
                    rollback_fails: false,
                    close_fails: false,
                    commit_gate: None,
                    rollback_gate: None,
                    close_gate: None,
                    deletion_gate: Some(Arc::clone(&gate)),
                }),
                TxType::Write,
                None,
            ),
            TxType::Write,
            CapabilitySet::new(),
            None,
            DatabaseExecutionIdentity::isolated("snapshot-cancellation-test"),
            None,
        );
        let task = tokio::spawn({
            let context = context.clone();
            async move {
                let plan = crate::query_plan_exec::race::delete_doctors_returning();
                let query = crate::query_plan_exec::staged_query(&plan, false).unwrap();
                context.dispatch_plan(query).await
            }
        });
        gate.entered.notified().await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(calls.queries.load(Ordering::SeqCst), 2);
        assert_eq!(
            context.lifecycle_state().await,
            TransactionContextState::RollbackOnly
        );
        assert!(context.commit().await.is_err());
        assert_eq!(calls.classified_commits.load(Ordering::SeqCst), 0);
        context.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn malformed_deletion_snapshot_poison_latches_the_borrowed_transaction() {
        let (context, calls) = active_context();
        let plan = crate::query_plan_exec::race::delete_doctors_returning();
        let query = crate::query_plan_exec::staged_query(&plan, false).unwrap();
        assert!(context.dispatch_plan(query).await.is_err());
        assert_eq!(calls.queries.load(Ordering::SeqCst), 1);
        assert_eq!(
            context.lifecycle_state().await,
            TransactionContextState::RollbackOnly
        );
        assert_eq!(
            context.rollback_only_cause().await.unwrap().code().as_str(),
            "query_remote_v3_answer_kind"
        );
        assert!(context.commit().await.is_err());
        assert_eq!(calls.classified_commits.load(Ordering::SeqCst), 0);
        context.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn mutation_lease_predispatch_drop_and_completed_success_leave_active() {
        let (context, _) = active_context();
        drop(context.acquire_mutation_lease().await.unwrap());
        assert_eq!(
            context.lifecycle_state().await,
            TransactionContextState::Active
        );

        context
            .acquire_mutation_lease()
            .await
            .unwrap()
            .complete_success();
        assert_eq!(
            context.lifecycle_state().await,
            TransactionContextState::Active
        );
        assert!(context.rollback_only_cause().await.is_none());
    }

    #[tokio::test]
    async fn mutation_lease_possible_effect_drop_latches_first_exact_redacted_cause() {
        let (context, _) = active_context();
        let exact = SdkExecutionDiagnostic::provider_failure(SdkProviderOperation::Write);
        {
            let mut lease = context.acquire_mutation_lease().await.unwrap();
            lease.phase = MutationLeasePhase::PossibleEffect;
            lease.record_failure(&exact);
            lease.record_failure(&SdkExecutionDiagnostic::internal_failure());
        }
        assert_eq!(
            context.lifecycle_state().await,
            TransactionContextState::RollbackOnly
        );
        assert_eq!(context.rollback_only_cause().await, Some(exact));
    }

    #[tokio::test]
    async fn cancelled_or_panicking_armed_lease_synchronously_poison_clones() {
        let (context, _) = active_context();
        let entered = Arc::new(Notify::new());
        let task = tokio::spawn({
            let context = context.clone();
            let entered = Arc::clone(&entered);
            async move {
                let mut lease = context.acquire_mutation_lease().await.unwrap();
                lease.phase = MutationLeasePhase::PossibleEffect;
                entered.notify_one();
                pending::<()>().await;
            }
        });
        entered.notified().await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(
            context.lifecycle_state().await,
            TransactionContextState::RollbackOnly
        );
        assert_eq!(
            context.rollback_only_cause().await.unwrap().code().as_str(),
            "internal_failure"
        );

        let (context, _) = active_context();
        let task = tokio::spawn({
            let context = context.clone();
            async move {
                let mut lease = context.acquire_mutation_lease().await.unwrap();
                lease.phase = MutationLeasePhase::PossibleEffect;
                panic!("test panic while mutation effect is possible");
            }
        });
        assert!(task.await.unwrap_err().is_panic());
        assert_eq!(
            context.lifecycle_state().await,
            TransactionContextState::RollbackOnly
        );
    }

    #[tokio::test]
    async fn clone_commit_waits_for_the_same_mutation_mutex() {
        let (context, calls) = active_context();
        let lease = context.acquire_mutation_lease().await.unwrap();
        let commit = tokio::spawn({
            let context = context.clone();
            async move { context.commit_sdk().await }
        });
        tokio::task::yield_now().await;
        assert_eq!(calls.classified_commits.load(Ordering::SeqCst), 0);
        drop(lease);
        commit.await.unwrap().unwrap();
        assert_eq!(calls.classified_commits.load(Ordering::SeqCst), 1);
        assert_eq!(
            context.lifecycle_state().await,
            TransactionContextState::Committed
        );
    }

    #[tokio::test]
    async fn bounded_v2_read_preserves_active_state_and_checks_control_before_query() {
        let (context, calls) = active_context();
        let mut consumer = |_item: super::super::backend::AnswerItem| {
            Ok(super::super::backend::AnswerControl::Continue)
        };
        let cancellation = super::super::backend::AnswerCancellation::default();
        cancellation.cancel();
        let error = context
            .query_v2_bounded(
                "match $x isa thing; fetch { iid: iid($x) };",
                AnswerLimits {
                    answer: BoundedAnswerLimits {
                        cancellation,
                        ..BoundedAnswerLimits::default()
                    },
                    max_collection_members: 1,
                },
                &mut consumer,
            )
            .await
            .unwrap_err();

        assert_eq!(calls.queries.load(Ordering::SeqCst), 0);
        assert_eq!(
            context.lifecycle_state().await,
            TransactionContextState::Active
        );
        assert!(matches!(error, OrmError::Match(_)));

        let stats = context
            .query_v2_bounded(
                "match $x isa thing; fetch { iid: iid($x) };",
                AnswerLimits::default(),
                &mut consumer,
            )
            .await
            .unwrap();
        assert_eq!(stats, BoundedAnswerStats::default());
        assert_eq!(calls.queries.load(Ordering::SeqCst), 1);
        assert_eq!(
            context.lifecycle_state().await,
            TransactionContextState::Active
        );
    }
}
