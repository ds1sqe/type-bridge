use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::__codegen::{
    self, CompleteModel, EncodedCreate, EncodedReference, EncodedScalar, HydratedRow,
    HydrationCapability, IntoEncodedCreate, MaterializeModel, Model, RelationModel, ThingModel,
    ValidationError, ValidationPath,
};
use crate::schema::{Schema, sealed};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig};
use type_bridge_contract::schema::DocumentId;
use type_bridge_orm::session::backend::{
    BoxFuture, DriverBackend, QueryResult, TransactionOps, TxType,
};
use type_bridge_orm::{Database as OrmDatabase, OrmError};
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
use type_bridge_schema_codegen::RustEmitter;

struct TestSchema;
impl sealed::Sealed for TestSchema {}
impl Schema for TestSchema {}

const ASSIGNMENT_JSON: &str = r#"{"kind":"relation","label":"assignment"}"#;
const POSITION_OWNS: &str =
    r#"{"attribute":"position","owner":{"kind":"relation","label":"assignment"}}"#;
const PERSON_JSON: &str = r#"{"kind":"entity","label":"person"}"#;
const NAME_OWNS: &str = r#"{"attribute":"name","owner":{"kind":"entity","label":"person"}}"#;

fn worker_role() -> &'static str {
    Box::leak(
        String::from_utf8(
            type_bridge_contract::codec::to_canonical_json(
                &type_bridge_contract::id::RoleId::new("engagement", "worker").unwrap(),
            )
            .unwrap(),
        )
        .unwrap()
        .into_boxed_str(),
    )
}

#[derive(Clone, Debug)]
struct AssignmentCreate {
    position: String,
    worker_iid: String,
}
impl sealed::Sealed for AssignmentCreate {}
impl IntoEncodedCreate for AssignmentCreate {
    fn into_encoded_create(self) -> Result<EncodedCreate, ValidationError> {
        if self.position == "reject-input" {
            return Err(ValidationError::new("position", "rejected_create"));
        }
        let reference = EncodedReference::try_new(
            PERSON_JSON,
            Some(self.worker_iid),
            vec![],
            &ValidationPath::root(),
        )?;
        Ok(EncodedCreate::new(
            ASSIGNMENT_JSON,
            vec![(POSITION_OWNS, vec![EncodedScalar::String(self.position)])],
            vec![(worker_role(), vec![reference])],
        ))
    }
}

#[derive(Debug)]
struct Assignment {
    iid: String,
    position: String,
    worker_iid: String,
    worker_name_key: Option<String>,
}
impl sealed::Sealed for Assignment {}
impl Model for Assignment {
    type Schema = TestSchema;
    const TYPE_ID_JSON: &'static str = ASSIGNMENT_JSON;
}
impl ThingModel for Assignment {
    fn thing_kind() -> __codegen::ThingKind {
        __codegen::ThingKind::Relation
    }
}
impl RelationModel for Assignment {}
impl CompleteModel for Assignment {
    type Create = AssignmentCreate;
    fn iid(&self) -> &str {
        &self.iid
    }
}
impl MaterializeModel for Assignment {
    fn materialize(row: &HydratedRow, _cap: &HydrationCapability) -> Result<Self, ValidationError> {
        row.validate_shape(
            Self::TYPE_ID_JSON,
            &[POSITION_OWNS],
            &[worker_role()],
            &__codegen::ValidationPath::root(),
        )?;
        let position = match row.fields().first().and_then(|(_, values)| values.first()) {
            Some(EncodedScalar::String(value)) => value.clone(),
            _ => return Err(ValidationError::new("position", "missing_position")),
        };
        if position == "reject-materialize" {
            return Err(ValidationError::new("position", "rejected_materialization"));
        }
        let players = row
            .roles()
            .iter()
            .find(|(token, _)| token.as_str() == worker_role())
            .map(|(_, players)| players.as_slice())
            .unwrap_or(&[]);
        let [worker] = players else {
            return Err(ValidationError::new("worker", "missing_worker"));
        };
        if worker.type_id_json() != PERSON_JSON {
            return Err(ValidationError::new("worker", "wrong_worker_type"));
        }
        let Some(worker_iid) = worker.iid() else {
            return Err(ValidationError::new("worker", "missing_worker_iid"));
        };
        let worker_name_key = worker
            .keys()
            .iter()
            .find(|(identity, _)| identity == NAME_OWNS)
            .and_then(|(_, value)| match value {
                EncodedScalar::String(value) => Some(value.clone()),
                _ => None,
            });
        Ok(Self {
            iid: row.iid().to_owned(),
            position,
            worker_iid: worker_iid.to_owned(),
            worker_name_key,
        })
    }
}

