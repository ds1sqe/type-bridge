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
    self, AbstractModel, CompleteModel, EncodedCreate, EncodedScalar, EntityModel, HydratedRow,
    HydrationCapability, IntoEncodedCreate, MaterializeModel, Model, ModelFamily, SubtypeRootModel,
    ThingModel, ValidationError,
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

fn assert_send<T: Send>(_: T) {}

#[derive(Clone, Debug)]
struct RecordCreate {
    name: String,
    value: String,
}
impl sealed::Sealed for RecordCreate {}
impl IntoEncodedCreate for RecordCreate {
    fn into_encoded_create(self) -> Result<EncodedCreate, ValidationError> {
        if self.name == "reject-input" {
            return Err(ValidationError::new("name", "rejected_create"));
        }
        Ok(EncodedCreate::new(
            r#"{"kind":"entity","label":"record"}"#,
            vec![
                (
                    r#"{"attribute":"name","owner":{"kind":"entity","label":"record"}}"#,
                    vec![EncodedScalar::String(self.name)],
                ),
                (
                    r#"{"attribute":"text","owner":{"kind":"entity","label":"record"}}"#,
                    vec![EncodedScalar::String(self.value)],
                ),
            ],
            vec![],
        ))
    }
}

#[derive(Debug)]
struct Record {
    iid: String,
    name: String,
    value: String,
}

fn minimal_fixture() -> type_bridge_orm::InstalledRuntimeProjection {
    fixture_with_ordered_text(false)
}

