//! N-API seam for one-exchange remote model-oriented typed queries.
//!
//! JavaScript retains the caller-owned async exchange. This module snapshots
//! immutable authority/capability/limit context, prepares released match
//! terminals through the shared V1-to-V2 adapter, and returns the same opaque
//! validated result proof consumed by direct typed queries.

use std::sync::Arc;

use napi::bindgen_prelude::{Array, AsyncTask, Buffer, Env, FromNapiValue, Reference, Unknown};
use napi_derive::napi;
use type_bridge_contract::limits::MAX_REMOTE_ENVELOPE_BYTES;
use type_bridge_orm::_registry::DescriptorRegistry;
use type_bridge_orm::{
    AnswerCancellation, PendingRemoteModelQueryV2, ProxyQueryConnection, QueryExecutionDeadline,
    QueryExecutionResourceLimits, RemoteModelQueryV2Error, RowCardinality, ValidatedMatchRequest,
    Window, lower_remote_query_diagnostic, validate_order_term_count,
};

use crate::match_runtime::{
    NodeMatchBindingHandle, NodeMatchFieldHandle, NodeMatchOrderHandle, NodeMatchQueryHandle,
    NodeMatchResultContext, NodeQueryCancellation, NodeQueryExecutionResources,
    NodeValidatedMatchResultHandle, borrow_reduce_terms, napi_match_error, napi_sdk_diagnostic,
    napi_target_error, order_handles, parse_enum, reduce_terms, unknown_u64,
};
use crate::query_v2_runtime::{NodeQueryV2Authority, bounded_response_snapshot, non_shared_buffer};

/// A proxy connection whose exchange JavaScript performs, with the policy
/// and cancellation owner its terminals run under.
#[napi]
pub struct NodeRemoteModelQueryContext {
    connection: ProxyQueryConnection,
    resources: QueryExecutionResourceLimits,
    cancellation: AnswerCancellation,
}

impl NodeRemoteModelQueryContext {
    fn new(
        env: &Env,
        authority: &NodeQueryV2Authority,
        advertisement: Unknown,
        answer_limits: QueryExecutionResourceLimits,
        resources: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> napi::Result<Self> {
        let advertisement = owned_advertisement(env, advertisement)?;
        let connection = ProxyQueryConnection::with_exchange(
            advertisement,
            authority.authority(),
            answer_limits,
        )
        .map_err(napi_sdk_diagnostic)?;
        Ok(Self {
            connection,
            resources,
            cancellation,
        })
    }

    fn prepare(
        &self,
        query: &NodeMatchQueryHandle,
        request: impl FnOnce() -> napi::Result<ValidatedMatchRequest>,
    ) -> napi::Result<NodePendingRemoteModelQuery> {
        query.ensure_open()?;
        let deadline = QueryExecutionDeadline::for_limits(self.resources);
        let request = request()?;
        let registry = query.inner().registry_arc();
        let pending = self
            .connection
            .prepare(
                &registry,
                request,
                self.resources,
                &self.cancellation,
                deadline,
            )
            .map_err(napi_target_error)?;
        Ok(NodePendingRemoteModelQuery {
            pending: Arc::new(pending),
            registry,
            result_context: query.result_context(
                self.connection.answer_limits(),
                deadline,
                self.cancellation.clone(),
            ),
            deadline,
            cancellation: self.cancellation.clone(),
        })
    }
}

/// One prepared model request with an atomic one-shot reply decoder.
#[napi]
pub struct NodePendingRemoteModelQuery {
    pending: Arc<PendingRemoteModelQueryV2>,
    registry: Arc<DescriptorRegistry>,
    result_context: NodeMatchResultContext,
    deadline: QueryExecutionDeadline,
    cancellation: AnswerCancellation,
}

/// One reply decode scheduled on the libuv worker pool.
pub struct DecodeRemoteModelReplyTask {
    input: Option<DecodeRemoteModelReply>,
}

struct DecodeRemoteModelReply {
    pending: Arc<PendingRemoteModelQueryV2>,
    registry: Arc<DescriptorRegistry>,
    response: Vec<u8>,
    result_context: NodeMatchResultContext,
    deadline: QueryExecutionDeadline,
    cancellation: AnswerCancellation,
}

impl napi::Task for DecodeRemoteModelReplyTask {
    type Output = NodeValidatedMatchResultHandle;
    type JsValue = NodeValidatedMatchResultHandle;

