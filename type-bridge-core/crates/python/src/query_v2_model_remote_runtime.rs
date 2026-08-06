//! PyO3 seam for one-exchange remote model-oriented typed queries.
//!
//! Python retains the caller-owned async exchange callback. This module owns
//! only immutable context snapshots, native terminal preparation, and the
//! one-shot authenticated response-to-result-proof transition.

use std::sync::Arc;

use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBool, PyList};
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::limits::MAX_REMOTE_ENVELOPE_BYTES;
use type_bridge_contract::query_remote::{
    RemoteCapabilities, checked_remote_deadline, checked_remote_limit,
};
use type_bridge_contract::query_remote_v2::RemoteLimitsV2;
use type_bridge_orm::_registry::DescriptorRegistry;
use type_bridge_orm::{
    OrderHandle, PendingRemoteModelQueryV2, RemoteModelQueryV2Error, Window,
    prepare_remote_model_query_v2, validate_public_order_term_count,
};

use crate::match_runtime::{
    PyMatchBindingHandle, PyMatchFieldHandle, PyMatchOrderHandle, PyMatchQueryHandle,
    borrow_reduce_terms, order_handles, parse_cardinality, py_match_error, py_match_orm_error,
    reduce_terms,
};
use crate::query_v2_runtime::{
    PyQueryV2Authority, PythonBytes, python_limit, python_optional_limit, value_error,
};
use crate::validated_result_runtime::PyValidatedMatchResultHandle;

/// Immutable native authority, advertisement, and limit snapshot.
#[pyclass(name = "RemoteModelQueryContext", frozen)]
pub(crate) struct PyRemoteModelQueryContext {
    advertisement: Vec<u8>,
    authority: Arc<type_bridge_orm::query_v2_prepared::QueryAuthority>,
    limits: RemoteLimitsV2,
}

/// One prepared model request with an atomic one-shot reply decoder.
#[pyclass(name = "PendingRemoteModelQuery", frozen)]
pub(crate) struct PyPendingRemoteModelQuery {
    pending: PendingRemoteModelQueryV2,
}

#[pymethods]
impl PyPendingRemoteModelQuery {
    /// Return an owned copy of the exact request bytes for caller transport.
    fn request_bytes(&self) -> Vec<u8> {
        self.pending.request_bytes().to_vec()
    }

    /// Claim, snapshot, authenticate, and decode one response to the same
    /// opaque validated-result proof used by direct typed queries.
    fn decode_reply(
        &self,
        py: Python<'_>,
        response: &Bound<'_, PyAny>,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let claimed = self.pending.claim_reply().map_err(remote_model_error)?;
        let response = PythonBytes::extract(response, "response")?
            .bounded_snapshot(claimed.response_snapshot_limit());
        let (request, result, registry) = py
            .allow_threads(move || claimed.decode(&response))
            .map_err(remote_model_error)?;
        Ok(PyValidatedMatchResultHandle::new(request, result, registry))
    }
}

/// Snapshot and validate the non-I/O context consumed by every remote
/// model-query terminal.
#[pyfunction]
#[pyo3(signature = (
    authority,
    advertisement,
    max_items,
    max_bytes,
    max_collection_members,
    max_graph_nodes,
    max_attribute_values,
    max_role_players,
    deadline_ms=None
))]
#[expect(clippy::too_many_arguments, reason = "flat explicit limit contract")]
pub(crate) fn query_v2_remote_model_context(
    py: Python<'_>,
    authority: &PyQueryV2Authority,
    advertisement: &Bound<'_, PyAny>,
    max_items: &Bound<'_, PyAny>,
    max_bytes: &Bound<'_, PyAny>,
    max_collection_members: &Bound<'_, PyAny>,
    max_graph_nodes: &Bound<'_, PyAny>,
    max_attribute_values: &Bound<'_, PyAny>,
    max_role_players: &Bound<'_, PyAny>,
    deadline_ms: Option<&Bound<'_, PyAny>>,
) -> PyResult<PyRemoteModelQueryContext> {
    let advertisement = PythonBytes::extract(advertisement, "advertisement")?
        .bounded_snapshot(MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1));
    let limits = remote_limits_v2(
        max_items,
        max_bytes,
        max_collection_members,
        max_graph_nodes,
        max_attribute_values,
        max_role_players,
        deadline_ms,
    )?;
    py.allow_threads({
        let advertisement = &advertisement;
        move || RemoteCapabilities::decode(advertisement)
    })
    .map_err(|diagnostic| value_error(&diagnostic))?;
    Ok(PyRemoteModelQueryContext {
        advertisement,
        authority: authority.authority(),
        limits,
    })
}

