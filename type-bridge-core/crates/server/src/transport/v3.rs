//! Staged V3 transactions over one executor identity.
//!
//! One canonical request stages ordered plans on one transaction and commits
//! once. Reply signing, replay protection and host policy hooks share the
//! executor identity advertised at `/v3/capabilities`.
//! Writes open only when a policy vouches for the caller through
//! [`Interceptor::on_v3_write_authority`](crate::interceptor::Interceptor::on_v3_write_authority)
//! or the operator explicitly allows unauthenticated staged writes.
//!
//! The executor is a library entry: `StagedExecutor::execute_staged_request`
//! takes one admitted caller and the signed request bytes and returns the
//! signed reply bytes, so an embedding host runs the staged route
//! in-process without the HTTP framing of `create_v3_router`.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::instrument::WithSubscriber;

use axum::Router;
use axum::body::{Bytes, to_bytes};
use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{get, post};
use type_bridge_contract::capability::CapabilityId;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::id::TypeId;
use type_bridge_contract::query_plan::QueryPattern;
use type_bridge_contract::query_plan::v3::{
    Pattern, QueryPlanV3, Stage, query_plan_v3_capability_vocabulary,
};
use type_bridge_contract::query_remote::{RemoteCapabilities, RemoteCapabilitiesFingerprint};
use type_bridge_contract::query_remote_v3::{
    CAP_QUERY_REMOTE_STAGED_V3, CAP_QUERY_REMOTE_WRITE_V3, StagedAnswerV3, StagedKindV3,
    StagedOutcomeV3, StagedReplyV3, StagedRequestV3,
};
use type_bridge_contract::sdk_diagnostic::{
    SdkCommitFailureOutcome, SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic,
};
use type_bridge_orm::lower_remote_query_diagnostic;
use type_bridge_orm::query_v2_remote::encode_unbound_remote_failure_v2;
use type_bridge_orm::session::staged::{PendingError, TransactionKind, TransactionOptions};

use super::staged_state::{
    ReplayStoreError, STAGED_PREFLIGHT_SLOTS, StagedQueryState, await_before_deadline,
    body_oversized, deadline_exceeded, declared_staged_body_oversized, envelope_response,
    policy_rejected, replay_capacity, replay_unavailable, replayed, response_audit_deadline,
    staged_request_metadata, unavailable,
};
use crate::interceptor::{RequestContext, V3CommitOutcome, V3PolicyOutcome, V3PolicyPlan};
use crate::pipeline::QueryPipeline;

/// Code of the signed failure refusing a staged write no policy admitted.
pub const STAGED_WRITE_NOT_AUTHORIZED: &str = "staged_write_not_authorized";

/// The staged V3 executor over one staged executor identity: the policy
/// pipeline, the query state (advertisement, ceilings, replay store, live
/// authority, signer) and the staged advertisement it answers under.
///
/// `create_v3_router` wraps one in HTTP; an embedding host builds one and
/// calls `StagedExecutor::execute_staged_request` directly.
pub struct StagedExecutor {
    pipeline: Arc<QueryPipeline>,
    pub(super) query: Arc<StagedQueryState>,
    advertisement: RemoteCapabilities,
    fingerprint: RemoteCapabilitiesFingerprint,
}

/// What one staged request came to, with the signed bytes to deliver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedOutcome {
    /// The signed reply (committed, failed or `commit_outcome_unknown`), or
    /// an unbound signed failure when the request could not be decoded.
    pub reply: Vec<u8>,
    /// Whether the transaction committed, did not, or may have.
    pub kind: StagedOutcomeKind,
    /// The reply's diagnostic code; `"ok"` when committed.
    pub code: String,
}

/// Whether a staged request's transaction committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagedOutcomeKind {
    /// The transaction committed (a read closed) and the reply carries its
    /// answers.
    Committed,
    /// Nothing committed: refused before execution, or failed with a
    /// diagnostic the reply carries.
    Failed,
    /// A write may have committed (replayed nonce, lost or unsignable
    /// reply, withheld reply, panic): the reply is `commit_outcome_unknown`
    /// and the caller must read the database before retrying.
    Unknown,
}

impl StagedOutcome {
    fn new(reply: Vec<u8>, kind: StagedOutcomeKind, code: &str) -> Self {
        Self {
            reply,
            kind,
            code: code.to_owned(),
        }
    }
}

