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
use type_bridge_contract::query_remote::RemoteLimits;
use type_bridge_orm::query_v2_prepared::{
    QueryAuthority, decode_prepared_remote_outcome, encode_prepared_remote_request,
    execute_prepared_local,
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

/// Execute one prepared plan locally; returns typed outcome JSON.
#[pyfunction]
pub fn query_v2_execute_local(
    database: &PyRustDatabase,
    authority: &PyQueryV2Authority,
    plan: Vec<u8>,
    invocation_json: &str,
) -> PyResult<String> {
    let (db, runtime) = database.handles();
    runtime
        .block_on(execute_prepared_local(
            &db,
            &authority.authority,
            &plan,
            invocation_json,
            BoundedAnswerLimits::default(),
        ))
        .map_err(|diagnostic| value_error(&diagnostic))
}

/// Encode one prepared invocation into remote request envelope bytes.
#[pyfunction]
#[pyo3(signature = (authority, plan, invocation_json, nonce, max_items, max_bytes, deadline_ms=None))]
pub fn query_v2_encode_remote_request(
    authority: &PyQueryV2Authority,
    plan: Vec<u8>,
    invocation_json: &str,
    nonce: &str,
    max_items: u64,
    max_bytes: u64,
    deadline_ms: Option<u64>,
) -> PyResult<Vec<u8>> {
    encode_prepared_remote_request(
        &authority.authority,
        &plan,
        invocation_json,
        RemoteLimits {
            deadline_ms,
            max_bytes,
            max_items,
        },
        nonce,
    )
    .map_err(|diagnostic| value_error(&diagnostic))
}

/// Decode one remote response into typed outcome JSON.
#[pyfunction]
pub fn query_v2_decode_remote_outcome(
    authority: &PyQueryV2Authority,
    plan: Vec<u8>,
    invocation_json: &str,
    response: Vec<u8>,
    nonce: &str,
    max_items: u64,
    max_bytes: u64,
) -> PyResult<String> {
    decode_prepared_remote_outcome(
        &authority.authority,
        &plan,
        invocation_json,
        &response,
        nonce,
        RemoteLimits {
            deadline_ms: None,
            max_bytes,
            max_items,
        },
    )
    .map_err(|diagnostic| value_error(&diagnostic))
}

/// Register the prepared V2 query surface on the native module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyQueryV2Authority>()?;
    m.add_function(wrap_pyfunction!(query_v2_authority, m)?)?;
    m.add_function(wrap_pyfunction!(query_v2_execute_local, m)?)?;
    m.add_function(wrap_pyfunction!(query_v2_encode_remote_request, m)?)?;
    m.add_function(wrap_pyfunction!(query_v2_decode_remote_outcome, m)?)?;
    Ok(())
}
