//! Native copy counts execute two planned reductions before the write: the
//! guarded candidate set and the full source set, each in a read transaction.
//! Recovery retains the write transaction until its before-commit event is durable.

use type_bridge_orm::Database;
use type_bridge_orm::session::TransactionContext;

use crate::error::MigrationError;
use crate::plan::ExecutionStep;
use crate::state::require_legacy_writer_open_in_transaction;

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
    pub(crate) result: Option<BackfillResult>,
}

/// Execute a backfill [`ExecutionStep`] against `db`, deriving and returning
/// matched/inserted/skipped counts. Explicit raw steps return `None` and run
/// no derived count queries.
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
/// Native count programs are carried alongside the mutation and validated
/// against the same schema authority; execution does not parse mutation text.
pub async fn execute_backfill(
    db: &Database,
    step: &ExecutionStep,
    step_index: usize,
) -> Result<Option<BackfillResult>, MigrationError> {
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
    if let Some(data) = &step.data {
        return prepare_native_backfill(db, step, data, step_index).await;
    }
    if step.operation_kind != crate::plan::OperationKind::RunTypeql {
        return Err(MigrationError::BackfillQuery {
            message: "CopyAttribute is missing native count plans".into(),
        });
    }
    Ok(PreparedBackfill {
        transaction: prepare_write(db, step, step_index).await?,
        result: None,
    })
}

async fn prepare_write(
    db: &Database,
    step: &ExecutionStep,
    step_index: usize,
) -> Result<TransactionContext, MigrationError> {
    let error = |cause| MigrationError::BackfillQuery {
        message: format!("backfill step {step_index}: {cause}"),
    };
    let transaction = db
        .transaction_context(TxType::Write)
        .await
        .map_err(|e| error(e.to_string()))?;
    let result = async {
        require_legacy_writer_open_in_transaction(&transaction).await?;
        step.execute_program(&transaction, crate::plan::MigrationAction::Apply)
            .await
            .map_err(|e| error(e.to_string()))
    }
    .await;
    if let Err(cause) = result {
        let _ = transaction.rollback().await;
        let _ = transaction.close().await;
        return Err(cause);
    }
    Ok(transaction)
}