impl StagedExecutor {
    /// Bind the executor to one pipeline and query state; the staged
    /// advertisement re-states the validated executor identity.
    pub fn new(pipeline: Arc<QueryPipeline>, query: Arc<StagedQueryState>) -> Self {
        // The executor advertisement's limits were validated when it was built, so
        // re-stating them under a wider capability set cannot fail.
        let profile = query
            .database
            .server_version()
            .as_ref()
            .and_then(type_bridge_core_lib::version::semantic_profile_id);
        let advertisement = staged_advertisement_for_profile(&query.advertisement, profile)
            .and_then(|advertisement| match query.staged_query_limits {
                Some(limits) => advertisement.with_query_limits(limits),
                None => Ok(advertisement),
            })
            .expect("validated advertisement re-states under the staged capabilities");
        #[cfg(feature = "typedb")]
        let advertisement = if query.raw_executor.is_some() {
            let mut capabilities = advertisement.capabilities().clone();
            capabilities
                .insert(CapabilityId::new("query.remote.raw").expect("static raw capability"));
            RemoteCapabilities::new(
                capabilities,
                advertisement.executor().clone(),
                advertisement.reply_key(),
            )
            .with_query_limits(advertisement.query_limits())
            .expect("validated limits")
            .with_answer_limits(advertisement.answer_limits().clone())
            .expect("validated limits")
        } else {
            advertisement
        };
        let fingerprint = advertisement
            .fingerprint()
            .expect("staged advertisement fingerprints");
        Self {
            pipeline,
            query,
            advertisement,
            fingerprint,
        }
    }

    /// The staged advertisement this executor answers under.
    pub fn advertisement(&self) -> &RemoteCapabilities {
        &self.advertisement
    }

    /// The policy pipeline that admits callers for this executor.
    pub fn pipeline(&self) -> &Arc<QueryPipeline> {
        &self.pipeline
    }

    fn unbound(&self, diagnostic: &Diagnostic) -> Vec<u8> {
        encode_unbound_remote_failure_v2(None, diagnostic, &self.fingerprint, &self.query.signer)
    }

    fn signed(
        &self,
        request: &StagedRequestV3,
        outcome: StagedOutcomeV3,
        answers: Vec<StagedAnswerV3>,
    ) -> Result<Vec<u8>, Diagnostic> {
        StagedReplyV3::new(request, outcome, answers)?.encode_signed(
            request,
            &self.advertisement,
            &self.query.signer,
        )
    }

    /// A request-bound signed failure; unbound only if it cannot be signed.
    fn failed(&self, request: &StagedRequestV3, diagnostic: SdkExecutionDiagnostic) -> Vec<u8> {
        self.signed(request, StagedOutcomeV3::Failed { diagnostic }, Vec::new())
            .unwrap_or_else(|error| self.unbound(&error))
    }

    /// A signed `commit_outcome_unknown` failure.
    fn unknown(&self, request: &StagedRequestV3) -> Vec<u8> {
        self.failed(
            request,
            SdkExecutionDiagnostic::commit_failure(SdkCommitFailureOutcome::Unknown),
        )
    }

    fn refusal(&self, request: Option<&StagedRequestV3>, diagnostic: &Diagnostic) -> Vec<u8> {
        match request {
            Some(request) => {
                self.failed(request, lower_remote_query_diagnostic(diagnostic.clone()))
            }
            None => self.unbound(diagnostic),
        }
    }
}

/// Build the staged capability vocabulary under one executor identity and limits.
pub fn staged_advertisement(base: &RemoteCapabilities) -> Result<RemoteCapabilities, Diagnostic> {
    staged_advertisement_for_profile(base, Some("typedb-3.13.6/v1"))
}

pub(super) fn staged_advertisement_for_profile(
    base: &RemoteCapabilities,
    profile: Option<&str>,
) -> Result<RemoteCapabilities, Diagnostic> {
    let mut capabilities: type_bridge_contract::capability::CapabilitySet = base
        .capabilities()
        .iter()
        .filter(|capability| {
            capability.as_str() != type_bridge_contract::query_plan::v3::CAP_NEGATED_GIVEN_IID
                || type_bridge_contract::query_plan::v3::supports_negated_given_iid(profile)
        })
        .cloned()
        .collect();
    for capability in query_plan_v3_capability_vocabulary().iter() {
        if capability.as_str() != type_bridge_contract::query_plan::v3::CAP_NEGATED_GIVEN_IID
            || type_bridge_contract::query_plan::v3::supports_negated_given_iid(profile)
        {
            capabilities.insert(capability.clone());
        }
    }
    for capability in [
        CAP_QUERY_REMOTE_STAGED_V3,
        CAP_QUERY_REMOTE_WRITE_V3,
        type_bridge_contract::query_remote_v3::CAP_QUERY_REMOTE_ANSWER_DIMENSIONS,
    ] {
        capabilities.insert(CapabilityId::new(capability)?);
    }
    let mut limits = base.query_limits();
    if limits.structural.selected_slots == type_bridge_contract::limits::MAX_SELECTED_SLOTS {
        limits.structural.selected_slots =
            type_bridge_contract::limits::MAX_BINDINGS.min(limits.structural.bindings);
    }
    RemoteCapabilities::new(capabilities, base.executor().clone(), base.reply_key())
        .with_query_limits(limits)?
        .with_answer_limits(base.answer_limits().clone())
}

