//! Staged V3 transactions over the V2 executor identity.
//!
//! One canonical request carries an ordered stack of `query-plan/v3`
//! programs. The executor stages them on one core transaction, commits once
//! (a read transaction closes instead) and answers with one signed reply
//! bound to the exact request bytes.
//!
//! V3 shares the V2 executor binding, reply signer, policy chain and nonce
//! replay store. It advertises its own capability set at `/v3/capabilities`,
//! so the released V2 advertisement and its fingerprint stay unchanged.
//! Writes open only when a policy vouches for the caller through
//! [`Interceptor::on_v3_write_authority`](crate::interceptor::Interceptor::on_v3_write_authority)
//! or the operator explicitly allows unauthenticated staged writes.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::to_bytes;
use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{get, post};
use type_bridge_contract::capability::CapabilityId;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCode};
use type_bridge_contract::limits::MAX_REMOTE_ENVELOPE_BYTES;
use type_bridge_contract::query_plan::QueryOutput;
use type_bridge_contract::query_plan::v3::{
    Operation, QueryPlanV3, WriteReturns, query_plan_v3_capability_vocabulary,
};
use type_bridge_contract::query_remote::{RemoteCapabilities, RemoteCapabilitiesFingerprint};
use type_bridge_contract::query_remote_v3::{
    CAP_QUERY_REMOTE_IDEMPOTENCY_V3, CAP_QUERY_REMOTE_STAGED_V3, CAP_QUERY_REMOTE_WRITE_V3,
    StagedAnswerV3, StagedKindV3, StagedOutcomeV3, StagedReplyV3, StagedRequestV3,
};
use type_bridge_contract::sdk_diagnostic::{
    SdkCommitFailureOutcome, SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic,
};
use type_bridge_orm::lower_remote_query_diagnostic;
use type_bridge_orm::query_v2_remote::encode_unbound_remote_failure_v2;
use type_bridge_orm::session::staged::{
    PendingError, TransactionKind, TransactionOptions, staged_documents_unsupported,
};

use super::v2::{
    ReplayStoreError, V2_PREFLIGHT_SLOTS, V2QueryState, await_before_deadline, body_oversized,
    deadline_exceeded, declared_v2_body_oversized, envelope_response, policy_rejected,
    replay_capacity, replay_unavailable, replayed, response_audit_deadline, unavailable,
    v2_request_metadata,
};
use crate::idempotency::{IdempotencyAdmission, IdempotencyError, RetainedOutcome};
use crate::interceptor::{RequestContext, V2PolicyOutcome};
use crate::pipeline::QueryPipeline;

/// Code of the signed failure refusing a staged write no policy admitted.
pub const STAGED_WRITE_NOT_AUTHORIZED: &str = "staged_write_not_authorized";

/// Retention reserved for a keyed reply, at least enough for a signed failure.
const FAILURE_REPLY_ALLOWANCE: usize = 64 * 1024;

struct V3RouterState {
    pipeline: Arc<QueryPipeline>,
    query: Arc<V2QueryState>,
    advertisement: RemoteCapabilities,
    fingerprint: RemoteCapabilitiesFingerprint,
}

impl V3RouterState {
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

    fn refusal(&self, request: Option<&StagedRequestV3>, diagnostic: &Diagnostic) -> Vec<u8> {
        match request {
            Some(request) => {
                self.failed(request, lower_remote_query_diagnostic(diagnostic.clone()))
            }
            None => self.unbound(diagnostic),
        }
    }
}

/// The staged V3 advertisement: the V2 capabilities plus the staged
/// vocabulary, under the same executor binding, reply key and limits.
pub fn staged_advertisement(v2: &RemoteCapabilities) -> Result<RemoteCapabilities, Diagnostic> {
    let mut capabilities = v2.capabilities().clone();
    for capability in query_plan_v3_capability_vocabulary().iter() {
        capabilities.insert(capability.clone());
    }
    for capability in [
        CAP_QUERY_REMOTE_STAGED_V3,
        CAP_QUERY_REMOTE_WRITE_V3,
        CAP_QUERY_REMOTE_IDEMPOTENCY_V3,
    ] {
        capabilities.insert(CapabilityId::new(capability)?);
    }
    RemoteCapabilities::new(capabilities, v2.executor().clone(), v2.reply_key())
        .with_query_limits(v2.query_limits())?
        .with_answer_limits(v2.answer_limits().clone())
}

