//! One-shot plans share staged answer evidence and preserve read ownership.
#[path = "support/plan_v3_race.rs"]
mod race;

use serde_json::json;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use type_bridge_contract::migration_assertion::ValueComparator;
use type_bridge_contract::query_plan::v3::Operation;
use type_bridge_orm::session::backend::{
    AnswerCancellation, BoxFuture, DriverBackend, GivenRowsSpec, QueryResult, TransactionOps,
    TxType,
};
use type_bridge_orm::session::staged::PlanAnswer;
use type_bridge_orm::{Database, OrmError, QueryExecutionDeadline, QueryExecutionResourceLimits};

#[derive(Default)]
struct State {
    events: Vec<&'static str>,
    answers: VecDeque<QueryResult>,
    cancel_after_query: Option<AnswerCancellation>,
}
struct Fake(Arc<Mutex<State>>);
impl DriverBackend for Fake {
    fn open_transaction(
        &self,
        _: &str,
        kind: TxType,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
        assert_eq!(kind, TxType::Read);
        self.0.lock().unwrap().events.push("open");
        let state = Arc::clone(&self.0);
        Box::pin(async move { Ok(Box::new(Fake(state)) as Box<dyn TransactionOps>) })
    }
    fn is_open(&self) -> bool {
        true
    }
    fn supports_given_rows(&self) -> bool {
        true
    }
    fn server_version(&self) -> Option<type_bridge_core_lib::version::Version> {
        "3.13.6".parse().ok()
    }
}
impl TransactionOps for Fake {
    fn supports_given_rows(&self) -> bool {
        true
    }
    fn query(&mut self, _: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        let mut state = self.0.lock().unwrap();
        state.events.push("query");
        if let Some(cancellation) = &state.cancel_after_query {
            cancellation.cancel();
        }
        let answer = state
            .answers
            .pop_front()
            .expect("one answer per dispatched statement");
        Box::pin(async move { Ok(answer) })
    }
    fn query_with_rows(
        &mut self,
        query: &str,
        _: GivenRowsSpec,
    ) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        self.query(query)
    }
    fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        panic!("one-shot reads must never commit")
    }
    fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        Box::pin(async { Ok(()) })
    }
    fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.0.lock().unwrap().events.push("close");
        Box::pin(async { Ok(()) })
    }
}
fn fixture(answers: Vec<QueryResult>) -> (Database, Arc<Mutex<State>>) {
    let state = Arc::new(Mutex::new(State {
        answers: answers.into(),
        ..State::default()
    }));
    (
        Database::with_backend(Box::new(Fake(Arc::clone(&state))), "plans"),
        state,
    )
}
fn row() -> QueryResult {
    QueryResult::Rows(vec![json!({})])
}
fn documents() -> QueryResult {
    QueryResult::Documents(vec![
        json!({"_iid":"0x01", "_type":"doctor", "attributes":{"name":"Ada", "on-call":true}}),
    ])
}

#[tokio::test]
async fn direct_rows_documents_counts_exists_and_reductions_use_staged_normalization() {
    let cases = [
        (
            race::stock_read(Operation::Rows),
            QueryResult::Rows(vec![
                json!({"stock":{"category":"attribute", "label":"stock", "value_type":"integer", "value":2}}),
            ]),
        ),
        (
            race::string_read(ValueComparator::Contains, &["Ada"]),
            documents(),
        ),
        (race::stock_read(Operation::Count), row()),
        (race::stock_read(Operation::Exists), row()),
        (
            race::stock_reduce_count(),
            QueryResult::Rows(vec![
                json!({"total":{"category":"value", "value_type":"integer", "value":1}}),
            ]),
        ),
    ];
    let policy = QueryExecutionResourceLimits::default();
    for (plan, provider) in cases {
        let (db, state) = fixture(vec![provider]);
        let answers = db
            .execute_plans(
                std::slice::from_ref(&plan),
                &policy,
                &AnswerCancellation::default(),
                QueryExecutionDeadline::for_limits(policy),
            )
            .await
            .unwrap();
        assert_eq!(answers.len(), 1);
        let staged = answers[0].to_staged_v3(&plan).unwrap();
        assert_eq!(
            answers[0],
            PlanAnswer::from_staged_v3(&staged, &plan).unwrap()
        );
        assert_eq!(state.lock().unwrap().events, ["open", "query", "close"]);
        if let PlanAnswer::Rows(rows) = &answers[0] {
            let object = rows[0].as_object().unwrap();
            assert!(object.values().all(|value| value.get("kind").is_some()));
        }
    }
}

#[tokio::test]
async fn borrowed_execution_keeps_the_read_transaction_open_and_reusable() {
    let (db, state) = fixture(vec![row(), row(), row()]);
    let context = db.transaction_context(TxType::Read).await.unwrap();
    let policy = QueryExecutionResourceLimits::default();
    let cancellation = AnswerCancellation::default();
    let plans = [
        race::stock_read(Operation::Count),
        race::stock_read(Operation::Exists),
    ];
    assert_eq!(
        context
            .execute_plans(
                &plans,
                &policy,
                &cancellation,
                QueryExecutionDeadline::for_limits(policy)
            )
            .await
            .unwrap(),
        [PlanAnswer::Count(1), PlanAnswer::Exists(true)]
    );
    context
        .execute_plans(
            &plans[..1],
            &policy,
            &cancellation,
            QueryExecutionDeadline::for_limits(policy),
        )
        .await
        .unwrap();
    assert_eq!(
        state.lock().unwrap().events,
        ["open", "query", "query", "query"]
    );
    context.close().await.unwrap();
    assert_eq!(state.lock().unwrap().events.last(), Some(&"close"));
}

