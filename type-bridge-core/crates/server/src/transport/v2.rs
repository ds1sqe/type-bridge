//! Versioned V2 query endpoints over the remote plan envelope.
//!
//! V2 shares the configured V1 policy chain, rejects replay before opening a
//! provider transaction, and executes under a mandatory server deadline. The
//! typed request/result bytes remain owned by `type-bridge-contract`.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::to_bytes;
use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::query_remote::{
    RemoteCapabilities, RemoteCapabilitiesFingerprint, RemoteExecutorBinding, RemoteQueryFailure,
};
#[cfg(test)]
use type_bridge_contract::query_remote::{RemoteReply, decode_remote_reply};
use type_bridge_contract::query_remote_v2::query_remote_v2_required_capabilities;
use type_bridge_contract::schema::{DeclaredSchema, DocumentId};
use type_bridge_contract::schema_delta::ManagedSchemaState;
use type_bridge_orm::Transaction;
use type_bridge_orm::query_v2_prepared::{
    acquire_live_authority_rebuild_permit, verify_managed_query_control_scope,
};
#[cfg(test)]
use type_bridge_orm::query_v2_remote::Ed25519RemoteReplyVerifier;
use type_bridge_orm::query_v2_remote::{
    RemoteReplySigningKey, RemoteRequestFormat, execute_admitted_remote_request_versioned,
    preflight_remote_request_versioned, remote_request_format,
};
use type_bridge_orm::session::backend::{MAX_QUERY_V2_SCHEMA_FENCE_DURATION, QueryV2AnswerLimits};
use type_bridge_orm::session::database::Database;
use type_bridge_query::MigrationAssertionValidationContext;
use type_bridge_schema::{ManagedDeltaContext, ResolvedSchema};
use type_bridge_schema_migration_typedb::{LiveQueryControlPresence, rebuild_live_query_authority};

use crate::interceptor::{RequestContext, V2PolicyOutcome, V2PolicyRequest};
use crate::pipeline::QueryPipeline;

const DEFAULT_EXECUTION_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_REPLAY_CAPACITY: usize = 100_000;
const RESPONSE_AUDIT_GRACE: Duration = Duration::from_millis(100);
const TRANSACTION_CLOSE_GRACE: Duration = Duration::from_secs(1);
const V2_PREFLIGHT_CONCURRENCY: usize = 4;

fn new_v2_preflight_slots() -> Arc<tokio::sync::Semaphore> {
    Arc::new(tokio::sync::Semaphore::new(V2_PREFLIGHT_CONCURRENCY))
}

static V2_PREFLIGHT_SLOTS: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(new_v2_preflight_slots);

async fn await_before_deadline<F>(deadline: Instant, future: F) -> Option<F::Output>
where
    F: Future,
{
    tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), future)
        .await
        .ok()
}

async fn close_v2_transaction(transaction: &mut Transaction) -> Result<(), ()> {
    match tokio::time::timeout(TRANSACTION_CLOSE_GRACE, transaction.close()).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) | Err(_) => Err(()),
    }
}

/// Failure returned by an atomic nonce reservation backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplayStoreError {
    /// The executor epoch has already consumed this nonce.
    Replayed,
    /// The bounded store cannot admit another live reservation.
    Capacity,
    /// The store cannot prove whether the nonce was reserved.
    Unavailable,
}

/// Atomic one-shot nonce registry.
///
/// Multi-instance deployments can inject a shared implementation whose
/// reservation operation is globally atomic, but must inject the matching
/// shared executor epoch through [`V2QueryState::with_shared_execution_epoch`]
/// at the same time. The built-in implementation is process-local and bounded,
/// suitable for the standalone single-node server.
pub trait RemoteReplayStore: Send + Sync {
    /// Atomically reserve `(executor_epoch, nonce)` until `retain_until`.
    ///
    /// Authenticated client identity is deliberately not part of replay
    /// ownership: the same captured request must remain one-shot even when it
    /// is resent under different valid credentials.
    fn reserve<'a>(
        &'a self,
        executor_epoch: &'a str,
        nonce: &'a str,
        retain_until: Instant,
    ) -> Pin<Box<dyn Future<Output = Result<(), ReplayStoreError>> + Send + 'a>>;
}

/// Bounded process-local replay registry used by the standalone server.
pub struct InMemoryReplayStore {
    capacity: usize,
    reservations: Mutex<ReplayReservations>,
}

type ReplayKey = (String, String);

#[derive(Default)]
struct ReplayReservations {
    by_key: HashMap<ReplayKey, Instant>,
    by_expiry: BinaryHeap<Reverse<(Instant, ReplayKey)>>,
}

impl ReplayReservations {
    fn discard_expired(&mut self, now: Instant) {
        while let Some(Reverse((expiry, _))) = self.by_expiry.peek() {
            if *expiry > now {
                break;
            }
            let Some(Reverse((expiry, key))) = self.by_expiry.pop() else {
                break;
            };
            if self
                .by_key
                .get(&key)
                .is_some_and(|stored| *stored == expiry)
            {
                self.by_key.remove(&key);
            }
        }
    }
}

impl InMemoryReplayStore {
    /// Create a registry with an explicit maximum live reservation count.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            reservations: Mutex::new(ReplayReservations::default()),
        }
    }
}

impl RemoteReplayStore for InMemoryReplayStore {
    fn reserve<'a>(
        &'a self,
        executor_epoch: &'a str,
        nonce: &'a str,
        retain_until: Instant,
    ) -> Pin<Box<dyn Future<Output = Result<(), ReplayStoreError>> + Send + 'a>> {
        Box::pin(async move {
            let mut reservations = self
                .reservations
                .lock()
                .map_err(|_| ReplayStoreError::Unavailable)?;
            let now = Instant::now();
            reservations.discard_expired(now);
            let key = (executor_epoch.to_owned(), nonce.to_owned());
            if reservations.by_key.contains_key(&key) {
                return Err(ReplayStoreError::Replayed);
            }
            if reservations.by_key.len() >= self.capacity {
                return Err(ReplayStoreError::Capacity);
            }
            reservations.by_key.insert(key.clone(), retain_until);
            reservations.by_expiry.push(Reverse((retain_until, key)));
            Ok(())
        })
    }
}

/// Everything one V2 query executor needs to serve envelopes.
pub struct V2QueryState {
    advertisement: RemoteCapabilities,
    advertisement_fingerprint: RemoteCapabilitiesFingerprint,
    ceilings: QueryV2AnswerLimits,
    database: Database,
    declared: DeclaredSchema,
    delta_context: ManagedDeltaContext,
    managed: ManagedSchemaState,
    resolved: ResolvedSchema,
    execution_timeout: Duration,
    replay_store: Arc<dyn RemoteReplayStore>,
    signer: RemoteReplySigningKey,
    control_policy: V2QueryControlPolicy,
}

#[derive(Clone, Copy)]
enum V2QueryControlPolicy {
    Managed,
    QueryOnly,
}

impl V2QueryState {
    /// Construct fail-closed V2 state with mandatory execution/replay bounds.
    pub fn new(
        advertised: CapabilitySet,
        ceilings: QueryV2AnswerLimits,
        database: Database,
        declared: DeclaredSchema,
        delta_context: ManagedDeltaContext,
        managed: ManagedSchemaState,
        resolved: ResolvedSchema,
    ) -> Result<Self, Diagnostic> {
        Self::new_with_control_policy(
            advertised,
            ceilings,
            database,
            declared,
            delta_context,
            managed,
            resolved,
            V2QueryControlPolicy::Managed,
        )
    }

