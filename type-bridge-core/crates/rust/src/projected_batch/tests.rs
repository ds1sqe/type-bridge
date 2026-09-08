use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, ProjectionHandler};
use type_bridge_contract::schema::{DocumentId, OwnsFactId};
use type_bridge_contract::sdk_diagnostic::{
    SdkCommitFailureOutcome, SdkDiagnosticPathSegment, SdkExecutionDiagnostic, SdkProviderOperation,
};
use type_bridge_contract::value::{CanonicalString, CanonicalValue};
use type_bridge_orm::session::backend::{
    AnswerConsumer, AnswerControl, AnswerItem, BoundedAnswerLimits, BoundedAnswerReader,
    BoundedAnswerStats, BoxFuture, DriverBackend, GivenRowsSpec, QueryResult, TransactionOps,
    TxType,
};
use type_bridge_orm::{
    _ProviderVersion, ClassifiedCommitError, CommitFailureCertainty, Database as OrmDatabase,
    MAX_QUERY_ITEMS, OrmError, ProjectedAttributeValue, ProjectedBatchResult, ProjectedBatchRow,
    ProjectedThing,
};
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
use type_bridge_schema_codegen::RustEmitter;

use super::*;
use crate::__codegen::{
    self, CompleteModel, EncodedCreate, EncodedReference, EncodedScalar, EntityModel, HydratedRow,
    HydrationCapability, IntoEncodedCreate, MaterializeModel, Model, ReferenceOrigin,
    RelationModel, ThingModel, ValidationError, ValidationPath,
};
use crate::hooks::{HookContext, HookError, HookFuture, LifecycleHook, PreHookResult};
use crate::schema::{Schema, sealed};
use crate::{ErrorCategory, ErrorDetail, ErrorPathSegment};

const PERSON_JSON: &str = r#"{"kind":"entity","label":"person"}"#;
const ASSIGNMENT_JSON: &str = r#"{"kind":"relation","label":"assignment"}"#;
const NAME_OWNS: &str = r#"{"attribute":"name","owner":{"kind":"entity","label":"person"}}"#;
const TAG_OWNS: &str = r#"{"attribute":"tag","owner":{"kind":"entity","label":"person"}}"#;
const POSITION_OWNS: &str =
    r#"{"attribute":"position","owner":{"kind":"relation","label":"assignment"}}"#;

const SUCCESSOR_SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  name: { value: string }
  tag: { value: string }
  position: { value: string }
entities:
  person:
    owns:
      name: { key: true }
      tag:
        card: { min: 0, max: 4 }
        ordered: true
relations:
  assignment:
    owns:
      position: { key: true }
    relates:
      worker: { card: 1 }
plays:
  person:
    assignment: [worker]
"#;

const FOREIGN_SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  name: { value: string }
  tag: { value: string }
  position: { value: string }
  foreign: { value: string }
entities:
  person:
    owns:
      name: { key: true }
      tag:
        card: { min: 0, max: 4 }
        ordered: true
      foreign: { card: { min: 0 } }
relations:
  assignment:
    owns:
      position: { key: true }
    relates:
      worker: { card: 1 }
plays:
  person:
    assignment: [worker]
"#;

struct TestSchema;
impl sealed::Sealed for TestSchema {}
impl Schema for TestSchema {}

fn worker_role() -> &'static str {
    Box::leak(
        String::from_utf8(
            type_bridge_contract::codec::to_canonical_json(
                &type_bridge_contract::id::RoleId::new("assignment", "worker").unwrap(),
            )
            .unwrap(),
        )
        .unwrap()
        .into_boxed_str(),
    )
}

#[derive(Clone, Debug)]
struct PersonCreate {
    name: String,
    tags: Vec<String>,
}

impl PersonCreate {
    fn named(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            tags: vec![format!("{name}-tag")],
        }
    }
}

impl sealed::Sealed for PersonCreate {}
impl IntoEncodedCreate for PersonCreate {
    fn into_encoded_create(self) -> Result<EncodedCreate, ValidationError> {
        if self.name == "reject-input" {
            return Err(ValidationError::new("name", "rejected_create"));
        }
        if self.name == "reject-nested" {
            return Err(ValidationError::new(
                "children[2].name",
                "rejected_nested_create",
            ));
        }
        if self.name == "foreign-field" {
            return Ok(EncodedCreate::new(
                PERSON_JSON,
                vec![(POSITION_OWNS, vec![EncodedScalar::String(self.name)])],
                vec![],
            ));
        }
        Ok(EncodedCreate::new(
            PERSON_JSON,
            vec![
                (NAME_OWNS, vec![EncodedScalar::String(self.name)]),
                (
                    TAG_OWNS,
                    self.tags.into_iter().map(EncodedScalar::String).collect(),
                ),
            ],
            vec![],
        ))
    }
}