fn fixture_with_ordered_text(ordered: bool) -> type_bridge_orm::InstalledRuntimeProjection {
    let docs = SchemaDocumentSet::parse([(
        DocumentId::new("e01.yaml").unwrap(),
        r#"format: typebridge.schema/v2
attributes:
  name: { value: string }
  text: { value: string }
entities:
  base: { abstract: true }
  record:
    sub: base
    owns:
      name: { key: true }
      text: { card: 1 }
  outside:
    owns:
      name: { key: true }
      text: { card: 1 }
"#
        .replace(
            "text: { card: 1 }",
            if ordered {
                "text: { ordered: true, card: 1 }"
            } else {
                "text: { card: 1 }"
            },
        )
        .as_str(),
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
impl sealed::Sealed for Record {}
impl Model for Record {
    type Schema = TestSchema;
    const TYPE_ID_JSON: &'static str = r#"{"kind":"entity","label":"record"}"#;
}
impl ThingModel for Record {
    fn thing_kind() -> __codegen::ThingKind {
        __codegen::ThingKind::Entity
    }
}
impl EntityModel for Record {}
impl CompleteModel for Record {
    type Create = RecordCreate;
    fn iid(&self) -> &str {
        &self.iid
    }
}
impl MaterializeModel for Record {
    fn materialize(row: &HydratedRow, _cap: &HydrationCapability) -> Result<Self, ValidationError> {
        row.validate_shape(
            Self::TYPE_ID_JSON,
            &[
                r#"{"attribute":"name","owner":{"kind":"entity","label":"record"}}"#,
                r#"{"attribute":"text","owner":{"kind":"entity","label":"record"}}"#,
            ],
            &[],
            &__codegen::ValidationPath::root(),
        )?;
        let name = match row.fields().first().and_then(|(_, v)| v.first()) {
            Some(EncodedScalar::String(v)) => v.clone(),
            _ => return Err(ValidationError::new("name", "missing_name")),
        };
        let value = match row.fields().get(1).and_then(|(_, v)| v.first()) {
            Some(EncodedScalar::String(v)) => v.clone(),
            _ => return Err(ValidationError::new("value", "missing_value")),
        };
        if value == "reject-materialize" {
            return Err(ValidationError::new("text", "rejected_materialization"));
        }
        Ok(Self {
            iid: row.iid().to_owned(),
            name,
            value,
        })
    }
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
    rollback_error: bool,
    close_error: bool,
}

impl DriverBackend for Backend {
    fn match_capabilities(&self) -> type_bridge_orm::CapabilitySet {
        type_bridge_orm::CapabilitySet::all()
    }
    fn open_transaction(
        &self,
        _db: &str,
        ty: TxType,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
        self.state.lock().unwrap().events.push(Event::Open(ty));
        let tx = Tx {
            state: Arc::clone(&self.state),
            responses: Arc::clone(&self.responses),
            rollback_error: self.rollback_error,
            close_error: self.close_error,
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
    rollback_error: bool,
    close_error: bool,
}

fn test_db(responses: Vec<Response>) -> (crate::session::Database<TestSchema>, Arc<Mutex<State>>) {
    test_db_with_failures(responses, false, false)
}

fn test_db_with_rollback(
    responses: Vec<Response>,
    rollback_error: bool,
) -> (crate::session::Database<TestSchema>, Arc<Mutex<State>>) {
    test_db_with_failures(responses, rollback_error, false)
}

fn test_db_with_failures(
    responses: Vec<Response>,
    rollback_error: bool,
    close_error: bool,
) -> (crate::session::Database<TestSchema>, Arc<Mutex<State>>) {
    let state = Arc::new(Mutex::new(State::default()));
    let backend = Backend {
        state: Arc::clone(&state),
        responses: Arc::new(Mutex::new(responses.into())),
        rollback_error,
        close_error,
    };
    (
        crate::session::Database::<TestSchema>::from_test_parts(
            OrmDatabase::with_backend(Box::new(backend), "e01"),
            minimal_fixture(),
        ),
        state,
    )
}

fn fetch(iid: &str, name: &str, value: &str) -> Response {
    Response::Result(QueryResult::Documents(vec![
        serde_json::json!({"_iid":iid,"_type":"record","attributes":{"name":name,"text":value}}),
    ]))
}

fn batch_fetch(rows: &[(u64, &str, &str, &str)]) -> Response {
    Response::Result(QueryResult::Documents(rows.iter().map(|(ordinal, iid, name, value)| {
        serde_json::json!({"ordinal":ordinal,"_iid":iid,"_type":"record","attributes":{"name":name,"text":value}})
    }).collect()))
}

fn assert_queries(events: &[Event], query_needles: &[&[&str]]) {
    assert_eq!(events.len(), query_needles.len() + 2);
    assert!(matches!(events[0], Event::Open(TxType::Write)));
    assert!(matches!(events[events.len() - 1], Event::Commit));
    for (idx, needles) in query_needles.iter().enumerate() {
        let Event::Query(q) = &events[idx + 1] else {
            panic!("event {} is not query", idx + 1)
        };
        for needle in needles.iter() {
            assert!(q.contains(needle), "query {idx}: {q} missing {needle}");
        }
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

#[test]
fn routed_database_manager_future_is_send() {
    let (db, state) = test_db(Vec::new());
    let manager = db.entities::<Record>();
    assert_send(manager.insert(RecordCreate {
        name: "send".into(),
        value: "proof".into(),
    }));
    assert!(state.lock().unwrap().events.is_empty());
}

#[tokio::test]
async fn public_preflight_failures_are_zero_io() {
    let (db, state) = test_db(Vec::new());
    assert_model_error(
        db.entities::<Record>()
            .get_by_iid("bad-iid")
            .await
            .unwrap_err(),
        crate::error::ModelValidationPhase::Input,
        "invalid_iid",
        &["iid"],
    );
    assert_eq!(state.lock().unwrap().events.as_slice(), &[] as &[Event]);
    assert_model_error(
        db.entities::<Record>()
            .update(
                "bad-iid",
                RecordCreate {
                    name: "n".into(),
                    value: "v".into(),
                },
            )
            .await
            .unwrap_err(),
        crate::error::ModelValidationPhase::Input,
        "invalid_iid",
        &["iid"],
    );
    assert_eq!(state.lock().unwrap().events.as_slice(), &[] as &[Event]);
    assert_model_error(
        db.entities::<Record>().delete("bad-iid").await.unwrap_err(),
        crate::error::ModelValidationPhase::Input,
        "invalid_iid",
        &["iid"],
    );
    assert_eq!(state.lock().unwrap().events.as_slice(), &[] as &[Event]);

    let (db, state) = test_db(Vec::new());
    let err = db
        .entities::<Record>()
        .insert_many(vec![
            RecordCreate {
                name: "ok".into(),
                value: "v".into(),
            },
            RecordCreate {
                name: "reject-input".into(),
                value: "v".into(),
            },
        ])
        .await
        .unwrap_err();
    assert!(matches!(&err, crate::Error::Classified { .. }));
    assert_eq!(err.category(), crate::ErrorCategory::ModelValidation);
    assert_eq!(
        err.model_validation_phase(),
        Some(crate::error::ModelValidationPhase::Input)
    );
    assert_eq!(err.code(), Some("rejected_create"));
    assert_eq!(
        err.path(),
        Some(&["rows".into(), "[1]".into(), "name".into()][..])
    );
    assert_eq!(state.lock().unwrap().events.as_slice(), &[] as &[Event]);
}

#[tokio::test]
async fn public_canonical_database_and_batch_paths_select_canonical_queries() {
    let responses = vec![
        fetch("0x1", "n1", "v"),
        batch_fetch(&[(0, "0x2", "n2", "v2")]),
        Response::Result(QueryResult::Rows(vec![serde_json::json!({"$count": 2})])),
    ];
    let (db, state) = test_db(responses);
    db.entities::<Record>()
        .insert(RecordCreate {
            name: "n1".into(),
            value: "v".into(),
        })
        .await
        .unwrap();
    db.entities::<Record>()
        .insert_many(vec![RecordCreate {
            name: "n2".into(),
            value: "v2".into(),
        }])
        .await
        .unwrap();
    db.entities::<Record>().count().await.unwrap();
    assert_eq!(
        state.lock().unwrap().query_modes,
        vec!["canonical", "canonical", "canonical"]
    );
}

#[tokio::test]
async fn public_canonical_read_error_closes_once_and_preserves_primary() {
    let (db, state) = test_db(vec![Response::Error("canonical read failed".into())]);
    let error = db.entities::<Record>().all().await.unwrap_err();
    assert!(matches!(error, crate::Error::QueryExecution { .. }));
    assert!(error.to_string().contains("canonical read failed"));
    let guard = state.lock().unwrap();
    let [Event::Open(TxType::Read), Event::Query(query), Event::Close] = guard.events.as_slice()
    else {
        panic!("unexpected events")
    };
    assert!(query.contains("isa! record") && query.contains("fetch"));
    assert_eq!(guard.query_modes, vec!["canonical"]);
}

#[tokio::test]
async fn public_exact_managers_report_schema_not_bound_before_io() {
    let state = Arc::new(Mutex::new(State::default()));
    let backend = Backend {
        state: Arc::clone(&state),
        responses: Arc::new(Mutex::new(VecDeque::new())),
        rollback_error: false,
        close_error: false,
    };
    let db = crate::session::Database::<TestSchema>::from_test_unbound_parts(
        OrmDatabase::with_backend(Box::new(backend), "e01"),
    );
    let check = |error: crate::Error| {
        let crate::Error::ModelValidation {
            phase,
            code,
            path,
            message,
            source,
        } = error
        else {
            panic!("wrong error: {error:?}")
        };
        assert_eq!(phase, crate::error::ModelValidationPhase::Input);
        assert_eq!(code, "schema_not_bound");
        assert!(path.is_empty());
        assert_eq!(message, "database is not schema-bound");
        assert!(source.is_none());
        assert_eq!(state.lock().unwrap().events.as_slice(), &[] as &[Event]);
    };
    check(
        db.entities::<Record>()
            .insert(RecordCreate {
                name: "n1".into(),
                value: "v".into(),
            })
            .await
            .unwrap_err(),
    );
    check(
        db.entities::<Record>()
            .put(RecordCreate {
                name: "n1".into(),
                value: "v".into(),
            })
            .await
            .unwrap_err(),
    );
    check(
        db.entities::<Record>()
            .update(
                "0x1",
                RecordCreate {
                    name: "n1".into(),
                    value: "v".into(),
                },
            )
            .await
            .unwrap_err(),
    );
    check(db.entities::<Record>().delete("0x1").await.unwrap_err());
    check(db.entities::<Record>().count().await.unwrap_err());
    check(db.entities::<Record>().get_by_iid("0x1").await.unwrap_err());
    check(db.entities::<Record>().all().await.unwrap_err());
}

#[tokio::test]
async fn public_get_by_iid_zero_uses_exact_read() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Documents(Vec::new()))]);
    assert!(
        db.entities::<Record>()
            .get_by_iid("0x1")
            .await
            .unwrap()
            .is_none()
    );
    let guard = state.lock().unwrap();
    let [Event::Open(TxType::Read), Event::Query(query), Event::Close] = guard.events.as_slice()
    else {
        panic!("unexpected events")
    };
    assert!(query.contains("isa! record") && query.contains("iid 0x1") && query.contains("fetch"));
    assert!(!query.contains("isa record"));
}

#[tokio::test]
async fn public_get_by_iid_one_materializes_exact_record() {
    let (db, state) = test_db(vec![fetch("0x1", "n1", "child-value")]);
    let Some(result) = db.entities::<Record>().get_by_iid("0x1").await.unwrap() else {
        panic!("expected record")
    };
    assert_eq!(result.iid, "0x1");
    assert_eq!(result.name, "n1");
    assert_eq!(result.value, "child-value");
    let guard = state.lock().unwrap();
    let [Event::Open(TxType::Read), Event::Query(query), Event::Close] = guard.events.as_slice()
    else {
        panic!("unexpected events")
    };
    assert!(query.contains("isa! record") && query.contains("iid 0x1") && query.contains("fetch"));
    assert!(!query.contains("isa record"));
}

#[tokio::test]
async fn public_count_uses_one_exact_reduction() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Rows(vec![
        serde_json::json!({"$count": 2}),
    ]))]);
    assert_eq!(db.entities::<Record>().count().await.unwrap(), 2_u64);
    let guard = state.lock().unwrap();
    let [Event::Open(TxType::Read), Event::Query(query), Event::Close] = guard.events.as_slice()
    else {
        panic!("unexpected events")
    };
    assert!(
        query.contains("isa! record")
            && query.contains("select $root0; distinct; reduce $count1 = count;")
    );
    assert!(!query.contains("isa record") && !query.contains("fetch"));
}

#[tokio::test]
async fn public_all_preserves_provider_order_with_exact_read() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Documents(vec![
        serde_json::json!({"_iid":"0x2","_type":"record","attributes":{"name":["n2"],"text":["second"]}}),
        serde_json::json!({"_iid":"0x1","_type":"record","attributes":{"name":["n1"],"text":["first"]}}),
    ]))]);
    let rows = db.entities::<Record>().all().await.unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| (r.iid.as_str(), r.name.as_str(), r.value.as_str()))
            .collect::<Vec<_>>(),
        vec![("0x2", "n2", "second"), ("0x1", "n1", "first")]
    );
    let guard = state.lock().unwrap();
    let [Event::Open(TxType::Read), Event::Query(query), Event::Close] = guard.events.as_slice()
    else {
        panic!("unexpected events")
    };
    assert!(query.contains("isa! record") && query.contains("fetch"));
    assert!(!query.contains("isa record"));
}

