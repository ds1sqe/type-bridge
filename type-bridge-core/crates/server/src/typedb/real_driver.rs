//! Real TypeDB backend adapter over the shared `type-bridge-typedb-runtime`.
//!
//! This module is only compiled when the `typedb` feature is enabled.

#[cfg(not(any(feature = "band7", feature = "band8")))]
compile_error!(
    "type-bridge-server: the `typedb` machinery requires at least one band feature; enable `band7` and/or `band8` (both are default)"
);

use type_bridge_typedb_runtime as runtime;

pub use runtime::{PINNED_DRIVER_VERSION, PINNED_DRIVER_VERSION_B7};

use super::backend::{BoxFuture, DriverBackend, QueryResultKind, TransactionOps, TransactionType};
use crate::config::TypeDBSection;
use crate::error::PipelineError;

/// Real TypeDB backend wrapping the shared runtime.
pub(crate) struct RealTypeDBBackend {
    inner: runtime::TypeDBRuntime,
}

impl RealTypeDBBackend {
    /// Connect to a TypeDB server using the provided configuration.
    ///
    /// Uses the same shared runtime gate as the ORM: caller-supplied
    /// `server_version` skips HTTP probing, otherwise HTTP probing falls back
    /// to gRPC-only negotiation when the HTTP endpoint is unavailable.
    pub(crate) async fn connect(config: &TypeDBSection) -> Result<Self, PipelineError> {
        let options = connect_options(config)?;
        let inner = runtime::TypeDBRuntime::connect(
            &config.address,
            &config.username,
            &config.password,
            options,
        )
        .await
        .map_err(PipelineError::from)?;
        Ok(Self { inner })
    }
}

impl DriverBackend for RealTypeDBBackend {
    fn open_transaction(
        &self,
        database: &str,
        tx_type: TransactionType,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, PipelineError>> {
        let runtime_tx_type = runtime_tx_type(tx_type);
        let database = database.to_string();
        Box::pin(async move {
            let inner = self
                .inner
                .open_transaction(&database, runtime_tx_type)
                .await
                .map_err(PipelineError::from)?;
            Ok(Box::new(RealTransaction { inner }) as Box<dyn TransactionOps>)
        })
    }

    fn database_exists(&self, database: &str) -> BoxFuture<'_, Result<bool, PipelineError>> {
        let database = database.to_string();
        Box::pin(async move {
            self.inner
                .database_exists(&database)
                .await
                .map_err(PipelineError::from)
        })
    }

    fn create_database(&self, database: &str) -> BoxFuture<'_, Result<(), PipelineError>> {
        let database = database.to_string();
        Box::pin(async move {
            self.inner
                .create_database(&database)
                .await
                .map_err(PipelineError::from)
        })
    }

    fn delete_database(&self, database: &str) -> BoxFuture<'_, Result<(), PipelineError>> {
        let database = database.to_string();
        Box::pin(async move {
            self.inner
                .delete_database(&database)
                .await
                .map_err(PipelineError::from)
        })
    }

    fn is_open(&self) -> bool {
        self.inner.is_open()
    }
}

struct RealTransaction {
    inner: runtime::RuntimeTransaction,
}

impl TransactionOps for RealTransaction {
    fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResultKind, PipelineError>> {
        let typeql = typeql.to_string();
        Box::pin(async move {
            self.inner
                .query(&typeql)
                .await
                .map(query_result_kind)
                .map_err(PipelineError::from)
        })
    }

    fn commit(&mut self) -> BoxFuture<'_, Result<(), PipelineError>> {
        Box::pin(async move { self.inner.commit().await.map_err(PipelineError::from) })
    }
}

fn connect_options(config: &TypeDBSection) -> Result<runtime::ConnectOptions, PipelineError> {
    let server_version = config
        .server_version
        .as_deref()
        .map(str::parse)
        .transpose()
        .map_err(PipelineError::UnsupportedVersion)?;

    Ok(runtime::ConnectOptions {
        http_port: config.http_port,
        tls: false,
        server_version,
    })
}

fn runtime_tx_type(tx_type: TransactionType) -> runtime::TxType {
    match tx_type {
        TransactionType::Read => runtime::TxType::Read,
        TransactionType::Write => runtime::TxType::Write,
        TransactionType::Schema => runtime::TxType::Schema,
    }
}

fn query_result_kind(result: runtime::QueryResult) -> QueryResultKind {
    match result {
        runtime::QueryResult::Ok => QueryResultKind::Ok,
        runtime::QueryResult::Rows(rows) => QueryResultKind::Rows(rows),
        runtime::QueryResult::Documents(docs) => QueryResultKind::Documents(docs),
    }
}

impl From<runtime::RuntimeError> for PipelineError {
    fn from(error: runtime::RuntimeError) -> Self {
        match error {
            runtime::RuntimeError::UnsupportedVersion(error) => Self::UnsupportedVersion(error),
            runtime::RuntimeError::Connection(message) => Self::Connection(message),
            runtime::RuntimeError::QueryExecution(message) => Self::QueryExecution(message),
            runtime::RuntimeError::Transaction(message) => Self::QueryExecution(message),
        }
    }
}
