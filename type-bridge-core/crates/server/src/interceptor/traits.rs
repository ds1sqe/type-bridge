use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::future::Future;
use std::pin::Pin;

use type_bridge_contract::capability::CapabilitySet;

use type_bridge_contract::failure::Failure;

use type_bridge_contract::id::TypeId;

use type_bridge_contract::query_plan::Operation;

use type_bridge_contract::query_plan::{InputRow, QueryPlan};

use type_bridge_orm::session::staged::TransactionKind;

use type_bridge_contract::sdk_diagnostic::{SdkCommitFailureOutcome, SdkExecutionDiagnostic};

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
            // Staged transport policy metadata deliberately contains raw HTTP
            // credentials. Keep explicit interceptor access while making the
            // ordinary whole-context debug path value-blind.
            .field("metadata_keys", &metadata_keys)
            .field("timestamp", &self.timestamp)
            .finish()
    }
}

/// Redacted staged result evidence presented to response and audit policies.
pub struct PolicyOutcome<'a> {
    success: bool,
    code: &'a str,
    response_bytes: usize,
}

impl<'a> PolicyOutcome<'a> {
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

/// Redacted view of one bound plan in a staged request, presented to
/// [`Interceptor::on_plan`] before the plan stages.
///
/// It exposes what the plan may do, never literal or input row values,
/// unless the policy opts in through [`Interceptor::wants_plan_rows`].
/// Its `Debug` output never shows values either: rows appear as a count.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct PolicyPlan<'a> {
    plan: &'a QueryPlan,
    types: &'a [TypeId],
    rows_visible: bool,
    capabilities: &'a CapabilitySet,
    result_refs: &'a [type_bridge_contract::query_remote::StagedResultRef],
    template_rows: Option<&'a [InputRow]>,
}

impl<'a> PolicyPlan<'a> {
    pub(crate) const fn new(plan: &'a QueryPlan, types: &'a [TypeId]) -> Self {
        Self {
            plan,
            types,
            rows_visible: false,
            capabilities: plan.required_capabilities(),
            result_refs: &[],
            template_rows: None,
        }
    }

    pub(crate) const fn with_result_refs(
        mut self,
        result_refs: &'a [type_bridge_contract::query_remote::StagedResultRef],
        rows: &'a [InputRow],
        capabilities: &'a CapabilitySet,
    ) -> Self {
        self.result_refs = result_refs;
        self.template_rows = Some(rows);
        self.capabilities = capabilities;
        self
    }

    /// Symbolic IID dependencies, visible independently of invocation-row policy.
    #[must_use]
    pub const fn result_refs(&self) -> &'a [type_bridge_contract::query_remote::StagedResultRef] {
        self.result_refs
    }

    pub(crate) const fn with_rows_visible(mut self, visible: bool) -> Self {
        self.rows_visible = visible;
        self
    }

    /// Capabilities the plan requires, e.g. `query.write.delete`.
    #[must_use]
    pub const fn capabilities(&self) -> &'a CapabilitySet {
        self.capabilities
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
            self.template_rows.or_else(|| self.plan.given_rows())
        } else {
            None
        }
    }
}

impl fmt::Debug for PolicyPlan<'_> {
    // Hand-written so literals and bound row values never reach logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let capabilities: Vec<&str> = self.capabilities().iter().map(|cap| cap.as_str()).collect();
        f.debug_struct("PolicyPlan")
            .field("operation", &self.operation())
            .field("capabilities", &capabilities)
            .field("types", &self.types)
            .field("writes", &self.writes())
            .field(
                "rows",
                &self
                    .template_rows
                    .or_else(|| self.plan.given_rows())
                    .map(<[InputRow]>::len),
            )
            .field("result_refs", &self.result_refs)
            .field("rows_visible", &self.rows_visible)
            .finish()
    }
}

/// Redacted outcome of a staged transaction presented to
/// [`Interceptor::on_commit`].
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct CommitOutcome<'a> {
    kind: TransactionKind,
    failure: Option<&'a SdkExecutionDiagnostic>,
}

impl<'a> CommitOutcome<'a> {
    pub(crate) const fn new(
        kind: TransactionKind,
        failure: Option<&'a SdkExecutionDiagnostic>,
    ) -> Self {
        Self { kind, failure }
    }

    /// The transaction kind the request staged on.
    #[must_use]
    pub const fn kind(&self) -> TransactionKind {
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
    /// A host policy refused raw execution with its stable diagnostic code.
    #[error("Raw query refused [{code}]")]
    Refused {
        /// Host-defined machine-readable refusal code.
        code: &'static str,
    },
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

impl InterceptError {
    /// Stable code returned when raw policy admission refuses a request.
    pub const fn code(&self) -> &str {
        match self {
            Self::Refused { code } => code,
            Self::AccessDenied { .. } => "access_denied",
            Self::RateLimited { .. } => "rate_limited",
            Self::ValidationFailed { .. } => "validation_failed",
            Self::Internal(_) => "interceptor_internal",
        }
    }
}

/// One policy's write admission for a staged request.
///
/// The server has no built-in authentication. A policy that authenticated
/// the caller in [`Interceptor::on_transport`] vouches for writes here.
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
/// Interceptors inspect typed plans and request context, admit or reject
/// execution, add context metadata, and observe commit and response outcomes.
pub trait Interceptor: Send + Sync {
    /// Human-readable name for logging.
    fn name(&self) -> &str;

    /// Authenticate and rate-limit transport metadata before envelope decode.
    fn on_transport<'a>(
        &'a self,
        _ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    /// Allow or refuse raw TypeQL before replay reservation and provider I/O.
    /// The query is unchanged; the host owns any policy for this escape hatch.
    fn on_raw<'a>(
        &'a self,
        _query: &'a str,
        _kind: TransactionKind,
        _ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    /// Vouch for a staged write transaction before anything opens.
    ///
    /// Writes are refused unless at least one policy answers
    /// [`StagedWriteAuthority::Authenticated`] and none fails, or the server
    /// explicitly allows unauthenticated staged writes. Runs first among the
    /// V3 hooks, after the transport hook and before [`Self::on_plan`].
    fn on_write_authority<'a>(
        &'a self,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<StagedWriteAuthority, InterceptError>> + Send + 'a>>
    {
        Box::pin(async { Ok(StagedWriteAuthority::Abstain) })
    }

    /// Authorize one bound plan of a staged request.
    ///
    /// Runs for every plan, in order, after write admission and before any
    /// nonce reservation, provider I/O or staging; the
    /// view is computed from the decoded request alone. `index` is the
    /// plan's position in the request. An error refuses the whole request:
    /// later plans are not consulted, nothing is reserved and nothing opens.
    fn on_plan<'a>(
        &'a self,
        _index: usize,
        _plan: &'a PolicyPlan<'a>,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    /// Whether [`Self::on_plan`] may see bound input row values.
    fn wants_plan_rows(&self) -> bool {
        false
    }

    /// Observe a staged transaction's outcome once it is known.
    ///
    /// Observe-only: errors are logged, never change the reply. Runs last
    /// among the V3 hooks, after reservation, staging and the commit, and
    /// before the response policies see the signed reply.
    fn on_commit<'a>(
        &'a self,
        _outcome: &'a CommitOutcome<'a>,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    /// Observe one redacted success or failure outcome for audit/policy.
    fn on_response<'a>(
        &'a self,
        _outcome: &'a PolicyOutcome<'a>,
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
}