#[derive(Debug)]
struct Person {
    iid: String,
    name: String,
    tags: Vec<String>,
    origin: ReferenceOrigin,
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
            &[NAME_OWNS, TAG_OWNS],
            &[],
            &ValidationPath::root(),
        )?;
        let strings = |token: &str| {
            row.fields()
                .iter()
                .find(|(identity, _)| identity == token)
                .map(|(_, values)| {
                    values
                        .iter()
                        .filter_map(|value| match value {
                            EncodedScalar::String(value) => Some(value.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        let name = strings(NAME_OWNS)
            .into_iter()
            .next()
            .ok_or_else(|| ValidationError::new("name", "missing_name"))?;
        if name == "reject-materialize" {
            return Err(ValidationError::new("name", "rejected_materialization"));
        }
        Ok(Self {
            iid: row.iid().to_owned(),
            name,
            tags: strings(TAG_OWNS),
            origin: row.origin().clone(),
        })
    }
}

#[derive(Clone, Debug)]
struct AssignmentCreate {
    position: String,
    worker_iid: String,
    worker_origin: ReferenceOrigin,
}

impl AssignmentCreate {
    fn named(position: &str) -> Self {
        Self {
            position: position.to_owned(),
            worker_iid: "0x99".to_owned(),
            worker_origin: ReferenceOrigin::default(),
        }
    }
}

impl sealed::Sealed for AssignmentCreate {}
impl IntoEncodedCreate for AssignmentCreate {
    fn into_encoded_create(self) -> Result<EncodedCreate, ValidationError> {
        let reference = EncodedReference::try_new_with_origin(
            PERSON_JSON,
            Some(self.worker_iid),
            vec![],
            self.worker_origin,
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
            &ValidationPath::root(),
        )?;
        let position = row
            .fields()
            .iter()
            .find(|(identity, _)| identity == POSITION_OWNS)
            .and_then(|(_, values)| values.first())
            .and_then(|value| match value {
                EncodedScalar::String(value) => Some(value.clone()),
                _ => None,
            })
            .ok_or_else(|| ValidationError::new("position", "missing_position"))?;
        Ok(Self {
            iid: row.iid().to_owned(),
            position,
        })
    }
}

#[derive(Debug)]
struct WrongPerson;
impl sealed::Sealed for WrongPerson {}
impl Model for WrongPerson {
    type Schema = TestSchema;
    const TYPE_ID_JSON: &'static str = ASSIGNMENT_JSON;
}
impl ThingModel for WrongPerson {
    fn thing_kind() -> __codegen::ThingKind {
        __codegen::ThingKind::Entity
    }
}
impl EntityModel for WrongPerson {}
impl CompleteModel for WrongPerson {
    type Create = PersonCreate;

    fn iid(&self) -> &str {
        unreachable!()
    }
}
impl MaterializeModel for WrongPerson {
    fn materialize(
        _row: &HydratedRow,
        _cap: &HydrationCapability,
    ) -> Result<Self, ValidationError> {
        unreachable!()
    }
}

#[derive(Clone)]
struct ProbeCreate(Arc<AtomicUsize>);
impl sealed::Sealed for ProbeCreate {}
impl IntoEncodedCreate for ProbeCreate {
    fn into_encoded_create(self) -> Result<EncodedCreate, ValidationError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        PersonCreate::named("probe").into_encoded_create()
    }
}

#[derive(Debug)]
struct ProbePerson(String);
impl sealed::Sealed for ProbePerson {}
impl Model for ProbePerson {
    type Schema = TestSchema;
    const TYPE_ID_JSON: &'static str = PERSON_JSON;
}
impl ThingModel for ProbePerson {
    fn thing_kind() -> __codegen::ThingKind {
        __codegen::ThingKind::Entity
    }
}
impl EntityModel for ProbePerson {}
impl CompleteModel for ProbePerson {
    type Create = ProbeCreate;

    fn iid(&self) -> &str {
        &self.0
    }
}
impl MaterializeModel for ProbePerson {
    fn materialize(row: &HydratedRow, _cap: &HydrationCapability) -> Result<Self, ValidationError> {
        Ok(Self(row.iid().to_owned()))
    }
}

#[derive(Debug)]
struct ProbeAssignment(String);
impl sealed::Sealed for ProbeAssignment {}
impl Model for ProbeAssignment {
    type Schema = TestSchema;
    const TYPE_ID_JSON: &'static str = ASSIGNMENT_JSON;
}
impl ThingModel for ProbeAssignment {
    fn thing_kind() -> __codegen::ThingKind {
        __codegen::ThingKind::Relation
    }
}
impl RelationModel for ProbeAssignment {}
impl CompleteModel for ProbeAssignment {
    type Create = ProbeCreate;

    fn iid(&self) -> &str {
        &self.0
    }
}
impl MaterializeModel for ProbeAssignment {
    fn materialize(row: &HydratedRow, _cap: &HydrationCapability) -> Result<Self, ValidationError> {
        Ok(Self(row.iid().to_owned()))
    }
}

#[derive(Default)]
struct State {
    opens: Vec<TxType>,
    calls: Vec<(String, GivenRowsSpec)>,
    responses: VecDeque<Response>,
    legacy_commits: usize,
    commits: usize,
    rollbacks: usize,
}

enum Response {
    Documents(Vec<Value>),
    Error,
}

#[derive(Clone, Copy)]
enum CommitBehavior {
    Success,
    Failure,
}

struct Backend {
    state: Arc<Mutex<State>>,
    commit: CommitBehavior,
}

impl DriverBackend for Backend {
    fn open_transaction(
        &self,
        _database: &str,
        tx_type: TxType,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
        self.state.lock().unwrap().opens.push(tx_type);
        let transaction = RecordingTransaction {
            state: Arc::clone(&self.state),
            commit: self.commit,
        };
        Box::pin(async move { Ok(Box::new(transaction) as Box<dyn TransactionOps>) })
    }

    fn is_open(&self) -> bool {
        true
    }

    fn server_version(&self) -> Option<_ProviderVersion> {
        Some(_ProviderVersion::new(3, 12, 1))
    }

    fn supports_given_rows(&self) -> bool {
        true
    }
}

struct RecordingTransaction {
    state: Arc<Mutex<State>>,
    commit: CommitBehavior,
}

impl TransactionOps for RecordingTransaction {
    fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        Box::pin(async { panic!("successor batch used the raw query seam") })
    }

    fn query_with_rows(
        &mut self,
        _typeql: &str,
        _rows: GivenRowsSpec,
    ) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        Box::pin(async { panic!("successor batch used the materializing Given seam") })
    }

    fn query_with_rows_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        rows: GivenRowsSpec,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        let response = {
            let mut state = self.state.lock().unwrap();
            state.calls.push((typeql.to_owned(), rows));
            state
                .responses
                .pop_front()
                .expect("unexpected successor batch provider call")
        };
        Box::pin(async move {
            match response {
                Response::Error => Err(OrmError::QueryExecution("redacted".to_owned())),
                Response::Documents(documents) => {
                    let mut reader = BoundedAnswerReader::new(limits);
                    reader.check_before_read()?;
                    for document in documents {
                        if reader.accept(AnswerItem::Document(document), consumer)?
                            == AnswerControl::Stop
                        {
                            break;
                        }
                    }
                    Ok(reader.stats())
                }
            }
        })
    }

    fn supports_given_rows(&self) -> bool {
        true
    }

    fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.state.lock().unwrap().legacy_commits += 1;
        Box::pin(async { Ok(()) })
    }

    fn commit_classified(&mut self) -> BoxFuture<'_, Result<(), ClassifiedCommitError>> {
        self.state.lock().unwrap().commits += 1;
        let behavior = self.commit;
        Box::pin(async move {
            match behavior {
                CommitBehavior::Success => Ok(()),
                CommitBehavior::Failure => Err(ClassifiedCommitError::Driver {
                    certainty: CommitFailureCertainty::DefinitelyAborted,
                    message: "redacted".to_owned(),
                }),
            }
        })
    }

    fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.state.lock().unwrap().rollbacks += 1;
        Box::pin(async { Ok(()) })
    }

    fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        Box::pin(async { Ok(()) })
    }
}

