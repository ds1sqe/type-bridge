//! N-API seam for proxy connections over the built-in HTTP route.
//!
//! A [`NodeProxyConnection`] owns one verified [`ProxyQueryConnection`] and a
//! private provider runtime. Every terminal validates its request on the
//! JavaScript thread, then runs through [`QueryTarget::Proxy`] on that runtime
//! and settles a Promise, so neither the event loop nor the libuv pool waits on
//! the network.

use std::future::Future;
use std::sync::Arc;

use napi::bindgen_prelude::{Array, Env, JsValue, Object, Reference, Unknown};
use napi_derive::napi;
use type_bridge_orm::{
    ProviderRuntimeOwner, ProxyEndpoint, ProxyQueryConnection, QueryTarget, ValidatedMatchRequest,
};

use crate::match_runtime::{
    NodeMatchBindingHandle, NodeMatchFieldHandle, NodeMatchQueryHandle, NodeQueryCancellation,
    NodeQueryExecutionResources, napi_sdk_diagnostic, napi_target_error,
};
use crate::query_v2_model_remote_runtime::{
    count_request, exists_request, owned_advertisement, page_request, reduce_by_field_request,
    reduce_by_fields_request, reduce_request, rows_request,
};
use crate::query_v2_runtime::NodeQueryV2Authority;
use crate::runtime_projection::direct_tls_from_node;

/// A verified connection to a TypeBridge server reached over HTTP.
#[napi]
pub struct NodeProxyConnection {
    connection: Arc<ProxyQueryConnection>,
    runtime: Arc<ProviderRuntimeOwner>,
}

#[napi]
impl NodeProxyConnection {
    /// Open `endpoint`, fetch its advertisement unless one is pinned, and
    /// verify it.
    ///
    /// `connectionLimits` bounds the fetch; `answerLimits` is the ceiling
    /// every terminal on the connection is constrained by. Header names and
    /// values are parallel arrays.
    #[napi(ts_return_type = "Promise<NodeProxyConnection>")]
    #[allow(clippy::too_many_arguments, reason = "flat explicit connect contract")]
    pub fn connect(
        env: Env,
        authority: &NodeQueryV2Authority,
        endpoint: String,
        tls_mode: String,
        tls_root_ca: Option<String>,
        header_names: Vec<String>,
        header_values: Vec<String>,
        advertisement: Option<Unknown>,
        connection_limits: Option<&NodeQueryExecutionResources>,
        answer_limits: Option<&NodeQueryExecutionResources>,
        cancellation: Option<&NodeQueryCancellation>,
    ) -> napi::Result<Object<'static>> {
        if header_names.len() != header_values.len() {
            return Err(napi::Error::new(
                napi::Status::InvalidArg,
                "proxy header names and values must have the same length",
            ));
        }
        let endpoint = header_names.into_iter().zip(header_values).fold(
            ProxyEndpoint::new(endpoint).tls(direct_tls_from_node(&tls_mode, tls_root_ca)?),
            |endpoint, (name, value)| endpoint.header(name, value),
        );
        let advertisement = advertisement
            .map(|advertisement| owned_advertisement(&env, advertisement))
            .transpose()?;
        let authority = authority.authority();
        let connection_limits = connection_limits
            .map(NodeQueryExecutionResources::inner)
            .unwrap_or_default();
        let answer_limits = answer_limits
            .map(NodeQueryExecutionResources::inner)
            .unwrap_or_default();
        let cancellation = cancellation
            .map(NodeQueryCancellation::inner)
            .unwrap_or_default();
        let runtime = ProviderRuntimeOwner::new().map(Arc::new).map_err(|error| {
            napi::Error::from_reason(format!("Failed to create Tokio runtime: {error}"))
        })?;
        let owner = Arc::clone(&runtime);
        settle(&env, &runtime, async move {
            let connection = ProxyQueryConnection::connect(
                &endpoint,
                advertisement,
                authority,
                connection_limits,
                answer_limits,
                &cancellation,
            )
            .await
            .map_err(napi_sdk_diagnostic)?;
            Ok(Self {
                connection: Arc::new(connection),
                runtime: owner,
            })
        })
    }

    /// Run a selected-row or exactly-one query.
    #[napi(ts_return_type = "Promise<NodeValidatedMatchResultHandle>")]
    pub fn rows(
        &self,
        env: Env,
        query: &NodeMatchQueryHandle,
        orders: Array,
        offset: Unknown,
        limit: Unknown,
        cardinality: String,
    ) -> napi::Result<Object<'static>> {
        self.execute(&env, query, || {
            rows_request(query, &orders, offset, limit, &cardinality)
        })
    }

    /// Run one distinct-root page.
    #[napi(ts_return_type = "Promise<NodeValidatedMatchResultHandle>")]
    #[allow(
        clippy::too_many_arguments,
        reason = "terminal mirrors released grammar"
    )]
    pub fn page(
        &self,
        env: Env,
        query: &NodeMatchQueryHandle,
        root: &NodeMatchBindingHandle,
        orders: Array,
        offset: Unknown,
        limit: Unknown,
        include_total: Unknown,
    ) -> napi::Result<Object<'static>> {
        self.execute(&env, query, || {
            page_request(query, root, &orders, offset, limit, include_total)
        })
    }

    /// Run one lossless distinct-root count.
    #[napi(ts_return_type = "Promise<NodeValidatedMatchResultHandle>")]
    pub fn count(
        &self,
        env: Env,
        query: &NodeMatchQueryHandle,
        root: &NodeMatchBindingHandle,
    ) -> napi::Result<Object<'static>> {
        self.execute(&env, query, || count_request(query, root))
    }

    /// Run one distinct-root existence query.
    #[napi(ts_return_type = "Promise<NodeValidatedMatchResultHandle>")]
    pub fn exists(
        &self,
        env: Env,
        query: &NodeMatchQueryHandle,
        root: &NodeMatchBindingHandle,
    ) -> napi::Result<Object<'static>> {
        self.execute(&env, query, || exists_request(query, root))
    }

    /// Run one typed ungrouped or grouped reduction over a distinct root.
    #[napi(ts_return_type = "Promise<NodeValidatedMatchResultHandle>")]
    pub fn reduce(
        &self,
        env: Env,
        query: &NodeMatchQueryHandle,
        root: &NodeMatchBindingHandle,
        group: Option<&NodeMatchBindingHandle>,
        reducers: Vec<String>,
        inputs: Vec<Option<Reference<NodeMatchFieldHandle>>>,
    ) -> napi::Result<Object<'static>> {
        self.execute(&env, query, || {
            reduce_request(query, root, group, &reducers, &inputs)
        })
    }

    /// Run one typed reduction grouped by a projected owned field.
    #[napi(
        js_name = "reduceByField",
        ts_return_type = "Promise<NodeValidatedMatchResultHandle>"
    )]
    pub fn reduce_by_field(
        &self,
        env: Env,
        query: &NodeMatchQueryHandle,
        root: &NodeMatchBindingHandle,
        group: &NodeMatchFieldHandle,
        reducers: Vec<String>,
        inputs: Vec<Option<Reference<NodeMatchFieldHandle>>>,
    ) -> napi::Result<Object<'static>> {
        self.execute(&env, query, || {
            reduce_by_field_request(query, root, group, &reducers, &inputs)
        })
    }

    /// Run one typed reduction grouped by an ordered tuple of projected owned
    /// fields.
    #[napi(
        js_name = "reduceByFields",
        ts_return_type = "Promise<NodeValidatedMatchResultHandle>"
    )]
    pub fn reduce_by_fields(
        &self,
        env: Env,
        query: &NodeMatchQueryHandle,
        root: &NodeMatchBindingHandle,
        groups: Vec<Reference<NodeMatchFieldHandle>>,
        reducers: Vec<String>,
        inputs: Vec<Option<Reference<NodeMatchFieldHandle>>>,
    ) -> napi::Result<Object<'static>> {
        self.execute(&env, query, || {
            reduce_by_fields_request(query, root, &groups, &reducers, &inputs)
        })
    }
}

