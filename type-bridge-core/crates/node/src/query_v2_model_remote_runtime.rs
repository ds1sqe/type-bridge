//! N-API seam for one-exchange remote model-oriented typed queries.
//!
//! JavaScript retains the caller-owned async exchange. This module snapshots
//! immutable authority/capability/limit context, prepares released match
//! terminals through the shared V1-to-V2 adapter, and returns the same opaque
//! validated result proof consumed by direct typed queries.

use std::sync::Arc;

use napi::bindgen_prelude::{
    Array, AsyncTask, BigInt, Buffer, Env, FromNapiValue, Reference, Unknown,
};
use napi_derive::napi;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::limits::MAX_REMOTE_ENVELOPE_BYTES;
use type_bridge_contract::query_remote::{
    RemoteCapabilities, checked_remote_deadline, checked_remote_limit, remote_limit_invalid,
};
use type_bridge_contract::query_remote_v2::RemoteLimitsV2;
use type_bridge_orm::{
    AnswerCancellation, ClaimedRemoteModelReplyV2, PendingRemoteModelQueryV2,
    QueryExecutionDeadline, QueryExecutionResourceLimits, RemoteModelQueryV2Error, Window,
    lower_remote_query_diagnostic, prepare_remote_model_query_v2_with_budget,
    validate_public_order_term_count,
};

use crate::match_runtime::{
    NodeMatchBindingHandle, NodeMatchFieldHandle, NodeMatchOrderHandle, NodeMatchQueryHandle,
    NodeMatchResultContext, NodeQueryCancellation, NodeQueryExecutionResources,
    NodeValidatedMatchResultHandle, borrow_reduce_terms, napi_match_error, napi_sdk_diagnostic,
    order_handles, parse_cardinality, reduce_terms,
};
use crate::query_v2_runtime::{NodeQueryV2Authority, bounded_response_snapshot, non_shared_buffer};

/// Immutable native authority, advertisement, and explicit remote limit set.
#[napi]
pub struct NodeRemoteModelQueryContext {
    advertisement: Vec<u8>,
    authority: Arc<type_bridge_orm::query_v2_prepared::QueryAuthority>,
    limits: RemoteLimitsV2,
    resources: QueryExecutionResourceLimits,
    cancellation: AnswerCancellation,
}

/// One prepared model request with an atomic one-shot reply decoder.
#[napi]
pub struct NodePendingRemoteModelQuery {
    pending: Arc<PendingRemoteModelQueryV2>,
    result_context: NodeMatchResultContext,
    cancellation: AnswerCancellation,
}

/// One claimed response decode scheduled on the libuv worker pool.
pub struct DecodeRemoteModelReplyTask {
    state: DecodeRemoteModelReplyTaskState,
}

enum DecodeRemoteModelReplyTaskState {
    Claimed {
        claimed: Box<Option<ClaimedRemoteModelReplyV2>>,
        response: Vec<u8>,
        result_context: Option<NodeMatchResultContext>,
        cancellation: AnswerCancellation,
    },
    Rejected {
        error: Option<napi::Error>,
    },
}

impl napi::Task for DecodeRemoteModelReplyTask {
    type Output = NodeValidatedMatchResultHandle;
    type JsValue = NodeValidatedMatchResultHandle;

    fn compute(&mut self) -> napi::Result<Self::Output> {
        match &mut self.state {
            DecodeRemoteModelReplyTaskState::Claimed {
                claimed,
                response,
                result_context,
                cancellation,
            } => {
                let (request, result, _registry) = claimed
                    .take()
                    .ok_or_else(|| napi::Error::from_reason("remote model reply task already ran"))?
                    .decode_with_cancellation(response, cancellation)
                    .map_err(remote_model_error)?;
                Ok(result_context
                    .take()
                    .ok_or_else(|| napi::Error::from_reason("remote model reply task already ran"))?
                    .attach(request, result))
            }
            DecodeRemoteModelReplyTaskState::Rejected { error } => {
                Err(error.take().unwrap_or_else(|| {
                    napi::Error::from_reason("remote model reply task already ran")
                }))
            }
        }
    }

    fn resolve(&mut self, _env: napi::Env, output: Self::Output) -> napi::Result<Self::JsValue> {
        Ok(output)
    }
}

