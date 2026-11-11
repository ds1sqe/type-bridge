//! Native dynamic managers and hydration contracts.

mod common;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use common::*;
use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, QueryResult, TransactionOps};
use type_bridge_orm::*;

#[path = "support/internal.rs"]
mod internal;
use internal::*;

#[derive(Debug, Default)]
struct RecordingState {
    opens: Vec<TxType>,
    queries: Vec<String>,
    query_modes: Vec<&'static str>,
    commits: usize,
    rollbacks: usize,
    closes: usize,
}

#[test]
fn dynamic_relation_identity_dto_is_owned_and_serializable() {
    let identity = DynamicRelationIdentity {
        iid: "0x1".into(),
        type_name: "employment".into(),
    };
    let encoded = serde_json::to_value(&identity).unwrap();
    assert_eq!(
        encoded,
        serde_json::json!({"iid":"0x1","type_name":"employment"})
    );
    assert_eq!(
        serde_json::from_value::<DynamicRelationIdentity>(encoded).unwrap(),
        identity
    );
}

#[test]
fn dynamic_relation_identity_hydration_accepts_exact_direct_and_wrapped() {
    use type_bridge_orm::_manager::hydration::hydrate_dynamic_relation_identity;
    let direct = serde_json::json!({"_iid":"0x1","_type":"employment"});
    let wrapped = serde_json::json!({"_iid":{"value":"0x2"},"_type":{"value":"contract"}});
    assert_eq!(
        hydrate_dynamic_relation_identity("employment", &direct).unwrap(),
        DynamicRelationIdentity {
            iid: "0x1".into(),
            type_name: "employment".into()
        }
    );
    assert_eq!(
        hydrate_dynamic_relation_identity("employment", &wrapped).unwrap(),
        DynamicRelationIdentity {
            iid: "0x2".into(),
            type_name: "contract".into()
        }
    );
}

#[test]
fn dynamic_relation_identity_hydration_rejects_exact_malformed_matrix() {
    use type_bridge_orm::_manager::hydration::hydrate_dynamic_relation_identity;
    let cases = [
        (
            serde_json::json!(null),
            "Expected JSON object for relation identity discovery",
        ),
        (
            serde_json::json!({}),
            "Relation identity discovery omitted its IID",
        ),
        (
            serde_json::json!({"_iid":"", "_type":"employment"}),
            "Relation identity discovery returned a blank IID",
        ),
        (
            serde_json::json!({"_iid":1, "_type":"employment"}),
            "Relation identity discovery returned a nonstring IID",
        ),
        (
            serde_json::json!({"_iid":{"value":1}, "_type":"employment"}),
            "Relation identity discovery returned a nonstring IID",
        ),
        (
            serde_json::json!({"_iid":"0x1"}),
            "Relation identity discovery omitted its concrete type",
        ),
        (
            serde_json::json!({"_iid":"0x1", "_type":""}),
            "Relation identity discovery returned a blank concrete type",
        ),
        (
            serde_json::json!({"_iid":"0x1", "_type":1}),
            "Relation identity discovery returned a nonstring concrete type",
        ),
        (
            serde_json::json!({"_iid":"0x1", "_type":{"value":1}}),
            "Relation identity discovery returned a nonstring concrete type",
        ),
        (
            serde_json::json!({"_iid":"bad", "_type":"employment"}),
            "Relation identity discovery returned a noncanonical IID",
        ),
    ];
    for (doc, message) in cases {
        assert!(
            matches!(hydrate_dynamic_relation_identity("employment", &doc), Err(OrmError::Hydration { type_name, message: actual }) if type_name == "employment" && actual == message)
        );
    }
}

enum RecordingResponse {
    Result(QueryResult),
    Error(&'static str),
}

struct RecordingBackend {
    responses: Arc<Mutex<VecDeque<RecordingResponse>>>,
    state: Arc<Mutex<RecordingState>>,
    rollback_error: bool,
    commit_error: bool,
    close_error: bool,
}

impl RecordingBackend {
    fn new(responses: Vec<RecordingResponse>) -> (Self, Arc<Mutex<RecordingState>>) {
        Self::with_rollback_error(responses, false)
    }

    fn with_rollback_error(
        responses: Vec<RecordingResponse>,
        rollback_error: bool,
    ) -> (Self, Arc<Mutex<RecordingState>>) {
        let state = Arc::new(Mutex::new(RecordingState::default()));
        (
            Self {
                responses: Arc::new(Mutex::new(responses.into())),
                state: Arc::clone(&state),
                rollback_error,
                commit_error: false,
                close_error: false,
            },
            state,
        )
    }

    fn with_failures(
        responses: Vec<RecordingResponse>,
        rollback_error: bool,
        commit_error: bool,
        close_error: bool,
    ) -> (Self, Arc<Mutex<RecordingState>>) {
        let state = Arc::new(Mutex::new(RecordingState::default()));
        (
            Self {
                responses: Arc::new(Mutex::new(responses.into())),
                state: Arc::clone(&state),
                rollback_error,
                commit_error,
                close_error,
            },
            state,
        )
    }
}

impl DriverBackend for RecordingBackend {
    fn open_transaction(
        &self,
        _database: &str,
        tx_type: TxType,
    ) -> BoxFuture<'_, std::result::Result<Box<dyn TransactionOps>, OrmError>> {
        self.state.lock().unwrap().opens.push(tx_type);
        let transaction = RecordingTransaction {
            responses: Arc::clone(&self.responses),
            state: Arc::clone(&self.state),
            rollback_error: self.rollback_error,
            commit_error: self.commit_error,
            close_error: self.close_error,
        };
        Box::pin(async move { Ok(Box::new(transaction) as Box<dyn TransactionOps>) })
    }

    fn is_open(&self) -> bool {
        true
    }
}

struct RecordingTransaction {
    responses: Arc<Mutex<VecDeque<RecordingResponse>>>,
    state: Arc<Mutex<RecordingState>>,
    rollback_error: bool,
    commit_error: bool,
    close_error: bool,
}

impl TransactionOps for RecordingTransaction {
    fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult>> {
        self.state.lock().unwrap().query_modes.push("legacy");
        self.state.lock().unwrap().queries.push(typeql.to_string());
        let response = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(RecordingResponse::Result(QueryResult::Ok));
        Box::pin(async move {
            match response {
                RecordingResponse::Result(result) => Ok(result),
                RecordingResponse::Error(message) => {
                    Err(OrmError::QueryExecution(message.to_string()))
                }
            }
        })
    }

    fn query_canonical(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult>> {
        self.state.lock().unwrap().query_modes.push("canonical");
        self.state.lock().unwrap().queries.push(typeql.to_string());
        let response = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(RecordingResponse::Result(QueryResult::Ok));
        Box::pin(async move {
            match response {
                RecordingResponse::Result(result) => Ok(result),
                RecordingResponse::Error(message) => {
                    Err(OrmError::QueryExecution(message.to_string()))
                }
            }
        })
    }

    fn commit(&mut self) -> BoxFuture<'_, Result<()>> {
        self.state.lock().unwrap().commits += 1;
        let fail = self.commit_error;
        Box::pin(async move {
            if fail {
                Err(OrmError::Transaction("commit failed".into()))
            } else {
                Ok(())
            }
        })
    }

