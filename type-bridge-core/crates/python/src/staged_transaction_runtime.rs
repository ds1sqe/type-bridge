//! PyO3 seam for the core staged `Transaction`.
//!
//! Plans stage as built plans (`type_bridge.plan`) or, over a proxy, as
//! canonical `query-plan/v3` bytes decoded against the package's declared
//! schema; nothing reaches the provider until commit.
//! A transaction opens directly on a `RustDatabase` (`staged_transaction`)
//! or over a proxy connection, where commit is one signed
//! `query-remote/v3` exchange.

use std::sync::{Arc, Mutex};

use pyo3::PyClassInitializer;
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyAny;
use pythonize::pythonize;
use tracing::Instrument as _;
use type_bridge_contract::query_plan::v3::decode_query_plan_v3;
use type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic;
use type_bridge_orm::ProviderRuntimeOwner;
use type_bridge_orm::query_v2_prepared::QueryAuthority;
use type_bridge_orm::session::staged::{
    CommitError, Pending, PendingError, PlanAnswer, Transaction, TransactionKind,
};

use crate::match_runtime::py_sdk_diagnostic;
use crate::orm_runtime::{commit_outcome_class, provider_block_on, py_provider_diagnostic};
use crate::plan_builder_runtime::PyPlan;
use crate::query_v2_runtime::{PythonBytes, value_error};

pub(crate) fn parse_kind(kind: &str) -> PyResult<TransactionKind> {
    match kind {
        "read" => Ok(TransactionKind::Read),
        "write" => Ok(TransactionKind::Write),
        "schema" => Ok(TransactionKind::Schema),
        other => Err(PyValueError::new_err(format!(
            "transaction kind must be 'read', 'write' or 'schema', not {other:?}"
        ))),
    }
}

// Commit outcomes raise the `type_bridge.failure` classes released writes
// raise, so `except CommitConflict` catches a losing staged commit.
fn staged_diagnostic(diagnostic: SdkExecutionDiagnostic) -> PyErr {
    if diagnostic
        .failure()
        .and_then(commit_outcome_class)
        .is_some()
    {
        py_provider_diagnostic(diagnostic)
    } else {
        py_sdk_diagnostic(diagnostic)
    }
}

fn commit_error(error: &CommitError) -> PyErr {
    staged_diagnostic(error.diagnostic())
}

fn pending_error(error: &PendingError) -> PyErr {
    match error {
        PendingError::InvalidPlan(diagnostic) | PendingError::NotAllowedInKind(diagnostic) => {
            value_error(diagnostic)
        }
        PendingError::Failed {
            failure: Some(failure),
            context,
            ..
        } => staged_diagnostic(SdkExecutionDiagnostic::from_failure(
            failure,
            context.as_ref().clone(),
        )),
        // The server's refusal keeps its stable code.
        PendingError::Refused { diagnostic, .. } => py_sdk_diagnostic(diagnostic.as_ref().clone()),
        other => PyRuntimeError::new_err(other.to_string()),
    }
}

/// A logical transaction: stage plans, then commit them as one provider
/// transaction. Reusable after commit or rollback until closed.
#[pyclass(name = "Transaction", frozen)]
pub(crate) struct PyStagedTransaction {
    // `None` only while a commit is in flight on another thread.
    inner: Mutex<Option<Transaction>>,
    runtime: Arc<ProviderRuntimeOwner>,
    // Decodes staged plan bytes; a direct transaction stages built plans only.
    authority: Option<Arc<QueryAuthority>>,
    commit_on_exit: bool,
}

impl PyStagedTransaction {
    pub(crate) fn new(
        transaction: Transaction,
        runtime: Arc<ProviderRuntimeOwner>,
        authority: Option<Arc<QueryAuthority>>,
        commit_on_exit: bool,
    ) -> Self {
        Self {
            inner: Mutex::new(Some(transaction)),
            runtime,
            authority,
            commit_on_exit,
        }
    }

    fn with<T>(&self, operation: impl FnOnce(&mut Transaction) -> PyResult<T>) -> PyResult<T> {
        let mut guard = self.inner.lock().expect("transaction lock poisoned");
        let transaction = guard.as_mut().ok_or_else(|| {
            PyRuntimeError::new_err("Transaction is committing on another thread")
        })?;
        operation(transaction)
    }

