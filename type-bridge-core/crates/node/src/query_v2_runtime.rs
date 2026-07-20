//! Node projection of the prepared V2 query facade.
//!
//! Exactly three things cross this boundary: canonical declared-schema
//! bytes (once, into an opaque authority handle), canonical plan bytes,
//! and small JSON payloads for invocations and typed outcomes. Local
//! execution and the remote envelope share one authority, so a prepared
//! plan runs identically through either path.

use std::sync::Arc;

use napi::bindgen_prelude::{AsyncTask, BigInt, Buffer};
use napi_derive::napi;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::query_remote::{
    RemoteLimits, checked_remote_deadline, checked_remote_limit, remote_limit_invalid,
};
use type_bridge_orm::query_v2_prepared::{
    QueryAuthority, decode_prepared_remote_outcome, decode_remote_capabilities,
    encode_prepared_remote_request, execute_prepared_local,
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

/// One local prepared execution scheduled off the JavaScript thread.
///
/// `compute` runs on the libuv worker pool, so a slow or stalled
/// provider never freezes timers or request handling on the main
/// thread; the optional deadline bounds the round trip itself.
pub struct ExecuteLocalTask {
    authority: Arc<QueryAuthority>,
    database: Arc<type_bridge_orm::Database>,
    invocation_json: String,
    limits: BoundedAnswerLimits,
    plan: Vec<u8>,
    runtime: Arc<tokio::runtime::Runtime>,
}

impl napi::Task for ExecuteLocalTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> napi::Result<String> {
        self.runtime
            .block_on(execute_prepared_local(
                &self.database,
                &self.authority,
                &self.plan,
                &self.invocation_json,
                self.limits.clone(),
            ))
            .map_err(|diagnostic| napi_error(&diagnostic))
    }

    fn resolve(&mut self, _env: napi::Env, output: String) -> napi::Result<String> {
        Ok(output)
    }
}

/// Execute one prepared plan locally; resolves to typed outcome JSON.
#[napi(js_name = "queryV2ExecuteLocal", ts_return_type = "Promise<string>")]
pub fn query_v2_execute_local(
    database: &NodeRustDatabase,
    authority: &NodeQueryV2Authority,
    plan: Buffer,
    invocation_json: String,
    deadline_ms: Option<BigInt>,
) -> napi::Result<AsyncTask<ExecuteLocalTask>> {
    let deadline = deadline_ms
        .as_ref()
        .map(remote_limit)
        .transpose()
        .map_err(|diagnostic| napi_error(&diagnostic))?
        .map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(ms));
    let limits = BoundedAnswerLimits {
        deadline,
        ..BoundedAnswerLimits::default()
    };
    let (db, runtime) = database.handles();
    Ok(AsyncTask::new(ExecuteLocalTask {
        authority: Arc::clone(&authority.authority),
        database: db,
        invocation_json,
        limits,
        plan: plan.to_vec(),
        runtime,
    }))
}

/// Convert one caller-supplied `BigInt` limit through the shared range check.
///
/// `BigInt` preserves the full unsigned 64-bit range without JavaScript
/// number precision loss; a negative or out-of-range value fails with
/// the same stable diagnostic the Python binding reports.
fn remote_limit(value: &BigInt) -> Result<u64, Diagnostic> {
    let (value, lossless) = value.get_i128();
    if lossless {
        checked_remote_limit(value)
    } else {
        Err(remote_limit_invalid())
    }
}

/// Build the exact remote limit set from checked caller arguments.
fn remote_limits(
    max_items: &BigInt,
    max_bytes: &BigInt,
    deadline_ms: Option<&BigInt>,
) -> napi::Result<RemoteLimits> {
    let build = || -> Result<RemoteLimits, Diagnostic> {
        Ok(RemoteLimits {
            deadline_ms: checked_remote_deadline(
                deadline_ms
                    .map(|value| remote_limit(value).map(i128::from))
                    .transpose()?,
            )?,
            max_bytes: remote_limit(max_bytes)?,
            max_items: remote_limit(max_items)?,
        })
    };
    build().map_err(|diagnostic| napi_error(&diagnostic))
}

/// Decode one capability advertisement into its sorted capability ids.
#[napi(js_name = "queryV2RemoteCapabilities")]
pub fn query_v2_remote_capabilities(advertisement: Buffer) -> napi::Result<Vec<String>> {
    decode_remote_capabilities(&advertisement).map_err(|diagnostic| napi_error(&diagnostic))
}

/// Encode one prepared invocation into remote request envelope bytes.
///
/// `advertisement` carries the executor's exact `/v2/capabilities`
/// bytes; a plan or multi-row invocation the executor cannot execute is
/// refused here, before any request bytes exist.
#[napi(js_name = "queryV2EncodeRemoteRequest")]
#[allow(clippy::too_many_arguments, reason = "flat binding surface")]
pub fn query_v2_encode_remote_request(
    authority: &NodeQueryV2Authority,
    plan: Buffer,
    invocation_json: String,
    advertisement: Buffer,
    nonce: String,
    max_items: BigInt,
    max_bytes: BigInt,
    deadline_ms: Option<BigInt>,
) -> napi::Result<Buffer> {
    let limits = remote_limits(&max_items, &max_bytes, deadline_ms.as_ref())?;
    let bytes = encode_prepared_remote_request(
        &authority.authority,
        &plan,
        &invocation_json,
        &advertisement,
        limits,
        &nonce,
    )
    .map_err(|diagnostic| napi_error(&diagnostic))?;
    Ok(bytes.into())
}

/// Decode one remote reply into typed outcome JSON.
///
/// The limit arguments must repeat the exact budgets the request was
/// encoded with — including the deadline — because the reply binds the
/// whole request envelope, budgets included.
#[napi(js_name = "queryV2DecodeRemoteOutcome")]
#[allow(clippy::too_many_arguments, reason = "flat binding surface")]
pub fn query_v2_decode_remote_outcome(
    authority: &NodeQueryV2Authority,
    plan: Buffer,
    invocation_json: String,
    response: Buffer,
    nonce: String,
    max_items: BigInt,
    max_bytes: BigInt,
    deadline_ms: Option<BigInt>,
) -> napi::Result<String> {
    let limits = remote_limits(&max_items, &max_bytes, deadline_ms.as_ref())?;
    decode_prepared_remote_outcome(
        &authority.authority,
        &plan,
        &invocation_json,
        &response,
        &nonce,
        limits,
    )
    .map_err(|diagnostic| napi_error(&diagnostic))
}