    fn compute(&mut self) -> napi::Result<Self::Output> {
        let input = self
            .input
            .take()
            .ok_or_else(|| napi::Error::from_reason("remote model reply task already ran"))?;
        // Claiming is the one-shot: a second decode of the same request is
        // refused here, before the response is inspected.
        let (request, result) = ProxyQueryConnection::finish(
            &input.pending,
            &input.response,
            &input.cancellation,
            input.deadline,
        )
        .map_err(napi_target_error)?;
        input.result_context.attach(request, result, input.registry)
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

    /// Snapshot the response, then claim, authenticate, and decode it off
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
        // before the one-shot claim, so FFI mistakes remain retryable.
        let response = non_shared_buffer(&env, response)?;
        let response =
            bounded_response_snapshot(&response, MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1));
        Ok(AsyncTask::new(DecodeRemoteModelReplyTask {
            input: Some(DecodeRemoteModelReply {
                pending: Arc::clone(&self.pending),
                registry: Arc::clone(&self.registry),
                response,
                result_context: self.result_context.clone(),
                deadline: self.deadline,
                cancellation: self.cancellation.clone(),
            }),
        }))
    }
}

/// Build the remote context from the common direct/remote resource and
/// cancellation owners. `answer_limits` is the caller-exchanged connection's
/// ceiling; left out, the session's own resources are the ceiling.
#[napi(js_name = "queryV2RemoteModelContextWithResources")]
pub fn query_v2_remote_model_context_with_resources(
    env: Env,
    authority: &NodeQueryV2Authority,
    advertisement: Unknown,
    resources: &NodeQueryExecutionResources,
    cancellation: &NodeQueryCancellation,
    answer_limits: Option<&NodeQueryExecutionResources>,
) -> napi::Result<NodeRemoteModelQueryContext> {
    let resources = resources.inner();
    NodeRemoteModelQueryContext::new(
        &env,
        authority,
        advertisement,
        answer_limits.map_or(resources, NodeQueryExecutionResources::inner),
        resources,
        cancellation.inner(),
    )
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
    context.prepare(query, || {
        rows_request(query, &orders, offset, limit, &cardinality)
    })
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
    context.prepare(query, || {
        page_request(query, root, &orders, offset, limit, include_total)
    })
}

/// Prepare one lossless distinct-root remote count.
#[napi(js_name = "queryV2PrepareRemoteModelCount")]
pub fn query_v2_prepare_remote_model_count(
    query: &NodeMatchQueryHandle,
    context: &NodeRemoteModelQueryContext,
    root: &NodeMatchBindingHandle,
) -> napi::Result<NodePendingRemoteModelQuery> {
    context.prepare(query, || count_request(query, root))
}

/// Prepare one distinct-root remote existence query.
#[napi(js_name = "queryV2PrepareRemoteModelExists")]
pub fn query_v2_prepare_remote_model_exists(
    query: &NodeMatchQueryHandle,
    context: &NodeRemoteModelQueryContext,
    root: &NodeMatchBindingHandle,
) -> napi::Result<NodePendingRemoteModelQuery> {
    context.prepare(query, || exists_request(query, root))
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
    context.prepare(query, || {
        reduce_request(query, root, group, &reducers, &inputs)
    })
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
    context.prepare(query, || {
        reduce_by_field_request(query, root, group, &reducers, &inputs)
    })
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
    context.prepare(query, || {
        reduce_by_fields_request(query, root, &groups, &reducers, &inputs)
    })
}

/// An owned, bounded, non-shared snapshot of advertisement bytes.
pub(crate) fn owned_advertisement(env: &Env, advertisement: Unknown) -> napi::Result<Vec<u8>> {
    let advertisement = non_shared_buffer(env, advertisement)?;
    Ok(bounded_response_snapshot(
        &advertisement,
        MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1),
    ))
}

// Window integers are marshalled like the direct terminals', whichever route
// then carries the request; the core owns every rule on their values.

pub(crate) fn rows_request(
    query: &NodeMatchQueryHandle,
    orders: &Array,
    offset: Unknown,
    limit: Unknown,
    cardinality: &str,
) -> napi::Result<ValidatedMatchRequest> {
    let orders = bounded_order_handles(orders, query.inner().limits())?;
    query
        .inner()
        .validate_fetch_rows(
            &orders,
            Window {
                offset: unknown_u64(offset, "offset")?,
                limit: unknown_u64(limit, "limit")?,
            },
            parse_enum::<RowCardinality>(cardinality)?,
        )
        .map_err(crate::napi_orm_error)
}

