#[path = "support/allocations.rs"]
mod allocations;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::projection::{
    BindingTarget, ProjectedModelForm, ProjectionConfig, ProjectionHandler,
};
use type_bridge_contract::schema::{DocumentId, OwnsFactId};
use type_bridge_contract::sdk_diagnostic::SdkDiagnosticCategory;
use type_bridge_contract::value::{CanonicalString, CanonicalValue};
use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, QueryResult, TransactionOps};
use type_bridge_orm::{
    ClassifiedCommitError, CommitFailureCertainty, Database, DatabaseConnectionAuthority,
    GivenRowsSpec, GivenValue, InstalledRuntimeProjection, OrmError, ProjectedAttributeValue,
    ProjectedCreate, ProjectedCrudExecutor, ProjectedReference, ProjectedThing, TxType,
};
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};

const SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  email: { value: string }
  nickname: { value: string }
entities:
  actor:
    abstract: true
  person:
    sub: actor
    owns:
      identifier: { key: true }
      email: { card: 1 }
      nickname: { card: { min: 0 } }
relations:
  membership:
    relates:
      member: { card: 1 }
  event:
    owns:
      identifier: { key: true }
      email: { card: 1 }
plays:
  actor:
    membership: [member]
  event:
    membership: [member]
"#;

const ORDERED_SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  tag: { value: string }
entities:
  person:
    owns:
      identifier: { key: true }
      tag:
        ordered: true
        distinct: true
        card: { min: 0 }
relations:
  collaboration:
    relates:
      participant:
        ordered: true
        distinct: true
        card: { min: 0 }
plays:
  person:
    collaboration: [participant]
"#;

#[derive(Debug, Default)]
struct RecordingState {
    opens: Vec<TxType>,
    queries: Vec<String>,
    query_modes: Vec<&'static str>,
    given_rows: Vec<GivenRowsSpec>,
    commits: usize,
    rollbacks: usize,
    closes: usize,
}

enum RecordingResponse {
    Result(QueryResult),
    Error(&'static str),
    Cancel(
        type_bridge_orm::session::backend::AnswerCancellation,
        QueryResult,
    ),
}

#[derive(Clone, Copy)]
enum CommitBehavior {
    Success,
    Failure(CommitFailureCertainty, &'static str),
}

struct RecordingBackend {
    given_enabled: bool,
    responses: Arc<Mutex<VecDeque<RecordingResponse>>>,
    state: Arc<Mutex<RecordingState>>,
    commit: CommitBehavior,
}

impl RecordingBackend {
    fn new(
        responses: Vec<RecordingResponse>,
        commit: CommitBehavior,
    ) -> (Self, Arc<Mutex<RecordingState>>) {
        let state = Arc::new(Mutex::new(RecordingState::default()));
        (
            Self {
                given_enabled: false,
                responses: Arc::new(Mutex::new(responses.into())),
                state: Arc::clone(&state),
                commit,
            },
            state,
        )
    }
}

impl DriverBackend for RecordingBackend {
    fn server_version(&self) -> Option<type_bridge_core_lib::version::Version> {
        self.given_enabled
            .then(|| type_bridge_core_lib::version::Version::new(3, 13, 6))
    }

    fn supports_given_rows(&self) -> bool {
        self.given_enabled
    }

    fn open_transaction(
        &self,
        _database: &str,
        tx_type: TxType,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
        self.state.lock().unwrap().opens.push(tx_type);
        let transaction = RecordingTransaction {
            responses: Arc::clone(&self.responses),
            state: Arc::clone(&self.state),
            commit: self.commit,
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
    commit: CommitBehavior,
}

impl RecordingTransaction {
    fn next_response(&self) -> RecordingResponse {
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("the projected CRUD test issued an unexpected provider query")
    }
}

impl TransactionOps for RecordingTransaction {
    fn supports_given_rows(&self) -> bool {
        true
    }

    fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        self.state.lock().unwrap().query_modes.push("legacy");
        self.state.lock().unwrap().queries.push(typeql.to_owned());
        let response = self.next_response();
        Box::pin(async move { response_result(response) })
    }

    fn query_canonical(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        self.state.lock().unwrap().query_modes.push("canonical");
        self.state.lock().unwrap().queries.push(typeql.to_owned());
        let response = self.next_response();
        Box::pin(async move { response_result(response) })
    }

    fn query_with_rows(
        &mut self,
        typeql: &str,
        rows: GivenRowsSpec,
    ) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        let mut state = self.state.lock().unwrap();
        state.query_modes.push("given");
        state.queries.push(typeql.to_owned());
        state.given_rows.push(rows);
        drop(state);
        let response = self.next_response();
        Box::pin(async move { response_result(response) })
    }

    fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        Box::pin(async { panic!("projected CRUD used the lossy commit path") })
    }

    fn commit_classified(&mut self) -> BoxFuture<'_, Result<(), ClassifiedCommitError>> {
        self.state.lock().unwrap().commits += 1;
        let behavior = self.commit;
        Box::pin(async move {
            match behavior {
                CommitBehavior::Success => Ok(()),
                CommitBehavior::Failure(certainty, message) => Err(ClassifiedCommitError::Driver {
                    certainty,
                    message: message.to_owned(),
                }),
            }
        })
    }

    fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.state.lock().unwrap().rollbacks += 1;
        Box::pin(async { Ok(()) })
    }

    fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.state.lock().unwrap().closes += 1;
        Box::pin(async { Ok(()) })
    }
}

fn response_result(response: RecordingResponse) -> Result<QueryResult, OrmError> {
    match response {
        RecordingResponse::Cancel(cancellation, result) => {
            cancellation.cancel();
            Ok(result)
        }
        RecordingResponse::Result(result) => Ok(result),
        RecordingResponse::Error(message) => Err(OrmError::QueryExecution(message.to_owned())),
    }
}

fn installed() -> InstalledRuntimeProjection {
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("projected-crud.yaml").unwrap(), SCHEMA)])
            .unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
    let projection = project(
        &resolved,
        BindingTarget::Python,
        &ProjectionConfig::python(),
        &[ProjectionHandler::python_v1()],
        &[],
    )
    .unwrap();
    InstalledRuntimeProjection::try_new(projection).unwrap()
}

fn installed_ordered() -> InstalledRuntimeProjection {
    installed_ordered_with_schema(ORDERED_SCHEMA)
}

fn installed_ordered_with_schema(schema: &str) -> InstalledRuntimeProjection {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("projected-crud-ordered.yaml").unwrap(),
        schema,
    )])
    .unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
    let projection = project(
        &resolved,
        BindingTarget::Python,
        &ProjectionConfig::python(),
        &[ProjectionHandler::python_v2()],
        &[],
    )
    .unwrap();
    InstalledRuntimeProjection::try_new(projection).unwrap()
}

fn type_id(kind: TypeKind, label: &str) -> TypeId {
    TypeId::new(kind, label).unwrap()
}

fn field(owner: &TypeId, attribute: &str) -> OwnsFactId {
    OwnsFactId::new(owner.clone(), AttributeId::new(attribute).unwrap()).unwrap()
}

fn string_value(
    installed: &InstalledRuntimeProjection,
    attribute: &str,
    value: &str,
) -> ProjectedAttributeValue {
    ProjectedAttributeValue::try_new(
        installed,
        type_id(TypeKind::Attribute, attribute),
        CanonicalValue::String(CanonicalString::new(value).unwrap()),
    )
    .unwrap()
}

fn person_create(installed: &InstalledRuntimeProjection) -> ProjectedCreate {
    let person = type_id(TypeKind::Entity, "person");
    ProjectedCreate::try_new(
        installed,
        person.clone(),
        vec![
            (
                field(&person, "identifier"),
                vec![string_value(installed, "identifier", "ada")],
            ),
            (
                field(&person, "email"),
                vec![string_value(installed, "email", "ada@example.test")],
            ),
        ],
        vec![],
    )
    .unwrap()
}

fn ordered_person_create(
    installed: &InstalledRuntimeProjection,
    tags: &[&str],
) -> Result<ProjectedCreate, type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic> {
    let person = type_id(TypeKind::Entity, "person");
    ProjectedCreate::try_new(
        installed,
        person.clone(),
        vec![
            (
                field(&person, "identifier"),
                vec![string_value(installed, "identifier", "data-ada")],
            ),
            (
                field(&person, "tag"),
                tags.iter()
                    .map(|tag| string_value(installed, "tag", tag))
                    .collect(),
            ),
        ],
        vec![],
    )
}

fn ordered_collaboration_create(
    installed: &InstalledRuntimeProjection,
) -> Result<ProjectedCreate, type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic> {
    let collaboration = type_id(TypeKind::Relation, "collaboration");
    let person = type_id(TypeKind::Entity, "person");
    ProjectedCreate::try_new(
        installed,
        collaboration,
        vec![],
        vec![(
            RoleId::new("collaboration", "participant").unwrap(),
            ["0x10", "0x11"]
                .into_iter()
                .map(|iid| {
                    ProjectedReference::try_new(
                        installed,
                        person.clone(),
                        Some(iid.to_owned()),
                        vec![],
                    )
                    .unwrap()
                })
                .collect(),
        )],
    )
}

fn membership_create(installed: &InstalledRuntimeProjection) -> ProjectedCreate {
    membership_create_for_player(installed, "person")
}

fn membership_create_for_player(
    installed: &InstalledRuntimeProjection,
    player_type: &str,
) -> ProjectedCreate {
    let membership = type_id(TypeKind::Relation, "membership");
    let player = type_id(TypeKind::Entity, player_type);
    let reference =
        ProjectedReference::try_new(installed, player, Some("0x10".into()), vec![]).unwrap();
    ProjectedCreate::try_new(
        installed,
        membership,
        vec![],
        vec![(
            RoleId::new("membership", "member").unwrap(),
            vec![reference],
        )],
    )
    .unwrap()
}

fn membership_create_for_relation_player(
    installed: &InstalledRuntimeProjection,
) -> ProjectedCreate {
    let membership = type_id(TypeKind::Relation, "membership");
    let player = type_id(TypeKind::Relation, "event");
    let reference =
        ProjectedReference::try_new(installed, player, Some("0x11".into()), vec![]).unwrap();
    ProjectedCreate::try_new(
        installed,
        membership,
        vec![],
        vec![(
            RoleId::new("membership", "member").unwrap(),
            vec![reference],
        )],
    )
    .unwrap()
}

fn membership_create_with_reference(
    installed: &InstalledRuntimeProjection,
    reference: ProjectedReference,
) -> ProjectedCreate {
    ProjectedCreate::try_new(
        installed,
        type_id(TypeKind::Relation, "membership"),
        vec![],
        vec![(
            RoleId::new("membership", "member").unwrap(),
            vec![reference],
        )],
    )
    .unwrap()
}

fn retained_complete_reference(
    installed: &InstalledRuntimeProjection,
    thing: &ProjectedThing,
) -> ProjectedReference {
    let visible = thing.try_to_reference(installed).unwrap();
    ProjectedReference::try_new_with_origin_carrier(
        installed,
        visible.type_id().clone(),
        visible.iid().map(str::to_owned),
        visible
            .keys()
            .iter()
            .map(|(field, value)| (field.clone(), value.clone()))
            .collect(),
        thing.origin_carrier(),
    )
    .unwrap()
}

fn retained_reference_facade(
    installed: &InstalledRuntimeProjection,
    visible: &ProjectedReference,
) -> ProjectedReference {
    ProjectedReference::try_new_with_origin_carrier(
        installed,
        visible.type_id().clone(),
        visible.iid().map(str::to_owned),
        visible
            .keys()
            .iter()
            .map(|(field, value)| (field.clone(), value.clone()))
            .collect(),
        visible.origin_carrier(),
    )
    .unwrap()
}

fn person_document(iid: &str) -> serde_json::Value {
    serde_json::json!({
        "_iid": iid,
        "_type": "person",
        "attributes": {
            "identifier": "ada",
            "email": "ada@example.test"
        }
    })
}

fn membership_document(iid: &str) -> serde_json::Value {
    serde_json::json!({
        "_iid": iid,
        "_type": "membership",
        "attributes": {},
        "_role_0_iid": "0x10",
        "_role_0_type": "person",
        "_role_0_attributes": {
            "identifier": "ada",
            "email": "ada@example.test"
        }
    })
}

fn membership_with_relation_player_document(iid: &str) -> serde_json::Value {
    serde_json::json!({
        "_iid": iid,
        "_type": "membership",
        "attributes": {},
        "_role_0_iid": "0x11",
        "_role_0_type": "event",
        "_role_0_attributes": {
            "identifier": "event-key",
            "email": "event@example.test"
        }
    })
}

fn event_document(iid: &str) -> serde_json::Value {
    serde_json::json!({
        "_iid": iid,
        "_type": "event",
        "attributes": {
            "identifier": "event-key",
            "email": "event@example.test"
        }
    })
}

fn documents(values: Vec<serde_json::Value>) -> RecordingResponse {
    RecordingResponse::Result(QueryResult::Documents(values))
}

fn ordered_collaboration_document(iids: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "_iid": "0x20", "_type": "collaboration", "attributes": {},
        "_role_0_iid": iids[0], "_role_0_type": "person",
        "_role_0_attributes": {"identifier": "person-key"}
    })
}

#[tokio::test]
async fn ordered_distinct_duplicates_fail_before_provider_io() {
    let installed = installed_ordered();
    let (backend, state) = RecordingBackend::new(vec![], CommitBehavior::Success);
    let database = Database::with_backend(Box::new(backend), "test");

    let diagnostic = async {
        let create = ordered_person_create(&installed, &["analyst", "analyst"])?;
        ProjectedCrudExecutor::new(&installed)
            .insert_entity(&database, &create)
            .await
    }
    .await
    .unwrap_err();

    assert_eq!(diagnostic.category(), SdkDiagnosticCategory::InvalidInput);
    assert_eq!(diagnostic.code().as_str(), "ordered_distinct_duplicate");

    let person = type_id(TypeKind::Entity, "person");
    let duplicate_player =
        ProjectedReference::try_new(&installed, person.clone(), Some("0x10".into()), vec![])
            .unwrap();
    let diagnostic = async {
        let create = ProjectedCreate::try_new(
            &installed,
            type_id(TypeKind::Relation, "collaboration"),
            vec![],
            vec![(
                RoleId::new("collaboration", "participant").unwrap(),
                vec![duplicate_player.clone(), duplicate_player],
            )],
        )?;
        ProjectedCrudExecutor::new(&installed)
            .insert_relation(&database, &create)
            .await
    }
    .await
    .unwrap_err();

    assert_eq!(diagnostic.category(), SdkDiagnosticCategory::InvalidInput);
    assert_eq!(diagnostic.code().as_str(), "ordered_distinct_duplicate");
    let state = state.lock().unwrap();
    assert!(state.opens.is_empty());
    assert!(state.queries.is_empty());
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 0);
    assert_eq!(state.closes, 0);
}

