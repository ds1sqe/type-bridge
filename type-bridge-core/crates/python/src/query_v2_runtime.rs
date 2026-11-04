//! Retained V2 prepared-query execution bindings.

use crate::orm_runtime::{PyDatabase, provider_block_on};
use crate::plan_runtime::{
    PyPlanAuthority, PythonBytes, PythonString, python_limit, python_optional_limit, value_error,
};
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::PyAny;
use std::sync::Arc;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::limits::MAX_REMOTE_ENVELOPE_BYTES;
use type_bridge_contract::query_remote::{
    checked_remote_deadline, checked_remote_limit, remote_deadline_limit,
};
use type_bridge_contract::query_remote_v2::RemoteLimitsV2;
use type_bridge_orm::query_v2_prepared::{
    PendingRemoteQueryV2, decode_remote_capabilities, execute_prepared_local,
    prepare_remote_query_v2,
};
use type_bridge_orm::session::backend::{AnswerLimits, BoundedAnswerLimits};

const LOCAL_WORKER_FAILED: &str =
    "query_v2_local_worker_failed: local provider worker terminated unexpectedly";

fn contain_local_worker_panic<T>(worker: impl FnOnce() -> T) -> PyResult<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(worker))
        .map_err(|_| PyRuntimeError::new_err(LOCAL_WORKER_FAILED))
}

/// One prepared request with an atomic one-shot reply decoder.
#[pyclass(name = "PendingQueryV2Remote", frozen)]
pub struct PyPendingQueryV2Remote {
    pending: PendingRemoteQueryV2,
}

#[pymethods]
impl PyPendingQueryV2Remote {
    /// Return the exact canonical bytes to send to `/v2/query`.
    fn request_bytes(&self) -> Vec<u8> {
        self.pending.request_bytes().to_vec()
    }

    /// Consume the only reply accepted and verify its request binding.
    ///
    /// The winner snapshots at most one byte beyond the protocol hard ceiling;
    /// the caller's success-byte budget applies only after authentication and
    /// reply-kind classification, so it cannot truncate failure evidence.
    /// Parsing, evidence validation, and outcome serialization then run without
    /// the GIL so a maximal envelope cannot stall unrelated Python threads.
    fn decode_reply(&self, py: Python<'_>, response: &Bound<'_, PyAny>) -> PyResult<String> {
        let claimed = self
            .pending
            .claim_reply()
            .map_err(|diagnostic| value_error(&diagnostic))?;
        let response = PythonBytes::extract(response, "response")?;
        let response = response.bounded_snapshot(claimed.response_snapshot_limit());
        py.detach(move || claimed.decode(&response))
            .map_err(|diagnostic| value_error(&diagnostic))
    }
}

fn local_limits(deadline_ms: Option<&Bound<'_, PyAny>>) -> PyResult<AnswerLimits> {
    let deadline = checked_remote_deadline(
        python_optional_limit(deadline_ms).map_err(|diagnostic| value_error(&diagnostic))?,
    )
    .map_err(|diagnostic| value_error(&diagnostic))?
    .map(|ms| {
        std::time::Instant::now()
            .checked_add(std::time::Duration::from_millis(ms))
            .ok_or_else(remote_deadline_limit)
    })
    .transpose()
    .map_err(|diagnostic| value_error(&diagnostic))?;
    Ok(AnswerLimits {
        answer: BoundedAnswerLimits {
            deadline,
            ..BoundedAnswerLimits::default()
        },
        ..AnswerLimits::default()
    })
}

/// Execute one prepared plan locally; returns typed outcome JSON.
///
/// The GIL is released for the whole provider round trip, and the
/// optional deadline bounds it — a stalled provider cannot pin other
/// Python threads for longer than the caller allows.
#[pyfunction]
#[pyo3(signature = (database, authority, plan, invocation_json, deadline_ms=None))]
pub fn query_v2_execute_local(
    py: Python<'_>,
    database: &PyDatabase,
    authority: &PyPlanAuthority,
    plan: &Bound<'_, PyAny>,
    invocation_json: &Bound<'_, PyAny>,
    deadline_ms: Option<&Bound<'_, PyAny>>,
) -> PyResult<String> {
    let plan = PythonBytes::extract(plan, "plan")?;
    let invocation_json = PythonString::extract(invocation_json, "invocation_json")?;
    let limits = local_limits(deadline_ms)?;
    let query_limits = authority.authority.limits();
    let plan = plan.bounded_snapshot(query_limits.codec.max_bytes.saturating_add(1));
    let invocation_json = invocation_json.bounded_snapshot(query_limits.invocation_bytes())?;
    let (db, runtime) = database.handles();
    let authority = Arc::clone(&authority.authority);
    contain_local_worker_panic(|| {
        provider_block_on(
            py,
            runtime.as_ref(),
            execute_prepared_local(&db, &authority, &plan, &invocation_json, limits),
        )
    })?
    .map_err(|diagnostic| value_error(&diagnostic))
}

