//! Opaque PyO3 seam for the canonical typed-match handle contract.
//!
//! These classes deliberately expose construction transitions and read-only
//! canonical diagnostics only. Python never owns a match plan, binding map,
//! validated request, provider row, TypeQL string, or invocation token here.

use pyo3::exceptions::{PyException, PyRuntimeError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyInt};
use pythonize::pythonize;
use type_bridge_orm::{
    BindingHandle, ComparisonOp, FieldHandle, MatchError, MissingOrder, OrderHandle, OrmError,
    PredicateHandle, QueryHandle, Reduction, RoleHandle, RowCardinality, SelectionHandle,
    SessionHandle, ShapeHandle, SortDirection, UnvalidatedMatchRequest, ValidatedMatchRequest,
    Window, validate_public_order_term_count,
};

use crate::orm_runtime::{
    PyDescriptorRegistry, PyDynamicValue, PyRustDatabase, PyRustTransactionContext,
    provider_block_on,
};
use crate::validated_result_runtime::PyValidatedMatchResultHandle;

pyo3::create_exception!(
    type_bridge_core,
    MatchRequestError,
    PyException,
    "Structured canonical match-request validation or lineage failure."
);

#[pyclass(name = "MatchSessionHandle", frozen, from_py_object)]
#[derive(Clone)]
struct PyMatchSessionHandle {
    inner: SessionHandle,
}

#[pyclass(name = "MatchBindingHandle", frozen, from_py_object)]
#[derive(Clone)]
pub(crate) struct PyMatchBindingHandle {
    inner: BindingHandle,
}

#[pyclass(name = "MatchFieldHandle", frozen, from_py_object)]
#[derive(Clone)]
struct PyMatchFieldHandle {
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
#[derive(Clone)]
pub(crate) struct PyMatchQueryHandle {
    inner: QueryHandle,
}

impl PyMatchBindingHandle {
    pub(crate) const fn inner(&self) -> &BindingHandle {
        &self.inner
    }
}

impl PyMatchQueryHandle {
    pub(crate) const fn inner(&self) -> &QueryHandle {
        &self.inner
    }
}

#[pymethods]
impl PyMatchSessionHandle {
    #[new]
    fn new(registry: PyRef<'_, PyDescriptorRegistry>) -> Self {
        Self {
            inner: SessionHandle::new(registry.registry_arc()),
        }
    }

    fn exact(&self, type_name: &str) -> PyResult<PyMatchBindingHandle> {
        self.inner
            .exact(type_name)
            .map(|inner| PyMatchBindingHandle { inner })
            .map_err(py_match_orm_error)
    }

    fn subtypes(&self, type_name: &str) -> PyResult<PyMatchBindingHandle> {
        self.inner
            .subtypes(type_name)
            .map(|inner| PyMatchBindingHandle { inner })
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
        self.inner
            .query(shape.inner.clone())
            .map(|inner| PyMatchQueryHandle { inner })
            .map_err(py_match_orm_error)
    }
}

fn python_reachability_depth(value: &Bound<'_, PyAny>, name: &str) -> PyResult<u8> {
    let value = value
        .cast_exact::<PyInt>()
        .map_err(|_| PyTypeError::new_err(format!("{name} must be an exact Python int")))?
        .extract::<i128>()
        .map_err(|_| {
            PyValueError::new_err(format!("{name} must be an integer between 0 and 255"))
        })?;
    u8::try_from(value)
        .map_err(|_| PyValueError::new_err(format!("{name} must be an integer between 0 and 255")))
}

#[pymethods]
impl PyMatchBindingHandle {
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
    fn compare_value(
        &self,
        operator: &str,
        value: PyRef<'_, PyDynamicValue>,
    ) -> PyResult<PyMatchPredicateHandle> {
        Ok(PyMatchPredicateHandle {
            inner: self
                .inner
                .compare_value(parse_comparison(operator)?, value.attribute_value()),
        })
    }

    fn compare_field(
        &self,
        operator: &str,
        other: PyRef<'_, PyMatchFieldHandle>,
    ) -> PyResult<PyMatchPredicateHandle> {
        self.inner
            .compare_field(parse_comparison(operator)?, &other.inner)
            .map(|inner| PyMatchPredicateHandle { inner })
            .map_err(py_match_orm_error)
    }

