//! Opaque PyO3 projection of the shared V2 plan-authoring state machine.
//!
//! This module owns only Python primitive/handle conversion. Every state
//! transition, semantic check, canonical byte, fingerprint, and capability
//! remains owned by `type-bridge-orm`.

use std::thread::{self, ThreadId};

use pyo3::exceptions::{PyOverflowError, PyTypeError};
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBool, PyByteArray, PyBytes, PyFloat, PyInt, PyString, PyTuple};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::id::{
    AttributeId, FunctionId, MAX_LABEL_BYTES, RoleId, TypeId, TypeKind,
};
use type_bridge_contract::limits::{
    MAX_BINDINGS, MAX_BOOLEAN_TERMS, MAX_CANONICAL_STRING_BYTES, MAX_INPUT_BYTES, MAX_INPUT_ROWS,
    MAX_ORDER_TERMS, MAX_OUTPUT_NAME_BYTES, MAX_SELECTED_SLOTS,
};
use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};
use type_bridge_orm::query_v2_builder::{
    AuthoredQueryInvocation, AuthoredQueryPlan, QueryAuthorityIdentity, QueryBindingHandle,
    QueryBuilderScalarInput, QueryDocumentFieldHandle, QueryInputHandle, QueryLocalFunctionHandle,
    QueryLocalReturnHandle, QueryOperandHandle, QueryOrderHandle, QueryPatternHandle,
    QueryPlanBuilder, QueryReduceAssignmentHandle, query_builder_binding_limit_error,
    query_builder_boolean_host_type_error, query_builder_comparator, query_builder_depth,
    query_builder_depth_error, query_builder_disjunction_term_limit_error,
    query_builder_document_output_limit_error, query_builder_function_argument_limit_error,
    query_builder_function_target, query_builder_host_collection_type_error,
    query_builder_invocation_input_byte_limit_error, query_builder_invocation_row_arity_error,
    query_builder_invocation_row_limit_error, query_builder_local_binding_limit_error,
    query_builder_local_body_limit_error, query_builder_local_parameters,
    query_builder_negation_term_limit_error, query_builder_order_direction,
    query_builder_reduce_term_limit_error, query_builder_reducer,
    query_builder_role_player_limit_error, query_builder_role_players,
    query_builder_root_pattern_limit_error, query_builder_row_output_limit_error,
    query_builder_scalar, query_builder_scalar_host_type_error,
    query_builder_scalar_integer_range_error, query_builder_scalar_unicode_error,
    query_builder_sort_term_limit_error, query_builder_try_term_limit_error,
    query_builder_type_kind, query_builder_unsigned, query_builder_unsigned_error,
    query_builder_value_type,
};

use crate::query_v2_runtime::{PyQueryV2Authority, PythonString, value_error};
use type_bridge_orm::query_v2_prepared::query_v2_host_string_type_error;

fn diagnostic<T>(result: Result<T, Diagnostic>) -> PyResult<T> {
    result.map_err(|diagnostic| value_error(&diagnostic))
}

fn python_host_string(value: &Bound<'_, PyAny>, limit: usize) -> PyResult<String> {
    PythonString::extract(value, "text")
        .map_err(|_| value_error(&query_v2_host_string_type_error()))?
        .bounded_snapshot(limit)
}

macro_rules! define_python_host_string {
    ($name:ident, $limit:expr) => {
        struct $name(String);

        #[allow(dead_code)]
        impl $name {
            fn into_inner(self) -> String {
                self.0
            }

            fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl<'py> FromPyObject<'py> for $name {
            fn extract_bound(value: &Bound<'py, PyAny>) -> PyResult<Self> {
                python_host_string(value, $limit).map(Self)
            }
        }
    };
}

define_python_host_string!(PythonVariableString, MAX_OUTPUT_NAME_BYTES);
define_python_host_string!(PythonLabelString, MAX_LABEL_BYTES);
define_python_host_string!(PythonVocabularyString, MAX_OUTPUT_NAME_BYTES);

fn python_scalar(value_type: ValueTypeTag, value: &Bound<'_, PyAny>) -> PyResult<CanonicalValue> {
    let input = match value_type {
        ValueTypeTag::String
        | ValueTypeTag::Date
        | ValueTypeTag::DateTime
        | ValueTypeTag::DateTimeTz
        | ValueTypeTag::Decimal
        | ValueTypeTag::Duration => {
            let value = PythonString::extract(value, "value")
                .map_err(|_| value_error(&query_builder_scalar_host_type_error()))?
                .bounded_snapshot_with_unicode_error(
                    MAX_CANONICAL_STRING_BYTES,
                    query_builder_scalar_unicode_error,
                )?;
            QueryBuilderScalarInput::Text(value)
        }
        ValueTypeTag::Long => {
            let integer = value
                .downcast_exact::<PyInt>()
                .map_err(|_| value_error(&query_builder_scalar_host_type_error()))?;
            let integer = integer
                .extract::<i64>()
                .map_err(|_| value_error(&query_builder_scalar_integer_range_error()))?;
            QueryBuilderScalarInput::Long(integer)
        }
        ValueTypeTag::Double => {
            let number = value
                .downcast_exact::<PyFloat>()
                .map_err(|_| value_error(&query_builder_scalar_host_type_error()))?;
            QueryBuilderScalarInput::Double(number.extract::<f64>()?)
        }
        ValueTypeTag::Boolean => {
            let boolean = value
                .downcast_exact::<PyBool>()
                .map_err(|_| value_error(&query_builder_scalar_host_type_error()))?;
            QueryBuilderScalarInput::Boolean(boolean.extract::<bool>()?)
        }
    };
    diagnostic(query_builder_scalar(value_type, input))
}

fn python_boolean(value: &Bound<'_, PyAny>) -> PyResult<bool> {
    value
        .downcast_exact::<PyBool>()
        .map_err(|_| value_error(&query_builder_boolean_host_type_error()))?
        .extract::<bool>()
        .map_err(|_| value_error(&query_builder_boolean_host_type_error()))
}

fn python_integer(value: &Bound<'_, PyAny>, error: fn() -> Diagnostic) -> PyResult<i128> {
    value
        .downcast_exact::<PyInt>()
        .map_err(|_| value_error(&error()))?
        .extract::<i128>()
        .map_err(|_| value_error(&error()))
}

fn python_depth(value: &Bound<'_, PyAny>) -> PyResult<u8> {
    diagnostic(query_builder_depth(python_integer(
        value,
        query_builder_depth_error,
    )?))
}

fn python_unsigned(value: &Bound<'_, PyAny>) -> PyResult<u64> {
    diagnostic(query_builder_unsigned(python_integer(
        value,
        query_builder_unsigned_error,
    )?))
}

fn sequence_length(
    value: &Bound<'_, PyAny>,
    limit: usize,
    oversized: fn() -> Diagnostic,
) -> PyResult<usize> {
    if value.is_instance_of::<PyString>()
        || value.is_instance_of::<PyBytes>()
        || value.is_instance_of::<PyByteArray>()
        // SAFETY: `value` is a live Python object and the GIL is held for the
        // duration of every native authoring call.
        || unsafe { ffi::PySequence_Check(value.as_ptr()) } == 0
    {
        return Err(value_error(&query_builder_host_collection_type_error()));
    }

    let length = match value.len() {
        Ok(length) => length,
        Err(error) if error.is_instance_of::<PyOverflowError>(value.py()) => {
            return Err(value_error(&oversized()));
        }
        Err(error) if error.is_instance_of::<PyTypeError>(value.py()) => {
            return Err(value_error(&query_builder_host_collection_type_error()));
        }
        Err(error) => return Err(error),
    };
    if length > limit {
        return Err(value_error(&oversized()));
    }
    Ok(length)
}

fn sequence_items_with_length<'py>(
    value: &Bound<'py, PyAny>,
    length: usize,
) -> PyResult<Vec<Bound<'py, PyAny>>> {
    (0..length).map(|index| value.get_item(index)).collect()
}

