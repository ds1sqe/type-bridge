//! PyO3 seam for a proxy connection over the built-in HTTP route.
//!
//! A `ProxyConnection` owns a verified server advertisement, the generated
//! package's query authority, its answer-limit ceiling, and the provider
//! runtime its terminals block on. Generated terminals run on it through the
//! same shared executor as direct databases and transactions.

use std::path::PathBuf;
use std::sync::Arc;

use pyo3::prelude::*;
use pyo3::types::PyAny;
use type_bridge_contract::limits::MAX_REMOTE_ENVELOPE_BYTES;
use type_bridge_orm::{
    AnswerCancellation, AnswerTransport, ProviderRuntimeOwner, ProxyEndpoint, ProxyQueryConnection,
    QueryExecutionResourceLimits,
};

use crate::match_runtime::{
    PyQueryCancellation, PyQueryExecutionResourceLimits, PyQueryLimits, py_sdk_diagnostic,
};
use crate::orm_runtime::provider_block_on;
use crate::plan_runtime::{PyPlanAuthority, PythonBytes};
use crate::runtime_projection::direct_tls_from_python;
use crate::staged_transaction_runtime::{PyStagedTransaction, parse_kind};
use type_bridge_orm::plan_authority::QueryAuthority;
use type_bridge_orm::session::staged::TransactionOptions;

/// One connection to a TypeBridge server reached over its HTTP routes.
#[pyclass(name = "ProxyConnection", frozen)]
pub(crate) struct PyProxyConnection {
    connection: Arc<ProxyQueryConnection>,
    runtime: Arc<ProviderRuntimeOwner>,
    authority: Arc<QueryAuthority>,
}

impl PyProxyConnection {
    pub(crate) fn handles(&self) -> (Arc<ProxyQueryConnection>, Arc<ProviderRuntimeOwner>) {
        (Arc::clone(&self.connection), Arc::clone(&self.runtime))
    }

    pub(crate) fn connection(&self) -> &ProxyQueryConnection {
        &self.connection
    }

    pub(crate) fn runtime(&self) -> &ProviderRuntimeOwner {
        self.runtime.as_ref()
    }
}

#[pymethods]
impl PyProxyConnection {
    /// Open an explicit raw context; clean context-manager exit commits by default.
    #[pyo3(signature = (kind="read", *, commit_on_exit=true))]
    fn raw_transaction(
        &self,
        py: Python<'_>,
        kind: &str,
        commit_on_exit: bool,
    ) -> PyResult<crate::raw_runtime::PyRawTransaction> {
        let _ = py;
        crate::raw_runtime::PyRawTransaction::proxy(self, kind, commit_on_exit)
    }

    /// Raw, unchecked TypeQL: the escape hatch. The TypeQL text is sent
    /// to TypeDB as written, and TypeDB's output comes back as it
    /// answered, in this SDK's native data types. TypeBridge adds no
    /// validation, rewriting, hydration, hooks, limits or extra
    /// queries. It runs in read, write and schema transactions and
    /// accepts any TypeQL, schema-breaking changes included. The caller
    /// owns its correctness; TypeBridge makes no promise about its
    /// effects. On a proxy connection it posts to the server's opt-in
    /// `/v3/raw` endpoint, which is off by default.

    #[pyo3(signature = (query, kind="read"))]
    fn execute_query(&self, py: Python<'_>, query: &str, kind: &str) -> PyResult<Py<PyAny>> {
        self.raw_transaction(py, kind, true)?.execute_one(py, query)
    }