/// Prepare a selected-row or exactly-one remote model query.
#[pyfunction]
#[pyo3(signature = (query, context, order, offset, limit, cardinality))]
pub(crate) fn query_v2_prepare_remote_model_rows(
    py: Python<'_>,
    query: &PyMatchQueryHandle,
    context: &PyRemoteModelQueryContext,
    order: &Bound<'_, PyAny>,
    offset: &Bound<'_, PyAny>,
    limit: &Bound<'_, PyAny>,
    cardinality: &str,
) -> PyResult<PyPendingRemoteModelQuery> {
    let offset = python_unsigned(offset)?;
    let limit = python_unsigned(limit)?;
    let query = query.inner().clone();
    let registry = query.registry_arc();
    let order = bounded_order_handles(py, order)?;
    let cardinality = parse_cardinality(cardinality)?;
    let request = py
        .allow_threads(move || {
            query.validate_fetch_rows(&order, Window { offset, limit }, cardinality)
        })
        .map_err(py_match_orm_error)?;
    prepare_pending(py, context, registry, request)
}

/// Prepare one distinct-root remote page.
#[pyfunction]
#[pyo3(signature = (query, context, root, order, offset, limit, include_total))]
#[expect(
    clippy::too_many_arguments,
    reason = "terminal mirrors released grammar"
)]
pub(crate) fn query_v2_prepare_remote_model_page(
    py: Python<'_>,
    query: &PyMatchQueryHandle,
    context: &PyRemoteModelQueryContext,
    root: &PyMatchBindingHandle,
    order: &Bound<'_, PyAny>,
    offset: &Bound<'_, PyAny>,
    limit: &Bound<'_, PyAny>,
    include_total: &Bound<'_, PyAny>,
) -> PyResult<PyPendingRemoteModelQuery> {
    let offset = python_unsigned(offset)?;
    let limit = python_unsigned(limit)?;
    let include_total = python_bool(include_total, "include_total")?;
    let query = query.inner().clone();
    let registry = query.registry_arc();
    let root = root.inner().clone();
    let order = bounded_order_handles(py, order)?;
    let request = py
        .allow_threads(move || {
            query.validate_page_by(&root, &order, Window { offset, limit }, include_total)
        })
        .map_err(py_match_orm_error)?;
    prepare_pending(py, context, registry, request)
}

/// Prepare one lossless distinct-root remote count.
#[pyfunction]
pub(crate) fn query_v2_prepare_remote_model_count(
    py: Python<'_>,
    query: &PyMatchQueryHandle,
    context: &PyRemoteModelQueryContext,
    root: &PyMatchBindingHandle,
) -> PyResult<PyPendingRemoteModelQuery> {
    let query = query.inner().clone();
    let registry = query.registry_arc();
    let root = root.inner().clone();
    let request = py
        .allow_threads(move || query.validate_count_by(&root))
        .map_err(py_match_orm_error)?;
    prepare_pending(py, context, registry, request)
}

/// Prepare one distinct-root remote existence query.
#[pyfunction]
pub(crate) fn query_v2_prepare_remote_model_exists(
    py: Python<'_>,
    query: &PyMatchQueryHandle,
    context: &PyRemoteModelQueryContext,
    root: &PyMatchBindingHandle,
) -> PyResult<PyPendingRemoteModelQuery> {
    let query = query.inner().clone();
    let registry = query.registry_arc();
    let root = root.inner().clone();
    let request = py
        .allow_threads(move || query.validate_exists_by(&root))
        .map_err(py_match_orm_error)?;
    prepare_pending(py, context, registry, request)
}

