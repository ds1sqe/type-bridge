//! In-memory mock `DriverBackend` for planner/executor unit tests.
//!
//! The mock records every interaction in an ordered event log so tests can
//! assert the exact sequence of opens, queries, commits, and rollbacks that
//! the executor issued.  A failure-injection knob lets tests verify
//! rollback-on-error and run-halt behavior without a live TypeDB connection.
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
/// `fail_on_query_index` is the 0-indexed position across ALL query calls
/// (across all transactions in a single `execute_plan` run) that should return
/// an error.  `None` means all queries succeed.
pub struct MockMigrationBackend {
    log: EventLog,
    /// Global query counter shared across all spawned transactions.
    query_count: Arc<Mutex<usize>>,
    /// If `Some(n)`, the n-th query call returns an error.
    fail_on_query_index: Option<usize>,
}

impl MockMigrationBackend {
    /// Create a new mock backend and return it together with the shared event log.
    ///
    /// Pass `fail_on_query_index = Some(n)` to make the n-th `query` call
    /// (0-indexed across the whole run) return an error.
    pub fn new(fail_on_query_index: Option<usize>) -> (Self, EventLog) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let backend = Self {
            log: Arc::clone(&log),
            query_count: Arc::new(Mutex::new(0)),
            fail_on_query_index,
        };
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
        Box::pin(async move {
            let tx: Box<dyn TransactionOps> = Box::new(MockMigrationTransaction {
                tx_type,
                log,
                query_count,
                fail_on,
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

        let should_fail = self.fail_on == Some(idx);
        let typeql_owned = typeql.to_string();
        let tx_type = self.tx_type;

        // Record the query event regardless of success/failure so the test can
        // assert that the query was attempted before the rollback event.
        self.log
            .lock()
            .unwrap()
            .push(MockEvent::Query(tx_type, typeql_owned));

        Box::pin(async move {
            if should_fail {
                Err(OrmError::Transaction(
                    "injected query failure for testing".to_string(),
                ))
            } else {
                Ok(QueryResult::Ok)
            }
        })
    }

    fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.log.lock().unwrap().push(MockEvent::Commit);
        Box::pin(async { Ok(()) })
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
