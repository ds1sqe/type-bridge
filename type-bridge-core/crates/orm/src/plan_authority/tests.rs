use super::*;
use crate::session::backend::{
    AnswerCancellation, BoxFuture, DriverBackend, QueryResult, TransactionOps, TxType,
};
use std::collections::VecDeque;
use std::sync::Mutex;

#[derive(Default)]
struct Evidence {
    opened: usize,
    queries: Vec<String>,
    canonical_calls: usize,
    closed: usize,
    hold: bool,
    holding: Arc<tokio::sync::Notify>,
    answers: VecDeque<QueryResult>,
}
struct Backend(Arc<Mutex<Evidence>>);
struct Provider(Arc<Mutex<Evidence>>);
impl DriverBackend for Backend {
    fn open_transaction(
        &self,
        _: &str,
        _: TxType,
    ) -> BoxFuture<'_, crate::Result<Box<dyn TransactionOps>>> {
        self.0.lock().unwrap().opened += 1;
        let provider = Provider(Arc::clone(&self.0));
        Box::pin(async move { Ok(Box::new(provider) as Box<dyn TransactionOps>) })
    }
    fn is_open(&self) -> bool {
        true
    }
}
impl TransactionOps for Provider {
    fn query(&mut self, text: &str) -> BoxFuture<'_, crate::Result<QueryResult>> {
        let mut state = self.0.lock().unwrap();
        state.queries.push(text.to_owned());
        let hold = state.hold;
        let holding = Arc::clone(&state.holding);
        let answer = state.answers.pop_front();
        Box::pin(async move {
            if hold {
                holding.notify_one();
                std::future::pending::<()>().await;
            }
            Ok(answer.expect("expected statement count"))
        })
    }
    fn query_canonical(&mut self, text: &str) -> BoxFuture<'_, crate::Result<QueryResult>> {
        self.0.lock().unwrap().canonical_calls += 1;
        self.query(text)
    }
    fn commit(&mut self) -> BoxFuture<'_, crate::Result<()>> {
        panic!("authority must not commit")
    }
    fn rollback(&mut self) -> BoxFuture<'_, crate::Result<()>> {
        panic!("authority must not roll back")
    }
    fn close(&mut self) -> BoxFuture<'_, crate::Result<()>> {
        self.0.lock().unwrap().closed += 1;
        Box::pin(async { Ok(()) })
    }
}
fn typed_rows(documents: Vec<serde_json::Value>) -> QueryResult {
    use type_bridge_contract::value::{CanonicalString, CanonicalValue};
    QueryResult::Rows(
        documents
            .into_iter()
            .map(|document| {
                let fields = document
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(key, value)| {
                        let value = match value {
                            serde_json::Value::String(value) => {
                                CanonicalValue::String(CanonicalString::new(value).unwrap())
                            }
                            value => CanonicalValue::Long(value.as_i64().unwrap()),
                        };
                        (
                            key.clone(),
                            serde_json::json!({"kind":"value","value":value}),
                        )
                    })
                    .collect::<serde_json::Map<_, _>>();
                serde_json::Value::Object(fields)
            })
            .collect(),
    )
}

fn control(scope: &str, fence: &str, state: &str) -> serde_json::Value {
    serde_json::json!({"scope":scope,"fence":fence,"state":state})
}

