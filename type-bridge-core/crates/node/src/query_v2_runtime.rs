//! Node projection of the prepared V2 query facade.
//!
//! Exactly three things cross this boundary: canonical declared-schema
//! bytes (once, into an opaque authority handle), canonical plan bytes,
//! and small JSON payloads for invocations and typed outcomes. Local
//! execution and the remote envelope share one authority, so a prepared
//! plan runs identically through either path.
//!
//! Typed Node authoring delegates every transition to the shared Rust
//! builder and passes its canonical plan bytes directly into this surface.

use std::sync::Arc;

use napi::NapiRaw;
use napi::bindgen_prelude::{AsyncTask, BigInt, Buffer, Env, FromNapiValue, Unknown};
use napi_derive::napi;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::limits::{
    MAX_CANONICAL_BYTES, MAX_CANONICAL_STRING_BYTES, MAX_QUERY_INVOCATION_BYTES,
    MAX_REMOTE_ENVELOPE_BYTES,
};
use type_bridge_contract::query_remote::{
    RemoteLimits, checked_remote_deadline, checked_remote_limit, remote_deadline_limit,
    remote_limit_invalid,
};
use type_bridge_orm::query_v2_prepared::{
    ClaimedRemoteReply, PendingRemoteQuery, QueryAuthority, decode_remote_capabilities,
    execute_prepared_local, prepare_remote_query, query_v2_host_string_unicode_error,
};
use type_bridge_orm::session::backend::{BoundedAnswerLimits, QueryV2AnswerLimits};

use crate::NodeRustDatabase;

const MAX_SEMANTIC_PROFILE_ID_BYTES: usize = 255;

pub(crate) fn napi_error(diagnostic: &Diagnostic) -> napi::Error {
    napi::Error::from_reason(serde_json::to_string(diagnostic).unwrap_or_else(|_| {
        r#"{"category":"invalid_contract","code":"query_v2_diagnostic_unencodable","message":"the structured query diagnostic could not be encoded","path":[],"details":{}}"#
            .to_owned()
    }))
}

fn binding_diagnostic(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static Node binding diagnostic code"),
        message,
    )
}

/// Opaque prepared-query schema authority handle.
#[napi]
pub struct NodeQueryV2Authority {
    authority: Arc<QueryAuthority>,
}

impl NodeQueryV2Authority {
    pub(crate) fn authority(&self) -> Arc<QueryAuthority> {
        Arc::clone(&self.authority)
    }
}

/// One prepared request with an atomic one-shot reply decoder.
#[napi]
pub struct NodePendingQueryV2Remote {
    pending: Arc<PendingRemoteQuery>,
}

/// One remote reply decode scheduled on the libuv worker pool.
pub struct DecodeRemoteReplyTask {
    state: DecodeRemoteReplyTaskState,
}

enum DecodeRemoteReplyTaskState {
    Claimed {
        claimed: Option<ClaimedRemoteReply>,
        response: Vec<u8>,
    },
    Rejected {
        reason: Option<String>,
    },
}

pub(crate) fn bounded_response_snapshot(response: &[u8], limit: usize) -> Vec<u8> {
    response[..response.len().min(limit)].to_vec()
}

fn bounded_buffer(buffer: &Buffer, limit: usize) -> &[u8] {
    &buffer[..buffer.len().min(limit)]
}

pub(crate) enum BoundedNodeString {
    Value(String),
    Oversized,
    InvalidUnicode,
}

