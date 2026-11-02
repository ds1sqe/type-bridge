//! Where one generated query terminal runs.
//!
//! A terminal runs on a direct database, a borrowed transaction, or a proxy
//! connection. The Rust SDK and every binding execute through
//! [`QueryTarget::execute`], so the connection's answer-limit ceiling and the
//! choice of transport are decided here once.

use std::sync::{Arc, Mutex};

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};

use type_bridge_contract::limits::{MAX_REMOTE_ENVELOPE_BYTES, QueryLimits, QueryLimitsError};
use type_bridge_contract::query_remote::RemoteCapabilities;
use type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic;

use crate::_registry::DescriptorRegistry;
use crate::error::OrmError;
use crate::execution_diagnostic::lower_remote_query_diagnostic;
use crate::match_request::{ValidatedMatchRequest, ValidatedMatchResult};
use crate::query_execution_limits::{QueryExecutionDeadline, QueryExecutionResourceLimits};
use crate::query_v2_model_remote::{
    PendingRemoteModelQueryV2, RemoteModelQueryV2Error, prepare_remote_model_query_with_policy,
};
use crate::query_v2_prepared::QueryAuthority;
use crate::session::backend::AnswerCancellation;
use crate::session::staged::{Transaction, TransactionOptions};
use crate::session::staged_proxy::ProxyStaging;
use crate::session::{
    Database, ProxyEndpoint, ProxyHttpTransport, StagedExchangeFn, TransactionContext,
};
use type_bridge_contract::sdk_diagnostic::SdkProviderOperation;

/// Why a terminal failed on its target.
#[derive(Debug, thiserror::Error)]
pub enum QueryTargetError {
    /// The direct executor refused or failed.
    #[error(transparent)]
    Direct(#[from] OrmError),
    /// Preparing the proxy request failed.
    #[error(transparent)]
    Prepare(RemoteModelQueryV2Error),
    /// Claiming, authenticating, or validating the proxy reply failed.
    #[error(transparent)]
    Reply(RemoteModelQueryV2Error),
    /// The proxy route failed, or the invocation was cancelled or ran out of
    /// time.
    #[error("{0:?}")]
    Interrupted(SdkExecutionDiagnostic),
}

impl From<SdkExecutionDiagnostic> for QueryTargetError {
    fn from(error: SdkExecutionDiagnostic) -> Self {
        Self::Interrupted(error)
    }
}

/// A connection to a TypeBridge server: its verified advertisement, the
/// schema authority requests are prepared under, the answer-limit ceiling,
/// and the built-in HTTP route unless the caller owns the exchange.
pub struct ProxyQueryConnection {
    http: Option<ProxyHttpTransport>,
    advertisement: Arc<[u8]>,
    authority: Arc<QueryAuthority>,
    answer_limits: QueryExecutionResourceLimits,
    admission_window_milliseconds: std::num::NonZeroU64,
    /// The staged (`/v3/capabilities`) advertisement, pinned or verified
    /// on first use against `advertisement`, then reused.
    staged_advertisement: Mutex<Option<StagedAdvertisement>>,
    requested_query_limits: Option<Arc<QueryLimits>>,
}

struct StagedAdvertisement {
    bytes: Arc<[u8]>,
    advertised_limits: QueryLimits,
    effective_limits: Option<Arc<QueryLimits>>,
}

impl StagedAdvertisement {
    fn new(
        bytes: Arc<[u8]>,
        advertised_limits: QueryLimits,
        requested: Option<&Arc<QueryLimits>>,
    ) -> Self {
        let mut value = Self {
            bytes,
            advertised_limits,
            effective_limits: None,
        };
        value.apply_policy(requested);
        value
    }

    fn apply_policy(&mut self, requested: Option<&Arc<QueryLimits>>) {
        let effective = requested
            .map_or(QueryLimits::V3, |limits| **limits)
            .min(self.advertised_limits);
        self.effective_limits = (effective != QueryLimits::V3).then(|| {
            requested
                .filter(|limits| ***limits == effective)
                .cloned()
                .unwrap_or_else(|| Arc::new(effective))
        });
    }
}

impl std::fmt::Debug for ProxyQueryConnection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProxyQueryConnection")
            .field("built_in_route", &self.http.is_some())
            .finish_non_exhaustive()
    }
}