    fn rollback(&mut self) -> BoxFuture<'_, Result<()>> {
        self.state.lock().unwrap().rollbacks += 1;
        let rollback_error = self.rollback_error;
        Box::pin(async move {
            if rollback_error {
                Err(OrmError::Transaction("recording rollback failed".into()))
            } else {
                Ok(())
            }
        })
    }

    fn close(&mut self) -> BoxFuture<'_, Result<()>> {
        self.state.lock().unwrap().closes += 1;
        let fail = self.close_error;
        Box::pin(async move {
            if fail {
                Err(OrmError::Transaction("close failed".into()))
            } else {
                Ok(())
            }
        })
    }
}

fn assert_canonical_mode_and_counts(
    state: &Arc<Mutex<RecordingState>>,
    opens: &[TxType],
    commits: usize,
    rollbacks: usize,
    closes: usize,
) {
    let s = state.lock().unwrap();
    assert_eq!(s.opens, opens);
    assert_eq!(s.query_modes, vec!["canonical"]);
    assert_eq!(s.commits, commits);
    assert_eq!(s.rollbacks, rollbacks);
    assert_eq!(s.closes, closes);
}

fn person_attrs() -> DynamicAttributeMap {
    vec![
        ("name".into(), AttributeValue::String("Alice".into())),
        ("age".into(), AttributeValue::Long(30)),
    ]
}

fn employment_attrs(position: &str) -> DynamicAttributeMap {
    vec![("position".into(), AttributeValue::String(position.into()))]
}

fn count_aggregate() -> DynamicAggregate {
    DynamicAggregate {
        result_key: "count".into(),
        function: "count".into(),
        attr_name: None,
    }
}

fn employment_role_players() -> Vec<DynamicRolePlayerInput> {
    vec![
        DynamicRolePlayerInput {
            role_name: "employee".into(),
            player_type_name: "person".into(),
            iid: Some("0xa001".into()),
            key: None,
        },
        DynamicRolePlayerInput {
            role_name: "employer".into(),
            player_type_name: "company".into(),
            iid: Some("0xc001".into()),
            key: None,
        },
    ]
}

// ── Phase 1 Gap A: expression-tree-filtered aggregate and group-by ────────────

// ── Phase 1 Gap A: relation aggregate_with_query / group_by_aggregate_with_query ──

// ── Phase 1 Gap B: StartsWith / EndsWith ─────────────────────────────────────

/// Records which query path a batch insert takes on a server that supports
/// given rows.
struct GivenRowsBackend {
    calls: Arc<Mutex<Vec<&'static str>>>,
}

impl DriverBackend for GivenRowsBackend {
    fn open_transaction(
        &self,
        _database: &str,
        _tx_type: TxType,
    ) -> BoxFuture<'_, std::result::Result<Box<dyn TransactionOps>, OrmError>> {
        let calls = Arc::clone(&self.calls);
        Box::pin(
            async move { Ok(Box::new(GivenRowsTransaction { calls }) as Box<dyn TransactionOps>) },
        )
    }

    fn is_open(&self) -> bool {
        true
    }

    fn server_version(&self) -> Option<type_bridge_core_lib::version::Version> {
        Some(type_bridge_core_lib::version::Version::new(3, 13, 6))
    }

    fn supports_given_rows(&self) -> bool {
        true
    }
}

struct GivenRowsTransaction {
    calls: Arc<Mutex<Vec<&'static str>>>,
}

impl TransactionOps for GivenRowsTransaction {
    fn supports_given_rows(&self) -> bool {
        true
    }
    fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult>> {
        self.calls.lock().unwrap().push("query");
        Box::pin(async {
            Ok(QueryResult::Documents(vec![
                serde_json::json!({"iid": "0x9"}),
            ]))
        })
    }

    fn query_canonical(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult>> {
        self.calls.lock().unwrap().push("query_canonical");
        Box::pin(async {
            Ok(QueryResult::Documents(vec![
                serde_json::json!({"iid": "0x9"}),
            ]))
        })
    }

    fn query_with_rows(
        &mut self,
        _typeql: &str,
        _rows: type_bridge_orm::session::backend::GivenRowsSpec,
    ) -> BoxFuture<'_, Result<QueryResult>> {
        self.calls.lock().unwrap().push("query_with_rows");
        // Answers arrive out of row order; the manager maps them back.
        Box::pin(async {
            Ok(QueryResult::Documents(vec![
                serde_json::json!({"_iid":"0xb","_type":"person","attributes":{"name":"Bob","age":25},"ordinal":1}),
                serde_json::json!({"_iid":"0xa","_type":"person","attributes":{"name":"Alice","age":30},"ordinal":0}),
            ]))
        })
    }

    fn commit(&mut self) -> BoxFuture<'_, Result<()>> {
        self.calls.lock().unwrap().push("commit");
        Box::pin(async { Ok(()) })
    }

    fn rollback(&mut self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn close(&mut self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

fn native_person_document(iid: &str) -> serde_json::Value {
    serde_json::json!({"_iid":iid,"_type":"person","attributes":{"name":"Alice","age":30}})
}

fn native_employment_document(iid: &str) -> serde_json::Value {
    serde_json::json!({
        "_iid":iid,"_type":"employment","attributes":{"position":"Engineer"},
        "_role_0_players":[native_person_document("0x02")],
        "_role_1_players":[{"_iid":"0x03","_type":"company","attributes":{"name":"Acme"}}]
    })
}

fn native_employment_scalar_document(iid: &str) -> serde_json::Value {
    serde_json::json!({"_iid":iid,"_type":"employment","attributes":{"position":"Engineer"},
        "_role_0_iid":"0x02","_role_0_type":"person","_role_0_attributes":{"name":"Alice","age":30},
        "_role_1_iid":"0x03","_role_1_type":"company","_role_1_attributes":{"name":"Acme"}})
}

fn native_role_players() -> Vec<DynamicRolePlayerInput> {
    let mut players = employment_role_players();
    players[0].iid = Some("0x02".into());
    players[1].iid = Some("0x03".into());
    players
}

fn native_people(db: &Database) -> DynamicEntityManager<'_> {
    let registry = manager_registry();
    let descriptor = registry.entity("person").unwrap();
    DynamicEntityManager::from_registry(db, registry, descriptor)
}

fn native_employments(db: &Database) -> DynamicRelationManager<'_> {
    let registry = manager_registry();
    let descriptor = registry.relation("employment").unwrap();
    DynamicRelationManager::from_registry(db, registry, descriptor)
}