    /// Construct fail-closed V2 state for an explicitly query-only database.
    ///
    /// This mode rejects both V2 and legacy migration-control partitions. It
    /// cannot be inferred from missing control facts on a managed database.
    pub fn new_query_only(
        advertised: CapabilitySet,
        ceilings: QueryV2AnswerLimits,
        database: Database,
        declared: DeclaredSchema,
        delta_context: ManagedDeltaContext,
        managed: ManagedSchemaState,
        resolved: ResolvedSchema,
    ) -> Result<Self, Diagnostic> {
        Self::new_with_control_policy(
            advertised,
            ceilings,
            database,
            declared,
            delta_context,
            managed,
            resolved,
            V2QueryControlPolicy::QueryOnly,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_control_policy(
        mut advertised: CapabilitySet,
        ceilings: QueryV2AnswerLimits,
        database: Database,
        declared: DeclaredSchema,
        delta_context: ManagedDeltaContext,
        managed: ManagedSchemaState,
        resolved: ResolvedSchema,
        control_policy: V2QueryControlPolicy,
    ) -> Result<Self, Diagnostic> {
        // Model evidence is constructed by the sole same-snapshot
        // compatibility executor. Its graph, attribute-value, and role-player
        // ceilings are fixed at the canonical collection ceiling; request
        // values may only tighten them during admission.
        for capability in query_remote_v2_required_capabilities(false) {
            advertised.insert(capability);
        }
        let executor = standalone_executor_binding();
        let signer = RemoteReplySigningKey::generate();
        let advertisement = RemoteCapabilities::new(advertised, executor, signer.public_key());
        let advertisement_fingerprint = advertisement.fingerprint()?;
        Ok(Self {
            advertisement,
            advertisement_fingerprint,
            ceilings,
            database,
            declared,
            delta_context,
            managed,
            resolved,
            execution_timeout: DEFAULT_EXECUTION_TIMEOUT,
            replay_store: Arc::new(InMemoryReplayStore::new(DEFAULT_REPLAY_CAPACITY)),
            signer,
            control_policy,
        })
    }

    /// Replace the replay registry without changing this standalone epoch.
    ///
    /// This does not enable load balancing because peer states still advertise
    /// different epochs. Multi-instance callers must use
    /// [`Self::with_shared_execution_epoch`] instead.
    #[must_use]
    pub fn with_replay_store(mut self, replay_store: Arc<dyn RemoteReplayStore>) -> Self {
        self.replay_store = replay_store;
        self
    }

    /// Configure one shared executor epoch together with its atomic replay store.
    ///
    /// This is the only API that lets multiple state instances advertise the
    /// same identity/epoch. Supplying the epoch and replay authority together
    /// prevents a load-balanced deployment from accidentally claiming shared
    /// replay semantics while retaining process-local nonce state.
    pub fn with_shared_execution_epoch(
        mut self,
        advertisement: RemoteCapabilities,
        replay_store: Arc<dyn RemoteReplayStore>,
        signer: RemoteReplySigningKey,
    ) -> Result<Self, Diagnostic> {
        if advertisement.capabilities() != self.advertisement.capabilities()
            || advertisement.reply_key() != signer.public_key()
        {
            return Err(diagnostic(
                DiagnosticCategory::Integrity,
                "query_remote_signer_mismatch",
                "shared executor advertisement does not bind the supplied reply signer",
            ));
        }
        let advertisement_fingerprint = advertisement.fingerprint()?;
        self.advertisement = advertisement;
        self.advertisement_fingerprint = advertisement_fingerprint;
        self.replay_store = replay_store;
        self.signer = signer;
        Ok(self)
    }

    /// Tighten the mandatory server execution duration.
    #[must_use]
    pub fn with_execution_timeout(mut self, timeout: Duration) -> Self {
        self.execution_timeout = timeout;
        self
    }

    fn signed_failure(&self, failure: &RemoteQueryFailure) -> Vec<u8> {
        failure.encode_signed_or_fallback(&self.advertisement_fingerprint, &self.signer)
    }

    fn unbound_failure(&self, diagnostic: &Diagnostic) -> Vec<u8> {
        self.signed_failure(&RemoteQueryFailure::new(None, diagnostic))
    }

    fn unbound_failure_for(&self, format: RemoteRequestFormat, diagnostic: &Diagnostic) -> Vec<u8> {
        match format {
            RemoteRequestFormat::V1 => self.unbound_failure(diagnostic),
            RemoteRequestFormat::V2 => {
                type_bridge_orm::query_v2_remote::encode_unbound_remote_failure_v2(
                    None,
                    diagnostic,
                    &self.advertisement_fingerprint,
                    &self.signer,
                )
            }
        }
    }

    async fn verify_live_authority(&self) -> Result<(), Diagnostic> {
        self.verify_database_profile()?;
        let rebuild_permit = acquire_live_authority_rebuild_permit().await.map_err(|_| {
            diagnostic(
                DiagnosticCategory::Integrity,
                "query_remote_live_schema_unavailable",
                "the executor could not reserve live-schema verification capacity",
            )
        })?;
        let export = self.database.schema_text().await.map_err(|_| {
            diagnostic(
                DiagnosticCategory::Integrity,
                "query_remote_live_schema_unavailable",
                "the executor could not verify its live schema authority",
            )
        })?;
        self.verify_live_authority_export_with_permit(export, rebuild_permit)
            .await
    }

    fn verify_database_profile(&self) -> Result<(), Diagnostic> {
        let server_version = self.database.server_version().ok_or_else(|| {
            diagnostic(
                DiagnosticCategory::Integrity,
                "query_remote_server_identity_unavailable",
                "the executor cannot prove the exact TypeDB semantic profile",
            )
        })?;
        let observed_profile = type_bridge_contract::fingerprint::SemanticProfileId::new(format!(
            "typedb-{server_version}/v1"
        ))?;
        if &observed_profile != self.delta_context.semantic_profile() {
            return Err(diagnostic(
                DiagnosticCategory::Integrity,
                "query_remote_semantic_profile_mismatch",
                "the executor TypeDB semantic profile differs from its declared authority",
            ));
        }
        Ok(())
    }

    async fn verify_live_authority_export_with_permit(
        &self,
        export: String,
        rebuild_permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Result<(), Diagnostic> {
        let document = DocumentId::new("typebridge-v2-request-live-export.typeql")?;
        let declared = self.declared.clone();
        let context = self.delta_context.clone();
        let live = tokio::task::spawn_blocking(move || {
            let _rebuild_permit = rebuild_permit;
            rebuild_live_query_authority(document, &export, &declared, &context)
        })
        .await
        .map_err(|_| {
            diagnostic(
                DiagnosticCategory::Integrity,
                "query_remote_live_schema_invalid",
                "the executor live schema cannot form trusted query authority",
            )
        })?
        .map_err(|_| {
            diagnostic(
                DiagnosticCategory::Integrity,
                "query_remote_live_schema_invalid",
                "the executor live schema cannot form trusted query authority",
            )
        })?;
        if live.managed() != &self.managed {
            return Err(diagnostic(
                DiagnosticCategory::Integrity,
                "query_remote_stale_schema",
                "the executor live schema no longer matches its declared authority",
            ));
        }
        match (self.control_policy, live.control_presence()) {
            (V2QueryControlPolicy::Managed, LiveQueryControlPresence::ManagedFence)
            | (V2QueryControlPolicy::QueryOnly, LiveQueryControlPresence::Absent) => {}
            (V2QueryControlPolicy::Managed, LiveQueryControlPresence::Absent) => {
                return Err(diagnostic(
                    DiagnosticCategory::Integrity,
                    "query_remote_managed_control_missing",
                    "the managed executor database has no migration control authority",
                ));
            }
            (V2QueryControlPolicy::QueryOnly, LiveQueryControlPresence::ManagedFence) => {
                return Err(diagnostic(
                    DiagnosticCategory::Integrity,
                    "query_remote_query_only_control_present",
                    "the query-only executor database unexpectedly has managed control authority",
                ));
            }
            (_, LiveQueryControlPresence::ManagedFenceWithExtensions) => {
                return Err(diagnostic(
                    DiagnosticCategory::Integrity,
                    "query_remote_live_schema_invalid",
                    "the executor managed control schema carries unsupported released-only extensions",
                ));
            }
        }
        if matches!(self.control_policy, V2QueryControlPolicy::QueryOnly)
            && live.legacy_control_present()
        {
            return Err(diagnostic(
                DiagnosticCategory::Integrity,
                "query_remote_query_only_legacy_control_present",
                "the query-only executor database unexpectedly has legacy migration control authority",
            ));
        }
        Ok(())
    }

    /// Prove the configured authority under one bounded schema fence before
    /// exposing this state through a standalone server.
    pub async fn verify_startup_authority(&self) -> Result<(), Diagnostic> {
        let deadline = Instant::now()
            .checked_add(DEFAULT_EXECUTION_TIMEOUT)
            .ok_or_else(|| {
                diagnostic(
                    DiagnosticCategory::ResourceLimit,
                    "transaction_deadline_exceeded",
                    "provider transaction deadline expired",
                )
            })?;
        self.verify_authority_before(deadline).await
    }

    async fn verify_authority_before(&self, deadline: Instant) -> Result<(), Diagnostic> {
        match await_before_deadline(deadline, self.verify_live_authority()).await {
            Some(result) => result?,
            None => {
                return Err(diagnostic(
                    DiagnosticCategory::ResourceLimit,
                    "transaction_deadline_exceeded",
                    "provider transaction deadline expired",
                ));
            }
        }
        let rebuild_permit =
            await_before_deadline(deadline, acquire_live_authority_rebuild_permit())
                .await
                .ok_or_else(|| {
                    diagnostic(
                        DiagnosticCategory::ResourceLimit,
                        "transaction_deadline_exceeded",
                        "provider transaction deadline expired",
                    )
                })?
                .map_err(|_| {
                    diagnostic(
                        DiagnosticCategory::Integrity,
                        "query_remote_live_schema_unavailable",
                        "the executor could not reserve live-schema verification capacity",
                    )
                })?;
        let fence_timeout = deadline
            .saturating_duration_since(Instant::now())
            .min(MAX_QUERY_V2_SCHEMA_FENCE_DURATION);
        if fence_timeout.is_zero() {
            return Err(diagnostic(
                DiagnosticCategory::ResourceLimit,
                "transaction_deadline_exceeded",
                "provider transaction deadline expired",
            ));
        }
        let open = self.database.schema_fenced_read_transaction(fence_timeout);
        let (mut transaction, fenced_schema) = await_before_deadline(deadline, open)
            .await
            .ok_or_else(|| {
                diagnostic(
                    DiagnosticCategory::ResourceLimit,
                    "transaction_deadline_exceeded",
                    "provider transaction deadline expired",
                )
            })?
            .map_err(|_| {
                diagnostic(
                    DiagnosticCategory::Integrity,
                    "query_remote_provider_unavailable",
                    "the executor could not open a schema-fenced startup transaction",
                )
            })?;
        let verified = async {
            await_before_deadline(
                deadline,
                self.verify_live_authority_export_with_permit(fenced_schema, rebuild_permit),
            )
            .await
            .ok_or_else(|| {
                diagnostic(
                    DiagnosticCategory::ResourceLimit,
                    "transaction_deadline_exceeded",
                    "provider transaction deadline expired",
                )
            })??;
            if matches!(self.control_policy, V2QueryControlPolicy::Managed) {
                verify_managed_query_control_scope(
                    &mut transaction,
                    self.delta_context.scope_id(),
                    Some(deadline),
                    &self.ceilings.answer.cancellation,
                )
                .await?;
            }
            Ok::<(), Diagnostic>(())
        }
        .await;
        let closed = close_v2_transaction(&mut transaction).await;
        verified?;
        if closed.is_err() {
            return Err(diagnostic(
                DiagnosticCategory::Integrity,
                "query_remote_transaction_close_failed",
                "the executor could not close its startup authority transaction",
            ));
        }
        Ok(())
    }
}

fn standalone_executor_binding() -> RemoteExecutorBinding {
    let identity = format!("standalone-{}", uuid::Uuid::new_v4().simple());
    let epoch = uuid::Uuid::new_v4().simple().to_string();
    RemoteExecutorBinding::new(identity, epoch)
        .expect("generated standalone executor identity and epoch are canonical")
}

struct V2RouterState {
    pipeline: Arc<QueryPipeline>,
    query: Arc<V2QueryState>,
}

/// The remote envelope has its own 32 MiB owning-format budget plus framing
/// slack for the HTTP body collector.
const V2_BODY_LIMIT_BYTES: usize =
    type_bridge_contract::limits::MAX_REMOTE_ENVELOPE_BYTES + 64 * 1024;

fn declared_v2_body_oversized(headers: &HeaderMap) -> bool {
    let Ok(limit) = u64::try_from(V2_BODY_LIMIT_BYTES) else {
        return false;
    };
    headers.get_all(CONTENT_LENGTH).iter().any(|value| {
        value
            .to_str()
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|declared| declared > limit)
    })
}

/// Build the V2 route surface under the same policy pipeline as V1.
pub fn create_v2_router(pipeline: Arc<QueryPipeline>, query: Arc<V2QueryState>) -> Router {
    let state = Arc::new(V2RouterState { pipeline, query });
    Router::new()
        .route("/v2/query", post(handle_v2_query))
        .route("/v2/capabilities", get(handle_v2_capabilities))
        .with_state(state)
}

/// Build the complete router: retained V1 routes beside the V2 surface.
pub fn create_router_with_v2(pipeline: Arc<QueryPipeline>, v2: Arc<V2QueryState>) -> Router {
    super::http::create_router(Arc::clone(&pipeline)).merge(create_v2_router(pipeline, v2))
}

async fn handle_v2_query(State(state): State<Arc<V2RouterState>>, request: Request) -> Response {
    let mut ceilings = state.query.ceilings.clone();
    let mandatory_deadline = match Instant::now().checked_add(state.query.execution_timeout) {
        Some(deadline) => deadline,
        None => {
            return envelope_response(state.query.unbound_failure(&deadline_exceeded()));
        }
    };
    ceilings.answer.deadline = Some(
        ceilings
            .answer
            .deadline
            .map_or(mandatory_deadline, |deadline| {
                deadline.min(mandatory_deadline)
            }),
    );

    // Authenticate/rate-limit from request parts before buffering or decoding
    // attacker-controlled envelope bytes. Legacy-AST-only policies fail
    // closed instead of receiving a misleading empty query.
    let metadata = v2_request_metadata(request.headers(), "query");
    let mut policy_context = match state.pipeline.v2_request_context(metadata) {
        Ok(context) => context,
        Err(_) => {
            return envelope_response(state.query.unbound_failure(&policy_rejected()));
        }
    };
    match await_before_deadline(
        mandatory_deadline,
        state.pipeline.begin_v2_request(&mut policy_context),
    )
    .await
    {
        Some(Ok(())) => {}
        Some(Err(_)) => {
            let failure = state.query.unbound_failure(&policy_rejected());
            return finish_v2_failure(
                &state,
                mandatory_deadline,
                &policy_context,
                "query_remote_policy_rejected",
                failure.clone(),
                failure,
                state.query.unbound_failure(&deadline_exceeded()),
            )
            .await;
        }
        None => {
            let deadline_failure = state.query.unbound_failure(&deadline_exceeded());
            return finish_v2_failure(
                &state,
                mandatory_deadline,
                &policy_context,
                "transaction_deadline_exceeded",
                deadline_failure.clone(),
                state.query.unbound_failure(&policy_rejected()),
                deadline_failure,
            )
            .await;
        }
    }

    if declared_v2_body_oversized(request.headers()) {
        let failure = state.query.unbound_failure(&body_oversized());
        return finish_v2_failure(
            &state,
            mandatory_deadline,
            &policy_context,
            "query_remote_envelope_oversized",
            failure.clone(),
            state.query.unbound_failure(&policy_rejected()),
            state.query.unbound_failure(&deadline_exceeded()),
        )
        .await;
    }

    // One owned admission covers both attacker-controlled body buffering and
    // provider-independent blocking work. If any async stage returns early,
    // the local permit drops; once moved, the blocking task owns its release.
    let permit = match await_before_deadline(
        mandatory_deadline,
        Arc::clone(&V2_PREFLIGHT_SLOTS).acquire_owned(),
    )
    .await
    {
        Some(Ok(permit)) => permit,
        Some(Err(_)) => {
            return finish_v2_failure(
                &state,
                mandatory_deadline,
                &policy_context,
                "query_remote_preflight_unavailable",
                state.query.unbound_failure(&unavailable()),
                state.query.unbound_failure(&policy_rejected()),
                state.query.unbound_failure(&deadline_exceeded()),
            )
            .await;
        }
        None => {
            let failure = state.query.unbound_failure(&deadline_exceeded());
            return finish_v2_failure(
                &state,
                mandatory_deadline,
                &policy_context,
                "transaction_deadline_exceeded",
                failure.clone(),
                state.query.unbound_failure(&policy_rejected()),
                failure,
            )
            .await;
        }
    };

    let body = match await_before_deadline(
        mandatory_deadline,
        to_bytes(request.into_body(), V2_BODY_LIMIT_BYTES),
    )
    .await
    {
        Some(Ok(body)) => body,
        Some(Err(_)) => {
            let failure = state.query.unbound_failure(&body_oversized());
            return finish_v2_failure(
                &state,
                mandatory_deadline,
                &policy_context,
                "query_remote_envelope_oversized",
                failure.clone(),
                state.query.unbound_failure(&policy_rejected()),
                state.query.unbound_failure(&deadline_exceeded()),
            )
            .await;
        }
        None => {
            let deadline_failure = state.query.unbound_failure(&deadline_exceeded());
            return finish_v2_failure(
                &state,
                mandatory_deadline,
                &policy_context,
                "transaction_deadline_exceeded",
                deadline_failure.clone(),
                state.query.unbound_failure(&policy_rejected()),
                deadline_failure,
            )
            .await;
        }
    };
    // Select the additive payload version from the explicit discriminator
    // before either decoder reconstructs a plan or invocation. Unknown and
    // malformed formats intentionally retain the historical V1 rejection
    // path.
    let request_format = remote_request_format(&body);

    // Contract/schema/capability work is provider-independent and runs in a
    // bounded blocking pool. The owned permit remains in the blocking task if
    // the async request deadline elapses, preventing detached CPU saturation.
    let resolved = state.query.resolved.clone();
    let managed = state.query.managed.clone();
    let advertisement = state.query.advertisement.clone();
    let preflight = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let context = MigrationAssertionValidationContext::new(&resolved, &managed);
        preflight_remote_request_versioned(&body, &context, &advertisement, ceilings)
            .map_err(Box::new)
    });
    let admitted = match await_before_deadline(mandatory_deadline, preflight).await {
        Some(Ok(Ok(admitted))) => admitted,
        Some(Ok(Err(rejection))) => {
            let rejection = *rejection;
            let code = rejection.diagnostic_code().to_owned();
            let failure = rejection
                .into_failure_envelope(&state.query.advertisement_fingerprint, &state.query.signer);
            return finish_v2_failure(
                &state,
                mandatory_deadline,
                &policy_context,
                &code,
                failure.clone(),
                state
                    .query
                    .unbound_failure_for(request_format, &policy_rejected()),
                failure,
            )
            .await;
        }
        Some(Err(_)) => {
            return finish_v2_failure(
                &state,
                mandatory_deadline,
                &policy_context,
                "query_remote_preflight_unavailable",
                state
                    .query
                    .unbound_failure_for(request_format, &unavailable()),
                state
                    .query
                    .unbound_failure_for(request_format, &policy_rejected()),
                state
                    .query
                    .unbound_failure_for(request_format, &deadline_exceeded()),
            )
            .await;
        }
        None => {
            let failure = state
                .query
                .unbound_failure_for(request_format, &deadline_exceeded());
            return finish_v2_failure(
                &state,
                mandatory_deadline,
                &policy_context,
                "transaction_deadline_exceeded",
                failure.clone(),
                state
                    .query
                    .unbound_failure_for(request_format, &policy_rejected()),
                failure,
            )
            .await;
        }
    };
    let deadline = admitted.deadline().unwrap_or(mandatory_deadline);
    let deadline_failure = admitted.bound_failure(&deadline_exceeded(), &state.query.signer);
    let response_policy_failure = admitted.bound_failure(&policy_rejected(), &state.query.signer);

