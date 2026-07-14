//! Backend abstraction traits for TypeDB connectivity.
//!
//! These traits enable testing with mock backends while using the real
//! TypeDB driver in production (behind the `typedb` feature flag).

use std::future::Future;
use std::pin::Pin;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use type_bridge_core_lib::ast::{
    TypedFetchRows, TypedHydrateThings, TypedLiteral, TypedPageRematch, TypedRootScan,
};
use type_bridge_core_lib::compiler::{PreparedTypedStatement, QueryCompiler};

use crate::error::OrmError;
use crate::match_request::CapabilitySet;
use crate::match_request::{MatchError, MatchErrorCategory, MatchErrorPathSegment};

/// Result of a query execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueryResult {
    /// No data returned (write/schema confirmation).
    Ok,
    /// Document results from a fetch clause.
    Documents(Vec<serde_json::Value>),
    /// Row results from a match + reduce/concept query.
    Rows(Vec<serde_json::Value>),
}

/// One row or document delivered by the bounded answer seam.
#[derive(Debug, Clone, PartialEq)]
pub enum AnswerItem {
    /// A concept row.
    Row(serde_json::Value),
    /// A concept document.
    Document(serde_json::Value),
}

impl AnswerItem {
    fn encoded_bytes(&self) -> Result<u64, OrmError> {
        let value = match self {
            Self::Row(value) | Self::Document(value) => value,
        };
        u64::try_from(serde_json::to_vec(value)?.len()).map_err(|_| {
            resource_error(
                "answer_byte_counter_overflow",
                "encoded provider answer length exceeds the counter range",
            )
        })
    }
}

/// Whether a bounded answer consumer needs another provider item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerControl {
    /// Continue reading.
    Continue,
    /// Stop before requesting another item.
    Stop,
}

/// Synchronous typed decode boundary invoked once per provider item.
pub trait AnswerConsumer: Send {
    /// Validate/decode one item and decide whether another item is required.
    fn accept(&mut self, item: AnswerItem) -> Result<AnswerControl, OrmError>;
}

impl<F> AnswerConsumer for F
where
    F: FnMut(AnswerItem) -> Result<AnswerControl, OrmError> + Send,
{
    fn accept(&mut self, item: AnswerItem) -> Result<AnswerControl, OrmError> {
        self(item)
    }
}

/// Cloneable cooperative cancellation signal checked between provider items
/// and used to wake in-flight provider awaits.
#[derive(Debug, Clone)]
pub struct AnswerCancellation {
    cancelled: watch::Sender<bool>,
}

impl Default for AnswerCancellation {
    fn default() -> Self {
        let (cancelled, _) = watch::channel(false);
        Self { cancelled }
    }
}

impl AnswerCancellation {
    /// Request cancellation.
    pub fn cancel(&self) {
        self.cancelled.send_replace(true);
    }

    /// Return whether cancellation was requested.
    pub fn is_cancelled(&self) -> bool {
        *self.cancelled.borrow()
    }

    #[cfg_attr(not(feature = "typedb"), allow(dead_code))]
    pub(crate) fn shared(&self) -> watch::Sender<bool> {
        self.cancelled.clone()
    }

    pub(crate) async fn cancelled(&self) {
        let mut receiver = self.cancelled.subscribe();
        if *receiver.borrow_and_update() {
            return;
        }
        while receiver.changed().await.is_ok() {
            if *receiver.borrow_and_update() {
                return;
            }
        }
    }
}

/// Hard limits enforced before and between streamed provider items.
#[derive(Debug, Clone)]
pub struct BoundedAnswerLimits {
    /// Maximum processed rows/documents.
    pub max_items: u64,
    /// Maximum encoded response bytes.
    pub max_bytes: u64,
    /// Optional transaction deadline.
    pub deadline: Option<Instant>,
    /// Cooperative cancellation signal.
    pub cancellation: AnswerCancellation,
}

impl Default for BoundedAnswerLimits {
    fn default() -> Self {
        Self {
            max_items: 100_000,
            max_bytes: 64 * 1024 * 1024,
            deadline: None,
            cancellation: AnswerCancellation::default(),
        }
    }
}