impl ProxyQueryConnection {
    /// Open `endpoint` and fetch its advertisement, or use the pinned
    /// `advertisement`, then validate it.
    ///
    /// `connection_limits` bounds the fetch; `answer_limits` is the ceiling
    /// every terminal on this connection is constrained by.
    ///
    /// # Errors
    ///
    /// Returns the endpoint, transport, interruption, or advertisement
    /// diagnostic.
    pub async fn connect(
        endpoint: &ProxyEndpoint,
        advertisement: Option<Vec<u8>>,
        authority: Arc<QueryAuthority>,
        connection_limits: QueryExecutionResourceLimits,
        answer_limits: QueryExecutionResourceLimits,
        cancellation: &AnswerCancellation,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        let deadline = QueryExecutionDeadline::for_limits(connection_limits.effective());
        let http = ProxyHttpTransport::open(endpoint)?;
        let advertisement = match advertisement {
            Some(pinned) => pinned,
            None => http.staged_capabilities(deadline, cancellation).await?,
        };
        Self::new(Some(http), advertisement, authority, answer_limits)
    }

    /// A connection whose exchange the caller performs: terminals run through
    /// [`Self::prepare`] and [`Self::finish`] instead of
    /// [`QueryTarget::execute`].
    ///
    /// # Errors
    ///
    /// Returns the advertisement diagnostic.
    pub fn with_exchange(
        advertisement: Vec<u8>,
        authority: Arc<QueryAuthority>,
        answer_limits: QueryExecutionResourceLimits,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        Self::new(None, advertisement, authority, answer_limits)
    }

    /// A connection over an already opened route (or a caller-owned exchange
    /// when `http` is `None`) and an already fetched advertisement, which is
    /// validated here.
    ///
    /// # Errors
    ///
    /// Returns the advertisement diagnostic.
    pub fn new(
        http: Option<ProxyHttpTransport>,
        advertisement: Vec<u8>,
        authority: Arc<QueryAuthority>,
        answer_limits: QueryExecutionResourceLimits,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        let decoded =
            RemoteCapabilities::decode(&advertisement).map_err(lower_remote_query_diagnostic)?;
        let advertisement: Arc<[u8]> = advertisement.into();
        let staged = decoded
            .capabilities()
            .iter()
            .any(|capability| capability.as_str() == "query.plan.v3")
            .then(|| {
                StagedAdvertisement::new(Arc::clone(&advertisement), decoded.query_limits(), None)
            });
        Ok(Self {
            http,
            advertisement,
            authority,
            answer_limits: answer_limits.effective(),
            admission_window_milliseconds: std::num::NonZeroU64::new(
                type_bridge_contract::query_remote::DEFAULT_ADMISSION_WINDOW_MILLISECONDS,
            )
            .unwrap(),
            staged_advertisement: Mutex::new(staged),
            requested_query_limits: None,
        })
    }

    /// Configure the finite request delivery window (default 60000 ms).
    /// The advertised server ceiling may tighten it; execution is independent.
    #[must_use]
    pub const fn with_admission_window_milliseconds(
        mut self,
        milliseconds: std::num::NonZeroU64,
    ) -> Self {
        self.admission_window_milliseconds = milliseconds;
        self
    }

    /// The configured delivery window, before intersecting the server ceiling.
    #[must_use]
    pub const fn admission_window_milliseconds(&self) -> u64 {
        self.admission_window_milliseconds.get()
    }

    /// Pin the staged (`GET /v3/capabilities`) advertisement, provisioned
    /// out of band, so staged transactions never fetch it.
    ///
    /// # Errors
    ///
    /// Returns the decode failure, or
    /// `query_remote_v3_advertisement_mismatch` when its executor binding
    /// or reply key differs from this connection's advertisement.
    pub fn with_staged_advertisement(
        self,
        staged_advertisement: Vec<u8>,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        let limits = self.check_staged(&staged_advertisement)?;
        *self.staged_cache() = Some(StagedAdvertisement::new(
            staged_advertisement.into(),
            limits,
            self.requested_query_limits.as_ref(),
        ));
        Ok(self)
    }

    /// The staged advertisement pinned or verified on this connection.
    #[must_use]
    pub fn staged_advertisement(&self) -> Option<Arc<[u8]>> {
        self.staged_cache()
            .as_ref()
            .map(|value| Arc::clone(&value.bytes))
    }