fn installed_from(source: &str, successor: bool) -> InstalledRuntimeProjection {
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("rust-batch.yaml").unwrap(), source)]).unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
    let emitter = RustEmitter::new();
    let handlers = if successor {
        emitter.generator_handlers_for(&resolved)
    } else {
        emitter.generator_handlers().to_vec()
    };
    let resources = if successor {
        emitter.code_resources_for(&resolved).unwrap()
    } else {
        emitter.code_resources().unwrap()
    };
    let projection = project(
        &resolved,
        BindingTarget::Rust,
        &ProjectionConfig::rust(),
        &handlers,
        &resources,
    )
    .unwrap();
    if successor {
        assert_eq!(
            projection.generator_handlers(),
            [ProjectionHandler::rust_v2()]
        );
    } else {
        assert_eq!(
            projection.generator_handlers(),
            [ProjectionHandler::rust_v1()]
        );
    }
    InstalledRuntimeProjection::try_new(projection).unwrap()
}

fn successor_installed() -> InstalledRuntimeProjection {
    installed_from(SUCCESSOR_SCHEMA, true)
}

fn fixture_named(
    name: &str,
    responses: Vec<Response>,
    commit: CommitBehavior,
    installed: InstalledRuntimeProjection,
) -> (crate::Database<TestSchema>, Arc<Mutex<State>>) {
    let state = Arc::new(Mutex::new(State {
        responses: responses.into(),
        ..State::default()
    }));
    let backend = Backend {
        state: Arc::clone(&state),
        commit,
    };
    (
        crate::Database::<TestSchema>::from_test_parts(
            OrmDatabase::with_backend(Box::new(backend), name),
            installed,
        ),
        state,
    )
}

fn fixture(
    responses: Vec<Response>,
    commit: CommitBehavior,
) -> (crate::Database<TestSchema>, Arc<Mutex<State>>) {
    fixture_named("rust-batch", responses, commit, successor_installed())
}

fn person_document(ordinal: usize, iid: &str, name: &str, tags: &[&str]) -> Value {
    let tags = tags
        .iter()
        .map(|tag| json!({"value": tag}))
        .collect::<Vec<_>>();
    json!({
        "ordinal": ordinal,
        "_iid": iid,
        "_type": "person",
        "attributes": {
            "name": [{"value": name}],
            "tag": tags,
        }
    })
}

fn assignment_document(ordinal: usize, iid: &str, position: &str) -> Value {
    json!({
        "ordinal": ordinal,
        "_iid": iid,
        "_type": "assignment",
        "attributes": {"position": [{"value": position}]},
        "role_players": [{
            "role": "assignment:worker",
            "iid": "0x99",
            "type_name": "person",
            "attributes": {"name": [{"value": "worker"}]},
        }]
    })
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Entity,
    Relation,
}

#[derive(Clone, Copy, Debug)]
enum Operation {
    Insert,
    Put,
    Update,
    Delete,
}

fn responses(kind: Kind, operation: Operation) -> Vec<Response> {
    match (kind, operation) {
        (Kind::Entity, Operation::Delete) | (Kind::Relation, Operation::Delete) => {
            vec![Response::Documents(vec![json!({"ordinal": 0})])]
        }
        (Kind::Entity, operation) => {
            let mut responses = Vec::new();
            if matches!(operation, Operation::Put) {
                responses.push(Response::Documents(vec![]));
            }
            responses.push(Response::Documents(vec![json!({
                "ordinal": 0,
                "iid": "0x10",
            })]));
            responses.push(Response::Documents(vec![person_document(
                0,
                "0x10",
                "person",
                &["one", "two"],
            )]));
            responses
        }
        (Kind::Relation, _) => vec![
            Response::Documents(vec![json!({
                "kind": 1,
                "ordinal": 0,
                "reference_ordinal": 0,
                "iid": "0x99",
                "type": "person",
            })]),
            Response::Documents(vec![json!({
                "ordinal": 0,
                "iid": "0x30",
            })]),
            Response::Documents(vec![assignment_document(0, "0x30", "position")]),
        ],
    }
}

fn expected_calls(kind: Kind, operation: Operation) -> usize {
    match (kind, operation) {
        (_, Operation::Delete) => 1,
        (Kind::Entity, Operation::Put) => 3,
        (Kind::Entity, _) => 2,
        (Kind::Relation, _) => 3,
    }
}

async fn run_owned(kind: Kind, operation: Operation) {
    let (database, state) = fixture(responses(kind, operation), CommitBehavior::Success);
    match (kind, operation) {
        (Kind::Entity, Operation::Insert) => {
            let output = database
                .entities::<Person>()
                .insert_many(vec![PersonCreate::named("person")])
                .await
                .unwrap();
            assert_eq!(output[0].iid, "0x10");
        }
        (Kind::Entity, Operation::Put) => {
            let output = database
                .entities::<Person>()
                .put_many(vec![PersonCreate::named("person")])
                .await
                .unwrap();
            assert_eq!(output[0].iid, "0x10");
        }
        (Kind::Entity, Operation::Update) => {
            let output = database
                .entities::<Person>()
                .update_many(vec![("0x10".to_owned(), PersonCreate::named("person"))])
                .await
                .unwrap();
            assert_eq!(output[0].iid, "0x10");
        }
        (Kind::Entity, Operation::Delete) => database
            .entities::<Person>()
            .delete_many(&["0x10".to_owned()])
            .await
            .unwrap(),
        (Kind::Relation, Operation::Insert) => {
            let output = database
                .relations::<Assignment>()
                .insert_many(vec![AssignmentCreate::named("position")])
                .await
                .unwrap();
            assert_eq!(output[0].iid, "0x30");
            assert_eq!(output[0].position, "position");
        }
        (Kind::Relation, Operation::Put) => {
            let output = database
                .relations::<Assignment>()
                .put_many(vec![AssignmentCreate::named("position")])
                .await
                .unwrap();
            assert_eq!(output[0].iid, "0x30");
            assert_eq!(output[0].position, "position");
        }
        (Kind::Relation, Operation::Update) => {
            let output = database
                .relations::<Assignment>()
                .update_many(vec![(
                    "0x30".to_owned(),
                    AssignmentCreate::named("position"),
                )])
                .await
                .unwrap();
            assert_eq!(output[0].iid, "0x30");
            assert_eq!(output[0].position, "position");
        }
        (Kind::Relation, Operation::Delete) => database
            .relations::<Assignment>()
            .delete_many(&["0x30".to_owned()])
            .await
            .unwrap(),
    }
    let state = state.lock().unwrap();
    assert_eq!(state.calls.len(), expected_calls(kind, operation));
    assert!(state.responses.is_empty());
    assert_eq!(state.legacy_commits, 0);
    assert_eq!(state.commits, 1);
    assert_eq!(state.rollbacks, 0);
}

