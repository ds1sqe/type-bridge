//! PyO3 seam for the core staged `Transaction`.
//!
//! Plans stage as built plans (`type_bridge.plan`) or, over a proxy, as
//! canonical `query-plan/v3` bytes decoded against the package's declared
//! schema; nothing reaches the provider until commit.
//! A transaction opens directly on a `PyDatabase` (`staged_transaction`)
//! or over a proxy connection, where commit is one signed
//! `query-remote/v3` exchange.

use std::sync::{Arc, Mutex};

use pyo3::PyClassInitializer;
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyAny;
use pythonize::pythonize;
use tracing::Instrument as _;
use type_bridge_contract::query_plan::v3::{BuiltPlans, decode_query_plan_v3};
use type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic;
use type_bridge_orm::ProviderRuntimeOwner;
use type_bridge_orm::plan_authority::QueryAuthority;
use type_bridge_orm::session::staged::{
    CommitError, Pending, PendingError, PlanAnswer, Transaction, TransactionKind,
    merge_plan_answers,
};

/// Shared transaction configuration; absent timeout inherits the connection.
pub(crate) fn staged_options(
    kind: Option<&str>,
    timeout: Option<Bound<'_, PyAny>>,
    collect: bool,
) -> PyResult<type_bridge_orm::session::staged::TransactionOptions> {
    let mut options = type_bridge_orm::session::staged::TransactionOptions::default()
        .with_collect_phase_timestamps(collect);
    if let Some(kind) = kind {
        options = options.with_kind(parse_kind(kind)?);
    }
    if let Some(timeout) = timeout {
        let milliseconds = timeout
            .cast_exact::<pyo3::types::PyInt>()
            .map_err(|_| {
                pyo3::exceptions::PyTypeError::new_err("timeout_milliseconds must be an exact int")
            })?
            .extract::<i64>()?;
        options = options.with_timeout(type_bridge_orm::QueryTimeout::from_signed_milliseconds(
            milliseconds,
        ));
    }
    Ok(options)
}

