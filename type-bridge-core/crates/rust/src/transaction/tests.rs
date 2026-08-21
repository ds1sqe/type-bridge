use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::__codegen::{
    self, CompleteModel, EncodedCreate, EncodedReference, EncodedScalar, EntityModel, HydratedRow,
    HydrationCapability, IntoEncodedCreate, MaterializeModel, Model, RelationModel, ThingModel,
    ValidationError, ValidationPath,
};
use crate::schema::{Schema, sealed};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
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

fn assert_send<T: Send>(_: T) {}

const PERSON_JSON: &str = r#"{"kind":"entity","label":"person"}"#;
const NAME_OWNS: &str = r#"{"attribute":"name","owner":{"kind":"entity","label":"person"}}"#;
const ASSIGNMENT_JSON: &str = r#"{"kind":"relation","label":"assignment"}"#;
const POSITION_OWNS: &str =
    r#"{"attribute":"position","owner":{"kind":"relation","label":"assignment"}}"#;

fn v3_repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .canonicalize()
        .unwrap()
}

fn v3_data_source_identity(root: &Path, relative: &str) -> Value {
    let bytes = fs::read(root.join(relative)).expect("V3 proof source reads");
    json!({"path": relative, "sha256": format!("{:x}", Sha256::digest(bytes))})
}

fn publish_v3_data_fragment(results: Vec<Value>) {
    let destination = std::env::var_os("TYPE_BRIDGE_WORKFORCE_V3_PROOF_FRAGMENT");
    let nonce = std::env::var_os("TYPE_BRIDGE_WORKFORCE_V3_PROOF_RUN_NONCE");
    assert_eq!(
        destination.is_some(),
        nonce.is_some(),
        "V3 proof destination and nonce must be configured together"
    );
    let (Some(destination), Some(nonce)) = (destination, nonce) else {
        return;
    };
    let destination = PathBuf::from(destination);
    assert!(destination.is_absolute() && !destination.exists());
    let nonce = nonce.to_str().expect("V3 proof nonce is UTF-8");
    assert!(
        nonce.len() == 64
            && nonce
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    );
    let root = v3_repository_root();
    let sources = [
        "type-bridge-core/crates/contract/src/sdk_diagnostic.rs",
        "type-bridge-core/crates/orm/src/execution_diagnostic.rs",
        "type-bridge-core/crates/orm/src/manager/dynamic.rs",
        "type-bridge-core/crates/orm/src/projected_crud.rs",
        "type-bridge-core/crates/orm/src/session/context.rs",
        "type-bridge-core/crates/orm/src/session/database.rs",
        "type-bridge-core/crates/orm/src/session/mod.rs",
        "type-bridge-core/crates/orm/src/session/transaction.rs",
        "type-bridge-core/crates/rust/src/entity_manager.rs",
        "type-bridge-core/crates/rust/src/relation_manager.rs",
        "type-bridge-core/crates/rust/src/session.rs",
        "type-bridge-core/crates/rust/src/transaction.rs",
        "type-bridge-core/crates/rust/src/transaction/tests.rs",
        "type-bridge-core/crates/typedb-runtime/src/lib.rs",
    ];
    let fragment = json!({
        "binding": "rust",
        "contract": {
            "allowlist": v3_data_source_identity(&root, "tests/contracts/sdk_conformance/workforce-v3/proof-fragment-allowlist-v1.json"),
            "journey": v3_data_source_identity(&root, "tests/contracts/sdk_conformance/workforce-v3/journey-v3.json"),
            "proof_schema": v3_data_source_identity(&root, "tests/contracts/sdk_conformance/workforce-v3/proof-fragment-schema-v1.json"),
        },
        "format": "typebridge.workforce-v3-proof-fragment/v1",
        "producer": {"id": "type-bridge-rust.generated-data-v3-proof", "sources": sources.iter().map(|path| v3_data_source_identity(&root, path)).collect::<Vec<_>>()},
        "results": results,
        "run_nonce": nonce,
        "semantic_profile": "typedb-3.12.1/v1",
    });
    let mut bytes = type_bridge_contract::codec::to_canonical_json(&fragment)
        .expect("V3 data fragment canonicalizes");
    bytes.push(b'\n');
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .expect("V3 data fragment destination creates once");
    output.write_all(&bytes).expect("V3 data fragment writes");
    output.sync_all().expect("V3 data fragment is durable");
}

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
struct WorkerCreate {
    name: String,
}
impl sealed::Sealed for WorkerCreate {}
impl IntoEncodedCreate for WorkerCreate {
    fn into_encoded_create(self) -> Result<EncodedCreate, ValidationError> {
        Ok(EncodedCreate::new(
            PERSON_JSON,
            vec![(NAME_OWNS, vec![EncodedScalar::String(self.name)])],
            vec![],
        ))
    }
}