#[tokio::test]
async fn native_entity_single_results_preserve_every_root_and_transaction_ownership() {
    for operation in [
        "insert",
        "put",
        "put_exact",
        "update",
        "update_exact",
        "update_and_get_exact",
    ] {
        for empty in [false, true] {
            for borrowed in [false, true] {
                let count = if empty {
                    0
                } else if operation.ends_with("exact") && operation != "put_exact" {
                    1
                } else {
                    2
                };
                let documents = ["0x11", "0x12"]
                    .into_iter()
                    .take(count)
                    .map(native_person_document)
                    .collect();
                let (backend, state) = RecordingBackend::new(vec![RecordingResponse::Result(
                    QueryResult::Documents(documents),
                )]);
                let db = Database::with_backend(Box::new(backend), "testdb");
                let tx = if borrowed {
                    Some(db.transaction_context(TxType::Write).await.unwrap())
                } else {
                    None
                };
                let manager = if let Some(tx) = &tx {
                    DynamicEntityManager::with_registry_transaction(
                        tx.clone(),
                        manager_registry(),
                        manager_registry().entity("person").unwrap(),
                    )
                    .literal_values()
                } else {
                    native_people(&db)
                };
                let attrs = person_attrs();
                let result = match operation {
                    "insert" => manager.insert(&attrs).await,
                    "put" => manager.put(&attrs).await,
                    "put_exact" => manager.put_exact(&attrs).await,
                    "update" => manager.update(None, &attrs).await,
                    "update_exact" => manager.update_exact("0x11", &attrs).await,
                    _ => manager.update_and_get_exact("0x11", &attrs).await,
                }
                .unwrap();
                assert_eq!(result.count(), count, "{operation} borrowed={borrowed}");
                assert_eq!(result.item().is_some(), count == 1);
                assert_eq!(
                    result
                        .items()
                        .iter()
                        .map(|row| row.iid.as_deref().unwrap())
                        .collect::<Vec<_>>(),
                    ["0x11", "0x12"][..count]
                );
                assert!(result.items().iter().all(|row| row.attributes
                    == vec![
                        ("age".into(), AttributeValue::Long(30)),
                        ("name".into(), AttributeValue::String("Alice".into()))
                    ]));
                {
                    let state = state.lock().unwrap();
                    assert_eq!(state.opens, [TxType::Write]);
                    assert_eq!(state.queries.len(), 1);
                    assert_eq!(state.query_modes, ["canonical"]);
                    assert_eq!(state.commits, usize::from(!borrowed));
                    assert_eq!(state.rollbacks, 0);
                    for forbidden in ["count(", "limit 1", "__tb_guard"] {
                        assert!(
                            !state.queries[0].contains(forbidden),
                            "{operation}: {forbidden}"
                        );
                    }
                }
                if let Some(tx) = tx {
                    tx.commit_sdk().await.unwrap();
                }
                assert_eq!(state.lock().unwrap().commits, 1);
            }
        }
    }
}

#[tokio::test]
async fn native_relation_single_results_preserve_every_root_and_all_players() {
    for operation in [
        "insert",
        "put",
        "put_exact",
        "update",
        "update_exact",
        "update_and_get_exact",
    ] {
        for empty in [false, true] {
            for borrowed in [false, true] {
                let count = if empty {
                    0
                } else if operation.ends_with("exact") && operation != "put_exact" {
                    1
                } else {
                    2
                };
                let docs = ["0x11", "0x12"]
                    .into_iter()
                    .take(count)
                    .map(native_employment_document)
                    .collect();
                let (backend, state) = RecordingBackend::new(vec![RecordingResponse::Result(
                    QueryResult::Documents(docs),
                )]);
                let db = Database::with_backend(Box::new(backend), "testdb");
                let tx = if borrowed {
                    Some(db.transaction_context(TxType::Write).await.unwrap())
                } else {
                    None
                };
                let registry = manager_registry();
                let manager = if let Some(tx) = &tx {
                    DynamicRelationManager::with_registry_transaction(
                        tx.clone(),
                        Arc::clone(&registry),
                        registry.relation("employment").unwrap(),
                    )
                    .literal_values()
                } else {
                    native_employments(&db)
                };
                let attrs = employment_attrs("Engineer");
                let players = native_role_players();
                let result = match operation {
                    "insert" => manager.insert(&attrs, &players).await,
                    "put" => manager.put(&attrs, &players).await,
                    "put_exact" => manager.put_exact(&attrs, &players).await,
                    "update" => manager.update(None, &attrs, &players).await,
                    "update_exact" => manager.update_exact("0x11", &attrs, &players).await,
                    _ => manager.update_and_get_exact("0x11", &attrs, &players).await,
                }
                .unwrap();
                assert_eq!(result.count(), count, "{operation} borrowed={borrowed}");
                assert_eq!(result.item().is_some(), count == 1);
                for row in result.items() {
                    assert_eq!(row.attributes, attrs);
                    assert_eq!(row.role_players.len(), 2);
                    assert!(
                        row.role_players
                            .iter()
                            .any(|player| player.player_iid.as_deref() == Some("0x02"))
                    );
                    assert!(
                        row.role_players
                            .iter()
                            .any(|player| player.player_iid.as_deref() == Some("0x03"))
                    );
                }
                {
                    let state = state.lock().unwrap();
                    assert_eq!(state.opens, [TxType::Write]);
                    assert_eq!(state.queries.len(), 1);
                    assert_eq!(state.query_modes, ["canonical"]);
                    assert_eq!(state.commits, usize::from(!borrowed));
                    assert_eq!(state.rollbacks, 0);
                    for forbidden in ["count(", "limit 1", "__tb_guard"] {
                        assert!(!state.queries[0].contains(forbidden));
                    }
                }
                if let Some(tx) = tx {
                    tx.commit_sdk().await.unwrap();
                }
                assert_eq!(state.lock().unwrap().commits, 1);
            }
        }
    }
}

