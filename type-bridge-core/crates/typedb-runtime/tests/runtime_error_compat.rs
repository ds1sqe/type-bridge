//! Downstream-style compile fixture for the 3.0 exhaustive error enum.

use type_bridge_typedb_runtime::RuntimeError;

fn classify_released_error(error: RuntimeError) -> &'static str {
    match error {
        RuntimeError::UnsupportedVersion(_) => "unsupported-version",
        RuntimeError::Connection(_) => "connection",
        RuntimeError::QueryExecution(_) => "query-execution",
        RuntimeError::Transaction(_) => "transaction",
        RuntimeError::ResourceLimit { .. } | RuntimeError::AnswerLimit { .. } => "resource-limit",
        RuntimeError::AnswerConsumer => "answer-consumer",
    }
}

#[test]
fn runtime_error_remains_exhaustively_matchable_with_answer_counters() {
    assert_eq!(
        classify_released_error(RuntimeError::Transaction(
            "Commit failed: constraint violated".to_owned(),
        )),
        "transaction",
    );
}
