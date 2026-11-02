//! Opt-in raw TypeQL transport. Host admission applies; TypeQL is unchecked.

use super::staged_state::{
    ReplayStoreError, await_before_deadline, response_audit_deadline, staged_request_metadata,
};
use super::v3::StagedExecutor;
use crate::interceptor::{V3CommitOutcome, V3PolicyOutcome};
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
    let Some(executor) = &state.query.raw_executor else {
        return refusal(StatusCode::FORBIDDEN, "raw_disabled");
    };
    let Some(deadline) = Instant::now().checked_add(state.query.execution_timeout) else {
        return refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            "transaction_deadline_exceeded",
        );
    };
    let mut ctx = match state
        .pipeline()
        .v3_request_context(staged_request_metadata(request.headers(), "raw"))
    {
        Ok(ctx) => ctx,
        Err(_) => return refusal(StatusCode::FORBIDDEN, "query_remote_policy_rejected"),
    };
    if !matches!(
        await_before_deadline(deadline, state.pipeline().begin_v3_request(&mut ctx)).await,
        Some(Ok(()))
    ) {
        return refusal(StatusCode::FORBIDDEN, "query_remote_policy_rejected");
    }
    let body = match await_before_deadline(
        deadline,
        to_bytes(request.into_body(), state.query.body_limit_bytes()),
    )
    .await
    {
        Some(Ok(body)) => body,
        _ => return refusal(StatusCode::BAD_REQUEST, "raw_request_invalid"),
    };
    let request = match serde_json::from_slice::<RawRequest>(&body) {
        Ok(request) => request,
        Err(_) => return refusal(StatusCode::BAD_REQUEST, "raw_request_invalid"),
    };
    let kind = match request.kind.as_str() {
        "read" => TransactionKind::Read,
        "write" => TransactionKind::Write,
        "schema" => TransactionKind::Schema,
        _ => return refusal(StatusCode::BAD_REQUEST, "raw_request_invalid"),
    };
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let Some(validity_ms) = u128::from(request.expires_at_unix_ms)
        .checked_sub(now_ms)
        .filter(|ms| *ms > 0 && *ms <= state.query.admission_window.as_millis())
    else {
        return refusal(StatusCode::BAD_REQUEST, "raw_request_expired");
    };
    if !(16..=128).contains(&request.nonce.len())
        || !request
            .nonce
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return refusal(StatusCode::BAD_REQUEST, "raw_request_invalid");
    }
    let Some(retain_until) = Instant::now().checked_add(Duration::from_millis(
        u64::try_from(validity_ms).expect("validity fits the u64 expiry"),
    )) else {
        return refusal(StatusCode::BAD_REQUEST, "raw_request_invalid");
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
            let outcome = V3PolicyOutcome::new(false, error.code(), 0);
            let _ = await_before_deadline(
                response_audit_deadline(deadline),
                state.pipeline().finish_v3_request(&outcome, &ctx),
            )
            .await;
            return refusal(StatusCode::FORBIDDEN, error.code());
        }
        None => return refusal(StatusCode::GATEWAY_TIMEOUT, "transaction_deadline_exceeded"),
    }
    if Instant::now() >= retain_until {
        return refusal(StatusCode::BAD_REQUEST, "raw_request_expired");
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
            return refusal(StatusCode::CONFLICT, "query_remote_replay");
        }
        Some(Err(ReplayStoreError::Capacity)) => {
            return refusal(
                StatusCode::SERVICE_UNAVAILABLE,
                "query_remote_replay_capacity",
            );
        }
        _ => {
            return refusal(
                StatusCode::SERVICE_UNAVAILABLE,
                "query_remote_replay_unavailable",
            );
        }
    }
    // Never cancel a commit: once dispatch starts the provider completes its
    // one transaction, and the host observes the actual durability outcome.
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
    let outcome = V3PolicyOutcome::new(success, code, size);
    if !matches!(
        await_before_deadline(
            response_audit_deadline(deadline),
            state.pipeline().finish_v3_request(&outcome, &ctx)
        )
        .await,
        Some(Ok(()))
    ) {
        return refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            if success && kind != TransactionKind::Read {
                "commit_outcome_unknown"
            } else {
                "query_remote_policy_rejected"
            },
        );
    }
    response
}
