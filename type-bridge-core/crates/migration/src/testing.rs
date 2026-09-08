//! In-memory mock `DriverBackend` for planner/executor unit tests.
//!
//! The mock records every interaction in an ordered event log so tests can
//! assert the exact sequence of opens, queries, commits, and rollbacks that
//! the executor issued.  A failure-injection knob lets tests verify
//! rollback-on-error and run-halt behavior without a live TypeDB connection.
//!
//! Two construction modes are available:
//!
//! - [`MockMigrationBackend::new`] — every query returns `QueryResult::Ok`
//!   except an optional injected failure at a given 0-indexed query position.
//!   Used by the existing executor tests.
//! - [`MockMigrationBackend::with_responses`] — each query returns the next
//!   `QueryResult` from a scripted list.  Used by the backfill count tests to
//!   supply scripted row counts without a live TypeDB connection.
//!
//! # Usage
//!
//! This example is ignored because the testing module is crate-private and
//! cannot be compiled as an external rustdoc consumer.
//!
//! ```ignore
//! use crate::testing::{MockEvent, MockMigrationBackend};
//! use type_bridge_orm::{Database, TxType};
//!
//! // Fail the second query (0-indexed: index 1).
//! let (backend, log) = MockMigrationBackend::new(Some(1));
//! let db = Database::with_backend(Box::new(backend), "test");
//!
//! // Script specific QueryResult values per query call.
//! use type_bridge_orm::session::backend::QueryResult;
//! use serde_json::json;
//! let scripted = vec![
//!     QueryResult::Rows(vec![json!({"c": 7})]),
//!     QueryResult::Rows(vec![json!({"c": 10})]),
//!     QueryResult::Ok,
//! ];
//! let (backend, log) = MockMigrationBackend::with_responses(scripted);
//! let db = Database::with_backend(Box::new(backend), "test");
//! ```

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use type_bridge_contract::reserved::{
    LEGACY_CUTOVER_ANCHOR_ENTITY, LEGACY_CUTOVER_ANCHOR_FINGERPRINT, LEGACY_CUTOVER_ANCHOR_KEY,
    LEGACY_CUTOVER_ANCHOR_SCOPE, LEGACY_CUTOVER_ANCHOR_SINGLETON_KEY,
    LEGACY_CUTOVER_SENTINEL_APP_LABEL, LEGACY_CUTOVER_SENTINEL_APPLIED_AT,
    LEGACY_LEDGER_APPLIED_ENTITY, LEGACY_WRITER_GUARD_QUERY_TAG, MANAGED_CONTROL_ENTITY,
    MANAGED_CONTROL_LEASE_FENCE, MANAGED_CONTROL_LEASE_HOLDER, MANAGED_CONTROL_LEASE_STATE,
    MANAGED_CONTROL_SCOPE,
};
use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, QueryResult, TransactionOps};
use type_bridge_orm::{ClassifiedCommitError, CommitFailureCertainty, OrmError, TxType};
use type_bridge_schema_compat::{LEGACY_LEDGER_SCHEMA_TYPEQL, MANAGED_FENCE_SCHEMA_TYPEQL};

// ── Event log ─────────────────────────────────────────────────────────────────

/// A single recorded event from the executor seam.
#[derive(Debug, Clone, PartialEq)]
pub enum MockEvent {
    /// A transaction was opened with the given type.
    OpenTx(TxType),
    /// A query was executed on a transaction of the given type.
    Query(TxType, String),
    /// The current transaction was committed.
    Commit,
    /// The current transaction was rolled back.
    Rollback,
    /// The current transaction was closed without committing.
    Close,
}

/// Shared, mutex-guarded ordered event log.
pub type EventLog = Arc<Mutex<Vec<MockEvent>>>;

fn all_state_labels() -> BTreeSet<String> {
    let schema = crate::state::schema::migration_state_schema();
    schema
        .attributes
        .keys()
        .chain(schema.entities.keys())
        .chain(schema.relations.keys())
        .cloned()
        .collect()
}