/// Build `POST /v3/staged` and `GET /v3/capabilities` over one staged executor.
pub fn create_v3_router(pipeline: Arc<QueryPipeline>, query: Arc<StagedQueryState>) -> Router {
    #[cfg(not(feature = "typedb"))]
    let disabled_pipeline = Arc::clone(&pipeline);
    let state = Arc::new(StagedExecutor::new(pipeline, query));
    let router = Router::new()
        .route("/v3/staged", post(handle_v3_staged))
        .route("/v3/capabilities", get(handle_v3_capabilities))
        .route("/health", get(handle_health))
        .fallback(super::http::route_removed);
    #[cfg(feature = "typedb")]
    let router = router.route("/v3/raw", post(super::raw::handle_raw));
    #[cfg(not(feature = "typedb"))]
    let router = router.route(
        "/v3/raw",
        post(move |headers: HeaderMap| {
            let pipeline = Arc::clone(&disabled_pipeline);
            async move { super::http::raw_disabled_with_pipeline(&pipeline, &headers).await }
        }),
    );
    router.with_state(state)
}

async fn handle_health(
    State(state): State<Arc<StagedExecutor>>,
) -> axum::Json<super::http::HealthBody> {
    super::http::health_response(state.query.database.is_connected())
}

async fn handle_v3_capabilities(
    State(state): State<Arc<StagedExecutor>>,
    request: Request,
) -> Response {
    let Some(deadline) = Instant::now().checked_add(state.query.execution_timeout) else {
        return envelope_response(state.unbound(&deadline_exceeded()));
    };
    let ctx = match admit_transport(&state, request.headers(), "capabilities", deadline).await {
        Ok(ctx) => ctx,
        Err(response) => return *response,
    };
    let outcome = match state.advertisement.encode() {
        Ok(bytes) => {
            finish(
                &state,
                deadline,
                &ctx,
                None,
                StagedOutcomeKind::Committed,
                "ok",
                bytes,
            )
            .await
        }
        Err(diagnostic) => refuse(&state, deadline, &ctx, None, diagnostic).await,
    };
    envelope_response(outcome.reply)
}

async fn handle_v3_staged(State(shared): State<Arc<StagedExecutor>>, request: Request) -> Response {
    let state = shared.as_ref();
    let Some(deadline) = Instant::now().checked_add(state.query.execution_timeout) else {
        return envelope_response(state.unbound(&deadline_exceeded()));
    };
    // Transport policy authenticates before any attacker-controlled bytes
    // are buffered, exactly as for `/v2/query`.
    let ctx = match admit_transport(state, request.headers(), "staged", deadline).await {
        Ok(ctx) => ctx,
        Err(response) => return *response,
    };
    if declared_staged_body_oversized(request.headers(), state.query.body_limit_bytes()) {
        let outcome = refuse(state, deadline, &ctx, None, body_oversized()).await;
        return envelope_response(outcome.reply);
    }
    let body = match await_before_deadline(
        deadline,
        to_bytes(request.into_body(), state.query.body_limit_bytes()),
    )
    .await
    {
        Some(Ok(body)) => body,
        Some(Err(_)) => {
            let outcome = refuse(state, deadline, &ctx, None, body_oversized()).await;
            return envelope_response(outcome.reply);
        }
        None => {
            let outcome = refuse(state, deadline, &ctx, None, deadline_exceeded()).await;
            return envelope_response(outcome.reply);
        }
    };
    let outcome = shared.execute_staged_request(ctx, body, deadline).await;
    envelope_response(outcome.reply)
}