    // Preserve released V1 precedence and policy observability byte-for-byte:
    // V1 plan authorization historically ran before replay reservation.
    if request_format == RemoteRequestFormat::V1 {
        let policy_request = V2PolicyRequest::new(admitted.plan(), admitted.invocation());
        match await_before_deadline(
            deadline,
            state
                .pipeline
                .authorize_v2_request(&policy_request, &mut policy_context),
        )
        .await
        {
            Some(Ok(())) => {}
            Some(Err(_)) => {
                return finish_v2_failure(
                    &state,
                    deadline,
                    &policy_context,
                    "query_remote_policy_rejected",
                    response_policy_failure.clone(),
                    response_policy_failure,
                    deadline_failure,
                )
                .await;
            }
            None => {
                return finish_v2_failure(
                    &state,
                    deadline,
                    &policy_context,
                    "transaction_deadline_exceeded",
                    deadline_failure.clone(),
                    response_policy_failure,
                    deadline_failure,
                )
                .await;
            }
        }
    }

    // V2 reserves the request nonce before plan-level policy callbacks.
    // Transport authentication already ran before the body was collected, but
    // a replay must not invoke request-specific application hooks or construct
    // any live schema/provider/model host state.
    let retain_until = admitted.replay_until();
    match await_before_deadline(
        deadline,
        state.query.replay_store.reserve(
            state.query.advertisement.executor().epoch(),
            admitted.nonce(),
            retain_until,
        ),
    )
    .await
    {
        Some(Ok(())) => {}
        Some(Err(error)) => {
            let diagnostic = match error {
                ReplayStoreError::Replayed => replayed(),
                ReplayStoreError::Capacity => replay_capacity(),
                ReplayStoreError::Unavailable => replay_unavailable(),
            };
            let code = diagnostic.code().as_str().to_owned();
            return finish_v2_failure(
                &state,
                deadline,
                &policy_context,
                &code,
                admitted.bound_failure(&diagnostic, &state.query.signer),
                response_policy_failure,
                deadline_failure,
            )
            .await;
        }
        None => {
            return finish_v2_failure(
                &state,
                deadline,
                &policy_context,
                "transaction_deadline_exceeded",
                deadline_failure.clone(),
                response_policy_failure,
                deadline_failure,
            )
            .await;
        }
    }

    if request_format == RemoteRequestFormat::V2 {
        let policy_request = V2PolicyRequest::new(admitted.plan(), admitted.invocation());
        match await_before_deadline(
            deadline,
            state
                .pipeline
                .authorize_v2_request(&policy_request, &mut policy_context),
        )
        .await
        {
            Some(Ok(())) => {}
            Some(Err(_)) => {
                return finish_v2_failure(
                    &state,
                    deadline,
                    &policy_context,
                    "query_remote_policy_rejected",
                    response_policy_failure.clone(),
                    response_policy_failure,
                    deadline_failure,
                )
                .await;
            }
            None => {
                return finish_v2_failure(
                    &state,
                    deadline,
                    &policy_context,
                    "transaction_deadline_exceeded",
                    deadline_failure.clone(),
                    response_policy_failure,
                    deadline_failure,
                )
                .await;
            }
        }
    }

    // A unique, authenticated request may force one bounded schema export, but
    // never a provider transaction unless current live authority has been
    // proved.
    match await_before_deadline(deadline, state.query.verify_live_authority()).await {
        Some(Ok(())) => {}
        Some(Err(diagnostic)) => {
            let code = diagnostic.code().as_str().to_owned();
            return finish_v2_failure(
                &state,
                deadline,
                &policy_context,
                &code,
                admitted.bound_failure(&diagnostic, &state.query.signer),
                response_policy_failure,
                deadline_failure,
            )
            .await;
        }
        None => {
            return finish_v2_failure(
                &state,
                deadline,
                &policy_context,
                "transaction_deadline_exceeded",
                deadline_failure.clone(),
                response_policy_failure,
                deadline_failure,
            )
            .await;
        }
    }

    let rebuild_permit =
        match await_before_deadline(deadline, acquire_live_authority_rebuild_permit()).await {
            Some(Ok(permit)) => permit,
            Some(Err(_)) => {
                let diagnostic = diagnostic(
                    DiagnosticCategory::Integrity,
                    "query_remote_live_schema_unavailable",
                    "the executor could not reserve live-schema verification capacity",
                );
                let code = diagnostic.code().as_str().to_owned();
                return finish_v2_failure(
                    &state,
                    deadline,
                    &policy_context,
                    &code,
                    admitted.bound_failure(&diagnostic, &state.query.signer),
                    response_policy_failure,
                    deadline_failure,
                )
                .await;
            }
            None => {
                return finish_v2_failure(
                    &state,
                    deadline,
                    &policy_context,
                    "transaction_deadline_exceeded",
                    deadline_failure.clone(),
                    response_policy_failure,
                    deadline_failure,
                )
                .await;
            }
        };

    let fence_timeout = deadline
        .saturating_duration_since(Instant::now())
        .min(MAX_QUERY_V2_SCHEMA_FENCE_DURATION);
    if fence_timeout.is_zero() {
        return finish_v2_failure(
            &state,
            deadline,
            &policy_context,
            "transaction_deadline_exceeded",
            deadline_failure.clone(),
            response_policy_failure,
            deadline_failure,
        )
        .await;
    }
    let open = state
        .query
        .database
        .schema_fenced_read_transaction(fence_timeout);
    let opened = match await_before_deadline(deadline, open).await {
        Some(result) => result,
        None => {
            return finish_v2_failure(
                &state,
                deadline,
                &policy_context,
                "transaction_deadline_exceeded",
                deadline_failure.clone(),
                response_policy_failure,
                deadline_failure,
            )
            .await;
        }
    };
    let (mut transaction, fenced_schema) = match opened {
        Ok(fenced) => fenced,
        Err(_) => {
            return finish_v2_failure(
                &state,
                deadline,
                &policy_context,
                "query_remote_provider_unavailable",
                admitted.bound_failure(&unavailable(), &state.query.signer),
                response_policy_failure,
                deadline_failure,
            )
            .await;
        }
    };

    // The backend captured this export while the bounded schema-exclusion
    // guard was held, so it is the transaction's exact schema authority.
    match await_before_deadline(
        deadline,
        state
            .query
            .verify_live_authority_export_with_permit(fenced_schema, rebuild_permit),
    )
    .await
    {
        Some(Ok(())) => {}
        Some(Err(diagnostic)) => {
            let code = diagnostic.code().as_str().to_owned();
            return finish_v2_failure_after_transaction_close(
                &state,
                &mut transaction,
                deadline,
                &policy_context,
                &code,
                admitted.bound_failure(&diagnostic, &state.query.signer),
                response_policy_failure,
                deadline_failure,
            )
            .await;
        }
        None => {
            return finish_v2_failure_after_transaction_close(
                &state,
                &mut transaction,
                deadline,
                &policy_context,
                "transaction_deadline_exceeded",
                deadline_failure.clone(),
                response_policy_failure,
                deadline_failure,
            )
            .await;
        }
    }
    if matches!(state.query.control_policy, V2QueryControlPolicy::Managed) {
        let control = verify_managed_query_control_scope(
            &mut transaction,
            state.query.delta_context.scope_id(),
            Some(deadline),
            &state.query.ceilings.answer.cancellation,
        );
        match await_before_deadline(deadline, control).await {
            Some(Ok(())) => {}
            Some(Err(diagnostic)) => {
                let code = diagnostic.code().as_str().to_owned();
                return finish_v2_failure_after_transaction_close(
                    &state,
                    &mut transaction,
                    deadline,
                    &policy_context,
                    &code,
                    admitted.bound_failure(&diagnostic, &state.query.signer),
                    response_policy_failure,
                    deadline_failure,
                )
                .await;
            }
            None => {
                return finish_v2_failure_after_transaction_close(
                    &state,
                    &mut transaction,
                    deadline,
                    &policy_context,
                    "transaction_deadline_exceeded",
                    deadline_failure.clone(),
                    response_policy_failure,
                    deadline_failure,
                )
                .await;
            }
        }
    }
    let reply_expectation = admitted.reply_expectation();
    let execute =
        execute_admitted_remote_request_versioned(admitted, &mut transaction, &state.query.signer);
    let bytes = match await_before_deadline(deadline, execute).await {
        Some(bytes) => bytes,
        None => {
            return finish_v2_failure_after_transaction_close(
                &state,
                &mut transaction,
                deadline,
                &policy_context,
                "transaction_deadline_exceeded",
                deadline_failure.clone(),
                response_policy_failure,
                deadline_failure,
            )
            .await;
        }
    };

    let (bytes, success, code) = match reply_expectation.audit(
        &bytes,
        &state.query.advertisement_fingerprint,
        state.query.advertisement.reply_key(),
    ) {
        Ok(audited) => (bytes, audited.success(), audited.code().to_owned()),
        Err(_) => {
            let diagnostic = internal_response_invalid();
            (
                reply_expectation.bound_internal_failure(
                    &diagnostic,
                    &state.query.advertisement_fingerprint,
                    &state.query.signer,
                ),
                false,
                diagnostic.code().as_str().to_owned(),
            )
        }
    };
    if close_v2_transaction(&mut transaction).await.is_err() {
        tracing::warn!(
            primary_code = %code,
            cleanup_code = "query_remote_transaction_close_failed",
            "remote query transaction cleanup failed"
        );
    }
    let outcome = V2PolicyOutcome::new(success, &code, bytes.len());
    match await_before_deadline(
        response_audit_deadline(deadline),
        state.pipeline.finish_v2_request(&outcome, &policy_context),
    )
    .await
    {
        Some(Ok(())) => {}
        Some(Err(_)) => return envelope_response(response_policy_failure),
        None => return envelope_response(deadline_failure),
    }
    envelope_response(bytes)
}

async fn finish_v2_failure(
    state: &V2RouterState,
    deadline: Instant,
    context: &RequestContext,
    code: &str,
    primary: Vec<u8>,
    policy_rejection: Vec<u8>,
    deadline_rejection: Vec<u8>,
) -> Response {
    let outcome = V2PolicyOutcome::new(false, code, primary.len());
    match await_before_deadline(
        response_audit_deadline(deadline),
        state.pipeline.finish_v2_request(&outcome, context),
    )
    .await
    {
        Some(Ok(())) => envelope_response(primary),
        Some(Err(_)) => envelope_response(policy_rejection),
        None => envelope_response(deadline_rejection),
    }
}

#[allow(clippy::too_many_arguments)]
async fn finish_v2_failure_after_transaction_close(
    state: &V2RouterState,
    transaction: &mut Transaction,
    deadline: Instant,
    context: &RequestContext,
    code: &str,
    primary: Vec<u8>,
    policy_rejection: Vec<u8>,
    deadline_rejection: Vec<u8>,
) -> Response {
    if close_v2_transaction(transaction).await.is_err() {
        tracing::warn!(
            primary_code = code,
            cleanup_code = "query_remote_transaction_close_failed",
            "remote query failed and transaction cleanup also failed"
        );
    }
    finish_v2_failure(
        state,
        deadline,
        context,
        code,
        primary,
        policy_rejection,
        deadline_rejection,
    )
    .await
}

