//! Real TypeDB backend adapter over the shared `type-bridge-typedb-runtime`.
//!
//! This module is only compiled when the `typedb` feature is enabled.

#[cfg(not(any(feature = "band8", feature = "band9")))]
compile_error!(
    "type-bridge-orm: the `typedb` machinery requires at least one band feature; enable `band8` and/or `band9` (both are default)"
);

use std::sync::Arc;

use type_bridge_core_lib::ast::{TypedFetchRows, TypedRootScan};
use type_bridge_typedb_runtime as runtime;

pub use runtime::{
    ConnectOptions, PINNED_DRIVER_VERSION, PINNED_DRIVER_VERSION_B9, PreparedSecureConnectOptions,
    SecureConnectError, SecureConnectOptions, SecureResult, TlsMode, embedded_driver_versions,
};

use super::backend::{
    AnswerConsumer, AnswerControl, AnswerItem, BoundedAnswerLimits, BoundedAnswerStats, BoxFuture,
    DriverBackend, GivenRowsSpec, GivenValue, MAX_ERROR_DRAIN_BYTES, MAX_ERROR_DRAIN_ITEMS,
    QueryResult, QueryV2AnswerLimits, SchemaFencedReadTransaction, TransactionOps, TxType,
    execute_typed_provider_statement, typed_fetch_provider_statement,
    typed_root_provider_statement, typed_tuple_provider_statement,
};
use crate::error::{ClassifiedCommitError, CommitFailureCertainty, OrmError};
use crate::match_request::{
    Capability, CapabilitySet, MatchError, MatchErrorCategory, MatchErrorPathSegment,
};

/// Real TypeDB backend wrapping the shared runtime.
pub struct RealBackend {
    inner: Arc<runtime::TypeDBRuntime>,
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
        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    /// Connect with an explicit typed TLS policy.
    pub async fn connect_secure(
        address: &str,
        username: &str,
        password: &str,
        options: SecureConnectOptions,
    ) -> SecureResult<Self> {
        let inner =
            runtime::TypeDBRuntime::connect_secure(address, username, password, options).await?;
        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    /// Connect with a transport policy prepared before credential resolution.
    #[doc(hidden)]
    pub async fn connect_prepared_secure(
        address: &str,
        username: &str,
        password: &str,
        options: PreparedSecureConnectOptions,
    ) -> SecureResult<Self> {
        let inner =
            runtime::TypeDBRuntime::connect_prepared_secure(address, username, password, options)
                .await?;
        Ok(Self {
            inner: Arc::new(inner),
        })
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

/// Ensure a TypeDB database exists with an explicit typed TLS policy.
pub async fn ensure_database_exists_secure(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
    options: SecureConnectOptions,
) -> SecureResult<()> {
    runtime::ensure_database_exists_secure(address, database, username, password, options).await
}

/// Ensure a database exists through an already prepared transport snapshot.
#[doc(hidden)]
pub async fn ensure_database_exists_prepared_secure(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
    options: PreparedSecureConnectOptions,
) -> SecureResult<()> {
    runtime::ensure_database_exists_prepared_secure(address, database, username, password, options)
        .await
}

/// Check whether a TypeDB database exists with an explicit typed TLS policy.
pub async fn database_exists_secure(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
    options: SecureConnectOptions,
) -> SecureResult<bool> {
    runtime::database_exists_secure(address, database, username, password, options).await
}

/// Check a database through an already prepared transport snapshot.
#[doc(hidden)]
pub async fn database_exists_prepared_secure(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
    options: PreparedSecureConnectOptions,
) -> SecureResult<bool> {
    runtime::database_exists_prepared_secure(address, database, username, password, options).await
}

/// Connect with an explicit typed TLS policy and delete a TypeDB database.
pub async fn delete_database_secure(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
    options: SecureConnectOptions,
) -> SecureResult<()> {
    runtime::delete_database_secure(address, database, username, password, options).await
}

/// Delete a database through an already prepared transport snapshot.
#[doc(hidden)]
pub async fn delete_database_prepared_secure(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
    options: PreparedSecureConnectOptions,
) -> SecureResult<()> {
    runtime::delete_database_prepared_secure(address, database, username, password, options).await
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
        let runtime = Arc::clone(&self.inner);
        Box::pin(async move {
            let inner = runtime
                .open_transaction(&database, runtime_tx_type)
                .await
                .map_err(OrmError::from)?;
            Ok(Box::new(RealTransaction {
                inner,
                runtime,
                database,
            }) as Box<dyn TransactionOps>)
        })
    }

    fn open_schema_fenced_read_transaction(
        &self,
        database: &str,
        timeout: std::time::Duration,
    ) -> BoxFuture<'_, Result<SchemaFencedReadTransaction, OrmError>> {
        let database = database.to_string();
        let runtime = Arc::clone(&self.inner);
        Box::pin(async move {
            // TypeDB admits WRITE and SCHEMA transactions under mutually
            // exclusive database guards. V2 emits read-only TypeQL and closes
            // without commit, so WRITE supplies schema exclusion without
            // turning the query API into a mutation surface.
            let mut inner = runtime
                .open_transaction_with_timeout(&database, runtime::TxType::Write, timeout)
                .await
                .map_err(OrmError::from)?;
            let schema_text = match runtime.schema_text(&database).await {
                Ok(schema_text) => schema_text,
                Err(error) => {
                    let _ = inner.close().await;
                    return Err(OrmError::from(error));
                }
            };
            Ok(SchemaFencedReadTransaction::new(
                Box::new(RealTransaction {
                    inner,
                    runtime,
                    database,
                }),
                schema_text,
            ))
        })
    }

    fn is_open(&self) -> bool {
        self.inner.is_open()
    }

    fn close_connection(&self) -> Result<(), OrmError> {
        self.inner.force_close().map_err(OrmError::from)
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
        Capability::BoundedReachability,
        Capability::TypedReduction,
    ])
}

