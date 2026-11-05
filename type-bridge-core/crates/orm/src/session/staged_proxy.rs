//! The proxy route of a staged [`super::staged::Transaction`]: commit sends
//! the stack as one signed `query-remote/v3` exchange.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rand_core::{OsRng, RngCore};
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::limits::{MAX_REMOTE_ENVELOPE_BYTES, QueryLimits};
use type_bridge_contract::query_remote::RemoteCapabilities;
use type_bridge_contract::query_remote_v3::{
    StagedLimitsV3, StagedPlanV3, StagedRequestSpecV3, StagedRequestV3, VerifiedStagedReplyV3,
};
use type_bridge_contract::schema::DeclaredSchema;
use type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic;

use super::backend::{AnswerCancellation, BoxFuture};
use super::proxy_connection::ProxyHttpTransport;
use crate::query_execution_limits::{QueryExecutionDeadline, QueryExecutionResourceLimits};
use crate::remote_signing::Ed25519RemoteReplyVerifier;

/// Caller-owned exchange of one staged request for its exact reply bytes.
pub type StagedExchangeFn = Arc<
    dyn Fn(Vec<u8>) -> BoxFuture<'static, Result<Vec<u8>, SdkExecutionDiagnostic>> + Send + Sync,
>;

/// Caller-owned asynchronous transport for the staged proxy protocol.
/// Reply authentication and execution deadlines remain with the connection.
pub trait StagedTransport: Send + Sync + 'static {
    /// Exchange one `/v3/raw` request, retaining raw provider errors.
    /// Custom transports opt in; raw never falls back to the staged endpoint.
    fn exchange_raw<'a>(
        &'a self,
        _request: &'a [u8],
    ) -> BoxFuture<'a, Result<Vec<u8>, super::raw::RawQueryError>> {
        Box::pin(async { Err(super::raw::RawQueryError::unsupported()) })
    }

    /// Fetch the exact `/v3/capabilities` advertisement.
    fn staged_capabilities(&self) -> BoxFuture<'_, Result<Vec<u8>, crate::QueryTargetError>>;
    /// Exchange the exact `/v3/staged` request for its exact reply bytes.
    fn exchange_staged<'a>(
        &'a self,
        request: &'a [u8],
    ) -> BoxFuture<'a, Result<Vec<u8>, crate::QueryTargetError>>;
}

/// How long the built-in route waits past the commit deadline for the
/// server's own deadline reply.
const SEND_GRACE: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub(crate) enum Route {
    Http(ProxyHttpTransport),
    Exchange(StagedExchangeFn),
    Transport(Arc<dyn StagedTransport>),
}
impl Route {
    pub(crate) async fn capabilities(
        &self,
        deadline: QueryExecutionDeadline,
        cancellation: &AnswerCancellation,
    ) -> Result<Vec<u8>, crate::QueryTargetError> {
        match self {
            Self::Http(http) => http
                .staged_capabilities(deadline, cancellation)
                .await
                .map_err(Into::into),
            Self::Transport(transport) => {
                super::proxy_connection::bounded_exchange(deadline, cancellation, async {
                    transport.staged_capabilities().await
                })
                .await
            }
            Self::Exchange(_) => Err(SdkExecutionDiagnostic::provider_failure(
                type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Connect,
            )
            .into()),
        }
    }

    pub(crate) async fn exchange(
        &self,
        bytes: Cow<'_, [u8]>,
        deadline: QueryExecutionDeadline,
        cancellation: &AnswerCancellation,
    ) -> Result<Vec<u8>, crate::QueryTargetError> {
        match self {
            Self::Http(http) => http
                .exchange_staged(
                    bytes.into_owned(),
                    MAX_REMOTE_ENVELOPE_BYTES,
                    deadline,
                    cancellation,
                )
                .await
                .map_err(Into::into),
            Self::Exchange(exchange) => {
                super::proxy_connection::bounded_exchange(deadline, cancellation, async {
                    exchange(bytes.into_owned()).await.map_err(Into::into)
                })
                .await
            }
            Self::Transport(transport) => {
                super::proxy_connection::bounded_exchange(deadline, cancellation, async {
                    transport.exchange_staged(bytes.as_ref()).await
                })
                .await
            }
        }
    }
}

