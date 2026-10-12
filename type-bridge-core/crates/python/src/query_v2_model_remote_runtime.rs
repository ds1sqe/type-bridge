//! PyO3 seam for one-exchange remote model-oriented typed queries.
//!
//! Python retains the caller-owned async exchange callback. The context is a
//! proxy connection whose exchange the caller performs: terminals are
//! prepared and their replies finished by that shared connection, so a
//! host-owned exchange runs under the same ceiling and checks as the
//! built-in route.

use std::sync::Arc;

use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBool, PyList};
use type_bridge_contract::limits::MAX_REMOTE_ENVELOPE_BYTES;
use type_bridge_orm::_registry::DescriptorRegistry;
use type_bridge_orm::{
    AnswerCancellation, OrderHandle, PendingRemoteModelQueryV2, ProjectedQueryOrigin,
    ProxyQueryConnection, QueryExecutionDeadline, QueryExecutionResourceLimits, QueryHandle,
    QueryTarget, RowCardinality, ValidatedMatchRequest, validate_order_term_count,
};

use crate::match_runtime::{
    PyMatchBindingHandle, PyMatchFieldHandle, PyMatchOrderHandle, PyMatchQueryHandle,
    PyQueryCancellation, PyQueryExecutionResourceLimits, PyQueryInvocationBudget,
    borrow_reduce_terms, order_handles, parse_enum, py_match_error, py_match_orm_error,
    py_query_target_error, py_sdk_diagnostic, python_window, reduce_terms, validated_result_handle,
};
use crate::query_v2_runtime::{PyQueryV2Authority, PythonBytes};
use crate::validated_result_runtime::PyValidatedMatchResultHandle;

/// A caller-exchanged proxy connection plus the session's resources and
/// cancellation owner.
#[pyclass(name = "RemoteModelQueryContext", frozen)]
pub(crate) struct PyRemoteModelQueryContext {
    connection: ProxyQueryConnection,
    resources: QueryExecutionResourceLimits,
    cancellation: AnswerCancellation,
}

/// One prepared model request with an atomic one-shot reply decoder.
#[pyclass(name = "PendingRemoteModelQuery", frozen)]
pub(crate) struct PyPendingRemoteModelQuery {
    pending: PendingRemoteModelQueryV2,
    installed: Option<Arc<type_bridge_orm::InstalledRuntimeProjection>>,
    registry: Arc<DescriptorRegistry>,
    resources: QueryExecutionResourceLimits,
    deadline: QueryExecutionDeadline,
    cancellation: AnswerCancellation,
}

#[pymethods]
impl PyPendingRemoteModelQuery {
    /// Return an owned copy of the exact request bytes for caller transport.
    fn request_bytes(&self) -> PyResult<Vec<u8>> {
        if self.pending.is_closed() {
            return Err(py_sdk_diagnostic(
                type_bridge_orm::query_resource_closed_diagnostic(),
            ));
        }
        Ok(self.pending.request_bytes().to_vec())
    }

    /// Close this pending request before its reply slot is claimed.
    fn close(&self) {
        self.pending.close();
    }

    /// Whether this request was explicitly closed before claim.
    #[getter]
    fn is_closed(&self) -> bool {
        self.pending.is_closed()
    }

    /// Claim, snapshot, authenticate, and decode one response to the same
    /// opaque validated-result proof used by direct typed queries.
    fn decode_reply(
        &self,
        py: Python<'_>,
        response: &Bound<'_, PyAny>,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        // Python type validation and the bounded caller-owned snapshot happen
        // before the semantic one-shot claim, so FFI mistakes remain retryable.
        let snapshot_limit = MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1);
        let response = PythonBytes::extract(response, "response")?.bounded_snapshot(snapshot_limit);
        let pending = &self.pending;
        let cancellation = &self.cancellation;
        let deadline = self.deadline;
        let (request, result) = py
            .detach(move || {
                ProxyQueryConnection::finish(pending, &response, cancellation, deadline)
            })
            .map_err(py_query_target_error)?;
        let installed = self.installed.as_ref();
        validated_result_handle(
            installed,
            installed.map(|_| ProjectedQueryOrigin::remote_unbound()),
            request,
            result,
            Arc::clone(&self.registry),
            PyQueryInvocationBudget::from_parts(
                self.resources,
                self.deadline,
                self.cancellation.clone(),
            ),
        )
    }
}