fn state_label_documents(labels: &BTreeSet<String>, typeql: &str) -> Vec<serde_json::Value> {
    let schema = crate::state::schema::migration_state_schema();
    let expected = if typeql.contains("match attribute $t") {
        schema.attributes.keys().collect::<Vec<_>>()
    } else if typeql.contains("match entity $t") {
        schema.entities.keys().collect::<Vec<_>>()
    } else {
        schema.relations.keys().collect::<Vec<_>>()
    };
    expected
        .into_iter()
        .filter(|label| labels.contains(*label))
        .map(|label| serde_json::json!({"label": label}))
        .collect()
}

const MOCK_CUTOVER_FINGERPRINT: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

fn legacy_guard_documents(
    typeql: &str,
    cutover_present: bool,
    state_labels: Option<&BTreeSet<String>>,
) -> Vec<serde_json::Value> {
    if typeql.contains("match entity $t") {
        let mut values = state_labels
            .map(|labels| state_label_documents(labels, typeql))
            .unwrap_or_default();
        if cutover_present {
            values.push(serde_json::json!({"label": MANAGED_CONTROL_ENTITY}));
            values.push(serde_json::json!({"label": LEGACY_CUTOVER_ANCHOR_ENTITY}));
        }
        return values;
    }
    if typeql.contains("match attribute $t") {
        let mut values = state_labels
            .map(|labels| state_label_documents(labels, typeql))
            .unwrap_or_default();
        if cutover_present {
            values.extend([
                serde_json::json!({"label": MANAGED_CONTROL_SCOPE}),
                serde_json::json!({"label": MANAGED_CONTROL_LEASE_HOLDER}),
                serde_json::json!({"label": MANAGED_CONTROL_LEASE_FENCE}),
                serde_json::json!({"label": MANAGED_CONTROL_LEASE_STATE}),
                serde_json::json!({"label": LEGACY_CUTOVER_ANCHOR_KEY}),
                serde_json::json!({"label": LEGACY_CUTOVER_ANCHOR_SCOPE}),
                serde_json::json!({"label": LEGACY_CUTOVER_ANCHOR_FINGERPRINT}),
            ]);
        }
        return values;
    }
    if !cutover_present {
        return Vec::new();
    }
    if typeql.contains(&format!("isa {MANAGED_CONTROL_ENTITY}")) {
        if typeql.contains("\"scope\": $scope") {
            return vec![serde_json::json!({
                "scope": "mock-scope",
                "fence": "1",
                "state": "free",
            })];
        }
        if typeql.contains("\"holder\": $holder") {
            return Vec::new();
        }
        return vec![serde_json::json!({"exists": true})];
    }
    if typeql.contains(&format!("isa {LEGACY_CUTOVER_ANCHOR_ENTITY}")) {
        if typeql.contains("\"fingerprint\": $fingerprint") {
            return vec![serde_json::json!({
                "key": LEGACY_CUTOVER_ANCHOR_SINGLETON_KEY,
                "scope": "mock-scope",
                "fingerprint": MOCK_CUTOVER_FINGERPRINT,
            })];
        }
        return vec![serde_json::json!({"exists": true})];
    }
    if typeql.contains(&format!("isa {LEGACY_LEDGER_APPLIED_ENTITY}")) {
        if typeql.contains("\"checksum\": $checksum") {
            return vec![serde_json::json!({
                "app": LEGACY_CUTOVER_SENTINEL_APP_LABEL,
                "applied": LEGACY_CUTOVER_SENTINEL_APPLIED_AT,
                "checksum": MOCK_CUTOVER_FINGERPRINT,
            })];
        }
        return vec![serde_json::json!({"exists": true})];
    }
    Vec::new()
}

// ── MockMigrationBackend ──────────────────────────────────────────────────────

/// In-memory `DriverBackend` that records the executor's interaction sequence.
///
/// See the module-level documentation for the two construction modes.
pub struct MockMigrationBackend {
    log: EventLog,
    /// Global query counter shared across all spawned transactions.
    query_count: Arc<Mutex<usize>>,
    /// If `Some(n)`, the n-th query call returns an error (used by `new`).
    fail_on_query_index: Option<usize>,
    /// Scripted per-query responses consumed in order (used by `with_responses`).
    /// When present, `fail_on_query_index` is ignored.
    scripted_responses: Option<Arc<Mutex<Vec<QueryResult>>>>,
    /// Global commit counter shared across all spawned transactions.
    commit_count: Arc<Mutex<usize>>,
    /// If `Some(n)`, the n-th commit returns an ambiguous driver error.
    fail_on_commit_index: Option<usize>,
    /// Optional typed certainty for the injected commit failure.
    commit_failure_certainty: Option<CommitFailureCertainty>,
    /// Global close counter shared across all spawned transactions.
    close_count: Arc<Mutex<usize>>,
    /// If `Some(n)`, the n-th close returns an injected cleanup failure.
    fail_on_close_index: Option<usize>,
    /// Model a complete legacy ledger carrying the permanent V2 sentinel.
    legacy_cutover_present: bool,
    /// Model the currently installed frozen migration-state labels.
    legacy_state_labels: Option<Arc<Mutex<BTreeSet<String>>>>,
}

