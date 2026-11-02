//! Shared authority, replay protection and framing for staged requests.

#![allow(deprecated)] // Legacy node bridges are removed in 3.0.
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::http::HeaderMap;
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::query_remote::{
    RemoteCapabilities, RemoteCapabilitiesFingerprint, RemoteExecutorBinding,
};
use type_bridge_contract::query_remote_v2::{
    CAP_QUERY_REMOTE_STATEMENT_LIMIT, query_remote_v2_required_capabilities,
};
use type_bridge_contract::schema::{DeclaredSchema, DocumentId};
use type_bridge_contract::schema_delta::ManagedSchemaState;
use type_bridge_orm::OwnedTransaction as Transaction;
use type_bridge_orm::query_v2_prepared::{
    acquire_live_authority_rebuild_permit, verify_managed_query_control_scope,
};
#[cfg(test)]
use type_bridge_orm::query_v2_remote::Ed25519RemoteReplyVerifier;
use type_bridge_orm::query_v2_remote::RemoteReplySigningKey;
use type_bridge_orm::session::backend::{AnswerLimits, MAX_QUERY_V2_SCHEMA_FENCE_DURATION};
use type_bridge_orm::session::database::Database;
use type_bridge_schema::{ManagedDeltaContext, ResolvedSchema};
use type_bridge_schema_compat::{LiveQueryControlPresence, rebuild_live_query_authority};

const DEFAULT_EXECUTION_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_REPLAY_CAPACITY: usize = 100_000;
const RESPONSE_AUDIT_GRACE: Duration = Duration::from_millis(100);
const TRANSACTION_CLOSE_GRACE: Duration = Duration::from_secs(1);
const STAGED_PREFLIGHT_CONCURRENCY: usize = 4;

fn new_staged_preflight_slots() -> Arc<tokio::sync::Semaphore> {
    Arc::new(tokio::sync::Semaphore::new(STAGED_PREFLIGHT_CONCURRENCY))
}

pub(super) static STAGED_PREFLIGHT_SLOTS: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(new_staged_preflight_slots);

pub(super) async fn await_before_deadline<F>(deadline: Instant, future: F) -> Option<F::Output>
where
    F: Future,
{
    tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), future)
        .await
        .ok()
}

