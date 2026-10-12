//! Opaque PyO3 seam for the canonical typed-match handle contract.
//!
//! These classes deliberately expose construction transitions and read-only
//! canonical diagnostics only. Python never owns a match plan, binding map,
//! validated request, provider row, TypeQL string, or invocation token here.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pyo3::exceptions::{PyException, PyOverflowError, PyRuntimeError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyAny, PyDict, PyInt, PyString, PyType};
use pythonize::pythonize;
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::id::{FunctionId, TypeId};
use type_bridge_contract::limits::QueryLimits;
use type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic;
use type_bridge_orm::_registry::DescriptorRegistry;
use type_bridge_orm::{
    AnswerCancellation, BindingHandle, ComparisonOp, FieldHandle, FunctionArgumentHandle,
    FunctionCallHandle, FunctionHandle, FunctionValueHandle, InstalledRuntimeProjection,
    MatchError, MissingOrder, OrderHandle, OrmError, PredicateHandle, ProjectedAttributeValue,
    ProjectedQueryOrigin, ProviderRuntimeOwner, QueryExecutionDeadline,
    QueryExecutionResourceLimits, QueryHandle, QueryTarget, QueryTargetError, Reduction,
    RemoteModelQueryV2Error, RoleHandle, RowCardinality, SelectionHandle, SessionHandle,
    ShapeHandle, SortDirection, UnknownEnumValue, UnvalidatedMatchRequest, ValidatedMatchRequest,
    Window, lower_match_error, lower_remote_query_diagnostic, query_resource_closed_diagnostic,
};

use crate::orm_runtime::{
    PyDescriptorRegistry, PyDynamicValue, PyRustDatabase, PyRustTransactionContext,
    provider_block_on,
};
use crate::proxy_connection_runtime::PyProxyConnection;
use crate::validated_result_runtime::PyValidatedMatchResultHandle;

pyo3::create_exception!(
    type_bridge_core,
    MatchRequestError,
    PyException,
    "Structured canonical match-request validation or lineage failure."
);

#[pyclass(name = "MatchSessionHandle", frozen, from_py_object)]
#[derive(Clone)]
pub(crate) struct PyMatchSessionHandle {
    inner: SessionHandle,
    installed: Option<Arc<InstalledRuntimeProjection>>,
    resources: QueryExecutionResourceLimits,
    cancellation: AnswerCancellation,
    closed: Arc<AtomicBool>,
}

#[pyclass(name = "MatchBindingHandle", frozen, from_py_object)]
#[derive(Clone)]
pub(crate) struct PyMatchBindingHandle {
    inner: BindingHandle,
}

#[pyclass(name = "MatchFieldHandle", frozen, from_py_object)]
#[derive(Clone)]
pub(crate) struct PyMatchFieldHandle {
    inner: FieldHandle,
}

#[pyclass(name = "MatchRoleHandle", frozen, from_py_object)]
#[derive(Clone)]
struct PyMatchRoleHandle {
    inner: RoleHandle,
}

#[pyclass(name = "MatchPredicateHandle", frozen, from_py_object)]
#[derive(Clone)]
struct PyMatchPredicateHandle {
    inner: PredicateHandle,
}

#[pyclass(name = "MatchOrderHandle", frozen, from_py_object)]
#[derive(Clone)]
pub(crate) struct PyMatchOrderHandle {
    inner: OrderHandle,
}

#[pyclass(name = "MatchSelectionHandle", frozen, from_py_object)]
#[derive(Clone)]
struct PyMatchSelectionHandle {
    inner: SelectionHandle,
}

#[pyclass(name = "MatchShapeHandle", frozen, from_py_object)]
#[derive(Clone)]
struct PyMatchShapeHandle {
    inner: ShapeHandle,
}

#[pyclass(name = "MatchQueryHandle", frozen, from_py_object)]
pub(crate) struct PyMatchQueryHandle {
    inner: QueryHandle,
    installed: Option<Arc<InstalledRuntimeProjection>>,
    resources: QueryExecutionResourceLimits,
    cancellation: AnswerCancellation,
    session_closed: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
}

impl Clone for PyMatchQueryHandle {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            installed: self.installed.as_ref().map(Arc::clone),
            resources: self.resources,
            cancellation: self.cancellation.clone(),
            session_closed: Arc::clone(&self.session_closed),
            closed: Arc::new(AtomicBool::new(self.closed.load(Ordering::Acquire))),
        }
    }
}

#[pyclass(name = "MatchFunctionHandle", frozen)]
struct PyMatchFunctionHandle {
    inner: FunctionHandle,
}

#[pyclass(name = "MatchFunctionValueHandle", frozen)]
struct PyMatchFunctionValueHandle {
    inner: FunctionValueHandle,
}

#[pyclass(name = "MatchFunctionArgumentHandle", frozen)]
struct PyMatchFunctionArgumentHandle {
    inner: FunctionArgumentHandle,
}

#[pyclass(name = "MatchFunctionCallHandle", frozen)]
struct PyMatchFunctionCallHandle {
    inner: FunctionCallHandle,
}

pub(crate) struct PyQueryInvocationBudget {
    resources: QueryExecutionResourceLimits,
    deadline: QueryExecutionDeadline,
    cancellation: AnswerCancellation,
}

impl PyQueryInvocationBudget {
    pub(crate) const fn from_parts(
        resources: QueryExecutionResourceLimits,
        deadline: QueryExecutionDeadline,
        cancellation: AnswerCancellation,
    ) -> Self {
        Self {
            resources,
            deadline,
            cancellation,
        }
    }
}