fn sequence_items<'py>(
    value: &Bound<'py, PyAny>,
    limit: usize,
    oversized: fn() -> Diagnostic,
) -> PyResult<Vec<Bound<'py, PyAny>>> {
    let length = sequence_length(value, limit, oversized)?;
    sequence_items_with_length(value, length)
}

fn binding_handles(
    py: Python<'_>,
    handles: &Bound<'_, PyAny>,
    limit: usize,
    oversized: fn() -> Diagnostic,
) -> PyResult<Vec<QueryBindingHandle>> {
    sequence_items(handles, limit, oversized)?
        .into_iter()
        .map(|handle| {
            handle
                .extract::<Py<PyQueryV2BindingHandle>>()
                .map(|handle| handle.borrow(py).inner.clone())
        })
        .collect()
}

fn operand_handles(
    py: Python<'_>,
    handles: &Bound<'_, PyAny>,
    limit: usize,
    oversized: fn() -> Diagnostic,
) -> PyResult<Vec<QueryOperandHandle>> {
    sequence_items(handles, limit, oversized)?
        .into_iter()
        .map(|handle| {
            handle
                .extract::<Py<PyQueryV2OperandHandle>>()
                .map(|handle| handle.borrow(py).inner.clone())
        })
        .collect()
}

fn pattern_handles(
    py: Python<'_>,
    handles: &Bound<'_, PyAny>,
    limit: usize,
    oversized: fn() -> Diagnostic,
) -> PyResult<Vec<QueryPatternHandle>> {
    sequence_items(handles, limit, oversized)?
        .into_iter()
        .map(|handle| {
            handle
                .extract::<Py<PyQueryV2PatternHandle>>()
                .map(|handle| handle.borrow(py).inner.clone())
        })
        .collect()
}

fn pattern_branches(
    py: Python<'_>,
    branches: &Bound<'_, PyAny>,
) -> PyResult<Vec<Vec<QueryPatternHandle>>> {
    sequence_items(
        branches,
        MAX_BOOLEAN_TERMS,
        query_builder_disjunction_term_limit_error,
    )?
    .into_iter()
    .map(|branch| {
        pattern_handles(
            py,
            &branch,
            MAX_BOOLEAN_TERMS,
            query_builder_disjunction_term_limit_error,
        )
    })
    .collect()
}

fn invocation_rows(
    plan: &AuthoredQueryPlan,
    rows: &Bound<'_, PyAny>,
) -> PyResult<Vec<Vec<Option<CanonicalValue>>>> {
    let columns = plan.input_columns();
    let row_count = sequence_length(
        rows,
        MAX_INPUT_ROWS,
        query_builder_invocation_row_limit_error,
    )?;
    if columns.is_empty() {
        return Ok(vec![Vec::new(); row_count]);
    }
    let mut converted = Vec::with_capacity(row_count);
    let mut input_bytes = 2usize;
    for row_index in 0..row_count {
        let row = rows.get_item(row_index)?;
        let value_count =
            sequence_length(&row, MAX_BINDINGS, query_builder_invocation_row_arity_error)?;
        if value_count != columns.len() {
            return Err(value_error(&query_builder_invocation_row_arity_error()));
        }
        let values = sequence_items_with_length(&row, value_count)?;
        input_bytes = input_bytes
            .saturating_add(usize::from(row_index > 0))
            .saturating_add(2);
        if input_bytes > MAX_INPUT_BYTES {
            return Err(value_error(
                &query_builder_invocation_input_byte_limit_error(),
            ));
        }
        let mut converted_row = Vec::with_capacity(values.len().min(columns.len()));
        for (column_index, (value, column)) in values.into_iter().zip(columns).enumerate() {
            let value = if value.is_none() {
                None
            } else {
                Some(python_scalar(column.value_type(), &value)?)
            };
            let encoded = serde_json::to_vec(&value)
                .map_err(|_| value_error(&query_builder_invocation_input_byte_limit_error()))?;
            input_bytes = input_bytes
                .saturating_add(usize::from(column_index > 0))
                .saturating_add(encoded.len());
            if input_bytes > MAX_INPUT_BYTES {
                return Err(value_error(
                    &query_builder_invocation_input_byte_limit_error(),
                ));
            }
            converted_row.push(value);
        }
        converted.push(converted_row);
    }
    Ok(converted)
}

