use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, ProjectionHandler};
use type_bridge_contract::schema::{DocumentId, OwnsFactId};
use type_bridge_contract::sdk_diagnostic::SdkDiagnosticCategory;
use type_bridge_contract::value::{CanonicalString, CanonicalValue};
use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, QueryResult, TransactionOps};
use type_bridge_orm::{
    ClassifiedCommitError, CommitFailureCertainty, Database, DatabaseConnectionAuthority,
    InstalledRuntimeProjection, OrmError, ProjectedAttributeValue, ProjectedCreate,
    ProjectedCrudExecutor, ProjectedReference, ProjectedThing, TxType,
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
  event: {}
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
    commits: usize,
    rollbacks: usize,
    closes: usize,
}

enum RecordingResponse {
    Result(QueryResult),
    Error(&'static str),
}

#[derive(Clone, Copy)]
enum CommitBehavior {
    Success,
    Failure(CommitFailureCertainty, &'static str),
}

struct RecordingBackend {
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
                responses: Arc::new(Mutex::new(responses.into())),
                state: Arc::clone(&state),
                commit,
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
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("projected-crud-ordered.yaml").unwrap(),
        ORDERED_SCHEMA,
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

fn insert_iid(iid: &str) -> RecordingResponse {
    RecordingResponse::Result(QueryResult::Documents(vec![
        serde_json::json!({"iid": iid}),
    ]))
}

fn person_document(iid: &str) -> serde_json::Value {
    serde_json::json!({
        "_iid": iid,
        "_type": "person",
        "attributes": {
            "identifier": [{"value": "ada"}],
            "email": [{"value": "ada@example.test"}]
        }
    })
}

fn membership_document(iid: &str) -> serde_json::Value {
    serde_json::json!({
        "_iid": iid,
        "_type": "membership",
        "_role_0_iid": "0x10",
        "_role_0_type": "person",
        "_role_0_attributes": {
            "identifier": [{"value": "ada"}],
            "email": [{"value": "ada@example.test"}]
        }
    })
}

fn membership_with_relation_player_document(iid: &str) -> serde_json::Value {
    serde_json::json!({
        "_iid": iid,
        "_type": "membership",
        "_role_0_iid": "0x11",
        "_role_0_type": "event",
        "_role_0_attributes": {}
    })
}

fn event_document(iid: &str) -> serde_json::Value {
    serde_json::json!({
        "_iid": iid,
        "_type": "event"
    })
}

fn documents(values: Vec<serde_json::Value>) -> RecordingResponse {
    RecordingResponse::Result(QueryResult::Documents(values))
}

fn ordered_collaboration_document(iids: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "_iid": "0x20",
        "_type": "collaboration",
        "attributes": {},
        "role_players": iids
            .iter()
            .map(|iid| serde_json::json!({
                "role_name": "participant",
                "player_iid": iid,
                "player_type_name": "person",
                "attributes": {"identifier": [{"value": format!("person-{iid}")}]}
            }))
            .collect::<Vec<_>>()
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
async fn ordered_collection_order_survives_write_lowering_and_hydration() {
    let installed = installed_ordered();
    let create = ordered_person_create(&installed, &["analyst", "mathematician"]).unwrap();
    let (backend, state) = RecordingBackend::new(
        vec![
            insert_iid("0x10"),
            documents(vec![serde_json::json!({
                "_iid": "0x10",
                "_type": "person",
                "attributes": {
                    "identifier": [{"value": "data-ada"}],
                    "tag": [{"value": "analyst"}, {"value": "mathematician"}]
                }
            })]),
        ],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");

    let thing = ProjectedCrudExecutor::new(&installed)
        .insert_entity(&database, &create)
        .await
        .unwrap();

    let person = type_id(TypeKind::Entity, "person");
    let tags = thing.fields()[&field(&person, "tag")]
        .iter()
        .map(|value| match value.value() {
            CanonicalValue::String(value) => value.as_str(),
            other => panic!("ordered tag fixture changed domain: {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(tags, ["analyst", "mathematician"]);
    let state = state.lock().unwrap();
    let write = &state.queries[0];
    assert!(write.find("analyst").unwrap() < write.find("mathematician").unwrap());
    assert_eq!(state.opens, [TxType::Write]);
    assert_eq!(state.queries.len(), 2);
    assert_eq!(state.commits, 1);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn ordered_role_order_survives_write_lowering_and_hydration() {
    let installed = installed_ordered();
    let create = ordered_collaboration_create(&installed).unwrap();
    let (backend, state) = RecordingBackend::new(
        vec![
            insert_iid("0x20"),
            documents(vec![ordered_collaboration_document(&["0x10", "0x11"])]),
        ],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");

    let thing = ProjectedCrudExecutor::new(&installed)
        .insert_relation(&database, &create)
        .await
        .unwrap();

    let participant = RoleId::new("collaboration", "participant").unwrap();
    assert_eq!(
        thing.roles()[&participant]
            .iter()
            .map(|player| player.iid())
            .collect::<Vec<_>>(),
        ["0x10", "0x11"],
    );
    let state = state.lock().unwrap();
    let write = &state.queries[0];
    assert!(write.find("iid 0x10").unwrap() < write.find("iid 0x11").unwrap());
    assert_eq!(state.opens, [TxType::Write]);
    assert_eq!(state.queries.len(), 2);
    assert_eq!(state.commits, 1);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn ordered_distinct_provider_duplicates_fail_hydration() {
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
    assert_eq!(diagnostic.code().as_str(), "ordered_distinct_duplicate");
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
            RecordingResponse::Result(QueryResult::Ok),
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
    assert_eq!(state.queries.len(), 2);
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 1);
    assert_eq!(state.closes, 0);
}

#[tokio::test]
async fn hydration_failure_after_insert_rolls_back_and_redacts_provider_shape() {
    let installed = installed();
    let create = person_create(&installed);
    let (backend, state) = RecordingBackend::new(
        vec![
            insert_iid("0x10"),
            documents(vec![serde_json::json!({
                "_iid": "0x10",
                "_type": "person",
                "attributes": {"identifier": [{"value": "ada"}]}
            })]),
        ],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");

    let diagnostic = ProjectedCrudExecutor::new(&installed)
        .insert_entity(&database, &create)
        .await
        .unwrap_err();

    assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Integrity);
    assert_eq!(diagnostic.code().as_str(), "provider_hydration_failed");
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
        vec![
            insert_iid("0x20"),
            documents(vec![
                membership_document("0x20"),
                membership_document("0x21"),
            ]),
        ],
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
    assert!(state.queries[1].contains("isa! membership"));
}

#[tokio::test]
async fn abstract_nominal_role_reference_resolves_one_exact_concrete_player() {
    let installed = installed();
    let create = membership_create_for_player(&installed, "actor");
    let (backend, state) = RecordingBackend::new(
        vec![
            insert_iid("0x20"),
            documents(vec![membership_document("0x20")]),
        ],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");

    let relation = ProjectedCrudExecutor::new(&installed)
        .insert_relation(&database, &create)
        .await
        .unwrap();

    assert_eq!(relation.iid(), "0x20");
    let state = state.lock().unwrap();
    assert_eq!(state.queries.len(), 2);
    assert!(state.queries[0].contains("isa! person"));
    assert!(!state.queries[0].contains("isa! actor"));
    assert_eq!(state.commits, 1);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn relation_role_player_uses_a_strict_relation_identity_lookup() {
    let installed = installed();
    let create = membership_create_for_relation_player(&installed);
    let (backend, state) = RecordingBackend::new(
        vec![
            documents(vec![serde_json::json!({"iid": "0x11"})]),
            insert_iid("0x20"),
            documents(vec![membership_with_relation_player_document("0x20")]),
        ],
        CommitBehavior::Success,
    );
    let database = Database::with_backend(Box::new(backend), "test");

    let relation = ProjectedCrudExecutor::new(&installed)
        .insert_relation(&database, &create)
        .await
        .unwrap();

    assert_eq!(relation.iid(), "0x20");
    let state = state.lock().unwrap();
    assert_eq!(state.queries.len(), 3);
    assert!(state.queries[0].contains("$p isa! event"));
    assert!(state.queries[0].contains("$p iid 0x11"));
    assert!(!state.queries[0].contains("$p isa event"));
    assert!(state.queries[1].contains("$p0 isa! event"));
    assert!(state.queries[1].contains("$p0 iid 0x11"));
    assert!(!state.queries[1].contains("$p0 isa event"));
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
        vec![
            insert_iid("0x20"),
            documents(vec![membership_document("0x20")]),
        ],
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
        assert_eq!(state.queries.len(), 2);
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
        vec![
            insert_iid("0x20"),
            documents(vec![membership_document("0x20")]),
        ],
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
        assert_eq!(state.queries.len(), 2);
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
        vec![insert_iid("0x10"), documents(vec![person_document("0x10")])],
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
            documents(vec![]),
            insert_iid("0x30"),
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
    assert_eq!(state.queries.len(), 3);
    assert!(state.queries[0].contains("isa! person"));
    assert!(state.queries[1].starts_with("insert"));
    assert!(state.queries[2].contains("iid 0x30"));
    assert_eq!(state.query_modes, ["canonical"; 3]);
    assert_eq!(state.commits, 1);
    assert_eq!(state.rollbacks, 0);
    assert_eq!(state.closes, 0);
}

#[tokio::test]
async fn owned_relation_put_and_update_rehydrate_before_commit() {
    let installed = installed();
    let create = membership_create(&installed);

    let (put_backend, put_state) = RecordingBackend::new(
        vec![
            documents(vec![serde_json::json!({"iid": "0x10"})]),
            insert_iid("0x20"),
            documents(vec![membership_document("0x20")]),
        ],
        CommitBehavior::Success,
    );
    let put_database = Database::with_backend(Box::new(put_backend), "test");
    let executor = ProjectedCrudExecutor::new(&installed);

    let inserted = executor.put_relation(&put_database, &create).await.unwrap();
    assert_eq!(inserted.iid(), "0x20");
    assert_eq!(
        inserted
            .roles()
            .get(&RoleId::new("membership", "member").unwrap())
            .unwrap()
            .len(),
        1
    );
    {
        let state = put_state.lock().unwrap();
        assert_eq!(state.opens, [TxType::Write]);
        assert_eq!(state.queries.len(), 3);
        assert!(state.queries[0].contains("isa! person"));
        assert!(state.queries[1].contains("insert"));
        assert!(state.queries[2].contains("isa! membership"));
        assert_eq!(state.query_modes, ["canonical"; 3]);
        assert_eq!(state.commits, 1);
        assert_eq!(state.rollbacks, 0);
    }

    let (update_backend, update_state) = RecordingBackend::new(
        vec![
            documents(vec![serde_json::json!({"iid": "0x10"})]),
            RecordingResponse::Result(QueryResult::Ok),
            RecordingResponse::Result(QueryResult::Ok),
            documents(vec![membership_document("0x20")]),
        ],
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
    assert_eq!(state.queries.len(), 4);
    assert!(state.queries[0].contains("isa! person"));
    assert!(state.queries[1].contains("delete"));
    assert!(state.queries[2].contains("insert"));
    assert!(state.queries[3].contains("isa! membership"));
    assert_eq!(state.query_modes, ["canonical"; 4]);
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
            RecordingResponse::Result(QueryResult::Rows(vec![serde_json::json!({"$count": 2})])),
            RecordingResponse::Result(QueryResult::Ok),
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
async fn delete_is_one_blind_exact_mutation_and_absent_is_success() {
    let installed = installed();
    let person = type_id(TypeKind::Entity, "person");
    let (backend, state) = RecordingBackend::new(
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
            RecordingResponse::Result(QueryResult::Rows(vec![serde_json::json!({"$count": 3})])),
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
            vec![insert_iid("0x10"), documents(vec![person_document("0x10")])],
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
