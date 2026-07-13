//! Backfill count derivation for [`StepKind::Backfill`] steps.
//!
//! TypeDB's `insert` answer is [`QueryResult::Ok`] — it carries no affected-row
//! count.  This module derives matched/inserted/skipped counts via bracketing
//! `reduce $c = count;` read queries around the write (D2).
//!
//! # Cost
//!
//! Two extra read-transaction count queries are issued per backfill step
//! (guarded count before insert, total-source count before insert).  This is
//! acceptable for one-shot migrations and is the only way to surface counts given
//! TypeDB's write answer surface.
//!
//! # Invariant 2 compliance
//!
//! The count queries are composed directly from the carried `step.forward` match
//! clause — no op-semantic re-derivation occurs here.  The backfill step's
//! `forward` text is the single source of truth for the query shape.

use type_bridge_orm::Database;
use type_bridge_orm::session::TransactionContext;
use type_bridge_orm::session::backend::QueryResult;

use crate::error::MigrationError;
use crate::plan::ExecutionStep;

use serde::{Deserialize, Serialize};
use type_bridge_orm::TxType;

/// Per-step backfill count result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackfillResult {
    /// Zero-based index of the step within the migration's step list.
    pub step_index: usize,
    /// Number of owner instances that matched the source predicate (the total
    /// candidate set, including those already having the destination).
    pub matched: u64,
    /// Number of instances that were actually backfilled (destination inserted).
    ///
    /// Computed as the count of the guarded match (insert-if-absent inserts
    /// exactly the guarded set).
    pub inserted: u64,
    /// Number of instances skipped because they already held the destination
    /// attribute (`matched - inserted`).
    pub skipped: u64,
    /// Number of instances with conflicting source values.  Always `0` for the
    /// single-valued v1 `CopyAttribute` op; reserved for future multi-valued ops.
    pub conflicts: u64,
}

/// Prepared backfill whose write query has succeeded but is not yet committed.
///
/// Kept crate-private so the recovery executor can durably emit its
/// before-commit event in the only safe gap between query success and commit.
pub(crate) struct PreparedBackfill {
    pub(crate) transaction: TransactionContext,
    pub(crate) result: BackfillResult,
}

/// Execute a backfill [`ExecutionStep`] against `db`, deriving and returning
/// matched/inserted/skipped counts.
///
/// # Execution sequence
///
/// 1. Run a `Read` count query over the **guarded** match clause (the
///    insert-if-absent predicate — rows that will be inserted).
/// 2. Run a `Read` count query over the **unguarded** match clause (all
///    candidate rows, including those already having the destination).
/// 3. Run the forward TypeQL under a `Write` transaction and commit.
///
/// Steps 1 and 2 run before the write so the counts reflect the pre-write state.
///
/// # Invariant 2
///
/// Both count queries are derived from `step.forward` by string manipulation of
/// the carried match clause.  No `OperationSpec` is inspected here.
pub async fn execute_backfill(
    db: &Database,
    step: &ExecutionStep,
    step_index: usize,
) -> Result<BackfillResult, MigrationError> {
    let prepared = prepare_backfill(db, step, step_index).await?;
    prepared
        .transaction
        .commit()
        .await
        .map_err(|e| MigrationError::BackfillQuery {
            message: format!("backfill step {step_index}: backfill write commit failed: {e}"),
        })?;
    Ok(prepared.result)
}