/// Opaque authority identity carried by plans and invocations.
#[pyclass(name = "_QueryV2AuthorityIdentity", frozen)]
pub struct PyQueryV2AuthorityIdentity {
    inner: QueryAuthorityIdentity,
}

#[pymethods]
impl PyQueryV2AuthorityIdentity {
    fn same_authority(&self, other: &Self) -> bool {
        self.inner == other.inner
    }

    fn __repr__(&self) -> &'static str {
        "_QueryV2AuthorityIdentity([OPAQUE])"
    }
}

/// Opaque builder-owned binding handle.
#[pyclass(name = "_QueryV2BindingHandle", frozen)]
pub struct PyQueryV2BindingHandle {
    inner: QueryBindingHandle,
}

/// Opaque builder-owned input-column handle.
#[pyclass(name = "_QueryV2InputHandle", frozen)]
pub struct PyQueryV2InputHandle {
    inner: QueryInputHandle,
}

/// Opaque builder-owned operand handle.
#[pyclass(name = "_QueryV2OperandHandle", frozen)]
pub struct PyQueryV2OperandHandle {
    inner: QueryOperandHandle,
}

/// Opaque builder-owned pattern handle.
#[pyclass(name = "_QueryV2PatternHandle", frozen)]
pub struct PyQueryV2PatternHandle {
    inner: QueryPatternHandle,
}

/// Opaque builder-owned order-term handle.
#[pyclass(name = "_QueryV2OrderHandle", frozen)]
pub struct PyQueryV2OrderHandle {
    inner: QueryOrderHandle,
}

/// Opaque builder-owned reducer-assignment handle.
#[pyclass(name = "_QueryV2ReduceAssignmentHandle", frozen)]
pub struct PyQueryV2ReduceAssignmentHandle {
    inner: QueryReduceAssignmentHandle,
}

/// Opaque builder-owned local-return handle.
#[pyclass(name = "_QueryV2LocalReturnHandle", frozen)]
pub struct PyQueryV2LocalReturnHandle {
    inner: QueryLocalReturnHandle,
}

/// Opaque builder-owned local-function handle.
#[pyclass(name = "_QueryV2LocalFunctionHandle", frozen)]
pub struct PyQueryV2LocalFunctionHandle {
    inner: QueryLocalFunctionHandle,
}

/// Opaque builder-owned document-field handle.
#[pyclass(name = "_QueryV2DocumentFieldHandle", frozen)]
pub struct PyQueryV2DocumentFieldHandle {
    inner: QueryDocumentFieldHandle,
}

/// Immutable plan-bound typed invocation.
#[pyclass(name = "AuthoredQueryInvocation", frozen)]
pub struct PyAuthoredQueryInvocation {
    inner: AuthoredQueryInvocation,
}

#[pymethods]
impl PyAuthoredQueryInvocation {
    #[getter]
    fn canonical_bytes<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.inner.canonical_bytes())
    }

    #[getter]
    fn operation(&self) -> &'static str {
        self.inner.operation_name()
    }

    #[getter]
    fn plan_fingerprint(&self) -> String {
        self.inner.plan_fingerprint_hex()
    }

    #[getter]
    fn authority_identity(&self) -> PyQueryV2AuthorityIdentity {
        PyQueryV2AuthorityIdentity {
            inner: self.inner.authority_identity().clone(),
        }
    }

    #[getter]
    fn required_transport_capabilities<'py>(
        &self,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(
            py,
            self.inner
                .required_transport_capabilities()
                .iter()
                .map(String::as_str),
        )
    }
}

/// Immutable finalized V2 query plan.
#[pyclass(name = "AuthoredQueryPlan", frozen)]
pub struct PyAuthoredQueryPlan {
    inner: AuthoredQueryPlan,
}

#[pymethods]
impl PyAuthoredQueryPlan {
    #[getter]
    fn canonical_bytes<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.inner.canonical_bytes())
    }

    #[getter]
    fn format(&self) -> &str {
        self.inner.format()
    }

    #[getter]
    fn fingerprint(&self) -> &str {
        self.inner.fingerprint_hex()
    }

    #[getter]
    fn required_capabilities<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(
            py,
            self.inner
                .required_capabilities()
                .iter()
                .map(String::as_str),
        )
    }

    #[getter]
    fn authority_identity(&self) -> PyQueryV2AuthorityIdentity {
        PyQueryV2AuthorityIdentity {
            inner: self.inner.authority_identity().clone(),
        }
    }

    fn rows(
        &self,
        _py: Python<'_>,
        rows: &Bound<'_, PyAny>,
    ) -> PyResult<PyAuthoredQueryInvocation> {
        let rows = invocation_rows(&self.inner, rows)?;
        diagnostic(self.inner.rows(rows)).map(|inner| PyAuthoredQueryInvocation { inner })
    }

    fn documents(
        &self,
        _py: Python<'_>,
        rows: &Bound<'_, PyAny>,
    ) -> PyResult<PyAuthoredQueryInvocation> {
        let rows = invocation_rows(&self.inner, rows)?;
        diagnostic(self.inner.documents(rows)).map(|inner| PyAuthoredQueryInvocation { inner })
    }

    fn count(
        &self,
        _py: Python<'_>,
        rows: &Bound<'_, PyAny>,
    ) -> PyResult<PyAuthoredQueryInvocation> {
        let rows = invocation_rows(&self.inner, rows)?;
        diagnostic(self.inner.count(rows)).map(|inner| PyAuthoredQueryInvocation { inner })
    }

    fn exists(
        &self,
        _py: Python<'_>,
        rows: &Bound<'_, PyAny>,
    ) -> PyResult<PyAuthoredQueryInvocation> {
        let rows = invocation_rows(&self.inner, rows)?;
        diagnostic(self.inner.exists(rows)).map(|inner| PyAuthoredQueryInvocation { inner })
    }
}

/// The only mutable Python handle for low-level V2 plan authoring.
#[pyclass(name = "QueryPlanBuilder")]
pub struct PyQueryPlanBuilder {
    inner: QueryPlanBuilder,
    owner_thread: ThreadId,
}

