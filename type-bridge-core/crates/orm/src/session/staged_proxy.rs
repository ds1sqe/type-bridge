//! The proxy route of a staged [`super::staged::Transaction`]: commit sends
//! the stack as one signed `query-remote/v3` exchange.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rand_core::{OsRng, RngCore};
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::limits::MAX_REMOTE_ENVELOPE_BYTES;
use type_bridge_contract::query_remote::RemoteCapabilities;
use type_bridge_contract::query_remote_v3::{
    StagedLimitsV3, StagedPlanV3, StagedRequestSpecV3, StagedRequestV3, VerifiedStagedReplyV3,
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
    admission_window_milliseconds: u64,
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
            admission_window_milliseconds:
                type_bridge_contract::query_remote::DEFAULT_ADMISSION_WINDOW_MILLISECONDS,
        })
    }

    pub(crate) fn with_admission_window_milliseconds(
        mut self,
        milliseconds: std::num::NonZeroU64,
    ) -> Self {
        self.admission_window_milliseconds = milliseconds.get();
        self
    }

    pub(crate) fn declared(&self) -> &DeclaredSchema {
        &self.declared
    }

    pub(crate) fn match_limits(&self) -> Result<QueryExecutionResourceLimits, Diagnostic> {
        let advertised = QueryExecutionResourceLimits::from_overrides(
            crate::query_execution_limits::AnswerTransport::Proxy,
            self.advertisement
                .answer_limits()
                .iter()
                .map(|(key, value)| (key.as_str(), *value)),
        )
        .map_err(|_| super::staged::proxy_refusal("advertised answer limits are invalid"))?;
        Ok(self.limits.constrained_by(advertised))
    }

    /// Build one request with a fresh nonce and an optional Match aggregate policy.
    pub(crate) fn request_with_policy(
        &self,
        plans: Vec<StagedPlanV3>,
        kind: super::staged::TransactionKind,
        deadline_ms: Option<u64>,
        policy: Option<QueryExecutionResourceLimits>,
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
        let deadline_ms = crate::QueryTimeout::from_milliseconds(deadline_ms.unwrap_or(0))
            .constrained_by(self.match_limits()?.timeout_milliseconds)
            .milliseconds();
        StagedRequestV3::new(
            StagedRequestSpecV3 {
                nonce,
                kind: Some(kind),
                limits: policy.map_or_else(
                    || StagedLimitsV3 {
                        collect_phase_timestamps: self.limits.collect_phase_timestamps,
                        deadline_ms,
                        admission_window_milliseconds: self.admission_window_milliseconds,
                        ..Default::default()
                    },
                    |policy| StagedLimitsV3 {
                        collect_phase_timestamps: self.limits.collect_phase_timestamps
                            || policy.collect_phase_timestamps,
                        deadline_ms: crate::QueryTimeout::from_milliseconds(
                            deadline_ms.unwrap_or(0),
                        )
                        .constrained_by(policy.timeout_milliseconds)
                        .milliseconds(),
                        admission_window_milliseconds: self.admission_window_milliseconds,
                        max_bytes: policy.bytes,
                        max_items: policy.items,
                        max_graph_nodes: Some(policy.graph_nodes),
                        max_attribute_values: Some(policy.attribute_values),
                        max_collection_members: Some(policy.collection_members),
                        max_role_players: Some(policy.role_players),
                        max_read_statements: policy.statement_limit(),
                        max_write_statements: policy.statement_limit(),
                        ..Default::default()
                    },
                ),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::match_request::*;
    use crate::session::staged::{PendingError, Transaction, TransactionOptions};
    use type_bridge_contract::query_remote::RemoteExecutorBinding;

    #[tokio::test]
    async fn invalid_proxy_match_limits_settle_every_pending_with_the_refusal() {
        let registry = crate::_registry::DescriptorRegistry::new();
        registry
            .register_entity(crate::_descriptor::EntityDescriptor {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        let authority =
            crate::match_request::descriptor_plan::MatchRequestAdapterAuthority::from_registry(
                &registry,
            )
            .unwrap();
        let signer = crate::query_v2_remote::RemoteReplySigningKey::from_secret_bytes([0x62; 32]);
        let advertisement = RemoteCapabilities::new(
            type_bridge_contract::query_plan::v3::query_plan_v3_capability_vocabulary(),
            RemoteExecutorBinding::new("invalid-match-limits", "epoch-00000000001").unwrap(),
            signer.public_key(),
        );
        let mut proxy = ProxyStaging::new(
            None,
            Some(Arc::new(|_| {
                Box::pin(async { panic!("a preflight refusal must not send") })
            })),
            &advertisement.encode().unwrap(),
            authority.declared().clone(),
            QueryExecutionResourceLimits::default(),
        )
        .unwrap();
        // Inject a future/invalid policy after wire admission to exercise the commit failure path.
        let mut raw = serde_json::to_value(&advertisement).unwrap();
        raw["answer_limits"] = serde_json::json!({"statements":4097});
        proxy.advertisement = serde_json::from_value(raw).unwrap();
        assert!(proxy.match_limits().is_err());
        let mut transaction = Transaction::new_proxy(proxy, TransactionOptions::default());
        let request = || {
            validate_match_request(
                &registry,
                MatchRequest::v1(
                    MatchPlan {
                        bindings: vec![MatchBinding {
                            id: BindingId::new(0),
                            descriptor: DescriptorId::new("entity:person"),
                            thing_kind: ThingKind::Entity,
                            match_mode: MatchMode::Exact,
                        }],
                        predicate: None,
                        allowed_cross_joins: Default::default(),
                    },
                    MatchOperation::CountBy {
                        root: BindingId::new(0),
                    },
                ),
            )
            .unwrap()
        };
        let first = transaction.stage_match(&registry, request()).unwrap();
        let second = transaction.stage_match(&registry, request()).unwrap();
        transaction.commit().await.unwrap_err();
        for pending in [first, second] {
            let Err(PendingError::InvalidPlan(diagnostic)) = pending.get() else {
                panic!("pending must carry the preflight refusal")
            };
            assert_eq!(diagnostic.code().as_str(), "staged_proxy_unsupported");
        }
    }
}