/// Snapshot one JavaScript string with Python-equivalent size/error
/// precedence. Python counts Unicode code points before attempting UTF-8
/// conversion, and an unpaired surrogate still counts as one code point.
/// JavaScript exposes UTF-16 code units, so count them explicitly before
/// deciding whether size or invalid Unicode wins.
pub(crate) unsafe fn bounded_node_string_snapshot(
    env: napi::sys::napi_env,
    value: napi::sys::napi_value,
    limit: usize,
) -> napi::Result<BoundedNodeString> {
    let mut utf16_length = 0_usize;
    napi::check_status!(
        unsafe {
            napi::sys::napi_get_value_string_utf16(
                env,
                value,
                std::ptr::null_mut(),
                0,
                &mut utf16_length,
            )
        },
        "Failed to inspect UTF-16 string input"
    )?;

    // Every Unicode code point occupies at most two UTF-16 code units. Above
    // this bound the Python code-point preflight must also classify the value
    // as oversized, so no UTF-16 allocation is needed.
    if utf16_length > limit.saturating_mul(2) {
        return Ok(BoundedNodeString::Oversized);
    }

    let mut utf16 = vec![0_u16; utf16_length.saturating_add(1)];
    let mut written = 0_usize;
    napi::check_status!(
        unsafe {
            napi::sys::napi_get_value_string_utf16(
                env,
                value,
                utf16.as_mut_ptr(),
                utf16.len(),
                &mut written,
            )
        },
        "Failed to copy UTF-16 string input"
    )?;
    if written != utf16_length {
        return Ok(BoundedNodeString::InvalidUnicode);
    }

    let mut code_points = 0_usize;
    let mut invalid_unicode = false;
    let mut index = 0_usize;
    while index < written {
        let unit = utf16[index];
        if (0xD800..=0xDBFF).contains(&unit) {
            if index + 1 < written && (0xDC00..=0xDFFF).contains(&utf16[index + 1]) {
                index += 2;
            } else {
                invalid_unicode = true;
                index += 1;
            }
        } else {
            if (0xDC00..=0xDFFF).contains(&unit) {
                invalid_unicode = true;
            }
            index += 1;
        }
        code_points = code_points.saturating_add(1);
    }

    if code_points > limit {
        return Ok(BoundedNodeString::Oversized);
    }
    if invalid_unicode {
        return Ok(BoundedNodeString::InvalidUnicode);
    }

    let mut utf8_length = 0_usize;
    napi::check_status!(
        unsafe {
            napi::sys::napi_get_value_string_utf8(
                env,
                value,
                std::ptr::null_mut(),
                0,
                &mut utf8_length,
            )
        },
        "Failed to inspect UTF-8 string input"
    )?;
    if utf8_length > limit {
        return Ok(BoundedNodeString::Oversized);
    }

    unsafe { String::from_napi_value(env, value) }.map(BoundedNodeString::Value)
}

/// Measure one JavaScript string before napi-rs allocates its Rust copy.
///
/// Node already owns the immutable JavaScript string, but the normal
/// `FromNapiValue for String` path allocates its complete UTF-8 representation
/// before semantic code can enforce its input ceiling. Inspecting the encoded
/// length through Node-API first keeps the boundary allocation bounded while
/// preserving the semantic diagnostic for an oversized value.
pub(crate) fn bounded_string(
    env: &Env,
    value: Unknown,
    limit: usize,
    oversized: impl FnOnce() -> Diagnostic,
) -> napi::Result<String> {
    bounded_string_with_unicode_error(
        env,
        value,
        limit,
        oversized,
        query_v2_host_string_unicode_error,
    )
}

/// Copy one bounded JavaScript string while rejecting lone UTF-16 surrogates
/// before Node-API can replacement-encode them into a different Rust string.
pub(crate) fn bounded_string_with_unicode_error(
    env: &Env,
    value: Unknown,
    limit: usize,
    oversized: impl FnOnce() -> Diagnostic,
    invalid_unicode: impl FnOnce() -> Diagnostic,
) -> napi::Result<String> {
    match unsafe { bounded_node_string_snapshot(env.raw(), value.raw(), limit) }? {
        BoundedNodeString::Value(value) => Ok(value),
        BoundedNodeString::Oversized => Err(napi_error(&oversized())),
        BoundedNodeString::InvalidUnicode => Err(napi_error(&invalid_unicode())),
    }
}

fn invocation_input_too_large() -> Diagnostic {
    binding_diagnostic(
        DiagnosticCategory::ResourceLimit,
        "query_invocation_input_byte_limit",
        "invocation input rows exceed the structural byte ceiling",
    )
}

fn managed_scope_id_too_large() -> Diagnostic {
    binding_diagnostic(
        DiagnosticCategory::InvalidContract,
        "malformed_managed_scope_id",
        "managed scope ID is empty or exceeds the canonical string limit",
    )
    .with_detail(
        "maximum_bytes",
        i64::try_from(MAX_CANONICAL_STRING_BYTES).unwrap_or(i64::MAX),
    )
}

fn semantic_profile_id_too_large() -> Diagnostic {
    binding_diagnostic(
        DiagnosticCategory::Integrity,
        "invalid_fingerprint_identifier",
        "fingerprint metadata identifier is malformed",
    )
    .with_detail("identifier_kind", "SemanticProfileId")
}