    /// Open `endpoint` and fetch its capability advertisement, or validate
    /// the pinned `advertisement` without fetching; `staged_advertisement`
    /// likewise pins the `/v3/capabilities` body. `connection_limits`
    /// bounds the fetch; `answer_limits` is the ceiling every terminal on
    /// this connection runs under.
    #[staticmethod]
    #[pyo3(signature = (
        authority,
        endpoint,
        *,
        tls_mode="disabled",
        tls_root_ca=None,
        headers=None,
        advertisement=None,
        connection_limits=None,
        answer_limits=None,
        cancellation=None,
        staged_advertisement=None,
        admission_window_milliseconds=std::num::NonZeroU64::new(60_000).unwrap(),
        query_limits=None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn connect(
        py: Python<'_>,
        authority: &PyPlanAuthority,
        endpoint: String,
        tls_mode: &str,
        tls_root_ca: Option<PathBuf>,
        headers: Option<Vec<(String, String)>>,
        advertisement: Option<&Bound<'_, PyAny>>,
        connection_limits: Option<PyRef<'_, PyQueryExecutionResourceLimits>>,
        answer_limits: Option<PyRef<'_, PyQueryExecutionResourceLimits>>,
        cancellation: Option<PyRef<'_, PyQueryCancellation>>,
        staged_advertisement: Option<&Bound<'_, PyAny>>,
        admission_window_milliseconds: std::num::NonZeroU64,
        query_limits: Option<PyRef<'_, PyQueryLimits>>,
    ) -> PyResult<Self> {
        let query_limits = query_limits
            .map(|limits| limits.inner().validate_plan())
            .transpose()
            .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
        let tls = direct_tls_from_python(tls_mode, tls_root_ca)?;
        let endpoint = headers.into_iter().flatten().fold(
            ProxyEndpoint::new(endpoint).tls(tls),
            |endpoint, (name, value)| endpoint.header(name, value),
        );
        let advertisement = advertisement
            .map(|value| {
                PythonBytes::extract(value, "advertisement").map(|bytes| {
                    bytes.bounded_snapshot(MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1))
                })
            })
            .transpose()?;
        let staged_advertisement = staged_advertisement
            .map(|value| {
                PythonBytes::extract(value, "staged_advertisement").map(|bytes| {
                    bytes.bounded_snapshot(MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1))
                })
            })
            .transpose()?;
        // A proxy connection without explicit limits runs at the wire defaults,
        // not the direct ones an omitted `QueryExecutionResourceLimits` takes.
        let limits = |value: Option<PyRef<'_, PyQueryExecutionResourceLimits>>| {
            value.map_or_else(
                || QueryExecutionResourceLimits::default_for(AnswerTransport::Proxy),
                |limits| limits.inner(),
            )
        };
        let connection_limits = limits(connection_limits);
        let caller_item_limit = answer_limits.as_ref().map(|limits| limits.inner().items);
        let answer_limits = limits(answer_limits);
        let cancellation: AnswerCancellation =
            cancellation.map(|value| value.inner()).unwrap_or_default();
        let runtime = ProviderRuntimeOwner::new().map(Arc::new).map_err(|error| {
            pyo3::exceptions::PyRuntimeError::new_err(format!(
                "Failed to create Tokio runtime: {error}"
            ))
        })?;
        let connection = provider_block_on(
            py,
            runtime.as_ref(),
            ProxyQueryConnection::connect(
                &endpoint,
                advertisement,
                authority.authority(),
                connection_limits,
                answer_limits,
                &cancellation,
            ),
        )
        .map_err(py_sdk_diagnostic)?
        .with_admission_window_milliseconds(admission_window_milliseconds)
        .with_caller_item_limit(caller_item_limit);
        let connection = match query_limits {
            Some(limits) => connection
                .with_query_limits(limits)
                .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?,
            None => connection,
        };
        // A pinned staged advertisement must name the same executor and
        // reply key as the connection's advertisement.
        let connection = match staged_advertisement {
            Some(staged) => connection
                .with_staged_advertisement(staged)
                .map_err(py_sdk_diagnostic)?,
            None => connection,
        };
        Ok(Self {
            connection: Arc::new(connection),
            runtime,
            authority: authority.authority(),
        })
    }

    /// Configured request delivery window, default 60000 ms; execution is independent.
    #[getter]
    fn admission_window_milliseconds(&self) -> u64 {
        self.connection.admission_window_milliseconds()
    }

    /// Open a staged transaction over this connection. Nothing is sent
    /// until commit, which posts the staged plans as one signed
    /// `query-remote/v3` request with a fresh nonce. The
    /// `/v3/capabilities` advertisement is fetched now.
    #[pyo3(signature = (kind=None, *, cancellation=None, commit_on_exit=true))]
    fn transaction(
        &self,
        py: Python<'_>,
        kind: Option<&str>,
        cancellation: Option<PyRef<'_, PyQueryCancellation>>,
        commit_on_exit: bool,
    ) -> PyResult<PyStagedTransaction> {
        let mut options = TransactionOptions::default();
        if let Some(kind) = kind {
            options = options.with_kind(parse_kind(kind)?);
        }
        let cancellation: AnswerCancellation =
            cancellation.map(|value| value.inner()).unwrap_or_default();
        let transaction = provider_block_on(
            py,
            self.runtime.as_ref(),
            self.connection.staged_transaction(options, &cancellation),
        )
        .map_err(py_sdk_diagnostic)?;
        Ok(PyStagedTransaction::new(
            transaction,
            Arc::clone(&self.runtime),
            Some(Arc::clone(&self.authority)),
            commit_on_exit,
        ))
    }

    /// Query policy tightened by the currently pinned staged server ceiling.
    #[getter]
    fn query_limits(&self) -> PyQueryLimits {
        PyQueryLimits::from_inner(self.connection.query_limits())
    }

    /// The ceiling every terminal on this connection is constrained by.
    #[getter]
    fn answer_limits(&self) -> PyQueryExecutionResourceLimits {
        PyQueryExecutionResourceLimits::from_inner(self.connection.answer_limits())
    }
}

/// Register the built-in proxy connection.
pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyProxyConnection>()?;
    Ok(())
}