/// Build the same remote context from the common direct/remote resource and
/// cancellation owners. `answer_limits` is the caller-exchanged connection's
/// ceiling; left out, the session's own resources are the ceiling.
#[pyfunction]
#[pyo3(signature = (authority, advertisement, resources, cancellation, *, answer_limits=None))]
pub(crate) fn query_v2_remote_model_context_with_resources(
    py: Python<'_>,
    authority: &PyQueryV2Authority,
    advertisement: &Bound<'_, PyAny>,
    resources: PyRef<'_, PyQueryExecutionResourceLimits>,
    cancellation: PyRef<'_, PyQueryCancellation>,
    answer_limits: Option<PyRef<'_, PyQueryExecutionResourceLimits>>,
) -> PyResult<PyRemoteModelQueryContext> {
    let advertisement = advertisement_snapshot(advertisement)?;
    let resources = resources.inner();
    remote_context(
        py,
        authority,
        advertisement,
        answer_limits.map_or(resources, |limits| limits.inner()),
        resources,
        cancellation.inner(),
    )
}

fn advertisement_snapshot(advertisement: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
    Ok(PythonBytes::extract(advertisement, "advertisement")?
        .bounded_snapshot(MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1)))
}

/// A caller-exchanged connection with the `answer_limits` ceiling, and the
/// session resources constrained by it.
fn remote_context(
    py: Python<'_>,
    authority: &PyQueryV2Authority,
    advertisement: Vec<u8>,
    answer_limits: QueryExecutionResourceLimits,
    resources: QueryExecutionResourceLimits,
    cancellation: AnswerCancellation,
) -> PyResult<PyRemoteModelQueryContext> {
    let authority = authority.authority();
    let connection = py
        .detach(move || {
            ProxyQueryConnection::with_exchange(advertisement, authority, answer_limits)
        })
        .map_err(py_sdk_diagnostic)?;
    let resources = QueryTarget::Proxy(&connection).constrain(resources);
    Ok(PyRemoteModelQueryContext {
        connection,
        resources,
        cancellation,
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
    prepare_terminal(py, query, context, |query| {
        let window = python_window(offset, limit)?;
        let query = query.clone();
        let order = bounded_order_handles(py, order, query.limits())?;
        let cardinality = parse_enum::<RowCardinality>(cardinality)?;
        py.detach(move || query.validate_fetch_rows(&order, window, cardinality))
            .map_err(py_match_orm_error)
    })
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
    prepare_terminal(py, query, context, |query| {
        let window = python_window(offset, limit)?;
        let include_total = python_bool(include_total, "include_total")?;
        let query = query.clone();
        let root = root.inner().clone();
        let order = bounded_order_handles(py, order, query.limits())?;
        py.detach(move || query.validate_page_by(&root, &order, window, include_total))
            .map_err(py_match_orm_error)
    })
}

/// Prepare one lossless distinct-root remote count.
#[pyfunction]
pub(crate) fn query_v2_prepare_remote_model_count(
    py: Python<'_>,
    query: &PyMatchQueryHandle,
    context: &PyRemoteModelQueryContext,
    root: &PyMatchBindingHandle,
) -> PyResult<PyPendingRemoteModelQuery> {
    prepare_terminal(py, query, context, |query| {
        let query = query.clone();
        let root = root.inner().clone();
        py.detach(move || query.validate_count_by(&root))
            .map_err(py_match_orm_error)
    })
}

/// Prepare one distinct-root remote existence query.
#[pyfunction]
pub(crate) fn query_v2_prepare_remote_model_exists(
    py: Python<'_>,
    query: &PyMatchQueryHandle,
    context: &PyRemoteModelQueryContext,
    root: &PyMatchBindingHandle,
) -> PyResult<PyPendingRemoteModelQuery> {
    prepare_terminal(py, query, context, |query| {
        let query = query.clone();
        let root = root.inner().clone();
        py.detach(move || query.validate_exists_by(&root))
            .map_err(py_match_orm_error)
    })
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
    prepare_terminal(py, query, context, |query| {
        let terms = reduce_terms(py, &reducers, &inputs)?;
        let terms = borrow_reduce_terms(&terms);
        query
            .validate_reduce_by(root.inner(), group.map(PyMatchBindingHandle::inner), &terms)
            .map_err(py_match_orm_error)
    })
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
    prepare_terminal(py, query, context, |query| {
        let terms = reduce_terms(py, &reducers, &inputs)?;
        let terms = borrow_reduce_terms(&terms);
        query
            .validate_reduce_by_field(root.inner(), group.inner(), &terms)
            .map_err(py_match_orm_error)
    })
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
    prepare_terminal(py, query, context, |query| {
        let groups = groups
            .iter()
            .map(|group| group.borrow(py).inner().clone())
            .collect::<Vec<_>>();
        let groups = groups.iter().collect::<Vec<_>>();
        let terms = reduce_terms(py, &reducers, &inputs)?;
        let terms = borrow_reduce_terms(&terms);
        query
            .validate_reduce_by_fields(root.inner(), &groups, &terms)
            .map_err(py_match_orm_error)
    })
}

/// Capture the invocation deadline, validate the terminal, and prepare it on
/// the context's connection. A closed query or an expired session refuses
/// before any argument is decoded.
fn prepare_terminal(
    py: Python<'_>,
    query: &PyMatchQueryHandle,
    context: &PyRemoteModelQueryContext,
    validate: impl FnOnce(&QueryHandle) -> PyResult<ValidatedMatchRequest>,
) -> PyResult<PyPendingRemoteModelQuery> {
    query.ensure_open()?;
    let resources = context.resources;
    let cancellation = context.cancellation.clone();
    let deadline = QueryExecutionDeadline::for_limits(resources);
    deadline.check(&cancellation).map_err(py_sdk_diagnostic)?;
    let request = validate(query.inner())?;
    let registry = query.inner().registry_arc();
    let connection = &context.connection;
    let pending = py
        .detach({
            let registry = &registry;
            let cancellation = &cancellation;
            move || connection.prepare(registry, request, resources, cancellation, deadline)
        })
        .map_err(py_query_target_error)?;
    Ok(PyPendingRemoteModelQuery {
        pending,
        installed: query.installed_projection(),
        registry,
        resources,
        deadline,
        cancellation,
    })
}

fn python_bool(value: &Bound<'_, PyAny>, argument: &'static str) -> PyResult<bool> {
    value
        .cast_exact::<PyBool>()
        .map(|value| value.is_true())
        .map_err(|_| PyTypeError::new_err(format!("argument '{argument}' must be bool")))
}

fn bounded_order_handles(
    py: Python<'_>,
    values: &Bound<'_, PyAny>,
    limits: type_bridge_contract::limits::QueryLimits,
) -> PyResult<Vec<OrderHandle>> {
    let values = values.cast::<PyList>()?;
    validate_order_term_count(values.len(), limits.structural).map_err(py_match_error)?;
    let handles = values
        .iter()
        .map(|value| {
            value
                .extract::<Py<PyMatchOrderHandle>>()
                .map_err(PyErr::from)
        })
        .collect::<PyResult<Vec<_>>>()?;
    Ok(order_handles(py, &handles))
}

/// Register the additive remote model-query native seam.
pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyRemoteModelQueryContext>()?;
    module.add_class::<PyPendingRemoteModelQuery>()?;
    module.add_function(wrap_pyfunction!(
        query_v2_remote_model_context_with_resources,
        module
    )?)?;
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

    use type_bridge_orm::Window;

    use super::{python_bool, python_window};

    #[test]
    fn hostile_native_windows_are_host_marshalling_errors() {
        Python::initialize();
        Python::attach(|py| {
            let one = py.eval(ffi::c_str!("1"), None, None).expect("integer");
            assert_eq!(
                python_window(&one, &one).expect("exact integers"),
                Window {
                    offset: 1,
                    limit: 1
                }
            );

            for (expression, overflow) in [
                (ffi::c_str!("True"), false),
                (ffi::c_str!("1.0"), false),
                (ffi::c_str!("-1"), true),
                (ffi::c_str!("1 << 65"), true),
            ] {
                let value = py.eval(expression, None, None).expect("hostile value");
                let error = python_window(&one, &value).expect_err("value must be rejected");
                if overflow {
                    assert!(error.is_instance_of::<pyo3::exceptions::PyOverflowError>(py));
                } else {
                    assert!(error.is_instance_of::<pyo3::exceptions::PyTypeError>(py));
                }
            }
        });
    }

    #[test]
    fn hostile_native_include_total_requires_exact_bool() {
        Python::initialize();
        Python::attach(|py| {
            let true_value = py.eval(ffi::c_str!("True"), None, None).expect("bool");
            assert!(python_bool(&true_value, "include_total").expect("exact bool"));

            let one = py.eval(ffi::c_str!("1"), None, None).expect("integer");
            let error = python_bool(&one, "include_total").expect_err("integer is not bool");
            assert!(error.is_instance_of::<pyo3::exceptions::PyTypeError>(py));
        });
    }
}
