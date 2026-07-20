//! Python projection of the prepared V2 query facade.
//!
//! Exactly three things cross this boundary: canonical declared-schema
//! bytes (once, into an opaque authority handle), canonical plan bytes,
//! and small JSON payloads for invocations and typed outcomes. Local
//! execution and the remote envelope share one authority, so a prepared
//! plan runs identically through either path.

use std::sync::Arc;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::query_remote::{
    RemoteLimits, checked_remote_deadline, checked_remote_limit,
};
use type_bridge_orm::query_v2_prepared::{
    QueryAuthority, decode_prepared_remote_outcome, decode_remote_capabilities,
    encode_prepared_remote_request, execute_prepared_local,
};
use type_bridge_orm::session::backend::BoundedAnswerLimits;

use crate::orm_runtime::PyRustDatabase;

fn value_error(diagnostic: &Diagnostic) -> PyErr {
    PyValueError::new_err(format!(
        "{}: {}",
        diagnostic.code().as_str(),
        diagnostic.message(),
    ))
}

/// Opaque prepared-query schema authority handle.
#[pyclass(name = "QueryV2Authority", frozen)]
pub struct PyQueryV2Authority {
    authority: Arc<QueryAuthority>,
}

/// Build one authority from canonical declared-schema bytes.
#[pyfunction]
pub fn query_v2_authority(
    declared_schema: Vec<u8>,
    scope: &str,
    profile: &str,
) -> PyResult<PyQueryV2Authority> {
    let authority = QueryAuthority::from_declared_bytes(&declared_schema, scope, profile)
        .map_err(|diagnostic| value_error(&diagnostic))?;
    Ok(PyQueryV2Authority {
        authority: Arc::new(authority),
    })
}

/// Build the local execution budget from a checked optional deadline.
fn local_limits(deadline_ms: Option<i128>) -> PyResult<BoundedAnswerLimits> {
    let deadline = checked_remote_deadline(deadline_ms)
        .map_err(|diagnostic| value_error(&diagnostic))?
        .map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(ms));
    Ok(BoundedAnswerLimits {
        deadline,
        ..BoundedAnswerLimits::default()
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
    database: &PyRustDatabase,
    authority: &PyQueryV2Authority,
    plan: Vec<u8>,
    invocation_json: String,
    deadline_ms: Option<i128>,
) -> PyResult<String> {
    let limits = local_limits(deadline_ms)?;
    let (db, runtime) = database.handles();
    let authority = Arc::clone(&authority.authority);
    py.allow_threads(move || {
        runtime.block_on(execute_prepared_local(
            &db,
            &authority,
            &plan,
            &invocation_json,
            limits,
        ))
    })
    .map_err(|diagnostic| value_error(&diagnostic))
}

/// Build the exact remote limit set from checked caller arguments.
///
/// Limits accept any Python integer and convert through the shared
/// contract range check, so a negative or oversized budget fails with
/// the same stable diagnostic the Node binding reports.
fn remote_limits(
    max_items: i128,
    max_bytes: i128,
    deadline_ms: Option<i128>,
) -> PyResult<RemoteLimits> {
    let build = || -> Result<RemoteLimits, Diagnostic> {
        Ok(RemoteLimits {
            deadline_ms: checked_remote_deadline(deadline_ms)?,
            max_bytes: checked_remote_limit(max_bytes)?,
            max_items: checked_remote_limit(max_items)?,
        })
    };
    build().map_err(|diagnostic| value_error(&diagnostic))
}

/// Decode one capability advertisement into its sorted capability ids.
#[pyfunction]
pub fn query_v2_remote_capabilities(advertisement: Vec<u8>) -> PyResult<Vec<String>> {
    decode_remote_capabilities(&advertisement).map_err(|diagnostic| value_error(&diagnostic))
}

/// Encode one prepared invocation into remote request envelope bytes.
///
/// `advertisement` carries the executor's exact `/v2/capabilities`
/// bytes; a plan or multi-row invocation the executor cannot execute is
/// refused here, before any request bytes exist.
#[pyfunction]
#[pyo3(signature = (authority, plan, invocation_json, advertisement, nonce, max_items, max_bytes, deadline_ms=None))]
#[expect(clippy::too_many_arguments, reason = "flat binding surface")]
pub fn query_v2_encode_remote_request(
    authority: &PyQueryV2Authority,
    plan: Vec<u8>,
    invocation_json: &str,
    advertisement: Vec<u8>,
    nonce: &str,
    max_items: i128,
    max_bytes: i128,
    deadline_ms: Option<i128>,
) -> PyResult<Vec<u8>> {
    let limits = remote_limits(max_items, max_bytes, deadline_ms)?;
    encode_prepared_remote_request(
        &authority.authority,
        &plan,
        invocation_json,
        &advertisement,
        limits,
        nonce,
    )
    .map_err(|diagnostic| value_error(&diagnostic))
}

/// Decode one remote reply into typed outcome JSON.
///
/// The limit arguments must repeat the exact budgets the request was
/// encoded with — including the deadline — because the reply binds the
/// whole request envelope, budgets included.
#[pyfunction]
#[pyo3(signature = (authority, plan, invocation_json, response, nonce, max_items, max_bytes, deadline_ms=None))]
#[expect(clippy::too_many_arguments, reason = "flat binding surface")]
pub fn query_v2_decode_remote_outcome(
    authority: &PyQueryV2Authority,
    plan: Vec<u8>,
    invocation_json: &str,
    response: Vec<u8>,
    nonce: &str,
    max_items: i128,
    max_bytes: i128,
    deadline_ms: Option<i128>,
) -> PyResult<String> {
    let limits = remote_limits(max_items, max_bytes, deadline_ms)?;
    decode_prepared_remote_outcome(
        &authority.authority,
        &plan,
        invocation_json,
        &response,
        nonce,
        limits,
    )
    .map_err(|diagnostic| value_error(&diagnostic))
}

/// Register the prepared V2 query surface on the native module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyQueryV2Authority>()?;
    m.add_function(wrap_pyfunction!(query_v2_authority, m)?)?;
    m.add_function(wrap_pyfunction!(query_v2_execute_local, m)?)?;
    m.add_function(wrap_pyfunction!(query_v2_remote_capabilities, m)?)?;
    m.add_function(wrap_pyfunction!(query_v2_encode_remote_request, m)?)?;
    m.add_function(wrap_pyfunction!(query_v2_decode_remote_outcome, m)?)?;
    Ok(())
}
