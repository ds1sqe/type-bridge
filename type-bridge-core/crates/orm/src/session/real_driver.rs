//! Real TypeDB backend adapter over the shared `type-bridge-typedb-runtime`.
//!
//! This module is only compiled when the `typedb` feature is enabled.

#[cfg(not(any(feature = "band7", feature = "band8", feature = "band9")))]
compile_error!(
    "type-bridge-orm: the `typedb` machinery requires at least one band feature; enable `band7`, `band8`, and/or `band9` (all are default)"
);

use type_bridge_typedb_runtime as runtime;

pub use runtime::{
    ConnectOptions, PINNED_DRIVER_VERSION, PINNED_DRIVER_VERSION_B7, PINNED_DRIVER_VERSION_B9,
    embedded_driver_versions,
};

use super::backend::{
    BoxFuture, DriverBackend, GivenRowsSpec, GivenValue, QueryResult, TransactionOps, TxType,
};
use crate::error::OrmError;

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

impl DriverBackend for RealBackend {
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

struct RealTransaction {
    inner: runtime::RuntimeTransaction,
}

impl TransactionOps for RealTransaction {
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
        }
    }
}