/// Validate a raw addon byte argument before napi-rs constructs a Rust slice.
///
/// A Node Buffer can be backed by SharedArrayBuffer. Another Worker may then
/// mutate the bytes while Rust holds the ordinary `&[u8]` exposed by napi-rs,
/// which violates Rust's aliasing model. Inspect the TypedArray's actual
/// backing store through stable N-API metadata first, reject shared storage,
/// and only then let napi-rs construct a Buffer view. The package facade may
/// copy shared input for convenience; direct `.node` consumers fail closed.
pub(crate) fn non_shared_buffer(env: &Env, value: Unknown) -> napi::Result<Buffer> {
    if !value.is_buffer()? {
        return Err(napi::Error::new(
            napi::Status::InvalidArg,
            "Expected a Buffer value".to_owned(),
        ));
    }

    let mut typed_array_type = 0;
    let mut length = 0_usize;
    let mut data = std::ptr::null_mut();
    let mut backing = std::ptr::null_mut();
    let mut byte_offset = 0_usize;
    napi::check_status!(
        unsafe {
            napi::sys::napi_get_typedarray_info(
                env.raw(),
                value.raw(),
                &mut typed_array_type,
                &mut length,
                &mut data,
                &mut backing,
                &mut byte_offset,
            )
        },
        "Failed to inspect Buffer backing storage"
    )?;
    let mut ordinary_array_buffer = false;
    napi::check_status!(
        unsafe { napi::sys::napi_is_arraybuffer(env.raw(), backing, &mut ordinary_array_buffer) },
        "Failed to classify Buffer backing storage"
    )?;
    if !ordinary_array_buffer {
        return Err(napi::Error::new(
            napi::Status::InvalidArg,
            "query_v2_shared_buffer_unsupported: raw native byte inputs cannot use SharedArrayBuffer storage"
                .to_owned(),
        ));
    }

    Buffer::from_unknown(value)
}

impl napi::Task for DecodeRemoteReplyTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> napi::Result<String> {
        match &mut self.state {
            DecodeRemoteReplyTaskState::Claimed { claimed, response } => claimed
                .take()
                .ok_or_else(|| napi::Error::from_reason("remote reply task already ran"))?
                .decode(response)
                .map_err(|diagnostic| napi_error(&diagnostic)),
            DecodeRemoteReplyTaskState::Rejected { reason } => {
                Err(napi::Error::from_reason(reason.take().unwrap_or_else(
                    || "remote reply task already ran".to_owned(),
                )))
            }
        }
    }

    fn resolve(&mut self, _env: napi::Env, output: String) -> napi::Result<String> {
        Ok(output)
    }
}

#[napi]
impl NodePendingQueryV2Remote {
    /// Return the exact canonical bytes to send to `/v2/query`.
    #[napi(js_name = "requestBytes")]
    pub fn request_bytes(&self) -> Buffer {
        self.pending.request_bytes().to_vec().into()
    }

    /// Consume the only reply accepted and verify its request binding.
    ///
    /// The immutable byte snapshot is bounded to one byte beyond the protocol
    /// hard ceiling so oversized inputs retain their canonical rejection
    /// without copying attacker-controlled bytes without limit. The caller's
    /// success-byte budget applies only after authentication and reply-kind
    /// classification, so it cannot truncate authenticated failure evidence.
    /// Parsing and outcome construction then run on the libuv worker pool.
    #[napi(js_name = "decodeReply", ts_return_type = "Promise<string>")]
    pub fn decode_reply(
        &self,
        env: Env,
        response: Unknown,
    ) -> napi::Result<AsyncTask<DecodeRemoteReplyTask>> {
        let state = match self.pending.claim_reply() {
            Ok(claimed) => {
                let snapshot_limit = claimed.response_snapshot_limit();
                let response = non_shared_buffer(&env, response)?;
                DecodeRemoteReplyTaskState::Claimed {
                    claimed: Some(claimed),
                    response: bounded_response_snapshot(&response, snapshot_limit),
                }
            }
            Err(diagnostic) => DecodeRemoteReplyTaskState::Rejected {
                reason: Some(format!(
                    "{}: {}",
                    diagnostic.code().as_str(),
                    diagnostic.message()
                )),
            },
        };
        Ok(AsyncTask::new(DecodeRemoteReplyTask { state }))
    }
}