#[tokio::test]
async fn public_all_materializer_failure_returns_no_vector() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Documents(vec![
        serde_json::json!({"_iid":"0x1","_type":"record","attributes":{"name":["n1"],"text":["first"]}}),
        serde_json::json!({"_iid":"0x2","_type":"record","attributes":{"name":["n2"],"text":["reject-materialize"]}}),
    ]))]);
    let error = db.entities::<Record>().all().await.unwrap_err();
    let crate::Error::ModelValidation {
        phase,
        code,
        path,
        message,
        ..
    } = &error
    else {
        panic!("wrong error: {error:?}")
    };
    assert_eq!(*phase, crate::error::ModelValidationPhase::Hydration);
    assert_eq!(code, "rejected_materialization");
    assert_eq!(path, &["text"]);
    assert_eq!(message, "text: rejected_materialization");
    let source = std::error::Error::source(&error).expect("materializer source");
    assert!(source.is::<ValidationError>());
    assert_eq!(source.to_string(), "text: rejected_materialization");
    let guard = state.lock().unwrap();
    let [Event::Open(TxType::Read), Event::Query(query), Event::Close] = guard.events.as_slice()
    else {
        panic!("unexpected events")
    };
    assert!(query.contains("isa! record") && query.contains("fetch"));
    assert!(!query.contains("isa record"));
}

fn assert_insert_failure_events(events: &[Event]) {
    let [
        Event::Open(TxType::Write),
        Event::Query(query),
        Event::Rollback,
    ] = events
    else {
        panic!("unexpected event stream: {events:?}")
    };
    for needle in ["insert", "isa record", "n1", "\"v\""] {
        assert!(query.contains(needle), "query missing {needle}: {query}");
    }
}

fn assert_post_write_events(events: &[Event], insert_value_needle: &str) {
    // One query: the insert fetches its own document.
    let [
        Event::Open(TxType::Write),
        Event::Query(insert),
        Event::Rollback,
    ] = events
    else {
        panic!("unexpected event stream: {events:?}")
    };
    for needle in ["insert", "isa record", "n1", insert_value_needle, "fetch"] {
        assert!(
            insert.contains(needle),
            "insert query missing {needle}: {insert}"
        );
    }
}

fn assert_precommit_insert_correlation_error(error: &crate::Error) {
    assert_eq!(
        error.category(),
        crate::error::ErrorCategory::ModelValidation
    );
    assert_eq!(error.code(), Some("query_remote_v3_document_value"));
    assert_eq!(
        error.model_validation_phase(),
        Some(crate::error::ModelValidationPhase::Input)
    );
    assert_eq!(error.path(), Some([].as_slice()));
    assert_eq!(error.diagnostic_path(), Some([].as_slice()));
    assert_eq!(
        error.message(),
        "Typed query evidence does not match the validated request invocation"
    );
    let source = std::error::Error::source(error)
        .unwrap()
        .downcast_ref::<type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic>()
        .unwrap();
    assert_eq!(source.category().as_str(), "integrity");
}

#[tokio::test]
async fn public_insert_missing_post_write_row_rolls_back() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Documents(Vec::new()))]);
    let error = db
        .entities::<Record>()
        .insert(RecordCreate {
            name: "n1".into(),
            value: "v".into(),
        })
        .await
        .unwrap_err();
    assert_precommit_insert_correlation_error(&error);
    assert_post_write_events(&state.lock().unwrap().events, "\"v\"");
}

#[tokio::test]
async fn public_insert_wrong_concrete_provider_row_rolls_back() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Documents(vec![
        serde_json::json!({"_iid":"0x1","_type":"outside","attributes":{"name":"n1","text":"v"}}),
    ]))]);
    let error = db
        .entities::<Record>()
        .insert(RecordCreate {
            name: "n1".into(),
            value: "v".into(),
        })
        .await
        .unwrap_err();
    // The exact-type plan rejects attributes of an undeclared result type before hydration.
    assert_eq!(error.code(), Some("query_remote_v3_answer_kind"));
    assert_post_write_events(&state.lock().unwrap().events, "\"v\"");
}

#[tokio::test]
async fn public_insert_materializer_rejection_rolls_back() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Documents(vec![
        serde_json::json!({"_iid":"0x1","_type":"record","attributes":{"name":"n1","text":"reject-materialize"}}),
    ]))]);
    let error = db
        .entities::<Record>()
        .insert(RecordCreate {
            name: "n1".into(),
            value: "reject-materialize".into(),
        })
        .await
        .unwrap_err();
    let crate::Error::ModelValidation {
        phase,
        code,
        path,
        message,
        ..
    } = &error
    else {
        panic!("wrong error: {error:?}")
    };
    assert_eq!(*phase, crate::error::ModelValidationPhase::Hydration);
    assert_eq!(code, "rejected_materialization");
    assert_eq!(path, &["text"]);
    assert_eq!(message, "text: rejected_materialization");
    let source = std::error::Error::source(&error).expect("materializer source");
    assert!(source.is::<ValidationError>());
    assert_eq!(source.to_string(), "text: rejected_materialization");
    assert_post_write_events(&state.lock().unwrap().events, "\"reject-materialize\"");
}

#[tokio::test]
async fn public_insert_many_provider_failure_returns_no_partial_vector_and_rolls_back() {
    let (db, state) = test_db(vec![Response::Error("batch insert failed".into())]);
    let error = db
        .entities::<Record>()
        .insert_many(vec![
            RecordCreate {
                name: "n1".into(),
                value: "a".into(),
            },
            RecordCreate {
                name: "n2".into(),
                value: "b".into(),
            },
        ])
        .await
        .unwrap_err();
    assert!(matches!(&error, crate::Error::Classified { .. }));
    assert_eq!(error.category(), crate::ErrorCategory::QueryExecution);
    assert_eq!(error.code(), Some("provider_operation_failed"));
    assert!(!error.to_string().contains("batch insert failed"));
    assert!(std::error::Error::source(&error).is_some());
    let events = state.lock().unwrap();
    let [
        Event::Open(TxType::Write),
        Event::Query(query),
        Event::Rollback,
    ] = events.events.as_slice()
    else {
        panic!("unexpected events: {:?}", events.events)
    };
    for needle in ["insert", "isa record", "n1", "n2", "fetch"] {
        assert!(query.contains(needle), "{query}");
    }
}

#[tokio::test]
async fn public_insert_provider_error_survives_rollback_failure() {
    let (db, state) = test_db_with_rollback(vec![Response::Error("provider failed".into())], true);
    let error = db
        .entities::<Record>()
        .insert(RecordCreate {
            name: "n1".into(),
            value: "v".into(),
        })
        .await
        .unwrap_err();
    let crate::Error::QueryExecution { message, .. } = &error else {
        panic!("expected query error")
    };
    assert_eq!(message, "Query execution error: provider failed");
    let source = std::error::Error::source(&error).expect("query source");
    assert!(source.is::<type_bridge_orm::OrmError>());
    assert_eq!(source.to_string(), "Query execution error: provider failed");
    assert!(!error.to_string().contains("recording rollback failed"));
    let guard = state.lock().unwrap();
    assert!(matches!(guard.events.last(), Some(Event::Close)));
    assert_insert_failure_events(&guard.events[..guard.events.len() - 1]);
}