/// Counters produced without materializing an unbounded answer vector.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BoundedAnswerStats {
    /// Items accepted from the provider.
    pub processed_items: u64,
    /// Encoded bytes accepted from the provider.
    pub response_bytes: u64,
    /// Whether the consumer stopped before provider exhaustion.
    pub stopped_early: bool,
}

/// Stateful limit enforcement shared by real and recording answer readers.
pub struct BoundedAnswerReader {
    limits: BoundedAnswerLimits,
    stats: BoundedAnswerStats,
}

impl BoundedAnswerReader {
    /// Create a reader with executor/session hard limits.
    pub fn new(limits: BoundedAnswerLimits) -> Self {
        Self {
            limits,
            stats: BoundedAnswerStats::default(),
        }
    }

    /// Check cancellation and deadline before starting or requesting an item.
    pub fn check_before_read(&self) -> Result<(), OrmError> {
        if self.limits.cancellation.is_cancelled() {
            return Err(resource_error(
                "provider_cancelled",
                "provider answer processing was cancelled",
            ));
        }
        if self.limits.deadline.is_some_and(|deadline| {
            tokio::time::Instant::now() >= tokio::time::Instant::from_std(deadline)
        }) {
            return Err(resource_error(
                "transaction_deadline_exceeded",
                "provider transaction deadline expired",
            ));
        }
        Ok(())
    }

    /// Process exactly one item and enforce counters before decode exposure.
    pub fn accept(
        &mut self,
        item: AnswerItem,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<AnswerControl, OrmError> {
        self.check_before_read()?;
        let next_items = self.stats.processed_items.checked_add(1).ok_or_else(|| {
            resource_error(
                "processed_item_counter_overflow",
                "processed provider item counter overflowed",
            )
        })?;
        if next_items > self.limits.max_items {
            return Err(resource_error(
                "processed_item_limit",
                "provider answer exceeded the processed-item ceiling",
            ));
        }
        let next_bytes = self
            .stats
            .response_bytes
            .checked_add(item.encoded_bytes()?)
            .ok_or_else(|| {
                resource_error(
                    "answer_byte_counter_overflow",
                    "provider answer byte counter overflowed",
                )
            })?;
        if next_bytes > self.limits.max_bytes {
            return Err(resource_error(
                "response_byte_limit",
                "provider answer exceeded the response-byte ceiling",
            ));
        }
        self.stats.processed_items = next_items;
        self.stats.response_bytes = next_bytes;
        let control = consumer.accept(item);
        self.check_before_read()?;
        let control = control?;
        if control == AnswerControl::Stop {
            self.stats.stopped_early = true;
        }
        Ok(control)
    }

    /// Return current bounded counters.
    pub const fn stats(&self) -> BoundedAnswerStats {
        self.stats
    }
}

/// Boxed future type alias for async trait methods.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Transaction type for TypeDB operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxType {
    /// Read-only transaction (no commit needed).
    Read,
    /// Read-write transaction (auto-committed after query).
    Write,
    /// Schema transaction for defining types.
    Schema,
}

/// A single value bound to a `given` variable.
///
/// Temporal variants carry ISO-8601 text; the real backend parses them when
/// lowering onto the driver, mirroring how [`QueryResult`] renders temporal
/// values back to strings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GivenValue {
    /// TypeQL `boolean`.
    Boolean(bool),
    /// TypeQL `integer`.
    Integer(i64),
    /// TypeQL `double`.
    Double(f64),
    /// TypeQL `string`.
    String(String),
    /// TypeQL `date`, ISO-8601 (`YYYY-MM-DD`).
    Date(String),
    /// TypeQL `datetime`, ISO-8601 without offset.
    Datetime(String),
    /// TypeQL `datetime-tz`, RFC 3339 with offset.
    DatetimeTz(String),
}