impl StagedExecutor {
    /// Execute one staged request from its signed bytes to its signed reply.
    ///
    /// `ctx` is the caller the transport admitted: build it with
    /// [`QueryPipeline::v3_request_context`] and
    /// [`QueryPipeline::begin_v3_request`], as the router does. `deadline`
    /// is the server's execution deadline; the request's own lifetime
    /// tightens it.
    ///
    /// In order: decode under the advertised limits, freshness, transaction
    /// kind, write vouching (`on_v3_write_authority`), every bound plan's
    /// policy (`on_v3_plan`, in index order, before anything is reserved
    /// or opened), nonce reservation for the request's lifetime, staging
    /// and one commit, commit observation (`on_v3_commit`, observe-only),
    /// answer assembly under the ceilings tightened by the request,
    /// signing, and the pipeline's response policies. A write whose
    /// outcome is lost answers `commit_outcome_unknown`
    /// ([`StagedOutcomeKind::Unknown`]), never a safe-to-retry refusal.
    ///
    /// The outcome is owned by a detached task once admitted, so a caller
    /// that stops waiting mid-commit does not drop the observers.
    pub async fn execute_staged_request(
        self: &Arc<Self>,
        ctx: RequestContext,
        body: Bytes,
        deadline: Instant,
    ) -> StagedOutcome {
        execute(self, ctx, body, deadline).await
    }
}

#[tracing::instrument(target = "type_bridge.io", name = "type_bridge.io.request", skip_all, fields(bytes_in = body.len()))]
async fn execute(
    shared: &Arc<StagedExecutor>,
    ctx: RequestContext,
    body: Bytes,
    deadline: Instant,
) -> StagedOutcome {
    let state = shared.as_ref();
    tracing::debug!(target: "type_bridge.io", bytes_in = body.len(), "type_bridge.io.request");
    let permit = match await_before_deadline(
        deadline,
        Arc::clone(&STAGED_PREFLIGHT_SLOTS).acquire_owned(),
    )
    .await
    {
        Some(Ok(permit)) => permit,
        Some(Err(_)) => return refuse(state, deadline, &ctx, None, unavailable()).await,
        None => return refuse(state, deadline, &ctx, None, deadline_exceeded()).await,
    };
    let query = Arc::clone(&state.query);
    let advertisement = state.advertisement.clone();
    let decode = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        StagedRequestV3::decode_with_limits(
            &body,
            &query.declared,
            &advertisement,
            advertisement.query_limits(),
        )
    });
    let staged = match await_before_deadline(deadline, decode).await {
        Some(Ok(Ok(staged))) => staged,
        Some(Ok(Err(diagnostic))) => {
            tracing::debug!(target: "type_bridge.io", decode = "refused", code = diagnostic.code().as_str(), "type_bridge.io.request");
            return refuse(state, deadline, &ctx, None, diagnostic).await;
        }
        Some(Err(_)) => return refuse(state, deadline, &ctx, None, unavailable()).await,
        None => return refuse(state, deadline, &ctx, None, deadline_exceeded()).await,
    };
    tracing::debug!(target: "type_bridge.io", decode = "ok", deadline_ms = u64::try_from(deadline.saturating_duration_since(Instant::now()).as_millis()).unwrap_or(u64::MAX), nonce = staged.nonce(), kind = ?staged.kind(), plans = staged.plans().len(), "type_bridge.io.request");
    let request = Some(&staged);

    let now_ms = unix_ms();
    if let Err(diagnostic) = staged.validate_at(now_ms) {
        return refuse(state, deadline, &ctx, request, diagnostic).await;
    }
    // The request's own lifetime tightens the mandatory server deadline and
    // bounds how long its nonce or retained reply is kept.
    let lifetime = Duration::from_millis(staged.expires_at_unix_ms().saturating_sub(now_ms));
    let expires = Instant::now().checked_add(lifetime).unwrap_or(deadline);
    let deadline = deadline.min(expires);
    let kind = match staged.kind() {
        StagedKindV3::Read => TransactionKind::Read,
        StagedKindV3::Write => TransactionKind::Write,
        StagedKindV3::Schema => {
            let failure = sdk_invalid(
                "query_remote_v3_kind",
                "staged requests never open a schema transaction",
            );
            return finish_failed(state, deadline, &ctx, &staged, failure).await;
        }
    };
    let open_writes = state.query.allow_unauthenticated_staged_writes;
    if kind == TransactionKind::Write && !open_writes {
        let vouched =
            match await_before_deadline(deadline, state.pipeline.authorize_v3_write(&ctx)).await {
                Some(Ok(vouched)) => vouched,
                Some(Err(_)) => {
                    return refuse(state, deadline, &ctx, request, policy_rejected()).await;
                }
                None => return refuse(state, deadline, &ctx, request, deadline_exceeded()).await,
            };
        if !vouched {
            let failure = sdk_invalid(
                STAGED_WRITE_NOT_AUTHORIZED,
                "staged write transactions need an authenticated caller",
            );
            return finish_failed(state, deadline, &ctx, &staged, failure).await;
        }
    }

    // Plan policy authorizes every bound plan before any nonce or key
    // reservation and before live-authority provider I/O, as V2's request
    // policy runs before replay or I/O. It sees only the decoded request.
    let plans = match staged
        .plans()
        .iter()
        .map(|entry| entry.bound_plan(&state.query.declared))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(plans) => plans,
        Err(diagnostic) => return refuse(state, deadline, &ctx, request, diagnostic).await,
    };
    for (index, plan) in plans.iter().enumerate() {
        let types = touched_types(plan);
        let view = V3PolicyPlan::new(plan, &types);
        match await_before_deadline(
            deadline,
            state.pipeline.authorize_v3_plan(index, view, &ctx),
        )
        .await
        {
            Some(Ok(())) => {}
            Some(Err(_)) => return refuse(state, deadline, &ctx, request, policy_rejected()).await,
            None => return refuse(state, deadline, &ctx, request, deadline_exceeded()).await,
        }
    }

    // From here the outcome is owned by a detached task: a client that goes
    // away mid-commit cannot drop the observers or leave the key in flight.
    let task = tokio::spawn(
        run_staged(
            Arc::clone(shared),
            ctx,
            staged.clone(),
            plans,
            kind,
            Window { deadline, expires },
        )
        .with_current_subscriber(),
    );
    match task.await {
        Ok(outcome) => outcome,
        // A panicked write may have committed.
        Err(_) if kind == TransactionKind::Write => StagedOutcome::new(
            state.unknown(&staged),
            StagedOutcomeKind::Unknown,
            &unknown_code(),
        ),
        Err(_) => {
            let diagnostic = unavailable();
            StagedOutcome::new(
                state.refusal(request, &diagnostic),
                StagedOutcomeKind::Failed,
                diagnostic.code().as_str(),
            )
        }
    }
}

