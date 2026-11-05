//! Python marshalling for the shared unchecked-query lifecycle.

use crate::orm_runtime::provider_block_on;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use type_bridge_orm::session::raw::{RawQueryError as CoreRawError, RawTransaction};
use type_bridge_orm::{CommitFailureCertainty, ProviderRuntimeOwner};

pyo3::create_exception!(type_bridge_core, RawQueryError, PyRuntimeError);

fn raw_error(error: CoreRawError) -> PyErr {
    Python::attach(|py| {
        let exception = RawQueryError::new_err(error.to_string());
        let value = exception.value(py);
        let attributes = (|| -> PyResult<()> {
            value.setattr("code", error.code)?;
            value.setattr("message", error.message)?;
            value.setattr("status", error.status)?;
            value.setattr("provider_code", error.provider_code)?;
            value.setattr(
                "commit_outcome",
                error.commit_outcome.map(|outcome| match outcome {
                    CommitFailureCertainty::DefinitelyAborted => "definitely_aborted",
                    CommitFailureCertainty::Unknown => "unknown",
                }),
            )?;
            Ok(())
        })();
        attributes.err().unwrap_or(exception)
    })
}

/// Explicit eager direct or single-query proxy raw context.
#[pyclass(name = "RawTransaction", frozen)]
pub(crate) struct PyRawTransaction {
    inner: tokio::sync::Mutex<RawTransaction>,
    runtime: Arc<ProviderRuntimeOwner>,
    finalized: AtomicBool,
    commit_on_exit: bool,
}

impl PyRawTransaction {
    pub(crate) fn new(
        inner: RawTransaction,
        runtime: Arc<ProviderRuntimeOwner>,
        commit_on_exit: bool,
    ) -> Self {
        Self {
            inner: tokio::sync::Mutex::new(inner),
            runtime,
            finalized: AtomicBool::new(false),
            commit_on_exit,
        }
    }

    pub(crate) fn direct(
        py: Python<'_>,
        database: &crate::orm_runtime::PyDatabase,
        kind: &str,
        commit_on_exit: bool,
    ) -> PyResult<Self> {
        let kind = crate::staged_transaction_runtime::parse_kind(kind)?;
        let (database, runtime) = database.handles();
        let transaction = provider_block_on(py, &runtime, database.raw_transaction(kind.into()))
            .map_err(raw_error)?;
        Ok(Self::new(transaction, runtime, commit_on_exit))
    }

    pub(crate) fn proxy(
        database: &crate::proxy_connection_runtime::PyProxyConnection,
        kind: &str,
        commit_on_exit: bool,
    ) -> PyResult<Self> {
        let kind = crate::staged_transaction_runtime::parse_kind(kind)?;
        let (connection, runtime) = database.handles();
        Ok(Self::new(
            connection.raw_transaction(kind.into()).map_err(raw_error)?,
            runtime,
            commit_on_exit,
        ))
    }

    pub(crate) fn execute_one(&self, py: Python<'_>, query: &str) -> PyResult<Py<PyAny>> {
        let result = self.execute_query(py, query).and_then(|answer| {
            self.commit(py)?;
            Ok(answer)
        });
        let closed = self.close(py);
        let answer = result?;
        closed?;
        Ok(answer)
    }
}

#[pymethods]
impl PyRawTransaction {
    /// Raw, unchecked TypeQL: the escape hatch. The TypeQL text is sent
    /// to TypeDB as written, and TypeDB's output comes back as it
    /// answered, in this SDK's native data types. TypeBridge adds no
    /// validation, rewriting, hydration, hooks, limits or extra
    /// queries. It runs in read, write and schema transactions and
    /// accepts any TypeQL, schema-breaking changes included. The caller
    /// owns its correctness; TypeBridge makes no promise about its
    /// effects. On a proxy connection it posts to the server's opt-in
    /// `/v3/raw` endpoint, which is off by default.
    ///
    /// On a proxy connection a raw transaction holds one query, because
    /// `/v3/raw` runs each request as its own transaction.
    fn execute_query(&self, py: Python<'_>, query: &str) -> PyResult<Py<PyAny>> {
        let value = provider_block_on(py, &self.runtime, async {
            let mut transaction = self.inner.lock().await;
            let bytes = transaction.execute_query(query).await?;
            transaction.decode_answer(&bytes)
        })
        .map_err(raw_error)?;
        crate::json_value_to_python(py, &value)
    }