/// Input rows for a `given`-stage query: a variable header plus value rows.
///
/// Every row must have exactly one entry per header variable, in header
/// order. Rows travel through the driver API, never the query string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GivenRowsSpec {
    /// Variable names, without the `$` sigil, in column order.
    pub variables: Vec<String>,
    /// Value rows; each inner vec is one input row in header order.
    pub rows: Vec<Vec<GivenValue>>,
}

/// One compiler-selected provider route for an internal typed statement.
pub(crate) struct TypedProviderStatement {
    typeql: String,
    given_rows: Option<GivenRowsSpec>,
}

impl TypedProviderStatement {
    fn inline(typeql: String) -> Self {
        Self {
            typeql,
            given_rows: None,
        }
    }

    fn prepared(statement: PreparedTypedStatement) -> Result<Self, OrmError> {
        if statement.parameters.is_empty() {
            return Ok(Self::inline(statement.typeql));
        }
        let mut variables = Vec::with_capacity(statement.parameters.len());
        let mut values = Vec::with_capacity(statement.parameters.len());
        for parameter in statement.parameters {
            variables.push(parameter.name);
            values.push(given_value_from_typed(parameter.value)?);
        }
        Ok(Self {
            typeql: statement.typeql,
            given_rows: Some(GivenRowsSpec {
                variables,
                rows: vec![values],
            }),
        })
    }
}

fn given_value_from_typed(value: TypedLiteral) -> Result<GivenValue, OrmError> {
    match value {
        TypedLiteral::String(value) => Ok(GivenValue::String(value)),
        TypedLiteral::Long(value) => Ok(GivenValue::Integer(value)),
        TypedLiteral::Double(value) => Ok(GivenValue::Double(value)),
        TypedLiteral::Boolean(value) => Ok(GivenValue::Boolean(value)),
        TypedLiteral::Date(_)
        | TypedLiteral::DateTime(_)
        | TypedLiteral::DateTimeTz(_)
        | TypedLiteral::Decimal(_)
        | TypedLiteral::Duration(_) => Err(OrmError::Compilation(
            "prepared typed statement contained a value that must remain inline".into(),
        )),
    }
}

pub(crate) fn typed_fetch_provider_statement(
    query: &TypedFetchRows,
    supports_given_rows: bool,
) -> Result<TypedProviderStatement, OrmError> {
    let compiler = QueryCompiler::new();
    if supports_given_rows {
        return compiler
            .prepare_typed_fetch_rows(query)
            .map_err(|error| OrmError::Compilation(error.to_string()))
            .and_then(TypedProviderStatement::prepared);
    }
    compiler
        .compile_typed_fetch_rows(query)
        .map(TypedProviderStatement::inline)
        .map_err(|error| OrmError::Compilation(error.to_string()))
}

pub(crate) fn typed_root_provider_statement(
    query: &TypedRootScan,
    supports_given_rows: bool,
) -> Result<TypedProviderStatement, OrmError> {
    let compiler = QueryCompiler::new();
    if supports_given_rows {
        return compiler
            .prepare_typed_root_scan(query)
            .map_err(|error| OrmError::Compilation(error.to_string()))
            .and_then(TypedProviderStatement::prepared);
    }
    compiler
        .compile_typed_root_scan(query)
        .map(TypedProviderStatement::inline)
        .map_err(|error| OrmError::Compilation(error.to_string()))
}

pub(crate) fn typed_rematch_provider_statement(
    query: &TypedPageRematch,
    supports_given_rows: bool,
) -> Result<TypedProviderStatement, OrmError> {
    let compiler = QueryCompiler::new();
    if supports_given_rows {
        return compiler
            .prepare_typed_page_rematch(query)
            .map_err(|error| OrmError::Compilation(error.to_string()))
            .and_then(TypedProviderStatement::prepared);
    }
    compiler
        .compile_typed_page_rematch(query)
        .map(TypedProviderStatement::inline)
        .map_err(|error| OrmError::Compilation(error.to_string()))
}