/// Where one generated terminal runs, as Python passes it.
#[derive(Clone, Copy)]
enum TerminalTarget<'a> {
    Database(&'a PyRustDatabase),
    Transaction(&'a PyRustTransactionContext),
    Proxy(&'a PyProxyConnection),
}

/// Canonical cross-binding generated-query execution budgets.
#[pyclass(name = "QueryExecutionResourceLimits", frozen, from_py_object)]
#[derive(Clone, Copy)]
pub(crate) struct PyQueryExecutionResourceLimits {
    inner: QueryExecutionResourceLimits,
}

impl PyQueryExecutionResourceLimits {
    pub(crate) const fn inner(&self) -> QueryExecutionResourceLimits {
        self.inner
    }

    pub(crate) const fn from_inner(inner: QueryExecutionResourceLimits) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyQueryExecutionResourceLimits {
    /// Construct an answer policy. An omitted dimension keeps its default;
    /// a given one may tighten or raise it, and zero is valid. A value above
    /// a dimension's supported maximum is refused by name, never clamped.
    #[new]
    #[pyo3(signature = (
        timeout_milliseconds=None,
        items=None,
        bytes=None,
        graph_nodes=None,
        attribute_values=None,
        collection_members=None,
        role_players=None,
        statements=None
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        timeout_milliseconds: Option<&Bound<'_, PyAny>>,
        items: Option<&Bound<'_, PyAny>>,
        bytes: Option<&Bound<'_, PyAny>>,
        graph_nodes: Option<&Bound<'_, PyAny>>,
        attribute_values: Option<&Bound<'_, PyAny>>,
        collection_members: Option<&Bound<'_, PyAny>>,
        role_players: Option<&Bound<'_, PyAny>>,
        statements: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let defaults = QueryExecutionResourceLimits::default();
        let statements = python_optional_resource_limit(
            statements,
            u64::from(defaults.statements),
            "statements",
        )?;
        let inner = QueryExecutionResourceLimits {
            timeout_milliseconds: python_optional_resource_limit(
                timeout_milliseconds,
                defaults.timeout_milliseconds,
                "timeout_milliseconds",
            )?,
            items: python_optional_resource_limit(items, defaults.items, "items")?,
            bytes: python_optional_resource_limit(bytes, defaults.bytes, "bytes")?,
            graph_nodes: python_optional_resource_limit(
                graph_nodes,
                defaults.graph_nodes,
                "graph_nodes",
            )?,
            attribute_values: python_optional_resource_limit(
                attribute_values,
                defaults.attribute_values,
                "attribute_values",
            )?,
            collection_members: python_optional_resource_limit(
                collection_members,
                defaults.collection_members,
                "collection_members",
            )?,
            role_players: python_optional_resource_limit(
                role_players,
                defaults.role_players,
                "role_players",
            )?,
            statements: u32::try_from(statements).unwrap_or(u32::MAX),
        }
        .validate()
        .map_err(|refusal| PyValueError::new_err(refusal.to_string()))?;
        Ok(Self { inner })
    }

    #[getter]
    const fn timeout_milliseconds(&self) -> u64 {
        self.inner.timeout_milliseconds
    }

    #[getter]
    const fn items(&self) -> u64 {
        self.inner.items
    }

    #[getter]
    const fn bytes(&self) -> u64 {
        self.inner.bytes
    }

    #[getter]
    const fn graph_nodes(&self) -> u64 {
        self.inner.graph_nodes
    }

    #[getter]
    const fn attribute_values(&self) -> u64 {
        self.inner.attribute_values
    }

    #[getter]
    const fn collection_members(&self) -> u64 {
        self.inner.collection_members
    }

    #[getter]
    const fn role_players(&self) -> u64 {
        self.inner.role_players
    }

    #[getter]
    const fn statements(&self) -> u32 {
        self.inner.statements
    }
}

/// Caller-owned cooperative cancellation shared by every query stage.
#[pyclass(name = "QueryCancellation", frozen, from_py_object)]
#[derive(Clone)]
pub(crate) struct PyQueryCancellation {
    inner: AnswerCancellation,
}

impl PyQueryCancellation {
    pub(crate) fn inner(&self) -> AnswerCancellation {
        self.inner.clone()
    }
}

#[pymethods]
impl PyQueryCancellation {
    #[new]
    pub(crate) fn new() -> Self {
        Self {
            inner: AnswerCancellation::default(),
        }
    }

    pub(crate) fn cancel(&self) {
        self.inner.cancel();
    }

    #[getter]
    fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
}

/// Query limits one generated session validates, encodes, and executes
/// under.
///
/// Construct with keyword overrides of the canonical defaults, named exactly
/// as [`type_bridge_contract::limits::QueryLimits::FIELDS`] lists them. Rust
/// admits the values; an unknown name or an unsupported value is refused
/// here, before any session exists.
#[pyclass(name = "QueryLimits", frozen, from_py_object)]
#[derive(Clone)]
pub(crate) struct PyQueryLimits {
    inner: QueryLimits,
}

impl PyQueryLimits {
    pub(crate) const fn inner(&self) -> QueryLimits {
        self.inner
    }
}

#[pymethods]
impl PyQueryLimits {
    #[new]
    #[pyo3(signature = (**overrides))]
    fn new(overrides: Option<&Bound<'_, pyo3::types::PyDict>>) -> PyResult<Self> {
        let mut named = Vec::new();
        if let Some(overrides) = overrides {
            for (name, value) in overrides.iter() {
                let name = name
                    .cast_exact::<pyo3::types::PyString>()
                    .map_err(|_| PyTypeError::new_err("query limit names must be exact str"))?
                    .to_str()?
                    .to_owned();
                let value = python_optional_resource_limit(Some(&value), 0, &name)?;
                named.push((name, value));
            }
        }
        QueryLimits::from_overrides(named.iter().map(|(name, value)| (name.as_str(), *value)))
            .map(|inner| Self { inner })
            .map_err(|error| PyValueError::new_err(error.to_string()))
    }

    /// Return the stable names of every query limit.
    #[staticmethod]
    fn names() -> Vec<&'static str> {
        QueryLimits::FIELDS.to_vec()
    }

    /// Return the effective value of one named limit.
    fn get(&self, name: &str) -> PyResult<u64> {
        self.inner
            .get(name)
            .map(|value| u64::try_from(value).unwrap_or(u64::MAX))
            .ok_or_else(|| PyValueError::new_err(format!("`{name}` is not a known query limit")))
    }

    fn __repr__(&self) -> String {
        let overrides = QueryLimits::FIELDS
            .iter()
            .filter_map(|name| {
                let value = self.inner.get(name)?;
                (Some(value) != QueryLimits::CANONICAL.get(name)).then(|| format!("{name}={value}"))
            })
            .collect::<Vec<_>>();
        format!("QueryLimits({})", overrides.join(", "))
    }
}

fn python_optional_resource_limit(
    value: Option<&Bound<'_, PyAny>>,
    default: u64,
    name: &str,
) -> PyResult<u64> {
    let Some(value) = value else {
        return Ok(default);
    };
    let value = value
        .cast_exact::<PyInt>()
        .map_err(|_| PyTypeError::new_err(format!("{name} must be an exact non-negative int")))?
        .extract::<i128>()
        .map_err(|_| PyValueError::new_err(format!("{name} must be a non-negative integer")))?;
    u64::try_from(value)
        .map_err(|_| PyValueError::new_err(format!("{name} must be a non-negative integer")))
}

impl PyMatchBindingHandle {
    pub(crate) const fn inner(&self) -> &BindingHandle {
        &self.inner
    }
}

impl PyMatchFieldHandle {
    pub(crate) const fn inner(&self) -> &FieldHandle {
        &self.inner
    }
}

impl PyMatchQueryHandle {
    pub(crate) const fn inner(&self) -> &QueryHandle {
        &self.inner
    }

    pub(crate) fn installed_projection(&self) -> Option<Arc<InstalledRuntimeProjection>> {
        self.installed.as_ref().map(Arc::clone)
    }

    pub(crate) fn begin_invocation(&self) -> PyResult<PyQueryInvocationBudget> {
        self.ensure_open()?;
        let deadline = QueryExecutionDeadline::for_limits(self.resources);
        deadline
            .check(&self.cancellation)
            .map_err(py_sdk_diagnostic)?;
        Ok(PyQueryInvocationBudget {
            resources: self.resources,
            deadline,
            cancellation: self.cancellation.clone(),
        })
    }

    fn derived(&self, inner: QueryHandle) -> Self {
        Self {
            inner,
            installed: self.installed.as_ref().map(Arc::clone),
            resources: self.resources,
            cancellation: self.cancellation.clone(),
            session_closed: Arc::clone(&self.session_closed),
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn ensure_open(&self) -> PyResult<()> {
        if self.closed.load(Ordering::Acquire) || self.session_closed.load(Ordering::Acquire) {
            return Err(py_sdk_diagnostic(query_resource_closed_diagnostic()));
        }
        Ok(())
    }
}

impl PyMatchSessionHandle {
    fn ensure_open(&self) -> PyResult<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(py_sdk_diagnostic(query_resource_closed_diagnostic()));
        }
        Ok(())
    }

    pub(crate) fn from_registry(registry: Arc<DescriptorRegistry>) -> Self {
        Self::from_registry_with_resources(
            registry,
            QueryExecutionResourceLimits::default(),
            AnswerCancellation::default(),
        )
    }

    pub(crate) fn from_registry_with_resources(
        registry: Arc<DescriptorRegistry>,
        resources: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Self {
        Self {
            inner: SessionHandle::with_limits(registry, QueryLimits::CANONICAL),
            installed: None,
            resources: resources.effective(),
            cancellation,
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn from_installed(
        installed: Arc<InstalledRuntimeProjection>,
        registry: Arc<DescriptorRegistry>,
        resources: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
        query_limits: QueryLimits,
    ) -> Self {
        Self {
            inner: SessionHandle::with_limits(registry, query_limits),
            installed: Some(installed),
            resources: resources.effective(),
            cancellation,
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    #[cfg(test)]
    pub(crate) fn projected_companion_enabled(&self) -> bool {
        self.installed
            .as_deref()
            .is_some_and(supports_projected_query_companion)
    }
}

fn supports_projected_query_companion(installed: &InstalledRuntimeProjection) -> bool {
    installed.projection().models().values().any(|model| {
        model
            .query_tokens()
            .fields()
            .values()
            .any(|field| !field.multiplicity().collection_mode().is_unordered())
            || model
                .query_tokens()
                .roles()
                .values()
                .any(|role| !role.multiplicity().collection_mode().is_unordered())
    })
}

#[pymethods]
impl PyMatchSessionHandle {
    #[new]
    fn new(registry: PyRef<'_, PyDescriptorRegistry>) -> Self {
        Self::from_registry(registry.registry_arc())
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    #[getter]
    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn exact(&self, type_name: &str) -> PyResult<PyMatchBindingHandle> {
        self.ensure_open()?;
        self.inner
            .exact(type_name)
            .map(|inner| PyMatchBindingHandle { inner })
            .map_err(py_match_orm_error)
    }

    fn subtypes(&self, type_name: &str) -> PyResult<PyMatchBindingHandle> {
        self.ensure_open()?;
        self.inner
            .subtypes(type_name)
            .map(|inner| PyMatchBindingHandle { inner })
            .map_err(py_match_orm_error)
    }

    fn function_by_id(&self, function_id: &str) -> PyResult<PyMatchFunctionHandle> {
        self.ensure_open()?;
        let id = FunctionId::new(function_id)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        self.inner
            .function(&id)
            .map(|inner| PyMatchFunctionHandle { inner })
            .map_err(py_match_orm_error)
    }

    fn function_value(
        &self,
        attribute_type_key: &str,
        value: PyRef<'_, PyDynamicValue>,
    ) -> PyResult<PyMatchFunctionValueHandle> {
        self.ensure_open()?;
        let installed = self.installed.as_deref().ok_or_else(|| {
            PyRuntimeError::new_err(
                "this match session has no installed function projection authority",
            )
        })?;
        let attribute_type: TypeId = serde_json::from_str(attribute_type_key)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        let canonical = to_canonical_json(&attribute_type)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        if canonical != attribute_type_key.as_bytes() {
            return Err(PyValueError::new_err(
                "function attribute identity is not canonical",
            ));
        }
        let projected = ProjectedAttributeValue::try_from_attribute_value(
            installed,
            attribute_type,
            &value.attribute_value(),
        )
        .map_err(py_sdk_diagnostic)?;
        self.inner
            .function_value(&projected)
            .map(|inner| PyMatchFunctionValueHandle { inner })
            .map_err(py_match_orm_error)
    }

    #[allow(clippy::too_many_arguments)]
    fn reachable(
        &self,
        relation_type: &str,
        role_from: &str,
        role_to: &str,
        source: PyRef<'_, PyMatchBindingHandle>,
        target: PyRef<'_, PyMatchBindingHandle>,
        min_depth: &Bound<'_, PyAny>,
        max_depth: &Bound<'_, PyAny>,
    ) -> PyResult<PyMatchPredicateHandle> {
        self.ensure_open()?;
        let min_depth = python_reachability_depth(min_depth, "min_depth")?;
        let max_depth = python_reachability_depth(max_depth, "max_depth")?;
        self.inner
            .reachable(
                relation_type,
                role_from,
                role_to,
                &source.inner,
                &target.inner,
                min_depth,
                max_depth,
            )
            .map(|inner| PyMatchPredicateHandle { inner })
            .map_err(py_match_orm_error)
    }

    fn positional(
        &self,
        py: Python<'_>,
        slots: Vec<Py<PyMatchSelectionHandle>>,
    ) -> PyResult<PyMatchShapeHandle> {
        self.ensure_open()?;
        let slots = slots
            .iter()
            .map(|slot| slot.borrow(py).inner.clone())
            .collect::<Vec<_>>();
        self.inner
            .positional(slots)
            .map(|inner| PyMatchShapeHandle { inner })
            .map_err(py_match_orm_error)
    }

    fn named(
        &self,
        py: Python<'_>,
        names: Vec<String>,
        selections: Vec<Py<PyMatchSelectionHandle>>,
    ) -> PyResult<PyMatchShapeHandle> {
        self.ensure_open()?;
        if names.len() != selections.len() {
            return Err(PyValueError::new_err(
                "named output names and selections must have equal length",
            ));
        }
        let slots = names
            .into_iter()
            .zip(selections.iter().map(|slot| slot.borrow(py).inner.clone()))
            .collect::<Vec<_>>();
        self.inner
            .named(slots)
            .map(|inner| PyMatchShapeHandle { inner })
            .map_err(py_match_orm_error)
    }

    fn named_checked(
        &self,
        py: Python<'_>,
        declarations: Vec<(String, String, bool)>,
        names: Vec<String>,
        selections: Vec<Py<PyMatchSelectionHandle>>,
    ) -> PyResult<PyMatchShapeHandle> {
        self.ensure_open()?;
        if names.len() != selections.len() {
            return Err(PyValueError::new_err(
                "named output names and selections must have equal length",
            ));
        }
        let slots = names
            .into_iter()
            .zip(selections.iter().map(|slot| slot.borrow(py).inner.clone()))
            .collect::<Vec<_>>();
        self.inner
            .named_checked(declarations, slots)
            .map(|inner| PyMatchShapeHandle { inner })
            .map_err(py_match_orm_error)
    }

    fn query(&self, shape: PyRef<'_, PyMatchShapeHandle>) -> PyResult<PyMatchQueryHandle> {
        self.ensure_open()?;
        self.inner
            .query(shape.inner.clone())
            .map(|inner| PyMatchQueryHandle {
                inner,
                installed: self
                    .installed
                    .as_ref()
                    .filter(|installed| supports_projected_query_companion(installed))
                    .map(Arc::clone),
                resources: self.resources,
                cancellation: self.cancellation.clone(),
                session_closed: Arc::clone(&self.closed),
                closed: Arc::new(AtomicBool::new(false)),
            })
            .map_err(py_match_orm_error)
    }
}

/// Marshal an exact Python `int` into a native unsigned integer.
///
/// Type conversion only: `bool` and other non-`int` values raise
/// `TypeError`, and a value outside the native range raises
/// `OverflowError`. Query rules on the converted value (a zero limit, a
/// window overflow, a depth ceiling) belong to the core and surface as
/// `MatchRequestError`.
fn exact_unsigned<T: TryFrom<i128>>(value: &Bound<'_, PyAny>, name: &str, max: u64) -> PyResult<T> {
    let range = || PyOverflowError::new_err(format!("{name} must be an integer from 0 to {max}"));
    value
        .cast_exact::<PyInt>()
        .map_err(|_| PyTypeError::new_err(format!("{name} must be an exact Python int")))?
        .extract::<i128>()
        .map_err(|_| range())
        .and_then(|value| T::try_from(value).map_err(|_| range()))
}

fn python_reachability_depth(value: &Bound<'_, PyAny>, name: &str) -> PyResult<u8> {
    exact_unsigned(value, name, u64::from(u8::MAX))
}

pub(crate) fn python_window(
    offset: &Bound<'_, PyAny>,
    limit: &Bound<'_, PyAny>,
) -> PyResult<Window> {
    Ok(Window {
        offset: exact_unsigned(offset, "offset", u64::MAX)?,
        limit: exact_unsigned(limit, "limit", u64::MAX)?,
    })
}

#[pymethods]
impl PyMatchFunctionHandle {
    fn call(
        &self,
        py: Python<'_>,
        arguments: Vec<Py<PyMatchFunctionArgumentHandle>>,
    ) -> PyResult<PyMatchFunctionCallHandle> {
        self.inner
            .call(
                arguments
                    .iter()
                    .map(|argument| argument.borrow(py).inner.clone()),
            )
            .map(|inner| PyMatchFunctionCallHandle { inner })
            .map_err(py_match_orm_error)
    }
}

#[pymethods]
impl PyMatchFunctionValueHandle {
    fn function_argument(&self) -> PyMatchFunctionArgumentHandle {
        PyMatchFunctionArgumentHandle {
            inner: self.inner.function_argument(),
        }
    }
}

#[pymethods]
impl PyMatchFunctionCallHandle {
    fn function_argument(&self) -> PyMatchFunctionArgumentHandle {
        PyMatchFunctionArgumentHandle {
            inner: self.inner.function_argument(),
        }
    }

    fn compare_field(
        &self,
        operator: &str,
        field: PyRef<'_, PyMatchFieldHandle>,
    ) -> PyResult<PyMatchPredicateHandle> {
        self.inner
            .compare_field(parse_enum::<ComparisonOp>(operator)?, &field.inner)
            .map(|inner| PyMatchPredicateHandle { inner })
            .map_err(py_match_orm_error)
    }

    fn compare_value(
        &self,
        operator: &str,
        value: PyRef<'_, PyMatchFunctionValueHandle>,
    ) -> PyResult<PyMatchPredicateHandle> {
        self.inner
            .compare_value(parse_enum::<ComparisonOp>(operator)?, &value.inner)
            .map(|inner| PyMatchPredicateHandle { inner })
            .map_err(py_match_orm_error)
    }

    fn compare_call(
        &self,
        operator: &str,
        other: PyRef<'_, PyMatchFunctionCallHandle>,
    ) -> PyResult<PyMatchPredicateHandle> {
        self.inner
            .compare_call(parse_enum::<ComparisonOp>(operator)?, &other.inner)
            .map(|inner| PyMatchPredicateHandle { inner })
            .map_err(py_match_orm_error)
    }
}

#[pymethods]
impl PyMatchBindingHandle {
    fn iid(&self, iid: &str) -> PyResult<PyMatchPredicateHandle> {
        self.inner
            .iid(iid)
            .map(|inner| PyMatchPredicateHandle { inner })
            .map_err(py_match_orm_error)
    }

    fn iid_in(&self, iids: Vec<String>) -> PyResult<PyMatchPredicateHandle> {
        self.inner
            .iid_in(iids)
            .map(|inner| PyMatchPredicateHandle { inner })
            .map_err(py_match_orm_error)
    }

    fn function_argument(&self) -> PyMatchFunctionArgumentHandle {
        PyMatchFunctionArgumentHandle {
            inner: self.inner.function_argument(),
        }
    }

    fn field(&self, field_name: &str) -> PyResult<PyMatchFieldHandle> {
        self.inner
            .field(field_name)
            .map(|inner| PyMatchFieldHandle { inner })
            .map_err(py_match_orm_error)
    }

    fn field_owned_by(&self, owner_type: &str, field_name: &str) -> PyResult<PyMatchFieldHandle> {
        self.inner
            .field_owned_by(owner_type, field_name)
            .map(|inner| PyMatchFieldHandle { inner })
            .map_err(py_match_orm_error)
    }

    fn role(&self, role_name: &str) -> PyResult<PyMatchRoleHandle> {
        self.inner
            .role(role_name)
            .map(|inner| PyMatchRoleHandle { inner })
            .map_err(py_match_orm_error)
    }

    fn role_owned_by(&self, owner_type: &str, role_name: &str) -> PyResult<PyMatchRoleHandle> {
        self.inner
            .role_owned_by(owner_type, role_name)
            .map(|inner| PyMatchRoleHandle { inner })
            .map_err(py_match_orm_error)
    }

    fn one(&self) -> PyMatchSelectionHandle {
        PyMatchSelectionHandle {
            inner: self.inner.one(),
        }
    }

    fn collect(&self) -> PyMatchSelectionHandle {
        PyMatchSelectionHandle {
            inner: self.inner.collect(),
        }
    }
}

#[pymethods]
impl PyMatchFieldHandle {
    fn presence(&self, present: bool) -> PyMatchPredicateHandle {
        PyMatchPredicateHandle {
            inner: self.inner.presence(present),
        }
    }

    fn compare_value(
        &self,
        operator: &str,
        value: PyRef<'_, PyDynamicValue>,
    ) -> PyResult<PyMatchPredicateHandle> {
        Ok(PyMatchPredicateHandle {
            inner: self.inner.compare_value(
                parse_enum::<ComparisonOp>(operator)?,
                value.attribute_value(),
            ),
        })
    }

    fn compare_field(
        &self,
        operator: &str,
        other: PyRef<'_, PyMatchFieldHandle>,
    ) -> PyResult<PyMatchPredicateHandle> {
        self.inner
            .compare_field(parse_enum::<ComparisonOp>(operator)?, &other.inner)
            .map(|inner| PyMatchPredicateHandle { inner })
            .map_err(py_match_orm_error)
    }

    fn order(&self, direction: &str, missing: &str) -> PyResult<PyMatchOrderHandle> {
        Ok(PyMatchOrderHandle {
            inner: self.inner.order(
                parse_enum::<SortDirection>(direction)?,
                parse_enum::<MissingOrder>(missing)?,
            ),
        })
    }
}

#[pymethods]
impl PyMatchRoleHandle {
    fn connects(
        &self,
        player: PyRef<'_, PyMatchBindingHandle>,
    ) -> PyResult<PyMatchPredicateHandle> {
        self.inner
            .connects(&player.inner)
            .map(|inner| PyMatchPredicateHandle { inner })
            .map_err(py_match_orm_error)
    }
}

#[pymethods]
impl PyMatchPredicateHandle {
    #[pyo3(name = "and_")]
    fn and_predicate(
        &self,
        other: PyRef<'_, PyMatchPredicateHandle>,
    ) -> PyResult<PyMatchPredicateHandle> {
        self.inner
            .and(&other.inner)
            .map(|inner| PyMatchPredicateHandle { inner })
            .map_err(py_match_orm_error)
    }

    #[pyo3(name = "or_")]
    fn or_predicate(
        &self,
        other: PyRef<'_, PyMatchPredicateHandle>,
    ) -> PyResult<PyMatchPredicateHandle> {
        self.inner
            .or(&other.inner)
            .map(|inner| PyMatchPredicateHandle { inner })
            .map_err(py_match_orm_error)
    }

    #[pyo3(name = "not_")]
    fn negated(&self) -> PyMatchPredicateHandle {
        PyMatchPredicateHandle {
            inner: self.inner.not(),
        }
    }
}

#[pymethods]
impl PyMatchSelectionHandle {
    #[pyo3(signature = (enabled=true))]
    fn distinct(&self, enabled: bool) -> PyResult<PyMatchSelectionHandle> {
        self.inner
            .distinct(enabled)
            .map(|inner| PyMatchSelectionHandle { inner })
            .map_err(py_match_orm_error)
    }

    fn order_by(&self, order: PyRef<'_, PyMatchOrderHandle>) -> PyResult<PyMatchSelectionHandle> {
        self.inner
            .order_by(order.inner.clone())
            .map(|inner| PyMatchSelectionHandle { inner })
            .map_err(py_match_orm_error)
    }
}

#[pymethods]
impl PyMatchQueryHandle {
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    #[getter]
    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) || self.session_closed.load(Ordering::Acquire)
    }

    /// Create an independently closable value-equivalent query handle.
    fn fork(&self) -> PyResult<PyMatchQueryHandle> {
        self.ensure_open()?;
        Ok(self.derived(self.inner.clone()))
    }

    fn add_hidden(&self, binding: PyRef<'_, PyMatchBindingHandle>) -> PyResult<PyMatchQueryHandle> {
        self.ensure_open()?;
        self.inner
            .add_hidden(binding.inner.clone())
            .map(|inner| self.derived(inner))
            .map_err(py_match_orm_error)
    }

    fn where_predicate(
        &self,
        predicate: PyRef<'_, PyMatchPredicateHandle>,
    ) -> PyResult<PyMatchQueryHandle> {
        self.ensure_open()?;
        self.inner
            .where_predicate(predicate.inner.clone())
            .map(|inner| self.derived(inner))
            .map_err(py_match_orm_error)
    }

    fn allow_cross_join(
        &self,
        left: PyRef<'_, PyMatchBindingHandle>,
        right: PyRef<'_, PyMatchBindingHandle>,
    ) -> PyResult<PyMatchQueryHandle> {
        self.ensure_open()?;
        self.inner
            .allow_cross_join(&left.inner, &right.inner)
            .map(|inner| self.derived(inner))
            .map_err(py_match_orm_error)
    }

    /// Refuse group fields at `group_by()` with the reduction terminal's
    /// diagnostic (`field_group_tuple_cap_exceeded` under `selected_slots`).
    fn check_group_fields(
        &self,
        py: Python<'_>,
        groups: Vec<Py<PyMatchFieldHandle>>,
    ) -> PyResult<()> {
        self.ensure_open()?;
        let groups = groups
            .iter()
            .map(|group| group.borrow(py).inner.clone())
            .collect::<Vec<_>>();
        let groups = groups.iter().collect::<Vec<_>>();
        self.inner
            .check_group_fields(&groups)
            .map_err(py_match_orm_error)
    }

    fn fetch_rows_diagnostic(
        &self,
        py: Python<'_>,
        order: Vec<Py<PyMatchOrderHandle>>,
        offset: &Bound<'_, PyAny>,
        limit: &Bound<'_, PyAny>,
        cardinality: &str,
    ) -> PyResult<String> {
        self.ensure_open()?;
        let order = order_handles(py, &order);
        let request = self.inner.fetch_rows(
            &order,
            python_window(offset, limit)?,
            parse_enum::<RowCardinality>(cardinality)?,
        );
        diagnostic_from_request(request)
    }

    fn page_by_diagnostic(
        &self,
        py: Python<'_>,
        root: PyRef<'_, PyMatchBindingHandle>,
        order: Vec<Py<PyMatchOrderHandle>>,
        offset: &Bound<'_, PyAny>,
        limit: &Bound<'_, PyAny>,
        include_total: bool,
    ) -> PyResult<String> {
        self.ensure_open()?;
        let order = order_handles(py, &order);
        diagnostic_from_request(self.inner.page_by(
            &root.inner,
            &order,
            python_window(offset, limit)?,
            include_total,
        ))
    }

    fn count_by_diagnostic(&self, root: PyRef<'_, PyMatchBindingHandle>) -> PyResult<String> {
        self.ensure_open()?;
        diagnostic_from_request(self.inner.count_by(&root.inner))
    }

    fn exists_by_diagnostic(&self, root: PyRef<'_, PyMatchBindingHandle>) -> PyResult<String> {
        self.ensure_open()?;
        diagnostic_from_request(self.inner.exists_by(&root.inner))
    }

    fn execute_fetch_rows_owned(
        &self,
        py: Python<'_>,
        database: &PyRustDatabase,
        order: Vec<Py<PyMatchOrderHandle>>,
        offset: &Bound<'_, PyAny>,
        limit: &Bound<'_, PyAny>,
        cardinality: &str,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let target = TerminalTarget::Database(database);
        self.fetch_rows_on(py, target, &order, offset, limit, cardinality)
    }

    fn execute_fetch_rows_borrowed(
        &self,
        py: Python<'_>,
        transaction: &PyRustTransactionContext,
        order: Vec<Py<PyMatchOrderHandle>>,
        offset: &Bound<'_, PyAny>,
        limit: &Bound<'_, PyAny>,
        cardinality: &str,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let target = TerminalTarget::Transaction(transaction);
        self.fetch_rows_on(py, target, &order, offset, limit, cardinality)
    }

    fn execute_fetch_rows_proxy(
        &self,
        py: Python<'_>,
        connection: &PyProxyConnection,
        order: Vec<Py<PyMatchOrderHandle>>,
        offset: &Bound<'_, PyAny>,
        limit: &Bound<'_, PyAny>,
        cardinality: &str,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let target = TerminalTarget::Proxy(connection);
        self.fetch_rows_on(py, target, &order, offset, limit, cardinality)
    }

    // PyO3 exposes these operation-local arguments as the stable Python terminal contract.
    #[allow(clippy::too_many_arguments)]
    fn execute_page_by_owned(
        &self,
        py: Python<'_>,
        database: &PyRustDatabase,
        root: PyRef<'_, PyMatchBindingHandle>,
        order: Vec<Py<PyMatchOrderHandle>>,
        offset: &Bound<'_, PyAny>,
        limit: &Bound<'_, PyAny>,
        include_total: bool,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let target = TerminalTarget::Database(database);
        self.page_by_on(
            py,
            target,
            &root.inner,
            &order,
            offset,
            limit,
            include_total,
        )
    }

    // PyO3 exposes these operation-local arguments as the stable Python terminal contract.
    #[allow(clippy::too_many_arguments)]
    fn execute_page_by_borrowed(
        &self,
        py: Python<'_>,
        transaction: &PyRustTransactionContext,
        root: PyRef<'_, PyMatchBindingHandle>,
        order: Vec<Py<PyMatchOrderHandle>>,
        offset: &Bound<'_, PyAny>,
        limit: &Bound<'_, PyAny>,
        include_total: bool,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let target = TerminalTarget::Transaction(transaction);
        self.page_by_on(
            py,
            target,
            &root.inner,
            &order,
            offset,
            limit,
            include_total,
        )
    }

    // PyO3 exposes these operation-local arguments as the stable Python terminal contract.
    #[allow(clippy::too_many_arguments)]
    fn execute_page_by_proxy(
        &self,
        py: Python<'_>,
        connection: &PyProxyConnection,
        root: PyRef<'_, PyMatchBindingHandle>,
        order: Vec<Py<PyMatchOrderHandle>>,
        offset: &Bound<'_, PyAny>,
        limit: &Bound<'_, PyAny>,
        include_total: bool,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let target = TerminalTarget::Proxy(connection);
        self.page_by_on(
            py,
            target,
            &root.inner,
            &order,
            offset,
            limit,
            include_total,
        )
    }

    fn execute_count_by_owned(
        &self,
        py: Python<'_>,
        database: &PyRustDatabase,
        root: PyRef<'_, PyMatchBindingHandle>,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let target = TerminalTarget::Database(database);
        self.run_terminal(py, target, |query| query.validate_count_by(&root.inner))
    }

    fn execute_count_by_borrowed(
        &self,
        py: Python<'_>,
        transaction: &PyRustTransactionContext,
        root: PyRef<'_, PyMatchBindingHandle>,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let target = TerminalTarget::Transaction(transaction);
        self.run_terminal(py, target, |query| query.validate_count_by(&root.inner))
    }

    fn execute_count_by_proxy(
        &self,
        py: Python<'_>,
        connection: &PyProxyConnection,
        root: PyRef<'_, PyMatchBindingHandle>,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let target = TerminalTarget::Proxy(connection);
        self.run_terminal(py, target, |query| query.validate_count_by(&root.inner))
    }

    fn execute_exists_by_owned(
        &self,
        py: Python<'_>,
        database: &PyRustDatabase,
        root: PyRef<'_, PyMatchBindingHandle>,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let target = TerminalTarget::Database(database);
        self.run_terminal(py, target, |query| query.validate_exists_by(&root.inner))
    }

    fn execute_exists_by_borrowed(
        &self,
        py: Python<'_>,
        transaction: &PyRustTransactionContext,
        root: PyRef<'_, PyMatchBindingHandle>,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let target = TerminalTarget::Transaction(transaction);
        self.run_terminal(py, target, |query| query.validate_exists_by(&root.inner))
    }

    fn execute_exists_by_proxy(
        &self,
        py: Python<'_>,
        connection: &PyProxyConnection,
        root: PyRef<'_, PyMatchBindingHandle>,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let target = TerminalTarget::Proxy(connection);
        self.run_terminal(py, target, |query| query.validate_exists_by(&root.inner))
    }

    #[pyo3(signature = (root, group, reducers, inputs))]
    fn reduce_by_diagnostic(
        &self,
        py: Python<'_>,
        root: PyRef<'_, PyMatchBindingHandle>,
        group: Option<PyRef<'_, PyMatchBindingHandle>>,
        reducers: Vec<String>,
        inputs: Vec<Option<Py<PyMatchFieldHandle>>>,
    ) -> PyResult<String> {
        self.ensure_open()?;
        let terms = reduce_terms(py, &reducers, &inputs)?;
        let terms = borrow_reduce_terms(&terms);
        diagnostic_from_request(self.inner.reduce_by(
            &root.inner,
            group.as_ref().map(|group| &group.inner),
            &terms,
        ))
    }

    // PyO3 exposes these operation-local arguments as the stable Python terminal contract.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (database, root, group, reducers, inputs))]
    fn execute_reduce_by_owned(
        &self,
        py: Python<'_>,
        database: &PyRustDatabase,
        root: PyRef<'_, PyMatchBindingHandle>,
        group: Option<PyRef<'_, PyMatchBindingHandle>>,
        reducers: Vec<String>,
        inputs: Vec<Option<Py<PyMatchFieldHandle>>>,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let target = TerminalTarget::Database(database);
        let group = group.as_ref().map(|group| &group.inner);
        self.reduce_by_on(py, target, &root.inner, group, &reducers, &inputs)
    }

    // PyO3 exposes these operation-local arguments as the stable Python terminal contract.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (transaction, root, group, reducers, inputs))]
    fn execute_reduce_by_borrowed(
        &self,
        py: Python<'_>,
        transaction: &PyRustTransactionContext,
        root: PyRef<'_, PyMatchBindingHandle>,
        group: Option<PyRef<'_, PyMatchBindingHandle>>,
        reducers: Vec<String>,
        inputs: Vec<Option<Py<PyMatchFieldHandle>>>,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let target = TerminalTarget::Transaction(transaction);
        let group = group.as_ref().map(|group| &group.inner);
        self.reduce_by_on(py, target, &root.inner, group, &reducers, &inputs)
    }

    // PyO3 exposes these operation-local arguments as the stable Python terminal contract.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (connection, root, group, reducers, inputs))]
    fn execute_reduce_by_proxy(
        &self,
        py: Python<'_>,
        connection: &PyProxyConnection,
        root: PyRef<'_, PyMatchBindingHandle>,
        group: Option<PyRef<'_, PyMatchBindingHandle>>,
        reducers: Vec<String>,
        inputs: Vec<Option<Py<PyMatchFieldHandle>>>,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let target = TerminalTarget::Proxy(connection);
        let group = group.as_ref().map(|group| &group.inner);
        self.reduce_by_on(py, target, &root.inner, group, &reducers, &inputs)
    }

    #[pyo3(signature = (root, group, reducers, inputs))]
    fn reduce_by_field_diagnostic(
        &self,
        py: Python<'_>,
        root: PyRef<'_, PyMatchBindingHandle>,
        group: PyRef<'_, PyMatchFieldHandle>,
        reducers: Vec<String>,
        inputs: Vec<Option<Py<PyMatchFieldHandle>>>,
    ) -> PyResult<String> {
        self.ensure_open()?;
        let terms = reduce_terms(py, &reducers, &inputs)?;
        let terms = borrow_reduce_terms(&terms);
        diagnostic_from_request(
            self.inner
                .reduce_by_field(&root.inner, &group.inner, &terms),
        )
    }

    // PyO3 exposes these operation-local arguments as the stable Python terminal contract.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (database, root, group, reducers, inputs))]
    fn execute_reduce_by_field_owned(
        &self,
        py: Python<'_>,
        database: &PyRustDatabase,
        root: PyRef<'_, PyMatchBindingHandle>,
        group: PyRef<'_, PyMatchFieldHandle>,
        reducers: Vec<String>,
        inputs: Vec<Option<Py<PyMatchFieldHandle>>>,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let target = TerminalTarget::Database(database);
        self.reduce_by_field_on(py, target, &root.inner, &group.inner, &reducers, &inputs)
    }

    // PyO3 exposes these operation-local arguments as the stable Python terminal contract.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (transaction, root, group, reducers, inputs))]
    fn execute_reduce_by_field_borrowed(
        &self,
        py: Python<'_>,
        transaction: &PyRustTransactionContext,
        root: PyRef<'_, PyMatchBindingHandle>,
        group: PyRef<'_, PyMatchFieldHandle>,
        reducers: Vec<String>,
        inputs: Vec<Option<Py<PyMatchFieldHandle>>>,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let target = TerminalTarget::Transaction(transaction);
        self.reduce_by_field_on(py, target, &root.inner, &group.inner, &reducers, &inputs)
    }

    // PyO3 exposes these operation-local arguments as the stable Python terminal contract.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (connection, root, group, reducers, inputs))]
    fn execute_reduce_by_field_proxy(
        &self,
        py: Python<'_>,
        connection: &PyProxyConnection,
        root: PyRef<'_, PyMatchBindingHandle>,
        group: PyRef<'_, PyMatchFieldHandle>,
        reducers: Vec<String>,
        inputs: Vec<Option<Py<PyMatchFieldHandle>>>,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let target = TerminalTarget::Proxy(connection);
        self.reduce_by_field_on(py, target, &root.inner, &group.inner, &reducers, &inputs)
    }

    #[pyo3(signature = (root, groups, reducers, inputs))]
    fn reduce_by_fields_diagnostic(
        &self,
        py: Python<'_>,
        root: PyRef<'_, PyMatchBindingHandle>,
        groups: Vec<Py<PyMatchFieldHandle>>,
        reducers: Vec<String>,
        inputs: Vec<Option<Py<PyMatchFieldHandle>>>,
    ) -> PyResult<String> {
        self.ensure_open()?;
        let groups = groups
            .iter()
            .map(|group| group.borrow(py).inner.clone())
            .collect::<Vec<_>>();
        let groups = groups.iter().collect::<Vec<_>>();
        let terms = reduce_terms(py, &reducers, &inputs)?;
        let terms = borrow_reduce_terms(&terms);
        diagnostic_from_request(self.inner.reduce_by_fields(&root.inner, &groups, &terms))
    }

    // PyO3 exposes these operation-local arguments as the stable Python terminal contract.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (database, root, groups, reducers, inputs))]
    fn execute_reduce_by_fields_owned(
        &self,
        py: Python<'_>,
        database: &PyRustDatabase,
        root: PyRef<'_, PyMatchBindingHandle>,
        groups: Vec<Py<PyMatchFieldHandle>>,
        reducers: Vec<String>,
        inputs: Vec<Option<Py<PyMatchFieldHandle>>>,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let target = TerminalTarget::Database(database);
        self.reduce_by_fields_on(py, target, &root.inner, &groups, &reducers, &inputs)
    }

    // PyO3 exposes these operation-local arguments as the stable Python terminal contract.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (transaction, root, groups, reducers, inputs))]
    fn execute_reduce_by_fields_borrowed(
        &self,
        py: Python<'_>,
        transaction: &PyRustTransactionContext,
        root: PyRef<'_, PyMatchBindingHandle>,
        groups: Vec<Py<PyMatchFieldHandle>>,
        reducers: Vec<String>,
        inputs: Vec<Option<Py<PyMatchFieldHandle>>>,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let target = TerminalTarget::Transaction(transaction);
        self.reduce_by_fields_on(py, target, &root.inner, &groups, &reducers, &inputs)
    }

    // PyO3 exposes these operation-local arguments as the stable Python terminal contract.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (connection, root, groups, reducers, inputs))]
    fn execute_reduce_by_fields_proxy(
        &self,
        py: Python<'_>,
        connection: &PyProxyConnection,
        root: PyRef<'_, PyMatchBindingHandle>,
        groups: Vec<Py<PyMatchFieldHandle>>,
        reducers: Vec<String>,
        inputs: Vec<Option<Py<PyMatchFieldHandle>>>,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let target = TerminalTarget::Proxy(connection);
        self.reduce_by_fields_on(py, target, &root.inner, &groups, &reducers, &inputs)
    }
}

/// One body per terminal kind; the Python methods above only name the target.
impl PyMatchQueryHandle {
    /// Open one invocation budget, validate the request, and run it on
    /// `target`. The budget is taken first so a closed query or an expired
    /// session refuses before any validation work.
    fn run_terminal(
        &self,
        py: Python<'_>,
        target: TerminalTarget<'_>,
        validate: impl FnOnce(&QueryHandle) -> Result<ValidatedMatchRequest, OrmError>,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        self.run_terminal_checked(py, target, |query| {
            validate(query).map_err(py_match_orm_error)
        })
    }

    fn run_terminal_checked(
        &self,
        py: Python<'_>,
        target: TerminalTarget<'_>,
        validate: impl FnOnce(&QueryHandle) -> PyResult<ValidatedMatchRequest>,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let budget = self.begin_invocation()?;
        let validated = validate(&self.inner)?;
        execute_on_target(
            py,
            target,
            self.installed.as_ref(),
            validated,
            self.inner.registry_arc(),
            budget,
        )
    }

    fn fetch_rows_on(
        &self,
        py: Python<'_>,
        target: TerminalTarget<'_>,
        order: &[Py<PyMatchOrderHandle>],
        offset: &Bound<'_, PyAny>,
        limit: &Bound<'_, PyAny>,
        cardinality: &str,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let window = python_window(offset, limit)?;
        self.run_terminal_checked(py, target, |query| {
            let orders = order_handles(py, order);
            query
                .validate_fetch_rows(&orders, window, parse_enum::<RowCardinality>(cardinality)?)
                .map_err(py_match_orm_error)
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn page_by_on(
        &self,
        py: Python<'_>,
        target: TerminalTarget<'_>,
        root: &BindingHandle,
        order: &[Py<PyMatchOrderHandle>],
        offset: &Bound<'_, PyAny>,
        limit: &Bound<'_, PyAny>,
        include_total: bool,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let window = python_window(offset, limit)?;
        self.run_terminal(py, target, |query| {
            let orders = order_handles(py, order);
            query.validate_page_by(root, &orders, window, include_total)
        })
    }

    fn reduce_by_on(
        &self,
        py: Python<'_>,
        target: TerminalTarget<'_>,
        root: &BindingHandle,
        group: Option<&BindingHandle>,
        reducers: &[String],
        inputs: &[Option<Py<PyMatchFieldHandle>>],
    ) -> PyResult<PyValidatedMatchResultHandle> {
        self.run_terminal_checked(py, target, |query| {
            let terms = reduce_terms(py, reducers, inputs)?;
            let terms = borrow_reduce_terms(&terms);
            query
                .validate_reduce_by(root, group, &terms)
                .map_err(py_match_orm_error)
        })
    }

    fn reduce_by_field_on(
        &self,
        py: Python<'_>,
        target: TerminalTarget<'_>,
        root: &BindingHandle,
        group: &FieldHandle,
        reducers: &[String],
        inputs: &[Option<Py<PyMatchFieldHandle>>],
    ) -> PyResult<PyValidatedMatchResultHandle> {
        self.run_terminal_checked(py, target, |query| {
            let terms = reduce_terms(py, reducers, inputs)?;
            let terms = borrow_reduce_terms(&terms);
            query
                .validate_reduce_by_field(root, group, &terms)
                .map_err(py_match_orm_error)
        })
    }

    fn reduce_by_fields_on(
        &self,
        py: Python<'_>,
        target: TerminalTarget<'_>,
        root: &BindingHandle,
        groups: &[Py<PyMatchFieldHandle>],
        reducers: &[String],
        inputs: &[Option<Py<PyMatchFieldHandle>>],
    ) -> PyResult<PyValidatedMatchResultHandle> {
        self.run_terminal_checked(py, target, |query| {
            let groups = groups
                .iter()
                .map(|group| group.borrow(py).inner.clone())
                .collect::<Vec<_>>();
            let groups = groups.iter().collect::<Vec<_>>();
            let terms = reduce_terms(py, reducers, inputs)?;
            let terms = borrow_reduce_terms(&terms);
            query
                .validate_reduce_by_fields(root, &groups, &terms)
                .map_err(py_match_orm_error)
        })
    }
}

pub(crate) fn reduce_terms(
    py: Python<'_>,
    reducers: &[String],
    inputs: &[Option<Py<PyMatchFieldHandle>>],
) -> PyResult<Vec<(Reduction, Option<FieldHandle>)>> {
    if reducers.len() != inputs.len() {
        return Err(PyValueError::new_err(
            "reducer names and reducer inputs must have equal length",
        ));
    }
    reducers
        .iter()
        .zip(inputs)
        .map(|(reducer, input)| {
            Ok((
                parse_enum::<Reduction>(reducer)?,
                input.as_ref().map(|field| field.borrow(py).inner.clone()),
            ))
        })
        .collect()
}

pub(crate) fn borrow_reduce_terms(
    terms: &[(Reduction, Option<FieldHandle>)],
) -> Vec<(Reduction, Option<&FieldHandle>)> {
    terms
        .iter()
        .map(|(reduction, input)| (*reduction, input.as_ref()))
        .collect()
}

/// Run one validated request on `target` through the shared terminal
/// executor, which holds it to the connection's answer-limit ceiling, and
/// materialize the result under the same constrained budget.
fn execute_on_target(
    py: Python<'_>,
    target: TerminalTarget<'_>,
    installed: Option<&Arc<InstalledRuntimeProjection>>,
    validated: ValidatedMatchRequest,
    registry: Arc<DescriptorRegistry>,
    budget: PyQueryInvocationBudget,
) -> PyResult<PyValidatedMatchResultHandle> {
    match target {
        TerminalTarget::Database(database) => {
            let (database, runtime) = database.handles();
            let origin = installed.map(|_| ProjectedQueryOrigin::for_database(database.as_ref()));
            let target = QueryTarget::Database(database.as_ref());
            execute_and_materialize(
                py, &runtime, target, installed, origin, validated, registry, budget,
            )
        }
        TerminalTarget::Transaction(transaction) => {
            let (transaction, runtime) = transaction.handles();
            let origin = installed
                .map(|_| ProjectedQueryOrigin::for_transaction(&transaction))
                .transpose()
                .map_err(py_sdk_diagnostic)?;
            let target = QueryTarget::Transaction(&transaction);
            execute_and_materialize(
                py, &runtime, target, installed, origin, validated, registry, budget,
            )
        }
        TerminalTarget::Proxy(connection) => {
            let origin = installed.map(|_| ProjectedQueryOrigin::remote_unbound());
            let target = QueryTarget::Proxy(connection.connection());
            execute_and_materialize(
                py,
                connection.runtime(),
                target,
                installed,
                origin,
                validated,
                registry,
                budget,
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_and_materialize(
    py: Python<'_>,
    runtime: &ProviderRuntimeOwner,
    target: QueryTarget<'_>,
    installed: Option<&Arc<InstalledRuntimeProjection>>,
    origin: Option<ProjectedQueryOrigin>,
    validated: ValidatedMatchRequest,
    registry: Arc<DescriptorRegistry>,
    budget: PyQueryInvocationBudget,
) -> PyResult<PyValidatedMatchResultHandle> {
    // Materialization runs under the same ceiling-bound budget as execution.
    let (resources, deadline) = target.budget(budget.resources, budget.deadline);
    let budget = PyQueryInvocationBudget::from_parts(resources, deadline, budget.cancellation);
    let (validated, result) = provider_block_on(
        py,
        runtime,
        target.execute(
            &registry,
            validated,
            budget.resources,
            &budget.cancellation,
            budget.deadline,
        ),
    )
    .map_err(py_query_target_error)?;
    validated_result_handle(installed, origin, validated, result, registry, budget)
}

/// Lower a terminal failure to the exception its stage raised before the
/// shared executor existed.
pub(crate) fn py_query_target_error(error: QueryTargetError) -> PyErr {
    match error {
        QueryTargetError::Direct(error) => py_match_orm_error(error),
        QueryTargetError::Prepare(error) | QueryTargetError::Reply(error) => {
            py_remote_model_error(error)
        }
        QueryTargetError::Interrupted(diagnostic) => py_sdk_diagnostic(diagnostic),
    }
}

pub(crate) fn py_remote_model_error(error: RemoteModelQueryV2Error) -> PyErr {
    match error {
        RemoteModelQueryV2Error::Diagnostic(diagnostic) => {
            py_sdk_diagnostic(lower_remote_query_diagnostic(diagnostic))
        }
        RemoteModelQueryV2Error::Match(error) => py_match_error(error),
    }
}

pub(crate) fn validated_result_handle(
    installed: Option<&Arc<InstalledRuntimeProjection>>,
    origin: Option<ProjectedQueryOrigin>,
    validated: ValidatedMatchRequest,
    result: type_bridge_orm::ValidatedMatchResult,
    registry: Arc<DescriptorRegistry>,
    budget: PyQueryInvocationBudget,
) -> PyResult<PyValidatedMatchResultHandle> {
    let projected = installed
        .zip(origin)
        .map(|(installed, origin)| {
            origin
                .materialize_borrowed_with_budget(
                    installed,
                    &registry,
                    &validated,
                    &result,
                    budget.resources.projected(),
                    &budget.cancellation,
                    Some(budget.deadline),
                )
                .map(|(value, _measure)| value)
                .map_err(py_sdk_diagnostic)
        })
        .transpose()?;
    match projected {
        Some(projected) => Ok(PyValidatedMatchResultHandle::new_with_projected_budget(
            validated,
            result,
            registry,
            projected,
            budget.deadline,
            budget.cancellation,
        )),
        None => Ok(PyValidatedMatchResultHandle::new_with_budget(
            validated,
            result,
            registry,
            budget.deadline,
            budget.cancellation,
        )),
    }
}

pub(crate) fn order_handles(py: Python<'_>, values: &[Py<PyMatchOrderHandle>]) -> Vec<OrderHandle> {
    values
        .iter()
        .map(|value| value.borrow(py).inner.clone())
        .collect()
}

fn diagnostic_from_request(
    request: Result<type_bridge_orm::MatchRequest, OrmError>,
) -> PyResult<String> {
    let request = request.map_err(py_match_orm_error)?;
    let diagnostic = UnvalidatedMatchRequest::from_request(request).map_err(py_match_error)?;
    let bytes = diagnostic.to_canonical_bytes().map_err(py_match_error)?;
    String::from_utf8(bytes)
        .map_err(|error| PyRuntimeError::new_err(format!("diagnostic was not UTF-8: {error}")))
}

/// Parse `value` with the core enum's one parser. An unknown spelling raises
/// `UnknownEnumValueError` carrying the `unknown_enum_value` diagnostic.
pub(crate) fn parse_enum<T>(value: &str) -> PyResult<T>
where
    T: std::str::FromStr<Err = UnknownEnumValue>,
{
    value.parse().map_err(py_unknown_enum_value)
}

pub(crate) fn py_unknown_enum_value(error: UnknownEnumValue) -> PyErr {
    py_value_error_diagnostic(
        &UNKNOWN_ENUM_VALUE_ERROR,
        &SdkExecutionDiagnostic::from(error),
    )
}

/// Raise a projected member-count refusal as `ProjectedCardinalityError`.
pub(crate) fn py_projected_cardinality(diagnostic: SdkExecutionDiagnostic) -> PyErr {
    py_value_error_diagnostic(&PROJECTED_CARDINALITY_ERROR, &diagnostic)
}

fn py_value_error_diagnostic(
    error_type: &'static ValueErrorType,
    diagnostic: &SdkExecutionDiagnostic,
) -> PyErr {
    Python::attach(|py| match error_type.get(py) {
        Ok(error_type) => structured_py_error(
            py,
            PyErr::from_type(error_type.clone(), diagnostic.message().as_str()),
            diagnostic,
        ),
        Err(error) => error,
    })
}

/// A structured SDK diagnostic class deriving from both `MatchRequestError`
/// and `ValueError`, so code that caught the earlier host `ValueError` still
/// catches the core's refusal.
struct ValueErrorType {
    name: &'static str,
    doc: &'static str,
    cell: PyOnceLock<Py<PyType>>,
}

impl ValueErrorType {
    const fn new(name: &'static str, doc: &'static str) -> Self {
        Self {
            name,
            doc,
            cell: PyOnceLock::new(),
        }
    }

    fn get<'py>(&'static self, py: Python<'py>) -> PyResult<&'py Bound<'py, PyType>> {
        self.cell
            .get_or_try_init(py, || {
                let namespace = PyDict::new(py);
                namespace.set_item("__module__", "type_bridge_core")?;
                namespace.set_item("__doc__", self.doc)?;
                let bases = (
                    py.get_type::<MatchRequestError>(),
                    py.get_type::<PyValueError>(),
                );
                py.get_type::<PyType>()
                    .call1((self.name, bases, namespace))?
                    .cast_into::<PyType>()
                    .map(Bound::unbind)
                    .map_err(PyErr::from)
            })
            .map(|error_type| error_type.bind(py))
    }
}

static UNKNOWN_ENUM_VALUE_ERROR: ValueErrorType = ValueErrorType::new(
    "UnknownEnumValueError",
    "A spelling no core enum parser accepts (code `unknown_enum_value`).",
);

static PROJECTED_CARDINALITY_ERROR: ValueErrorType = ValueErrorType::new(
    "ProjectedCardinalityError",
    "A generated model member holds a value count outside its projected \
     cardinality (codes `missing_required_field`, `field_cardinality_violation`, \
     `missing_required_role`, `role_cardinality_violation`).",
);

/// Check `value` against the core enum named by `kind`; return its canonical
/// spelling.
#[pyfunction]
fn canonical_enum_spelling(kind: &str, value: &str) -> PyResult<&'static str> {
    type_bridge_orm::canonical_enum_spelling(kind, value).map_err(py_unknown_enum_value)
}

pub(crate) fn py_match_orm_error(error: OrmError) -> PyErr {
    match error {
        OrmError::Match(error) => py_match_error(error),
        other => PyRuntimeError::new_err(other.to_string()),
    }
}

pub(crate) fn py_match_error(error: MatchError) -> PyErr {
    py_sdk_diagnostic(lower_match_error(&error))
}

pub(crate) fn py_sdk_diagnostic(diagnostic: SdkExecutionDiagnostic) -> PyErr {
    Python::attach(|py| {
        let py_error = MatchRequestError::new_err(diagnostic.message().as_str());
        structured_py_error(py, py_error, &diagnostic)
    })
}

/// Set one attribute on `py_error` per key of the diagnostic's canonical host
/// JSON form (`version`, `category`, `sdk_category`, `query_category`,
/// `code`, `message`, `path`, `details`).
fn structured_py_error(
    py: Python<'_>,
    py_error: PyErr,
    diagnostic: &SdkExecutionDiagnostic,
) -> PyErr {
    let attach = || -> PyResult<()> {
        let value = py_error.value(py);
        let form = pythonize(py, &diagnostic.host_json())?.cast_into::<PyDict>()?;
        for (key, item) in form.iter() {
            value.setattr(key.cast_into::<PyString>()?, item)?;
        }
        Ok(())
    };
    match attach() {
        Ok(()) => py_error,
        Err(attribute_error) => attribute_error,
    }
}

/// Register the native typed-match handle seam on the extension module.
pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add(
        "MatchRequestError",
        module.py().get_type::<MatchRequestError>(),
    )?;
    module.add_class::<PyMatchSessionHandle>()?;
    module.add_class::<PyMatchBindingHandle>()?;
    module.add_class::<PyMatchFunctionHandle>()?;
    module.add_class::<PyMatchFunctionValueHandle>()?;
    module.add_class::<PyMatchFunctionArgumentHandle>()?;
    module.add_class::<PyMatchFunctionCallHandle>()?;
    module.add_class::<PyMatchFieldHandle>()?;
    module.add_class::<PyMatchRoleHandle>()?;
    module.add_class::<PyMatchPredicateHandle>()?;
    module.add_class::<PyMatchOrderHandle>()?;
    module.add_class::<PyMatchSelectionHandle>()?;
    module.add_class::<PyMatchShapeHandle>()?;
    module.add_class::<PyMatchQueryHandle>()?;
    module.add_class::<PyQueryExecutionResourceLimits>()?;
    // Answer-policy defaults, read from the core so Python holds no copies.
    module.add(
        "DEFAULT_QUERY_TIMEOUT_MILLISECONDS",
        type_bridge_orm::DEFAULT_QUERY_TIMEOUT_MILLISECONDS,
    )?;
    module.add("DEFAULT_QUERY_ITEMS", type_bridge_orm::DEFAULT_QUERY_ITEMS)?;
    module.add("DEFAULT_QUERY_BYTES", type_bridge_orm::DEFAULT_QUERY_BYTES)?;
    module.add(
        "DEFAULT_QUERY_GRAPH_NODES",
        type_bridge_orm::DEFAULT_QUERY_GRAPH_NODES,
    )?;
    module.add(
        "DEFAULT_QUERY_ATTRIBUTE_VALUES",
        type_bridge_orm::DEFAULT_QUERY_ATTRIBUTE_VALUES,
    )?;
    module.add(
        "DEFAULT_QUERY_COLLECTION_MEMBERS",
        type_bridge_orm::DEFAULT_QUERY_COLLECTION_MEMBERS,
    )?;
    module.add(
        "DEFAULT_QUERY_ROLE_PLAYERS",
        type_bridge_orm::DEFAULT_QUERY_ROLE_PLAYERS,
    )?;
    module.add(
        "DEFAULT_QUERY_STATEMENTS",
        type_bridge_orm::DEFAULT_QUERY_STATEMENTS,
    )?;
    module.add_class::<PyQueryLimits>()?;
    module.add_class::<PyQueryCancellation>()?;
    crate::validated_result_runtime::register(module)?;
    module.add_function(wrap_pyfunction!(canonical_enum_spelling, module)?)?;
    module.add(
        "UnknownEnumValueError",
        UNKNOWN_ENUM_VALUE_ERROR.get(module.py())?,
    )?;
    module.add(
        "ProjectedCardinalityError",
        PROJECTED_CARDINALITY_ERROR.get(module.py())?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};
    use std::mem::size_of;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use tokio::sync::Notify;
    use type_bridge_orm::_descriptor::{EntityDescriptor, OwnedAttributeDescriptor};
    use type_bridge_orm::_entity::Annotation;
    use type_bridge_orm::_registry::DescriptorRegistry;
    use type_bridge_orm::session::backend::{
        AnswerCancellation, BoundedAnswerLimits, BoundedAnswerReader, BoxFuture, DriverBackend,
        TransactionOps, TxType,
    };
    use type_bridge_orm::{
        AttributeValue, CapabilitySet, Database, MatchExecutionLimits, ValueType,
    };

    use super::*;

    /// A canonical diagnostic must validate and recheck against its registry.
    fn revalidate_diagnostic(
        registry: &DescriptorRegistry,
        diagnostic: &str,
    ) -> Result<String, MatchError> {
        let unvalidated = UnvalidatedMatchRequest::from_canonical_bytes(diagnostic.as_bytes())?;
        let validated = unvalidated.validate(registry)?;
        validated.recheck_schema(registry)?;
        Ok(diagnostic.to_owned())
    }

    fn registry() -> Arc<DescriptorRegistry> {
        let registry = Arc::new(DescriptorRegistry::new());
        registry
            .register_entity(EntityDescriptor {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeDescriptor {
                    field_name: "name".into(),
                    attr_name: "person-name".into(),
                    value_type: ValueType::String,
                    annotations: vec![Annotation::Key],
                    is_optional: false,
                    is_ordered: false,
                    doc: None,
                    meta: Default::default(),
                }],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        registry
    }

    struct ProviderOpenFailureBackend;

    impl DriverBackend for ProviderOpenFailureBackend {
        fn match_capabilities(&self) -> CapabilitySet {
            CapabilitySet::all()
        }

        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            Box::pin(async {
                Err(OrmError::Connection(
                    "credential=python-binding-secret".into(),
                ))
            })
        }

        fn is_open(&self) -> bool {
            true
        }
    }

    struct BlockingOpenBackend {
        opens: Arc<AtomicUsize>,
        opened: Arc<Notify>,
    }

    impl DriverBackend for BlockingOpenBackend {
        fn match_capabilities(&self) -> CapabilitySet {
            CapabilitySet::all()
        }

        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            self.opened.notify_one();
            Box::pin(std::future::pending())
        }

        fn is_open(&self) -> bool {
            true
        }
    }

    fn source_identity(root: &Path, relative: &str) -> Value {
        let path = root.join(relative);
        let metadata = path
            .symlink_metadata()
            .unwrap_or_else(|error| panic!("proof source {relative} is not inspectable: {error}"));
        assert!(metadata.is_file() && !metadata.file_type().is_symlink());
        let mut source = File::open(&path)
            .unwrap_or_else(|error| panic!("proof source {relative} is not readable: {error}"));
        let mut bytes = Vec::new();
        source.read_to_end(&mut bytes).unwrap();
        json!({"path": relative, "sha256": format!("{:x}", Sha256::digest(bytes))})
    }

    fn emit_direct_cancellation_proof(observation: Value) {
        let Ok(destination) = std::env::var("TYPE_BRIDGE_SDK_V2_PROOF_FRAGMENT") else {
            return;
        };
        let nonce = std::env::var("TYPE_BRIDGE_SDK_V2_PROOF_RUN_NONCE")
            .expect("sdk-v2 proof fragment requires the same-run nonce");
        assert!(
            nonce.len() == 64
                && nonce
                    .bytes()
                    .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value)),
            "sdk-v2 proof run nonce must be 64 lowercase hex characters"
        );
        let destination = PathBuf::from(destination);
        assert!(
            destination.is_absolute(),
            "sdk-v2 proof fragment path must be absolute"
        );
        let parent = destination
            .parent()
            .expect("proof fragment path has no parent");
        let parent_metadata = parent
            .symlink_metadata()
            .expect("sdk-v2 proof fragment parent is not inspectable");
        assert!(parent_metadata.is_dir() && !parent_metadata.file_type().is_symlink());
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let root = manifest
            .ancestors()
            .nth(3)
            .expect("Python crate is not nested under the repository root");
        let contract = json!({
            "allowlist": source_identity(
                root,
                "tests/contracts/sdk_conformance/sdk-v2/proof-fragment-allowlist-v1.json",
            ),
            "journey": source_identity(
                root,
                "tests/contracts/sdk_conformance/sdk-v2/journey-v2.json",
            ),
            "proof_schema": source_identity(
                root,
                "tests/contracts/sdk_conformance/sdk-v2/proof-fragment-schema-v1.json",
            ),
        });
        let sources = [
            "type-bridge-core/crates/orm/src/match_request/selected_result_executor.rs",
            "type-bridge-core/crates/python/src/match_runtime.rs",
        ];
        let fragment = json!({
            "binding": "python",
            "contract": contract,
            "format": "typebridge.sdk-v2-proof-fragment/v1",
            "producer": {
                "id": "python.native_direct_cancellation",
                "sources": sources.map(|relative| source_identity(root, relative)),
            },
            "results": [{
                "observation": observation,
                "observation_ref": "cancellation_direct",
                "outcome": "passed",
                "proof_kind": "direct_runtime",
                "test_id": "python.native_direct_cancellation",
            }],
            "run_nonce": nonce,
            "semantic_profile": "typedb-3.12.1/v1",
        });
        let mut payload = serde_json::to_vec(&fragment).unwrap();
        payload.push(b'\n');
        assert!(payload.len() <= 64 * 1024);
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)
            .expect("sdk-v2 proof fragment destination must not exist");
        output.write_all(&payload).unwrap();
        output.sync_all().unwrap();
    }

    fn marshalled_category_code(error: OrmError) -> (String, String) {
        Python::initialize();
        let error = py_match_orm_error(error);
        Python::attach(|py| {
            let value = error.value(py);
            (
                value
                    .getattr("sdk_category")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                value.getattr("code").unwrap().extract::<String>().unwrap(),
            )
        })
    }

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn opaque_wrappers_are_narrow_and_thread_safe() {
        assert_eq!(
            size_of::<PyMatchBindingHandle>(),
            size_of::<BindingHandle>()
        );
        assert_eq!(size_of::<PyMatchFieldHandle>(), size_of::<FieldHandle>());
        assert_eq!(size_of::<PyMatchRoleHandle>(), size_of::<RoleHandle>());
        assert_eq!(
            size_of::<PyMatchPredicateHandle>(),
            size_of::<PredicateHandle>()
        );
        assert_eq!(size_of::<PyMatchOrderHandle>(), size_of::<OrderHandle>());
        assert_eq!(
            size_of::<PyMatchSelectionHandle>(),
            size_of::<SelectionHandle>()
        );
        assert_eq!(size_of::<PyMatchShapeHandle>(), size_of::<ShapeHandle>());
        assert!(size_of::<PyMatchSessionHandle>() <= 128);
        assert!(size_of::<PyMatchQueryHandle>() <= 128);

        assert_send_sync::<PyMatchSessionHandle>();
        assert_send_sync::<PyMatchBindingHandle>();
        assert_send_sync::<PyMatchFieldHandle>();
        assert_send_sync::<PyMatchRoleHandle>();
        assert_send_sync::<PyMatchPredicateHandle>();
        assert_send_sync::<PyMatchOrderHandle>();
        assert_send_sync::<PyMatchSelectionHandle>();
        assert_send_sync::<PyMatchShapeHandle>();
        assert_send_sync::<PyMatchQueryHandle>();
        assert_send_sync::<PyQueryExecutionResourceLimits>();
        assert_send_sync::<PyQueryCancellation>();
    }

    #[test]
    fn terminal_diagnostic_round_trips_only_through_revalidation() {
        let registry = registry();
        let session = SessionHandle::new(Arc::clone(&registry));
        let person = session.exact("person").unwrap();
        let name_order = person
            .field("name")
            .unwrap()
            .order(SortDirection::Ascending, MissingOrder::Reject);
        let shape = session.positional([person.one()]).unwrap();
        let query = session.query(shape).unwrap();
        let requests = [
            query.fetch_rows(
                &[],
                Window {
                    offset: 0,
                    limit: 1,
                },
                RowCardinality::ExactlyOne,
            ),
            query.page_by(
                &person,
                &[name_order],
                Window {
                    offset: 0,
                    limit: 10,
                },
                true,
            ),
            query.count_by(&person),
            query.exists_by(&person),
            query.reduce_by(&person, None, &[(Reduction::Count, None)]),
        ];

        for request in requests {
            let diagnostic = diagnostic_from_request(request).unwrap();
            assert!(!diagnostic.contains("request_token"));
            assert_eq!(
                revalidate_diagnostic(&registry, &diagnostic).unwrap(),
                diagnostic
            );
        }
        assert_eq!(
            diagnostic_from_request(query.count_by(&person)).unwrap(),
            include_str!("../../orm/tests/fixtures/match_request/single-count.json").trim()
        );
    }

    #[test]
    fn persistent_native_transitions_do_not_mutate_the_base_lineage() {
        let registry = registry();
        let session = SessionHandle::new(Arc::clone(&registry));
        let person = session.exact("person").unwrap();
        let name = person.field("name").unwrap();
        let shape = session.positional([person.one()]).unwrap();
        let base = session.query(shape).unwrap();
        let filtered = base
            .where_predicate(
                name.compare_value(ComparisonOp::Equal, AttributeValue::String("Alice".into())),
            )
            .unwrap();

        let base_diagnostic = diagnostic_from_request(base.count_by(&person)).unwrap();
        let filtered_diagnostic = diagnostic_from_request(filtered.count_by(&person)).unwrap();
        assert_ne!(base_diagnostic, filtered_diagnostic);
        assert_eq!(
            diagnostic_from_request(base.count_by(&person)).unwrap(),
            base_diagnostic
        );
    }

    #[test]
    fn query_and_session_close_preserve_persistent_handle_independence() {
        Python::initialize();
        let session = PyMatchSessionHandle::from_registry(registry());
        let person = session.inner.exact("person").unwrap();
        let shape = session.inner.positional([person.one()]).unwrap();
        let query = PyMatchQueryHandle {
            inner: session.inner.query(shape).unwrap(),
            installed: None,
            resources: session.resources,
            cancellation: session.cancellation.clone(),
            session_closed: Arc::clone(&session.closed),
            closed: Arc::new(AtomicBool::new(false)),
        };
        let clone = query.fork().unwrap();
        let derived = query.derived(
            query
                .inner
                .where_predicate(person.iid("0x1").unwrap())
                .unwrap(),
        );

        query.close();
        query.close();
        assert!(query.is_closed());
        assert!(!clone.is_closed());
        assert!(!derived.is_closed());
        assert!(query.fork().is_err());
        clone.fork().expect("independent clone remains composable");
        derived.close();
        assert!(!clone.is_closed());

        session.close();
        session.close();
        assert!(session.is_closed());
        assert!(clone.is_closed());
        assert!(clone.begin_invocation().is_err());
    }

    #[test]
    fn unknown_enum_spelling_is_a_structured_value_error() {
        Python::initialize();
        let error = parse_enum::<Reduction>("variance").unwrap_err();
        Python::attach(|py| {
            let value = error.value(py);
            assert!(value.is_instance_of::<MatchRequestError>());
            assert!(value.is_instance_of::<PyValueError>());
            assert_eq!(
                value.get_type().name().unwrap().to_string(),
                "UnknownEnumValueError"
            );
            let text = |name: &str| value.getattr(name).unwrap().extract::<String>().unwrap();
            assert_eq!(text("category"), "invalid_input");
            assert_eq!(text("code"), "unknown_enum_value");
            let details: Value =
                pythonize::depythonize(&value.getattr("details").unwrap()).unwrap();
            assert_eq!(
                details,
                json!({
                    "kind": {"kind": "query_identity", "value": "reducer"},
                    "value": {"kind": "query_identity", "value": "variance"},
                    "accepted": {
                        "kind": "query_identity_list",
                        "value": ["count", "sum", "min", "max", "mean", "median", "std"],
                    },
                })
            );
        });
    }

    #[test]
    fn rust_match_errors_remain_structured_before_python_marshalling() {
        let session = SessionHandle::new(registry());
        let OrmError::Match(error) = session.exact("missing").unwrap_err() else {
            panic!("expected typed match error")
        };
        assert_eq!(error.category().as_str(), "invalid_plan");
        assert_eq!(error.code().as_str(), "unknown_descriptor");
        assert!(error.path().is_empty());
        assert!(error.details().is_empty());
    }

    #[test]
    fn python_match_exception_carries_typed_error_attributes() {
        use pythonize::depythonize;

        let error = UnvalidatedMatchRequest::from_canonical_bytes(b"{}").unwrap_err();
        Python::initialize();
        let error = py_match_error(error);
        Python::attach(|py| {
            let value = error.value(py);
            assert!(value.is_instance_of::<MatchRequestError>());
            assert_eq!(
                value
                    .getattr("category")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "invalid_plan"
            );
            assert_eq!(
                value.getattr("code").unwrap().extract::<String>().unwrap(),
                "malformed_diagnostic"
            );
            let path: serde_json::Value = depythonize(&value.getattr("path").unwrap()).unwrap();
            let details: serde_json::Value =
                depythonize(&value.getattr("details").unwrap()).unwrap();
            assert_eq!(path, serde_json::json!([{"kind": "request"}]));
            assert_eq!(
                details,
                serde_json::json!({
                    "actual_bytes": {"kind": "byte_count", "value": "2"}
                })
            );
        });
    }

    #[test]
    fn python_marshalling_preserves_timeout_cancel_and_provider_diagnostics() {
        use pythonize::depythonize;

        let cancellation = AnswerCancellation::default();
        cancellation.cancel();
        let cancelled = BoundedAnswerReader::new(BoundedAnswerLimits {
            cancellation,
            ..BoundedAnswerLimits::default()
        })
        .check_before_read()
        .unwrap_err();
        let timed_out = BoundedAnswerReader::new(BoundedAnswerLimits {
            deadline: Instant::now().checked_sub(Duration::from_secs(1)),
            ..BoundedAnswerLimits::default()
        })
        .check_before_read()
        .unwrap_err();

        let registry = registry();
        let session = SessionHandle::new(Arc::clone(&registry));
        let person = session.exact("person").unwrap();
        let shape = session.positional([person.one()]).unwrap();
        let validated = session
            .query(shape)
            .unwrap()
            .validate_count_by(&person)
            .unwrap();
        let database = Database::with_backend(Box::new(ProviderOpenFailureBackend), "test");
        let provider = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(database.execute_match(&registry, &validated))
            .unwrap_err();

        Python::initialize();
        for (error, category, code) in [
            (cancelled, "cancelled", "provider_cancelled"),
            (timed_out, "resource_limit", "transaction_deadline_exceeded"),
            (provider, "provider", "provider_transaction_open_failed"),
        ] {
            let error = py_match_orm_error(error);
            Python::attach(|py| {
                let value = error.value(py);
                assert!(value.is_instance_of::<MatchRequestError>());
                assert_eq!(
                    value
                        .getattr("category")
                        .unwrap()
                        .extract::<String>()
                        .unwrap(),
                    category
                );
                assert_eq!(
                    value.getattr("code").unwrap().extract::<String>().unwrap(),
                    code
                );
                let path: serde_json::Value = depythonize(&value.getattr("path").unwrap()).unwrap();
                assert_eq!(path, serde_json::json!([{"kind": "provider_evidence"}]));
                assert!(
                    !value
                        .getattr("message")
                        .unwrap()
                        .extract::<String>()
                        .unwrap()
                        .contains("python-binding-secret")
                );
            });
        }
    }

    #[test]
    fn python_direct_cancellation_fragment_is_measured_from_owned_execution() {
        let registry = registry();
        let session = SessionHandle::new(Arc::clone(&registry));
        let person = session.exact("person").unwrap();
        let shape = session.positional([person.one()]).unwrap();
        let validated = session
            .query(shape)
            .unwrap()
            .validate_count_by(&person)
            .unwrap();

        let pre_dispatch_opens = Arc::new(AtomicUsize::new(0));
        let pre_dispatch_opened = Arc::new(Notify::new());
        let pre_dispatch_database = Database::with_backend(
            Box::new(BlockingOpenBackend {
                opens: Arc::clone(&pre_dispatch_opens),
                opened: pre_dispatch_opened,
            }),
            "test",
        );
        let pre_dispatch_cancellation = AnswerCancellation::default();
        pre_dispatch_cancellation.cancel();
        let pre_dispatch_limits = MatchExecutionLimits::tightened(
            u64::MAX,
            u64::MAX,
            Duration::from_secs(30),
            pre_dispatch_cancellation,
        );

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let pre_dispatch_error = runtime
            .block_on(pre_dispatch_database.execute_match_with_limits(
                &registry,
                &validated,
                pre_dispatch_limits,
            ))
            .unwrap_err();
        assert_eq!(pre_dispatch_opens.load(Ordering::SeqCst), 0);
        let (pre_dispatch_category, pre_dispatch_code) =
            marshalled_category_code(pre_dispatch_error);

        let in_flight_opens = Arc::new(AtomicUsize::new(0));
        let in_flight_opened = Arc::new(Notify::new());
        let in_flight_database = Database::with_backend(
            Box::new(BlockingOpenBackend {
                opens: Arc::clone(&in_flight_opens),
                opened: Arc::clone(&in_flight_opened),
            }),
            "test",
        );
        let in_flight_cancellation = AnswerCancellation::default();
        let in_flight_limits = MatchExecutionLimits::tightened(
            u64::MAX,
            u64::MAX,
            Duration::from_secs(30),
            in_flight_cancellation.clone(),
        );
        let in_flight_error = runtime.block_on(async {
            let opened = in_flight_opened.notified();
            tokio::pin!(opened);
            let execution = in_flight_database.execute_match_with_limits(
                &registry,
                &validated,
                in_flight_limits,
            );
            tokio::pin!(execution);
            tokio::select! {
                () = &mut opened => {}
                result = &mut execution => panic!("blocked provider returned before cancellation: {result:?}"),
            }
            assert_eq!(in_flight_opens.load(Ordering::SeqCst), 1);
            in_flight_cancellation.cancel();
            tokio::time::timeout(Duration::from_secs(1), execution)
                .await
                .expect("cancellation did not wake the in-flight provider await")
                .unwrap_err()
        });
        let (in_flight_category, in_flight_code) = marshalled_category_code(in_flight_error);

        let observation = json!({
            "in_flight": {
                "category": in_flight_category,
                "code": in_flight_code,
                "partial_result": false,
                "provider_await_woken": true,
            },
            "pre_dispatch": {
                "category": pre_dispatch_category,
                "code": pre_dispatch_code,
                "partial_result": false,
                "provider_calls": pre_dispatch_opens.load(Ordering::SeqCst),
            },
        });
        assert_eq!(
            observation,
            json!({
                "in_flight": {
                    "category": "cancelled",
                    "code": "provider_cancelled",
                    "partial_result": false,
                    "provider_await_woken": true,
                },
                "pre_dispatch": {
                    "category": "cancelled",
                    "code": "provider_cancelled",
                    "partial_result": false,
                    "provider_calls": 0,
                },
            })
        );
        emit_direct_cancellation_proof(observation);
    }

    #[test]
    fn registered_python_handles_expose_no_state_attributes() {
        Python::initialize();
        Python::attach(|py| {
            let module = PyModule::new(py, "type_bridge_core").unwrap();
            register(&module).unwrap();
            for name in [
                "MatchSessionHandle",
                "MatchBindingHandle",
                "MatchFieldHandle",
                "MatchRoleHandle",
                "MatchPredicateHandle",
                "MatchOrderHandle",
                "MatchSelectionHandle",
                "MatchShapeHandle",
                "MatchQueryHandle",
                "QueryExecutionResourceLimits",
                "QueryCancellation",
                "MatchRequestError",
            ] {
                assert!(
                    module.hasattr(name).unwrap(),
                    "missing native symbol {name}"
                );
            }

            let session = Py::new(py, PyMatchSessionHandle::from_registry(registry())).unwrap();
            let handle = session.bind(py);
            assert!(!handle.hasattr("plan").unwrap());
            assert!(!handle.hasattr("bindings").unwrap());
            assert!(!handle.hasattr("request_token").unwrap());
            assert!(handle.getattr("__dict__").is_err());
        });
    }

    #[test]
    fn public_resource_limits_keep_a_raise_refuse_past_the_maximum_and_preserve_zero() {
        Python::initialize();
        Python::attach(|py| {
            let module = PyModule::new(py, "type_bridge_core").unwrap();
            register(&module).unwrap();
            let globals = pyo3::types::PyDict::new(py);
            globals.set_item("m", &module).unwrap();
            py.run(
                pyo3::ffi::c_str!(
                    "plus = m.QueryExecutionResourceLimits(30001, 65537, 33554433, 65537, 65537, 65537, 65537, 3)\nzero = m.QueryExecutionResourceLimits(0, 0, 0, 0, 0, 0, 0, 0)\nrefused = []\nfor arguments in ({'timeout_milliseconds': 86400001}, {'statements': 4}):\n    try:\n        m.QueryExecutionResourceLimits(**arguments)\n    except ValueError as error:\n        refused.append(str(error))"
                ),
                Some(&globals),
                None,
            )
            .unwrap();
            let plus = globals.get_item("plus").unwrap().unwrap();
            let zero = globals.get_item("zero").unwrap().unwrap();
            let refused = globals
                .get_item("refused")
                .unwrap()
                .unwrap()
                .extract::<Vec<String>>()
                .unwrap();
            assert_eq!(refused.len(), 2, "{refused:?}");
            assert!(refused[0].contains("timeout_milliseconds"), "{refused:?}");
            assert!(refused[1].contains("statements"), "{refused:?}");
            for (name, expected) in [
                ("timeout_milliseconds", 30_001_u64),
                ("items", 65_537),
                ("bytes", 33_554_433),
                ("graph_nodes", 65_537),
                ("attribute_values", 65_537),
                ("collection_members", 65_537),
                ("role_players", 65_537),
                ("statements", 3),
            ] {
                assert_eq!(
                    plus.getattr(name).unwrap().extract::<u64>().unwrap(),
                    expected,
                    "{name} must keep what the caller raised it to"
                );
                assert_eq!(
                    zero.getattr(name).unwrap().extract::<u64>().unwrap(),
                    0,
                    "{name} must preserve zero tightening"
                );
            }
            let cancellation = module
                .getattr("QueryCancellation")
                .unwrap()
                .call0()
                .unwrap();
            assert!(
                !cancellation
                    .getattr("is_cancelled")
                    .unwrap()
                    .is_truthy()
                    .unwrap()
            );
            cancellation.call_method0("cancel").unwrap();
            assert!(
                cancellation
                    .getattr("is_cancelled")
                    .unwrap()
                    .is_truthy()
                    .unwrap()
            );
        });
    }
}