/// Execute the read counts and write query for a backfill without committing.
pub(crate) async fn prepare_backfill(
    db: &Database,
    step: &ExecutionStep,
    step_index: usize,
) -> Result<PreparedBackfill, MigrationError> {
    // ── Decompose the carried forward text ──────────────────────────────────
    //
    // The planner writes CopyAttribute steps in the form:
    //   "match\n  ...\n  not { ... };\n...\ninsert\n  ..."
    //
    // We split on "\ninsert\n" to extract the match section.
    let (match_section, _insert_section) = step
        .forward
        .split_once("\ninsert\n")
        .ok_or_else(|| MigrationError::BackfillQuery {
            message: format!(
                "backfill step {step_index}: forward TypeQL does not contain '\\ninsert\\n' separator; \
                 cannot compose count queries without re-deriving semantics (invariant 2)"
            ),
        })?;

    // ── Guarded count: rows the insert will affect (insert-if-absent set) ──
    //
    // The match section already contains the `not { $x has <dest> $d; }` guard,
    // so this count == inserted.
    let guarded_count_query = format!("{match_section}\nreduce $c = count;");

    // ── Unguarded count: all candidate rows (matched) ───────────────────────
    //
    // Strip the `not { ... }` line from the match section.  The guard line is
    // always of the form `  not { ... };` in the planner output.
    let unguarded_match = strip_not_guard(match_section);
    let total_count_query = format!("{unguarded_match}\nreduce $c = count;");

    // ── Run guarded count (Read tx) ─────────────────────────────────────────
    let inserted: u64 = {
        let ctx = db.transaction_context(TxType::Read).await.map_err(|e| {
            MigrationError::BackfillQuery {
                message: format!(
                    "backfill step {step_index}: failed to open read tx for guarded count: {e}"
                ),
            }
        })?;
        let result =
            ctx.query(&guarded_count_query)
                .await
                .map_err(|e| MigrationError::BackfillQuery {
                    message: format!("backfill step {step_index}: guarded count query failed: {e}"),
                })?;
        // Rollback / close (read txs don't need commit; best-effort close).
        let _ = ctx.rollback().await;
        extract_count(result, step_index, "guarded")?
    };

    // ── Run total count (Read tx) ───────────────────────────────────────────
    let matched: u64 = {
        let ctx = db.transaction_context(TxType::Read).await.map_err(|e| {
            MigrationError::BackfillQuery {
                message: format!(
                    "backfill step {step_index}: failed to open read tx for total count: {e}"
                ),
            }
        })?;
        let result =
            ctx.query(&total_count_query)
                .await
                .map_err(|e| MigrationError::BackfillQuery {
                    message: format!("backfill step {step_index}: total count query failed: {e}"),
                })?;
        let _ = ctx.rollback().await;
        extract_count(result, step_index, "total")?
    };

    // ── Prepare the backfill write (Write tx, deliberately uncommitted) ─────
    let transaction =
        db.transaction_context(TxType::Write)
            .await
            .map_err(|e| MigrationError::BackfillQuery {
                message: format!("backfill step {step_index}: failed to open write tx: {e}"),
            })?;
    if let Err(error) = transaction.query(&step.forward).await {
        let _ = transaction.rollback().await;
        return Err(MigrationError::BackfillQuery {
            message: format!("backfill step {step_index}: backfill write query failed: {error}"),
        });
    }

    let skipped = matched.saturating_sub(inserted);

    Ok(PreparedBackfill {
        transaction,
        result: BackfillResult {
            step_index,
            matched,
            inserted,
            skipped,
            conflicts: 0,
        },
    })
}

/// Extract the count value from a `reduce $c = count;` answer.
///
/// TypeDB returns the answer keyed by the reduce variable (`c`) carrying a value
/// envelope; the in-memory mock may use any key and a bare number. Both shapes
/// are reduced via [`scalar_to_u64`]; an empty result set falls back to `0`.
fn extract_count(
    result: QueryResult,
    step_index: usize,
    label: &str,
) -> Result<u64, MigrationError> {
    // Both Rows and Documents carry the reduce answer as a single object keyed
    // by the reduce variable (`c`); the mock may use any key, so we fall back to
    // the first value. The answer itself is a TypeDB value envelope that
    // `scalar_to_u64` unwraps.
    let answer = match result {
        QueryResult::Rows(items) | QueryResult::Documents(items) => match items.first() {
            Some(item) => item.clone(),
            None => return Ok(0),
        },
        // QueryResult::Ok from a read-reduce query is unexpected; treat as 0.
        QueryResult::Ok => return Ok(0),
    };

    let value = answer
        .get("c")
        .or_else(|| answer.as_object().and_then(|m| m.values().next()))
        .ok_or_else(|| MigrationError::BackfillQuery {
            message: format!(
                "backfill step {step_index}: {label} count answer has no recognizable key: {answer}"
            ),
        })?;

    scalar_to_u64(value).ok_or_else(|| MigrationError::BackfillQuery {
        message: format!(
            "backfill step {step_index}: {label} count value is not a number: {value}"
        ),
    })
}