/// Build one authority from canonical declared-schema bytes.
#[napi(js_name = "queryV2Authority")]
pub fn query_v2_authority(
    env: Env,
    declared_schema: Unknown,
    scope: Unknown,
    profile: Unknown,
) -> napi::Result<NodeQueryV2Authority> {
    let declared_schema = non_shared_buffer(&env, declared_schema)?;
    let scope = bounded_string(
        &env,
        scope,
        MAX_CANONICAL_STRING_BYTES,
        managed_scope_id_too_large,
    )?;
    let profile = bounded_string(
        &env,
        profile,
        MAX_SEMANTIC_PROFILE_ID_BYTES,
        semantic_profile_id_too_large,
    )?;
    let authority = QueryAuthority::from_declared_bytes(
        bounded_buffer(&declared_schema, MAX_CANONICAL_BYTES.saturating_add(1)),
        &scope,
        &profile,
    )
    .map_err(|diagnostic| napi_error(&diagnostic))?;
    Ok(NodeQueryV2Authority {
        authority: Arc::new(authority),
    })
}

/// Build a local-only authority for a database with no migration controls.
#[napi(js_name = "queryV2QueryOnlyAuthority")]
pub fn query_v2_query_only_authority(
    env: Env,
    database: &NodeRustDatabase,
    declared_schema: Unknown,
    scope: Unknown,
    profile: Unknown,
) -> napi::Result<NodeQueryV2Authority> {
    let declared_schema = non_shared_buffer(&env, declared_schema)?;
    let scope = bounded_string(
        &env,
        scope,
        MAX_CANONICAL_STRING_BYTES,
        managed_scope_id_too_large,
    )?;
    let profile = bounded_string(
        &env,
        profile,
        MAX_SEMANTIC_PROFILE_ID_BYTES,
        semantic_profile_id_too_large,
    )?;
    let (database, _) = database.handles();
    let authority = QueryAuthority::from_declared_bytes_query_only(
        bounded_buffer(&declared_schema, MAX_CANONICAL_BYTES.saturating_add(1)),
        &scope,
        &profile,
        &database,
    )
    .map_err(|diagnostic| napi_error(&diagnostic))?;
    Ok(NodeQueryV2Authority {
        authority: Arc::new(authority),
    })
}

/// Execute one prepared plan locally; resolves to typed outcome JSON.
///
/// Provider I/O runs on the database handle's private Tokio runtime. Only the
/// final deferred resolution enters Node, so a stalled provider cannot consume
/// one of libuv's shared blocking-worker slots indefinitely.
#[napi(js_name = "queryV2ExecuteLocal", ts_return_type = "Promise<string>")]
pub fn query_v2_execute_local(
    env: Env,
    database: &NodeRustDatabase,
    authority: &NodeQueryV2Authority,
    plan: Unknown,
    invocation_json: Unknown,
    deadline_ms: Option<BigInt>,
) -> napi::Result<napi::JsObject> {
    let plan = non_shared_buffer(&env, plan)?;
    let invocation_json = bounded_string(
        &env,
        invocation_json,
        MAX_QUERY_INVOCATION_BYTES,
        invocation_input_too_large,
    )?;
    let deadline = checked_remote_deadline(
        deadline_ms
            .as_ref()
            .map(|value| remote_limit(value).map(i128::from))
            .transpose()
            .map_err(|diagnostic| napi_error(&diagnostic))?,
    )
    .map_err(|diagnostic| napi_error(&diagnostic))?
    .map(|ms| {
        std::time::Instant::now()
            .checked_add(std::time::Duration::from_millis(ms))
            .ok_or_else(remote_deadline_limit)
    })
    .transpose()
    .map_err(|diagnostic| napi_error(&diagnostic))?;
    let limits = QueryV2AnswerLimits {
        answer: BoundedAnswerLimits {
            deadline,
            ..BoundedAnswerLimits::default()
        },
        ..QueryV2AnswerLimits::default()
    };
    let (database, runtime) = database.handles();
    let authority = Arc::clone(&authority.authority);
    let plan = bounded_response_snapshot(&plan, MAX_CANONICAL_BYTES.saturating_add(1));
    let (deferred, promise) = env.create_deferred::<String, _>()?;

    let execution = runtime.spawn(async move {
        execute_prepared_local(&database, &authority, &plan, &invocation_json, limits).await
    });
    // Keep the private executor alive through completion. Awaiting the first
    // task as a JoinHandle also converts an unexpected Rust panic into a
    // rejected promise instead of leaving JavaScript waiting forever.
    let runtime_owner = Arc::clone(&runtime);
    runtime.spawn(async move {
        match execution.await {
            Ok(Ok(output)) => deferred.resolve(move |_env| Ok(output)),
            Ok(Err(diagnostic)) => deferred.reject(napi_error(&diagnostic)),
            Err(_) => deferred.reject(napi::Error::from_reason(
                "query_v2_local_worker_failed: local provider worker terminated unexpectedly",
            )),
        }
        drop(runtime_owner);
    });
    Ok(promise)
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
    max_collection_members: &BigInt,
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
            max_collection_members: remote_limit(max_collection_members)?,
        })
    };
    build().map_err(|diagnostic| napi_error(&diagnostic))
}