pub(crate) async fn execute_typed_provider_statement<T: TransactionOps + ?Sized>(
    transaction: &mut T,
    statement: TypedProviderStatement,
    limits: BoundedAnswerLimits,
    consumer: &mut dyn AnswerConsumer,
) -> Result<BoundedAnswerStats, OrmError> {
    if let Some(rows) = statement.given_rows {
        transaction
            .query_with_rows_bounded(&statement.typeql, rows, limits, consumer)
            .await
    } else {
        transaction
            .query_bounded(&statement.typeql, limits, consumer)
            .await
    }
}

/// Abstraction over a TypeDB transaction.
///
/// Implementations handle query execution and transaction lifecycle.
/// The `RealBackend` (behind the `typedb` feature) provides the real
/// TypeDB implementation; tests use mock implementations.
pub trait TransactionOps: Send {
    /// Whether this transaction's active provider can transport `given` rows.
    ///
    /// Implementations opt in explicitly. The default keeps mocks and legacy
    /// backends on their existing unsupported/fallback behavior.
    fn supports_given_rows(&self) -> bool {
        false
    }

    /// Execute a TypeQL query string.
    fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>>;

    /// Execute a TypeQL query with `given`-stage input rows.
    ///
    /// Rows travel through the driver API, not the query string. Defaults to
    /// an error for backends without given-stage support (mocks, pre-band-9
    /// drivers); the real backend overrides this on band-9 connections.
    fn query_with_rows(
        &mut self,
        typeql: &str,
        rows: GivenRowsSpec,
    ) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        let _ = (typeql, rows);
        Box::pin(async move {
            Err(OrmError::QueryExecution(
                "given-stage parameterized queries are not supported by this backend".into(),
            ))
        })
    }

    /// Execute a `given` query through the bounded answer seam.
    ///
    /// The default safely applies limits while consuming a materialized answer
    /// from mocks and legacy backends. Real transactions override this method
    /// so early stop, cancellation, deadlines, and limits apply while polling
    /// the driver stream.
    fn query_with_rows_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        rows: GivenRowsSpec,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        Box::pin(async move {
            let mut reader = BoundedAnswerReader::new(limits);
            reader.check_before_read()?;
            match self.query_with_rows(typeql, rows).await? {
                QueryResult::Ok => {}
                QueryResult::Rows(rows) => {
                    for row in rows {
                        if reader.accept(AnswerItem::Row(row), consumer)? == AnswerControl::Stop {
                            break;
                        }
                    }
                }
                QueryResult::Documents(documents) => {
                    for document in documents {
                        if reader.accept(AnswerItem::Document(document), consumer)?
                            == AnswerControl::Stop
                        {
                            break;
                        }
                    }
                }
            }
            Ok(reader.stats())
        })
    }

    /// Execute a legacy TypeQL string through the bounded answer seam.
    ///
    /// The default preserves compatibility for test/legacy transactions that
    /// only implement materialized `query`. Real transactions override this
    /// method and enforce limits while consuming the driver stream.
    fn query_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        Box::pin(async move {
            let mut reader = BoundedAnswerReader::new(limits);
            reader.check_before_read()?;
            let result = self.query(typeql).await?;
            let items = match result {
                QueryResult::Ok => Vec::new(),
                QueryResult::Rows(rows) => rows.into_iter().map(AnswerItem::Row).collect(),
                QueryResult::Documents(documents) => {
                    documents.into_iter().map(AnswerItem::Document).collect()
                }
            };
            for item in items {
                if reader.accept(item, consumer)? == AnswerControl::Stop {
                    break;
                }
            }
            Ok(reader.stats())
        })
    }

    /// Compile and execute one canonical typed selected-row statement.
    fn query_typed_bounded<'a>(
        &'a mut self,
        query: &'a TypedFetchRows,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        Box::pin(async move {
            let statement = typed_fetch_provider_statement(query, self.supports_given_rows())?;
            execute_typed_provider_statement(self, statement, limits, consumer).await
        })
    }

    /// Compile and execute one typed distinct-root stream.
    fn query_root_typed_bounded<'a>(
        &'a mut self,
        query: &'a TypedRootScan,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        Box::pin(async move {
            let statement = typed_root_provider_statement(query, self.supports_given_rows())?;
            execute_typed_provider_statement(self, statement, limits, consumer).await
        })
    }

    /// Compile and execute one exact batched page re-match/hydration.
    fn rematch_page_typed_bounded<'a>(
        &'a mut self,
        query: &'a TypedPageRematch,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        Box::pin(async move {
            let statement = typed_rematch_provider_statement(query, self.supports_given_rows())?;
            execute_typed_provider_statement(self, statement, limits, consumer).await
        })
    }

    /// Compile and execute one complete batched selected-thing hydration.
    fn hydrate_typed_bounded<'a>(
        &'a mut self,
        query: &'a TypedHydrateThings,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        Box::pin(async move {
            let typeql = QueryCompiler::new()
                .compile_typed_hydrate_things(query)
                .map_err(|error| OrmError::Compilation(error.to_string()))?;
            self.query_bounded(&typeql, limits, consumer).await
        })
    }

    /// Commit this transaction. Only meaningful for write/schema transactions.
    fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>>;

    /// Roll back this transaction.
    fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>>;

    /// Close this transaction without committing.
    fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>>;
}

