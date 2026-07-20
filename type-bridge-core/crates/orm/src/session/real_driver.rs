//! Real TypeDB backend adapter over the shared `type-bridge-typedb-runtime`.
//!
//! This module is only compiled when the `typedb` feature is enabled.

#[cfg(not(any(feature = "band7", feature = "band8", feature = "band9")))]
compile_error!(
    "type-bridge-orm: the `typedb` machinery requires at least one band feature; enable `band7`, `band8`, and/or `band9` (all are default)"
);

use type_bridge_core_lib::ast::{TypedFetchRows, TypedRootScan};
use type_bridge_typedb_runtime as runtime;

pub use runtime::{
    ConnectOptions, PINNED_DRIVER_VERSION, PINNED_DRIVER_VERSION_B7, PINNED_DRIVER_VERSION_B9,
    embedded_driver_versions,
};

use super::backend::{
    AnswerConsumer, AnswerControl, AnswerItem, BoundedAnswerLimits, BoundedAnswerStats, BoxFuture,
    DriverBackend, GivenRowsSpec, GivenValue, QueryResult, TransactionOps, TxType,
    execute_typed_provider_statement, typed_fetch_provider_statement,
    typed_root_provider_statement,
};
use crate::error::{CommitFailureCertainty, OrmError};
use crate::match_request::{
    Capability, CapabilitySet, MatchError, MatchErrorCategory, MatchErrorPathSegment,
};

/// Real TypeDB backend wrapping the shared runtime.
pub struct RealBackend {
    inner: runtime::TypeDBRuntime,
}

impl RealBackend {
    /// Connect to a TypeDB server.
    ///
    /// Validates the supplied server version, probes via HTTP, or falls back to
    /// gRPC-only negotiation according to the shared runtime gate.
    pub async fn connect(
        address: &str,
        username: &str,
        password: &str,
        options: ConnectOptions,
    ) -> Result<Self, OrmError> {
        let inner = runtime::TypeDBRuntime::connect(address, username, password, options)
            .await
            .map_err(OrmError::from)?;
        Ok(Self { inner })
    }
}

/// Ensure a TypeDB database exists, creating it if absent.
pub async fn ensure_database_exists(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
    options: ConnectOptions,
) -> Result<(), OrmError> {
    runtime::ensure_database_exists(address, database, username, password, options)
        .await
        .map_err(OrmError::from)
}

/// Check whether a TypeDB database exists without creating it.
pub async fn database_exists(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
    options: ConnectOptions,
) -> Result<bool, OrmError> {
    runtime::database_exists(address, database, username, password, options)
        .await
        .map_err(OrmError::from)
}

impl DriverBackend for RealBackend {
    fn match_capabilities(&self) -> CapabilitySet {
        real_match_capabilities()
    }

    fn open_transaction(
        &self,
        database: &str,
        tx_type: TxType,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
        let runtime_tx_type = runtime_tx_type(tx_type);
        let database = database.to_string();
        Box::pin(async move {
            let inner = self
                .inner
                .open_transaction(&database, runtime_tx_type)
                .await
                .map_err(OrmError::from)?;
            Ok(Box::new(RealTransaction { inner }) as Box<dyn TransactionOps>)
        })
    }

    fn is_open(&self) -> bool {
        self.inner.is_open()
    }

    fn server_version(&self) -> Option<type_bridge_core_lib::version::Version> {
        self.inner.server_version()
    }

    fn supports_given_rows(&self) -> bool {
        self.inner.supports_given_rows()
    }

    fn database_exists(&self, database: &str) -> BoxFuture<'_, Result<bool, OrmError>> {
        let database = database.to_string();
        Box::pin(async move {
            self.inner
                .database_exists(&database)
                .await
                .map_err(OrmError::from)
        })
    }

    fn create_database(&self, database: &str) -> BoxFuture<'_, Result<(), OrmError>> {
        let database = database.to_string();
        Box::pin(async move {
            self.inner
                .create_database(&database)
                .await
                .map_err(OrmError::from)
        })
    }

    fn delete_database(&self, database: &str) -> BoxFuture<'_, Result<(), OrmError>> {
        let database = database.to_string();
        Box::pin(async move {
            self.inner
                .delete_database(&database)
                .await
                .map_err(OrmError::from)
        })
    }

    fn schema_text(&self, database: &str) -> BoxFuture<'_, Result<String, OrmError>> {
        let database = database.to_string();
        Box::pin(async move {
            self.inner
                .schema_text(&database)
                .await
                .map_err(OrmError::from)
        })
    }
}