impl MockMigrationBackend {
    /// Create a new mock backend and return it together with the shared event log.
    ///
    /// Pass `fail_on_query_index = Some(n)` to make the n-th `query` call
    /// (0-indexed across the whole run) return an error.  All other queries
    /// return `QueryResult::Ok`.
    pub fn new(fail_on_query_index: Option<usize>) -> (Self, EventLog) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let backend = Self {
            log: Arc::clone(&log),
            query_count: Arc::new(Mutex::new(0)),
            fail_on_query_index,
            scripted_responses: None,
            commit_count: Arc::new(Mutex::new(0)),
            fail_on_commit_index: None,
            commit_failure_certainty: None,
            close_count: Arc::new(Mutex::new(0)),
            fail_on_close_index: None,
            legacy_cutover_present: false,
            legacy_state_labels: None,
        };
        (backend, log)
    }

    /// Create a mock backend with scripted per-query responses.
    ///
    /// Each `query` call consumes the next element from `responses` in order.
    /// If the list is exhausted, subsequent queries return `QueryResult::Ok`.
    /// Useful for backfill count tests that need to return `Rows([{"c": N}])`.
    pub fn with_responses(responses: Vec<QueryResult>) -> (Self, EventLog) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let backend = Self {
            log: Arc::clone(&log),
            query_count: Arc::new(Mutex::new(0)),
            fail_on_query_index: None,
            scripted_responses: Some(Arc::new(Mutex::new(responses))),
            commit_count: Arc::new(Mutex::new(0)),
            fail_on_commit_index: None,
            commit_failure_certainty: None,
            close_count: Arc::new(Mutex::new(0)),
            fail_on_close_index: None,
            legacy_cutover_present: false,
            legacy_state_labels: None,
        };
        (backend, log)
    }

    /// Create a backend whose legacy applied ledger carries the V2 cutover
    /// sentinel. Guard probes are answered without consuming scripted user
    /// query responses, allowing tests to prove rejection ordering.
    pub fn with_legacy_cutover() -> (Self, EventLog) {
        let (mut backend, log) = Self::new(None);
        backend.legacy_cutover_present = true;
        backend.legacy_state_labels = Some(Arc::new(Mutex::new(all_state_labels())));
        (backend, log)
    }

    /// Create a backend with a complete frozen state schema and scripted
    /// responses for the actual ledger/run-log reads.
    pub fn with_state_read_responses(responses: Vec<QueryResult>) -> (Self, EventLog) {
        let (mut backend, log) = Self::with_responses(responses);
        backend.legacy_state_labels = Some(Arc::new(Mutex::new(all_state_labels())));
        (backend, log)
    }

    /// Create a complete state backend that fails one ledger query and the
    /// selected transaction close, for primary-versus-cleanup precedence tests.
    pub fn with_state_read_and_close_failure(
        fail_on_query_index: usize,
        fail_on_close_index: usize,
    ) -> (Self, EventLog) {
        let (mut backend, log) = Self::new(Some(fail_on_query_index));
        backend.legacy_state_labels = Some(Arc::new(Mutex::new(all_state_labels())));
        backend.fail_on_close_index = Some(fail_on_close_index);
        (backend, log)
    }

    /// Create a state backend beginning from an interrupted legacy bootstrap.
    /// The returned label set allows the caller to verify incremental repair.
    pub fn with_partial_state_schema(
        missing: &[&str],
        legacy_cutover_present: bool,
    ) -> (Self, EventLog, Arc<Mutex<BTreeSet<String>>>) {
        let (mut backend, log) = Self::new(None);
        let mut labels = all_state_labels();
        for label in missing {
            labels.remove(*label);
        }
        let labels = Arc::new(Mutex::new(labels));
        backend.legacy_cutover_present = legacy_cutover_present;
        backend.legacy_state_labels = Some(Arc::clone(&labels));
        (backend, log, labels)
    }

    /// Create a backend whose selected commit returns an ambiguous error.
    pub fn with_commit_failure(fail_on_commit_index: usize) -> (Self, EventLog) {
        let (mut backend, log) = Self::new(None);
        backend.fail_on_commit_index = Some(fail_on_commit_index);
        (backend, log)
    }

    /// Create a backend whose selected commit is known to have been aborted.
    pub fn with_definitely_aborted_commit_failure(fail_on_commit_index: usize) -> (Self, EventLog) {
        let (mut backend, log) = Self::new(None);
        backend.fail_on_commit_index = Some(fail_on_commit_index);
        backend.commit_failure_certainty = Some(CommitFailureCertainty::DefinitelyAborted);
        (backend, log)
    }
}