async fn run_borrowed(kind: Kind, operation: Operation) {
    let (database, state) = fixture(responses(kind, operation), CommitBehavior::Success);
    let transaction = database.write().await.unwrap();
    match (kind, operation) {
        (Kind::Entity, Operation::Insert) => {
            transaction
                .entities::<Person>()
                .insert_many(vec![PersonCreate::named("person")])
                .await
                .unwrap();
        }
        (Kind::Entity, Operation::Put) => {
            transaction
                .entities::<Person>()
                .put_many(vec![PersonCreate::named("person")])
                .await
                .unwrap();
        }
        (Kind::Entity, Operation::Update) => {
            transaction
                .entities::<Person>()
                .update_many(vec![("0x10".to_owned(), PersonCreate::named("person"))])
                .await
                .unwrap();
        }
        (Kind::Entity, Operation::Delete) => transaction
            .entities::<Person>()
            .delete_many(&["0x10".to_owned()])
            .await
            .unwrap(),
        (Kind::Relation, Operation::Insert) => {
            transaction
                .relations::<Assignment>()
                .insert_many(vec![AssignmentCreate::named("position")])
                .await
                .unwrap();
        }
        (Kind::Relation, Operation::Put) => {
            transaction
                .relations::<Assignment>()
                .put_many(vec![AssignmentCreate::named("position")])
                .await
                .unwrap();
        }
        (Kind::Relation, Operation::Update) => {
            transaction
                .relations::<Assignment>()
                .update_many(vec![(
                    "0x30".to_owned(),
                    AssignmentCreate::named("position"),
                )])
                .await
                .unwrap();
        }
        (Kind::Relation, Operation::Delete) => transaction
            .relations::<Assignment>()
            .delete_many(&["0x30".to_owned()])
            .await
            .unwrap(),
    }
    {
        let state = state.lock().unwrap();
        assert_eq!(state.calls.len(), expected_calls(kind, operation));
        assert_eq!(state.legacy_commits, 0);
        assert_eq!(state.commits, 0);
        assert_eq!(state.rollbacks, 0);
    }
    transaction.commit().await.unwrap();
    let state = state.lock().unwrap();
    assert!(state.responses.is_empty());
    assert_eq!(state.legacy_commits, 0);
    assert_eq!(state.commits, 1);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn rust_v2_routes_all_four_operations_for_both_kinds_owned_and_borrowed() {
    for kind in [Kind::Entity, Kind::Relation] {
        for operation in [
            Operation::Insert,
            Operation::Put,
            Operation::Update,
            Operation::Delete,
        ] {
            run_owned(kind, operation).await;
            run_borrowed(kind, operation).await;
        }
    }
}

#[tokio::test]
async fn successor_empty_batches_do_no_provider_work_on_every_public_surface() {
    let (database, state) = fixture(vec![], CommitBehavior::Success);
    assert!(
        database
            .entities::<Person>()
            .insert_many(vec![])
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        database
            .entities::<Person>()
            .put_many(vec![])
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        database
            .entities::<Person>()
            .update_many(vec![])
            .await
            .unwrap()
            .is_empty()
    );
    database
        .entities::<Person>()
        .delete_many(&[])
        .await
        .unwrap();
    assert!(
        database
            .relations::<Assignment>()
            .insert_many(vec![])
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        database
            .relations::<Assignment>()
            .put_many(vec![])
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        database
            .relations::<Assignment>()
            .update_many(vec![])
            .await
            .unwrap()
            .is_empty()
    );
    database
        .relations::<Assignment>()
        .delete_many(&[])
        .await
        .unwrap();
    let authority = database
        .entities::<WrongPerson>()
        .insert_many(vec![])
        .await
        .unwrap_err();
    assert_eq!(authority.code(), Some("wrong_model_kind"));
    {
        let state = state.lock().unwrap();
        assert!(state.opens.is_empty());
        assert!(state.calls.is_empty());
    }

    let transaction = database.write().await.unwrap();
    transaction
        .entities::<Person>()
        .insert_many(vec![])
        .await
        .unwrap();
    transaction
        .entities::<Person>()
        .put_many(vec![])
        .await
        .unwrap();
    transaction
        .entities::<Person>()
        .update_many(vec![])
        .await
        .unwrap();
    transaction
        .entities::<Person>()
        .delete_many(&[])
        .await
        .unwrap();
    transaction
        .relations::<Assignment>()
        .insert_many(vec![])
        .await
        .unwrap();
    transaction
        .relations::<Assignment>()
        .put_many(vec![])
        .await
        .unwrap();
    transaction
        .relations::<Assignment>()
        .update_many(vec![])
        .await
        .unwrap();
    transaction
        .relations::<Assignment>()
        .delete_many(&[])
        .await
        .unwrap();
    let authority = transaction
        .entities::<WrongPerson>()
        .insert_many(vec![])
        .await
        .unwrap_err();
    assert_eq!(authority.code(), Some("wrong_model_kind"));
    {
        let state = state.lock().unwrap();
        assert_eq!(state.opens, [TxType::Write]);
        assert!(state.calls.is_empty());
        assert_eq!(state.commits, 0);
    }
    transaction.rollback().await.unwrap();
}

#[tokio::test]
async fn owned_entity_results_preserve_row_and_ordered_collection_order() {
    let (database, state) = fixture(
        vec![
            Response::Documents(vec![
                json!({"ordinal": 1, "iid": "0x11"}),
                json!({"ordinal": 0, "iid": "0x10"}),
            ]),
            Response::Documents(vec![
                person_document(1, "0x11", "second", &["b", "a"]),
                person_document(0, "0x10", "first", &["z", "y"]),
            ]),
        ],
        CommitBehavior::Success,
    );
    let output = database
        .entities::<Person>()
        .insert_many(vec![
            PersonCreate::named("first"),
            PersonCreate::named("second"),
        ])
        .await
        .unwrap();
    assert_eq!(
        output
            .iter()
            .map(|person| person.name.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert_eq!(output[0].tags, ["z", "y"]);
    assert_eq!(output[1].tags, ["b", "a"]);
    let state = state.lock().unwrap();
    assert_eq!(state.commits, 1);
    assert_eq!(state.rollbacks, 0);
}

fn assert_row_prefix(error: &crate::Error, ordinal: u64) {
    assert!(matches!(
        error.diagnostic_path(),
        Some([
            ErrorPathSegment::Argument(argument),
            ErrorPathSegment::Index(index),
            ..
        ]) if argument == "rows" && *index == ordinal
    ));
}

#[tokio::test]
async fn duplicate_key_and_target_keep_common_details_and_run_no_hooks_or_io() {
    let (database, state) = fixture(vec![], CommitBehavior::Success);
    let hook = Arc::new(CountingHook::default());
    let mut manager = database.entities::<Person>();
    manager.add_hook(hook.clone());
    let key = manager
        .insert_many(vec![
            PersonCreate::named("same"),
            PersonCreate::named("same"),
        ])
        .await
        .unwrap_err();
    assert_eq!(key.category(), ErrorCategory::ModelValidation);
    assert_eq!(key.code(), Some("duplicate_batch_key"));
    assert_eq!(
        key.model_validation_phase(),
        Some(ModelValidationPhase::Input)
    );
    assert_eq!(
        key.diagnostic_path(),
        Some(
            &[
                ErrorPathSegment::Argument("rows".to_owned()),
                ErrorPathSegment::Index(1),
                ErrorPathSegment::Identifier("person:name".to_owned()),
            ][..]
        )
    );
    assert_eq!(
        key.details()
            .and_then(|details| details.get("first_conflicting_index")),
        Some(&ErrorDetail::Long(0))
    );
    assert_eq!(hook.before.load(Ordering::SeqCst), 0);
    assert_eq!(hook.after.load(Ordering::SeqCst), 0);

    let target = database
        .entities::<Person>()
        .update_many(vec![
            ("0xAb".to_owned(), PersonCreate::named("one")),
            ("0xab".to_owned(), PersonCreate::named("two")),
        ])
        .await
        .unwrap_err();
    assert_eq!(target.category(), ErrorCategory::ModelValidation);
    assert_eq!(target.code(), Some("duplicate_batch_target"));
    assert_eq!(
        target.diagnostic_path(),
        Some(
            &[
                ErrorPathSegment::Argument("rows".to_owned()),
                ErrorPathSegment::Index(1),
                ErrorPathSegment::Argument("iid".to_owned()),
            ][..]
        )
    );
    assert_eq!(
        target
            .details()
            .and_then(|details| details.get("first_conflicting_index")),
        Some(&ErrorDetail::Long(0))
    );
    let state = state.lock().unwrap();
    assert!(state.opens.is_empty());
    assert!(state.calls.is_empty());
}

#[tokio::test]
async fn generated_input_failures_keep_rows_index_and_field_suffix() {
    let (database, state) = fixture(vec![], CommitBehavior::Success);
    let error = database
        .entities::<Person>()
        .insert_many(vec![
            PersonCreate::named("ok"),
            PersonCreate::named("reject-input"),
        ])
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("rejected_create"));
    assert_eq!(
        error.path(),
        Some(&["rows".to_owned(), "[1]".to_owned(), "name".to_owned()][..])
    );
    assert_eq!(
        error.diagnostic_path(),
        Some(
            &[
                ErrorPathSegment::Argument("rows".to_owned()),
                ErrorPathSegment::Index(1),
                ErrorPathSegment::Field("name".to_owned()),
            ][..]
        )
    );

    let nested = database
        .entities::<Person>()
        .insert_many(vec![PersonCreate::named("reject-nested")])
        .await
        .unwrap_err();
    assert_eq!(nested.code(), Some("rejected_nested_create"));
    assert_eq!(
        nested.diagnostic_path(),
        Some(
            &[
                ErrorPathSegment::Argument("rows".to_owned()),
                ErrorPathSegment::Index(0),
                ErrorPathSegment::Field("children".to_owned()),
                ErrorPathSegment::Index(2),
                ErrorPathSegment::Field("name".to_owned()),
            ][..]
        )
    );
    let state = state.lock().unwrap();
    assert!(state.opens.is_empty());
    assert!(state.calls.is_empty());
}

#[tokio::test]
async fn invalid_later_iid_is_common_validated_before_hooks_and_model_authority_wins() {
    let (database, state) = fixture(vec![], CommitBehavior::Success);
    let hook = Arc::new(CountingHook::default());
    let mut manager = database.entities::<Person>();
    manager.add_hook(hook.clone());
    let error = manager
        .update_many(vec![
            ("0x10".to_owned(), PersonCreate::named("one")),
            ("not-an-iid".to_owned(), PersonCreate::named("two")),
        ])
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("noncanonical_iid"));
    assert_eq!(
        error.diagnostic_path(),
        Some(
            &[
                ErrorPathSegment::Argument("rows".to_owned()),
                ErrorPathSegment::Index(1),
                ErrorPathSegment::Argument("iid".to_owned()),
            ][..]
        )
    );
    assert_eq!(hook.before.load(Ordering::SeqCst), 0);

    let authority = database
        .entities::<WrongPerson>()
        .update_many(vec![(
            "not-an-iid".to_owned(),
            PersonCreate::named("ignored"),
        )])
        .await
        .unwrap_err();
    assert_eq!(authority.code(), Some("wrong_model_kind"));
    let state = state.lock().unwrap();
    assert!(state.opens.is_empty());
    assert!(state.calls.is_empty());
}

#[tokio::test]
async fn v1_delete_many_keeps_iid_first_and_does_not_run_hooks() {
    let (database, state) = fixture_named(
        "legacy-delete",
        vec![],
        CommitBehavior::Success,
        installed_from(SUCCESSOR_SCHEMA, false),
    );
    let hook = Arc::new(CountingHook::default());
    let mut manager = database.entities::<Person>();
    manager.add_hook(hook.clone());
    let error = manager
        .delete_many(&["not-an-iid".to_owned()])
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("invalid_iid"));
    assert_eq!(hook.before.load(Ordering::SeqCst), 0);
    let state = state.lock().unwrap();
    assert!(state.opens.is_empty());
    assert!(state.calls.is_empty());
}

