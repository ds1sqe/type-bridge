//! Fail-closed per-step execution for externally owned migration ledgers.
//!
//! Recovery execution is deliberately separate from TypeBridge's migration
//! state store. A caller first prepares a [`CheckedExecutionPlan`], persists
//! or inspects its complete ordered step sequence, then supplies a
//! [`StepRecoveryController`] that classifies every step before execution.
//!
//! The controller callbacks are not atomic with TypeDB commits. A durable
//! `BeforeCommit` event narrows the uncertainty window, but a process can still
//! disappear after TypeDB commits and before `Committed` is recorded. On the
//! next attempt the controller must reconcile that step or classify it as
//! indeterminate; the executor never infers safe replay from a callback alone.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use type_bridge_orm::Database;

use crate::backfill::{BackfillResult, prepare_backfill};
use crate::error::MigrationError;
use crate::graph::AppliedMigrationRecord;
use crate::plan::{
    ExecutionPlan, ExecutionStep, MigrationAction, MigrationExecution, OperationKind, StepKind,
    plan,
};
use crate::spec::MigrationGraph;

/// Boxed future returned by [`StepRecoveryController`] callbacks.
pub type RecoveryFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Stable deterministic identity for one checked execution step.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExecutionStepId(String);

impl ExecutionStepId {
    /// Borrow the versioned hexadecimal identity.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ExecutionStepId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Artifact identity shared by all checked steps in one migration execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckedMigrationIdentity {
    /// Application or migration package label.
    pub app_label: String,
    /// Migration file stem.
    pub name: String,
    /// Checked artifact checksum.
    pub checksum: String,
    /// Apply or rollback direction.
    pub action: MigrationAction,
}

/// One checked execution step exposed before mutation begins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckedExecutionStep {
    /// Deterministic identity bound to the artifact and ordered position.
    pub id: ExecutionStepId,
    /// Checked migration identity.
    pub migration: CheckedMigrationIdentity,
    /// Zero-based position within the migration's lowered step sequence.
    pub step_index: usize,
    /// Artifact operation kind that produced the step.
    pub operation_kind: OperationKind,
    /// Transaction and TypeQL payload executed for this step.
    pub execution: ExecutionStep,
}

/// One checked migration execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckedMigrationExecution {
    /// Checked artifact and direction identity.
    pub identity: CheckedMigrationIdentity,
    /// Ordered checked steps.
    pub steps: Vec<CheckedExecutionStep>,
    /// Whether every step has a reverse operation.
    pub reversible: bool,
}

/// Checked recovery plan whose complete step sequence is inspectable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckedExecutionPlan {
    /// Migrations scheduled for apply.
    pub to_apply: Vec<CheckedMigrationExecution>,
    /// Migrations scheduled for rollback.
    pub to_rollback: Vec<CheckedMigrationExecution>,
}

impl CheckedExecutionPlan {
    /// Iterate all steps in execution order: rollbacks, then applies.
    pub fn ordered_steps(&self) -> impl Iterator<Item = &CheckedExecutionStep> {
        self.to_rollback
            .iter()
            .chain(self.to_apply.iter())
            .flat_map(|migration| migration.steps.iter())
    }
}

/// Validate, plan, and bind a migration graph for fail-closed recovery.
///
/// This is the preferred entry point for embedders. It performs the normal
/// graph and checksum-drift gates, then derives step identities directly from
/// the checksums carried by that same graph.
pub fn plan_recovery(
    graph: &MigrationGraph,
    applied: &[AppliedMigrationRecord],
    target: Option<&str>,
) -> crate::Result<CheckedExecutionPlan> {
    let execution_plan = plan(graph, applied, target)?;
    let checksums = graph
        .migrations
        .iter()
        .filter_map(|migration| {
            migration.checksum.as_ref().map(|checksum| {
                (
                    (migration.app_label.clone(), migration.name.clone()),
                    checksum.clone(),
                )
            })
        })
        .collect();
    prepare_recovery_plan(&execution_plan, &checksums)
}

/// Bind an execution plan to checked artifact checksums and stable step IDs.
///
/// This function is pure and opens no database or migration-state store. Every
/// migration in `plan` must have a non-empty checksum entry keyed by
/// `(app_label, name)`.
pub fn prepare_recovery_plan(
    plan: &ExecutionPlan,
    checksums: &BTreeMap<(String, String), String>,
) -> crate::Result<CheckedExecutionPlan> {
    Ok(CheckedExecutionPlan {
        to_apply: prepare_migrations(&plan.to_apply, checksums)?,
        to_rollback: prepare_migrations(&plan.to_rollback, checksums)?,
    })
}

