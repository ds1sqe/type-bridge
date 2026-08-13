//! Generated-only exact entity CRUD exports.

use crate::abi::{TypeBridgeByteView, TypeBridgeStatus};
use crate::execution_diagnostic::TypeBridgeExecutionDiagnostics;
use crate::projected_model::{TypeBridgeProjectedCreate, TypeBridgeProjectedThing};
use crate::projected_token::TypeBridgeProjectedTokenV1;
use crate::runtime::{
    TypeBridgeCancellation, TypeBridgeDatabase, TypeBridgeReadTransaction,
    TypeBridgeWriteTransaction,
};
use crate::thing_crud::{
    CrudKind, MemoryRange, ReadTarget, WriteTarget, prepare_count_outputs,
    prepare_diagnostics_output, prepare_thing_outputs, run_count_call, run_delete_call,
    run_get_call, run_put_call, run_write_thing_call,
};

/// Insert one exact projected entity using one owned write transaction and commit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_entity_insert(
    database: *const TypeBridgeDatabase,
    model: *const TypeBridgeProjectedTokenV1,
    create: *const TypeBridgeProjectedCreate,
    cancellation: *const TypeBridgeCancellation,
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller input ranges are described before any output write.
    let prepared = match unsafe {
        prepare_thing_outputs(
            &[
                MemoryRange::of_object(database),
                MemoryRange::of_object(model),
                MemoryRange::of_object(create),
                MemoryRange::of_object(cancellation),
            ],
            out_thing,
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if database.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller retains the database handle and all delegated inputs for this call.
    unsafe {
        run_write_thing_call(
            CrudKind::Entity,
            WriteTarget::Database(&*database),
            prepared,
            model,
            create,
            None,
            cancellation,
        )
    }
}
#[cfg(test)]
pub(crate) mod tests {
    use std::collections::VecDeque;
    use std::mem::size_of;
    use std::ptr;
    use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
    use type_bridge_contract::codec::to_canonical_json;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::projection::{
        BindingTarget, CSymbolPrefix, ProjectedTokenIdentity, ProjectedTokenKind, ProjectionConfig,
        TYPE_BRIDGE_PROJECTED_TOKEN_VERSION,
    };
    use type_bridge_contract::schema::{DocumentId, OwnsFactId, encode_declared_schema};
    use type_bridge_contract::value::{CanonicalString, CanonicalValue};
    use type_bridge_orm::session::backend::{
        BoxFuture, DriverBackend, QueryResult, TransactionOps,
    };
    use type_bridge_orm::{
        ClassifiedCommitError, CommitFailureCertainty, Database, OrmError, ProjectedAttributeValue,
        ProjectedCreate, ProjectedReference, TxType,
    };
    use type_bridge_schema::{
        BUILTIN_SCHEMA_CAPABILITY_IDS, ManagedDeltaContext, SchemaDocumentSet,
        build_schema_authority, encode_schema_authority, normalize_documents, project, resolve,
    };
    use type_bridge_schema_codegen::CEmitter;

    use super::*;
    use crate::abi::{
        ABI_MAJOR, ABI_MINOR, TypeBridgeSchemaPackage, TypeBridgeSchemaPackageDescriptorV1,
    };
    use crate::allocation::{AllocationSite, inject_failure};
    use crate::execution_diagnostic::{
        TypeBridgeExecutionDiagnosticCategory, TypeBridgeExecutionDiagnosticViewV1,
        type_bridge_execution_diagnostics_close, type_bridge_execution_diagnostics_get_v1,
    };
    use crate::projected_model::{
        TypeBridgeProjectedReference, type_bridge_projected_reference_clone,
        type_bridge_projected_reference_close, type_bridge_projected_thing_close,
    };
    use crate::relation_crud::*;
    use crate::runtime::{
        type_bridge_cancellation_close, type_bridge_cancellation_is_requested,
        type_bridge_cancellation_open, type_bridge_cancellation_request,
        type_bridge_read_transaction_close, type_bridge_read_transaction_open,
        type_bridge_write_transaction_commit, type_bridge_write_transaction_open,
        type_bridge_write_transaction_rollback,
    };

    const SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  department: { value: string }
entities:
  person:
    owns:
      identifier: { key: true }
      department: { card: { min: 0, max: 1 } }
  organization:
    owns:
      identifier: { key: true }
relations:
  event: {}
  membership:
    relates:
      member: { card: 1 }
plays:
  person:
    membership: [member]
  event:
    membership: [member]
functions:
  identity-string:
    parameters:
      - { name: input, type: string }
    returns: { scalar: string }
    body: { typeql: "match let $output = $input; return first $output;" }
  person-identifier:
    parameters:
      - { name: person, type: person }
    returns: { scalar: string }
    body: { typeql: "match $person has identifier $identifier; return first $identifier;" }
"#;

    const COMMIT_SUCCESS: u8 = 0;
    const COMMIT_ABORTED: u8 = 1;
    const COMMIT_UNKNOWN: u8 = 2;
    const COMMIT_PANIC: u8 = 3;

    enum Response {
        Result(QueryResult),
        Error,
        Panic,
    }

    struct FakeState {
        responses: Mutex<VecDeque<Response>>,
        opens: Mutex<Vec<TxType>>,
        queries: Mutex<Vec<String>>,
        commits: AtomicUsize,
        rollbacks: AtomicUsize,
        closes: AtomicUsize,
        commit: AtomicU8,
    }

    impl FakeState {
        fn new(responses: Vec<Response>, commit: u8) -> Arc<Self> {
            Arc::new(Self {
                responses: Mutex::new(responses.into()),
                opens: Mutex::new(Vec::new()),
                queries: Mutex::new(Vec::new()),
                commits: AtomicUsize::new(0),
                rollbacks: AtomicUsize::new(0),
                closes: AtomicUsize::new(0),
                commit: AtomicU8::new(commit),
            })
        }

        fn query_count(&self) -> usize {
            self.queries.lock().unwrap().len()
        }
    }

    struct FakeBackend {
        state: Arc<FakeState>,
    }

    impl DriverBackend for FakeBackend {
        fn open_transaction(
            &self,
            _database: &str,
            tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            self.state.opens.lock().unwrap().push(tx_type);
            let state = Arc::clone(&self.state);
            Box::pin(
                async move { Ok(Box::new(FakeTransaction { state }) as Box<dyn TransactionOps>) },
            )
        }

        fn is_open(&self) -> bool {
            true
        }
    }

    struct FakeTransaction {
        state: Arc<FakeState>,
    }

    impl TransactionOps for FakeTransaction {
        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            panic!("entity CRUD must use canonical query execution")
        }

        fn query_canonical(
            &mut self,
            typeql: &str,
        ) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            self.state.queries.lock().unwrap().push(typeql.to_owned());
            let response = self
                .state
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("the C entity CRUD test issued an unexpected provider query");
            match response {
                Response::Result(value) => Box::pin(async move { Ok(value) }),
                Response::Error => Box::pin(async {
                    Err(OrmError::QueryExecution(
                        "provider-secret-query-shape".to_owned(),
                    ))
                }),
                Response::Panic => panic!("fake provider query panic"),
            }
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { panic!("entity CRUD must use classified commit") })
        }

        fn commit_classified(&mut self) -> BoxFuture<'_, Result<(), ClassifiedCommitError>> {
            self.state.commits.fetch_add(1, Ordering::AcqRel);
            let behavior = self.state.commit.load(Ordering::Acquire);
            Box::pin(async move {
                match behavior {
                    COMMIT_SUCCESS => Ok(()),
                    COMMIT_ABORTED => Err(ClassifiedCommitError::Driver {
                        certainty: CommitFailureCertainty::DefinitelyAborted,
                        message: "provider-secret-aborted".to_owned(),
                    }),
                    COMMIT_UNKNOWN => Err(ClassifiedCommitError::Driver {
                        certainty: CommitFailureCertainty::Unknown,
                        message: "provider-secret-unknown".to_owned(),
                    }),
                    COMMIT_PANIC => panic!("fake provider commit panic"),
                    value => panic!("unknown fake commit behavior {value}"),
                }
            })
        }

        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.state.rollbacks.fetch_add(1, Ordering::AcqRel);
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.state.closes.fetch_add(1, Ordering::AcqRel);
            Box::pin(async { Ok(()) })
        }
    }

    struct PackageEvidence {
        authority: Vec<u8>,
        declared: Vec<u8>,
        projection: Vec<u8>,
        semantic: Vec<u8>,
        binding: Vec<u8>,
        scope: Vec<u8>,
        profile: Vec<u8>,
    }

    impl PackageEvidence {
        fn descriptor(&self) -> TypeBridgeSchemaPackageDescriptorV1 {
            TypeBridgeSchemaPackageDescriptorV1 {
                struct_size: size_of::<TypeBridgeSchemaPackageDescriptorV1>() as u32,
                abi_major: ABI_MAJOR,
                abi_minor: ABI_MINOR,
                schema_authority_json: byte_view(&self.authority),
                declared_schema_json: byte_view(&self.declared),
                runtime_projection_json: byte_view(&self.projection),
                semantic_fingerprint_json: byte_view(&self.semantic),
                binding_fingerprint_json: byte_view(&self.binding),
                managed_scope: byte_view(&self.scope),
                semantic_profile: byte_view(&self.profile),
                reserved: [0; 4],
            }
        }
    }

    fn byte_view(value: &[u8]) -> TypeBridgeByteView {
        TypeBridgeByteView {
            data: value.as_ptr(),
            length: value.len(),
        }
    }

    pub(crate) fn package(prefix: &str) -> TypeBridgeSchemaPackage {
        let documents =
            SchemaDocumentSet::parse([(DocumentId::new("entity-crud.yaml").unwrap(), SCHEMA)])
                .unwrap();
        let declared = normalize_documents(&documents).unwrap();
        let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
        let resolved = resolve(&declared, &profile).unwrap();
        let available: CapabilitySet = BUILTIN_SCHEMA_CAPABILITY_IDS
            .iter()
            .map(|value| CapabilityId::new(*value).unwrap())
            .collect();
        let scope = ManagedScopeId::new(format!("c-entity-crud-{prefix}")).unwrap();
        let context = ManagedDeltaContext::new(scope.clone(), profile.clone(), available);
        let authority =
            build_schema_authority(&declared, declared.required_capabilities(), &context).unwrap();
        let emitter = CEmitter::new();
        let projection = project(
            &resolved,
            BindingTarget::C,
            &ProjectionConfig::c(CSymbolPrefix::new(prefix).unwrap()),
            &emitter.generator_handlers(),
            &emitter.code_resources().unwrap(),
        )
        .unwrap();
        let evidence = PackageEvidence {
            authority: encode_schema_authority(&authority),
            declared: encode_declared_schema(&declared).unwrap(),
            projection: to_canonical_json(&projection).unwrap(),
            semantic: to_canonical_json(projection.semantic_fingerprint()).unwrap(),
            binding: to_canonical_json(projection.projection_fingerprint()).unwrap(),
            scope: scope.as_str().as_bytes().to_vec(),
            profile: profile.as_str().as_bytes().to_vec(),
        };
        crate::schema_package::open(evidence.descriptor()).unwrap()
    }

    pub(crate) fn model_token(
        package: &TypeBridgeSchemaPackage,
        type_id: TypeId,
    ) -> TypeBridgeProjectedTokenV1 {
        let identity = ProjectedTokenIdentity::Model(type_id);
        let ordinal = package
            .state
            ._projection
            .projected_token_ordinal(&identity)
            .unwrap();
        TypeBridgeProjectedTokenV1 {
            struct_size: size_of::<TypeBridgeProjectedTokenV1>() as u32,
            version: TYPE_BRIDGE_PROJECTED_TOKEN_VERSION,
            kind: ProjectedTokenKind::Model.as_u32(),
            ordinal,
            projection_digest: package
                .state
                ._projection
                .projection_fingerprint()
                .as_fingerprint()
                .digest()
                .bytes(),
            reserved: [0; 4],
        }
    }

    fn person_create(package: &TypeBridgeSchemaPackage) -> Box<TypeBridgeProjectedCreate> {
        entity_create(package, "person")
    }

    fn entity_create(
        package: &TypeBridgeSchemaPackage,
        entity: &str,
    ) -> Box<TypeBridgeProjectedCreate> {
        let person = TypeId::new(TypeKind::Entity, entity).unwrap();
        let attribute = TypeId::new(TypeKind::Attribute, "identifier").unwrap();
        let field =
            OwnsFactId::new(person.clone(), AttributeId::new("identifier").unwrap()).unwrap();
        let value = ProjectedAttributeValue::try_new(
            &package.state.installed_projection,
            attribute,
            CanonicalValue::String(CanonicalString::new("ada").unwrap()),
        )
        .unwrap();
        let value = ProjectedCreate::try_new(
            &package.state.installed_projection,
            person,
            vec![(field, vec![value])],
            vec![],
        )
        .unwrap();
        Box::new(TypeBridgeProjectedCreate {
            package: Arc::clone(&package.state),
            value,
        })
    }

    fn person_document(iid: &str) -> serde_json::Value {
        serde_json::json!({
            "_iid": iid,
            "_type": "person",
            "attributes": {"identifier": [{"value": "ada"}]}
        })
    }

    fn insert_iid(iid: &str) -> Response {
        Response::Result(QueryResult::Documents(vec![
            serde_json::json!({"iid": iid}),
        ]))
    }

    fn documents(values: Vec<serde_json::Value>) -> Response {
        Response::Result(QueryResult::Documents(values))
    }

    struct Fixture {
        database: Box<TypeBridgeDatabase>,
        package: TypeBridgeSchemaPackage,
        token: TypeBridgeProjectedTokenV1,
        create: Box<TypeBridgeProjectedCreate>,
        state: Arc<FakeState>,
    }

    fn fixture(responses: Vec<Response>, commit: u8) -> Fixture {
        let package = package("entitycrud");
        let token = model_token(&package, TypeId::new(TypeKind::Entity, "person").unwrap());
        let create = person_create(&package);
        let state = FakeState::new(responses, commit);
        let database = Database::with_backend(
            Box::new(FakeBackend {
                state: Arc::clone(&state),
            }),
            "workforce",
        );
        let database = Box::new(TypeBridgeDatabase::from_test_database(
            Arc::clone(&package.state),
            database,
        ));
        Fixture {
            database,
            package,
            token,
            create,
            state,
        }
    }

    fn relation_create(
        package: &TypeBridgeSchemaPackage,
        player_kind: TypeKind,
        player_label: &str,
        player_iid: &str,
    ) -> Box<TypeBridgeProjectedCreate> {
        let membership = TypeId::new(TypeKind::Relation, "membership").unwrap();
        let player = TypeId::new(player_kind, player_label).unwrap();
        let reference = ProjectedReference::try_new(
            &package.state.installed_projection,
            player,
            Some(player_iid.to_owned()),
            vec![],
        )
        .unwrap();
        let value = ProjectedCreate::try_new(
            &package.state.installed_projection,
            membership,
            vec![],
            vec![(
                RoleId::new("membership", "member").unwrap(),
                vec![reference],
            )],
        )
        .unwrap();
        Box::new(TypeBridgeProjectedCreate {
            package: Arc::clone(&package.state),
            value,
        })
    }

    fn membership_document(iid: &str) -> serde_json::Value {
        serde_json::json!({
            "_iid": iid,
            "_type": "membership",
            "_role_0_iid": "0x10",
            "_role_0_type": "person",
            "_role_0_attributes": {
                "identifier": [{"value": "ada"}]
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

    struct RelationFixture {
        database: Box<TypeBridgeDatabase>,
        package: TypeBridgeSchemaPackage,
        token: TypeBridgeProjectedTokenV1,
        create: Box<TypeBridgeProjectedCreate>,
        state: Arc<FakeState>,
    }

    fn relation_fixture(responses: Vec<Response>, commit: u8) -> RelationFixture {
        let package = package("relationcrud");
        let token = model_token(
            &package,
            TypeId::new(TypeKind::Relation, "membership").unwrap(),
        );
        let create = relation_create(&package, TypeKind::Entity, "person", "0x10");
        let state = FakeState::new(responses, commit);
        let database = Database::with_backend(
            Box::new(FakeBackend {
                state: Arc::clone(&state),
            }),
            "workforce",
        );
        let database = Box::new(TypeBridgeDatabase::from_test_database(
            Arc::clone(&package.state),
            database,
        ));
        RelationFixture {
            database,
            package,
            token,
            create,
            state,
        }
    }

    fn copied(view: TypeBridgeByteView) -> Vec<u8> {
        if view.length == 0 {
            return Vec::new();
        }
        // SAFETY: diagnostic views remain borrowed from a live handle in every caller.
        unsafe { std::slice::from_raw_parts(view.data, view.length) }.to_vec()
    }

    fn diagnostic(
        diagnostics: *mut TypeBridgeExecutionDiagnostics,
    ) -> (TypeBridgeExecutionDiagnosticCategory, String, String) {
        assert!(!diagnostics.is_null());
        let mut view = TypeBridgeExecutionDiagnosticViewV1 {
            struct_size: 0,
            version: 0,
            category: TypeBridgeExecutionDiagnosticCategory::Internal,
            reserved0: 0,
            code: TypeBridgeByteView {
                data: ptr::null(),
                length: 0,
            },
            message: TypeBridgeByteView {
                data: ptr::null(),
                length: 0,
            },
            path_count: 0,
            detail_count: 0,
            reserved: [0; 4],
        };
        assert_eq!(
            // SAFETY: both pointers are valid for the duration of the borrow.
            unsafe { type_bridge_execution_diagnostics_get_v1(diagnostics, 0, &mut view) },
            TypeBridgeStatus::Ok
        );
        (
            view.category,
            String::from_utf8(copied(view.code)).unwrap(),
            String::from_utf8(copied(view.message)).unwrap(),
        )
    }

    fn close_diagnostics(diagnostics: &mut *mut TypeBridgeExecutionDiagnostics) {
        assert_eq!(
            // SAFETY: the slot owns either null or one diagnostics handle.
            unsafe { type_bridge_execution_diagnostics_close(diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert!(diagnostics.is_null());
    }

    fn close_thing(thing: &mut *mut TypeBridgeProjectedThing) {
        assert_eq!(
            // SAFETY: the slot owns either null or one thing handle.
            unsafe { type_bridge_projected_thing_close(thing) },
            TypeBridgeStatus::Ok
        );
        assert!(thing.is_null());
    }

    #[test]
    fn database_owned_insert_rehydrates_then_commits_once() {
        let fixture = fixture(
            vec![insert_iid("0x10"), documents(vec![person_document("0x10")])],
            COMMIT_SUCCESS,
        );
        let mut thing = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            // SAFETY: the fixture retains all inputs and owns both output slots.
            unsafe {
                type_bridge_database_entity_insert(
                    &*fixture.database,
                    &fixture.token,
                    &*fixture.create,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert!(!thing.is_null());
        assert!(diagnostics.is_null());
        // SAFETY: success returned one live projected thing.
        assert_eq!(unsafe { &*thing }.value.iid(), "0x10");
        assert_eq!(*fixture.state.opens.lock().unwrap(), [TxType::Write]);
        assert_eq!(fixture.state.query_count(), 2);
        assert_eq!(fixture.state.commits.load(Ordering::Acquire), 1);
        assert_eq!(fixture.state.rollbacks.load(Ordering::Acquire), 0);
        close_thing(&mut thing);
    }

    #[test]
    fn projected_thing_reservation_failure_is_pre_provider_and_retry_safe() {
        let fixture = fixture(
            vec![insert_iid("0x10"), documents(vec![person_document("0x10")])],
            COMMIT_SUCCESS,
        );
        let mut thing = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        let failure = inject_failure(AllocationSite::ProjectedThingHandle, 0);
        assert_eq!(
            // SAFETY: the fixture retains all inputs and owns both output slots.
            unsafe {
                type_bridge_database_entity_insert(
                    &*fixture.database,
                    &fixture.token,
                    &*fixture.create,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit
        );
        drop(failure);
        assert!(thing.is_null());
        assert_eq!(diagnostic(diagnostics).1, "c_allocation_exhausted");
        assert!(fixture.state.opens.lock().unwrap().is_empty());
        assert_eq!(fixture.state.query_count(), 0);
        assert_eq!(fixture.state.commits.load(Ordering::Acquire), 0);
        assert_eq!(fixture.state.rollbacks.load(Ordering::Acquire), 0);
        close_diagnostics(&mut diagnostics);

        assert_eq!(
            // SAFETY: no provider operation was attempted by the failed reservation.
            unsafe {
                type_bridge_database_entity_insert(
                    &*fixture.database,
                    &fixture.token,
                    &*fixture.create,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(fixture.state.query_count(), 2);
        assert_eq!(fixture.state.commits.load(Ordering::Acquire), 1);
        assert_eq!(fixture.state.rollbacks.load(Ordering::Acquire), 0);
        close_thing(&mut thing);
    }

    #[test]
    fn borrowed_thing_reservation_failure_leaves_transaction_live_and_unpoisoned() {
        let fixture = fixture(
            vec![insert_iid("0x10"), documents(vec![person_document("0x10")])],
            COMMIT_SUCCESS,
        );
        let mut transaction = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            // SAFETY: outputs are distinct writable owner slots.
            unsafe {
                type_bridge_write_transaction_open(
                    &*fixture.database,
                    ptr::null(),
                    &mut transaction,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        let mut thing = ptr::dangling_mut();
        let failure = inject_failure(AllocationSite::ProjectedThingHandle, 0);
        assert_eq!(
            // SAFETY: transaction and create remain live for this borrowed call.
            unsafe {
                type_bridge_write_transaction_entity_insert(
                    transaction,
                    &fixture.token,
                    &*fixture.create,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ResourceLimit
        );
        drop(failure);
        assert!(thing.is_null());
        assert_eq!(diagnostic(diagnostics).1, "c_allocation_exhausted");
        assert_eq!(fixture.state.query_count(), 0);
        assert_eq!(fixture.state.commits.load(Ordering::Acquire), 0);
        assert_eq!(fixture.state.rollbacks.load(Ordering::Acquire), 0);
        close_diagnostics(&mut diagnostics);

        assert_eq!(
            // SAFETY: the failed reservation did not consume or poison the transaction.
            unsafe {
                type_bridge_write_transaction_entity_insert(
                    transaction,
                    &fixture.token,
                    &*fixture.create,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        close_thing(&mut thing);
        assert_eq!(
            // SAFETY: rollback proves the borrowed transaction owner remains live.
            unsafe { type_bridge_write_transaction_rollback(&mut transaction, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert!(transaction.is_null());
        assert_eq!(fixture.state.query_count(), 2);
        assert_eq!(fixture.state.commits.load(Ordering::Acquire), 0);
        assert_eq!(fixture.state.rollbacks.load(Ordering::Acquire), 1);
    }

    #[test]
    fn database_owned_write_rolls_back_errors_and_classifies_commit_certainty() {
        for (responses, expected_code) in [
            (
                vec![insert_iid("0x10"), documents(vec![])],
                "mutation_rehydration_missing",
            ),
            (vec![Response::Error], "provider_operation_failed"),
        ] {
            let fixture = fixture(responses, COMMIT_SUCCESS);
            let mut thing = ptr::dangling_mut();
            let mut diagnostics = ptr::dangling_mut();
            assert_eq!(
                // SAFETY: the fixture retains all inputs and owns both outputs.
                unsafe {
                    type_bridge_database_entity_insert(
                        &*fixture.database,
                        &fixture.token,
                        &*fixture.create,
                        ptr::null(),
                        &mut thing,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::ExecutionFailed
            );
            assert!(thing.is_null());
            let (_, code, message) = diagnostic(diagnostics);
            assert_eq!(code, expected_code);
            assert!(!message.contains("provider-secret"));
            assert_eq!(fixture.state.commits.load(Ordering::Acquire), 0);
            assert_eq!(fixture.state.rollbacks.load(Ordering::Acquire), 1);
            close_diagnostics(&mut diagnostics);
        }

        for (behavior, expected_status, expected_code) in [
            (
                COMMIT_ABORTED,
                TypeBridgeStatus::ExecutionFailed,
                "commit_definitely_aborted",
            ),
            (
                COMMIT_UNKNOWN,
                TypeBridgeStatus::CommitOutcomeUnknown,
                "commit_outcome_unknown",
            ),
            (
                COMMIT_PANIC,
                TypeBridgeStatus::CommitOutcomeUnknown,
                "commit_outcome_unknown",
            ),
        ] {
            let fixture = fixture(
                vec![insert_iid("0x10"), documents(vec![person_document("0x10")])],
                behavior,
            );
            let mut thing = ptr::dangling_mut();
            let mut diagnostics = ptr::dangling_mut();
            assert_eq!(
                // SAFETY: the fixture retains all inputs and owns both outputs.
                unsafe {
                    type_bridge_database_entity_insert(
                        &*fixture.database,
                        &fixture.token,
                        &*fixture.create,
                        ptr::null(),
                        &mut thing,
                        &mut diagnostics,
                    )
                },
                expected_status
            );
            assert!(thing.is_null());
            let (category, code, message) = diagnostic(diagnostics);
            assert_eq!(category, TypeBridgeExecutionDiagnosticCategory::Transaction);
            assert_eq!(code, expected_code);
            assert!(!message.contains("provider-secret"));
            assert_eq!(fixture.state.commits.load(Ordering::Acquire), 1);
            assert_eq!(fixture.state.rollbacks.load(Ordering::Acquire), 0);
            close_diagnostics(&mut diagnostics);
        }
    }

    #[test]
    fn database_get_count_and_blind_delete_have_exact_owned_lifecycles() {
        let fixture = fixture(
            vec![
                documents(vec![]),
                Response::Result(QueryResult::Rows(vec![serde_json::json!({"$count": 7})])),
                Response::Result(QueryResult::Ok),
            ],
            COMMIT_SUCCESS,
        );
        let iid = byte_view(b"0x10");
        let mut thing = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            // SAFETY: all borrowed inputs and writable outputs remain live.
            unsafe {
                type_bridge_database_entity_get_by_iid(
                    &*fixture.database,
                    &fixture.token,
                    iid,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert!(thing.is_null());
        assert!(diagnostics.is_null());

        let mut count = u64::MAX;
        assert_eq!(
            // SAFETY: all borrowed inputs and writable outputs remain live.
            unsafe {
                type_bridge_database_entity_count(
                    &*fixture.database,
                    &fixture.token,
                    ptr::null(),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(count, 7);
        assert!(diagnostics.is_null());

        assert_eq!(
            // SAFETY: all borrowed inputs and the writable output remain live.
            unsafe {
                type_bridge_database_entity_delete_by_iid(
                    &*fixture.database,
                    &fixture.token,
                    iid,
                    ptr::null(),
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert!(diagnostics.is_null());
        assert_eq!(
            *fixture.state.opens.lock().unwrap(),
            [TxType::Read, TxType::Read, TxType::Write]
        );
        assert_eq!(fixture.state.query_count(), 3);
        assert_eq!(fixture.state.closes.load(Ordering::Acquire), 2);
        assert_eq!(fixture.state.commits.load(Ordering::Acquire), 1);
        assert_eq!(fixture.state.rollbacks.load(Ordering::Acquire), 0);
        let queries = fixture.state.queries.lock().unwrap();
        assert!(queries[2].contains("delete"));
        assert!(queries[2].contains("isa! person"));
    }

    #[test]
    fn borrowed_transactions_never_terminal_the_callers_handle() {
        let fixture = fixture(
            vec![
                insert_iid("0x10"),
                documents(vec![person_document("0x10")]),
                Response::Result(QueryResult::Rows(vec![serde_json::json!({"$count": 1})])),
            ],
            COMMIT_SUCCESS,
        );
        let mut diagnostics = ptr::null_mut();
        let mut transaction = ptr::null_mut();
        assert_eq!(
            // SAFETY: outputs are distinct writable owner slots.
            unsafe {
                type_bridge_write_transaction_open(
                    &*fixture.database,
                    ptr::null(),
                    &mut transaction,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert!(!transaction.is_null());

        let mut thing = ptr::dangling_mut();
        assert_eq!(
            // SAFETY: transaction and create remain live for this borrowed call.
            unsafe {
                type_bridge_write_transaction_entity_insert(
                    transaction,
                    &fixture.token,
                    &*fixture.create,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert!(!thing.is_null());
        assert_eq!(fixture.state.commits.load(Ordering::Acquire), 0);
        assert_eq!(fixture.state.rollbacks.load(Ordering::Acquire), 0);
        close_thing(&mut thing);

        let mut count = 0;
        assert_eq!(
            // SAFETY: the borrowed transaction was not consumed by insert.
            unsafe {
                type_bridge_write_transaction_entity_count(
                    transaction,
                    &fixture.token,
                    ptr::null(),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(count, 1);
        assert_eq!(fixture.state.commits.load(Ordering::Acquire), 0);
        assert_eq!(fixture.state.closes.load(Ordering::Acquire), 0);

        assert_eq!(
            // SAFETY: the owner slot contains the live write transaction.
            unsafe {
                type_bridge_write_transaction_commit(
                    &mut transaction,
                    ptr::null(),
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert!(transaction.is_null());
        assert_eq!(fixture.state.commits.load(Ordering::Acquire), 1);
    }

    #[test]
    fn provider_panic_poisons_borrowed_handles_until_cleanup() {
        let fixture = fixture(vec![Response::Panic], COMMIT_SUCCESS);
        let mut diagnostics = ptr::null_mut();
        let mut transaction = ptr::null_mut();
        assert_eq!(
            // SAFETY: outputs are distinct writable owner slots.
            unsafe {
                type_bridge_write_transaction_open(
                    &*fixture.database,
                    ptr::null(),
                    &mut transaction,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        let mut count = u64::MAX;
        assert_eq!(
            // SAFETY: the transaction and output slots are live.
            unsafe {
                type_bridge_write_transaction_entity_count(
                    transaction,
                    &fixture.token,
                    ptr::null(),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Panic
        );
        assert_eq!(count, 0);
        assert!(diagnostics.is_null());
        assert_eq!(fixture.state.query_count(), 1);

        count = u64::MAX;
        assert_eq!(
            // SAFETY: the poisoned handle stays live but admits only cleanup.
            unsafe {
                type_bridge_write_transaction_entity_count(
                    transaction,
                    &fixture.token,
                    ptr::null(),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed
        );
        assert_eq!(count, 0);
        let (category, code, _) = diagnostic(diagnostics);
        assert_eq!(category, TypeBridgeExecutionDiagnosticCategory::Transaction);
        assert_eq!(code, "c_transaction_poisoned");
        assert_eq!(fixture.state.query_count(), 1);
        close_diagnostics(&mut diagnostics);

        assert_eq!(
            // SAFETY: commit rejection must leave this owner slot live.
            unsafe {
                type_bridge_write_transaction_commit(
                    &mut transaction,
                    ptr::null(),
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed
        );
        assert!(!transaction.is_null());
        assert_eq!(diagnostic(diagnostics).1, "c_transaction_poisoned");
        assert_eq!(fixture.state.commits.load(Ordering::Acquire), 0);
        close_diagnostics(&mut diagnostics);
        assert_eq!(
            // SAFETY: rollback is explicitly permitted on a poisoned handle.
            unsafe { type_bridge_write_transaction_rollback(&mut transaction, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert!(transaction.is_null());
        assert_eq!(fixture.state.rollbacks.load(Ordering::Acquire), 1);

        let read_fixture = self::fixture(vec![Response::Panic], COMMIT_SUCCESS);
        let mut read = ptr::null_mut();
        assert_eq!(
            // SAFETY: outputs are distinct writable owner slots.
            unsafe {
                type_bridge_read_transaction_open(
                    &*read_fixture.database,
                    ptr::null(),
                    &mut read,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            // SAFETY: the read transaction and outputs remain live.
            unsafe {
                type_bridge_read_transaction_entity_count(
                    read,
                    &read_fixture.token,
                    ptr::null(),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Panic
        );
        assert_eq!(
            // SAFETY: the second operation observes poison before provider dispatch.
            unsafe {
                type_bridge_read_transaction_entity_count(
                    read,
                    &read_fixture.token,
                    ptr::null(),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed
        );
        assert_eq!(diagnostic(diagnostics).1, "c_transaction_poisoned");
        assert_eq!(read_fixture.state.query_count(), 1);
        close_diagnostics(&mut diagnostics);
        assert_eq!(
            // SAFETY: close is explicitly permitted on a poisoned read handle.
            unsafe { type_bridge_read_transaction_close(&mut read, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert!(read.is_null());
        assert_eq!(read_fixture.state.closes.load(Ordering::Acquire), 1);
    }

    #[test]
    fn cancellation_foreign_forged_kind_and_iid_fail_before_io() {
        let fixture = fixture(vec![], COMMIT_SUCCESS);
        let mut cancellation = ptr::null_mut();
        assert_eq!(
            // SAFETY: output slot is writable.
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            // SAFETY: cancellation is live.
            unsafe { type_bridge_cancellation_request(cancellation) },
            TypeBridgeStatus::Ok
        );
        let mut thing = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            // SAFETY: all borrowed inputs and writable outputs remain live.
            unsafe {
                type_bridge_database_entity_insert(
                    &*fixture.database,
                    &fixture.token,
                    &*fixture.create,
                    cancellation,
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Cancelled
        );
        assert!(thing.is_null());
        assert_eq!(diagnostic(diagnostics).1, "cancelled_before_dispatch");
        close_diagnostics(&mut diagnostics);

        let foreign = package("foreigncrud");
        let foreign_create = person_create(&foreign);
        assert_eq!(
            // SAFETY: all borrowed inputs and writable outputs remain live.
            unsafe {
                type_bridge_database_entity_insert(
                    &*fixture.database,
                    &fixture.token,
                    &*foreign_create,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed
        );
        assert_eq!(
            diagnostic(diagnostics).1,
            "generated_token_package_mismatch"
        );
        close_diagnostics(&mut diagnostics);

        let other_create = entity_create(&fixture.package, "organization");
        assert_eq!(
            // SAFETY: the explicit nominal mismatch is a valid hostile ABI input.
            unsafe {
                type_bridge_database_entity_insert(
                    &*fixture.database,
                    &fixture.token,
                    &*other_create,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(diagnostic(diagnostics).1, "c_entity_create_model_mismatch");
        close_diagnostics(&mut diagnostics);
        assert!(fixture.state.opens.lock().unwrap().is_empty());
        assert_eq!(fixture.state.query_count(), 0);

        let mut forged = fixture.token;
        forged.projection_digest[0] ^= 0xff;
        assert_eq!(
            // SAFETY: all borrowed inputs and writable outputs remain live.
            unsafe {
                type_bridge_database_entity_get_by_iid(
                    &*fixture.database,
                    &forged,
                    byte_view(b"0x10"),
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed
        );
        assert_eq!(
            diagnostic(diagnostics).1,
            "generated_token_package_mismatch"
        );
        close_diagnostics(&mut diagnostics);

        let attribute_token = model_token(
            &fixture.package,
            TypeId::new(TypeKind::Attribute, "identifier").unwrap(),
        );
        let mut count = u64::MAX;
        assert_eq!(
            // SAFETY: all borrowed inputs and writable outputs remain live.
            unsafe {
                type_bridge_database_entity_count(
                    &*fixture.database,
                    &attribute_token,
                    ptr::null(),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(count, 0);
        assert_eq!(diagnostic(diagnostics).1, "c_entity_model_kind_invalid");
        close_diagnostics(&mut diagnostics);

        assert_eq!(
            // SAFETY: all borrowed inputs and writable outputs remain live.
            unsafe {
                type_bridge_database_entity_get_by_iid(
                    &*fixture.database,
                    &fixture.token,
                    byte_view(b"not-an-iid"),
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(diagnostic(diagnostics).1, "c_entity_iid_invalid");
        close_diagnostics(&mut diagnostics);
        assert!(fixture.state.opens.lock().unwrap().is_empty());
        assert_eq!(fixture.state.query_count(), 0);
        assert_eq!(
            // SAFETY: the owner slot contains one live cancellation.
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
    }

    #[repr(align(8))]
    struct AlignedBytes([u8; 24]);

    #[test]
    fn overlap_preflight_never_corrupts_live_inputs_and_clears_overlapping_outputs() {
        let fixture = fixture(
            vec![Response::Result(QueryResult::Rows(vec![
                serde_json::json!({
                    "$count": 4
                }),
            ]))],
            COMMIT_SUCCESS,
        );
        let mut untouched_diagnostics = ptr::dangling_mut();

        let database_output = (&*fixture.database as *const TypeBridgeDatabase)
            .cast_mut()
            .cast::<u64>();
        assert_eq!(
            // SAFETY: this hostile overlap must be rejected without writing either output.
            unsafe {
                type_bridge_database_entity_count(
                    &*fixture.database,
                    &fixture.token,
                    ptr::null(),
                    database_output,
                    &mut untouched_diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(untouched_diagnostics, ptr::dangling_mut());

        let create_output = (&*fixture.create as *const TypeBridgeProjectedCreate)
            .cast_mut()
            .cast::<*mut TypeBridgeProjectedThing>();
        assert_eq!(
            // SAFETY: this hostile overlap must leave the create handle intact.
            unsafe {
                type_bridge_database_entity_insert(
                    &*fixture.database,
                    &fixture.token,
                    &*fixture.create,
                    ptr::null(),
                    create_output,
                    &mut untouched_diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(untouched_diagnostics, ptr::dangling_mut());

        let model_output = (&fixture.token as *const TypeBridgeProjectedTokenV1)
            .cast_mut()
            .cast::<u64>();
        assert_eq!(
            // SAFETY: this hostile overlap must leave the generated token intact.
            unsafe {
                type_bridge_database_entity_count(
                    &*fixture.database,
                    &fixture.token,
                    ptr::null(),
                    model_output,
                    &mut untouched_diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );

        let mut cancellation = ptr::null_mut();
        assert_eq!(
            // SAFETY: output slot is writable.
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
        let cancellation_output = cancellation.cast::<u64>();
        assert_eq!(
            // SAFETY: this hostile overlap must leave the cancellation handle intact.
            unsafe {
                type_bridge_database_entity_count(
                    &*fixture.database,
                    &fixture.token,
                    cancellation,
                    cancellation_output,
                    &mut untouched_diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        let mut requested = u8::MAX;
        assert_eq!(
            // SAFETY: alias rejection preserved the cancellation allocation.
            unsafe { type_bridge_cancellation_is_requested(cancellation, &mut requested) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(requested, 0);

        let mut iid_storage = AlignedBytes([0; 24]);
        iid_storage.0[..4].copy_from_slice(b"0x10");
        let iid = TypeBridgeByteView {
            data: iid_storage.0.as_ptr(),
            length: 4,
        };
        assert_eq!(
            // SAFETY: this hostile overlap must leave the IID bytes intact.
            unsafe {
                type_bridge_database_entity_get_by_iid(
                    &*fixture.database,
                    &fixture.token,
                    iid,
                    ptr::null(),
                    iid_storage.0.as_mut_ptr().cast(),
                    &mut untouched_diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(&iid_storage.0[..4], b"0x10");

        let mut overlapping = AlignedBytes([0xaa; 24]);
        let first = overlapping
            .0
            .as_mut_ptr()
            .cast::<*mut TypeBridgeProjectedThing>();
        // Deliberately unaligned and partially overlapping: no typed Rust reference is made.
        let second = unsafe { overlapping.0.as_mut_ptr().add(1) }
            .cast::<*mut TypeBridgeExecutionDiagnostics>();
        assert_eq!(
            // SAFETY: both raw output byte ranges are allocated and writable.
            unsafe {
                type_bridge_database_entity_get_by_iid(
                    &*fixture.database,
                    &fixture.token,
                    byte_view(b"0x10"),
                    ptr::null(),
                    first,
                    second,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(&overlapping.0[..9], &[0; 9]);
        assert_eq!(fixture.state.query_count(), 0);

        let mut count = u64::MAX;
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            // SAFETY: every hostile call preserved the inputs for this valid dispatch.
            unsafe {
                type_bridge_database_entity_count(
                    &*fixture.database,
                    &fixture.token,
                    ptr::null(),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(count, 4);
        assert!(diagnostics.is_null());
        assert_eq!(fixture.state.query_count(), 1);
        assert_eq!(
            // SAFETY: alias rejection preserved the cancellation allocation.
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
    }

    #[test]
    fn transaction_input_overlap_is_pre_io_and_handle_remains_usable() {
        let fixture = fixture(
            vec![Response::Result(QueryResult::Rows(vec![
                serde_json::json!({
                    "$count": 2
                }),
            ]))],
            COMMIT_SUCCESS,
        );
        let mut diagnostics = ptr::null_mut();
        let mut transaction = ptr::null_mut();
        assert_eq!(
            // SAFETY: outputs are distinct writable owner slots.
            unsafe {
                type_bridge_write_transaction_open(
                    &*fixture.database,
                    ptr::null(),
                    &mut transaction,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        let transaction_output = transaction.cast::<u64>();
        let mut untouched_diagnostics = ptr::dangling_mut();
        assert_eq!(
            // SAFETY: the hostile output overlaps the borrowed transaction input.
            unsafe {
                type_bridge_write_transaction_entity_count(
                    transaction,
                    &fixture.token,
                    ptr::null(),
                    transaction_output,
                    &mut untouched_diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(untouched_diagnostics, ptr::dangling_mut());
        assert_eq!(fixture.state.query_count(), 0);

        let mut count = 0;
        assert_eq!(
            // SAFETY: alias rejection preserved the live transaction handle.
            unsafe {
                type_bridge_write_transaction_entity_count(
                    transaction,
                    &fixture.token,
                    ptr::null(),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(count, 2);
        assert_eq!(fixture.state.query_count(), 1);
        assert_eq!(
            // SAFETY: the owner slot still contains the live write transaction.
            unsafe { type_bridge_write_transaction_rollback(&mut transaction, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
    }

    #[test]
    fn remaining_put_update_get_and_delete_exports_share_exact_lifecycles() {
        let put_fixture = fixture(
            vec![
                documents(vec![]),
                insert_iid("0x20"),
                documents(vec![person_document("0x20")]),
            ],
            COMMIT_SUCCESS,
        );
        let mut thing = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            // SAFETY: all retained inputs and output slots remain live.
            unsafe {
                type_bridge_database_entity_put(
                    &*put_fixture.database,
                    &put_fixture.token,
                    &*put_fixture.create,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(put_fixture.state.query_count(), 3);
        assert_eq!(put_fixture.state.commits.load(Ordering::Acquire), 1);
        close_thing(&mut thing);

        let update_fixture = fixture(
            vec![
                Response::Result(QueryResult::Ok),
                documents(vec![person_document("0x20")]),
            ],
            COMMIT_SUCCESS,
        );
        assert_eq!(
            // SAFETY: all retained inputs and output slots remain live.
            unsafe {
                type_bridge_database_entity_update(
                    &*update_fixture.database,
                    &update_fixture.token,
                    byte_view(b"0x20"),
                    &*update_fixture.create,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(update_fixture.state.query_count(), 2);
        assert_eq!(update_fixture.state.commits.load(Ordering::Acquire), 1);
        close_thing(&mut thing);

        let read_fixture = fixture(
            vec![documents(vec![person_document("0x20")])],
            COMMIT_SUCCESS,
        );
        let mut read = ptr::null_mut();
        assert_eq!(
            // SAFETY: transaction outputs are distinct writable slots.
            unsafe {
                type_bridge_read_transaction_open(
                    &*read_fixture.database,
                    ptr::null(),
                    &mut read,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            // SAFETY: the borrowed read handle and all other inputs remain live.
            unsafe {
                type_bridge_read_transaction_entity_get_by_iid(
                    read,
                    &read_fixture.token,
                    byte_view(b"0x20"),
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(read_fixture.state.closes.load(Ordering::Acquire), 0);
        close_thing(&mut thing);
        assert_eq!(
            // SAFETY: the owner slot contains the still-live read transaction.
            unsafe { type_bridge_read_transaction_close(&mut read, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(read_fixture.state.closes.load(Ordering::Acquire), 1);

        let write_fixture = fixture(
            vec![
                documents(vec![]),
                insert_iid("0x30"),
                documents(vec![person_document("0x30")]),
                documents(vec![person_document("0x30")]),
                Response::Result(QueryResult::Ok),
                documents(vec![person_document("0x30")]),
                Response::Result(QueryResult::Ok),
            ],
            COMMIT_SUCCESS,
        );
        let mut write = ptr::null_mut();
        assert_eq!(
            // SAFETY: transaction outputs are distinct writable slots.
            unsafe {
                type_bridge_write_transaction_open(
                    &*write_fixture.database,
                    ptr::null(),
                    &mut write,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            // SAFETY: all retained inputs and output slots remain live.
            unsafe {
                type_bridge_write_transaction_entity_put(
                    write,
                    &write_fixture.token,
                    &*write_fixture.create,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        close_thing(&mut thing);
        assert_eq!(
            // SAFETY: the borrowed write handle remains live after put.
            unsafe {
                type_bridge_write_transaction_entity_get_by_iid(
                    write,
                    &write_fixture.token,
                    byte_view(b"0x30"),
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        close_thing(&mut thing);
        assert_eq!(
            // SAFETY: the borrowed write handle remains live after get.
            unsafe {
                type_bridge_write_transaction_entity_update(
                    write,
                    &write_fixture.token,
                    byte_view(b"0x30"),
                    &*write_fixture.create,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        close_thing(&mut thing);
        assert_eq!(
            // SAFETY: blind delete borrows but does not terminal the write handle.
            unsafe {
                type_bridge_write_transaction_entity_delete_by_iid(
                    write,
                    &write_fixture.token,
                    byte_view(b"0x30"),
                    ptr::null(),
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(write_fixture.state.query_count(), 7);
        assert_eq!(write_fixture.state.commits.load(Ordering::Acquire), 0);
        assert_eq!(write_fixture.state.rollbacks.load(Ordering::Acquire), 0);
        assert_eq!(
            // SAFETY: all borrowed operations left the owner slot live.
            unsafe { type_bridge_write_transaction_rollback(&mut write, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(write_fixture.state.rollbacks.load(Ordering::Acquire), 1);
    }

    #[test]
    fn relation_database_owned_mutations_rehydrate_before_one_commit() {
        let insert = relation_fixture(
            vec![
                insert_iid("0x20"),
                documents(vec![membership_document("0x20")]),
            ],
            COMMIT_SUCCESS,
        );
        let mut thing = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            // SAFETY: the fixture retains all immutable inputs and both output slots.
            unsafe {
                type_bridge_database_relation_insert(
                    &*insert.database,
                    &insert.token,
                    &*insert.create,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(unsafe { &*thing }.value.iid(), "0x20");
        assert_eq!(insert.state.query_count(), 2);
        assert_eq!(insert.state.commits.load(Ordering::Acquire), 1);
        assert_eq!(insert.state.rollbacks.load(Ordering::Acquire), 0);
        close_thing(&mut thing);

        let put = relation_fixture(
            vec![
                documents(vec![serde_json::json!({"iid": "0x10"})]),
                insert_iid("0x21"),
                documents(vec![membership_document("0x21")]),
            ],
            COMMIT_SUCCESS,
        );
        assert_eq!(
            // SAFETY: the fixture retains all immutable inputs and both output slots.
            unsafe {
                type_bridge_database_relation_put(
                    &*put.database,
                    &put.token,
                    &*put.create,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(unsafe { &*thing }.value.iid(), "0x21");
        assert_eq!(put.state.query_count(), 3);
        assert_eq!(put.state.commits.load(Ordering::Acquire), 1);
        close_thing(&mut thing);

        let update = relation_fixture(
            vec![
                documents(vec![serde_json::json!({"iid": "0x10"})]),
                Response::Result(QueryResult::Ok),
                Response::Result(QueryResult::Ok),
                documents(vec![membership_document("0x22")]),
            ],
            COMMIT_SUCCESS,
        );
        assert_eq!(
            // SAFETY: the fixture retains all immutable inputs and both output slots.
            unsafe {
                type_bridge_database_relation_update(
                    &*update.database,
                    &update.token,
                    byte_view(b"0x22"),
                    &*update.create,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(unsafe { &*thing }.value.iid(), "0x22");
        assert_eq!(update.state.query_count(), 4);
        assert_eq!(update.state.commits.load(Ordering::Acquire), 1);
        assert_eq!(update.state.rollbacks.load(Ordering::Acquire), 0);
        close_thing(&mut thing);
    }

    #[test]
    fn every_relation_read_and_borrowed_export_preserves_exact_ownership() {
        let owned = relation_fixture(
            vec![
                documents(vec![]),
                Response::Result(QueryResult::Rows(vec![serde_json::json!({"$count": 3})])),
                Response::Result(QueryResult::Ok),
            ],
            COMMIT_SUCCESS,
        );
        let mut thing = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            unsafe {
                type_bridge_database_relation_get_by_iid(
                    &*owned.database,
                    &owned.token,
                    byte_view(b"0x20"),
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert!(thing.is_null());
        let mut count = u64::MAX;
        assert_eq!(
            unsafe {
                type_bridge_database_relation_count(
                    &*owned.database,
                    &owned.token,
                    ptr::null(),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(count, 3);
        assert_eq!(
            unsafe {
                type_bridge_database_relation_delete_by_iid(
                    &*owned.database,
                    &owned.token,
                    byte_view(b"0x20"),
                    ptr::null(),
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(owned.state.query_count(), 3);
        assert_eq!(owned.state.closes.load(Ordering::Acquire), 2);
        assert_eq!(owned.state.commits.load(Ordering::Acquire), 1);

        let read_fixture = relation_fixture(
            vec![
                documents(vec![membership_document("0x20")]),
                Response::Result(QueryResult::Rows(vec![serde_json::json!({"$count": 1})])),
            ],
            COMMIT_SUCCESS,
        );
        let mut read = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_open(
                    &*read_fixture.database,
                    ptr::null(),
                    &mut read,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_relation_get_by_iid(
                    read,
                    &read_fixture.token,
                    byte_view(b"0x20"),
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        close_thing(&mut thing);
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_relation_count(
                    read,
                    &read_fixture.token,
                    ptr::null(),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(count, 1);
        assert_eq!(read_fixture.state.closes.load(Ordering::Acquire), 0);
        assert_eq!(
            unsafe { type_bridge_read_transaction_close(&mut read, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );

        let write_fixture = relation_fixture(
            vec![
                insert_iid("0x30"),
                documents(vec![membership_document("0x30")]),
                documents(vec![serde_json::json!({"iid": "0x10"})]),
                insert_iid("0x31"),
                documents(vec![membership_document("0x31")]),
                documents(vec![membership_document("0x31")]),
                documents(vec![serde_json::json!({"iid": "0x10"})]),
                Response::Result(QueryResult::Ok),
                Response::Result(QueryResult::Ok),
                documents(vec![membership_document("0x31")]),
                Response::Result(QueryResult::Ok),
                Response::Result(QueryResult::Rows(vec![serde_json::json!({"$count": 1})])),
            ],
            COMMIT_SUCCESS,
        );
        let mut write = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_open(
                    &*write_fixture.database,
                    ptr::null(),
                    &mut write,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        for status in [
            unsafe {
                type_bridge_write_transaction_relation_insert(
                    write,
                    &write_fixture.token,
                    &*write_fixture.create,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            {
                close_thing(&mut thing);
                unsafe {
                    type_bridge_write_transaction_relation_put(
                        write,
                        &write_fixture.token,
                        &*write_fixture.create,
                        ptr::null(),
                        &mut thing,
                        &mut diagnostics,
                    )
                }
            },
            {
                close_thing(&mut thing);
                unsafe {
                    type_bridge_write_transaction_relation_get_by_iid(
                        write,
                        &write_fixture.token,
                        byte_view(b"0x31"),
                        ptr::null(),
                        &mut thing,
                        &mut diagnostics,
                    )
                }
            },
            {
                close_thing(&mut thing);
                unsafe {
                    type_bridge_write_transaction_relation_update(
                        write,
                        &write_fixture.token,
                        byte_view(b"0x31"),
                        &*write_fixture.create,
                        ptr::null(),
                        &mut thing,
                        &mut diagnostics,
                    )
                }
            },
        ] {
            assert_eq!(status, TypeBridgeStatus::Ok);
        }
        close_thing(&mut thing);
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_relation_delete_by_iid(
                    write,
                    &write_fixture.token,
                    byte_view(b"0x31"),
                    ptr::null(),
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_relation_count(
                    write,
                    &write_fixture.token,
                    ptr::null(),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(count, 1);
        assert_eq!(write_fixture.state.query_count(), 12);
        assert_eq!(write_fixture.state.commits.load(Ordering::Acquire), 0);
        assert_eq!(write_fixture.state.rollbacks.load(Ordering::Acquire), 0);
        assert_eq!(
            unsafe { type_bridge_write_transaction_rollback(&mut write, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(write_fixture.state.rollbacks.load(Ordering::Acquire), 1);
    }

    #[test]
    fn relation_player_resolution_is_exact_and_ambiguous_hydration_rolls_back() {
        let fixture = relation_fixture(
            vec![
                documents(vec![serde_json::json!({"iid": "0x11"})]),
                insert_iid("0x20"),
                documents(vec![membership_with_relation_player_document("0x20")]),
            ],
            COMMIT_SUCCESS,
        );
        let create = relation_create(&fixture.package, TypeKind::Relation, "event", "0x11");
        let mut thing = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            // SAFETY: all immutable handles and writable output slots remain live.
            unsafe {
                type_bridge_database_relation_insert(
                    &*fixture.database,
                    &fixture.token,
                    &*create,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(unsafe { &*thing }.value.iid(), "0x20");
        let role = RoleId::new("membership", "member").unwrap();
        let player = &unsafe { &*thing }.value.roles()[&role][0];
        assert_eq!(player.reference().type_id().kind(), TypeKind::Relation);
        let queries = fixture.state.queries.lock().unwrap();
        assert_eq!(queries.len(), 3);
        assert!(queries[0].contains("$p isa! event"));
        assert!(queries[0].contains("$p iid 0x11"));
        assert!(!queries[0].contains("$p isa event"));
        drop(queries);
        close_thing(&mut thing);

        let ambiguous = relation_fixture(
            vec![
                insert_iid("0x20"),
                documents(vec![
                    membership_document("0x20"),
                    membership_document("0x21"),
                ]),
            ],
            COMMIT_SUCCESS,
        );
        assert_eq!(
            // SAFETY: all immutable handles and writable output slots remain live.
            unsafe {
                type_bridge_database_relation_insert(
                    &*ambiguous.database,
                    &ambiguous.token,
                    &*ambiguous.create,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed
        );
        assert!(thing.is_null());
        assert_eq!(diagnostic(diagnostics).1, "provider_hydration_failed");
        assert_eq!(ambiguous.state.commits.load(Ordering::Acquire), 0);
        assert_eq!(ambiguous.state.rollbacks.load(Ordering::Acquire), 1);
        close_diagnostics(&mut diagnostics);
    }

    #[test]
    fn known_origin_relation_players_reject_all_owned_mutations_before_open() {
        let package = package("relationorigin");
        let event_token = model_token(&package, TypeId::new(TypeKind::Relation, "event").unwrap());
        let membership_token = model_token(
            &package,
            TypeId::new(TypeKind::Relation, "membership").unwrap(),
        );
        let source_state = FakeState::new(
            vec![documents(vec![serde_json::json!({
                "_iid": "0x11",
                "_type": "event"
            })])],
            COMMIT_SUCCESS,
        );
        let source_database = Box::new(TypeBridgeDatabase::from_test_database(
            Arc::clone(&package.state),
            Database::with_backend(
                Box::new(FakeBackend {
                    state: Arc::clone(&source_state),
                }),
                "source",
            ),
        ));
        let mut thing = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            // SAFETY: all immutable handles and writable output slots remain live.
            unsafe {
                type_bridge_database_relation_get_by_iid(
                    &*source_database,
                    &event_token,
                    byte_view(b"0x11"),
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        let reference = unsafe { &*thing }
            .value
            .try_to_reference(&package.state.installed_projection)
            .unwrap();
        close_thing(&mut thing);
        let reference = Box::new(TypeBridgeProjectedReference {
            package: Arc::clone(&package.state),
            value: reference,
        });
        let mut cloned_reference = ptr::dangling_mut();
        assert_eq!(
            // SAFETY: the source reference and independent outputs remain live.
            unsafe {
                type_bridge_projected_reference_clone(
                    &*reference,
                    &mut cloned_reference,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert!(!cloned_reference.is_null());
        let cloned_value = unsafe { &*cloned_reference }.value.clone();
        let create = Box::new(TypeBridgeProjectedCreate {
            package: Arc::clone(&package.state),
            value: ProjectedCreate::try_new(
                &package.state.installed_projection,
                TypeId::new(TypeKind::Relation, "membership").unwrap(),
                vec![],
                vec![(
                    RoleId::new("membership", "member").unwrap(),
                    vec![cloned_value],
                )],
            )
            .unwrap(),
        });
        let target_state = FakeState::new(vec![], COMMIT_SUCCESS);
        let target_database = Box::new(TypeBridgeDatabase::from_test_database(
            Arc::clone(&package.state),
            Database::with_backend(
                Box::new(FakeBackend {
                    state: Arc::clone(&target_state),
                }),
                "target",
            ),
        ));
        macro_rules! assert_origin_rejected {
            ($status:expr) => {{
                let status = $status;
                assert_eq!(status, TypeBridgeStatus::InvalidArgument);
                assert!(thing.is_null());
                assert_eq!(diagnostic(diagnostics).1, "reference_database_mismatch");
                close_diagnostics(&mut diagnostics);
            }};
        }
        // SAFETY: every call retains all inputs; the mismatch must precede provider open.
        assert_origin_rejected!(unsafe {
            type_bridge_database_relation_insert(
                &*target_database,
                &membership_token,
                &*create,
                ptr::null(),
                &mut thing,
                &mut diagnostics,
            )
        });
        // SAFETY: every call retains all inputs; the mismatch must precede provider open.
        assert_origin_rejected!(unsafe {
            type_bridge_database_relation_put(
                &*target_database,
                &membership_token,
                &*create,
                ptr::null(),
                &mut thing,
                &mut diagnostics,
            )
        });
        // SAFETY: every call retains all inputs; the mismatch must precede provider open.
        assert_origin_rejected!(unsafe {
            type_bridge_database_relation_update(
                &*target_database,
                &membership_token,
                byte_view(b"0x20"),
                &*create,
                ptr::null(),
                &mut thing,
                &mut diagnostics,
            )
        });
        assert!(target_state.opens.lock().unwrap().is_empty());
        assert_eq!(target_state.query_count(), 0);
        assert_eq!(target_state.commits.load(Ordering::Acquire), 0);
        assert_eq!(target_state.rollbacks.load(Ordering::Acquire), 0);
        // SAFETY: this slot uniquely owns the cloned reference handle.
        assert_eq!(
            unsafe { type_bridge_projected_reference_close(&mut cloned_reference) },
            TypeBridgeStatus::Ok
        );
        assert!(reference.value.origin_carrier().is_some());
    }

    #[test]
    fn relation_failures_roll_back_and_commit_certainty_is_preserved() {
        let provider_failure = relation_fixture(vec![Response::Error], COMMIT_SUCCESS);
        let mut thing = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            // SAFETY: all retained inputs and writable outputs remain live.
            unsafe {
                type_bridge_database_relation_insert(
                    &*provider_failure.database,
                    &provider_failure.token,
                    &*provider_failure.create,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed
        );
        assert!(thing.is_null());
        let (_, code, message) = diagnostic(diagnostics);
        assert_eq!(code, "provider_operation_failed");
        assert!(!message.contains("provider-secret"));
        assert_eq!(provider_failure.state.commits.load(Ordering::Acquire), 0);
        assert_eq!(provider_failure.state.rollbacks.load(Ordering::Acquire), 1);
        close_diagnostics(&mut diagnostics);

        for (behavior, expected_status, expected_code) in [
            (
                COMMIT_ABORTED,
                TypeBridgeStatus::ExecutionFailed,
                "commit_definitely_aborted",
            ),
            (
                COMMIT_UNKNOWN,
                TypeBridgeStatus::CommitOutcomeUnknown,
                "commit_outcome_unknown",
            ),
            (
                COMMIT_PANIC,
                TypeBridgeStatus::CommitOutcomeUnknown,
                "commit_outcome_unknown",
            ),
        ] {
            let fixture = relation_fixture(
                vec![
                    insert_iid("0x20"),
                    documents(vec![membership_document("0x20")]),
                ],
                behavior,
            );
            assert_eq!(
                // SAFETY: all retained inputs and writable outputs remain live.
                unsafe {
                    type_bridge_database_relation_insert(
                        &*fixture.database,
                        &fixture.token,
                        &*fixture.create,
                        ptr::null(),
                        &mut thing,
                        &mut diagnostics,
                    )
                },
                expected_status
            );
            assert!(thing.is_null());
            let (category, code, message) = diagnostic(diagnostics);
            assert_eq!(category, TypeBridgeExecutionDiagnosticCategory::Transaction);
            assert_eq!(code, expected_code);
            assert!(!message.contains("provider-secret"));
            assert_eq!(fixture.state.commits.load(Ordering::Acquire), 1);
            assert_eq!(fixture.state.rollbacks.load(Ordering::Acquire), 0);
            close_diagnostics(&mut diagnostics);
        }
    }

    #[test]
    fn relation_provider_panic_poisons_borrowed_handles_until_cleanup() {
        let fixture = relation_fixture(vec![Response::Panic], COMMIT_SUCCESS);
        let mut transaction = ptr::null_mut();
        let mut diagnostics = ptr::null_mut();
        assert_eq!(
            // SAFETY: transaction owner and diagnostics slots are distinct and writable.
            unsafe {
                type_bridge_write_transaction_open(
                    &*fixture.database,
                    ptr::null(),
                    &mut transaction,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        let mut count = u64::MAX;
        assert_eq!(
            // SAFETY: the transaction is live and exclusively borrowed for this call.
            unsafe {
                type_bridge_write_transaction_relation_count(
                    transaction,
                    &fixture.token,
                    ptr::null(),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Panic
        );
        assert_eq!(count, 0);
        assert_eq!(fixture.state.query_count(), 1);
        count = u64::MAX;
        assert_eq!(
            // SAFETY: a poisoned handle remains allocated but admits no CRUD dispatch.
            unsafe {
                type_bridge_write_transaction_relation_count(
                    transaction,
                    &fixture.token,
                    ptr::null(),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed
        );
        assert_eq!(count, 0);
        assert_eq!(diagnostic(diagnostics).1, "c_transaction_poisoned");
        assert_eq!(fixture.state.query_count(), 1);
        close_diagnostics(&mut diagnostics);
        assert_eq!(
            // SAFETY: commit rejection must not consume the poisoned owner slot.
            unsafe {
                type_bridge_write_transaction_commit(
                    &mut transaction,
                    ptr::null(),
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed
        );
        assert!(!transaction.is_null());
        assert_eq!(diagnostic(diagnostics).1, "c_transaction_poisoned");
        assert_eq!(fixture.state.commits.load(Ordering::Acquire), 0);
        close_diagnostics(&mut diagnostics);
        assert_eq!(
            // SAFETY: rollback is the permitted terminal cleanup for poisoned writes.
            unsafe { type_bridge_write_transaction_rollback(&mut transaction, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(fixture.state.rollbacks.load(Ordering::Acquire), 1);

        let read_fixture = relation_fixture(vec![Response::Panic], COMMIT_SUCCESS);
        let mut read = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_open(
                    &*read_fixture.database,
                    ptr::null(),
                    &mut read,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_relation_count(
                    read,
                    &read_fixture.token,
                    ptr::null(),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Panic
        );
        assert_eq!(
            unsafe {
                type_bridge_read_transaction_relation_count(
                    read,
                    &read_fixture.token,
                    ptr::null(),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed
        );
        assert_eq!(diagnostic(diagnostics).1, "c_transaction_poisoned");
        close_diagnostics(&mut diagnostics);
        assert_eq!(
            unsafe { type_bridge_read_transaction_close(&mut read, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(read_fixture.state.query_count(), 1);
        assert_eq!(read_fixture.state.closes.load(Ordering::Acquire), 1);
    }

    #[test]
    fn repeated_relation_blind_delete_is_one_exact_absence_safe_statement() {
        let fixture = relation_fixture(
            vec![
                Response::Result(QueryResult::Ok),
                Response::Result(QueryResult::Ok),
            ],
            COMMIT_SUCCESS,
        );
        let mut diagnostics = ptr::dangling_mut();
        for _ in 0..2 {
            assert_eq!(
                // SAFETY: all retained inputs and the diagnostics output remain live.
                unsafe {
                    type_bridge_database_relation_delete_by_iid(
                        &*fixture.database,
                        &fixture.token,
                        byte_view(b"0x20"),
                        ptr::null(),
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Ok
            );
            assert!(diagnostics.is_null());
        }
        assert_eq!(fixture.state.query_count(), 2);
        assert_eq!(fixture.state.commits.load(Ordering::Acquire), 2);
        let queries = fixture.state.queries.lock().unwrap();
        assert!(queries.iter().all(|query| query.contains("delete")));
        assert!(
            queries
                .iter()
                .all(|query| query.contains("isa! membership"))
        );
    }

    #[test]
    fn relation_fences_and_full_input_aliases_are_pre_io_and_read_only() {
        let fixture = relation_fixture(
            vec![Response::Result(QueryResult::Rows(vec![
                serde_json::json!({
                    "$count": 4
                }),
            ]))],
            COMMIT_SUCCESS,
        );
        let mut thing = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();

        let mut cancellation = ptr::null_mut();
        assert_eq!(
            // SAFETY: the cancellation owner slot is writable.
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            // SAFETY: the cancellation handle is live and exclusively borrowed.
            unsafe { type_bridge_cancellation_request(cancellation) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            // SAFETY: all retained inputs and outputs remain live for the call.
            unsafe {
                type_bridge_database_relation_insert(
                    &*fixture.database,
                    &fixture.token,
                    &*fixture.create,
                    cancellation,
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Cancelled
        );
        assert_eq!(diagnostic(diagnostics).1, "cancelled_before_dispatch");
        close_diagnostics(&mut diagnostics);

        let foreign = package("foreignrelation");
        let foreign_create = relation_create(&foreign, TypeKind::Entity, "person", "0x10");
        assert_eq!(
            unsafe {
                type_bridge_database_relation_insert(
                    &*fixture.database,
                    &fixture.token,
                    &*foreign_create,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed
        );
        assert_eq!(
            diagnostic(diagnostics).1,
            "generated_token_package_mismatch"
        );
        close_diagnostics(&mut diagnostics);

        let event_create = Box::new(TypeBridgeProjectedCreate {
            package: Arc::clone(&fixture.package.state),
            value: ProjectedCreate::try_new(
                &fixture.package.state.installed_projection,
                TypeId::new(TypeKind::Relation, "event").unwrap(),
                vec![],
                vec![],
            )
            .unwrap(),
        });
        assert_eq!(
            unsafe {
                type_bridge_database_relation_insert(
                    &*fixture.database,
                    &fixture.token,
                    &*event_create,
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(
            diagnostic(diagnostics).1,
            "c_relation_create_model_mismatch"
        );
        close_diagnostics(&mut diagnostics);

        let entity_token = model_token(
            &fixture.package,
            TypeId::new(TypeKind::Entity, "person").unwrap(),
        );
        let mut count = u64::MAX;
        assert_eq!(
            unsafe {
                type_bridge_database_relation_count(
                    &*fixture.database,
                    &entity_token,
                    ptr::null(),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(count, 0);
        assert_eq!(diagnostic(diagnostics).1, "c_relation_model_kind_invalid");
        close_diagnostics(&mut diagnostics);

        let mut forged = fixture.token;
        forged.projection_digest[0] ^= 0xff;
        assert_eq!(
            unsafe {
                type_bridge_database_relation_get_by_iid(
                    &*fixture.database,
                    &forged,
                    byte_view(b"0x20"),
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed
        );
        assert_eq!(
            diagnostic(diagnostics).1,
            "generated_token_package_mismatch"
        );
        close_diagnostics(&mut diagnostics);
        assert_eq!(
            unsafe {
                type_bridge_database_relation_get_by_iid(
                    &*fixture.database,
                    &fixture.token,
                    byte_view(b"not-an-iid"),
                    ptr::null(),
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(diagnostic(diagnostics).1, "c_relation_iid_invalid");
        close_diagnostics(&mut diagnostics);

        let mut untouched = ptr::dangling_mut();
        let database_output = (&*fixture.database as *const TypeBridgeDatabase)
            .cast_mut()
            .cast::<u64>();
        assert_eq!(
            unsafe {
                type_bridge_database_relation_count(
                    &*fixture.database,
                    &fixture.token,
                    ptr::null(),
                    database_output,
                    &mut untouched,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(untouched, ptr::dangling_mut());
        let model_output = (&fixture.token as *const TypeBridgeProjectedTokenV1)
            .cast_mut()
            .cast::<u64>();
        assert_eq!(
            unsafe {
                type_bridge_database_relation_count(
                    &*fixture.database,
                    &fixture.token,
                    ptr::null(),
                    model_output,
                    &mut untouched,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        let create_output = (&*fixture.create as *const TypeBridgeProjectedCreate)
            .cast_mut()
            .cast::<*mut TypeBridgeProjectedThing>();
        assert_eq!(
            unsafe {
                type_bridge_database_relation_insert(
                    &*fixture.database,
                    &fixture.token,
                    &*fixture.create,
                    ptr::null(),
                    create_output,
                    &mut untouched,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        let cancellation_output = cancellation.cast::<u64>();
        assert_eq!(
            unsafe {
                type_bridge_database_relation_count(
                    &*fixture.database,
                    &fixture.token,
                    cancellation,
                    cancellation_output,
                    &mut untouched,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        let mut requested = u8::MAX;
        assert_eq!(
            unsafe { type_bridge_cancellation_is_requested(cancellation, &mut requested) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(requested, 1);
        assert_eq!(
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok
        );

        let mut iid_storage = AlignedBytes([0; 24]);
        iid_storage.0[..4].copy_from_slice(b"0x20");
        let iid = TypeBridgeByteView {
            data: iid_storage.0.as_ptr(),
            length: 4,
        };
        assert_eq!(
            unsafe {
                type_bridge_database_relation_get_by_iid(
                    &*fixture.database,
                    &fixture.token,
                    iid,
                    ptr::null(),
                    iid_storage.0.as_mut_ptr().cast(),
                    &mut untouched,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(&iid_storage.0[..4], b"0x20");
        assert!(fixture.state.opens.lock().unwrap().is_empty());
        assert_eq!(fixture.state.query_count(), 0);

        diagnostics = ptr::dangling_mut();
        assert_eq!(
            unsafe {
                type_bridge_database_relation_count(
                    &*fixture.database,
                    &fixture.token,
                    ptr::null(),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(count, 4);
        assert!(diagnostics.is_null());
        assert_eq!(fixture.state.query_count(), 1);

        let transaction_fixture = relation_fixture(
            vec![Response::Result(QueryResult::Rows(vec![
                serde_json::json!({
                    "$count": 2
                }),
            ]))],
            COMMIT_SUCCESS,
        );
        let mut transaction = ptr::null_mut();
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_open(
                    &*transaction_fixture.database,
                    ptr::null(),
                    &mut transaction,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        let transaction_output = transaction.cast::<u64>();
        untouched = ptr::dangling_mut();
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_relation_count(
                    transaction,
                    &transaction_fixture.token,
                    ptr::null(),
                    transaction_output,
                    &mut untouched,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(untouched, ptr::dangling_mut());
        assert_eq!(transaction_fixture.state.query_count(), 0);
        assert_eq!(
            unsafe {
                type_bridge_write_transaction_relation_count(
                    transaction,
                    &transaction_fixture.token,
                    ptr::null(),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(count, 2);
        assert_eq!(
            unsafe { type_bridge_write_transaction_rollback(&mut transaction, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
    }
}

/// Put one exact projected entity using one owned write transaction and commit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_entity_put(
    database: *const TypeBridgeDatabase,
    model: *const TypeBridgeProjectedTokenV1,
    create: *const TypeBridgeProjectedCreate,
    cancellation: *const TypeBridgeCancellation,
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller input ranges are described before any output write.
    let prepared = match unsafe {
        prepare_thing_outputs(
            &[
                MemoryRange::of_object(database),
                MemoryRange::of_object(model),
                MemoryRange::of_object(create),
                MemoryRange::of_object(cancellation),
            ],
            out_thing,
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if database.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller retains the database handle and all delegated inputs for this call.
    unsafe {
        run_put_call(
            CrudKind::Entity,
            WriteTarget::Database(&*database),
            prepared,
            model,
            create,
            cancellation,
        )
    }
}

/// Read one exact projected entity by IID using one owned read transaction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_entity_get_by_iid(
    database: *const TypeBridgeDatabase,
    model: *const TypeBridgeProjectedTokenV1,
    iid: TypeBridgeByteView,
    cancellation: *const TypeBridgeCancellation,
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller input ranges are described before any output write.
    let prepared = match unsafe {
        prepare_thing_outputs(
            &[
                MemoryRange::of_object(database),
                MemoryRange::of_object(model),
                MemoryRange::of_bytes(iid),
                MemoryRange::of_object(cancellation),
            ],
            out_thing,
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if database.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller retains the database handle and all delegated inputs for this call.
    unsafe {
        run_get_call(
            CrudKind::Entity,
            ReadTarget::Database(&*database),
            prepared,
            model,
            iid,
            cancellation,
        )
    }
}

/// Replace and rehydrate one exact projected entity, then commit once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_entity_update(
    database: *const TypeBridgeDatabase,
    model: *const TypeBridgeProjectedTokenV1,
    iid: TypeBridgeByteView,
    create: *const TypeBridgeProjectedCreate,
    cancellation: *const TypeBridgeCancellation,
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller input ranges are described before any output write.
    let prepared = match unsafe {
        prepare_thing_outputs(
            &[
                MemoryRange::of_object(database),
                MemoryRange::of_object(model),
                MemoryRange::of_bytes(iid),
                MemoryRange::of_object(create),
                MemoryRange::of_object(cancellation),
            ],
            out_thing,
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if database.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller retains the database handle and all delegated inputs for this call.
    unsafe {
        run_write_thing_call(
            CrudKind::Entity,
            WriteTarget::Database(&*database),
            prepared,
            model,
            create,
            Some(iid),
            cancellation,
        )
    }
}

/// Blind-delete one exact projected entity by IID and commit once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_entity_delete_by_iid(
    database: *const TypeBridgeDatabase,
    model: *const TypeBridgeProjectedTokenV1,
    iid: TypeBridgeByteView,
    cancellation: *const TypeBridgeCancellation,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let prepared = match prepare_diagnostics_output(
        &[
            MemoryRange::of_object(database),
            MemoryRange::of_object(model),
            MemoryRange::of_bytes(iid),
            MemoryRange::of_object(cancellation),
        ],
        out_diagnostics,
    ) {
        Ok(value) => value,
        Err(status) => return status,
    };
    if database.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller retains the database handle and all delegated inputs for this call.
    unsafe {
        run_delete_call(
            CrudKind::Entity,
            WriteTarget::Database(&*database),
            prepared,
            model,
            iid,
            cancellation,
        )
    }
}

/// Count exact projected entity instances using one owned read transaction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_database_entity_count(
    database: *const TypeBridgeDatabase,
    model: *const TypeBridgeProjectedTokenV1,
    cancellation: *const TypeBridgeCancellation,
    out_count: *mut u64,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let prepared = match prepare_count_outputs(
        &[
            MemoryRange::of_object(database),
            MemoryRange::of_object(model),
            MemoryRange::of_object(cancellation),
        ],
        out_count,
        out_diagnostics,
    ) {
        Ok(value) => value,
        Err(status) => return status,
    };
    if database.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller retains the database handle and all delegated inputs for this call.
    unsafe {
        run_count_call(
            CrudKind::Entity,
            ReadTarget::Database(&*database),
            prepared,
            model,
            cancellation,
        )
    }
}

/// Read one exact entity without consuming the caller's read transaction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_read_transaction_entity_get_by_iid(
    transaction: *const TypeBridgeReadTransaction,
    model: *const TypeBridgeProjectedTokenV1,
    iid: TypeBridgeByteView,
    cancellation: *const TypeBridgeCancellation,
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller input ranges are described before any output write.
    let prepared = match unsafe {
        prepare_thing_outputs(
            &[
                MemoryRange::of_object(transaction),
                MemoryRange::of_object(model),
                MemoryRange::of_bytes(iid),
                MemoryRange::of_object(cancellation),
            ],
            out_thing,
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if transaction.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller provides serialized access to the live transaction handle.
    unsafe {
        run_get_call(
            CrudKind::Entity,
            ReadTarget::ReadTransaction(&*transaction),
            prepared,
            model,
            iid,
            cancellation,
        )
    }
}

/// Count exact entities without consuming the caller's read transaction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_read_transaction_entity_count(
    transaction: *const TypeBridgeReadTransaction,
    model: *const TypeBridgeProjectedTokenV1,
    cancellation: *const TypeBridgeCancellation,
    out_count: *mut u64,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let prepared = match prepare_count_outputs(
        &[
            MemoryRange::of_object(transaction),
            MemoryRange::of_object(model),
            MemoryRange::of_object(cancellation),
        ],
        out_count,
        out_diagnostics,
    ) {
        Ok(value) => value,
        Err(status) => return status,
    };
    if transaction.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller provides serialized access to the live transaction handle.
    unsafe {
        run_count_call(
            CrudKind::Entity,
            ReadTarget::ReadTransaction(&*transaction),
            prepared,
            model,
            cancellation,
        )
    }
}

macro_rules! invoke_write_transaction_create {
    (insert, $target:expr, $prepared:expr, $model:expr, $create:expr, $cancellation:expr, $out_thing:expr, $out_diagnostics:expr $(,)?) => {
        run_write_thing_call(
            CrudKind::Entity,
            $target,
            $prepared,
            $model,
            $create,
            None,
            $cancellation,
        )
    };
    (put, $target:expr, $prepared:expr, $model:expr, $create:expr, $cancellation:expr, $out_thing:expr, $out_diagnostics:expr $(,)?) => {
        run_put_call(
            CrudKind::Entity,
            $target,
            $prepared,
            $model,
            $create,
            $cancellation,
        )
    };
}

macro_rules! write_transaction_create_operation {
    ($name:ident, $operation:ident) => {
        #[doc = "Execute one exact entity mutation without terminally consuming the write transaction."]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(
            transaction: *const TypeBridgeWriteTransaction,
            model: *const TypeBridgeProjectedTokenV1,
            create: *const TypeBridgeProjectedCreate,
            cancellation: *const TypeBridgeCancellation,
            out_thing: *mut *mut TypeBridgeProjectedThing,
            out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            // SAFETY: all caller input ranges are described before any output write.
            let prepared = match unsafe {
                prepare_thing_outputs(
                    &[
                        MemoryRange::of_object(transaction),
                        MemoryRange::of_object(model),
                        MemoryRange::of_object(create),
                        MemoryRange::of_object(cancellation),
                    ],
                    out_thing,
                    out_diagnostics,
                )
            } {
                Ok(value) => value,
                Err(status) => return status,
            };
            if transaction.is_null() {
                return TypeBridgeStatus::InvalidArgument;
            }
            // SAFETY: caller provides serialized access to the live transaction handle.
            unsafe {
                invoke_write_transaction_create!(
                    $operation,
                    WriteTarget::Transaction(&*transaction),
                    prepared,
                    model,
                    create,
                    cancellation,
                    out_thing,
                    out_diagnostics,
                )
            }
        }
    };
}

write_transaction_create_operation!(type_bridge_write_transaction_entity_insert, insert);
write_transaction_create_operation!(type_bridge_write_transaction_entity_put, put);

/// Update one exact entity without consuming the caller's write transaction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_write_transaction_entity_update(
    transaction: *const TypeBridgeWriteTransaction,
    model: *const TypeBridgeProjectedTokenV1,
    iid: TypeBridgeByteView,
    create: *const TypeBridgeProjectedCreate,
    cancellation: *const TypeBridgeCancellation,
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller input ranges are described before any output write.
    let prepared = match unsafe {
        prepare_thing_outputs(
            &[
                MemoryRange::of_object(transaction),
                MemoryRange::of_object(model),
                MemoryRange::of_bytes(iid),
                MemoryRange::of_object(create),
                MemoryRange::of_object(cancellation),
            ],
            out_thing,
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if transaction.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller provides serialized access to the live transaction handle.
    unsafe {
        run_write_thing_call(
            CrudKind::Entity,
            WriteTarget::Transaction(&*transaction),
            prepared,
            model,
            create,
            Some(iid),
            cancellation,
        )
    }
}

/// Read one exact entity without consuming the caller's write transaction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_write_transaction_entity_get_by_iid(
    transaction: *const TypeBridgeWriteTransaction,
    model: *const TypeBridgeProjectedTokenV1,
    iid: TypeBridgeByteView,
    cancellation: *const TypeBridgeCancellation,
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: all caller input ranges are described before any output write.
    let prepared = match unsafe {
        prepare_thing_outputs(
            &[
                MemoryRange::of_object(transaction),
                MemoryRange::of_object(model),
                MemoryRange::of_bytes(iid),
                MemoryRange::of_object(cancellation),
            ],
            out_thing,
            out_diagnostics,
        )
    } {
        Ok(value) => value,
        Err(status) => return status,
    };
    if transaction.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller provides serialized access to the live transaction handle.
    unsafe {
        run_get_call(
            CrudKind::Entity,
            ReadTarget::WriteTransaction(&*transaction),
            prepared,
            model,
            iid,
            cancellation,
        )
    }
}

/// Blind-delete one entity without consuming the caller's write transaction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_write_transaction_entity_delete_by_iid(
    transaction: *const TypeBridgeWriteTransaction,
    model: *const TypeBridgeProjectedTokenV1,
    iid: TypeBridgeByteView,
    cancellation: *const TypeBridgeCancellation,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let prepared = match prepare_diagnostics_output(
        &[
            MemoryRange::of_object(transaction),
            MemoryRange::of_object(model),
            MemoryRange::of_bytes(iid),
            MemoryRange::of_object(cancellation),
        ],
        out_diagnostics,
    ) {
        Ok(value) => value,
        Err(status) => return status,
    };
    if transaction.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller provides serialized access to the live transaction handle.
    unsafe {
        run_delete_call(
            CrudKind::Entity,
            WriteTarget::Transaction(&*transaction),
            prepared,
            model,
            iid,
            cancellation,
        )
    }
}

/// Count exact entities without consuming the caller's write transaction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_write_transaction_entity_count(
    transaction: *const TypeBridgeWriteTransaction,
    model: *const TypeBridgeProjectedTokenV1,
    cancellation: *const TypeBridgeCancellation,
    out_count: *mut u64,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let prepared = match prepare_count_outputs(
        &[
            MemoryRange::of_object(transaction),
            MemoryRange::of_object(model),
            MemoryRange::of_object(cancellation),
        ],
        out_count,
        out_diagnostics,
    ) {
        Ok(value) => value,
        Err(status) => return status,
    };
    if transaction.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: caller provides serialized access to the live transaction handle.
    unsafe {
        run_count_call(
            CrudKind::Entity,
            ReadTarget::WriteTransaction(&*transaction),
            prepared,
            model,
            cancellation,
        )
    }
}