#[tokio::test]
async fn v1_public_commit_keeps_the_released_unclassified_terminal_seam() {
    let (database, state) = fixture_named(
        "legacy-commit",
        vec![],
        CommitBehavior::Success,
        installed_from(SUCCESSOR_SCHEMA, false),
    );
    let transaction = database.write().await.unwrap();
    assert!(
        transaction
            .entities::<Person>()
            .insert_many(vec![])
            .await
            .unwrap()
            .is_empty()
    );
    transaction.commit().await.unwrap();
    let state = state.lock().unwrap();
    assert_eq!(state.legacy_commits, 1);
    assert_eq!(state.commits, 0);
    assert!(state.calls.is_empty());
}

#[tokio::test]
async fn materialization_failure_rolls_back_owned_and_makes_borrowed_commit_unavailable() {
    let failure_responses = || {
        vec![
            Response::Documents(vec![
                json!({"ordinal": 0, "iid": "0x10"}),
                json!({"ordinal": 1, "iid": "0x11"}),
            ]),
            Response::Documents(vec![
                person_document(0, "0x10", "ok", &["ok"]),
                person_document(1, "0x11", "reject-materialize", &["bad"]),
            ]),
        ]
    };
    let inputs = || {
        vec![
            PersonCreate::named("ok"),
            PersonCreate::named("reject-materialize"),
        ]
    };

    let (database, state) = fixture(failure_responses(), CommitBehavior::Success);
    let error = database
        .entities::<Person>()
        .insert_many(inputs())
        .await
        .unwrap_err();
    assert_eq!(error.category(), ErrorCategory::Integrity);
    assert_eq!(error.code(), Some("generated_model_materialization_failed"));
    assert_eq!(
        error.diagnostic_path(),
        Some(
            &[
                ErrorPathSegment::Argument("rows".to_owned()),
                ErrorPathSegment::Index(1),
            ][..]
        )
    );
    {
        let state = state.lock().unwrap();
        assert_eq!(state.commits, 0);
        assert_eq!(state.rollbacks, 1);
    }

    let (database, state) = fixture(failure_responses(), CommitBehavior::Success);
    let transaction = database.write().await.unwrap();
    let error = transaction
        .entities::<Person>()
        .insert_many(inputs())
        .await
        .unwrap_err();
    assert_eq!(error.category(), ErrorCategory::Integrity);
    assert_eq!(error.code(), Some("generated_model_materialization_failed"));
    assert_row_prefix(&error, 1);
    let commit = transaction.commit().await.unwrap_err();
    assert_eq!(commit.category(), ErrorCategory::Transaction);
    assert_eq!(commit.code(), Some("transaction_rollback_only"));
    let state = state.lock().unwrap();
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn owned_provider_and_commit_failures_have_certain_terminal_behavior() {
    let (database, state) = fixture(vec![Response::Error], CommitBehavior::Success);
    let provider = database
        .entities::<Person>()
        .insert_many(vec![PersonCreate::named("person")])
        .await
        .unwrap_err();
    assert_eq!(provider.category(), ErrorCategory::QueryExecution);
    assert_eq!(provider.code(), Some("provider_operation_failed"));
    assert_eq!(
        provider.diagnostic_path(),
        Some(&[ErrorPathSegment::Identifier("entity:person".to_owned())][..])
    );
    assert_eq!(
        provider
            .details()
            .and_then(|details| details.get("operation")),
        Some(&ErrorDetail::Text("write".to_owned()))
    );
    {
        let state = state.lock().unwrap();
        assert_eq!(state.commits, 0);
        assert_eq!(state.rollbacks, 1);
    }

    let (database, state) = fixture(
        responses(Kind::Entity, Operation::Insert),
        CommitBehavior::Failure,
    );
    let commit = database
        .entities::<Person>()
        .insert_many(vec![PersonCreate::named("person")])
        .await
        .unwrap_err();
    assert_eq!(commit.category(), ErrorCategory::Transaction);
    assert_eq!(commit.code(), Some("commit_definitely_aborted"));
    assert_eq!(
        commit.diagnostic_path(),
        Some(&[ErrorPathSegment::Identifier("entity:person".to_owned())][..])
    );
    assert_eq!(
        commit
            .details()
            .and_then(|details| details.get("commit_outcome")),
        Some(&ErrorDetail::Text("definitely_aborted".to_owned()))
    );
    let state = state.lock().unwrap();
    assert_eq!(state.commits, 1);
    assert_eq!(state.rollbacks, 0);
}

#[derive(Default)]
struct CountingHook {
    before: AtomicUsize,
    after: AtomicUsize,
}

impl LifecycleHook for CountingHook {
    fn name(&self) -> &str {
        "counting"
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

#[derive(Default)]
struct RejectSecondHook {
    before: AtomicUsize,
    after: AtomicUsize,
}

impl LifecycleHook for RejectSecondHook {
    fn name(&self) -> &str {
        "reject-second"
    }

    fn before_operation<'a>(
        &'a self,
        _context: &'a mut HookContext<'_>,
    ) -> HookFuture<'a, Result<PreHookResult, HookError>> {
        let ordinal = self.before.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if ordinal == 1 {
                Ok(PreHookResult::Reject {
                    reason: "rejected".to_owned(),
                })
            } else {
                Ok(PreHookResult::Continue)
            }
        })
    }

    fn after_operation<'a>(
        &'a self,
        _context: &'a HookContext<'_>,
    ) -> HookFuture<'a, Result<(), HookError>> {
        self.after.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}