#[tokio::test]
async fn native_single_malformed_answers_rollback_owned_and_poison_borrowed_writes() {
    for relation in [false, true] {
        let good = if relation {
            native_employment_document("0x11")
        } else {
            native_person_document("0x11")
        };
        let mut bad_documents = vec![
            serde_json::json!({}),
            good.clone(),
            good.clone(),
            good.clone(),
            good.clone(),
        ];
        bad_documents[1]["_iid"] = serde_json::json!("invalid");
        bad_documents[2]["_type"] = serde_json::json!("company");
        bad_documents[3]["attributes"] = serde_json::json!({"name":false});
        bad_documents[4]
            .as_object_mut()
            .unwrap()
            .remove(if relation { "_role_0_players" } else { "_type" });
        let mut answers = bad_documents
            .into_iter()
            .map(|doc| QueryResult::Documents(vec![doc]))
            .collect::<Vec<_>>();
        answers.extend([QueryResult::Ok, QueryResult::Rows(vec![])]);
        for (case, answer) in answers.into_iter().enumerate() {
            for borrowed in [false, true] {
                let (backend, state) = RecordingBackend::with_rollback_error(
                    vec![RecordingResponse::Result(answer.clone())],
                    true,
                );
                let db = Database::with_backend(Box::new(backend), "testdb");
                let tx = if borrowed {
                    Some(db.transaction_context(TxType::Write).await.unwrap())
                } else {
                    None
                };
                let registry = manager_registry();
                let result = if relation {
                    let manager = if let Some(tx) = &tx {
                        DynamicRelationManager::with_registry_transaction(
                            tx.clone(),
                            Arc::clone(&registry),
                            registry.relation("employment").unwrap(),
                        )
                        .literal_values()
                    } else {
                        native_employments(&db)
                    };
                    manager
                        .insert(&employment_attrs("Engineer"), &native_role_players())
                        .await
                        .map(|_| ())
                } else {
                    let manager = if let Some(tx) = &tx {
                        DynamicEntityManager::with_registry_transaction(
                            tx.clone(),
                            Arc::clone(&registry),
                            registry.entity("person").unwrap(),
                        )
                        .literal_values()
                    } else {
                        native_people(&db)
                    };
                    manager.insert(&person_attrs()).await.map(|_| ())
                };
                let error = result.expect_err(&format!(
                    "relation={relation} borrowed={borrowed} case={case}"
                ));
                assert!(!error.to_string().contains("recording rollback failed"));
                if let Some(tx) = tx {
                    assert!(tx.commit_sdk().await.is_err());
                    assert!(tx.rollback().await.is_err());
                }
                let state = state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Write]);
                assert_eq!(state.queries.len(), 1);
                assert_eq!(state.commits, 0);
                assert_eq!(state.rollbacks, 1);
            }
        }
    }
}

#[tokio::test]
async fn native_write_provider_failure_keeps_primary_and_owned_commit_failure_is_not_rolled_back() {
    for relation in [false, true] {
        for commit_failure in [false, true] {
            let response = if commit_failure {
                RecordingResponse::Result(QueryResult::Documents(vec![if relation {
                    native_employment_document("0x11")
                } else {
                    native_person_document("0x11")
                }]))
            } else {
                RecordingResponse::Error("primary write failed")
            };
            let (backend, state) =
                RecordingBackend::with_failures(vec![response], true, commit_failure, true);
            let db = Database::with_backend(Box::new(backend), "testdb");
            let error = if relation {
                native_employments(&db)
                    .insert(&employment_attrs("Engineer"), &native_role_players())
                    .await
                    .map(|_| ())
                    .unwrap_err()
            } else {
                native_people(&db)
                    .insert(&person_attrs())
                    .await
                    .map(|_| ())
                    .unwrap_err()
            };
            if commit_failure {
                let OrmError::ExecutionDiagnostic(diagnostic) = error else {
                    panic!("classified commit failure");
                };
                assert_eq!(diagnostic.code().as_str(), "commit_outcome_unknown");
            } else {
                assert!(
                    matches!(error, OrmError::QueryExecution(message) if message == "primary write failed")
                );
            }
            let state = state.lock().unwrap();
            assert_eq!(state.opens, [TxType::Write]);
            assert_eq!(state.queries.len(), 1);
            assert_eq!(state.commits, usize::from(commit_failure));
            assert_eq!(state.rollbacks, usize::from(!commit_failure));
        }
    }
}

#[tokio::test]
async fn native_exact_mutation_input_validation_precedes_provider_io() {
    let (backend, state) = RecordingBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let people = native_people(&db);
    assert!(
        people
            .update_exact("invalid", &person_attrs())
            .await
            .is_err()
    );
    assert!(people.delete_by_iid_exact("invalid").await.is_err());
    assert!(
        people
            .update_exact("0x11", &vec![("unknown".into(), AttributeValue::Long(1))])
            .await
            .is_err()
    );
    let relations = native_employments(&db);
    assert!(
        relations
            .update_exact(
                "invalid",
                &employment_attrs("Engineer"),
                &native_role_players()
            )
            .await
            .is_err()
    );
    assert!(
        relations
            .put_exact(&employment_attrs("Engineer"), &[])
            .await
            .is_err()
    );
    let mut wrong_role = native_role_players();
    wrong_role[0].role_name = "unknown".into();
    assert!(
        relations
            .update_exact("0x11", &employment_attrs("Engineer"), &wrong_role)
            .await
            .is_err()
    );
    let state = state.lock().unwrap();
    assert!(state.opens.is_empty());
    assert!(state.queries.is_empty());
    assert_eq!((state.commits, state.rollbacks, state.closes), (0, 0, 0));
}

#[tokio::test]
async fn native_borrowed_read_transactions_refuse_every_single_write_without_queries() {
    let (backend, state) = RecordingBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let tx = db.transaction_context(TxType::Read).await.unwrap();
    let registry = manager_registry();
    let people = DynamicEntityManager::with_registry_transaction(
        tx.clone(),
        Arc::clone(&registry),
        registry.entity("person").unwrap(),
    )
    .literal_values();
    assert!(people.insert(&person_attrs()).await.is_err());
    assert!(people.put_exact(&person_attrs()).await.is_err());
    assert!(people.update_exact("0x11", &person_attrs()).await.is_err());
    let relations = DynamicRelationManager::with_registry_transaction(
        tx.clone(),
        Arc::clone(&registry),
        registry.relation("employment").unwrap(),
    )
    .literal_values();
    assert!(
        relations
            .insert(&employment_attrs("Engineer"), &native_role_players())
            .await
            .is_err()
    );
    assert!(
        relations
            .put_exact(&employment_attrs("Engineer"), &native_role_players())
            .await
            .is_err()
    );
    assert!(
        relations
            .update_exact(
                "0x11",
                &employment_attrs("Engineer"),
                &native_role_players()
            )
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    let state = state.lock().unwrap();
    assert_eq!(state.opens, [TxType::Read]);
    assert!(state.queries.is_empty());
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 1);
}

#[tokio::test]
async fn canonical_insert_many_sends_one_given_rows_query() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let db = Database::with_backend(
        Box::new(GivenRowsBackend {
            calls: Arc::clone(&calls),
        }),
        "testdb",
    );
    let iids = native_people(&db)
        .insert_many(&[
            person_attrs(),
            vec![
                ("name".into(), AttributeValue::String("Bob".into())),
                ("age".into(), AttributeValue::Long(25)),
            ],
        ])
        .await
        .unwrap();
    assert_eq!(iids, ["0xa", "0xb"]);
    assert_eq!(*calls.lock().unwrap(), ["query_with_rows", "commit"]);
}