    fn drive(&self, py: Python<'_>) -> PyResult<()> {
        let entry = crate::perf_stages::entry();
        let _entered = entry.enter();
        let mut transaction = self
            .inner
            .lock()
            .expect("transaction lock poisoned")
            .take()
            .ok_or_else(|| {
                PyRuntimeError::new_err("Transaction is committing on another thread")
            })?;
        let result = provider_block_on(
            py,
            self.runtime.as_ref(),
            async {
                let result = transaction.commit().await;
                (transaction, result)
            }
            // The provider may poll on another thread; the span follows it.
            .instrument(entry.clone()),
        );
        let (transaction, result) = result;
        *self.inner.lock().expect("transaction lock poisoned") = Some(transaction);
        result.map_err(|error| commit_error(&error))
    }
}

#[pymethods]
impl PyStagedTransaction {
    /// Stage one plan, a built `Plan` or, over a proxy, canonical
    /// `query-plan/v3` bytes; its answer resolves at commit. A plan the
    /// transaction cannot carry fails here.
    fn stage(&self, plan: &Bound<'_, PyAny>) -> PyResult<PyPending> {
        let _entry = crate::perf_stages::entry().entered();
        let decoded;
        let plan = if let Ok(built) = plan.cast::<PyPlan>() {
            built.get().inner()
        } else {
            let authority = self.authority.as_ref().ok_or_else(|| {
                pyo3::exceptions::PyTypeError::new_err(
                    "a direct transaction stages built plans; build the plan with \
                     PlanBuilder.build(authority)",
                )
            })?;
            let bytes = PythonBytes::extract(plan, "plan")?.bounded_snapshot(
                type_bridge_contract::limits::MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1),
            );
            decoded = decode_query_plan_v3(&bytes, authority.declared())
                .map_err(|error| value_error(&error))?;
            &decoded
        };
        let pending = self.with(|transaction| {
            transaction
                .stage_plan(plan)
                .map_err(|error| pending_error(&error))
        })?;
        Ok(PyPending { pending })
    }

    /// Fire the staged plans as one transaction and resolve their answers.
    fn commit(&self, py: Python<'_>) -> PyResult<()> {
        self.drive(py)
    }

    /// Discard staged work; the transaction stays usable.
    fn rollback(&self) -> PyResult<()> {
        self.with(|transaction| {
            transaction
                .rollback()
                .map_err(|error| pending_error(&error))
        })
    }

    /// Discard staged work and end the transaction.
    fn close(&self) -> PyResult<()> {
        self.with(|transaction| transaction.close().map_err(|error| pending_error(&error)))
    }

    fn __enter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    /// Commit on a clean exit (unless opened with `commit_on_exit=False`),
    /// discard on an exception, and close. A failed commit raises after the
    /// close; an unknown outcome means the caller reads the database.
    fn __exit__(
        &self,
        py: Python<'_>,
        exc_type: Option<&Bound<'_, PyAny>>,
        _exc_value: Option<&Bound<'_, PyAny>>,
        _traceback: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<bool> {
        let committed = if exc_type.is_none() && self.commit_on_exit {
            self.drive(py)
        } else {
            Ok(())
        };
        let closed = self.close();
        committed?;
        // Closing an already-closed transaction is not an error on exit.
        drop(closed);
        Ok(false)
    }
}

/// An answer that resolves only after its transaction commits.
#[pyclass(name = "Pending", frozen)]
pub(crate) struct PyPending {
    pending: Pending<PlanAnswer>,
}

#[pymethods]
impl PyPending {
    /// The resolved answer: `Rows`, `Documents`, `Count`, `Exists` or `Matched`. Raises
    /// the commit's typed failure, or a lifecycle error before commit.
    fn result(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let _entry = crate::perf_stages::entry().entered();
        let answer = self.pending.get().map_err(|error| pending_error(&error))?;
        answer_object(py, answer.as_ref())
    }
}

fn answer_object(py: Python<'_>, answer: &PlanAnswer) -> PyResult<Py<PyAny>> {
    let base = PyClassInitializer::from(PyPlanAnswer);
    let object = match answer {
        PlanAnswer::Rows(rows) => {
            let rows = pythonize(py, rows)
                .map_err(|error| PyValueError::new_err(error.to_string()))?
                .unbind();
            Py::new(py, base.add_subclass(PyRows { rows }))?.into_any()
        }
        PlanAnswer::Documents(documents) => {
            let documents = pythonize(py, documents)
                .map_err(|error| PyValueError::new_err(error.to_string()))?
                .unbind();
            Py::new(py, base.add_subclass(PyDocuments { documents }))?.into_any()
        }
        PlanAnswer::Count(count) => {
            Py::new(py, base.add_subclass(PyCount { count: *count }))?.into_any()
        }
        PlanAnswer::Exists(exists) => {
            Py::new(py, base.add_subclass(PyExists { exists: *exists }))?.into_any()
        }
        PlanAnswer::Matched(count) => {
            Py::new(py, base.add_subclass(PyMatched { count: *count }))?.into_any()
        }
        _ => {
            return Err(PyRuntimeError::new_err(
                "plan answer kind is not supported by this binding",
            ));
        }
    };
    Ok(object)
}