use crate::match_runtime::{
    PyQueryInvocationBudget, py_sdk_diagnostic, validated_invocation_handle,
};
use crate::orm_runtime::{commit_outcome_class, provider_block_on, py_provider_diagnostic};
use crate::plan_builder_runtime::{PyPlan, PyPlanProgram, PyStagedPlan};
use crate::plan_runtime::{PythonBytes, value_error};
use crate::validated_result_runtime::{MatchInvocation, PyValidatedMatchResultHandle};
use type_bridge_orm::_registry::DescriptorRegistry;
use type_bridge_orm::{
    AnswerCancellation, InstalledRuntimeProjection, QueryExecutionDeadline,
    QueryExecutionResourceLimits, ValidatedMatchRequest, ValidatedMatchResult,
};

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
        PendingError::Failed { diagnostic, .. } => staged_diagnostic(diagnostic.as_ref().clone()),
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
    origin: type_bridge_orm::ProjectedQueryOrigin,
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
            origin: type_bridge_orm::ProjectedQueryOrigin::remote_unbound(),
        }
    }

    pub(crate) fn with_origin(mut self, origin: type_bridge_orm::ProjectedQueryOrigin) -> Self {
        self.origin = origin;
        self
    }

    fn with<T>(&self, operation: impl FnOnce(&mut Transaction) -> PyResult<T>) -> PyResult<T> {
        let mut guard = self.inner.lock().expect("transaction lock poisoned");
        let transaction = guard.as_mut().ok_or_else(|| {
            PyRuntimeError::new_err("Transaction is committing on another thread")
        })?;
        operation(transaction)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn stage_typed_query(
        &self,
        py: Python<'_>,
        installed: Option<Arc<InstalledRuntimeProjection>>,
        validated: ValidatedMatchRequest,
        registry: Arc<DescriptorRegistry>,
        resources: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
        decoder: Py<PyAny>,
    ) -> PyResult<PyPending> {
        if !decoder.bind(py).is_callable() {
            return Err(pyo3::exceptions::PyTypeError::new_err(
                "typed query decoder must be callable",
            ));
        }
        let request = Arc::new(validated);
        let pending = self.with(|transaction| {
            transaction
                .stage_shared_match_with_limits(
                    &registry,
                    Arc::clone(&request),
                    type_bridge_orm::MatchExecutionLimits::from_policy(
                        resources,
                        cancellation.clone(),
                    ),
                )
                .map_err(|error| pending_error(&error))
        })?;
        Ok(PyPending {
            source: PendingSource::Query {
                pending,
                request,
                installed,
                registry,
                resources,
                cancellation,
                resolved: std::sync::OnceLock::new(),
            },
            origin: self.origin.clone(),
            decoder: Some(decoder),
        })
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
        let mut decoder = None;
        let plans = if let Ok(built) = plan.cast::<PyPlan>() {
            let built = built.borrow();
            decoder = built
                .decoder
                .as_ref()
                .map(|decoder| decoder.clone_ref(plan.py()));
            Arc::clone(built.built()?)
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
            let decoded = decode_query_plan_v3(&bytes, authority.declared())
                .map_err(|error| value_error(&error))?;
            Arc::new(PyPlanProgram::Raw(Box::new(BuiltPlans::single(decoded))))
        };
        let (pending, models) = match plans.as_ref() {
            PyPlanProgram::Models(model) => {
                let answer = self.with(|transaction| {
                    transaction
                        .stage_plan_fetch(
                            Arc::clone(&plans),
                            Arc::clone(&model.registry),
                            Default::default(),
                        )
                        .map_err(|error| pending_error(&error))
                })?;
                (vec![], Some(answer))
            }
            PyPlanProgram::Raw(_) => {
                let answers = self.with(|transaction| {
                    transaction
                        .stage_plans(Arc::clone(&plans), plans.returns_documents())
                        .map_err(|error| pending_error(&error))
                })?;
                (answers, None)
            }
        };
        Ok(PyPending {
            source: PendingSource::Plans {
                pending,
                models,
                plans,
                projected: std::sync::OnceLock::new(),
            },
            decoder,
            origin: self.origin.clone(),
        })
    }

    /// Queue a complete authored invocation stack with block-local IID references.
    fn stage_stack(&self, plans: Vec<PyRef<'_, PyStagedPlan>>) -> PyResult<Vec<PyPending>> {
        let Some(first) = plans.first() else {
            return Ok(vec![]);
        };
        let pending = self.with(|transaction| {
            transaction
                .stage_plan_stack(
                    plans.iter().map(|plan| plan.entry.clone()).collect(),
                    Arc::new(first.authority.declared().clone()),
                )
                .map_err(|error| pending_error(&error))
        })?;
        Ok(plans
            .iter()
            .zip(pending)
            .map(|(plan, pending)| PyPending {
                source: PendingSource::Plans {
                    pending: vec![pending],
                    models: None,
                    plans: Arc::new(PyPlanProgram::Raw(Box::new(BuiltPlans::single(
                        plan.entry.plan().clone(),
                    )))),
                    projected: std::sync::OnceLock::new(),
                },
                decoder: None,
                origin: self.origin.clone(),
            })
            .collect())
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
    source: PendingSource,
    origin: type_bridge_orm::ProjectedQueryOrigin,
    decoder: Option<Py<PyAny>>,
}
enum PendingSource {
    Plans {
        pending: Vec<Pending<PlanAnswer>>,
        plans: Arc<PyPlanProgram>,
        models: Option<Pending<type_bridge_orm::match_request::OwnedPlanFetchResult>>,
        projected: std::sync::OnceLock<
            Result<type_bridge_orm::ProjectedQueryValue, SdkExecutionDiagnostic>,
        >,
    },
    Query {
        pending: Pending<ValidatedMatchResult>,
        request: Arc<ValidatedMatchRequest>,
        installed: Option<Arc<InstalledRuntimeProjection>>,
        registry: Arc<DescriptorRegistry>,
        resources: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
        resolved: std::sync::OnceLock<PyValidatedMatchResultHandle>,
    },
}

#[pymethods]
impl PyPending {
    #[classmethod]
    fn __class_getitem__(
        cls: &Bound<'_, pyo3::types::PyType>,
        _item: &Bound<'_, PyAny>,
    ) -> Py<pyo3::types::PyType> {
        cls.clone().unbind()
    }

    /// Return the plan answer or decoded typed-query value after commit. Raises
    /// the commit's typed failure, or a lifecycle error before commit.
    fn result(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let _entry = crate::perf_stages::entry().entered();
        let answer = match &self.source {
            PendingSource::Plans {
                pending,
                plans,
                models,
                projected,
            } => {
                if let Some(pending) = models {
                    let proof = pending.get().map_err(|error| pending_error(&error))?;
                    let PyPlanProgram::Models(model) = plans.as_ref() else {
                        unreachable!("model pending retains its program")
                    };
                    return model.materialize(py, &proof, self.origin.clone(), projected);
                }
                let answers = pending
                    .iter()
                    .map(|pending| pending.get())
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| pending_error(&error))?;
                let correlations = pending
                    .iter()
                    .map(|pending| pending.correlation())
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| pending_error(&error))?;
                let answer = merge_plan_answers(
                    match plans.as_ref() {
                        PyPlanProgram::Raw(plans) => plans,
                        _ => unreachable!("raw answer retains raw plans"),
                    },
                    &answers,
                    &correlations,
                )
                .map_err(|error| value_error(&error))?;
                answer_object(py, answer.as_ref())?
            }
            PendingSource::Query {
                pending,
                request,
                installed,
                registry,
                resources,
                cancellation,
                resolved,
            } => {
                let deadline = QueryExecutionDeadline::for_limits(*resources);
                let result = if let Some(result) = resolved.get() {
                    result.with_shared_budget(deadline, cancellation.clone())
                } else {
                    let result = pending.get().map_err(|error| pending_error(&error))?;
                    let budget = PyQueryInvocationBudget::from_parts(
                        *resources,
                        deadline,
                        cancellation.clone(),
                    );
                    let handle = validated_invocation_handle(
                        installed.as_ref(),
                        Some(self.origin.clone()),
                        MatchInvocation::Shared {
                            request: Arc::clone(request),
                            result,
                        },
                        Arc::clone(registry),
                        budget,
                    )?;
                    let _ = resolved.set(handle.clone());
                    handle
                };
                Py::new(py, result)?.into_any()
            }
        };

        match &self.decoder {
            Some(decoder) => decoder.call1(py, (answer,)),
            None => Ok(answer),
        }
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
    pub(crate) rows: Py<PyAny>,
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

#[cfg(test)]
mod pending_diagnostic_redaction_tests {
    use super::*;
    #[test]
    fn pending_failure_uses_the_canonical_diagnostic_without_server_text() {
        let error = PendingError::Failed {
            diagnostic: Box::new(
                type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic::commit_failure(
                    type_bridge_contract::sdk_diagnostic::SdkCommitFailureOutcome::Unknown,
                ),
            ),
            op_index: None,
            message: "signed-server-secret".into(),
            failure: Some(Box::new(
                type_bridge_contract::failure::Failure::Indeterminate { code: None },
            )),
            context: Default::default(),
        };
        assert!(!error.to_string().contains("signed-server-secret"));
        Python::initialize();
        Python::attach(|py| {
            let surfaced = pending_error(&error);
            assert!(!surfaced.to_string().contains("signed-server-secret"));
            assert!(
                !surfaced
                    .value(py)
                    .str()
                    .unwrap()
                    .to_string()
                    .contains("signed-server-secret")
            );
        });
    }
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    #[test]
    fn staged_policy_defaults_and_signed_timeouts() {
        Python::initialize();
        Python::attach(|py| {
            let defaults = staged_options(None, None, false).unwrap();
            assert_eq!(
                defaults.timeout.unwrap_or_default().milliseconds(),
                Some(60_000)
            );
            assert!(!defaults.collect_phase_timestamps);
            for (input, expected) in [(0_i64, None), (-1, None), (86_400_001, Some(86_400_001))] {
                let value = input.into_pyobject(py).unwrap().into_any();
                let options = staged_options(Some("write"), Some(value), true).unwrap();
                assert_eq!(options.timeout.unwrap().milliseconds(), expected);
                assert!(options.collect_phase_timestamps);
            }
            assert!(
                staged_options(
                    None,
                    Some(true.into_pyobject(py).unwrap().to_owned().into_any()),
                    false
                )
                .is_err()
            );
        });
    }
}
