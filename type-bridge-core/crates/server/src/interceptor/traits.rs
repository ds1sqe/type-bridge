#![allow(deprecated)] // Legacy node bridges are removed in 11.30 / 3.0.
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::future::Future;
use std::pin::Pin;

#[cfg(feature = "v2-query")]
use type_bridge_contract::capability::CapabilitySet;
#[cfg(feature = "v2-query")]
use type_bridge_contract::failure::Failure;
#[cfg(feature = "v2-query")]
use type_bridge_contract::id::TypeId;
#[cfg(feature = "v2-query")]
use type_bridge_contract::query_plan::v3::Operation;
#[cfg(feature = "v2-query")]
use type_bridge_contract::query_plan::{InputRow, QueryInvocation, QueryPlan, QueryPlanV3};
#[cfg(feature = "v2-query")]
use type_bridge_contract::query_remote_v3::StagedKindV3;
#[cfg(feature = "v2-query")]
use type_bridge_contract::sdk_diagnostic::{SdkCommitFailureOutcome, SdkExecutionDiagnostic};
use type_bridge_core_lib::ast::Clause;

use super::crud_info::CrudInfo;

/// Metadata attached to each request flowing through the interceptor chain.
#[derive(Clone)]
pub struct RequestContext {
    /// Unique request identifier shared by all pipeline stages.
    pub request_id: String,
    /// Authenticated client identity, or a transport-defined placeholder.
    pub client_id: String,
    /// Database selected for the request.
    pub database: String,
    /// Requested provider transaction mode.
    pub transaction_type: String,
    /// Transport and interceptor metadata carried with the request.
    pub metadata: HashMap<String, serde_json::Value>,
    /// UTC time at which the context was constructed.
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// CRUD analysis produced by the adapter, when available.
    pub crud_info: Option<CrudInfo>,
}

impl fmt::Debug for RequestContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let metadata_keys = self.metadata.keys().collect::<BTreeSet<_>>();
        formatter
            .debug_struct("RequestContext")
            .field("request_id", &self.request_id)
            .field("client_id", &self.client_id)
            .field("database", &self.database)
            .field("transaction_type", &self.transaction_type)
            // V2 transport policy metadata deliberately contains raw HTTP
            // credentials. Keep explicit interceptor access while making the
            // ordinary whole-context debug path value-blind.
            .field("metadata_keys", &metadata_keys)
            .field("timestamp", &self.timestamp)
            .field("crud_info", &self.crud_info)
            .finish()
    }
}

/// Immutable typed V2 contract presented to authorization policies after
/// provider-independent validation and before replay/provider admission.
#[cfg(feature = "v2-query")]
pub struct V2PolicyRequest<'a> {
    plan: &'a QueryPlan,
    invocation: &'a QueryInvocation,
}

#[cfg(feature = "v2-query")]
impl<'a> V2PolicyRequest<'a> {
    pub(crate) const fn new(plan: &'a QueryPlan, invocation: &'a QueryInvocation) -> Self {
        Self { plan, invocation }
    }

    /// Return the exact schema-bound plan admitted by contract validation.
    #[must_use]
    pub const fn plan(&self) -> &'a QueryPlan {
        self.plan
    }

    /// Return the exact operation and input rows bound into the request.
    #[must_use]
    pub const fn invocation(&self) -> &'a QueryInvocation {
        self.invocation
    }
}

/// Redacted V2 result evidence presented to response and audit policies.
#[cfg(feature = "v2-query")]
pub struct V2PolicyOutcome<'a> {
    success: bool,
    code: &'a str,
    response_bytes: usize,
}

#[cfg(feature = "v2-query")]
impl<'a> V2PolicyOutcome<'a> {
    /// Construct one redacted outcome without provider or query payload text.
    #[must_use]
    pub const fn new(success: bool, code: &'a str, response_bytes: usize) -> Self {
        Self {
            success,
            code,
            response_bytes,
        }
    }

    /// Whether execution produced the successful bound response.
    #[must_use]
    pub const fn success(&self) -> bool {
        self.success
    }

    /// Stable diagnostic code, or `ok` for success.
    #[must_use]
    pub const fn code(&self) -> &'a str {
        self.code
    }

    /// Encoded response size, or zero when no response was constructed.
    #[must_use]
    pub const fn response_bytes(&self) -> usize {
        self.response_bytes
    }
}