/// When a staged request must finish, and when its nonce may be forgotten.
#[derive(Clone, Copy)]
struct Window {
    deadline: Instant,
    expires: Instant,
}

/// Reserve the nonce, execute, observe and police one admitted request;
/// return the bytes to deliver.
#[tracing::instrument(target = "type_bridge.io", name = "type_bridge.io.request", skip_all, fields(nonce = staged.nonce(), plans = plans.len()))]
async fn run_staged(
    shared: Arc<StagedExecutor>,
    ctx: RequestContext,
    staged: StagedRequestV3,
    plans: Vec<QueryPlanV3>,
    kind: TransactionKind,
    window: Window,
) -> StagedOutcome {
    let state = shared.as_ref();
    let deadline = window.deadline;
    // Every executed request holds its nonce for its whole lifetime, so a
    // replayed request is refused before it can execute again.
    let reserve = state.query.replay_store.reserve(
        state.query.advertisement.executor().epoch(),
        staged.nonce(),
        window.expires,
    );
    let refused = match await_before_deadline(deadline, reserve).await {
        Some(Ok(())) => None,
        // An earlier copy of these exact write bytes ran and may have
        // committed, so the replay never reads as nothing applied.
        Some(Err(ReplayStoreError::Replayed)) if kind == TransactionKind::Write => {
            let reply = state.unknown(&staged);
            let code = unknown_code();
            return settle(
                state,
                deadline,
                &ctx,
                &staged,
                reply,
                StagedOutcomeKind::Unknown,
                &code,
                true,
            )
            .await;
        }
        Some(Err(ReplayStoreError::Replayed)) => Some(replayed()),
        Some(Err(ReplayStoreError::Capacity)) => Some(replay_capacity()),
        Some(Err(ReplayStoreError::Unavailable)) => Some(replay_unavailable()),
        None => Some(deadline_exceeded()),
    };
    if let Some(diagnostic) = refused {
        let reply = state.refusal(Some(&staged), &diagnostic);
        let code = diagnostic.code().as_str();
        return settle(
            state,
            deadline,
            &ctx,
            &staged,
            reply,
            StagedOutcomeKind::Failed,
            code,
            false,
        )
        .await;
    }

    let executed = execute_staged(state, &staged, &plans, kind, deadline).await;
    // Once a write committed, or may have, no reply may read as a
    // safe-to-retry refusal.
    let may_have_committed = kind == TransactionKind::Write
        && executed.as_ref().map_or_else(
            |diagnostic| diagnostic.commit_outcome() == Some(SdkCommitFailureOutcome::Unknown),
            |_| true,
        );
    // Observe-only and bounded like the response audit: a stuck or failing
    // observer never changes the reply.
    let outcome = V3CommitOutcome::new(kind, executed.as_ref().err());
    let _ = await_before_deadline(
        response_audit_deadline(deadline),
        state.pipeline.observe_v3_commit(&outcome, &ctx),
    )
    .await;
    let (reply, outcome, code) = match executed {
        Ok(answers) => match state.signed(&staged, StagedOutcomeV3::Committed, answers) {
            Ok(reply) => (reply, StagedOutcomeKind::Committed, "ok".to_owned()),
            // The write already committed, so an unsignable success (e.g.
            // over its byte budget) must not read as a safe-to-retry refusal.
            Err(_) if kind == TransactionKind::Write => {
                let reply = state.unknown(&staged);
                (reply, StagedOutcomeKind::Unknown, unknown_code())
            }
            Err(diagnostic) => {
                let code = diagnostic.code().as_str().to_owned();
                (
                    state.refusal(Some(&staged), &diagnostic),
                    StagedOutcomeKind::Failed,
                    code,
                )
            }
        },
        Err(diagnostic) => {
            let code = diagnostic.code().as_str().to_owned();
            let outcome = if diagnostic.commit_outcome() == Some(SdkCommitFailureOutcome::Unknown) {
                StagedOutcomeKind::Unknown
            } else {
                StagedOutcomeKind::Failed
            };
            (state.failed(&staged, diagnostic), outcome, code)
        }
    };
    settle(
        state,
        deadline,
        &ctx,
        &staged,
        reply,
        outcome,
        &code,
        may_have_committed,
    )
    .await
}

