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
    AnswerCancellation, ProviderRuntimeOwner, ProxyEndpoint, ProxyQueryConnection,
    QueryExecutionResourceLimits,
};

use crate::match_runtime::{
    PyQueryCancellation, PyQueryExecutionResourceLimits, py_sdk_diagnostic,
};
use crate::orm_runtime::provider_block_on;
use crate::query_v2_runtime::{PyQueryV2Authority, PythonBytes};
use crate::runtime_projection::direct_tls_from_python;

/// One connection to a TypeBridge server reached over its HTTP routes.
#[pyclass(name = "ProxyConnection", frozen)]
pub(crate) struct PyProxyConnection {
    connection: ProxyQueryConnection,
    runtime: Arc<ProviderRuntimeOwner>,
}

impl PyProxyConnection {
    pub(crate) const fn connection(&self) -> &ProxyQueryConnection {
        &self.connection
    }

    pub(crate) fn runtime(&self) -> &ProviderRuntimeOwner {
        self.runtime.as_ref()
    }
}

#[pymethods]
impl PyProxyConnection {
    /// Open `endpoint` and fetch its capability advertisement, or validate
    /// the pinned `advertisement` without fetching. `connection_limits`
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
    ))]
    #[allow(clippy::too_many_arguments)]
    fn connect(
        py: Python<'_>,
        authority: &PyQueryV2Authority,
        endpoint: String,
        tls_mode: &str,
        tls_root_ca: Option<PathBuf>,
        headers: Option<Vec<(String, String)>>,
        advertisement: Option<&Bound<'_, PyAny>>,
        connection_limits: Option<PyRef<'_, PyQueryExecutionResourceLimits>>,
        answer_limits: Option<PyRef<'_, PyQueryExecutionResourceLimits>>,
        cancellation: Option<PyRef<'_, PyQueryCancellation>>,
    ) -> PyResult<Self> {
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
        let limits = |value: Option<PyRef<'_, PyQueryExecutionResourceLimits>>| {
            value.map_or_else(QueryExecutionResourceLimits::default, |limits| {
                limits.inner()
            })
        };
        let connection_limits = limits(connection_limits);
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
        .map_err(py_sdk_diagnostic)?;
        Ok(Self {
            connection,
            runtime,
        })
    }

    /// The ceiling every terminal on this connection is constrained by.
    #[getter]
    const fn answer_limits(&self) -> PyQueryExecutionResourceLimits {
        PyQueryExecutionResourceLimits::from_inner(self.connection.answer_limits())
    }
}

/// Register the built-in proxy connection.
pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyProxyConnection>()?;
    Ok(())
}