    fn order(&self, direction: &str, missing: &str) -> PyResult<PyMatchOrderHandle> {
        Ok(PyMatchOrderHandle {
            inner: self.inner.order(
                parse_sort_direction(direction)?,
                parse_missing_order(missing)?,
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
    fn add_hidden(&self, binding: PyRef<'_, PyMatchBindingHandle>) -> PyResult<PyMatchQueryHandle> {
        self.inner
            .add_hidden(binding.inner.clone())
            .map(|inner| PyMatchQueryHandle { inner })
            .map_err(py_match_orm_error)
    }

    fn where_predicate(
        &self,
        predicate: PyRef<'_, PyMatchPredicateHandle>,
    ) -> PyResult<PyMatchQueryHandle> {
        self.inner
            .where_predicate(predicate.inner.clone())
            .map(|inner| PyMatchQueryHandle { inner })
            .map_err(py_match_orm_error)
    }

    fn allow_cross_join(
        &self,
        left: PyRef<'_, PyMatchBindingHandle>,
        right: PyRef<'_, PyMatchBindingHandle>,
    ) -> PyResult<PyMatchQueryHandle> {
        self.inner
            .allow_cross_join(&left.inner, &right.inner)
            .map(|inner| PyMatchQueryHandle { inner })
            .map_err(py_match_orm_error)
    }

    fn fetch_rows_diagnostic(
        &self,
        py: Python<'_>,
        order: Vec<Py<PyMatchOrderHandle>>,
        offset: u64,
        limit: u64,
        cardinality: &str,
    ) -> PyResult<String> {
        let order = order_handles(py, &order);
        let request = self.inner.fetch_rows(
            &order,
            Window { offset, limit },
            parse_cardinality(cardinality)?,
        );
        diagnostic_from_request(request)
    }

    fn page_by_diagnostic(
        &self,
        py: Python<'_>,
        root: PyRef<'_, PyMatchBindingHandle>,
        order: Vec<Py<PyMatchOrderHandle>>,
        offset: u64,
        limit: u64,
        include_total: bool,
    ) -> PyResult<String> {
        let order = order_handles(py, &order);
        diagnostic_from_request(self.inner.page_by(
            &root.inner,
            &order,
            Window { offset, limit },
            include_total,
        ))
    }

    fn count_by_diagnostic(&self, root: PyRef<'_, PyMatchBindingHandle>) -> PyResult<String> {
        diagnostic_from_request(self.inner.count_by(&root.inner))
    }

    fn exists_by_diagnostic(&self, root: PyRef<'_, PyMatchBindingHandle>) -> PyResult<String> {
        diagnostic_from_request(self.inner.exists_by(&root.inner))
    }

    fn execute_fetch_rows_owned(
        &self,
        py: Python<'_>,
        database: &PyRustDatabase,
        order: Vec<Py<PyMatchOrderHandle>>,
        offset: u64,
        limit: u64,
        cardinality: &str,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let orders = order_handles(py, &order);
        let validated = self
            .inner
            .validate_fetch_rows(
                &orders,
                Window { offset, limit },
                parse_cardinality(cardinality)?,
            )
            .map_err(py_match_orm_error)?;
        let registry = self.inner.registry_arc();
        execute_validated_owned(py, database, validated, registry)
    }

    fn execute_fetch_rows_borrowed(
        &self,
        py: Python<'_>,
        transaction: &PyRustTransactionContext,
        order: Vec<Py<PyMatchOrderHandle>>,
        offset: u64,
        limit: u64,
        cardinality: &str,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let orders = order_handles(py, &order);
        let validated = self
            .inner
            .validate_fetch_rows(
                &orders,
                Window { offset, limit },
                parse_cardinality(cardinality)?,
            )
            .map_err(py_match_orm_error)?;
        let registry = self.inner.registry_arc();
        execute_validated_borrowed(py, transaction, validated, registry)
    }

    // PyO3 exposes these operation-local arguments as the stable Python terminal contract.
    #[allow(clippy::too_many_arguments)]
    fn execute_page_by_owned(
        &self,
        py: Python<'_>,
        database: &PyRustDatabase,
        root: PyRef<'_, PyMatchBindingHandle>,
        order: Vec<Py<PyMatchOrderHandle>>,
        offset: u64,
        limit: u64,
        include_total: bool,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let orders = order_handles(py, &order);
        let validated = self
            .inner
            .validate_page_by(
                &root.inner,
                &orders,
                Window { offset, limit },
                include_total,
            )
            .map_err(py_match_orm_error)?;
        execute_validated_owned(py, database, validated, self.inner.registry_arc())
    }

    // PyO3 exposes these operation-local arguments as the stable Python terminal contract.
    #[allow(clippy::too_many_arguments)]
    fn execute_page_by_borrowed(
        &self,
        py: Python<'_>,
        transaction: &PyRustTransactionContext,
        root: PyRef<'_, PyMatchBindingHandle>,
        order: Vec<Py<PyMatchOrderHandle>>,
        offset: u64,
        limit: u64,
        include_total: bool,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let orders = order_handles(py, &order);
        let validated = self
            .inner
            .validate_page_by(
                &root.inner,
                &orders,
                Window { offset, limit },
                include_total,
            )
            .map_err(py_match_orm_error)?;
        execute_validated_borrowed(py, transaction, validated, self.inner.registry_arc())
    }

    fn execute_count_by_owned(
        &self,
        py: Python<'_>,
        database: &PyRustDatabase,
        root: PyRef<'_, PyMatchBindingHandle>,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let validated = self
            .inner
            .validate_count_by(&root.inner)
            .map_err(py_match_orm_error)?;
        execute_validated_owned(py, database, validated, self.inner.registry_arc())
    }

    fn execute_count_by_borrowed(
        &self,
        py: Python<'_>,
        transaction: &PyRustTransactionContext,
        root: PyRef<'_, PyMatchBindingHandle>,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let validated = self
            .inner
            .validate_count_by(&root.inner)
            .map_err(py_match_orm_error)?;
        execute_validated_borrowed(py, transaction, validated, self.inner.registry_arc())
    }

    fn execute_exists_by_owned(
        &self,
        py: Python<'_>,
        database: &PyRustDatabase,
        root: PyRef<'_, PyMatchBindingHandle>,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let validated = self
            .inner
            .validate_exists_by(&root.inner)
            .map_err(py_match_orm_error)?;
        execute_validated_owned(py, database, validated, self.inner.registry_arc())
    }

    fn execute_exists_by_borrowed(
        &self,
        py: Python<'_>,
        transaction: &PyRustTransactionContext,
        root: PyRef<'_, PyMatchBindingHandle>,
    ) -> PyResult<PyValidatedMatchResultHandle> {
        let validated = self
            .inner
            .validate_exists_by(&root.inner)
            .map_err(py_match_orm_error)?;
        execute_validated_borrowed(py, transaction, validated, self.inner.registry_arc())
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
        let terms = reduce_terms(py, &reducers, &inputs)?;
        let terms = borrow_reduce_terms(&terms);
        let validated = self
            .inner
            .validate_reduce_by(
                &root.inner,
                group.as_ref().map(|group| &group.inner),
                &terms,
            )
            .map_err(py_match_orm_error)?;
        execute_validated_owned(py, database, validated, self.inner.registry_arc())
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
        let terms = reduce_terms(py, &reducers, &inputs)?;
        let terms = borrow_reduce_terms(&terms);
        let validated = self
            .inner
            .validate_reduce_by(
                &root.inner,
                group.as_ref().map(|group| &group.inner),
                &terms,
            )
            .map_err(py_match_orm_error)?;
        execute_validated_borrowed(py, transaction, validated, self.inner.registry_arc())
    }
}

fn reduce_terms(
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
                parse_reduction(reducer)?,
                input.as_ref().map(|field| field.borrow(py).inner.clone()),
            ))
        })
        .collect()
}

fn borrow_reduce_terms(
    terms: &[(Reduction, Option<FieldHandle>)],
) -> Vec<(Reduction, Option<&FieldHandle>)> {
    terms
        .iter()
        .map(|(reduction, input)| (*reduction, input.as_ref()))
        .collect()
}

fn execute_validated_owned(
    py: Python<'_>,
    database: &PyRustDatabase,
    validated: ValidatedMatchRequest,
    registry: std::sync::Arc<type_bridge_orm::DescriptorRegistry>,
) -> PyResult<PyValidatedMatchResultHandle> {
    let (database, runtime) = database.handles();
    let result = provider_block_on(
        py,
        runtime.as_ref(),
        database.execute_match(&registry, &validated),
    )
    .map_err(py_match_orm_error)?;
    Ok(PyValidatedMatchResultHandle::new(
        validated, result, registry,
    ))
}

fn execute_validated_borrowed(
    py: Python<'_>,
    transaction: &PyRustTransactionContext,
    validated: ValidatedMatchRequest,
    registry: std::sync::Arc<type_bridge_orm::DescriptorRegistry>,
) -> PyResult<PyValidatedMatchResultHandle> {
    let (transaction, runtime) = transaction.handles();
    let result = provider_block_on(
        py,
        runtime.as_ref(),
        transaction.execute_match(&registry, &validated),
    )
    .map_err(py_match_orm_error)?;
    Ok(PyValidatedMatchResultHandle::new(
        validated, result, registry,
    ))
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

fn revalidate_diagnostic(
    registry: &type_bridge_orm::DescriptorRegistry,
    diagnostic: &str,
) -> Result<String, MatchError> {
    let unvalidated = UnvalidatedMatchRequest::from_canonical_bytes(diagnostic.as_bytes())?;
    let validated = unvalidated.validate(registry)?;
    validated.recheck_schema(registry)?;
    Ok(diagnostic.to_owned())
}

#[pyfunction]
fn revalidate_match_diagnostic(
    registry: PyRef<'_, PyDescriptorRegistry>,
    diagnostic: &str,
) -> PyResult<String> {
    let registry = registry.registry_arc();
    revalidate_diagnostic(&registry, diagnostic).map_err(py_match_error)
}

fn parse_comparison(value: &str) -> PyResult<ComparisonOp> {
    match value {
        "equal" => Ok(ComparisonOp::Equal),
        "not_equal" => Ok(ComparisonOp::NotEqual),
        "less_than" => Ok(ComparisonOp::LessThan),
        "less_than_or_equal" => Ok(ComparisonOp::LessThanOrEqual),
        "greater_than" => Ok(ComparisonOp::GreaterThan),
        "greater_than_or_equal" => Ok(ComparisonOp::GreaterThanOrEqual),
        "contains" => Ok(ComparisonOp::Contains),
        "starts_with" => Ok(ComparisonOp::StartsWith),
        "ends_with" => Ok(ComparisonOp::EndsWith),
        "regex" => Ok(ComparisonOp::Regex),
        _ => Err(PyValueError::new_err(format!(
            "unknown comparison operator {value:?}"
        ))),
    }
}

fn parse_sort_direction(value: &str) -> PyResult<SortDirection> {
    match value {
        "ascending" => Ok(SortDirection::Ascending),
        "descending" => Ok(SortDirection::Descending),
        _ => Err(PyValueError::new_err(format!(
            "unknown sort direction {value:?}"
        ))),
    }
}

fn parse_missing_order(value: &str) -> PyResult<MissingOrder> {
    match value {
        "reject" => Ok(MissingOrder::Reject),
        "first" => Ok(MissingOrder::First),
        "last" => Ok(MissingOrder::Last),
        _ => Err(PyValueError::new_err(format!(
            "unknown missing-value order {value:?}"
        ))),
    }
}

pub(crate) fn parse_cardinality(value: &str) -> PyResult<RowCardinality> {
    match value {
        "exactly_one" => Ok(RowCardinality::ExactlyOne),
        "bounded_many" => Ok(RowCardinality::BoundedMany),
        _ => Err(PyValueError::new_err(format!(
            "unknown row cardinality {value:?}"
        ))),
    }
}

fn parse_reduction(value: &str) -> PyResult<Reduction> {
    match value {
        "count" => Ok(Reduction::Count),
        "sum" => Ok(Reduction::Sum),
        "min" => Ok(Reduction::Min),
        "max" => Ok(Reduction::Max),
        "mean" => Ok(Reduction::Mean),
        "median" => Ok(Reduction::Median),
        "std" => Ok(Reduction::Std),
        _ => Err(PyValueError::new_err(format!("unknown reducer {value:?}"))),
    }
}

pub(crate) fn py_match_orm_error(error: OrmError) -> PyErr {
    match error {
        OrmError::Match(error) => py_match_error(error),
        other => PyRuntimeError::new_err(other.to_string()),
    }
}

pub(crate) fn py_match_error(error: MatchError) -> PyErr {
    Python::attach(|py| {
        let py_error = MatchRequestError::new_err(error.message().to_owned());
        let attach = || -> PyResult<()> {
            let value = py_error.value(py);
            value.setattr("category", error.category().as_str())?;
            value.setattr("code", error.code().as_str())?;
            value.setattr("message", error.message())?;
            value.setattr("path", pythonize(py, error.path().segments())?)?;
            value.setattr("details", pythonize(py, error.details())?)?;
            Ok(())
        };
        match attach() {
            Ok(()) => py_error,
            Err(attribute_error) => attribute_error,
        }
    })
}

/// Apply the canonical public-order ceiling before a Python iterable is
/// materialized into native order handles.
#[pyfunction]
pub(crate) fn validate_match_order_term_count(actual: usize) -> PyResult<()> {
    validate_public_order_term_count(actual).map_err(py_match_error)
}

/// Register the native typed-match handle seam on the extension module.
pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add(
        "MatchRequestError",
        module.py().get_type::<MatchRequestError>(),
    )?;
    module.add_class::<PyMatchSessionHandle>()?;
    module.add_class::<PyMatchBindingHandle>()?;
    module.add_class::<PyMatchFieldHandle>()?;
    module.add_class::<PyMatchRoleHandle>()?;
    module.add_class::<PyMatchPredicateHandle>()?;
    module.add_class::<PyMatchOrderHandle>()?;
    module.add_class::<PyMatchSelectionHandle>()?;
    module.add_class::<PyMatchShapeHandle>()?;
    module.add_class::<PyMatchQueryHandle>()?;
    crate::validated_result_runtime::register(module)?;
    module.add_function(wrap_pyfunction!(revalidate_match_diagnostic, module)?)?;
    module.add_function(wrap_pyfunction!(validate_match_order_term_count, module)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use type_bridge_orm::session::backend::{
        AnswerCancellation, BoundedAnswerLimits, BoundedAnswerReader, BoxFuture, DriverBackend,
        TransactionOps, TxType,
    };
    use type_bridge_orm::{
        Annotation, AttributeValue, CapabilitySet, Database, DescriptorRegistry, EntityDescriptor,
        OwnedAttributeDescriptor, ValueType,
    };

    use super::*;

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

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn wrappers_are_one_handle_wide_and_thread_safe() {
        assert_eq!(
            size_of::<PyMatchSessionHandle>(),
            size_of::<SessionHandle>()
        );
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
        assert_eq!(size_of::<PyMatchQueryHandle>(), size_of::<QueryHandle>());

        assert_send_sync::<PyMatchSessionHandle>();
        assert_send_sync::<PyMatchBindingHandle>();
        assert_send_sync::<PyMatchFieldHandle>();
        assert_send_sync::<PyMatchRoleHandle>();
        assert_send_sync::<PyMatchPredicateHandle>();
        assert_send_sync::<PyMatchOrderHandle>();
        assert_send_sync::<PyMatchSelectionHandle>();
        assert_send_sync::<PyMatchShapeHandle>();
        assert_send_sync::<PyMatchQueryHandle>();
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
    fn reducer_names_parse_to_the_canonical_closed_vocabulary() {
        for (name, expected) in [
            ("count", Reduction::Count),
            ("sum", Reduction::Sum),
            ("min", Reduction::Min),
            ("max", Reduction::Max),
            ("mean", Reduction::Mean),
            ("median", Reduction::Median),
            ("std", Reduction::Std),
        ] {
            assert_eq!(parse_reduction(name).unwrap(), expected);
        }
        assert!(parse_reduction("variance").is_err());
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
                    "actual_bytes": {"kind": "unsigned", "value": 2}
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
            (cancelled, "resource_limit", "provider_cancelled"),
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
                "MatchRequestError",
                "revalidate_match_diagnostic",
            ] {
                assert!(
                    module.hasattr(name).unwrap(),
                    "missing native symbol {name}"
                );
            }

            let session = Py::new(
                py,
                PyMatchSessionHandle {
                    inner: SessionHandle::new(registry()),
                },
            )
            .unwrap();
            let handle = session.bind(py);
            assert!(!handle.hasattr("plan").unwrap());
            assert!(!handle.hasattr("bindings").unwrap());
            assert!(!handle.hasattr("request_token").unwrap());
            assert!(handle.getattr("__dict__").is_err());
        });
    }
}