fn unknown_code() -> String {
    SdkExecutionDiagnostic::commit_failure(SdkCommitFailureOutcome::Unknown)
        .code()
        .as_str()
        .to_owned()
}

/// Police `reply` and return what is delivered.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(target = "type_bridge.io", name = "type_bridge.io.reply", skip_all, fields(bytes_out = tracing::field::Empty, outcome = tracing::field::Empty, signing_key_id = %state.advertisement.reply_key_id().to_hex()))]
async fn settle(
    state: &StagedExecutor,
    deadline: Instant,
    ctx: &RequestContext,
    staged: &StagedRequestV3,
    reply: Vec<u8>,
    kind: StagedOutcomeKind,
    code: &str,
    may_have_committed: bool,
) -> StagedOutcome {
    // The response policy decides: a write it withholds after commit becomes
    // an unknown outcome, and any other refusal replaces the reply.
    let success = kind == StagedOutcomeKind::Committed;
    let outcome = match police(state, deadline, ctx, success, code, reply.len()).await {
        None => StagedOutcome::new(reply, kind, code),
        Some(_) if may_have_committed => StagedOutcome::new(
            state.unknown(staged),
            StagedOutcomeKind::Unknown,
            &unknown_code(),
        ),
        Some(refusal) => StagedOutcome::new(
            state.refusal(Some(staged), &refusal),
            StagedOutcomeKind::Failed,
            refusal.code().as_str(),
        ),
    };
    tracing::Span::current().record("bytes_out", outcome.reply.len());
    tracing::Span::current().record("outcome", outcome.code.as_str());
    tracing::debug!(target: "type_bridge.io", bytes_out = outcome.reply.len(), outcome = outcome.code.as_str(), "type_bridge.io.reply");
    outcome
}