/// What a proxy-backed transaction needs to send and verify one commit.
pub(crate) struct ProxyStaging {
    route: Route,
    advertisement: RemoteCapabilities,
    declared: Arc<DeclaredSchema>,
    limits: QueryExecutionResourceLimits,
    admission_window_milliseconds: u64,
    query_limits: QueryLimits,
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
        Self::with_route(route, advertisement, Arc::new(declared), limits)
    }

    pub(crate) fn with_route(
        route: Route,
        advertisement: &[u8],
        declared: Arc<DeclaredSchema>,
        limits: QueryExecutionResourceLimits,
    ) -> Result<Self, Diagnostic> {
        Ok(Self {
            route,
            advertisement: RemoteCapabilities::decode(advertisement)?,
            declared,
            limits,
            admission_window_milliseconds:
                type_bridge_contract::query_remote::DEFAULT_ADMISSION_WINDOW_MILLISECONDS,
            query_limits: QueryLimits::V3,
        })
    }

    pub(crate) fn with_admission_window_milliseconds(
        mut self,
        milliseconds: std::num::NonZeroU64,
    ) -> Self {
        self.admission_window_milliseconds = milliseconds.get();
        self
    }

    pub(crate) fn with_query_limits(mut self, limits: QueryLimits) -> Self {
        self.query_limits = limits.min(self.advertisement.query_limits());
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
        StagedRequestV3::new_with_limits(
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
            self.query_limits,
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
    ) -> Result<Vec<u8>, crate::QueryTargetError> {
        if let Route::Exchange(exchange) = &self.route {
            return exchange(bytes).await.map_err(Into::into);
        }
        let cancellation = AnswerCancellation::default();
        let deadline = match timeout {
            Some(timeout) => QueryExecutionDeadline::from_timeout_milliseconds(
                u64::try_from(timeout.saturating_add(SEND_GRACE).as_millis()).unwrap_or(u64::MAX),
            ),
            None => QueryExecutionDeadline::for_limits(self.limits.effective()),
        };
        self.route
            .exchange(Cow::Owned(bytes), deadline, &cancellation)
            .await
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
        let signer = crate::remote_signing::RemoteReplySigningKey::from_secret_bytes([0x62; 32]);
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

#[cfg(test)]
mod query_policy_tests {
    use super::*;
    use type_bridge_contract::query_plan::QueryOperand;
    use type_bridge_contract::query_plan::v3::{Expr, PlanBuilder};
    use type_bridge_contract::query_remote::{RemoteExecutorBinding, RemoteSigningPublicKey};
    use type_bridge_contract::value::CanonicalValue;

    #[test]
    fn staged_requests_honor_raised_client_limits_and_tighter_server_limits() {
        let schema = crate::query_plan_exec::race::schema();
        let mut raised = QueryLimits::V3;
        raised.structural.bindings = 1024;
        raised.structural.boolean_terms = 512;
        let mut builder = PlanBuilder::with_limits(raised);
        for index in 0..300 {
            builder.let_(
                &format!("value_{index}"),
                Expr::Operand(QueryOperand::Literal {
                    value: CanonicalValue::Long(index),
                }),
            );
        }
        builder.count();
        let plan = builder
            .build(
                &schema,
                crate::query_plan_exec::race::touch()
                    .managed_semantics()
                    .clone(),
            )
            .unwrap();
        let entry = StagedPlanV3::new(&plan, vec![], &schema).unwrap();
        let mut caps = type_bridge_contract::query_plan::v3::query_plan_v3_capability_vocabulary();
        caps.insert(
            type_bridge_contract::capability::CapabilityId::new(
                type_bridge_contract::query_remote_v3::CAP_QUERY_REMOTE_STAGED_V3,
            )
            .unwrap(),
        );
        let advertisement = RemoteCapabilities::new(
            caps,
            RemoteExecutorBinding::new("staged-policy-tests", "epoch-00000000001").unwrap(),
            RemoteSigningPublicKey::from_bytes([8; 32]),
        )
        .with_query_limits(raised)
        .unwrap();
        let make_proxy = |advertisement: &RemoteCapabilities| {
            ProxyStaging::new(
                None,
                Some(Arc::new(|_| {
                    Box::pin(async { panic!("request preparation cannot exchange") })
                })),
                &advertisement.encode().unwrap(),
                schema.clone(),
                Default::default(),
            )
            .unwrap()
        };
        let proxy = make_proxy(&advertisement);
        assert!(
            proxy
                .request_with_policy(
                    vec![entry.clone()],
                    super::super::staged::TransactionKind::Read,
                    None,
                    None
                )
                .is_err()
        );
        let request = proxy
            .with_query_limits(raised)
            .request_with_policy(
                vec![entry.clone()],
                super::super::staged::TransactionKind::Read,
                None,
                None,
            )
            .unwrap();
        assert_eq!(request.plans()[0].plan().bindings().len(), 300);
        let tighter = advertisement.with_query_limits(QueryLimits::V3).unwrap();
        assert!(
            make_proxy(&tighter)
                .with_query_limits(raised)
                .request_with_policy(
                    vec![entry],
                    super::super::staged::TransactionKind::Read,
                    None,
                    None
                )
                .is_err()
        );
    }
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountCalls(Arc<AtomicUsize>);
    impl StagedTransport for CountCalls {
        fn staged_capabilities(&self) -> BoxFuture<'_, Result<Vec<u8>, crate::QueryTargetError>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(std::future::pending())
        }
        fn exchange_staged<'a>(
            &'a self,
            _: &'a [u8],
        ) -> BoxFuture<'a, Result<Vec<u8>, crate::QueryTargetError>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(std::future::pending())
        }
    }

    #[tokio::test]
    async fn cancelled_or_expired_routes_do_not_invoke_caller_transports() {
        let calls = Arc::new(AtomicUsize::new(0));
        let route = Route::Transport(Arc::new(CountCalls(Arc::clone(&calls))));
        for cancelled in [true, false] {
            let signal = AnswerCancellation::default();
            let deadline = if cancelled {
                signal.cancel();
                QueryExecutionDeadline::from_timeout_milliseconds(30_000)
            } else {
                QueryExecutionDeadline::expired()
            };
            let code = if cancelled {
                "provider_cancelled"
            } else {
                "transaction_deadline_exceeded"
            };
            assert_eq!(
                route
                    .capabilities(deadline, &signal)
                    .await
                    .unwrap_err()
                    .into_diagnostic(
                        type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Read
                    )
                    .code()
                    .as_str(),
                code
            );
            assert_eq!(
                route
                    .exchange(Cow::Borrowed(b"request"), deadline, &signal)
                    .await
                    .unwrap_err()
                    .into_diagnostic(
                        type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Read
                    )
                    .code()
                    .as_str(),
                code
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