fn prepare_migrations(
    migrations: &[MigrationExecution],
    checksums: &BTreeMap<(String, String), String>,
) -> crate::Result<Vec<CheckedMigrationExecution>> {
    migrations
        .iter()
        .map(|migration| {
            let checksum = checksums
                .get(&(migration.app_label.clone(), migration.name.clone()))
                .filter(|checksum| !checksum.is_empty())
                .ok_or_else(|| MigrationError::MissingRecoveryChecksum {
                    app_label: migration.app_label.clone(),
                    name: migration.name.clone(),
                })?
                .clone();
            let identity = CheckedMigrationIdentity {
                app_label: migration.app_label.clone(),
                name: migration.name.clone(),
                checksum,
                action: migration.action,
            };
            let steps = migration
                .steps
                .iter()
                .enumerate()
                .map(|(step_index, execution)| CheckedExecutionStep {
                    id: step_id(&identity, step_index, execution.operation_kind),
                    migration: identity.clone(),
                    step_index,
                    operation_kind: execution.operation_kind,
                    execution: execution.clone(),
                })
                .collect();
            Ok(CheckedMigrationExecution {
                identity,
                steps,
                reversible: migration.reversible,
            })
        })
        .collect()
}

fn step_id(
    migration: &CheckedMigrationIdentity,
    step_index: usize,
    operation_kind: OperationKind,
) -> ExecutionStepId {
    let mut digest = Sha256::new();
    hash_field(&mut digest, b"type-bridge-execution-step-v1");
    hash_field(&mut digest, migration.checksum.as_bytes());
    hash_field(&mut digest, migration.app_label.as_bytes());
    hash_field(&mut digest, migration.name.as_bytes());
    hash_field(
        &mut digest,
        match migration.action {
            MigrationAction::Apply => b"apply",
            MigrationAction::Rollback => b"rollback",
        },
    );
    hash_field(
        &mut digest,
        &u64::try_from(step_index).unwrap_or(u64::MAX).to_be_bytes(),
    );
    hash_field(&mut digest, operation_kind.as_str().as_bytes());
    ExecutionStepId(format!("tb-step-v1:{:x}", digest.finalize()))
}

fn hash_field(digest: &mut Sha256, value: &[u8]) {
    digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    digest.update(value);
}

/// Evidence supporting execution of a step currently classified as pending.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PendingProof {
    /// External state proves that the step has not committed.
    NotCommitted,
    /// Replay is safe under a named operation-specific idempotency contract.
    IdempotentReplay {
        /// Stable name of the caller-owned idempotency strategy.
        strategy: String,
    },
}

/// External recovery decision made before a checked step can execute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum StepRecoveryDecision {
    /// The step may execute under the supplied proof.
    Pending {
        /// Why execution or replay is safe.
        proof: PendingProof,
    },
    /// The step is proven committed and must be skipped.
    Applied {
        /// Optional caller-owned reconciliation evidence description.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        evidence: Option<String>,
    },
    /// The outcome cannot be proven; automatic execution must stop.
    Indeterminate {
        /// Human-readable reconciliation blocker.
        reason: String,
    },
}

/// Per-step event emitted around the TypeDB commit boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepRecoveryEventKind {
    /// The query has succeeded and the transaction is ready to commit.
    BeforeCommit,
    /// TypeDB returned a successful commit response.
    Committed,
    /// The step failed before commit was called.
    FailedBeforeCommit,
    /// Commit returned an error, so its durable outcome is not assumed.
    UnknownCommitOutcome,
}

/// Typed event delivered to an external recovery controller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepRecoveryEvent {
    /// Checked step associated with the event.
    pub step: CheckedExecutionStep,
    /// Commit-boundary event kind.
    pub kind: StepRecoveryEventKind,
    /// Failure or diagnostic detail when relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Derived counts for a prepared or committed backfill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backfill: Option<BackfillResult>,
}