macro_rules! stand_in {
    ($name:ident, $identity:literal) => {
        #[derive(Debug)]
        struct $name;
        impl sealed::Sealed for $name {}
        impl Model for $name {
            type Schema = TestSchema;
            const TYPE_ID_JSON: &'static str = $identity;
        }
        impl ThingModel for $name {
            fn thing_kind() -> __codegen::ThingKind {
                __codegen::ThingKind::Entity
            }
        }
        impl EntityModel for $name {}
        impl CompleteModel for $name {
            type Create = RecordCreate;
            fn iid(&self) -> &str {
                unreachable!()
            }
        }
        impl MaterializeModel for $name {
            fn materialize(
                _: &HydratedRow,
                _: &HydrationCapability,
            ) -> Result<Self, ValidationError> {
                unreachable!()
            }
        }
    };
}

stand_in!(
    NoncanonicalIdentity,
    r#"{"label":"record","kind":"entity"}"#
);
stand_in!(RelationIdentity, r#"{"kind":"relation","label":"record"}"#);
stand_in!(
    UnprojectedIdentity,
    r#"{"kind":"entity","label":"unprojected"}"#
);

struct Base;
impl sealed::Sealed for Base {}
impl Model for Base {
    type Schema = TestSchema;
    const TYPE_ID_JSON: &'static str = r#"{"kind":"entity","label":"base"}"#;
}
impl ThingModel for Base {
    fn thing_kind() -> __codegen::ThingKind {
        __codegen::ThingKind::Entity
    }
}
impl EntityModel for Base {}
impl AbstractModel for Base {}
#[derive(Debug)]
enum BaseFamily {
    Record(Record),
}
impl sealed::Sealed for BaseFamily {}
impl ModelFamily for BaseFamily {
    type Root = Base;
    type Schema = TestSchema;
    fn iid(&self) -> &str {
        match self {
            Self::Record(value) => value.iid(),
        }
    }
}
impl SubtypeRootModel for Base {
    type Subtypes = BaseFamily;
    fn __tb_dispatch_subtype(
        row: &HydratedRow,
        cap: &HydrationCapability,
    ) -> Result<Self::Subtypes, ValidationError> {
        if row.type_id_json() == Record::TYPE_ID_JSON {
            Ok(BaseFamily::Record(Record::materialize(row, cap)?))
        } else {
            Err(ValidationError::new("type_id", "wrong_concrete_model_type"))
        }
    }
}

static LEAF_MATERIALIZE_CALLS: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug)]
struct Leaf;
impl sealed::Sealed for Leaf {}
impl Model for Leaf {
    type Schema = TestSchema;
    const TYPE_ID_JSON: &'static str = r#"{"kind":"entity","label":"record"}"#;
}
impl ThingModel for Leaf {
    fn thing_kind() -> __codegen::ThingKind {
        __codegen::ThingKind::Entity
    }
}
impl EntityModel for Leaf {}
impl CompleteModel for Leaf {
    type Create = RecordCreate;
    fn iid(&self) -> &str {
        unreachable!()
    }
}
impl MaterializeModel for Leaf {
    fn materialize(row: &HydratedRow, cap: &HydrationCapability) -> Result<Self, ValidationError> {
        LEAF_MATERIALIZE_CALLS.fetch_add(1, Ordering::SeqCst);
        Record::materialize(row, cap).map(|_| Leaf)
    }
}
impl SubtypeRootModel for Leaf {
    type Subtypes = Leaf;
    fn __tb_dispatch_subtype(
        row: &HydratedRow,
        cap: &HydrationCapability,
    ) -> Result<Self::Subtypes, ValidationError> {
        if row.type_id_json() == Self::TYPE_ID_JSON {
            Self::materialize(row, cap)
        } else {
            Err(ValidationError::new("type_id", "wrong_concrete_model_type"))
        }
    }
}

#[tokio::test]
async fn public_leaf_subtypes_reject_wrong_identity_before_materializer() {
    LEAF_MATERIALIZE_CALLS.store(0, Ordering::SeqCst);
    let (db, state) = test_db(vec![
        Response::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid":"0x1","_type":"outside"}),
        ])),
        Response::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid":"0x1","_type":"outside","attributes":{"name":["n1"],"text":["v"]}}),
        ])),
    ]);
    let error = db.entities::<Leaf>().subtypes().all().await.unwrap_err();
    let crate::Error::ModelValidation {
        phase,
        code,
        path,
        message,
        ..
    } = &error
    else {
        panic!("wrong error: {error:?}")
    };
    assert_eq!(*phase, crate::error::ModelValidationPhase::Hydration);
    assert_eq!(code, "wrong_concrete_model_type");
    assert_eq!(path, &["type_id"]);
    assert_eq!(message, "type_id: wrong_concrete_model_type");
    let source = std::error::Error::source(&error).unwrap();
    assert!(source.is::<ValidationError>());
    assert_eq!(source.to_string(), "type_id: wrong_concrete_model_type");
    assert_eq!(LEAF_MATERIALIZE_CALLS.load(Ordering::SeqCst), 0);
    let guard = state.lock().unwrap();
    let [
        Event::Open(TxType::Read),
        Event::Query(root),
        Event::Query(child),
        Event::Close,
    ] = guard.events.as_slice()
    else {
        panic!("unexpected events")
    };
    for needle in ["$root0 isa record", "select $root0;", "distinct;"] {
        assert!(root.contains(needle));
    }
    assert!(!root.contains("attributes") && !root.contains("isa! record"));
    assert!(child.contains("isa! outside") && child.contains("iid 0x1") && child.contains("fetch"));
}

#[tokio::test]
async fn public_subtypes_all_rehydrates_concrete_children_in_discovery_order() {
    let (db, state) = test_db(vec![
        Response::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid":"0x2","_type":"record"}),
            serde_json::json!({"_iid":"0x1","_type":"record"}),
        ])),
        fetch("0x2", "n2", "second"),
        fetch("0x1", "n1", "first"),
    ]);
    let rows = db.entities::<Base>().subtypes().all().await.unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| match row {
                BaseFamily::Record(value) => (
                    value.iid.as_str(),
                    value.name.as_str(),
                    value.value.as_str()
                ),
            })
            .collect::<Vec<_>>(),
        vec![("0x2", "n2", "second"), ("0x1", "n1", "first")]
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
    for needle in ["$root0 isa base", "select $root0;", "distinct;"] {
        assert!(discovery.contains(needle));
    }
    assert!(!discovery.contains("attributes") && !discovery.contains("isa! base"));
    for (query, iid) in [(child2, "0x2"), (child1, "0x1")] {
        assert!(query.contains("isa! record") && query.contains(iid) && query.contains("fetch"));
    }
}

#[tokio::test]
async fn public_subtypes_get_by_iid_zero_closes_one_read_context() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Documents(Vec::new()))]);
    assert!(
        db.entities::<Base>()
            .subtypes()
            .get_by_iid("0x1")
            .await
            .unwrap()
            .is_none()
    );
    let guard = state.lock().unwrap();
    let [Event::Open(TxType::Read), Event::Query(query), Event::Close] = guard.events.as_slice()
    else {
        panic!("unexpected events")
    };
    assert!(
        query.contains("$root0 isa base")
            && query.contains("select $root0;")
            && query.contains("distinct;")
            && query.contains("$root0")
            && query.contains("iid 0x1")
    );
    assert!(!query.contains("attributes"));
}