/// Build `POST /v3/staged` and `GET /v3/capabilities` over one V2 executor.
pub fn create_v3_router(pipeline: Arc<QueryPipeline>, query: Arc<V2QueryState>) -> Router {
    // The V2 advertisement's limits were validated when it was built, so
    // re-stating them under a wider capability set cannot fail.
    let advertisement = staged_advertisement(&query.advertisement)
        .expect("validated V2 advertisement re-states under the staged capabilities");
    let fingerprint = advertisement
        .fingerprint()
        .expect("staged advertisement fingerprints");
    let state = Arc::new(V3RouterState {
        pipeline,
        query,
        advertisement,
        fingerprint,
    });
    Router::new()
        .route("/v3/staged", post(handle_v3_staged))
        .route("/v3/capabilities", get(handle_v3_capabilities))
        .with_state(state)
}

async fn handle_v3_capabilities(
    State(state): State<Arc<V3RouterState>>,
    request: Request,
) -> Response {
    let Some(deadline) = Instant::now().checked_add(state.query.execution_timeout) else {
        return envelope_response(state.unbound(&deadline_exceeded()));
    };
    let ctx = match admit_transport(&state, request.headers(), "capabilities", deadline).await {
        Ok(ctx) => ctx,
        Err(response) => return *response,
    };
    match state.advertisement.encode() {
        Ok(bytes) => finish(&state, deadline, &ctx, None, true, "ok", bytes).await,
        Err(diagnostic) => refuse(&state, deadline, &ctx, None, diagnostic).await,
    }
}