#[tokio::test]
async fn native_exact_duplicate_iids_and_key_aliases_follow_typeql_player_semantics() {
    for operation in ["insert", "put_exact", "update_exact"] {
        for key_alias in [false, true] {
            let (backend, state) = RecordingBackend::new(vec![RecordingResponse::Result(
                QueryResult::Documents(vec![native_employment_document("0x11")]),
            )]);
            let db = Database::with_backend(Box::new(backend), "testdb");
            let mut players = native_role_players();
            let duplicate = if key_alias {
                DynamicRolePlayerInput {
                    iid: None,
                    key: Some(("name".into(), AttributeValue::String("Alice".into()))),
                    ..players[0].clone()
                }
            } else {
                players[0].clone()
            };
            players.push(duplicate);
            let manager = native_employments(&db);
            let result = match operation {
                "insert" => {
                    manager
                        .insert(&employment_attrs("Engineer"), &players)
                        .await
                }
                "put_exact" => {
                    manager
                        .put_exact(&employment_attrs("Engineer"), &players)
                        .await
                }
                _ => {
                    manager
                        .update_exact("0x11", &employment_attrs("Engineer"), &players)
                        .await
                }
            }
            .unwrap();
            assert_eq!(result.count(), 1);
            assert_eq!(result.item().unwrap().role_players.len(), 2);
            let state = state.lock().unwrap();
            assert_eq!(state.queries.len(), 1);
            assert_eq!(state.commits, 1);
            assert_eq!(state.rollbacks, 0);
            assert!(!state.queries[0].contains("!= "));
        }
    }
}

#[tokio::test]
async fn native_read_cleanup_preserves_primary_errors_and_reports_read_close_failure() {
    for relation in [false, true] {
        for query_failure in [false, true] {
            let response = if query_failure {
                RecordingResponse::Error("primary read failed")
            } else {
                RecordingResponse::Result(QueryResult::Documents(vec![]))
            };
            let (backend, state) =
                RecordingBackend::with_failures(vec![response], false, false, true);
            let db = Database::with_backend(Box::new(backend), "testdb");
            let error = if relation {
                native_employments(&db).all().await.map(|_| ()).unwrap_err()
            } else {
                native_people(&db).all().await.map(|_| ()).unwrap_err()
            };
            assert!(
                matches!(error, OrmError::QueryExecution(ref message) if query_failure && message == "primary read failed")
                    || matches!(error, OrmError::Transaction(ref message) if !query_failure && message == "close failed"),
                "{error:?}"
            );
            assert_canonical_mode_and_counts(&state, &[TxType::Read], 0, 0, 1);
        }
    }
}

#[tokio::test]
async fn native_read_terminals_preserve_filters_windows_not_found_and_scalar_results() {
    use type_bridge_contract::value::CanonicalValue;
    for relation in [false, true] {
        let document = if relation {
            native_employment_scalar_document("0x11")
        } else {
            native_person_document("0x11")
        };
        let responses = vec![
            QueryResult::Documents(vec![document.clone()]),
            QueryResult::Documents(vec![document]),
            QueryResult::Documents(vec![]),
            QueryResult::Rows(vec![
                serde_json::json!({"count1":{"kind":"value","value":CanonicalValue::Long(3)}}),
            ]),
            QueryResult::Rows(vec![
                serde_json::json!({"root0":{"kind":"thing","iid":"0x11","type_id":{"kind":if relation {"relation"} else {"entity"},"label":if relation {"employment"} else {"person"}}}}),
            ]),
            QueryResult::Rows(vec![]),
        ];
        let (backend, state) = RecordingBackend::new(
            responses
                .into_iter()
                .map(RecordingResponse::Result)
                .collect(),
        );
        let db = Database::with_backend(Box::new(backend), "testdb");
        if relation {
            let manager = native_employments(&db);
            let rows = manager
                .get_exact(&[Filter::string_eq("position", "Engineer")])
                .await
                .unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].role_players.len(), 2);
            assert_eq!(
                manager
                    .first_exact_with_query(&[])
                    .await
                    .unwrap()
                    .unwrap()
                    .iid
                    .as_deref(),
                Some("0x11")
            );
            assert!(matches!(
                manager.get_one(&[]).await,
                Err(OrmError::NotFound(_))
            ));
            assert_eq!(manager.count_exact().await.unwrap(), 3);
            assert!(manager.exists_exact_with_query(&[]).await.unwrap());
            assert!(!manager.exists_exact_with_query(&[]).await.unwrap());
        } else {
            let manager = native_people(&db);
            let rows = manager
                .get_exact(&[Filter::string_eq("name", "Alice")])
                .await
                .unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].iid.as_deref(), Some("0x11"));
            assert_eq!(
                manager
                    .first_exact_with_query(&[])
                    .await
                    .unwrap()
                    .unwrap()
                    .iid
                    .as_deref(),
                Some("0x11")
            );
            assert!(matches!(
                manager.get_one(&[]).await,
                Err(OrmError::NotFound(_))
            ));
            assert_eq!(manager.count_exact().await.unwrap(), 3);
            assert!(manager.exists_exact_with_query(&[]).await.unwrap());
            assert!(!manager.exists_exact_with_query(&[]).await.unwrap());
        }
        let state = state.lock().unwrap();
        assert_eq!(state.opens, [TxType::Read; 6]);
        assert_eq!(state.queries.len(), 6);
        let label = if relation { "employment" } else { "person" };
        assert!(state.queries[0].contains(&format!("isa! {label}")));
        assert!(state.queries[0].contains(if relation { "has position" } else { "has name" }));
        assert!(state.queries[1].contains("limit 1"));
        assert!(state.queries[3].contains("reduce $count1 = count"));
        for query in &state.queries[3..] {
            assert!(!query.contains("fetch"));
        }
        assert_eq!((state.commits, state.rollbacks, state.closes), (0, 0, 6));
    }
}