fn minimal_fixture() -> type_bridge_orm::InstalledRuntimeProjection {
    let docs = SchemaDocumentSet::parse([(
        DocumentId::new("r01.yaml").unwrap(),
        r#"format: typebridge.schema/v2
attributes:
  name: { value: string }
  position: { value: string }
entities:
  person:
    owns:
      name: { key: true }
relations:
  engagement:
    abstract: true
    relates:
      worker: { card: 1 }
  assignment:
    sub: engagement
    owns:
      position: { key: true }
  sidework: {}
plays:
  person:
    engagement: [worker]
"#,
    )])
    .unwrap();
    let resolved = resolve(
        &normalize_documents(&docs).unwrap(),
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
    let emitter = RustEmitter::new();
    let projection = project(
        &resolved,
        BindingTarget::Rust,
        &ProjectionConfig::rust(),
        &emitter.generator_handlers(),
        &emitter.code_resources().unwrap(),
    )
    .unwrap();
    type_bridge_orm::InstalledRuntimeProjection::try_new(projection).unwrap()
}

#[derive(Default)]
struct State {
    events: Vec<Event>,
    query_modes: Vec<&'static str>,
}
#[derive(Debug, PartialEq, Eq)]
enum Event {
    Open(TxType),
    Query(String),
    Commit,
    Rollback,
    Close,
}
enum Response {
    Result(QueryResult),
    Error(String),
}

struct Backend {
    state: Arc<Mutex<State>>,
    responses: Arc<Mutex<VecDeque<Response>>>,
}

impl DriverBackend for Backend {
    fn open_transaction(
        &self,
        _db: &str,
        ty: TxType,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
        self.state.lock().unwrap().events.push(Event::Open(ty));
        let tx = Tx {
            state: Arc::clone(&self.state),
            responses: Arc::clone(&self.responses),
        };
        Box::pin(async move { Ok(Box::new(tx) as Box<dyn TransactionOps>) })
    }
    fn is_open(&self) -> bool {
        true
    }
}

struct Tx {
    state: Arc<Mutex<State>>,
    responses: Arc<Mutex<VecDeque<Response>>>,
}

impl TransactionOps for Tx {
    fn query(&mut self, q: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        self.state.lock().unwrap().query_modes.push("legacy");
        self.query_recorded(q)
    }
    fn query_canonical(&mut self, q: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        self.state.lock().unwrap().query_modes.push("canonical");
        self.query_recorded(q)
    }
    fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.state.lock().unwrap().events.push(Event::Commit);
        Box::pin(async { Ok(()) })
    }
    fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.state.lock().unwrap().events.push(Event::Rollback);
        Box::pin(async { Ok(()) })
    }
    fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.state.lock().unwrap().events.push(Event::Close);
        Box::pin(async { Ok(()) })
    }
}

impl Tx {
    fn query_recorded(&mut self, q: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        self.state
            .lock()
            .unwrap()
            .events
            .push(Event::Query(q.to_owned()));
        let result = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| panic!("unexpected recording query: {q}"));
        Box::pin(async move {
            match result {
                Response::Result(value) => Ok(value),
                Response::Error(error) => Err(OrmError::QueryExecution(error)),
            }
        })
    }
}