fn response_audit_deadline(request_deadline: Instant) -> Instant {
    Instant::now()
        .checked_add(RESPONSE_AUDIT_GRACE)
        .unwrap_or(request_deadline)
}

#[cfg(test)]
fn bound_internal_response_failure(
    state: &V2QueryState,
    nonce: &str,
    request: &type_bridge_contract::query_remote::RemoteRequestFingerprint,
) -> (Vec<u8>, bool, String) {
    let diagnostic = internal_response_invalid();
    (
        RemoteQueryFailure::bound(nonce.to_owned(), request, &diagnostic)
            .encode_signed_or_fallback(&state.advertisement_fingerprint, &state.signer),
        false,
        diagnostic.code().as_str().to_owned(),
    )
}

fn v2_request_metadata(
    headers: &HeaderMap,
    endpoint: &'static str,
) -> HashMap<String, serde_json::Value> {
    // Authentication and tenant policies commonly consume transport
    // credentials rather than query-AST fields. Keep them outside the canonical
    // plan envelope, but expose every valid textual HTTP header to the same
    // interceptor context as V1 body metadata. The built-in audit interceptor
    // records only its fixed allowlist and never serializes this map.
    let mut values = serde_json::Map::new();
    for name in headers.keys() {
        let entries = headers
            .get_all(name)
            .iter()
            .filter_map(|value| value.to_str().ok().map(str::to_owned))
            .map(serde_json::Value::String)
            .collect::<Vec<_>>();
        if !entries.is_empty() {
            values.insert(name.as_str().to_owned(), serde_json::Value::Array(entries));
        }
    }
    HashMap::from([
        ("transport".to_owned(), serde_json::json!("v2")),
        ("v2_endpoint".to_owned(), serde_json::json!(endpoint)),
        ("query_format".to_owned(), serde_json::json!("typed-plan")),
        ("http_headers".to_owned(), serde_json::Value::Object(values)),
    ])
}

async fn handle_v2_capabilities(
    State(state): State<Arc<V2RouterState>>,
    request: Request,
) -> Response {
    let Some(deadline) = Instant::now().checked_add(state.query.execution_timeout) else {
        return envelope_response(state.query.unbound_failure(&deadline_exceeded()));
    };

    // Discovery is cheap only after the configured V2 transport policy has
    // authenticated/rate-limited it. In particular, rejected discovery never
    // reaches the live schema exporter below.
    let metadata = v2_request_metadata(request.headers(), "capabilities");
    let mut policy_context = match state.pipeline.v2_request_context(metadata) {
        Ok(context) => context,
        Err(_) => {
            return envelope_response(state.query.unbound_failure(&policy_rejected()));
        }
    };
    let policy_failure = state.query.unbound_failure(&policy_rejected());
    let deadline_failure = state.query.unbound_failure(&deadline_exceeded());
    match await_before_deadline(
        deadline,
        state.pipeline.begin_v2_request(&mut policy_context),
    )
    .await
    {
        Some(Ok(())) => {}
        Some(Err(_)) => {
            return finish_v2_failure(
                &state,
                deadline,
                &policy_context,
                "query_remote_policy_rejected",
                policy_failure.clone(),
                policy_failure,
                deadline_failure,
            )
            .await;
        }
        None => {
            return finish_v2_failure(
                &state,
                deadline,
                &policy_context,
                "transaction_deadline_exceeded",
                deadline_failure.clone(),
                policy_failure,
                deadline_failure,
            )
            .await;
        }
    }

    // Discovery proves that the advertised executor still has the configured
    // live schema/profile authority, but does not open an execution
    // transaction. Exact schema fencing and managed-control admission belong
    // to startup and to the query transaction that consumes them.
    match await_before_deadline(deadline, state.query.verify_live_authority()).await {
        Some(Ok(())) => {}
        Some(Err(diagnostic)) => {
            let code = diagnostic.code().as_str().to_owned();
            let failure = state.query.unbound_failure(&diagnostic);
            return finish_v2_failure(
                &state,
                deadline,
                &policy_context,
                &code,
                failure,
                policy_failure,
                deadline_failure,
            )
            .await;
        }
        None => {
            return finish_v2_failure(
                &state,
                deadline,
                &policy_context,
                "transaction_deadline_exceeded",
                deadline_failure.clone(),
                policy_failure,
                deadline_failure,
            )
            .await;
        }
    }
    let bytes = match state.query.advertisement.encode() {
        Ok(bytes) => bytes,
        Err(diagnostic) => {
            let code = diagnostic.code().as_str().to_owned();
            let failure = state.query.unbound_failure(&diagnostic);
            return finish_v2_failure(
                &state,
                deadline,
                &policy_context,
                &code,
                failure,
                policy_failure,
                deadline_failure,
            )
            .await;
        }
    };
    let outcome = V2PolicyOutcome::new(true, "ok", bytes.len());
    match await_before_deadline(
        response_audit_deadline(deadline),
        state.pipeline.finish_v2_request(&outcome, &policy_context),
    )
    .await
    {
        Some(Ok(())) => envelope_response(bytes),
        Some(Err(_)) => envelope_response(policy_failure),
        None => envelope_response(deadline_failure),
    }
}

fn envelope_response(bytes: Vec<u8>) -> Response {
    ([(CONTENT_TYPE, "application/json")], bytes).into_response()
}

fn diagnostic(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static V2 transport code"),
        message,
    )
}

fn unavailable() -> Diagnostic {
    diagnostic(
        DiagnosticCategory::Integrity,
        "query_remote_provider_unavailable",
        "the executor could not open a read transaction",
    )
}

fn policy_rejected() -> Diagnostic {
    diagnostic(
        DiagnosticCategory::Integrity,
        "query_remote_policy_rejected",
        "the executor policy rejected the request",
    )
}

fn replayed() -> Diagnostic {
    diagnostic(
        DiagnosticCategory::Integrity,
        "query_remote_replay",
        "the request nonce was already consumed",
    )
}

fn replay_capacity() -> Diagnostic {
    diagnostic(
        DiagnosticCategory::ResourceLimit,
        "query_remote_replay_capacity",
        "the replay registry cannot admit another request",
    )
}

fn replay_unavailable() -> Diagnostic {
    diagnostic(
        DiagnosticCategory::Integrity,
        "query_remote_replay_unavailable",
        "the replay registry could not prove nonce uniqueness",
    )
}

fn deadline_exceeded() -> Diagnostic {
    diagnostic(
        DiagnosticCategory::ResourceLimit,
        "transaction_deadline_exceeded",
        "provider transaction deadline expired",
    )
}

fn body_oversized() -> Diagnostic {
    diagnostic(
        DiagnosticCategory::ResourceLimit,
        "query_remote_envelope_oversized",
        "remote request envelope exceeds the transport byte ceiling",
    )
}