    /// Commit pending direct writes; successful proxy writes are already committed.
    fn commit(&self, py: Python<'_>) -> PyResult<()> {
        self.finalized.store(true, Ordering::Release);
        provider_block_on(py, &self.runtime, async {
            self.inner.lock().await.commit().await
        })
        .map_err(raw_error)
    }

    /// Roll back direct writes; committed proxy writes return a typed refusal.
    fn rollback(&self, py: Python<'_>) -> PyResult<()> {
        self.finalized.store(true, Ordering::Release);
        provider_block_on(py, &self.runtime, async {
            self.inner.lock().await.rollback().await
        })
        .map_err(raw_error)
    }

    /// Release the context without committing pending direct writes.
    fn close(&self, py: Python<'_>) -> PyResult<()> {
        self.finalized.store(true, Ordering::Release);
        provider_block_on(py, &self.runtime, async {
            self.inner.lock().await.close().await
        })
        .map_err(raw_error)
    }

    fn __enter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    fn __exit__(
        &self,
        py: Python<'_>,
        exc_type: Option<&Bound<'_, PyAny>>,
        _exc_value: Option<&Bound<'_, PyAny>>,
        _traceback: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<bool> {
        let committed =
            if exc_type.is_none() && self.commit_on_exit && !self.finalized.load(Ordering::Acquire)
            {
                self.commit(py)
            } else {
                Ok(())
            };
        let closed = self.close(py);
        committed?;
        closed?;
        Ok(false)
    }
}

pub(crate) fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyRawTransaction>()?;
    module.add("RawQueryError", module.py().get_type::<RawQueryError>())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use type_bridge_orm::{TxType, session::raw::RawExchangeFn};

    #[test]
    fn raw_error_preserves_provider_code_message_and_unknown_commit() {
        Python::initialize();
        Python::attach(|py| {
            let error = raw_error(CoreRawError {
                code: "commit_outcome_unknown".into(),
                message: "exact provider message".into(),
                status: 503,
                provider_code: Some("CONNECTION".into()),
                commit_outcome: Some(CommitFailureCertainty::Unknown),
            });
            assert!(error.is_instance_of::<RawQueryError>(py));
            let value = error.value(py);
            for (field, expected) in [
                ("code", "commit_outcome_unknown"),
                ("message", "exact provider message"),
                ("provider_code", "CONNECTION"),
                ("commit_outcome", "unknown"),
            ] {
                assert_eq!(
                    value.getattr(field).unwrap().extract::<String>().unwrap(),
                    expected
                );
            }
            assert_eq!(
                value.getattr("status").unwrap().extract::<u16>().unwrap(),
                503
            );
        });
    }

    #[test]
    fn raw_answers_keep_python_integer_precision_and_proxy_lifecycle() {
        Python::initialize();
        let exchange: RawExchangeFn = Arc::new(|_| {
            Box::pin(async {
                Ok(br#"{"answers":[{"min":-9223372036854775808,"max":9223372036854775807,"unsafe":9007199254740993,"safe":7,"double":1.25}]}"#.to_vec())
            })
        });
        let transaction = RawTransaction::proxy(
            exchange,
            TxType::Write,
            std::num::NonZeroU64::new(60_000).unwrap(),
        );
        let transaction = PyRawTransaction::new(
            transaction,
            Arc::new(ProviderRuntimeOwner::new().unwrap()),
            true,
        );
        Python::attach(|py| {
            let answer = transaction.execute_query(py, "exact raw text").unwrap();
            let row = answer
                .bind(py)
                .get_item("answers")
                .unwrap()
                .get_item(0)
                .unwrap();
            for (key, expected) in [
                ("min", i64::MIN),
                ("max", i64::MAX),
                ("unsafe", 9_007_199_254_740_993),
                ("safe", 7),
            ] {
                assert_eq!(
                    row.get_item(key).unwrap().extract::<i64>().unwrap(),
                    expected
                );
            }
            assert_eq!(
                row.get_item("double").unwrap().extract::<f64>().unwrap(),
                1.25
            );
            let error = transaction.execute_query(py, "second").unwrap_err();
            assert_eq!(
                error
                    .value(py)
                    .getattr("code")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "raw_transaction_single_query"
            );
            transaction.commit(py).unwrap();
            let error = transaction.rollback(py).unwrap_err();
            assert_eq!(
                error
                    .value(py)
                    .getattr("code")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "raw_transaction_already_committed"
            );
            transaction.close(py).unwrap();
        });
    }
}
