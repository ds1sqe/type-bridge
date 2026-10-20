//! Standalone, process-local retention of signed staged replies.
//!
//! A route must authenticate the client, validate request freshness and reserve
//! before executing. It completes the reservation with a signed, request-bound
//! reply. This module opens no transactions and installs no route. A restart
//! loses all entries: the old executor advertisement must fail closed, and a
//! retry may be indeterminate. Never promise durable exactly-once execution.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::fingerprint::Fingerprint;
use type_bridge_contract::limits::MAX_REMOTE_ENVELOPE_BYTES;
use type_bridge_contract::query_remote::{
    MAX_REMOTE_DEADLINE_MS, QUERY_REMOTE_REQUEST_FINGERPRINT_DOMAIN,
};
use type_bridge_contract::query_remote_v3::{IdempotencyKey, QUERY_REMOTE_REQUEST_FORMAT_V3};

/// Standalone defaults: at most 100,000 live entries.
pub const DEFAULT_IDEMPOTENCY_CAPACITY: usize = 100_000;
/// Default combined retained/reply-reserved payload bytes: 256 MiB.
pub const DEFAULT_IDEMPOTENCY_BYTES: usize = 256 * 1024 * 1024;
/// Hard request lifetime; no store entry may outlive five minutes from admission.
pub const MAX_IDEMPOTENCY_TTL: Duration = Duration::from_millis(MAX_REMOTE_DEADLINE_MS);
/// Default per-client share: one client holds at most 1/4 of the live
/// entries and of the retained bytes. Under the default byte budget that is
/// one reply allowance of the full remote envelope in flight beside as many
/// bytes of retained replies, so a retained reply never blocks the client's
/// next default keyed request.
pub const DEFAULT_IDEMPOTENCY_CLIENT_SHARE: usize = 4;

/// Explicit memory and expiry configuration for the standalone store.
///
/// Build it with [`IdempotencyConfig::new`] or from [`Default`] and adjust
/// the public fields; [`IdempotencyStore::new`] validates the result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct IdempotencyConfig {
    /// Maximum live entries, including in-flight operations.
    pub capacity: usize,
    /// Maximum retained bytes plus reserved in-flight reply allowances.
    pub max_reply_bytes: usize,
    /// Maximum remaining request lifetime, at most five minutes.
    pub max_ttl: Duration,
    /// Per-client bound: one client may hold at most `1 / client_share` of
    /// `capacity` and of `max_reply_bytes` (at least one entry and one
    /// byte). `1` leaves only the store-wide bounds; `0` is invalid.
    pub client_share: usize,
}
impl IdempotencyConfig {
    /// A store bound of `capacity` live entries, `max_reply_bytes` retained
    /// plus reserved reply bytes and a `max_ttl` remaining lifetime.
    #[must_use]
    pub const fn new(capacity: usize, max_reply_bytes: usize, max_ttl: Duration) -> Self {
        Self {
            capacity,
            max_reply_bytes,
            max_ttl,
            client_share: DEFAULT_IDEMPOTENCY_CLIENT_SHARE,
        }
    }
    /// Bound each client to `1 / client_share` of the store.
    #[must_use]
    pub const fn with_client_share(mut self, client_share: usize) -> Self {
        self.client_share = client_share;
        self
    }
    fn client_capacity(&self) -> usize {
        (self.capacity / self.client_share).max(1)
    }
    fn client_reply_bytes(&self) -> usize {
        (self.max_reply_bytes / self.client_share).max(1)
    }
}
impl Default for IdempotencyConfig {
    fn default() -> Self {
        Self {
            capacity: DEFAULT_IDEMPOTENCY_CAPACITY,
            max_reply_bytes: DEFAULT_IDEMPOTENCY_BYTES,
            max_ttl: MAX_IDEMPOTENCY_TTL,
            client_share: DEFAULT_IDEMPOTENCY_CLIENT_SHARE,
        }
    }
}
/// Typed refusal; no client identity, key, query or reply bytes enter its text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum IdempotencyError {
    /// No live entry is evicted to make room for this request.
    Capacity,
    /// This client already holds its share of entries or retained bytes.
    ClientCapacity,
    /// The same exact request is still running.
    RetryLater,
    /// A live client/key pair belongs to different request bytes.
    KeyReused,
    /// Invalid identity, lifetime, fingerprint or reservation budget.
    InvalidRequest,
    /// Completion exceeds the allowance reserved before execution.
    ReplyTooLarge,
    /// Expired, foreign, or already completed reservation ticket.
    ReservationLost,
    /// The store cannot prove its atomic state after internal failure.
    Unavailable,
}
impl IdempotencyError {
    /// Stable wire diagnostic code for route integration.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Capacity => "idempotency_capacity",
            Self::ClientCapacity => "idempotency_client_capacity",
            Self::RetryLater => "retry_later",
            Self::KeyReused => "idempotency_key_reused",
            Self::InvalidRequest => "idempotency_request_invalid",
            Self::ReplyTooLarge => "idempotency_reply_limit",
            Self::ReservationLost => "idempotency_reservation_lost",
            Self::Unavailable => "idempotency_unavailable",
        }
    }
    /// Redacted typed diagnostic without request-owned text.
    pub fn diagnostic(self) -> Diagnostic {
        let (category, message) = match self {
            Self::Capacity => (
                DiagnosticCategory::ResourceLimit,
                "idempotency retention capacity is exhausted",
            ),
            Self::ClientCapacity => (
                DiagnosticCategory::ResourceLimit,
                "this client's share of idempotency retention is exhausted",
            ),
            Self::RetryLater => (
                DiagnosticCategory::ResourceLimit,
                "the same idempotent request is still in flight",
            ),
            Self::KeyReused => (
                DiagnosticCategory::Integrity,
                "an idempotency key is already bound to different request bytes",
            ),
            Self::InvalidRequest => (
                DiagnosticCategory::InvalidContract,
                "idempotency admission metadata is invalid",
            ),
            Self::ReplyTooLarge => (
                DiagnosticCategory::ResourceLimit,
                "signed reply exceeds its reserved retention allowance",
            ),
            Self::ReservationLost => (
                DiagnosticCategory::Integrity,
                "idempotency reservation is no longer owned by this completion",
            ),
            Self::Unavailable => (
                DiagnosticCategory::Integrity,
                "idempotency state is unavailable",
            ),
        };
        Diagnostic::new(
            category,
            type_bridge_contract::diagnostic::DiagnosticCode::new(self.code())
                .expect("static idempotency diagnostic code"),
            message,
        )
    }
}
impl std::fmt::Display for IdempotencyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for IdempotencyError {}