#[tokio::test]
async fn nonempty_ordered_collections_are_refused_before_provider_io() {
    let installed = installed_ordered();
    for create in [
        ordered_person_create(&installed, &["analyst", "mathematician"]).unwrap(),
        ordered_collaboration_create(&installed).unwrap(),
    ] {
        let (backend, state) = RecordingBackend::new(vec![], CommitBehavior::Success);
        let database = Database::with_backend(Box::new(backend), "test");
        let executor = ProjectedCrudExecutor::new(&installed);
        let error = if create.type_id().kind() == TypeKind::Entity {
            executor
                .insert_entity(&database, &create)
                .await
                .unwrap_err()
        } else {
            executor
                .insert_relation(&database, &create)
                .await
                .unwrap_err()
        };
        assert_eq!(
            error.category(),
            SdkDiagnosticCategory::UnsupportedCapability
        );
        assert_eq!(
            error.code().as_str(),
            "query_plan_ordered_collection_unsupported"
        );
        let state = state.lock().unwrap();
        assert!(state.opens.is_empty());
        assert!(state.queries.is_empty());
        assert_eq!(state.commits, 0);
    }
}

#[tokio::test]
async fn empty_ordered_ownership_insert_and_update_use_one_native_plan() {
    let installed = installed_ordered();
    let create = ordered_person_create(&installed, &[]).unwrap();
    let document = serde_json::json!({
        "_iid": "0x10", "_type": "person",
        "attributes": {"identifier": "data-ada"}
    });
    let (backend, state) = RecordingBackend::new(
        vec![documents(vec![document.clone()]), documents(vec![document])],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");
    let executor = ProjectedCrudExecutor::new(&installed);
    let inserted = executor.insert_entity(&database, &create).await.unwrap();
    assert!(inserted.fields()[&field(create.type_id(), "tag")].is_empty());
    let updated = executor
        .update_entity(&database, "0x10", &create)
        .await
        .unwrap();
    assert!(updated.fields()[&field(create.type_id(), "tag")].is_empty());
    let state = state.lock().unwrap();
    assert_eq!(state.opens, [TxType::Write, TxType::Write]);
    assert_eq!(state.queries.len(), 2);
    assert!(
        state
            .queries
            .iter()
            .all(|query| query.contains("fetch") && !query.contains("has tag"))
    );
    assert_eq!(state.commits, 2);
}

#[tokio::test]
async fn empty_ordered_role_insert_update_and_read_use_native_plans() {
    let schema = ORDERED_SCHEMA
        .replace(
            "      participant:\n",
            "      anchor: { card: { min: 0, max: 1 } }\n      participant:\n",
        )
        .replace(
            "collaboration: [participant]",
            "collaboration: [anchor, participant]",
        );
    let installed = installed_ordered_with_schema(&schema);
    let player = ProjectedReference::try_new(
        &installed,
        type_id(TypeKind::Entity, "person"),
        Some("0x10".into()),
        vec![],
    )
    .unwrap();
    let relation = type_id(TypeKind::Relation, "collaboration");
    let role = RoleId::new("collaboration", "participant").unwrap();
    let create = ProjectedCreate::try_new(
        &installed,
        relation.clone(),
        vec![],
        vec![
            (
                RoleId::new("collaboration", "anchor").unwrap(),
                vec![player],
            ),
            (role.clone(), vec![]),
        ],
    )
    .unwrap();
    let document = serde_json::json!({"_iid": "0x20", "_type": "collaboration", "attributes": {},
        "_role_0_iid": "0x10", "_role_0_type": "person", "_role_0_attributes": {"identifier": "ada"},
        "_role_1_iid": null, "_role_1_type": null, "_role_1_attributes": null});
    let (backend, state) = RecordingBackend::new(
        vec![
            documents(vec![document.clone()]),
            documents(vec![document.clone()]),
            documents(vec![document]),
        ],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");
    let executor = ProjectedCrudExecutor::new(&installed);
    let inserted = executor.insert_relation(&database, &create).await.unwrap();
    assert!(inserted.roles()[&role].is_empty());
    let updated = executor
        .update_relation(&database, "0x20", &create)
        .await
        .unwrap();
    assert!(updated.roles()[&role].is_empty());
    let fetched = executor
        .get_relation_by_iid(&database, &relation, "0x20")
        .await
        .unwrap()
        .unwrap();
    assert!(fetched.roles()[&role].is_empty());
    let state = state.lock().unwrap();
    assert_eq!(state.queries.len(), 3);
    assert_eq!(state.query_modes, ["canonical"; 3]);
    assert_eq!(state.commits, 2);
}

#[tokio::test]
async fn single_relation_writes_refuse_stale_ordered_players_and_preserve_transaction_failure() {
    use type_bridge_orm::QueryExecutionResourceLimits;
    use type_bridge_orm::session::backend::AnswerCancellation;
    let schema = ORDERED_SCHEMA
        .replace(
            "  collaboration:\n    relates:\n",
            "  collaboration:\n    owns:\n      identifier: { key: true }\n    relates:\n      anchor: { card: { min: 0, max: 1 } }\n",
        )
        .replace("collaboration: [participant]", "collaboration: [anchor, participant]");
    let installed = installed_ordered_with_schema(&schema);
    let relation = type_id(TypeKind::Relation, "collaboration");
    let player = ProjectedReference::try_new(
        &installed,
        type_id(TypeKind::Entity, "person"),
        Some("0x10".into()),
        vec![],
    )
    .unwrap();
    let create = ProjectedCreate::try_new(
        &installed,
        relation.clone(),
        vec![(
            field(&relation, "identifier"),
            vec![string_value(&installed, "identifier", "team")],
        )],
        vec![
            (
                RoleId::new("collaboration", "anchor").unwrap(),
                vec![player],
            ),
            (RoleId::new("collaboration", "participant").unwrap(), vec![]),
        ],
    )
    .unwrap();
    let document = serde_json::json!({
        "_iid": "0x20", "_type": "collaboration", "attributes": {"identifier": "team"},
        "_role_0_iid": "0x10", "_role_0_type": "person", "_role_0_attributes": {"identifier": "ada"},
        "_role_1_iid": "0x11", "_role_1_type": "person", "_role_1_attributes": {"identifier": "bob"}
    });
    for operation in [
        "update",
        "controlled_update",
        "put",
        "controlled_put",
        "delete_returning",
    ] {
        for borrowed in [false, true] {
            let mut responses = Vec::new();
            responses.push(documents(vec![document.clone()]));
            if operation == "delete_returning" {
                responses.push(RecordingResponse::Result(QueryResult::Ok));
            }
            let (mut backend, state) = RecordingBackend::new(responses, CommitBehavior::Success);
            backend.given_enabled = true;
            let database = Database::with_backend(Box::new(backend), "stale-ordered-role");
            let executor = ProjectedCrudExecutor::new(&installed);
            let failure = if borrowed {
                let transaction = database.transaction_context(TxType::Write).await.unwrap();
                let failure = match operation {
                    "update" => executor
                        .update_relation_in_transaction(&transaction, "0x20", &create)
                        .await
                        .map(|_| ()),
                    "controlled_update" => executor
                        .update_relation_in_transaction_controlled(
                            &transaction,
                            "0x20",
                            &create,
                            QueryExecutionResourceLimits::default(),
                            AnswerCancellation::default(),
                        )
                        .await
                        .map(|_| ()),
                    "controlled_put" => executor
                        .put_relation_in_transaction_controlled(
                            &transaction,
                            &create,
                            QueryExecutionResourceLimits::default(),
                            AnswerCancellation::default(),
                        )
                        .await
                        .map(|_| ()),
                    "put" => executor
                        .put_relation_in_transaction(&transaction, &create)
                        .await
                        .map(|_| ()),
                    _ => executor
                        .delete_relation_by_iid_returning_in_transaction(
                            &transaction,
                            &relation,
                            "0x20",
                        )
                        .await
                        .map(|_| ()),
                }
                .unwrap_err();
                assert_eq!(
                    transaction.lifecycle_state().await,
                    type_bridge_orm::session::TransactionContextState::RollbackOnly
                );
                {
                    let state = state.lock().unwrap();
                    assert_eq!((state.commits, state.rollbacks), (0, 0));
                }
                assert_eq!(
                    transaction.commit_sdk().await.unwrap_err().code().as_str(),
                    "transaction_rollback_only"
                );
                transaction.rollback().await.unwrap();
                failure
            } else {
                match operation {
                    "update" => executor
                        .update_relation(&database, "0x20", &create)
                        .await
                        .map(|_| ()),
                    "controlled_update" => executor
                        .update_relation_controlled(
                            &database,
                            "0x20",
                            &create,
                            QueryExecutionResourceLimits::default(),
                            AnswerCancellation::default(),
                        )
                        .await
                        .map(|_| ()),
                    "controlled_put" => executor
                        .put_relation_controlled(
                            &database,
                            &create,
                            QueryExecutionResourceLimits::default(),
                            AnswerCancellation::default(),
                        )
                        .await
                        .map(|_| ()),
                    "put" => executor.put_relation(&database, &create).await.map(|_| ()),
                    _ => executor
                        .delete_relation_by_iid_returning(&database, &relation, "0x20")
                        .await
                        .map(|_| ()),
                }
                .unwrap_err()
            };
            assert_eq!(failure.category(), SdkDiagnosticCategory::Integrity);
            assert_eq!(
                failure.code().as_str(),
                "query_plan_hydration_ordered_role",
                "{operation} borrowed={borrowed}"
            );
            let state = state.lock().unwrap();
            let calls = if operation == "delete_returning" {
                2
            } else {
                1
            };
            assert_eq!(
                state.queries.len(),
                calls,
                "{operation} borrowed={borrowed}"
            );
            let modes = match operation {
                "put" | "controlled_put" => vec!["given"],
                "delete_returning" => vec!["given", "given"],
                _ => vec!["canonical"],
            };
            assert_eq!(state.query_modes, modes, "{operation} borrowed={borrowed}");
            assert_eq!((state.commits, state.rollbacks), (0, 1));
        }
    }
}

#[tokio::test]
async fn populated_ordered_provider_role_is_refused_during_native_hydration() {
    let installed = installed_ordered();
    let (backend, state) = RecordingBackend::new(
        vec![documents(vec![ordered_collaboration_document(&[
            "0x10", "0x10",
        ])])],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");

    let diagnostic = ProjectedCrudExecutor::new(&installed)
        .get_relation_by_iid(
            &database,
            &type_id(TypeKind::Relation, "collaboration"),
            "0x20",
        )
        .await
        .unwrap_err();

    assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Integrity);
    assert_eq!(
        diagnostic.code().as_str(),
        "query_plan_hydration_ordered_role"
    );
    let state = state.lock().unwrap();
    assert_eq!(state.opens, [TxType::Read]);
    assert_eq!(state.queries.len(), 1);
    assert_eq!(state.closes, 1);
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn stale_entity_update_rolls_back_and_never_reports_success() {
    let installed = installed();
    let create = person_create(&installed);
    let (backend, state) = RecordingBackend::new(
        vec![
            // The update plan matched no IID: no document, nothing written.
            documents(vec![]),
        ],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");

    let diagnostic = ProjectedCrudExecutor::new(&installed)
        .update_entity(&database, "0x10", &create)
        .await
        .unwrap_err();

    assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Integrity);
    assert_eq!(diagnostic.code().as_str(), "mutation_rehydration_missing");
    let state = state.lock().unwrap();
    assert_eq!(state.opens, [TxType::Write]);
    assert_eq!(state.queries.len(), 1);
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 1);
    assert_eq!(state.closes, 0);
}

#[tokio::test]
async fn hydration_failure_after_insert_rolls_back_and_redacts_provider_shape() {
    let installed = installed();
    let create = person_create(&installed);
    let (backend, state) = RecordingBackend::new(
        vec![documents(vec![serde_json::json!({
            "_iid": "0x10",
            "_type": "person",
            "attributes": {"identifier": "ada"}
        })])],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");

    let diagnostic = ProjectedCrudExecutor::new(&installed)
        .insert_entity(&database, &create)
        .await
        .unwrap_err();

    assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Integrity);
    assert_eq!(diagnostic.code().as_str(), "query_remote_v3_document_value");
    let rendered = format!("{diagnostic:?} {diagnostic}");
    assert!(!rendered.contains("email"));
    let state = state.lock().unwrap();
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 1);
}

#[tokio::test]
async fn relation_iid_ambiguity_is_rejected_before_commit() {
    let installed = installed();
    let create = membership_create(&installed);
    let (backend, state) = RecordingBackend::new(
        vec![documents(vec![
            membership_document("0x20"),
            membership_document("0x21"),
        ])],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");

    let diagnostic = ProjectedCrudExecutor::new(&installed)
        .insert_relation(&database, &create)
        .await
        .unwrap_err();

    assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Integrity);
    let state = state.lock().unwrap();
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 1);
    assert!(state.queries[0].contains("insert") && state.queries[0].contains("membership"));
    // The relation's own fetch is the hydration read; nothing follows it.
    assert_eq!(state.queries.len(), 1);
    assert!(state.queries[0].contains("isa! $PlanFetchType"));
}