/// Decode one capability advertisement into its sorted capability ids.
#[napi(js_name = "queryV2RemoteCapabilities")]
pub fn query_v2_remote_capabilities(env: Env, advertisement: Unknown) -> napi::Result<Vec<String>> {
    let advertisement = non_shared_buffer(&env, advertisement)?;
    decode_remote_capabilities(bounded_buffer(
        &advertisement,
        MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1),
    ))
    .map_err(|diagnostic| napi_error(&diagnostic))
}

/// Prepare one remote invocation and its one-shot reply decoder.
///
/// `advertisement` carries the executor's exact `/v2/capabilities`
/// bytes; a plan or multi-row invocation the executor cannot execute is
/// refused here, before any request bytes exist.
/// `max_bytes` limits successful signed response bytes; authenticated failure
/// envelopes remain decodable under the protocol hard ceiling.
#[napi(js_name = "queryV2PrepareRemote")]
#[allow(clippy::too_many_arguments, reason = "flat binding surface")]
pub fn query_v2_prepare_remote(
    env: Env,
    authority: &NodeQueryV2Authority,
    plan: Unknown,
    invocation_json: Unknown,
    advertisement: Unknown,
    max_items: BigInt,
    max_bytes: BigInt,
    max_collection_members: BigInt,
    deadline_ms: Option<BigInt>,
) -> napi::Result<NodePendingQueryV2Remote> {
    let plan = non_shared_buffer(&env, plan)?;
    let invocation_json = bounded_string(
        &env,
        invocation_json,
        MAX_QUERY_INVOCATION_BYTES,
        invocation_input_too_large,
    )?;
    let advertisement = non_shared_buffer(&env, advertisement)?;
    let limits = remote_limits(
        &max_items,
        &max_bytes,
        &max_collection_members,
        deadline_ms.as_ref(),
    )?;
    let pending = prepare_remote_query(
        &authority.authority,
        bounded_buffer(&plan, MAX_CANONICAL_BYTES.saturating_add(1)),
        &invocation_json,
        bounded_buffer(&advertisement, MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1)),
        limits,
    )
    .map_err(|diagnostic| napi_error(&diagnostic))?;
    Ok(NodePendingQueryV2Remote {
        pending: Arc::new(pending),
    })
}

#[cfg(test)]
mod tests {
    use super::bounded_response_snapshot;
    use type_bridge_contract::limits::MAX_REMOTE_ENVELOPE_BYTES;

    #[test]
    fn reply_snapshot_never_exceeds_the_supplied_budget_marker() {
        let response = vec![0x5a; 4_096];
        let snapshot = bounded_response_snapshot(&response, 257);
        assert_eq!(snapshot.len(), 257);
        assert_eq!(snapshot, response[..257]);
    }

    #[test]
    fn reply_snapshot_copies_only_the_hard_ceiling_oversize_marker() {
        let response = vec![0x5a; MAX_REMOTE_ENVELOPE_BYTES + 4_096];
        let snapshot =
            bounded_response_snapshot(&response, MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1));
        assert_eq!(snapshot.len(), MAX_REMOTE_ENVELOPE_BYTES + 1);
        assert_eq!(snapshot, response[..MAX_REMOTE_ENVELOPE_BYTES + 1]);
    }

    #[test]
    fn local_provider_wait_does_not_use_napi_async_work() {
        let source = include_str!("query_v2_runtime.rs");
        let start = source
            .find("pub fn query_v2_execute_local(")
            .expect("local entry point");
        let end = source[start..]
            .find("fn remote_limit(")
            .map(|offset| start + offset)
            .expect("next entry point");
        let local = &source[start..end];

        assert!(local.contains("create_deferred"));
        assert!(local.contains("runtime.spawn"));
        assert!(!local.contains("AsyncTask"));
        assert!(!local.contains("block_on"));
    }
}