impl PyQueryPlanBuilder {
    fn ensure_owner_thread(&self) -> PyResult<()> {
        if thread::current().id() == self.owner_thread {
            Ok(())
        } else {
            Err(value_error(&Diagnostic::new(
                DiagnosticCategory::InvalidContract,
                DiagnosticCode::new("query_builder_cross_thread")
                    .expect("static Python builder diagnostic code"),
                "query plan builders cannot be used from a thread other than their creator",
            )))
        }
    }
}

#[pymethods]
impl PyQueryPlanBuilder {
    #[new]
    fn new(authority: &PyQueryV2Authority) -> Self {
        Self {
            inner: QueryPlanBuilder::new(authority.authority()),
            owner_thread: thread::current().id(),
        }
    }

    fn binding(&mut self, variable: PythonVariableString) -> PyResult<PyQueryV2BindingHandle> {
        self.ensure_owner_thread()?;
        diagnostic(self.inner.binding(variable.into_inner()))
            .map(|inner| PyQueryV2BindingHandle { inner })
    }

    fn input(
        &mut self,
        public_name: PythonVariableString,
        value_type: PythonVocabularyString,
        optional: &Bound<'_, PyAny>,
    ) -> PyResult<PyQueryV2InputHandle> {
        self.ensure_owner_thread()?;
        let value_type = diagnostic(query_builder_value_type(value_type.as_str()))?;
        let optional = python_boolean(optional)?;
        diagnostic(
            self.inner
                .input(public_name.into_inner(), value_type, optional),
        )
        .map(|inner| PyQueryV2InputHandle { inner })
    }

    fn binding_operand(
        &self,
        binding: &PyQueryV2BindingHandle,
    ) -> PyResult<PyQueryV2OperandHandle> {
        self.ensure_owner_thread()?;
        diagnostic(self.inner.binding_operand(&binding.inner))
            .map(|inner| PyQueryV2OperandHandle { inner })
    }

    fn literal_operand(
        &self,
        value_type: PythonVocabularyString,
        value: &Bound<'_, PyAny>,
    ) -> PyResult<PyQueryV2OperandHandle> {
        self.ensure_owner_thread()?;
        let value_type = diagnostic(query_builder_value_type(value_type.as_str()))?;
        let value = python_scalar(value_type, value)?;
        diagnostic(self.inner.literal_operand(value)).map(|inner| PyQueryV2OperandHandle { inner })
    }

    fn input_operand(&self, input: &PyQueryV2InputHandle) -> PyResult<PyQueryV2OperandHandle> {
        self.ensure_owner_thread()?;
        diagnostic(self.inner.input_operand(&input.inner))
            .map(|inner| PyQueryV2OperandHandle { inner })
    }

    fn isa(
        &self,
        binding: &PyQueryV2BindingHandle,
        type_kind: PythonVocabularyString,
        type_label: PythonLabelString,
        include_subtypes: &Bound<'_, PyAny>,
    ) -> PyResult<PyQueryV2PatternHandle> {
        self.ensure_owner_thread()?;
        let kind = diagnostic(query_builder_type_kind(type_kind.as_str()))?;
        let type_id = diagnostic(TypeId::new(kind, type_label.into_inner()))?;
        let include_subtypes = python_boolean(include_subtypes)?;
        diagnostic(self.inner.isa(&binding.inner, type_id, include_subtypes))
            .map(|inner| PyQueryV2PatternHandle { inner })
    }

    fn has(
        &self,
        owner: &PyQueryV2BindingHandle,
        attribute: &PyQueryV2BindingHandle,
        attribute_label: PythonLabelString,
    ) -> PyResult<PyQueryV2PatternHandle> {
        self.ensure_owner_thread()?;
        let attribute_id = diagnostic(AttributeId::new(attribute_label.into_inner()))?;
        diagnostic(self.inner.has(&owner.inner, &attribute.inner, attribute_id))
            .map(|inner| PyQueryV2PatternHandle { inner })
    }

    fn links(
        &self,
        py: Python<'_>,
        relation: &PyQueryV2BindingHandle,
        relation_label: PythonLabelString,
        roles: &Bound<'_, PyAny>,
        players: &Bound<'_, PyAny>,
    ) -> PyResult<PyQueryV2PatternHandle> {
        self.ensure_owner_thread()?;
        let relation_label = relation_label.into_inner();
        let relation_id = diagnostic(TypeId::new(TypeKind::Relation, relation_label.clone()))?;
        let roles = sequence_items(
            roles,
            MAX_BOOLEAN_TERMS,
            query_builder_role_player_limit_error,
        )?
        .into_iter()
        .map(|role| {
            role.extract::<PythonLabelString>()
                .map(PythonLabelString::into_inner)
        })
        .collect::<PyResult<Vec<_>>>()?;
        let players = binding_handles(
            py,
            players,
            MAX_BOOLEAN_TERMS,
            query_builder_role_player_limit_error,
        )?;
        let players = diagnostic(query_builder_role_players(&relation_label, roles, players))?;
        diagnostic(self.inner.links(&relation.inner, relation_id, players))
            .map(|inner| PyQueryV2PatternHandle { inner })
    }

    fn value(
        &self,
        comparator: PythonVocabularyString,
        left: &PyQueryV2OperandHandle,
        right: &PyQueryV2OperandHandle,
    ) -> PyResult<PyQueryV2PatternHandle> {
        self.ensure_owner_thread()?;
        let comparator = diagnostic(query_builder_comparator(comparator.as_str()))?;
        diagnostic(self.inner.value(comparator, &left.inner, &right.inner))
            .map(|inner| PyQueryV2PatternHandle { inner })
    }

    fn not_(
        &self,
        py: Python<'_>,
        patterns: &Bound<'_, PyAny>,
    ) -> PyResult<PyQueryV2PatternHandle> {
        self.ensure_owner_thread()?;
        diagnostic(self.inner.not(pattern_handles(
            py,
            patterns,
            MAX_BOOLEAN_TERMS,
            query_builder_negation_term_limit_error,
        )?))
        .map(|inner| PyQueryV2PatternHandle { inner })
    }

    fn or_(&self, py: Python<'_>, branches: &Bound<'_, PyAny>) -> PyResult<PyQueryV2PatternHandle> {
        self.ensure_owner_thread()?;
        diagnostic(self.inner.or(pattern_branches(py, branches)?))
            .map(|inner| PyQueryV2PatternHandle { inner })
    }