fn test_db(responses: Vec<Response>) -> (crate::session::Database<TestSchema>, Arc<Mutex<State>>) {
    let state = Arc::new(Mutex::new(State::default()));
    let backend = Backend {
        state: Arc::clone(&state),
        responses: Arc::new(Mutex::new(responses.into())),
    };
    (
        crate::session::Database::<TestSchema>::from_test_parts(
            OrmDatabase::with_backend(Box::new(backend), "r01"),
            minimal_fixture(),
        ),
        state,
    )
}

fn iid_doc(iid: &str) -> Response {
    Response::Result(QueryResult::Documents(vec![serde_json::json!({
        "iid": iid
    })]))
}

fn fetch_doc(iid: &str, position: &str, worker_iid: &str, worker_name: &str) -> serde_json::Value {
    serde_json::json!({
        "_iid": iid,
        "_type": "assignment",
        "attributes": {"position": [position]},
        "_role_0_iid": worker_iid,
        "_role_0_type": "person",
        "_role_0_attributes": {"name": [worker_name]}
    })
}

fn fetch(iid: &str, position: &str, worker_iid: &str, worker_name: &str) -> Response {
    Response::Result(QueryResult::Documents(vec![fetch_doc(
        iid,
        position,
        worker_iid,
        worker_name,
    )]))
}

fn create(position: &str, worker_iid: &str) -> AssignmentCreate {
    AssignmentCreate {
        position: position.into(),
        worker_iid: worker_iid.into(),
    }
}

fn assert_model_error(
    error: crate::Error,
    phase: crate::error::ModelValidationPhase,
    code: &str,
    path: &[&str],
) {
    let crate::Error::ModelValidation {
        phase: actual_phase,
        code: actual,
        path: actual_path,
        ..
    } = error
    else {
        panic!("expected model validation error")
    };
    assert_eq!(actual_phase, phase);
    assert_eq!(actual, code);
    assert_eq!(
        actual_path,
        path.iter().map(|v| (*v).to_owned()).collect::<Vec<_>>()
    );
}

fn assert_assignment(value: &Assignment, iid: &str, position: &str, worker_iid: &str) {
    assert_eq!(value.iid, iid);
    assert_eq!(value.position, position);
    assert_eq!(value.worker_iid, worker_iid);
    assert!(value.worker_name_key.is_some());
}