pub(crate) fn page_request(
    query: &NodeMatchQueryHandle,
    root: &NodeMatchBindingHandle,
    orders: &Array,
    offset: Unknown,
    limit: Unknown,
    include_total: Unknown,
) -> napi::Result<ValidatedMatchRequest> {
    let orders = bounded_order_handles(orders, query.inner().limits())?;
    query
        .inner()
        .validate_page_by(
            root.inner(),
            &orders,
            Window {
                offset: unknown_u64(offset, "offset")?,
                limit: unknown_u64(limit, "limit")?,
            },
            node_bool(include_total, "includeTotal")?,
        )
        .map_err(crate::napi_orm_error)
}

pub(crate) fn count_request(
    query: &NodeMatchQueryHandle,
    root: &NodeMatchBindingHandle,
) -> napi::Result<ValidatedMatchRequest> {
    query
        .inner()
        .validate_count_by(root.inner())
        .map_err(crate::napi_orm_error)
}

pub(crate) fn exists_request(
    query: &NodeMatchQueryHandle,
    root: &NodeMatchBindingHandle,
) -> napi::Result<ValidatedMatchRequest> {
    query
        .inner()
        .validate_exists_by(root.inner())
        .map_err(crate::napi_orm_error)
}

pub(crate) fn reduce_request(
    query: &NodeMatchQueryHandle,
    root: &NodeMatchBindingHandle,
    group: Option<&NodeMatchBindingHandle>,
    reducers: &[String],
    inputs: &[Option<Reference<NodeMatchFieldHandle>>],
) -> napi::Result<ValidatedMatchRequest> {
    let terms = reduce_terms(reducers, inputs)?;
    let terms = borrow_reduce_terms(&terms);
    query
        .inner()
        .validate_reduce_by(
            root.inner(),
            group.map(NodeMatchBindingHandle::inner),
            &terms,
        )
        .map_err(crate::napi_orm_error)
}

pub(crate) fn reduce_by_field_request(
    query: &NodeMatchQueryHandle,
    root: &NodeMatchBindingHandle,
    group: &NodeMatchFieldHandle,
    reducers: &[String],
    inputs: &[Option<Reference<NodeMatchFieldHandle>>],
) -> napi::Result<ValidatedMatchRequest> {
    let terms = reduce_terms(reducers, inputs)?;
    let terms = borrow_reduce_terms(&terms);
    query
        .inner()
        .validate_reduce_by_field(root.inner(), group.inner(), &terms)
        .map_err(crate::napi_orm_error)
}

pub(crate) fn reduce_by_fields_request(
    query: &NodeMatchQueryHandle,
    root: &NodeMatchBindingHandle,
    groups: &[Reference<NodeMatchFieldHandle>],
    reducers: &[String],
    inputs: &[Option<Reference<NodeMatchFieldHandle>>],
) -> napi::Result<ValidatedMatchRequest> {
    let groups = groups.iter().map(|group| group.inner()).collect::<Vec<_>>();
    let terms = reduce_terms(reducers, inputs)?;
    let terms = borrow_reduce_terms(&terms);
    query
        .inner()
        .validate_reduce_by_fields(root.inner(), &groups, &terms)
        .map_err(crate::napi_orm_error)
}

fn node_bool(value: Unknown, argument: &'static str) -> napi::Result<bool> {
    bool::from_unknown(value).map_err(|_| {
        napi::Error::new(
            napi::Status::InvalidArg,
            format!("argument '{argument}' must be boolean"),
        )
    })
}

fn bounded_order_handles(
    orders: &Array,
    limits: type_bridge_contract::limits::QueryLimits,
) -> napi::Result<Vec<type_bridge_orm::OrderHandle>> {
    let length = usize::try_from(orders.len()).unwrap_or(usize::MAX);
    validate_order_term_count(length, limits.structural).map_err(napi_match_error)?;
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

pub(crate) fn remote_model_error(error: RemoteModelQueryV2Error) -> napi::Error {
    match error {
        RemoteModelQueryV2Error::Diagnostic(diagnostic) => {
            napi_sdk_diagnostic(lower_remote_query_diagnostic(diagnostic))
        }
        RemoteModelQueryV2Error::Match(error) => napi_match_error(error),
    }
}