    fn try_(
        &self,
        py: Python<'_>,
        patterns: &Bound<'_, PyAny>,
    ) -> PyResult<PyQueryV2PatternHandle> {
        self.ensure_owner_thread()?;
        diagnostic(self.inner.r#try(pattern_handles(
            py,
            patterns,
            MAX_BOOLEAN_TERMS,
            query_builder_try_term_limit_error,
        )?))
        .map(|inner| PyQueryV2PatternHandle { inner })
    }

    #[allow(clippy::too_many_arguments)]
    fn reachable(
        &self,
        source: &PyQueryV2BindingHandle,
        target: &PyQueryV2BindingHandle,
        relation_label: PythonLabelString,
        role_from: PythonLabelString,
        role_to: PythonLabelString,
        min_depth: &Bound<'_, PyAny>,
        max_depth: &Bound<'_, PyAny>,
    ) -> PyResult<PyQueryV2PatternHandle> {
        self.ensure_owner_thread()?;
        let min_depth = python_depth(min_depth)?;
        let max_depth = python_depth(max_depth)?;
        let relation_label = relation_label.into_inner();
        let relation = diagnostic(TypeId::new(TypeKind::Relation, relation_label.clone()))?;
        let role_from = diagnostic(RoleId::new(relation_label.clone(), role_from.into_inner()))?;
        let role_to = diagnostic(RoleId::new(relation_label, role_to.into_inner()))?;
        diagnostic(self.inner.reachable(
            &source.inner,
            &target.inner,
            relation,
            role_from,
            role_to,
            min_depth,
            max_depth,
        ))
        .map(|inner| PyQueryV2PatternHandle { inner })
    }

    #[pyo3(signature = (assigned, arguments, function_name=None, local_function=None))]
    fn function_call(
        &self,
        py: Python<'_>,
        assigned: &PyQueryV2BindingHandle,
        arguments: &Bound<'_, PyAny>,
        function_name: Option<PythonLabelString>,
        local_function: Option<Py<PyQueryV2LocalFunctionHandle>>,
    ) -> PyResult<PyQueryV2PatternHandle> {
        self.ensure_owner_thread()?;
        let function_name = function_name
            .map(PythonLabelString::into_inner)
            .map(FunctionId::new)
            .transpose()
            .map_err(|diagnostic| value_error(&diagnostic))?;
        let local_function = local_function.as_ref().map(|function| function.borrow(py));
        let target = diagnostic(query_builder_function_target(
            function_name,
            local_function.as_ref().map(|function| &function.inner),
        ))?;
        diagnostic(self.inner.function_call(
            &assigned.inner,
            target,
            operand_handles(
                py,
                arguments,
                MAX_BOOLEAN_TERMS,
                query_builder_function_argument_limit_error,
            )?,
        ))
        .map(|inner| PyQueryV2PatternHandle { inner })
    }

    fn order(
        &self,
        binding: &PyQueryV2BindingHandle,
        direction: PythonVocabularyString,
    ) -> PyResult<PyQueryV2OrderHandle> {
        self.ensure_owner_thread()?;
        let direction = diagnostic(query_builder_order_direction(direction.as_str()))?;
        diagnostic(self.inner.order(&binding.inner, direction))
            .map(|inner| PyQueryV2OrderHandle { inner })
    }

    #[pyo3(signature = (assigned, reducer, input=None))]
    fn reduce_assignment(
        &self,
        assigned: &PyQueryV2BindingHandle,
        reducer: PythonVocabularyString,
        input: Option<&PyQueryV2BindingHandle>,
    ) -> PyResult<PyQueryV2ReduceAssignmentHandle> {
        self.ensure_owner_thread()?;
        let reducer = diagnostic(query_builder_reducer(reducer.as_str()))?;
        diagnostic(self.inner.reduce_assignment(
            &assigned.inner,
            reducer,
            input.map(|input| &input.inner),
        ))
        .map(|inner| PyQueryV2ReduceAssignmentHandle { inner })
    }

    fn local_return(
        &self,
        reducer: PythonVocabularyString,
        input: &PyQueryV2BindingHandle,
        value_type: PythonVocabularyString,
    ) -> PyResult<PyQueryV2LocalReturnHandle> {
        self.ensure_owner_thread()?;
        let reducer = diagnostic(query_builder_reducer(reducer.as_str()))?;
        let value_type = diagnostic(query_builder_value_type(value_type.as_str()))?;
        diagnostic(self.inner.local_return(reducer, &input.inner, value_type))
            .map(|inner| PyQueryV2LocalReturnHandle { inner })
    }

    #[allow(clippy::too_many_arguments)]
    fn local_function(
        &mut self,
        py: Python<'_>,
        name: PythonLabelString,
        bindings: &Bound<'_, PyAny>,
        parameter_bindings: &Bound<'_, PyAny>,
        parameter_labels: &Bound<'_, PyAny>,
        body: &Bound<'_, PyAny>,
        returns: &PyQueryV2LocalReturnHandle,
    ) -> PyResult<PyQueryV2LocalFunctionHandle> {
        self.ensure_owner_thread()?;
        let name = diagnostic(FunctionId::new(name.into_inner()))?;
        let bindings = binding_handles(
            py,
            bindings,
            MAX_BINDINGS,
            query_builder_local_binding_limit_error,
        )?;
        let parameter_bindings = binding_handles(
            py,
            parameter_bindings,
            MAX_BINDINGS,
            query_builder_local_binding_limit_error,
        )?;
        let parameter_labels = sequence_items(
            parameter_labels,
            MAX_BINDINGS,
            query_builder_local_binding_limit_error,
        )?
        .into_iter()
        .map(|label| {
            label
                .extract::<PythonLabelString>()
                .map(PythonLabelString::into_inner)
        })
        .collect::<PyResult<Vec<_>>>()?;
        let parameters = diagnostic(query_builder_local_parameters(
            parameter_bindings,
            parameter_labels,
        ))?;
        diagnostic(self.inner.local_function(
            name,
            bindings,
            parameters,
            pattern_handles(
                py,
                body,
                MAX_BOOLEAN_TERMS,
                query_builder_local_body_limit_error,
            )?,
            &returns.inner,
        ))
        .map(|inner| PyQueryV2LocalFunctionHandle { inner })
    }

    #[pyo3(name = "match")]
    fn match_(&mut self, py: Python<'_>, patterns: &Bound<'_, PyAny>) -> PyResult<()> {
        self.ensure_owner_thread()?;
        diagnostic(self.inner.r#match(pattern_handles(
            py,
            patterns,
            MAX_BOOLEAN_TERMS,
            query_builder_root_pattern_limit_error,
        )?))
    }

    fn select(&mut self, py: Python<'_>, bindings: &Bound<'_, PyAny>) -> PyResult<()> {
        self.ensure_owner_thread()?;
        diagnostic(self.inner.select(binding_handles(
            py,
            bindings,
            MAX_BINDINGS,
            query_builder_binding_limit_error,
        )?))
    }

    fn require(&mut self, py: Python<'_>, bindings: &Bound<'_, PyAny>) -> PyResult<()> {
        self.ensure_owner_thread()?;
        diagnostic(self.inner.require(binding_handles(
            py,
            bindings,
            MAX_BINDINGS,
            query_builder_binding_limit_error,
        )?))
    }

    fn distinct(&mut self) -> PyResult<()> {
        self.ensure_owner_thread()?;
        diagnostic(self.inner.distinct())
    }

    fn reduce(
        &mut self,
        py: Python<'_>,
        assignments: &Bound<'_, PyAny>,
        groups: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        self.ensure_owner_thread()?;
        let assignments = sequence_items(
            assignments,
            MAX_BOOLEAN_TERMS,
            query_builder_reduce_term_limit_error,
        )?
        .into_iter()
        .map(|assignment| {
            assignment
                .extract::<Py<PyQueryV2ReduceAssignmentHandle>>()
                .map(|assignment| assignment.borrow(py).inner.clone())
        })
        .collect::<PyResult<Vec<_>>>()?;
        diagnostic(self.inner.reduce(
            assignments,
            binding_handles(py, groups, MAX_BINDINGS, query_builder_binding_limit_error)?,
        ))
    }

    fn sort(&mut self, py: Python<'_>, terms: &Bound<'_, PyAny>) -> PyResult<()> {
        self.ensure_owner_thread()?;
        let terms = sequence_items(terms, MAX_ORDER_TERMS, query_builder_sort_term_limit_error)?
            .into_iter()
            .map(|term| {
                term.extract::<Py<PyQueryV2OrderHandle>>()
                    .map(|term| term.borrow(py).inner.clone())
            })
            .collect::<PyResult<Vec<_>>>()?;
        diagnostic(self.inner.sort(terms))
    }

    fn offset(&mut self, rows: &Bound<'_, PyAny>) -> PyResult<()> {
        self.ensure_owner_thread()?;
        diagnostic(self.inner.offset(python_unsigned(rows)?))
    }

    fn limit(&mut self, rows: &Bound<'_, PyAny>) -> PyResult<()> {
        self.ensure_owner_thread()?;
        diagnostic(self.inner.limit(python_unsigned(rows)?))
    }

    fn document_binding(
        &self,
        key: PythonVariableString,
        binding: &PyQueryV2BindingHandle,
    ) -> PyResult<PyQueryV2DocumentFieldHandle> {
        self.ensure_owner_thread()?;
        diagnostic(
            self.inner
                .document_binding(key.into_inner(), &binding.inner),
        )
        .map(|inner| PyQueryV2DocumentFieldHandle { inner })
    }

    fn document_attribute_list(
        &self,
        key: PythonVariableString,
        owner: &PyQueryV2BindingHandle,
        attribute_label: PythonLabelString,
    ) -> PyResult<PyQueryV2DocumentFieldHandle> {
        self.ensure_owner_thread()?;
        let attribute = diagnostic(AttributeId::new(attribute_label.into_inner()))?;
        diagnostic(
            self.inner
                .document_attribute_list(key.into_inner(), &owner.inner, attribute),
        )
        .map(|inner| PyQueryV2DocumentFieldHandle { inner })
    }

    fn finalize_rows(
        &mut self,
        py: Python<'_>,
        columns: &Bound<'_, PyAny>,
    ) -> PyResult<PyAuthoredQueryPlan> {
        self.ensure_owner_thread()?;
        diagnostic(self.inner.finalize_rows(binding_handles(
            py,
            columns,
            MAX_SELECTED_SLOTS,
            query_builder_row_output_limit_error,
        )?))
        .map(|inner| PyAuthoredQueryPlan { inner })
    }

    fn finalize_documents(
        &mut self,
        py: Python<'_>,
        fields: &Bound<'_, PyAny>,
    ) -> PyResult<PyAuthoredQueryPlan> {
        self.ensure_owner_thread()?;
        let fields = sequence_items(
            fields,
            MAX_SELECTED_SLOTS,
            query_builder_document_output_limit_error,
        )?
        .into_iter()
        .map(|field| {
            field
                .extract::<Py<PyQueryV2DocumentFieldHandle>>()
                .map(|field| field.borrow(py).inner.clone())
        })
        .collect::<PyResult<Vec<_>>>()?;
        diagnostic(self.inner.finalize_documents(fields)).map(|inner| PyAuthoredQueryPlan { inner })
    }
}