#[tokio::test]
async fn public_relation_preflight_failures_are_zero_io() {
    let (db, state) = test_db(Vec::new());
    assert_model_error(
        db.relations::<Assignment>()
            .get_by_iid("bad-iid")
            .await
            .unwrap_err(),
        crate::error::ModelValidationPhase::Input,
        "invalid_iid",
        &["iid"],
    );
    assert_model_error(
        db.relations::<Assignment>()
            .update("bad-iid", create("p", "0x9"))
            .await
            .unwrap_err(),
        crate::error::ModelValidationPhase::Input,
        "invalid_iid",
        &["iid"],
    );
    assert_model_error(
        db.relations::<Assignment>()
            .delete("bad-iid")
            .await
            .unwrap_err(),
        crate::error::ModelValidationPhase::Input,
        "invalid_iid",
        &["iid"],
    );
    assert_model_error(
        db.relations::<Assignment>()
            .insert_many(vec![create("ok", "0x9"), create("reject-input", "0x9")])
            .await
            .unwrap_err(),
        crate::error::ModelValidationPhase::Input,
        "rejected_create",
        &["position"],
    );
    assert_model_error(
        db.relations::<Assignment>()
            .insert(create("p", "not-an-iid"))
            .await
            .unwrap_err(),
        crate::error::ModelValidationPhase::Input,
        "noncanonical_player_iid",
        &["worker[0]", "iid"],
    );
    assert_eq!(state.lock().unwrap().events.as_slice(), &[] as &[Event]);
    assert!(
        db.relations::<Assignment>()
            .insert_many(vec![])
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        db.relations::<Assignment>()
            .put_many(vec![])
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(state.lock().unwrap().events.as_slice(), &[] as &[Event]);
}

#[tokio::test]
async fn public_relation_managers_report_schema_not_bound_before_io() {
    let state = Arc::new(Mutex::new(State::default()));
    let backend = Backend {
        state: Arc::clone(&state),
        responses: Arc::new(Mutex::new(VecDeque::new())),
    };
    let db = crate::session::Database::<TestSchema>::from_test_unbound_parts(
        OrmDatabase::with_backend(Box::new(backend), "r01"),
    );
    let check = |error: crate::Error| {
        assert_model_error(
            error,
            crate::error::ModelValidationPhase::Input,
            "schema_not_bound",
            &[],
        );
        assert_eq!(state.lock().unwrap().events.as_slice(), &[] as &[Event]);
    };
    check(
        db.relations::<Assignment>()
            .insert(create("p", "0x9"))
            .await
            .unwrap_err(),
    );
    check(
        db.relations::<Assignment>()
            .put(create("p", "0x9"))
            .await
            .unwrap_err(),
    );
    check(
        db.relations::<Assignment>()
            .insert_many(vec![create("p", "0x9")])
            .await
            .unwrap_err(),
    );
    check(
        db.relations::<Assignment>()
            .put_many(vec![create("p", "0x9")])
            .await
            .unwrap_err(),
    );
    check(
        db.relations::<Assignment>()
            .update("0x1", create("p", "0x9"))
            .await
            .unwrap_err(),
    );
    check(
        db.relations::<Assignment>()
            .delete("0x1")
            .await
            .unwrap_err(),
    );
    check(db.relations::<Assignment>().count().await.unwrap_err());
    check(
        db.relations::<Assignment>()
            .get_by_iid("0x1")
            .await
            .unwrap_err(),
    );
    check(db.relations::<Assignment>().all().await.unwrap_err());
}

#[tokio::test]
async fn public_relation_insert_runs_canonical_insert_fetch_commit() {
    let (db, state) = test_db(vec![
        iid_doc("0x1"),
        fetch("0x1", "captain", "0x9", "alice"),
    ]);
    let value = db
        .relations::<Assignment>()
        .insert(create("captain", "0x9"))
        .await
        .unwrap();
    assert_assignment(&value, "0x1", "captain", "0x9");
    let guard = state.lock().unwrap();
    let [
        Event::Open(TxType::Write),
        Event::Query(insert),
        Event::Query(fetch),
        Event::Commit,
    ] = guard.events.as_slice()
    else {
        panic!("unexpected events: {:?}", guard.events)
    };
    for needle in ["insert", "isa assignment", "worker", "captain", "iid 0x9"] {
        assert!(insert.contains(needle), "insert missing {needle}: {insert}");
    }
    for needle in ["iid 0x1", "fetch"] {
        assert!(fetch.contains(needle), "fetch missing {needle}: {fetch}");
    }
    assert!(guard.query_modes.iter().all(|mode| *mode == "canonical"));
}

#[tokio::test]
async fn public_relation_insert_provider_error_rolls_back() {
    let (db, state) = test_db(vec![Response::Error("insert failed".into())]);
    let error = db
        .relations::<Assignment>()
        .insert(create("p", "0x9"))
        .await
        .unwrap_err();
    assert!(matches!(error, crate::Error::QueryExecution { .. }));
    assert!(error.to_string().contains("insert failed"));
    let guard = state.lock().unwrap();
    let [Event::Open(TxType::Write), Event::Query(_), Event::Rollback] = guard.events.as_slice()
    else {
        panic!("unexpected events: {:?}", guard.events)
    };
}

#[tokio::test]
async fn public_relation_insert_missing_post_write_row_rolls_back() {
    let (db, state) = test_db(vec![
        iid_doc("0x1"),
        Response::Result(QueryResult::Documents(Vec::new())),
    ]);
    let error = db
        .relations::<Assignment>()
        .insert(create("p", "0x9"))
        .await
        .unwrap_err();
    assert_model_error(
        error,
        crate::error::ModelValidationPhase::Hydration,
        "missing_post_write_row",
        &["iid"],
    );
    let guard = state.lock().unwrap();
    assert!(matches!(guard.events.last(), Some(Event::Rollback)));
}

#[tokio::test]
async fn public_relation_hydration_failure_rolls_back() {
    let (db, state) = test_db(vec![
        iid_doc("0x1"),
        Response::Result(QueryResult::Documents(vec![serde_json::json!({
            "_iid": "0x1",
            "_type": "assignment",
            "attributes": {"position": ["p"]},
            "_role_0_iid": "0x9",
            "_role_0_type": "person",
            "_role_0_attributes": {"bogus": ["x"]}
        })])),
    ]);
    let error = db
        .relations::<Assignment>()
        .insert(create("p", "0x9"))
        .await
        .unwrap_err();
    assert_model_error(
        error,
        crate::error::ModelValidationPhase::Hydration,
        "invalid_player_attributes",
        &["worker[0]", "attributes"],
    );
    let guard = state.lock().unwrap();
    assert!(matches!(guard.events.last(), Some(Event::Rollback)));
}

#[tokio::test]
async fn public_relation_materializer_failure_rolls_back() {
    let (db, state) = test_db(vec![
        iid_doc("0x1"),
        fetch("0x1", "reject-materialize", "0x9", "alice"),
    ]);
    let error = db
        .relations::<Assignment>()
        .insert(create("p", "0x9"))
        .await
        .unwrap_err();
    assert_model_error(
        error,
        crate::error::ModelValidationPhase::Hydration,
        "rejected_materialization",
        &["position"],
    );
    let guard = state.lock().unwrap();
    assert!(matches!(guard.events.last(), Some(Event::Rollback)));
}

#[tokio::test]
async fn public_relation_batch_correlates_in_input_order() {
    let (db, state) = test_db(vec![
        iid_doc("0x1"),
        iid_doc("0x2"),
        fetch("0x1", "first", "0x9", "alice"),
        fetch("0x2", "second", "0x9", "alice"),
    ]);
    let values = db
        .relations::<Assignment>()
        .insert_many(vec![create("first", "0x9"), create("second", "0x9")])
        .await
        .unwrap();
    assert_eq!(values.len(), 2);
    assert_assignment(&values[0], "0x1", "first", "0x9");
    assert_assignment(&values[1], "0x2", "second", "0x9");
    let guard = state.lock().unwrap();
    assert!(matches!(guard.events.last(), Some(Event::Commit)));
    assert!(guard.query_modes.iter().all(|mode| *mode == "canonical"));
}

#[tokio::test]
async fn public_relation_batch_provider_error_rolls_back_whole_call() {
    let (db, state) = test_db(vec![
        iid_doc("0x1"),
        Response::Error("second insert failed".into()),
    ]);
    let error = db
        .relations::<Assignment>()
        .insert_many(vec![create("first", "0x9"), create("second", "0x9")])
        .await
        .unwrap_err();
    assert!(matches!(error, crate::Error::QueryExecution { .. }));
    assert!(error.to_string().contains("second insert failed"));
    let guard = state.lock().unwrap();
    assert!(matches!(guard.events.last(), Some(Event::Rollback)));
}

#[tokio::test]
async fn public_relation_get_by_iid_zero_and_one_use_exact_read() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Documents(Vec::new()))]);
    assert!(
        db.relations::<Assignment>()
            .get_by_iid("0x1")
            .await
            .unwrap()
            .is_none()
    );
    {
        let guard = state.lock().unwrap();
        let [Event::Open(TxType::Read), Event::Query(query), Event::Close] =
            guard.events.as_slice()
        else {
            panic!("unexpected events: {:?}", guard.events)
        };
        assert!(query.contains("iid 0x1") && query.contains("fetch"));
        assert_eq!(guard.query_modes, vec!["canonical"]);
    }

    let (db, state) = test_db(vec![fetch("0x1", "captain", "0x9", "alice")]);
    let value = db
        .relations::<Assignment>()
        .get_by_iid("0x1")
        .await
        .unwrap()
        .expect("expected assignment");
    assert_assignment(&value, "0x1", "captain", "0x9");
    assert_eq!(value.worker_name_key.as_deref(), Some("alice"));
    let guard = state.lock().unwrap();
    let [Event::Open(TxType::Read), Event::Query(_), Event::Close] = guard.events.as_slice() else {
        panic!("unexpected events: {:?}", guard.events)
    };
}