#[napi]
impl NodePendingRemoteModelQuery {
    /// Return an owned copy of the exact request bytes for caller transport.
    #[napi(js_name = "requestBytes")]
    pub fn request_bytes(&self) -> napi::Result<Buffer> {
        if self.pending.is_closed() {
            return Err(napi_sdk_diagnostic(
                type_bridge_orm::query_resource_closed_diagnostic(),
            ));
        }
        Ok(self.pending.request_bytes().to_vec().into())
    }

    /// Close this pending request before its reply slot is claimed.
    #[napi]
    pub fn close(&self) {
        self.pending.close();
    }

    /// Whether this request was explicitly closed before claim.
    #[napi(getter, js_name = "isClosed")]
    pub fn is_closed(&self) -> bool {
        self.pending.is_closed()
    }

    /// Claim before inspecting or copying response storage, then decode off
    /// the JavaScript thread to the ordinary opaque match-result proof.
    #[napi(
        js_name = "decodeReply",
        ts_return_type = "Promise<NodeValidatedMatchResultHandle>"
    )]
    pub fn decode_reply(
        &self,
        env: Env,
        response: Unknown,
    ) -> napi::Result<AsyncTask<DecodeRemoteModelReplyTask>> {
        // Complete N-API type validation and the bounded caller-owned copy
        // before the semantic one-shot claim, so FFI mistakes remain retryable.
        let response = non_shared_buffer(&env, response)?;
        let response =
            bounded_response_snapshot(&response, MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1));
        let state = match self
            .pending
            .claim_reply_with_cancellation(&self.cancellation)
        {
            Ok(claimed) => DecodeRemoteModelReplyTaskState::Claimed {
                claimed: Box::new(Some(claimed)),
                response,
                result_context: Some(self.result_context.clone()),
                cancellation: self.cancellation.clone(),
            },
            Err(error) => DecodeRemoteModelReplyTaskState::Rejected {
                error: Some(remote_model_error(error)),
            },
        };
        Ok(AsyncTask::new(DecodeRemoteModelReplyTask { state }))
    }
}

/// Snapshot and validate the non-I/O context shared by model-query terminals.
#[napi(js_name = "queryV2RemoteModelContext")]
#[allow(clippy::too_many_arguments, reason = "flat explicit limit contract")]
pub fn query_v2_remote_model_context(
    env: Env,
    authority: &NodeQueryV2Authority,
    advertisement: Unknown,
    max_items: Unknown,
    max_bytes: Unknown,
    max_collection_members: Unknown,
    max_graph_nodes: Unknown,
    max_attribute_values: Unknown,
    max_role_players: Unknown,
    deadline_ms: Option<Unknown>,
) -> napi::Result<NodeRemoteModelQueryContext> {
    let advertisement = non_shared_buffer(&env, advertisement)?;
    let advertisement =
        bounded_response_snapshot(&advertisement, MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1));
    let limits = remote_limits_v2(
        max_items,
        max_bytes,
        max_collection_members,
        max_graph_nodes,
        max_attribute_values,
        max_role_players,
        deadline_ms,
    )?;
    let resources = QueryExecutionResourceLimits::tightened(
        limits
            .deadline_ms
            .unwrap_or(type_bridge_orm::MAX_QUERY_TIMEOUT_MILLISECONDS),
        limits.max_items,
        limits.max_bytes,
        limits.max_graph_nodes,
        limits.max_attribute_values,
        limits.max_collection_members,
        limits.max_role_players,
        limits.max_statements,
    );
    RemoteCapabilities::decode(&advertisement)
        .map_err(|diagnostic| napi_sdk_diagnostic(lower_remote_query_diagnostic(diagnostic)))?;
    Ok(NodeRemoteModelQueryContext {
        advertisement,
        authority: authority.authority(),
        limits,
        resources,
        cancellation: AnswerCancellation::default(),
    })
}