struct TimingHook {
    state: Arc<Mutex<State>>,
    before: AtomicUsize,
    after: AtomicUsize,
    violation: AtomicBool,
}

impl LifecycleHook for TimingHook {
    fn name(&self) -> &str {
        "timing"
    }

    fn before_operation<'a>(
        &'a self,
        _context: &'a mut HookContext<'_>,
    ) -> HookFuture<'a, Result<PreHookResult, HookError>> {
        self.before.fetch_add(1, Ordering::SeqCst);
        let state = self.state.lock().unwrap();
        if !state.opens.is_empty()
            || !state.calls.is_empty()
            || state.commits != 0
            || state.rollbacks != 0
        {
            self.violation.store(true, Ordering::SeqCst);
        }
        Box::pin(async { Ok(PreHookResult::Continue) })
    }

    fn after_operation<'a>(
        &'a self,
        _context: &'a HookContext<'_>,
    ) -> HookFuture<'a, Result<(), HookError>> {
        self.after.fetch_add(1, Ordering::SeqCst);
        let state = self.state.lock().unwrap();
        if state.commits != 1 || state.rollbacks != 0 {
            self.violation.store(true, Ordering::SeqCst);
        }
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn hooks_run_before_common_work_and_only_after_owned_commit() {
    let (database, state) = fixture(
        vec![
            Response::Documents(vec![
                json!({"ordinal": 1, "iid": "0x11"}),
                json!({"ordinal": 0, "iid": "0x10"}),
            ]),
            Response::Documents(vec![
                person_document(1, "0x11", "second", &["second"]),
                person_document(0, "0x10", "first", &["first"]),
            ]),
        ],
        CommitBehavior::Success,
    );
    let hook = Arc::new(TimingHook {
        state: Arc::clone(&state),
        before: AtomicUsize::new(0),
        after: AtomicUsize::new(0),
        violation: AtomicBool::new(false),
    });
    let mut manager = database.entities::<Person>();
    manager.add_hook(hook.clone());
    manager
        .insert_many(vec![
            PersonCreate::named("first"),
            PersonCreate::named("second"),
        ])
        .await
        .unwrap();
    assert_eq!(hook.before.load(Ordering::SeqCst), 2);
    assert_eq!(hook.after.load(Ordering::SeqCst), 2);
    assert!(!hook.violation.load(Ordering::SeqCst));

    let (database, state) = fixture(vec![Response::Error], CommitBehavior::Success);
    let hook = Arc::new(CountingHook::default());
    let mut manager = database.entities::<Person>();
    manager.add_hook(hook.clone());
    manager
        .insert_many(vec![
            PersonCreate::named("first"),
            PersonCreate::named("second"),
        ])
        .await
        .unwrap_err();
    assert_eq!(hook.before.load(Ordering::SeqCst), 2);
    assert_eq!(hook.after.load(Ordering::SeqCst), 0);
    {
        let state = state.lock().unwrap();
        assert_eq!(state.rollbacks, 1);
        assert_eq!(state.commits, 0);
    }

    let (database, state) = fixture(vec![], CommitBehavior::Success);
    let hook = Arc::new(RejectSecondHook::default());
    let mut manager = database.entities::<Person>();
    manager.add_hook(hook.clone());
    let error = manager
        .insert_many(vec![
            PersonCreate::named("first"),
            PersonCreate::named("second"),
        ])
        .await
        .unwrap_err();
    assert_eq!(error.category(), ErrorCategory::Lifecycle);
    assert_eq!(error.code(), Some("lifecycle_hook_rejected"));
    assert_eq!(
        error.diagnostic_path(),
        Some(
            &[
                ErrorPathSegment::Argument("rows".to_owned()),
                ErrorPathSegment::Index(1),
            ][..]
        )
    );
    assert_eq!(hook.before.load(Ordering::SeqCst), 2);
    assert_eq!(hook.after.load(Ordering::SeqCst), 0);
    let state = state.lock().unwrap();
    assert!(state.opens.is_empty());
    assert!(state.calls.is_empty());
}

