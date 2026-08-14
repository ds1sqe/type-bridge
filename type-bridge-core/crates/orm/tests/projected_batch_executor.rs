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
    AnswerConsumer, AnswerControl, AnswerItem, BoundedAnswerLimits, BoundedAnswerReader,
    BoundedAnswerStats, BoxFuture, DriverBackend, GivenRowsSpec, QueryResult, TransactionOps,
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
    Documents(Vec<Value>),
    ProviderAfter(Vec<Value>),
    CancelAfter(Vec<Value>, AnswerCancellation),
    PendingAfter(Vec<Value>),
    Underreported(Vec<Value>),
    Error(&'static str),
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
                .expect("unexpected projected batch provider call")
        };
        Box::pin(async move {
            let mut reader = BoundedAnswerReader::new(limits);
            reader.check_before_read()?;
            let mut deliver = |documents: Vec<Value>| -> Result<(), OrmError> {
                for document in documents {
                    if reader.accept(AnswerItem::Document(document), consumer)?
                        == AnswerControl::Stop
                    {
                        break;
                    }
                }
                Ok(())
            };
            match response {
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
                        if consumer.accept(AnswerItem::Document(document))? == AnswerControl::Stop {
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
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("projected-batch-executor.yaml").unwrap(),
        SCHEMA,
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
            &[ProjectionHandler::python_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap()
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

fn roleless_membership_create(
    installed: &InstalledRuntimeProjection,
    identifier: &str,
) -> ProjectedCreate {
    let membership = type_id(TypeKind::Relation, "membership");
    ProjectedCreate::try_new(
        installed,
        membership.clone(),
        vec![(
            field(&membership, "identifier"),
            vec![string_value(installed, "identifier", identifier)],
        )],
        vec![],
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

fn prerequisite_documents() -> Vec<Value> {
    (0..3)
        .map(|ordinal| {
            json!({
                "kind": 1,
                "ordinal": ordinal,
                "reference_ordinal": ordinal,
                "iid": format!("0x1{ordinal}"),
                "type": "person"
            })
        })
        .collect()
}

fn mutation_documents(kind: BatchKind) -> Vec<Value> {
    let prefix = match kind {
        BatchKind::Entity => "0x2",
        BatchKind::Relation => "0x3",
    };
    (0..3)
        .map(|ordinal| json!({"ordinal": ordinal, "iid": format!("{prefix}{ordinal}")}))
        .collect()
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
    let mut responses = Vec::new();
    if matches!(kind, BatchKind::Relation) {
        responses.push(Response::Documents(prerequisite_documents()));
    }
    responses.push(Response::Documents(mutation_documents(kind)));
    responses.push(Response::Documents(hydration_documents(kind)));
    responses
}

fn assert_owned_atomic_failure(state: &Arc<Mutex<State>>, expected_calls: usize) {
    let state = state.lock().unwrap();
    assert_eq!(state.calls.len(), expected_calls);
    assert_eq!(state.rollbacks, 1);
    assert_eq!(state.commits, 0);
    assert!(state.responses.is_empty());
}

#[tokio::test]
async fn empty_and_band8_reject_without_transaction_or_provider_work() {
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
        ProjectedBatchResult::Things(vec![])
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
        vec![
            Response::Documents(vec![
                json!({"ordinal": 2, "iid": "0x22"}),
                json!({"ordinal": 0, "iid": "0x20"}),
                json!({"ordinal": 1, "iid": "0x21"}),
            ]),
            Response::Documents(vec![
                person_document(1, "0x21", "person-1"),
                person_document(2, "0x22", "person-2"),
                person_document(0, "0x20", "person-0"),
            ]),
        ],
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
                    .into_iter()
                    .map(|thing| thing.iid().to_owned())
                    .collect())
            },
        )
        .await
        .unwrap();

    assert_eq!(mapped, ["0x20", "0x21", "0x22"]);
    let state = state.lock().unwrap();
    assert_eq!(state.calls.len(), 2);
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
                    let mut mapped = Vec::with_capacity(things.len());
                    for (ordinal, thing) in things.into_iter().enumerate() {
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
        assert_eq!(state.calls.len(), 2);
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
    assert_eq!(state.calls.len(), 2);
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
                let mut mapped = Vec::with_capacity(things.len());
                for (ordinal, thing) in things.into_iter().enumerate() {
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
    assert_eq!(state.calls.len(), 2);
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
    assert_eq!(state.calls.len(), 2);
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
        timeout_milliseconds: 100,
        ..QueryExecutionResourceLimits::default()
    };
    let mapper_ran = Arc::new(AtomicBool::new(false));
    let mapper_probe = Arc::clone(&mapper_ran);

    let diagnostic = ProjectedBatchExecutor::new(&installed)
        .execute_in_transaction_mapped(
            &transaction,
            &batch,
            ProjectedBatchInvocationControl::capture(limits, AnswerCancellation::default()),
            Vec::<String>::new(),
            move |result| {
                mapper_probe.store(true, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(150));
                let ProjectedBatchResult::Things(things) = result else {
                    panic!("insert returned a delete result")
                };
                Ok(things
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
async fn entity_insert_is_two_calls_and_correlates_shuffled_documents_by_ordinal() {
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
        vec![
            Response::Documents(vec![
                json!({"ordinal": 1, "iid": "0x11"}),
                json!({"ordinal": 0, "iid": "0x10"}),
            ]),
            Response::Documents(vec![
                person_document(1, "0x11", "bob"),
                person_document(0, "0x10", "ada"),
            ]),
        ],
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
        things.iter().map(|thing| thing.iid()).collect::<Vec<_>>(),
        ["0x10", "0x11"]
    );
    let state = state.lock().unwrap();
    assert_eq!(state.calls.len(), 2);
    assert_eq!(state.calls[0].1.rows.len(), 2);
    assert!(state.calls[0].0.contains("insert\n$thing isa person"));
    assert!(state.calls[1].0.contains("let $actual-iid = iid($thing)"));
    assert!(!state.calls[1].0.contains("iid $target-iid"));
    assert_eq!(state.commits, 1);
    assert_eq!(state.rollbacks, 0);
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
        vec![
            Response::Documents(vec![json!({
                "kind": 0,
                "ordinal": 0,
                "iid": "0x10",
                "type": "person"
            })]),
            Response::Documents(vec![
                json!({"ordinal": 1, "iid": "0x11"}),
                json!({"ordinal": 0, "iid": "0x10"}),
            ]),
            Response::Documents(vec![
                person_document(0, "0x10", "hit"),
                person_document(1, "0x11", "miss"),
            ]),
        ],
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
        things.iter().map(|thing| thing.iid()).collect::<Vec<_>>(),
        ["0x10", "0x11"]
    );
    let state = state.lock().unwrap();
    assert_eq!(state.calls.len(), 3);
    assert_eq!(state.calls[1].1.rows.len(), 2);
    assert_eq!(state.calls[1].0.matches("\nput\n").count(), 1);
    assert!(state.calls[1].0.contains("has identifier == $put-key-0"));
}

#[tokio::test]
async fn relation_insert_prereads_players_and_exact_binds_relation_before_links() {
    let installed = installed();
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
        vec![
            Response::Documents(vec![json!({
                "kind": 1,
                "ordinal": 0,
                "reference_ordinal": 0,
                "iid": "0xAB",
                "type": "person"
            })]),
            Response::Documents(vec![json!({"ordinal": 0, "iid": "0x20"})]),
            Response::Documents(vec![{
                let mut document = membership_document(0, "0x20", "m-1", "0xab");
                document["role_players"][0]["role"] = json!("membership:member");
                document
            }]),
        ],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );

    ProjectedBatchExecutor::new(&installed)
        .execute(&database, &batch, control())
        .await
        .unwrap();
    let state = state.lock().unwrap();
    assert_eq!(state.calls.len(), 3);
    assert!(matches!(
        &state.calls[0].1.rows[0][4],
        type_bridge_orm::session::backend::GivenValue::String(iid) if iid == "0xab"
    ));
    assert!(
        state.calls[0]
            .0
            .contains("$thing isa! person;\n  let $actual-iid = iid($thing)")
    );
    let attach = &state.calls[2].0;
    let exact = attach.find("$thing isa! membership;").unwrap();
    let links = attach.find("$thing links (member:").unwrap();
    assert!(exact < links);
    assert!(attach.contains("reduce $event-count = count groupby $ordinal, $thing, $thing-type"));
    assert!(state.calls[2].1.rows.iter().flatten().any(
        |value| matches!(value, type_bridge_orm::session::backend::GivenValue::String(iid) if iid == "0xab")
    ));
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
        vec![
            Response::Documents(vec![json!({
                "kind": 1,
                "ordinal": 0,
                "reference_ordinal": 0,
                "iid": "0x10",
                "type": "person"
            })]),
            Response::Documents(vec![json!({"ordinal": 0, "iid": "0x20"})]),
            Response::Documents(vec![scoped_document()]),
        ],
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
        let key_column = state.calls[0]
            .1
            .variables
            .iter()
            .position(|variable| variable == "reference-key-0")
            .unwrap();
        assert!(matches!(
            &state.calls[0].1.rows[0][key_column],
            type_bridge_orm::session::backend::GivenValue::String(value) if value == "player-key"
        ));
        assert!(
            state.calls[0]
                .0
                .contains("$thing has identifier == $reference-key-0;")
        );
    }

    let (database, state) = fixture(
        vec![
            Response::Documents(vec![json!({
                "kind": 1,
                "ordinal": 0,
                "reference_ordinal": 0,
                "iid": "0x10",
                "type": "person"
            })]),
            Response::Documents(vec![json!({"ordinal": 0, "iid": "0x20"})]),
            Response::Documents(vec![{
                let mut document = scoped_document();
                document["role_players"][0]["role"] = json!("other:member");
                document
            }]),
        ],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    let diagnostic = ProjectedBatchExecutor::new(&installed)
        .execute(&database, &batch, control())
        .await
        .unwrap_err();
    assert_eq!(diagnostic.code().as_str(), "provider_hydration_failed");
    let state = state.lock().unwrap();
    assert_eq!(state.calls.len(), 3);
    assert_eq!(state.rollbacks, 1);
    assert_eq!(state.commits, 0);
}

#[tokio::test]
async fn relation_put_is_three_calls_with_one_multirow_put_and_exact_replacement() {
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
        vec![
            Response::Documents(vec![
                json!({"kind": 1, "ordinal": 1, "reference_ordinal": 1, "iid": "0x11", "type": "person"}),
                json!({"kind": 0, "ordinal": 0, "iid": "0x30", "type": "membership"}),
                json!({"kind": 1, "ordinal": 0, "reference_ordinal": 0, "iid": "0x10", "type": "person"}),
            ]),
            Response::Documents(vec![
                json!({"ordinal": 1, "iid": "0x31"}),
                json!({"ordinal": 0, "iid": "0x30"}),
            ]),
            Response::Documents(vec![
                scoped_document(1, "0x31", "miss", "0x11"),
                scoped_document(0, "0x30", "hit", "0x10"),
            ]),
        ],
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
        things.iter().map(|thing| thing.iid()).collect::<Vec<_>>(),
        ["0x30", "0x31"]
    );
    let state = state.lock().unwrap();
    assert_eq!(state.calls.len(), 3);
    assert_eq!(state.calls[0].1.rows.len(), 4);
    assert_eq!(state.calls[1].0.matches("\nput\n").count(), 1);
    assert_eq!(state.calls[1].1.rows.len(), 2);
    assert!(state.calls[1].0.contains("$thing isa! membership;"));
    assert!(
        state.calls[1]
            .0
            .contains("delete try { links (member: $old-player-0) of $thing; }")
    );
    assert!(state.responses.is_empty());
}

#[tokio::test]
async fn relation_update_is_three_calls_and_clears_keys_and_roles() {
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
        vec![
            Response::Documents(vec![json!({
                "kind": 1,
                "ordinal": 0,
                "reference_ordinal": 0,
                "iid": "0x11",
                "type": "person"
            })]),
            Response::Documents(vec![json!({"ordinal": 0, "iid": "0x30"})]),
            Response::Documents(vec![hydrated]),
        ],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    ProjectedBatchExecutor::new(&installed)
        .execute(&database, &batch, control())
        .await
        .unwrap();
    let state = state.lock().unwrap();
    assert_eq!(state.calls.len(), 3);
    assert!(state.calls[1].0.contains("has $old-attribute-0 of $thing"));
    assert!(
        state.calls[1]
            .0
            .contains("delete try { links (member: $old-player-0) of $thing; }")
    );
    assert!(state.calls[2].0.contains("$thing links (member:"));
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
    assert!(state.calls[0].0.contains("$thing isa! membership;"));
    assert!(state.calls[0].0.contains("delete try { $thing; }"));
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
    assert!(state.calls[0].0.contains("delete try { $thing; }"));
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
    assert_eq!(state.calls.len(), 2);
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
        vec![ProjectedBatchRow::Create(person_create(&installed, "ada"))],
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
    assert_eq!(
        diagnostic.code().as_str(),
        "projected_batch_reply_item_limit"
    );
    let state = state.lock().unwrap();
    assert!(state.calls.is_empty());
    assert_eq!(state.rollbacks, 1);
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
async fn update_is_two_calls_clears_keys_and_rejects_a_changed_target_iid() {
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
        vec![
            Response::Documents(vec![json!({"ordinal": 0, "iid": "0xab"})]),
            Response::Documents(vec![person_document(0, "0xab", "new-key")]),
        ],
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
        assert_eq!(state.calls.len(), 2);
        assert!(matches!(
            &state.calls[0].1.rows[0][1],
            type_bridge_orm::session::backend::GivenValue::String(iid) if iid == "0xab"
        ));
        assert!(state.calls[0].0.contains("$thing isa! person;"));
        assert!(state.calls[0].0.contains("has $old-attribute-0 of $thing"));
    }

    let (database, state) = fixture(
        vec![Response::Documents(vec![json!({
            "ordinal": 0,
            "iid": "0x11"
        })])],
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
async fn borrowed_success_stays_active_and_band8_failure_does_not_poison() {
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
        vec![
            Response::Documents(vec![json!({"ordinal": 0, "iid": "0x10"})]),
            Response::Documents(vec![person_document(0, "0x10", "ada")]),
        ],
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
    assert_eq!(
        diagnostic.code().as_str(),
        "projected_batch_mutation_identity_missing"
    );
    assert_eq!(
        transaction.rollback_only_cause().await.unwrap().code(),
        diagnostic.code()
    );
    assert_eq!(state.lock().unwrap().calls.len(), 1);
}

#[tokio::test]
async fn iid_reference_preread_rejects_a_different_provider_identity_before_mutation() {
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
        vec![Response::Documents(vec![json!({
            "kind": 1,
            "ordinal": 0,
            "reference_ordinal": 0,
            "iid": "0x11",
            "type": "person"
        })])],
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
        TransactionContextState::Active
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
                    roleless_membership_create(&installed, &format!("invalid-{ordinal}"))
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
        assert_eq!(diagnostic.code().as_str(), "relation_requires_role_player");
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
            let mut responses = Vec::new();
            if matches!(kind, BatchKind::Relation) {
                responses.push(Response::Documents(prerequisite_documents()));
            }
            responses.push(Response::ProviderAfter(
                mutation_documents(kind)[..failure_position].to_vec(),
            ));
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
            assert_owned_atomic_failure(
                &state,
                if matches!(kind, BatchKind::Relation) {
                    2
                } else {
                    1
                },
            );
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
            if matches!(kind, BatchKind::Relation) {
                responses.push(Response::Documents(prerequisite_documents()));
            }
            responses.push(Response::Documents(mutation_documents(kind)));
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
            assert_owned_atomic_failure(
                &state,
                if matches!(kind, BatchKind::Relation) {
                    3
                } else {
                    2
                },
            );
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
            let mut responses = Vec::new();
            if matches!(kind, BatchKind::Relation) {
                responses.push(Response::Documents(prerequisite_documents()));
            }
            responses.push(Response::CancelAfter(
                mutation_documents(kind)[..interruption_position].to_vec(),
                cancellation.clone(),
            ));
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
            assert_owned_atomic_failure(
                &state,
                if matches!(kind, BatchKind::Relation) {
                    2
                } else {
                    1
                },
            );
        }
    }
}

#[tokio::test]
async fn first_middle_last_inflight_timeout_rolls_back_both_batch_kinds() {
    let installed = installed();
    for kind in [BatchKind::Entity, BatchKind::Relation] {
        for interruption_position in 0..3 {
            let batch = three_row_batch(&installed, kind);
            let mut responses = Vec::new();
            if matches!(kind, BatchKind::Relation) {
                responses.push(Response::Documents(prerequisite_documents()));
            }
            responses.push(Response::PendingAfter(
                mutation_documents(kind)[..interruption_position].to_vec(),
            ));
            let (database, state) = fixture(
                responses,
                Version::new(3, 12, 1),
                true,
                CommitBehavior::Success,
            );
            let limits = QueryExecutionResourceLimits {
                timeout_milliseconds: 50,
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
            assert_owned_atomic_failure(
                &state,
                if matches!(kind, BatchKind::Relation) {
                    2
                } else {
                    1
                },
            );
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
        vec![
            Response::Documents(vec![json!({"ordinal": 0, "iid": "0x10"})]),
            Response::Underreported(vec![
                person_document(0, "0x10", "ada"),
                person_document(0, "0x10", "ada"),
            ]),
        ],
        Version::new(3, 12, 1),
        true,
        CommitBehavior::Success,
    );
    let limits = QueryExecutionResourceLimits {
        items: 2,
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
    assert_owned_atomic_failure(&state, 2);
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
        vec![
            Response::Documents(vec![json!({"ordinal": 0, "iid": "0x10"})]),
            Response::Underreported(vec![person_document(0, "0x10", &"x".repeat(131_072))]),
        ],
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
    assert_owned_atomic_failure(&state, 2);

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
        vec![
            Response::Documents(vec![json!({
                "kind": 1,
                "ordinal": 0,
                "reference_ordinal": 0,
                "iid": "0x10",
                "type": "person"
            })]),
            Response::Documents(vec![json!({"ordinal": 0, "iid": "0x30"})]),
            Response::Documents(vec![output]),
        ],
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
    assert_owned_atomic_failure(&state, 3);
}