/// Build the same remote context from the common direct/remote resource and
/// cancellation owners.
#[napi(js_name = "queryV2RemoteModelContextWithResources")]
pub fn query_v2_remote_model_context_with_resources(
    env: Env,
    authority: &NodeQueryV2Authority,
    advertisement: Unknown,
    resources: &NodeQueryExecutionResources,
    cancellation: &NodeQueryCancellation,
) -> napi::Result<NodeRemoteModelQueryContext> {
    let advertisement = non_shared_buffer(&env, advertisement)?;
    let advertisement =
        bounded_response_snapshot(&advertisement, MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1));
    RemoteCapabilities::decode(&advertisement)
        .map_err(|diagnostic| napi_sdk_diagnostic(lower_remote_query_diagnostic(diagnostic)))?;
    let resources = resources.inner();
    Ok(NodeRemoteModelQueryContext {
        advertisement,
        authority: authority.authority(),
        limits: resources.remote(),
        resources,
        cancellation: cancellation.inner(),
    })
}

/// Prepare a selected-row or exactly-one remote model query.
#[napi(js_name = "queryV2PrepareRemoteModelRows")]
pub fn query_v2_prepare_remote_model_rows(
    query: &NodeMatchQueryHandle,
    context: &NodeRemoteModelQueryContext,
    orders: Array,
    offset: Unknown,
    limit: Unknown,
    cardinality: String,
) -> napi::Result<NodePendingRemoteModelQuery> {
    let (deadline, cancellation) = begin_remote_invocation(query, context)?;
    let orders = bounded_order_handles(&orders)?;
    let request = query
        .inner()
        .validate_fetch_rows(
            &orders,
            Window {
                offset: node_unsigned(offset)?,
                limit: node_unsigned(limit)?,
            },
            parse_cardinality(&cardinality)?,
        )
        .map_err(crate::napi_orm_error)?;
    prepare_pending(query, context, request, deadline, cancellation)
}

/// Prepare one distinct-root remote page.
#[napi(js_name = "queryV2PrepareRemoteModelPage")]
#[allow(
    clippy::too_many_arguments,
    reason = "terminal mirrors released grammar"
)]
pub fn query_v2_prepare_remote_model_page(
    query: &NodeMatchQueryHandle,
    context: &NodeRemoteModelQueryContext,
    root: &NodeMatchBindingHandle,
    orders: Array,
    offset: Unknown,
    limit: Unknown,
    include_total: Unknown,
) -> napi::Result<NodePendingRemoteModelQuery> {
    let (deadline, cancellation) = begin_remote_invocation(query, context)?;
    let orders = bounded_order_handles(&orders)?;
    let request = query
        .inner()
        .validate_page_by(
            root.inner(),
            &orders,
            Window {
                offset: node_unsigned(offset)?,
                limit: node_unsigned(limit)?,
            },
            node_bool(include_total, "includeTotal")?,
        )
        .map_err(crate::napi_orm_error)?;
    prepare_pending(query, context, request, deadline, cancellation)
}

/// Prepare one lossless distinct-root remote count.
#[napi(js_name = "queryV2PrepareRemoteModelCount")]
pub fn query_v2_prepare_remote_model_count(
    query: &NodeMatchQueryHandle,
    context: &NodeRemoteModelQueryContext,
    root: &NodeMatchBindingHandle,
) -> napi::Result<NodePendingRemoteModelQuery> {
    let (deadline, cancellation) = begin_remote_invocation(query, context)?;
    let request = query
        .inner()
        .validate_count_by(root.inner())
        .map_err(crate::napi_orm_error)?;
    prepare_pending(query, context, request, deadline, cancellation)
}

/// Prepare one distinct-root remote existence query.
#[napi(js_name = "queryV2PrepareRemoteModelExists")]
pub fn query_v2_prepare_remote_model_exists(
    query: &NodeMatchQueryHandle,
    context: &NodeRemoteModelQueryContext,
    root: &NodeMatchBindingHandle,
) -> napi::Result<NodePendingRemoteModelQuery> {
    let (deadline, cancellation) = begin_remote_invocation(query, context)?;
    let request = query
        .inner()
        .validate_exists_by(root.inner())
        .map_err(crate::napi_orm_error)?;
    prepare_pending(query, context, request, deadline, cancellation)
}