/// Stage every policy-authorized bound plan in order on one core
/// transaction, then commit.
///
/// Nothing opens until commit, so a plan refused while staging leaves no
/// provider transaction behind.
async fn execute_staged(
    state: &StagedExecutor,
    staged: &StagedRequestV3,
    plans: &[QueryPlanV3],
    kind: TransactionKind,
    deadline: Instant,
) -> Result<Vec<StagedAnswerV3>, SdkExecutionDiagnostic> {
    let lower = lower_remote_query_diagnostic;
    await_before_deadline(deadline, state.query.verify_live_authority())
        .await
        .ok_or_else(|| lower(deadline_exceeded()))?
        .map_err(lower)?;
    // Enforce the advertised answer ceilings, tightened by the request's
    // own budgets and deadline, while answers are read, not only once the
    // reply is built. The deadline binds as the answer timeout: a provider
    // transaction lifetime is not something every backend supports.
    let requested = staged.limits();
    let mut limits = state.query.ceilings.clone();
    limits.answer.max_items = limits.answer.max_items.min(requested.max_items);
    limits.answer.max_bytes = limits.answer.max_bytes.min(requested.max_bytes);
    if let Some(maximum) = requested.max_graph_nodes {
        limits.max_graph_nodes = limits.max_graph_nodes.min(maximum);
    }
    if let Some(maximum) = requested.max_attribute_values {
        limits.max_attribute_values = limits.max_attribute_values.min(maximum);
    }
    if let Some(maximum) = requested.max_collection_members {
        limits.max_collection_members = limits.max_collection_members.min(maximum);
    }
    if let Some(maximum) = requested.max_role_players {
        limits.max_role_players = limits.max_role_players.min(maximum);
    }
    limits.answer.deadline = Some(
        limits
            .answer
            .deadline
            .map_or(deadline, |own| own.min(deadline)),
    );
    let options = TransactionOptions::default()
        .with_kind(kind)
        .with_answer_limits(limits);
    let mut transaction = state.query.database.transaction(options);
    let mut pending = Vec::with_capacity(plans.len());
    for plan in plans {
        pending.push(transaction.stage_plan(plan).map_err(pending_diagnostic)?);
    }
    match await_before_deadline(deadline, transaction.commit()).await {
        Some(Ok(())) => {}
        Some(Err(error)) => return Err(error.diagnostic()),
        // A write abandoned mid-commit may or may not have applied.
        None if kind == TransactionKind::Write => {
            return Err(SdkExecutionDiagnostic::commit_failure(
                SdkCommitFailureOutcome::Unknown,
            ));
        }
        None => return Err(lower(deadline_exceeded())),
    }
    // Plan answers resolved before the provider commit; only their reply
    // mapping runs now, after a write already committed.
    pending
        .iter()
        .zip(plans)
        .map(|(answer, plan)| {
            let answer = answer.get().map_err(pending_diagnostic)?;
            answer.to_staged_v3(plan).map_err(lower)
        })
        .collect::<Result<_, _>>()
        .map_err(|diagnostic| match kind {
            TransactionKind::Write => {
                SdkExecutionDiagnostic::commit_failure(SdkCommitFailureOutcome::Unknown)
            }
            _ => diagnostic,
        })
}

fn pending_diagnostic(error: PendingError) -> SdkExecutionDiagnostic {
    match error {
        PendingError::InvalidPlan(diagnostic) => lower_remote_query_diagnostic(*diagnostic),
        PendingError::Failed { diagnostic, .. } | PendingError::Refused { diagnostic, .. } => {
            *diagnostic
        }
        _ => SdkExecutionDiagnostic::internal_failure(),
    }
}

/// Entity and relation types a plan matches, inserts or puts, sorted.
fn touched_types(plan: &QueryPlanV3) -> Vec<TypeId> {
    fn walk(patterns: &[QueryPattern], types: &mut BTreeSet<TypeId>) {
        for pattern in patterns {
            match pattern {
                QueryPattern::Isa { type_id, .. } => {
                    types.insert(type_id.clone());
                }
                QueryPattern::Links { relation_id, .. } => {
                    types.insert(relation_id.clone());
                }
                QueryPattern::Reachable { relation, .. } => {
                    types.insert(relation.clone());
                }
                QueryPattern::Or { branches } => {
                    branches.iter().for_each(|branch| walk(branch, types));
                }
                QueryPattern::Not { patterns } | QueryPattern::Try { patterns } => {
                    walk(patterns, types);
                }
                _ => {}
            }
        }
    }
    let mut types = BTreeSet::new();
    for stage in plan.pipeline() {
        match stage {
            Stage::Match { patterns } => {
                for pattern in patterns {
                    if let Pattern::Read(pattern) = pattern {
                        walk(std::slice::from_ref(pattern), &mut types);
                    }
                }
            }
            Stage::Insert { things } | Stage::Put { things, .. } => {
                types.extend(things.iter().map(|thing| thing.type_id.clone()));
            }
            _ => {}
        }
    }
    types.into_iter().collect()
}

fn sdk_invalid(code: &'static str, message: &'static str) -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::invalid_input(
        SdkDiagnosticCode::new(code).expect("static staged diagnostic code"),
        SdkDiagnosticMessage::new(message).expect("static staged diagnostic message"),
    )
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