    /// Select each query-limit dimension independently, constrained by the server.
    ///
    /// The staged advertisement supplies the receiver ceiling. The default path
    /// retains no extra allocation; custom policies are shared by manager plans.
    /// # Errors
    /// Returns an unsupported limit before any provider I/O.
    pub fn with_query_limits(mut self, limits: QueryLimits) -> Result<Self, QueryLimitsError> {
        let limits = limits.validate_v3()?;
        self.requested_query_limits = (limits != QueryLimits::V3).then(|| Arc::new(limits));
        if let Some(staged) = self.staged_cache().as_mut() {
            staged.apply_policy(self.requested_query_limits.as_ref());
        }
        Ok(self)
    }

    /// Query policy intersected with the pinned staged server ceiling.
    /// Before a staged advertisement is fetched, returns the requested policy.
    #[must_use]
    pub fn query_limits(&self) -> QueryLimits {
        self.shared_query_limits()
            .as_deref()
            .copied()
            .unwrap_or(QueryLimits::V3)
    }

    pub(crate) fn shared_query_limits(&self) -> Option<Arc<QueryLimits>> {
        match self.staged_cache().as_ref() {
            Some(staged) => staged.effective_limits.clone(),
            None => self.requested_query_limits.clone(),
        }
    }

    fn staged_cache(&self) -> std::sync::MutexGuard<'_, Option<StagedAdvertisement>> {
        self.staged_advertisement
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A staged reply is trusted only under the signer and executor this
    /// connection already trusts: a fetched staged advertisement cannot
    /// substitute its own reply key.
    fn check_staged(&self, staged: &[u8]) -> Result<QueryLimits, SdkExecutionDiagnostic> {
        let pinned = RemoteCapabilities::decode(&self.advertisement)
            .map_err(lower_remote_query_diagnostic)?;
        let staged = RemoteCapabilities::decode(staged).map_err(lower_remote_query_diagnostic)?;
        if staged.executor() != pinned.executor()
            || staged.reply_key() != pinned.reply_key()
            || staged.reply_key_id() != pinned.reply_key_id()
        {
            return Err(lower_remote_query_diagnostic(Diagnostic::new(
                DiagnosticCategory::InvalidContract,
                DiagnosticCode::new("query_remote_v3_advertisement_mismatch")
                    .expect("the static mismatch code is canonical"),
                "the staged advertisement's executor or reply key differs from the connection's",
            )));
        }
        Ok(staged.query_limits())
    }

    /// The ceiling every terminal on this connection is constrained by.
    #[must_use]
    pub const fn answer_limits(&self) -> QueryExecutionResourceLimits {
        self.answer_limits
    }

    /// Open a staged transaction whose commit is one signed
    /// `query-remote/v3` exchange through the built-in route. The staged
    /// advertisement is fetched from `/v3/capabilities` now; nothing else
    /// is sent until commit.
    ///
    /// # Errors
    ///
    /// Returns the provider failure when the connection has no built-in
    /// route or the advertisement cannot be fetched or decoded.
    pub async fn staged_transaction(
        &self,
        options: TransactionOptions,
        cancellation: &AnswerCancellation,
    ) -> Result<Transaction, SdkExecutionDiagnostic> {
        let http = self.http.clone().ok_or_else(|| {
            SdkExecutionDiagnostic::provider_failure(SdkProviderOperation::Connect)
        })?;
        let advertisement = self
            .ensure_staged_advertisement(
                QueryExecutionDeadline::for_limits(self.answer_limits),
                cancellation,
            )
            .await?;
        self.staged(Some(http), None, &advertisement, options)
    }

    async fn ensure_staged_advertisement(
        &self,
        deadline: QueryExecutionDeadline,
        cancellation: &AnswerCancellation,
    ) -> Result<Arc<[u8]>, SdkExecutionDiagnostic> {
        if let Some(advertisement) = self.staged_advertisement() {
            return Ok(advertisement);
        }
        let http = self.http.as_ref().ok_or_else(caller_owned_exchange)?;
        let fetched = http.staged_capabilities(deadline, cancellation).await?;
        let limits = self.check_staged(&fetched)?;
        let fetched: Arc<[u8]> = fetched.into();
        *self.staged_cache() = Some(StagedAdvertisement::new(
            Arc::clone(&fetched),
            limits,
            self.requested_query_limits.as_ref(),
        ));
        Ok(fetched)
    }

