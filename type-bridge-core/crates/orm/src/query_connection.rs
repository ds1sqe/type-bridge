//! Where one generated query terminal runs.
//!
//! A terminal runs on a direct database, a borrowed transaction, or a proxy
//! connection. The Rust SDK and every binding execute through
//! [`QueryTarget::execute`], so the connection's answer-limit ceiling and the
//! choice of transport are decided here once.

use std::sync::{Arc, Mutex};

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};

use type_bridge_contract::limits::MAX_REMOTE_ENVELOPE_BYTES;
use type_bridge_contract::query_remote::RemoteCapabilities;
use type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic;

use crate::_registry::DescriptorRegistry;
use crate::error::OrmError;
use crate::execution_diagnostic::lower_remote_query_diagnostic;
use crate::match_request::{ValidatedMatchRequest, ValidatedMatchResult};
use crate::query_execution_limits::{QueryExecutionDeadline, QueryExecutionResourceLimits};
use crate::query_v2_model_remote::{
    PendingRemoteModelQueryV2, RemoteModelQueryV2Error, prepare_remote_model_query_v2_with_budget,
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
    /// The staged (`/v3/capabilities`) advertisement, pinned or verified
    /// on first use against `advertisement`, then reused.
    staged_advertisement: Mutex<Option<Arc<[u8]>>>,
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
            None => http.capabilities(deadline, cancellation).await?,
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
        RemoteCapabilities::decode(&advertisement).map_err(lower_remote_query_diagnostic)?;
        Ok(Self {
            http,
            advertisement: advertisement.into(),
            authority,
            answer_limits: answer_limits.effective(),
            staged_advertisement: Mutex::new(None),
        })
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
        self.check_staged(&staged_advertisement)?;
        *self.staged_cache() = Some(staged_advertisement.into());
        Ok(self)
    }

    /// The staged advertisement pinned or verified on this connection.
    #[must_use]
    pub fn staged_advertisement(&self) -> Option<Arc<[u8]>> {
        self.staged_cache().clone()
    }

    fn staged_cache(&self) -> std::sync::MutexGuard<'_, Option<Arc<[u8]>>> {
        self.staged_advertisement
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A staged reply is trusted only under the signer and executor this
    /// connection already trusts: a fetched staged advertisement cannot
    /// substitute its own reply key.
    fn check_staged(&self, staged: &[u8]) -> Result<(), SdkExecutionDiagnostic> {
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
        Ok(())
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
        let advertisement = match self.staged_advertisement() {
            Some(advertisement) => advertisement,
            None => {
                let deadline = QueryExecutionDeadline::for_limits(self.answer_limits);
                let fetched = http.staged_capabilities(deadline, cancellation).await?;
                self.check_staged(&fetched)?;
                let fetched: Arc<[u8]> = fetched.into();
                *self.staged_cache() = Some(Arc::clone(&fetched));
                fetched
            }
        };
        self.staged(Some(http), None, &advertisement, options)
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
        self.check_staged(staged_advertisement)?;
        self.staged_cache()
            .get_or_insert_with(|| staged_advertisement.into());
        self.staged(None, Some(exchange), staged_advertisement, options)
    }

    fn staged(
        &self,
        http: Option<ProxyHttpTransport>,
        exchange: Option<StagedExchangeFn>,
        advertisement: &[u8],
        options: TransactionOptions,
    ) -> Result<Transaction, SdkExecutionDiagnostic> {
        let proxy = ProxyStaging::new(
            http,
            exchange,
            advertisement,
            self.authority.declared().clone(),
            self.answer_limits,
        )
        .map_err(lower_remote_query_diagnostic)?;
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
        let pending = prepare_remote_model_query_v2_with_budget(
            &self.authority,
            registry,
            validated,
            &self.advertisement,
            resources.remote(),
            deadline,
            cancellation,
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
                let pending =
                    proxy.prepare(registry, validated, resources, cancellation, deadline)?;
                let response = http
                    .exchange(
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