#[tokio::test]
async fn native_many_results_preserve_input_order_atomicity_and_borrowed_ownership() {
    for relation in [false, true] {
        for operation in ["insert", "put", "put_exact"] {
            for borrowed in [false, true] {
                let documents = [(1, "0x12"), (0, "0x11")]
                    .into_iter()
                    .map(|(ordinal, iid)| {
                        let mut doc = if relation {
                            native_employment_document(iid)
                        } else {
                            native_person_document(iid)
                        };
                        doc["ordinal"] = serde_json::json!(ordinal);
                        // Bulk writes retain the existing scalar role projection.
                        if relation {
                            doc.as_object_mut()
                                .unwrap()
                                .retain(|key, _| !key.starts_with("_role_"));
                            doc["_role_0_iid"] = serde_json::json!("0x02");
                            doc["_role_0_type"] = serde_json::json!("person");
                            doc["_role_0_attributes"] =
                                serde_json::json!({"name":"Alice","age":30});
                            doc["_role_1_iid"] = serde_json::json!("0x03");
                            doc["_role_1_type"] = serde_json::json!("company");
                            doc["_role_1_attributes"] = serde_json::json!({"name":"Acme"});
                        }
                        doc
                    })
                    .collect();
                let answer = if relation && operation == "put" {
                    QueryResult::Rows(vec![serde_json::json!({
                        "row0":{"kind":"thing","iid":"0x11","type_id":{"kind":"relation","label":"employment"}},
                        "row4":{"kind":"thing","iid":"0x12","type_id":{"kind":"relation","label":"employment"}}
                    })])
                } else if operation == "put" {
                    QueryResult::Rows([(1, "0x12"), (0, "0x11")].into_iter().map(|(ordinal, iid)| serde_json::json!({
                        "root0":{"kind":"thing","iid":iid,"type_id":{"kind":if relation {"relation"} else {"entity"},"label":if relation {"employment"} else {"person"}}},
                        "ordinal1":{"kind":"value","value":type_bridge_contract::value::CanonicalValue::Long(ordinal)}
                    })).collect())
                } else {
                    QueryResult::Documents(documents)
                };
                let (backend, state) =
                    RecordingBackend::new(vec![RecordingResponse::Result(answer)]);
                let db = Database::with_backend(Box::new(backend), "testdb");
                let tx = if borrowed {
                    Some(db.transaction_context(TxType::Write).await.unwrap())
                } else {
                    None
                };
                let registry = manager_registry();
                let iids = if relation {
                    let manager = if let Some(tx) = &tx {
                        DynamicRelationManager::with_registry_transaction(
                            tx.clone(),
                            Arc::clone(&registry),
                            registry.relation("employment").unwrap(),
                        )
                        .literal_values()
                    } else {
                        native_employments(&db)
                    };
                    let items = [
                        (employment_attrs("Engineer"), native_role_players()),
                        (employment_attrs("Engineer"), native_role_players()),
                    ];
                    match operation {
                        "insert" => manager.insert_many(&items).await,
                        "put" => manager.put_many(&items).await,
                        _ => manager.put_many_exact(&items).await,
                    }
                } else {
                    let manager = if let Some(tx) = &tx {
                        DynamicEntityManager::with_registry_transaction(
                            tx.clone(),
                            Arc::clone(&registry),
                            registry.entity("person").unwrap(),
                        )
                        .literal_values()
                    } else {
                        native_people(&db)
                    };
                    let items = [person_attrs(), vec![("name".into(), AttributeValue::String("Bob".into())), ("age".into(), AttributeValue::Long(25))]];
                    match operation {
                        "insert" => manager.insert_many(&items).await,
                        "put" => manager.put_many(&items).await,
                        _ => manager.put_many_exact(&items).await,
                    }
                }
                .unwrap_or_else(|error| panic!("relation={relation} operation={operation} borrowed={borrowed}: {error:?}; queries={:?}", state.lock().unwrap().queries));
                assert_eq!(
                    iids,
                    ["0x11", "0x12"],
                    "relation={relation} operation={operation} borrowed={borrowed}"
                );
                {
                    let state = state.lock().unwrap();
                    assert_eq!(state.opens, [TxType::Write]);
                    assert_eq!(state.queries.len(), 1);
                    assert_eq!(state.query_modes, ["canonical"]);
                    assert_eq!(
                        (state.commits, state.rollbacks),
                        (usize::from(!borrowed), 0)
                    );
                }
                if let Some(tx) = tx {
                    tx.commit_sdk().await.unwrap();
                }
                assert_eq!(state.lock().unwrap().commits, 1);
            }
        }
    }
}

#[tokio::test]
async fn native_empty_batches_are_zero_io_and_late_answer_failure_is_atomic() {
    let (backend, state) = RecordingBackend::new(vec![]);
    let db = Database::with_backend(Box::new(backend), "testdb");
    let people = native_people(&db);
    let relations = native_employments(&db);
    assert!(people.insert_many(&[]).await.unwrap().is_empty());
    assert!(people.put_many(&[]).await.unwrap().is_empty());
    assert!(people.put_many_exact(&[]).await.unwrap().is_empty());
    assert!(
        people
            .update_many_and_get_exact(&[])
            .await
            .unwrap()
            .is_empty()
    );
    people.delete_many_by_iid_exact(&[]).await.unwrap();
    assert!(relations.insert_many(&[]).await.unwrap().is_empty());
    assert!(relations.put_many(&[]).await.unwrap().is_empty());
    assert!(relations.put_many_exact(&[]).await.unwrap().is_empty());
    assert!(
        relations
            .update_many_and_get_exact(&[])
            .await
            .unwrap()
            .is_empty()
    );
    relations.delete_many_by_iid_exact(&[]).await.unwrap();
    assert!(state.lock().unwrap().opens.is_empty());
    assert!(state.lock().unwrap().queries.is_empty());
    for relation in [false, true] {
        for borrowed in [false, true] {
            let good = if relation {
                native_employment_document("0x11")
            } else {
                native_person_document("0x11")
            };
            let mut first = good.clone();
            first["ordinal"] = serde_json::json!(0);
            let mut second = good;
            second["ordinal"] = serde_json::json!(1);
            second["_iid"] = serde_json::json!("invalid");
            let (backend, state) = RecordingBackend::new(vec![RecordingResponse::Result(
                QueryResult::Documents(vec![first, second]),
            )]);
            let db = Database::with_backend(Box::new(backend), "testdb");
            let tx = if borrowed {
                Some(db.transaction_context(TxType::Write).await.unwrap())
            } else {
                None
            };
            let registry = manager_registry();
            let result = if relation {
                let manager = if let Some(tx) = &tx {
                    DynamicRelationManager::with_registry_transaction(
                        tx.clone(),
                        Arc::clone(&registry),
                        registry.relation("employment").unwrap(),
                    )
                    .literal_values()
                } else {
                    native_employments(&db)
                };
                manager
                    .insert_many(&[
                        (employment_attrs("Engineer"), native_role_players()),
                        (employment_attrs("Engineer"), native_role_players()),
                    ])
                    .await
            } else {
                let manager = if let Some(tx) = &tx {
                    DynamicEntityManager::with_registry_transaction(
                        tx.clone(),
                        Arc::clone(&registry),
                        registry.entity("person").unwrap(),
                    )
                    .literal_values()
                } else {
                    native_people(&db)
                };
                manager.insert_many(&[person_attrs(), person_attrs()]).await
            };
            assert!(result.is_err());
            if let Some(tx) = tx {
                assert!(tx.commit_sdk().await.is_err());
                tx.rollback().await.unwrap();
            }
            let state = state.lock().unwrap();
            assert_eq!(state.opens, [TxType::Write]);
            assert_eq!(state.commits, 0);
            assert_eq!(state.rollbacks, 1);
        }
    }
}

fn native_identity_rows(relation: bool, iids: &[&str]) -> QueryResult {
    QueryResult::Rows(iids.iter().map(|iid| serde_json::json!({"root0":{"kind":"thing","iid":iid,"type_id":{"kind":if relation {"relation"} else {"entity"},"label":if relation {"employment"} else {"person"}}}})).collect())
}