    /// Open a staged transaction over a caller-owned exchange: `exchange`
    /// sends the exact request bytes to `/v3/staged` and returns the exact
    /// reply bytes. `staged_advertisement` is the `/v3/capabilities` body;
    /// it must name this connection's executor and reply key.
    ///
    /// # Errors
    ///
    /// Returns the decode failure of the advertisement, or
    /// `query_remote_v3_advertisement_mismatch`.
    pub fn staged_transaction_with_exchange(
        &self,
        staged_advertisement: &[u8],
        exchange: StagedExchangeFn,
        options: TransactionOptions,
    ) -> Result<Transaction, SdkExecutionDiagnostic> {
        let limits = self.check_staged(staged_advertisement)?;
        {
            let mut cache = self.staged_cache();
            if cache
                .as_ref()
                .is_none_or(|cached| cached.bytes.as_ref() != staged_advertisement)
            {
                *cache = Some(StagedAdvertisement::new(
                    staged_advertisement.into(),
                    limits,
                    self.requested_query_limits.as_ref(),
                ));
            }
        }
        self.staged(None, Some(exchange), staged_advertisement, options)
    }

    fn staged(
        &self,
        http: Option<ProxyHttpTransport>,
        exchange: Option<StagedExchangeFn>,
        advertisement: &[u8],
        mut options: TransactionOptions,
    ) -> Result<Transaction, SdkExecutionDiagnostic> {
        options.collect_phase_timestamps |= self.answer_limits.collect_phase_timestamps;
        options.timeout = Some(
            options
                .timeout
                .unwrap_or(self.answer_limits.timeout_milliseconds)
                .constrained_by(self.answer_limits.timeout_milliseconds),
        );
        let proxy = ProxyStaging::new(
            http,
            exchange,
            advertisement,
            self.authority.declared().clone(),
            self.answer_limits,
        )
        .map_err(lower_remote_query_diagnostic)?
        .with_admission_window_milliseconds(self.admission_window_milliseconds);
        let proxy = match self.requested_query_limits.as_deref() {
            Some(limits) => proxy.with_query_limits(*limits),
            None => proxy,
        };
        Ok(Transaction::new_proxy(proxy, options))
    }

    /// Whether terminals reach the server through the built-in HTTP route.
    #[must_use]
    pub const fn has_built_in_route(&self) -> bool {
        self.http.is_some()
    }

    /// Prepare one request for a caller-owned exchange, with `resources`
    /// constrained by this connection's ceiling. Send
    /// [`PendingRemoteModelQueryV2::request_bytes`] and pass the reply to
    /// [`Self::finish`].
    ///
    /// # Errors
    ///
    /// Returns the interruption or preparation failure.
    pub fn prepare(
        &self,
        registry: &DescriptorRegistry,
        validated: ValidatedMatchRequest,
        resources: QueryExecutionResourceLimits,
        cancellation: &AnswerCancellation,
        deadline: QueryExecutionDeadline,
    ) -> Result<PendingRemoteModelQueryV2, QueryTargetError> {
        deadline.check(cancellation)?;
        let resources = resources.constrained_by(self.answer_limits);
        let advertisement = self.staged_advertisement().ok_or_else(|| {
            lower_remote_query_diagnostic(Diagnostic::new(
                DiagnosticCategory::UnsupportedCapability,
                DiagnosticCode::new("unsupported_required_capability").expect("static code"),
                "model queries require the pinned staged advertisement",
            ))
        })?;
        let pending = prepare_remote_model_query_with_policy(
            &self.authority,
            registry,
            validated,
            &advertisement,
            resources,
            deadline,
            cancellation,
            self.admission_window_milliseconds.get(),
        )
        .map_err(QueryTargetError::Prepare)?;
        deadline.check(cancellation)?;
        Ok(pending)
    }