struct RealTransaction {
    inner: runtime::RuntimeTransaction,
    runtime: Arc<runtime::TypeDBRuntime>,
    database: String,
}

impl TransactionOps for RealTransaction {
    fn supports_given_rows(&self) -> bool {
        self.inner.supports_given_rows()
    }

    fn schema_snapshot(&mut self) -> BoxFuture<'_, Result<Option<String>, OrmError>> {
        let runtime = Arc::clone(&self.runtime);
        let database = self.database.clone();
        Box::pin(async move {
            runtime
                .schema_text(&database)
                .await
                .map(Some)
                .map_err(OrmError::from)
        })
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

    fn query_v2_with_rows_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        rows: GivenRowsSpec,
        limits: QueryV2AnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        let QueryV2AnswerLimits {
            answer: limits,
            max_collection_members,
        } = limits;
        let rows = runtime_given_rows(rows);
        Box::pin(async move {
            let runtime_limits = runtime::QueryV2RuntimeAnswerLimits {
                answer: runtime::RuntimeAnswerLimits {
                    max_items: limits.max_items,
                    max_bytes: limits.max_bytes,
                    deadline: limits.deadline,
                    cancellation: runtime::RuntimeAnswerCancellation::from_shared(
                        limits.cancellation.shared(),
                    ),
                },
                max_collection_members,
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
                .query_v2_with_rows_bounded(typeql, rows, runtime_limits, &mut adapter)
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

    fn query_v2_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        limits: QueryV2AnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        let QueryV2AnswerLimits {
            answer: limits,
            max_collection_members,
        } = limits;
        Box::pin(async move {
            let runtime_limits = runtime::QueryV2RuntimeAnswerLimits {
                answer: runtime::RuntimeAnswerLimits {
                    max_items: limits.max_items,
                    max_bytes: limits.max_bytes,
                    deadline: limits.deadline,
                    cancellation: runtime::RuntimeAnswerCancellation::from_shared(
                        limits.cancellation.shared(),
                    ),
                },
                max_collection_members,
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
                .query_v2_bounded(typeql, runtime_limits, &mut adapter)
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

    fn query_canonical<'a>(
        &'a mut self,
        typeql: &str,
    ) -> BoxFuture<'a, Result<QueryResult, OrmError>> {
        let typeql = typeql.to_owned();
        Box::pin(async move {
            let limits = runtime::QueryV2RuntimeAnswerLimits {
                answer: runtime::RuntimeAnswerLimits {
                    max_items: u64::MAX,
                    max_bytes: u64::MAX,
                    deadline: None,
                    cancellation: runtime::RuntimeAnswerCancellation::default(),
                },
                max_collection_members: u64::MAX,
            };
            self.inner
                .query_v2_materialized(&typeql, limits)
                .await
                .map(query_result)
                .map_err(OrmError::from)
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
            let mut adapter = NormalizingAnswerConsumer::new(
                consumer,
                &bindings,
                &limits,
                ExpectedRowKind::SOLUTION,
            );
            let result =
                execute_typed_provider_statement(self, statement, limits, &mut adapter).await;
            adapter.complete(result)
        })
    }

    fn supports_exactly_one_tuple_proof(&self) -> bool {
        true
    }

    fn query_tuple_typed_bounded<'a>(
        &'a mut self,
        query: &'a TypedFetchRows,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        Box::pin(async move {
            let statement = typed_tuple_provider_statement(query, self.supports_given_rows())?;
            let bindings = query.projection.clone();
            let mut adapter = NormalizingAnswerConsumer::new(
                consumer,
                &bindings,
                &limits,
                ExpectedRowKind::TUPLE,
            );
            let result =
                execute_typed_provider_statement(self, statement, limits, &mut adapter).await;
            adapter.complete(result)
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
            let bindings = [query.root];
            let mut adapter =
                NormalizingAnswerConsumer::new(consumer, &bindings, &limits, ExpectedRowKind::ROOT);
            let result =
                execute_typed_provider_statement(self, statement, limits, &mut adapter).await;
            adapter.complete(result)
        })
    }

    fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        Box::pin(async move { self.inner.commit().await.map_err(OrmError::from) })
    }