/// External-ledger classification and durable event boundary.
///
/// `classify` is invoked before every checked step. It may inspect `db` to
/// reconcile schema state or apply an operation-specific strategy. Returning
/// [`StepRecoveryDecision::Indeterminate`] is the required fail-closed answer
/// whenever the outcome cannot be proven.
pub trait StepRecoveryController: Send + Sync {
    /// Classify a checked step before any transaction for it is opened.
    fn classify<'a>(
        &'a self,
        db: &'a Database,
        step: &'a CheckedExecutionStep,
    ) -> RecoveryFuture<'a, crate::Result<StepRecoveryDecision>>;

    /// Persist or otherwise handle a typed commit-boundary event.
    fn record_event<'a>(
        &'a self,
        event: StepRecoveryEvent,
    ) -> RecoveryFuture<'a, crate::Result<()>>;
}

/// Typed result of one checked step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepExecutionResult {
    /// Checked step and stable identity.
    pub step: CheckedExecutionStep,
    /// Proven execution outcome.
    pub outcome: StepExecutionOutcome,
}

/// Proven result category for one recovery step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum StepExecutionOutcome {
    /// Reconciliation proved the step applied, so it was skipped.
    Applied {
        /// Optional caller-owned evidence description.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        evidence: Option<String>,
    },
    /// The step committed and its committed event was recorded successfully.
    Committed {
        /// Derived counts for a backfill step.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        backfill: Option<BackfillResult>,
    },
    /// The step is proven not to have reached commit.
    FailedBeforeCommit {
        /// Failure detail.
        error: String,
    },
    /// The durable outcome or recovery evidence cannot be proven.
    Indeterminate {
        /// Reconciliation blocker or ambiguous commit detail.
        error: String,
    },
}

/// Terminal status for a checked migration attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryMigrationStatus {
    /// Every step was skipped as applied or committed successfully.
    Succeeded,
    /// A step failed before commit.
    Failed,
    /// A step was or became indeterminate.
    Indeterminate,
}

/// Result of one checked migration attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoveryMigrationResult {
    /// Checked migration identity.
    pub migration: CheckedMigrationIdentity,
    /// Terminal migration status.
    pub status: RecoveryMigrationStatus,
    /// Results for all classified steps up to the terminal step.
    pub steps: Vec<StepExecutionResult>,
    /// Migration-level failure summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Terminal status for a recovery plan attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryPlanStatus {
    /// Every scheduled migration succeeded.
    Succeeded,
    /// Execution stopped on a known pre-commit failure.
    Failed,
    /// Execution stopped because an outcome could not be proven.
    Indeterminate,
}

/// Results accumulated by fail-closed recovery execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoveryExecutionResult {
    /// Terminal plan status.
    pub status: RecoveryPlanStatus,
    /// Attempted migration results in execution order.
    pub migrations: Vec<RecoveryMigrationResult>,
}

/// Execute a checked plan under an external fail-closed recovery controller.
///
/// Rollbacks run first, then applies. Execution stops at the first failed or
/// indeterminate migration. No TypeBridge migration-state store is created.
pub async fn execute_recovery_plan<C: StepRecoveryController + ?Sized>(
    db: &Database,
    plan: &CheckedExecutionPlan,
    controller: &C,
) -> RecoveryExecutionResult {
    let mut migrations = Vec::new();
    for migration in plan.to_rollback.iter().chain(plan.to_apply.iter()) {
        let result = execute_recovery_migration(db, migration, controller).await;
        let terminal = result.status;
        migrations.push(result);
        match terminal {
            RecoveryMigrationStatus::Succeeded => {}
            RecoveryMigrationStatus::Failed => {
                return RecoveryExecutionResult {
                    status: RecoveryPlanStatus::Failed,
                    migrations,
                };
            }
            RecoveryMigrationStatus::Indeterminate => {
                return RecoveryExecutionResult {
                    status: RecoveryPlanStatus::Indeterminate,
                    migrations,
                };
            }
        }
    }
    RecoveryExecutionResult {
        status: RecoveryPlanStatus::Succeeded,
        migrations,
    }
}