    /// Authenticate and validate the reply to a [`Self::prepare`]d request.
    ///
    /// # Errors
    ///
    /// Returns the interruption, or the claim or reply failure.
    pub fn finish(
        pending: &PendingRemoteModelQueryV2,
        response: &[u8],
        cancellation: &AnswerCancellation,
        deadline: QueryExecutionDeadline,
    ) -> Result<(ValidatedMatchRequest, ValidatedMatchResult), QueryTargetError> {
        deadline.check(cancellation)?;
        let claimed = pending
            .claim_reply_with_cancellation(cancellation)
            .map_err(QueryTargetError::Reply)?;
        let (request, result, _registry) = claimed
            .decode_with_cancellation(response, cancellation)
            .map_err(QueryTargetError::Reply)?;
        deadline.check(cancellation)?;
        Ok((request, result))
    }
}

/// Where one terminal runs.
#[derive(Clone, Copy)]
pub enum QueryTarget<'a> {
    /// A direct database; the terminal opens its own read transaction.
    Database(&'a Database),
    /// A transaction the caller opened.
    Transaction(&'a TransactionContext),
    /// A TypeBridge server.
    Proxy(&'a ProxyQueryConnection),
}

impl QueryTarget<'_> {
    /// The answer-limit ceiling of the target's connection, if it has one.
    #[must_use]
    pub const fn answer_limits(&self) -> Option<QueryExecutionResourceLimits> {
        match self {
            Self::Database(database) => database.answer_limits(),
            Self::Transaction(transaction) => transaction.answer_limits(),
            Self::Proxy(proxy) => Some(proxy.answer_limits),
        }
    }

    /// `requested` constrained by the connection's ceiling: the policy a
    /// session on this target runs under. Sessions apply it when they are
    /// created, so their deadline already honours the ceiling.
    #[must_use]
    pub const fn constrain(
        &self,
        requested: QueryExecutionResourceLimits,
    ) -> QueryExecutionResourceLimits {
        match self.answer_limits() {
            Some(ceiling) => requested.constrained_by(ceiling),
            None => requested.effective(),
        }
    }

    /// The resources and deadline a terminal on this target runs under:
    /// `resources` constrained by the connection's ceiling, and the earlier
    /// of `deadline` and the ceiling's timeout. A binding that materializes
    /// the result itself uses the same budget.
    #[must_use]
    pub fn budget(
        &self,
        resources: QueryExecutionResourceLimits,
        deadline: QueryExecutionDeadline,
    ) -> (QueryExecutionResourceLimits, QueryExecutionDeadline) {
        match self.answer_limits() {
            Some(ceiling) => (
                resources.constrained_by(ceiling),
                deadline.earliest(QueryExecutionDeadline::for_limits(ceiling)),
            ),
            None => (resources.effective(), deadline),
        }
    }

    /// Run one validated request under `resources` and `deadline`, both of
    /// which this call constrains by the connection's ceiling again, so a
    /// caller cannot exceed it.
    ///
    /// # Errors
    ///
    /// Returns the direct, proxy, or interruption failure. A proxy connection
    /// without the built-in route refuses with
    /// `proxy_exchange_caller_owned`; use [`ProxyQueryConnection::prepare`].
    pub async fn execute(
        &self,
        registry: &DescriptorRegistry,
        validated: ValidatedMatchRequest,
        resources: QueryExecutionResourceLimits,
        cancellation: &AnswerCancellation,
        deadline: QueryExecutionDeadline,
    ) -> Result<(ValidatedMatchRequest, ValidatedMatchResult), QueryTargetError> {
        // A session constrained when it was created already runs under this
        // budget; one that was not still cannot exceed the ceiling.
        let (resources, deadline) = self.budget(resources, deadline);
        match self {
            Self::Database(database) => {
                let result = database
                    .execute_match_with_limits(
                        registry,
                        &validated,
                        resources.direct_with_deadline(cancellation.clone(), deadline),
                    )
                    .await?;
                Ok((validated, result))
            }
            Self::Transaction(transaction) => {
                let result = transaction
                    .execute_match_with_limits(
                        registry,
                        &validated,
                        resources.direct_with_deadline(cancellation.clone(), deadline),
                    )
                    .await?;
                Ok((validated, result))
            }
            Self::Proxy(proxy) => {
                let Some(http) = &proxy.http else {
                    return Err(caller_owned_exchange().into());
                };
                proxy
                    .ensure_staged_advertisement(deadline, cancellation)
                    .await?;
                let pending =
                    proxy.prepare(registry, validated, resources, cancellation, deadline)?;
                let response = http
                    .exchange_staged(
                        pending.request_bytes().to_vec(),
                        MAX_REMOTE_ENVELOPE_BYTES,
                        deadline,
                        cancellation,
                    )
                    .await?;
                ProxyQueryConnection::finish(&pending, &response, cancellation, deadline)
            }
        }
    }
}

fn caller_owned_exchange() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::invalid_input(
        type_bridge_contract::sdk_diagnostic::SdkDiagnosticCode::new("proxy_exchange_caller_owned")
            .expect("static proxy diagnostic code is canonical"),
        type_bridge_contract::sdk_diagnostic::SdkDiagnosticMessage::new(
            "This proxy connection's exchange is performed by the caller",
        )
        .expect("static proxy diagnostic message is valid"),
    )
}