/// Prepare one typed ungrouped or grouped reduction over a distinct root.
#[pyfunction]
#[pyo3(signature = (query, context, root, group, reducers, inputs))]
pub(crate) fn query_v2_prepare_remote_model_reduce(
    py: Python<'_>,
    query: &PyMatchQueryHandle,
    context: &PyRemoteModelQueryContext,
    root: &PyMatchBindingHandle,
    group: Option<&PyMatchBindingHandle>,
    reducers: Vec<String>,
    inputs: Vec<Option<Py<PyMatchFieldHandle>>>,
) -> PyResult<PyPendingRemoteModelQuery> {
    let terms = reduce_terms(py, &reducers, &inputs)?;
    let terms = borrow_reduce_terms(&terms);
    let request = query
        .inner()
        .validate_reduce_by(root.inner(), group.map(PyMatchBindingHandle::inner), &terms)
        .map_err(py_match_orm_error)?;
    prepare_pending(py, context, query.inner().registry_arc(), request)
}

/// Prepare one typed reduction grouped by a projected owned field.
#[pyfunction]
#[pyo3(signature = (query, context, root, group, reducers, inputs))]
pub(crate) fn query_v2_prepare_remote_model_reduce_by_field(
    py: Python<'_>,
    query: &PyMatchQueryHandle,
    context: &PyRemoteModelQueryContext,
    root: &PyMatchBindingHandle,
    group: &PyMatchFieldHandle,
    reducers: Vec<String>,
    inputs: Vec<Option<Py<PyMatchFieldHandle>>>,
) -> PyResult<PyPendingRemoteModelQuery> {
    let terms = reduce_terms(py, &reducers, &inputs)?;
    let terms = borrow_reduce_terms(&terms);
    let request = query
        .inner()
        .validate_reduce_by_field(root.inner(), group.inner(), &terms)
        .map_err(py_match_orm_error)?;
    prepare_pending(py, context, query.inner().registry_arc(), request)
}

/// Prepare one typed reduction grouped by an ordered tuple of projected owned fields.
#[pyfunction]
#[pyo3(signature = (query, context, root, groups, reducers, inputs))]
pub(crate) fn query_v2_prepare_remote_model_reduce_by_fields(
    py: Python<'_>,
    query: &PyMatchQueryHandle,
    context: &PyRemoteModelQueryContext,
    root: &PyMatchBindingHandle,
    groups: Vec<Py<PyMatchFieldHandle>>,
    reducers: Vec<String>,
    inputs: Vec<Option<Py<PyMatchFieldHandle>>>,
) -> PyResult<PyPendingRemoteModelQuery> {
    let groups = groups
        .iter()
        .map(|group| group.borrow(py).inner().clone())
        .collect::<Vec<_>>();
    let groups = groups.iter().collect::<Vec<_>>();
    let terms = reduce_terms(py, &reducers, &inputs)?;
    let terms = borrow_reduce_terms(&terms);
    let request = query
        .inner()
        .validate_reduce_by_fields(root.inner(), &groups, &terms)
        .map_err(py_match_orm_error)?;
    prepare_pending(py, context, query.inner().registry_arc(), request)
}

fn prepare_pending(
    py: Python<'_>,
    context: &PyRemoteModelQueryContext,
    registry: Arc<DescriptorRegistry>,
    request: type_bridge_orm::ValidatedMatchRequest,
) -> PyResult<PyPendingRemoteModelQuery> {
    let authority = Arc::clone(&context.authority);
    let advertisement = context.advertisement.clone();
    let limits = context.limits;
    let pending = py
        .allow_threads(move || {
            prepare_remote_model_query_v2(&authority, &registry, request, &advertisement, limits)
        })
        .map_err(remote_model_error)?;
    Ok(PyPendingRemoteModelQuery { pending })
}

fn remote_limits_v2(
    max_items: &Bound<'_, PyAny>,
    max_bytes: &Bound<'_, PyAny>,
    max_collection_members: &Bound<'_, PyAny>,
    max_graph_nodes: &Bound<'_, PyAny>,
    max_attribute_values: &Bound<'_, PyAny>,
    max_role_players: &Bound<'_, PyAny>,
    deadline_ms: Option<&Bound<'_, PyAny>>,
) -> PyResult<RemoteLimitsV2> {
    let build = || -> Result<RemoteLimitsV2, Diagnostic> {
        Ok(RemoteLimitsV2 {
            deadline_ms: checked_remote_deadline(python_optional_limit(deadline_ms)?)?,
            max_bytes: checked_remote_limit(python_limit(max_bytes)?)?,
            max_items: checked_remote_limit(python_limit(max_items)?)?,
            max_collection_members: checked_remote_limit(python_limit(max_collection_members)?)?,
            max_graph_nodes: checked_remote_limit(python_limit(max_graph_nodes)?)?,
            max_attribute_values: checked_remote_limit(python_limit(max_attribute_values)?)?,
            max_role_players: checked_remote_limit(python_limit(max_role_players)?)?,
        })
    };
    build().map_err(|diagnostic| value_error(&diagnostic))
}