#[tokio::test]
async fn native_control_checks_keep_two_statements_and_refusal_codes() {
    for (rows, holders, expected) in [
        (vec![control("scope", "1", "free")], vec![], None),
        (vec![], vec![], Some("query_prepared_control_invalid")),
        (
            vec![control("scope", "1", "free"); 2],
            vec![],
            Some("query_prepared_control_invalid"),
        ),
        (
            vec![control("other", "1", "free")],
            vec![],
            Some("query_prepared_managed_scope_mismatch"),
        ),
        (
            vec![control("scope", "01", "free")],
            vec![],
            Some("query_prepared_control_invalid"),
        ),
        (
            vec![control("scope", "0", "free")],
            vec![],
            Some("query_prepared_control_invalid"),
        ),
        (
            vec![control("scope", "1", "held")],
            vec![],
            Some("query_prepared_migration_in_progress"),
        ),
        (
            vec![control("scope", "1", "free")],
            vec![serde_json::json!({"holder":"writer"})],
            Some("query_prepared_migration_in_progress"),
        ),
        (
            vec![serde_json::json!({"scope":"scope","fence":7,"state":"free"})],
            vec![],
            Some("query_prepared_control_invalid"),
        ),
    ] {
        let reaches_holder = rows == vec![control("scope", "1", "free")];
        let evidence = Arc::new(Mutex::new(Evidence {
            answers: VecDeque::from([typed_rows(rows), typed_rows(holders)]),
            ..Default::default()
        }));
        let db =
            crate::Database::with_backend(Box::new(Backend(Arc::clone(&evidence))), "authority");
        let mut tx = db.read_transaction().await.unwrap();
        let result = verify_managed_query_control_scope(
            &mut tx,
            &ManagedScopeId::new("scope").unwrap(),
            None,
            &AnswerCancellation::default(),
        )
        .await;
        assert_eq!(
            result.as_ref().err().map(|error| error.code().as_str()),
            expected
        );
        {
            let state = evidence.lock().unwrap();
            assert_eq!(state.opened, 1);
            assert_eq!(state.closed, 0);
            assert_eq!(state.canonical_calls, if reaches_holder { 2 } else { 1 });
            assert_eq!(state.queries.len(), state.canonical_calls);
            assert!(state.queries[0].contains("limit 2;"));
            assert!(state.queries[0].contains("has typebridge-internal-control-scope $scope"));
            if reaches_holder {
                assert!(state.queries[1].contains("limit 1;"));
                assert!(state.queries[1].contains("has typebridge-internal-lease-holder $holder"));
            }
        }
        tx.close().await.unwrap();
    }
}

