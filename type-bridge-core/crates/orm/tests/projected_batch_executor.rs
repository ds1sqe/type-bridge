#[path = "support/native_batch.rs"]
mod native_batch;
mod native_batch_answer_item {
    pub use type_bridge_orm::session::backend::{AnswerItem, QueryResult};
}
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, ProjectionHandler};
use type_bridge_contract::schema::{DocumentId, OwnsFactId};
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCategory, SdkDiagnosticPathSegment, SdkExecutionDiagnostic,
};
use type_bridge_contract::temporal::CanonicalDuration;
use type_bridge_contract::value::{CanonicalString, CanonicalValue};
use type_bridge_core_lib::version::Version;
use type_bridge_orm::session::backend::{
    AnswerConsumer, AnswerControl, BoundedAnswerLimits, BoundedAnswerReader, BoundedAnswerStats,
    BoxFuture, DriverBackend, GivenRowsSpec, QueryResult, TransactionOps,
};
use type_bridge_orm::{
    AnswerCancellation, ClassifiedCommitError, CommitFailureCertainty, Database,
    InstalledRuntimeProjection, OrmError, ProjectedAttributeValue, ProjectedBatch,
    ProjectedBatchExecutor, ProjectedBatchInvocationControl, ProjectedBatchOperation,
    ProjectedBatchResult, ProjectedBatchRow, ProjectedCreate, ProjectedReference,
    QueryExecutionResourceLimits, TransactionContextState, TxType,
};
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};

const SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  tag: { value: string }
  lag: { value: duration }
entities:
  person:
    owns:
      identifier: { key: true }
      tag: { card: { min: 0 } }
      lag: { card: { min: 0 } }
relations:
  membership:
    owns:
      identifier: { key: true }
    relates:
      member: { card: { min: 0, max: 4 } }
plays:
  person:
    membership: [member]
"#;

#[derive(Default)]
struct State {
    opens: Vec<TxType>,
    calls: Vec<(String, GivenRowsSpec)>,
    responses: VecDeque<Response>,
    commits: usize,
    rollbacks: usize,
}

enum Response {
    ReferenceGuard(Vec<Value>),
    Documents(Vec<Value>),
    ProviderAfter(Vec<Value>),
    CancelAfter(Vec<Value>, AnswerCancellation),
    PendingAfter(Vec<Value>),
    Underreported(Vec<Value>),
    Error(&'static str),
}

fn reference_proof_row(
    source: &str,
    documents: &[Value],
) -> type_bridge_orm::session::backend::AnswerItem {
    let names = source
        .rsplit("select ")
        .next()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .split(',')
        .map(|name| name.trim().trim_start_matches('$'))
        .collect::<Vec<_>>();
    let mut row = serde_json::Map::new();
    for pair in names.chunks_exact(2) {
        let marker = format!("let ${} = ", pair[0]);
        let ordinal: usize = source
            .split(&marker)
            .nth(1)
            .expect("reference ordinal assignment")
            .split(';')
            .next()
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        row.insert(
            pair[0].into(),
            json!({"kind":"value", "value":CanonicalValue::Long(ordinal as i64)}),
        );
        let identity = documents
            .iter()
            .find(|document| document["ordinal"].as_u64() == Some(ordinal as u64));
        row.insert(pair[1].into(), identity.map_or_else(|| json!({"kind":"absent"}), |document| json!({"kind":"thing", "iid":document["iid"], "type_id":{"kind":"entity", "label":document["_type"]}})));
    }
    type_bridge_orm::session::backend::AnswerItem::Row(row.into())
}

#[derive(Clone, Copy)]
enum CommitBehavior {
    Success,
    Failure,
}

struct RecordingBackend {
    state: Arc<Mutex<State>>,
    version: Version,
    given: bool,
    commit: CommitBehavior,
}

impl DriverBackend for RecordingBackend {
    fn open_transaction(
        &self,
        _database: &str,
        tx_type: TxType,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
        self.state.lock().unwrap().opens.push(tx_type);
        let transaction = RecordingTransaction {
            state: Arc::clone(&self.state),
            given: self.given,
            commit: self.commit,
        };
        Box::pin(async move { Ok(Box::new(transaction) as Box<dyn TransactionOps>) })
    }

    fn is_open(&self) -> bool {
        true
    }

    fn server_version(&self) -> Option<Version> {
        Some(self.version)
    }

    fn supports_given_rows(&self) -> bool {
        self.given
    }
}

struct RecordingTransaction {
    state: Arc<Mutex<State>>,
    given: bool,
    commit: CommitBehavior,
}

impl TransactionOps for RecordingTransaction {
    fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        Box::pin(async { panic!("projected batch used an unbounded/raw query seam") })
    }

    fn query_with_rows(
        &mut self,
        _typeql: &str,
        _rows: GivenRowsSpec,
    ) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        Box::pin(async { panic!("projected batch used a materializing Given seam") })
    }

    fn query_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        self.query_with_rows_bounded(
            typeql,
            GivenRowsSpec {
                variables: vec![],
                rows: vec![],
            },
            limits,
            consumer,
        )
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
            if typeql.contains("$reference_ordinal") {
                let Some(Response::ReferenceGuard(documents)) = state.responses.front() else {
                    panic!("explicit reference guard evidence");
                };
                Response::ReferenceGuard(documents.clone())
            } else {
                if matches!(state.responses.front(), Some(Response::ReferenceGuard(_))) {
                    state.responses.pop_front();
                }
                state
                    .responses
                    .pop_front()
                    .expect("unexpected projected batch provider call")
            }
        };
        let source = typeql.to_owned();
        Box::pin(async move {
            let mut reader = BoundedAnswerReader::new(limits);
            reader.check_before_read()?;
            let mut deliver = |documents: Vec<Value>| -> Result<(), OrmError> {
                for document in documents {
                    if reader.accept(native_batch::item(&source, document), consumer)?
                        == AnswerControl::Stop
                    {
                        break;
                    }
                }
                Ok(())
            };
            match response {
                Response::ReferenceGuard(documents) => {
                    reader.accept(reference_proof_row(&source, &documents), consumer)?;
                    Ok(reader.stats())
                }
                Response::Documents(documents) => {
                    deliver(documents)?;
                    Ok(reader.stats())
                }
                Response::ProviderAfter(documents) => {
                    deliver(documents)?;
                    Err(OrmError::QueryExecution("stream failed".to_owned()))
                }
                Response::CancelAfter(documents, cancellation) => {
                    deliver(documents)?;
                    cancellation.cancel();
                    std::future::pending().await
                }
                Response::PendingAfter(documents) => {
                    deliver(documents)?;
                    std::future::pending().await
                }
                Response::Underreported(documents) => {
                    for document in documents {
                        if consumer.accept(native_batch::item(&source, document))?
                            == AnswerControl::Stop
                        {
                            break;
                        }
                    }
                    Ok(BoundedAnswerStats::default())
                }
                Response::Error(message) => Err(OrmError::QueryExecution(message.to_owned())),
            }
        })
    }

    fn supports_given_rows(&self) -> bool {
        self.given
    }

    fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        Box::pin(async { panic!("projected batch used the unclassified commit seam") })
    }

    fn commit_classified(&mut self) -> BoxFuture<'_, Result<(), ClassifiedCommitError>> {
        self.state.lock().unwrap().commits += 1;
        let behavior = self.commit;
        Box::pin(async move {
            match behavior {
                CommitBehavior::Success => Ok(()),
                CommitBehavior::Failure => Err(ClassifiedCommitError::Driver {
                    certainty: CommitFailureCertainty::DefinitelyAborted,
                    message: "commit failed".to_owned(),
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

fn fixture(
    responses: Vec<Response>,
    version: Version,
    given: bool,
    commit: CommitBehavior,
) -> (Database, Arc<Mutex<State>>) {
    let state = Arc::new(Mutex::new(State {
        responses: responses.into(),
        ..State::default()
    }));
    let backend = RecordingBackend {
        state: Arc::clone(&state),
        version,
        given,
        commit,
    };
    (
        Database::with_backend(Box::new(backend), "batch-test"),
        state,
    )
}

fn installed() -> InstalledRuntimeProjection {
    installed_with_schema(SCHEMA, ProjectionHandler::python_v1())
}

fn installed_with_schema(schema: &str, handler: ProjectionHandler) -> InstalledRuntimeProjection {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("projected-batch-executor.yaml").unwrap(),
        schema,
    )])
    .unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
    InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &[handler],
            &[],
        )
        .unwrap(),
    )
    .unwrap()
}

