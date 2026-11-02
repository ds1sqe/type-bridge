//! Opt-in raw TypeQL transport. Host admission applies; TypeQL is unchecked.

use super::staged_state::{
    ReplayStoreError, await_before_deadline, response_audit_deadline, staged_request_metadata,
};
use super::v3::StagedExecutor;
use crate::interceptor::{RequestContext, V3CommitOutcome, V3PolicyOutcome};
use axum::body::to_bytes;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use type_bridge_orm::session::staged::TransactionKind;
use type_bridge_typedb_runtime::raw::RawQueryError;

/// Raw execution at the provider boundary, shared by embedded and HTTP hosts.
pub trait RawQueryExecutor: Send + Sync {
    /// Return the exact TypeDB HTTP query-answer JSON bytes for one transaction.
    fn execute<'a>(
        &'a self,
        database: &'a str,
        query: &'a str,
        kind: TransactionKind,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, RawQueryError>> + Send + 'a>>;
}

impl RawQueryExecutor for type_bridge_typedb_runtime::TypeDBRuntime {
    fn execute<'a>(
        &'a self,
        database: &'a str,
        query: &'a str,
        kind: TransactionKind,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, RawQueryError>> + Send + 'a>> {
        let kind = match kind {
            TransactionKind::Read => type_bridge_typedb_runtime::TxType::Read,
            TransactionKind::Write => type_bridge_typedb_runtime::TxType::Write,
            TransactionKind::Schema => type_bridge_typedb_runtime::TxType::Schema,
        };
        Box::pin(self.execute_raw(database, query, kind))
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRequest {
    query: String,
    kind: String,
    nonce: String,
    expires_at_unix_ms: u64,
}

fn refusal(status: StatusCode, code: &str) -> Response {
    (status, axum::Json(serde_json::json!({"error":code}))).into_response()
}

pub(super) async fn handle_raw(
    State(state): State<Arc<StagedExecutor>>,
    request: Request,
) -> Response {
    let deadline = Instant::now()
        .checked_add(state.query.execution_timeout)
        .unwrap_or_else(Instant::now);
    let mut metadata = staged_request_metadata(request.headers(), "raw");
    metadata.insert("query_format".into(), serde_json::json!("raw-typeql"));
    let mut ctx = state.pipeline().request_context(metadata);
    let Some(executor) = state.query.raw_executor.clone() else {
        return refuse(
            &state,
            &ctx,
            deadline,
            StatusCode::FORBIDDEN,
            "raw_disabled",
        )
        .await;
    };
    if state.pipeline().validate_v3_coverage().is_err() {
        return refuse(
            &state,
            &ctx,
            deadline,
            StatusCode::FORBIDDEN,
            "query_remote_policy_rejected",
        )
        .await;
    }
    if !matches!(
        await_before_deadline(deadline, state.pipeline().begin_v3_request(&mut ctx)).await,
        Some(Ok(()))
    ) {
        return refuse(
            &state,
            &ctx,
            deadline,
            StatusCode::FORBIDDEN,
            "query_remote_policy_rejected",
        )
        .await;
    }
    let body = match await_before_deadline(
        deadline,
        to_bytes(request.into_body(), state.query.body_limit_bytes()),
    )
    .await
    {
        Some(Ok(body)) => body,
        _ => {
            return refuse(
                &state,
                &ctx,
                deadline,
                StatusCode::BAD_REQUEST,
                "raw_request_invalid",
            )
            .await;
        }
    };
    let request = match serde_json::from_slice::<RawRequest>(&body) {
        Ok(request) => request,
        Err(_) => {
            return refuse(
                &state,
                &ctx,
                deadline,
                StatusCode::BAD_REQUEST,
                "raw_request_invalid",
            )
            .await;
        }
    };
    ctx.metadata.insert(
        "compiled_typeql".into(),
        serde_json::Value::String(request.query.clone()),
    );
    let kind = match request.kind.as_str() {
        "read" => TransactionKind::Read,
        "write" => TransactionKind::Write,
        "schema" => TransactionKind::Schema,
        _ => {
            return refuse(
                &state,
                &ctx,
                deadline,
                StatusCode::BAD_REQUEST,
                "raw_request_invalid",
            )
            .await;
        }
    };
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let Some(validity_ms) = u128::from(request.expires_at_unix_ms)
        .checked_sub(now_ms)
        .filter(|ms| *ms > 0 && *ms <= state.query.admission_window.as_millis())
    else {
        return refuse(
            &state,
            &ctx,
            deadline,
            StatusCode::BAD_REQUEST,
            "raw_request_expired",
        )
        .await;
    };
    if !(16..=128).contains(&request.nonce.len())
        || !request
            .nonce
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return refuse(
            &state,
            &ctx,
            deadline,
            StatusCode::BAD_REQUEST,
            "raw_request_invalid",
        )
        .await;
    }
    let Some(retain_until) = Instant::now().checked_add(Duration::from_millis(
        u64::try_from(validity_ms).expect("validity fits the u64 expiry"),
    )) else {
        return refuse(
            &state,
            &ctx,
            deadline,
            StatusCode::BAD_REQUEST,
            "raw_request_invalid",
        )
        .await;
    };
    ctx.transaction_type = request.kind;
    match await_before_deadline(
        deadline,
        state
            .pipeline()
            .authorize_v3_raw(&request.query, kind, &mut ctx),
    )
    .await
    {
        Some(Ok(())) => {}
        Some(Err(error)) => {
            return refuse(&state, &ctx, deadline, StatusCode::FORBIDDEN, error.code()).await;
        }
        None => {
            return refuse(
                &state,
                &ctx,
                deadline,
                StatusCode::GATEWAY_TIMEOUT,
                "transaction_deadline_exceeded",
            )
            .await;
        }
    }
    if kind != TransactionKind::Read && !state.query.allow_unauthenticated_staged_writes {
        match await_before_deadline(deadline, state.pipeline().authorize_v3_write(&ctx)).await {
            Some(Ok(true)) => {}
            Some(Ok(false)) => {
                return refuse(
                    &state,
                    &ctx,
                    deadline,
                    StatusCode::FORBIDDEN,
                    super::v3::STAGED_WRITE_NOT_AUTHORIZED,
                )
                .await;
            }
            Some(Err(_)) => {
                return refuse(
                    &state,
                    &ctx,
                    deadline,
                    StatusCode::FORBIDDEN,
                    "query_remote_policy_rejected",
                )
                .await;
            }
            None => {
                return refuse(
                    &state,
                    &ctx,
                    deadline,
                    StatusCode::GATEWAY_TIMEOUT,
                    "transaction_deadline_exceeded",
                )
                .await;
            }
        }
    }
    if Instant::now() >= retain_until {
        return refuse(
            &state,
            &ctx,
            deadline,
            StatusCode::BAD_REQUEST,
            "raw_request_expired",
        )
        .await;
    }
    match await_before_deadline(
        deadline,
        state.query.replay_store.reserve(
            state.advertisement().executor().epoch(),
            &request.nonce,
            retain_until,
        ),
    )
    .await
    {
        Some(Ok(())) => {}
        Some(Err(ReplayStoreError::Replayed)) => {
            return refuse(
                &state,
                &ctx,
                deadline,
                StatusCode::CONFLICT,
                "query_remote_replay",
            )
            .await;
        }
        Some(Err(ReplayStoreError::Capacity)) => {
            return refuse(
                &state,
                &ctx,
                deadline,
                StatusCode::SERVICE_UNAVAILABLE,
                "query_remote_replay_capacity",
            )
            .await;
        }
        _ => {
            return refuse(
                &state,
                &ctx,
                deadline,
                StatusCode::SERVICE_UNAVAILABLE,
                "query_remote_replay_unavailable",
            )
            .await;
        }
    }
    // Never cancel a commit: once dispatch starts the provider completes its
    // one transaction, and the host observes the actual durability outcome.
    let finish_state = Arc::clone(&state);
    let finish_ctx = ctx.clone();
    let task = tokio::spawn(async move {
        let answer = executor
            .execute(state.query.database.database_name(), &request.query, kind)
            .await;
        if answer.is_ok() && kind != TransactionKind::Read {
            let outcome = V3CommitOutcome::new(kind, None);
            let _ = await_before_deadline(
                response_audit_deadline(deadline),
                state.pipeline().observe_v3_commit(&outcome, &ctx),
            )
            .await;
        }
        let (response, success, code, size) = match answer {
            Ok(bytes) => {
                let size = bytes.len();
                (
                    (
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        bytes,
                    )
                        .into_response(),
                    true,
                    "ok",
                    size,
                )
            }
            Err(error) => {
                let status = StatusCode::from_u16(error.status).unwrap_or(StatusCode::BAD_GATEWAY);
                ((status,axum::Json(serde_json::json!({"error":"typedb_error","typedb":{"code":error.code,"message":error.message}}))).into_response(),false,"typedb_error",0)
            }
        };
        finish(
            &state,
            &ctx,
            deadline,
            response,
            V3PolicyOutcome::new(success, code, size),
            success && kind != TransactionKind::Read,
        )
        .await
    });
    match task.await {
        Ok(response) => response,
        Err(_) => {
            refuse(
                &finish_state,
                &finish_ctx,
                deadline,
                StatusCode::SERVICE_UNAVAILABLE,
                "commit_outcome_unknown",
            )
            .await
        }
    }
}

async fn refuse(
    state: &StagedExecutor,
    ctx: &RequestContext,
    deadline: Instant,
    status: StatusCode,
    code: &str,
) -> Response {
    finish(
        state,
        ctx,
        deadline,
        refusal(status, code),
        V3PolicyOutcome::new(false, code, 0),
        false,
    )
    .await
}

async fn finish(
    state: &StagedExecutor,
    ctx: &RequestContext,
    deadline: Instant,
    response: Response,
    outcome: V3PolicyOutcome<'_>,
    may_have_committed: bool,
) -> Response {
    if !matches!(
        await_before_deadline(
            response_audit_deadline(deadline),
            state.pipeline().finish_v3_request(&outcome, ctx)
        )
        .await,
        Some(Ok(()))
    ) {
        return refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            if may_have_committed {
                "commit_outcome_unknown"
            } else {
                "query_remote_policy_rejected"
            },
        );
    }
    response
}