/// Prepare one typed ungrouped or grouped reduction over a distinct root.
#[napi(js_name = "queryV2PrepareRemoteModelReduce")]
pub fn query_v2_prepare_remote_model_reduce(
    query: &NodeMatchQueryHandle,
    context: &NodeRemoteModelQueryContext,
    root: &NodeMatchBindingHandle,
    group: Option<&NodeMatchBindingHandle>,
    reducers: Vec<String>,
    inputs: Vec<Option<Reference<NodeMatchFieldHandle>>>,
) -> napi::Result<NodePendingRemoteModelQuery> {
    let (deadline, cancellation) = begin_remote_invocation(query, context)?;
    let terms = reduce_terms(&reducers, &inputs)?;
    let terms = borrow_reduce_terms(&terms);
    let request = query
        .inner()
        .validate_reduce_by(
            root.inner(),
            group.map(NodeMatchBindingHandle::inner),
            &terms,
        )
        .map_err(crate::napi_orm_error)?;
    prepare_pending(query, context, request, deadline, cancellation)
}

/// Prepare one typed reduction grouped by a projected owned field.
#[napi(js_name = "queryV2PrepareRemoteModelReduceByField")]
pub fn query_v2_prepare_remote_model_reduce_by_field(
    query: &NodeMatchQueryHandle,
    context: &NodeRemoteModelQueryContext,
    root: &NodeMatchBindingHandle,
    group: &NodeMatchFieldHandle,
    reducers: Vec<String>,
    inputs: Vec<Option<Reference<NodeMatchFieldHandle>>>,
) -> napi::Result<NodePendingRemoteModelQuery> {
    let (deadline, cancellation) = begin_remote_invocation(query, context)?;
    let terms = reduce_terms(&reducers, &inputs)?;
    let terms = borrow_reduce_terms(&terms);
    let request = query
        .inner()
        .validate_reduce_by_field(root.inner(), group.inner(), &terms)
        .map_err(crate::napi_orm_error)?;
    prepare_pending(query, context, request, deadline, cancellation)
}

/// Prepare one typed reduction grouped by an ordered tuple of projected owned fields.
#[napi(js_name = "queryV2PrepareRemoteModelReduceByFields")]
pub fn query_v2_prepare_remote_model_reduce_by_fields(
    query: &NodeMatchQueryHandle,
    context: &NodeRemoteModelQueryContext,
    root: &NodeMatchBindingHandle,
    groups: Vec<Reference<NodeMatchFieldHandle>>,
    reducers: Vec<String>,
    inputs: Vec<Option<Reference<NodeMatchFieldHandle>>>,
) -> napi::Result<NodePendingRemoteModelQuery> {
    let (deadline, cancellation) = begin_remote_invocation(query, context)?;
    let groups = groups.iter().map(|group| group.inner()).collect::<Vec<_>>();
    let terms = reduce_terms(&reducers, &inputs)?;
    let terms = borrow_reduce_terms(&terms);
    let request = query
        .inner()
        .validate_reduce_by_fields(root.inner(), &groups, &terms)
        .map_err(crate::napi_orm_error)?;
    prepare_pending(query, context, request, deadline, cancellation)
}

fn begin_remote_invocation(
    query: &NodeMatchQueryHandle,
    context: &NodeRemoteModelQueryContext,
) -> napi::Result<(QueryExecutionDeadline, AnswerCancellation)> {
    query.ensure_open()?;
    let deadline = QueryExecutionDeadline::for_limits(context.resources);
    deadline
        .check(&context.cancellation)
        .map_err(crate::match_runtime::napi_sdk_diagnostic)?;
    Ok((deadline, context.cancellation.clone()))
}

fn prepare_pending(
    query: &NodeMatchQueryHandle,
    context: &NodeRemoteModelQueryContext,
    request: type_bridge_orm::ValidatedMatchRequest,
    deadline: QueryExecutionDeadline,
    cancellation: AnswerCancellation,
) -> napi::Result<NodePendingRemoteModelQuery> {
    let registry = query.inner().registry_arc();
    let pending = prepare_remote_model_query_v2_with_budget(
        &context.authority,
        &registry,
        request,
        &context.advertisement,
        context.limits,
        deadline,
        &cancellation,
    )
    .map_err(remote_model_error)?;
    Ok(NodePendingRemoteModelQuery {
        pending: Arc::new(pending),
        result_context: query.result_context(deadline, cancellation.clone()),
        cancellation,
    })
}

