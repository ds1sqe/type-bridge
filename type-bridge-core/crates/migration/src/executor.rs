//! Async migration executor.
//!
//! Runs an [`ExecutionPlan`] produced by the planner over a [`Database`],
//! committing each step in its own transaction.  The executor is
//! runtime-agnostic: the caller (`PyMigrationRunner` in Phase 3) drives it by
//! `block_on`-ing the returned future on the shared `Arc<Runtime>`.
//!
//! # Per-step commit contract
//!
//! TypeDB forbids multiple `define` blocks per transaction.  Each
//! [`ExecutionStep`] is executed in its own transaction and committed
//! independently.  When a step fails, earlier committed steps in the same
//! migration are **not** rolled back (there is no cross-step atomicity).
//! This matches the current Python per-statement-commit behavior and is the
//! documented contract for this boundary.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use type_bridge_orm::Database;

use crate::backfill::{BackfillResult, execute_backfill};
use crate::plan::{ExecutionPlan, MigrationAction, MigrationExecution, StepKind};
use crate::state::{
    MigrationExecutorInfo, MigrationStateStore, finished_run_record, require_legacy_writer_open,
    require_legacy_writer_open_in_transaction, started_run_record,
};

/// Result of executing a single migration (one entry per attempted migration).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MigrationResult {
    /// Application or package label.
    pub app_label: String,
    /// Migration file stem, e.g. `0001_initial`.
    pub name: String,
    /// Whether this was an apply or a rollback.
    pub action: MigrationAction,
    /// `true` when all steps completed and committed successfully.
    pub success: bool,
    /// Human-readable failure reason, present only when `success` is `false`.
    pub error: Option<String>,
    /// Per-step backfill counts, present only when the migration contained at
    /// least one [`StepKind::Backfill`] step.  `None` for pure-schema migrations
    /// so the field does not appear in JSON output (no bloat — D2a).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backfill: Option<Vec<BackfillResult>>,
}

/// Execute a [`ExecutionPlan`] against `db`.
///
/// Rollbacks are processed first (already in reverse order from the planner),
/// then applies.  The run halts at the first failed migration and returns all
/// results accumulated so far including the failure.
///
/// Returns one [`MigrationResult`] per attempted migration, in execution order
/// (rollbacks then applies).
pub async fn execute_plan(db: &Database, plan: ExecutionPlan) -> Vec<MigrationResult> {
    let mut results: Vec<MigrationResult> = Vec::new();

    // Process rollbacks before applies (planner already reverse-ordered them).
    for migration in plan.to_rollback {
        let result = execute_migration(db, &migration).await;
        let should_halt = !result.success;
        results.push(result);
        if should_halt {
            return results;
        }
    }

    // Process applies.
    for migration in plan.to_apply {
        let result = execute_migration(db, &migration).await;
        let should_halt = !result.success;
        results.push(result);
        if should_halt {
            return results;
        }
    }

    results
}

/// Execute a plan while writing one DB-backed run-log row per attempted migration.
pub async fn execute_plan_with_run_log<S: MigrationStateStore>(
    db: &Database,
    store: &S,
    plan: ExecutionPlan,
    checksums: &BTreeMap<(String, String), String>,
    executor: &MigrationExecutorInfo,
) -> crate::Result<Vec<MigrationResult>> {
    let mut results: Vec<MigrationResult> = Vec::new();

    for migration in plan.to_rollback {
        let result = execute_logged_migration(db, store, &migration, checksums, executor).await?;
        let should_halt = !result.success;
        results.push(result);
        if should_halt {
            return Ok(results);
        }
    }

    for migration in plan.to_apply {
        let result = execute_logged_migration(db, store, &migration, checksums, executor).await?;
        let should_halt = !result.success;
        results.push(result);
        if should_halt {
            return Ok(results);
        }
    }

    Ok(results)
}