type StoreKey = (String, IdempotencyKey);
struct Entry {
    fingerprint: Fingerprint,
    expires: Instant,
    generation: u64,
    state: State,
}
enum State {
    InFlight {
        allowance: usize,
    },
    Done {
        reply: Arc<[u8]>,
        outcome: RetainedOutcome,
    },
}
impl Entry {
    fn charged_bytes(&self) -> usize {
        match &self.state {
            State::InFlight { allowance } => *allowance,
            State::Done { reply, .. } => reply.len(),
        }
    }
}
/// Value-free summary of a retained reply, kept beside its opaque signed
/// bytes so a replay is audited and policed with the outcome the first
/// caller received.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetainedOutcome {
    success: bool,
    code: DiagnosticCode,
}
impl RetainedOutcome {
    /// Summarize one delivered reply: whether it is the bound success, and
    /// its stable code (`ok` for success).
    #[must_use]
    pub const fn new(success: bool, code: DiagnosticCode) -> Self {
        Self { success, code }
    }
    /// Whether the retained reply is the successful bound response.
    #[must_use]
    pub const fn success(&self) -> bool {
        self.success
    }
    /// Stable diagnostic code of the retained reply, or `ok` for success.
    #[must_use]
    pub const fn code(&self) -> &DiagnosticCode {
        &self.code
    }
}
/// Live entries and charged bytes held by one client.
#[derive(Clone, Copy, Default)]
struct ClientUsage {
    entries: usize,
    bytes: usize,
}
#[derive(Default)]
struct Entries {
    by_key: BTreeMap<StoreKey, Entry>,
    expiry: BTreeSet<(Instant, StoreKey)>,
    charged_bytes: usize,
    by_client: BTreeMap<String, ClientUsage>,
    generation: u64,
}
impl Entries {
    /// `client`'s usage after releasing `entries` and `bytes`, or
    /// `Unavailable` if the accounting would underflow.
    fn client_release(
        &self,
        client: &str,
        entries: usize,
        bytes: usize,
    ) -> Result<ClientUsage, IdempotencyError> {
        let usage = self.by_client.get(client).copied().unwrap_or_default();
        Ok(ClientUsage {
            entries: usage
                .entries
                .checked_sub(entries)
                .ok_or(IdempotencyError::Unavailable)?,
            bytes: usage
                .bytes
                .checked_sub(bytes)
                .ok_or(IdempotencyError::Unavailable)?,
        })
    }
    fn set_client(&mut self, client: &str, usage: ClientUsage) {
        if usage.entries == 0 && usage.bytes == 0 {
            self.by_client.remove(client);
        } else {
            self.by_client.insert(client.to_owned(), usage);
        }
    }
    fn purge(&mut self, now: Instant) -> Result<(), IdempotencyError> {
        while self
            .expiry
            .first()
            .is_some_and(|(expires, _)| *expires <= now)
        {
            let (expires, key) = self.expiry.first().expect("first expiry exists").clone();
            if self
                .by_key
                .get(&key)
                .is_some_and(|entry| entry.expires == expires)
            {
                let charged = self
                    .by_key
                    .get(&key)
                    .expect("matching entry exists")
                    .charged_bytes();
                let total = self
                    .charged_bytes
                    .checked_sub(charged)
                    .ok_or(IdempotencyError::Unavailable)?;
                let client = self.client_release(&key.0, 1, charged)?;
                self.charged_bytes = total;
                self.set_client(&key.0, client);
                self.by_key.remove(&key);
            }
            self.expiry.remove(&(expires, key));
        }
        Ok(())
    }
}
/// Private completion authority issued by one successful atomic admission.
/// Dropping a ticket does not free the reservation: execution might have committed.
#[must_use]
pub struct IdempotencyReservation {
    key: StoreKey,
    fingerprint: Fingerprint,
    generation: u64,
    owner: Arc<()>,
}
impl std::fmt::Debug for IdempotencyReservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IdempotencyReservation(..)")
    }
}
/// Atomic admission either grants execution or returns exact retained signed bytes.
#[must_use]
#[non_exhaustive]
pub enum IdempotencyAdmission {
    /// The caller owns execution and eventual completion for this request.
    Reserved(IdempotencyReservation),
    /// The request already finished; replay these identical signed bytes,
    /// whose outcome was recorded when they were retained.
    Replay {
        /// The exact signed bytes the first caller received.
        reply: Arc<[u8]>,
        /// What those bytes reported.
        outcome: RetainedOutcome,
    },
}
impl std::fmt::Debug for IdempotencyAdmission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Reserved(_) => f.write_str("Reserved(..)"),
            Self::Replay { reply, outcome } => f
                .debug_struct("Replay")
                .field("bytes", &reply.len())
                .field("outcome", outcome)
                .finish(),
        }
    }
}
/// Thread-safe bounded in-memory store. New requests are refused under pressure;
/// neither in-flight nor completed unexpired entries are evicted.
/// Dropping a reservation retains it because execution might have committed.
/// Routes complete pre-execution failures with a signed failed reply.
pub struct IdempotencyStore {
    config: IdempotencyConfig,
    entries: Mutex<Entries>,
    owner: Arc<()>,
}
impl IdempotencyStore {
    /// Validate configuration. Capacity and byte limits must be positive;
    /// configured TTL cannot exceed the request's five-minute hard ceiling.
    pub fn new(config: IdempotencyConfig) -> Result<Self, IdempotencyError> {
        if config.capacity == 0
            || config.max_reply_bytes == 0
            || config.client_share == 0
            || config.max_ttl.is_zero()
            || config.max_ttl > MAX_IDEMPOTENCY_TTL
        {
            return Err(IdempotencyError::InvalidRequest);
        }
        Ok(Self {
            config,
            entries: Mutex::new(Entries::default()),
            owner: Arc::new(()),
        })
    }
    /// The validated configuration this store enforces.
    pub fn config(&self) -> IdempotencyConfig {
        self.config
    }
    /// Reserve before executing a keyed request. `client` must come from
    /// authenticated transport/policy, not the request body. `reply_allowance`
    /// must cover the largest signed success or failure the route can produce.
    /// Reserving bytes now prevents a successful commit from losing its reply
    /// because another request consumed retention memory before completion.
    pub fn begin(
        &self,
        client: &str,
        key: &IdempotencyKey,
        fingerprint: &Fingerprint,
        expires: Instant,
        reply_allowance: usize,
        now: Instant,
    ) -> Result<IdempotencyAdmission, IdempotencyError> {
        if client.is_empty()
            || client.len() > 512
            || client.chars().any(char::is_control)
            || reply_allowance == 0
            || reply_allowance > MAX_REMOTE_ENVELOPE_BYTES
            || expires <= now
            || expires.duration_since(now) > self.config.max_ttl
            || fingerprint.domain().as_str() != QUERY_REMOTE_REQUEST_FINGERPRINT_DOMAIN
            || fingerprint.canonicalization().as_str() != QUERY_REMOTE_REQUEST_FORMAT_V3
            || fingerprint.semantic_profile().is_some()
        {
            return Err(IdempotencyError::InvalidRequest);
        }
        let store_key = (client.to_owned(), key.clone());
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| IdempotencyError::Unavailable)?;
        entries.purge(now)?;
        if let Some(entry) = entries.by_key.get(&store_key) {
            if entry.fingerprint != *fingerprint {
                return Err(IdempotencyError::KeyReused);
            }
            return match &entry.state {
                State::InFlight { .. } => Err(IdempotencyError::RetryLater),
                State::Done { reply, outcome } => Ok(IdempotencyAdmission::Replay {
                    reply: Arc::clone(reply),
                    outcome: outcome.clone(),
                }),
            };
        }
        let charged = entries
            .charged_bytes
            .checked_add(reply_allowance)
            .ok_or(IdempotencyError::Capacity)?;
        if entries.by_key.len() >= self.config.capacity || charged > self.config.max_reply_bytes {
            return Err(IdempotencyError::Capacity);
        }
        // One client cannot exhaust the store for every other client.
        let usage = entries.by_client.get(client).copied().unwrap_or_default();
        let client_usage = ClientUsage {
            entries: usage.entries.saturating_add(1),
            bytes: usage.bytes.saturating_add(reply_allowance),
        };
        if client_usage.entries > self.config.client_capacity()
            || client_usage.bytes > self.config.client_reply_bytes()
        {
            return Err(IdempotencyError::ClientCapacity);
        }
        let generation = entries
            .generation
            .checked_add(1)
            .ok_or(IdempotencyError::Unavailable)?;
        entries.generation = generation;
        entries.expiry.insert((expires, store_key.clone()));
        entries.by_key.insert(
            store_key.clone(),
            Entry {
                fingerprint: fingerprint.clone(),
                expires,
                generation,
                state: State::InFlight {
                    allowance: reply_allowance,
                },
            },
        );
        entries.charged_bytes = charged;
        entries.set_client(client, client_usage);
        Ok(IdempotencyAdmission::Reserved(IdempotencyReservation {
            key: store_key,
            fingerprint: fingerprint.clone(),
            generation,
            owner: Arc::clone(&self.owner),
        }))
    }
    /// Retain opaque signed, request-bound reply bytes after execution, including
    /// signed failed/indeterminate outcomes, with the `outcome` they report.
    /// The caller owns signing and correlation. A ticket cannot overwrite a
    /// completed or newer reservation. An invalid completion leaves the
    /// in-flight entry intact until expiry.
    pub fn complete(
        &self,
        reservation: &IdempotencyReservation,
        signed_reply: Arc<[u8]>,
        outcome: RetainedOutcome,
        now: Instant,
    ) -> Result<(), IdempotencyError> {
        if !Arc::ptr_eq(&reservation.owner, &self.owner) {
            return Err(IdempotencyError::ReservationLost);
        }
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| IdempotencyError::Unavailable)?;
        entries.purge(now)?;
        let entry = entries
            .by_key
            .get(&reservation.key)
            .ok_or(IdempotencyError::ReservationLost)?;
        if entry.generation != reservation.generation
            || entry.fingerprint != reservation.fingerprint
        {
            return Err(IdempotencyError::ReservationLost);
        }
        let State::InFlight { allowance } = entry.state else {
            return Err(IdempotencyError::ReservationLost);
        };
        if signed_reply.is_empty() || signed_reply.len() > allowance {
            return Err(IdempotencyError::ReplyTooLarge);
        }
        let released = allowance - signed_reply.len();
        let total = entries
            .charged_bytes
            .checked_sub(released)
            .ok_or(IdempotencyError::Unavailable)?;
        let client = entries.client_release(&reservation.key.0, 0, released)?;
        entries.charged_bytes = total;
        entries.set_client(&reservation.key.0, client);
        let entry = entries
            .by_key
            .get_mut(&reservation.key)
            .expect("validated entry exists");
        entry.state = State::Done {
            reply: signed_reply,
            outcome,
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use type_bridge_contract::fingerprint::{CanonicalizationVersion, FingerprintDomain};
    fn fingerprint(bytes: &[u8]) -> Fingerprint {
        Fingerprint::compute(
            FingerprintDomain::new(QUERY_REMOTE_REQUEST_FINGERPRINT_DOMAIN).unwrap(),
            CanonicalizationVersion::new(QUERY_REMOTE_REQUEST_FORMAT_V3).unwrap(),
            None,
            bytes,
        )
    }
    fn key(n: u8) -> IdempotencyKey {
        IdempotencyKey::new(vec![n; 16]).unwrap()
    }
    fn store(capacity: usize, bytes: usize) -> IdempotencyStore {
        IdempotencyStore::new(IdempotencyConfig {
            capacity,
            max_reply_bytes: bytes,
            client_share: 1,
            ..Default::default()
        })
        .unwrap()
    }
    fn ticket(admission: IdempotencyAdmission) -> IdempotencyReservation {
        match admission {
            IdempotencyAdmission::Reserved(ticket) => ticket,
            IdempotencyAdmission::Replay { .. } => panic!("expected fresh reservation"),
        }
    }
    fn ok() -> RetainedOutcome {
        RetainedOutcome::new(true, DiagnosticCode::new("ok").unwrap())
    }
    #[test]
    fn same_key_retries_wait_then_replay_exact_signed_bytes_and_outcome() {
        let store = store(2, 100);
        let now = Instant::now();
        let expiry = now + Duration::from_secs(5);
        let fp = fingerprint(b"one");
        let reservation = ticket(
            store
                .begin("client", &key(1), &fp, expiry, 50, now)
                .unwrap(),
        );
        assert_eq!(
            store
                .begin("client", &key(1), &fp, expiry, 50, now)
                .unwrap_err(),
            IdempotencyError::RetryLater
        );
        let reply: Arc<[u8]> = Arc::from(&b"opaque-signed-reply"[..]);
        // A retained failure replays as the failure it was.
        let failed = RetainedOutcome::new(
            false,
            DiagnosticCode::new("commit_definitely_aborted").unwrap(),
        );
        store
            .complete(&reservation, Arc::clone(&reply), failed.clone(), now)
            .unwrap();
        match store
            .begin("client", &key(1), &fp, expiry, 50, now)
            .unwrap()
        {
            IdempotencyAdmission::Replay {
                reply: cached,
                outcome,
            } => {
                assert!(Arc::ptr_eq(&cached, &reply));
                assert_eq!(&*cached, &*reply);
                assert_eq!(outcome, failed);
            }
            _ => panic!("expected stored reply"),
        }
        assert_eq!(
            store.complete(&reservation, Arc::from(&b"replacement"[..]), ok(), now),
            Err(IdempotencyError::ReservationLost)
        );
    }
    #[test]
    fn different_fingerprint_is_refused_in_flight_and_after_completion() {
        let store = store(2, 100);
        let now = Instant::now();
        let expiry = now + Duration::from_secs(5);
        let fp = fingerprint(b"one");
        let other = fingerprint(b"two");
        let reservation = ticket(
            store
                .begin("client", &key(1), &fp, expiry, 50, now)
                .unwrap(),
        );
        assert_eq!(
            store
                .begin("client", &key(1), &other, expiry, 50, now)
                .unwrap_err(),
            IdempotencyError::KeyReused
        );
        store
            .complete(&reservation, Arc::from(&b"signed"[..]), ok(), now)
            .unwrap();
        assert_eq!(
            store
                .begin("client", &key(1), &other, expiry, 50, now)
                .unwrap_err(),
            IdempotencyError::KeyReused
        );
        assert!(matches!(
            store
                .begin("another-client", &key(1), &other, expiry, 50, now)
                .unwrap(),
            IdempotencyAdmission::Reserved(_)
        ));
    }
    #[test]
    fn capacity_pressure_never_evicts_unexpired_inflight_or_done() {
        for complete in [false, true] {
            let store = store(1, 100);
            let now = Instant::now();
            let expiry = now + Duration::from_secs(5);
            let fp = fingerprint(b"one");
            let reservation = ticket(
                store
                    .begin("client", &key(1), &fp, expiry, 50, now)
                    .unwrap(),
            );
            if complete {
                store
                    .complete(&reservation, Arc::from(&b"signed"[..]), ok(), now)
                    .unwrap();
            }
            assert_eq!(
                store
                    .begin("client", &key(2), &fp, expiry, 50, now)
                    .unwrap_err(),
                IdempotencyError::Capacity
            );
            let after = expiry;
            assert!(matches!(
                store
                    .begin(
                        "client",
                        &key(2),
                        &fp,
                        after + Duration::from_secs(5),
                        50,
                        after
                    )
                    .unwrap(),
                IdempotencyAdmission::Reserved(_)
            ));
            assert_eq!(
                store.complete(&reservation, Arc::from(&b"late"[..]), ok(), after),
                Err(IdempotencyError::ReservationLost)
            );
            let entries = store.entries.lock().unwrap();
            assert_eq!(entries.by_key.len(), 1);
            assert_eq!(entries.expiry.len(), 1);
            assert_eq!(entries.charged_bytes, 50);
        }
    }
    #[test]
    fn byte_allowance_is_reserved_before_execution_and_released_on_completion() {
        let store = store(3, 10);
        let now = Instant::now();
        let expiry = now + Duration::from_secs(5);
        let fp = fingerprint(b"one");
        let reservation = ticket(store.begin("client", &key(1), &fp, expiry, 6, now).unwrap());
        assert_eq!(
            store
                .begin("client", &key(2), &fp, expiry, 6, now)
                .unwrap_err(),
            IdempotencyError::Capacity
        );
        assert_eq!(
            store.complete(&reservation, Arc::from(&b"too-large"[..]), ok(), now),
            Err(IdempotencyError::ReplyTooLarge)
        );
        assert_eq!(
            store
                .begin("client", &key(1), &fp, expiry, 6, now)
                .unwrap_err(),
            IdempotencyError::RetryLater
        );
        store
            .complete(&reservation, Arc::from(&b"ok"[..]), ok(), now)
            .unwrap();
        assert!(matches!(
            store.begin("client", &key(2), &fp, expiry, 6, now).unwrap(),
            IdempotencyAdmission::Reserved(_)
        ));
        assert_eq!(store.entries.lock().unwrap().charged_bytes, 8);
    }
    #[test]
    fn old_or_foreign_tickets_cannot_complete_a_new_reservation() {
        let first = store(2, 100);
        let second = store(2, 100);
        let now = Instant::now();
        let expiry = now + Duration::from_secs(1);
        let fp = fingerprint(b"one");
        let old = ticket(
            first
                .begin("client", &key(1), &fp, expiry, 50, now)
                .unwrap(),
        );
        let foreign = ticket(
            second
                .begin("client", &key(1), &fp, expiry, 50, now)
                .unwrap(),
        );
        assert_eq!(
            first.complete(&foreign, Arc::from(&b"foreign"[..]), ok(), now),
            Err(IdempotencyError::ReservationLost)
        );
        let after = expiry;
        let new = ticket(
            first
                .begin(
                    "client",
                    &key(1),
                    &fp,
                    after + Duration::from_secs(1),
                    50,
                    after,
                )
                .unwrap(),
        );
        assert_eq!(
            first.complete(&old, Arc::from(&b"stale"[..]), ok(), after),
            Err(IdempotencyError::ReservationLost)
        );
        first
            .complete(&new, Arc::from(&b"current"[..]), ok(), after)
            .unwrap();
    }
    #[test]
    fn retries_do_not_extend_expiry_and_dropping_ticket_does_not_release_execution() {
        let store = store(1, 100);
        let now = Instant::now();
        let expiry = now + Duration::from_secs(1);
        let fp = fingerprint(b"one");
        drop(ticket(
            store
                .begin("client", &key(1), &fp, expiry, 50, now)
                .unwrap(),
        ));
        assert_eq!(
            store
                .begin(
                    "client",
                    &key(1),
                    &fp,
                    expiry + Duration::from_secs(10),
                    50,
                    now
                )
                .unwrap_err(),
            IdempotencyError::RetryLater
        );
        assert!(matches!(
            store
                .begin(
                    "client",
                    &key(1),
                    &fp,
                    expiry + Duration::from_secs(1),
                    50,
                    expiry
                )
                .unwrap(),
            IdempotencyAdmission::Reserved(_)
        ));
    }
    #[test]
    fn one_concurrent_caller_owns_execution() {
        let store = Arc::new(store(2, 100));
        let barrier = Arc::new(std::sync::Barrier::new(16));
        let now = Instant::now();
        let expiry = now + Duration::from_secs(5);
        let fp = fingerprint(b"one");
        let threads: Vec<_> = (0..16)
            .map(|_| {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                let fp = fp.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    store.begin("client", &key(1), &fp, expiry, 50, now)
                })
            })
            .collect();
        let mut winner = None;
        let mut waiting = 0;
        for thread in threads {
            match thread.join().unwrap() {
                Ok(IdempotencyAdmission::Reserved(ticket)) => {
                    assert!(winner.replace(ticket).is_none());
                }
                Err(IdempotencyError::RetryLater) => waiting += 1,
                _ => panic!("unexpected concurrent admission"),
            }
        }
        assert_eq!(waiting, 15);
        store
            .complete(&winner.unwrap(), Arc::from(&b"signed"[..]), ok(), now)
            .unwrap();
        assert!(matches!(
            store
                .begin("client", &key(1), &fp, expiry, 50, now)
                .unwrap(),
            IdempotencyAdmission::Replay { .. }
        ));
    }
    #[test]
    fn one_client_cannot_exhaust_the_store_for_others() {
        // Four entries and 400 bytes, a quarter of each per client.
        let store = IdempotencyStore::new(
            IdempotencyConfig::new(4, 400, MAX_IDEMPOTENCY_TTL).with_client_share(4),
        )
        .unwrap();
        let now = Instant::now();
        let expiry = now + Duration::from_secs(5);
        let fp = fingerprint(b"one");
        // Over the per-client byte share in one reservation.
        assert_eq!(
            store
                .begin("greedy", &key(1), &fp, expiry, 101, now)
                .unwrap_err(),
            IdempotencyError::ClientCapacity
        );
        let reservation = ticket(
            store
                .begin("greedy", &key(1), &fp, expiry, 100, now)
                .unwrap(),
        );
        // Over the per-client entry share, even after completion shrank the
        // charged bytes.
        store
            .complete(&reservation, Arc::from(&b"signed"[..]), ok(), now)
            .unwrap();
        let error = store
            .begin("greedy", &key(2), &fp, expiry, 10, now)
            .unwrap_err();
        assert_eq!(error, IdempotencyError::ClientCapacity);
        assert_eq!(error.code(), "idempotency_client_capacity");
        // Another client still has its own share.
        assert!(matches!(
            store
                .begin("other", &key(1), &fp, expiry, 100, now)
                .unwrap(),
            IdempotencyAdmission::Reserved(_)
        ));
        // Expiry returns the greedy client's share.
        assert!(matches!(
            store
                .begin(
                    "greedy",
                    &key(2),
                    &fp,
                    expiry + Duration::from_secs(5),
                    100,
                    expiry
                )
                .unwrap(),
            IdempotencyAdmission::Reserved(_)
        ));
        let entries = store.entries.lock().unwrap();
        assert_eq!(entries.by_client.len(), 1);
        assert_eq!(entries.by_client["greedy"].bytes, 100);
    }
    #[test]
    fn default_client_share_admits_a_full_allowance_beside_retained_replies() {
        let store = IdempotencyStore::new(IdempotencyConfig::default()).unwrap();
        assert_eq!(
            store.config().client_share,
            DEFAULT_IDEMPOTENCY_CLIENT_SHARE
        );
        let now = Instant::now();
        let expiry = now + Duration::from_secs(5);
        let fp = fingerprint(b"one");
        let full = MAX_REMOTE_ENVELOPE_BYTES;
        let first = ticket(
            store
                .begin("client", &key(1), &fp, expiry, full, now)
                .unwrap(),
        );
        store
            .complete(&first, Arc::from(&b"signed"[..]), ok(), now)
            .unwrap();
        // The retained reply does not block the next full allowance ...
        assert!(matches!(
            store
                .begin("client", &key(2), &fp, expiry, full, now)
                .unwrap(),
            IdempotencyAdmission::Reserved(_)
        ));
        // ... but a third concurrent one exceeds the client's share.
        assert_eq!(
            store
                .begin("client", &key(3), &fp, expiry, full, now)
                .unwrap_err(),
            IdempotencyError::ClientCapacity
        );
    }
    #[test]
    fn config_constructor_matches_the_defaults() {
        let config = IdempotencyConfig::new(
            DEFAULT_IDEMPOTENCY_CAPACITY,
            DEFAULT_IDEMPOTENCY_BYTES,
            MAX_IDEMPOTENCY_TTL,
        );
        assert_eq!(config, IdempotencyConfig::default());
    }
    #[test]
    fn invalid_config_lifetime_identity_and_overflow_fail_closed() {
        for config in [
            IdempotencyConfig {
                capacity: 0,
                ..Default::default()
            },
            IdempotencyConfig {
                max_reply_bytes: 0,
                ..Default::default()
            },
            IdempotencyConfig {
                max_ttl: Duration::ZERO,
                ..Default::default()
            },
            IdempotencyConfig::default().with_client_share(0),
            IdempotencyConfig {
                max_ttl: MAX_IDEMPOTENCY_TTL + Duration::from_nanos(1),
                ..Default::default()
            },
        ] {
            assert!(IdempotencyStore::new(config).is_err());
        }
        let store = store(2, 100);
        let now = Instant::now();
        let fp = fingerprint(b"one");
        for (client, expiry, allowance) in [
            ("", now + Duration::from_secs(1), 10),
            ("client\nSECRET_SENTINEL", now + Duration::from_secs(1), 10),
            ("client", now, 10),
            (
                "client",
                now + MAX_IDEMPOTENCY_TTL + Duration::from_nanos(1),
                10,
            ),
            ("client", now + Duration::from_secs(1), 0),
        ] {
            let error = store
                .begin(client, &key(1), &fp, expiry, allowance, now)
                .unwrap_err();
            assert_eq!(error, IdempotencyError::InvalidRequest);
            assert!(!format!("{:?}", error.diagnostic()).contains("SECRET_SENTINEL"));
        }
        store.entries.lock().unwrap().generation = u64::MAX;
        assert_eq!(
            store
                .begin(
                    "client",
                    &key(1),
                    &fp,
                    now + Duration::from_secs(1),
                    10,
                    now
                )
                .unwrap_err(),
            IdempotencyError::Unavailable
        );
    }
    #[test]
    fn accounting_underflow_preserves_reservation_and_expiry() {
        let store = store(1, 100);
        let now = Instant::now();
        let expiry = now + Duration::from_secs(1);
        let reservation = ticket(
            store
                .begin("client", &key(1), &fingerprint(b"one"), expiry, 50, now)
                .unwrap(),
        );
        store.entries.lock().unwrap().charged_bytes = 0;
        assert_eq!(
            store.complete(&reservation, Arc::from(&b"reply"[..]), ok(), now),
            Err(IdempotencyError::Unavailable)
        );
        let mut entries = store.entries.lock().unwrap();
        assert!(matches!(
            entries.by_key.get(&reservation.key).unwrap().state,
            State::InFlight { .. }
        ));
        assert_eq!(entries.purge(expiry), Err(IdempotencyError::Unavailable));
        assert_eq!(entries.by_key.len(), 1);
        assert_eq!(entries.expiry.len(), 1);
    }
    #[test]
    fn poisoning_never_authorizes_a_second_execution() {
        let store = store(1, 100);
        let now = Instant::now();
        let fp = fingerprint(b"one");
        let _ = std::panic::catch_unwind(|| {
            let _guard = store.entries.lock().unwrap();
            panic!("test poison");
        });
        assert_eq!(
            store
                .begin(
                    "client",
                    &key(1),
                    &fp,
                    now + Duration::from_secs(1),
                    10,
                    now
                )
                .unwrap_err(),
            IdempotencyError::Unavailable
        );
    }
}
