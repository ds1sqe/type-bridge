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
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
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

/// Explicit memory and expiry configuration for the standalone store.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IdempotencyConfig {
    /// Maximum live entries, including in-flight operations.
    pub capacity: usize,
    /// Maximum retained bytes plus reserved in-flight reply allowances.
    pub max_reply_bytes: usize,
    /// Maximum remaining request lifetime, at most five minutes.
    pub max_ttl: Duration,
}
impl Default for IdempotencyConfig {
    fn default() -> Self {
        Self {
            capacity: DEFAULT_IDEMPOTENCY_CAPACITY,
            max_reply_bytes: DEFAULT_IDEMPOTENCY_BYTES,
            max_ttl: MAX_IDEMPOTENCY_TTL,
        }
    }
}
/// Typed refusal; no client identity, key, query or reply bytes enter its text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum IdempotencyError {
    /// No live entry is evicted to make room for this request.
    Capacity,
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
    InFlight { allowance: usize },
    Done(Arc<[u8]>),
}
impl Entry {
    fn charged_bytes(&self) -> usize {
        match &self.state {
            State::InFlight { allowance } => *allowance,
            State::Done(bytes) => bytes.len(),
        }
    }
}
#[derive(Default)]
struct Entries {
    by_key: BTreeMap<StoreKey, Entry>,
    expiry: BTreeSet<(Instant, StoreKey)>,
    charged_bytes: usize,
    generation: u64,
}
impl Entries {
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
                let entry = self.by_key.get(&key).expect("matching entry exists");
                self.charged_bytes = self
                    .charged_bytes
                    .checked_sub(entry.charged_bytes())
                    .ok_or(IdempotencyError::Unavailable)?;
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
    /// The request already finished; replay these identical signed bytes.
    Replay(Arc<[u8]>),
}
impl std::fmt::Debug for IdempotencyAdmission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Reserved(_) => f.write_str("Reserved(..)"),
            Self::Replay(reply) => f
                .debug_struct("Replay")
                .field("bytes", &reply.len())
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
                State::Done(reply) => Ok(IdempotencyAdmission::Replay(Arc::clone(reply))),
            };
        }
        let charged = entries
            .charged_bytes
            .checked_add(reply_allowance)
            .ok_or(IdempotencyError::Capacity)?;
        if entries.by_key.len() >= self.config.capacity || charged > self.config.max_reply_bytes {
            return Err(IdempotencyError::Capacity);
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
        Ok(IdempotencyAdmission::Reserved(IdempotencyReservation {
            key: store_key,
            fingerprint: fingerprint.clone(),
            generation,
            owner: Arc::clone(&self.owner),
        }))
    }
    /// Retain opaque signed, request-bound reply bytes after execution, including
    /// signed failed/indeterminate outcomes. The caller owns signing and
    /// correlation. A ticket cannot overwrite a completed or newer reservation.
    /// An invalid completion leaves the in-flight entry intact until expiry.
    pub fn complete(
        &self,
        reservation: &IdempotencyReservation,
        signed_reply: Arc<[u8]>,
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
        entries.charged_bytes = entries
            .charged_bytes
            .checked_sub(released)
            .ok_or(IdempotencyError::Unavailable)?;
        entries
            .by_key
            .get_mut(&reservation.key)
            .expect("validated entry exists")
            .state = State::Done(signed_reply);
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
            ..Default::default()
        })
        .unwrap()
    }
    fn ticket(admission: IdempotencyAdmission) -> IdempotencyReservation {
        match admission {
            IdempotencyAdmission::Reserved(ticket) => ticket,
            IdempotencyAdmission::Replay(_) => panic!("expected fresh reservation"),
        }
    }
    #[test]
    fn same_key_retries_wait_then_replay_exact_signed_bytes() {
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
        store
            .complete(&reservation, Arc::clone(&reply), now)
            .unwrap();
        match store
            .begin("client", &key(1), &fp, expiry, 50, now)
            .unwrap()
        {
            IdempotencyAdmission::Replay(cached) => {
                assert!(Arc::ptr_eq(&cached, &reply));
                assert_eq!(&*cached, &*reply);
            }
            _ => panic!("expected stored reply"),
        }
        assert_eq!(
            store.complete(&reservation, Arc::from(&b"replacement"[..]), now),
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
            .complete(&reservation, Arc::from(&b"signed"[..]), now)
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
                    .complete(&reservation, Arc::from(&b"signed"[..]), now)
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
                store.complete(&reservation, Arc::from(&b"late"[..]), after),
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
            store.complete(&reservation, Arc::from(&b"too-large"[..]), now),
            Err(IdempotencyError::ReplyTooLarge)
        );
        assert_eq!(
            store
                .begin("client", &key(1), &fp, expiry, 6, now)
                .unwrap_err(),
            IdempotencyError::RetryLater
        );
        store
            .complete(&reservation, Arc::from(&b"ok"[..]), now)
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
            first.complete(&foreign, Arc::from(&b"foreign"[..]), now),
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
            first.complete(&old, Arc::from(&b"stale"[..]), after),
            Err(IdempotencyError::ReservationLost)
        );
        first
            .complete(&new, Arc::from(&b"current"[..]), after)
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
            .complete(&winner.unwrap(), Arc::from(&b"signed"[..]), now)
            .unwrap();
        assert!(matches!(
            store
                .begin("client", &key(1), &fp, expiry, 50, now)
                .unwrap(),
            IdempotencyAdmission::Replay(_)
        ));
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
            store.complete(&reservation, Arc::from(&b"reply"[..]), now),
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