fn real_match_capabilities() -> CapabilitySet {
    CapabilitySet::from_iter([
        Capability::ResourceBoundedStreaming,
        Capability::ExactEntityTarget,
        Capability::ExactRelationTarget,
        Capability::SubtypeEntityTarget,
        Capability::SubtypeRelationTarget,
        Capability::FieldComparison,
        Capability::BooleanPattern,
        Capability::SelectedTupleDistinct,
        Capability::StableSelectedOrder,
        Capability::DistinctRootSelection,
        Capability::StableRootOrder,
        Capability::SameTransactionRehydration,
        Capability::BatchIdentityRebind,
        Capability::DistinctRootCount,
        Capability::DistinctRootExists,
        Capability::Collect,
        Capability::CollectDistinct,
        Capability::StableCollectionOrder,
    ])
}

struct RealTransaction {
    inner: runtime::RuntimeTransaction,
}

impl TransactionOps for RealTransaction {
    fn supports_given_rows(&self) -> bool {
        self.inner.supports_given_rows()
    }

    fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        let typeql = typeql.to_string();
        Box::pin(async move {
            self.inner
                .query(&typeql)
                .await
                .map(query_result)
                .map_err(OrmError::from)
        })
    }

    fn query_with_rows(
        &mut self,
        typeql: &str,
        rows: GivenRowsSpec,
    ) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        let typeql = typeql.to_string();
        let rows = runtime_given_rows(rows);
        Box::pin(async move {
            self.inner
                .query_with_rows(&typeql, rows)
                .await
                .map(query_result)
                .map_err(OrmError::from)
        })
    }

    fn query_with_rows_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        rows: GivenRowsSpec,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        let rows = runtime_given_rows(rows);
        Box::pin(async move {
            let runtime_limits = runtime::RuntimeAnswerLimits {
                max_items: limits.max_items,
                max_bytes: limits.max_bytes,
                deadline: limits.deadline,
                cancellation: runtime::RuntimeAnswerCancellation::from_shared(
                    limits.cancellation.shared(),
                ),
            };
            let mut consumer_error = None;
            let mut adapter = |item| {
                let item = match item {
                    runtime::RuntimeAnswerItem::Row(value) => AnswerItem::Row(value),
                    runtime::RuntimeAnswerItem::Document(value) => AnswerItem::Document(value),
                };
                match consumer.accept(item) {
                    Ok(AnswerControl::Continue) => Ok(runtime::RuntimeAnswerControl::Continue),
                    Ok(AnswerControl::Stop) => Ok(runtime::RuntimeAnswerControl::Stop),
                    Err(error) => {
                        consumer_error = Some(error);
                        Err(runtime::RuntimeError::AnswerConsumer)
                    }
                }
            };
            let result = self
                .inner
                .query_with_rows_bounded(typeql, rows, runtime_limits, &mut adapter)
                .await;
            if matches!(&result, Err(runtime::RuntimeError::AnswerConsumer))
                && let Some(error) = consumer_error
            {
                return Err(error);
            }
            let stats = result.map_err(OrmError::from)?;
            Ok(BoundedAnswerStats {
                processed_items: stats.processed_items,
                response_bytes: stats.response_bytes,
                stopped_early: stats.stopped_early,
            })
        })
    }

    fn query_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        Box::pin(async move {
            let runtime_limits = runtime::RuntimeAnswerLimits {
                max_items: limits.max_items,
                max_bytes: limits.max_bytes,
                deadline: limits.deadline,
                cancellation: runtime::RuntimeAnswerCancellation::from_shared(
                    limits.cancellation.shared(),
                ),
            };
            let mut consumer_error = None;
            let mut adapter = |item| {
                let item = match item {
                    runtime::RuntimeAnswerItem::Row(value) => AnswerItem::Row(value),
                    runtime::RuntimeAnswerItem::Document(value) => AnswerItem::Document(value),
                };
                match consumer.accept(item) {
                    Ok(AnswerControl::Continue) => Ok(runtime::RuntimeAnswerControl::Continue),
                    Ok(AnswerControl::Stop) => Ok(runtime::RuntimeAnswerControl::Stop),
                    Err(error) => {
                        consumer_error = Some(error);
                        Err(runtime::RuntimeError::AnswerConsumer)
                    }
                }
            };
            let result = self
                .inner
                .query_bounded(typeql, runtime_limits, &mut adapter)
                .await;
            if matches!(&result, Err(runtime::RuntimeError::AnswerConsumer))
                && let Some(error) = consumer_error
            {
                return Err(error);
            }
            let stats = result.map_err(OrmError::from)?;
            Ok(BoundedAnswerStats {
                processed_items: stats.processed_items,
                response_bytes: stats.response_bytes,
                stopped_early: stats.stopped_early,
            })
        })
    }

    fn query_typed_bounded<'a>(
        &'a mut self,
        query: &'a TypedFetchRows,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        Box::pin(async move {
            let statement = typed_fetch_provider_statement(query, self.supports_given_rows())?;
            let bindings = query
                .targets
                .iter()
                .map(|target| target.binding)
                .collect::<Vec<_>>();
            let mut consumer_error = None;
            let result = {
                let mut adapter = |item| {
                    let item = match item {
                        AnswerItem::Row(value) => match normalize_solution_row(value, &bindings) {
                            Ok(value) => AnswerItem::Row(value),
                            Err(error) => {
                                consumer_error = Some(error);
                                return Err(OrmError::QueryExecution(
                                    "selected solution row normalization failed".into(),
                                ));
                            }
                        },
                        AnswerItem::Document(_) => {
                            consumer_error = Some(
                                MatchError::new(
                                    MatchErrorCategory::ResultDecode,
                                    "solution_answer_kind",
                                    "selected solution statement returned a document",
                                )
                                .at(MatchErrorPathSegment::ProviderEvidence)
                                .into(),
                            );
                            return Err(OrmError::QueryExecution(
                                "selected solution answer kind was invalid".into(),
                            ));
                        }
                    };
                    consumer.accept(item)
                };
                execute_typed_provider_statement(self, statement, limits, &mut adapter).await
            };
            if let Some(error) = consumer_error {
                return Err(error);
            }
            result
        })
    }

    fn query_root_typed_bounded<'a>(
        &'a mut self,
        query: &'a TypedRootScan,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        Box::pin(async move {
            let statement = typed_root_provider_statement(query, self.supports_given_rows())?;
            let mut consumer_error = None;
            let result = {
                let mut adapter = |item| {
                    let item = match item {
                        AnswerItem::Row(value) => {
                            match normalize_solution_row(value, &[query.root]) {
                                Ok(value) => AnswerItem::Row(value),
                                Err(error) => {
                                    consumer_error = Some(error);
                                    return Err(OrmError::QueryExecution(
                                        "distinct-root row normalization failed".into(),
                                    ));
                                }
                            }
                        }
                        AnswerItem::Document(_) => {
                            consumer_error = Some(
                                MatchError::new(
                                    MatchErrorCategory::ResultDecode,
                                    "root_answer_kind",
                                    "distinct-root statement returned a document",
                                )
                                .at(MatchErrorPathSegment::ProviderEvidence)
                                .into(),
                            );
                            return Err(OrmError::QueryExecution(
                                "distinct-root answer kind was invalid".into(),
                            ));
                        }
                    };
                    consumer.accept(item)
                };
                execute_typed_provider_statement(self, statement, limits, &mut adapter).await
            };
            if let Some(error) = consumer_error {
                return Err(error);
            }
            result
        })
    }

    fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        Box::pin(async move { self.inner.commit().await.map_err(OrmError::from) })
    }

    fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        Box::pin(async move { self.inner.rollback().await.map_err(OrmError::from) })
    }

    fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        Box::pin(async move { self.inner.close().await.map_err(OrmError::from) })
    }
}

