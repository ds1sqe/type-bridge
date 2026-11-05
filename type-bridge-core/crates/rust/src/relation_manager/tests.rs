mod native_batch_answer_item {
    pub use type_bridge_orm::session::backend::{AnswerItem, QueryResult};
}
#[allow(dead_code)]
mod native_batch {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../orm/tests/support/native_batch.rs"
    ));
}
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::__codegen::{
    self, CompleteModel, EncodedCreate, EncodedReference, EncodedScalar, EntityModel, HydratedRow,
    HydrationCapability, IntoEncodedCreate, IntoEncodedReference, MaterializeModel, Model,
    ReferenceOrigin, RelationModel, ThingModel, ValidationError, ValidationPath,
};
use crate::hooks::{HookContext, HookError, HookFuture, LifecycleHook, PreHookResult};
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

#[derive(Default)]
struct ContinueHook {
    before: AtomicUsize,
    after: AtomicUsize,
}

impl LifecycleHook for ContinueHook {
    fn name(&self) -> &str {
        "continue"
    }

    fn before_operation<'a>(
        &'a self,
        _context: &'a mut HookContext<'_>,
    ) -> HookFuture<'a, Result<PreHookResult, HookError>> {
        self.before.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(PreHookResult::Continue) })
    }

    fn after_operation<'a>(
        &'a self,
        _context: &'a HookContext<'_>,
    ) -> HookFuture<'a, Result<(), HookError>> {
        self.after.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}

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
    worker: AssignmentWorker,
}

#[derive(Clone, Debug)]
enum AssignmentWorker {
    Iid(String),
    Reference(PersonRef),
}
impl sealed::Sealed for AssignmentCreate {}
impl IntoEncodedCreate for AssignmentCreate {
    fn into_encoded_create(self) -> Result<EncodedCreate, ValidationError> {
        if self.position == "reject-input" {
            return Err(ValidationError::new("position", "rejected_create"));
        }
        let reference = match self.worker {
            AssignmentWorker::Iid(worker_iid) => EncodedReference::try_new(
                PERSON_JSON,
                Some(worker_iid),
                vec![],
                &ValidationPath::root(),
            )?,
            AssignmentWorker::Reference(reference) => reference.into_encoded_reference()?,
        };
        Ok(EncodedCreate::new(
            ASSIGNMENT_JSON,
            vec![(POSITION_OWNS, vec![EncodedScalar::String(self.position)])],
            vec![(worker_role(), vec![reference])],
        ))
    }
}

#[derive(Clone, Debug)]
struct PersonCreate {
    name: String,
}
impl sealed::Sealed for PersonCreate {}
impl IntoEncodedCreate for PersonCreate {
    fn into_encoded_create(self) -> Result<EncodedCreate, ValidationError> {
        Ok(EncodedCreate::new(
            PERSON_JSON,
            vec![(NAME_OWNS, vec![EncodedScalar::String(self.name)])],
            vec![],
        ))
    }
}

#[derive(Clone, Debug)]
struct Person {
    origin: ReferenceOrigin,
    iid: String,
    name: String,
}
impl sealed::Sealed for Person {}
impl Model for Person {
    type Schema = TestSchema;
    const TYPE_ID_JSON: &'static str = PERSON_JSON;
}
impl ThingModel for Person {
    fn thing_kind() -> __codegen::ThingKind {
        __codegen::ThingKind::Entity
    }
}
impl EntityModel for Person {}
impl CompleteModel for Person {
    type Create = PersonCreate;