fn internal_response_invalid() -> Diagnostic {
    diagnostic(
        DiagnosticCategory::Integrity,
        "query_remote_internal_response_invalid",
        "the executor produced invalid bound response evidence",
    )
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use axum::body::Body;
    use axum::http::StatusCode;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use type_bridge_contract::capability::CapabilityId;
    use type_bridge_contract::codec::FormatVersion;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
    use type_bridge_contract::limits::{
        MAX_CANONICAL_COLLECTION_LEN, MAX_REMOTE_ENVELOPE_BYTES, StructuralLimits,
    };
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
    use type_bridge_contract::query_plan::{
        DocumentField, DocumentSource, QueryInvocation, QueryOperation, QueryOutput, QueryPattern,
        QueryPlan, QueryPlanFingerprint, ReadStage,
    };
    use type_bridge_contract::query_plan::{
        query_plan_capability_vocabulary, query_plan_v2_capability_vocabulary,
    };
    use type_bridge_contract::query_remote::{
        RemoteLimits, RemoteOutcome, RemoteOutcomeShape, RemoteQueryResponse,
        RemoteReplyDecodeLimits, RemoteRequestFingerprint,
    };
    use type_bridge_contract::query_remote_v2::{
        RemoteLimitsV2, RemoteQueryRequestV2, RemoteReplyDecodeLimitsV2, RemoteReplyV2,
        RemoteRequestFingerprintV2, decode_remote_reply_v2, decode_signed_remote_failure_v2,
    };
    use type_bridge_contract::schema::{
        OwnsFact, OwnsFactId, SchemaFact, SourceSpan, SourcedSchemaFact, TypeFact, ValueFact,
        ValueFactId,
    };
    use type_bridge_contract::value::ValueTypeTag;
    use type_bridge_orm::error::OrmError;
    use type_bridge_orm::query_v2_remote::{encode_remote_request, encode_remote_request_v2};
    use type_bridge_orm::session::backend::{
        BoxFuture, DriverBackend, QueryResult, SchemaFencedReadTransaction, TransactionOps, TxType,
    };
    use type_bridge_query::validate_query_plan;
    use type_bridge_schema::{managed_schema_state, resolve};

    use crate::interceptor::{InterceptError, Interceptor};
    use crate::pipeline::{PipelineBuilder, QueryPipeline};
    use crate::test_helpers::MockExecutor;

    use super::*;

    const CURRENT_SCHEMA: &str = "define\nattribute transport-name, value string;\nentity transport-person, owns transport-name;";
    const STALE_SCHEMA: &str = "define\nattribute transport-name, value string;\nentity transport-person, owns transport-name;\nentity transport-unexpected;";
    const TEST_NONCE: &str = "server-route-nonce-000001";

    #[derive(Default)]
    struct BackendMetrics {
        fail_close: AtomicBool,
        fail_query: AtomicBool,
        schema_exports: AtomicUsize,
        transaction_opens: AtomicUsize,
        transaction_closes: AtomicUsize,
        query_executions: AtomicUsize,
    }

    struct CountingBackend {
        fallback_schema: String,
        metrics: Arc<BackendMetrics>,
        observations: Mutex<VecDeque<String>>,
    }

    impl CountingBackend {
        fn new(
            observations: impl IntoIterator<Item = &'static str>,
            metrics: Arc<BackendMetrics>,
        ) -> Self {
            Self {
                fallback_schema: CURRENT_SCHEMA.to_owned(),
                metrics,
                observations: Mutex::new(
                    observations
                        .into_iter()
                        .map(str::to_owned)
                        .collect::<VecDeque<_>>(),
                ),
            }
        }
    }

    impl DriverBackend for CountingBackend {
        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            self.metrics
                .transaction_opens
                .fetch_add(1, Ordering::SeqCst);
            let transaction = CountingTransaction {
                metrics: Arc::clone(&self.metrics),
            };
            Box::pin(async move { Ok(Box::new(transaction) as Box<dyn TransactionOps>) })
        }

        fn open_schema_fenced_read_transaction(
            &self,
            database: &str,
            _timeout: Duration,
        ) -> BoxFuture<'_, Result<SchemaFencedReadTransaction, OrmError>> {
            let database = database.to_owned();
            Box::pin(async move {
                let transaction = self.open_transaction(&database, TxType::Write).await?;
                let schema_text = self.schema_text(&database).await?;
                Ok(SchemaFencedReadTransaction::new(transaction, schema_text))
            })
        }

        fn is_open(&self) -> bool {
            true
        }

        fn server_version(&self) -> Option<type_bridge_core_lib::version::Version> {
            Some(type_bridge_core_lib::version::Version::new(3, 12, 1))
        }

        fn schema_text(&self, _database: &str) -> BoxFuture<'_, Result<String, OrmError>> {
            self.metrics.schema_exports.fetch_add(1, Ordering::SeqCst);
            let schema = self
                .observations
                .lock()
                .expect("schema observation lock")
                .pop_front()
                .unwrap_or_else(|| self.fallback_schema.clone());
            Box::pin(async move { Ok(schema) })
        }
    }

    struct CountingTransaction {
        metrics: Arc<BackendMetrics>,
    }

    impl TransactionOps for CountingTransaction {
        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            self.metrics.query_executions.fetch_add(1, Ordering::SeqCst);
            let fail = self.metrics.fail_query.load(Ordering::SeqCst);
            Box::pin(async move {
                if fail {
                    Err(OrmError::QueryExecution("masked query fixture".into()))
                } else {
                    Ok(QueryResult::Rows(Vec::new()))
                }
            })
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.metrics
                .transaction_closes
                .fetch_add(1, Ordering::SeqCst);
            let fail = self.metrics.fail_close.load(Ordering::SeqCst);
            Box::pin(async move {
                if fail {
                    Err(OrmError::Transaction("masked close fixture".into()))
                } else {
                    Ok(())
                }
            })
        }
    }

    #[derive(Default)]
    struct PolicyMetrics {
        requests: AtomicUsize,
        responses: AtomicUsize,
        transports: AtomicUsize,
        outcomes: Mutex<Vec<(String, usize)>>,
    }

    struct CountingV2Policy {
        metrics: Arc<PolicyMetrics>,
        reject_request: bool,
        reject_transport: bool,
    }

    impl Interceptor for CountingV2Policy {
        fn name(&self) -> &str {
            "counting-v2-policy"
        }

        fn on_request<'a>(
            &'a self,
            clauses: Vec<type_bridge_core_lib::ast::Clause>,
            _ctx: &'a mut RequestContext,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<Vec<type_bridge_core_lib::ast::Clause>, InterceptError>>
                    + Send
                    + 'a,
            >,
        > {
            Box::pin(async move { Ok(clauses) })
        }

        fn supports_v2(&self) -> bool {
            true
        }

        fn on_v2_transport<'a>(
            &'a self,
            _ctx: &'a mut RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
            Box::pin(async move {
                self.metrics.transports.fetch_add(1, Ordering::SeqCst);
                if self.reject_transport {
                    return Err(InterceptError::AccessDenied {
                        reason: "test policy rejection".to_owned(),
                    });
                }
                Ok(())
            })
        }

        fn on_v2_request<'a>(
            &'a self,
            _request: &'a V2PolicyRequest<'a>,
            _ctx: &'a mut RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
            Box::pin(async move {
                self.metrics.requests.fetch_add(1, Ordering::SeqCst);
                if self.reject_request {
                    return Err(InterceptError::AccessDenied {
                        reason: "test request-policy rejection".to_owned(),
                    });
                }
                Ok(())
            })
        }

        fn on_v2_response<'a>(
            &'a self,
            outcome: &'a V2PolicyOutcome<'a>,
            _ctx: &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
            Box::pin(async move {
                self.metrics.responses.fetch_add(1, Ordering::SeqCst);
                self.metrics
                    .outcomes
                    .lock()
                    .expect("policy outcome lock")
                    .push((outcome.code().to_owned(), outcome.response_bytes()));
                Ok(())
            })
        }
    }

    struct AuthorityFixture {
        declared: DeclaredSchema,
        delta_context: ManagedDeltaContext,
        managed: ManagedSchemaState,
        resolved: ResolvedSchema,
    }

    fn authority_fixture(include_unexpected: bool) -> AuthorityFixture {
        let person = TypeId::new(TypeKind::Entity, "transport-person").expect("type");
        let name = AttributeId::new("transport-name").expect("attribute");
        let mut facts = vec![
            SchemaFact::Type(TypeFact::new(person.clone()).expect("type fact")),
            SchemaFact::Type(
                TypeFact::new(TypeId::new(TypeKind::Attribute, "transport-name").expect("type"))
                    .expect("type fact"),
            ),
            SchemaFact::Value(ValueFact::new(
                ValueFactId::new(name.clone()),
                ValueTypeTag::String,
            )),
            SchemaFact::Owns(OwnsFact::new(
                OwnsFactId::new(person, name).expect("owns fact ID"),
            )),
        ];
        if include_unexpected {
            facts.push(SchemaFact::Type(
                TypeFact::new(TypeId::new(TypeKind::Entity, "transport-unexpected").expect("type"))
                    .expect("type fact"),
            ));
        }
        let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
            let byte = u64::try_from(index).expect("source byte");
            let line = u32::try_from(index + 1).expect("source line");
            SourcedSchemaFact::new(
                fact,
                SourceSpan::new(
                    DocumentId::new("server-v2-route-fixture.typeql").expect("document"),
                    byte,
                    byte + 1,
                    line,
                    1,
                    line,
                    2,
                )
                .expect("source span"),
            )
        });
        let declared = DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced)
            .expect("declared schema");
        let profile = SemanticProfileId::new("typedb-3.12.1/v1").expect("profile");
        let resolved = resolve(&declared, &profile).expect("resolved schema");
        let delta_context = ManagedDeltaContext::new(
            ManagedScopeId::new("server-v2-route").expect("scope"),
            profile,
            CapabilitySet::new(),
        );
        let managed = managed_schema_state(&declared, &delta_context).expect("managed schema");
        AuthorityFixture {
            declared,
            delta_context,
            managed,
            resolved,
        }
    }

    fn validated_request(
        authority: &AuthorityFixture,
        advertisement: &RemoteCapabilities,
        nonce: &str,
    ) -> (Vec<u8>, QueryPlanFingerprint, RemoteRequestFingerprint) {
        validated_request_for(
            authority,
            advertisement,
            nonce,
            QueryOperation::Rows,
            QueryOutput::Rows {
                columns: vec![BindingId::new(0).expect("binding id")],
            },
            RemoteLimits {
                deadline_ms: Some(5_000),
                max_bytes: 1 << 20,
                max_items: 100,
                max_collection_members: 1 << 16,
            },
        )
    }

    fn validated_request_for(
        authority: &AuthorityFixture,
        advertisement: &RemoteCapabilities,
        nonce: &str,
        operation: QueryOperation,
        output: QueryOutput,
        limits: RemoteLimits,
    ) -> (Vec<u8>, QueryPlanFingerprint, RemoteRequestFingerprint) {
        let mut bindings = vec![AssertionBinding::new(
            BindingId::new(0).expect("binding id"),
            QueryVariable::new("person").expect("binding variable"),
        )];
        let mut patterns = vec![QueryPattern::Isa {
            binding: BindingId::new(0).expect("binding id"),
            include_subtypes: false,
            type_id: TypeId::new(TypeKind::Entity, "transport-person").expect("type"),
        }];
        if matches!(output, QueryOutput::Documents { .. }) {
            bindings.push(AssertionBinding::new(
                BindingId::new(1).expect("binding id"),
                QueryVariable::new("name").expect("binding variable"),
            ));
            patterns.push(QueryPattern::Has {
                attribute: BindingId::new(1).expect("binding id"),
                attribute_id: AttributeId::new("transport-name").expect("attribute"),
                owner: BindingId::new(0).expect("binding id"),
            });
        }
        let plan = QueryPlan::new(
            bindings,
            Vec::new(),
            vec![ReadStage::Match { patterns }],
            output,
            authority.managed.managed_semantic_schema().clone(),
        )
        .expect("query plan");
        let context =
            MigrationAssertionValidationContext::new(&authority.resolved, &authority.managed);
        let validated = validate_query_plan(&plan, &context, StructuralLimits::CANONICAL)
            .expect("validated query");
        let invocation =
            QueryInvocation::new(&plan, operation, Vec::new()).expect("query invocation");
        let request = encode_remote_request(&validated, &invocation, advertisement, limits, nonce)
            .expect("remote request");
        let plan_fingerprint = QueryPlanFingerprint::compute(&plan).expect("plan fingerprint");
        let request_fingerprint =
            RemoteRequestFingerprint::compute(&request).expect("request fingerprint");
        (request, plan_fingerprint, request_fingerprint)
    }

    fn validated_request_v2(
        authority: &AuthorityFixture,
        advertisement: &RemoteCapabilities,
        nonce: &str,
    ) -> (
        Vec<u8>,
        QueryPlan,
        RemoteQueryRequestV2,
        RemoteRequestFingerprintV2,
        RemoteLimitsV2,
    ) {
        let plan = QueryPlan::new_v2(
            vec![AssertionBinding::new(
                BindingId::new(0).expect("binding id"),
                QueryVariable::new("person").expect("binding variable"),
            )],
            Vec::new(),
            vec![ReadStage::Match {
                patterns: vec![QueryPattern::Isa {
                    binding: BindingId::new(0).expect("binding id"),
                    include_subtypes: false,
                    type_id: TypeId::new(TypeKind::Entity, "transport-person").expect("type"),
                }],
            }],
            QueryOutput::Rows {
                columns: vec![BindingId::new(0).expect("binding id")],
            },
            authority.managed.managed_semantic_schema().clone(),
        )
        .expect("V2 query plan");
        let context =
            MigrationAssertionValidationContext::new(&authority.resolved, &authority.managed);
        let validated = validate_query_plan(&plan, &context, StructuralLimits::CANONICAL)
            .expect("validated V2 query");
        let invocation =
            QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).expect("V2 invocation");
        let limits = RemoteLimitsV2 {
            deadline_ms: Some(5_000),
            max_bytes: 1 << 20,
            max_items: 100,
            max_collection_members: 1 << 16,
            max_graph_nodes: 0,
            max_attribute_values: 0,
            max_role_players: 0,
        };
        let request =
            encode_remote_request_v2(&validated, &invocation, advertisement, limits, nonce)
                .expect("V2 remote request");
        let envelope = RemoteQueryRequestV2::decode(&request).expect("V2 request envelope");
        let fingerprint =
            RemoteRequestFingerprintV2::compute(&request).expect("V2 request fingerprint");
        (request, plan, envelope, fingerprint, limits)
    }

    struct RouteFixture {
        advertisement: RemoteCapabilities,
        metrics: Arc<BackendMetrics>,
        nonce: &'static str,
        plan_fingerprint: QueryPlanFingerprint,
        query: Arc<V2QueryState>,
        request: Vec<u8>,
        request_fingerprint: RemoteRequestFingerprint,
        router: Router,
    }

    fn default_pipeline() -> QueryPipeline {
        PipelineBuilder::new(MockExecutor::new())
            .with_default_database("server-v2-route")
            .build()
            .expect("query pipeline")
    }

    fn pipeline_with_policy(metrics: Arc<PolicyMetrics>, reject_transport: bool) -> QueryPipeline {
        PipelineBuilder::new(MockExecutor::new())
            .with_default_database("server-v2-route")
            .with_interceptor(CountingV2Policy {
                metrics,
                reject_request: false,
                reject_transport,
            })
            .build()
            .expect("query pipeline")
    }

    fn pipeline_with_rejecting_request_policy(metrics: Arc<PolicyMetrics>) -> QueryPipeline {
        PipelineBuilder::new(MockExecutor::new())
            .with_default_database("server-v2-route")
            .with_interceptor(CountingV2Policy {
                metrics,
                reject_request: true,
                reject_transport: false,
            })
            .build()
            .expect("query pipeline")
    }

    fn route_fixture(
        observations: impl IntoIterator<Item = &'static str>,
        advertised: CapabilitySet,
        pipeline: QueryPipeline,
    ) -> RouteFixture {
        route_fixture_with_replay_store(
            observations,
            advertised,
            pipeline,
            Arc::new(InMemoryReplayStore::new(DEFAULT_REPLAY_CAPACITY)),
        )
    }

    fn route_fixture_with_replay_store(
        observations: impl IntoIterator<Item = &'static str>,
        advertised: CapabilitySet,
        pipeline: QueryPipeline,
        replay_store: Arc<dyn RemoteReplayStore>,
    ) -> RouteFixture {
        let authority = authority_fixture(false);
        let metrics = Arc::new(BackendMetrics::default());
        let database = Database::with_backend(
            Box::new(CountingBackend::new(observations, Arc::clone(&metrics))),
            "server-v2-route",
        );
        let query = Arc::new(
            V2QueryState::new_query_only(
                advertised,
                QueryV2AnswerLimits::default(),
                database,
                authority.declared,
                authority.delta_context,
                authority.managed,
                authority.resolved,
            )
            .expect("executor advertisement is canonical")
            .with_replay_store(replay_store),
        );
        let advertisement = query.advertisement.clone();
        let authority = authority_fixture(false);
        let (request, plan_fingerprint, request_fingerprint) =
            validated_request(&authority, &advertisement, TEST_NONCE);
        let router = create_router_with_v2(Arc::new(pipeline), Arc::clone(&query));
        RouteFixture {
            advertisement,
            metrics,
            nonce: TEST_NONCE,
            plan_fingerprint,
            query,
            request,
            request_fingerprint,
            router,
        }
    }

    async fn post_query(router: &Router, body: Vec<u8>) -> Vec<u8> {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v2/query")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .expect("request"),
            )
            .await
            .expect("route response");
        assert_eq!(response.status(), StatusCode::OK);
        response
            .into_body()
            .collect()
            .await
            .expect("response body")
            .to_bytes()
            .to_vec()
    }

    struct V1RouteSnapshot {
        method: &'static str,
        path: &'static str,
        request_body: &'static [u8],
        status: StatusCode,
        response_body: &'static [u8],
    }

    async fn assert_v1_route_snapshot(router: &Router, snapshot: V1RouteSnapshot) {
        let mut request = Request::builder()
            .method(snapshot.method)
            .uri(snapshot.path);
        if !snapshot.request_body.is_empty() {
            request = request.header("content-type", "application/json");
        }
        let response = router
            .clone()
            .oneshot(
                request
                    .body(Body::from(snapshot.request_body))
                    .expect("V1 snapshot request"),
            )
            .await
            .expect("V1 snapshot response");
        assert_eq!(
            response.status(),
            snapshot.status,
            "{} {} status",
            snapshot.method,
            snapshot.path,
        );
        assert_eq!(
            response.headers().len(),
            2,
            "{} {} released header set",
            snapshot.method,
            snapshot.path,
        );
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("application/json"),
            "{} {} content type",
            snapshot.method,
            snapshot.path,
        );
        let expected_content_length = snapshot.response_body.len().to_string();
        assert_eq!(
            response
                .headers()
                .get("content-length")
                .and_then(|value| value.to_str().ok()),
            Some(expected_content_length.as_str()),
            "{} {} content length",
            snapshot.method,
            snapshot.path,
        );
        let body = response
            .into_body()
            .collect()
            .await
            .expect("V1 snapshot body")
            .to_bytes();
        assert_eq!(
            body.as_ref(),
            snapshot.response_body,
            "{} {} released body",
            snapshot.method,
            snapshot.path,
        );
    }

    #[tokio::test]
    async fn merged_router_retains_every_released_v1_route_snapshot() {
        const QUERY_FAILURE: &[u8] = br#"{"status":"error","error":{"code":"QUERY_EXECUTION_ERROR","message":"Query execution error: Unknown transaction type: v1-wire-probe"}}"#;
        const SCHEMA_FAILURE: &[u8] = br#"{"status":"error","error":{"code":"SCHEMA_ERROR","message":"Schema error: No schema loaded"}}"#;
        let pipeline = PipelineBuilder::new(MockExecutor::failing(
            "Unknown transaction type: v1-wire-probe",
        ))
        .with_default_database("server-v2-route")
        .build()
        .expect("V1 snapshot pipeline");
        let fixture = route_fixture(
            [CURRENT_SCHEMA],
            query_plan_v2_capability_vocabulary(),
            pipeline,
        );
        let snapshots = [
            V1RouteSnapshot {
                method: "GET",
                path: "/health",
                request_body: b"",
                status: StatusCode::OK,
                response_body:
                    br#"{"status":"ok","version":"1.5.11","typedb_connected":true}"#,
            },
            V1RouteSnapshot {
                method: "POST",
                path: "/query",
                request_body: br#"{"transaction_type":"read","clauses":[]}"#,
                status: StatusCode::BAD_REQUEST,
                response_body: QUERY_FAILURE,
            },
            V1RouteSnapshot {
                method: "POST",
                path: "/query/raw",
                request_body: br#"{"transaction_type":"read","query":"match $p isa person; fetch { \"person\": { $p.* } };"}"#,
                status: StatusCode::BAD_REQUEST,
                response_body: QUERY_FAILURE,
            },
            V1RouteSnapshot {
                method: "POST",
                path: "/query/validate",
                request_body: br#"{"clauses":[]}"#,
                status: StatusCode::INTERNAL_SERVER_ERROR,
                response_body: SCHEMA_FAILURE,
            },
            V1RouteSnapshot {
                method: "GET",
                path: "/schema",
                request_body: b"",
                status: StatusCode::INTERNAL_SERVER_ERROR,
                response_body: SCHEMA_FAILURE,
            },
        ];

        for snapshot in snapshots {
            assert_v1_route_snapshot(&fixture.router, snapshot).await;
        }
    }

    fn failure_code(query: &V2QueryState, bytes: &[u8]) -> String {
        type_bridge_contract::query_remote::decode_signed_remote_failure(
            bytes,
            &query.advertisement_fingerprint,
            query.advertisement.reply_key(),
            u64::try_from(type_bridge_contract::limits::MAX_REMOTE_ENVELOPE_BYTES)
                .expect("wire ceiling"),
            &Ed25519RemoteReplyVerifier,
        )
        .expect("failure envelope")
        .diagnostic()
        .expect("failure diagnostic")
        .code()
        .as_str()
        .to_owned()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocking_preflight_admission_has_a_fixed_concurrency_bound_and_releases() {
        let slots = new_v2_preflight_slots();
        let permits = (0..V2_PREFLIGHT_CONCURRENCY)
            .map(|_| {
                Arc::clone(&slots)
                    .try_acquire_owned()
                    .expect("configured preflight slot")
            })
            .collect::<Vec<_>>();
        assert!(
            Arc::clone(&slots).try_acquire_owned().is_err(),
            "a fifth blocking preflight must wait rather than detach unbounded CPU work",
        );
        drop(permits);
        let released = Arc::clone(&slots)
            .try_acquire_owned()
            .expect("released blocking preflight slot");
        drop(released);

        let permit = Arc::clone(&slots)
            .acquire_owned()
            .await
            .expect("blocking-task permit");
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
        })
        .await
        .expect("blocking preflight completion");
        assert_eq!(
            slots.available_permits(),
            V2_PREFLIGHT_CONCURRENCY,
            "the blocking task must release its owned permit on completion",
        );
    }

    #[tokio::test]
    async fn one_route_dispatches_v1_and_v2_without_downgrade_and_replays_before_host_work() {
        let policy_metrics = Arc::new(PolicyMetrics::default());
        let fixture = route_fixture(
            [CURRENT_SCHEMA; 4],
            query_plan_capability_vocabulary(),
            pipeline_with_policy(Arc::clone(&policy_metrics), false),
        );

        let v1 = post_query(&fixture.router, fixture.request.clone()).await;
        assert!(matches!(
            decode_remote_reply(
                &v1,
                fixture.nonce,
                &fixture.plan_fingerprint,
                &fixture.request_fingerprint,
                &fixture.query.advertisement_fingerprint,
                fixture.advertisement.reply_key(),
                RemoteReplyDecodeLimits {
                    shape: RemoteOutcomeShape::Rows { width: 1 },
                    max_bytes: 1 << 20,
                    max_items: 100,
                    max_collection_members: 1 << 16,
                },
                &Ed25519RemoteReplyVerifier,
            )
            .expect("V1 reply"),
            RemoteReply::Response(_)
        ));

        let authority = authority_fixture(false);
        let v2_nonce = "server-v2-dispatch-nonce-0001";
        let (request, plan, envelope, fingerprint, limits) =
            validated_request_v2(&authority, &fixture.advertisement, v2_nonce);
        let v2 = post_query(&fixture.router, request.clone()).await;
        assert!(matches!(
            decode_remote_reply_v2(
                &v2,
                &envelope,
                &plan.fingerprint().expect("V2 plan fingerprint"),
                &fingerprint,
                &fixture.query.advertisement_fingerprint,
                fixture.advertisement.reply_key(),
                RemoteReplyDecodeLimitsV2 {
                    max_bytes: limits.max_bytes,
                    max_items: limits.max_items,
                    max_collection_members: limits.max_collection_members,
                    max_graph_nodes: limits.max_graph_nodes,
                    max_attribute_values: limits.max_attribute_values,
                    max_role_players: limits.max_role_players,
                },
                &Ed25519RemoteReplyVerifier,
            )
            .expect("V2 reply"),
            RemoteReplyV2::Response(_)
        ));
        assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 2);
        assert_eq!(fixture.metrics.query_executions.load(Ordering::SeqCst), 2);
        assert_eq!(policy_metrics.requests.load(Ordering::SeqCst), 2);
        let exports_before_replay = fixture.metrics.schema_exports.load(Ordering::SeqCst);

        let replay = post_query(&fixture.router, request).await;
        let failure = match decode_remote_reply_v2(
            &replay,
            &envelope,
            &plan.fingerprint().expect("V2 plan fingerprint"),
            &fingerprint,
            &fixture.query.advertisement_fingerprint,
            fixture.advertisement.reply_key(),
            RemoteReplyDecodeLimitsV2 {
                max_bytes: limits.max_bytes,
                max_items: limits.max_items,
                max_collection_members: limits.max_collection_members,
                max_graph_nodes: limits.max_graph_nodes,
                max_attribute_values: limits.max_attribute_values,
                max_role_players: limits.max_role_players,
            },
            &Ed25519RemoteReplyVerifier,
        )
        .expect("request-bound V2 replay failure")
        {
            RemoteReplyV2::Failure(failure) => failure,
            RemoteReplyV2::Response(_) => panic!("replayed V2 request cannot succeed"),
        };
        assert_eq!(
            failure
                .diagnostic()
                .expect("complete V2 diagnostic")
                .code()
                .as_str(),
            "query_remote_replay"
        );
        assert_eq!(
            fixture.metrics.schema_exports.load(Ordering::SeqCst),
            exports_before_replay,
            "replay rejects before live authority reconstruction"
        );
        assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 2);
        assert_eq!(fixture.metrics.query_executions.load(Ordering::SeqCst), 2);
        assert_eq!(
            policy_metrics.requests.load(Ordering::SeqCst),
            2,
            "V2 replay rejects before request-specific application policy"
        );
    }

    #[tokio::test]
    async fn declared_oversized_body_rejects_before_collection_and_preflight() {
        let fixture = route_fixture(
            [CURRENT_SCHEMA],
            query_plan_capability_vocabulary(),
            default_pipeline(),
        );
        let response = fixture
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v2/query")
                    .header(CONTENT_TYPE, "application/json")
                    .header(CONTENT_LENGTH, V2_BODY_LIMIT_BYTES + 1)
                    .body(Body::empty())
                    .expect("oversized request declaration"),
            )
            .await
            .expect("route response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("response body")
            .to_bytes();
        assert_eq!(
            failure_code(&fixture.query, &body),
            "query_remote_envelope_oversized",
        );
        assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.metrics.schema_exports.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hostile_maximal_body_dispatches_from_its_prefix_without_host_construction() {
        let fixture = route_fixture(
            [CURRENT_SCHEMA],
            query_plan_v2_capability_vocabulary(),
            default_pipeline(),
        );
        let authority = authority_fixture(false);
        let (request, _, _, _, _) = validated_request_v2(
            &authority,
            &fixture.advertisement,
            "server-v2-hostile-prefix-0001",
        );
        let format_field = br#","format":"typebridge.query-remote-request/v2","#;
        let format_end = request
            .windows(format_field.len())
            .position(|window| window == format_field)
            .map(|offset| offset + format_field.len())
            .expect("canonical V2 format field is in the bounded prefix");
        let mut hostile = request[..format_end].to_vec();
        hostile.resize(MAX_REMOTE_ENVELOPE_BYTES, b'[');

        let response = post_query(&fixture.router, hostile).await;
        let failure = decode_signed_remote_failure_v2(
            &response,
            &fixture.query.advertisement_fingerprint,
            fixture.advertisement.reply_key(),
            u64::try_from(MAX_REMOTE_ENVELOPE_BYTES).expect("wire ceiling"),
            &Ed25519RemoteReplyVerifier,
        )
        .expect("the bounded prefix selects a signed V2 pre-request failure");
        assert_eq!(
            failure
                .diagnostic()
                .expect("complete V2 diagnostic")
                .code()
                .as_str(),
            "query_remote_v2_format_missing",
        );
        assert_eq!(fixture.metrics.schema_exports.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.metrics.query_executions.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn response_wire_floors_reject_before_host_work_and_admit_exact_boundaries() {
        let fixture = route_fixture(
            [CURRENT_SCHEMA; 8],
            query_plan_capability_vocabulary(),
            default_pipeline(),
        );
        let authority = authority_fixture(false);
        let shapes = vec![
            (
                "rows",
                QueryOperation::Rows,
                QueryOutput::Rows {
                    columns: vec![BindingId::new(0).expect("binding id")],
                },
                RemoteOutcome::Rows { rows: Vec::new() },
                RemoteOutcomeShape::Rows { width: 1 },
            ),
            (
                "documents",
                QueryOperation::Rows,
                QueryOutput::Documents {
                    fields: vec![DocumentField::new(
                        QueryVariable::new("person").expect("document key"),
                        DocumentSource::Binding {
                            binding: BindingId::new(1).expect("binding id"),
                        },
                    )],
                },
                RemoteOutcome::Documents {
                    documents: Vec::new(),
                },
                RemoteOutcomeShape::Documents { width: 1 },
            ),
            (
                "count",
                QueryOperation::Count,
                QueryOutput::Rows {
                    columns: vec![BindingId::new(0).expect("binding id")],
                },
                RemoteOutcome::Count { value: 0 },
                RemoteOutcomeShape::Count,
            ),
            (
                "exists",
                QueryOperation::Exists,
                QueryOutput::Rows {
                    columns: vec![BindingId::new(0).expect("binding id")],
                },
                RemoteOutcome::Exists { value: false },
                RemoteOutcomeShape::Exists,
            ),
        ];
        let limits = |max_bytes| RemoteLimits {
            deadline_ms: Some(30_000),
            max_bytes,
            // A zero-item answer makes the empty/zero/false outcomes above the
            // exact smallest admissible successes for all four shapes.
            max_items: 0,
            max_collection_members: 0,
        };

        for (name, operation, output, outcome, response_shape) in &shapes {
            let probe_nonce = format!("server-wire-floor-{name}-probe-0001");
            let (_, plan, request) = validated_request_for(
                &authority,
                &fixture.advertisement,
                &probe_nonce,
                *operation,
                output.clone(),
                limits(1 << 20),
            );
            let floor = u64::try_from(
                RemoteQueryResponse::new(&probe_nonce, &plan, &request, outcome.clone())
                    .expect("minimal response")
                    .signed_encoded_len(
                        &fixture.query.advertisement_fingerprint,
                        fixture.advertisement.reply_key(),
                    )
                    .expect("minimal signed response length"),
            )
            .expect("wire length");

            for (suffix, max_bytes) in [("zero", 0), ("below", floor - 1)] {
                let nonce = format!("server-wire-floor-{name}-{suffix}-0001");
                let (request_bytes, plan, request) = validated_request_for(
                    &authority,
                    &fixture.advertisement,
                    &nonce,
                    *operation,
                    output.clone(),
                    limits(max_bytes),
                );
                let body = post_query(&fixture.router, request_bytes).await;
                let failure = match decode_remote_reply(
                    &body,
                    &nonce,
                    &plan,
                    &request,
                    &fixture.query.advertisement_fingerprint,
                    fixture.advertisement.reply_key(),
                    RemoteReplyDecodeLimits {
                        shape: *response_shape,
                        max_bytes,
                        max_items: 0,
                        max_collection_members: 0,
                    },
                    &Ed25519RemoteReplyVerifier,
                )
                .expect("request-bound failure remains decodable at the success budget")
                {
                    RemoteReply::Failure(failure) => failure,
                    RemoteReply::Response(_) => panic!("undersized request cannot succeed"),
                };
                assert_eq!(
                    failure
                        .diagnostic()
                        .expect("failure diagnostic")
                        .code()
                        .as_str(),
                    "query_remote_response_oversized",
                    "{name} at {suffix} budget",
                );
            }
        }
        assert_eq!(fixture.metrics.schema_exports.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.metrics.query_executions.load(Ordering::SeqCst), 0);

        for (name, operation, output, outcome, _) in shapes {
            let probe_nonce = format!("server-wire-floor-{name}-probe-0002");
            let (_, plan, request) = validated_request_for(
                &authority,
                &fixture.advertisement,
                &probe_nonce,
                operation,
                output.clone(),
                limits(1 << 20),
            );
            let floor = u64::try_from(
                RemoteQueryResponse::new(&probe_nonce, &plan, &request, outcome.clone())
                    .expect("minimal response")
                    .signed_encoded_len(
                        &fixture.query.advertisement_fingerprint,
                        fixture.advertisement.reply_key(),
                    )
                    .expect("minimal signed response length"),
            )
            .expect("wire length");
            let nonce = format!("server-wire-floor-{name}-exact-0001");
            let (request_bytes, plan, request) = validated_request_for(
                &authority,
                &fixture.advertisement,
                &nonce,
                operation,
                output,
                limits(floor),
            );
            let expected = RemoteQueryResponse::new(&nonce, &plan, &request, outcome)
                .expect("exact response")
                .encode_signed(
                    &fixture.query.advertisement_fingerprint,
                    &fixture.query.signer,
                )
                .expect("exact signed response");
            assert_eq!(u64::try_from(expected.len()).expect("wire length"), floor);
            assert_eq!(post_query(&fixture.router, request_bytes).await, expected);
        }
        assert_eq!(fixture.metrics.schema_exports.load(Ordering::SeqCst), 8);
        assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 4);
        assert_eq!(fixture.metrics.transaction_closes.load(Ordering::SeqCst), 4);
        assert_eq!(fixture.metrics.query_executions.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn rejected_query_shapes_never_open_a_data_transaction() {
        let malformed = route_fixture(
            [CURRENT_SCHEMA],
            query_plan_capability_vocabulary(),
            default_pipeline(),
        );
        let body = post_query(&malformed.router, b"{".to_vec()).await;
        assert_eq!(
            failure_code(&malformed.query, &body),
            "malformed_canonical_json"
        );
        assert_eq!(
            malformed.metrics.transaction_opens.load(Ordering::SeqCst),
            0,
        );
        assert_eq!(malformed.metrics.schema_exports.load(Ordering::SeqCst), 0,);

        let forged_owner = route_fixture(
            [CURRENT_SCHEMA],
            query_plan_capability_vocabulary(),
            default_pipeline(),
        );
        let foreign_signer = RemoteReplySigningKey::from_secret_bytes([0x91; 32]);
        let foreign_advertisement = RemoteCapabilities::new(
            query_plan_capability_vocabulary(),
            RemoteExecutorBinding::new("foreign-executor", "foreign-executor-epoch-0001")
                .expect("foreign executor binding"),
            foreign_signer.public_key(),
        );
        let authority = authority_fixture(false);
        let (foreign_request, _, _) = validated_request(
            &authority,
            &foreign_advertisement,
            "server-foreign-owner-nonce-01",
        );
        let body = post_query(&forged_owner.router, foreign_request).await;
        assert_eq!(
            failure_code(&forged_owner.query, &body),
            "query_remote_executor_mismatch",
        );
        assert_eq!(
            forged_owner
                .metrics
                .transaction_opens
                .load(Ordering::SeqCst),
            0,
        );
        assert_eq!(
            forged_owner.metrics.schema_exports.load(Ordering::SeqCst),
            0,
        );

        let forged_v2 = route_fixture_with_replay_store(
            [CURRENT_SCHEMA],
            query_plan_capability_vocabulary(),
            default_pipeline(),
            Arc::new(InMemoryReplayStore::new(0)),
        );
        let foreign_signer = RemoteReplySigningKey::from_secret_bytes([0x92; 32]);
        let foreign_advertisement = RemoteCapabilities::new(
            forged_v2.advertisement.capabilities().clone(),
            RemoteExecutorBinding::new("foreign-v2-executor", "foreign-v2-executor-epoch-0001")
                .expect("foreign V2 executor binding"),
            foreign_signer.public_key(),
        );
        let authority = authority_fixture(false);
        let (foreign_request, _, _, _, _) = validated_request_v2(
            &authority,
            &foreign_advertisement,
            "server-foreign-v2-owner-nonce-0001",
        );
        let body = post_query(&forged_v2.router, foreign_request).await;
        let signed: serde_json::Value =
            serde_json::from_slice(&body).expect("signed foreign-owner V2 failure");
        assert_eq!(
            signed["payload"]["code"],
            serde_json::json!("query_remote_v2_advertisement_mismatch"),
            "foreign V2 ownership rejects before the zero-capacity replay store"
        );
        assert_eq!(forged_v2.metrics.schema_exports.load(Ordering::SeqCst), 0);
        assert_eq!(
            forged_v2.metrics.transaction_opens.load(Ordering::SeqCst),
            0
        );
        assert_eq!(forged_v2.metrics.query_executions.load(Ordering::SeqCst), 0);

        let unsupported = route_fixture([CURRENT_SCHEMA], CapabilitySet::new(), default_pipeline());
        let body = post_query(&unsupported.router, unsupported.request.clone()).await;
        assert_eq!(
            failure_code(&unsupported.query, &body),
            "query_remote_capability_unsupported"
        );
        assert_eq!(
            unsupported.metrics.transaction_opens.load(Ordering::SeqCst),
            0,
        );
        assert_eq!(unsupported.metrics.schema_exports.load(Ordering::SeqCst), 0,);

        let invalid = route_fixture(
            [CURRENT_SCHEMA],
            query_plan_capability_vocabulary(),
            default_pipeline(),
        );
        let request = String::from_utf8(invalid.request.clone()).expect("request JSON");
        let request = request.replace("\"rows\":[]", "\"rows\":[[]]");
        assert_ne!(request.as_bytes(), invalid.request.as_slice());
        let body = post_query(&invalid.router, request.into_bytes()).await;
        assert_eq!(
            failure_code(&invalid.query, &body),
            "query_invocation_unexpected_inputs"
        );
        assert_eq!(invalid.metrics.transaction_opens.load(Ordering::SeqCst), 0,);
        assert_eq!(invalid.metrics.schema_exports.load(Ordering::SeqCst), 0);

        let stale_plan = route_fixture(
            [CURRENT_SCHEMA],
            query_plan_capability_vocabulary(),
            default_pipeline(),
        );
        let stale_authority = authority_fixture(true);
        let (request, _, _) = validated_request(
            &stale_authority,
            &stale_plan.advertisement,
            "server-stale-plan-nonce-0001",
        );
        let body = post_query(&stale_plan.router, request).await;
        assert_eq!(
            failure_code(&stale_plan.query, &body),
            "query_plan_managed_semantic_mismatch",
        );
        assert_eq!(
            stale_plan.metrics.transaction_opens.load(Ordering::SeqCst),
            0,
        );
        assert_eq!(stale_plan.metrics.schema_exports.load(Ordering::SeqCst), 0,);

        let stale_live = route_fixture(
            [STALE_SCHEMA],
            query_plan_capability_vocabulary(),
            default_pipeline(),
        );
        let body = post_query(&stale_live.router, stale_live.request.clone()).await;
        assert_eq!(
            failure_code(&stale_live.query, &body),
            "query_remote_stale_schema"
        );
        assert_eq!(
            stale_live.metrics.transaction_opens.load(Ordering::SeqCst),
            0,
        );
        assert_eq!(stale_live.metrics.schema_exports.load(Ordering::SeqCst), 1,);
        assert_eq!(
            stale_live.metrics.query_executions.load(Ordering::SeqCst),
            0,
        );
    }

    #[tokio::test]
    async fn replay_is_rejected_before_live_schema_or_data_transaction_work() {
        let policy_metrics = Arc::new(PolicyMetrics::default());
        let fixture = route_fixture(
            [CURRENT_SCHEMA],
            query_plan_capability_vocabulary(),
            pipeline_with_policy(Arc::clone(&policy_metrics), false),
        );
        fixture
            .query
            .replay_store
            .reserve(
                fixture.advertisement.executor().epoch(),
                fixture.nonce,
                Instant::now() + Duration::from_secs(60),
            )
            .await
            .expect("pre-reserve nonce");
        let body = post_query(&fixture.router, fixture.request.clone()).await;
        assert_eq!(failure_code(&fixture.query, &body), "query_remote_replay");
        assert_eq!(fixture.metrics.schema_exports.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 0,);
        assert_eq!(fixture.metrics.query_executions.load(Ordering::SeqCst), 0,);
        assert_eq!(
            policy_metrics.requests.load(Ordering::SeqCst),
            1,
            "released V1 plan-policy-before-replay precedence remains unchanged"
        );
        assert_eq!(
            policy_metrics.transports.load(Ordering::SeqCst),
            1,
            "pre-body transport authentication still protects the endpoint"
        );

        let rejecting_metrics = Arc::new(PolicyMetrics::default());
        let rejecting = route_fixture(
            [CURRENT_SCHEMA],
            query_plan_capability_vocabulary(),
            pipeline_with_rejecting_request_policy(Arc::clone(&rejecting_metrics)),
        );
        rejecting
            .query
            .replay_store
            .reserve(
                rejecting.advertisement.executor().epoch(),
                rejecting.nonce,
                Instant::now() + Duration::from_secs(60),
            )
            .await
            .expect("pre-reserve rejecting V1 nonce");
        let expected = RemoteQueryFailure::bound(
            rejecting.nonce.to_owned(),
            &rejecting.request_fingerprint,
            &policy_rejected(),
        )
        .encode_signed_or_fallback(
            &rejecting.query.advertisement_fingerprint,
            &rejecting.query.signer,
        );
        let body = post_query(&rejecting.router, rejecting.request.clone()).await;
        assert_eq!(
            body, expected,
            "released V1 policy rejection bytes retain precedence over replay"
        );
        assert_eq!(rejecting_metrics.requests.load(Ordering::SeqCst), 1);
        assert_eq!(rejecting.metrics.schema_exports.load(Ordering::SeqCst), 0);
        assert_eq!(
            rejecting.metrics.transaction_opens.load(Ordering::SeqCst),
            0
        );
    }

    #[tokio::test]
    async fn authority_is_rechecked_across_one_data_snapshot_before_execution() {
        let stable = route_fixture(
            [CURRENT_SCHEMA, CURRENT_SCHEMA],
            query_plan_capability_vocabulary(),
            default_pipeline(),
        );
        let body = post_query(&stable.router, stable.request.clone()).await;
        assert!(matches!(
            decode_remote_reply(
                &body,
                stable.nonce,
                &stable.plan_fingerprint,
                &stable.request_fingerprint,
                &stable.query.advertisement_fingerprint,
                stable.advertisement.reply_key(),
                type_bridge_contract::query_remote::RemoteReplyDecodeLimits {
                    shape: type_bridge_contract::query_remote::RemoteOutcomeShape::Rows {
                        width: 1,
                    },
                    max_bytes: 1 << 20,
                    max_items: 100,
                    max_collection_members: 1 << 16,
                },
                &Ed25519RemoteReplyVerifier,
            )
            .expect("bound successful reply"),
            RemoteReply::Response(_),
        ));
        assert_eq!(stable.metrics.schema_exports.load(Ordering::SeqCst), 2);
        assert_eq!(stable.metrics.transaction_opens.load(Ordering::SeqCst), 1);
        assert_eq!(stable.metrics.transaction_closes.load(Ordering::SeqCst), 1);
        assert_eq!(stable.metrics.query_executions.load(Ordering::SeqCst), 1);

        let changed = route_fixture(
            [CURRENT_SCHEMA, STALE_SCHEMA],
            query_plan_capability_vocabulary(),
            default_pipeline(),
        );
        changed.metrics.fail_close.store(true, Ordering::SeqCst);
        let body = post_query(&changed.router, changed.request.clone()).await;
        let failure = match decode_remote_reply(
            &body,
            changed.nonce,
            &changed.plan_fingerprint,
            &changed.request_fingerprint,
            &changed.query.advertisement_fingerprint,
            changed.advertisement.reply_key(),
            type_bridge_contract::query_remote::RemoteReplyDecodeLimits {
                shape: type_bridge_contract::query_remote::RemoteOutcomeShape::Rows { width: 1 },
                max_bytes: 1 << 20,
                max_items: 100,
                max_collection_members: 1 << 16,
            },
            &Ed25519RemoteReplyVerifier,
        )
        .expect("bound stale reply")
        {
            RemoteReply::Failure(failure) => failure,
            RemoteReply::Response(_) => panic!("stale post-open authority executed"),
        };
        assert_eq!(
            failure
                .diagnostic()
                .expect("stale diagnostic")
                .code()
                .as_str(),
            "query_remote_stale_schema",
        );
        assert_eq!(changed.metrics.schema_exports.load(Ordering::SeqCst), 2);
        assert_eq!(changed.metrics.transaction_opens.load(Ordering::SeqCst), 1);
        assert_eq!(changed.metrics.transaction_closes.load(Ordering::SeqCst), 1);
        assert_eq!(changed.metrics.query_executions.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn close_failure_is_warning_only_and_preserves_the_primary_success_reply() {
        let fixture = route_fixture(
            [CURRENT_SCHEMA, CURRENT_SCHEMA],
            query_plan_capability_vocabulary(),
            default_pipeline(),
        );
        fixture.metrics.fail_close.store(true, Ordering::SeqCst);
        let body = post_query(&fixture.router, fixture.request.clone()).await;
        assert!(matches!(
            decode_remote_reply(
                &body,
                fixture.nonce,
                &fixture.plan_fingerprint,
                &fixture.request_fingerprint,
                &fixture.query.advertisement_fingerprint,
                fixture.advertisement.reply_key(),
                type_bridge_contract::query_remote::RemoteReplyDecodeLimits {
                    shape: type_bridge_contract::query_remote::RemoteOutcomeShape::Rows {
                        width: 1,
                    },
                    max_bytes: 1 << 20,
                    max_items: 100,
                    max_collection_members: 1 << 16,
                },
                &Ed25519RemoteReplyVerifier,
            )
            .expect("close failure must not replace the bound primary reply"),
            RemoteReply::Response(_),
        ));
        assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.metrics.transaction_closes.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.metrics.query_executions.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn provider_failure_closes_once_and_close_failure_preserves_exact_bound_bytes() {
        let fixture = route_fixture(
            [CURRENT_SCHEMA, CURRENT_SCHEMA],
            query_plan_capability_vocabulary(),
            default_pipeline(),
        );
        fixture.metrics.fail_query.store(true, Ordering::SeqCst);
        fixture.metrics.fail_close.store(true, Ordering::SeqCst);
        let body = post_query(&fixture.router, fixture.request.clone()).await;
        let expected = RemoteQueryFailure::bound(
            fixture.nonce.to_owned(),
            &fixture.request_fingerprint,
            &diagnostic(
                DiagnosticCategory::Integrity,
                "query_remote_provider_failed",
                "the executor provider call failed",
            ),
        )
        .encode_signed_or_fallback(
            &fixture.query.advertisement_fingerprint,
            &fixture.query.signer,
        );
        assert_eq!(body, expected);
        assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.metrics.transaction_closes.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.metrics.query_executions.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn capability_discovery_runs_v2_transport_policy_and_audit_before_export() {
        let policy_metrics = Arc::new(PolicyMetrics::default());
        let model_vocabulary = query_plan_v2_capability_vocabulary();
        let fixture = route_fixture(
            [CURRENT_SCHEMA],
            model_vocabulary.clone(),
            pipeline_with_policy(Arc::clone(&policy_metrics), false),
        );
        assert!(
            model_vocabulary
                .iter()
                .all(|capability| fixture.advertisement.capabilities().contains(capability)),
            "the production model executor advertises the complete V2 plan vocabulary",
        );
        let expected = fixture.advertisement.encode().expect("advertisement bytes");
        let response = fixture
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v2/capabilities")
                    .header("authorization", "Bearer test")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("capability response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("capability body")
            .to_bytes();
        assert_eq!(body.as_ref(), expected.as_slice());
        assert_eq!(policy_metrics.transports.load(Ordering::SeqCst), 1);
        assert_eq!(
            policy_metrics.requests.load(Ordering::SeqCst),
            0,
            "discovery has no query plan to pass to plan authorization",
        );
        assert_eq!(policy_metrics.responses.load(Ordering::SeqCst), 1);
        assert_eq!(
            policy_metrics
                .outcomes
                .lock()
                .expect("policy outcomes")
                .as_slice(),
            [("ok".to_owned(), expected.len())],
        );
        assert_eq!(fixture.metrics.schema_exports.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.metrics.transaction_opens.load(Ordering::SeqCst), 0);

        let rejected_policy = Arc::new(PolicyMetrics::default());
        let rejected = route_fixture(
            [CURRENT_SCHEMA],
            query_plan_capability_vocabulary(),
            pipeline_with_policy(Arc::clone(&rejected_policy), true),
        );
        let response = rejected
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v2/capabilities")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("capability rejection");
        let body = response
            .into_body()
            .collect()
            .await
            .expect("rejection body")
            .to_bytes();
        assert_eq!(
            failure_code(&rejected.query, &body),
            "query_remote_policy_rejected"
        );
        assert_eq!(rejected_policy.transports.load(Ordering::SeqCst), 1);
        assert_eq!(rejected_policy.responses.load(Ordering::SeqCst), 1);
        assert_eq!(rejected.metrics.schema_exports.load(Ordering::SeqCst), 0);
        assert_eq!(rejected.metrics.transaction_opens.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn one_absolute_deadline_bounds_every_async_transport_stage() {
        let deadline = Instant::now() + Duration::from_millis(10);
        let started = Instant::now();
        let result = await_before_deadline(deadline, std::future::pending::<()>()).await;
        assert!(result.is_none());
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "a pending policy/provider stage must not outlive the round-trip deadline"
        );
        assert_eq!(
            await_before_deadline(Instant::now() + Duration::from_secs(1), async { 7_u8 }).await,
            Some(7),
        );
    }

    #[tokio::test]
    async fn replay_store_is_atomic_and_global_within_an_executor_epoch() {
        let store = Arc::new(InMemoryReplayStore::new(32));
        let expiry = Instant::now() + Duration::from_secs(60);
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let store = Arc::clone(&store);
            tasks.push(tokio::spawn(async move {
                store
                    .reserve("executor-epoch-a", "same-nonce", expiry)
                    .await
            }));
        }
        let mut admitted = 0;
        let mut replayed = 0;
        for task in tasks {
            match task.await.expect("reservation task") {
                Ok(()) => admitted += 1,
                Err(ReplayStoreError::Replayed) => replayed += 1,
                other => panic!("unexpected reservation result: {other:?}"),
            }
        }
        assert_eq!(admitted, 1);
        assert_eq!(replayed, 15);
        assert_eq!(
            store
                .reserve("executor-epoch-a", "same-nonce", expiry)
                .await,
            Err(ReplayStoreError::Replayed),
            "different authenticated clients cannot replay one captured request",
        );
        store
            .reserve("executor-epoch-b", "same-nonce", expiry)
            .await
            .expect("a separately advertised epoch has its own nonce namespace");
    }

    #[test]
    fn standalone_restart_rotates_executor_identity_and_epoch() {
        let first = standalone_executor_binding();
        let restarted = standalone_executor_binding();
        assert_ne!(first.identity(), restarted.identity());
        assert_ne!(first.epoch(), restarted.epoch());

        let capabilities = CapabilitySet::new();
        let first_signer = RemoteReplySigningKey::from_secret_bytes([0x31; 32]);
        let restarted_signer = RemoteReplySigningKey::from_secret_bytes([0x32; 32]);
        let first = RemoteCapabilities::new(capabilities.clone(), first, first_signer.public_key())
            .fingerprint()
            .expect("first advertisement fingerprint");
        let restarted =
            RemoteCapabilities::new(capabilities, restarted, restarted_signer.public_key())
                .fingerprint()
                .expect("restarted advertisement fingerprint");
        assert_ne!(first, restarted);
    }

    #[test]
    fn oversized_executor_capability_set_is_a_typed_construction_error() {
        let authority = authority_fixture(false);
        let metrics = Arc::new(BackendMetrics::default());
        let database = Database::with_backend(
            Box::new(CountingBackend::new([], metrics)),
            "server-v2-capability-limit",
        );
        let mut advertised = CapabilitySet::new();
        for index in 0..=MAX_CANONICAL_COLLECTION_LEN {
            advertised.insert(
                CapabilityId::new(format!("query.generated-{index}"))
                    .expect("generated capability ID"),
            );
        }

        let error = match V2QueryState::new_query_only(
            advertised,
            QueryV2AnswerLimits::default(),
            database,
            authority.declared,
            authority.delta_context,
            authority.managed,
            authority.resolved,
        ) {
            Ok(_) => panic!("oversized advertisement must not construct state"),
            Err(error) => error,
        };
        assert_eq!(error.code().as_str(), "canonical_collection_too_large");
    }

    #[test]
    fn malformed_internal_reply_is_bound_failure_never_success() {
        let fixture = route_fixture(
            [CURRENT_SCHEMA],
            query_plan_capability_vocabulary(),
            default_pipeline(),
        );
        let request = type_bridge_contract::query_remote::RemoteRequestFingerprint::compute(
            b"canonical-request-fixture",
        )
        .expect("request fingerprint");
        let nonce = "internal-reply-nonce-000001";
        let (bytes, success, code) =
            bound_internal_response_failure(&fixture.query, nonce, &request);
        assert!(!success);
        assert_eq!(code, "query_remote_internal_response_invalid");
        let failure = type_bridge_contract::query_remote::decode_signed_remote_failure(
            &bytes,
            &fixture.query.advertisement_fingerprint,
            fixture.advertisement.reply_key(),
            u64::try_from(type_bridge_contract::limits::MAX_REMOTE_ENVELOPE_BYTES)
                .expect("wire ceiling"),
            &Ed25519RemoteReplyVerifier,
        )
        .expect("bound failure");
        failure
            .verify_binding(nonce, &request)
            .expect("internal failure binding");
        assert_eq!(
            failure.diagnostic().expect("diagnostic").code().as_str(),
            "query_remote_internal_response_invalid",
        );
    }

    #[test]
    fn response_audit_grace_is_small_and_independent_of_request_deadline() {
        let started = Instant::now();
        let audit = response_audit_deadline(started + Duration::from_secs(30));
        let grace = audit.duration_since(started);
        assert!(grace >= RESPONSE_AUDIT_GRACE);
        assert!(grace < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn replay_store_expires_entries_and_fails_closed_at_capacity() {
        let store = InMemoryReplayStore::new(1);
        store
            .reserve("executor-epoch", "expired", Instant::now())
            .await
            .expect("first reservation");
        store
            .reserve(
                "executor-epoch",
                "replacement",
                Instant::now() + Duration::from_secs(60),
            )
            .await
            .expect("expired entries are removed");
        assert_eq!(
            store
                .reserve(
                    "executor-epoch",
                    "another",
                    Instant::now() + Duration::from_secs(60),
                )
                .await,
            Err(ReplayStoreError::Capacity),
        );
    }

    #[tokio::test]
    async fn replay_store_ignores_a_stale_expiry_index_entry() {
        let store = InMemoryReplayStore::new(2);
        let key = ("executor-epoch".to_owned(), "still-live".to_owned());
        let live_until = Instant::now() + Duration::from_secs(60);
        {
            let mut reservations = store.reservations.lock().expect("replay store lock");
            reservations.by_key.insert(key.clone(), live_until);
            reservations
                .by_expiry
                .push(Reverse((Instant::now(), key.clone())));
            reservations.by_expiry.push(Reverse((live_until, key)));
        }

        store
            .reserve(
                "executor-epoch",
                "other",
                Instant::now() + Duration::from_secs(60),
            )
            .await
            .expect("stale expiry metadata does not consume capacity");
        assert_eq!(
            store
                .reserve("executor-epoch", "still-live", live_until)
                .await,
            Err(ReplayStoreError::Replayed),
            "an obsolete expiry record must not remove the current reservation",
        );
    }

    #[test]
    fn request_policy_metadata_retains_textual_transport_credentials() {
        let mut headers = HeaderMap::new();
        headers.append("authorization", "Bearer first".parse().unwrap());
        headers.append("authorization", "Bearer second".parse().unwrap());
        let metadata = v2_request_metadata(&headers, "query");
        assert_eq!(metadata["transport"], "v2");
        assert_eq!(metadata["v2_endpoint"], "query");
        assert_eq!(
            metadata["http_headers"]["authorization"],
            serde_json::json!(["Bearer first", "Bearer second"]),
        );
    }
}
