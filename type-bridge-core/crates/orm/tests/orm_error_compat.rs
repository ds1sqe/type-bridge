//! Downstream-style compile fixture for the released exhaustive ORM error enum.

use type_bridge_orm::OrmError;
use type_bridge_orm::session::backend::{
    AnswerCancellation, BoundedAnswerLimits, BoxFuture, QueryResult, TransactionOps,
};

struct ReleasedTransactionImpl;

impl TransactionOps for ReleasedTransactionImpl {
    fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        Box::pin(async { Ok(QueryResult::Ok) })
    }

    fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        Box::pin(async { Ok(()) })
    }

    fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        Box::pin(async { Ok(()) })
    }

    fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        Box::pin(async { Ok(()) })
    }
}

fn accepts_released_transaction_impl<T: TransactionOps>() {}

fn classify_released_error(error: OrmError) -> &'static str {
    match error {
        OrmError::Match(_) => "match",
        OrmError::UnsupportedVersion(_) => "unsupported-version",
        OrmError::Connection(_) => "connection",
        OrmError::QueryExecution(_) => "query-execution",
        OrmError::Transaction(_) => "transaction",
        OrmError::Hydration { .. } => "hydration",
        OrmError::NotFound(_) => "not-found",
        OrmError::InvalidFilter(_) => "invalid-filter",
        OrmError::DescriptorValidation { .. } => "descriptor-validation",
        OrmError::DescriptorConflict { .. } => "descriptor-conflict",
        OrmError::DescriptorNotFound(_) => "descriptor-not-found",
        OrmError::Compilation(_) => "compilation",
        OrmError::Schema(_) => "schema",
        OrmError::Serialization(_) => "serialization",
        OrmError::Hook(_) => "hook",
    }
}

#[test]
fn released_orm_error_remains_exhaustively_matchable_without_commit_variant() {
    assert_eq!(
        classify_released_error(OrmError::Transaction(
            "Commit failed: constraint violated".to_owned(),
        )),
        "transaction",
    );
}

#[test]
fn released_limit_structs_and_transaction_trait_remain_source_compatible() {
    accepts_released_transaction_impl::<ReleasedTransactionImpl>();

    let limits = BoundedAnswerLimits {
        max_items: 7,
        max_bytes: 11,
        deadline: None,
        cancellation: AnswerCancellation::default(),
    };
    let BoundedAnswerLimits {
        max_items,
        max_bytes,
        deadline,
        cancellation,
    } = limits;
    assert_eq!((max_items, max_bytes, deadline), (7, 11, None));
    assert!(!cancellation.is_cancelled());

    let runtime_limits = type_bridge_typedb_runtime::RuntimeAnswerLimits {
        max_items: 13,
        max_bytes: 17,
        deadline: None,
        cancellation: type_bridge_typedb_runtime::RuntimeAnswerCancellation::default(),
    };
    let type_bridge_typedb_runtime::RuntimeAnswerLimits {
        max_items,
        max_bytes,
        deadline,
        cancellation,
    } = runtime_limits;
    assert_eq!((max_items, max_bytes, deadline), (13, 17, None));
    assert!(!cancellation.is_cancelled());
}
