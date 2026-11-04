//! Retained V2 prepared-query execution bindings.

use crate::NodeRustDatabase;
use crate::plan_runtime::{
    NodeQueryV2Authority, bounded_buffer, bounded_response_snapshot, bounded_string,
    invocation_input_too_large, napi_error, non_shared_buffer,
};
use napi::bindgen_prelude::{AsyncTask, BigInt, Buffer, Env, JsValue, Object, Unknown};
use napi_derive::napi;
use std::sync::Arc;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::limits::MAX_REMOTE_ENVELOPE_BYTES;
use type_bridge_contract::query_remote::{
    checked_remote_deadline, checked_remote_limit, remote_deadline_limit, remote_limit_invalid,
};
use type_bridge_contract::query_remote_v2::RemoteLimitsV2;
use type_bridge_orm::query_v2_prepared::{
    ClaimedRemoteReplyV2, PendingRemoteQueryV2, decode_remote_capabilities, execute_prepared_local,
    prepare_remote_query_v2,
};
use type_bridge_orm::session::backend::{AnswerLimits, BoundedAnswerLimits};

/// One prepared request with an atomic one-shot reply decoder.
#[napi]
pub struct NodePendingQueryV2Remote {
    pending: Arc<PendingRemoteQueryV2>,
}

/// One remote reply decode scheduled on the libuv worker pool.
pub struct DecodeRemoteReplyTask {
    state: DecodeRemoteReplyTaskState,
}

enum DecodeRemoteReplyTaskState {
    Claimed {
        claimed: Option<ClaimedRemoteReplyV2>,
        response: Vec<u8>,
    },
    Rejected {
        reason: Option<String>,
    },
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

/// Execute one prepared plan locally; resolves to typed outcome JSON.
///
/// Provider I/O runs on the database handle's private Tokio runtime. Only the
/// final deferred resolution enters Node, so a stalled provider cannot consume
/// one of libuv's shared blocking-worker slots indefinitely.
#[napi(js_name = "queryV2ExecuteLocal", ts_return_type = "Promise<string>")]
pub fn query_v2_execute_local<'env>(
    env: Env,
    database: &NodeRustDatabase,
    authority: &NodeQueryV2Authority,
    plan: Unknown<'env>,
    invocation_json: Unknown<'env>,
    deadline_ms: Option<BigInt>,
) -> napi::Result<Object<'env>> {
    let plan = non_shared_buffer(&env, plan)?;
    let invocation_json = bounded_string(
        &env,
        invocation_json,
        authority.authority.limits().invocation_bytes(),
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
    let limits = AnswerLimits {
        answer: BoundedAnswerLimits {
            deadline,
            ..BoundedAnswerLimits::default()
        },
        ..AnswerLimits::default()
    };
    let (database, runtime) = database.handles();
    let authority = Arc::clone(&authority.authority);
    let plan =
        bounded_response_snapshot(&plan, authority.limits().codec.max_bytes.saturating_add(1));
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
    Ok(Object::from_raw(env.raw(), promise.raw()))
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
) -> napi::Result<RemoteLimitsV2> {
    let build = || -> Result<RemoteLimitsV2, Diagnostic> {
        // A low-level plan returns no hydration graph, so the graph budgets
        // keep the proxy (wire) defaults this remote request travels under.
        let policy = type_bridge_orm::QueryExecutionResourceLimits::default_for(
            type_bridge_orm::AnswerTransport::Proxy,
        );
        Ok(RemoteLimitsV2 {
            deadline_ms: checked_remote_deadline(
                deadline_ms
                    .map(|value| remote_limit(value).map(i128::from))
                    .transpose()?,
            )?,
            max_bytes: remote_limit(max_bytes)?,
            max_items: remote_limit(max_items)?,
            max_collection_members: remote_limit(max_collection_members)?,
            max_graph_nodes: policy.graph_nodes,
            max_attribute_values: policy.attribute_values,
            max_role_players: policy.role_players,
            max_statements: policy.statement_limit(),
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
        authority.authority.limits().invocation_bytes(),
        invocation_input_too_large,
    )?;
    let advertisement = non_shared_buffer(&env, advertisement)?;
    let limits = remote_limits(
        &max_items,
        &max_bytes,
        &max_collection_members,
        deadline_ms.as_ref(),
    )?;
    let pending = prepare_remote_query_v2(
        &authority.authority,
        bounded_buffer(
            &plan,
            authority
                .authority
                .limits()
                .codec
                .max_bytes
                .saturating_add(1),
        ),
        &invocation_json,
        bounded_buffer(&advertisement, MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1)),
        limits,
    )
    .map_err(|diagnostic| napi_error(&diagnostic))?;
    Ok(NodePendingQueryV2Remote {
        pending: Arc::new(pending),
    })
}