async fn execute_recovery_migration<C: StepRecoveryController + ?Sized>(
    db: &Database,
    migration: &CheckedMigrationExecution,
    controller: &C,
) -> RecoveryMigrationResult {
    if migration.identity.action == MigrationAction::Rollback && !migration.reversible {
        return RecoveryMigrationResult {
            migration: migration.identity.clone(),
            status: RecoveryMigrationStatus::Failed,
            steps: Vec::new(),
            error: Some(format!("{} is not reversible", migration.identity.name)),
        };
    }

    let mut results = Vec::new();
    for step in &migration.steps {
        let decision = match controller.classify(db, step).await {
            Ok(decision) => decision,
            Err(error) => {
                let message = format!("failed to classify step {}: {error}", step.id);
                results.push(step_result(
                    step,
                    StepExecutionOutcome::Indeterminate {
                        error: message.clone(),
                    },
                ));
                return migration_result(
                    migration,
                    RecoveryMigrationStatus::Indeterminate,
                    results,
                    message,
                );
            }
        };

        match decision {
            StepRecoveryDecision::Applied { evidence } => {
                results.push(step_result(
                    step,
                    StepExecutionOutcome::Applied { evidence },
                ));
            }
            StepRecoveryDecision::Indeterminate { reason } => {
                results.push(step_result(
                    step,
                    StepExecutionOutcome::Indeterminate {
                        error: reason.clone(),
                    },
                ));
                return migration_result(
                    migration,
                    RecoveryMigrationStatus::Indeterminate,
                    results,
                    reason,
                );
            }
            StepRecoveryDecision::Pending { proof: _ } => {
                let outcome = execute_pending_step(db, step, controller).await;
                let terminal = match &outcome {
                    StepExecutionOutcome::Applied { .. }
                    | StepExecutionOutcome::Committed { .. } => None,
                    StepExecutionOutcome::FailedBeforeCommit { error } => {
                        Some((RecoveryMigrationStatus::Failed, error.clone()))
                    }
                    StepExecutionOutcome::Indeterminate { error } => {
                        Some((RecoveryMigrationStatus::Indeterminate, error.clone()))
                    }
                };
                results.push(step_result(step, outcome));
                if let Some((status, error)) = terminal {
                    return migration_result(migration, status, results, error);
                }
            }
        }
    }

    RecoveryMigrationResult {
        migration: migration.identity.clone(),
        status: RecoveryMigrationStatus::Succeeded,
        steps: results,
        error: None,
    }
}

async fn execute_pending_step<C: StepRecoveryController + ?Sized>(
    db: &Database,
    step: &CheckedExecutionStep,
    controller: &C,
) -> StepExecutionOutcome {
    if step.execution.kind == StepKind::Backfill && step.migration.action == MigrationAction::Apply
    {
        return execute_pending_backfill(db, step, controller).await;
    }

    let typeql = match step.migration.action {
        MigrationAction::Apply => step.execution.forward.as_str(),
        MigrationAction::Rollback => match step.execution.reverse.as_deref() {
            Some(reverse) => reverse,
            None => {
                return failed_before_commit(
                    controller,
                    step,
                    "rollback step has no reverse TypeQL".to_string(),
                    None,
                )
                .await;
            }
        },
    };

    if let Err(error) = db.check_schema_annotation_support(typeql) {
        return failed_before_commit(controller, step, error.to_string(), None).await;
    }
    let transaction = match db.transaction_context(step.execution.tx_type).await {
        Ok(transaction) => transaction,
        Err(error) => {
            return failed_before_commit(
                controller,
                step,
                format!("failed to open transaction: {error}"),
                None,
            )
            .await;
        }
    };
    if let Err(error) = transaction.query(typeql).await {
        let _ = transaction.rollback().await;
        return failed_before_commit(controller, step, format!("query failed: {error}"), None)
            .await;
    }

    if let Err(error) = controller
        .record_event(event(step, StepRecoveryEventKind::BeforeCommit, None, None))
        .await
    {
        let _ = transaction.rollback().await;
        return failed_before_commit(
            controller,
            step,
            format!("before-commit event was not recorded: {error}"),
            None,
        )
        .await;
    }

    if let Err(error) = transaction.commit().await {
        let message = format!("commit outcome is unknown: {error}");
        return unknown_commit(controller, step, message, None).await;
    }
    committed(controller, step, None).await
}