/// Build the exact remote limit set from checked caller arguments.
///
/// Limits accept any Python integer and convert through the shared
/// contract range check, so a negative or oversized budget fails with
/// the same stable diagnostic the Node binding reports.
fn remote_limits(
    max_items: &Bound<'_, PyAny>,
    max_bytes: &Bound<'_, PyAny>,
    max_collection_members: &Bound<'_, PyAny>,
    deadline_ms: Option<&Bound<'_, PyAny>>,
) -> PyResult<RemoteLimitsV2> {
    let build = || -> Result<RemoteLimitsV2, Diagnostic> {
        // A low-level plan returns no hydration graph, so the graph budgets
        // keep the proxy (wire) defaults this remote request travels under.
        let policy = type_bridge_orm::QueryExecutionResourceLimits::default_for(
            type_bridge_orm::AnswerTransport::Proxy,
        );
        Ok(RemoteLimitsV2 {
            deadline_ms: checked_remote_deadline(python_optional_limit(deadline_ms)?)?,
            max_bytes: checked_remote_limit(python_limit(max_bytes)?)?,
            max_items: checked_remote_limit(python_limit(max_items)?)?,
            max_collection_members: checked_remote_limit(python_limit(max_collection_members)?)?,
            max_graph_nodes: policy.graph_nodes,
            max_attribute_values: policy.attribute_values,
            max_role_players: policy.role_players,
            max_statements: policy.statement_limit(),
        })
    };
    build().map_err(|diagnostic| value_error(&diagnostic))
}

/// Decode one capability advertisement into its sorted capability ids.
#[pyfunction]
pub fn query_v2_remote_capabilities(
    py: Python<'_>,
    advertisement: &Bound<'_, PyAny>,
) -> PyResult<Vec<String>> {
    let advertisement = PythonBytes::extract(advertisement, "advertisement")?
        .bounded_snapshot(MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1));
    py.detach(move || decode_remote_capabilities(&advertisement))
        .map_err(|diagnostic| value_error(&diagnostic))
}

/// Prepare one remote invocation and its one-shot reply decoder.
///
/// `advertisement` carries the executor's exact `/v2/capabilities`
/// bytes; a plan or multi-row invocation the executor cannot execute is
/// refused here, before any request bytes exist.
/// `max_bytes` limits successful signed response bytes; authenticated failure
/// envelopes remain decodable under the protocol hard ceiling.
#[pyfunction]
#[pyo3(signature = (authority, plan, invocation_json, advertisement, max_items, max_bytes, max_collection_members, deadline_ms=None))]
#[expect(clippy::too_many_arguments, reason = "flat binding surface")]
pub fn query_v2_prepare_remote(
    py: Python<'_>,
    authority: &PyPlanAuthority,
    plan: &Bound<'_, PyAny>,
    invocation_json: &Bound<'_, PyAny>,
    advertisement: &Bound<'_, PyAny>,
    max_items: &Bound<'_, PyAny>,
    max_bytes: &Bound<'_, PyAny>,
    max_collection_members: &Bound<'_, PyAny>,
    deadline_ms: Option<&Bound<'_, PyAny>>,
) -> PyResult<PyPendingQueryV2Remote> {
    let plan = PythonBytes::extract(plan, "plan")?;
    let invocation_json = PythonString::extract(invocation_json, "invocation_json")?;
    let advertisement = PythonBytes::extract(advertisement, "advertisement")?;
    let limits = remote_limits(max_items, max_bytes, max_collection_members, deadline_ms)?;
    let query_limits = authority.authority.limits();
    let plan = plan.bounded_snapshot(query_limits.codec.max_bytes.saturating_add(1));
    let invocation_json = invocation_json.bounded_snapshot(query_limits.invocation_bytes())?;
    let advertisement = advertisement.bounded_snapshot(MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1));
    let authority = Arc::clone(&authority.authority);
    let pending = py
        .detach(move || {
            prepare_remote_query_v2(&authority, &plan, &invocation_json, &advertisement, limits)
        })
        .map_err(|diagnostic| value_error(&diagnostic))?;
    Ok(PyPendingQueryV2Remote { pending })
}

/// Register the retained V2 execution surface.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyPendingQueryV2Remote>()?;
    m.add_function(wrap_pyfunction!(query_v2_execute_local, m)?)?;
    m.add_function(wrap_pyfunction!(query_v2_remote_capabilities, m)?)?;
    m.add_function(wrap_pyfunction!(query_v2_prepare_remote, m)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_worker_panics_become_stable_python_runtime_errors() {
        Python::initialize();
        let error = contain_local_worker_panic(|| -> () {
            panic!("provider panic detail must not cross FFI");
        })
        .expect_err("panic is contained");
        Python::attach(|py| {
            assert!(error.is_instance_of::<pyo3::exceptions::PyRuntimeError>(py));
            assert_eq!(
                error
                    .value(py)
                    .str()
                    .expect("exception string")
                    .to_str()
                    .expect("UTF-8 exception"),
                LOCAL_WORKER_FAILED,
            );
        });
    }
}
