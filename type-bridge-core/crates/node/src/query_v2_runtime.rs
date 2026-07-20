//! Node projection of the prepared V2 query facade.
//!
//! Exactly three things cross this boundary: canonical declared-schema
//! bytes (once, into an opaque authority handle), canonical plan bytes,
//! and small JSON payloads for invocations and typed outcomes. Local
//! execution and the remote envelope share one authority, so a prepared
//! plan runs identically through either path.

use std::sync::Arc;

use napi::bindgen_prelude::Buffer;
use napi_derive::napi;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::query_remote::RemoteLimits;
use type_bridge_orm::query_v2_prepared::{
    QueryAuthority, decode_prepared_remote_outcome, encode_prepared_remote_request,
    execute_prepared_local,
};
use type_bridge_orm::session::backend::BoundedAnswerLimits;

use crate::NodeRustDatabase;

fn napi_error(diagnostic: &Diagnostic) -> napi::Error {
    napi::Error::from_reason(format!(
        "{}: {}",
        diagnostic.code().as_str(),
        diagnostic.message(),
    ))
}

/// Opaque prepared-query schema authority handle.
#[napi]
pub struct NodeQueryV2Authority {
    authority: Arc<QueryAuthority>,
}

/// Build one authority from canonical declared-schema bytes.
#[napi(js_name = "queryV2Authority")]
pub fn query_v2_authority(
    declared_schema: Buffer,
    scope: String,
    profile: String,
) -> napi::Result<NodeQueryV2Authority> {
    let authority = QueryAuthority::from_declared_bytes(&declared_schema, &scope, &profile)
        .map_err(|diagnostic| napi_error(&diagnostic))?;
    Ok(NodeQueryV2Authority {
        authority: Arc::new(authority),
    })
}

/// Execute one prepared plan locally; returns typed outcome JSON.
#[napi(js_name = "queryV2ExecuteLocal")]
pub fn query_v2_execute_local(
    database: &NodeRustDatabase,
    authority: &NodeQueryV2Authority,
    plan: Buffer,
    invocation_json: String,
) -> napi::Result<String> {
    let (db, runtime) = database.handles();
    runtime
        .block_on(execute_prepared_local(
            &db,
            &authority.authority,
            &plan,
            &invocation_json,
            BoundedAnswerLimits::default(),
        ))
        .map_err(|diagnostic| napi_error(&diagnostic))
}

/// Encode one prepared invocation into remote request envelope bytes.
#[napi(js_name = "queryV2EncodeRemoteRequest")]
pub fn query_v2_encode_remote_request(
    authority: &NodeQueryV2Authority,
    plan: Buffer,
    invocation_json: String,
    nonce: String,
    max_items: i64,
    max_bytes: i64,
    deadline_ms: Option<i64>,
) -> napi::Result<Buffer> {
    let bytes = encode_prepared_remote_request(
        &authority.authority,
        &plan,
        &invocation_json,
        RemoteLimits {
            deadline_ms: deadline_ms.and_then(|value| u64::try_from(value).ok()),
            max_bytes: u64::try_from(max_bytes).unwrap_or(0),
            max_items: u64::try_from(max_items).unwrap_or(0),
        },
        &nonce,
    )
    .map_err(|diagnostic| napi_error(&diagnostic))?;
    Ok(bytes.into())
}

/// Decode one remote response into typed outcome JSON.
#[napi(js_name = "queryV2DecodeRemoteOutcome")]
pub fn query_v2_decode_remote_outcome(
    authority: &NodeQueryV2Authority,
    plan: Buffer,
    invocation_json: String,
    response: Buffer,
    nonce: String,
    max_items: i64,
    max_bytes: i64,
) -> napi::Result<String> {
    decode_prepared_remote_outcome(
        &authority.authority,
        &plan,
        &invocation_json,
        &response,
        &nonce,
        RemoteLimits {
            deadline_ms: None,
            max_bytes: u64::try_from(max_bytes).unwrap_or(0),
            max_items: u64::try_from(max_items).unwrap_or(0),
        },
    )
    .map_err(|diagnostic| napi_error(&diagnostic))
}