async fn execute_pending_backfill<C: StepRecoveryController + ?Sized>(
    db: &Database,
    step: &CheckedExecutionStep,
    controller: &C,
) -> StepExecutionOutcome {
    let prepared = match prepare_backfill(db, &step.execution, step.step_index).await {
        Ok(prepared) => prepared,
        Err(error) => {
            return failed_before_commit(controller, step, error.to_string(), None).await;
        }
    };
    let backfill = prepared.result.clone();
    if let Err(error) = controller
        .record_event(event(
            step,
            StepRecoveryEventKind::BeforeCommit,
            None,
            Some(backfill.clone()),
        ))
        .await
    {
        let _ = prepared.transaction.rollback().await;
        return failed_before_commit(
            controller,
            step,
            format!("before-commit event was not recorded: {error}"),
            Some(backfill),
        )
        .await;
    }
    if let Err(error) = prepared.transaction.commit().await {
        let message = format!("backfill commit outcome is unknown: {error}");
        return unknown_commit(controller, step, message, Some(backfill)).await;
    }
    committed(controller, step, Some(backfill)).await
}

async fn committed<C: StepRecoveryController + ?Sized>(
    controller: &C,
    step: &CheckedExecutionStep,
    backfill: Option<BackfillResult>,
) -> StepExecutionOutcome {
    if let Err(error) = controller
        .record_event(event(
            step,
            StepRecoveryEventKind::Committed,
            None,
            backfill.clone(),
        ))
        .await
    {
        return StepExecutionOutcome::Indeterminate {
            error: format!(
                "TypeDB committed step {}, but its committed event was not durably recorded: {error}",
                step.id
            ),
        };
    }
    StepExecutionOutcome::Committed { backfill }
}

async fn failed_before_commit<C: StepRecoveryController + ?Sized>(
    controller: &C,
    step: &CheckedExecutionStep,
    mut message: String,
    backfill: Option<BackfillResult>,
) -> StepExecutionOutcome {
    if let Err(event_error) = controller
        .record_event(event(
            step,
            StepRecoveryEventKind::FailedBeforeCommit,
            Some(message.clone()),
            backfill,
        ))
        .await
    {
        message.push_str(&format!(
            "; failed to record failed-before-commit event: {event_error}"
        ));
    }
    StepExecutionOutcome::FailedBeforeCommit { error: message }
}

async fn unknown_commit<C: StepRecoveryController + ?Sized>(
    controller: &C,
    step: &CheckedExecutionStep,
    mut message: String,
    backfill: Option<BackfillResult>,
) -> StepExecutionOutcome {
    if let Err(event_error) = controller
        .record_event(event(
            step,
            StepRecoveryEventKind::UnknownCommitOutcome,
            Some(message.clone()),
            backfill,
        ))
        .await
    {
        message.push_str(&format!(
            "; failed to record unknown-commit event: {event_error}"
        ));
    }
    StepExecutionOutcome::Indeterminate { error: message }
}

fn event(
    step: &CheckedExecutionStep,
    kind: StepRecoveryEventKind,
    message: Option<String>,
    backfill: Option<BackfillResult>,
) -> StepRecoveryEvent {
    StepRecoveryEvent {
        step: step.clone(),
        kind,
        message,
        backfill,
    }
}

fn step_result(step: &CheckedExecutionStep, outcome: StepExecutionOutcome) -> StepExecutionResult {
    StepExecutionResult {
        step: step.clone(),
        outcome,
    }
}