#[tokio::test]
async fn foreign_origin_package_and_model_fences_precede_provider_io() {
    let (source, _) = fixture_named(
        "source",
        responses(Kind::Entity, Operation::Insert),
        CommitBehavior::Success,
        successor_installed(),
    );
    let person = source
        .entities::<Person>()
        .insert_many(vec![PersonCreate::named("person")])
        .await
        .unwrap()
        .remove(0);
    let (target, state) = fixture_named(
        "target",
        vec![],
        CommitBehavior::Success,
        successor_installed(),
    );
    let origin = target
        .relations::<Assignment>()
        .insert_many(vec![AssignmentCreate {
            position: "position".to_owned(),
            worker_iid: person.iid,
            worker_origin: person.origin,
        }])
        .await
        .unwrap_err();
    assert_eq!(origin.code(), Some("reference_database_mismatch"));

    let foreign = installed_from(FOREIGN_SCHEMA, true);
    let target_installed = target.installed_schema().unwrap();
    let person_id = TypeId::new(TypeKind::Entity, "person").unwrap();
    let foreign_create = crate::projected_codec::project_create(
        PersonCreate::named("foreign"),
        &person_id,
        &foreign,
    )
    .unwrap();
    let package = prepare_batch(
        target_installed,
        person_id,
        ProjectedBatchOperation::Insert,
        vec![ProjectedBatchRow::Create(foreign_create)],
    )
    .unwrap_err();
    assert_eq!(package.code(), Some("generated_token_package_mismatch"));

    let model = target
        .entities::<Person>()
        .insert_many(vec![PersonCreate::named("foreign-field")])
        .await
        .unwrap_err();
    assert_eq!(model.code(), Some("unexpected_field_evidence"));
    assert_row_prefix(&model, 0);
    let state = state.lock().unwrap();
    assert!(state.opens.is_empty());
    assert!(state.calls.is_empty());
}

#[tokio::test]
async fn hard_row_cap_precedes_adapter_reservation_and_projection() {
    let limit = usize::try_from(MAX_QUERY_ITEMS).unwrap();
    let allocation = reserved_binding_vec_inner::<()>(limit + 1, true).unwrap_err();
    assert_eq!(allocation.code(), Some("batch_item_limit"));

    let (database, state) = fixture(vec![], CommitBehavior::Success);
    let projections = Arc::new(AtomicUsize::new(0));
    let inputs = || {
        (0..=limit)
            .map(|_| ProbeCreate(Arc::clone(&projections)))
            .collect()
    };
    let hook = Arc::new(CountingHook::default());
    let mut entities = database.entities::<ProbePerson>();
    entities.add_hook(hook.clone());
    let error = entities.insert_many(inputs()).await.unwrap_err();
    assert_eq!(error.code(), Some("batch_item_limit"));
    assert_eq!(
        error.diagnostic_path(),
        Some(
            &[
                ErrorPathSegment::Argument("limits".to_owned()),
                ErrorPathSegment::Argument("items".to_owned()),
            ][..]
        )
    );
    assert_eq!(
        error.details().and_then(|details| details.get("actual")),
        Some(&ErrorDetail::Long(
            i64::try_from(MAX_QUERY_ITEMS + 1).unwrap()
        ))
    );
    assert_eq!(
        error.details().and_then(|details| details.get("maximum")),
        Some(&ErrorDetail::Long(i64::try_from(MAX_QUERY_ITEMS).unwrap()))
    );
    let mut relations = database.relations::<ProbeAssignment>();
    relations.add_hook(hook.clone());
    let error = relations.insert_many(inputs()).await.unwrap_err();
    assert_eq!(error.code(), Some("batch_item_limit"));
    assert_eq!(hook.before.load(Ordering::SeqCst), 0);

    let transaction = database.write().await.unwrap();
    let error = transaction
        .entities::<ProbePerson>()
        .insert_many(inputs())
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("batch_item_limit"));
    let error = transaction
        .relations::<ProbeAssignment>()
        .insert_many(inputs())
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("batch_item_limit"));
    assert_eq!(projections.load(Ordering::SeqCst), 0);
    {
        let state = state.lock().unwrap();
        assert_eq!(state.opens, [TxType::Write]);
        assert!(state.calls.is_empty());
    }
    transaction.rollback().await.unwrap();
}