/// The answer of one staged plan: `Rows`, `Documents`, `Count`, `Exists` or `Matched`.
#[pyclass(name = "PlanAnswer", subclass, frozen)]
pub(crate) struct PyPlanAnswer;

/// A read plan's projected rows, one dict per row.
#[pyclass(name = "Rows", extends = PyPlanAnswer, frozen)]
pub(crate) struct PyRows {
    #[pyo3(get)]
    rows: Py<PyAny>,
}

#[pymethods]
impl PyRows {
    #[new]
    fn new(rows: Py<PyAny>) -> (Self, PyPlanAnswer) {
        (Self { rows }, PyPlanAnswer)
    }

    #[classattr]
    fn __match_args__() -> (&'static str,) {
        ("rows",)
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        match other.cast::<Self>() {
            Ok(other) => self
                .rows
                .bind(other.py())
                .eq(other.get().rows.bind(other.py())),
            Err(_) => Ok(false),
        }
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!("Rows({})", self.rows.bind(py).repr()?))
    }
}

/// Schema-validated fetch documents; model hydration belongs to the typed facade.
#[pyclass(name = "Documents", extends = PyPlanAnswer, frozen)]
pub(crate) struct PyDocuments {
    #[pyo3(get)]
    documents: Py<PyAny>,
}

#[pymethods]
impl PyDocuments {
    #[new]
    fn new(documents: Py<PyAny>) -> (Self, PyPlanAnswer) {
        (Self { documents }, PyPlanAnswer)
    }

    #[classattr]
    fn __match_args__() -> (&'static str,) {
        ("documents",)
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        match other.cast::<Self>() {
            Ok(other) => self
                .documents
                .bind(other.py())
                .eq(other.get().documents.bind(other.py())),
            Err(_) => Ok(false),
        }
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!("Documents({})", self.documents.bind(py).repr()?))
    }
}

macro_rules! scalar_answer {
    ($rust:ident, $name:literal, $field:ident: $ty:ty) => {
        #[doc = concat!("A plan answer: `", $name, "(", stringify!($field), ")`.")]
        #[pyclass(name = $name, extends = PyPlanAnswer, frozen, eq)]
        #[derive(PartialEq)]
        pub(crate) struct $rust {
            #[pyo3(get)]
            $field: $ty,
        }

        #[pymethods]
        impl $rust {
            #[new]
            fn new($field: $ty) -> (Self, PyPlanAnswer) {
                (Self { $field }, PyPlanAnswer)
            }

            #[classattr]
            fn __match_args__() -> (&'static str,) {
                (stringify!($field),)
            }

            fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
                let value = self.$field.into_pyobject(py)?;
                Ok(format!("{}({})", $name, value.as_any().repr()?))
            }
        }
    };
}

scalar_answer!(PyCount, "Count", count: u64);
scalar_answer!(PyExists, "Exists", exists: bool);
scalar_answer!(PyMatched, "Matched", count: u64);

pub(crate) fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyStagedTransaction>()?;
    module.add_class::<PyPending>()?;
    module.add_class::<PyPlanAnswer>()?;
    module.add_class::<PyRows>()?;
    module.add_class::<PyDocuments>()?;
    module.add_class::<PyCount>()?;
    module.add_class::<PyExists>()?;
    module.add_class::<PyMatched>()?;
    Ok(())
}

#[cfg(test)]
mod document_answer_tests {
    use super::*;

    #[test]
    fn pending_conversion_returns_a_documents_instance() {
        Python::initialize();
        Python::attach(|py| {
            let documents = vec![
                serde_json::json!({"_iid":"0x01", "_type":"person", "attributes":{"name":"Ada"}}),
            ];
            let answer = answer_object(py, &PlanAnswer::Documents(documents.clone())).unwrap();
            let instance = answer.bind(py).cast::<PyDocuments>().unwrap();
            let expected = pythonize(py, &documents).unwrap();
            assert!(instance.get().documents.bind(py).eq(expected).unwrap());
        });
    }
}