#[cfg(test)]
mod policy_tests {
    use super::*;
    use type_bridge_contract::query_remote::{RemoteExecutorBinding, RemoteSigningPublicKey};

    fn advertisement(limits: QueryLimits) -> RemoteCapabilities {
        let mut caps = type_bridge_contract::query_plan::v3::query_plan_v3_capability_vocabulary();
        caps.insert(
            type_bridge_contract::capability::CapabilityId::new(
                type_bridge_contract::query_remote_v3::CAP_QUERY_REMOTE_STAGED_V3,
            )
            .unwrap(),
        );
        RemoteCapabilities::new(
            caps,
            RemoteExecutorBinding::new("proxy-query-policy", "epoch-00000000001").unwrap(),
            RemoteSigningPublicKey::from_bytes([7; 32]),
        )
        .with_query_limits(limits)
        .unwrap()
    }

    fn connection(advertisement: RemoteCapabilities) -> ProxyQueryConnection {
        let schema = crate::query_plan_exec::race::schema();
        let authority = QueryAuthority::from_declared_bytes(
            &type_bridge_contract::schema::encode_declared_schema(&schema).unwrap(),
            "proxy-policy-tests",
            "typedb-3.13.6/v1",
        )
        .unwrap();
        ProxyQueryConnection::with_exchange(
            advertisement.encode().unwrap(),
            Arc::new(authority),
            Default::default(),
        )
        .unwrap()
    }

    #[test]
    fn proxy_query_policy_intersects_every_dimension_and_shares_custom_values() {
        let default = connection(advertisement(QueryLimits::V3));
        assert_eq!(default.query_limits(), QueryLimits::V3);
        assert!(default.shared_query_limits().is_none());
        let mut server = QueryLimits::V3;
        server.structural.bindings = 768;
        server.structural.boolean_terms = 300;
        server.structural.predicate_depth = 16;
        server.codec.max_bytes /= 2;
        let mut client = QueryLimits::V3;
        client.structural.bindings = 1024;
        client.structural.boolean_terms = 512;
        client.structural.predicate_depth = 32;
        client.structural.input_rows /= 2;
        let connection = connection(advertisement(server))
            .with_query_limits(client)
            .unwrap();
        let effective = client.min(server);
        assert_eq!(connection.query_limits(), effective);
        for name in QueryLimits::FIELDS {
            assert_eq!(
                effective.get(name),
                Some(client.get(name).unwrap().min(server.get(name).unwrap()))
            );
        }
        let first = connection.shared_query_limits().unwrap();
        let second = connection.shared_query_limits().unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        let connection = connection.with_query_limits(server).unwrap();
        assert!(Arc::ptr_eq(
            connection.requested_query_limits.as_ref().unwrap(),
            &connection.shared_query_limits().unwrap()
        ));
    }

    #[test]
    fn staged_advertisement_recomputes_policy_and_invalid_values_are_refused() {
        let mut requested = QueryLimits::V3;
        requested.structural.bindings = 1024;
        requested.structural.boolean_terms = 512;
        let mut server = requested;
        server.structural.bindings = 512;
        server.structural.boolean_terms = 300;
        let connection = connection(advertisement(QueryLimits::V3))
            .with_query_limits(requested)
            .unwrap();
        assert_eq!(connection.query_limits(), QueryLimits::V3);
        let connection = connection
            .with_staged_advertisement(advertisement(server).encode().unwrap())
            .unwrap();
        assert_eq!(connection.query_limits(), requested.min(server));
        let mut invalid = requested;
        invalid.structural.bindings = 0;
        assert!(connection.with_query_limits(invalid).is_err());
    }
}