pub(super) async fn close_staged_transaction(transaction: &mut Transaction) -> Result<(), ()> {
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
/// shared executor epoch through [`StagedQueryState::with_shared_execution_epoch`]
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

/// Authority and execution state shared by staged requests.
pub struct StagedQueryState {
    pub(super) advertisement: RemoteCapabilities,
    pub(super) staged_query_limits: Option<type_bridge_contract::limits::QueryLimits>,
    advertisement_fingerprint: RemoteCapabilitiesFingerprint,
    pub(super) ceilings: AnswerLimits,
    pub(super) database: Database,
    pub(super) declared: DeclaredSchema,
    delta_context: ManagedDeltaContext,
    managed: ManagedSchemaState,
    _resolved: ResolvedSchema,
    pub(super) execution_timeout: Duration,
    pub(super) replay_store: Arc<dyn RemoteReplayStore>,
    pub(super) signer: RemoteReplySigningKey,
    control_policy: StagedQueryControlPolicy,
    pub(super) allow_unauthenticated_staged_writes: bool,
}

#[derive(Clone, Copy)]
enum StagedQueryControlPolicy {
    Managed,
    QueryOnly,
}

impl StagedQueryState {
    /// Construct fail-closed V2 state with mandatory execution/replay bounds.
    pub fn new(
        advertised: CapabilitySet,
        ceilings: AnswerLimits,
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
            StagedQueryControlPolicy::Managed,
        )
    }

    /// Construct fail-closed V2 state for an explicitly query-only database.
    ///
    /// This mode rejects both V2 and legacy migration-control partitions. It
    /// cannot be inferred from missing control facts on a managed database.
    pub fn new_query_only(
        advertised: CapabilitySet,
        ceilings: AnswerLimits,
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
            StagedQueryControlPolicy::QueryOnly,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_control_policy(
        mut advertised: CapabilitySet,
        ceilings: AnswerLimits,
        database: Database,
        declared: DeclaredSchema,
        delta_context: ManagedDeltaContext,
        managed: ManagedSchemaState,
        resolved: ResolvedSchema,
        control_policy: StagedQueryControlPolicy,
    ) -> Result<Self, Diagnostic> {
        // Model evidence is constructed by the sole same-snapshot
        // compatibility executor. Its graph, attribute-value, and role-player
        // ceilings are fixed at the canonical collection ceiling; request
        // values may only tighten them during admission.
        for capability in query_remote_v2_required_capabilities(false) {
            advertised.insert(capability);
        }
        advertised.insert(
            type_bridge_contract::capability::CapabilityId::new(CAP_QUERY_REMOTE_STATEMENT_LIMIT)
                .expect("static remote statement-limit capability is canonical"),
        );
        let executor = standalone_executor_binding();
        let signer = RemoteReplySigningKey::generate();
        let advertisement = RemoteCapabilities::new(advertised, executor, signer.public_key());
        let advertisement_fingerprint = advertisement.fingerprint()?;
        Ok(Self {
            advertisement,
            staged_query_limits: None,
            advertisement_fingerprint,
            ceilings,
            database,
            declared,
            delta_context,
            managed,
            _resolved: resolved,
            execution_timeout: DEFAULT_EXECUTION_TIMEOUT,
            replay_store: Arc::new(InMemoryReplayStore::new(DEFAULT_REPLAY_CAPACITY)),
            signer,
            control_policy,
            allow_unauthenticated_staged_writes: false,
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

    /// Advertise and enforce `limits` instead of the canonical query limits.
    ///
    /// The advertisement is the executor's only statement of its limits and
    /// admission reads them back from it, so what a client is told and what
    /// this executor enforces cannot diverge.
    pub fn with_query_limits(
        mut self,
        limits: type_bridge_contract::limits::QueryLimits,
    ) -> Result<Self, Diagnostic> {
        let advertisement = self.advertisement.clone().with_query_limits(limits)?;
        self.advertisement_fingerprint = advertisement.fingerprint()?;
        self.advertisement = advertisement;
        Ok(self)
    }

    /// Set V3 structural limits independently of the frozen V2 advertisement.
    pub fn with_staged_query_limits(
        mut self,
        limits: type_bridge_contract::limits::QueryLimits,
    ) -> Result<Self, Diagnostic> {
        let limits = limits.validate_v3().map_err(|_| {
            diagnostic(
                DiagnosticCategory::ResourceLimit,
                "query_remote_v3_query_limits",
                "staged query limits exceed their supported range",
            )
        })?;
        self.staged_query_limits = Some(limits);
        Ok(self)
    }

    /// Advertise and enforce `ceilings` instead of the default answer
    /// policy: how much one query may return through this executor.
    ///
    /// As with the query limits, the advertisement is the statement admission
    /// reads back. The hosting process's provider ceilings follow it, so a
    /// raised policy is not cut short by an unrelated default.
    pub fn with_answer_limits(
        mut self,
        ceilings: type_bridge_orm::QueryExecutionResourceLimits,
    ) -> Result<Self, Diagnostic> {
        let advertisement = self
            .advertisement
            .clone()
            .with_answer_limits(ceilings.overrides())?;
        self.advertisement_fingerprint = advertisement.fingerprint()?;
        self.advertisement = advertisement;
        self.ceilings.answer.max_items = ceilings.items;
        self.ceilings.answer.max_bytes = ceilings.bytes;
        self.ceilings.max_collection_members = ceilings.collection_members;
        Ok(self)
    }

    /// Largest HTTP body the collector reads: the advertised request budget
    /// plus framing slack.
    pub(super) fn body_limit_bytes(&self) -> usize {
        self.advertisement
            .query_limits()
            .remote_request_bytes
            .saturating_add(STAGED_BODY_FRAMING_SLACK_BYTES)
    }

    /// Admit staged write transactions that no interceptor vouches for.
    ///
    /// Off by default: a staged write otherwise needs a policy answering
    /// [`StagedWriteAuthority::Authenticated`](crate::interceptor::StagedWriteAuthority).
    #[must_use]
    pub fn with_unauthenticated_staged_writes(mut self, allow: bool) -> Self {
        self.allow_unauthenticated_staged_writes = allow;
        self
    }

    /// Tighten the mandatory server execution duration.
    #[must_use]
    pub fn with_execution_timeout(mut self, timeout: Duration) -> Self {
        self.execution_timeout = timeout;
        self
    }

    pub(super) async fn verify_live_authority(&self) -> Result<(), Diagnostic> {
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
        let observed_profile = type_bridge_contract::fingerprint::SemanticProfileId::new(
            type_bridge_core_lib::version::semantic_profile_id(&server_version).ok_or_else(
                || {
                    diagnostic(
                        DiagnosticCategory::Integrity,
                        "query_remote_server_profile_unsupported",
                        "the executor TypeDB version has no supported semantic profile",
                    )
                },
            )?,
        )?;
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
            (StagedQueryControlPolicy::Managed, LiveQueryControlPresence::ManagedFence)
            | (StagedQueryControlPolicy::QueryOnly, LiveQueryControlPresence::Absent) => {}
            (StagedQueryControlPolicy::Managed, LiveQueryControlPresence::Absent) => {
                return Err(diagnostic(
                    DiagnosticCategory::Integrity,
                    "query_remote_managed_control_missing",
                    "the managed executor database has no migration control authority",
                ));
            }
            (StagedQueryControlPolicy::QueryOnly, LiveQueryControlPresence::ManagedFence) => {
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
        if matches!(self.control_policy, StagedQueryControlPolicy::QueryOnly)
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
            if matches!(self.control_policy, StagedQueryControlPolicy::Managed) {
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
        let closed = close_staged_transaction(&mut transaction).await;
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

/// Framing slack the HTTP body collector allows beyond the request budget.
pub(super) const STAGED_BODY_FRAMING_SLACK_BYTES: usize = 64 * 1024;

pub(super) fn declared_staged_body_oversized(headers: &HeaderMap, limit_bytes: usize) -> bool {
    let Ok(limit) = u64::try_from(limit_bytes) else {
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

pub(super) fn response_audit_deadline(request_deadline: Instant) -> Instant {
    Instant::now()
        .checked_add(RESPONSE_AUDIT_GRACE)
        .unwrap_or(request_deadline)
}

pub(super) fn staged_request_metadata(
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
        ("transport".to_owned(), serde_json::json!("v3")),
        ("v3_endpoint".to_owned(), serde_json::json!(endpoint)),
        ("query_format".to_owned(), serde_json::json!("typed-plan")),
        ("http_headers".to_owned(), serde_json::Value::Object(values)),
    ])
}

pub(super) fn envelope_response(bytes: Vec<u8>) -> Response {
    ([(CONTENT_TYPE, "application/json")], bytes).into_response()
}

pub(super) fn diagnostic(
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

pub(super) fn unavailable() -> Diagnostic {
    diagnostic(
        DiagnosticCategory::Integrity,
        "query_remote_provider_unavailable",
        "the executor could not open a read transaction",
    )
}

pub(super) fn policy_rejected() -> Diagnostic {
    diagnostic(
        DiagnosticCategory::Integrity,
        "query_remote_policy_rejected",
        "the executor policy rejected the request",
    )
}

pub(super) fn replayed() -> Diagnostic {
    diagnostic(
        DiagnosticCategory::Integrity,
        "query_remote_replay",
        "the request nonce was already consumed",
    )
}

pub(super) fn replay_capacity() -> Diagnostic {
    diagnostic(
        DiagnosticCategory::ResourceLimit,
        "query_remote_replay_capacity",
        "the replay registry cannot admit another request",
    )
}

pub(super) fn replay_unavailable() -> Diagnostic {
    diagnostic(
        DiagnosticCategory::Integrity,
        "query_remote_replay_unavailable",
        "the replay registry could not prove nonce uniqueness",
    )
}

pub(super) fn deadline_exceeded() -> Diagnostic {
    diagnostic(
        DiagnosticCategory::ResourceLimit,
        "transaction_deadline_exceeded",
        "provider transaction deadline expired",
    )
}

pub(super) fn body_oversized() -> Diagnostic {
    diagnostic(
        DiagnosticCategory::ResourceLimit,
        "query_remote_envelope_oversized",
        "remote request envelope exceeds the transport byte ceiling",
    )
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use crate::interceptor::{RequestContext, V3PolicyOutcome};
    use axum::Router;
    use axum::body::Body;
    use axum::extract::Request;
    use axum::http::StatusCode;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use type_bridge_contract::capability::CapabilityId;
    use type_bridge_contract::codec::FormatVersion;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
    use type_bridge_contract::query_plan::QueryOutput;
    use type_bridge_contract::query_plan::QueryPattern;
    use type_bridge_contract::query_remote_v2::decode_signed_remote_failure_v2;
    use type_bridge_contract::schema::{
        OwnsFact, OwnsFactId, SchemaFact, SourceSpan, SourcedSchemaFact, TypeFact, ValueFact,
        ValueFactId,
    };
    use type_bridge_contract::value::ValueTypeTag;
    use type_bridge_orm::error::OrmError;
    use type_bridge_orm::session::backend::{
        BoxFuture, DriverBackend, QueryResult, SchemaFencedReadTransaction, TransactionOps, TxType,
    };
    use type_bridge_schema::{managed_schema_state, resolve};

    use crate::interceptor::{InterceptError, Interceptor};
    use crate::pipeline::{PipelineBuilder, QueryPipeline};

    use super::*;

    const CURRENT_SCHEMA: &str = "define\nattribute transport-name, value string;\nentity transport-person, owns transport-name;";
    const TEST_NONCE: &str = "server-route-nonce-000001";

    #[derive(Default)]
    struct BackendMetrics {
        fail_close: AtomicBool,
        fail_commit: AtomicBool,
        fail_query: AtomicBool,
        commits: AtomicUsize,
        schema_exports: AtomicUsize,
        transaction_opens: AtomicUsize,
        transaction_closes: AtomicUsize,
        query_executions: AtomicUsize,
        /// While set, every commit waits for `release_commit`.
        hold_commit: AtomicBool,
        release_commit: tokio::sync::Notify,
        /// Rows every query answers with; empty by default.
        rows: Mutex<Vec<serde_json::Value>>,
        documents: AtomicBool,
        staged_insert_rows: AtomicBool,
    }

    struct CountingBackend {
        fallback_schema: String,
        metrics: Arc<BackendMetrics>,
        observations: Mutex<VecDeque<String>>,
        server_version: type_bridge_core_lib::version::Version,
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
                server_version: type_bridge_core_lib::version::Version::new(3, 12, 1),
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
            Some(self.server_version)
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
        fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            self.metrics.query_executions.fetch_add(1, Ordering::SeqCst);
            let fail = self.metrics.fail_query.load(Ordering::SeqCst);
            let mut rows = self.metrics.rows.lock().expect("rows lock").clone();
            if rows.is_empty()
                && self.metrics.staged_insert_rows.load(Ordering::SeqCst)
                && typeql.split_whitespace().any(|token| token == "insert")
            {
                rows.push(serde_json::json!({"person": {"kind": "thing", "type_id": {"kind": "entity", "label": "transport-person"}, "iid": "0x1e"}}));
            }
            let documents = self.metrics.documents.load(Ordering::SeqCst);
            Box::pin(async move {
                if fail {
                    Err(OrmError::QueryExecution("masked query fixture".into()))
                } else {
                    Ok(if documents {
                        QueryResult::Documents(rows)
                    } else {
                        QueryResult::Rows(rows)
                    })
                }
            })
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        /// A failing commit reports the provider's redacted STC2 rendering.
        fn commit_classified(
            &mut self,
        ) -> BoxFuture<'_, Result<(), type_bridge_orm::ClassifiedCommitError>> {
            if self.metrics.fail_commit.load(Ordering::SeqCst) {
                return Box::pin(async {
                    Err(type_bridge_orm::ClassifiedCommitError::Driver {
                        certainty: type_bridge_orm::CommitFailureCertainty::DefinitelyAborted,
                        message: "[STC2] TypeDB commit rejected: isolation conflict".into(),
                    })
                });
            }
            let hold = self.metrics.hold_commit.load(Ordering::SeqCst);
            let metrics = Arc::clone(&self.metrics);
            Box::pin(async move {
                if hold {
                    metrics.release_commit.notified().await;
                }
                metrics.commits.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
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

    fn default_pipeline() -> QueryPipeline {
        PipelineBuilder::new()
            .with_default_database("server-staged-route")
            .build()
            .expect("query pipeline")
    }
    mod staged {
        include!("v3_tests.rs");
    }
}