/// Redacted view of one bound plan in a staged V3 request, presented to
/// [`Interceptor::on_v3_plan`] before the plan stages.
///
/// It exposes what the plan may do, never literal or input row values,
/// unless the policy opts in through [`Interceptor::wants_v3_plan_rows`].
/// Its `Debug` output never shows values either: rows appear as a count.
#[cfg(feature = "v2-query")]
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct V3PolicyPlan<'a> {
    plan: &'a QueryPlanV3,
    types: &'a [TypeId],
    rows_visible: bool,
}

#[cfg(feature = "v2-query")]
impl<'a> V3PolicyPlan<'a> {
    pub(crate) const fn new(plan: &'a QueryPlanV3, types: &'a [TypeId]) -> Self {
        Self {
            plan,
            types,
            rows_visible: false,
        }
    }

    pub(crate) const fn with_rows_visible(mut self, visible: bool) -> Self {
        self.rows_visible = visible;
        self
    }

    /// Capabilities the plan requires, e.g. `query.write.delete`.
    #[must_use]
    pub const fn capabilities(&self) -> &'a CapabilitySet {
        self.plan.required_capabilities()
    }

    /// Whether the plan requires the capability spelled `id`.
    #[must_use]
    pub fn requires(&self, id: &str) -> bool {
        self.capabilities().iter().any(|cap| cap.as_str() == id)
    }

    /// Entity and relation types the plan matches, inserts or puts, sorted.
    #[must_use]
    pub const fn types(&self) -> &'a [TypeId] {
        self.types
    }

    /// The plan's terminal operation.
    #[must_use]
    pub const fn operation(&self) -> Operation {
        self.plan.operation()
    }

    /// Whether the plan has at least one write stage.
    #[must_use]
    pub const fn writes(&self) -> bool {
        self.plan.writes()
    }

    /// Bound input rows; `None` unless the policy asked to see row values.
    #[must_use]
    pub fn rows(&self) -> Option<&'a [InputRow]> {
        if self.rows_visible {
            self.plan.given_rows()
        } else {
            None
        }
    }
}

#[cfg(feature = "v2-query")]
impl fmt::Debug for V3PolicyPlan<'_> {
    // Hand-written so literals and bound row values never reach logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let capabilities: Vec<&str> = self.capabilities().iter().map(|cap| cap.as_str()).collect();
        f.debug_struct("V3PolicyPlan")
            .field("operation", &self.operation())
            .field("capabilities", &capabilities)
            .field("types", &self.types)
            .field("writes", &self.writes())
            .field("rows", &self.plan.given_rows().map(<[InputRow]>::len))
            .field("rows_visible", &self.rows_visible)
            .finish()
    }
}

/// Redacted outcome of a staged V3 transaction presented to
/// [`Interceptor::on_v3_commit`].
#[cfg(feature = "v2-query")]
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct V3CommitOutcome<'a> {
    kind: StagedKindV3,
    failure: Option<&'a SdkExecutionDiagnostic>,
}

#[cfg(feature = "v2-query")]
impl<'a> V3CommitOutcome<'a> {
    pub(crate) const fn new(
        kind: StagedKindV3,
        failure: Option<&'a SdkExecutionDiagnostic>,
    ) -> Self {
        Self { kind, failure }
    }

    /// The transaction kind the request staged on.
    #[must_use]
    pub const fn kind(&self) -> StagedKindV3 {
        self.kind
    }

    /// Whether every plan staged and the transaction completed: a write
    /// committed, or a read closed successfully (reads are reported as
    /// committed too, see [`Self::kind`]).
    #[must_use]
    pub const fn committed(&self) -> bool {
        self.failure.is_none()
    }

    /// Stable diagnostic code, or `ok` when committed.
    #[must_use]
    pub fn code(&self) -> &'a str {
        self.failure.map_or("ok", |failure| failure.code().as_str())
    }

    /// Durability certainty of a failed commit; `None` when committed or
    /// when the failure happened before commit.
    #[must_use]
    pub fn certainty(&self) -> Option<SdkCommitFailureOutcome> {
        self.failure
            .and_then(SdkExecutionDiagnostic::commit_outcome)
    }

    /// Value-free typed provider failure, such as an `STC2` commit conflict.
    #[must_use]
    pub fn failure(&self) -> Option<&'a Failure> {
        self.failure.and_then(SdkExecutionDiagnostic::failure)
    }
}

