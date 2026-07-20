//! Versioned V2 query endpoints over the remote plan envelope.
//!
//! These routes live beside the retained V1 surface and never reinterpret
//! its payloads: `/v2/query` consumes and produces the versioned envelope
//! bytes defined by `type-bridge-contract`, executed through the shared
//! `type-bridge-orm` engine, and `/v2/capabilities` advertises the
//! executor's capability set for pre-flight negotiation. Transport stays
//! dumb: every `/v2/query` reply is an envelope with HTTP 200, and failure
//! envelopes carry the structured diagnostic.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::query_remote::{RemoteCapabilities, RemoteQueryFailure};
use type_bridge_contract::schema_delta::ManagedSchemaState;
use type_bridge_orm::query_v2_remote::{execute_admitted_remote_request, preflight_remote_request};
use type_bridge_orm::session::backend::BoundedAnswerLimits;
use type_bridge_orm::session::database::Database;
use type_bridge_query::MigrationAssertionValidationContext;
use type_bridge_schema::ResolvedSchema;

use crate::pipeline::QueryPipeline;

/// Everything one V2 query executor needs to serve envelopes.
pub struct V2QueryState {
    /// The advertised executor capability set.
    pub advertised: CapabilitySet,
    /// Executor ceilings caller budgets tighten under.
    pub ceilings: BoundedAnswerLimits,
    /// The connected database plans execute against.
    pub database: Database,
    /// The managed schema state plans must bind.
    pub managed: ManagedSchemaState,
    /// The resolved schema authority plans validate against.
    pub resolved: ResolvedSchema,
}

/// Transport body ceiling: the canonical 16 MiB envelope limit plus
/// framing slack. Any contract-valid envelope is admitted and answered
/// with an envelope; bodies beyond this explicit transport ceiling are
/// refused at the transport layer before buffering.
const V2_BODY_LIMIT_BYTES: usize = type_bridge_contract::limits::MAX_CANONICAL_BYTES + 64 * 1024;

/// Build the V2 route surface over one executor state.
pub fn create_v2_router(state: Arc<V2QueryState>) -> Router {
    Router::new()
        .route("/v2/query", post(handle_v2_query))
        .route("/v2/capabilities", get(handle_v2_capabilities))
        .layer(axum::extract::DefaultBodyLimit::max(V2_BODY_LIMIT_BYTES))
        .with_state(state)
}

/// Build the complete router: retained V1 routes beside the V2 surface.
pub fn create_router_with_v2(pipeline: Arc<QueryPipeline>, v2: Arc<V2QueryState>) -> Router {
    super::http::create_router(pipeline).merge(create_v2_router(v2))
}

async fn handle_v2_query(State(state): State<Arc<V2QueryState>>, body: Bytes) -> Response {
    // Preflight before any provider resource: malformed, stale,
    // unsupported, or over-budget traffic never opens a transaction.
    let context = MigrationAssertionValidationContext::new(&state.resolved, &state.managed);
    let admitted = match preflight_remote_request(
        &body,
        &context,
        &state.advertised,
        state.ceilings.clone(),
    ) {
        Ok(admitted) => admitted,
        Err(rejection) => {
            return envelope_response(rejection.into_failure_envelope());
        }
    };
    let mut transaction = match state.database.read_transaction().await {
        Ok(transaction) => transaction,
        Err(_) => {
            return envelope_response(
                RemoteQueryFailure::new(Some(admitted.nonce().to_owned()), &unavailable())
                    .encode()
                    .unwrap_or_default(),
            );
        }
    };
    let bytes = execute_admitted_remote_request(admitted, &mut transaction).await;
    envelope_response(bytes)
}

async fn handle_v2_capabilities(State(state): State<Arc<V2QueryState>>) -> Response {
    envelope_response(
        RemoteCapabilities::new(state.advertised.clone())
            .encode()
            .unwrap_or_default(),
    )
}

fn envelope_response(bytes: Vec<u8>) -> Response {
    ([(CONTENT_TYPE, "application/json")], bytes).into_response()
}

fn unavailable() -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::Integrity,
        DiagnosticCode::new("query_remote_provider_unavailable").expect("static remote code"),
        "the executor could not open a read transaction",
    )
}