    fn commit_classified(&mut self) -> BoxFuture<'_, Result<(), ClassifiedCommitError>> {
        Box::pin(async move {
            self.inner
                .commit_classified()
                .await
                .map_err(ClassifiedCommitError::from)
        })
    }

    fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        Box::pin(async move { self.inner.rollback().await.map_err(OrmError::from) })
    }

    fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        Box::pin(async move { self.inner.close().await.map_err(OrmError::from) })
    }
}

#[derive(Clone, Copy)]
struct ExpectedRowKind {
    wrong_kind_code: &'static str,
    wrong_kind_message: &'static str,
}

impl ExpectedRowKind {
    const SOLUTION: Self = Self {
        wrong_kind_code: "solution_answer_kind",
        wrong_kind_message: "selected solution statement returned a document",
    };
    const TUPLE: Self = Self {
        wrong_kind_code: "tuple_answer_kind",
        wrong_kind_message: "selected tuple statement returned a document",
    };
    const ROOT: Self = Self {
        wrong_kind_code: "root_answer_kind",
        wrong_kind_message: "distinct-root statement returned a document",
    };
}

/// Normalize real-driver concept rows while preserving the first decode
/// failure and consuming only a small, finite suffix toward a terminal frame.
struct NormalizingAnswerConsumer<'a> {
    inner: &'a mut dyn AnswerConsumer,
    bindings: &'a [u16],
    expected: ExpectedRowKind,
    max_items: u64,
    max_bytes: u64,
    processed_items: u64,
    response_bytes: u64,
    drained_items: u64,
    drained_bytes: u64,
    first_error: Option<OrmError>,
}

impl<'a> NormalizingAnswerConsumer<'a> {
    fn new(
        inner: &'a mut dyn AnswerConsumer,
        bindings: &'a [u16],
        limits: &BoundedAnswerLimits,
        expected: ExpectedRowKind,
    ) -> Self {
        Self {
            inner,
            bindings,
            expected,
            max_items: limits.max_items,
            max_bytes: limits.max_bytes,
            processed_items: 0,
            response_bytes: 0,
            drained_items: 0,
            drained_bytes: 0,
            first_error: None,
        }
    }

    fn complete(
        self,
        provider: Result<BoundedAnswerStats, OrmError>,
    ) -> Result<BoundedAnswerStats, OrmError> {
        match self.first_error {
            Some(error) => Err(error),
            None => provider,
        }
    }

    fn reject(&mut self, error: OrmError) -> AnswerControl {
        if self.first_error.is_none() {
            self.first_error = Some(error);
        }
        if self.has_drain_capacity() {
            AnswerControl::Continue
        } else {
            AnswerControl::Stop
        }
    }