/// Errors that interceptors can produce.
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)] // variants for future interceptors
pub enum InterceptError {
    /// An authorization policy rejected the request.
    #[error("Access denied: {reason}")]
    AccessDenied {
        /// Stable human-readable rejection reason.
        reason: String,
    },
    /// A rate policy rejected the request.
    #[error("Rate limited: {reason}")]
    RateLimited {
        /// Stable human-readable rejection reason.
        reason: String,
    },
    /// An interceptor-specific validation policy rejected the request.
    #[error("Validation failed: {reason}")]
    ValidationFailed {
        /// Stable human-readable rejection reason.
        reason: String,
    },
    /// An interceptor failed unexpectedly.
    #[error("Internal error: {0}")]
    Internal(String),
}

/// One policy's write admission for a staged V3 request.
///
/// The server has no built-in authentication. A policy that authenticated
/// the caller in [`Interceptor::on_v2_transport`] vouches for writes here.
#[cfg(feature = "v2-query")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum StagedWriteAuthority {
    /// This policy does not vouch for the caller.
    Abstain,
    /// This policy authenticated the caller and admits staged writes.
    Authenticated,
}

/// The core interceptor trait.
///
/// Interceptors receive the query AST and context, and can:
/// - Pass through unchanged
/// - Transform the query (e.g., add tenant filters)
/// - Reject the query (return Err)
/// - Add metadata to context for downstream interceptors
pub trait Interceptor: Send + Sync {
    /// Human-readable name for logging.
    fn name(&self) -> &str;

    /// Called before the query is compiled and sent to TypeDB.
    /// Returns the (possibly transformed) clauses.
    fn on_request<'a>(
        &'a self,
        clauses: Vec<Clause>,
        ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>>;

    /// Declare that this policy intentionally covers the typed V2 lifecycle.
    ///
    /// The default is fail-closed: an AST policy cannot silently receive an
    /// empty legacy query and accidentally authorize a V2 plan it never saw.
    #[cfg(feature = "v2-query")]
    fn supports_v2(&self) -> bool {
        false
    }

    /// Declare that this policy intentionally covers staged V3 requests.
    ///
    /// The default is fail-closed: `/v3/staged` never calls
    /// [`Self::on_v2_request`], so a V2 policy that authorizes typed plans
    /// there would be bypassed. While any configured policy leaves this
    /// `false`, every V3 request is refused at transport admission.
    #[cfg(feature = "v2-query")]
    fn supports_v3(&self) -> bool {
        false
    }

    /// Authenticate and rate-limit transport metadata before envelope decode.
    #[cfg(feature = "v2-query")]
    fn on_v2_transport<'a>(
        &'a self,
        _ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    /// Authorize the exact validated plan and invocation before replay or I/O.
    #[cfg(feature = "v2-query")]
    fn on_v2_request<'a>(
        &'a self,
        _request: &'a V2PolicyRequest<'a>,
        _ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    /// Vouch for a staged V3 write transaction before anything opens.
    ///
    /// Writes are refused unless at least one policy answers
    /// [`StagedWriteAuthority::Authenticated`] and none fails, or the server
    /// explicitly allows unauthenticated staged writes.
    #[cfg(feature = "v2-query")]
    fn on_v3_write_authority<'a>(
        &'a self,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<StagedWriteAuthority, InterceptError>> + Send + 'a>>
    {
        Box::pin(async { Ok(StagedWriteAuthority::Abstain) })
    }

    /// Authorize one bound plan of a staged V3 request.
    ///
    /// Runs for every plan, in order, after write admission and before any
    /// nonce or idempotency-key reservation, provider I/O or staging; the
    /// view is computed from the decoded request alone. `index` is the
    /// plan's position in the request. An error refuses the whole request:
    /// later plans are not consulted, nothing is reserved and nothing opens.
    #[cfg(feature = "v2-query")]
    fn on_v3_plan<'a>(
        &'a self,
        _index: usize,
        _plan: &'a V3PolicyPlan<'a>,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    /// Whether [`Self::on_v3_plan`] may see bound input row values.
    #[cfg(feature = "v2-query")]
    fn wants_v3_plan_rows(&self) -> bool {
        false
    }