#[tokio::test]
async fn public_subtypes_get_by_iid_one_rehydrates_concrete_child() {
    let (db, state) = test_db(vec![
        Response::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid":"0x1","_type":"record"}),
        ])),
        fetch("0x1", "n1", "child"),
    ]);
    let Some(BaseFamily::Record(row)) = db
        .entities::<Base>()
        .subtypes()
        .get_by_iid("0x1")
        .await
        .unwrap()
    else {
        panic!("expected child")
    };
    assert_eq!(
        (row.iid.as_str(), row.name.as_str(), row.value.as_str()),
        ("0x1", "n1", "child")
    );
    let guard = state.lock().unwrap();
    let [
        Event::Open(TxType::Read),
        Event::Query(root),
        Event::Query(child),
        Event::Close,
    ] = guard.events.as_slice()
    else {
        panic!("unexpected events")
    };
    assert!(
        root.contains("$root0 isa base")
            && root.contains("select $root0;")
            && root.contains("distinct;")
            && root.contains("$root0")
            && root.contains("iid 0x1")
    );
    assert!(!root.contains("attributes") && !root.contains("isa! base"));
    assert!(child.contains("isa! record") && child.contains("iid 0x1") && child.contains("fetch"));
}

#[tokio::test]
async fn public_subtypes_get_by_iid_duplicate_discovery_closes_without_child_fetch() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Documents(vec![
        serde_json::json!({"_iid":"0x1","_type":"record"}),
        serde_json::json!({"_iid":"0x1","_type":"record"}),
    ]))]);
    let error = db
        .entities::<Base>()
        .subtypes()
        .get_by_iid("0x1")
        .await
        .unwrap_err();
    let crate::Error::ModelValidation {
        phase,
        code,
        path,
        message,
        ..
    } = &error
    else {
        panic!("wrong error: {error:?}")
    };
    assert_eq!(*phase, crate::error::ModelValidationPhase::Hydration);
    assert_eq!(code, "invalid_provider_evidence");
    assert!(path.is_empty());
    assert_eq!(
        message,
        "Hydration error for type 'base': Deletion snapshot has invalid exact identity evidence"
    );
    let source = std::error::Error::source(&error).unwrap();
    assert!(source.is::<type_bridge_orm::OrmError>());
    assert_eq!(source.to_string(), message.as_str());
    let guard = state.lock().unwrap();
    let [Event::Open(TxType::Read), Event::Query(query), Event::Close] = guard.events.as_slice()
    else {
        panic!("unexpected events")
    };
    assert!(query.contains("$root0 isa base") && query.contains("iid 0x1"));
}

#[tokio::test]
async fn public_subtypes_count_uses_one_inclusive_reduction() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Rows(vec![
        serde_json::json!({"$count":2}),
    ]))]);
    assert_eq!(
        db.entities::<Base>().subtypes().count().await.unwrap(),
        2_u64
    );
    let guard = state.lock().unwrap();
    let [Event::Open(TxType::Read), Event::Query(query), Event::Close] = guard.events.as_slice()
    else {
        panic!("unexpected events")
    };
    assert!(query.contains("$root0 isa base") && query.contains("reduce $count1 = count;"));
    assert!(
        !query.contains("isa! base")
            && !query.contains("fetch")
            && !query.contains("\"_iid\"")
            && !query.contains("\"_type\"")
    );
}

#[tokio::test]
async fn public_subtypes_invalid_iid_is_zero_io() {
    let (db, state) = test_db(Vec::new());
    let error = db
        .entities::<Base>()
        .subtypes()
        .get_by_iid("bad-iid")
        .await
        .unwrap_err();
    assert_model_error(
        error,
        crate::error::ModelValidationPhase::Input,
        "invalid_iid",
        &["iid"],
    );
    assert_eq!(state.lock().unwrap().events.as_slice(), &[] as &[Event]);
}

fn assert_subtype_discovery_query(query: &str) {
    for needle in ["$root0 isa base", "select $root0;", "distinct;"] {
        assert!(query.contains(needle));
    }
    assert!(!query.contains("attributes") && !query.contains("$e isa! base"));
}

#[tokio::test]
async fn public_subtypes_all_unknown_discovered_type_fails_closed() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Documents(vec![
        serde_json::json!({"_iid":"0x1","_type":"unknown"}),
    ]))]);
    let error = db.entities::<Base>().subtypes().all().await.unwrap_err();
    let crate::Error::ModelValidation {
        phase,
        code,
        path,
        message,
        ..
    } = &error
    else {
        panic!("wrong error: {error:?}")
    };
    assert_eq!(*phase, crate::error::ModelValidationPhase::Hydration);
    assert_eq!(code, "model_not_projected");
    assert_eq!(path, &["type"]);
    assert_eq!(
        message,
        "the selected entity is absent from the installed Rust projection"
    );
    assert!(std::error::Error::source(&error).is_none());
    let guard = state.lock().unwrap();
    let [Event::Open(TxType::Read), Event::Query(root), Event::Close] = guard.events.as_slice()
    else {
        panic!("unexpected events")
    };
    assert_subtype_discovery_query(root);
}

#[tokio::test]
async fn public_subtypes_all_abstract_discovered_type_fails_closed() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Documents(vec![
        serde_json::json!({"_iid":"0x1","_type":"base"}),
    ]))]);
    let error = db.entities::<Base>().subtypes().all().await.unwrap_err();
    let crate::Error::ModelValidation {
        phase,
        code,
        path,
        message,
        ..
    } = &error
    else {
        panic!("wrong error: {error:?}")
    };
    assert_eq!(*phase, crate::error::ModelValidationPhase::Hydration);
    assert_eq!(code, "model_not_constructible");
    assert_eq!(path, &["type"]);
    assert_eq!(message, "the selected entity is not constructible");
    assert!(std::error::Error::source(&error).is_none());
    let guard = state.lock().unwrap();
    let [Event::Open(TxType::Read), Event::Query(root), Event::Close] = guard.events.as_slice()
    else {
        panic!("unexpected events")
    };
    assert_subtype_discovery_query(root);
}

#[tokio::test]
async fn public_subtypes_all_out_of_closure_type_fails_generated_dispatch() {
    let (db, state) = test_db(vec![
        Response::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid":"0x1","_type":"outside"}),
        ])),
        Response::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid":"0x1","_type":"outside","attributes":{"name":["n1"],"text":["v"]}}),
        ])),
    ]);
    let error = db.entities::<Base>().subtypes().all().await.unwrap_err();
    let crate::Error::ModelValidation {
        phase,
        code,
        path,
        message,
        ..
    } = &error
    else {
        panic!("wrong error: {error:?}")
    };
    assert_eq!(*phase, crate::error::ModelValidationPhase::Hydration);
    assert_eq!(code, "wrong_concrete_model_type");
    assert_eq!(path, &["type_id"]);
    assert_eq!(message, "type_id: wrong_concrete_model_type");
    let source = std::error::Error::source(&error).unwrap();
    assert!(source.is::<ValidationError>());
    assert_eq!(source.to_string(), "type_id: wrong_concrete_model_type");
    let guard = state.lock().unwrap();
    let [
        Event::Open(TxType::Read),
        Event::Query(root),
        Event::Query(child),
        Event::Close,
    ] = guard.events.as_slice()
    else {
        panic!("unexpected events")
    };
    assert_subtype_discovery_query(root);
    assert!(child.contains("isa! outside") && child.contains("iid 0x1") && child.contains("fetch"));
}