#[tokio::test]
async fn native_identity_discovery_preserves_order_and_rejects_wrong_or_duplicate_iids() {
    for relation in [false, true] {
        let responses = vec![
            native_identity_rows(relation, &["0x12", "0x11"]),
            native_identity_rows(relation, &[]),
            native_identity_rows(relation, &["0x11"]),
            native_identity_rows(relation, &["0x12"]),
            native_identity_rows(relation, &["0x11", "0x11"]),
        ];
        let (backend, state) = RecordingBackend::new(
            responses
                .into_iter()
                .map(RecordingResponse::Result)
                .collect(),
        );
        let db = Database::with_backend(Box::new(backend), "testdb");
        if relation {
            let manager = native_employments(&db);
            assert_eq!(
                manager
                    .discover_all()
                    .await
                    .unwrap()
                    .iter()
                    .map(|row| row.iid.as_str())
                    .collect::<Vec<_>>(),
                ["0x12", "0x11"]
            );
            assert!(manager.discover_by_iid("0x11").await.unwrap().is_none());
            assert_eq!(
                manager.discover_by_iid("0x11").await.unwrap().unwrap().iid,
                "0x11"
            );
            assert!(manager.discover_by_iid("0x11").await.is_err());
            assert!(manager.discover_by_iid("0x11").await.is_err());
            assert!(manager.discover_by_iid("bad").await.is_err());
        } else {
            let manager = native_people(&db);
            assert_eq!(
                manager
                    .discover_all()
                    .await
                    .unwrap()
                    .iter()
                    .map(|row| row.iid.as_str())
                    .collect::<Vec<_>>(),
                ["0x12", "0x11"]
            );
            assert!(manager.discover_by_iid("0x11").await.unwrap().is_none());
            assert_eq!(
                manager.discover_by_iid("0x11").await.unwrap().unwrap().iid,
                "0x11"
            );
            assert!(manager.discover_by_iid("0x11").await.is_err());
            assert!(manager.discover_by_iid("0x11").await.is_err());
            assert!(manager.discover_by_iid("bad").await.is_err());
        }
        let state = state.lock().unwrap();
        assert_eq!(state.opens, [TxType::Read; 5]);
        assert_eq!(state.queries.len(), 5);
        assert!(
            state
                .queries
                .iter()
                .all(|query| !query.contains("fetch") && !query.contains("attributes"))
        );
        assert_eq!((state.commits, state.rollbacks, state.closes), (0, 0, 5));
    }
}

#[tokio::test]
async fn native_identity_discovery_rejects_malformed_typed_evidence() {
    for relation in [false, true] {
        for answer in [
            QueryResult::Ok,
            QueryResult::Documents(vec![]),
            QueryResult::Rows(vec![serde_json::json!({})]),
            QueryResult::Rows(vec![
                serde_json::json!({"root0":{"kind":"thing","iid":"invalid","type_id":{"kind":"entity","label":"person"}}}),
            ]),
            QueryResult::Rows(vec![
                serde_json::json!({"root0":{"kind":"thing","iid":"0x11","type_id":{"kind":"entity","label":"company"}}}),
            ]),
        ] {
            let (backend, state) = RecordingBackend::new(vec![RecordingResponse::Result(answer)]);
            let db = Database::with_backend(Box::new(backend), "testdb");
            let result = if relation {
                native_employments(&db).discover_all().await.map(|_| ())
            } else {
                native_people(&db).discover_all().await.map(|_| ())
            };
            assert!(result.is_err());
            assert_canonical_mode_and_counts(&state, &[TxType::Read], 0, 0, 1);
        }
    }
}

#[tokio::test]
async fn native_by_iid_reads_refuse_foreign_roots_and_coalesce_players() {
    for relation in [false, true] {
        for exact in [false, true] {
            for wrong in [false, true] {
                let document = if relation {
                    native_employment_scalar_document(if wrong { "0x12" } else { "0x11" })
                } else {
                    native_person_document(if wrong { "0x12" } else { "0x11" })
                };
                let (backend, state) = RecordingBackend::new(vec![RecordingResponse::Result(
                    QueryResult::Documents(vec![document.clone(), document]),
                )]);
                let db = Database::with_backend(Box::new(backend), "testdb");
                let result = if relation {
                    let manager = native_employments(&db);
                    let result = if exact {
                        manager.get_by_iid_exact("0x11").await
                    } else {
                        manager.get_by_iid("0x11").await
                    };
                    result.map(|rows| {
                        assert_eq!(rows.len(), 1);
                        assert_eq!(rows[0].role_players.len(), 2);
                    })
                } else {
                    let manager = native_people(&db);
                    let result = if exact {
                        manager.get_by_iid_exact("0x11").await
                    } else {
                        manager.get_by_iid("0x11").await
                    };
                    result.map(|row| {
                        assert_eq!(row.unwrap().iid.as_deref(), Some("0x11"));
                    })
                };
                assert_eq!(result.is_err(), wrong);
                assert_canonical_mode_and_counts(&state, &[TxType::Read], 0, 0, 1);
            }
        }
    }
}

#[tokio::test]
async fn native_aggregate_and_group_terminals_keep_expression_filters_database_side() {
    use type_bridge_contract::value::{CanonicalString, CanonicalValue};
    for relation in [false, true] {
        let field = if relation { "position" } else { "name" };
        let expected = if relation { "Engineer" } else { "Alice" };
        let expression = DynamicExpr::Or {
            exprs: vec![
                DynamicExpr::Compare {
                    attr_name: field.into(),
                    operator: DynamicComparisonOp::Eq,
                    value: AttributeValue::String(expected.into()),
                },
                DynamicExpr::Not {
                    expr: Box::new(DynamicExpr::Compare {
                        attr_name: field.into(),
                        operator: DynamicComparisonOp::Eq,
                        value: AttributeValue::String("excluded".into()),
                    }),
                },
            ],
        };
        let responses = vec![
            QueryResult::Rows(vec![
                serde_json::json!({"reduced1":{"kind":"value","value":CanonicalValue::Long(3)}}),
            ]),
            QueryResult::Rows(vec![
                serde_json::json!({"reduced3":{"kind":"value","value":CanonicalValue::Long(3)}}),
            ]),
            QueryResult::Rows(vec![
                serde_json::json!({"field1":{"kind":"attribute","type_id":{"kind":"attribute","label":field},"value":CanonicalValue::String(CanonicalString::new(expected).unwrap())},"reduced2":{"kind":"value","value":CanonicalValue::Long(3)}}),
            ]),
        ];
        let (backend, state) = RecordingBackend::new(
            responses
                .into_iter()
                .map(RecordingResponse::Result)
                .collect(),
        );
        let db = Database::with_backend(Box::new(backend), "testdb");
        let result = if relation {
            let manager = native_employments(&db);
            assert_eq!(
                manager.aggregate(&[], &[count_aggregate()]).await.unwrap()[0]["count"],
                3
            );
            assert_eq!(
                manager
                    .aggregate_with_query(std::slice::from_ref(&expression), &[count_aggregate()])
                    .await
                    .unwrap()[0]["count"],
                3
            );
            manager
                .group_by_aggregate(&[], &[field.into()], &[count_aggregate()])
                .await
                .unwrap()
        } else {
            let manager = native_people(&db);
            assert_eq!(
                manager.aggregate(&[], &[count_aggregate()]).await.unwrap()[0]["count"],
                3
            );
            assert_eq!(
                manager
                    .aggregate_with_query(std::slice::from_ref(&expression), &[count_aggregate()])
                    .await
                    .unwrap()[0]["count"],
                3
            );
            manager
                .group_by_aggregate(&[], &[field.into()], &[count_aggregate()])
                .await
                .unwrap()
        };
        assert_eq!(result[0]["group0"], expected);
        assert_eq!(result[0]["count"], 3);
        let state = state.lock().unwrap();
        assert_eq!(state.opens, [TxType::Read; 3]);
        assert!(
            state
                .queries
                .iter()
                .all(|query| query.contains("reduce") && !query.contains("fetch"))
        );
        assert!(state.queries[1].contains(" or ") && state.queries[1].contains("not"));
        assert!(state.queries[2].contains(&format!("has {field}")));
        assert_eq!((state.commits, state.rollbacks, state.closes), (0, 0, 3));
    }
}