#[tokio::test]
async fn public_relation_all_preserves_provider_order() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Documents(vec![
        fetch_doc("0x2", "second", "0x9", "alice"),
        fetch_doc("0x1", "first", "0x8", "bob"),
    ]))]);
    let values = db.relations::<Assignment>().all().await.unwrap();
    assert_eq!(values.len(), 2);
    assert_assignment(&values[0], "0x2", "second", "0x9");
    assert_assignment(&values[1], "0x1", "first", "0x8");
    let guard = state.lock().unwrap();
    let [Event::Open(TxType::Read), Event::Query(query), Event::Close] = guard.events.as_slice()
    else {
        panic!("unexpected events: {:?}", guard.events)
    };
    assert!(query.contains("fetch"));
    assert_eq!(guard.query_modes, vec!["canonical"]);
}

#[tokio::test]
async fn public_relation_count_uses_one_exact_reduction() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Rows(vec![
        serde_json::json!({"$count": 3}),
    ]))]);
    assert_eq!(db.relations::<Assignment>().count().await.unwrap(), 3_u64);
    let guard = state.lock().unwrap();
    let [Event::Open(TxType::Read), Event::Query(query), Event::Close] = guard.events.as_slice()
    else {
        panic!("unexpected events: {:?}", guard.events)
    };
    assert!(query.contains("count"));
    assert!(!query.contains("fetch"));
    assert_eq!(guard.query_modes, vec!["canonical"]);
}