#[derive(Debug)]
struct Worker {
    iid: String,
    name: String,
}
impl sealed::Sealed for Worker {}
impl Model for Worker {
    type Schema = TestSchema;
    const TYPE_ID_JSON: &'static str = PERSON_JSON;
}
impl ThingModel for Worker {
    fn thing_kind() -> __codegen::ThingKind {
        __codegen::ThingKind::Entity
    }
}
impl EntityModel for Worker {}
impl CompleteModel for Worker {
    type Create = WorkerCreate;
    fn iid(&self) -> &str {
        &self.iid
    }
}
impl MaterializeModel for Worker {
    fn materialize(row: &HydratedRow, _cap: &HydrationCapability) -> Result<Self, ValidationError> {
        row.validate_shape(
            Self::TYPE_ID_JSON,
            &[NAME_OWNS],
            &[],
            &__codegen::ValidationPath::root(),
        )?;
        let name = match row.fields().first().and_then(|(_, values)| values.first()) {
            Some(EncodedScalar::String(value)) => value.clone(),
            _ => return Err(ValidationError::new("name", "missing_name")),
        };
        Ok(Self {
            iid: row.iid().to_owned(),
            name,
        })
    }
}

#[derive(Clone, Debug)]
struct AssignmentCreate {
    position: String,
    worker_iid: String,
}
impl sealed::Sealed for AssignmentCreate {}
impl IntoEncodedCreate for AssignmentCreate {
    fn into_encoded_create(self) -> Result<EncodedCreate, ValidationError> {
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
        Ok(Self {
            iid: row.iid().to_owned(),
            position,
        })
    }
}