/// Reduce a TypeDB `reduce $c = count;` answer to a `u64`.
///
/// TypeDB returns the count as a value document
/// `{"category":"Value","label":"integer","value":N,"value_type":"integer"}`;
/// the in-memory mock returns a bare number. Descend into a `"value"` field when
/// the answer is an object so both shapes parse identically (the same envelope
/// the state backend unwraps in `state::typedb::extract_scalar`).
fn scalar_to_u64(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(_) => value.as_u64().or_else(|| value.as_f64().map(|f| f as u64)),
        serde_json::Value::Object(map) => map.get("value").and_then(scalar_to_u64),
        _ => None,
    }
}

/// Remove the `not { ... };` guard line from a match section.
///
/// The planner always writes the guard on a single line starting with
/// `  not { ` (two-space indent).  We strip any line whose trimmed form starts
/// with `not {`.
fn strip_not_guard(match_section: &str) -> String {
    match_section
        .lines()
        .filter(|line| !line.trim_start().starts_with("not {"))
        .collect::<Vec<_>>()
        .join("\n")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::StepKind;
    use crate::testing::{MockEvent, MockMigrationBackend};
    use type_bridge_orm::{Database, TxType};

    // Build a backfill ExecutionStep carrying the TypeQL the planner passes
    // through from `CopyAttribute.to_typeql()` (value-copy form `has <dest> == $v`).
    fn backfill_step() -> ExecutionStep {
        ExecutionStep {
            tx_type: TxType::Write,
            kind: StepKind::Backfill,
            operation_kind: crate::plan::OperationKind::CopyAttribute,
            forward: "match\n  $x isa person, has old-name $v;\n  not { $x has new-name $d; };\ninsert\n  $x has new-name == $v;".to_string(),
            reverse: Some("match $x isa person, has new-name $v;\ndelete $v of $x;".to_string()),
        }
    }

    // ── test: correct counts returned from scripted responses ─────────────────

    #[tokio::test]
    async fn backfill_derives_counts_from_scripted_mock_responses() {
        use serde_json::json;
        use type_bridge_orm::session::backend::QueryResult;

        // Script: guarded count = 7, total count = 10 (3 already have dest → skipped=3).
        // Transaction order: Read (guarded), Read (total), Write.
        let scripted = vec![
            // Tx 1: Read — guarded count query → 7 rows to insert
            QueryResult::Rows(vec![json!({"c": 7})]),
            // Tx 2: Read — total count query → 10 total candidates
            QueryResult::Rows(vec![json!({"c": 10})]),
            // Tx 3: Write — the backfill insert → Ok
            QueryResult::Ok,
        ];

        let (backend, log) = MockMigrationBackend::with_responses(scripted);
        let db = Database::with_backend(Box::new(backend), "test");
        let step = backfill_step();

        let result = execute_backfill(&db, &step, 0)
            .await
            .expect("execute_backfill should succeed");

        assert_eq!(result.step_index, 0);
        assert_eq!(result.inserted, 7, "inserted = guarded count");
        assert_eq!(result.matched, 10, "matched = total count");
        assert_eq!(result.skipped, 3, "skipped = matched - inserted");
        assert_eq!(result.conflicts, 0, "conflicts always 0 in v1");

        // Verify the write ran under TxType::Write and counts under TxType::Read.
        let events = log.lock().unwrap();
        // Expect: OpenTx(Read), Query(Read, ...), Rollback,
        //         OpenTx(Read), Query(Read, ...), Rollback,
        //         OpenTx(Write), Query(Write, ...), Commit
        assert!(
            matches!(events[0], MockEvent::OpenTx(TxType::Read)),
            "first tx must be Read (guarded count)"
        );
        assert!(
            matches!(events[3], MockEvent::OpenTx(TxType::Read)),
            "second tx must be Read (total count)"
        );
        assert!(
            matches!(events[6], MockEvent::OpenTx(TxType::Write)),
            "third tx must be Write (backfill insert)"
        );
        // Commit closes the write tx.
        assert!(matches!(events[8], MockEvent::Commit));
    }

    // ── test: count queries are composed from the step's carried match (invariant 2) ──

    #[tokio::test]
    async fn count_queries_are_built_from_carried_match_not_re_derived() {
        use serde_json::json;
        use type_bridge_orm::session::backend::QueryResult;

        let scripted = vec![
            QueryResult::Rows(vec![json!({"c": 0})]),
            QueryResult::Rows(vec![json!({"c": 0})]),
            QueryResult::Ok,
        ];

        let (backend, log) = MockMigrationBackend::with_responses(scripted);
        let db = Database::with_backend(Box::new(backend), "test");
        let step = backfill_step();

        execute_backfill(&db, &step, 1)
            .await
            .expect("execute_backfill should succeed");

        let events = log.lock().unwrap();

        // The first count query (guarded) must contain the match body text from the step.
        let guarded_query = events.iter().find_map(|e| {
            if let MockEvent::Query(TxType::Read, q) = e {
                Some(q.as_str())
            } else {
                None
            }
        });
        assert!(
            guarded_query.is_some(),
            "expected at least one Read query in the event log"
        );
        let q = guarded_query.unwrap();
        // The guarded count query must contain the core match pattern from the
        // step's forward text — proving no semantic re-derivation happened.
        assert!(
            q.contains("$x isa person, has old-name $v"),
            "guarded count query must contain the step's match body; got: {q}"
        );
        assert!(
            q.contains("reduce $c = count"),
            "guarded count query must end with reduce count; got: {q}"
        );
    }

    // ── test: write runs under TxType::Write ──────────────────────────────────

    #[tokio::test]
    async fn backfill_write_runs_under_write_tx() {
        use serde_json::json;
        use type_bridge_orm::session::backend::QueryResult;

        let scripted = vec![
            QueryResult::Rows(vec![json!({"c": 5})]),
            QueryResult::Rows(vec![json!({"c": 5})]),
            QueryResult::Ok,
        ];

        let (backend, log) = MockMigrationBackend::with_responses(scripted);
        let db = Database::with_backend(Box::new(backend), "test");
        let step = backfill_step();

        execute_backfill(&db, &step, 2)
            .await
            .expect("execute_backfill should succeed");

        let events = log.lock().unwrap();

        // Find the Write-typed query event.
        let write_query = events.iter().find_map(|e| {
            if let MockEvent::Query(TxType::Write, q) = e {
                Some(q.as_str())
            } else {
                None
            }
        });
        assert!(write_query.is_some(), "expected a Write-typed query event");
        // The write query must be the step's full forward text (not a count query).
        let wq = write_query.unwrap();
        assert!(
            wq.contains("insert"),
            "write query must contain 'insert'; got: {wq}"
        );
        assert!(
            !wq.contains("reduce"),
            "write query must not contain 'reduce' (it is the insert, not a count query); got: {wq}"
        );
    }

    // ── test: strip_not_guard removes the guard line ──────────────────────────

    #[test]
    fn strip_not_guard_removes_not_line() {
        let match_section =
            "match\n  $x isa person, has old-name $v;\n  not { $x has new-name $d; };";
        let stripped = strip_not_guard(match_section);
        assert!(
            !stripped.contains("not {"),
            "stripped result must not contain 'not {{': {stripped}"
        );
        assert!(
            stripped.contains("$x isa person"),
            "stripped result must preserve the main match line: {stripped}"
        );
    }

    // ── test: zero counts (empty database) ───────────────────────────────────

    #[tokio::test]
    async fn backfill_with_zero_counts_returns_zero_result() {
        use serde_json::json;
        use type_bridge_orm::session::backend::QueryResult;

        let scripted = vec![
            QueryResult::Rows(vec![json!({"c": 0})]),
            QueryResult::Rows(vec![json!({"c": 0})]),
            QueryResult::Ok,
        ];

        let (backend, _log) = MockMigrationBackend::with_responses(scripted);
        let db = Database::with_backend(Box::new(backend), "test");
        let step = backfill_step();

        let result = execute_backfill(&db, &step, 0)
            .await
            .expect("execute_backfill should succeed with zero counts");

        assert_eq!(result.matched, 0);
        assert_eq!(result.inserted, 0);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.conflicts, 0);
    }
}