#[tokio::test]
async fn public_relation_delete_commits_one_exact_delete() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Ok)]);
    db.relations::<Assignment>().delete("0x1").await.unwrap();
    let guard = state.lock().unwrap();
    let [
        Event::Open(TxType::Write),
        Event::Query(query),
        Event::Commit,
    ] = guard.events.as_slice()
    else {
        panic!("unexpected events: {:?}", guard.events)
    };
    assert!(query.contains("delete") && query.contains("iid 0x1"));
    assert_eq!(guard.query_modes, vec!["canonical"]);
}

#[tokio::test]
async fn public_relation_update_replaces_and_rehydrates() {
    let (db, state) = test_db(vec![
        iid_doc("0x9"),
        Response::Result(QueryResult::Ok),
        Response::Result(QueryResult::Ok),
        fetch("0x1", "captain", "0x9", "alice"),
    ]);
    let value = db
        .relations::<Assignment>()
        .update("0x1", create("captain", "0x9"))
        .await
        .unwrap();
    assert_assignment(&value, "0x1", "captain", "0x9");
    let guard = state.lock().unwrap();
    assert!(matches!(
        guard.events.first(),
        Some(Event::Open(TxType::Write))
    ));
    assert!(matches!(guard.events.last(), Some(Event::Commit)));
    assert!(guard.query_modes.iter().all(|mode| *mode == "canonical"));
}