#[tokio::test]
async fn native_inclusive_write_hydrates_concrete_subtype_fields_and_exact_write_refuses_them() {
    let schema = MANAGER_SCHEMA.replacen("  company:\n", "  employee:\n    sub: person\n    owns:\n      position: { card: { min: 0, max: 1 } }\n  company:\n", 1);
    let registry = manager_registry_for_schema(&schema);
    for exact in [false, true] {
        let document = serde_json::json!({"_iid":"0x11","_type":"employee","attributes":{"name":"Alice","age":30,"position":"Engineer"}});
        let (backend, state) = RecordingBackend::new(vec![RecordingResponse::Result(
            QueryResult::Documents(vec![document]),
        )]);
        let db = Database::with_backend(Box::new(backend), "testdb");
        let manager = DynamicEntityManager::from_registry(
            &db,
            Arc::clone(&registry),
            registry.entity("person").unwrap(),
        );
        let result = if exact {
            manager.put_exact(&person_attrs()).await
        } else {
            manager.put(&person_attrs()).await
        };
        if exact {
            assert!(result.is_err());
        } else {
            let result = result.unwrap();
            assert_eq!(result.count(), 1);
            let row = result.item().unwrap();
            assert_eq!(row.type_name.as_deref(), Some("employee"));
            assert!(
                row.attributes
                    .contains(&("position".into(), AttributeValue::String("Engineer".into())))
            );
        }
        let state = state.lock().unwrap();
        assert_eq!(state.queries.len(), 1);
        assert_eq!(state.commits, usize::from(!exact));
        assert_eq!(state.rollbacks, usize::from(exact));
    }
}

#[tokio::test]
async fn native_key_only_exact_updates_hydrate_with_one_read_plan() {
    let registry = manager_registry_for_schema(
        "format: typebridge.schema/v2\nattributes:\n  name: { value: string }\nentities:\n  person:\n    owns:\n      name: { key: true }\n",
    );
    for present in [false, true] {
        let docs = if present {
            vec![serde_json::json!({"_iid":"0x11","_type":"person","attributes":{"name":"Alice"}})]
        } else {
            vec![]
        };
        let (backend, state) = RecordingBackend::new(vec![RecordingResponse::Result(
            QueryResult::Documents(docs),
        )]);
        let db = Database::with_backend(Box::new(backend), "testdb");
        let manager = DynamicEntityManager::from_registry(
            &db,
            Arc::clone(&registry),
            registry.entity("person").unwrap(),
        );
        let result = manager
            .update_exact(
                "0x11",
                &vec![("name".into(), AttributeValue::String("Alice".into()))],
            )
            .await
            .unwrap();
        assert_eq!(result.count(), usize::from(present));
        assert_eq!(result.item().is_some(), present);
        if let Some(row) = result.item() {
            assert_eq!(
                row.attributes,
                vec![("name".into(), AttributeValue::String("Alice".into()))]
            );
        }
        let state = state.lock().unwrap();
        assert_eq!(state.opens, [TxType::Write]);
        assert_eq!(state.queries.len(), 1);
        assert!(state.queries[0].contains("fetch"));
        for forbidden in ["insert", "delete", "put", "count("] {
            assert!(!state.queries[0].contains(forbidden));
        }
        assert_eq!((state.commits, state.rollbacks), (1, 0));
    }
}

#[tokio::test]
async fn native_successful_write_commit_keeps_all_results_without_a_second_close() {
    for relation in [false, true] {
        let docs = ["0x11", "0x12"]
            .into_iter()
            .map(|iid| {
                if relation {
                    native_employment_document(iid)
                } else {
                    native_person_document(iid)
                }
            })
            .collect();
        let (backend, state) = RecordingBackend::with_failures(
            vec![RecordingResponse::Result(QueryResult::Documents(docs))],
            false,
            false,
            true,
        );
        let db = Database::with_backend(Box::new(backend), "testdb");
        let count = if relation {
            native_employments(&db)
                .insert(&employment_attrs("Engineer"), &native_role_players())
                .await
                .unwrap()
                .count()
        } else {
            native_people(&db)
                .insert(&person_attrs())
                .await
                .unwrap()
                .count()
        };
        assert_eq!(count, 2);
        assert_canonical_mode_and_counts(&state, &[TxType::Write], 1, 0, 0);
    }
}

#[tokio::test]
async fn native_borrowed_reads_reuse_canonical_context_without_lifecycle_changes() {
    for relation in [false, true] {
        for kind in [TxType::Read, TxType::Write] {
            let document = if relation {
                native_employment_scalar_document("0x11")
            } else {
                native_person_document("0x11")
            };
            let (backend, state) = RecordingBackend::new(vec![RecordingResponse::Result(
                QueryResult::Documents(vec![document]),
            )]);
            let db = Database::with_backend(Box::new(backend), "testdb");
            let tx = db.transaction_context(kind).await.unwrap();
            let registry = manager_registry();
            let count = if relation {
                DynamicRelationManager::with_registry_transaction(
                    tx.clone(),
                    Arc::clone(&registry),
                    registry.relation("employment").unwrap(),
                )
                .all()
                .await
                .unwrap()
                .len()
            } else {
                DynamicEntityManager::with_registry_transaction(
                    tx.clone(),
                    Arc::clone(&registry),
                    registry.entity("person").unwrap(),
                )
                .all()
                .await
                .unwrap()
                .len()
            };
            assert_eq!(count, 1);
            assert_canonical_mode_and_counts(&state, &[kind], 0, 0, 0);
            tx.rollback().await.unwrap();
            assert_eq!(
                tx.lifecycle_state().await,
                TransactionContextState::RolledBack
            );
            assert_canonical_mode_and_counts(&state, &[kind], 0, 1, 0);
            tx.close().await.unwrap();
            assert_eq!(tx.lifecycle_state().await, TransactionContextState::Closed);
            assert_canonical_mode_and_counts(&state, &[kind], 0, 1, 1);
        }
    }
}