fn resource_error(code: &'static str, message: &'static str) -> OrmError {
    MatchError::new(MatchErrorCategory::ResourceLimit, code, message)
        .at(MatchErrorPathSegment::ProviderEvidence)
        .into()
}

/// Abstraction over a TypeDB driver connection.
///
/// Opens transactions against a named database. Implementations must be
/// thread-safe (`Send + Sync`) for use across async tasks.
pub trait DriverBackend: Send + Sync {
    /// Canonical selected-match capabilities supported by this backend.
    ///
    /// The default is deliberately empty: legacy/materializing backends must
    /// opt in only after implementing the complete typed bounded-answer seam.
    fn match_capabilities(&self) -> CapabilitySet {
        CapabilitySet::new()
    }

    /// Open a new transaction against the given database.
    fn open_transaction(
        &self,
        database: &str,
        tx_type: TxType,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>>;

    /// Check if the underlying connection is still alive.
    fn is_open(&self) -> bool;

    /// The server version detected at connect time, when the backend knows it.
    ///
    /// Defaults to `None` for backends without a version gate (mocks, embedded
    /// test backends). The real TypeDB backend reports the version the connect
    /// gate detected; it is `None` only on the band-7 gRPC fallback.
    fn server_version(&self) -> Option<type_bridge_core_lib::version::Version> {
        None
    }

    /// Whether the active negotiated provider can transport `given` rows.
    ///
    /// Server syntax support and provider transport support are separate: a
    /// 3.12 server reached through a band-8 fallback still cannot accept the
    /// band-9 `GivenRows` driver payload. Backends must opt in explicitly.
    fn supports_given_rows(&self) -> bool {
        false
    }

    /// Check whether a database exists, when the backend supports database
    /// lifecycle operations.
    fn database_exists(&self, database: &str) -> BoxFuture<'_, Result<bool, OrmError>> {
        let database = database.to_string();
        Box::pin(async move {
            Err(OrmError::Connection(format!(
                "Database existence checks are not supported by this backend for database '{database}'"
            )))
        })
    }

