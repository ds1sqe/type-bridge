//! PyO3 seam for the core staged `Transaction`.
//!
//! Plans stage as built plans (`type_bridge.plan`) or, over a proxy, as
//! canonical `query-plan/v3` bytes decoded against the package's declared
//! schema; nothing reaches the provider until commit.
//! A transaction opens directly on a `RustDatabase` (`staged_transaction`)
//! or over a proxy connection, where commit is one signed
//! `query-remote/v3` exchange and `commit_retry` resends the last attempt
//! after an unknown outcome.

use std::sync::{Arc, Mutex};

use pyo3::PyClassInitializer;
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyAny;
use pythonize::pythonize;
use type_bridge_contract::query_plan::v3::decode_query_plan_v3;
use type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic;
use type_bridge_orm::ProviderRuntimeOwner;
use type_bridge_orm::query_v2_prepared::QueryAuthority;
use type_bridge_orm::session::staged::{
    CommitError, Pending, PendingError, PlanAnswer, Transaction, TransactionKind,
};

use crate::match_runtime::py_sdk_diagnostic;
use crate::orm_runtime::provider_block_on;
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

fn commit_error(error: &CommitError) -> PyErr {
    py_sdk_diagnostic(error.diagnostic())
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
        } => py_sdk_diagnostic(SdkExecutionDiagnostic::from_failure(
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

    fn drive(&self, py: Python<'_>, retry: bool) -> PyResult<()> {
        let mut transaction = self
            .inner
            .lock()
            .expect("transaction lock poisoned")
            .take()
            .ok_or_else(|| {
                PyRuntimeError::new_err("Transaction is committing on another thread")
            })?;
        let result = provider_block_on(py, self.runtime.as_ref(), async {
            let result = if retry {
                transaction.commit_retry().await
            } else {
                transaction.commit().await
            };
            (transaction, result)
        });
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

    /// Whether the last commit's outcome is unknown; `commit_retry()` then
    /// learns it without applying the writes twice.
    fn has_unknown_outcome(&self) -> PyResult<bool> {
        self.with(|transaction| Ok(transaction.has_unknown_outcome()))
    }

    /// Fire the staged plans as one transaction and resolve their answers.
    fn commit(&self, py: Python<'_>) -> PyResult<()> {
        self.drive(py, false)
    }

    /// Resend the last commit attempt, with the same idempotency key, after
    /// its outcome was unknown. The server replays a stored reply, so the
    /// writes apply at most once.
    fn commit_retry(&self, py: Python<'_>) -> PyResult<()> {
        self.drive(py, true)
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
    /// discard on an exception, and close. A commit whose outcome is
    /// unknown raises and leaves the transaction open, so the caller can
    /// call `commit_retry()`; closing would lose the only way to learn
    /// whether the writes applied.
    fn __exit__(
        &self,
        py: Python<'_>,
        exc_type: Option<&Bound<'_, PyAny>>,
        _exc_value: Option<&Bound<'_, PyAny>>,
        _traceback: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<bool> {
        let committed = if exc_type.is_none() && self.commit_on_exit {
            self.drive(py, false)
        } else {
            Ok(())
        };
        if committed.is_err() && self.with(|transaction| Ok(transaction.has_unknown_outcome()))? {
            return committed.map(|()| false);
        }
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
    /// The resolved answer: `Rows`, `Count`, `Exists` or `Matched`. Raises
    /// the commit's typed failure, or a lifecycle error before commit.
    fn result(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let answer = self.pending.get().map_err(|error| pending_error(&error))?;
        let base = PyClassInitializer::from(PyPlanAnswer);
        let object = match answer.as_ref() {
            PlanAnswer::Rows(rows) => {
                let rows = pythonize(py, rows)
                    .map_err(|error| PyValueError::new_err(error.to_string()))?
                    .unbind();
                Py::new(py, base.add_subclass(PyRows { rows }))?.into_any()
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
}

/// The answer of one staged plan: `Rows`, `Count`, `Exists` or `Matched`.
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
    module.add_class::<PyCount>()?;
    module.add_class::<PyExists>()?;
    module.add_class::<PyMatched>()?;
    Ok(())
}
