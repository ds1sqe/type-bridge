//! The proxy route of a staged [`super::staged::Transaction`]: commit sends
//! the stack as one signed `query-remote/v3` exchange.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rand_core::{OsRng, RngCore};
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::limits::MAX_REMOTE_ENVELOPE_BYTES;
use type_bridge_contract::query_remote::RemoteCapabilities;
use type_bridge_contract::query_remote_v3::{
    StagedLimitsV3, StagedPlanV3, StagedRequestSpecV3, StagedRequestV3,
    VerifiedStagedReplyV3,
};
use type_bridge_contract::schema::DeclaredSchema;
use type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic;

use super::backend::{AnswerCancellation, BoxFuture};
use super::proxy_connection::ProxyHttpTransport;
use crate::query_execution_limits::{QueryExecutionDeadline, QueryExecutionResourceLimits};
use crate::query_v2_remote::Ed25519RemoteReplyVerifier;

/// Caller-owned exchange of one staged request for its exact reply bytes.
pub type StagedExchangeFn = Arc<
    dyn Fn(Vec<u8>) -> BoxFuture<'static, Result<Vec<u8>, SdkExecutionDiagnostic>> + Send + Sync,
>;

/// How long the built-in route waits past the commit deadline for the
/// server's own deadline reply.
const SEND_GRACE: Duration = Duration::from_secs(5);

enum Route {
    Http(ProxyHttpTransport),
    Exchange(StagedExchangeFn),
}

/// What a proxy-backed transaction needs to send and verify one commit.
pub(crate) struct ProxyStaging {
    route: Route,
    advertisement: RemoteCapabilities,
    declared: DeclaredSchema,
    limits: QueryExecutionResourceLimits,
}

impl ProxyStaging {
    pub(crate) fn new(
        http: Option<ProxyHttpTransport>,
        exchange: Option<StagedExchangeFn>,
        advertisement: &[u8],
        declared: DeclaredSchema,
        limits: QueryExecutionResourceLimits,
    ) -> Result<Self, Diagnostic> {
        let route = match (exchange, http) {
            (Some(exchange), _) => Route::Exchange(exchange),
            (None, Some(http)) => Route::Http(http),
            (None, None) => {
                return Err(super::staged::proxy_refusal(
                    "a staged proxy transaction needs an exchange or the built-in route",
                ));
            }
        };
        Ok(Self {
            route,
            advertisement: RemoteCapabilities::decode(advertisement)?,
            declared,
            limits,
        })
    }

    pub(crate) fn declared(&self) -> &DeclaredSchema {
        &self.declared
    }

    /// Build one request with a fresh nonce.
    pub(crate) fn request(
        &self,
        plans: Vec<StagedPlanV3>,
        kind: super::staged::TransactionKind,
        deadline_ms: Option<u64>,
    ) -> Result<StagedRequestV3, Diagnostic> {
        use type_bridge_contract::query_remote_v3::StagedKindV3;
        let mut nonce = [0u8; 16];
        OsRng.fill_bytes(&mut nonce);
        let nonce = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            });
        let kind = match kind {
            super::staged::TransactionKind::Read => StagedKindV3::Read,
            super::staged::TransactionKind::Write => StagedKindV3::Write,
            super::staged::TransactionKind::Schema => StagedKindV3::Schema,
        };
        StagedRequestV3::new(
            StagedRequestSpecV3 {
                nonce,
                kind: Some(kind),
                limits: StagedLimitsV3 {
                    deadline_ms,
                    ..StagedLimitsV3::default()
                },
                plans,
                prepared_at_unix_ms: now,
            },
            &self.declared,
            &self.advertisement,
        )
    }

    /// Send the exact request bytes. Any error is a transport outcome: the
    /// request may have reached the server.
    ///
    /// `timeout` is the commit's own deadline, the one the request carries
    /// as `deadline_ms`; the built-in route waits that long plus a grace
    /// period, so it never gives up before the server does.
    pub(crate) async fn send(
        &self,
        bytes: Vec<u8>,
        timeout: Option<Duration>,
    ) -> Result<Vec<u8>, SdkExecutionDiagnostic> {
        match &self.route {
            Route::Exchange(exchange) => exchange(bytes).await,
            Route::Http(http) => {
                let cancellation = AnswerCancellation::default();
                let deadline = match timeout {
                    Some(timeout) => QueryExecutionDeadline::from_timeout_milliseconds(
                        u64::try_from(timeout.saturating_add(SEND_GRACE).as_millis())
                            .unwrap_or(u64::MAX),
                    ),
                    None => QueryExecutionDeadline::for_limits(self.limits.effective()),
                };
                http.exchange_staged(bytes, MAX_REMOTE_ENVELOPE_BYTES, deadline, &cancellation)
                    .await
            }
        }
    }

    pub(crate) fn verify(
        &self,
        reply: &[u8],
        request: &StagedRequestV3,
    ) -> Result<VerifiedStagedReplyV3, Diagnostic> {
        VerifiedStagedReplyV3::decode(
            reply,
            request,
            &self.advertisement,
            self.advertisement.reply_key(),
            &Ed25519RemoteReplyVerifier,
        )
    }
}