#[tokio::test]
async fn abstract_nominal_role_reference_resolves_one_exact_concrete_player() {
    let installed = installed();
    let create = membership_create_for_player(&installed, "actor");
    let (backend, state) = RecordingBackend::new(
        vec![documents(vec![membership_document("0x20")])],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");

    let relation = ProjectedCrudExecutor::new(&installed)
        .insert_relation(&database, &create)
        .await
        .unwrap();

    assert_eq!(relation.iid(), "0x20");
    let member = RoleId::new("membership", "member").unwrap();
    let player = &relation.roles()[&member][0];
    assert_eq!(player.exact_form(), Some(ProjectedModelForm::Complete));
    assert_eq!(player.keys().len(), 1);
    assert_eq!(player.fields().len(), 3);
    assert_eq!(
        player.fields()[&field(&type_id(TypeKind::Entity, "person"), "email")][0].value(),
        &CanonicalValue::String(CanonicalString::new("ada@example.test").unwrap())
    );
    let state = state.lock().unwrap();
    assert_eq!(state.queries.len(), 1);
    assert!(state.queries[0].contains("isa! person"));
    assert!(!state.queries[0].contains("isa! actor"));
    assert_eq!(state.commits, 1);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn relation_role_player_resolves_inside_the_write_by_strict_identity() {
    let installed = installed();
    let create = membership_create_for_relation_player(&installed);
    let (backend, state) = RecordingBackend::new(
        vec![documents(vec![membership_with_relation_player_document(
            "0x20",
        )])],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");

    let relation = ProjectedCrudExecutor::new(&installed)
        .insert_relation(&database, &create)
        .await
        .unwrap();

    assert_eq!(relation.iid(), "0x20");
    let member = RoleId::new("membership", "member").unwrap();
    let player = &relation.roles()[&member][0];
    assert_eq!(player.exact_form(), Some(ProjectedModelForm::Reference));
    assert!(player.fields().is_empty());
    assert_eq!(player.keys().len(), 1);
    assert_eq!(
        player.keys()[&field(&type_id(TypeKind::Relation, "event"), "identifier")].value(),
        &CanonicalValue::String(CanonicalString::new("event-key").unwrap())
    );
    assert!(player.reference().origin_carrier().is_some());
    let state = state.lock().unwrap();
    // The reference resolves inside the write: no lookup precedes it.
    assert_eq!(state.queries.len(), 1);
    assert!(state.queries[0].contains("$player1 isa! event"));
    assert!(state.queries[0].contains("$player1 iid 0x11"));
    assert!(!state.queries[0].contains("$player1 isa event"));
    assert!(state.queries[0].contains("insert"));
    assert_eq!(state.commits, 1);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn provider_origin_reference_is_rejected_for_another_database_before_io() {
    let installed = installed();
    let membership = type_id(TypeKind::Relation, "membership");
    let role = RoleId::new("membership", "member").unwrap();
    let (source_backend, _source_state) = RecordingBackend::new(
        vec![documents(vec![membership_document("0x20")])],
        CommitBehavior::Success,
    );
    let source_database = Database::with_backend(Box::new(source_backend), "source");
    let hydrated = ProjectedCrudExecutor::new(&installed)
        .get_relation_by_iid(&source_database, &membership, "0x20")
        .await
        .unwrap()
        .unwrap();
    let reference = hydrated.roles().get(&role).unwrap()[0].reference().clone();
    let create = ProjectedCreate::try_new(
        &installed,
        membership,
        vec![],
        vec![(role, vec![reference])],
    )
    .unwrap();

    let (target_backend, target_state) = RecordingBackend::new(vec![], CommitBehavior::Success);
    let target_database = Database::with_backend(Box::new(target_backend), "target");
    let diagnostic = ProjectedCrudExecutor::new(&installed)
        .insert_relation(&target_database, &create)
        .await
        .unwrap_err();

    assert_eq!(diagnostic.category(), SdkDiagnosticCategory::InvalidInput);
    assert_eq!(diagnostic.code().as_str(), "reference_database_mismatch");
    let state = target_state.lock().unwrap();
    assert!(state.opens.is_empty());
    assert!(state.queries.is_empty());
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn retained_complete_entity_origin_accepts_same_database_and_fences_foreign_before_io() {
    let installed = installed();
    let person = type_id(TypeKind::Entity, "person");
    let authority = DatabaseConnectionAuthority::isolated();
    let (source_backend, source_state) = RecordingBackend::new(
        vec![documents(vec![person_document("0x10")])],
        CommitBehavior::Success,
    );
    let source_database =
        Database::with_backend_authority(Box::new(source_backend), "shared", authority.clone());
    let hydrated = ProjectedCrudExecutor::new(&installed)
        .get_entity_by_iid(&source_database, &person, "0x10")
        .await
        .unwrap()
        .unwrap();
    let reference = retained_complete_reference(&installed, &hydrated);
    assert_eq!(
        format!("{:?}", reference.origin_carrier().unwrap()),
        "ProjectedReferenceOrigin([REDACTED])"
    );
    let create = membership_create_with_reference(&installed, reference);

    let (same_backend, same_state) = RecordingBackend::new(
        vec![documents(vec![membership_document("0x20")])],
        CommitBehavior::Success,
    );
    let same_database =
        Database::with_backend_authority(Box::new(same_backend), "shared", authority);
    let created = ProjectedCrudExecutor::new(&installed)
        .insert_relation(&same_database, &create)
        .await
        .unwrap();
    assert_eq!(created.iid(), "0x20");
    {
        let state = same_state.lock().unwrap();
        assert_eq!(state.opens, [TxType::Write]);
        assert_eq!(state.queries.len(), 1);
        assert_eq!(state.commits, 1);
        assert_eq!(state.rollbacks, 0);
    }

    let (foreign_backend, foreign_state) = RecordingBackend::new(vec![], CommitBehavior::Success);
    let foreign_database = Database::with_backend(Box::new(foreign_backend), "shared");
    let diagnostic = ProjectedCrudExecutor::new(&installed)
        .insert_relation(&foreign_database, &create)
        .await
        .unwrap_err();
    assert_eq!(diagnostic.category(), SdkDiagnosticCategory::InvalidInput);
    assert_eq!(diagnostic.code().as_str(), "reference_database_mismatch");
    let state = foreign_state.lock().unwrap();
    assert!(state.opens.is_empty());
    assert!(state.queries.is_empty());
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 0);
    assert_eq!(source_state.lock().unwrap().closes, 1);
}

#[tokio::test]
async fn retained_relation_reference_facade_fences_foreign_database_before_io() {
    let installed = installed();
    let event = type_id(TypeKind::Relation, "event");
    let (source_backend, _source_state) = RecordingBackend::new(
        vec![documents(vec![event_document("0x11")])],
        CommitBehavior::Success,
    );
    let source_database = Database::with_backend(Box::new(source_backend), "source");
    let hydrated = ProjectedCrudExecutor::new(&installed)
        .get_relation_by_iid(&source_database, &event, "0x11")
        .await
        .unwrap()
        .unwrap();
    let projected = hydrated.try_to_reference(&installed).unwrap();
    let retained = retained_reference_facade(&installed, &projected);
    let create = membership_create_with_reference(&installed, retained);

    let (target_backend, target_state) = RecordingBackend::new(vec![], CommitBehavior::Success);
    let target_database = Database::with_backend(Box::new(target_backend), "target");
    let diagnostic = ProjectedCrudExecutor::new(&installed)
        .insert_relation(&target_database, &create)
        .await
        .unwrap_err();

    assert_eq!(diagnostic.category(), SdkDiagnosticCategory::InvalidInput);
    assert_eq!(diagnostic.code().as_str(), "reference_database_mismatch");
    let state = target_state.lock().unwrap();
    assert!(state.opens.is_empty());
    assert!(state.queries.is_empty());
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn borrowed_origin_is_preserved_and_a_value_equal_lookalike_remains_unbound() {
    let installed = installed();
    let person = type_id(TypeKind::Entity, "person");
    let authority = DatabaseConnectionAuthority::isolated();
    let (source_backend, source_state) = RecordingBackend::new(
        vec![documents(vec![person_document("0x10")])],
        CommitBehavior::Success,
    );
    let source_database =
        Database::with_backend_authority(Box::new(source_backend), "shared", authority.clone());
    let transaction = source_database
        .transaction_context(TxType::Read)
        .await
        .unwrap();
    let hydrated = ProjectedCrudExecutor::new(&installed)
        .get_entity_by_iid_in_transaction(&transaction, &person, "0x10")
        .await
        .unwrap()
        .unwrap();
    let retained = retained_complete_reference(&installed, &hydrated);
    let visible_lookalike = ProjectedReference::try_new(
        &installed,
        retained.type_id().clone(),
        retained.iid().map(str::to_owned),
        retained
            .keys()
            .iter()
            .map(|(field, value)| (field.clone(), value.clone()))
            .collect(),
    )
    .unwrap();
    assert_eq!(retained, visible_lookalike);
    assert!(retained.origin_carrier().is_some());
    assert!(visible_lookalike.origin_carrier().is_none());

    let retained_create = membership_create_with_reference(&installed, retained);
    let (same_backend, same_state) = RecordingBackend::new(vec![], CommitBehavior::Success);
    let same_database =
        Database::with_backend_authority(Box::new(same_backend), "shared", authority);
    ProjectedCrudExecutor::new(&installed)
        .preflight_relation_create_for_database_with_compatibility(&same_database, &retained_create)
        .unwrap();
    assert!(same_state.lock().unwrap().opens.is_empty());

    let lookalike_create = membership_create_with_reference(&installed, visible_lookalike);
    let (foreign_backend, foreign_state) = RecordingBackend::new(
        vec![documents(vec![membership_document("0x20")])],
        CommitBehavior::Success,
    );
    let foreign_database = Database::with_backend(Box::new(foreign_backend), "foreign");
    let created = ProjectedCrudExecutor::new(&installed)
        .insert_relation(&foreign_database, &lookalike_create)
        .await
        .unwrap();
    assert_eq!(created.iid(), "0x20");
    {
        let state = foreign_state.lock().unwrap();
        assert_eq!(state.opens, [TxType::Write]);
        assert_eq!(state.queries.len(), 1);
        assert_eq!(state.commits, 1);
    }
    transaction.close().await.unwrap();
    assert_eq!(source_state.lock().unwrap().closes, 1);
}

#[tokio::test]
async fn borrowed_write_transaction_is_never_implicitly_finished() {
    let installed = installed();
    let create = person_create(&installed);
    let (backend, state) = RecordingBackend::new(
        vec![documents(vec![person_document("0x10")])],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");
    let transaction = database.transaction_context(TxType::Write).await.unwrap();

    let thing = ProjectedCrudExecutor::new(&installed)
        .insert_entity_in_transaction(&transaction, &create)
        .await
        .unwrap();
    assert_eq!(thing.iid(), "0x10");
    {
        let state = state.lock().unwrap();
        assert_eq!(state.opens, [TxType::Write]);
        assert_eq!(state.commits, 0);
        assert_eq!(state.rollbacks, 0);
        assert_eq!(state.closes, 0);
    }

    transaction.commit_classified().await.unwrap();
    assert_eq!(state.lock().unwrap().commits, 1);
}

#[tokio::test]
async fn owned_entity_put_rehydrates_before_one_classified_commit() {
    let installed = installed();
    let create = person_create(&installed);
    let (backend, state) = RecordingBackend::new(
        vec![
            // The put answers its own hydration document.
            documents(vec![person_document("0x30")]),
        ],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");

    let thing = ProjectedCrudExecutor::new(&installed)
        .put_entity(&database, &create)
        .await
        .unwrap();

    assert_eq!(thing.iid(), "0x30");
    let state = state.lock().unwrap();
    assert_eq!(state.opens, [TxType::Write]);
    // One put plan, no key lookup and no read-back: put the key, clear the
    // omitted nickname, update the given email, fetch the document.
    assert_eq!(state.queries.len(), 1);
    let put = &state.queries[0];
    assert!(
        put.starts_with("put $thing0 isa! person, has identifier \"ada\""),
        "{put}"
    );
    assert!(
        put.contains("has nickname") && put.contains("delete"),
        "{put}"
    );
    assert!(
        put.contains("update $thing0 has email \"ada@example.test\""),
        "{put}"
    );
    assert!(put.contains("fetch"), "{put}");
    assert_eq!(state.query_modes, ["canonical"]);
    assert_eq!(state.commits, 1);
    assert_eq!(state.rollbacks, 0);
    assert_eq!(state.closes, 0);
}

#[tokio::test]
async fn owned_relation_put_and_update_rehydrate_before_commit() {
    let installed = installed();
    let create = membership_create(&installed);

    let (put_backend, put_state) = RecordingBackend::new(
        vec![documents(vec![membership_document("0x20")])],
        CommitBehavior::Success,
    );
    let put_database = Database::with_backend(Box::new(put_backend), "test");
    let executor = ProjectedCrudExecutor::new(&installed);

    let inserted = executor.put_relation(&put_database, &create).await.unwrap();
    assert_eq!(inserted.iid(), "0x20");
    let member = RoleId::new("membership", "member").unwrap();
    assert_eq!(inserted.roles().get(&member).unwrap().len(), 1);
    let player = &inserted.roles()[&member][0];
    assert_eq!(player.exact_form(), Some(ProjectedModelForm::Complete));
    assert_eq!(player.keys().len(), 1);
    assert_eq!(player.fields().len(), 3);
    assert_eq!(
        player.fields()[&field(&type_id(TypeKind::Entity, "person"), "email")][0].value(),
        &CanonicalValue::String(CanonicalString::new("ada@example.test").unwrap())
    );
    assert!(player.fields()[&field(&type_id(TypeKind::Entity, "person"), "nickname")].is_empty());
    assert!(player.reference().origin_carrier().is_some());
    {
        let state = put_state.lock().unwrap();
        assert_eq!(state.opens, [TxType::Write]);
        assert_eq!(state.queries.len(), 1);
        assert!(state.queries[0].contains("isa! person"));
        assert!(state.queries[0].contains("insert"));
        assert!(state.queries[0].contains("fetch"));
        assert_eq!(state.query_modes, ["canonical"]);
        assert_eq!(state.commits, 1);
        assert_eq!(state.rollbacks, 0);
    }

    let (update_backend, update_state) = RecordingBackend::new(
        vec![documents(vec![membership_document("0x20")])],
        CommitBehavior::Success,
    );
    let update_database = Database::with_backend(Box::new(update_backend), "test");

    let updated = executor
        .update_relation(&update_database, "0x20", &create)
        .await
        .unwrap();
    assert_eq!(updated.iid(), "0x20");
    let state = update_state.lock().unwrap();
    assert_eq!(state.opens, [TxType::Write]);
    // One plan: match the relation and the player, clear, attach, fetch.
    assert_eq!(state.queries.len(), 1);
    let update = &state.queries[0];
    assert!(update.contains("isa! membership") && update.contains("isa! person"));
    assert!(update.contains("delete") && update.contains("insert") && update.contains("fetch"));
    assert_eq!(state.query_modes, ["canonical"]);
    assert_eq!(state.commits, 1);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn relation_get_count_and_blind_delete_preserve_exact_behavior() {
    let installed = installed();
    let membership = type_id(TypeKind::Relation, "membership");
    let (backend, state) = RecordingBackend::new(
        vec![
            documents(vec![membership_document("0x20")]),
            RecordingResponse::Result(QueryResult::Rows(vec![
                serde_json::json!({"count1": {"kind":"value", "value":CanonicalValue::Long(2)}}),
            ])),
            RecordingResponse::Result(QueryResult::Rows(vec![])),
        ],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");
    let executor = ProjectedCrudExecutor::new(&installed);

    assert_eq!(
        executor
            .get_relation_by_iid(&database, &membership, "0x20")
            .await
            .unwrap()
            .unwrap()
            .iid(),
        "0x20"
    );
    assert_eq!(
        executor
            .count_relations(&database, &membership)
            .await
            .unwrap(),
        2
    );
    executor
        .delete_relation_by_iid(&database, &membership, "0x20")
        .await
        .unwrap();

    let state = state.lock().unwrap();
    assert_eq!(state.opens, [TxType::Read, TxType::Read, TxType::Write]);
    assert_eq!(state.queries.len(), 3);
    assert!(
        state
            .queries
            .iter()
            .all(|query| query.contains("isa! membership"))
    );
    assert_eq!(state.query_modes, ["canonical"; 3]);
    assert_eq!(state.closes, 2);
    assert_eq!(state.commits, 1);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn dynamic_returning_delete_captures_all_roots_before_mutation() {
    let installed = installed();
    let descriptor = Arc::new(
        installed
            .entity_descriptor(&type_id(TypeKind::Entity, "person"))
            .unwrap()
            .clone(),
    );
    let (backend, state) = RecordingBackend::new(
        vec![
            documents(vec![person_document("0x12"), person_document("0x10")]),
            RecordingResponse::Result(QueryResult::Ok),
        ],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");
    let snapshots =
        type_bridge_orm::_manager::DynamicEntityManager::new_canonical(&database, descriptor)
            .delete_many_by_iid_exact_returning(&["0x10".into(), "0x11".into(), "0x12".into()])
            .await
            .unwrap();
    assert_eq!(
        snapshots,
        vec![person_document("0x10"), person_document("0x12")]
    );
    let state = state.lock().unwrap();
    assert_eq!(state.commits, 1);
    assert_eq!(state.queries.len(), 2);
    assert_eq!(state.given_rows.len(), 2);
    for (query, rows) in state.queries.iter().zip(&state.given_rows) {
        assert!(query.starts_with("given $snapshot_iid: string;"));
        assert!(!query.contains("0x10") && !query.contains("0x11"));
        assert_eq!(rows.variables, ["snapshot_iid"]);
    }
    for rows in &state.given_rows {
        assert_eq!(
            rows.rows,
            ["0x10", "0x11", "0x12"]
                .into_iter()
                .map(|iid| vec![GivenValue::String(iid.into())])
                .collect::<Vec<_>>()
        );
    }
    assert!(state.queries[0].contains("fetch") && !state.queries[0].contains("delete"));
    assert!(state.queries[1].contains("delete"));
}

#[tokio::test]
async fn dynamic_returning_delete_normalizes_iid_case_in_given_rows() {
    let installed = installed();
    let descriptor = Arc::new(
        installed
            .entity_descriptor(&type_id(TypeKind::Entity, "person"))
            .unwrap()
            .clone(),
    );
    let (backend, state) = RecordingBackend::new(
        vec![
            documents(vec![person_document("0xab")]),
            RecordingResponse::Result(QueryResult::Ok),
        ],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");
    let snapshots =
        type_bridge_orm::_manager::DynamicEntityManager::new_canonical(&database, descriptor)
            .delete_many_by_iid_exact_returning(&["0xAB".into()])
            .await
            .unwrap();
    assert_eq!(snapshots, vec![person_document("0xab")]);
    let state = state.lock().unwrap();
    assert_eq!(state.given_rows.len(), 2);
    for rows in &state.given_rows {
        assert_eq!(rows.rows, vec![vec![GivenValue::String("0xab".into())]]);
    }
}

#[tokio::test]
async fn dynamic_returning_delete_refuses_duplicate_iids_before_opening() {
    let installed = installed();
    let descriptor = Arc::new(
        installed
            .entity_descriptor(&type_id(TypeKind::Entity, "person"))
            .unwrap()
            .clone(),
    );
    let (backend, state) = RecordingBackend::new(vec![], CommitBehavior::Success);
    let database = Database::with_backend(Box::new(backend), "test");
    let error =
        type_bridge_orm::_manager::DynamicEntityManager::new_canonical(&database, descriptor)
            .delete_many_by_iid_exact_returning(&["0x10".into(), "0x10".into()])
            .await
            .unwrap_err();
    assert!(matches!(error, OrmError::Match(_)));
    let state = state.lock().unwrap();
    assert!(state.opens.is_empty());
    assert!(state.queries.is_empty());
}

#[tokio::test]
async fn dynamic_returning_delete_validates_the_entire_batch_before_mutation() {
    let installed = installed();
    let descriptor = Arc::new(
        installed
            .entity_descriptor(&type_id(TypeKind::Entity, "person"))
            .unwrap()
            .clone(),
    );
    let (backend, state) = RecordingBackend::new(
        vec![
            // A provider result for a different root must invalidate the batch.
            documents(vec![person_document("0x10"), person_document("0x12")]),
        ],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");
    let result =
        type_bridge_orm::_manager::DynamicEntityManager::new_canonical(&database, descriptor)
            .delete_many_by_iid_exact_returning(&["0x10".into(), "0x11".into()])
            .await;
    assert!(result.is_err());
    let state = state.lock().unwrap();
    assert_eq!(state.queries.len(), 1);
    assert!(state.queries.iter().all(|query| !query.contains("delete")));
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 1);
}

#[tokio::test]
async fn dynamic_returning_delete_enforces_batch_answer_and_statement_limits() {
    let installed = installed();
    let descriptor = Arc::new(
        installed
            .entity_descriptor(&type_id(TypeKind::Entity, "person"))
            .unwrap()
            .clone(),
    );
    for (dimension, ceiling, expected_queries) in [
        ("statements", 1, 0),
        ("items", 1, 1),
        ("graph_nodes", 1, 1),
        ("attribute_values", 3, 1),
        ("collection_members", 3, 1),
    ] {
        let (backend, state) = RecordingBackend::new(
            vec![documents(vec![
                person_document("0x10"),
                person_document("0x11"),
            ])],
            CommitBehavior::Success,
        );
        let mut limits = type_bridge_orm::QueryExecutionResourceLimits::default_for(
            type_bridge_orm::AnswerTransport::Direct,
        );
        match dimension {
            "statements" => limits.statements = Some(ceiling as u32),
            "items" => limits.items = ceiling,
            "graph_nodes" => limits.graph_nodes = ceiling,
            "attribute_values" => limits.attribute_values = ceiling,
            "collection_members" => limits.collection_members = ceiling,
            _ => unreachable!(),
        }
        let database = Database::with_backend_answer_limits(Box::new(backend), "test", limits);
        let error = type_bridge_orm::_manager::DynamicEntityManager::new_canonical(
            &database,
            Arc::clone(&descriptor),
        )
        .delete_many_by_iid_exact_returning(&["0x10".into(), "0x11".into()])
        .await
        .unwrap_err();
        assert!(matches!(error, OrmError::Match(_)));
        let state = state.lock().unwrap();
        assert_eq!(state.queries.len(), expected_queries);
        assert!(state.queries.iter().all(|query| !query.contains("delete")));
        assert_eq!(state.commits, 0);
        assert_eq!(state.rollbacks, 1);
    }
}

#[tokio::test]
async fn dynamic_returning_relation_enforces_cumulative_snapshot_limits() {
    let installed = installed();
    let descriptor = Arc::new(
        installed
            .relation_descriptor(&type_id(TypeKind::Relation, "membership"))
            .unwrap()
            .clone(),
    );
    for dimension in [
        "graph_nodes",
        "attribute_values",
        "collection_members",
        "role_players",
    ] {
        let (backend, state) = RecordingBackend::new(
            vec![documents(vec![
                membership_document("0x20"),
                membership_document("0x21"),
            ])],
            CommitBehavior::Success,
        );
        let mut limits = type_bridge_orm::QueryExecutionResourceLimits::default();
        // Each snapshot fits by itself; the second root must exceed the batch ceiling.
        match dimension {
            "graph_nodes" => limits.graph_nodes = 3,
            "attribute_values" => limits.attribute_values = 3,
            "collection_members" => limits.collection_members = 5,
            "role_players" => limits.role_players = 1,
            _ => unreachable!(),
        }
        let database = Database::with_backend_answer_limits(Box::new(backend), "test", limits);
        let error = type_bridge_orm::_manager::DynamicRelationManager::new_canonical(
            &database,
            Arc::clone(&descriptor),
        )
        .delete_many_by_iid_exact_returning(&["0x20".into(), "0x21".into()])
        .await
        .unwrap_err();
        assert!(matches!(error, OrmError::Match(_)), "{dimension}: {error}");
        let state = state.lock().unwrap();
        assert_eq!(state.queries.len(), 1, "{dimension}");
        assert!(state.queries.iter().all(|query| !query.contains("delete")));
        assert_eq!(state.commits, 0);
        assert_eq!(state.rollbacks, 1);
    }
}

#[tokio::test]
async fn dynamic_returning_delete_does_not_publish_on_commit_failure() {
    let installed = installed();
    let descriptor = Arc::new(
        installed
            .entity_descriptor(&type_id(TypeKind::Entity, "person"))
            .unwrap()
            .clone(),
    );
    for certainty in [
        CommitFailureCertainty::DefinitelyAborted,
        CommitFailureCertainty::Unknown,
    ] {
        let (backend, state) = RecordingBackend::new(
            vec![
                documents(vec![person_document("0x10")]),
                RecordingResponse::Result(QueryResult::Ok),
            ],
            CommitBehavior::Failure(certainty, "commit failed"),
        );
        let database = Database::with_backend(Box::new(backend), "test");
        let result = type_bridge_orm::_manager::DynamicEntityManager::new_canonical(
            &database,
            Arc::clone(&descriptor),
        )
        .delete_many_by_iid_exact_returning(&["0x10".into()])
        .await;
        assert!(result.is_err());
        let state = state.lock().unwrap();
        assert_eq!(state.queries.len(), 2);
        assert_eq!(state.commits, 1);
    }
}

#[tokio::test]
async fn dynamic_returning_relation_coalesces_all_player_documents() {
    let installed = installed_polymorphic();
    let descriptor = Arc::new(
        installed
            .relation_descriptor(&type_id(TypeKind::Relation, "membership"))
            .unwrap()
            .clone(),
    );
    let document = |iid: &str, label: &str, key: &str| {
        serde_json::json!({
            "_iid":"0x20", "_type":"membership", "attributes":{},
            "_role_0_iid":iid, "_role_0_type":label, "_role_0_attributes":{"identifier":key}
        })
    };
    let (backend, state) = RecordingBackend::new(
        vec![
            documents(vec![
                document("0x10", "person", "ada"),
                document("0x11", "company", "acme"),
            ]),
            RecordingResponse::Result(QueryResult::Ok),
        ],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");
    let snapshots =
        type_bridge_orm::_manager::DynamicRelationManager::new_canonical(&database, descriptor)
            .delete_many_by_iid_exact_returning(&["0x20".into()])
            .await
            .unwrap();
    assert_eq!(snapshots.len(), 1);
    let players = snapshots[0]["role_players"].as_array().unwrap();
    assert_eq!(players.len(), 2);
    for (iid, identifier) in [("0x10", "ada"), ("0x11", "acme")] {
        let player = players.iter().find(|player| player["iid"] == iid).unwrap();
        assert_eq!(player["attributes"]["identifier"], identifier);
    }
    assert!(snapshots[0].get("_role_0_iid").is_none());
    let state = state.lock().unwrap();
    assert_eq!(state.commits, 1);
    assert_eq!(state.given_rows.len(), 2);
    for (query, rows) in state.queries.iter().zip(&state.given_rows) {
        assert!(query.starts_with("given $snapshot_iid: string;"));
        assert!(!query.contains("0x20"));
        assert_eq!(rows.rows, vec![vec![GivenValue::String("0x20".into())]]);
    }
}

#[tokio::test]
async fn dynamic_returning_relation_refuses_missing_required_role_before_deletion() {
    let installed = installed();
    let descriptor = Arc::new(
        installed
            .relation_descriptor(&type_id(TypeKind::Relation, "membership"))
            .unwrap()
            .clone(),
    );
    let (backend, state) = RecordingBackend::new(
        vec![documents(vec![serde_json::json!({
            "_iid":"0x20", "_type":"membership", "attributes":{}
        })])],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");
    assert!(
        type_bridge_orm::_manager::DynamicRelationManager::new_canonical(&database, descriptor)
            .delete_many_by_iid_exact_returning(&["0x20".into()])
            .await
            .is_err()
    );
    let state = state.lock().unwrap();
    assert_eq!(state.queries.len(), 1);
    assert!(state.queries[0].contains("try"));
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 1);
}

#[tokio::test]
async fn dynamic_returning_delete_failure_poison_latches_borrowed_context() {
    let installed = installed();
    let descriptor = Arc::new(
        installed
            .entity_descriptor(&type_id(TypeKind::Entity, "person"))
            .unwrap()
            .clone(),
    );
    let (backend, state) = RecordingBackend::new(
        vec![
            documents(vec![person_document("0x10")]),
            RecordingResponse::Error("delete failed"),
        ],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");
    let tx = database.transaction_context(TxType::Write).await.unwrap();
    assert!(
        type_bridge_orm::_manager::DynamicEntityManager::with_canonical_transaction(
            tx.clone(),
            descriptor
        )
        .delete_many_by_iid_exact_returning(&["0x10".into()])
        .await
        .is_err()
    );
    assert!(tx.commit().await.is_err());
    tx.rollback().await.unwrap();
    let state = state.lock().unwrap();
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 1);
}

#[tokio::test]
async fn returning_delete_hydrates_entity_and_relation_snapshots_and_absence() {
    let installed = installed();
    let person = type_id(TypeKind::Entity, "person");
    let membership = type_id(TypeKind::Relation, "membership");
    let (mut backend, state) = RecordingBackend::new(
        vec![
            documents(vec![person_document("0x10")]),
            RecordingResponse::Result(QueryResult::Ok),
            documents(vec![
                serde_json::json!({"_iid":"0x20", "_type":"membership", "attributes":{}, "_role_0_players":[person_document("0x10")]}),
            ]),
            RecordingResponse::Result(QueryResult::Ok),
            documents(vec![]),
            RecordingResponse::Result(QueryResult::Ok),
        ],
        CommitBehavior::Success,
    );
    backend.given_enabled = true;
    let database = Database::with_backend(Box::new(backend), "test");
    let executor = ProjectedCrudExecutor::new(&installed);
    let deleted = executor
        .delete_entity_by_iid_returning(&database, &person, "0x10")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(deleted.iid(), "0x10");
    assert!(deleted.is_deleted());
    assert_eq!(
        deleted
            .try_to_reference(&installed)
            .unwrap_err()
            .code()
            .as_str(),
        "thing_deleted"
    );
    let deleted = executor
        .delete_relation_by_iid_returning(&database, &membership, "0x20")
        .await
        .unwrap()
        .unwrap();
    assert!(deleted.is_deleted());
    let player = &deleted.roles()[&RoleId::new("membership", "member").unwrap()][0];
    assert_eq!(player.iid(), "0x10");
    assert!(
        executor
            .delete_entity_by_iid_returning(&database, &person, "0x10")
            .await
            .unwrap()
            .is_none()
    );
    let state = state.lock().unwrap();
    assert_eq!(state.commits, 3);
    assert_eq!(state.rollbacks, 0);
    assert_eq!(state.queries.len(), 6);
    for pair in state.queries.as_chunks::<2>().0 {
        assert!(pair[0].contains("fetch"));
        assert!(!pair[0].contains("delete "));
        assert!(pair[1].contains("delete "));
    }
}

#[tokio::test]
async fn returning_delete_rolls_back_when_snapshot_hydration_fails() {
    let installed = installed();
    let (mut backend, state) = RecordingBackend::new(
        vec![
            documents(vec![
                serde_json::json!({"_iid":"0x10", "_type":"person", "attributes":{}}),
            ]),
            RecordingResponse::Result(QueryResult::Ok),
        ],
        CommitBehavior::Success,
    );
    backend.given_enabled = true;
    let database = Database::with_backend(Box::new(backend), "test");
    assert!(
        ProjectedCrudExecutor::new(&installed)
            .delete_entity_by_iid_returning(&database, &type_id(TypeKind::Entity, "person"), "0x10")
            .await
            .is_err()
    );
    let state = state.lock().unwrap();
    assert_eq!(state.queries.len(), 1);
    assert!(state.queries[0].contains("fetch"));
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 1);
}

#[tokio::test]
async fn delete_is_one_blind_exact_mutation_and_absent_is_success() {
    let installed = installed();
    let person = type_id(TypeKind::Entity, "person");
    let (backend, state) = RecordingBackend::new(
        // A bare acknowledgement is success, like the released blind delete.
        vec![RecordingResponse::Result(QueryResult::Ok)],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");

    ProjectedCrudExecutor::new(&installed)
        .delete_entity_by_iid(&database, &person, "0x10")
        .await
        .unwrap();

    let state = state.lock().unwrap();
    assert_eq!(state.queries.len(), 1);
    assert!(state.queries[0].contains("isa! person"));
    assert!(state.queries[0].contains("delete"));
    assert_eq!(state.commits, 1);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn owned_reads_close_and_preserve_exact_get_and_count_behavior() {
    let installed = installed();
    let person = type_id(TypeKind::Entity, "person");
    let (backend, state) = RecordingBackend::new(
        vec![
            documents(vec![person_document("0x10")]),
            RecordingResponse::Result(QueryResult::Rows(vec![
                serde_json::json!({"count1": {"kind":"value", "value":CanonicalValue::Long(3)}}),
            ])),
        ],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");
    let executor = ProjectedCrudExecutor::new(&installed);

    let thing = executor
        .get_entity_by_iid(&database, &person, "0x10")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(thing.iid(), "0x10");
    assert_eq!(
        executor.count_entities(&database, &person).await.unwrap(),
        3
    );

    let state = state.lock().unwrap();
    assert_eq!(state.opens, [TxType::Read, TxType::Read]);
    assert_eq!(state.closes, 2);
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 0);
    assert_eq!(state.query_modes, ["canonical", "canonical"]);
    assert!(
        state
            .queries
            .iter()
            .all(|query| query.contains("isa! person"))
    );
}

#[tokio::test]
async fn classified_commit_outcomes_are_exact_and_provider_text_is_redacted() {
    const SECRET: &str = "provider-secret-endpoint-password";
    for (certainty, expected_code) in [
        (
            CommitFailureCertainty::DefinitelyAborted,
            "commit_definitely_aborted",
        ),
        (CommitFailureCertainty::Unknown, "commit_outcome_unknown"),
    ] {
        let installed = installed();
        let create = person_create(&installed);
        let (backend, state) = RecordingBackend::new(
            vec![documents(vec![person_document("0x10")])],
            CommitBehavior::Failure(certainty, SECRET),
        );
        let database = Database::with_backend(Box::new(backend), "test");

        let diagnostic = ProjectedCrudExecutor::new(&installed)
            .insert_entity(&database, &create)
            .await
            .unwrap_err();

        assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Transaction);
        assert_eq!(diagnostic.code().as_str(), expected_code);
        assert!(!format!("{diagnostic:?} {diagnostic}").contains(SECRET));
        let state = state.lock().unwrap();
        assert_eq!(state.commits, 1);
        assert_eq!(state.rollbacks, 0);
        assert_eq!(state.closes, 0);
    }
}

#[tokio::test]
async fn provider_query_text_is_never_copied_into_execution_diagnostics() {
    const SECRET: &str = "query-provider-secret-and-path";
    let installed = installed();
    let create = person_create(&installed);
    let (backend, state) = RecordingBackend::new(
        vec![RecordingResponse::Error(SECRET)],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");

    let diagnostic = ProjectedCrudExecutor::new(&installed)
        .insert_entity(&database, &create)
        .await
        .unwrap_err();

    assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Provider);
    assert!(!format!("{diagnostic:?} {diagnostic}").contains(SECRET));
    assert_eq!(state.lock().unwrap().rollbacks, 1);
}

const POLYMORPHIC_SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
entities:
  party:
    abstract: true
    owns:
      identifier: { key: true }
  person:
    sub: party
  company:
    sub: party
relations:
  membership:
    relates:
      member: { card: { min: 1 } }
plays:
  party:
    membership: [member]
"#;

fn installed_polymorphic() -> InstalledRuntimeProjection {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("projected-crud-polymorphic.yaml").unwrap(),
        POLYMORPHIC_SCHEMA,
    )])
    .unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
    let projection = project(
        &resolved,
        BindingTarget::Python,
        &ProjectionConfig::python(),
        &[ProjectionHandler::python_v1()],
        &[],
    )
    .unwrap();
    InstalledRuntimeProjection::try_new(projection).unwrap()
}

/// A membership whose member is the abstract `party` named by its key.
fn party_key_membership(installed: &InstalledRuntimeProjection, key: &str) -> ProjectedCreate {
    let party = type_id(TypeKind::Entity, "party");
    let reference = ProjectedReference::try_new(
        installed,
        party.clone(),
        None,
        vec![(
            field(&party, "identifier"),
            string_value(installed, "identifier", key),
        )],
    )
    .unwrap();
    membership_create_with_reference(installed, reference)
}

#[tokio::test]
async fn polymorphic_key_reference_is_one_disjunctive_write_without_a_lookup() {
    let installed = installed_polymorphic();
    let create = party_key_membership(&installed, "acme");
    let (backend, state) = RecordingBackend::new(
        vec![RecordingResponse::Error("stop after the write")],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");

    let _ = ProjectedCrudExecutor::new(&installed)
        .insert_relation(&database, &create)
        .await;

    let state = state.lock().unwrap();
    // One query: the write resolves the reference and fetches the result.
    let write = &state.queries[0];
    assert_eq!(state.queries.len(), 1, "{:?}", state.queries);
    assert!(write.starts_with("match"), "{write}");
    assert!(write.contains("$player1 isa! company") && write.contains("$player1 isa! person"));
    assert!(write.contains(" or "), "{write}");
    assert!(write.contains("insert $relation0 isa membership, links (member: $player1)"));
    assert!(write.contains("fetch"), "{write}");
}

#[tokio::test]
async fn missing_reference_writes_nothing_and_keeps_the_released_diagnostic() {
    let installed = installed_polymorphic();
    let create = party_key_membership(&installed, "absent");
    let (backend, state) = RecordingBackend::new(vec![documents(vec![])], CommitBehavior::Success);
    let database = Database::with_backend(Box::new(backend), "test");

    let diagnostic = ProjectedCrudExecutor::new(&installed)
        .insert_relation(&database, &create)
        .await
        .unwrap_err();

    assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Integrity);
    assert_eq!(
        diagnostic.code().as_str(),
        "provider_identity_cardinality_invalid"
    );
    let state = state.lock().unwrap();
    assert_eq!(state.queries.len(), 1);
    assert!(state.queries[0].contains("insert"));
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 1);
}

#[tokio::test]
async fn a_reference_matching_several_things_poisons_the_borrowed_transaction() {
    let installed = installed_polymorphic();
    let create = party_key_membership(&installed, "shared");
    let (backend, state) = RecordingBackend::new(
        vec![documents(
            ["0x20", "0x21"]
                .map(|iid| {
                    let mut document = membership_document(iid);
                    document["_role_0_attributes"]
                        .as_object_mut()
                        .unwrap()
                        .remove("email");
                    document
                })
                .to_vec(),
        )],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");
    let transaction = database.transaction_context(TxType::Write).await.unwrap();

    let diagnostic = ProjectedCrudExecutor::new(&installed)
        .insert_relation_in_transaction(&transaction, &create)
        .await
        .unwrap_err();

    assert_eq!(
        diagnostic.code().as_str(),
        "provider_identity_cardinality_invalid"
    );
    assert_eq!(
        transaction.lifecycle_state().await,
        type_bridge_orm::session::TransactionContextState::RollbackOnly
    );
    assert_eq!(state.lock().unwrap().queries.len(), 1);
    transaction.rollback().await.unwrap();
}

#[tokio::test]
async fn two_references_resolving_to_one_player_are_refused_and_poison_the_transaction() {
    let installed = installed_polymorphic();
    let party = type_id(TypeKind::Entity, "party");
    let by_iid =
        ProjectedReference::try_new(&installed, party.clone(), Some("0x10".into()), vec![])
            .unwrap();
    let by_key = ProjectedReference::try_new(
        &installed,
        party.clone(),
        None,
        vec![(
            field(&party, "identifier"),
            string_value(&installed, "identifier", "acme"),
        )],
    )
    .unwrap();
    let create = ProjectedCreate::try_new(
        &installed,
        type_id(TypeKind::Relation, "membership"),
        vec![],
        vec![(
            RoleId::new("membership", "member").unwrap(),
            vec![by_iid, by_key],
        )],
    )
    .unwrap();
    let (backend, state) = RecordingBackend::new(
        vec![documents(vec![serde_json::json!({
            "_iid": "0x20",
            "_type": "membership",
            "attributes": {},
            "_role_0_iid": "0x10",
            "_role_0_type": "person",
            "_role_0_attributes": {"identifier": "ada"}
        })])],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");
    let transaction = database.transaction_context(TxType::Write).await.unwrap();

    let diagnostic = ProjectedCrudExecutor::new(&installed)
        .insert_relation_in_transaction(&transaction, &create)
        .await
        .unwrap_err();

    assert_eq!(diagnostic.code().as_str(), "duplicate_resolved_role_player");
    assert_eq!(
        transaction.lifecycle_state().await,
        type_bridge_orm::session::TransactionContextState::RollbackOnly
    );
    let queries = state.lock().unwrap().queries.clone();
    assert_eq!(queries.len(), 1);
    assert!(queries[0].contains("_role_0_iid"), "{}", queries[0]);
    transaction.rollback().await.unwrap();
}

#[tokio::test]
async fn a_failed_hydration_after_a_put_poisons_the_borrowed_transaction() {
    let installed = installed();
    let create = person_create(&installed);
    let (backend, _state) = RecordingBackend::new(
        vec![documents(vec![serde_json::json!({
            "_iid": "0x30",
            "_type": "actor",
            "attributes": {}
        })])],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");
    let transaction = database.transaction_context(TxType::Write).await.unwrap();

    let diagnostic = ProjectedCrudExecutor::new(&installed)
        .put_entity_in_transaction(&transaction, &create)
        .await
        .unwrap_err();

    assert_eq!(diagnostic.code().as_str(), "query_remote_v3_document_type");
    assert_eq!(
        transaction.lifecycle_state().await,
        type_bridge_orm::session::TransactionContextState::RollbackOnly
    );
    transaction.rollback().await.unwrap();
}

#[tokio::test]
async fn two_distinct_players_in_one_role_hydrate_from_several_documents() {
    let installed = installed_polymorphic();
    let party = type_id(TypeKind::Entity, "party");
    let by_iid =
        ProjectedReference::try_new(&installed, party.clone(), Some("0x10".into()), vec![])
            .unwrap();
    let by_key = ProjectedReference::try_new(
        &installed,
        party.clone(),
        None,
        vec![(
            field(&party, "identifier"),
            string_value(&installed, "identifier", "acme"),
        )],
    )
    .unwrap();
    let member = RoleId::new("membership", "member").unwrap();
    let create = ProjectedCreate::try_new(
        &installed,
        type_id(TypeKind::Relation, "membership"),
        vec![],
        vec![(member.clone(), vec![by_iid, by_key])],
    )
    .unwrap();
    let document = |player: &str, player_type: &str, key: &str| {
        serde_json::json!({
            "_iid": "0x20",
            "_type": "membership",
            "attributes": {},
            "_role_0_iid": player,
            "_role_0_type": player_type,
            "_role_0_attributes": {"identifier": key}
        })
    };
    let (backend, state) = RecordingBackend::new(
        vec![documents(vec![
            document("0x10", "person", "ada"),
            document("0x11", "company", "acme"),
        ])],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");

    let relation = ProjectedCrudExecutor::new(&installed)
        .insert_relation(&database, &create)
        .await
        .unwrap();

    let mut players: Vec<_> = relation.roles()[&member]
        .iter()
        .map(|player| player.iid().to_owned())
        .collect();
    players.sort();
    assert_eq!(players, ["0x10", "0x11"]);
    let state = state.lock().unwrap();
    assert_eq!(state.queries.len(), 1);
    assert_eq!(state.commits, 1);
}

#[tokio::test]
async fn relation_update_without_players_refuses_before_provider_work() {
    let documents_set = SchemaDocumentSet::parse([(
        DocumentId::new("projected-crud-optional-role.yaml").unwrap(),
        r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
entities:
  person:
    owns:
      identifier: { key: true }
relations:
  tagging:
    relates:
      target: { card: { min: 0, max: 2 } }
plays:
  person:
    tagging: [target]
"#,
    )])
    .unwrap();
    let resolved = resolve(
        &normalize_documents(&documents_set).unwrap(),
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
    let installed = InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &[ProjectionHandler::python_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    let diagnostic = ProjectedCreate::try_new(
        &installed,
        type_id(TypeKind::Relation, "tagging"),
        vec![],
        vec![],
    )
    .unwrap_err();
    assert_eq!(diagnostic.category(), SdkDiagnosticCategory::InvalidInput);
    assert_eq!(diagnostic.code().as_str(), "relation_requires_player");
    assert_eq!(
        diagnostic.path(),
        &[
            type_bridge_contract::sdk_diagnostic::SdkDiagnosticPathSegment::Type(type_id(
                TypeKind::Relation,
                "tagging"
            ))
        ]
    );
}

#[tokio::test]
async fn relation_update_matching_nothing_keeps_the_released_codes() {
    let installed = installed();
    let create = membership_create(&installed);
    for (probe, expected) in [
        (documents(vec![]), "mutation_rehydration_missing"),
        (
            documents(vec![membership_document("0x20")]),
            "provider_hydration_failed",
        ),
    ] {
        let (backend, state) =
            RecordingBackend::new(vec![documents(vec![]), probe], CommitBehavior::Success);
        let database = Database::with_backend(Box::new(backend), "test");
        let diagnostic = ProjectedCrudExecutor::new(&installed)
            .update_relation(&database, "0x20", &create)
            .await
            .unwrap_err();
        assert_eq!(diagnostic.code().as_str(), expected);
        let state = state.lock().unwrap();
        assert_eq!(state.queries.len(), 2);
        assert_eq!(state.commits, 0);
    }
}

fn installed_with_authority() -> InstalledRuntimeProjection {
    let declared = normalize_documents(
        &SchemaDocumentSet::parse([(DocumentId::new("manager-authority.yaml").unwrap(), SCHEMA)])
            .unwrap(),
    )
    .unwrap();
    let context = type_bridge_schema::ManagedDeltaContext::new(
        type_bridge_contract::managed_scope::ManagedScopeId::new("manager-test").unwrap(),
        SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
        type_bridge_schema::schema_authority_capability_vocabulary(),
    );
    let authority = type_bridge_schema::build_schema_authority(
        &declared,
        declared.required_capabilities(),
        &context,
    )
    .unwrap();
    installed().with_schema_authority(&authority)
}

#[derive(Debug, PartialEq)]
struct NativePerson {
    iid: Option<String>,
    identifier: String,
    email: String,
}
impl type_bridge_orm::_entity::TypeBridgeEntity for NativePerson {
    const TYPE_NAME: &'static str = "person";
    fn owned_attributes() -> &'static [type_bridge_orm::_entity::OwnedAttributeInfo] {
        &[]
    }
    fn iid(&self) -> Option<&str> {
        self.iid.as_deref()
    }
    fn set_iid(&mut self, iid: String) {
        self.iid = Some(iid);
    }
    fn to_attribute_values(&self) -> Vec<(&'static str, type_bridge_orm::AttributeValue)> {
        vec![]
    }
    fn from_document(doc: &serde_json::Map<String, serde_json::Value>) -> Result<Self, OrmError> {
        let identifier = doc["identifier"].as_str().unwrap().to_owned();
        let email = doc["email"].as_str().unwrap().to_owned();
        if email == "reject@example.test" {
            return Err(OrmError::Hydration {
                type_name: "person".into(),
                message: "handwritten materializer rejected input".into(),
            });
        }
        Ok(Self {
            iid: None,
            identifier,
            email,
        })
    }
}

#[tokio::test]
async fn handwritten_and_projected_returning_delete_share_queries_and_snapshots() {
    let installed = installed_with_authority();
    let registry = Arc::new(installed.match_registry().unwrap());
    let make_backend = || {
        let (mut backend, state) = RecordingBackend::new(
            vec![
                documents(vec![person_document("0x10")]),
                RecordingResponse::Result(QueryResult::Ok),
            ],
            CommitBehavior::Success,
        );
        backend.given_enabled = true;
        (backend, state)
    };
    let (backend, handwritten_state) = make_backend();
    let handwritten_database = Database::with_backend(Box::new(backend), "test");
    let manager = type_bridge_orm::_manager::EntityManager::<NativePerson>::from_registry(
        &handwritten_database,
        registry,
    )
    .unwrap();
    let snapshot = manager
        .delete_returning(&NativePerson {
            iid: Some("0x10".into()),
            identifier: String::new(),
            email: String::new(),
        })
        .await
        .unwrap()
        .unwrap();
    let (backend, projected_state) = make_backend();
    let projected_database = Database::with_backend(Box::new(backend), "test");
    let projected = ProjectedCrudExecutor::new(&installed)
        .delete_entity_by_iid_returning(
            &projected_database,
            &type_id(TypeKind::Entity, "person"),
            "0x10",
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.iid.as_deref(), Some(projected.iid()));
    let person = type_id(TypeKind::Entity, "person");
    for (name, actual) in [
        ("identifier", snapshot.identifier),
        ("email", snapshot.email),
    ] {
        assert_eq!(
            projected.fields()[&field(&person, name)][0].value(),
            &CanonicalValue::String(CanonicalString::new(actual).unwrap())
        );
    }
    let handwritten = handwritten_state.lock().unwrap();
    let projected = projected_state.lock().unwrap();
    assert_eq!(handwritten.queries, projected.queries);
    assert_eq!(
        serde_json::to_vec(&handwritten.given_rows).unwrap(),
        serde_json::to_vec(&projected.given_rows).unwrap()
    );
    assert_eq!(handwritten.queries.len(), 2);
    assert_eq!((handwritten.commits, handwritten.rollbacks), (1, 0));
    assert_eq!((projected.commits, projected.rollbacks), (1, 0));
}

#[tokio::test]
async fn handwritten_returning_materialization_failure_rolls_back_the_deletion() {
    let installed = installed_with_authority();
    let mut doc = person_document("0x10");
    doc["attributes"]["email"] = serde_json::json!("reject@example.test");
    let (mut backend, state) = RecordingBackend::new(
        vec![
            documents(vec![doc]),
            RecordingResponse::Result(QueryResult::Ok),
        ],
        CommitBehavior::Success,
    );
    backend.given_enabled = true;
    let database = Database::with_backend(Box::new(backend), "test");
    let manager = type_bridge_orm::_manager::EntityManager::<NativePerson>::from_registry(
        &database,
        Arc::new(installed.match_registry().unwrap()),
    )
    .unwrap();
    let error = manager
        .delete_returning(&NativePerson {
            iid: Some("0x10".into()),
            identifier: String::new(),
            email: String::new(),
        })
        .await
        .unwrap_err();
    assert!(matches!(error, OrmError::Hydration { .. }), "{error:?}");
    let state = state.lock().unwrap();
    assert_eq!(state.queries.len(), 2);
    assert_eq!((state.commits, state.rollbacks), (0, 1));
}

#[derive(Debug, PartialEq)]
struct NativeMembership {
    iid: Option<String>,
    players: Vec<String>,
}
impl type_bridge_orm::_relation::TypeBridgeRelation for NativeMembership {
    const TYPE_NAME: &'static str = "membership";
    fn owned_attributes() -> &'static [type_bridge_orm::_entity::OwnedAttributeInfo] {
        &[]
    }
    fn role_info() -> &'static [type_bridge_orm::_relation::RoleInfo] {
        &[]
    }
    fn iid(&self) -> Option<&str> {
        self.iid.as_deref()
    }
    fn set_iid(&mut self, iid: String) {
        self.iid = Some(iid);
    }
    fn to_attribute_values(&self) -> Vec<(&'static str, type_bridge_orm::AttributeValue)> {
        vec![]
    }
    fn to_role_player_refs(&self) -> Vec<type_bridge_orm::_relation::RolePlayerRef> {
        vec![]
    }
    fn from_document(doc: &serde_json::Map<String, serde_json::Value>) -> Result<Self, OrmError> {
        Ok(Self {
            iid: None,
            players: doc["role_players"]
                .as_array()
                .unwrap()
                .iter()
                .map(|player| player["player_iid"].as_str().unwrap().to_owned())
                .collect(),
        })
    }
}

#[tokio::test]
async fn handwritten_and_projected_relations_share_returning_deletion() {
    let installed = installed_with_authority();
    let make_backend = || {
        let (mut backend, state) = RecordingBackend::new(
            vec![
                documents(vec![
                    serde_json::json!({"_iid":"0x20", "_type":"membership", "attributes":{}, "_role_0_players":[person_document("0x10")]}),
                ]),
                RecordingResponse::Result(QueryResult::Ok),
            ],
            CommitBehavior::Success,
        );
        backend.given_enabled = true;
        (backend, state)
    };
    let (backend, handwritten_state) = make_backend();
    let database = Database::with_backend(Box::new(backend), "test");
    let manager = type_bridge_orm::_manager::RelationManager::<NativeMembership>::from_registry(
        &database,
        Arc::new(installed.match_registry().unwrap()),
    )
    .unwrap();
    let snapshot = manager
        .delete_returning(&NativeMembership {
            iid: Some("0x20".into()),
            players: vec![],
        })
        .await
        .unwrap()
        .unwrap();
    let (backend, projected_state) = make_backend();
    let projected_db = Database::with_backend(Box::new(backend), "test");
    let projected = ProjectedCrudExecutor::new(&installed)
        .delete_relation_by_iid_returning(
            &projected_db,
            &type_id(TypeKind::Relation, "membership"),
            "0x20",
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.iid.as_deref(), Some(projected.iid()));
    assert_eq!(
        snapshot.players,
        projected.roles()[&RoleId::new("membership", "member").unwrap()]
            .iter()
            .map(|player| player.iid().to_owned())
            .collect::<Vec<_>>()
    );
    let handwritten = handwritten_state.lock().unwrap();
    let projected = projected_state.lock().unwrap();
    assert_eq!(handwritten.queries, projected.queries);
    assert_eq!(handwritten.queries.len(), 2);
    assert_eq!(
        serde_json::to_vec(&handwritten.given_rows).unwrap(),
        serde_json::to_vec(&projected.given_rows).unwrap()
    );
    assert_eq!((handwritten.commits, handwritten.rollbacks), (1, 0));
    assert_eq!((projected.commits, projected.rollbacks), (1, 0));
}

struct CommitHook {
    state: Arc<Mutex<RecordingState>>,
    events: Arc<Mutex<Vec<&'static str>>>,
}
impl type_bridge_orm::hooks::LifecycleHook for CommitHook {
    fn name(&self) -> &str {
        "returning-commit-order"
    }
    fn before_operation<'a>(
        &'a self,
        _: &'a mut type_bridge_orm::hooks::HookContext,
    ) -> BoxFuture<
        'a,
        Result<type_bridge_orm::hooks::PreHookResult, type_bridge_orm::hooks::HookError>,
    > {
        Box::pin(async move {
            assert_eq!(self.state.lock().unwrap().commits, 0);
            self.events.lock().unwrap().push("pre");
            Ok(type_bridge_orm::hooks::PreHookResult::Continue)
        })
    }
    fn after_operation<'a>(
        &'a self,
        _: &'a type_bridge_orm::hooks::HookContext,
    ) -> BoxFuture<'a, Result<(), type_bridge_orm::hooks::HookError>> {
        Box::pin(async move {
            assert_eq!(self.state.lock().unwrap().commits, 1);
            self.events.lock().unwrap().push("post");
            Ok(())
        })
    }
}

#[tokio::test]
async fn handwritten_returning_hooks_publish_only_after_successful_commit() {
    for fail_commit in [false, true] {
        let installed = installed_with_authority();
        let (mut backend, state) = RecordingBackend::new(
            vec![
                documents(vec![person_document("0x10")]),
                RecordingResponse::Result(QueryResult::Ok),
            ],
            if fail_commit {
                CommitBehavior::Failure(
                    CommitFailureCertainty::DefinitelyAborted,
                    "provider-secret",
                )
            } else {
                CommitBehavior::Success
            },
        );
        backend.given_enabled = true;
        let database = Database::with_backend(Box::new(backend), "test");
        let mut manager = type_bridge_orm::_manager::EntityManager::<NativePerson>::from_registry(
            &database,
            Arc::new(installed.match_registry().unwrap()),
        )
        .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        manager.add_hook(Arc::new(CommitHook {
            state: Arc::clone(&state),
            events: Arc::clone(&events),
        }));
        let result = manager
            .delete_returning(&NativePerson {
                iid: Some("0x10".into()),
                identifier: String::new(),
                email: String::new(),
            })
            .await;
        if fail_commit {
            let OrmError::ExecutionDiagnostic(error) = result.unwrap_err() else {
                panic!("classified redacted refusal");
            };
            assert_eq!(error.code().as_str(), "commit_definitely_aborted");
            assert!(!error.to_string().contains("provider-secret"));
            assert_eq!(*events.lock().unwrap(), ["pre"]);
        } else {
            assert!(result.unwrap().is_some());
            assert_eq!(*events.lock().unwrap(), ["pre", "post"]);
        }
        let state = state.lock().unwrap();
        assert_eq!((state.commits, state.rollbacks), (1, 0));
    }
}

#[test]
fn native_manager_future_sizes_stay_bounded() {
    fn size<F, Fut>(_: F) -> usize
    where
        F: FnOnce() -> Fut,
    {
        std::mem::size_of::<Fut>()
    }
    let installed = installed_with_authority();
    let (backend, _) = RecordingBackend::new(vec![], CommitBehavior::Success);
    let db = Database::with_backend(Box::new(backend), "size-only");
    let crud = ProjectedCrudExecutor::new(&installed);
    let person = type_id(TypeKind::Entity, "person");
    let relation = type_id(TypeKind::Relation, "membership");
    let create = person_create(&installed);
    let relation_create = membership_create(&installed);
    for (name, bytes, ceiling) in [
        (
            "entity-get",
            size(|| crud.get_entity_by_iid(&db, &person, "0x10")),
            6480,
        ),
        (
            "entity-count",
            size(|| crud.count_entities(&db, &person)),
            5744,
        ),
        (
            "entity-insert",
            size(|| crud.insert_entity(&db, &create)),
            9368,
        ),
        (
            "entity-returning",
            size(|| crud.delete_entity_by_iid_returning(&db, &person, "0x10")),
            8192,
        ),
        (
            "relation-insert",
            size(|| crud.insert_relation(&db, &relation_create)),
            8192,
        ),
        (
            "relation-get",
            size(|| crud.get_relation_by_iid(&db, &relation, "0x20")),
            8192,
        ),
        (
            "relation-returning",
            size(|| crud.delete_relation_by_iid_returning(&db, &relation, "0x20")),
            8192,
        ),
    ] {
        eprintln!("MANAGER_FUTURE {name} bytes={bytes} ceiling={ceiling}");
        assert!(bytes <= ceiling, "{name}: {bytes} > {ceiling}");
    }
}

#[tokio::test]
async fn default_manager_read_does_not_allocate_an_authored_bulk_payload() {
    let installed = installed_with_authority();
    let (backend, state) = RecordingBackend::new(
        vec![documents(vec![person_document("0x10")])],
        CommitBehavior::Success,
    );
    let db = Database::with_backend(Box::new(backend), "read-allocation");
    let crud = ProjectedCrudExecutor::new(&installed);
    let person = type_id(TypeKind::Entity, "person");
    let bytes = std::mem::size_of::<type_bridge_contract::query_plan::v3::BuiltPlans>();
    let (result, count) =
        allocations::count(bytes, crud.get_entity_by_iid(&db, &person, "0x10")).await;
    assert_eq!(result.unwrap().unwrap().iid(), "0x10");
    assert_eq!(
        count, 0,
        "default manager read allocated a {bytes}-byte bulk payload"
    );
    assert_eq!(state.lock().unwrap().opens, vec![TxType::Read]);
}

#[tokio::test]
async fn single_crud_does_not_allocate_the_batch_execution_future() {
    use type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic;
    use type_bridge_orm::session::backend::AnswerCancellation;
    use type_bridge_orm::{
        ProjectedBatch, ProjectedBatchExecutor, ProjectedBatchInvocationControl,
        ProjectedBatchOperation, ProjectedBatchRow, QueryExecutionResourceLimits,
    };
    let installed = installed_with_authority();
    let create = person_create(&installed);
    let model = create.type_id().clone();
    let batch = ProjectedBatch::try_new(
        &installed,
        model.clone(),
        ProjectedBatchOperation::Insert,
        vec![ProjectedBatchRow::Create(create.clone())],
    )
    .unwrap();
    let (backend, state) = RecordingBackend::new(
        vec![documents(vec![person_document("0x10")])],
        CommitBehavior::Success,
    );
    let db = Database::with_backend(Box::new(backend), "default-allocation");
    let crud = ProjectedCrudExecutor::new(&installed);
    let executor = ProjectedBatchExecutor::new(&installed);
    let mapper_model = model.clone();
    let probe = executor.execute_mapped(
        &db,
        &batch,
        ProjectedBatchInvocationControl::capture(
            QueryExecutionResourceLimits::default(),
            AnswerCancellation::default(),
        ),
        None,
        move |_| {
            drop(mapper_model);
            Ok::<Option<ProjectedThing>, SdkExecutionDiagnostic>(None)
        },
    );
    let bytes = std::mem::size_of_val(&probe);
    eprintln!("CONTROLLED_FUTURE_ALLOCATION bytes={bytes}");
    drop(probe);
    let (result, count) = allocations::count(bytes, crud.insert_entity(&db, &create)).await;
    assert_eq!(result.unwrap().iid(), "0x10");
    assert_eq!(
        count, 0,
        "default call allocated the {bytes}-byte controlled execution future"
    );
    assert_eq!(state.lock().unwrap().commits, 1);

    let (mut backend, state) = RecordingBackend::new(
        vec![RecordingResponse::Error("allocation probe")],
        CommitBehavior::Success,
    );
    backend.given_enabled = true;
    let db = Database::with_backend(Box::new(backend), "controlled-allocation");
    let (result, count) = allocations::count(
        bytes,
        crud.insert_entity_controlled(
            &db,
            &create,
            QueryExecutionResourceLimits::default(),
            AnswerCancellation::default(),
        ),
    )
    .await;
    assert_eq!(
        result.unwrap_err().code().as_str(),
        "provider_operation_failed"
    );
    assert_eq!(
        count, 0,
        "single-plan control must not allocate the old {bytes}-byte batch execution future"
    );
    assert_eq!(state.lock().unwrap().rollbacks, 1);
}

#[tokio::test]
async fn registry_manager_single_and_bulk_commits_keep_certainty_and_redact_provider_text() {
    use type_bridge_orm::_dynamic::DynamicRolePlayerInput;
    use type_bridge_orm::_manager::{DynamicEntityManager, DynamicRelationManager};
    use type_bridge_orm::AttributeValue;
    let installed = installed_with_authority();
    let registry = Arc::new(installed.match_registry().unwrap());
    for (certainty, code) in [
        (
            CommitFailureCertainty::DefinitelyAborted,
            "commit_definitely_aborted",
        ),
        (CommitFailureCertainty::Unknown, "commit_outcome_unknown"),
    ] {
        for (relation, bulk) in [(false, false), (false, true), (true, false), (true, true)] {
            let label = if relation { "membership" } else { "person" };
            let kind = if relation { "relation" } else { "entity" };
            let responses = if bulk {
                let rows = (0..2)
                    .map(|index| {
                        let iid = if index == 0 { "0x10" } else { "0x11" };
                        let mut document = if relation {
                            membership_document(iid)
                        } else {
                            person_document(iid)
                        };
                        document["ordinal"] = serde_json::json!(index);
                        if relation {
                            document["_role_0_iid"] = serde_json::json!("0x01");
                        }
                        document
                    })
                    .collect();
                vec![documents(rows)]
            } else {
                vec![RecordingResponse::Result(QueryResult::Rows(vec![
                    serde_json::json!({"root0":{"kind":"thing","iid":"0x10","type_id":{"kind":kind,"label":label}}}),
                ]))]
            };
            let (backend, state) = RecordingBackend::new(
                responses,
                CommitBehavior::Failure(certainty, "provider-secret"),
            );
            let db = Database::with_backend(Box::new(backend), "native-commit");
            let result = if relation {
                let manager = DynamicRelationManager::from_registry(
                    &db,
                    Arc::clone(&registry),
                    registry.relation("membership").unwrap(),
                );
                let players = vec![DynamicRolePlayerInput {
                    role_name: "member".into(),
                    player_type_name: "person".into(),
                    iid: Some("0x01".into()),
                    key: None,
                }];
                if bulk {
                    manager
                        .insert_many(&[(vec![], players.clone()), (vec![], players)])
                        .await
                        .map(|_| ())
                } else {
                    manager.insert(&vec![], &players).await.map(|_| ())
                }
            } else {
                let manager = DynamicEntityManager::from_registry(
                    &db,
                    Arc::clone(&registry),
                    registry.entity("person").unwrap(),
                );
                let attributes = vec![
                    ("identifier".into(), AttributeValue::String("ada".into())),
                    (
                        "email".into(),
                        AttributeValue::String("ada@example.test".into()),
                    ),
                ];
                if bulk {
                    let mut second = attributes.clone();
                    second.push(("nickname".into(), AttributeValue::String("Ada".into())));
                    manager.insert_many(&[attributes, second]).await.map(|_| ())
                } else {
                    manager.insert(&attributes).await.map(|_| ())
                }
            };
            let OrmError::ExecutionDiagnostic(error) = result.unwrap_err() else {
                panic!("classified native manager commit");
            };
            use type_bridge_contract::sdk_diagnostic::{
                SdkCommitFailureOutcome, SdkDiagnosticCategory, SdkDiagnosticDetailValue,
            };
            assert_eq!(error.category(), SdkDiagnosticCategory::Transaction);
            let outcome = match certainty {
                CommitFailureCertainty::DefinitelyAborted => {
                    SdkCommitFailureOutcome::DefinitelyAborted
                }
                CommitFailureCertainty::Unknown => SdkCommitFailureOutcome::Unknown,
            };
            assert!(
                error
                    .details()
                    .values()
                    .any(|value| value == &SdkDiagnosticDetailValue::CommitOutcome(outcome))
            );
            assert_eq!(
                type_bridge_orm::lower_execution_error(
                    OrmError::ExecutionDiagnostic(error.clone()),
                    type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Write
                ),
                *error
            );
            assert_eq!(error.code().as_str(), code);
            assert!(!error.to_string().contains("provider-secret"));
            let state = state.lock().unwrap();
            assert_eq!((state.commits, state.rollbacks), (1, 0));
        }
    }
}

#[tokio::test]
async fn controlled_single_updates_share_the_uncontrolled_one_query_for_owned_and_borrowed_targets()
{
    use type_bridge_orm::QueryExecutionResourceLimits;
    use type_bridge_orm::session::backend::AnswerCancellation;
    let installed = installed();
    let executor = ProjectedCrudExecutor::new(&installed);
    for relation in [false, true] {
        let input = if relation {
            membership_create(&installed)
        } else {
            person_create(&installed)
        };
        let iid = if relation { "0x20" } else { "0x10" };
        let mut baseline = None;
        for controlled in [false, true] {
            for borrowed in [false, true] {
                let document = if relation {
                    membership_document(iid)
                } else {
                    person_document(iid)
                };
                let (mut backend, state) =
                    RecordingBackend::new(vec![documents(vec![document])], CommitBehavior::Success);
                backend.given_enabled = true;
                let database = Database::with_backend(Box::new(backend), "single-update");
                let limits = QueryExecutionResourceLimits {
                    statements: Some(1),
                    ..Default::default()
                };
                let cancellation = AnswerCancellation::default();
                let thing = if borrowed {
                    let transaction = database.transaction_context(TxType::Write).await.unwrap();
                    let result = match (relation, controlled) {
                        (false, false) => {
                            executor
                                .update_entity_in_transaction(&transaction, iid, &input)
                                .await
                        }
                        (false, true) => {
                            executor
                                .update_entity_in_transaction_controlled(
                                    &transaction,
                                    iid,
                                    &input,
                                    limits,
                                    cancellation,
                                )
                                .await
                        }
                        (true, false) => {
                            executor
                                .update_relation_in_transaction(&transaction, iid, &input)
                                .await
                        }
                        (true, true) => {
                            executor
                                .update_relation_in_transaction_controlled(
                                    &transaction,
                                    iid,
                                    &input,
                                    limits,
                                    cancellation,
                                )
                                .await
                        }
                    }
                    .unwrap();
                    assert_eq!(state.lock().unwrap().commits, 0);
                    transaction.commit_sdk().await.unwrap();
                    result
                } else {
                    match (relation, controlled) {
                        (false, false) => executor.update_entity(&database, iid, &input).await,
                        (false, true) => {
                            executor
                                .update_entity_controlled(
                                    &database,
                                    iid,
                                    &input,
                                    limits,
                                    cancellation,
                                )
                                .await
                        }
                        (true, false) => executor.update_relation(&database, iid, &input).await,
                        (true, true) => {
                            executor
                                .update_relation_controlled(
                                    &database,
                                    iid,
                                    &input,
                                    limits,
                                    cancellation,
                                )
                                .await
                        }
                    }
                    .unwrap()
                };
                assert_eq!(thing.iid(), iid);
                let state = state.lock().unwrap();
                assert_eq!(
                    state.queries.len(),
                    1,
                    "relation={relation} controlled={controlled} borrowed={borrowed}"
                );
                assert_eq!(state.commits, 1);
                assert_eq!(state.rollbacks, 0);
                let actual = (state.queries.clone(), state.given_rows.clone());
                if let Some(expected) = &baseline {
                    assert_eq!(&actual, expected);
                } else {
                    baseline = Some(actual);
                }
            }
        }
    }
}

#[tokio::test]
async fn controlled_single_inserts_share_the_uncontrolled_one_query_for_owned_and_borrowed_targets()
{
    use type_bridge_orm::QueryExecutionResourceLimits;
    use type_bridge_orm::session::backend::AnswerCancellation;
    let installed = installed();
    let executor = ProjectedCrudExecutor::new(&installed);
    for relation in [false, true] {
        let input = if relation {
            membership_create(&installed)
        } else {
            person_create(&installed)
        };
        let iid = if relation { "0x20" } else { "0x10" };
        let mut baseline = None;
        for controlled in [false, true] {
            for borrowed in [false, true] {
                let document = if relation {
                    membership_document(iid)
                } else {
                    person_document(iid)
                };
                let (mut backend, state) =
                    RecordingBackend::new(vec![documents(vec![document])], CommitBehavior::Success);
                backend.given_enabled = true;
                let database = Database::with_backend(Box::new(backend), "single-insert");
                let limits = QueryExecutionResourceLimits {
                    statements: Some(1),
                    ..Default::default()
                };
                let cancellation = AnswerCancellation::default();
                let thing = if borrowed {
                    let transaction = database.transaction_context(TxType::Write).await.unwrap();
                    let result = match (relation, controlled) {
                        (false, false) => {
                            executor
                                .insert_entity_in_transaction(&transaction, &input)
                                .await
                        }
                        (false, true) => {
                            executor
                                .insert_entity_in_transaction_controlled(
                                    &transaction,
                                    &input,
                                    limits,
                                    cancellation,
                                )
                                .await
                        }
                        (true, false) => {
                            executor
                                .insert_relation_in_transaction(&transaction, &input)
                                .await
                        }
                        (true, true) => {
                            executor
                                .insert_relation_in_transaction_controlled(
                                    &transaction,
                                    &input,
                                    limits,
                                    cancellation,
                                )
                                .await
                        }
                    }
                    .unwrap();
                    assert_eq!(state.lock().unwrap().commits, 0);
                    transaction.commit_sdk().await.unwrap();
                    result
                } else {
                    match (relation, controlled) {
                        (false, false) => executor.insert_entity(&database, &input).await,
                        (false, true) => {
                            executor
                                .insert_entity_controlled(&database, &input, limits, cancellation)
                                .await
                        }
                        (true, false) => executor.insert_relation(&database, &input).await,
                        (true, true) => {
                            executor
                                .insert_relation_controlled(&database, &input, limits, cancellation)
                                .await
                        }
                    }
                    .unwrap()
                };
                assert_eq!(thing.iid(), iid);
                let state = state.lock().unwrap();
                assert_eq!(
                    state.queries.len(),
                    1,
                    "relation={relation} controlled={controlled} borrowed={borrowed}"
                );
                assert_eq!(state.commits, 1);
                assert_eq!(state.rollbacks, 0);
                let actual = (state.queries.clone(), state.given_rows.clone());
                if let Some(expected) = &baseline {
                    assert_eq!(&actual, expected);
                } else {
                    baseline = Some(actual);
                }
            }
        }
    }
}

#[tokio::test]
async fn controlled_single_update_preserves_captured_control_and_poison_after_dispatch() {
    use type_bridge_orm::session::backend::AnswerCancellation;
    use type_bridge_orm::{ProjectedBatchInvocationControl, QueryExecutionResourceLimits};
    let installed = installed();
    let executor = ProjectedCrudExecutor::new(&installed);
    let input = person_create(&installed);
    for expired in [false, true] {
        let cancellation = AnswerCancellation::default();
        let mut limits = QueryExecutionResourceLimits::default();
        if expired {
            limits.timeout_milliseconds = type_bridge_orm::QueryTimeout::from_milliseconds(1);
        } else {
            cancellation.cancel();
        }
        let control = ProjectedBatchInvocationControl::capture(limits, cancellation);
        if expired {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let (mut backend, state) = RecordingBackend::new(vec![], CommitBehavior::Success);
        backend.given_enabled = true;
        let database = Database::with_backend(Box::new(backend), "single-update");
        let failure = executor
            .update_entity_controlled_with_control(&database, "0x10", &input, control)
            .await
            .unwrap_err();
        assert_eq!(
            failure.code().as_str(),
            if expired {
                "transaction_deadline_exceeded"
            } else {
                "provider_cancelled"
            }
        );
        assert!(state.lock().unwrap().opens.is_empty());
    }
    for cancelled in [false, true] {
        for borrowed in [false, true] {
            let cancellation = AnswerCancellation::default();
            let mut limits = QueryExecutionResourceLimits::default();
            let mut document = person_document("0x10");
            let response = if cancelled {
                RecordingResponse::Cancel(
                    cancellation.clone(),
                    QueryResult::Documents(vec![document]),
                )
            } else {
                limits.attribute_values = 2;
                document["attributes"]["nickname"] = serde_json::json!(["extra"]);
                documents(vec![document])
            };
            let (mut backend, state) =
                RecordingBackend::new(vec![response], CommitBehavior::Success);
            backend.given_enabled = true;
            let database = Database::with_backend(Box::new(backend), "single-update");
            let failure = if borrowed {
                let transaction = database.transaction_context(TxType::Write).await.unwrap();
                let failure = executor
                    .update_entity_in_transaction_controlled(
                        &transaction,
                        "0x10",
                        &input,
                        limits,
                        cancellation,
                    )
                    .await
                    .unwrap_err();
                let commit = transaction.commit_sdk().await.unwrap_err();
                assert_eq!(commit.code().as_str(), "transaction_rollback_only");
                transaction.rollback().await.unwrap();
                failure
            } else {
                executor
                    .update_entity_controlled(&database, "0x10", &input, limits, cancellation)
                    .await
                    .unwrap_err()
            };
            if cancelled {
                assert_eq!(failure.code().as_str(), "provider_cancelled");
            } else {
                assert_eq!(
                    failure.category(),
                    SdkDiagnosticCategory::ResourceLimit,
                    "{failure:?}"
                );
            }
            let state = state.lock().unwrap();
            assert_eq!(state.queries.len(), 1);
            assert_eq!(state.commits, 0);
            assert_eq!(state.rollbacks, 1);
        }
    }
}

#[tokio::test]
async fn controlled_relation_update_diagnoses_missing_references_only_after_an_empty_write() {
    use type_bridge_orm::QueryExecutionResourceLimits;
    use type_bridge_orm::session::backend::AnswerCancellation;
    let installed = installed();
    let input = membership_create(&installed);
    for reference_exists in [false, true] {
        for borrowed in [false, true] {
            let references = if reference_exists {
                vec![serde_json::json!({
                    "kind0": {"kind":"value", "value":{"kind":"long", "value":"1"}},
                    "ordinal1": {"kind":"value", "value":{"kind":"long", "value":"0"}},
                    "reference2": {"kind":"value", "value":{"kind":"long", "value":"0"}},
                    "thing3": {"kind":"thing", "iid":"0x10", "type_id":{"kind":"entity", "label":"person"}}
                })]
            } else {
                vec![]
            };
            let (mut backend, state) = RecordingBackend::new(
                vec![
                    documents(vec![]),
                    RecordingResponse::Result(QueryResult::Rows(references)),
                ],
                CommitBehavior::Success,
            );
            backend.given_enabled = true;
            let database = Database::with_backend(Box::new(backend), "single-update");
            let executor = ProjectedCrudExecutor::new(&installed);
            let limits = QueryExecutionResourceLimits::default();
            let cancellation = AnswerCancellation::default();
            let failure = if borrowed {
                let transaction = database.transaction_context(TxType::Write).await.unwrap();
                let failure = executor
                    .update_relation_in_transaction_controlled(
                        &transaction,
                        "0x20",
                        &input,
                        limits,
                        cancellation,
                    )
                    .await
                    .unwrap_err();
                if reference_exists {
                    assert_eq!(
                        transaction.commit_sdk().await.unwrap_err().code().as_str(),
                        "transaction_rollback_only"
                    );
                    transaction.rollback().await.unwrap();
                } else {
                    transaction.commit_sdk().await.unwrap();
                }
                failure
            } else {
                executor
                    .update_relation_controlled(&database, "0x20", &input, limits, cancellation)
                    .await
                    .unwrap_err()
            };
            assert_eq!(
                failure.code().as_str(),
                if reference_exists {
                    "projected_batch_mutation_result_missing"
                } else {
                    "projected_batch_reference_missing"
                },
                "{failure:?}"
            );
            let state = state.lock().unwrap();
            assert_eq!(state.queries.len(), 2);
            assert_eq!(state.commits, usize::from(borrowed && !reference_exists));
            assert_eq!(state.rollbacks, usize::from(!borrowed || reference_exists));
        }
    }
}

#[tokio::test]
async fn single_updates_refuse_a_foreign_iid_before_owned_commit_and_poison_borrowed_targets() {
    use type_bridge_orm::QueryExecutionResourceLimits;
    use type_bridge_orm::session::backend::AnswerCancellation;
    let installed = installed();
    let executor = ProjectedCrudExecutor::new(&installed);
    for relation in [false, true] {
        let input = if relation {
            membership_create(&installed)
        } else {
            person_create(&installed)
        };
        let iid = if relation { "0x20" } else { "0x10" };
        for controlled in [false, true] {
            for borrowed in [false, true] {
                let document = if relation {
                    membership_document("0xff")
                } else {
                    person_document("0xff")
                };
                let (mut backend, state) =
                    RecordingBackend::new(vec![documents(vec![document])], CommitBehavior::Success);
                backend.given_enabled = true;
                let database = Database::with_backend(Box::new(backend), "single-update");
                let limits = QueryExecutionResourceLimits {
                    statements: Some(1),
                    ..Default::default()
                };
                let cancellation = AnswerCancellation::default();
                let failure = if borrowed {
                    let transaction = database.transaction_context(TxType::Write).await.unwrap();
                    let result = match (relation, controlled) {
                        (false, false) => {
                            executor
                                .update_entity_in_transaction(&transaction, iid, &input)
                                .await
                        }
                        (false, true) => {
                            executor
                                .update_entity_in_transaction_controlled(
                                    &transaction,
                                    iid,
                                    &input,
                                    limits,
                                    cancellation,
                                )
                                .await
                        }
                        (true, false) => {
                            executor
                                .update_relation_in_transaction(&transaction, iid, &input)
                                .await
                        }
                        (true, true) => {
                            executor
                                .update_relation_in_transaction_controlled(
                                    &transaction,
                                    iid,
                                    &input,
                                    limits,
                                    cancellation,
                                )
                                .await
                        }
                    }
                    .unwrap_err();
                    assert_eq!(state.lock().unwrap().commits, 0);
                    assert_eq!(
                        transaction.commit_sdk().await.unwrap_err().code().as_str(),
                        "transaction_rollback_only"
                    );
                    transaction.rollback().await.unwrap();
                    result
                } else {
                    match (relation, controlled) {
                        (false, false) => executor.update_entity(&database, iid, &input).await,
                        (false, true) => {
                            executor
                                .update_entity_controlled(
                                    &database,
                                    iid,
                                    &input,
                                    limits,
                                    cancellation,
                                )
                                .await
                        }
                        (true, false) => executor.update_relation(&database, iid, &input).await,
                        (true, true) => {
                            executor
                                .update_relation_controlled(
                                    &database,
                                    iid,
                                    &input,
                                    limits,
                                    cancellation,
                                )
                                .await
                        }
                    }
                    .unwrap_err()
                };
                assert_eq!(
                    failure.code().as_str(),
                    "projected_batch_update_identity_changed"
                );
                let state = state.lock().unwrap();
                assert_eq!(
                    state.queries.len(),
                    1,
                    "relation={relation} controlled={controlled} borrowed={borrowed}"
                );
                assert_eq!(state.commits, 0);
                assert_eq!(state.rollbacks, 1);
            }
        }
    }
}

#[tokio::test]
async fn controlled_relation_update_diagnosis_errors_fail_the_lease_and_respect_statement_ceiling()
{
    use type_bridge_orm::QueryExecutionResourceLimits;
    use type_bridge_orm::session::backend::AnswerCancellation;
    let installed = installed();
    let input = membership_create(&installed);
    for outcome in ["provider", "cancelled", "statement_ceiling"] {
        for borrowed in [false, true] {
            let cancellation = AnswerCancellation::default();
            let mut responses = vec![documents(vec![])];
            let mut limits = QueryExecutionResourceLimits::default();
            match outcome {
                "provider" => responses.push(RecordingResponse::Error("diagnosis-provider-secret")),
                "cancelled" => responses.push(RecordingResponse::Cancel(
                    cancellation.clone(),
                    QueryResult::Rows(vec![]),
                )),
                _ => limits.statements = Some(1),
            }
            let (mut backend, state) = RecordingBackend::new(responses, CommitBehavior::Success);
            backend.given_enabled = true;
            let database = Database::with_backend(Box::new(backend), "single-update");
            let executor = ProjectedCrudExecutor::new(&installed);
            let failure = if borrowed {
                let transaction = database.transaction_context(TxType::Write).await.unwrap();
                let failure = executor
                    .update_relation_in_transaction_controlled(
                        &transaction,
                        "0x20",
                        &input,
                        limits,
                        cancellation,
                    )
                    .await
                    .unwrap_err();
                assert_eq!(
                    transaction.commit_sdk().await.unwrap_err().code().as_str(),
                    "transaction_rollback_only"
                );
                transaction.rollback().await.unwrap();
                failure
            } else {
                executor
                    .update_relation_controlled(&database, "0x20", &input, limits, cancellation)
                    .await
                    .unwrap_err()
            };
            assert_eq!(
                failure.code().as_str(),
                match outcome {
                    "provider" => "provider_operation_failed",
                    "cancelled" => "provider_cancelled",
                    _ => "projected_batch_mutation_result_missing",
                }
            );
            assert!(!failure.to_string().contains("diagnosis-provider-secret"));
            let state = state.lock().unwrap();
            assert_eq!(
                state.queries.len(),
                if outcome == "statement_ceiling" { 1 } else { 2 }
            );
            assert_eq!((state.commits, state.rollbacks), (0, 1));
        }
    }
}