#[tokio::test]
async fn native_control_checks_preserve_preflight_deadline_and_cancellation() {
    for cancelled in [false, true] {
        let evidence = Arc::new(Mutex::new(Evidence::default()));
        let db =
            crate::Database::with_backend(Box::new(Backend(Arc::clone(&evidence))), "authority");
        let mut tx = db.read_transaction().await.unwrap();
        let cancellation = AnswerCancellation::default();
        let deadline = if cancelled {
            cancellation.cancel();
            None
        } else {
            Some(std::time::Instant::now() - std::time::Duration::from_secs(1))
        };
        let error = verify_managed_query_control_scope(
            &mut tx,
            &ManagedScopeId::new("scope").unwrap(),
            deadline,
            &cancellation,
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.code().as_str(),
            if cancelled {
                "provider_cancelled"
            } else {
                "transaction_deadline_exceeded"
            }
        );
        assert_eq!(evidence.lock().unwrap().canonical_calls, 0);
        tx.close().await.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn native_control_reads_obey_inflight_deadline_and_cancellation() {
    for cancelled in [false, true] {
        let evidence = Arc::new(Mutex::new(Evidence {
            hold: true,
            ..Default::default()
        }));
        let db =
            crate::Database::with_backend(Box::new(Backend(Arc::clone(&evidence))), "authority");
        let mut tx = db.read_transaction().await.unwrap();
        let cancellation = AnswerCancellation::default();
        let scope = ManagedScopeId::new("scope").unwrap();
        let holding = Arc::clone(&evidence.lock().unwrap().holding);
        let deadline =
            (!cancelled).then(|| std::time::Instant::now() + std::time::Duration::from_secs(3600));
        let execute = verify_managed_query_control_scope(&mut tx, &scope, deadline, &cancellation);
        let cancel = async {
            holding.notified().await;
            if cancelled {
                cancellation.cancel();
            } else {
                tokio::time::advance(std::time::Duration::from_secs(3601)).await;
            }
        };
        let (result, ()) = tokio::join!(execute, cancel);
        assert_eq!(
            result.unwrap_err().code().as_str(),
            if cancelled {
                "provider_cancelled"
            } else {
                "transaction_deadline_exceeded"
            }
        );
        assert_eq!(evidence.lock().unwrap().canonical_calls, 1);
        assert_eq!(evidence.lock().unwrap().closed, 0);
        tx.close().await.unwrap();
    }
}

#[tokio::test]
async fn callback_borrow_requires_the_exact_active_write_fence() {
    for (rows, allowed) in [
        (vec![], false),
        (
            vec![
                serde_json::json!({"scope":"scope", "fence":"1", "state":"free", "holder":"engine"}),
            ],
            false,
        ),
        (
            vec![
                serde_json::json!({"scope":"scope", "fence":"2", "state":"held", "holder":"engine"}),
            ],
            false,
        ),
        (
            vec![
                serde_json::json!({"scope":"other", "fence":"1", "state":"held", "holder":"engine"}),
            ],
            false,
        ),
        (
            vec![
                serde_json::json!({"scope":"scope", "fence":"1", "state":"held", "holder":"engine"}),
            ],
            true,
        ),
    ] {
        let evidence = Arc::new(Mutex::new(Evidence {
            answers: VecDeque::from([typed_rows(rows)]),
            ..Default::default()
        }));
        let db =
            crate::Database::with_backend(Box::new(Backend(Arc::clone(&evidence))), "authority");
        let mut tx = db.write_transaction().await.unwrap();
        let scope = ManagedScopeId::new("scope").unwrap();
        {
            let borrowed = tx
                .borrow_migration_context(
                    &db,
                    &scope,
                    1,
                    "engine",
                    &AnswerCancellation::default(),
                    crate::QueryExecutionDeadline::from_timeout_milliseconds(0),
                )
                .await;
            assert_eq!(borrowed.is_ok(), allowed);
            if let Ok(borrowed) = borrowed {
                assert!(borrowed.context().commit().await.is_err());
                borrowed.finish().await.unwrap();
            }
        }
        assert_eq!(evidence.lock().unwrap().canonical_calls, 1);
        tx.close().await.unwrap();
    }
}

#[tokio::test]
async fn import_fallback_requires_all_current_authority_rows_absent() {
    for present in [None, Some(0), Some(1), Some(2)] {
        let mut answers = VecDeque::from([typed_rows(vec![])]);
        for index in 0..3 {
            answers.push_back(typed_rows(if present == Some(index) {
                vec![serde_json::json!({})]
            } else {
                vec![]
            }));
            if present == Some(index) {
                break;
            }
        }
        let evidence = Arc::new(Mutex::new(Evidence {
            answers,
            ..Default::default()
        }));
        let db =
            crate::Database::with_backend(Box::new(Backend(Arc::clone(&evidence))), "authority");
        let mut tx = db.read_transaction().await.unwrap();
        let result =
            query_import_pending(&mut tx, true, None, &AnswerCancellation::default()).await;
        if present.is_some() {
            assert_eq!(
                result.unwrap_err().code().as_str(),
                "query_prepared_control_invalid"
            );
        } else {
            assert!(result.unwrap());
        }
        tx.close().await.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn callback_fence_read_observes_inflight_cancellation_and_deadline() {
    for cancel in [true, false] {
        let evidence = Arc::new(Mutex::new(Evidence {
            hold: true,
            ..Default::default()
        }));
        let entered = Arc::clone(&evidence.lock().unwrap().holding);
        let db =
            crate::Database::with_backend(Box::new(Backend(Arc::clone(&evidence))), "authority");
        let mut tx = db.write_transaction().await.unwrap();
        let cancellation = AnswerCancellation::default();
        let scope = ManagedScopeId::new("scope").unwrap();
        let deadline = crate::QueryExecutionDeadline::from_instant(
            std::time::Instant::now() + std::time::Duration::from_secs(3600),
        );
        let completed = async {
            let result = tx
                .borrow_migration_context(&db, &scope, 1, "engine", &cancellation, deadline)
                .await;
            assert!(result.is_err());
        };
        tokio::pin!(completed);
        tokio::select! {
            () = &mut completed => panic!("fence read returned before pending provider I/O"),
            () = entered.notified() => {},
        }
        if cancel {
            cancellation.cancel();
        } else {
            tokio::time::advance(std::time::Duration::from_secs(3601)).await;
        }
        tokio::time::timeout(std::time::Duration::from_secs(5), &mut completed)
            .await
            .unwrap();
        assert_eq!(evidence.lock().unwrap().canonical_calls, 1);
    }
}