#[tokio::test]
async fn public_relation_put_key_hit_replaces_existing_and_preserves_iid() {
    let (db, state) = test_db(vec![
        iid_doc("0x1"),
        iid_doc("0x9"),
        Response::Result(QueryResult::Ok),
        Response::Result(QueryResult::Ok),
        fetch("0x1", "captain", "0x9", "alice"),
    ]);
    let value = db
        .relations::<Assignment>()
        .put(create("captain", "0x9"))
        .await
        .unwrap();
    assert_assignment(&value, "0x1", "captain", "0x9");
    let guard = state.lock().unwrap();
    assert!(matches!(
        guard.events.first(),
        Some(Event::Open(TxType::Write))
    ));
    assert!(matches!(guard.events.last(), Some(Event::Commit)));
    let queries: Vec<&String> = guard
        .events
        .iter()
        .filter_map(|event| match event {
            Event::Query(query) => Some(query),
            _ => None,
        })
        .collect();
    assert_eq!(queries.len(), 5);
    assert!(guard.query_modes.iter().all(|mode| *mode == "canonical"));
}

#[tokio::test]
async fn public_relation_put_key_miss_inserts_and_rehydrates() {
    let (db, state) = test_db(vec![
        Response::Result(QueryResult::Documents(Vec::new())),
        iid_doc("0x9"),
        iid_doc("0x1"),
        fetch("0x1", "captain", "0x9", "alice"),
    ]);
    let value = db
        .relations::<Assignment>()
        .put(create("captain", "0x9"))
        .await
        .unwrap();
    assert_assignment(&value, "0x1", "captain", "0x9");
    let guard = state.lock().unwrap();
    assert!(matches!(
        guard.events.first(),
        Some(Event::Open(TxType::Write))
    ));
    assert!(matches!(guard.events.last(), Some(Event::Commit)));
    assert!(guard.query_modes.iter().all(|mode| *mode == "canonical"));
}

struct Engagement;
impl sealed::Sealed for Engagement {}
impl Model for Engagement {
    type Schema = TestSchema;
    const TYPE_ID_JSON: &'static str = r#"{"kind":"relation","label":"engagement"}"#;
}
impl ThingModel for Engagement {
    fn thing_kind() -> __codegen::ThingKind {
        __codegen::ThingKind::Relation
    }
}
impl RelationModel for Engagement {}
impl __codegen::AbstractModel for Engagement {}
#[derive(Debug)]
enum EngagementFamily {
    Assignment(Assignment),
}
impl sealed::Sealed for EngagementFamily {}
impl __codegen::ModelFamily for EngagementFamily {
    type Root = Engagement;
    type Schema = TestSchema;
    fn iid(&self) -> &str {
        match self {
            Self::Assignment(value) => value.iid(),
        }
    }
}
impl __codegen::SubtypeRootModel for Engagement {
    type Subtypes = EngagementFamily;
    fn __tb_dispatch_subtype(
        row: &HydratedRow,
        cap: &HydrationCapability,
    ) -> Result<Self::Subtypes, ValidationError> {
        if row.type_id_json() == Assignment::TYPE_ID_JSON {
            Ok(EngagementFamily::Assignment(Assignment::materialize(
                row, cap,
            )?))
        } else {
            Err(ValidationError::new("type_id", "wrong_concrete_model_type"))
        }
    }
}

fn identity_doc(iid: &str, type_name: &str) -> serde_json::Value {
    serde_json::json!({"_iid": iid, "_type": type_name})
}

#[tokio::test]
async fn public_relation_subtypes_all_rehydrates_concrete_children_in_discovery_order() {
    let (db, state) = test_db(vec![
        Response::Result(QueryResult::Documents(vec![
            identity_doc("0x2", "assignment"),
            identity_doc("0x1", "assignment"),
        ])),
        fetch("0x2", "second", "0x9", "alice"),
        fetch("0x1", "first", "0x8", "bob"),
    ]);
    let values = db.relations::<Engagement>().subtypes().all().await.unwrap();
    assert_eq!(
        values
            .iter()
            .map(|value| {
                let EngagementFamily::Assignment(assignment) = value;
                (
                    assignment.iid.as_str(),
                    assignment.position.as_str(),
                    assignment.worker_iid.as_str(),
                )
            })
            .collect::<Vec<_>>(),
        vec![("0x2", "second", "0x9"), ("0x1", "first", "0x8")]
    );
    let guard = state.lock().unwrap();
    let [
        Event::Open(TxType::Read),
        Event::Query(discovery),
        Event::Query(child2),
        Event::Query(child1),
        Event::Close,
    ] = guard.events.as_slice()
    else {
        panic!("unexpected events: {:?}", guard.events)
    };
    assert!(discovery.contains("sub engagement"));
    assert!(discovery.contains("_iid") && discovery.contains("_type"));
    for (query, iid) in [(child2, "0x2"), (child1, "0x1")] {
        assert!(query.contains("assignment") && query.contains(iid) && query.contains("fetch"));
    }
    assert!(guard.query_modes.iter().all(|mode| *mode == "canonical"));
}