async fn execute_logged_migration<S: MigrationStateStore>(
    db: &Database,
    store: &S,
    migration: &MigrationExecution,
    checksums: &BTreeMap<(String, String), String>,
    executor: &MigrationExecutorInfo,
) -> crate::Result<MigrationResult> {
    // External state stores cannot share the target transaction, so reject an
    // already-cut-over target before touching their run log.  Each target
    // mutation repeats the check in its own transaction below.
    require_legacy_writer_open(db).await?;
    let checksum = checksums
        .get(&(migration.app_label.clone(), migration.name.clone()))
        .cloned()
        .unwrap_or_default();
    let run = started_run_record(migration, checksum, executor);
    store.record_run(run.clone()).await?;

    let result = execute_migration(db, migration).await;
    let status = if result.success {
        "succeeded"
    } else {
        "failed"
    };
    let finished = finished_run_record(run, status, result.error.clone());
    store.record_run(finished).await?;
    Ok(result)
}

/// Execute a single [`MigrationExecution`].
///
/// Returns one [`MigrationResult`] for this attempted migration.
pub async fn execute_migration(db: &Database, migration: &MigrationExecution) -> MigrationResult {
    // Non-reversible rollback: fail immediately without opening a transaction.
    if migration.action == MigrationAction::Rollback && !migration.reversible {
        return MigrationResult {
            app_label: migration.app_label.clone(),
            name: migration.name.clone(),
            action: migration.action,
            success: false,
            error: Some(format!("{} is not reversible", migration.name)),
            backfill: None,
        };
    }

    if let Err(error) = require_legacy_writer_open(db).await {
        return MigrationResult {
            app_label: migration.app_label.clone(),
            name: migration.name.clone(),
            action: migration.action,
            success: false,
            error: Some(error.to_string()),
            backfill: None,
        };
    }

    let mut backfill_results: Vec<BackfillResult> = Vec::new();

    for (step_index, step) in migration.steps.iter().enumerate() {
        // Backfill steps are routed through the count-deriving path.
        if step.kind == StepKind::Backfill && migration.action == MigrationAction::Apply {
            match execute_backfill(db, step, step_index).await {
                Ok(bf_result) => {
                    backfill_results.push(bf_result);
                    // Backfill execution committed internally; continue.
                    continue;
                }
                Err(e) => {
                    return MigrationResult {
                        app_label: migration.app_label.clone(),
                        name: migration.name.clone(),
                        action: migration.action,
                        success: false,
                        error: Some(format!("backfill step {step_index} failed: {e}")),
                        backfill: None,
                    };
                }
            }
        }

        // Choose forward or reverse TypeQL based on the action.
        let typeql: &str = match migration.action {
            MigrationAction::Apply => &step.forward,
            MigrationAction::Rollback => {
                // reversible was true above, so every step has a reverse.
                // Unwrap is safe here — the planner guarantees this when
                // `migration.reversible == true`.
                step.reverse.as_deref().unwrap_or(&step.forward)
            }
        };

        // Version-gate annotation-bearing schema DDL before opening a
        // transaction: pre-3.12 servers reject @doc/@meta with a syntax
        // error; the gate produces an actionable versioned error instead.
        if let Err(e) = db.check_schema_annotation_support(typeql) {
            return MigrationResult {
                app_label: migration.app_label.clone(),
                name: migration.name.clone(),
                action: migration.action,
                success: false,
                error: Some(e.to_string()),
                backfill: None,
            };
        }

        // Open a transaction for this step.
        let ctx = match db.transaction_context(step.tx_type).await {
            Ok(ctx) => ctx,
            Err(e) => {
                return MigrationResult {
                    app_label: migration.app_label.clone(),
                    name: migration.name.clone(),
                    action: migration.action,
                    success: false,
                    error: Some(format!("failed to open transaction: {e}")),
                    backfill: None,
                };
            }
        };

        if let Err(error) = require_legacy_writer_open_in_transaction(&ctx).await {
            let _ = ctx.rollback().await;
            return MigrationResult {
                app_label: migration.app_label.clone(),
                name: migration.name.clone(),
                action: migration.action,
                success: false,
                error: Some(error.to_string()),
                backfill: None,
            };
        }

        // Execute the query.
        if let Err(e) = ctx.query(typeql).await {
            // Best-effort rollback; ignore its error.
            let _ = ctx.rollback().await;
            return MigrationResult {
                app_label: migration.app_label.clone(),
                name: migration.name.clone(),
                action: migration.action,
                success: false,
                error: Some(format!("query failed: {e}")),
                backfill: None,
            };
        }

        // Commit the step.
        if let Err(e) = ctx.commit().await {
            return MigrationResult {
                app_label: migration.app_label.clone(),
                name: migration.name.clone(),
                action: migration.action,
                success: false,
                error: Some(format!("commit failed: {e}")),
                backfill: None,
            };
        }
    }

    // All steps succeeded.
    let backfill = if backfill_results.is_empty() {
        None
    } else {
        Some(backfill_results)
    };
    MigrationResult {
        app_label: migration.app_label.clone(),
        name: migration.name.clone(),
        action: migration.action,
        success: true,
        error: None,
        backfill,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::ExecutionStep;
    use crate::state::InMemoryStateStore;
    use crate::testing::{MockEvent, MockMigrationBackend};
    use type_bridge_orm::{Database, TxType};

    // ── helpers ───────────────────────────────────────────────────────────────

    fn schema_step(forward: &str, reverse: Option<&str>) -> ExecutionStep {
        ExecutionStep {
            tx_type: TxType::Schema,
            kind: crate::plan::StepKind::Schema,
            operation_kind: crate::plan::OperationKind::RunTypeql,
            forward: forward.to_string(),
            reverse: reverse.map(str::to_string),
        }
    }

    fn apply_migration(name: &str, steps: Vec<ExecutionStep>) -> MigrationExecution {
        let reversible = steps.iter().all(|s| s.reverse.is_some());
        MigrationExecution {
            app_label: "app".to_string(),
            name: name.to_string(),
            action: MigrationAction::Apply,
            steps,
            reversible,
        }
    }

    fn rollback_migration(
        name: &str,
        steps: Vec<ExecutionStep>,
        reversible: bool,
    ) -> MigrationExecution {
        MigrationExecution {
            app_label: "app".to_string(),
            name: name.to_string(),
            action: MigrationAction::Rollback,
            steps,
            reversible,
        }
    }

    #[tokio::test]
    async fn cutover_rejects_before_executor_typeql_or_run_log_mutation() {
        let (backend, log) = MockMigrationBackend::with_legacy_cutover();
        let db = Database::with_backend(Box::new(backend), "test");
        let migration = apply_migration(
            "0001_blocked",
            vec![schema_step("define entity must-not-run;", None)],
        );
        let plan = ExecutionPlan {
            to_apply: vec![migration.clone()],
            to_rollback: Vec::new(),
        };

        let results = execute_plan(&db, plan).await;
        assert_eq!(results.len(), 1);
        assert!(!results[0].success);
        assert!(
            results[0]
                .error
                .as_deref()
                .is_some_and(|error| error.contains(crate::LEGACY_WRITER_CUTOVER_MESSAGE))
        );
        assert_eq!(
            *log.lock().unwrap(),
            vec![MockEvent::OpenTx(TxType::Read), MockEvent::Close]
        );

        let store = InMemoryStateStore::new();
        let logged = execute_plan_with_run_log(
            &db,
            &store,
            ExecutionPlan {
                to_apply: vec![migration],
                to_rollback: Vec::new(),
            },
            &BTreeMap::new(),
            &MigrationExecutorInfo::default(),
        )
        .await
        .expect_err("cutover must reject before the external run log");
        assert!(
            logged
                .to_string()
                .contains(crate::LEGACY_WRITER_CUTOVER_MESSAGE)
        );
        assert!(store.load_runs().await.unwrap().is_empty());
    }

    // ── test: apply-only plan ─────────────────────────────────────────────────

    #[tokio::test]
    async fn apply_only_executes_steps_in_order_under_schema_tx() {
        let (backend, log) = MockMigrationBackend::new(None);
        let db = Database::with_backend(Box::new(backend), "test");

        let plan = ExecutionPlan {
            to_apply: vec![
                apply_migration(
                    "0001_initial",
                    vec![schema_step("define attribute a, value string;", None)],
                ),
                apply_migration(
                    "0002_add",
                    vec![schema_step(
                        "define attribute b, value string;",
                        Some("undefine attribute b;"),
                    )],
                ),
            ],
            to_rollback: Vec::new(),
        };

        let results = execute_plan(&db, plan).await;

        assert_eq!(results.len(), 2);
        assert!(results[0].success);
        assert_eq!(results[0].name, "0001_initial");
        assert!(results[1].success);
        assert_eq!(results[1].name, "0002_add");

        // Each migration first performs the read-only cutover preflight, then
        // executes its step under the intended schema transaction.
        let events = log.lock().unwrap();
        assert_eq!(
            *events,
            vec![
                MockEvent::OpenTx(TxType::Read),
                MockEvent::Close,
                MockEvent::OpenTx(TxType::Schema),
                MockEvent::Query(
                    TxType::Schema,
                    "define attribute a, value string;".to_string()
                ),
                MockEvent::Commit,
                MockEvent::OpenTx(TxType::Read),
                MockEvent::Close,
                MockEvent::OpenTx(TxType::Schema),
                MockEvent::Query(
                    TxType::Schema,
                    "define attribute b, value string;".to_string()
                ),
                MockEvent::Commit,
            ]
        );
    }

    #[tokio::test]
    async fn execute_plan_with_run_log_records_started_and_finished_rows() {
        let (backend, _log) = MockMigrationBackend::new(None);
        let db = Database::with_backend(Box::new(backend), "test");
        let store = InMemoryStateStore::new();
        let mut checksums = BTreeMap::new();
        checksums.insert(
            ("app".to_string(), "0001_initial".to_string()),
            "checksum-1".to_string(),
        );
        let executor = MigrationExecutorInfo {
            ip: Some("127.0.0.1".to_string()),
            mac: Some("00:11:22:33:44:55".to_string()),
        };
        let plan = ExecutionPlan {
            to_apply: vec![apply_migration(
                "0001_initial",
                vec![schema_step("define attribute a, value string;", None)],
            )],
            to_rollback: Vec::new(),
        };

        let results = execute_plan_with_run_log(&db, &store, plan, &checksums, &executor)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].success);
        let runs = store.load_runs().await.unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].name, "0001_initial");
        assert_eq!(runs[0].checksum, "checksum-1");
        assert_eq!(runs[0].direction, "apply");
        assert_eq!(runs[0].status, "succeeded");
        assert!(runs[0].finished_at.is_some());
        assert_eq!(runs[0].executor_ip.as_deref(), Some("127.0.0.1"));
        assert_eq!(runs[0].executor_mac.as_deref(), Some("00:11:22:33:44:55"));
    }

    // ── test: rollback path ───────────────────────────────────────────────────

    #[tokio::test]
    async fn rollback_executes_reverse_typeql_under_schema_tx() {
        let (backend, log) = MockMigrationBackend::new(None);
        let db = Database::with_backend(Box::new(backend), "test");

        let plan = ExecutionPlan {
            to_apply: Vec::new(),
            to_rollback: vec![rollback_migration(
                "0002_add",
                vec![schema_step(
                    "define attribute b, value string;",
                    Some("undefine attribute b;"),
                )],
                true,
            )],
        };

        let results = execute_plan(&db, plan).await;

        assert_eq!(results.len(), 1);
        assert!(results[0].success);
        assert_eq!(results[0].action, MigrationAction::Rollback);

        let events = log.lock().unwrap();
        assert_eq!(
            *events,
            vec![
                MockEvent::OpenTx(TxType::Read),
                MockEvent::Close,
                MockEvent::OpenTx(TxType::Schema),
                // Reverse TypeQL is used for rollback.
                MockEvent::Query(TxType::Schema, "undefine attribute b;".to_string()),
                MockEvent::Commit,
            ]
        );
    }

    // ── test: per-step-commit / no atomicity (oracle risk 1) ──────────────────
    //
    // A migration with 3 steps where the mock fails the 2nd query:
    //   - Step 1: Query + Commit (durable)
    //   - Step 2: Query fails → Rollback
    //   - Step 3: never opened
    //   - Run halts after the failure
    //   - Result: success=false

    #[tokio::test]
    async fn middle_step_failure_commits_prior_steps_and_halts() {
        // Fail the 2nd query (0-indexed: index 1).
        let (backend, log) = MockMigrationBackend::new(Some(1));
        let db = Database::with_backend(Box::new(backend), "test");

        let plan = ExecutionPlan {
            to_apply: vec![apply_migration(
                "0001_three_steps",
                vec![
                    schema_step("define attribute a, value string;", None),
                    schema_step("define attribute b, value string;", None),
                    schema_step("define attribute c, value string;", None),
                ],
            )],
            to_rollback: Vec::new(),
        };

        let results = execute_plan(&db, plan).await;

        // Only one migration attempted; it failed.
        assert_eq!(results.len(), 1);
        assert!(!results[0].success);
        assert!(results[0].error.is_some());

        let events = log.lock().unwrap();

        // Migration entry: read-only cutover guard, then close the snapshot.
        assert!(matches!(events[0], MockEvent::OpenTx(TxType::Read)));
        assert!(matches!(events[1], MockEvent::Close));

        // Step 1: OpenTx → Query → Commit (durable)
        assert!(matches!(events[2], MockEvent::OpenTx(TxType::Schema)));
        assert!(
            matches!(&events[3], MockEvent::Query(TxType::Schema, q) if q == "define attribute a, value string;")
        );
        assert!(matches!(events[4], MockEvent::Commit));

        // Step 2: OpenTx → Query (fails) → Rollback
        assert!(matches!(events[5], MockEvent::OpenTx(TxType::Schema)));
        assert!(
            matches!(&events[6], MockEvent::Query(TxType::Schema, q) if q == "define attribute b, value string;")
        );
        assert!(matches!(events[7], MockEvent::Rollback));

        // Step 3 must NOT appear — run halted after step 2 failure.
        assert_eq!(events.len(), 8, "step 3 must not be opened");
    }

    // ── test: non-reversible rollback ─────────────────────────────────────────

    #[tokio::test]
    async fn non_reversible_rollback_returns_failed_result_without_opening_tx() {
        let (backend, log) = MockMigrationBackend::new(None);
        let db = Database::with_backend(Box::new(backend), "test");

        let plan = ExecutionPlan {
            to_apply: Vec::new(),
            to_rollback: vec![rollback_migration(
                "0001_initial",
                // Steps carry no reverse — this migration is non-reversible.
                vec![schema_step("define attribute a, value string;", None)],
                false,
            )],
        };

        let results = execute_plan(&db, plan).await;

        assert_eq!(results.len(), 1);
        assert!(!results[0].success);
        assert!(
            results[0]
                .error
                .as_deref()
                .unwrap_or("")
                .contains("not reversible"),
            "error message must mention 'not reversible'; got: {:?}",
            results[0].error
        );

        // No transaction was opened.
        let events = log.lock().unwrap();
        assert!(
            events.is_empty(),
            "no events expected for non-reversible rollback; got: {events:?}"
        );
    }

    // ── test: results are in execution order (rollbacks then applies) ─────────

    #[tokio::test]
    async fn results_are_in_execution_order_rollbacks_first() {
        let (backend, _log) = MockMigrationBackend::new(None);
        let db = Database::with_backend(Box::new(backend), "test");

        let plan = ExecutionPlan {
            to_apply: vec![apply_migration(
                "0003_apply",
                vec![schema_step("define attribute c, value string;", None)],
            )],
            to_rollback: vec![rollback_migration(
                "0002_rollback",
                vec![schema_step(
                    "define attribute b, value string;",
                    Some("undefine attribute b;"),
                )],
                true,
            )],
        };

        let results = execute_plan(&db, plan).await;

        assert_eq!(results.len(), 2);
        // Rollback comes first in results.
        assert_eq!(results[0].name, "0002_rollback");
        assert_eq!(results[0].action, MigrationAction::Rollback);
        assert_eq!(results[1].name, "0003_apply");
        assert_eq!(results[1].action, MigrationAction::Apply);
    }

    // ── test: failure halts remaining migrations ──────────────────────────────

    #[tokio::test]
    async fn failure_in_first_migration_halts_remaining() {
        // Fail the very first query.
        let (backend, _log) = MockMigrationBackend::new(Some(0));
        let db = Database::with_backend(Box::new(backend), "test");

        let plan = ExecutionPlan {
            to_apply: vec![
                apply_migration(
                    "0001_fail",
                    vec![schema_step("define attribute a, value string;", None)],
                ),
                apply_migration(
                    "0002_skipped",
                    vec![schema_step("define attribute b, value string;", None)],
                ),
            ],
            to_rollback: Vec::new(),
        };

        let results = execute_plan(&db, plan).await;

        // Only the first migration result is returned; the second is never attempted.
        assert_eq!(results.len(), 1);
        assert!(!results[0].success);
        assert_eq!(results[0].name, "0001_fail");
    }
}