    /// Create a database, when the backend supports database lifecycle
    /// operations.
    fn create_database(&self, database: &str) -> BoxFuture<'_, Result<(), OrmError>> {
        let database = database.to_string();
        Box::pin(async move {
            Err(OrmError::Connection(format!(
                "Database creation is not supported by this backend for database '{database}'"
            )))
        })
    }

    /// Delete a database, when the backend supports database lifecycle
    /// operations.
    fn delete_database(&self, database: &str) -> BoxFuture<'_, Result<(), OrmError>> {
        let database = database.to_string();
        Box::pin(async move {
            Err(OrmError::Connection(format!(
                "Database deletion is not supported by this backend for database '{database}'"
            )))
        })
    }

    /// Export the database schema as TypeQL text, when the backend supports it.
    fn schema_text(&self, database: &str) -> BoxFuture<'_, Result<String, OrmError>> {
        let database = database.to_string();
        Box::pin(async move {
            Err(OrmError::Connection(format!(
                "Schema export is not supported by this backend for database '{database}'"
            )))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use type_bridge_core_lib::ast::{
        TypedComparisonOperator, TypedFieldBinding, TypedMatchPredicate, TypedMatchTarget,
        TypedThingKind,
    };

    #[derive(Default)]
    struct StopAfterOne {
        accepted: Vec<AnswerItem>,
    }

    impl AnswerConsumer for StopAfterOne {
        fn accept(&mut self, item: AnswerItem) -> Result<AnswerControl, OrmError> {
            self.accepted.push(item);
            Ok(AnswerControl::Stop)
        }
    }

    struct UnsupportedGivenTransaction;

    impl TransactionOps for UnsupportedGivenTransaction {
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

    struct MaterializedGivenTransaction;

    impl TransactionOps for MaterializedGivenTransaction {
        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            Box::pin(async { panic!("bounded given fallback used the raw query path") })
        }

        fn query_with_rows(
            &mut self,
            _typeql: &str,
            _rows: GivenRowsSpec,
        ) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            Box::pin(async {
                Ok(QueryResult::Rows(vec![
                    serde_json::json!({"v": 1}),
                    serde_json::json!({"v": 2}),
                ]))
            })
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

    struct TypedRouteTransaction {
        supports_given: bool,
        plain: Vec<String>,
        prepared: Vec<(String, GivenRowsSpec)>,
    }

    impl TypedRouteTransaction {
        fn new(supports_given: bool) -> Self {
            Self {
                supports_given,
                plain: Vec::new(),
                prepared: Vec::new(),
            }
        }
    }

    impl TransactionOps for TypedRouteTransaction {
        fn supports_given_rows(&self) -> bool {
            self.supports_given
        }

        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            Box::pin(async { panic!("typed route must use a bounded provider seam") })
        }

        fn query_bounded<'a>(
            &'a mut self,
            typeql: &'a str,
            _limits: BoundedAnswerLimits,
            _consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            self.plain.push(typeql.to_owned());
            Box::pin(async { Ok(BoundedAnswerStats::default()) })
        }

        fn query_with_rows_bounded<'a>(
            &'a mut self,
            typeql: &'a str,
            rows: GivenRowsSpec,
            _limits: BoundedAnswerLimits,
            _consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            self.prepared.push((typeql.to_owned(), rows));
            Box::pin(async { Ok(BoundedAnswerStats::default()) })
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

    fn typed_literal_fetch(value: TypedLiteral) -> TypedFetchRows {
        TypedFetchRows {
            targets: vec![TypedMatchTarget {
                binding: 0,
                kind: TypedThingKind::Entity,
                type_name: "person".into(),
                exact: false,
            }],
            fields: vec![TypedFieldBinding {
                id: 0,
                owner: 0,
                field_name: "name".into(),
            }],
            predicate: Some(TypedMatchPredicate::FieldValue {
                field: 0,
                operator: TypedComparisonOperator::Equal,
                value,
            }),
            projection: vec![0],
            distinct: true,
            order: vec![],
            offset: 0,
            limit: 1,
        }
    }

    fn empty_given_rows() -> GivenRowsSpec {
        GivenRowsSpec {
            variables: vec!["v".into()],
            rows: Vec::new(),
        }
    }

    #[tokio::test]
    async fn transaction_defaults_to_unsupported_given_rows() {
        let mut transaction = UnsupportedGivenTransaction;
        let mut consumer = |_item| Ok(AnswerControl::Continue);

        assert!(!transaction.supports_given_rows());
        let error = transaction
            .query_with_rows_bounded(
                "given $v: integer; match $x isa thing;",
                empty_given_rows(),
                BoundedAnswerLimits::default(),
                &mut consumer,
            )
            .await
            .unwrap_err();

        assert!(
            matches!(error, OrmError::QueryExecution(message) if message.contains("not supported by this backend"))
        );
    }

    #[tokio::test]
    async fn bounded_given_fallback_stops_while_consuming_materialized_mock_rows() {
        let mut transaction = MaterializedGivenTransaction;
        let mut accepted = Vec::new();
        let mut consumer = |item| {
            accepted.push(item);
            Ok(AnswerControl::Stop)
        };

        let stats = transaction
            .query_with_rows_bounded(
                "given $v: integer; match $x isa thing;",
                empty_given_rows(),
                BoundedAnswerLimits::default(),
                &mut consumer,
            )
            .await
            .unwrap();

        assert_eq!(accepted, [AnswerItem::Row(serde_json::json!({"v": 1}))]);
        assert_eq!(stats.processed_items, 1);
        assert!(stats.stopped_early);
    }

    #[tokio::test]
    async fn bounded_given_fallback_enforces_limits_before_mock_consumer() {
        let mut transaction = MaterializedGivenTransaction;
        let mut accepted = 0;
        let mut consumer = |_item| {
            accepted += 1;
            Ok(AnswerControl::Continue)
        };

        let error = transaction
            .query_with_rows_bounded(
                "given $v: integer; match $x isa thing;",
                empty_given_rows(),
                BoundedAnswerLimits {
                    max_items: 1,
                    ..BoundedAnswerLimits::default()
                },
                &mut consumer,
            )
            .await
            .unwrap_err();

        assert_eq!(accepted, 1);
        let OrmError::Match(error) = error else {
            panic!("expected structured item limit error")
        };
        assert_eq!(error.code().as_str(), "processed_item_limit");
    }

    #[tokio::test]
    async fn typed_statements_route_one_given_row_only_on_lossless_band9_values() {
        let string_fetch = typed_literal_fetch(TypedLiteral::String("Alice".into()));
        let mut consumer = |_item| Ok(AnswerControl::Continue);

        let mut older_band = TypedRouteTransaction::new(false);
        older_band
            .query_typed_bounded(&string_fetch, BoundedAnswerLimits::default(), &mut consumer)
            .await
            .unwrap();
        assert_eq!(older_band.prepared.len(), 0);
        assert_eq!(older_band.plain.len(), 1);
        assert!(older_band.plain[0].contains("$f0 == \"Alice\""));

        let mut band9 = TypedRouteTransaction::new(true);
        band9
            .query_typed_bounded(&string_fetch, BoundedAnswerLimits::default(), &mut consumer)
            .await
            .unwrap();
        assert!(band9.plain.is_empty());
        assert_eq!(band9.prepared.len(), 1);
        let (typeql, rows) = &band9.prepared[0];
        assert!(typeql.starts_with("given $g0: string;\nmatch"));
        assert!(typeql.contains("$f0 == $g0"));
        assert_eq!(rows.variables, ["g0"]);
        assert_eq!(rows.rows, [vec![GivenValue::String("Alice".into())]]);

        let root = TypedRootScan {
            targets: string_fetch.targets.clone(),
            fields: string_fetch.fields.clone(),
            predicate: string_fetch.predicate.clone(),
            root: 0,
            order: vec![],
            offset: None,
            limit: Some(1),
        };
        band9
            .query_root_typed_bounded(&root, BoundedAnswerLimits::default(), &mut consumer)
            .await
            .unwrap();
        let rematch = TypedPageRematch {
            targets: string_fetch.targets.clone(),
            fields: string_fetch.fields.clone(),
            predicate: string_fetch.predicate.clone(),
            root: 0,
            root_concept_ids: vec!["0x01".into()],
            collection_orders: vec![],
        };
        band9
            .rematch_page_typed_bounded(&rematch, BoundedAnswerLimits::default(), &mut consumer)
            .await
            .unwrap();
        assert_eq!(band9.prepared.len(), 3);
        assert!(band9.prepared[1].0.starts_with("given $g0: string;\nmatch"));
        assert!(band9.prepared[2].0.starts_with("given $g0: string;\nmatch"));
        assert!(band9.prepared.iter().all(|(_, rows)| rows.rows.len() == 1));

        let temporal_fetch = typed_literal_fetch(TypedLiteral::DateTimeTz(
            "1987-12-22T17:29 Asia/Kolkata".into(),
        ));
        band9
            .query_typed_bounded(
                &temporal_fetch,
                BoundedAnswerLimits::default(),
                &mut consumer,
            )
            .await
            .unwrap();
        assert_eq!(band9.prepared.len(), 3);
        assert_eq!(band9.plain.len(), 1);
        assert!(band9.plain[0].contains("1987-12-22T17:29 Asia/Kolkata"));
    }

    #[test]
    fn bounded_reader_stops_before_requesting_another_item() {
        let mut reader = BoundedAnswerReader::new(BoundedAnswerLimits::default());
        let mut consumer = StopAfterOne::default();

        assert_eq!(
            reader
                .accept(AnswerItem::Row(serde_json::json!({"v": 1})), &mut consumer)
                .unwrap(),
            AnswerControl::Stop
        );
        assert_eq!(consumer.accepted.len(), 1);
        assert_eq!(reader.stats().processed_items, 1);
        assert!(reader.stats().stopped_early);
    }

    #[test]
    fn bounded_reader_rejects_item_and_byte_overflow_before_consumer() {
        let mut reader = BoundedAnswerReader::new(BoundedAnswerLimits {
            max_items: 0,
            ..BoundedAnswerLimits::default()
        });
        let mut consumer = StopAfterOne::default();
        let error = reader
            .accept(AnswerItem::Row(serde_json::json!(1)), &mut consumer)
            .unwrap_err();
        assert!(matches!(error, OrmError::Match(_)));
        assert!(consumer.accepted.is_empty());

        let mut reader = BoundedAnswerReader::new(BoundedAnswerLimits {
            max_bytes: 1,
            ..BoundedAnswerLimits::default()
        });
        let error = reader
            .accept(
                AnswerItem::Document(serde_json::json!({"too": "large"})),
                &mut consumer,
            )
            .unwrap_err();
        assert!(matches!(error, OrmError::Match(_)));
        assert!(consumer.accepted.is_empty());
    }

    #[test]
    fn bounded_reader_observes_deadline_and_cancellation() {
        let cancellation = AnswerCancellation::default();
        cancellation.cancel();
        let reader = BoundedAnswerReader::new(BoundedAnswerLimits {
            cancellation,
            ..BoundedAnswerLimits::default()
        });
        assert!(reader.check_before_read().is_err());

        let reader = BoundedAnswerReader::new(BoundedAnswerLimits {
            deadline: Some(Instant::now()),
            ..BoundedAnswerLimits::default()
        });
        assert!(reader.check_before_read().is_err());
    }

    #[test]
    fn bounded_reader_rechecks_cancellation_after_consumer_work() {
        let cancellation = AnswerCancellation::default();
        let trigger = cancellation.clone();
        let mut reader = BoundedAnswerReader::new(BoundedAnswerLimits {
            cancellation,
            ..BoundedAnswerLimits::default()
        });
        let mut consumer = move |_item| {
            trigger.cancel();
            Ok(AnswerControl::Continue)
        };

        let error = reader
            .accept(AnswerItem::Row(serde_json::json!({"v": 1})), &mut consumer)
            .unwrap_err();
        let OrmError::Match(error) = error else {
            panic!("expected structured cancellation error")
        };
        assert_eq!(error.code().as_str(), "provider_cancelled");

        let cancellation = AnswerCancellation::default();
        let trigger = cancellation.clone();
        let mut reader = BoundedAnswerReader::new(BoundedAnswerLimits {
            cancellation,
            ..BoundedAnswerLimits::default()
        });
        let mut failing_consumer = move |_item| -> Result<AnswerControl, OrmError> {
            trigger.cancel();
            Err(OrmError::QueryExecution("decoder failed".into()))
        };
        let error = reader
            .accept(
                AnswerItem::Row(serde_json::json!({"v": 1})),
                &mut failing_consumer,
            )
            .unwrap_err();
        let OrmError::Match(error) = error else {
            panic!("expected structured cancellation error")
        };
        assert_eq!(error.code().as_str(), "provider_cancelled");
    }
}