fn remote_model_error(error: RemoteModelQueryV2Error) -> PyErr {
    match error {
        RemoteModelQueryV2Error::Diagnostic(diagnostic) => value_error(&diagnostic),
        RemoteModelQueryV2Error::Match(error) => py_match_error(error),
    }
}

fn python_unsigned(value: &Bound<'_, PyAny>) -> PyResult<u64> {
    checked_remote_limit(python_limit(value).map_err(|diagnostic| value_error(&diagnostic))?)
        .map_err(|diagnostic| value_error(&diagnostic))
}

fn python_bool(value: &Bound<'_, PyAny>, argument: &'static str) -> PyResult<bool> {
    value
        .downcast_exact::<PyBool>()
        .map(|value| value.is_true())
        .map_err(|_| PyTypeError::new_err(format!("argument '{argument}' must be bool")))
}

fn bounded_order_handles(py: Python<'_>, values: &Bound<'_, PyAny>) -> PyResult<Vec<OrderHandle>> {
    let values = values.downcast::<PyList>()?;
    validate_public_order_term_count(values.len()).map_err(py_match_error)?;
    let handles = values
        .iter()
        .map(|value| value.extract::<Py<PyMatchOrderHandle>>())
        .collect::<PyResult<Vec<_>>>()?;
    Ok(order_handles(py, &handles))
}

/// Register the additive remote model-query native seam.
pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyRemoteModelQueryContext>()?;
    module.add_class::<PyPendingRemoteModelQuery>()?;
    module.add_function(wrap_pyfunction!(query_v2_remote_model_context, module)?)?;
    module.add_function(wrap_pyfunction!(
        query_v2_prepare_remote_model_rows,
        module
    )?)?;
    module.add_function(wrap_pyfunction!(
        query_v2_prepare_remote_model_page,
        module
    )?)?;
    module.add_function(wrap_pyfunction!(
        query_v2_prepare_remote_model_count,
        module
    )?)?;
    module.add_function(wrap_pyfunction!(
        query_v2_prepare_remote_model_exists,
        module
    )?)?;
    module.add_function(wrap_pyfunction!(
        query_v2_prepare_remote_model_reduce,
        module
    )?)?;
    module.add_function(wrap_pyfunction!(
        query_v2_prepare_remote_model_reduce_by_field,
        module
    )?)?;
    module.add_function(wrap_pyfunction!(
        query_v2_prepare_remote_model_reduce_by_fields,
        module
    )?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use pyo3::ffi;
    use pyo3::prelude::*;

    use super::{python_bool, python_unsigned};

    #[test]
    fn hostile_native_windows_require_exact_non_boolean_u64_values() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let one = py.eval(ffi::c_str!("1"), None, None).expect("integer");
            assert_eq!(python_unsigned(&one).expect("exact integer"), 1);

            for expression in [
                ffi::c_str!("True"),
                ffi::c_str!("1.0"),
                ffi::c_str!("-1"),
                ffi::c_str!("1 << 65"),
            ] {
                let value = py.eval(expression, None, None).expect("hostile value");
                let error = python_unsigned(&value).expect_err("value must be rejected");
                let code = error
                    .value(py)
                    .getattr("code")
                    .expect("structured code")
                    .extract::<String>()
                    .expect("string code");
                assert_eq!(code, "query_remote_limit_invalid");
            }
        });
    }

    #[test]
    fn hostile_native_include_total_requires_exact_bool() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let true_value = py.eval(ffi::c_str!("True"), None, None).expect("bool");
            assert!(python_bool(&true_value, "include_total").expect("exact bool"));

            let one = py.eval(ffi::c_str!("1"), None, None).expect("integer");
            let error = python_bool(&one, "include_total").expect_err("integer is not bool");
            assert!(error.is_instance_of::<pyo3::exceptions::PyTypeError>(py));
        });
    }
}