impl NodeProxyConnection {
    /// Validate one request on the JavaScript thread, then run it on this
    /// connection under the query session's policy constrained by the
    /// connection's ceiling.
    fn execute(
        &self,
        env: &Env,
        query: &NodeMatchQueryHandle,
        request: impl FnOnce() -> napi::Result<ValidatedMatchRequest>,
    ) -> napi::Result<Object<'static>> {
        let (deadline, cancellation) = query.begin_invocation()?;
        let validated = request()?;
        let (resources, deadline) =
            query.terminal_budget(&QueryTarget::Proxy(&self.connection), deadline);
        let registry = query.inner().registry_arc();
        let result_context = query.result_context(resources, deadline, cancellation.clone());
        let connection = Arc::clone(&self.connection);
        settle(env, &self.runtime, async move {
            let (request, result) = QueryTarget::Proxy(&connection)
                .execute(&registry, validated, resources, &cancellation, deadline)
                .await
                .map_err(napi_target_error)?;
            result_context.attach(request, result, registry)
        })
    }
}

/// Run `work` on the connection's runtime and return a Promise settled with
/// its outcome.
fn settle<T>(
    env: &Env,
    runtime: &Arc<ProviderRuntimeOwner>,
    work: impl Future<Output = napi::Result<T>> + Send + 'static,
) -> napi::Result<Object<'static>>
where
    T: napi::bindgen_prelude::ToNapiValue + Send + 'static,
{
    let (deferred, promise) = env.create_deferred::<T, _>()?;
    let execution = runtime.spawn(work);
    // Keep the runtime alive through completion, and turn a panic in `work`
    // into a rejection instead of a Promise that never settles.
    let owner = Arc::clone(runtime);
    runtime.spawn(async move {
        match execution.await {
            Ok(Ok(output)) => deferred.resolve(move |_env| Ok(output)),
            Ok(Err(error)) => deferred.reject(error),
            Err(_) => deferred.reject(napi::Error::from_reason(
                "proxy_worker_failed: proxy worker terminated unexpectedly",
            )),
        }
        drop(owner);
    });
    Ok(Object::from_raw(env.raw(), promise.raw()))
}