    /// Observe a staged V3 transaction's outcome once it is known.
    ///
    /// Observe-only: errors are logged, never change the reply.
    #[cfg(feature = "v2-query")]
    fn on_v3_commit<'a>(
        &'a self,
        _outcome: &'a V3CommitOutcome<'a>,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    /// Observe one redacted success or failure outcome for audit/policy.
    #[cfg(feature = "v2-query")]
    fn on_v2_response<'a>(
        &'a self,
        _outcome: &'a V2PolicyOutcome<'a>,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    /// Called after query execution, before response is sent to client.
    /// Default implementation is a no-op pass-through.
    fn on_response<'a>(
        &'a self,
        _result: &'a serde_json::Value,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn intercept_error_access_denied_display() {
        let e = InterceptError::AccessDenied {
            reason: "no permission".into(),
        };
        assert_eq!(e.to_string(), "Access denied: no permission");
    }

    #[test]
    fn intercept_error_rate_limited_display() {
        let e = InterceptError::RateLimited {
            reason: "too many requests".into(),
        };
        assert_eq!(e.to_string(), "Rate limited: too many requests");
    }

    #[test]
    fn intercept_error_validation_failed_display() {
        let e = InterceptError::ValidationFailed {
            reason: "bad input".into(),
        };
        assert_eq!(e.to_string(), "Validation failed: bad input");
    }

    #[test]
    fn intercept_error_internal_display() {
        let e = InterceptError::Internal("something broke".into());
        assert_eq!(e.to_string(), "Internal error: something broke");
    }

    #[test]
    fn request_context_clone() {
        let ctx = RequestContext {
            request_id: "req-1".into(),
            client_id: "client-1".into(),
            database: "db".into(),
            transaction_type: "read".into(),
            metadata: HashMap::new(),
            timestamp: chrono::Utc::now(),
            crud_info: None,
        };
        let cloned = ctx.clone();
        assert_eq!(cloned.request_id, "req-1");
        assert_eq!(cloned.database, "db");
    }

    #[test]
    fn request_context_debug() {
        let metadata = HashMap::from([
            (
                "http_headers".to_owned(),
                serde_json::json!({
                    "authorization": ["Bearer debug-secret"],
                    "cookie": ["session=debug-secret"],
                }),
            ),
            ("tenant".to_owned(), serde_json::json!("private-tenant")),
        ]);
        let ctx = RequestContext {
            request_id: "req-1".into(),
            client_id: "client-1".into(),
            database: "db".into(),
            transaction_type: "read".into(),
            metadata,
            timestamp: chrono::Utc::now(),
            crud_info: None,
        };
        let debug = format!("{:?}", ctx);
        assert!(debug.contains("req-1"));
        assert!(debug.contains("http_headers"));
        assert!(debug.contains("tenant"));
        assert!(!debug.contains("debug-secret"));
        assert!(!debug.contains("private-tenant"));
        assert!(!debug.contains("authorization"));
        assert!(!debug.contains("cookie"));
    }

    #[tokio::test]
    async fn default_on_response_returns_ok() {
        struct MinimalInterceptor;

        impl Interceptor for MinimalInterceptor {
            fn name(&self) -> &str {
                "minimal"
            }
            fn on_request<'a>(
                &'a self,
                clauses: Vec<Clause>,
                _ctx: &'a mut RequestContext,
            ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>>
            {
                Box::pin(async move { Ok(clauses) })
            }
            // on_response uses default impl
        }

        let interceptor = MinimalInterceptor;

        assert_eq!(interceptor.name(), "minimal");
        let mut ctx = RequestContext {
            request_id: "req-1".into(),
            client_id: "client-1".into(),
            database: "db".into(),
            transaction_type: "read".into(),
            metadata: HashMap::new(),
            timestamp: chrono::Utc::now(),
            crud_info: None,
        };
        let req_result = interceptor.on_request(vec![], &mut ctx).await;
        assert!(req_result.is_ok());

        let result = interceptor.on_response(&serde_json::json!({}), &ctx).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn interceptor_trait_object_safety() {
        struct DummyInterceptor;

        impl Interceptor for DummyInterceptor {
            fn name(&self) -> &str {
                "dummy"
            }
            fn on_request<'a>(
                &'a self,
                clauses: Vec<Clause>,
                _ctx: &'a mut RequestContext,
            ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>>
            {
                Box::pin(async move { Ok(clauses) })
            }
        }

        let boxed: Box<dyn Interceptor> = Box::new(DummyInterceptor);
        assert_eq!(boxed.name(), "dummy");
        let mut ctx = RequestContext {
            request_id: "req-1".into(),
            client_id: "client-1".into(),
            database: "db".into(),
            transaction_type: "read".into(),
            metadata: HashMap::new(),
            timestamp: chrono::Utc::now(),
            crud_info: None,
        };
        let result = boxed.on_request(vec![], &mut ctx).await;
        assert!(result.unwrap().is_empty());
    }
}