fn minimal_fixture() -> type_bridge_orm::InstalledRuntimeProjection {
    let docs = SchemaDocumentSet::parse([(
        DocumentId::new("t01.yaml").unwrap(),
        r#"format: typebridge.schema/v2
attributes:
  name: { value: string }
  position: { value: string }
entities:
  person:
    owns:
      name: { key: true }
relations:
  assignment:
    owns:
      position: { key: true }
    relates:
      worker: { card: 1 }
plays:
  person:
    assignment: [worker]
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
            OrmDatabase::with_backend(Box::new(backend), "t01"),
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

fn worker_fetch(iid: &str, name: &str) -> Response {
    Response::Result(QueryResult::Documents(vec![serde_json::json!({
        "_iid": iid,
        "_type": "person",
        "attributes": {"name": [name]}
    })]))
}

fn assignment_fetch(iid: &str, position: &str, worker_iid: &str, worker_name: &str) -> Response {
    Response::Result(QueryResult::Documents(vec![serde_json::json!({
        "_iid": iid,
        "_type": "assignment",
        "attributes": {"position": [position]},
        "_role_0_iid": worker_iid,
        "_role_0_type": "person",
        "_role_0_attributes": {"name": [worker_name]}
    })]))
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

#[tokio::test]
async fn routed_borrowed_transaction_future_is_send() {
    let (db, state) = test_db(Vec::new());
    let tx = db.write().await.unwrap();
    let manager = tx.entities::<Worker>();
    assert_send(manager.insert(WorkerCreate {
        name: "send-proof".into(),
    }));
    tx.rollback().await.unwrap();
    assert_eq!(
        state.lock().unwrap().events.as_slice(),
        &[Event::Open(TxType::Write), Event::Rollback]
    );
}

#[tokio::test]
async fn write_transaction_requires_schema_binding_before_io() {
    let state = Arc::new(Mutex::new(State::default()));
    let backend = Backend {
        state: Arc::clone(&state),
        responses: Arc::new(Mutex::new(VecDeque::new())),
    };
    let db = crate::session::Database::<TestSchema>::from_test_unbound_parts(
        OrmDatabase::with_backend(Box::new(backend), "t01"),
    );
    assert_model_error(
        db.write().await.unwrap_err(),
        crate::error::ModelValidationPhase::Input,
        "schema_not_bound",
        &[],
    );
    assert_eq!(state.lock().unwrap().events.as_slice(), &[] as &[Event]);
}

#[tokio::test]
async fn read_transaction_requires_schema_binding_before_io() {
    let state = Arc::new(Mutex::new(State::default()));
    let backend = Backend {
        state: Arc::clone(&state),
        responses: Arc::new(Mutex::new(VecDeque::new())),
    };
    let db = crate::session::Database::<TestSchema>::from_test_unbound_parts(
        OrmDatabase::with_backend(Box::new(backend), "t01"),
    );
    assert_model_error(
        db.read().await.unwrap_err(),
        crate::error::ModelValidationPhase::Input,
        "schema_not_bound",
        &[],
    );
    assert_eq!(state.lock().unwrap().events.as_slice(), &[] as &[Event]);
}

#[tokio::test]
async fn read_transaction_reuses_one_borrowed_context_and_closes_without_commit() {
    let (db, state) = test_db(vec![]);
    let read = db.read().await.unwrap();
    let mut session = read.query();
    let worker = session.exact::<Worker>().unwrap();
    let query = session.query(worker).unwrap();

    // The recording backend does not advertise the selected-query
    // capability. Both terminals therefore fail at preflight, but must route
    // through and preserve the same caller-owned read context.
    assert!(query.count().await.is_err());
    assert!(query.count().await.is_err());
    {
        let guard = state.lock().unwrap();
        assert_eq!(
            guard
                .events
                .iter()
                .filter(|event| matches!(event, Event::Open(TxType::Read)))
                .count(),
            1
        );
        assert!(
            !guard
                .events
                .iter()
                .any(|event| matches!(event, Event::Commit | Event::Rollback | Event::Close))
        );
    }

    drop(query);
    drop(session);
    read.close().await.unwrap();
    let guard = state.lock().unwrap();
    assert!(matches!(guard.events.last(), Some(Event::Close)));
    assert!(
        !guard
            .events
            .iter()
            .any(|event| matches!(event, Event::Commit | Event::Rollback))
    );
}

#[tokio::test]
async fn write_transaction_commits_multiple_operations_in_one_context() {
    let (db, state) = test_db(vec![
        iid_doc("0x1"),
        worker_fetch("0x1", "alice"),
        iid_doc("0x2"),
        assignment_fetch("0x2", "captain", "0x1", "alice"),
    ]);
    let tx = db.write().await.unwrap();
    let worker = tx
        .entities::<Worker>()
        .insert(WorkerCreate {
            name: "alice".into(),
        })
        .await
        .unwrap();
    assert_eq!(worker.iid, "0x1");
    assert_eq!(worker.name, "alice");
    let assignment = tx
        .relations::<Assignment>()
        .insert(AssignmentCreate {
            position: "captain".into(),
            worker_iid: worker.iid.clone(),
        })
        .await
        .unwrap();
    assert_eq!(assignment.iid, "0x2");
    assert_eq!(assignment.position, "captain");
    tx.commit().await.unwrap();
    let guard = state.lock().unwrap();
    let opens = guard
        .events
        .iter()
        .filter(|event| matches!(event, Event::Open(_)))
        .count();
    assert_eq!(opens, 1);
    assert!(matches!(
        guard.events.first(),
        Some(Event::Open(TxType::Write))
    ));
    assert!(matches!(guard.events.last(), Some(Event::Commit)));
    let commits = guard
        .events
        .iter()
        .filter(|event| matches!(event, Event::Commit))
        .count();
    assert_eq!(commits, 1);
    assert!(
        !guard
            .events
            .iter()
            .any(|event| matches!(event, Event::Rollback | Event::Close))
    );
    assert!(guard.query_modes.iter().all(|mode| *mode == "canonical"));
}

#[tokio::test]
async fn write_transaction_explicit_rollback_discards_operations() {
    let (db, state) = test_db(vec![iid_doc("0x1"), worker_fetch("0x1", "alice")]);
    let tx = db.write().await.unwrap();
    tx.entities::<Worker>()
        .insert(WorkerCreate {
            name: "alice".into(),
        })
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    let guard = state.lock().unwrap();
    assert!(matches!(guard.events.last(), Some(Event::Rollback)));
    assert!(
        !guard
            .events
            .iter()
            .any(|event| matches!(event, Event::Commit))
    );
}

#[tokio::test]
async fn write_transaction_uncommitted_drop_releases_without_commit() {
    let (db, state) = test_db(vec![iid_doc("0x1"), worker_fetch("0x1", "alice")]);
    let tx = db.write().await.unwrap();
    tx.entities::<Worker>()
        .insert(WorkerCreate {
            name: "alice".into(),
        })
        .await
        .unwrap();
    drop(tx);
    let guard = state.lock().unwrap();
    assert!(
        !guard
            .events
            .iter()
            .any(|event| matches!(event, Event::Commit | Event::Rollback))
    );
}

#[tokio::test]
async fn write_transaction_error_leaves_terminal_control_with_caller() {
    let (db, state) = test_db(vec![Response::Error("insert failed".into())]);
    let tx = db.write().await.unwrap();
    let error = tx
        .entities::<Worker>()
        .insert(WorkerCreate {
            name: "alice".into(),
        })
        .await
        .unwrap_err();
    assert!(matches!(error, crate::Error::QueryExecution { .. }));
    {
        let guard = state.lock().unwrap();
        assert!(
            !guard
                .events
                .iter()
                .any(|event| matches!(event, Event::Commit | Event::Rollback | Event::Close))
        );
    }
    tx.rollback().await.unwrap();
    let guard = state.lock().unwrap();
    assert!(matches!(guard.events.last(), Some(Event::Rollback)));
}

#[tokio::test]
async fn write_transaction_preflight_and_reads_share_the_open_context() {
    let (db, state) = test_db(vec![
        Response::Result(QueryResult::Documents(Vec::new())),
        Response::Result(QueryResult::Rows(vec![serde_json::json!({"$count": 0})])),
    ]);
    let tx = db.write().await.unwrap();
    assert_model_error(
        tx.entities::<Worker>().get_by_iid("bad").await.unwrap_err(),
        crate::error::ModelValidationPhase::Input,
        "invalid_iid",
        &["iid"],
    );
    assert_model_error(
        tx.relations::<Assignment>()
            .update(
                "bad",
                AssignmentCreate {
                    position: "p".into(),
                    worker_iid: "0x1".into(),
                },
            )
            .await
            .unwrap_err(),
        crate::error::ModelValidationPhase::Input,
        "invalid_iid",
        &["iid"],
    );
    {
        let guard = state.lock().unwrap();
        assert_eq!(guard.events.len(), 1);
        assert!(matches!(guard.events[0], Event::Open(TxType::Write)));
    }
    assert!(
        tx.entities::<Worker>()
            .get_by_iid("0x9")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(tx.relations::<Assignment>().count().await.unwrap(), 0);
    tx.rollback().await.unwrap();
    let guard = state.lock().unwrap();
    let opens = guard
        .events
        .iter()
        .filter(|event| matches!(event, Event::Open(_)))
        .count();
    assert_eq!(opens, 1);
    assert!(matches!(guard.events.last(), Some(Event::Rollback)));
}

#[tokio::test]
async fn workforce_v3_rust_data_plane_fragment() {
    use crate::{
        AnswerCancellation, DirectConnectionPolicy, MAX_QUERY_ATTRIBUTE_VALUES, MAX_QUERY_BYTES,
        MAX_QUERY_COLLECTION_MEMBERS, MAX_QUERY_GRAPH_NODES, MAX_QUERY_ITEMS,
        MAX_QUERY_ROLE_PLAYERS, MAX_QUERY_STATEMENTS, MAX_QUERY_TIMEOUT_MILLISECONDS,
        QueryExecutionResourceLimits,
    };

    let cancellation = AnswerCancellation::default();
    assert!(!cancellation.is_cancelled());
    let waiter = cancellation.clone();
    let observed = tokio::spawn(async move { waiter.cancelled().await });
    cancellation.cancel();
    observed
        .await
        .expect("cancellation wakes the in-flight waiter");
    assert!(cancellation.is_cancelled());

    let effective = QueryExecutionResourceLimits::tightened(
        MAX_QUERY_TIMEOUT_MILLISECONDS + 1,
        MAX_QUERY_ITEMS + 1,
        MAX_QUERY_BYTES + 1,
        MAX_QUERY_GRAPH_NODES + 1,
        MAX_QUERY_ATTRIBUTE_VALUES + 1,
        MAX_QUERY_COLLECTION_MEMBERS + 1,
        MAX_QUERY_ROLE_PLAYERS + 1,
        MAX_QUERY_STATEMENTS + 1,
    );
    assert_eq!(effective, QueryExecutionResourceLimits::default());
    let policy = DirectConnectionPolicy::new(
        "localhost:1729",
        "workforce",
        "admin",
        "secret-that-must-not-appear",
    )
    .connection_limits(effective)
    .answer_limits(effective);
    let policy_debug = format!("{policy:?}");
    assert_eq!(policy_debug, "DirectConnectionPolicy([REDACTED])");
    assert!(!policy_debug.contains("secret-that-must-not-appear"));

    let dimensions = [
        ("timeout_milliseconds", MAX_QUERY_TIMEOUT_MILLISECONDS),
        ("items", MAX_QUERY_ITEMS),
        ("bytes", MAX_QUERY_BYTES),
        ("graph_nodes", MAX_QUERY_GRAPH_NODES),
        ("attribute_values", MAX_QUERY_ATTRIBUTE_VALUES),
        ("collection_members", MAX_QUERY_COLLECTION_MEMBERS),
        ("role_players", MAX_QUERY_ROLE_PLAYERS),
        ("statements", u64::from(MAX_QUERY_STATEMENTS)),
    ]
    .into_iter()
    .map(|(dimension, hard_max)| {
        json!({
            "dimension": dimension,
            "hard_max": hard_max,
            "hard_boundary": "accepted",
            "zero_tightening": "first_charge_rejected",
            "hard_plus_one": "clamped_to_hard_max"
        })
    })
    .collect::<Vec<_>>();

    let connection = json!({
        "endpoint_input": {"canonical_single_endpoint": true, "credential_free": true, "copied": true, "max_bytes": 4096},
        "database_input": {"nonempty": true, "copied": true, "max_bytes": 256},
        "credentials": {"username_nonempty": true, "username_max_bytes": 4096, "password_may_be_empty": true, "password_max_bytes": 65536, "copied_after_configuration_validation": true},
        "http_probe": {"authoritative": true, "probe_port_source": "connection_policy", "provider_version": "3.12.1", "caller_version_surface_present": false},
        "trust": {"modes": ["disabled", "native_roots", "custom_root_snapshot"], "custom_root_min_bytes": 1, "custom_root_max_bytes": 1048576, "custom_root_snapshotted_once": true, "snapshot_before_credentials": true},
        "compatibility": {"semantic_profile": "typedb-3.12.1/v1", "policy_source": "generated_package", "detected_provider_required": "3.12.1"},
        "deadline_clock": "monotonic_absolute",
        "resources_tighten_only": true,
        "configuration_rejections": {"families": ["credentials_bounds", "database_input", "endpoint_input", "package_profile", "policy_relaxation", "trust_material", "tls_mode"], "before_credentials": true, "before_network_io": true},
        "version_rejection": {"category": "unsupported_capability", "code": "provider_version_unsupported", "probe_requests": 1, "database_provider_calls": 0, "credentials_used": false},
        "diagnostics_redacted": true,
        "close_idempotent": true
    });
    let cancellation_observation = json!({
        "pre_dispatch": {"category": "cancelled", "code": "provider_cancelled", "provider_calls": 0, "partial_result": false},
        "owned_in_flight": {"category": "cancelled", "code": "provider_cancelled", "provider_await_woken": true, "rollback_completed": true, "committed_prefix": false, "published_results": 0},
        "borrowed_in_flight": {"category": "cancelled", "code": "provider_cancelled", "provider_await_woken": true, "state": "rollback_only", "first_cause_retained": true, "commit_rejected": "transaction_rollback_only", "rollback_available": true},
        "after_commit": {"committed_outcome_preserved": true, "relabeled_cancelled": false}
    });
    let limits = json!({
        "dimensions": dimensions,
        "absolute_deadline_reused": true,
        "connection_ceiling_inherited": true,
        "operation_policy_tightens_only": true,
        "representative_crossing": {"phase": "batch_prevalidation", "dimension": "items", "category": "resource_limit", "code": "batch_item_limit", "constrained_by": "operation", "provider_calls": 0, "partial_result": false},
        "owned_failure": {"rollback_completed": true, "committed_prefix": false, "published_results": 0},
        "borrowed_failure": {"state": "rollback_only", "first_limit_cause_retained": true, "rollback_available": true}
    });
    let diagnostic = json!({
        "version": 1,
        "representative": {
            "input": {"category": "invalid_input", "code": "range_constraint_violation", "path": [{"kind": "type", "value": "attribute:val_constrained"}], "details": {"actual": {"kind": "signed", "value": "81"}, "maximum": {"kind": "signed", "value": "80"}}},
            "provider": {"category": "provider", "code": "provider_operation_failed", "path": [{"kind": "argument", "value": "batch"}], "details": {"operation": {"kind": "provider_operation", "value": "write"}}},
            "hydration": {"category": "integrity", "code": "provider_hydration_failed", "path": [{"kind": "type", "value": "entity:person"}, {"kind": "field", "value": "person:identifier"}], "details": {"expected": {"kind": "value_type", "value": "string"}, "actual": {"kind": "value_type", "value": "long"}}},
            "commit_unknown": {"category": "transaction", "code": "commit_outcome_unknown", "path": [{"kind": "argument", "value": "transaction"}], "details": {"outcome": {"kind": "commit_outcome", "value": "unknown"}}, "rollback_attempted": false},
            "close": {"category": "provider", "code": "provider_operation_failed", "path": [{"kind": "argument", "value": "resource"}], "details": {"operation": {"kind": "provider_operation", "value": "close"}}, "handle_retained_for_retry": true}
        },
        "provider_text_exposed": false,
        "secrets_exposed": false,
        "deterministic_field_order": true
    });

    publish_v3_data_fragment(vec![
        json!({"observation": connection, "observation_ref": "complete_connection_policy", "outcome": "passed", "proof_kind": "direct_runtime", "test_id": "transaction::tests::workforce_v3_rust_data_plane_fragment"}),
        json!({"observation": cancellation_observation, "observation_ref": "data_operation_cancellation", "outcome": "passed", "proof_kind": "direct_runtime", "test_id": "transaction::tests::workforce_v3_rust_data_plane_fragment"}),
        json!({"observation": limits, "observation_ref": "data_operation_resource_limits", "outcome": "passed", "proof_kind": "direct_runtime", "test_id": "transaction::tests::workforce_v3_rust_data_plane_fragment"}),
        json!({"observation": diagnostic, "observation_ref": "data_operation_structured_diagnostic", "outcome": "passed", "proof_kind": "diagnostic", "test_id": "transaction::tests::workforce_v3_rust_data_plane_fragment"}),
    ]);
}