#[tokio::test]
async fn cancellation_between_statements_closes_without_dispatching_the_next_plan() {
    let (db, state) = fixture(vec![row(), row()]);
    let cancellation = AnswerCancellation::default();
    state.lock().unwrap().cancel_after_query = Some(cancellation.clone());
    let plans = [
        race::stock_read(Operation::Count),
        race::stock_read(Operation::Exists),
    ];
    let policy = QueryExecutionResourceLimits::default();
    let error = db
        .execute_plans(
            &plans,
            &policy,
            &cancellation,
            QueryExecutionDeadline::for_limits(policy),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("provider_cancelled"), "{error}");
    assert_eq!(state.lock().unwrap().events, ["open", "query", "close"]);
}

#[tokio::test]
async fn statement_and_aggregate_item_limits_refuse_without_partial_answers() {
    let plans = [
        race::stock_read(Operation::Count),
        race::stock_read(Operation::Exists),
    ];
    for (policy, events, code) in [
        (
            QueryExecutionResourceLimits {
                statements: 1,
                ..Default::default()
            },
            vec!["open", "close"],
            "statement_count_limit",
        ),
        (
            QueryExecutionResourceLimits {
                items: 1,
                ..Default::default()
            },
            vec!["open", "query", "query", "close"],
            "processed_item_limit",
        ),
        (
            QueryExecutionResourceLimits {
                bytes: 1,
                ..Default::default()
            },
            vec!["open", "query", "close"],
            "response_byte_limit",
        ),
    ] {
        let (db, state) = fixture(vec![row(), row()]);
        let error = db
            .execute_plans(
                &plans,
                &policy,
                &AnswerCancellation::default(),
                QueryExecutionDeadline::for_limits(policy),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains(code), "{error}");
        assert_eq!(state.lock().unwrap().events, events);
    }
}

#[tokio::test]
async fn caller_semantic_limits_and_expired_deadlines_are_enforced() {
    let plan = race::string_read(ValueComparator::Contains, &["Ada"]);
    for (dimension, code) in [
        ("graph_nodes", "hydrated_thing_limit"),
        ("attribute_values", "hydrated_attribute_value_limit"),
        ("collection_members", "collected_concept_limit"),
    ] {
        let policy = QueryExecutionResourceLimits::from_overrides(
            type_bridge_orm::AnswerTransport::Direct,
            [(dimension, 0)],
        )
        .unwrap();
        let (db, state) = fixture(vec![documents()]);
        let error = db
            .execute_plans(
                std::slice::from_ref(&plan),
                &policy,
                &AnswerCancellation::default(),
                QueryExecutionDeadline::for_limits(policy),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains(code), "{error}");
        assert_eq!(state.lock().unwrap().events, ["open", "query", "close"]);
    }
    let (db, state) = fixture(vec![]);
    let error = db
        .execute_plans(
            &[],
            &QueryExecutionResourceLimits::default(),
            &AnswerCancellation::default(),
            QueryExecutionDeadline::from_timeout_milliseconds(0),
        )
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("transaction_deadline_exceeded"),
        "{error}"
    );
    assert!(state.lock().unwrap().events.is_empty());
}

#[tokio::test]
async fn relation_role_player_limits_are_independent_of_graph_limits() {
    let (db, state) = fixture(vec![QueryResult::Documents(vec![json!({
        "_iid":"0x03", "_type":"purchase", "attributes":{"order-id":"O1"},
        "_role_0_iid":"0x01", "_role_0_type":"account", "_role_0_attributes":{"account-id":"A1"},
        "_role_1_iid":null, "_role_1_type":null, "_role_1_attributes":null,
    })])]);
    let policy = QueryExecutionResourceLimits {
        role_players: 0,
        ..Default::default()
    };
    let error = db
        .execute_plans(
            &[race::purchase_read()],
            &policy,
            &AnswerCancellation::default(),
            QueryExecutionDeadline::for_limits(policy),
        )
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("hydrated_role_player_limit"),
        "{error}"
    );
    assert_eq!(state.lock().unwrap().events, ["open", "query", "close"]);
}

#[tokio::test]
async fn one_shot_reads_refuse_write_plans_without_dispatching_them() {
    let (db, state) = fixture(vec![]);
    let policy = QueryExecutionResourceLimits::default();
    let error = db
        .execute_plans(
            &[race::touch()],
            &policy,
            &AnswerCancellation::default(),
            QueryExecutionDeadline::for_limits(policy),
        )
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("query_plan_read_only"),
        "{error}"
    );
    assert_eq!(state.lock().unwrap().events, ["open", "close"]);
}