fn installed_with_ordered_role() -> InstalledRuntimeProjection {
    let schema = SCHEMA
        .replace(
            "member: { card: { min: 0, max: 4 } }",
            "member: { card: { min: 0, max: 4 } }\n      participant: { card: { min: 0, max: 3 }, ordered: true }",
        )
        .replace("membership: [member]", "membership: [member, participant]");
    installed_with_schema(&schema, ProjectionHandler::python_v2())
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

fn person_create(installed: &InstalledRuntimeProjection, identifier: &str) -> ProjectedCreate {
    let person = type_id(TypeKind::Entity, "person");
    ProjectedCreate::try_new(
        installed,
        person.clone(),
        vec![(
            field(&person, "identifier"),
            vec![string_value(installed, "identifier", identifier)],
        )],
        vec![],
    )
    .unwrap()
}

fn membership_create(
    installed: &InstalledRuntimeProjection,
    identifier: &str,
    player_iid: &str,
) -> ProjectedCreate {
    let person = type_id(TypeKind::Entity, "person");
    let player =
        ProjectedReference::try_new(installed, person, Some(player_iid.to_owned()), vec![])
            .unwrap();
    membership_create_with_reference(installed, identifier, player)
}

fn membership_create_by_key(
    installed: &InstalledRuntimeProjection,
    identifier: &str,
    player_key: &str,
) -> ProjectedCreate {
    let person = type_id(TypeKind::Entity, "person");
    let player = ProjectedReference::try_new(
        installed,
        person.clone(),
        None,
        vec![(
            field(&person, "identifier"),
            string_value(installed, "identifier", player_key),
        )],
    )
    .unwrap();
    membership_create_with_reference(installed, identifier, player)
}

fn membership_create_with_reference(
    installed: &InstalledRuntimeProjection,
    identifier: &str,
    player: ProjectedReference,
) -> ProjectedCreate {
    let membership = type_id(TypeKind::Relation, "membership");
    ProjectedCreate::try_new(
        installed,
        membership.clone(),
        vec![(
            field(&membership, "identifier"),
            vec![string_value(installed, "identifier", identifier)],
        )],
        vec![(RoleId::new("membership", "member").unwrap(), vec![player])],
    )
    .unwrap()
}

fn control() -> ProjectedBatchInvocationControl {
    ProjectedBatchInvocationControl::capture(
        QueryExecutionResourceLimits::default(),
        AnswerCancellation::default(),
    )
}

fn person_document(ordinal: usize, iid: &str, identifier: &str) -> Value {
    json!({
        "ordinal": ordinal,
        "_iid": iid,
        "_type": "person",
        "attributes": {"identifier": [{"value": identifier}]}
    })
}

fn membership_document(ordinal: usize, iid: &str, identifier: &str, player_iid: &str) -> Value {
    json!({
        "ordinal": ordinal,
        "_iid": iid,
        "_type": "membership",
        "attributes": {"identifier": [{"value": identifier}]},
        "role_players": [{
            "role": "member",
            "iid": player_iid,
            "type_name": "person",
            "attributes": {"identifier": [{"value": "player-key"}]}
        }]
    })
}

#[derive(Clone, Copy, Debug)]
enum BatchKind {
    Entity,
    Relation,
}

fn three_row_batch(installed: &InstalledRuntimeProjection, kind: BatchKind) -> ProjectedBatch {
    match kind {
        BatchKind::Entity => ProjectedBatch::try_new(
            installed,
            type_id(TypeKind::Entity, "person"),
            ProjectedBatchOperation::Insert,
            (0..3)
                .map(|ordinal| {
                    ProjectedBatchRow::Create(person_create(
                        installed,
                        &format!("person-{ordinal}"),
                    ))
                })
                .collect(),
        ),
        BatchKind::Relation => ProjectedBatch::try_new(
            installed,
            type_id(TypeKind::Relation, "membership"),
            ProjectedBatchOperation::Insert,
            (0..3)
                .map(|ordinal| {
                    ProjectedBatchRow::Create(membership_create(
                        installed,
                        &format!("membership-{ordinal}"),
                        &format!("0x1{ordinal}"),
                    ))
                })
                .collect(),
        ),
    }
    .unwrap()
}

fn hydration_documents(kind: BatchKind) -> Vec<Value> {
    (0..3)
        .map(|ordinal| match kind {
            BatchKind::Entity => person_document(
                ordinal,
                &format!("0x2{ordinal}"),
                &format!("person-{ordinal}"),
            ),
            BatchKind::Relation => {
                let mut document = membership_document(
                    ordinal,
                    &format!("0x3{ordinal}"),
                    &format!("membership-{ordinal}"),
                    &format!("0x1{ordinal}"),
                );
                document["role_players"][0]["role"] = json!("membership:member");
                document
            }
        })
        .collect()
}

fn successful_responses(kind: BatchKind) -> Vec<Response> {
    vec![Response::Documents(hydration_documents(kind))]
}

fn assert_owned_atomic_failure(state: &Arc<Mutex<State>>, expected_calls: usize) {
    let state = state.lock().unwrap();
    assert_eq!(state.calls.len(), expected_calls);
    assert_eq!(state.rollbacks, 1);
    assert_eq!(state.commits, 0);
    assert!(state.responses.is_empty());
}

#[tokio::test]
async fn empty_and_missing_given_transport_reject_without_transaction_or_provider_work() {
    let installed = installed();
    let person = type_id(TypeKind::Entity, "person");
    let empty = ProjectedBatch::try_new(
        &installed,
        person.clone(),
        ProjectedBatchOperation::Insert,
        vec![],
    )
    .unwrap();
    let (database, state) = fixture(
        vec![],
        Version::new(3, 11, 5),
        false,
        CommitBehavior::Success,
    );
    assert_eq!(
        ProjectedBatchExecutor::new(&installed)
            .execute(&database, &empty, control())
            .await
            .unwrap(),
        ProjectedBatchResult::Things(Default::default())
    );
    assert!(state.lock().unwrap().opens.is_empty());

    let batch = ProjectedBatch::try_new(
        &installed,
        person,
        ProjectedBatchOperation::Insert,
        vec![ProjectedBatchRow::Create(person_create(&installed, "ada"))],
    )
    .unwrap();
    let diagnostic = ProjectedBatchExecutor::new(&installed)
        .execute(&database, &batch, control())
        .await
        .unwrap_err();
    assert_eq!(
        diagnostic.category(),
        SdkDiagnosticCategory::UnsupportedCapability
    );
    assert_eq!(
        diagnostic.code().as_str(),
        "projected_batch_transport_unsupported"
    );
    let state = state.lock().unwrap();
    assert!(state.opens.is_empty());
    assert!(state.calls.is_empty());
}

#[tokio::test]
async fn empty_unkeyed_put_keeps_admission_without_provider_or_transaction_work() {
    let schema = SCHEMA.replace("identifier: { key: true }", "identifier: { card: 1 }");
    let installed = installed_with_schema(&schema, ProjectionHandler::python_v1());
    let empty = ProjectedBatch::try_new(
        &installed,
        type_id(TypeKind::Entity, "person"),
        ProjectedBatchOperation::Put,
        vec![],
    )
    .unwrap();
    let (database, state) = fixture(
        vec![],
        Version::new(3, 14, 0),
        true,
        CommitBehavior::Success,
    );
    let result = ProjectedBatchExecutor::new(&installed)
        .execute(&database, &empty, control())
        .await
        .unwrap();
    assert_eq!(result, ProjectedBatchResult::Things(Default::default()));
    let state = state.lock().unwrap();
    assert!(state.opens.is_empty());
    assert!(state.calls.is_empty());
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn keyless_put_plans_over_the_statement_limit_refuse_before_provider_work() {
    let schema = SCHEMA.replace("identifier: { key: true }", "identifier: { card: 1 }");
    let installed = installed_with_schema(&schema, ProjectionHandler::python_v1());
    // Each equal row must see the previous insert: three plans.
    let row = ProjectedBatchRow::Create(person_create(&installed, "same"));
    let batch = ProjectedBatch::try_new(
        &installed,
        type_id(TypeKind::Entity, "person"),
        ProjectedBatchOperation::Put,
        vec![row.clone(), row.clone(), row],
    )
    .unwrap();
    assert_eq!(batch.resource_measure().statements(), 1);
    let (database, state) = fixture(
        vec![],
        Version::new(3, 14, 0),
        true,
        CommitBehavior::Success,
    );
    let limits = QueryExecutionResourceLimits {
        statements: Some(2),
        ..Default::default()
    };
    let diagnostic = ProjectedBatchExecutor::new(&installed)
        .execute(
            &database,
            &batch,
            ProjectedBatchInvocationControl::capture(limits, AnswerCancellation::default()),
        )
        .await
        .unwrap_err();
    assert_eq!(
        diagnostic.code().as_str(),
        "projected_batch_statement_limit"
    );
    let state = state.lock().unwrap();
    assert!(state.calls.is_empty());
    assert_eq!(state.commits, 0);
}

#[tokio::test]
async fn empty_mapped_execution_is_infallible_and_never_invokes_the_mapper() {
    let installed = installed();
    let empty = ProjectedBatch::try_new(
        &installed,
        type_id(TypeKind::Entity, "person"),
        ProjectedBatchOperation::Insert,
        vec![],
    )
    .unwrap();
    let (database, state) = fixture(
        vec![],
        Version::new(3, 11, 5),
        false,
        CommitBehavior::Success,
    );

    let result = ProjectedBatchExecutor::new(&installed)
        .execute_mapped(&database, &empty, control(), Vec::<String>::new(), |_| {
            panic!("an empty batch must not invoke its mapper")
        })
        .await
        .unwrap();

    assert!(result.is_empty());
    let state = state.lock().unwrap();
    assert!(state.opens.is_empty());
    assert!(state.calls.is_empty());
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn mapped_execution_preserves_input_order_before_owned_commit() {
    let installed = installed();
    let batch = three_row_batch(&installed, BatchKind::Entity);
    let (database, state) = fixture(
        vec![Response::Documents(vec![
            person_document(1, "0x21", "person-1"),
            person_document(2, "0x22", "person-2"),
            person_document(0, "0x20", "person-0"),
        ])],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    let mapper_state = Arc::clone(&state);

    let mapped = ProjectedBatchExecutor::new(&installed)
        .execute_mapped(
            &database,
            &batch,
            control(),
            Vec::<String>::new(),
            |result| {
                let state = mapper_state.lock().unwrap();
                assert_eq!(state.commits, 0, "mapper must run before commit");
                assert_eq!(state.rollbacks, 0, "mapper must run before rollback");
                drop(state);
                let ProjectedBatchResult::Things(things) = result else {
                    panic!("insert returned a delete result")
                };
                Ok(things
                    .into_items()
                    .into_iter()
                    .map(|thing| thing.iid().to_owned())
                    .collect())
            },
        )
        .await
        .unwrap();

    assert_eq!(mapped, ["0x20", "0x21", "0x22"]);
    let state = state.lock().unwrap();
    assert_eq!(state.calls.len(), 1);
    assert_eq!(state.commits, 1);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn first_middle_and_last_mapper_failures_roll_back_owned_execution_once() {
    for failing_ordinal in 0..3 {
        let installed = installed();
        let batch = three_row_batch(&installed, BatchKind::Entity);
        let (database, state) = fixture(
            successful_responses(BatchKind::Entity),
            Version::new(3, 12, 1),
            true,
            CommitBehavior::Success,
        );

        let diagnostic = ProjectedBatchExecutor::new(&installed)
            .execute_mapped(
                &database,
                &batch,
                control(),
                Vec::<String>::new(),
                |result| {
                    let ProjectedBatchResult::Things(things) = result else {
                        panic!("insert returned a delete result")
                    };
                    let mut mapped = Vec::with_capacity(things.count());
                    let (items, indexes) = things.into_parts();
                    for (thing, ordinal) in items.into_iter().zip(indexes) {
                        if ordinal == failing_ordinal {
                            return Err(ProjectedBatchExecutor::binding_materialization_failure(
                                u64::try_from(ordinal).expect("three-row batch ordinal fits u64"),
                            ));
                        }
                        mapped.push(thing.iid().to_owned());
                    }
                    Ok(mapped)
                },
            )
            .await
            .unwrap_err();

        assert_eq!(
            diagnostic.code().as_str(),
            "generated_model_materialization_failed"
        );
        assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Integrity);
        assert!(matches!(
            diagnostic.path(),
            [SdkDiagnosticPathSegment::Argument(argument), SdkDiagnosticPathSegment::Index(index)]
                if argument.as_str() == "rows" && *index == failing_ordinal as u64
        ));
        let state = state.lock().unwrap();
        assert_eq!(state.calls.len(), 1);
        assert_eq!(state.commits, 0);
        assert_eq!(state.rollbacks, 1);
        assert!(state.responses.is_empty());
    }
}

#[tokio::test]
async fn owned_mapper_panic_rolls_back_once_before_resuming_the_original_panic() {
    const PANIC_MESSAGE: &str = "owned mapper panic sentinel";

    let installed = installed();
    let batch = three_row_batch(&installed, BatchKind::Entity);
    let (database, state) = fixture(
        successful_responses(BatchKind::Entity),
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );

    let task = tokio::spawn(async move {
        ProjectedBatchExecutor::new(&installed)
            .execute_mapped(
                &database,
                &batch,
                control(),
                (),
                |_| -> std::result::Result<(), SdkExecutionDiagnostic> {
                    std::panic::panic_any(PANIC_MESSAGE)
                },
            )
            .await
    });
    let panic = task
        .await
        .expect_err("the original mapper panic must resume after cleanup")
        .into_panic();
    assert_eq!(panic.downcast_ref::<&str>(), Some(&PANIC_MESSAGE));

    let state = state.lock().unwrap();
    assert_eq!(state.calls.len(), 1);
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 1);
    assert!(state.responses.is_empty());
}

#[tokio::test]
async fn borrowed_mapper_failure_latches_the_exact_cause_and_rejects_commit() {
    let installed = installed();
    let batch = three_row_batch(&installed, BatchKind::Entity);
    let (database, state) = fixture(
        successful_responses(BatchKind::Entity),
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    let transaction = database.transaction_context(TxType::Write).await.unwrap();

    let diagnostic = ProjectedBatchExecutor::new(&installed)
        .execute_in_transaction_mapped(
            &transaction,
            &batch,
            control(),
            Vec::<String>::new(),
            |result| {
                let ProjectedBatchResult::Things(things) = result else {
                    panic!("insert returned a delete result")
                };
                let mut mapped = Vec::with_capacity(things.count());
                let (items, indexes) = things.into_parts();
                for (thing, ordinal) in items.into_iter().zip(indexes) {
                    if ordinal == 1 {
                        return Err(ProjectedBatchExecutor::binding_materialization_failure(
                            u64::try_from(ordinal).expect("three-row batch ordinal fits u64"),
                        ));
                    }
                    mapped.push(thing.iid().to_owned());
                }
                Ok(mapped)
            },
        )
        .await
        .unwrap_err();

    assert_eq!(
        diagnostic.code().as_str(),
        "generated_model_materialization_failed"
    );
    assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Integrity);
    assert!(matches!(
        diagnostic.path(),
        [SdkDiagnosticPathSegment::Argument(argument), SdkDiagnosticPathSegment::Index(1)]
            if argument.as_str() == "rows"
    ));
    assert_eq!(
        transaction.lifecycle_state().await,
        TransactionContextState::RollbackOnly
    );
    assert_eq!(
        transaction.rollback_only_cause().await.as_ref(),
        Some(&diagnostic)
    );
    let commit = transaction.commit_sdk().await.unwrap_err();
    assert_eq!(commit.code().as_str(), "transaction_rollback_only");
    let state = state.lock().unwrap();
    assert_eq!(state.calls.len(), 1);
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn borrowed_mapper_panic_poisons_before_resuming_the_original_panic() {
    const PANIC_MESSAGE: &str = "borrowed mapper panic sentinel";

    let installed = installed();
    let batch = three_row_batch(&installed, BatchKind::Entity);
    let (database, state) = fixture(
        successful_responses(BatchKind::Entity),
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    let transaction = database.transaction_context(TxType::Write).await.unwrap();
    let inspected_transaction = transaction.clone();

    let task = tokio::spawn(async move {
        ProjectedBatchExecutor::new(&installed)
            .execute_in_transaction_mapped(
                &transaction,
                &batch,
                control(),
                (),
                |_| -> std::result::Result<(), SdkExecutionDiagnostic> {
                    std::panic::panic_any(PANIC_MESSAGE)
                },
            )
            .await
    });
    let panic = task
        .await
        .expect_err("the original mapper panic must resume after poisoning")
        .into_panic();
    assert_eq!(panic.downcast_ref::<&str>(), Some(&PANIC_MESSAGE));

    assert_eq!(
        inspected_transaction.lifecycle_state().await,
        TransactionContextState::RollbackOnly
    );
    let cause = inspected_transaction
        .rollback_only_cause()
        .await
        .expect("mapper panic must record a redacted rollback-only cause");
    assert_eq!(cause.category(), SdkDiagnosticCategory::Integrity);
    assert_eq!(
        cause.code().as_str(),
        "generated_model_materialization_failed"
    );
    assert_eq!(
        cause.message().as_str(),
        "The generated model could not materialize validated projected evidence"
    );
    assert!(cause.details().is_empty());
    assert!(matches!(
        cause.path(),
        [SdkDiagnosticPathSegment::Argument(argument)] if argument.as_str() == "rows"
    ));
    assert_eq!(
        inspected_transaction
            .commit_sdk()
            .await
            .unwrap_err()
            .code()
            .as_str(),
        "transaction_rollback_only"
    );
    let state = state.lock().unwrap();
    assert_eq!(state.calls.len(), 1);
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn cancellation_triggered_by_the_mapper_poisons_borrowed_execution() {
    let installed = installed();
    let batch = three_row_batch(&installed, BatchKind::Entity);
    let (database, state) = fixture(
        successful_responses(BatchKind::Entity),
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    let transaction = database.transaction_context(TxType::Write).await.unwrap();
    let cancellation = AnswerCancellation::default();
    let mapper_cancellation = cancellation.clone();

    let diagnostic = ProjectedBatchExecutor::new(&installed)
        .execute_in_transaction_mapped(
            &transaction,
            &batch,
            ProjectedBatchInvocationControl::capture(
                QueryExecutionResourceLimits::default(),
                cancellation,
            ),
            Vec::<String>::new(),
            move |result| {
                mapper_cancellation.cancel();
                let ProjectedBatchResult::Things(things) = result else {
                    panic!("insert returned a delete result")
                };
                Ok(things
                    .into_items()
                    .into_iter()
                    .map(|thing| thing.iid().to_owned())
                    .collect())
            },
        )
        .await
        .unwrap_err();

    assert_eq!(diagnostic.code().as_str(), "provider_cancelled");
    assert_eq!(
        transaction.lifecycle_state().await,
        TransactionContextState::RollbackOnly
    );
    assert_eq!(
        transaction.rollback_only_cause().await.as_ref(),
        Some(&diagnostic)
    );
    assert_eq!(
        transaction.commit_sdk().await.unwrap_err().code().as_str(),
        "transaction_rollback_only"
    );
    let state = state.lock().unwrap();
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn deadline_crossed_by_the_mapper_poisons_borrowed_execution() {
    let installed = installed();
    let batch = three_row_batch(&installed, BatchKind::Entity);
    let (database, state) = fixture(
        successful_responses(BatchKind::Entity),
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    let transaction = database.transaction_context(TxType::Write).await.unwrap();
    let limits = QueryExecutionResourceLimits {
        timeout_milliseconds: type_bridge_orm::QueryTimeout::from_milliseconds(2_000),
        ..QueryExecutionResourceLimits::default()
    };
    let mapper_ran = Arc::new(AtomicBool::new(false));
    let mapper_probe = Arc::clone(&mapper_ran);
    let control = ProjectedBatchInvocationControl::capture(limits, AnswerCancellation::default());
    let deadline = control.deadline().instant();

    let diagnostic = ProjectedBatchExecutor::new(&installed)
        .execute_in_transaction_mapped(
            &transaction,
            &batch,
            control,
            Vec::<String>::new(),
            move |result| {
                mapper_probe.store(true, Ordering::SeqCst);
                // This synchronous checkpoint uses the real clock, beyond Tokio virtual time.
                std::thread::sleep(
                    deadline
                        .expect("test has a finite deadline")
                        .saturating_duration_since(std::time::Instant::now())
                        + Duration::from_millis(1),
                );
                let ProjectedBatchResult::Things(things) = result else {
                    panic!("insert returned a delete result")
                };
                Ok(things
                    .into_items()
                    .into_iter()
                    .map(|thing| thing.iid().to_owned())
                    .collect())
            },
        )
        .await
        .unwrap_err();

    assert_eq!(diagnostic.code().as_str(), "transaction_deadline_exceeded");
    assert!(
        mapper_ran.load(Ordering::SeqCst),
        "deadline mapper must run"
    );
    assert_eq!(
        transaction.lifecycle_state().await,
        TransactionContextState::RollbackOnly
    );
    assert_eq!(
        transaction.rollback_only_cause().await.as_ref(),
        Some(&diagnostic)
    );
    assert_eq!(
        transaction.commit_sdk().await.unwrap_err().code().as_str(),
        "transaction_rollback_only"
    );
    let state = state.lock().unwrap();
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn entity_insert_is_one_call_and_correlates_shuffled_documents_by_ordinal() {
    let ordered_schema = SCHEMA.replace(
        "tag: { card: { min: 0 } }",
        "tag: { card: { min: 0 }, ordered: true }",
    );
    for (installed, ordered) in [
        (installed(), false),
        (
            installed_with_schema(&ordered_schema, ProjectionHandler::python_v2()),
            true,
        ),
    ] {
        let person = type_id(TypeKind::Entity, "person");
        let batch = ProjectedBatch::try_new(
            &installed,
            person,
            ProjectedBatchOperation::Insert,
            vec![
                ProjectedBatchRow::Create(person_create(&installed, "ada")),
                ProjectedBatchRow::Create(person_create(&installed, "bob")),
            ],
        )
        .unwrap();
        let (database, state) = fixture(
            vec![Response::Documents(vec![
                person_document(1, "0x11", "bob"),
                person_document(0, "0x10", "ada"),
            ])],
            Version::new(3, 12, 1),
            true,
            CommitBehavior::Success,
        );

        let ProjectedBatchResult::Things(things) = ProjectedBatchExecutor::new(&installed)
            .execute(&database, &batch, control())
            .await
            .unwrap()
        else {
            panic!("insert returned delete result")
        };
        assert_eq!(
            things
                .items()
                .iter()
                .map(|thing| thing.iid())
                .collect::<Vec<_>>(),
            ["0x10", "0x11"]
        );
        let state = state.lock().unwrap();
        assert_eq!(
            state.calls[0].0,
            include_str!(
                "fixtures/native_batch/entity_insert_is_one_call_and_correlates_shuffled_documents_by_ordinal.tql"
            )
        );
        assert_eq!(state.calls.len(), 1);
        assert_eq!(state.calls[0].1.rows.len(), 2);
        assert!(state.calls[0].0.contains("insert $root0 isa person;"));
        assert!(state.calls[0].0.contains("fetch"));
        assert_eq!(state.commits, 1);
        assert_eq!(state.rollbacks, 0);
        if ordered {
            assert!(
                state
                    .calls
                    .iter()
                    .all(|(source, _)| !source.contains("has tag"))
            );
        }
    }
}

#[tokio::test]
async fn put_uses_one_multirow_key_pattern_and_preserves_hit_identity() {
    let installed = installed();
    let person = type_id(TypeKind::Entity, "person");
    let batch = ProjectedBatch::try_new(
        &installed,
        person,
        ProjectedBatchOperation::Put,
        vec![
            ProjectedBatchRow::Create(person_create(&installed, "hit")),
            ProjectedBatchRow::Create(person_create(&installed, "miss")),
        ],
    )
    .unwrap();
    let (database, state) = fixture(
        vec![Response::Documents(vec![
            person_document(0, "0x10", "hit"),
            person_document(1, "0x11", "miss"),
        ])],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );

    let ProjectedBatchResult::Things(things) = ProjectedBatchExecutor::new(&installed)
        .execute(&database, &batch, control())
        .await
        .unwrap()
    else {
        panic!("put returned delete result")
    };
    assert_eq!(
        things
            .items()
            .iter()
            .map(|thing| thing.iid())
            .collect::<Vec<_>>(),
        ["0x10", "0x11"]
    );
    let state = state.lock().unwrap();
    assert_eq!(
        state.calls[0].0,
        include_str!(
            "fixtures/native_batch/put_uses_one_multirow_key_pattern_and_preserves_hit_identity.tql"
        )
    );
    assert_eq!(state.calls.len(), 1);
    assert_eq!(state.calls[0].1.rows.len(), 2);
    assert_eq!(
        state.calls[0].0.matches("put $root0 isa! person").count(),
        1
    );
    assert!(state.calls[0].0.contains("has identifier == $field_0_0"));
}

#[tokio::test]
async fn empty_ordered_only_model_batches_insert_and_update_without_empty_write_stages() {
    let installed = installed_with_schema(
        "format: typebridge.schema/v2\nattributes:\n  tag: { value: string }\nentities:\n  memo:\n    owns:\n      tag: { card: { min: 0, max: 3 }, ordered: true }\n",
        ProjectionHandler::python_v2(),
    );
    let memo = type_id(TypeKind::Entity, "memo");
    for operation in [
        ProjectedBatchOperation::Insert,
        ProjectedBatchOperation::Update,
    ] {
        let create = ProjectedCreate::try_new(&installed, memo.clone(), vec![], vec![]).unwrap();
        let row = match operation {
            ProjectedBatchOperation::Insert => ProjectedBatchRow::Create(create),
            ProjectedBatchOperation::Update => ProjectedBatchRow::Update {
                iid: "0x10".into(),
                replacement: create,
            },
            _ => unreachable!(),
        };
        let batch =
            ProjectedBatch::try_new(&installed, memo.clone(), operation, vec![row]).unwrap();
        let (database, state) = fixture(
            vec![Response::Documents(vec![
                json!({"ordinal": 0, "_iid": "0x10", "_type": "memo", "attributes": {}}),
            ])],
            Version::new(3, 13, 6),
            true,
            CommitBehavior::Success,
        );
        let ProjectedBatchResult::Things(things) = ProjectedBatchExecutor::new(&installed)
            .execute(&database, &batch, control())
            .await
            .unwrap()
        else {
            panic!("memo model result")
        };
        assert_eq!(things.count(), 1);
        assert_eq!(things.items()[0].iid(), "0x10");
        assert!(things.items()[0].fields()[&field(&memo, "tag")].is_empty());
        let state = state.lock().unwrap();
        assert_eq!(state.calls.len(), 1);
        assert!(
            state
                .calls
                .iter()
                .all(|(source, _)| !source.contains("has tag"))
        );
        assert_eq!((state.commits, state.rollbacks), (1, 0));
    }

    let create = ProjectedCreate::try_new(&installed, memo.clone(), vec![], vec![]).unwrap();
    let batch = ProjectedBatch::try_new(
        &installed,
        memo,
        ProjectedBatchOperation::Insert,
        vec![ProjectedBatchRow::Create(create)],
    )
    .unwrap();
    let (database, state) = fixture(
        vec![
            Response::Documents(vec![json!({"ordinal": 0, "iid": "0x10"})]),
            Response::Error("readback failed"),
        ],
        Version::new(3, 13, 6),
        true,
        CommitBehavior::Success,
    );
    let transaction = database.transaction_context(TxType::Write).await.unwrap();
    let diagnostic = ProjectedBatchExecutor::new(&installed)
        .execute_in_transaction(&transaction, &batch, control())
        .await
        .unwrap_err();
    assert_eq!(
        transaction.lifecycle_state().await,
        TransactionContextState::RollbackOnly
    );
    assert_eq!(
        transaction.rollback_only_cause().await.unwrap().code(),
        diagnostic.code()
    );
    assert_eq!(
        transaction.commit_sdk().await.unwrap_err().code().as_str(),
        "transaction_rollback_only"
    );
    let state = state.lock().unwrap();
    assert_eq!(state.calls.len(), 1);
    assert_eq!((state.commits, state.rollbacks), (0, 0));
}

#[tokio::test]
async fn relation_insert_resolves_players_and_creates_exact_type_before_links() {
    for (variant, installed) in [installed(), installed_with_ordered_role()]
        .into_iter()
        .enumerate()
    {
        let membership = type_id(TypeKind::Relation, "membership");
        let batch = ProjectedBatch::try_new(
            &installed,
            membership,
            ProjectedBatchOperation::Insert,
            vec![ProjectedBatchRow::Create(membership_create(
                &installed, "m-1", "0xAb",
            ))],
        )
        .unwrap();
        let (database, state) = fixture(
            vec![Response::Documents(vec![{
                let mut document = membership_document(0, "0x20", "m-1", "0xab");
                document["role_players"][0]["role"] = json!("membership:member");
                document
            }])],
            Version::new(3, 12, 1),
            true,
            CommitBehavior::Success,
        );

        ProjectedBatchExecutor::new(&installed)
            .execute(&database, &batch, control())
            .await
            .unwrap();
        let state = state.lock().unwrap();
        assert_eq!(
            state.calls[0].0,
            [
                include_str!(
                    "fixtures/native_batch/relation_insert_resolves_players_and_creates_exact_type_before_links.tql"
                ),
                include_str!(
                    "fixtures/native_batch/relation_insert_resolves_players_and_creates_exact_type_before_links_ordered_role.tql"
                )
            ][variant]
        );
        assert_eq!(state.calls.len(), 1);
        let (source, rows) = &state.calls[0];
        assert!(rows.rows.iter().flatten().any(|value| matches!(value, type_bridge_orm::session::backend::GivenValue::String(iid) if iid == "0xab")));
        assert!(source.contains("isa! person;") && source.contains("iid("));
        assert!(
            source.find("insert $root0 isa membership;").unwrap()
                < source.find("$root0 links (member:").unwrap()
        );
        assert!(source.contains("select $root0, $ordinal1; distinct;"));
        assert!(source.contains("\"_role_0_players\""));
    }
}

#[tokio::test]
async fn key_reference_uses_its_selected_key_and_scoped_roles_are_validated() {
    let installed = installed();
    let membership = type_id(TypeKind::Relation, "membership");
    let batch = ProjectedBatch::try_new(
        &installed,
        membership,
        ProjectedBatchOperation::Insert,
        vec![ProjectedBatchRow::Create(membership_create_by_key(
            &installed,
            "m-key",
            "player-key",
        ))],
    )
    .unwrap();
    let scoped_document = || {
        let mut document = membership_document(0, "0x20", "m-key", "0x10");
        document["role_players"][0]["role"] = json!("membership:member");
        document
    };
    let (database, state) = fixture(
        vec![Response::Documents(vec![scoped_document()])],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    ProjectedBatchExecutor::new(&installed)
        .execute(&database, &batch, control())
        .await
        .unwrap();
    {
        let state = state.lock().unwrap();
        assert_eq!(
            state.calls[0].0,
            include_str!(
                "fixtures/native_batch/key_reference_uses_its_selected_key_and_scoped_roles_are_validated.tql"
            )
        );
        let key_column = state.calls[0]
            .1
            .variables
            .iter()
            .position(|variable| variable == "player_0_0")
            .unwrap();
        assert!(matches!(
            &state.calls[0].1.rows[0][key_column],
            type_bridge_orm::session::backend::GivenValue::String(value) if value == "player-key"
        ));
        assert!(state.calls[0].0.contains("== $player_0_0;"));
    }

    let (database, state) = fixture(
        vec![Response::Documents(vec![{
            let mut document = scoped_document();
            document["role_players"][0]["role"] = json!("other:member");
            document
        }])],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    let diagnostic = ProjectedBatchExecutor::new(&installed)
        .execute(&database, &batch, control())
        .await
        .unwrap_err();
    assert_eq!(diagnostic.code().as_str(), "query_remote_document_key");
    let state = state.lock().unwrap();
    assert_eq!(state.calls.len(), 1);
    assert_eq!(state.rollbacks, 1);
    assert_eq!(state.commits, 0);
}

#[tokio::test]
async fn mixed_iid_and_key_reference_arms_bind_shared_iid_evidence() {
    let installed = installed();
    let membership = type_id(TypeKind::Relation, "membership");
    let batch = ProjectedBatch::try_new(
        &installed,
        membership,
        ProjectedBatchOperation::Insert,
        vec![
            ProjectedBatchRow::Create(membership_create(&installed, "m-iid", "0x10")),
            ProjectedBatchRow::Create(membership_create_by_key(&installed, "m-key", "player-key")),
        ],
    )
    .unwrap();
    let scoped_document = |ordinal, iid: &str, key: &str, player: &str| {
        let mut document = membership_document(ordinal, iid, key, player);
        document["role_players"][0]["role"] = json!("membership:member");
        document
    };
    let (database, state) = fixture(
        vec![Response::Documents(vec![
            scoped_document(1, "0x21", "m-key", "0x11"),
            scoped_document(0, "0x20", "m-iid", "0x10"),
        ])],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );

    ProjectedBatchExecutor::new(&installed)
        .execute(&database, &batch, control())
        .await
        .unwrap();

    let state = state.lock().unwrap();
    assert_eq!(
        state.calls[0].0,
        include_str!(
            "fixtures/native_batch/mixed_iid_and_key_reference_arms_bind_shared_iid_evidence.tql"
        )
    );
    assert_eq!(state.calls.len(), 1);
    let source = &state.calls[0].0;
    assert!(source.contains("iid(") && source.contains("has identifier"));
    assert!(source.contains("try {") && source.contains(" or "));
    assert!(source.contains("\"_role_0_players\""));
}

#[tokio::test]
async fn mixed_relation_put_is_one_call_and_keeps_key_hit_identity() {
    let installed = installed();
    let membership = type_id(TypeKind::Relation, "membership");
    let batch = ProjectedBatch::try_new(
        &installed,
        membership,
        ProjectedBatchOperation::Put,
        vec![
            ProjectedBatchRow::Create(membership_create(&installed, "hit", "0x10")),
            ProjectedBatchRow::Create(membership_create(&installed, "miss", "0x11")),
        ],
    )
    .unwrap();
    let scoped_document = |ordinal, iid: &str, key: &str, player: &str| {
        let mut document = membership_document(ordinal, iid, key, player);
        document["role_players"][0]["role"] = json!("membership:member");
        document
    };
    let (database, state) = fixture(
        vec![Response::Documents(vec![
            scoped_document(1, "0x31", "miss", "0x11"),
            scoped_document(0, "0x30", "hit", "0x10"),
        ])],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    let ProjectedBatchResult::Things(things) = ProjectedBatchExecutor::new(&installed)
        .execute(&database, &batch, control())
        .await
        .unwrap()
    else {
        panic!("relation put returned delete result")
    };
    assert_eq!(
        things
            .items()
            .iter()
            .map(|thing| thing.iid())
            .collect::<Vec<_>>(),
        ["0x30", "0x31"]
    );
    let state = state.lock().unwrap();
    assert_eq!(
        state.calls[0].0,
        include_str!(
            "fixtures/native_batch/mixed_relation_put_is_one_call_and_keeps_key_hit_identity.tql"
        )
    );
    assert_eq!(state.calls.len(), 1);
    assert_eq!(state.calls[0].1.rows.len(), 2);
    let source = &state.calls[0].0;
    assert!(source.contains("put $root0 isa! membership"));
    assert!(source.contains("delete try { links (member:"));
    assert!(source.contains("of $root0;"));
    assert!(source.contains("$root0 links (member:"));
    assert!(source.contains("\"_role_0_players\""));
    assert!(state.responses.is_empty());
}

#[tokio::test]
async fn relation_update_is_one_call_and_clears_keys_and_roles() {
    let installed = installed();
    let membership = type_id(TypeKind::Relation, "membership");
    let batch = ProjectedBatch::try_new(
        &installed,
        membership,
        ProjectedBatchOperation::Update,
        vec![ProjectedBatchRow::Update {
            iid: "0x30".to_owned(),
            replacement: membership_create(&installed, "updated", "0x11"),
        }],
    )
    .unwrap();
    let mut hydrated = membership_document(0, "0x30", "updated", "0x11");
    hydrated["role_players"][0]["role"] = json!("membership:member");
    let (database, state) = fixture(
        vec![Response::Documents(vec![hydrated])],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    ProjectedBatchExecutor::new(&installed)
        .execute(&database, &batch, control())
        .await
        .unwrap();
    let state = state.lock().unwrap();
    assert_eq!(
        state.calls[0].0,
        include_str!(
            "fixtures/native_batch/relation_update_is_one_call_and_clears_keys_and_roles.tql"
        )
    );
    assert_eq!(state.calls.len(), 1);
    let source = &state.calls[0].0;
    assert!(source.contains("has $PlanHas") && source.contains("of $root0;"));
    assert_eq!(source.matches("links (member: $PlanPlayers").count(), 2);
    assert!(source.contains("$root0 links (member:"));
    assert!(state.responses.is_empty());
}

#[tokio::test]
async fn relation_delete_is_one_exact_idempotent_call() {
    let installed = installed();
    let batch = ProjectedBatch::try_new(
        &installed,
        type_id(TypeKind::Relation, "membership"),
        ProjectedBatchOperation::Delete,
        vec![
            ProjectedBatchRow::Delete {
                iid: "0x30".to_owned(),
            },
            ProjectedBatchRow::Delete {
                iid: "0x31".to_owned(),
            },
        ],
    )
    .unwrap();
    let (database, state) = fixture(
        vec![Response::Documents(vec![
            json!({"ordinal": 1}),
            json!({"ordinal": 0, "iid": "0x30"}),
        ])],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    assert_eq!(
        ProjectedBatchExecutor::new(&installed)
            .execute(&database, &batch, control())
            .await
            .unwrap(),
        ProjectedBatchResult::Deleted
    );
    let state = state.lock().unwrap();
    assert_eq!(state.calls.len(), 1);
    assert!(state.calls[0].0.contains("$thing1 isa! membership;"));
    assert!(state.calls[0].0.contains("delete $thing1;"));
    assert!(!state.calls[0].0.contains(" links "));
    assert!(state.responses.is_empty());
}

#[tokio::test]
async fn delete_is_one_idempotent_call_and_accepts_absent_targets() {
    let installed = installed();
    let person = type_id(TypeKind::Entity, "person");
    let batch = ProjectedBatch::try_new(
        &installed,
        person,
        ProjectedBatchOperation::Delete,
        vec![
            ProjectedBatchRow::Delete { iid: "0x10".into() },
            ProjectedBatchRow::Delete { iid: "0x11".into() },
        ],
    )
    .unwrap();
    let (database, state) = fixture(
        vec![Response::Documents(vec![
            json!({"ordinal": 1}),
            json!({"ordinal": 0}),
        ])],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    assert_eq!(
        ProjectedBatchExecutor::new(&installed)
            .execute(&database, &batch, control())
            .await
            .unwrap(),
        ProjectedBatchResult::Deleted
    );
    let state = state.lock().unwrap();
    assert_eq!(state.calls.len(), 1);
    assert!(state.calls[0].0.contains("delete $thing1;"));
    assert_eq!(state.commits, 1);
}

#[tokio::test]
async fn borrowed_postmutation_failure_is_atomic_and_latches_exact_first_cause() {
    let installed = installed();
    let person = type_id(TypeKind::Entity, "person");
    let batch = ProjectedBatch::try_new(
        &installed,
        person,
        ProjectedBatchOperation::Insert,
        vec![ProjectedBatchRow::Create(person_create(&installed, "ada"))],
    )
    .unwrap();
    let (database, state) = fixture(
        vec![
            Response::Documents(vec![json!({"ordinal": 0, "iid": "0x10"})]),
            Response::Error("hydrate failed"),
        ],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    let transaction = database.transaction_context(TxType::Write).await.unwrap();
    let diagnostic = ProjectedBatchExecutor::new(&installed)
        .execute_in_transaction(&transaction, &batch, control())
        .await
        .unwrap_err();
    assert_eq!(
        transaction.lifecycle_state().await,
        TransactionContextState::RollbackOnly
    );
    assert_eq!(
        transaction.rollback_only_cause().await.unwrap().code(),
        diagnostic.code()
    );
    let commit = transaction.commit_sdk().await.unwrap_err();
    assert_eq!(commit.code().as_str(), "transaction_rollback_only");
    let state = state.lock().unwrap();
    assert_eq!(state.calls.len(), 1);
    assert_eq!(state.commits, 0);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn deterministic_item_limit_failure_precedes_first_mutation_dispatch() {
    let installed = installed();
    let person = type_id(TypeKind::Entity, "person");
    let batch = ProjectedBatch::try_new(
        &installed,
        person,
        ProjectedBatchOperation::Insert,
        vec![
            ProjectedBatchRow::Create(person_create(&installed, "ada")),
            ProjectedBatchRow::Create(person_create(&installed, "bob")),
        ],
    )
    .unwrap();
    let (database, state) = fixture(
        vec![],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    let limits = QueryExecutionResourceLimits {
        items: 1,
        ..QueryExecutionResourceLimits::default()
    };
    let diagnostic = ProjectedBatchExecutor::new(&installed)
        .execute(
            &database,
            &batch,
            ProjectedBatchInvocationControl::capture(limits, AnswerCancellation::default()),
        )
        .await
        .unwrap_err();
    assert_eq!(diagnostic.category(), SdkDiagnosticCategory::ResourceLimit);
    assert_eq!(diagnostic.code().as_str(), "batch_item_limit");
    let state = state.lock().unwrap();
    assert!(state.calls.is_empty());
    assert_eq!(state.rollbacks, 0);
    assert_eq!(state.commits, 0);
}

#[tokio::test]
async fn owned_commit_failure_is_not_followed_by_rollback() {
    let installed = installed();
    let person = type_id(TypeKind::Entity, "person");
    let batch = ProjectedBatch::try_new(
        &installed,
        person,
        ProjectedBatchOperation::Delete,
        vec![ProjectedBatchRow::Delete { iid: "0x10".into() }],
    )
    .unwrap();
    let (database, state) = fixture(
        vec![Response::Documents(vec![json!({"ordinal": 0})])],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Failure,
    );
    ProjectedBatchExecutor::new(&installed)
        .execute(&database, &batch, control())
        .await
        .unwrap_err();
    let state = state.lock().unwrap();
    assert_eq!(state.commits, 1);
    assert_eq!(state.rollbacks, 0);
}

#[tokio::test]
async fn update_is_one_call_clears_keys_and_rejects_a_changed_target_iid() {
    let installed = installed();
    let person = type_id(TypeKind::Entity, "person");
    let batch = ProjectedBatch::try_new(
        &installed,
        person.clone(),
        ProjectedBatchOperation::Update,
        vec![ProjectedBatchRow::Update {
            iid: "0xAb".into(),
            replacement: person_create(&installed, "new-key"),
        }],
    )
    .unwrap();
    let (database, state) = fixture(
        vec![Response::Documents(vec![person_document(
            0, "0xab", "new-key",
        )])],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    ProjectedBatchExecutor::new(&installed)
        .execute(&database, &batch, control())
        .await
        .unwrap();
    {
        let state = state.lock().unwrap();
        assert_eq!(
            state.calls[0].0,
            include_str!(
                "fixtures/native_batch/update_is_one_call_clears_keys_and_rejects_a_changed_target_iid.tql"
            )
        );
        assert_eq!(state.calls.len(), 1);
        assert!(matches!(
            &state.calls[0].1.rows[0][1],
            type_bridge_orm::session::backend::GivenValue::String(iid) if iid == "0xab"
        ));
        assert!(state.calls[0].0.contains("$root0 isa! person;"));
        assert!(state.calls[0].0.contains("of $root0"));
    }

    let (database, state) = fixture(
        vec![Response::Documents(vec![person_document(
            0, "0x11", "new-key",
        )])],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    let diagnostic = ProjectedBatchExecutor::new(&installed)
        .execute(&database, &batch, control())
        .await
        .unwrap_err();
    assert_eq!(
        diagnostic.code().as_str(),
        "projected_batch_update_identity_changed"
    );
    let state = state.lock().unwrap();
    assert_eq!(state.calls.len(), 1);
    assert_eq!(state.rollbacks, 1);
    assert_eq!(state.commits, 0);
}

#[tokio::test]
async fn borrowed_success_stays_active_and_transport_refusal_does_not_poison() {
    let installed = installed();
    let person = type_id(TypeKind::Entity, "person");
    let batch = ProjectedBatch::try_new(
        &installed,
        person,
        ProjectedBatchOperation::Insert,
        vec![ProjectedBatchRow::Create(person_create(&installed, "ada"))],
    )
    .unwrap();
    let (database, _) = fixture(
        vec![Response::Documents(vec![person_document(0, "0x10", "ada")])],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    let transaction = database.transaction_context(TxType::Write).await.unwrap();
    ProjectedBatchExecutor::new(&installed)
        .execute_in_transaction(&transaction, &batch, control())
        .await
        .unwrap();
    assert_eq!(
        transaction.lifecycle_state().await,
        TransactionContextState::Active
    );

    let (database, state) = fixture(
        vec![],
        Version::new(3, 11, 5),
        true,
        CommitBehavior::Success,
    );
    let transaction = database.transaction_context(TxType::Write).await.unwrap();
    let diagnostic = ProjectedBatchExecutor::new(&installed)
        .execute_in_transaction(&transaction, &batch, control())
        .await
        .unwrap_err();
    assert_eq!(
        diagnostic.code().as_str(),
        "projected_batch_transport_unsupported"
    );
    assert_eq!(
        transaction.lifecycle_state().await,
        TransactionContextState::Active
    );
    assert!(state.lock().unwrap().calls.is_empty());
}

#[tokio::test]
async fn first_malformed_mutation_item_wins_over_a_later_answer_limit() {
    let installed = installed();
    let person = type_id(TypeKind::Entity, "person");
    let batch = ProjectedBatch::try_new(
        &installed,
        person,
        ProjectedBatchOperation::Insert,
        vec![ProjectedBatchRow::Create(person_create(&installed, "ada"))],
    )
    .unwrap();
    let (database, state) = fixture(
        vec![Response::Documents(vec![
            json!({"ordinal": 0}),
            json!({"ordinal": 0, "iid": "0x10"}),
            json!({"ordinal": 0, "iid": "0x11"}),
        ])],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    let transaction = database.transaction_context(TxType::Write).await.unwrap();
    let diagnostic = ProjectedBatchExecutor::new(&installed)
        .execute_in_transaction(&transaction, &batch, control())
        .await
        .unwrap_err();
    assert_eq!(diagnostic.code().as_str(), "query_remote_document_value");
    assert_eq!(
        transaction.rollback_only_cause().await.unwrap().code(),
        diagnostic.code()
    );
    assert_eq!(state.lock().unwrap().calls.len(), 1);
}

#[tokio::test]
async fn inline_write_answer_rejects_a_different_requested_player_identity() {
    let installed = installed();
    let membership = type_id(TypeKind::Relation, "membership");
    let batch = ProjectedBatch::try_new(
        &installed,
        membership,
        ProjectedBatchOperation::Insert,
        vec![ProjectedBatchRow::Create(membership_create(
            &installed, "m-1", "0x10",
        ))],
    )
    .unwrap();
    let (database, state) = fixture(
        vec![Response::Documents(vec![membership_document(
            0, "0x20", "m-1", "0x11",
        )])],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    let transaction = database.transaction_context(TxType::Write).await.unwrap();
    let diagnostic = ProjectedBatchExecutor::new(&installed)
        .execute_in_transaction(&transaction, &batch, control())
        .await
        .unwrap_err();
    assert_eq!(diagnostic.code().as_str(), "provider_identity_mismatch");
    assert_eq!(
        transaction.lifecycle_state().await,
        TransactionContextState::RollbackOnly
    );
    assert_eq!(state.lock().unwrap().calls.len(), 1);
}

#[tokio::test]
async fn provider_temporal_preflight_keeps_the_exact_batch_field_path() {
    let installed = installed();
    let person = type_id(TypeKind::Entity, "person");
    let lag = field(&person, "lag");
    let create = ProjectedCreate::try_new(
        &installed,
        person.clone(),
        vec![
            (
                field(&person, "identifier"),
                vec![string_value(&installed, "identifier", "ada")],
            ),
            (
                lag.clone(),
                vec![
                    ProjectedAttributeValue::try_new(
                        &installed,
                        type_id(TypeKind::Attribute, "lag"),
                        CanonicalValue::Duration(CanonicalDuration::new(true, 0, 1, 0, 0).unwrap()),
                    )
                    .unwrap(),
                ],
            ),
        ],
        vec![],
    )
    .unwrap();
    let batch = ProjectedBatch::try_new(
        &installed,
        person,
        ProjectedBatchOperation::Insert,
        vec![ProjectedBatchRow::Create(create)],
    )
    .unwrap();
    let (database, state) = fixture(
        vec![],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    let diagnostic = ProjectedBatchExecutor::new(&installed)
        .execute(&database, &batch, control())
        .await
        .unwrap_err();
    assert_eq!(diagnostic.category(), SdkDiagnosticCategory::InvalidInput);
    assert_eq!(diagnostic.code().as_str(), "provider_duration_out_of_range");
    assert_eq!(
        diagnostic.path(),
        [
            SdkDiagnosticPathSegment::Argument(
                type_bridge_contract::sdk_diagnostic::SdkDiagnosticName::new("rows").unwrap(),
            ),
            SdkDiagnosticPathSegment::Index(0),
            SdkDiagnosticPathSegment::Field(lag),
        ]
    );
    assert!(state.lock().unwrap().opens.is_empty());
}

#[test]
fn first_middle_last_validation_rejects_entity_and_relation_rows_without_io() {
    let installed = installed();
    for failure_ordinal in 0..3 {
        let person = type_id(TypeKind::Entity, "person");
        let entity_rows = (0..3)
            .map(|ordinal| ProjectedBatchRow::Update {
                iid: if ordinal == failure_ordinal {
                    "not-an-iid".to_owned()
                } else {
                    format!("0x4{ordinal}")
                },
                replacement: person_create(&installed, &format!("entity-{ordinal}")),
            })
            .collect();
        let diagnostic = ProjectedBatch::try_new(
            &installed,
            person,
            ProjectedBatchOperation::Update,
            entity_rows,
        )
        .unwrap_err();
        assert_eq!(diagnostic.category(), SdkDiagnosticCategory::InvalidInput);
        assert!(
            diagnostic
                .path()
                .contains(&SdkDiagnosticPathSegment::Index(failure_ordinal))
        );

        let membership = type_id(TypeKind::Relation, "membership");
        let relation_rows = (0..3)
            .map(|ordinal| {
                ProjectedBatchRow::Create(if ordinal == failure_ordinal {
                    person_create(&installed, &format!("invalid-{ordinal}"))
                } else {
                    membership_create(
                        &installed,
                        &format!("relation-{ordinal}"),
                        &format!("0x5{ordinal}"),
                    )
                })
            })
            .collect();
        let diagnostic = ProjectedBatch::try_new(
            &installed,
            membership,
            ProjectedBatchOperation::Insert,
            relation_rows,
        )
        .unwrap_err();
        assert_eq!(diagnostic.code().as_str(), "batch_row_model_mismatch");
        assert!(
            diagnostic
                .path()
                .contains(&SdkDiagnosticPathSegment::Index(failure_ordinal))
        );
    }

    let (_database, state) = fixture(
        vec![],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    let state = state.lock().unwrap();
    assert!(state.opens.is_empty());
    assert!(state.calls.is_empty());
}

#[tokio::test]
async fn first_middle_last_provider_failures_roll_back_entity_and_relation_batches() {
    let installed = installed();
    for kind in [BatchKind::Entity, BatchKind::Relation] {
        for failure_position in 0..3 {
            let batch = three_row_batch(&installed, kind);
            let responses = vec![Response::ProviderAfter(
                hydration_documents(kind)[..failure_position].to_vec(),
            )];
            let (database, state) = fixture(
                responses,
                Version::new(3, 12, 1),
                true,
                CommitBehavior::Success,
            );
            let diagnostic = ProjectedBatchExecutor::new(&installed)
                .execute(&database, &batch, control())
                .await
                .unwrap_err();
            assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Provider);
            assert_owned_atomic_failure(&state, 1);
        }
    }
}

#[tokio::test]
async fn first_middle_last_hydration_failures_publish_nothing_and_roll_back() {
    let installed = installed();
    for kind in [BatchKind::Entity, BatchKind::Relation] {
        for failure_position in 0..3 {
            let batch = three_row_batch(&installed, kind);
            let mut responses = Vec::new();
            let mut documents = hydration_documents(kind);
            match kind {
                BatchKind::Entity => documents[failure_position]["_iid"] = json!("bad-iid"),
                BatchKind::Relation => {
                    documents[failure_position]["role_players"][0]["role"] = json!("other:member");
                }
            }
            responses.push(Response::Documents(documents));
            let (database, state) = fixture(
                responses,
                Version::new(3, 12, 1),
                true,
                CommitBehavior::Success,
            );
            let diagnostic = ProjectedBatchExecutor::new(&installed)
                .execute(&database, &batch, control())
                .await
                .unwrap_err();
            assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Integrity);
            assert_owned_atomic_failure(&state, 1);
        }
    }
}

#[tokio::test]
async fn first_middle_last_inflight_cancellation_rolls_back_both_batch_kinds() {
    let installed = installed();
    for kind in [BatchKind::Entity, BatchKind::Relation] {
        for interruption_position in 0..3 {
            let batch = three_row_batch(&installed, kind);
            let cancellation = AnswerCancellation::default();
            let responses = vec![Response::CancelAfter(
                hydration_documents(kind)[..interruption_position].to_vec(),
                cancellation.clone(),
            )];
            let (database, state) = fixture(
                responses,
                Version::new(3, 12, 1),
                true,
                CommitBehavior::Success,
            );
            let diagnostic = ProjectedBatchExecutor::new(&installed)
                .execute(
                    &database,
                    &batch,
                    ProjectedBatchInvocationControl::capture(
                        QueryExecutionResourceLimits::default(),
                        cancellation,
                    ),
                )
                .await
                .unwrap_err();
            assert_eq!(diagnostic.code().as_str(), "provider_cancelled");
            assert_owned_atomic_failure(&state, 1);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn first_middle_last_inflight_timeout_rolls_back_both_batch_kinds() {
    let installed = installed();
    for kind in [BatchKind::Entity, BatchKind::Relation] {
        for interruption_position in 0..3 {
            let batch = three_row_batch(&installed, kind);
            let responses = vec![Response::PendingAfter(
                hydration_documents(kind)[..interruption_position].to_vec(),
            )];
            let (database, state) = fixture(
                responses,
                Version::new(3, 12, 1),
                true,
                CommitBehavior::Success,
            );
            let limits = QueryExecutionResourceLimits {
                // Real preparation must reach the pending stream before virtual timeout.
                timeout_milliseconds: type_bridge_orm::QueryTimeout::from_milliseconds(10_000),
                ..QueryExecutionResourceLimits::default()
            };
            let diagnostic = ProjectedBatchExecutor::new(&installed)
                .execute(
                    &database,
                    &batch,
                    ProjectedBatchInvocationControl::capture(limits, AnswerCancellation::default()),
                )
                .await
                .unwrap_err();
            assert_eq!(diagnostic.code().as_str(), "transaction_deadline_exceeded");
            assert_owned_atomic_failure(&state, 1);
        }
    }
}

#[tokio::test]
async fn allocation_preflight_rejects_both_batch_kinds_before_opening_transactions() {
    let installed = installed();
    for kind in [BatchKind::Entity, BatchKind::Relation] {
        let batch = three_row_batch(&installed, kind);
        let (database, state) = fixture(
            vec![],
            Version::new(3, 12, 1),
            true,
            CommitBehavior::Success,
        );
        let limits = QueryExecutionResourceLimits {
            bytes: batch.resource_measure().bytes(),
            ..QueryExecutionResourceLimits::default()
        };
        let diagnostic = ProjectedBatchExecutor::new(&installed)
            .execute(
                &database,
                &batch,
                ProjectedBatchInvocationControl::capture(limits, AnswerCancellation::default()),
            )
            .await
            .unwrap_err();
        assert_eq!(
            diagnostic.code().as_str(),
            "projected_batch_preparation_allocation_limit"
        );
        let state = state.lock().unwrap();
        assert!(state.opens.is_empty());
        assert!(state.calls.is_empty());
        assert_eq!(state.rollbacks, 0);
        assert_eq!(state.commits, 0);
    }
}

#[tokio::test]
async fn commit_failures_are_terminal_for_complete_entity_and_relation_batches() {
    let installed = installed();
    for kind in [BatchKind::Entity, BatchKind::Relation] {
        let batch = three_row_batch(&installed, kind);
        let (database, state) = fixture(
            successful_responses(kind),
            Version::new(3, 12, 1),
            true,
            CommitBehavior::Failure,
        );
        ProjectedBatchExecutor::new(&installed)
            .execute(&database, &batch, control())
            .await
            .unwrap_err();
        let state = state.lock().unwrap();
        assert!(state.responses.is_empty());
        assert_eq!(state.commits, 1);
        assert_eq!(state.rollbacks, 0);
    }
}

#[tokio::test]
async fn local_reply_accounting_defeats_underreported_provider_stats() {
    let installed = installed();
    let batch = ProjectedBatch::try_new(
        &installed,
        type_id(TypeKind::Entity, "person"),
        ProjectedBatchOperation::Insert,
        vec![ProjectedBatchRow::Create(person_create(&installed, "ada"))],
    )
    .unwrap();
    let (database, state) = fixture(
        vec![Response::Underreported(vec![
            person_document(0, "0x10", "ada"),
            person_document(0, "0x10", "ada"),
        ])],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    let limits = QueryExecutionResourceLimits {
        items: 1,
        ..QueryExecutionResourceLimits::default()
    };
    let diagnostic = ProjectedBatchExecutor::new(&installed)
        .execute(
            &database,
            &batch,
            ProjectedBatchInvocationControl::capture(limits, AnswerCancellation::default()),
        )
        .await
        .unwrap_err();
    assert_eq!(diagnostic.category(), SdkDiagnosticCategory::ResourceLimit);
    assert_owned_atomic_failure(&state, 1);
}

#[tokio::test]
async fn reply_bytes_and_projected_structure_are_bounded_before_publication() {
    let installed = installed();
    let entity_batch = ProjectedBatch::try_new(
        &installed,
        type_id(TypeKind::Entity, "person"),
        ProjectedBatchOperation::Insert,
        vec![ProjectedBatchRow::Create(person_create(&installed, "ada"))],
    )
    .unwrap();
    let (database, state) = fixture(
        vec![Response::Underreported(vec![person_document(
            0,
            "0x10",
            &"x".repeat(131_072),
        )])],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    let limits = QueryExecutionResourceLimits {
        bytes: 64 * 1_024,
        ..QueryExecutionResourceLimits::default()
    };
    let diagnostic = ProjectedBatchExecutor::new(&installed)
        .execute(
            &database,
            &entity_batch,
            ProjectedBatchInvocationControl::capture(limits, AnswerCancellation::default()),
        )
        .await
        .unwrap_err();
    assert_eq!(diagnostic.category(), SdkDiagnosticCategory::ResourceLimit);
    assert_owned_atomic_failure(&state, 1);

    let relation_batch = ProjectedBatch::try_new(
        &installed,
        type_id(TypeKind::Relation, "membership"),
        ProjectedBatchOperation::Insert,
        vec![ProjectedBatchRow::Create(membership_create(
            &installed,
            "membership",
            "0x10",
        ))],
    )
    .unwrap();
    let mut output = membership_document(0, "0x30", "membership", "0x10");
    output["role_players"].as_array_mut().unwrap().push(json!({
        "role": "membership:member",
        "iid": "0x11",
        "type_name": "person",
        "attributes": {"identifier": [{"value": "second-player"}]}
    }));
    output["role_players"][0]["role"] = json!("membership:member");
    let (database, state) = fixture(
        vec![Response::Documents(vec![output])],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    let limits = QueryExecutionResourceLimits {
        role_players: 1,
        ..QueryExecutionResourceLimits::default()
    };
    let diagnostic = ProjectedBatchExecutor::new(&installed)
        .execute(
            &database,
            &relation_batch,
            ProjectedBatchInvocationControl::capture(limits, AnswerCancellation::default()),
        )
        .await
        .unwrap_err();
    assert_eq!(
        diagnostic.code().as_str(),
        "projected_batch_output_resource_limit"
    );
    assert_owned_atomic_failure(&state, 1);
}

#[tokio::test]
async fn native_batches_split_five_thousand_rows_atomically_for_every_write_form() {
    let installed = installed();
    for kind in [BatchKind::Entity, BatchKind::Relation] {
        for operation in [
            ProjectedBatchOperation::Insert,
            ProjectedBatchOperation::Put,
            ProjectedBatchOperation::Update,
            ProjectedBatchOperation::Delete,
        ] {
            for fail_last in [false, true] {
                let model = type_id(
                    match kind {
                        BatchKind::Entity => TypeKind::Entity,
                        BatchKind::Relation => TypeKind::Relation,
                    },
                    match kind {
                        BatchKind::Entity => "person",
                        BatchKind::Relation => "membership",
                    },
                );
                let batch = ProjectedBatch::try_new(
                    &installed,
                    model,
                    operation,
                    (0..5000)
                        .map(|ordinal| {
                            let iid = format!("0x{:04x}", ordinal + 1);
                            if operation == ProjectedBatchOperation::Delete {
                                return ProjectedBatchRow::Delete { iid };
                            }
                            let replacement = match kind {
                                BatchKind::Entity => {
                                    person_create(&installed, &format!("p-{ordinal}"))
                                }
                                BatchKind::Relation => {
                                    membership_create(&installed, &format!("p-{ordinal}"), "0xff")
                                }
                            };
                            if operation == ProjectedBatchOperation::Update {
                                ProjectedBatchRow::Update { iid, replacement }
                            } else {
                                ProjectedBatchRow::Create(replacement)
                            }
                        })
                        .collect(),
                )
                .unwrap();
                let mut responses = Vec::new();
                for range in [0..4096, 4096..5000] {
                    let documents = range
                        .clone()
                        .map(|ordinal| {
                            let iid = format!("0x{:04x}", ordinal + 1);
                            if operation == ProjectedBatchOperation::Delete {
                                return json!({"ordinal":ordinal});
                            }
                            match kind {
                                BatchKind::Entity => {
                                    person_document(ordinal, &iid, &format!("p-{ordinal}"))
                                }
                                BatchKind::Relation => membership_document(
                                    ordinal,
                                    &iid,
                                    &format!("p-{ordinal}"),
                                    "0xff",
                                ),
                            }
                        })
                        .collect();
                    responses.push(if fail_last && range.start == 4096 {
                        Response::ProviderAfter(documents)
                    } else {
                        Response::Documents(documents)
                    });
                }
                let (database, state) = fixture(
                    responses,
                    Version::new(3, 13, 6),
                    true,
                    CommitBehavior::Success,
                );
                let result = ProjectedBatchExecutor::new(&installed)
                    .execute(&database, &batch, control())
                    .await;
                if fail_last {
                    assert_eq!(
                        result.unwrap_err().code().as_str(),
                        "provider_operation_failed",
                        "{kind:?} {operation:?}"
                    );
                } else if operation == ProjectedBatchOperation::Delete {
                    assert_eq!(result.unwrap(), ProjectedBatchResult::Deleted);
                } else {
                    let ProjectedBatchResult::Things(things) = result.unwrap() else {
                        panic!("write result");
                    };
                    assert_eq!(things.count(), 5000);
                    for (ordinal, thing) in things.items().iter().enumerate() {
                        assert_eq!(thing.iid(), format!("0x{:04x}", ordinal + 1));
                    }
                }
                let state = state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Write]);
                assert_eq!(
                    state
                        .calls
                        .iter()
                        .map(|(_, rows)| rows.rows.len())
                        .collect::<Vec<_>>(),
                    [4096, 904]
                );
                assert_eq!(
                    (state.commits, state.rollbacks),
                    if fail_last { (0, 1) } else { (1, 0) }
                );
                assert!(state.responses.is_empty());
            }
        }
    }
}

#[tokio::test]
async fn wide_unordered_fields_use_only_forced_pieces_and_last_failure_rolls_back() {
    let installed = installed();
    let person = type_id(TypeKind::Entity, "person");
    let tag = field(&person, "tag");
    for count in [253, 254, 300] {
        for operation in [
            ProjectedBatchOperation::Insert,
            ProjectedBatchOperation::Put,
            ProjectedBatchOperation::Update,
        ] {
            let boundary = if operation == ProjectedBatchOperation::Update {
                253
            } else {
                254
            };
            for fail_last in [false, true] {
                let values = (0..count)
                    .map(|index| string_value(&installed, "tag", &format!("v-{index:04}")))
                    .collect();
                let create = ProjectedCreate::try_new(
                    &installed,
                    person.clone(),
                    vec![
                        (
                            field(&person, "identifier"),
                            vec![string_value(&installed, "identifier", "wide")],
                        ),
                        (tag.clone(), values),
                    ],
                    vec![],
                )
                .unwrap();
                let row = if operation == ProjectedBatchOperation::Update {
                    ProjectedBatchRow::Update {
                        iid: "0x10".into(),
                        replacement: create,
                    }
                } else {
                    ProjectedBatchRow::Create(create)
                };
                let batch =
                    ProjectedBatch::try_new(&installed, person.clone(), operation, vec![row])
                        .unwrap();
                let mut document = person_document(0, "0x10", "wide");
                document["attributes"]["tag"] = (0..count)
                    .map(|index| json!({"value":format!("v-{index:04}")}))
                    .collect::<Vec<_>>()
                    .into();
                let mut responses = Vec::new();
                if count > boundary {
                    responses.push(Response::Documents(vec![
                        json!({"ordinal":0,"iid":"0x10","_type":"person"}),
                    ]));
                }
                responses.push(if fail_last {
                    Response::ProviderAfter(vec![document])
                } else {
                    Response::Documents(vec![document])
                });
                let (database, state) = fixture(
                    responses,
                    Version::new(3, 13, 6),
                    true,
                    CommitBehavior::Success,
                );
                let result = ProjectedBatchExecutor::new(&installed)
                    .execute(&database, &batch, control())
                    .await;
                if fail_last {
                    assert_eq!(
                        result.unwrap_err().code().as_str(),
                        "provider_operation_failed"
                    );
                } else {
                    let ProjectedBatchResult::Things(things) = result.unwrap_or_else(|error| {
                        panic!("count={count} operation={operation:?}: {error:?}")
                    }) else {
                        panic!("write result")
                    };
                    assert_eq!(things.items()[0].fields()[&tag].len(), count);
                    let actual = things.items()[0].fields()[&tag]
                        .iter()
                        .map(|value| value.value().clone())
                        .collect::<std::collections::BTreeSet<_>>();
                    let expected = (0..count)
                        .map(|index| {
                            CanonicalValue::String(
                                CanonicalString::new(format!("v-{index:04}")).unwrap(),
                            )
                        })
                        .collect();
                    assert_eq!(actual, expected);
                }
                let state = state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Write]);
                assert_eq!(state.calls.len(), if count <= boundary { 1 } else { 2 });
                assert_eq!(
                    state
                        .calls
                        .iter()
                        .filter(|(query, _)| query.contains("fetch"))
                        .count(),
                    1
                );
                if count > boundary {
                    assert!(!state.calls[0].0.contains("fetch"));
                }
                assert_eq!(
                    (state.commits, state.rollbacks),
                    if fail_last { (0, 1) } else { (1, 0) }
                );
            }
        }
    }
}

#[tokio::test]
async fn wide_unordered_players_use_only_forced_pieces_and_last_failure_rolls_back() {
    let installed = installed_with_schema(
        &SCHEMA.replace(
            "member: { card: { min: 0, max: 4 } }",
            "member: { card: { min: 0 } }",
        ),
        ProjectionHandler::python_v1(),
    );
    let membership = type_id(TypeKind::Relation, "membership");
    let person = type_id(TypeKind::Entity, "person");
    let member = RoleId::new("membership", "member").unwrap();
    for count in [253_usize, 254, 300] {
        for operation in [
            ProjectedBatchOperation::Insert,
            ProjectedBatchOperation::Put,
            ProjectedBatchOperation::Update,
        ] {
            let boundary = if operation == ProjectedBatchOperation::Update {
                253
            } else {
                254
            };
            for fail_last in [false, true] {
                let players = (0..count)
                    .map(|index| {
                        ProjectedReference::try_new(
                            &installed,
                            person.clone(),
                            Some(format!("0x{:04x}", index + 1)),
                            vec![],
                        )
                        .unwrap()
                    })
                    .collect();
                let create = ProjectedCreate::try_new(
                    &installed,
                    membership.clone(),
                    vec![(
                        field(&membership, "identifier"),
                        vec![string_value(&installed, "identifier", "wide-role")],
                    )],
                    vec![(member.clone(), players)],
                )
                .unwrap();
                let row = if operation == ProjectedBatchOperation::Update {
                    ProjectedBatchRow::Update {
                        iid: "0x8000".into(),
                        replacement: create,
                    }
                } else {
                    ProjectedBatchRow::Create(create)
                };
                let batch =
                    ProjectedBatch::try_new(&installed, membership.clone(), operation, vec![row])
                        .unwrap();
                let mut document = membership_document(0, "0x8000", "wide-role", "0x01");
                document["role_players"] = (0..count).map(|index| json!({"role":"member", "iid":format!("0x{:04x}",index+1), "type_name":"person", "attributes":{"identifier":[{"value":format!("p-{index:04}")}]}})).collect::<Vec<_>>().into();
                let mut responses = Vec::new();
                let guard_count = if count <= boundary {
                    0
                } else if count == 300 {
                    5
                } else {
                    4
                };
                if guard_count > 0 {
                    responses.push(Response::ReferenceGuard((1..count).map(|index| json!({"ordinal":index,"iid":format!("0x{:04x}", index + 1),"_type":"person"})).collect()));
                }
                if count > boundary {
                    responses.push(Response::Documents(vec![
                        json!({"ordinal":0,"iid":"0x8000","_type":"membership"}),
                    ]));
                }
                responses.push(if fail_last {
                    Response::ProviderAfter(vec![document])
                } else {
                    Response::Documents(vec![document])
                });
                let (database, state) = fixture(
                    responses,
                    Version::new(3, 13, 6),
                    true,
                    CommitBehavior::Success,
                );
                let result = ProjectedBatchExecutor::new(&installed)
                    .execute(&database, &batch, control())
                    .await;
                if fail_last {
                    assert_eq!(
                        result.unwrap_err().code().as_str(),
                        "provider_operation_failed"
                    );
                } else {
                    let ProjectedBatchResult::Things(things) = result.unwrap_or_else(|error| {
                        panic!("count={count} operation={operation:?}: {error:?}")
                    }) else {
                        panic!("write result")
                    };
                    assert_eq!(things.items()[0].roles()[&member].len(), count);
                    let actual = things.items()[0].roles()[&member]
                        .iter()
                        .map(|player| player.iid().to_owned())
                        .collect::<std::collections::BTreeSet<_>>();
                    let expected = (0..count)
                        .map(|index| format!("0x{:04x}", index + 1))
                        .collect();
                    assert_eq!(actual, expected);
                }
                let state = state.lock().unwrap();
                assert_eq!(state.opens, [TxType::Write]);
                assert_eq!(
                    state.calls.len(),
                    if count <= boundary {
                        1
                    } else {
                        2 + guard_count
                    }
                );
                assert_eq!(
                    state
                        .calls
                        .iter()
                        .filter(|(query, _)| query.contains("fetch"))
                        .count(),
                    1
                );
                assert_eq!(
                    (state.commits, state.rollbacks),
                    if fail_last { (0, 1) } else { (1, 0) }
                );
            }
        }
    }
}

#[tokio::test]
async fn homogeneous_relation_put_hits_and_misses_keep_one_provider_call() {
    for hit in [false, true] {
        let installed = installed();
        let batch = ProjectedBatch::try_new(
            &installed,
            type_id(TypeKind::Relation, "membership"),
            ProjectedBatchOperation::Put,
            vec![ProjectedBatchRow::Create(membership_create(
                &installed, "m", "0x10",
            ))],
        )
        .unwrap();
        let iid = if hit { "0x30" } else { "0x31" };
        let (database, state) = fixture(
            vec![Response::Documents(vec![membership_document(
                0, iid, "m", "0x10",
            )])],
            Version::new(3, 13, 6),
            true,
            CommitBehavior::Success,
        );
        let ProjectedBatchResult::Things(things) = ProjectedBatchExecutor::new(&installed)
            .execute(&database, &batch, control())
            .await
            .unwrap()
        else {
            panic!("put result");
        };
        assert_eq!(things.items()[0].iid(), iid);
        let state = state.lock().unwrap();
        assert_eq!(state.calls.len(), 1);
        assert!(state.calls[0].0.contains("put $root0 isa! membership"));
        assert!(state.calls[0].0.contains("$root0 links (member:"));
        assert_eq!((state.commits, state.rollbacks), (1, 0));
    }
}

#[tokio::test]
async fn ordered_role_returning_delete_keeps_empty_roles_and_refuses_population_before_commit() {
    let installed = installed_with_ordered_role();
    let role = RoleId::new("membership", "participant").unwrap();
    for populated in [false, true] {
        let batch = ProjectedBatch::try_new(
            &installed,
            type_id(TypeKind::Relation, "membership"),
            ProjectedBatchOperation::Delete,
            vec![ProjectedBatchRow::Delete { iid: "0x20".into() }],
        )
        .unwrap();
        let mut document = membership_document(0, "0x20", "m-1", "0xab");
        document["role_players"][0]["role"] = json!("membership:member");
        if populated {
            document["role_players"]
                .as_array_mut()
                .unwrap()
                .push(json!({
                    "role": "membership:participant", "iid": "0xac", "type_name": "person",
                    "attributes": {"identifier": [{"value": "ordered-player"}]},
                }));
        }
        let (database, state) = fixture(
            vec![
                Response::Documents(vec![document]),
                Response::Documents(vec![]),
            ],
            Version::new(3, 13, 6),
            true,
            CommitBehavior::Success,
        );
        let result = ProjectedBatchExecutor::new(&installed)
            .returning_deleted()
            .execute(&database, &batch, control())
            .await;
        if populated {
            assert_eq!(
                result.unwrap_err().code().as_str(),
                "query_plan_hydration_ordered_role"
            );
            let state = state.lock().unwrap();
            assert_eq!(state.calls.len(), 1);
            assert!(!state.calls[0].0.contains("delete"));
            assert_eq!((state.commits, state.rollbacks), (0, 1));
        } else {
            let ProjectedBatchResult::Things(things) = result.unwrap() else {
                panic!("returning deletion")
            };
            assert_eq!(things.count(), 1);
            assert_eq!(things.items()[0].iid(), "0x20");
            assert!(things.items()[0].is_deleted());
            assert!(things.items()[0].roles()[&role].is_empty());
            let state = state.lock().unwrap();
            assert_eq!(state.calls.len(), 2);
            assert!(!state.calls[0].0.contains("delete"));
            assert!(state.calls[1].0.contains("delete"));
            assert_eq!((state.commits, state.rollbacks), (1, 0));
        }
    }
}

#[tokio::test]
async fn returning_batch_captures_all_chunks_before_any_deletion() {
    let installed = installed();
    let batch = ProjectedBatch::try_new(
        &installed,
        type_id(TypeKind::Entity, "person"),
        ProjectedBatchOperation::Delete,
        (0..5000)
            .map(|ordinal| ProjectedBatchRow::Delete {
                iid: format!("0x{:04x}", ordinal + 1),
            })
            .collect(),
    )
    .unwrap();
    for fail_last in [false, true] {
        let mut responses = Vec::new();
        for range in [0..4096, 4096..5000] {
            responses.push(Response::Documents(
                range
                    .map(|ordinal| {
                        person_document(
                            ordinal,
                            &format!("0x{:04x}", ordinal + 1),
                            &format!("p-{ordinal}"),
                        )
                    })
                    .collect(),
            ));
        }
        responses.push(Response::Documents(vec![]));
        responses.push(if fail_last {
            Response::ProviderAfter(vec![])
        } else {
            Response::Documents(vec![])
        });
        let (database, state) = fixture(
            responses,
            Version::new(3, 13, 6),
            true,
            CommitBehavior::Success,
        );
        let result = ProjectedBatchExecutor::new(&installed)
            .returning_deleted()
            .execute(&database, &batch, control())
            .await;
        if fail_last {
            assert_eq!(
                result.unwrap_err().code().as_str(),
                "provider_operation_failed"
            );
            let state = state.lock().unwrap();
            assert_eq!(state.opens, [TxType::Write]);
            assert_eq!(state.calls.len(), 4);
            assert!(
                state.calls[..2]
                    .iter()
                    .all(|(query, _)| query.contains("fetch") && !query.contains("delete"))
            );
            assert_eq!((state.commits, state.rollbacks), (0, 1));
            continue;
        }
        let ProjectedBatchResult::Things(things) = result.unwrap() else {
            panic!("returning deletion");
        };
        assert_eq!(things.count(), 5000);
        for (ordinal, thing) in things.items().iter().enumerate() {
            assert!(thing.is_deleted());
            assert_eq!(thing.iid(), format!("0x{:04x}", ordinal + 1));
        }
        let state = state.lock().unwrap();
        assert_eq!(state.calls.len(), 4);
        assert!(
            state.calls[..2]
                .iter()
                .all(|(query, _)| query.contains("fetch") && !query.contains("delete"))
        );
        assert!(
            state.calls[2..]
                .iter()
                .all(|(query, _)| query.contains("delete"))
        );
        assert_eq!(
            state
                .calls
                .iter()
                .map(|(_, rows)| rows.rows.len())
                .collect::<Vec<_>>(),
            [4096, 904, 4096, 904]
        );
        assert_eq!((state.commits, state.rollbacks), (1, 0));
    }
}

#[tokio::test]
async fn returning_capture_keeps_the_first_malformed_item_over_a_later_provider_failure() {
    let installed = installed();
    let batch = ProjectedBatch::try_new(
        &installed,
        type_id(TypeKind::Entity, "person"),
        ProjectedBatchOperation::Delete,
        vec![ProjectedBatchRow::Delete { iid: "0x10".into() }],
    )
    .unwrap();
    let mut malformed = person_document(0, "0x10", "p");
    malformed.as_object_mut().unwrap().remove("attributes");
    let (database, state) = fixture(
        vec![Response::ProviderAfter(vec![malformed])],
        Version::new(3, 13, 6),
        true,
        CommitBehavior::Success,
    );
    let error = ProjectedBatchExecutor::new(&installed)
        .returning_deleted()
        .execute(&database, &batch, control())
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "query_remote_document_value");
    let state = state.lock().unwrap();
    assert_eq!(state.calls.len(), 1);
    assert_eq!((state.commits, state.rollbacks), (0, 1));
}

#[tokio::test]
async fn key_changing_self_updates_verify_source_iids_without_an_extra_statement() {
    let installed = installed_with_schema(
        "format: typebridge.schema/v2\nattributes:\n  identifier: { value: string }\nrelations:\n  membership:\n    owns:\n      identifier: { key: true }\n    relates:\n      member: { card: { min: 0, max: 1 } }\nplays:\n  membership:\n    membership: [member]\n",
        ProjectionHandler::python_v1(),
    );
    let membership = type_id(TypeKind::Relation, "membership");
    let member = RoleId::new("membership", "member").unwrap();
    let key = field(&membership, "identifier");
    for by_iid in [false, true] {
        for (source_iid, returned_iid, accepted) in [
            ("0x20", "0x20", true),
            ("0x99", "0x20", false),
            ("0x20", "0x99", false),
        ] {
            if by_iid && source_iid != "0x20" {
                continue;
            }
            let reference = ProjectedReference::try_new(
                &installed,
                membership.clone(),
                by_iid.then(|| "0x20".to_owned()),
                vec![(key.clone(), string_value(&installed, "identifier", "old"))],
            )
            .unwrap();
            let replacement = ProjectedCreate::try_new(
                &installed,
                membership.clone(),
                vec![(
                    key.clone(),
                    vec![string_value(&installed, "identifier", "new")],
                )],
                vec![(member.clone(), vec![reference])],
            )
            .unwrap();
            let batch = ProjectedBatch::try_new(
                &installed,
                membership.clone(),
                ProjectedBatchOperation::Update,
                vec![ProjectedBatchRow::Update {
                    iid: "0x20".into(),
                    replacement,
                }],
            )
            .unwrap();
            let mut document = json!({"ordinal":0,"_iid":"0x20","_type":"membership","attributes":{"identifier":[{"value":"new"}]},"role_players":[{"role":"member","iid":returned_iid,"type_name":"membership","attributes":{"identifier":[{"value":"new"}]}}],"reference_0_2":source_iid});
            if by_iid {
                document.as_object_mut().unwrap().remove("reference_0_2");
            }
            let (database, state) = fixture(
                vec![Response::Documents(vec![document])],
                Version::new(3, 13, 6),
                true,
                CommitBehavior::Success,
            );
            let result = ProjectedBatchExecutor::new(&installed)
                .execute(&database, &batch, control())
                .await;
            if accepted {
                result.unwrap();
            } else {
                assert_eq!(
                    result.unwrap_err().code().as_str(),
                    "provider_identity_mismatch"
                );
            }
            let state = state.lock().unwrap();
            assert_eq!(state.calls.len(), 1);
            assert_eq!(
                state.calls[0]
                    .0
                    .contains("\"reference_0_2\": iid($player2)"),
                !by_iid
            );
            assert_eq!(
                (state.commits, state.rollbacks),
                if accepted { (1, 0) } else { (0, 1) }
            );
        }
    }
}

#[tokio::test]
async fn raised_query_dimensions_keep_wide_projected_batches_in_one_statement() {
    let installed = installed();
    let person = type_id(TypeKind::Entity, "person");
    let tag = field(&person, "tag");
    let mut limits = type_bridge_contract::limits::QueryLimits::PLAN;
    limits.structural.bindings = 1024;
    limits.structural.boolean_terms = 512;
    for given in [false, true] {
        for operation in [
            ProjectedBatchOperation::Insert,
            ProjectedBatchOperation::Put,
            ProjectedBatchOperation::Update,
        ] {
            let create = ProjectedCreate::try_new(
                &installed,
                person.clone(),
                vec![
                    (
                        field(&person, "identifier"),
                        vec![string_value(&installed, "identifier", "wide")],
                    ),
                    (
                        tag.clone(),
                        (0..300)
                            .map(|index| string_value(&installed, "tag", &format!("v-{index:04}")))
                            .collect(),
                    ),
                ],
                vec![],
            )
            .unwrap();
            let row = if operation == ProjectedBatchOperation::Update {
                ProjectedBatchRow::Update {
                    iid: "0x10".into(),
                    replacement: create,
                }
            } else {
                ProjectedBatchRow::Create(create)
            };
            let batch =
                ProjectedBatch::try_new(&installed, person.clone(), operation, vec![row]).unwrap();
            let mut document = person_document(0, "0x10", "wide");
            document["attributes"]["tag"] = (0..300)
                .map(|index| json!({"value":format!("v-{index:04}")}))
                .collect::<Vec<_>>()
                .into();
            let (database, state) = fixture(
                vec![Response::Documents(vec![document])],
                Version::new(3, 13, 6),
                given,
                CommitBehavior::Success,
            );
            let database = database.with_query_limits(limits).unwrap();
            let ProjectedBatchResult::Things(things) = ProjectedBatchExecutor::new(&installed)
                .execute(&database, &batch, control())
                .await
                .unwrap()
            else {
                panic!("write result")
            };
            assert_eq!(things.items()[0].fields()[&tag].len(), 300);
            let state = state.lock().unwrap();
            assert_eq!(state.opens, [TxType::Write]);
            assert_eq!(
                state.calls.len(),
                1,
                "operation={operation:?} given={given}"
            );
            assert_eq!((state.commits, state.rollbacks), (1, 0));
        }
    }
}

#[tokio::test]
async fn missing_wide_player_rolls_back_before_allocation() {
    let installed = installed_with_schema(
        &SCHEMA.replace("  membership:\n    owns:\n      identifier: { key: true }", "  membership:\n    owns:\n      identifier: { key: true }\n      tag: { card: { min: 0 } }"),
        ProjectionHandler::python_v1(),
    );
    let membership = type_id(TypeKind::Relation, "membership");
    let person = type_id(TypeKind::Entity, "person");
    let member = RoleId::new("membership", "member").unwrap();
    let create = ProjectedCreate::try_new(
        &installed,
        membership.clone(),
        vec![
            (
                field(&membership, "identifier"),
                vec![string_value(
                    &installed,
                    "identifier",
                    "missing-wide-player",
                )],
            ),
            (
                field(&membership, "tag"),
                (0..300)
                    .map(|index| string_value(&installed, "tag", &format!("tag-{index}")))
                    .collect(),
            ),
        ],
        vec![(
            member.clone(),
            (1..=3)
                .map(|index| {
                    ProjectedReference::try_new(
                        &installed,
                        person.clone(),
                        Some(format!("0x{index:04x}")),
                        vec![],
                    )
                    .unwrap()
                })
                .collect(),
        )],
    )
    .unwrap();
    let batch = ProjectedBatch::try_new(
        &installed,
        membership.clone(),
        ProjectedBatchOperation::Insert,
        vec![ProjectedBatchRow::Create(create)],
    )
    .unwrap();
    let (database, state) = fixture(
        vec![Response::ReferenceGuard(vec![
            json!({"ordinal":1,"iid":"0x0002","_type":"person"}),
        ])],
        Version::new(3, 13, 6),
        true,
        CommitBehavior::Success,
    );
    let error = ProjectedBatchExecutor::new(&installed)
        .execute(&database, &batch, control())
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "projected_batch_reference_missing");
    assert_eq!(error.category(), SdkDiagnosticCategory::Integrity);
    assert_eq!(
        error.path(),
        &[
            SdkDiagnosticPathSegment::Argument(
                type_bridge_contract::sdk_diagnostic::SdkDiagnosticName::new("rows").unwrap()
            ),
            SdkDiagnosticPathSegment::Index(0),
            SdkDiagnosticPathSegment::Type(membership),
            SdkDiagnosticPathSegment::Role(member),
            SdkDiagnosticPathSegment::Index(2),
            SdkDiagnosticPathSegment::Type(person),
        ]
    );
    let state = state.lock().unwrap();
    assert_eq!(state.opens, [TxType::Write]);
    assert_eq!(state.calls.len(), 1);
    assert!(!state.calls[0].0.contains("insert"));
    assert_eq!((state.commits, state.rollbacks), (0, 1));
}