    fn iid(&self) -> &str {
        &self.iid
    }
}
impl MaterializeModel for Person {
    fn materialize(row: &HydratedRow, _cap: &HydrationCapability) -> Result<Self, ValidationError> {
        row.validate_shape(
            Self::TYPE_ID_JSON,
            &[NAME_OWNS],
            &[],
            &ValidationPath::root(),
        )?;
        let name = match row.fields().first().and_then(|(_, values)| values.first()) {
            Some(EncodedScalar::String(value)) => value.clone(),
            _ => return Err(ValidationError::new("name", "missing_name")),
        };
        Ok(Self {
            origin: row.origin().clone(),
            iid: row.iid().to_owned(),
            name,
        })
    }
}
impl Person {
    fn reference(&self) -> PersonRef {
        PersonRef {
            origin: self.origin.clone(),
            iid: Some(self.iid.clone()),
            name: Some(self.name.clone()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct PersonRef {
    origin: ReferenceOrigin,
    iid: Option<String>,
    name: Option<String>,
}
impl sealed::Sealed for PersonRef {}
impl IntoEncodedReference for PersonRef {
    fn into_encoded_reference(self) -> Result<EncodedReference, ValidationError> {
        let keys = self
            .name
            .map(|name| (NAME_OWNS, EncodedScalar::String(name)))
            .into_iter()
            .collect();
        EncodedReference::try_new_with_origin(
            PERSON_JSON,
            self.iid,
            keys,
            self.origin,
            &ValidationPath::root(),
        )
    }
}

#[derive(Debug)]
struct Assignment {
    origin: ReferenceOrigin,
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
        let worker_name_key = worker.fields().map_or_else(
            || {
                worker
                    .keys()
                    .iter()
                    .find(|(identity, _)| identity == NAME_OWNS)
                    .and_then(|(_, value)| match value {
                        EncodedScalar::String(value) => Some(value.clone()),
                        _ => None,
                    })
            },
            |fields| {
                fields
                    .iter()
                    .find(|(identity, _)| identity == NAME_OWNS)
                    .and_then(|(_, values)| match values.as_slice() {
                        [EncodedScalar::String(value)] => Some(value.clone()),
                        _ => None,
                    })
            },
        );
        Ok(Self {
            origin: row.origin().clone(),
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
    fn query_plan_bounded<'a>(
        &'a mut self,
        source: &'a str,
        limits: type_bridge_orm::session::backend::AnswerLimits,
        consumer: &'a mut dyn type_bridge_orm::session::backend::AnswerConsumer,
    ) -> BoxFuture<'a, Result<type_bridge_orm::session::backend::BoundedAnswerStats, OrmError>>
    {
        Box::pin(async move {
            use type_bridge_orm::session::backend::{
                AnswerControl, AnswerItem, BoundedAnswerReader,
            };
            let result = self.query_canonical(source).await?;
            let items = match result {
                QueryResult::Ok => vec![],
                QueryResult::Rows(rows) => rows.into_iter().map(AnswerItem::Row).collect(),
                QueryResult::Documents(documents) => {
                    documents.into_iter().map(AnswerItem::Document).collect()
                }
            };
            let mut reader = BoundedAnswerReader::new(limits.answer);
            reader.check_before_read()?;
            for item in items {
                if reader.accept(item, consumer)? == AnswerControl::Stop {
                    break;
                }
            }
            Ok(reader.stats())
        })
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
        let source = q.to_owned();
        Box::pin(async move {
            match result {
                Response::Result(value) => Ok(native_batch::result(&source, value)),
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

fn fetch_doc(iid: &str, position: &str, worker_iid: &str, worker_name: &str) -> serde_json::Value {
    serde_json::json!({
        "_iid": iid,
        "_type": "assignment",
        "attributes": {"position": position},
        "_role_0_iid": worker_iid,
        "_role_0_type": "person",
        "_role_0_attributes": {"name": worker_name}
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

fn person_fetch(iid: &str, name: &str) -> Response {
    Response::Result(QueryResult::Documents(vec![serde_json::json!({
        "_iid": iid,
        "_type": "person",
        "attributes": {"name": name}
    })]))
}

fn create(position: &str, worker_iid: &str) -> AssignmentCreate {
    AssignmentCreate {
        position: position.into(),
        worker: AssignmentWorker::Iid(worker_iid.into()),
    }
}

fn create_from_person(position: &str, person: &Person) -> AssignmentCreate {
    AssignmentCreate {
        position: position.into(),
        worker: AssignmentWorker::Reference(person.reference()),
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
    let error = db
        .relations::<Assignment>()
        .insert_many(vec![create("ok", "0x9"), create("reject-input", "0x9")])
        .await
        .unwrap_err();
    assert!(matches!(&error, crate::Error::Classified { .. }));
    assert_eq!(error.category(), crate::ErrorCategory::ModelValidation);
    assert_eq!(
        error.model_validation_phase(),
        Some(crate::error::ModelValidationPhase::Input)
    );
    assert_eq!(error.code(), Some("rejected_create"));
    assert_eq!(
        error.path(),
        Some(&["rows".into(), "[1]".into(), "position".into()][..])
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
            .items()
            .is_empty()
    );
    assert!(
        db.relations::<Assignment>()
            .put_many(vec![])
            .await
            .unwrap()
            .items()
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
    let (db, state) = test_db(vec![fetch("0x1", "captain", "0x9", "alice")]);
    let value = db
        .relations::<Assignment>()
        .insert(create("captain", "0x9"))
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    assert_assignment(&value, "0x1", "captain", "0x9");
    let guard = state.lock().unwrap();
    // One query: the insert fetches its own hydration document.
    let [
        Event::Open(TxType::Write),
        Event::Query(insert),
        Event::Commit,
    ] = guard.events.as_slice()
    else {
        panic!("unexpected events: {:?}", guard.events)
    };
    for needle in [
        "insert",
        "isa assignment",
        "worker",
        "captain",
        "iid 0x9",
        "fetch",
    ] {
        assert!(insert.contains(needle), "insert missing {needle}: {insert}");
    }
    assert!(guard.query_modes.iter().all(|mode| *mode == "canonical"));
}

#[tokio::test]
async fn hydrated_generated_reference_preserves_database_origin_before_relation_io() {
    let (source, source_state) = test_db(vec![
        person_fetch("0x9", "alice"),
        fetch("0x1", "captain", "0x9", "alice"),
    ]);
    let person = source
        .entities::<Person>()
        .get_by_iid("0x9")
        .await
        .unwrap()
        .expect("source person exists");
    assert_eq!(
        person.reference(),
        PersonRef {
            origin: ReferenceOrigin::default(),
            iid: Some("0x9".into()),
            name: Some("alice".into()),
        },
        "opaque origin must not change generated reference value equality"
    );

    let (foreign, foreign_state) = test_db(Vec::new());
    let error = foreign
        .relations::<Assignment>()
        .insert(create_from_person("captain", &person))
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("reference_database_mismatch"));
    assert!(
        foreign_state.lock().unwrap().events.is_empty(),
        "foreign-origin rejection must occur before opening a transaction"
    );

    let (hooked_foreign, hooked_foreign_state) = test_db(Vec::new());
    let hook = Arc::new(ContinueHook::default());
    let mut hooked_manager = hooked_foreign.relations::<Assignment>();
    hooked_manager.add_hook(hook.clone());
    let error = hooked_manager
        .insert(create_from_person("captain", &person))
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("reference_database_mismatch"));
    assert_eq!(hook.before.load(Ordering::SeqCst), 1);
    assert_eq!(hook.after.load(Ordering::SeqCst), 0);
    assert!(
        hooked_foreign_state.lock().unwrap().events.is_empty(),
        "hooked foreign-origin rejection must occur after pre-hooks and before provider I/O"
    );

    let source_hook = Arc::new(ContinueHook::default());
    let mut source_manager = source.relations::<Assignment>();
    source_manager.add_hook(source_hook.clone());
    let stored = source_manager
        .insert(create_from_person("captain", &person))
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    assert_assignment(&stored, "0x1", "captain", "0x9");
    assert_eq!(source_hook.before.load(Ordering::SeqCst), 1);
    assert_eq!(source_hook.after.load(Ordering::SeqCst), 1);
    let guard = source_state.lock().unwrap();
    assert_eq!(
        guard
            .events
            .iter()
            .filter(|event| matches!(event, Event::Open(_)))
            .count(),
        2,
        "one source read and one same-database relation write"
    );
    assert!(matches!(guard.events.last(), Some(Event::Commit)));
}

#[tokio::test]
async fn detached_snapshot_reference_rejects_mutation_before_provider_io() {
    let (db, state) = test_db(Vec::new());
    let detached = Person {
        origin: ReferenceOrigin::detached_snapshot(),
        iid: "0x9".into(),
        name: "alice".into(),
    };

    let error = db
        .relations::<Assignment>()
        .insert(create_from_person("captain", &detached))
        .await
        .unwrap_err();

    assert_eq!(error.code(), Some("projected_snapshot_detached"));
    assert!(
        state.lock().unwrap().events.is_empty(),
        "detached snapshot rejection must occur before provider I/O"
    );
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
async fn public_relation_insert_without_a_written_row_commits_an_empty_result() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Documents(Vec::new()))]);
    let result = db
        .relations::<Assignment>()
        .insert(create("p", "0x9"))
        .await
        .unwrap();
    assert_eq!(result.count(), 0);
    assert!(result.items().is_empty());
    assert!(result.item().is_none());
    let guard = state.lock().unwrap();
    assert!(matches!(
        guard.events.as_slice(),
        [Event::Open(TxType::Write), Event::Query(_), Event::Commit]
    ));
}

#[tokio::test]
async fn public_relation_document_validation_failure_rolls_back() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Documents(vec![
        serde_json::json!({
            "_iid": "0x1",
            "_type": "assignment",
            "attributes": {"position": "p"},
            "_role_0_iid": "0x9",
            "_role_0_type": "person",
            "_role_0_attributes": {"bogus": ["x"]}
        }),
    ]))]);
    let error = db
        .relations::<Assignment>()
        .insert(create("p", "0x9"))
        .await
        .unwrap_err();
    // The shared plan decoder rejects an undeclared player attribute before hydration.
    assert_eq!(error.code(), Some("query_remote_v3_answer_kind"));
    let guard = state.lock().unwrap();
    assert!(matches!(
        guard.events.as_slice(),
        [Event::Open(TxType::Write), Event::Query(_), Event::Rollback]
    ));
}

#[tokio::test]
async fn public_relation_materializer_failure_rolls_back() {
    let (db, state) = test_db(vec![fetch("0x1", "reject-materialize", "0x9", "alice")]);
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
    let (db, state) = test_db(vec![Response::Result(QueryResult::Documents(vec![
        {
            let mut document = fetch_doc("0x2", "second", "0x9", "alice");
            document["ordinal"] = serde_json::json!(1);
            document
        },
        {
            let mut document = fetch_doc("0x1", "first", "0x9", "alice");
            document["ordinal"] = serde_json::json!(0);
            document
        },
    ]))]);
    let values = db
        .relations::<Assignment>()
        .insert_many(vec![create("first", "0x9"), create("second", "0x9")])
        .await
        .unwrap();
    assert_eq!(values.count(), 2);
    assert_assignment(&values.items()[0], "0x1", "first", "0x9");
    assert_assignment(&values.items()[1], "0x2", "second", "0x9");
    let guard = state.lock().unwrap();
    assert_eq!(guard.query_modes, ["canonical"]);
    assert!(matches!(guard.events.last(), Some(Event::Commit)));
    assert!(guard.query_modes.iter().all(|mode| *mode == "canonical"));
}

#[tokio::test]
async fn public_relation_batch_provider_error_rolls_back_whole_call() {
    let (db, state) = test_db(vec![Response::Error("batch insert failed".into())]);
    let error = db
        .relations::<Assignment>()
        .insert_many(vec![create("first", "0x9"), create("second", "0x9")])
        .await
        .unwrap_err();
    assert!(matches!(&error, crate::Error::Classified { .. }));
    assert_eq!(error.category(), crate::ErrorCategory::QueryExecution);
    assert_eq!(error.code(), Some("provider_operation_failed"));
    assert!(!error.to_string().contains("batch insert failed"));
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
    assert!(
        values
            .iter()
            .all(|value| value.origin.projected().is_some())
    );
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
    let (db, state) = test_db(vec![Response::Result(QueryResult::Rows(Vec::new()))]);
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
    assert!(query.contains("delete") && query.contains("isa! assignment"));
    assert!(query.contains("let $PlanLiteralCell1 = \"0x1\""));
    assert!(query.contains("iid($thing1) == $target_iid"));
    assert_eq!(guard.query_modes, vec!["canonical"]);
}

#[tokio::test]
async fn public_relation_update_replaces_and_rehydrates() {
    // One plan: match the relation and its player, replace, fetch.
    let (db, state) = test_db(vec![fetch("0x1", "captain", "0x9", "alice")]);
    let value = db
        .relations::<Assignment>()
        .update("0x1", create("captain", "0x9"))
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
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
    let (db, state) = test_db(vec![fetch("0x1", "captain", "0x9", "alice")]);
    let value = db
        .relations::<Assignment>()
        .put(create("captain", "0x9"))
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
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
    assert_eq!(queries.len(), 1);
    assert!(queries[0].contains("put") && queries[0].contains("fetch"));
    assert!(guard.query_modes.iter().all(|mode| *mode == "canonical"));
}

#[tokio::test]
async fn public_relation_put_key_miss_inserts_and_rehydrates() {
    let (db, state) = test_db(vec![fetch("0x1", "captain", "0x9", "alice")]);
    let value = db
        .relations::<Assignment>()
        .put(create("captain", "0x9"))
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
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

fn assert_subtype_fetch_query(query: &str) {
    for needle in [
        "$root0 isa engagement",
        "select $root0;",
        "distinct;",
        "fetch",
        "attributes",
    ] {
        assert!(query.contains(needle), "{query}");
    }
    assert!(!query.contains("isa! engagement"));
}

#[tokio::test]
async fn public_relation_subtypes_all_fetches_complete_children_in_one_plan() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Documents(vec![
        fetch_doc("0x2", "second", "0x9", "alice"),
        fetch_doc("0x1", "first", "0x8", "bob"),
    ]))]);
    let values = db.relations::<Engagement>().subtypes().all().await.unwrap();
    assert!(values.iter().all(|value| {
        let EngagementFamily::Assignment(assignment) = value;
        assignment.origin.projected().is_some()
    }));
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
    let [Event::Open(TxType::Read), Event::Query(query), Event::Close] = guard.events.as_slice()
    else {
        panic!("unexpected events: {:?}", guard.events);
    };
    assert_subtype_fetch_query(query);
    assert_eq!(guard.query_modes, vec!["canonical"]);
}

#[tokio::test]
async fn public_relation_subtypes_get_by_iid_zero_and_one() {
    for present in [false, true] {
        let mut expected_query = None;
        for filtered in [false, true] {
            let rows = if present {
                vec![fetch_doc("0x1", "captain", "0x9", "alice")]
            } else {
                vec![]
            };
            let (db, state) = test_db(vec![Response::Result(QueryResult::Documents(rows))]);
            let manager = db.relations::<Engagement>().subtypes();
            let value = if filtered {
                manager
                    .filter()
                    .unwrap()
                    .iid(&crate::Iid::new("0x1").unwrap())
                    .unwrap()
                    .first()
                    .await
                    .unwrap()
            } else {
                manager.get_by_iid("0x1").await.unwrap()
            };
            if present {
                let Some(EngagementFamily::Assignment(assignment)) = value else {
                    panic!("expected child");
                };
                assert_assignment(&assignment, "0x1", "captain", "0x9");
                assert!(assignment.origin.projected().is_some());
            } else {
                assert!(value.is_none());
            }
            let guard = state.lock().unwrap();
            let [Event::Open(TxType::Read), Event::Query(query), Event::Close] =
                guard.events.as_slice()
            else {
                panic!("unexpected events: {:?}", guard.events);
            };
            assert_subtype_fetch_query(query);
            assert!(query.contains("iid 0x1"));
            if let Some(expected) = &expected_query {
                assert_eq!(query, expected);
            } else {
                expected_query = Some(query.clone());
            }
        }
    }
}

#[tokio::test]
async fn public_relation_subtypes_reject_invalid_evidence_before_materialization() {
    let valid = fetch_doc("0x1", "captain", "0x9", "alice");
    let mut missing = valid.clone();
    missing["attributes"] = serde_json::json!({});
    let mut conflicting = valid.clone();
    conflicting["attributes"]["position"] = serde_json::json!("different");
    for (rows, code) in [
        (
            vec![serde_json::json!({"_iid":"0x1","_type":"unknown","attributes":{}})],
            "query_remote_v3_document_type",
        ),
        (
            vec![serde_json::json!({"_iid":"0x1","_type":"engagement","attributes":{}})],
            "query_remote_v3_document_type",
        ),
        (
            vec![serde_json::json!({"_iid":"0x1","_type":"sidework","attributes":{}})],
            "query_remote_v3_document_type",
        ),
        (vec![missing], "query_remote_v3_document_value"),
        (vec![valid, conflicting], "query_plan_hydration_conflict"),
    ] {
        let (db, state) = test_db(vec![Response::Result(QueryResult::Documents(rows))]);
        let error = db
            .relations::<Engagement>()
            .subtypes()
            .all()
            .await
            .unwrap_err();
        assert_eq!(error.category(), crate::ErrorCategory::ModelValidation);
        assert_eq!(error.sdk_category(), Some("integrity"));
        assert_eq!(error.code(), Some(code));
        let guard = state.lock().unwrap();
        let [Event::Open(TxType::Read), Event::Query(query), Event::Close] =
            guard.events.as_slice()
        else {
            panic!("unexpected events: {:?}", guard.events);
        };
        assert_subtype_fetch_query(query);
    }
}

#[tokio::test]
async fn public_relation_subtypes_all_coalesces_identical_native_documents() {
    let row = fetch_doc("0x1", "captain", "0x9", "alice");
    let (db, _) = test_db(vec![Response::Result(QueryResult::Documents(vec![
        row.clone(),
        row,
    ]))]);
    let values = db.relations::<Engagement>().subtypes().all().await.unwrap();
    let [EngagementFamily::Assignment(assignment)] = values.as_slice() else {
        panic!("expected one child");
    };
    assert_assignment(assignment, "0x1", "captain", "0x9");
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
