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

use std::sync::{Arc, Mutex};

use type_bridge_orm::OrmError;
use type_bridge_orm::TxType;
use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, QueryResult, TransactionOps};

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
        };
        (backend, log)
    }

    /// Create a backend whose selected commit returns an ambiguous error.
    pub fn with_commit_failure(fail_on_commit_index: usize) -> (Self, EventLog) {
        let (mut backend, log) = Self::new(None);
        backend.fail_on_commit_index = Some(fail_on_commit_index);
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
        Box::pin(async move {
            let tx: Box<dyn TransactionOps> = Box::new(MockMigrationTransaction {
                tx_type,
                log,
                query_count,
                fail_on,
                scripted_responses: scripted,
                commit_count,
                fail_on_commit_index,
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
}

impl TransactionOps for MockMigrationTransaction {
    fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
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
        Box::pin(async move {
            if fail {
                Err(OrmError::Transaction(
                    "injected ambiguous commit response for testing".to_string(),
                ))
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
        Box::pin(async { Ok(()) })
    }
}