    fn has_drain_capacity(&self) -> bool {
        self.processed_items < self.max_items
            && self.response_bytes < self.max_bytes
            && self.drained_items < MAX_ERROR_DRAIN_ITEMS
            && self.drained_bytes < MAX_ERROR_DRAIN_BYTES
    }

    fn accept_suffix(&mut self, item: AnswerItem) -> AnswerControl {
        let Ok(item_bytes) = item.encoded_bytes() else {
            return AnswerControl::Stop;
        };
        let Some(next_items) = self.processed_items.checked_add(1) else {
            return AnswerControl::Stop;
        };
        let Some(next_bytes) = self.response_bytes.checked_add(item_bytes) else {
            return AnswerControl::Stop;
        };
        let Some(next_drained_items) = self.drained_items.checked_add(1) else {
            return AnswerControl::Stop;
        };
        let Some(next_drained_bytes) = self.drained_bytes.checked_add(item_bytes) else {
            return AnswerControl::Stop;
        };
        if next_items > self.max_items
            || next_bytes > self.max_bytes
            || next_drained_items > MAX_ERROR_DRAIN_ITEMS
            || next_drained_bytes > MAX_ERROR_DRAIN_BYTES
        {
            return AnswerControl::Stop;
        }
        self.processed_items = next_items;
        self.response_bytes = next_bytes;
        self.drained_items = next_drained_items;
        self.drained_bytes = next_drained_bytes;
        if self.has_drain_capacity() {
            AnswerControl::Continue
        } else {
            AnswerControl::Stop
        }
    }

    fn wrong_kind_error(&self) -> OrmError {
        MatchError::new(
            MatchErrorCategory::ResultDecode,
            self.expected.wrong_kind_code,
            self.expected.wrong_kind_message,
        )
        .at(MatchErrorPathSegment::ProviderEvidence)
        .into()
    }
}

impl AnswerConsumer for NormalizingAnswerConsumer<'_> {
    fn accept(&mut self, item: AnswerItem) -> Result<AnswerControl, OrmError> {
        if self.first_error.is_some() {
            return Ok(self.accept_suffix(item));
        }

        let next_items = match self.processed_items.checked_add(1) {
            Some(next_items) => next_items,
            None => {
                return Ok(self.reject(provider_resource_error(
                    "processed_item_counter_overflow",
                    "processed provider item counter overflowed",
                )));
            }
        };
        if next_items > self.max_items {
            return Ok(self.reject(provider_resource_error(
                "processed_item_limit",
                "provider answer exceeded the processed-item ceiling",
            )));
        }
        let item_bytes = match item.encoded_bytes() {
            Ok(item_bytes) => item_bytes,
            Err(error) => return Ok(self.reject(error)),
        };
        let next_bytes = match self.response_bytes.checked_add(item_bytes) {
            Some(next_bytes) => next_bytes,
            None => {
                return Ok(self.reject(provider_resource_error(
                    "answer_byte_counter_overflow",
                    "provider answer byte counter overflowed",
                )));
            }
        };
        if next_bytes > self.max_bytes {
            return Ok(self.reject(provider_resource_error(
                "response_byte_limit",
                "provider answer exceeded the response-byte ceiling",
            )));
        }
        self.processed_items = next_items;
        self.response_bytes = next_bytes;

        let normalized = match item {
            AnswerItem::Row(value) => match normalize_solution_row(value, self.bindings) {
                Ok(value) => AnswerItem::Row(value),
                Err(error) => return Ok(self.reject(error)),
            },
            AnswerItem::Document(_) => return Ok(self.reject(self.wrong_kind_error())),
        };
        match self.inner.accept(normalized) {
            Ok(control) => Ok(control),
            Err(error) => Ok(self.reject(error)),
        }
    }
}