async fn admit_transport(
    state: &StagedExecutor,
    headers: &HeaderMap,
    endpoint: &'static str,
    deadline: Instant,
) -> Result<RequestContext, Box<Response>> {
    let mut metadata = staged_request_metadata(headers, endpoint);
    metadata.insert("transport".to_owned(), serde_json::json!("v3"));
    metadata.insert("v3_endpoint".to_owned(), serde_json::json!(endpoint));
    // A policy that has not declared V3 coverage fails closed here, before
    // its transport hook runs, as V2 does for AST-only policies.
    let mut ctx = state
        .pipeline
        .v3_request_context(metadata)
        .map_err(|_| Box::new(envelope_response(state.unbound(&policy_rejected()))))?;
    match await_before_deadline(deadline, state.pipeline.begin_v3_request(&mut ctx)).await {
        Some(Ok(())) => Ok(ctx),
        Some(Err(_)) => Err(Box::new(envelope_response(
            refuse(state, deadline, &ctx, None, policy_rejected())
                .await
                .reply,
        ))),
        None => Err(Box::new(envelope_response(
            refuse(state, deadline, &ctx, None, deadline_exceeded())
                .await
                .reply,
        ))),
    }
}

async fn refuse(
    state: &StagedExecutor,
    deadline: Instant,
    ctx: &RequestContext,
    request: Option<&StagedRequestV3>,
    diagnostic: Diagnostic,
) -> StagedOutcome {
    let reply = state.refusal(request, &diagnostic);
    finish(
        state,
        deadline,
        ctx,
        request,
        StagedOutcomeKind::Failed,
        diagnostic.code().as_str(),
        reply,
    )
    .await
}

async fn finish_failed(
    state: &StagedExecutor,
    deadline: Instant,
    ctx: &RequestContext,
    request: &StagedRequestV3,
    diagnostic: SdkExecutionDiagnostic,
) -> StagedOutcome {
    let code = diagnostic.code().as_str().to_owned();
    let reply = state.failed(request, diagnostic);
    finish(
        state,
        deadline,
        ctx,
        Some(request),
        StagedOutcomeKind::Failed,
        &code,
        reply,
    )
    .await
}

/// Run the response/audit policies, then deliver `reply` or their refusal.
async fn finish(
    state: &StagedExecutor,
    deadline: Instant,
    ctx: &RequestContext,
    request: Option<&StagedRequestV3>,
    kind: StagedOutcomeKind,
    code: &str,
    reply: Vec<u8>,
) -> StagedOutcome {
    tracing::debug!(target: "type_bridge.io", bytes_out = reply.len(), outcome = code, "type_bridge.io.reply");
    let success = kind == StagedOutcomeKind::Committed;
    match police(state, deadline, ctx, success, code, reply.len()).await {
        Some(refusal) => StagedOutcome::new(
            state.refusal(request, &refusal),
            StagedOutcomeKind::Failed,
            refusal.code().as_str(),
        ),
        None => StagedOutcome::new(reply, kind, code),
    }
}

/// Run the response/audit policies over a reply of `len` bytes; return why
/// they withhold it, if they do.
async fn police(
    state: &StagedExecutor,
    deadline: Instant,
    ctx: &RequestContext,
    success: bool,
    code: &str,
    len: usize,
) -> Option<Diagnostic> {
    let outcome = V3PolicyOutcome::new(success, code, len);
    match await_before_deadline(
        response_audit_deadline(deadline),
        state.pipeline.finish_v3_request(&outcome, ctx),
    )
    .await
    {
        Some(Ok(())) => None,
        Some(Err(_)) => Some(policy_rejected()),
        None => Some(deadline_exceeded()),
    }
}

#[cfg(test)]
mod pending_diagnostic_redaction_tests {
    use super::*;
    #[test]
    fn pending_failure_uses_the_canonical_diagnostic_without_server_text() {
        let error = PendingError::Failed {
            diagnostic: Box::new(
                type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic::commit_failure(
                    type_bridge_contract::sdk_diagnostic::SdkCommitFailureOutcome::Unknown,
                ),
            ),
            op_index: None,
            message: "signed-server-secret".into(),
            failure: Some(Box::new(
                type_bridge_contract::failure::Failure::Indeterminate { code: None },
            )),
            context: Default::default(),
        };
        assert!(!error.to_string().contains("signed-server-secret"));
        let surfaced = pending_diagnostic(error);
        assert!(
            !serde_json::to_string(&surfaced.host_json())
                .unwrap()
                .contains("signed-server-secret")
        );
        assert_eq!(surfaced.code().as_str(), "commit_outcome_unknown");
    }
}