async fn handle_v3_staged(State(state): State<Arc<V3RouterState>>, request: Request) -> Response {
    let Some(deadline) = Instant::now().checked_add(state.query.execution_timeout) else {
        return envelope_response(state.unbound(&deadline_exceeded()));
    };
    // Transport policy authenticates before any attacker-controlled bytes
    // are buffered, exactly as for `/v2/query`.
    let ctx = match admit_transport(&state, request.headers(), "staged", deadline).await {
        Ok(ctx) => ctx,
        Err(response) => return *response,
    };
    let state = state.as_ref();
    if declared_v2_body_oversized(request.headers(), state.query.body_limit_bytes()) {
        return refuse(state, deadline, &ctx, None, body_oversized()).await;
    }
    let permit = match await_before_deadline(
        deadline,
        Arc::clone(&V2_PREFLIGHT_SLOTS).acquire_owned(),
    )
    .await
    {
        Some(Ok(permit)) => permit,
        Some(Err(_)) => return refuse(state, deadline, &ctx, None, unavailable()).await,
        None => return refuse(state, deadline, &ctx, None, deadline_exceeded()).await,
    };
    let body = match await_before_deadline(
        deadline,
        to_bytes(request.into_body(), state.query.body_limit_bytes()),
    )
    .await
    {
        Some(Ok(body)) => body,
        Some(Err(_)) => return refuse(state, deadline, &ctx, None, body_oversized()).await,
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
            query.advertisement.query_limits(),
        )
    });
    let staged = match await_before_deadline(deadline, decode).await {
        Some(Ok(Ok(staged))) => staged,
        Some(Ok(Err(diagnostic))) => return refuse(state, deadline, &ctx, None, diagnostic).await,
        Some(Err(_)) => return refuse(state, deadline, &ctx, None, unavailable()).await,
        None => return refuse(state, deadline, &ctx, None, deadline_exceeded()).await,
    };
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
    if staged
        .plans()
        .iter()
        .any(|entry| returns_documents(entry.plan()))
    {
        return refuse(
            state,
            deadline,
            &ctx,
            request,
            staged_documents_unsupported(),
        )
        .await;
    }
    if kind == TransactionKind::Write && !state.query.allow_unauthenticated_staged_writes {
        match await_before_deadline(deadline, state.pipeline.authorize_v3_write(&ctx)).await {
            Some(Ok(true)) => {}
            Some(Ok(false)) => {
                let failure = sdk_invalid(
                    STAGED_WRITE_NOT_AUTHORIZED,
                    "staged write transactions need an authenticated caller",
                );
                return finish_failed(state, deadline, &ctx, &staged, failure).await;
            }
            Some(Err(_)) => return refuse(state, deadline, &ctx, request, policy_rejected()).await,
            None => return refuse(state, deadline, &ctx, request, deadline_exceeded()).await,
        }
    }

    // A keyed request reserves its retained reply; an unkeyed one its nonce.
    // A keyed retry legitimately repeats the nonce, so it skips the nonce
    // store and the idempotency fingerprint decides instead.
    let reservation = match staged.idempotency_key() {
        Some(key) => {
            let fingerprint = match staged.fingerprint() {
                Ok(fingerprint) => fingerprint,
                Err(diagnostic) => return refuse(state, deadline, &ctx, request, diagnostic).await,
            };
            let allowance = usize::try_from(staged.limits().max_bytes)
                .unwrap_or(usize::MAX)
                .clamp(FAILURE_REPLY_ALLOWANCE, MAX_REMOTE_ENVELOPE_BYTES);
            match state.query.idempotency.begin(
                &ctx.client_id,
                key,
                &fingerprint,
                expires,
                allowance,
                Instant::now(),
            ) {
                Ok(IdempotencyAdmission::Reserved(ticket)) => Some(ticket),
                Ok(IdempotencyAdmission::Replay { reply, outcome }) => {
                    // Audited and policed as what the first caller received.
                    let (success, code) = (outcome.success(), outcome.code().as_str());
                    return match police(state, deadline, &ctx, success, code, reply.len()).await {
                        None => envelope_response(reply.to_vec()),
                        // A withheld write replay may hide a commit. The
                        // retained entry stays.
                        Some(_) if kind == TransactionKind::Write => {
                            let unknown = SdkExecutionDiagnostic::commit_failure(
                                SdkCommitFailureOutcome::Unknown,
                            );
                            envelope_response(state.failed(&staged, unknown))
                        }
                        Some(refusal) => envelope_response(state.refusal(request, &refusal)),
                    };
                }
                Err(error) => {
                    return refuse(state, deadline, &ctx, request, error.diagnostic()).await;
                }
            }
        }
        None => {
            let reserve = state.query.replay_store.reserve(
                state.query.advertisement.executor().epoch(),
                staged.nonce(),
                expires,
            );
            match await_before_deadline(deadline, reserve).await {
                Some(Ok(())) => None,
                Some(Err(error)) => {
                    let diagnostic = match error {
                        ReplayStoreError::Replayed => replayed(),
                        ReplayStoreError::Capacity => replay_capacity(),
                        ReplayStoreError::Unavailable => replay_unavailable(),
                    };
                    return refuse(state, deadline, &ctx, request, diagnostic).await;
                }
                None => return refuse(state, deadline, &ctx, request, deadline_exceeded()).await,
            }
        }
    };

    let executed = execute_staged(state, &staged, kind, deadline).await;
    // Once a write committed, or may have, no reply may read as a
    // safe-to-retry refusal.
    let may_have_committed = kind == TransactionKind::Write
        && executed.as_ref().map_or_else(
            |diagnostic| diagnostic.commit_outcome() == Some(SdkCommitFailureOutcome::Unknown),
            |_| true,
        );
    let (reply, success, code) = match executed {
        Ok(answers) => match state.signed(&staged, StagedOutcomeV3::Committed, answers) {
            Ok(reply) => (reply, true, "ok".to_owned()),
            // The write already committed, so an unsignable success (e.g.
            // over its byte budget) must not read as a safe-to-retry refusal.
            Err(_) if kind == TransactionKind::Write => {
                let diagnostic =
                    SdkExecutionDiagnostic::commit_failure(SdkCommitFailureOutcome::Unknown);
                let code = diagnostic.code().as_str().to_owned();
                (state.failed(&staged, diagnostic), false, code)
            }
            Err(diagnostic) => {
                let code = diagnostic.code().as_str().to_owned();
                (state.refusal(request, &diagnostic), false, code)
            }
        },
        Err(diagnostic) => {
            let code = diagnostic.code().as_str().to_owned();
            (state.failed(&staged, diagnostic), false, code)
        }
    };
    // Minor 11.34: the observe-only `on_v3_commit(ctx, outcome)` hook runs
    // here, after execution and before the reply is retained.
    // The response policy decides before retention: a write it withholds
    // after commit becomes an unknown outcome, and a keyed retry replays
    // exactly that. Any other refusal is delivered but not retained.
    let (reply, refusal, success, code) =
        match police(state, deadline, &ctx, success, &code, reply.len()).await {
            None => (reply, None, success, code),
            Some(_) if may_have_committed => {
                let unknown =
                    SdkExecutionDiagnostic::commit_failure(SdkCommitFailureOutcome::Unknown);
                let code = unknown.code().as_str().to_owned();
                (state.failed(&staged, unknown), None, false, code)
            }
            Some(refusal) => {
                let refusal = state.refusal(Some(&staged), &refusal);
                (reply, Some(refusal), success, code)
            }
        };
    if let Some(ticket) = reservation {
        // A replay is audited with the outcome retained here. Every code is
        // a validated diagnostic code, so the summary always builds; a lost
        // completion leaves the key in flight until expiry, and the reply
        // itself is still delivered.
        let retained = DiagnosticCode::new(code)
            .map_err(|_| IdempotencyError::Unavailable)
            .and_then(|code| {
                state.query.idempotency.complete(
                    &ticket,
                    Arc::from(reply.as_slice()),
                    RetainedOutcome::new(success, code),
                    Instant::now(),
                )
            });
        if let Err(error) = retained {
            tracing::warn!(code = error.code(), "staged reply could not be retained");
        }
    }
    envelope_response(refusal.unwrap_or(reply))
}