fn provider_resource_error(code: &'static str, message: &'static str) -> OrmError {
    MatchError::new(MatchErrorCategory::ResourceLimit, code, message)
        .at(MatchErrorPathSegment::ProviderEvidence)
        .into()
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
            if !type_bridge_contract::id::is_canonical_thing_iid(concept_id) {
                return Err(MatchError::new(
                    MatchErrorCategory::ResultDecode,
                    "malformed_provider_concept_id",
                    "provider concept IID must be bounded canonical hexadecimal",
                )
                .at(MatchErrorPathSegment::ProviderEvidence));
            }
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
        GivenValue::Empty => runtime::GivenValue::Empty,
        GivenValue::Boolean(b) => runtime::GivenValue::Boolean(b),
        GivenValue::Integer(i) => runtime::GivenValue::Integer(i),
        GivenValue::Double(d) => runtime::GivenValue::Double(d),
        GivenValue::String(s) => runtime::GivenValue::String(s),
        GivenValue::Date(s) => runtime::GivenValue::Date(s),
        GivenValue::Datetime(s) => runtime::GivenValue::Datetime(s),
        GivenValue::DatetimeTz(s) => runtime::GivenValue::DatetimeTz(s),
        GivenValue::DatetimeTzExact {
            local,
            named_zone,
            effective_offset_seconds,
        } => runtime::GivenValue::DatetimeTzExact {
            local,
            named_zone,
            effective_offset_seconds,
        },
        GivenValue::Decimal(s) => runtime::GivenValue::Decimal(s),
        GivenValue::Duration {
            months,
            days,
            nanos,
        } => runtime::GivenValue::Duration {
            months,
            days,
            nanos,
        },
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

impl From<runtime::RuntimeCommitError> for ClassifiedCommitError {
    fn from(error: runtime::RuntimeCommitError) -> Self {
        match error {
            runtime::RuntimeCommitError::Runtime(error) => Self::Orm(error.into()),
            runtime::RuntimeCommitError::Driver { certainty, message } => Self::Driver {
                certainty: match certainty {
                    runtime::CommitFailureCertainty::DefinitelyAborted => {
                        CommitFailureCertainty::DefinitelyAborted
                    }
                    runtime::CommitFailureCertainty::Unknown => CommitFailureCertainty::Unknown,
                },
                message,
            },
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
            let error = ClassifiedCommitError::from(runtime::RuntimeCommitError::Driver {
                certainty: runtime_certainty,
                message: "driver response".to_owned(),
            });
            assert_eq!(error.commit_failure_certainty(), Some(orm_certainty));
            assert_eq!(
                error.to_string(),
                "Transaction error: Commit failed: driver response"
            );
            let legacy = error.into_orm_error();
            assert!(
                matches!(&legacy, OrmError::Transaction(message) if message == "Commit failed: driver response")
            );
            assert_eq!(
                legacy.to_string(),
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

    #[test]
    fn normalization_failure_drains_only_the_bounded_item_suffix() {
        let limits = BoundedAnswerLimits {
            max_items: 100,
            max_bytes: 1024 * 1024,
            ..BoundedAnswerLimits::default()
        };
        let mut accepted = 0_u64;
        {
            let mut sink = |_item: AnswerItem| {
                accepted += 1;
                Ok(AnswerControl::Continue)
            };
            let bindings = [0];
            let mut adapter = NormalizingAnswerConsumer::new(
                &mut sink,
                &bindings,
                &limits,
                ExpectedRowKind::SOLUTION,
            );

            assert_eq!(
                adapter
                    .accept(AnswerItem::Row(serde_json::json!([])))
                    .unwrap(),
                AnswerControl::Continue
            );
            for index in 0..MAX_ERROR_DRAIN_ITEMS {
                let control = adapter
                    .accept(AnswerItem::Row(serde_json::json!({"suffix": index})))
                    .unwrap();
                let expected = if index + 1 == MAX_ERROR_DRAIN_ITEMS {
                    AnswerControl::Stop
                } else {
                    AnswerControl::Continue
                };
                assert_eq!(control, expected);
            }
            assert_eq!(adapter.processed_items, MAX_ERROR_DRAIN_ITEMS + 1);
            assert_eq!(adapter.drained_items, MAX_ERROR_DRAIN_ITEMS);

            let error = adapter
                .complete(Ok(BoundedAnswerStats::default()))
                .unwrap_err();
            assert_eq!(match_code(&error), "malformed_solution_row");
        }
        assert_eq!(accepted, 0);
    }

    #[test]
    fn normalization_failure_respects_byte_and_statement_ceilings() {
        let bindings = [0];
        let mut sink = |_item: AnswerItem| Ok(AnswerControl::Continue);
        let byte_limits = BoundedAnswerLimits {
            max_items: 100,
            max_bytes: 2 * MAX_ERROR_DRAIN_BYTES,
            ..BoundedAnswerLimits::default()
        };
        {
            let mut byte_adapter = NormalizingAnswerConsumer::new(
                &mut sink,
                &bindings,
                &byte_limits,
                ExpectedRowKind::SOLUTION,
            );
            assert_eq!(
                byte_adapter
                    .accept(AnswerItem::Row(serde_json::json!([])))
                    .unwrap(),
                AnswerControl::Continue
            );
            assert_eq!(
                byte_adapter
                    .accept(AnswerItem::Row(serde_json::json!({
                        "padding": "x".repeat(usize::try_from(MAX_ERROR_DRAIN_BYTES).unwrap())
                    })))
                    .unwrap(),
                AnswerControl::Stop
            );
            assert_eq!(byte_adapter.drained_items, 0);
            assert_eq!(byte_adapter.drained_bytes, 0);
        }

        let item_limits = BoundedAnswerLimits {
            max_items: 1,
            max_bytes: 1024,
            ..BoundedAnswerLimits::default()
        };
        let mut item_adapter = NormalizingAnswerConsumer::new(
            &mut sink,
            &bindings,
            &item_limits,
            ExpectedRowKind::SOLUTION,
        );
        assert_eq!(
            item_adapter
                .accept(AnswerItem::Row(serde_json::json!([])))
                .unwrap(),
            AnswerControl::Stop
        );
    }

    #[test]
    fn malformed_iid_enters_the_raw_normalizer_byte_bounded_drain() {
        let limits = BoundedAnswerLimits {
            max_items: 100,
            max_bytes: 2 * MAX_ERROR_DRAIN_BYTES,
            ..BoundedAnswerLimits::default()
        };
        let bindings = [0];
        let mut accepted = 0_u64;
        {
            let mut sink = |_item: AnswerItem| {
                accepted += 1;
                Ok(AnswerControl::Continue)
            };
            let mut adapter = NormalizingAnswerConsumer::new(
                &mut sink,
                &bindings,
                &limits,
                ExpectedRowKind::SOLUTION,
            );
            assert_eq!(
                adapter
                    .accept(AnswerItem::Row(serde_json::json!({
                        "b0": {"category": "entity", "label": "person", "iid": "0X01"}
                    })))
                    .unwrap(),
                AnswerControl::Continue
            );
            assert!(adapter.first_error.is_some());
            assert_eq!(adapter.processed_items, 1);
            assert_eq!(adapter.drained_items, 0);

            assert_eq!(
                adapter
                    .accept(AnswerItem::Row(serde_json::json!({
                        "b0": {"iid": "0x01"},
                        "padding": "x".repeat(
                            usize::try_from(MAX_ERROR_DRAIN_BYTES).unwrap()
                        )
                    })))
                    .unwrap(),
                AnswerControl::Stop
            );
            assert_eq!(adapter.drained_items, 0);
            assert_eq!(adapter.drained_bytes, 0);
            let error = adapter
                .complete(Ok(BoundedAnswerStats::default()))
                .unwrap_err();
            assert_eq!(match_code(&error), "malformed_provider_concept_id");
        }
        assert_eq!(accepted, 0);
    }

    #[test]
    fn every_typed_row_kind_uses_the_same_bounded_sticky_failure_adapter() {
        for (expected, code) in [
            (ExpectedRowKind::SOLUTION, "solution_answer_kind"),
            (ExpectedRowKind::TUPLE, "tuple_answer_kind"),
            (ExpectedRowKind::ROOT, "root_answer_kind"),
        ] {
            let limits = BoundedAnswerLimits {
                max_items: 1,
                max_bytes: 1024,
                ..BoundedAnswerLimits::default()
            };
            let bindings = [0];
            let mut sink = |_item: AnswerItem| Ok(AnswerControl::Continue);
            let mut adapter =
                NormalizingAnswerConsumer::new(&mut sink, &bindings, &limits, expected);
            assert_eq!(
                adapter
                    .accept(AnswerItem::Document(serde_json::json!({})))
                    .unwrap(),
                AnswerControl::Stop
            );
            let error = adapter
                .complete(Ok(BoundedAnswerStats::default()))
                .unwrap_err();
            assert_eq!(match_code(&error), code);
        }
    }

    fn match_code(error: &OrmError) -> &str {
        let OrmError::Match(error) = error else {
            panic!("expected structured match error, got {error}")
        };
        error.code().as_str()
    }
}