#[allow(clippy::too_many_arguments, reason = "flat explicit limit contract")]
fn remote_limits_v2(
    max_items: Unknown,
    max_bytes: Unknown,
    max_collection_members: Unknown,
    max_graph_nodes: Unknown,
    max_attribute_values: Unknown,
    max_role_players: Unknown,
    deadline_ms: Option<Unknown>,
) -> napi::Result<RemoteLimitsV2> {
    let build = || -> Result<RemoteLimitsV2, Diagnostic> {
        Ok(RemoteLimitsV2 {
            deadline_ms: checked_remote_deadline(deadline_ms.map(node_limit).transpose()?)?,
            max_bytes: checked_remote_limit(node_limit(max_bytes)?)?,
            max_items: checked_remote_limit(node_limit(max_items)?)?,
            max_collection_members: checked_remote_limit(node_limit(max_collection_members)?)?,
            max_graph_nodes: checked_remote_limit(node_limit(max_graph_nodes)?)?,
            max_attribute_values: checked_remote_limit(node_limit(max_attribute_values)?)?,
            max_role_players: checked_remote_limit(node_limit(max_role_players)?)?,
            max_statements: 3,
        })
    };
    build().map_err(|diagnostic| napi_sdk_diagnostic(lower_remote_query_diagnostic(diagnostic)))
}

fn node_limit(value: Unknown) -> Result<i128, Diagnostic> {
    let value = BigInt::from_unknown(value).map_err(|_| remote_limit_invalid())?;
    node_limit_bigint(&value)
}

fn node_limit_bigint(value: &BigInt) -> Result<i128, Diagnostic> {
    let (value, lossless) = value.get_i128();
    if lossless {
        Ok(value)
    } else {
        Err(remote_limit_invalid())
    }
}

fn node_unsigned(value: Unknown) -> napi::Result<u64> {
    let value = BigInt::from_unknown(value)
        .map_err(|_| remote_limit_invalid())
        .and_then(|value| checked_node_limit_bigint(&value));
    value.map_err(|diagnostic| napi_sdk_diagnostic(lower_remote_query_diagnostic(diagnostic)))
}

fn checked_node_limit_bigint(value: &BigInt) -> Result<u64, Diagnostic> {
    checked_remote_limit(node_limit_bigint(value)?)
}

fn node_bool(value: Unknown, argument: &'static str) -> napi::Result<bool> {
    bool::from_unknown(value).map_err(|_| {
        napi::Error::new(
            napi::Status::InvalidArg,
            format!("argument '{argument}' must be boolean"),
        )
    })
}

fn bounded_order_handles(orders: &Array) -> napi::Result<Vec<type_bridge_orm::OrderHandle>> {
    let length = usize::try_from(orders.len()).unwrap_or(usize::MAX);
    validate_public_order_term_count(length).map_err(napi_match_error)?;
    let mut references = Vec::with_capacity(length);
    for index in 0..orders.len() {
        references.push(
            orders
                .get::<Reference<NodeMatchOrderHandle>>(index)?
                .ok_or_else(|| napi::Error::from_reason("order array index was unavailable"))?,
        );
    }
    Ok(order_handles(&references))
}

fn remote_model_error(error: RemoteModelQueryV2Error) -> napi::Error {
    match error {
        RemoteModelQueryV2Error::Diagnostic(diagnostic) => {
            napi_sdk_diagnostic(lower_remote_query_diagnostic(diagnostic))
        }
        RemoteModelQueryV2Error::Match(error) => napi_match_error(error),
    }
}

#[cfg(test)]
mod tests {
    use napi::bindgen_prelude::BigInt;

    use super::checked_node_limit_bigint;

    #[test]
    fn hostile_native_bigints_share_the_remote_limit_diagnostic() {
        assert_eq!(
            checked_node_limit_bigint(&BigInt {
                sign_bit: false,
                words: vec![7],
            })
            .expect("small unsigned bigint"),
            7
        );
        for value in [
            BigInt {
                sign_bit: true,
                words: vec![1],
            },
            BigInt {
                sign_bit: false,
                words: vec![0, 0, 1],
            },
        ] {
            let diagnostic = checked_node_limit_bigint(&value).expect_err("hostile bigint");
            assert_eq!(diagnostic.code().as_str(), "query_remote_limit_invalid");
        }
    }
}