#[tokio::test]
async fn public_subtypes_all_missing_concrete_row_fails_closed() {
    let (db, state) = test_db(vec![
        Response::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid":"0x1","_type":"record"}),
        ])),
        Response::Result(QueryResult::Documents(Vec::new())),
    ]);
    let error = db.entities::<Base>().subtypes().all().await.unwrap_err();
    let crate::Error::ModelValidation {
        phase,
        code,
        path,
        message,
        ..
    } = &error
    else {
        panic!("wrong error: {error:?}")
    };
    assert_eq!(*phase, crate::error::ModelValidationPhase::Hydration);
    assert_eq!(code, "missing_concrete_row");
    assert_eq!(path, &["iid"]);
    assert_eq!(message, "discovered entity row is missing");
    assert!(std::error::Error::source(&error).is_none());
    let guard = state.lock().unwrap();
    let [
        Event::Open(TxType::Read),
        Event::Query(root),
        Event::Query(child),
        Event::Close,
    ] = guard.events.as_slice()
    else {
        panic!("unexpected events")
    };
    assert_subtype_discovery_query(root);
    assert!(child.contains("isa! record") && child.contains("iid 0x1") && child.contains("fetch"));
}

#[tokio::test]
async fn public_subtypes_all_conflicting_concrete_rows_fails_closed() {
    let row = serde_json::json!({"_iid":"0x1","_type":"record","attributes":{"name":["n1"],"text":["v"]}});
    let mut conflicting = row.clone();
    conflicting["attributes"]["text"] = serde_json::json!(["different"]);
    let (db, state) = test_db(vec![
        Response::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid":"0x1","_type":"record"}),
        ])),
        Response::Result(QueryResult::Documents(vec![row, conflicting])),
    ]);
    let error = db.entities::<Base>().subtypes().all().await.unwrap_err();
    assert_eq!(error.category(), crate::ErrorCategory::ModelValidation);
    assert_eq!(error.code(), Some("query_plan_hydration_conflict"));
    assert_eq!(error.path(), Some(["result".to_owned()].as_slice()));
    let guard = state.lock().unwrap();
    let [
        Event::Open(TxType::Read),
        Event::Query(root),
        Event::Query(child),
        Event::Close,
    ] = guard.events.as_slice()
    else {
        panic!("unexpected events")
    };
    assert_subtype_discovery_query(root);
    assert!(child.contains("isa! record") && child.contains("iid 0x1") && child.contains("fetch"));
}

#[tokio::test]
async fn public_subtypes_all_wrong_returned_concrete_type_fails_closed() {
    let (db, state) = test_db(vec![
        Response::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid":"0x1","_type":"record"}),
        ])),
        Response::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid":"0x1","_type":"outside","attributes":{"name":["n1"],"text":["v"]}}),
        ])),
    ]);
    let error = db.entities::<Base>().subtypes().all().await.unwrap_err();
    assert_eq!(error.category(), crate::ErrorCategory::ModelValidation);
    assert_eq!(error.sdk_category(), Some("integrity"));
    assert_eq!(error.code(), Some("query_remote_v3_answer_kind"));
    assert_eq!(error.path(), Some([].as_slice()));
    let source = std::error::Error::source(&error).unwrap();
    assert!(source.is::<type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic>());
    let guard = state.lock().unwrap();
    let [
        Event::Open(TxType::Read),
        Event::Query(root),
        Event::Query(child),
        Event::Close,
    ] = guard.events.as_slice()
    else {
        panic!("unexpected events")
    };
    assert_subtype_discovery_query(root);
    assert!(child.contains("isa! record") && child.contains("iid 0x1") && child.contains("fetch"));
}

#[tokio::test]
async fn public_subtypes_all_missing_required_child_field_fails_closed() {
    let (db, state) = test_db(vec![
        Response::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid":"0x1","_type":"record"}),
        ])),
        Response::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid":"0x1","_type":"record","attributes":{"name":["n1"]}}),
        ])),
    ]);
    let error = db.entities::<Base>().subtypes().all().await.unwrap_err();
    assert_eq!(error.category(), crate::ErrorCategory::ModelValidation);
    assert_eq!(error.sdk_category(), Some("integrity"));
    assert_eq!(error.code(), Some("query_remote_v3_document_value"));
    assert_eq!(error.path(), Some([].as_slice()));
    let source = std::error::Error::source(&error).unwrap();
    assert!(source.is::<type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic>());
    let guard = state.lock().unwrap();
    let [
        Event::Open(TxType::Read),
        Event::Query(root),
        Event::Query(child),
        Event::Close,
    ] = guard.events.as_slice()
    else {
        panic!("unexpected events")
    };
    assert_subtype_discovery_query(root);
    assert!(child.contains("isa! record") && child.contains("iid 0x1") && child.contains("fetch"));
}

#[tokio::test]
async fn public_subtypes_all_generated_materializer_rejection_fails_closed() {
    let (db, state) = test_db(vec![
        Response::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid":"0x1","_type":"record"}),
        ])),
        fetch("0x1", "n1", "reject-materialize"),
    ]);
    let error = db.entities::<Base>().subtypes().all().await.unwrap_err();
    let crate::Error::ModelValidation {
        phase,
        code,
        path,
        message,
        ..
    } = &error
    else {
        panic!("wrong error: {error:?}")
    };
    assert_eq!(*phase, crate::error::ModelValidationPhase::Hydration);
    assert_eq!(code, "rejected_materialization");
    assert_eq!(path, &["text"]);
    assert_eq!(message, "text: rejected_materialization");
    let source = std::error::Error::source(&error).unwrap();
    assert!(source.is::<ValidationError>());
    assert_eq!(source.to_string(), "text: rejected_materialization");
    let guard = state.lock().unwrap();
    let [
        Event::Open(TxType::Read),
        Event::Query(root),
        Event::Query(child),
        Event::Close,
    ] = guard.events.as_slice()
    else {
        panic!("unexpected events")
    };
    assert_subtype_discovery_query(root);
    assert!(child.contains("isa! record") && child.contains("iid 0x1") && child.contains("fetch"));
}

#[tokio::test]
async fn public_subtypes_all_discovery_provider_failure_closes_and_preserves_primary() {
    let (db, state) = test_db(vec![Response::Error("discovery failed".into())]);
    let error = db.entities::<Base>().subtypes().all().await.unwrap_err();
    let crate::Error::QueryExecution { message, .. } = &error else {
        panic!("wrong error: {error:?}")
    };
    assert_eq!(message, "Query execution error: discovery failed");
    let source = std::error::Error::source(&error).unwrap();
    assert!(source.is::<type_bridge_orm::OrmError>());
    assert_eq!(source.to_string(), message.as_str());
    let guard = state.lock().unwrap();
    let [Event::Open(TxType::Read), Event::Query(root), Event::Close] = guard.events.as_slice()
    else {
        panic!("unexpected events")
    };
    assert_subtype_discovery_query(root);
}