fn normalize_solution_row(
    value: serde_json::Value,
    bindings: &[u16],
) -> Result<serde_json::Value, OrmError> {
    let row = value.as_object().ok_or_else(|| {
        MatchError::new(
            MatchErrorCategory::ResultDecode,
            "malformed_solution_row",
            "TypeDB selected solution row is not an object",
        )
        .at(MatchErrorPathSegment::ProviderEvidence)
    })?;
    let assignments = bindings
        .iter()
        .map(|binding| {
            let name = format!("b{binding}");
            let concept = row
                .get(&name)
                .or_else(|| row.get(&format!("${name}")))
                .ok_or_else(|| {
                    MatchError::new(
                        MatchErrorCategory::ResultDecode,
                        "missing_provider_binding",
                        "TypeDB selected solution row omitted a positive binding",
                    )
                    .at(MatchErrorPathSegment::ProviderEvidence)
                })?;
            let concept_id = concept
                .get("iid")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    MatchError::new(
                        MatchErrorCategory::ResultDecode,
                        "missing_provider_concept_id",
                        "TypeDB selected binding did not contain an IID",
                    )
                    .at(MatchErrorPathSegment::ProviderEvidence)
                })?;
            Ok(serde_json::json!({
                "binding": binding,
                "concept_id": concept_id,
            }))
        })
        .collect::<Result<Vec<_>, MatchError>>()?;
    Ok(serde_json::json!({
        "bindings": assignments,
        "satisfied_role_edges": [],
    }))
}