/// Stage every bound plan in order on one core transaction, then commit.
///
/// Nothing opens until commit, so a plan refused while staging leaves no
/// provider transaction behind.
async fn execute_staged(
    state: &V3RouterState,
    request: &StagedRequestV3,
    kind: TransactionKind,
    deadline: Instant,
) -> Result<Vec<StagedAnswerV3>, SdkExecutionDiagnostic> {
    let lower = lower_remote_query_diagnostic;
    await_before_deadline(deadline, state.query.verify_live_authority())
        .await
        .ok_or_else(|| lower(deadline_exceeded()))?
        .map_err(lower)?;
    let plans = request
        .plans()
        .iter()
        .map(|entry| entry.bound_plan(&state.query.declared))
        .collect::<Result<Vec<_>, _>>()
        .map_err(lower)?;
    let mut transaction = state
        .query
        .database
        .transaction(TransactionOptions::default().with_kind(kind));
    let mut pending = Vec::with_capacity(plans.len());
    for plan in &plans {
        // Minor 11.34: the per-plan policy `on_v3_plan(ctx, index, plan)`
        // decides here, before this plan stages.
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
        .zip(&plans)
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
        PendingError::Failed {
            failure: Some(failure),
            context,
            ..
        } => SdkExecutionDiagnostic::from_failure(&failure, *context),
        _ => SdkExecutionDiagnostic::internal_failure(),
    }
}

fn returns_documents(plan: &QueryPlanV3) -> bool {
    matches!(plan.output(), QueryOutput::Documents { .. })
        && matches!(
            plan.operation(),
            Operation::Rows
                | Operation::Write {
                    returns: WriteReturns::Rows
                }
        )
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
    state: &V3RouterState,
    headers: &HeaderMap,
    endpoint: &'static str,
    deadline: Instant,
) -> Result<RequestContext, Box<Response>> {
    let mut metadata = v2_request_metadata(headers, endpoint);
    metadata.insert("transport".to_owned(), serde_json::json!("v3"));
    metadata.insert("v3_endpoint".to_owned(), serde_json::json!(endpoint));
    metadata.remove("v2_endpoint");
    // A policy that has not declared V3 coverage fails closed here, before
    // its transport hook runs, as V2 does for AST-only policies.
    let mut ctx = state
        .pipeline
        .v3_request_context(metadata)
        .map_err(|_| Box::new(envelope_response(state.unbound(&policy_rejected()))))?;
    match await_before_deadline(deadline, state.pipeline.begin_v2_request(&mut ctx)).await {
        Some(Ok(())) => Ok(ctx),
        Some(Err(_)) => Err(Box::new(
            refuse(state, deadline, &ctx, None, policy_rejected()).await,
        )),
        None => Err(Box::new(
            refuse(state, deadline, &ctx, None, deadline_exceeded()).await,
        )),
    }
}

async fn refuse(
    state: &V3RouterState,
    deadline: Instant,
    ctx: &RequestContext,
    request: Option<&StagedRequestV3>,
    diagnostic: Diagnostic,
) -> Response {
    let reply = state.refusal(request, &diagnostic);
    finish(
        state,
        deadline,
        ctx,
        request,
        false,
        diagnostic.code().as_str(),
        reply,
    )
    .await
}

async fn finish_failed(
    state: &V3RouterState,
    deadline: Instant,
    ctx: &RequestContext,
    request: &StagedRequestV3,
    diagnostic: SdkExecutionDiagnostic,
) -> Response {
    let code = diagnostic.code().as_str().to_owned();
    let reply = state.failed(request, diagnostic);
    finish(state, deadline, ctx, Some(request), false, &code, reply).await
}

/// Run the response/audit policies, then deliver `reply` or their refusal.
async fn finish(
    state: &V3RouterState,
    deadline: Instant,
    ctx: &RequestContext,
    request: Option<&StagedRequestV3>,
    success: bool,
    code: &str,
    reply: Vec<u8>,
) -> Response {
    match police(state, deadline, ctx, success, code, reply.len()).await {
        Some(refusal) => envelope_response(state.refusal(request, &refusal)),
        None => envelope_response(reply),
    }
}

/// Run the response/audit policies over a reply of `len` bytes; return why
/// they withhold it, if they do.
async fn police(
    state: &V3RouterState,
    deadline: Instant,
    ctx: &RequestContext,
    success: bool,
    code: &str,
    len: usize,
) -> Option<Diagnostic> {
    let outcome = V2PolicyOutcome::new(success, code, len);
    match await_before_deadline(
        response_audit_deadline(deadline),
        state.pipeline.finish_v2_request(&outcome, ctx),
    )
    .await
    {
        Some(Ok(())) => None,
        Some(Err(_)) => Some(policy_rejected()),
        None => Some(deadline_exceeded()),
    }
}