#[tokio::test]
async fn public_subtypes_all_later_child_fetch_failure_returns_no_partial_vector() {
    let (db, state) = test_db(vec![
        Response::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid":"0x1","_type":"record"}),
            serde_json::json!({"_iid":"0x2","_type":"record"}),
        ])),
        fetch("0x1", "n1", "first"),
        Response::Error("second child fetch failed".into()),
    ]);
    let error = db.entities::<Base>().subtypes().all().await.unwrap_err();
    let crate::Error::QueryExecution { message, .. } = &error else {
        panic!("wrong error: {error:?}")
    };
    assert_eq!(message, "Query execution error: second child fetch failed");
    let source = std::error::Error::source(&error).unwrap();
    assert!(source.is::<type_bridge_orm::OrmError>());
    assert_eq!(source.to_string(), message.as_str());
    let guard = state.lock().unwrap();
    let [
        Event::Open(TxType::Read),
        Event::Query(root),
        Event::Query(child1),
        Event::Query(child2),
        Event::Close,
    ] = guard.events.as_slice()
    else {
        panic!("unexpected events")
    };
    assert_subtype_discovery_query(root);
    assert!(
        child1.contains("isa! record") && child1.contains("iid 0x1") && child1.contains("fetch")
    );
    assert!(
        child2.contains("isa! record") && child2.contains("iid 0x2") && child2.contains("fetch")
    );
}

fn assert_authority_error(error: crate::Error, code: &str) {
    let crate::Error::ModelValidation {
        phase,
        code: actual,
        path,
        ..
    } = error
    else {
        panic!("wrong error: {error:?}")
    };
    assert_eq!(phase, crate::error::ModelValidationPhase::Input);
    assert_eq!(actual, code);
    assert_eq!(path, vec!["type"]);
}

#[tokio::test]
async fn public_insert_noncanonical_model_identity_is_zero_io() {
    let (db, state) = test_db(Vec::new());
    let err = db
        .entities::<NoncanonicalIdentity>()
        .insert(RecordCreate {
            name: "n1".into(),
            value: "v".into(),
        })
        .await
        .unwrap_err();
    assert_authority_error(err, "invalid_model_identity");
    assert_eq!(state.lock().unwrap().events.as_slice(), &[] as &[Event]);
}

#[tokio::test]
async fn public_insert_relation_model_identity_is_zero_io() {
    let (db, state) = test_db(Vec::new());
    let err = db
        .entities::<RelationIdentity>()
        .insert(RecordCreate {
            name: "n1".into(),
            value: "v".into(),
        })
        .await
        .unwrap_err();
    assert_authority_error(err, "wrong_model_kind");
    assert_eq!(state.lock().unwrap().events.as_slice(), &[] as &[Event]);
}

#[tokio::test]
async fn public_insert_unprojected_model_identity_is_zero_io() {
    let (db, state) = test_db(Vec::new());
    let err = db
        .entities::<UnprojectedIdentity>()
        .insert(RecordCreate {
            name: "n1".into(),
            value: "v".into(),
        })
        .await
        .unwrap_err();
    assert_authority_error(err, "model_not_projected");
    assert_eq!(state.lock().unwrap().events.as_slice(), &[] as &[Event]);
}

#[tokio::test]
async fn public_insert_provider_query_error_rolls_back() {
    let (db, state) = test_db(vec![Response::Error("provider failed".into())]);
    let err = db
        .entities::<Record>()
        .insert(RecordCreate {
            name: "n1".into(),
            value: "v".into(),
        })
        .await
        .unwrap_err();
    let crate::Error::QueryExecution { message, .. } = &err else {
        panic!("wrong public error")
    };
    assert_eq!(message, "Query execution error: provider failed");
    assert_eq!(
        std::error::Error::source(&err).unwrap().to_string(),
        "Query execution error: provider failed"
    );
    assert_insert_failure_events(&state.lock().unwrap().events);
}

#[tokio::test]
async fn public_insert_zero_iid_documents_rolls_back() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Documents(Vec::new()))]);
    let err = db
        .entities::<Record>()
        .insert(RecordCreate {
            name: "n1".into(),
            value: "v".into(),
        })
        .await
        .unwrap_err();
    assert_precommit_insert_correlation_error(&err);
    assert_insert_failure_events(&state.lock().unwrap().events);
}

#[tokio::test]
async fn public_insert_multiple_iid_documents_rolls_back() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Documents(vec![
        serde_json::json!({"_iid":"0x1","_type":"record","attributes":{"name":"n1","text":"v"}}),
        serde_json::json!({"_iid":"0x2","_type":"record","attributes":{"name":"n1","text":"v"}}),
    ]))]);
    let err = db
        .entities::<Record>()
        .insert(RecordCreate {
            name: "n1".into(),
            value: "v".into(),
        })
        .await
        .unwrap_err();
    assert_precommit_insert_correlation_error(&err);
    assert_insert_failure_events(&state.lock().unwrap().events);
}

