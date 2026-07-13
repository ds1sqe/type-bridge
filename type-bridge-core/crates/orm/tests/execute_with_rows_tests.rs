//! Offline lifecycle tests for [`Database::execute_with_rows`].

use std::sync::{Arc, Mutex};

use type_bridge_core_lib::version::Version;
use type_bridge_orm::session::backend::{
    BoxFuture, DriverBackend, GivenRowsSpec, QueryResult, TransactionOps,
};
use type_bridge_orm::{Database, OrmError, TxType};

type Events = Arc<Mutex<Vec<&'static str>>>;

struct LifecycleBackend {
    events: Events,
    query_fails: bool,
    commit_fails: bool,
}

impl DriverBackend for LifecycleBackend {
    fn open_transaction(
        &self,
        _database: &str,
        _tx_type: TxType,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
        let events = Arc::clone(&self.events);
        let query_fails = self.query_fails;
        let commit_fails = self.commit_fails;
        Box::pin(async move {
            events.lock().unwrap().push("open");
            Ok(Box::new(LifecycleTransaction {
                events,
                query_fails,
                commit_fails,
            }) as Box<dyn TransactionOps>)
        })
    }

    fn is_open(&self) -> bool {
        true
    }

    fn server_version(&self) -> Option<Version> {
        Some(Version::new(3, 12, 0))
    }

    fn supports_given_rows(&self) -> bool {
        true
    }
}

struct LifecycleTransaction {
    events: Events,
    query_fails: bool,
    commit_fails: bool,
}

impl TransactionOps for LifecycleTransaction {
    fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        Box::pin(async { panic!("execute_with_rows used the raw query path") })
    }

    fn query_with_rows(
        &mut self,
        _typeql: &str,
        _rows: GivenRowsSpec,
    ) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        self.events.lock().unwrap().push("query");
        let query_fails = self.query_fails;
        Box::pin(async move {
            if query_fails {
                Err(OrmError::QueryExecution("query failed".into()))
            } else {
                Ok(QueryResult::Ok)
            }
        })
    }

    fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.events.lock().unwrap().push("commit");
        let commit_fails = self.commit_fails;
        Box::pin(async move {
            if commit_fails {
                Err(OrmError::Transaction("commit failed".into()))
            } else {
                Ok(())
            }
        })
    }

    fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.events.lock().unwrap().push("rollback");
        Box::pin(async { Ok(()) })
    }

    fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.events.lock().unwrap().push("close");
        Box::pin(async { Ok(()) })
    }
}

fn database(query_fails: bool, commit_fails: bool) -> (Database, Events) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let backend = LifecycleBackend {
        events: Arc::clone(&events),
        query_fails,
        commit_fails,
    };
    (Database::with_backend(Box::new(backend), "testdb"), events)
}

fn empty_rows() -> GivenRowsSpec {
    GivenRowsSpec {
        variables: vec!["n".into()],
        rows: vec![],
    }
}

#[tokio::test]
async fn execute_with_rows_write_success_commits_and_closes() {
    let (database, events) = database(false, false);

    let result = database
        .execute_with_rows("given $n: string; insert ...;", TxType::Write, empty_rows())
        .await
        .expect("write query should succeed");

    assert!(matches!(result, QueryResult::Ok));
    assert_eq!(
        *events.lock().unwrap(),
        ["open", "query", "commit", "close"]
    );
}

#[tokio::test]
async fn execute_with_rows_read_success_closes_without_commit() {
    let (database, events) = database(false, false);

    let result = database
        .execute_with_rows("given $n: string; match ...;", TxType::Read, empty_rows())
        .await
        .expect("read query should succeed");

    assert!(matches!(result, QueryResult::Ok));
    assert_eq!(*events.lock().unwrap(), ["open", "query", "close"]);
}

#[tokio::test]
async fn execute_with_rows_query_error_rolls_back_and_closes() {
    let (database, events) = database(true, false);

    let error = database
        .execute_with_rows("given $n: string; insert ...;", TxType::Write, empty_rows())
        .await
        .expect_err("query failure should be returned");

    assert!(
        matches!(error, OrmError::QueryExecution(ref message) if message == "query failed"),
        "unexpected error: {error}"
    );
    assert_eq!(
        *events.lock().unwrap(),
        ["open", "query", "rollback", "close"]
    );
}

#[tokio::test]
async fn execute_with_rows_commit_error_still_closes() {
    let (database, events) = database(false, true);

    let error = database
        .execute_with_rows("given $n: string; insert ...;", TxType::Write, empty_rows())
        .await
        .expect_err("commit failure should be returned");

    assert!(
        matches!(error, OrmError::Transaction(ref message) if message == "commit failed"),
        "unexpected error: {error}"
    );
    assert_eq!(
        *events.lock().unwrap(),
        ["open", "query", "commit", "close"]
    );
}