fn migration_result(
    migration: &CheckedMigrationExecution,
    status: RecoveryMigrationStatus,
    steps: Vec<StepExecutionResult>,
    error: String,
) -> RecoveryMigrationResult {
    RecoveryMigrationResult {
        migration: migration.identity.clone(),
        status,
        steps,
        error: Some(error),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::Mutex;

    use serde_json::json;
    use type_bridge_orm::session::backend::QueryResult;
    use type_bridge_orm::{Database, TxType};

    use super::*;
    use crate::spec::{MigrationGraph, MigrationSpec, OperationSpec};
    use crate::testing::{MockEvent, MockMigrationBackend};

    struct TestController {
        decisions: Mutex<VecDeque<StepRecoveryDecision>>,
        events: Mutex<Vec<StepRecoveryEvent>>,
        fail_event: Option<StepRecoveryEventKind>,
    }

    impl TestController {
        fn new(decisions: Vec<StepRecoveryDecision>) -> Self {
            Self {
                decisions: Mutex::new(decisions.into()),
                events: Mutex::new(Vec::new()),
                fail_event: None,
            }
        }

        fn failing_event(
            decisions: Vec<StepRecoveryDecision>,
            kind: StepRecoveryEventKind,
        ) -> Self {
            Self {
                decisions: Mutex::new(decisions.into()),
                events: Mutex::new(Vec::new()),
                fail_event: Some(kind),
            }
        }

        fn event_kinds(&self) -> Vec<StepRecoveryEventKind> {
            self.events
                .lock()
                .unwrap()
                .iter()
                .map(|event| event.kind)
                .collect()
        }
    }

    impl StepRecoveryController for TestController {
        fn classify<'a>(
            &'a self,
            _db: &'a Database,
            _step: &'a CheckedExecutionStep,
        ) -> RecoveryFuture<'a, crate::Result<StepRecoveryDecision>> {
            let decision = self.decisions.lock().unwrap().pop_front();
            Box::pin(async move {
                decision.ok_or_else(|| MigrationError::Recovery {
                    message: "test controller has no decision".to_string(),
                })
            })
        }

        fn record_event<'a>(
            &'a self,
            event: StepRecoveryEvent,
        ) -> RecoveryFuture<'a, crate::Result<()>> {
            let should_fail = self.fail_event == Some(event.kind);
            self.events.lock().unwrap().push(event);
            Box::pin(async move {
                if should_fail {
                    Err(MigrationError::Recovery {
                        message: "injected event persistence failure".to_string(),
                    })
                } else {
                    Ok(())
                }
            })
        }
    }

    fn pending() -> StepRecoveryDecision {
        StepRecoveryDecision::Pending {
            proof: PendingProof::NotCommitted,
        }
    }

    fn applied(evidence: &str) -> StepRecoveryDecision {
        StepRecoveryDecision::Applied {
            evidence: Some(evidence.to_string()),
        }
    }

    fn execution_step(
        tx_type: TxType,
        kind: StepKind,
        operation_kind: OperationKind,
        forward: &str,
    ) -> ExecutionStep {
        ExecutionStep {
            tx_type,
            kind,
            operation_kind,
            forward: forward.to_string(),
            reverse: Some(format!("reverse {forward}")),
        }
    }

    fn schema_step(forward: &str) -> ExecutionStep {
        execution_step(
            TxType::Schema,
            StepKind::Schema,
            OperationKind::AddAttribute,
            forward,
        )
    }

    fn write_step(forward: &str) -> ExecutionStep {
        execution_step(
            TxType::Write,
            StepKind::Write,
            OperationKind::RunTypeql,
            forward,
        )
    }

    fn backfill_step() -> ExecutionStep {
        execution_step(
            TxType::Write,
            StepKind::Backfill,
            OperationKind::CopyAttribute,
            "match\n  $x isa person, has old-name $v;\n  not { $x has new-name $d; };\ninsert\n  $x has new-name == $v;",
        )
    }

    fn checked_plan(steps: Vec<ExecutionStep>) -> CheckedExecutionPlan {
        let plan = ExecutionPlan {
            to_apply: vec![MigrationExecution {
                app_label: "app".to_string(),
                name: "0001_initial".to_string(),
                action: MigrationAction::Apply,
                reversible: steps.iter().all(|step| step.reverse.is_some()),
                steps,
            }],
            to_rollback: Vec::new(),
        };
        let checksums = BTreeMap::from([(
            ("app".to_string(), "0001_initial".to_string()),
            "checksum-1".to_string(),
        )]);
        prepare_recovery_plan(&plan, &checksums).unwrap()
    }

    #[test]
    fn checked_plan_exposes_stable_checksum_bound_step_sequence() {
        let steps = vec![
            schema_step("define attribute a, value string;"),
            write_step("insert $p isa person;"),
        ];
        let first = checked_plan(steps.clone());
        let second = checked_plan(steps);
        let first_ids: Vec<_> = first.ordered_steps().map(|step| step.id.clone()).collect();
        let second_ids: Vec<_> = second.ordered_steps().map(|step| step.id.clone()).collect();

        assert_eq!(first_ids, second_ids);
        assert_ne!(first_ids[0], first_ids[1]);
        assert_eq!(
            first_ids[0].as_str(),
            "tb-step-v1:0a308e166ea81d161a8c92cbf01157935fbf53b55111df3c75f5129a183b74f9"
        );
        assert_eq!(first_ids[0].as_str().len(), 75);
        assert_eq!(
            serde_json::to_value(&first).unwrap()["to_apply"][0]["steps"]
                .as_array()
                .unwrap()
                .len(),
            2
        );

        let mut different_checksums = BTreeMap::from([(
            ("app".to_string(), "0001_initial".to_string()),
            "checksum-2".to_string(),
        )]);
        let raw = ExecutionPlan {
            to_apply: vec![MigrationExecution {
                app_label: "app".to_string(),
                name: "0001_initial".to_string(),
                action: MigrationAction::Apply,
                steps: vec![schema_step("define attribute a, value string;")],
                reversible: true,
            }],
            to_rollback: Vec::new(),
        };
        let changed = prepare_recovery_plan(&raw, &different_checksums).unwrap();
        assert_ne!(first_ids[0], changed.ordered_steps().next().unwrap().id);
        different_checksums.clear();
        assert!(matches!(
            prepare_recovery_plan(&raw, &different_checksums),
            Err(MigrationError::MissingRecoveryChecksum { .. })
        ));
    }

    #[test]
    fn plan_recovery_binds_ids_to_the_validated_graph_artifact() {
        let graph = MigrationGraph {
            migrations: vec![MigrationSpec {
                app_label: "app".to_string(),
                name: "0001_initial".to_string(),
                dependencies: Vec::new(),
                operations: vec![OperationSpec::RunTypeql {
                    forward: "define attribute a, value string;".to_string(),
                    reverse: None,
                }],
                checksum: Some("artifact-checksum".to_string()),
                reversible: false,
            }],
        };

        let checked = plan_recovery(&graph, &[], None).unwrap();
        let step = checked.ordered_steps().next().unwrap();

        assert_eq!(step.migration.checksum, "artifact-checksum");
        assert_eq!(step.operation_kind, OperationKind::RunTypeql);
        assert_eq!(checked.ordered_steps().count(), 1);
    }

    #[tokio::test]
    async fn safe_resume_skips_applied_schema_and_executes_proven_pending_data() {
        let plan = checked_plan(vec![
            schema_step("define attribute a, value string;"),
            write_step("insert $p isa person;"),
        ]);
        let controller = TestController::new(vec![applied("schema introspection"), pending()]);
        let (backend, log) = MockMigrationBackend::new(None);
        let db = Database::with_backend(Box::new(backend), "test");

        let result = execute_recovery_plan(&db, &plan, &controller).await;

        assert_eq!(result.status, RecoveryPlanStatus::Succeeded);
        assert!(matches!(
            result.migrations[0].steps[0].outcome,
            StepExecutionOutcome::Applied { .. }
        ));
        assert!(matches!(
            result.migrations[0].steps[1].outcome,
            StepExecutionOutcome::Committed { .. }
        ));
        assert_eq!(
            controller.event_kinds(),
            vec![
                StepRecoveryEventKind::BeforeCommit,
                StepRecoveryEventKind::Committed
            ]
        );
        assert_eq!(
            *log.lock().unwrap(),
            vec![
                MockEvent::OpenTx(TxType::Write),
                MockEvent::Query(TxType::Write, "insert $p isa person;".to_string()),
                MockEvent::Commit,
            ]
        );
    }

    #[tokio::test]
    async fn indeterminate_run_typeql_blocks_without_replay() {
        let plan = checked_plan(vec![write_step("insert $p isa person;")]);
        let controller = TestController::new(vec![StepRecoveryDecision::Indeterminate {
            reason: "prior before-commit receipt has no matching outcome".to_string(),
        }]);
        let (backend, log) = MockMigrationBackend::new(None);
        let db = Database::with_backend(Box::new(backend), "test");

        let result = execute_recovery_plan(&db, &plan, &controller).await;

        assert_eq!(result.status, RecoveryPlanStatus::Indeterminate);
        assert!(matches!(
            result.migrations[0].steps[0].outcome,
            StepExecutionOutcome::Indeterminate { .. }
        ));
        assert!(log.lock().unwrap().is_empty());
        assert!(controller.event_kinds().is_empty());
    }

    #[tokio::test]
    async fn failure_after_earlier_commit_is_typed_failed_before_commit() {
        let plan = checked_plan(vec![
            schema_step("define attribute a, value string;"),
            schema_step("define attribute b, value string;"),
        ]);
        let controller = TestController::new(vec![pending(), pending()]);
        let (backend, _log) = MockMigrationBackend::new(Some(1));
        let db = Database::with_backend(Box::new(backend), "test");

        let result = execute_recovery_plan(&db, &plan, &controller).await;

        assert_eq!(result.status, RecoveryPlanStatus::Failed);
        assert!(matches!(
            result.migrations[0].steps[0].outcome,
            StepExecutionOutcome::Committed { .. }
        ));
        assert!(matches!(
            result.migrations[0].steps[1].outcome,
            StepExecutionOutcome::FailedBeforeCommit { .. }
        ));
        assert_eq!(
            controller.event_kinds(),
            vec![
                StepRecoveryEventKind::BeforeCommit,
                StepRecoveryEventKind::Committed,
                StepRecoveryEventKind::FailedBeforeCommit,
            ]
        );
    }

    #[tokio::test]
    async fn ambiguous_commit_response_is_indeterminate() {
        let plan = checked_plan(vec![schema_step("define attribute a, value string;")]);
        let controller = TestController::new(vec![pending()]);
        let (backend, _log) = MockMigrationBackend::with_commit_failure(0);
        let db = Database::with_backend(Box::new(backend), "test");

        let result = execute_recovery_plan(&db, &plan, &controller).await;

        assert_eq!(result.status, RecoveryPlanStatus::Indeterminate);
        assert!(matches!(
            result.migrations[0].steps[0].outcome,
            StepExecutionOutcome::Indeterminate { .. }
        ));
        assert_eq!(
            controller.event_kinds(),
            vec![
                StepRecoveryEventKind::BeforeCommit,
                StepRecoveryEventKind::UnknownCommitOutcome,
            ]
        );
    }

    #[tokio::test]
    async fn lost_committed_event_delivery_is_indeterminate_and_halts() {
        let plan = checked_plan(vec![
            schema_step("define attribute a, value string;"),
            schema_step("define attribute b, value string;"),
        ]);
        let controller = TestController::failing_event(
            vec![pending(), pending()],
            StepRecoveryEventKind::Committed,
        );
        let (backend, log) = MockMigrationBackend::new(None);
        let db = Database::with_backend(Box::new(backend), "test");

        let result = execute_recovery_plan(&db, &plan, &controller).await;

        assert_eq!(result.status, RecoveryPlanStatus::Indeterminate);
        assert_eq!(result.migrations[0].steps.len(), 1);
        assert!(matches!(
            result.migrations[0].steps[0].outcome,
            StepExecutionOutcome::Indeterminate { .. }
        ));
        assert_eq!(
            log.lock()
                .unwrap()
                .iter()
                .filter(|event| matches!(event, MockEvent::Commit))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn final_external_applied_record_retry_skips_all_proven_steps() {
        let plan = checked_plan(vec![
            schema_step("define attribute a, value string;"),
            write_step("insert $p isa person;"),
        ]);
        let controller = TestController::new(vec![
            applied("durable step receipt"),
            applied("operation-specific reconciliation"),
        ]);
        let (backend, log) = MockMigrationBackend::new(None);
        let db = Database::with_backend(Box::new(backend), "test");

        let result = execute_recovery_plan(&db, &plan, &controller).await;

        assert_eq!(result.status, RecoveryPlanStatus::Succeeded);
        assert!(
            result.migrations[0]
                .steps
                .iter()
                .all(|step| matches!(step.outcome, StepExecutionOutcome::Applied { .. }))
        );
        assert!(log.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn backfill_emits_counts_on_both_commit_boundary_events() {
        let plan = checked_plan(vec![backfill_step()]);
        let controller = TestController::new(vec![StepRecoveryDecision::Pending {
            proof: PendingProof::IdempotentReplay {
                strategy: "copy-if-absent".to_string(),
            },
        }]);
        let responses = vec![
            QueryResult::Rows(vec![json!({"c": 4})]),
            QueryResult::Rows(vec![json!({"c": 7})]),
            QueryResult::Ok,
        ];
        let (backend, _log) = MockMigrationBackend::with_responses(responses);
        let db = Database::with_backend(Box::new(backend), "test");

        let result = execute_recovery_plan(&db, &plan, &controller).await;

        assert_eq!(result.status, RecoveryPlanStatus::Succeeded);
        let events = controller.events.lock().unwrap();
        assert_eq!(events.len(), 2);
        for event in events.iter() {
            let counts = event.backfill.as_ref().unwrap();
            assert_eq!(counts.inserted, 4);
            assert_eq!(counts.matched, 7);
            assert_eq!(counts.skipped, 3);
        }
        assert!(matches!(
            result.migrations[0].steps[0].outcome,
            StepExecutionOutcome::Committed { backfill: Some(_) }
        ));
    }
}