#[tokio::test]
async fn public_insert_noncanonical_iid_rolls_back_without_refetch() {
    let (db, state) = test_db(vec![Response::Result(QueryResult::Documents(vec![
        serde_json::json!({"_iid":"bad","_type":"record","attributes":{"name":"n1","text":"v"}}),
    ]))]);
    let err = db
        .entities::<Record>()
        .insert(RecordCreate {
            name: "n1".into(),
            value: "v".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("query_remote_v3_document_value"));
    assert_insert_failure_events(&state.lock().unwrap().events);
}

#[tokio::test]
async fn public_insert_uses_recording_projection_and_materializes_iid() {
    let state = Arc::new(Mutex::new(State::default()));
    let responses = VecDeque::from([Response::Result(QueryResult::Documents(vec![
        serde_json::json!({"_iid":"0x1","_type":"record","attributes":{"name":"n1","text":"hello"}}),
    ]))]);
    let backend = Backend {
        state: Arc::clone(&state),
        responses: Arc::new(Mutex::new(responses)),
        rollback_error: false,
        close_error: false,
    };
    let orm = OrmDatabase::with_backend(Box::new(backend), "e01");
    let db = crate::session::Database::<TestSchema>::from_test_parts(orm, minimal_fixture());
    let result = db
        .entities::<Record>()
        .insert(RecordCreate {
            name: "n1".into(),
            value: "hello".into(),
        })
        .await
        .unwrap();
    assert_eq!(result.iid, "0x1");
    assert_eq!(result.value, "hello");
    let s = state.lock().unwrap();
    assert_queries(
        &s.events,
        &[&["insert", "isa record", "n1", "hello", "fetch"]],
    );
    assert_eq!(result.name, "n1");
}

#[tokio::test]
async fn public_empty_batches_perform_no_provider_operations() {
    let state = Arc::new(Mutex::new(State::default()));
    let backend = Backend {
        state: Arc::clone(&state),
        responses: Arc::new(Mutex::new(VecDeque::new())),
        rollback_error: false,
        close_error: false,
    };
    let db = crate::session::Database::<TestSchema>::from_test_parts(
        OrmDatabase::with_backend(Box::new(backend), "e01"),
        minimal_fixture(),
    );
    assert!(
        db.entities::<Record>()
            .insert_many(Vec::new())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        db.entities::<Record>()
            .put_many(Vec::new())
            .await
            .unwrap()
            .is_empty()
    );
    let s = state.lock().unwrap();
    assert!(s.events.is_empty());
}

#[tokio::test]
async fn public_put_update_delete_success_lifecycles() {
    let (db, state) = test_db(vec![fetch("0x2", "n2", "new")]);
    let result = db
        .entities::<Record>()
        .put(RecordCreate {
            name: "n2".into(),
            value: "new".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        (&result.iid, &result.name, &result.value),
        (&"0x2".to_string(), &"n2".to_string(), &"new".to_string())
    );
    {
        let s = state.lock().unwrap();
        assert_queries(
            &s.events,
            // One put plan (key, other fields, its own fetch): no key lookup
            // and no read-back.
            &[&[
                "put $thing0 isa! record",
                "name",
                "n2",
                "update",
                "new",
                "fetch",
            ]],
        );
    }

    let (db, state) = test_db(vec![fetch("0x2", "n2", "upd")]);
    let result = db
        .entities::<Record>()
        .update(
            "0x2",
            RecordCreate {
                name: "n2".into(),
                value: "upd".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        (&result.iid, &result.name, &result.value),
        (&"0x2".to_string(), &"n2".to_string(), &"upd".to_string())
    );
    {
        let s = state.lock().unwrap();
        assert_queries(
            &s.events,
            // One update plan: match by IID, replace, fetch.
            &[&["isa! record", "iid 0x2", "upd", "fetch"]],
        );
    }

    let (db, state) = test_db(vec![Response::Result(QueryResult::Rows(Vec::new()))]);
    db.entities::<Record>().delete("0x2").await.unwrap();
    let events = &state.lock().unwrap().events;
    assert_queries(events, &[&["isa! record", "iid 0x2", "delete"]]);
}

#[tokio::test]
async fn public_insert_many_and_put_many_preserve_order() {
    let (db, state) = test_db(vec![batch_fetch(&[
        (1, "0x2", "n2", "b"),
        (0, "0x1", "n1", "a"),
    ])]);
    let out = db
        .entities::<Record>()
        .insert_many(vec![
            RecordCreate {
                name: "n1".into(),
                value: "a".into(),
            },
            RecordCreate {
                name: "n2".into(),
                value: "b".into(),
            },
        ])
        .await
        .unwrap();
    assert_eq!(
        out.iter()
            .map(|r| (r.iid.as_str(), r.name.as_str(), r.value.as_str()))
            .collect::<Vec<_>>(),
        vec![("0x1", "n1", "a"), ("0x2", "n2", "b")]
    );
    {
        let s = state.lock().unwrap();
        assert_queries(
            &s.events,
            &[&["insert", "isa record", "n1", "n2", "a", "b", "fetch"]],
        );
    }

    let (db, state) = test_db(vec![batch_fetch(&[
        (1, "0x2", "n2", "b"),
        (0, "0x1", "n1", "a"),
    ])]);
    let out = db
        .entities::<Record>()
        .put_many(vec![
            RecordCreate {
                name: "n1".into(),
                value: "a".into(),
            },
            RecordCreate {
                name: "n2".into(),
                value: "b".into(),
            },
        ])
        .await
        .unwrap();
    let s = state.lock().unwrap();
    assert_queries(
        &s.events,
        &[&["put", "record", "name", "n1", "n2", "a", "b", "fetch"]],
    );
    assert_eq!(
        out.iter()
            .map(|r| (r.iid.as_str(), r.name.as_str(), r.value.as_str()))
            .collect::<Vec<_>>(),
        vec![("0x1", "n1", "a"), ("0x2", "n2", "b")]
    );
}
#[tokio::test]
async fn public_subtypes_all_successful_read_reports_close_failure() {
    let (db, state) = test_db_with_failures(
        vec![
            Response::Result(QueryResult::Documents(vec![
                serde_json::json!({"_iid":"0x1","_type":"record"}),
            ])),
            fetch("0x1", "n1", "v"),
        ],
        false,
        true,
    );
    let error = db.entities::<Base>().subtypes().all().await.unwrap_err();
    let crate::Error::Transaction { message, .. } = &error else {
        panic!("wrong error: {error:?}")
    };
    assert_eq!(message, "Transaction error: close failed");
    let source = std::error::Error::source(&error).unwrap();
    assert!(source.is::<type_bridge_orm::OrmError>());
    assert_eq!(source.to_string(), message.as_str());
    let guard = state.lock().unwrap();
    let [
        Event::Open(TxType::Read),
        Event::Query(root),
        Event::Query(child),
        Event::Close,
    ] = guard.events.as_slice()
    else {
        panic!("unexpected events")
    };
    assert_subtype_discovery_query(root);
    assert!(child.contains("isa! record") && child.contains("iid 0x1") && child.contains("fetch"));
}

#[tokio::test]
async fn public_subtypes_all_provider_error_survives_close_failure() {
    let (db, state) = test_db_with_failures(
        vec![Response::Error("discovery failed".into())],
        false,
        true,
    );
    let error = db.entities::<Base>().subtypes().all().await.unwrap_err();
    let crate::Error::QueryExecution { message, .. } = &error else {
        panic!("wrong error: {error:?}")
    };
    assert_eq!(message, "Query execution error: discovery failed");
    let source = std::error::Error::source(&error).unwrap();
    assert!(source.is::<type_bridge_orm::OrmError>());
    assert_eq!(source.to_string(), message.as_str());
    assert!(!error.to_string().contains("close failed"));
    let guard = state.lock().unwrap();
    let [Event::Open(TxType::Read), Event::Query(root), Event::Close] = guard.events.as_slice()
    else {
        panic!("unexpected events")
    };
    assert_subtype_discovery_query(root);
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
    fn query_v2_bounded<'a>(
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
        let fail = self.rollback_error;
        Box::pin(async move {
            if fail {
                Err(OrmError::Transaction("rollback failed".into()))
            } else {
                Ok(())
            }
        })
    }
    fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.state.lock().unwrap().events.push(Event::Close);
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
                Response::Result(v) => Ok(native_batch::result(&source, v)),
                Response::Error(e) => Err(OrmError::QueryExecution(e)),
            }
        })
    }
}

#[tokio::test]
async fn public_subtypes_all_coalesces_identical_native_documents() {
    let row = serde_json::json!({"_iid":"0x1","_type":"record","attributes":{"name":["n1"],"text":["v"]}});
    let (db, _) = test_db(vec![
        Response::Result(QueryResult::Documents(vec![
            serde_json::json!({"_iid":"0x1","_type":"record"}),
        ])),
        Response::Result(QueryResult::Documents(vec![row.clone(), row])),
    ]);
    let rows = db.entities::<Base>().subtypes().all().await.unwrap();
    let [BaseFamily::Record(record)] = rows.as_slice() else {
        panic!("expected one coalesced model")
    };
    assert_eq!(
        (
            record.iid.as_str(),
            record.name.as_str(),
            record.value.as_str()
        ),
        ("0x1", "n1", "v")
    );
}

#[tokio::test]
async fn schema_less_generated_managers_keep_the_query_authority_fence() {
    let (db, state) = test_db(vec![fetch("0x1", "n1", "v")]);
    let rows = db.entities::<Record>().all().await.unwrap();
    assert_eq!(rows.len(), 1);
    let before = state.lock().unwrap().events.len();
    let mut session = db.query().unwrap();
    let record = session.exact::<Record>().unwrap();
    let error = session.query(record).unwrap().count().await.unwrap_err();
    assert_eq!(error.code(), Some("schema_authority_required"));
    assert_eq!(state.lock().unwrap().events.len(), before);
}

#[tokio::test]
async fn nonempty_ordered_write_is_refused_before_provider_io() {
    let state = Arc::new(Mutex::new(State::default()));
    let backend = Backend {
        state: Arc::clone(&state),
        responses: Arc::new(Mutex::new(VecDeque::new())),
        rollback_error: false,
        close_error: false,
    };
    let orm = OrmDatabase::with_backend(Box::new(backend), "ordered-refusal");
    let db = crate::session::Database::<TestSchema>::from_test_parts(
        orm,
        fixture_with_ordered_text(true),
    );
    let error = db
        .entities::<Record>()
        .insert(RecordCreate {
            name: "n1".into(),
            value: "nonempty".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(
        error.code(),
        Some("query_plan_ordered_collection_unsupported")
    );
    assert!(state.lock().unwrap().events.is_empty());
}