/// Register the complete opaque native authoring surface.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyQueryPlanBuilder>()?;
    m.add_class::<PyAuthoredQueryPlan>()?;
    m.add_class::<PyAuthoredQueryInvocation>()?;
    m.add_class::<PyQueryV2AuthorityIdentity>()?;
    m.add_class::<PyQueryV2BindingHandle>()?;
    m.add_class::<PyQueryV2InputHandle>()?;
    m.add_class::<PyQueryV2OperandHandle>()?;
    m.add_class::<PyQueryV2PatternHandle>()?;
    m.add_class::<PyQueryV2OrderHandle>()?;
    m.add_class::<PyQueryV2ReduceAssignmentHandle>()?;
    m.add_class::<PyQueryV2LocalReturnHandle>()?;
    m.add_class::<PyQueryV2LocalFunctionHandle>()?;
    m.add_class::<PyQueryV2DocumentFieldHandle>()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pyo3::ffi;
    use pyo3::types::PyAnyMethods;
    use pyo3::{IntoPyObjectExt, Python};
    use type_bridge_contract::codec::FormatVersion;
    use type_bridge_contract::id::{TypeId, TypeKind};
    use type_bridge_contract::limits::{
        MAX_BOOLEAN_TERMS, MAX_INPUT_BYTES, MAX_INPUT_ROWS, MAX_QUERY_INVOCATION_BYTES,
    };
    use type_bridge_contract::query_plan::InputRow;
    use type_bridge_contract::schema::{
        DeclaredSchema, DocumentId, SchemaFact, SourceSpan, SourcedSchemaFact, TypeFact,
        encode_declared_schema,
    };
    use type_bridge_contract::value::{CanonicalString, CanonicalValue, ValueTypeTag};
    use type_bridge_orm::query_v2_builder::QUERY_PLAN_BUILDER_OPERATIONS;
    use type_bridge_orm::query_v2_builder::QueryPlanBuilder;
    use type_bridge_orm::query_v2_prepared::QueryAuthority;

    use super::{
        PyAuthoredQueryPlan, invocation_rows, python_boolean, python_depth, python_scalar,
        python_unsigned, sequence_items,
    };

    fn error_code(py: Python<'_>, error: pyo3::PyErr) -> String {
        error
            .value(py)
            .getattr("code")
            .expect("structured code")
            .extract()
            .expect("string code")
    }

    fn no_input_plan() -> PyAuthoredQueryPlan {
        let person = TypeId::new(TypeKind::Entity, "person").expect("person type");
        let fact = SourcedSchemaFact::new(
            SchemaFact::Type(TypeFact::new(person.clone()).expect("type fact")),
            SourceSpan::new(
                DocumentId::new("python-builder-test").expect("document"),
                0,
                1,
                1,
                1,
                1,
                2,
            )
            .expect("source"),
        );
        let declared = DeclaredSchema::from_facts(FormatVersion::V1, Default::default(), [fact])
            .expect("declared schema");
        let authority = QueryAuthority::from_declared_bytes(
            &encode_declared_schema(&declared).expect("declared bytes"),
            "python-builder-test",
            "typedb-3.12.1/v1",
        )
        .expect("authority");
        let mut builder = QueryPlanBuilder::new(Arc::new(authority));
        let binding = builder.binding("person").expect("binding");
        let pattern = builder.isa(&binding, person, false).expect("isa pattern");
        builder.r#match(vec![pattern]).expect("match");
        PyAuthoredQueryPlan {
            inner: builder.finalize_rows(vec![binding]).expect("plan"),
        }
    }

    fn string_input_plan() -> PyAuthoredQueryPlan {
        let person = TypeId::new(TypeKind::Entity, "person").expect("person type");
        let fact = SourcedSchemaFact::new(
            SchemaFact::Type(TypeFact::new(person.clone()).expect("type fact")),
            SourceSpan::new(
                DocumentId::new("python-builder-input-test").expect("document"),
                0,
                1,
                1,
                1,
                1,
                2,
            )
            .expect("source"),
        );
        let declared = DeclaredSchema::from_facts(FormatVersion::V1, Default::default(), [fact])
            .expect("declared schema");
        let authority = QueryAuthority::from_declared_bytes(
            &encode_declared_schema(&declared).expect("declared bytes"),
            "python-builder-input-test",
            "typedb-3.12.1/v1",
        )
        .expect("authority");
        let mut builder = QueryPlanBuilder::new(Arc::new(authority));
        builder
            .input("supplied_text", ValueTypeTag::String, false)
            .expect("string input");
        let binding = builder.binding("person").expect("binding");
        let pattern = builder.isa(&binding, person, false).expect("isa pattern");
        builder.r#match(vec![pattern]).expect("match");
        PyAuthoredQueryPlan {
            inner: builder.finalize_rows(vec![binding]).expect("input plan"),
        }
    }

    #[test]
    fn every_shared_operation_has_one_python_method() {
        let methods = [
            ("binding", "binding"),
            ("input", "input"),
            ("binding_operand", "binding_operand"),
            ("literal_operand", "literal_operand"),
            ("input_operand", "input_operand"),
            ("isa", "isa"),
            ("has", "has"),
            ("links", "links"),
            ("value", "value"),
            ("not", "not_"),
            ("or", "or_"),
            ("try", "try_"),
            ("reachable", "reachable"),
            ("function_call", "function_call"),
            ("order", "order"),
            ("reduce_assignment", "reduce_assignment"),
            ("local_return", "local_return"),
            ("local_function", "local_function"),
            ("match", "match_"),
            ("select", "select"),
            ("require", "require"),
            ("distinct", "distinct"),
            ("reduce", "reduce"),
            ("sort", "sort"),
            ("offset", "offset"),
            ("limit", "limit"),
            ("document_binding", "document_binding"),
            ("document_attribute_list", "document_attribute_list"),
            ("finalize_rows", "finalize_rows"),
            ("finalize_documents", "finalize_documents"),
        ];
        assert_eq!(
            methods.map(|(shared, _)| shared),
            QUERY_PLAN_BUILDER_OPERATIONS
        );
        let source = include_str!("query_v2_builder_runtime.rs");
        for (_, method) in methods {
            assert!(
                source.contains(&format!("fn {method}(")),
                "missing Python method {method}"
            );
        }
    }

    #[test]
    fn python_integer_boundaries_reject_bool_non_integer_negative_and_huge() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            for value in [
                true.into_py_any(py).expect("bool"),
                1.0_f64.into_py_any(py).expect("float"),
                (-1_i64).into_py_any(py).expect("negative"),
                i128::MAX.into_py_any(py).expect("huge"),
            ] {
                assert_eq!(
                    error_code(py, python_depth(value.bind(py)).expect_err("invalid depth"),),
                    "query_builder_depth_range"
                );
            }

            for value in [
                true.into_py_any(py).expect("bool"),
                1.0_f64.into_py_any(py).expect("float"),
                (-1_i64).into_py_any(py).expect("negative"),
                i128::MAX.into_py_any(py).expect("huge"),
            ] {
                assert_eq!(
                    error_code(
                        py,
                        python_unsigned(value.bind(py)).expect_err("invalid unsigned"),
                    ),
                    "query_builder_unsigned_integer_range"
                );
            }
            assert_eq!(
                python_depth(255_i64.into_py_any(py).expect("depth").bind(py))
                    .expect("maximum depth"),
                u8::MAX
            );
            assert_eq!(
                python_unsigned(u64::MAX.into_py_any(py).expect("window").bind(py))
                    .expect("maximum window"),
                u64::MAX
            );
        });
    }

    #[test]
    fn python_semantic_flags_and_text_scalars_require_exact_host_types() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            assert!(python_boolean(true.into_py_any(py).expect("bool").bind(py)).expect("true"));
            assert!(!python_boolean(false.into_py_any(py).expect("bool").bind(py)).expect("false"));
            for value in [
                1_i64.into_py_any(py).expect("integer"),
                "true".into_py_any(py).expect("string"),
                py.None(),
            ] {
                assert_eq!(
                    error_code(
                        py,
                        python_boolean(value.bind(py)).expect_err("not an exact bool"),
                    ),
                    "query_builder_boolean_host_type"
                );
            }

            for value in [
                1_i64.into_py_any(py).expect("integer"),
                true.into_py_any(py).expect("bool"),
                py.None(),
            ] {
                assert_eq!(
                    error_code(
                        py,
                        python_scalar(ValueTypeTag::String, value.bind(py))
                            .expect_err("not an exact string"),
                    ),
                    "query_builder_scalar_host_type"
                );
            }
        });
    }

    #[test]
    fn zero_input_rows_preserve_direct_unexpected_input_precedence() {
        pyo3::prepare_freethreaded_python();
        let plan = no_input_plan();
        Python::with_gil(|py| {
            let empty = Vec::<Vec<Option<i64>>>::new()
                .into_py_any(py)
                .expect("empty rows");
            plan.rows(py, empty.bind(py))
                .expect("zero rows is the no-input invocation shape");
            for rows in [
                vec![Vec::new()],
                vec![vec![Some(1_i64.into_py_any(py).expect("extra cell"))]],
            ] {
                let rows = rows.into_py_any(py).expect("rows");
                assert_eq!(
                    error_code(
                        py,
                        plan.rows(py, rows.bind(py))
                            .err()
                            .expect("outer row is unexpected"),
                    ),
                    "query_invocation_unexpected_inputs"
                );
            }
        });
    }

    #[test]
    fn python_sequences_reject_hostile_lengths_before_element_access() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            py.run(
                ffi::c_str!(
                    r#"
class HostileSequence:
    accessed = False
    def __len__(self):
        return 257
    def __getitem__(self, index):
        self.accessed = True
        raise AssertionError("element access must not occur")
"#
                ),
                None,
                None,
            )
            .expect("hostile sequence class");
            let sequence = py
                .eval(ffi::c_str!("HostileSequence()"), None, None)
                .expect("hostile sequence");
            let error = sequence_items(
                &sequence,
                MAX_BOOLEAN_TERMS,
                type_bridge_orm::query_v2_builder::query_builder_root_pattern_limit_error,
            )
            .expect_err("oversized sequence");
            assert_eq!(error_code(py, error), "query_plan_pattern_limit");
            assert!(
                !sequence
                    .getattr("accessed")
                    .expect("access flag")
                    .extract::<bool>()
                    .expect("bool flag")
            );
        });
    }

    #[test]
    fn python_invocation_rows_are_count_and_byte_bounded_before_materialization() {
        pyo3::prepare_freethreaded_python();
        let plan = string_input_plan();
        Python::with_gil(|py| {
            py.run(
                ffi::c_str!(
                    r#"
class HostileRows:
    accessed = False
    def __len__(self):
        return 4097
    def __getitem__(self, index):
        self.accessed = True
        raise AssertionError("row access must not occur")
"#
                ),
                None,
                None,
            )
            .expect("hostile rows class");
            let hostile = py
                .eval(ffi::c_str!("HostileRows()"), None, None)
                .expect("hostile rows");
            assert_eq!(
                error_code(
                    py,
                    invocation_rows(&plan.inner, &hostile).expect_err("row ceiling"),
                ),
                "query_invocation_row_limit",
            );
            assert!(
                !hostile
                    .getattr("accessed")
                    .expect("access flag")
                    .extract::<bool>()
                    .expect("bool flag")
            );

            let oversized_chunk = "x".repeat((MAX_INPUT_BYTES / 5) + 32);
            let oversized = vec![vec![oversized_chunk]; 5]
                .into_py_any(py)
                .expect("oversized rows");
            assert_eq!(
                error_code(
                    py,
                    invocation_rows(&plan.inner, oversized.bind(py))
                        .expect_err("aggregate byte ceiling"),
                ),
                "query_invocation_input_byte_limit",
            );

            let base_chunk = "x".repeat((MAX_INPUT_BYTES / 5).saturating_sub(128));
            let mut chunks = vec![base_chunk; 5];
            let build_contract_rows = |chunks: &[String]| {
                chunks
                    .iter()
                    .map(|chunk| {
                        InputRow::new(vec![Some(CanonicalValue::String(
                            CanonicalString::new(chunk.clone()).expect("bounded string"),
                        ))])
                    })
                    .collect::<Vec<_>>()
            };
            let initial_size = serde_json::to_vec(&build_contract_rows(&chunks))
                .expect("input rows")
                .len();
            chunks
                .last_mut()
                .expect("last chunk")
                .push_str(&"x".repeat(MAX_INPUT_BYTES - initial_size));
            let exact = chunks
                .iter()
                .map(|chunk| vec![chunk.clone()])
                .collect::<Vec<_>>()
                .into_py_any(py)
                .expect("exact rows");
            let exact = invocation_rows(&plan.inner, exact.bind(py)).expect("exact input ceiling");
            let invocation = plan.inner.exists(exact).expect("exact invocation");
            assert_eq!(
                invocation.canonical_bytes().len(),
                MAX_QUERY_INVOCATION_BYTES,
            );
            assert_eq!(MAX_INPUT_ROWS, 4_096);
        });
    }
}