impl DriverBackend for MockMigrationBackend {
    fn open_transaction(
        &self,
        _database: &str,
        tx_type: TxType,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
        self.log.lock().unwrap().push(MockEvent::OpenTx(tx_type));
        let log = Arc::clone(&self.log);
        let query_count = Arc::clone(&self.query_count);
        let fail_on = self.fail_on_query_index;
        let scripted = self.scripted_responses.as_ref().map(Arc::clone);
        let commit_count = Arc::clone(&self.commit_count);
        let fail_on_commit_index = self.fail_on_commit_index;
        let commit_failure_certainty = self.commit_failure_certainty;
        let close_count = Arc::clone(&self.close_count);
        let fail_on_close_index = self.fail_on_close_index;
        let legacy_cutover_present = self.legacy_cutover_present;
        let legacy_state_labels = self.legacy_state_labels.as_ref().map(Arc::clone);
        Box::pin(async move {
            let tx: Box<dyn TransactionOps> = Box::new(MockMigrationTransaction {
                tx_type,
                log,
                query_count,
                fail_on,
                scripted_responses: scripted,
                commit_count,
                fail_on_commit_index,
                commit_failure_certainty,
                close_count,
                fail_on_close_index,
                legacy_cutover_present,
                legacy_state_labels,
            });
            Ok(tx)
        })
    }

    fn is_open(&self) -> bool {
        true
    }
}

// ── MockMigrationTransaction ──────────────────────────────────────────────────

struct MockMigrationTransaction {
    tx_type: TxType,
    log: EventLog,
    query_count: Arc<Mutex<usize>>,
    fail_on: Option<usize>,
    /// When present, each query consumes the next scripted response.
    scripted_responses: Option<Arc<Mutex<Vec<QueryResult>>>>,
    commit_count: Arc<Mutex<usize>>,
    fail_on_commit_index: Option<usize>,
    commit_failure_certainty: Option<CommitFailureCertainty>,
    close_count: Arc<Mutex<usize>>,
    fail_on_close_index: Option<usize>,
    legacy_cutover_present: bool,
    legacy_state_labels: Option<Arc<Mutex<BTreeSet<String>>>>,
}