#[test]
fn adapter_allocation_failures_are_shared_rows_resource_diagnostics() {
    let error = reserved_binding_vec_inner::<()>(1, true).unwrap_err();
    assert_eq!(error.category(), ErrorCategory::ResourceLimit);
    assert_eq!(error.code(), Some("projected_batch_allocation_exhausted"));
    assert_eq!(
        error.diagnostic_path(),
        Some(&[ErrorPathSegment::Argument("rows".to_owned())][..])
    );

    let installed = successor_installed();
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let name = OwnsFactId::new(person.clone(), AttributeId::new("name").unwrap()).unwrap();
    let tag = OwnsFactId::new(person.clone(), AttributeId::new("tag").unwrap()).unwrap();
    let string = |attribute: &str, value: &str| {
        ProjectedAttributeValue::try_new(
            &installed,
            TypeId::new(TypeKind::Attribute, attribute).unwrap(),
            CanonicalValue::String(CanonicalString::new(value).unwrap()),
        )
        .unwrap()
    };
    let thing = ProjectedThing::try_new(
        &installed,
        person,
        "0x10".to_owned(),
        vec![
            (name, vec![string("name", "person")]),
            (tag, vec![string("tag", "tag")]),
        ],
        vec![],
    )
    .unwrap();
    let diagnostic = materialize_result_inner::<Person>(
        ProjectedBatchResult::Things(vec![thing]),
        &installed,
        true,
    )
    .unwrap_err();
    assert_eq!(
        diagnostic.code().as_str(),
        "projected_batch_allocation_exhausted"
    );
    assert!(matches!(
        diagnostic.path(),
        [SdkDiagnosticPathSegment::Argument(argument)] if argument.as_str() == "rows"
    ));
}

#[test]
fn projected_batch_error_conversion_preserves_closed_category_code_path_and_details() {
    let installed = successor_installed();
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let create = crate::projected_codec::project_create(
        PersonCreate::named("duplicate"),
        &person,
        &installed,
    )
    .unwrap();
    let invalid = ProjectedBatch::try_new(
        &installed,
        person,
        ProjectedBatchOperation::Insert,
        vec![
            ProjectedBatchRow::Create(create.clone()),
            ProjectedBatchRow::Create(create),
        ],
    )
    .unwrap_err();
    let invalid = Error::from_projected_batch(invalid, ModelValidationPhase::Input);
    assert_eq!(invalid.category(), ErrorCategory::ModelValidation);
    assert_eq!(invalid.code(), Some("duplicate_batch_key"));
    assert_eq!(
        invalid.model_validation_phase(),
        Some(ModelValidationPhase::Input)
    );
    assert_eq!(
        invalid.diagnostic_path(),
        Some(
            &[
                ErrorPathSegment::Argument("rows".to_owned()),
                ErrorPathSegment::Index(1),
                ErrorPathSegment::Identifier("person:name".to_owned()),
            ][..]
        )
    );
    assert_eq!(
        invalid
            .details()
            .and_then(|details| details.get("first_conflicting_index")),
        Some(&ErrorDetail::Long(0))
    );

    let integrity = Error::from_projected_batch(
        ProjectedBatchExecutor::binding_materialization_failure(3),
        ModelValidationPhase::Hydration,
    );
    assert_eq!(integrity.category(), ErrorCategory::Integrity);
    assert_eq!(
        integrity.code(),
        Some("generated_model_materialization_failed")
    );
    assert_eq!(
        integrity.diagnostic_path(),
        Some(
            &[
                ErrorPathSegment::Argument("rows".to_owned()),
                ErrorPathSegment::Index(3),
            ][..]
        )
    );

    let provider = Error::from_projected_batch(
        SdkExecutionDiagnostic::provider_failure(SdkProviderOperation::Write),
        ModelValidationPhase::Input,
    );
    assert_eq!(provider.category(), ErrorCategory::QueryExecution);
    assert_eq!(provider.code(), Some("provider_operation_failed"));
    assert_eq!(
        provider
            .details()
            .and_then(|details| details.get("operation")),
        Some(&ErrorDetail::Text("write".to_owned()))
    );

    let transaction = Error::from_projected_batch(
        SdkExecutionDiagnostic::commit_failure(SdkCommitFailureOutcome::DefinitelyAborted),
        ModelValidationPhase::Input,
    );
    assert_eq!(transaction.category(), ErrorCategory::Transaction);
    assert_eq!(transaction.code(), Some("commit_definitely_aborted"));
    assert_eq!(
        transaction
            .details()
            .and_then(|details| details.get("commit_outcome")),
        Some(&ErrorDetail::Text("definitely_aborted".to_owned()))
    );

    let cancelled = Error::from_projected_batch(
        SdkExecutionDiagnostic::data_operation_cancelled(),
        ModelValidationPhase::Input,
    );
    assert_eq!(cancelled.category(), ErrorCategory::Cancelled);
    assert_eq!(cancelled.code(), Some("provider_cancelled"));

    let resource = Error::from_projected_batch(
        ProjectedBatch::binding_allocation_failure(),
        ModelValidationPhase::Input,
    );
    assert_eq!(resource.category(), ErrorCategory::ResourceLimit);
    assert_eq!(
        resource.code(),
        Some("projected_batch_allocation_exhausted")
    );
    assert_eq!(
        resource.diagnostic_path(),
        Some(&[ErrorPathSegment::Argument("rows".to_owned())][..])
    );
}