#[tokio::test]
async fn public_relation_subtypes_get_by_iid_zero_and_one() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Documents(Vec::new()))]);
    assert!(
        db.relations::<Engagement>()
            .subtypes()
            .get_by_iid("0x1")
            .await
            .unwrap()
            .is_none()
    );
    {
        let guard = state.lock().unwrap();
        let [Event::Open(TxType::Read), Event::Query(query), Event::Close] =
            guard.events.as_slice()
        else {
            panic!("unexpected events: {:?}", guard.events)
        };
        assert!(query.contains("sub engagement") && query.contains("iid 0x1"));
    }
    let (db, _state) = test_db(vec![
        Response::Result(QueryResult::Documents(vec![identity_doc(
            "0x1",
            "assignment",
        )])),
        fetch("0x1", "captain", "0x9", "alice"),
    ]);
    let value = db
        .relations::<Engagement>()
        .subtypes()
        .get_by_iid("0x1")
        .await
        .unwrap()
        .expect("expected family member");
    let EngagementFamily::Assignment(assignment) = value;
    assert_assignment(&assignment, "0x1", "captain", "0x9");
}

#[tokio::test]
async fn public_relation_subtypes_reject_foreign_concrete_type() {
    let (db, state) = test_db(vec![
        Response::Result(QueryResult::Documents(vec![identity_doc(
            "0x1", "sidework",
        )])),
        Response::Result(QueryResult::Documents(vec![serde_json::json!({
            "_iid": "0x1",
            "_type": "sidework",
            "attributes": {}
        })])),
    ]);
    let error = db
        .relations::<Engagement>()
        .subtypes()
        .all()
        .await
        .unwrap_err();
    assert_model_error(
        error,
        crate::error::ModelValidationPhase::Hydration,
        "wrong_concrete_model_type",
        &["type_id"],
    );
    let guard = state.lock().unwrap();
    assert!(matches!(guard.events.last(), Some(Event::Close)));
}

#[tokio::test]
async fn public_relation_subtypes_missing_concrete_row_closes_context() {
    let (db, state) = test_db(vec![
        Response::Result(QueryResult::Documents(vec![identity_doc(
            "0x1",
            "assignment",
        )])),
        Response::Result(QueryResult::Documents(Vec::new())),
    ]);
    let error = db
        .relations::<Engagement>()
        .subtypes()
        .all()
        .await
        .unwrap_err();
    assert_model_error(
        error,
        crate::error::ModelValidationPhase::Hydration,
        "missing_concrete_row",
        &["iid"],
    );
    let guard = state.lock().unwrap();
    assert!(matches!(guard.events.last(), Some(Event::Close)));
}

#[tokio::test]
async fn public_relation_subtypes_count_uses_inclusive_scope() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Rows(vec![
        serde_json::json!({"$count": 5}),
    ]))]);
    assert_eq!(
        db.relations::<Engagement>()
            .subtypes()
            .count()
            .await
            .unwrap(),
        5_u64
    );
    let guard = state.lock().unwrap();
    let [Event::Open(TxType::Read), Event::Query(query), Event::Close] = guard.events.as_slice()
    else {
        panic!("unexpected events: {:?}", guard.events)
    };
    assert!(query.contains("count"));
    assert!(query.contains("engagement"));
    assert_eq!(guard.query_modes, vec!["canonical"]);
}