impl TransactionOps for MockMigrationTransaction {
    fn schema_snapshot(&mut self) -> BoxFuture<'_, Result<Option<String>, OrmError>> {
        let snapshot = self
            .legacy_cutover_present
            .then(|| format!("{MANAGED_FENCE_SCHEMA_TYPEQL}\n{LEGACY_LEDGER_SCHEMA_TYPEQL}"));
        Box::pin(async move { Ok(snapshot) })
    }

    fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        // Existing executor tests predate the permanent cutover probe and
        // intentionally assert only user-query ordering.  Model a fresh
        // database with no legacy ledger without consuming scripted answers;
        // dedicated cutover tests use a marker-aware backend instead.
        if typeql.starts_with(LEGACY_WRITER_GUARD_QUERY_TAG) {
            let labels = self
                .legacy_state_labels
                .as_ref()
                .map(|labels| labels.lock().unwrap());
            let values =
                legacy_guard_documents(typeql, self.legacy_cutover_present, labels.as_deref());
            return Box::pin(async move { Ok(QueryResult::Documents(values)) });
        }
        if crate::state::typedb::is_legacy_state_schema_probe_query(typeql) {
            let values = self
                .legacy_state_labels
                .as_ref()
                .map(|labels| state_label_documents(&labels.lock().unwrap(), typeql))
                .unwrap_or_default();
            return Box::pin(async move { Ok(QueryResult::Documents(values)) });
        }
        if typeql.contains("fetch { \"label\": label($t) }")
            && let Some(labels) = &self.legacy_state_labels
        {
            let values = state_label_documents(&labels.lock().unwrap(), typeql);
            return Box::pin(async move { Ok(QueryResult::Documents(values)) });
        }
        // Determine current query index and bump the counter.
        let idx = {
            let mut count = self.query_count.lock().unwrap();
            let current = *count;
            *count += 1;
            current
        };

        let typeql_owned = typeql.to_string();
        let tx_type = self.tx_type;

        // Record the query event regardless of success/failure so the test can
        // assert that the query was attempted before the rollback event.
        self.log
            .lock()
            .unwrap()
            .push(MockEvent::Query(tx_type, typeql_owned));

        if typeql.trim_start().starts_with("define")
            && let Some(labels) = &self.legacy_state_labels
        {
            let schema = crate::state::schema::migration_state_schema();
            let mut installed = labels.lock().unwrap();
            for label in schema.attributes.keys() {
                if typeql.contains(&format!("attribute {label}")) {
                    installed.insert(label.clone());
                }
            }
            for label in schema.entities.keys() {
                if typeql.contains(&format!("entity {label}")) {
                    installed.insert(label.clone());
                }
            }
        }

        // Determine what to return.
        let response: Result<QueryResult, OrmError> =
            if let Some(scripted) = &self.scripted_responses {
                // Scripted mode: consume next response.
                let mut responses = scripted.lock().unwrap();
                if responses.is_empty() {
                    Ok(QueryResult::Ok)
                } else {
                    Ok(responses.remove(0))
                }
            } else {
                // Failure-injection mode.
                let should_fail = self.fail_on == Some(idx);
                if should_fail {
                    Err(OrmError::Transaction(
                        "injected query failure for testing".to_string(),
                    ))
                } else {
                    Ok(QueryResult::Ok)
                }
            };

        Box::pin(async move { response })
    }

    fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.log.lock().unwrap().push(MockEvent::Commit);
        let index = {
            let mut count = self.commit_count.lock().unwrap();
            let index = *count;
            *count += 1;
            index
        };
        let fail = self.fail_on_commit_index == Some(index);
        let certainty = self.commit_failure_certainty;
        Box::pin(async move {
            if fail {
                Err(match certainty {
                    Some(_) => OrmError::Transaction(
                        "Commit failed: injected rejected commit response for testing".to_string(),
                    ),
                    None => OrmError::Transaction(
                        "injected ambiguous commit response for testing".to_string(),
                    ),
                })
            } else {
                Ok(())
            }
        })
    }

    fn commit_classified(&mut self) -> BoxFuture<'_, Result<(), ClassifiedCommitError>> {
        self.log.lock().unwrap().push(MockEvent::Commit);
        let index = {
            let mut count = self.commit_count.lock().unwrap();
            let index = *count;
            *count += 1;
            index
        };
        let fail = self.fail_on_commit_index == Some(index);
        let certainty = self.commit_failure_certainty;
        Box::pin(async move {
            if fail {
                Err(match certainty {
                    Some(certainty) => ClassifiedCommitError::Driver {
                        certainty,
                        message: "injected rejected commit response for testing".to_string(),
                    },
                    None => ClassifiedCommitError::from(OrmError::Transaction(
                        "injected ambiguous commit response for testing".to_string(),
                    )),
                })
            } else {
                Ok(())
            }
        })
    }

    fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.log.lock().unwrap().push(MockEvent::Rollback);
        Box::pin(async { Ok(()) })
    }

    fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.log.lock().unwrap().push(MockEvent::Close);
        let index = {
            let mut count = self.close_count.lock().unwrap();
            let index = *count;
            *count += 1;
            index
        };
        let fail = self.fail_on_close_index == Some(index);
        Box::pin(async move {
            if fail {
                Err(OrmError::Transaction(
                    "injected close failure for testing".to_owned(),
                ))
            } else {
                Ok(())
            }
        })
    }
}