fn runtime_tx_type(tx_type: TxType) -> runtime::TxType {
    match tx_type {
        TxType::Read => runtime::TxType::Read,
        TxType::Write => runtime::TxType::Write,
        TxType::Schema => runtime::TxType::Schema,
    }
}

fn runtime_given_rows(spec: GivenRowsSpec) -> runtime::GivenRowsSpec {
    runtime::GivenRowsSpec {
        variables: spec.variables,
        rows: spec
            .rows
            .into_iter()
            .map(|row| row.into_iter().map(runtime_given_value).collect())
            .collect(),
    }
}

fn runtime_given_value(value: GivenValue) -> runtime::GivenValue {
    match value {
        GivenValue::Boolean(b) => runtime::GivenValue::Boolean(b),
        GivenValue::Integer(i) => runtime::GivenValue::Integer(i),
        GivenValue::Double(d) => runtime::GivenValue::Double(d),
        GivenValue::String(s) => runtime::GivenValue::String(s),
        GivenValue::Date(s) => runtime::GivenValue::Date(s),
        GivenValue::Datetime(s) => runtime::GivenValue::Datetime(s),
        GivenValue::DatetimeTz(s) => runtime::GivenValue::DatetimeTz(s),
    }
}

fn query_result(result: runtime::QueryResult) -> QueryResult {
    match result {
        runtime::QueryResult::Ok => QueryResult::Ok,
        runtime::QueryResult::Documents(docs) => QueryResult::Documents(docs),
        runtime::QueryResult::Rows(rows) => QueryResult::Rows(rows),
    }
}

impl From<runtime::RuntimeError> for OrmError {
    fn from(error: runtime::RuntimeError) -> Self {
        match error {
            runtime::RuntimeError::UnsupportedVersion(error) => Self::UnsupportedVersion(error),
            runtime::RuntimeError::Connection(message) => Self::Connection(message),
            runtime::RuntimeError::QueryExecution(message) => Self::QueryExecution(message),
            runtime::RuntimeError::Transaction(message) => Self::Transaction(message),
            runtime::RuntimeError::Commit { certainty, message } => Self::Commit {
                certainty: match certainty {
                    runtime::CommitFailureCertainty::DefinitelyAborted => {
                        CommitFailureCertainty::DefinitelyAborted
                    }
                    runtime::CommitFailureCertainty::Unknown => CommitFailureCertainty::Unknown,
                },
                message,
            },
            runtime::RuntimeError::ResourceLimit { code, message } => {
                MatchError::new(MatchErrorCategory::ResourceLimit, code, message)
                    .at(MatchErrorPathSegment::ProviderEvidence)
                    .into()
            }
            runtime::RuntimeError::AnswerConsumer => {
                Self::QueryExecution("answer consumer rejected provider data".into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_failure_certainty_maps_without_changing_display() {
        for (runtime_certainty, orm_certainty) in [
            (
                runtime::CommitFailureCertainty::DefinitelyAborted,
                CommitFailureCertainty::DefinitelyAborted,
            ),
            (
                runtime::CommitFailureCertainty::Unknown,
                CommitFailureCertainty::Unknown,
            ),
        ] {
            let error = OrmError::from(runtime::RuntimeError::Commit {
                certainty: runtime_certainty,
                message: "driver response".to_owned(),
            });
            assert_eq!(error.commit_failure_certainty(), Some(orm_certainty));
            assert_eq!(
                error.to_string(),
                "Transaction error: Commit failed: driver response"
            );
        }
    }

    #[test]
    fn real_backend_declares_the_exact_typed_match_capability_inventory() {
        assert_eq!(
            real_match_capabilities(),
            CapabilitySet::from_iter(Capability::ALL)
        );
    }

    #[test]
    fn selected_solution_normalization_keeps_every_binding_iid() {
        let value = normalize_solution_row(
            serde_json::json!({
                "b0": {"category": "entity", "label": "person", "iid": "0x01"},
                "$b2": {"category": "relation", "label": "employment", "iid": "0x10"}
            }),
            &[0, 2],
        )
        .unwrap();

        assert_eq!(
            value,
            serde_json::json!({
                "bindings": [
                    {"binding": 0, "concept_id": "0x01"},
                    {"binding": 2, "concept_id": "0x10"}
                ],
                "satisfied_role_edges": []
            })
        );
    }

    #[test]
    fn selected_solution_normalization_fails_closed_on_missing_iid() {
        let error = normalize_solution_row(
            serde_json::json!({"b0": {"category": "entity", "label": "person"}}),
            &[0],
        )
        .unwrap_err();
        let OrmError::Match(error) = error else {
            panic!("expected structured match error")
        };
        assert_eq!(error.code().as_str(), "missing_provider_concept_id");
    }
}