async fn prepare_native_backfill(
    db: &Database,
    step: &ExecutionStep,
    data: &crate::NativeDataStep,
    step_index: usize,
) -> Result<PreparedBackfill, MigrationError> {
    let error = |message: String| MigrationError::BackfillQuery {
        message: format!("backfill step {step_index}: {message}"),
    };
    let counts = data
        .counts()
        .ok_or_else(|| error("native count plans are missing".into()))?;
    let mut values = [0; 2];
    for (index, plan) in counts.iter().enumerate() {
        let transaction = db
            .transaction_context(TxType::Read)
            .await
            .map_err(|e| error(e.to_string()))?;
        let limits = type_bridge_orm::QueryExecutionResourceLimits::default();
        let result = transaction
            .execute_plans(
                std::slice::from_ref(plan),
                &limits,
                &type_bridge_orm::AnswerCancellation::default(),
                type_bridge_orm::QueryExecutionDeadline::for_limits(limits),
            )
            .await;
        let _ = transaction.rollback().await;
        let _ = transaction.close().await;
        let mut answers = result.map_err(|e| error(e.to_string()))?;
        let answer = answers
            .pop()
            .ok_or_else(|| error("count answer is missing".into()))?;
        let rows = answer.scalar_rows(plan).map_err(|e| error(e.to_string()))?;
        let [row] = rows.as_slice() else {
            return Err(error("count must return exactly one row".into()));
        };
        let [type_bridge_contract::value::CanonicalValue::Long(value)] = row.as_slice() else {
            return Err(error("count must return one integer".into()));
        };
        values[index] =
            u64::try_from(*value).map_err(|_| error("count must be nonnegative".into()))?;
    }
    let [inserted, matched] = values;
    let transaction = prepare_write(db, step, step_index).await?;
    Ok(PreparedBackfill {
        transaction,
        result: Some(BackfillResult {
            step_index,
            matched,
            inserted,
            skipped: matched.saturating_sub(inserted),
            conflicts: 0,
        }),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::testing::{MockEvent, MockMigrationBackend};
    use type_bridge_orm::session::backend::QueryResult;

    pub(crate) fn count(value: i64) -> QueryResult {
        QueryResult::Rows(vec![
            serde_json::json!({"c": type_bridge_contract::query_remote::RemoteValue::Value {
                value: type_bridge_contract::value::CanonicalValue::Long(value),
            }}),
        ])
    }

    #[tokio::test]
    async fn raw_backfill_sends_exactly_one_unchanged_statement_and_has_no_counts() {
        let text = "# opaque bytes\nprovider-specific text @doc \"untouched\";";
        let step = ExecutionStep {
            data: None,
            tx_type: TxType::Write,
            kind: crate::StepKind::Backfill,
            operation_kind: crate::OperationKind::RunTypeql,
            forward: text.into(),
            reverse: None,
        };
        let (backend, log) = MockMigrationBackend::new(None);
        let db = Database::with_backend(Box::new(backend), "test");
        assert_eq!(execute_backfill(&db, &step, 0).await.unwrap(), None);
        assert_eq!(
            *log.lock().unwrap(),
            vec![
                MockEvent::OpenTx(TxType::Write),
                MockEvent::Query(TxType::Write, text.into()),
                MockEvent::Commit
            ]
        );
    }

    #[tokio::test]
    async fn native_counts_preserve_transaction_sequence_and_report_zero_or_nonzero() {
        for (inserted, matched) in [(7, 10), (0, 0)] {
            let (backend, log) = MockMigrationBackend::with_responses(vec![
                count(inserted),
                count(matched),
                QueryResult::Rows(vec![]),
            ]);
            let db = Database::with_backend(Box::new(backend), "test");
            let step = crate::copy_plan::tests::step();
            let result = execute_backfill(&db, &step, 4).await.unwrap().unwrap();
            assert_eq!(
                result,
                BackfillResult {
                    step_index: 4,
                    inserted: inserted as u64,
                    matched: matched as u64,
                    skipped: (matched - inserted) as u64,
                    conflicts: 0
                }
            );
            let events = log.lock().unwrap();
            assert!(matches!(events[0], MockEvent::OpenTx(TxType::Read)));
            assert!(matches!(events[4], MockEvent::OpenTx(TxType::Read)));
            assert!(matches!(events[8], MockEvent::OpenTx(TxType::Write)));
            assert!(matches!(events[10], MockEvent::Commit));
            let queries = events
                .iter()
                .filter_map(|e| match e {
                    MockEvent::Query(_, q) => Some(q),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(queries.len(), 3);
            assert!(queries[0].contains("not {"));
            assert!(!queries[1].contains("not {"));
            assert!(queries[0].contains("reduce"));
            assert!(queries[1].contains("reduce"));
            assert!(!queries[2].contains("reduce"));
        }
    }

    #[tokio::test]
    async fn cutover_rejects_native_backfill_before_the_write_query() {
        let (backend, log) =
            MockMigrationBackend::with_legacy_cutover_responses(vec![count(2), count(3)]);
        let db = Database::with_backend(Box::new(backend), "test");
        let error = execute_backfill(&db, &crate::copy_plan::tests::step(), 0)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains(crate::LEGACY_WRITER_CUTOVER_MESSAGE)
        );
        let events = log.lock().unwrap();
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, MockEvent::Query(TxType::Write, _)))
        );
        assert!(matches!(events.last(), Some(MockEvent::Close)));
    }

    #[tokio::test]
    async fn malformed_counts_fail_before_the_write_and_close_read_transaction() {
        for response in [count(-1), QueryResult::Rows(vec![]), QueryResult::Ok] {
            let (backend, log) = MockMigrationBackend::with_responses(vec![response]);
            let db = Database::with_backend(Box::new(backend), "test");
            assert!(
                execute_backfill(&db, &crate::copy_plan::tests::step(), 0)
                    .await
                    .is_err()
            );
            let events = log.lock().unwrap();
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, MockEvent::OpenTx(TxType::Write)))
            );
            assert!(events.iter().any(|e| matches!(e, MockEvent::Close)));
        }
    }
}
