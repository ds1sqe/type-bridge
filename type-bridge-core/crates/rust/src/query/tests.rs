use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::{future::Future, pin::Pin};

use crate::__codegen::{
    self, CompleteModel, EncodedCreate, EntityModel, HydratedRow, HydrationCapability,
    IntoEncodedCreate, MaterializeModel, Model, ReferenceOrigin, SubtypeRootModel, ThingModel,
    ValidationError,
};
use crate::schema::{Schema, sealed};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig};
use type_bridge_contract::schema::DocumentId;
use type_bridge_orm::session::backend::{
    BoxFuture, DriverBackend, QueryResult, TransactionOps, TxType,
};
use type_bridge_orm::{CapabilitySet, Database as OrmDatabase, OrmError};
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
use type_bridge_schema_codegen::RustEmitter;

#[derive(Debug)]
struct TestSchema;
impl sealed::Sealed for TestSchema {}
impl Schema for TestSchema {}

const RECORD_JSON: &str = r#"{"kind":"entity","label":"record"}"#;
const ROUTE_JSON: &str = r#"{"kind":"relation","label":"route"}"#;
const NAME_OWNS: &str = r#"{"attribute":"name","owner":{"kind":"entity","label":"record"}}"#;
const TALLY_OWNS: &str = r#"{"attribute":"tally","owner":{"kind":"entity","label":"record"}}"#;
const ROUTE_FROM: &str = r#"{"declaring_relation":"route","label":"origin"}"#;
const ROUTE_TO: &str = r#"{"declaring_relation":"route","label":"destination"}"#;
const SUCCESSOR_SCHEMA_SOURCE: &str = r#"format: typebridge.schema/v2
attributes:
  name: { value: string }
  tally: { value: integer }
  tags: { value: string }
entities:
  base: { abstract: true }
  record:
    sub: base
    owns:
      name: { key: true }
      tally: { card: 1 }
  other:
    owns:
      name: { key: true }
  ordered-marker:
    owns:
      tags: { card: { min: 0, max: 2 }, ordered: true }
relations:
  container:
    relates:
      item: { card: 1 }
  route:
    relates:
      origin: { card: 1 }
      destination: { card: 1 }
plays:
  record:
    route:
      origin: { card: 1 }
      destination: { card: 1 }
  route:
    container: [item]
"#;

#[derive(Clone, Debug)]
struct RecordCreate;
impl sealed::Sealed for RecordCreate {}
impl IntoEncodedCreate for RecordCreate {
    fn into_encoded_create(self) -> Result<EncodedCreate, ValidationError> {
        Ok(EncodedCreate::new(RECORD_JSON, vec![], vec![]))
    }
}

#[test]
fn selected_materialization_canonicalizes_remote_compatibility_scalars() {
    use type_bridge_orm::AttributeValue;

    assert_eq!(
        super::canonicalize_selected_value(AttributeValue::DateTime(
            "2026-08-03T03:55:00.000000000".into(),
        ))
        .unwrap(),
        AttributeValue::DateTime("2026-08-03T03:55:00".into())
    );
    assert_eq!(
        super::canonicalize_selected_value(AttributeValue::DateTimeTZ(
            "2026-08-03T03:55:00.000000000+00:00".into(),
        ))
        .unwrap(),
        AttributeValue::DateTimeTZ("2026-08-03T03:55:00Z".into())
    );
    assert_eq!(
        super::canonicalize_selected_value(AttributeValue::Decimal("00123.4500dec".into()))
            .unwrap(),
        AttributeValue::Decimal("123.45".into())
    );
    assert_eq!(
        super::canonicalize_selected_value(AttributeValue::Duration("P1D".into())).unwrap(),
        AttributeValue::Duration("P1D".into())
    );
    assert_eq!(
        super::canonicalize_selected_value(AttributeValue::Duration("PT1H".into())).unwrap(),
        AttributeValue::Duration("PT1H".into())
    );
    match super::canonicalize_selected_value(AttributeValue::DateTime("not-a-datetime".into()))
        .unwrap_err()
    {
        crate::Error::ModelValidation { phase, code, .. } => {
            assert_eq!(phase, crate::ModelValidationPhase::Hydration);
            assert_eq!(code, "hydrated_attribute_value_type");
        }
        other => panic!("unexpected scalar canonicalization error: {other:?}"),
    }
}

#[derive(Debug)]
struct Record;
impl sealed::Sealed for Record {}
impl Model for Record {
    type Schema = TestSchema;
    const TYPE_ID_JSON: &'static str = RECORD_JSON;
}
impl ThingModel for Record {
    fn thing_kind() -> __codegen::ThingKind {
        __codegen::ThingKind::Entity
    }
}
impl EntityModel for Record {}
impl __codegen::NominalUpcast<Record> for Record {}
impl __codegen::NominalUpcast<Base> for Record {}
impl CompleteModel for Record {
    type Create = RecordCreate;
    fn iid(&self) -> &str {
        unreachable!()
    }
}
impl MaterializeModel for Record {
    fn materialize(row: &HydratedRow, _cap: &HydrationCapability) -> Result<Self, ValidationError> {
        row.validate_shape(
            Self::TYPE_ID_JSON,
            &[NAME_OWNS, TALLY_OWNS],
            &[],
            &__codegen::ValidationPath::root(),
        )?;
        Ok(Self)
    }
}

#[derive(Debug)]
struct OriginRecord {
    iid: String,
    origin: ReferenceOrigin,
}
impl sealed::Sealed for OriginRecord {}
impl Model for OriginRecord {
    type Schema = TestSchema;
    const TYPE_ID_JSON: &'static str = RECORD_JSON;
}
impl ThingModel for OriginRecord {
    fn thing_kind() -> __codegen::ThingKind {
        __codegen::ThingKind::Entity
    }
}
impl EntityModel for OriginRecord {}
impl __codegen::NominalUpcast<OriginRecord> for OriginRecord {}
impl CompleteModel for OriginRecord {
    type Create = RecordCreate;

    fn iid(&self) -> &str {
        &self.iid
    }
}
impl MaterializeModel for OriginRecord {
    fn materialize(row: &HydratedRow, _cap: &HydrationCapability) -> Result<Self, ValidationError> {
        row.validate_shape(
            Self::TYPE_ID_JSON,
            &[NAME_OWNS, TALLY_OWNS],
            &[],
            &__codegen::ValidationPath::root(),
        )?;
        Ok(Self {
            iid: row.iid().to_owned(),
            origin: row.origin().clone(),
        })
    }
}

#[derive(Debug)]
struct SlowRecord;
impl sealed::Sealed for SlowRecord {}
impl Model for SlowRecord {
    type Schema = TestSchema;
    const TYPE_ID_JSON: &'static str = RECORD_JSON;
}
impl ThingModel for SlowRecord {
    fn thing_kind() -> __codegen::ThingKind {
        __codegen::ThingKind::Entity
    }
}
impl EntityModel for SlowRecord {}
impl __codegen::NominalUpcast<SlowRecord> for SlowRecord {}
impl CompleteModel for SlowRecord {
    type Create = RecordCreate;
    fn iid(&self) -> &str {
        unreachable!()
    }
}

static SLOW_MATERIALIZATION_CALLS: AtomicUsize = AtomicUsize::new(0);

impl MaterializeModel for SlowRecord {
    fn materialize(_: &HydratedRow, _: &HydrationCapability) -> Result<Self, ValidationError> {
        SLOW_MATERIALIZATION_CALLS.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(25));
        Ok(Self)
    }
}

#[derive(Debug)]
struct SuccessorSlowRecord;
impl sealed::Sealed for SuccessorSlowRecord {}
impl Model for SuccessorSlowRecord {
    type Schema = TestSchema;
    const TYPE_ID_JSON: &'static str = RECORD_JSON;
}
impl ThingModel for SuccessorSlowRecord {
    fn thing_kind() -> __codegen::ThingKind {
        __codegen::ThingKind::Entity
    }
}
impl EntityModel for SuccessorSlowRecord {}
impl __codegen::NominalUpcast<SuccessorSlowRecord> for SuccessorSlowRecord {}
impl CompleteModel for SuccessorSlowRecord {
    type Create = RecordCreate;
    fn iid(&self) -> &str {
        unreachable!()
    }
}

static SUCCESSOR_SLOW_MATERIALIZATION_CALLS: AtomicUsize = AtomicUsize::new(0);

impl MaterializeModel for SuccessorSlowRecord {
    fn materialize(_: &HydratedRow, _: &HydrationCapability) -> Result<Self, ValidationError> {
        SUCCESSOR_SLOW_MATERIALIZATION_CALLS.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(25));
        Ok(Self)
    }
}

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
impl __codegen::NominalUpcast<Base> for Base {}
impl SubtypeRootModel for Base {
    type Subtypes = Record;
    fn __tb_dispatch_subtype(
        row: &HydratedRow,
        cap: &HydrationCapability,
    ) -> Result<Self::Subtypes, ValidationError> {
        Record::materialize(row, cap)
    }
}

#[derive(Debug)]
struct Ghost;
impl sealed::Sealed for Ghost {}
impl Model for Ghost {
    type Schema = TestSchema;
    const TYPE_ID_JSON: &'static str = r#"{"kind":"entity","label":"ghost"}"#;
}
impl ThingModel for Ghost {
    fn thing_kind() -> __codegen::ThingKind {
        __codegen::ThingKind::Entity
    }
}
impl EntityModel for Ghost {}
impl CompleteModel for Ghost {
    type Create = RecordCreate;
    fn iid(&self) -> &str {
        unreachable!()
    }
}
impl MaterializeModel for Ghost {
    fn materialize(_: &HydratedRow, _: &HydrationCapability) -> Result<Self, ValidationError> {
        unreachable!()
    }
}

#[derive(Debug)]
struct Route;
impl sealed::Sealed for Route {}
impl Model for Route {
    type Schema = TestSchema;
    const TYPE_ID_JSON: &'static str = ROUTE_JSON;
}
impl ThingModel for Route {
    fn thing_kind() -> __codegen::ThingKind {
        __codegen::ThingKind::Relation
    }
}
impl __codegen::RelationModel for Route {}
impl __codegen::NominalUpcast<Route> for Route {}
impl CompleteModel for Route {
    type Create = RecordCreate;
    fn iid(&self) -> &str {
        unreachable!()
    }
}
impl MaterializeModel for Route {
    fn materialize(_: &HydratedRow, _: &HydrationCapability) -> Result<Self, ValidationError> {
        unreachable!()
    }
}

#[derive(Debug)]
struct OriginRoute {
    iid: String,
    origin: ReferenceOrigin,
    player_origins: Vec<ReferenceOrigin>,
}
impl sealed::Sealed for OriginRoute {}
impl Model for OriginRoute {
    type Schema = TestSchema;
    const TYPE_ID_JSON: &'static str = ROUTE_JSON;
}
impl ThingModel for OriginRoute {
    fn thing_kind() -> __codegen::ThingKind {
        __codegen::ThingKind::Relation
    }
}
impl __codegen::RelationModel for OriginRoute {}
impl CompleteModel for OriginRoute {
    type Create = RecordCreate;

    fn iid(&self) -> &str {
        &self.iid
    }
}
impl MaterializeModel for OriginRoute {
    fn materialize(row: &HydratedRow, _cap: &HydrationCapability) -> Result<Self, ValidationError> {
        row.validate_shape(
            Self::TYPE_ID_JSON,
            &[],
            &[ROUTE_FROM, ROUTE_TO],
            &__codegen::ValidationPath::root(),
        )?;
        Ok(Self {
            iid: row.iid().to_owned(),
            origin: row.origin().clone(),
            player_origins: row
                .roles()
                .iter()
                .flat_map(|(_, players)| players.iter())
                .map(|player| player.origin().clone())
                .collect(),
        })
    }
}

struct RouteFromPlayers;
struct RouteToPlayers;
impl __codegen::RolePlayer<Record> for RouteFromPlayers {}
impl __codegen::RolePlayer<Record> for RouteToPlayers {}
impl __codegen::RoleTokenCompatible<Route, RouteFromPlayers> for Route {}
impl __codegen::RoleTokenCompatible<Route, RouteToPlayers> for Route {}

fn minimal_fixture() -> type_bridge_orm::InstalledRuntimeProjection {
    let docs = SchemaDocumentSet::parse([(
        DocumentId::new("q01.yaml").unwrap(),
        r#"format: typebridge.schema/v2
attributes:
  name: { value: string }
  tally: { value: integer }
entities:
  base: { abstract: true }
  record:
    sub: base
    owns:
      name: { key: true }
      tally: { card: 1 }
  other:
    owns:
      name: { key: true }
relations:
  route:
    relates:
      origin: { card: 1 }
      destination: { card: 1 }
plays:
  record:
    route:
      origin: { card: 1 }
      destination: { card: 1 }
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

fn successor_fixture() -> type_bridge_orm::InstalledRuntimeProjection {
    let docs = SchemaDocumentSet::parse([(
        DocumentId::new("q02.yaml").unwrap(),
        SUCCESSOR_SCHEMA_SOURCE,
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
        &emitter.generator_handlers_for(&resolved),
        &emitter.code_resources_for(&resolved).unwrap(),
    )
    .unwrap();
    assert_eq!(
        projection.generator_handlers(),
        [type_bridge_contract::projection::ProjectionHandler::rust_v2()]
    );
    type_bridge_orm::InstalledRuntimeProjection::try_new(projection).unwrap()
}

fn successor_package() -> crate::schema::SchemaPackage<TestSchema> {
    use type_bridge_contract::codec::to_canonical_json;
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::schema::encode_declared_schema;
    use type_bridge_schema::{
        ManagedDeltaContext, build_schema_authority, encode_schema_authority,
        schema_authority_capability_vocabulary,
    };

    let docs = SchemaDocumentSet::parse([(
        DocumentId::new("q02.yaml").unwrap(),
        SUCCESSOR_SCHEMA_SOURCE,
    )])
    .unwrap();
    let declared = normalize_documents(&docs).unwrap();
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    let authority = build_schema_authority(
        &declared,
        declared.required_capabilities(),
        &ManagedDeltaContext::new(
            ManagedScopeId::new("query-origin-test").unwrap(),
            profile,
            schema_authority_capability_vocabulary(),
        ),
    )
    .unwrap();
    let emitter = RustEmitter::new();
    let projection = project(
        &resolved,
        BindingTarget::Rust,
        &ProjectionConfig::rust(),
        &emitter.generator_handlers_for(&resolved),
        &emitter.code_resources_for(&resolved).unwrap(),
    )
    .unwrap();
    let leak = |bytes: Vec<u8>| {
        Box::leak(String::from_utf8(bytes).unwrap().into_boxed_str()) as &'static str
    };
    crate::schema::SchemaPackage::new_with_authority(
        leak(to_canonical_json(projection.semantic_fingerprint()).unwrap()),
        leak(to_canonical_json(projection.projection_fingerprint()).unwrap()),
        leak(to_canonical_json(&projection).unwrap()),
        leak(encode_schema_authority(&authority)),
        leak(encode_declared_schema(&declared).unwrap()),
        "query-origin-test",
        "typedb-3.12.1/v1",
    )
}

struct RemoteOriginTransport {
    advertisement_contract: type_bridge_contract::query_remote::RemoteCapabilities,
    advertisement: Vec<u8>,
    exchanges: Arc<AtomicUsize>,
    signer: type_bridge_orm::query_v2_remote::RemoteReplySigningKey,
}

impl crate::remote::RemoteQueryTransport for RemoteOriginTransport {
    fn capabilities(&self) -> Pin<Box<dyn Future<Output = crate::Result<Vec<u8>>> + Send + '_>> {
        let advertisement = self.advertisement.clone();
        Box::pin(async move { Ok(advertisement) })
    }

    fn exchange<'a>(
        &'a self,
        request: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = crate::Result<Vec<u8>>> + Send + 'a>> {
        use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
        use type_bridge_contract::query_plan::CompatibilityValueV2;
        use type_bridge_contract::query_remote_v2::{
            HydratedRowV2, HydrationAttributeEvidenceV2, HydrationGraphV2, HydrationNodeIdV2,
            HydrationNodeKindV2, HydrationNodeV2, HydrationReferenceV2, HydrationSlotV2,
            RemoteOutcomeV2, RemoteQueryRequestV2, RemoteQueryResponseV2,
        };
        use type_bridge_contract::value::{CanonicalString, CanonicalValue};

        self.exchanges.fetch_add(1, Ordering::SeqCst);
        let request = RemoteQueryRequestV2::decode(request).unwrap();
        request
            .validate_advertisement(&self.advertisement_contract)
            .unwrap();
        let plan = request.plan().unwrap();
        let record = TypeId::new(TypeKind::Entity, "record").unwrap();
        let node = HydrationNodeIdV2::new(0);
        let graph = HydrationGraphV2::new(vec![HydrationNodeV2::new(
            node,
            "0x01".into(),
            record.clone(),
            HydrationNodeKindV2::Entity,
            vec![
                HydrationAttributeEvidenceV2::new(
                    AttributeId::new("name").unwrap(),
                    vec![CompatibilityValueV2::canonical(CanonicalValue::String(
                        CanonicalString::new("Alice").unwrap(),
                    ))],
                ),
                HydrationAttributeEvidenceV2::new(
                    AttributeId::new("tally").unwrap(),
                    vec![CompatibilityValueV2::canonical(CanonicalValue::Long(1))],
                ),
            ],
            vec![],
        )])
        .unwrap();
        let outcome = RemoteOutcomeV2::HydratedRows {
            graph,
            rows: vec![HydratedRowV2::new(vec![HydrationSlotV2::Singular {
                value: HydrationReferenceV2::new(record, node),
            }])],
        };
        let response = RemoteQueryResponseV2::new(
            request.nonce(),
            &plan,
            &request.fingerprint().unwrap(),
            request.result_kind(),
            outcome,
        )
        .unwrap()
        .encode_signed(
            &self.advertisement_contract.fingerprint().unwrap(),
            &self.signer,
        )
        .unwrap();
        Box::pin(async move { Ok(response) })
    }
}

fn remote_origin_transport() -> (RemoteOriginTransport, Arc<AtomicUsize>) {
    use type_bridge_contract::query_plan::query_plan_v2_capability_vocabulary;
    use type_bridge_contract::query_remote::{RemoteCapabilities, RemoteExecutorBinding};
    use type_bridge_contract::query_remote_v2::query_remote_v2_required_capabilities;
    use type_bridge_orm::query_v2_remote::RemoteReplySigningKey;

    let signer = RemoteReplySigningKey::from_secret_bytes([0x52; 32]);
    let mut capabilities = query_plan_v2_capability_vocabulary();
    for capability in query_remote_v2_required_capabilities(true) {
        capabilities.insert(capability);
    }
    let advertisement_contract = RemoteCapabilities::new(
        capabilities,
        RemoteExecutorBinding::new("query-origin-test", "epoch-00000000001").unwrap(),
        signer.public_key(),
    );
    let advertisement = advertisement_contract.encode().unwrap();
    let exchanges = Arc::new(AtomicUsize::new(0));
    (
        RemoteOriginTransport {
            advertisement_contract,
            advertisement,
            exchanges: Arc::clone(&exchanges),
            signer,
        },
        exchanges,
    )
}

#[derive(Clone, Copy)]
enum SuccessorAnswer {
    Record,
    RecordGrouped,
    RecordMany,
    RecordPair,
    RecordGraphPage,
    Route,
}

#[derive(Default)]
struct SuccessorState {
    opened: AtomicUsize,
    closed: AtomicUsize,
    statements: Mutex<Vec<String>>,
}

struct SuccessorBackend {
    answer: SuccessorAnswer,
    state: Arc<SuccessorState>,
}

impl DriverBackend for SuccessorBackend {
    fn match_capabilities(&self) -> CapabilitySet {
        CapabilitySet::all()
    }

    fn open_transaction(
        &self,
        _database: &str,
        _tx_type: TxType,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
        self.state.opened.fetch_add(1, Ordering::SeqCst);
        let transaction = SuccessorTransaction {
            answer: self.answer,
            state: Arc::clone(&self.state),
            statement: 0,
        };
        Box::pin(async move { Ok(Box::new(transaction) as Box<dyn TransactionOps>) })
    }

    fn is_open(&self) -> bool {
        true
    }
}

struct SuccessorTransaction {
    answer: SuccessorAnswer,
    state: Arc<SuccessorState>,
    statement: usize,
}

impl TransactionOps for SuccessorTransaction {
    fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        self.state
            .statements
            .lock()
            .unwrap()
            .push(typeql.to_owned());
        let statement = self.statement;
        self.statement += 1;
        let answer = self.answer;
        Box::pin(async move {
            match (statement, answer) {
                (0, SuccessorAnswer::Record) => Ok(QueryResult::Rows(vec![serde_json::json!({
                    "bindings": [{"binding": 0, "concept_id": "0x01"}],
                    "satisfied_role_edges": [],
                })])),
                (1, SuccessorAnswer::Record) => {
                    Ok(QueryResult::Documents(vec![record_hydration(0, "0x01")]))
                }
                (0, SuccessorAnswer::RecordGrouped) => {
                    Ok(QueryResult::Rows(vec![serde_json::json!({
                        "bindings": [{"binding": 0, "concept_id": "0x01"}],
                        "satisfied_role_edges": [],
                    })]))
                }
                (1, SuccessorAnswer::RecordGrouped) => {
                    Ok(QueryResult::Documents(vec![serde_json::json!({
                        "bindings": [
                            record_hydration(0, "0x01"),
                            record_hydration_with_name(1, "0x02", "Bob"),
                        ],
                        "satisfied_role_edges": [],
                    })]))
                }
                (0, SuccessorAnswer::RecordMany) => Ok(QueryResult::Rows(vec![
                    serde_json::json!({
                        "bindings": [{"binding": 0, "concept_id": "0x01"}],
                        "satisfied_role_edges": [],
                    }),
                    serde_json::json!({
                        "bindings": [{"binding": 0, "concept_id": "0x02"}],
                        "satisfied_role_edges": [],
                    }),
                ])),
                (1, SuccessorAnswer::RecordMany) => Ok(QueryResult::Documents(vec![
                    record_hydration(0, "0x01"),
                    record_hydration_with_name(0, "0x02", "Bob"),
                ])),
                (0, SuccessorAnswer::RecordPair) => {
                    Ok(QueryResult::Rows(vec![serde_json::json!({
                        "bindings": [
                            {"binding": 0, "concept_id": "0x01"},
                            {"binding": 1, "concept_id": "0x02"},
                        ],
                        "satisfied_role_edges": [],
                    })]))
                }
                (1, SuccessorAnswer::RecordPair) => Ok(QueryResult::Documents(vec![
                    record_hydration(0, "0x01"),
                    record_hydration(1, "0x02"),
                ])),
                (0, SuccessorAnswer::RecordGraphPage) => {
                    Ok(QueryResult::Rows(vec![serde_json::json!({
                        "bindings": [{"binding": 0, "concept_id": "0x01"}],
                        "satisfied_role_edges": [],
                    })]))
                }
                (1, SuccessorAnswer::RecordGraphPage) => Ok(QueryResult::Documents(vec![
                    rematched_records("0x01", "0x02"),
                    rematched_records("0x01", "0x03"),
                ])),
                (0, SuccessorAnswer::Route) => Ok(QueryResult::Rows(vec![serde_json::json!({
                    "bindings": [{"binding": 0, "concept_id": "0x10"}],
                    "satisfied_role_edges": [],
                })])),
                (1, SuccessorAnswer::Route) => {
                    Ok(QueryResult::Documents(vec![route_hydration(0, "0x10")]))
                }
                _ => Err(OrmError::QueryExecution(format!(
                    "unexpected successor query statement {statement}"
                ))),
            }
        })
    }

    fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        Box::pin(async { Ok(()) })
    }

    fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        Box::pin(async { Ok(()) })
    }

    fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.state.closed.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}

fn record_hydration(binding: u16, concept_id: &str) -> serde_json::Value {
    record_hydration_with_name(binding, concept_id, "Alice")
}

fn record_hydration_with_name(binding: u16, concept_id: &str, name: &str) -> serde_json::Value {
    serde_json::json!({
        "binding": binding,
        "concept_id": concept_id,
        "concrete_type": "record",
        "kind": "entity",
        "attributes": [
            {"field": "name", "value_type": "string", "values": [name]},
            {"field": "tally", "value_type": "long", "values": [1]},
        ],
        "roles": [],
    })
}

fn rematched_records(root: &str, member: &str) -> serde_json::Value {
    let member_name = if member == "0x02" { "Bob" } else { "Carol" };
    serde_json::json!({
        "bindings": [
            record_hydration(0, root),
            record_hydration_with_name(1, member, member_name),
        ],
        "satisfied_role_edges": [],
    })
}

fn route_hydration(binding: u16, concept_id: &str) -> serde_json::Value {
    let player = |concept_id: &str, name: &str| {
        serde_json::json!({
            "concept_id": concept_id,
            "declared_type": "record",
            "concrete_type": "record",
            "kind": "entity",
            "attributes": [
                {"field": "name", "value_type": "string", "values": [name]},
                {"field": "tally", "value_type": "long", "values": [1]},
            ],
        })
    };
    serde_json::json!({
        "binding": binding,
        "concept_id": concept_id,
        "concrete_type": "route",
        "kind": "relation",
        "attributes": [],
        "roles": [
            {"role": "origin", "players": [player("0x01", "Alice")]},
            {"role": "destination", "players": [player("0x02", "Bob")]},
        ],
    })
}

fn successor_test_db(
    database: &str,
    answer: SuccessorAnswer,
) -> (crate::session::Database<TestSchema>, Arc<SuccessorState>) {
    let state = Arc::new(SuccessorState::default());
    let backend = SuccessorBackend {
        answer,
        state: Arc::clone(&state),
    };
    (
        crate::session::Database::<TestSchema>::from_test_parts(
            OrmDatabase::with_backend(Box::new(backend), database),
            successor_fixture(),
        ),
        state,
    )
}

fn legacy_origin_test_db() -> (crate::session::Database<TestSchema>, Arc<SuccessorState>) {
    let state = Arc::new(SuccessorState::default());
    let backend = SuccessorBackend {
        answer: SuccessorAnswer::Record,
        state: Arc::clone(&state),
    };
    (
        crate::session::Database::<TestSchema>::from_test_parts(
            OrmDatabase::with_backend(Box::new(backend), "legacy-origin"),
            minimal_fixture(),
        ),
        state,
    )
}

#[derive(Default)]
struct State {
    opened: usize,
}

struct Backend {
    state: Arc<Mutex<State>>,
}

impl DriverBackend for Backend {
    fn open_transaction(
        &self,
        _db: &str,
        _ty: TxType,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
        self.state.lock().unwrap().opened += 1;
        Box::pin(async { Err(OrmError::Transaction("query tests perform no I/O".into())) })
    }
    fn is_open(&self) -> bool {
        true
    }
}

fn test_db() -> (crate::session::Database<TestSchema>, Arc<Mutex<State>>) {
    let state = Arc::new(Mutex::new(State::default()));
    let backend = Backend {
        state: Arc::clone(&state),
    };
    (
        crate::session::Database::<TestSchema>::from_test_parts(
            OrmDatabase::with_backend(Box::new(backend), "q01"),
            minimal_fixture(),
        ),
        state,
    )
}

fn assert_model_error(error: crate::Error, code: &str) {
    let crate::Error::ModelValidation {
        phase,
        code: actual,
        ..
    } = error
    else {
        panic!("expected model validation error")
    };
    assert_eq!(phase, crate::error::ModelValidationPhase::Input);
    assert_eq!(actual, code);
}

#[test]
fn query_session_requires_schema_binding_before_io() {
    let state = Arc::new(Mutex::new(State::default()));
    let backend = Backend {
        state: Arc::clone(&state),
    };
    let db = crate::session::Database::<TestSchema>::from_test_unbound_parts(
        OrmDatabase::with_backend(Box::new(backend), "q01"),
    );
    assert_model_error(db.query().unwrap_err(), "schema_not_bound");
    assert_eq!(state.lock().unwrap().opened, 0);
}

#[test]
fn query_session_allocates_fresh_copy_bindings_without_io() {
    let (db, state) = test_db();
    let mut session = db.query().unwrap();
    let first = session.exact::<Record>().unwrap();
    let second = session.exact::<Record>().unwrap();
    let family = session.subtypes::<Base>().unwrap();
    assert_ne!(first, second);
    let copied = first;
    assert_eq!(copied, first);
    assert!(session.handle_by_key(first.key()).is_ok());
    assert!(session.handle_by_key(second.key()).is_ok());
    assert!(session.handle_by_key(family.key()).is_ok());
    assert_eq!(state.lock().unwrap().opened, 0);
}

#[tokio::test]
async fn successor_direct_and_borrowed_queries_retain_the_database_origin() {
    let (database, state) = successor_test_db("successor-origin", SuccessorAnswer::Record);
    let mut direct = database.query().unwrap();
    assert!(direct.uses_successor_projected_materialization());
    let record = direct.exact::<OriginRecord>().unwrap();
    let materialized = direct.query(record).unwrap().one().await.unwrap();
    assert_eq!(materialized.iid, "0x01");
    assert!(materialized.origin.projected().is_some());
    assert_eq!(state.opened.load(Ordering::SeqCst), 1);
    assert_eq!(state.closed.load(Ordering::SeqCst), 1);

    let read = database.read().await.unwrap();
    let mut borrowed = read.query();
    let record = borrowed.exact::<OriginRecord>().unwrap();
    let materialized = borrowed.query(record).unwrap().one().await.unwrap();
    assert_eq!(materialized.iid, "0x01");
    assert!(materialized.origin.projected().is_some());
    assert_eq!(state.opened.load(Ordering::SeqCst), 2);
    assert_eq!(state.closed.load(Ordering::SeqCst), 1);
    read.close().await.unwrap();
    assert_eq!(state.closed.load(Ordering::SeqCst), 2);

    let (legacy, legacy_state) = legacy_origin_test_db();
    let mut legacy_session = legacy.query().unwrap();
    assert!(!legacy_session.uses_successor_projected_materialization());
    assert_eq!(
        legacy_session.installed.projection().generator_handlers(),
        [type_bridge_contract::projection::ProjectionHandler::rust_v1()]
    );
    let record = legacy_session.exact::<OriginRecord>().unwrap();
    let materialized = legacy_session.query(record).unwrap().one().await.unwrap();
    assert!(
        materialized.origin.projected().is_none(),
        "the exact Rust-v1 handler retains its released unbound hydration behavior"
    );
    assert_eq!(legacy_state.opened.load(Ordering::SeqCst), 1);
    assert_eq!(legacy_state.closed.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn successor_authenticated_remote_query_is_explicitly_unbound() {
    use crate::remote::{RemoteConnectionOptions, RemoteDatabase, RemoteQueryLimits};
    use type_bridge_contract::id::{RoleId, TypeId, TypeKind};
    use type_bridge_orm::{ProjectedCreate, ProjectedCrudExecutor, ProjectedReference};

    let (transport, exchanges) = remote_origin_transport();
    let remote = RemoteDatabase::connect(RemoteConnectionOptions::new(
        "query-origin-test",
        "typedb-3.12.1/v1",
        RemoteQueryLimits::new(8, 1 << 20, 16, 16, 64, 32),
        transport,
    ))
    .await
    .unwrap()
    .with_schema(successor_package())
    .unwrap();
    let mut session = remote.query().unwrap();
    let record = session.exact::<OriginRecord>().unwrap();
    let record = session.query(record).unwrap().one().await.unwrap();
    assert_eq!(record.iid, "0x01");
    assert!(record.origin.projected().is_none());
    assert_eq!(exchanges.load(Ordering::SeqCst), 1);

    let (first, first_state) = successor_test_db("remote-target-a", SuccessorAnswer::Record);
    let (second, second_state) = successor_test_db("remote-target-b", SuccessorAnswer::Record);
    let installed = first.installed_schema().unwrap();
    let reference = ProjectedReference::try_new_with_origin_carrier(
        installed,
        TypeId::new(TypeKind::Entity, "record").unwrap(),
        Some(record.iid),
        vec![],
        record.origin.projected(),
    )
    .unwrap();
    let create = ProjectedCreate::try_new(
        installed,
        TypeId::new(TypeKind::Relation, "route").unwrap(),
        vec![],
        vec![
            (
                RoleId::new("route", "origin").unwrap(),
                vec![reference.clone()],
            ),
            (
                RoleId::new("route", "destination").unwrap(),
                vec![reference],
            ),
        ],
    )
    .unwrap();
    let executor = ProjectedCrudExecutor::new(installed);
    executor
        .preflight_relation_create_for_database_with_compatibility(first.inner_orm(), &create)
        .expect("remote-unbound references are admissible for one target database");
    executor
        .preflight_relation_create_for_database_with_compatibility(second.inner_orm(), &create)
        .expect("remote-unbound references are not falsely fenced to the first target");
    assert_eq!(first_state.opened.load(Ordering::SeqCst), 0);
    assert_eq!(second_state.opened.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn successor_relation_query_retains_outer_and_nested_player_origins() {
    let (database, state) = successor_test_db("successor-relation", SuccessorAnswer::Route);
    let mut session = database.query().unwrap();
    let route = session.exact::<OriginRoute>().unwrap();
    let materialized = session.query(route).unwrap().one().await.unwrap();
    assert_eq!(materialized.iid, "0x10");
    assert!(materialized.origin.projected().is_some());
    assert_eq!(materialized.player_origins.len(), 2);
    assert!(
        materialized
            .player_origins
            .iter()
            .all(|origin| origin.projected().is_some())
    );
    assert_eq!(state.opened.load(Ordering::SeqCst), 1);
    assert_eq!(state.closed.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn successor_tuple_and_named_query_shapes_retain_each_origin() {
    use crate::__codegen::FieldToken;

    #[derive(crate::SelectedRow)]
    struct OriginPair {
        left: OriginRecord,
        right: OriginRecord,
    }

    const NAME: FieldToken<OriginRecord, String> = FieldToken::new(NAME_OWNS, "{}");
    let (database, state) = successor_test_db("successor-pair", SuccessorAnswer::RecordPair);

    let mut tuple_session = database.query().unwrap();
    let left = tuple_session.exact::<OriginRecord>().unwrap();
    let right = tuple_session.exact::<OriginRecord>().unwrap();
    let (left_value, right_value) = tuple_session
        .query((left, right))
        .unwrap()
        .where_(left.field(NAME).eq_field(right.field(NAME)))
        .unwrap()
        .one()
        .await
        .unwrap();
    assert!(left_value.origin.projected().is_some());
    assert!(right_value.origin.projected().is_some());

    let mut named_session = database.query().unwrap();
    let left = named_session.exact::<OriginRecord>().unwrap();
    let right = named_session.exact::<OriginRecord>().unwrap();
    let OriginPair {
        left: left_value,
        right: right_value,
    } = named_session
        .query(OriginPair::select(left, right).unwrap())
        .unwrap()
        .where_(left.field(NAME).eq_field(right.field(NAME)))
        .unwrap()
        .one()
        .await
        .unwrap();
    assert!(left_value.origin.projected().is_some());
    assert!(right_value.origin.projected().is_some());
    assert_eq!(state.opened.load(Ordering::SeqCst), 2);
    assert_eq!(state.closed.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn successor_named_collected_page_retains_root_and_member_origins() {
    use crate::__codegen::FieldToken;
    use crate::query::PageOptions;

    #[derive(crate::SelectedRow)]
    struct OriginGraph {
        root: OriginRecord,
        members: Vec<OriginRecord>,
    }

    const NAME: FieldToken<OriginRecord, String> = FieldToken::new(NAME_OWNS, "{}");
    let (database, state) = successor_test_db("successor-page", SuccessorAnswer::RecordGraphPage);
    let mut session = database.query().unwrap();
    let root = session.exact::<OriginRecord>().unwrap();
    let member = session.exact::<OriginRecord>().unwrap();
    let members = member
        .collect()
        .distinct()
        .order_by(member.field(NAME).asc())
        .unwrap();
    let query = session
        .query(OriginGraph::select(root, members).unwrap())
        .unwrap()
        .allow_cross_join(root, member)
        .unwrap();
    let page = query
        .page_by(root, PageOptions::new(2).order_by(root.field(NAME).asc()))
        .await
        .unwrap();
    assert_eq!(page.offset(), 0);
    assert_eq!(page.limit(), 2);
    assert_eq!(page.total(), None);
    let [OriginGraph { root, members }] = page.items() else {
        panic!("expected one projected page root")
    };
    assert!(root.origin.projected().is_some());
    assert_eq!(members.len(), 2);
    assert!(
        members
            .iter()
            .all(|member| member.origin.projected().is_some())
    );
    assert_eq!(state.opened.load(Ordering::SeqCst), 1);
    assert_eq!(state.closed.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn successor_query_origins_fence_entity_and_relation_players_before_foreign_io() {
    use type_bridge_contract::id::{RoleId, TypeId, TypeKind};
    use type_bridge_orm::{ProjectedCreate, ProjectedCrudExecutor, ProjectedReference};

    let (entity_source, _) = successor_test_db("entity-source", SuccessorAnswer::Record);
    let mut entity_session = entity_source.query().unwrap();
    let record = entity_session.exact::<OriginRecord>().unwrap();
    let record = entity_session.query(record).unwrap().one().await.unwrap();
    let installed = entity_source.installed_schema().unwrap();
    let record_reference = ProjectedReference::try_new_with_origin_carrier(
        installed,
        TypeId::new(TypeKind::Entity, "record").unwrap(),
        Some(record.iid),
        vec![],
        record.origin.projected(),
    )
    .unwrap();
    let route_create = ProjectedCreate::try_new(
        installed,
        TypeId::new(TypeKind::Relation, "route").unwrap(),
        vec![],
        vec![
            (
                RoleId::new("route", "origin").unwrap(),
                vec![record_reference.clone()],
            ),
            (
                RoleId::new("route", "destination").unwrap(),
                vec![record_reference],
            ),
        ],
    )
    .unwrap();
    ProjectedCrudExecutor::new(installed)
        .preflight_relation_create_for_database_with_compatibility(
            entity_source.inner_orm(),
            &route_create,
        )
        .expect("same-database generated entity references remain admissible");
    let (entity_foreign, entity_foreign_state) =
        successor_test_db("entity-foreign", SuccessorAnswer::Record);
    let failure = ProjectedCrudExecutor::new(installed)
        .preflight_relation_create_for_database_with_compatibility(
            entity_foreign.inner_orm(),
            &route_create,
        )
        .expect_err("foreign generated entity references must be fenced");
    assert_eq!(
        failure.diagnostic().code().as_str(),
        "reference_database_mismatch"
    );
    assert_eq!(entity_foreign_state.opened.load(Ordering::SeqCst), 0);

    let (relation_source, _) = successor_test_db("relation-source", SuccessorAnswer::Route);
    let mut relation_session = relation_source.query().unwrap();
    let route = relation_session.exact::<OriginRoute>().unwrap();
    let route = relation_session.query(route).unwrap().one().await.unwrap();
    let installed = relation_source.installed_schema().unwrap();
    let route_reference = ProjectedReference::try_new_with_origin_carrier(
        installed,
        TypeId::new(TypeKind::Relation, "route").unwrap(),
        Some(route.iid),
        vec![],
        route.origin.projected(),
    )
    .unwrap();
    let container_create = ProjectedCreate::try_new(
        installed,
        TypeId::new(TypeKind::Relation, "container").unwrap(),
        vec![],
        vec![(
            RoleId::new("container", "item").unwrap(),
            vec![route_reference],
        )],
    )
    .unwrap();
    ProjectedCrudExecutor::new(installed)
        .preflight_relation_create_for_database_with_compatibility(
            relation_source.inner_orm(),
            &container_create,
        )
        .expect("same-database generated relation-as-player references remain admissible");
    let (relation_foreign, relation_foreign_state) =
        successor_test_db("relation-foreign", SuccessorAnswer::Route);
    let failure = ProjectedCrudExecutor::new(installed)
        .preflight_relation_create_for_database_with_compatibility(
            relation_foreign.inner_orm(),
            &container_create,
        )
        .expect_err("foreign generated relation-as-player references must be fenced");
    assert_eq!(
        failure.diagnostic().code().as_str(),
        "reference_database_mismatch"
    );
    assert_eq!(relation_foreign_state.opened.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn successor_grouped_thing_aggregate_retains_the_group_origin() {
    use crate::aggregate;

    let (database, state) = successor_test_db("successor-grouped", SuccessorAnswer::RecordGrouped);
    let mut session = database.query().unwrap();
    let root = session.exact::<OriginRecord>().unwrap();
    let group = session.exact::<OriginRecord>().unwrap();
    let query = session
        .query(root)
        .unwrap()
        .allow_cross_join(root, group)
        .unwrap();
    let rows = query
        .group_by(group)
        .unwrap()
        .aggregate((aggregate::count(),))
        .await
        .unwrap();
    let [(group, (count,))] = rows.as_slice() else {
        panic!("expected one projected thing group")
    };
    assert_eq!(group.iid, "0x02");
    assert!(group.origin.projected().is_some());
    assert_eq!(*count, 1);
    assert_eq!(state.opened.load(Ordering::SeqCst), 1);
    assert_eq!(state.closed.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn successor_cancellation_during_generated_materialization_publishes_no_partial_rows() {
    use crate::__codegen::FieldToken;
    use crate::query::RowsOptions;

    const NAME: FieldToken<SuccessorSlowRecord, String> = FieldToken::new(NAME_OWNS, "{}");
    SUCCESSOR_SLOW_MATERIALIZATION_CALLS.store(0, Ordering::SeqCst);
    let (database, state) = successor_test_db("successor-cancel", SuccessorAnswer::RecordMany);
    let cancellation = type_bridge_orm::AnswerCancellation::default();
    let mut session = database
        .query_with_resources(
            type_bridge_orm::QueryExecutionResourceLimits::default(),
            cancellation.clone(),
        )
        .unwrap();
    let record = session.exact::<SuccessorSlowRecord>().unwrap();
    let query = session.query(record).unwrap();
    let canceller = std::thread::spawn(move || {
        while SUCCESSOR_SLOW_MATERIALIZATION_CALLS.load(Ordering::SeqCst) == 0 {
            std::thread::yield_now();
        }
        cancellation.cancel();
    });
    let result = query
        .rows(RowsOptions::new(2).order_by(record.field(NAME).asc()))
        .await;
    canceller.join().unwrap();
    let error = result.expect_err("cancellation must discard the partially built output vector");
    assert_eq!(
        SUCCESSOR_SLOW_MATERIALIZATION_CALLS.load(Ordering::SeqCst),
        1
    );
    assert_eq!(error.category(), crate::ErrorCategory::Cancelled);
    assert_eq!(error.code(), Some("provider_cancelled"));
    assert_eq!(state.opened.load(Ordering::SeqCst), 1);
    assert_eq!(state.closed.load(Ordering::SeqCst), 1);
}

#[test]
fn query_resources_close_idempotently_with_independent_persistent_lineages() {
    let (db, state) = test_db();
    let mut session = db.query().unwrap();
    let record = session.exact::<Record>().unwrap();
    let input = session.query(record).unwrap();
    let cloned = input.clone();
    let derived = input.where_(record.iid("0x01")).unwrap();
    let sibling = session.query(record).unwrap();

    derived.close();
    derived.close();
    assert!(derived.is_closed());
    let derived_error = match derived.where_(record.iid("0x02")) {
        Err(error) => error,
        Ok(_) => panic!("closed derived query must reject composition"),
    };
    assert_eq!(derived_error.code(), Some("query_resource_closed"));
    assert!(input.lineage().is_ok());
    assert!(cloned.lineage().is_ok());
    assert!(sibling.lineage().is_ok());

    input.close();
    input.close();
    assert!(input.is_closed());
    assert_eq!(
        input.lineage().unwrap_err().code(),
        Some("query_resource_closed")
    );
    assert!(cloned.lineage().is_ok());
    assert!(sibling.lineage().is_ok());

    session.close();
    session.close();
    assert!(session.is_closed());
    assert_eq!(
        sibling.lineage().unwrap_err().code(),
        Some("query_resource_closed")
    );
    assert_eq!(
        session.exact::<Record>().unwrap_err().code(),
        Some("query_resource_closed")
    );
    assert_eq!(state.lock().unwrap().opened, 0);
}

#[test]
fn query_session_rejects_cross_session_and_unprojected_handles() {
    let (db, state) = test_db();
    let mut session_a = db.query().unwrap();
    let session_b = db.query().unwrap();
    let binding_a = session_a.exact::<Record>().unwrap();
    assert_model_error(
        session_b
            .handle_by_key(binding_a.key())
            .map(|_| ())
            .unwrap_err(),
        "cross_session_handle",
    );
    let ghost = session_a.exact::<Ghost>().unwrap_err();
    assert!(!matches!(ghost, crate::Error::ModelValidation { .. }));
    assert_eq!(ghost.code(), Some("unknown_descriptor"));
    assert_eq!(state.lock().unwrap().opened, 0);
}

#[test]
fn bounded_reachability_is_generated_typed_and_pre_io() {
    use crate::__codegen::{RoleToken, TypeToken};

    const ROUTE_TOKEN: TypeToken<Route> = TypeToken::new(ROUTE_JSON, "{}");
    const FROM: RoleToken<Route, RouteFromPlayers> = RoleToken::new(ROUTE_FROM, "{}");
    const TO: RoleToken<Route, RouteToPlayers> = RoleToken::new(ROUTE_TO, "{}");

    let (db, state) = test_db();
    let mut session = db.query().unwrap();
    let source = session.exact::<Record>().unwrap();
    let target = session.exact::<Record>().unwrap();
    let reachable = session
        .reachable(ROUTE_TOKEN, FROM, TO, source, target, 1, 2)
        .unwrap();
    assert!(matches!(
        reachable.expr,
        super::PredicateExpr::Reachable {
            source: super::BindingKey { index: 0, .. },
            target: super::BindingKey { index: 1, .. },
            min_depth: 1,
            max_depth: 2,
            ..
        }
    ));
    assert!(
        session
            .reachable(ROUTE_TOKEN, FROM, TO, source, target, 3, 2)
            .unwrap_err()
            .to_string()
            .contains("reachable_bounds")
    );

    let other_session = db.query().unwrap();
    assert_model_error(
        other_session
            .reachable(ROUTE_TOKEN, FROM, TO, source, target, 1, 2)
            .unwrap_err(),
        "cross_session_handle",
    );
    assert_eq!(state.lock().unwrap().opened, 0);
}

#[test]
fn selected_rows_resolve_kind_qualified_descriptor_ids_to_provider_labels() {
    use type_bridge_orm::match_request::result::HydratedThing;

    let (db, _state) = test_db();
    let session = db.query().unwrap();
    let thing: HydratedThing = serde_json::from_value(serde_json::json!({
        "concept_id": "0x01",
        "declared_descriptor": "entity:record",
        "concrete_descriptor": "entity:record",
        "kind": "entity",
        "attributes": [
            {
                "field": {"owner": "entity:record", "name": "name"},
                "values": [{"String": "Alice"}]
            },
            {
                "field": {"owner": "entity:record", "name": "tally"},
                "values": [{"Long": 1}]
            }
        ],
        "roles": []
    }))
    .unwrap();

    let row = session.client_row_for(&thing).unwrap();
    assert_eq!(row.type_id_json(), RECORD_JSON);
    assert_eq!(row.iid(), "0x01");
}

#[test]
fn query_literals_validate_at_construction() {
    use crate::value::{Date, DateTime, DateTimeTz, Decimal, Double, Duration, Regex, Text};
    assert_eq!(Text::new("Al").unwrap().as_str(), "Al");
    assert!(Text::new("x".repeat(2 * 1024 * 1024)).is_err());
    assert!(Regex::new(r"^A[[:alpha:]]+$").is_ok());
    assert!(Regex::new("(unclosed").is_err());
    assert_eq!(Double::new(90.0).unwrap().get(), 90.0);
    assert!(Double::new(f64::NAN).is_err());
    assert!(Double::new(f64::INFINITY).is_err());
    let negative_zero = Double::new(-0.0).unwrap();
    assert_eq!(negative_zero.get().to_bits(), (-0.0f64).to_bits());
    assert_eq!(Decimal::new("123.45").unwrap().as_str(), "123.45");
    assert!(Decimal::new("not-a-decimal").is_err());
    assert_eq!(Date::new("2026-07-29").unwrap().as_str(), "2026-07-29");
    assert!(Date::new("2026-13-99").is_err());
    assert!(DateTime::new("2026-07-29T03:55:00").is_ok());
    assert!(DateTime::new("yesterday").is_err());
    assert!(DateTimeTz::new("2026-07-29T03:55:00Z").is_ok());
    assert!(DateTimeTz::new("2026-07-29T03:55:00").is_err());
    assert!(Duration::new("P1D").is_ok());
    assert!(Duration::new("one day").is_err());
}

#[test]
fn predicates_lower_operators_and_compose_by_domain() {
    use super::{BindingKey, PredicateExpr};
    use crate::__codegen::{EncodedScalar, FieldToken, IntoEncodedScalar, QueryValued, RoleToken};
    use crate::value::{Regex, Text};
    use type_bridge_orm::AttributeValue;
    use type_bridge_orm::match_request::model::ComparisonOp;

    struct Assignment;
    impl sealed::Sealed for Assignment {}
    impl Model for Assignment {
        type Schema = TestSchema;
        const TYPE_ID_JSON: &'static str = r#"{"kind":"relation","label":"assignment"}"#;
    }
    impl ThingModel for Assignment {
        fn thing_kind() -> __codegen::ThingKind {
            __codegen::ThingKind::Relation
        }
    }
    struct WorkerPlayers;
    impl crate::__codegen::RelationModel for Assignment {}
    impl crate::__codegen::RoleTokenCompatible<Assignment, WorkerPlayers> for Assignment {}
    impl crate::__codegen::RolePlayer<Record> for WorkerPlayers {}
    impl CompleteModel for Assignment {
        type Create = RecordCreate;
        fn iid(&self) -> &str {
            unreachable!()
        }
    }
    impl MaterializeModel for Assignment {
        fn materialize(_: &HydratedRow, _: &HydrationCapability) -> Result<Self, ValidationError> {
            unreachable!()
        }
    }

    const NAME: FieldToken<Record, String> = FieldToken::new(NAME_OWNS, "{}");
    const TALLY: FieldToken<Record, i64> = FieldToken::new(
        r#"{"attribute":"tally","owner":{"kind":"entity","label":"record"}}"#,
        "{}",
    );
    struct StoredName(String);
    impl IntoEncodedScalar for StoredName {
        fn into_encoded_scalar(&self) -> EncodedScalar {
            EncodedScalar::String(self.0.clone())
        }
    }
    impl QueryValued for StoredName {
        type Domain = String;
    }
    const WRAPPED_NAME: FieldToken<Record, StoredName> = FieldToken::new(NAME_OWNS, "{}");
    const WORKER: RoleToken<Assignment, WorkerPlayers> = RoleToken::new(
        r#"{"declaring_relation":"assignment","label":"worker"}"#,
        "{}",
    );

    let (db, _state) = test_db();
    let mut session = db.query().unwrap();
    let record = session.exact::<Record>().unwrap();
    let other = session.exact::<Record>().unwrap();

    let name = record.field(NAME);
    let eq = name.eq(Text::new("Alice").unwrap());
    assert_eq!(
        eq.expr,
        PredicateExpr::FieldValue {
            binding: BindingKey {
                nonce: eq_nonce(&eq),
                index: 0
            },
            owns_id_json: NAME_OWNS,
            operator: ComparisonOp::Equal,
            value: AttributeValue::String("Alice".into()),
        }
    );
    let wrapped_eq = record.field(WRAPPED_NAME).eq(StoredName("Bob".into()));
    assert!(matches!(
        wrapped_eq.expr,
        PredicateExpr::FieldValue {
            operator: ComparisonOp::Equal,
            value: AttributeValue::String(ref value),
            ..
        } if value == "Bob"
    ));
    let tally = record.field(TALLY);
    let ordered = tally.ge(2_i64);
    assert!(matches!(
        ordered.expr,
        PredicateExpr::FieldValue {
            operator: ComparisonOp::GreaterThanOrEqual,
            value: AttributeValue::Long(2),
            ..
        }
    ));
    let prefix = name.starts_with(Text::new("Al").unwrap());
    assert!(matches!(
        prefix.expr,
        PredicateExpr::FieldValue {
            operator: ComparisonOp::StartsWith,
            value: AttributeValue::String(_),
            ..
        }
    ));
    let pattern = name.regex(Regex::new("^A").unwrap());
    assert!(matches!(
        pattern.expr,
        PredicateExpr::FieldValue {
            operator: ComparisonOp::Regex,
            ..
        }
    ));
    let cross = name.eq_field(other.field(NAME));
    assert!(matches!(
        cross.expr,
        PredicateExpr::FieldField {
            operator: ComparisonOp::Equal,
            left_binding: BindingKey { index: 0, .. },
            right_binding: BindingKey { index: 1, .. },
            ..
        }
    ));

    let composed = (name.eq(Text::new("a").unwrap()) & name.ne(Text::new("b").unwrap()))
        | !name.contains(Text::new("c").unwrap());
    let PredicateExpr::Or(terms) = composed.expr else {
        panic!("expected disjunction")
    };
    assert_eq!(terms.len(), 2);
    assert!(matches!(&terms[0], PredicateExpr::And(inner) if inner.len() == 2));
    assert!(matches!(&terms[1], PredicateExpr::Not(_)));

    let mut relation_session = db.query().unwrap();
    let assignment = relation_session.exact::<Assignment>();
    let assignment = match assignment {
        Ok(binding) => binding,
        Err(error) => {
            assert_eq!(error.code(), Some("unknown_descriptor"));
            return;
        }
    };
    let connects = assignment.role(WORKER).connects(record);
    assert!(matches!(connects.expr, PredicateExpr::Connects { .. }));
}

fn eq_nonce(predicate: &crate::query::Predicate<TestSchema>) -> u64 {
    match &predicate.expr {
        super::PredicateExpr::FieldValue { binding, .. } => binding.nonce,
        _ => panic!("expected field-value predicate"),
    }
}

#[tokio::test]
async fn query_facade_builds_validated_requests_and_replays_recorded_results() {
    use crate::__codegen::FieldToken;
    use crate::value::Text;
    use type_bridge_orm::match_request::model::Window;
    use type_bridge_orm::match_request::recording::{
        RecordingMatchExecutor, RecordingMatchResponse,
    };
    use type_bridge_orm::match_request::result::MatchResult;

    const NAME: FieldToken<Record, String> = FieldToken::new(NAME_OWNS, "{}");
    let (db, _state) = test_db();
    let mut session = db.query().unwrap();
    let record = session.exact::<Record>().unwrap();
    let name = record.field(NAME);
    let query = session
        .query(record)
        .unwrap()
        .where_(name.starts_with(Text::new("Al").unwrap()))
        .unwrap();

    let registry = std::sync::Arc::clone(db.match_registry().unwrap());
    let mut executor = RecordingMatchExecutor::new(std::sync::Arc::clone(&registry));

    let rows_request = query
        .validated_rows(
            &[name.asc()],
            Window {
                offset: 0,
                limit: 10,
            },
        )
        .unwrap();
    executor.push(RecordingMatchResponse::EmptyRows);
    let result = executor.execute(&rows_request).unwrap();
    let deadline = query.session.begin_invocation().unwrap();
    let outputs = query
        .outputs_from_rows(&rows_request, &result, deadline)
        .unwrap();
    assert!(outputs.is_empty());

    let count_request = query.validated_count_by(record).unwrap();
    executor.push(RecordingMatchResponse::Count(7));
    let count_result = executor.execute(&count_request).unwrap();
    let MatchResult::Count { value, .. } = count_result.for_request(&count_request).unwrap() else {
        panic!("expected a count result")
    };
    assert_eq!(*value, 7);

    let exists_request = query.validated_exists_by(record).unwrap();
    executor.push(RecordingMatchResponse::Exists(true));
    let exists_result = executor.execute(&exists_request).unwrap();
    let MatchResult::Exists { value, .. } = exists_result.for_request(&exists_request).unwrap()
    else {
        panic!("expected an existence result")
    };
    assert!(*value);
    assert_eq!(executor.calls(), 3);
}

#[test]
fn singular_keyless_relation_uses_exactly_one_without_a_stable_order_key() {
    use type_bridge_orm::match_request::model::{MatchOperation, RowCardinality, Window};

    let (db, _state) = test_db();
    let mut session = db.query().unwrap();
    let route = session.exact::<Route>().unwrap();
    let query = session.query(route).unwrap();

    let singular = query.validated_one().unwrap();
    let MatchOperation::FetchRows {
        cardinality,
        window,
        ..
    } = singular.request().operation
    else {
        panic!("expected row fetch")
    };
    assert_eq!(cardinality, RowCardinality::ExactlyOne);
    assert_eq!(
        window,
        Window {
            offset: 0,
            limit: 1
        }
    );

    let bounded = query
        .validated_rows(
            &[],
            Window {
                offset: 0,
                limit: 2,
            },
        )
        .unwrap_err();
    assert_eq!(bounded.code(), Some("missing_stable_unique_key"));
}

#[test]
fn singular_tuple_and_derived_named_shapes_validate_and_materialize_in_order() {
    use crate::__codegen::FieldToken;
    use crate::query::SelectedShape;
    use type_bridge_orm::match_request::model::{FetchShape, MatchOperation, Window};
    use type_bridge_orm::match_request::result::MatchRow;

    #[derive(crate::SelectedRow)]
    struct Pair {
        left: Record,
        right: Record,
    }

    const NAME: FieldToken<Record, String> = FieldToken::new(NAME_OWNS, "{}");
    let (db, state) = test_db();
    let mut session = db.query().unwrap();
    let left = session.exact::<Record>().unwrap();
    let right = session.exact::<Record>().unwrap();
    let connected = left.field(NAME).eq_field(right.field(NAME));

    let tuple_query = session
        .query((left, right))
        .unwrap()
        .where_(connected.clone())
        .unwrap();
    let tuple_request = tuple_query
        .validated_rows(
            &[],
            Window {
                offset: 0,
                limit: 4,
            },
        )
        .unwrap();
    let MatchOperation::FetchRows {
        output: FetchShape::Positional { slots },
        ..
    } = &tuple_request.request().operation
    else {
        panic!("expected positional tuple output")
    };
    assert_eq!(slots.len(), 2);

    let count_left = tuple_query.validated_count_by(left).unwrap();
    let count_right = tuple_query.validated_count_by(right).unwrap();
    let MatchOperation::CountBy { root: left_root } = count_left.request().operation else {
        panic!("expected left-root count")
    };
    let MatchOperation::CountBy { root: right_root } = count_right.request().operation else {
        panic!("expected right-root count")
    };
    assert_ne!(left_root, right_root);
    tuple_query.validated_exists_by(right).unwrap();

    let named = Pair::select(left, right).unwrap();
    let named_query = session
        .query(named.clone())
        .unwrap()
        .where_(connected)
        .unwrap();
    let named_request = named_query
        .validated_rows(
            &[],
            Window {
                offset: 0,
                limit: 4,
            },
        )
        .unwrap();
    let MatchOperation::FetchRows {
        output: FetchShape::Named { slots },
        ..
    } = &named_request.request().operation
    else {
        panic!("expected named selected output")
    };
    assert_eq!(
        slots
            .iter()
            .map(|slot| slot.name.as_str())
            .collect::<Vec<_>>(),
        ["left", "right"]
    );

    let thing = serde_json::json!({
        "concept_id": "0x01",
        "declared_descriptor": "entity:record",
        "concrete_descriptor": "entity:record",
        "kind": "entity",
        "attributes": [
            {
                "field": {"owner": "entity:record", "name": "name"},
                "values": [{"String": "Alice"}]
            },
            {
                "field": {"owner": "entity:record", "name": "tally"},
                "values": [{"Long": 1}]
            }
        ],
        "roles": []
    });
    let row: MatchRow = serde_json::from_value(serde_json::json!({
        "slots": [
            {"kind": "one", "value": thing.clone()},
            {"kind": "one", "value": thing}
        ]
    }))
    .unwrap();
    let Pair {
        left: _left,
        right: _right,
    } = named.__materialize_row(&session, &row).unwrap();
    assert_eq!(state.lock().unwrap().opened, 0);
}

#[tokio::test]
async fn collected_named_shapes_validate_root_pages_and_owned_envelopes() {
    use crate::__codegen::FieldToken;
    use crate::query::{PageOptions, SelectedShape};
    use type_bridge_orm::match_request::model::{FetchShape, FetchSlot, MatchOperation, Window};
    use type_bridge_orm::match_request::recording::{
        RecordingMatchExecutor, RecordingMatchResponse,
    };
    use type_bridge_orm::match_request::result::MatchRow;

    #[derive(crate::SelectedRow)]
    struct Graph {
        root: Record,
        members: Vec<Record>,
    }

    const NAME: FieldToken<Record, String> = FieldToken::new(NAME_OWNS, "{}");
    let (db, state) = test_db();
    let mut session = db.query().unwrap();
    let root = session.exact::<Record>().unwrap();
    let member = session.exact::<Record>().unwrap();
    let connected = root.field(NAME).eq_field(member.field(NAME));

    let wrong_order = root
        .collect()
        .order_by(member.field(NAME).asc())
        .err()
        .unwrap();
    assert_model_error(wrong_order, "collection_order_binding_mismatch");

    let collected = member
        .collect()
        .distinct()
        .order_by(member.field(NAME).desc())
        .unwrap();
    let named = Graph::select(root, collected).unwrap();
    let query = session
        .query(named.clone())
        .unwrap()
        .where_(connected)
        .unwrap();
    let validated = query
        .validated_page(
            root,
            &[root.field(NAME).asc()],
            Window {
                offset: 3,
                limit: 5,
            },
            true,
        )
        .unwrap();
    let MatchOperation::PageBy {
        output: FetchShape::Named { slots },
        ..
    } = &validated.request().operation
    else {
        panic!("expected named page output")
    };
    assert_eq!(slots[0].name, "root");
    assert!(matches!(slots[0].slot, FetchSlot::One { .. }));
    assert_eq!(slots[1].name, "members");
    assert!(matches!(
        &slots[1].slot,
        FetchSlot::Collect {
            distinct: true,
            order,
            ..
        } if !order.is_empty()
    ));

    let collected_root_query = session.query(member.collect()).unwrap();
    let collected_root_error = collected_root_query
        .validated_page(
            member,
            &[],
            Window {
                offset: 0,
                limit: 1,
            },
            false,
        )
        .unwrap_err();
    assert!(
        collected_root_error
            .to_string()
            .contains("collected_page_root")
    );

    let singular_non_root = session
        .query((root, member))
        .unwrap()
        .where_(root.field(NAME).eq_field(member.field(NAME)))
        .unwrap();
    let singular_non_root_error = singular_non_root
        .validated_page(
            root,
            &[],
            Window {
                offset: 0,
                limit: 1,
            },
            false,
        )
        .unwrap_err();
    assert!(
        singular_non_root_error
            .to_string()
            .contains("singular_non_root_page_slot")
    );

    let registry = Arc::clone(db.match_registry().unwrap());
    let mut executor = RecordingMatchExecutor::new(registry);
    executor.push(RecordingMatchResponse::EmptyPage { total: Some(3) });
    let result = executor.execute(&validated).unwrap();
    let deadline = query.session.begin_invocation().unwrap();
    let page = query.output_page(&validated, &result, deadline).unwrap();
    assert!(page.items().is_empty());
    assert_eq!(page.offset(), 3);
    assert_eq!(page.limit(), 5);
    assert_eq!(page.total(), Some(3));
    assert!(page.into_items().is_empty());

    let thing = serde_json::json!({
        "concept_id": "0x01",
        "declared_descriptor": "entity:record",
        "concrete_descriptor": "entity:record",
        "kind": "entity",
        "attributes": [
            {
                "field": {"owner": "entity:record", "name": "name"},
                "values": [{"String": "Alice"}]
            },
            {
                "field": {"owner": "entity:record", "name": "tally"},
                "values": [{"Long": 1}]
            }
        ],
        "roles": []
    });
    let row: MatchRow = serde_json::from_value(serde_json::json!({
        "slots": [
            {"kind": "one", "value": thing.clone()},
            {"kind": "many", "value": [thing.clone(), thing]}
        ]
    }))
    .unwrap();
    let Graph {
        root: _root,
        members,
    } = named.__materialize_row(&session, &row).unwrap();
    assert_eq!(members.len(), 2);

    assert_model_error(
        query
            .page_by(root, PageOptions::new(0))
            .await
            .err()
            .unwrap(),
        "zero_limit",
    );
    assert_eq!(state.lock().unwrap().opened, 0);
}

#[tokio::test]
async fn query_facade_rejects_cross_owner_fields_and_zero_limits_before_io() {
    use crate::__codegen::FieldToken;
    use crate::query::RowsOptions;
    use crate::value::Text;
    use type_bridge_orm::match_request::model::Window;

    #[derive(Debug)]
    struct Other;
    impl sealed::Sealed for Other {}
    impl Model for Other {
        type Schema = TestSchema;
        const TYPE_ID_JSON: &'static str = r#"{"kind":"entity","label":"other"}"#;
    }
    impl ThingModel for Other {
        fn thing_kind() -> __codegen::ThingKind {
            __codegen::ThingKind::Entity
        }
    }
    impl EntityModel for Other {}
    // A forged compatibility impl (possible only outside the generator)
    // must still fail closed against the installed registry at lowering.
    impl __codegen::NominalUpcast<Other> for Record {}

    const OTHER_NAME: FieldToken<Other, String> = FieldToken::new(
        r#"{"attribute":"name","owner":{"kind":"entity","label":"other"}}"#,
        "{}",
    );
    let (db, state) = test_db();
    let mut session = db.query().unwrap();
    let record = session.exact::<Record>().unwrap();
    let cross_owner = record.field(OTHER_NAME).eq(Text::new("x").unwrap());
    let query = session.query(record).unwrap().where_(cross_owner).unwrap();
    let error = query
        .validated_rows(
            &[],
            Window {
                offset: 0,
                limit: 1,
            },
        )
        .unwrap_err();
    assert_eq!(error.category(), crate::ErrorCategory::QueryAuthoring);
    assert_eq!(error.code(), Some("cross_owner_field"));
    assert_eq!(error.path(), Some(&[][..]));
    assert_eq!(error.model_validation_phase(), None);

    let mut foreign_session = db.query().unwrap();
    let foreign = foreign_session.exact::<Record>().unwrap();
    const NAME: FieldToken<Record, String> = FieldToken::new(NAME_OWNS, "{}");
    let foreign_predicate = foreign.field(NAME).eq(Text::new("x").unwrap());
    let cross_query = session
        .query(record)
        .unwrap()
        .where_(foreign_predicate)
        .unwrap();
    assert_model_error(
        cross_query
            .validated_rows(
                &[],
                Window {
                    offset: 0,
                    limit: 1,
                },
            )
            .unwrap_err(),
        "cross_session_handle",
    );

    let plain = session.query(record).unwrap();
    assert_model_error(
        plain.rows(RowsOptions::new(0)).await.unwrap_err(),
        "zero_limit",
    );
    assert_eq!(state.lock().unwrap().opened, 0);
}

#[tokio::test]
async fn query_terminal_budget_rejects_zero_timeout_and_precancellation_before_provider_io() {
    let (db, state) = test_db();
    let zero_timeout = type_bridge_orm::QueryExecutionResourceLimits {
        timeout_milliseconds: 0,
        ..type_bridge_orm::QueryExecutionResourceLimits::default()
    };
    let mut timed_session = db
        .query_with_resources(zero_timeout, type_bridge_orm::AnswerCancellation::default())
        .unwrap();
    let timed_record = timed_session.exact::<Record>().unwrap();
    let timed_error = timed_session
        .query(timed_record)
        .unwrap()
        .count()
        .await
        .expect_err("zero timeout must fail before opening a transaction");
    assert_eq!(timed_error.category(), crate::ErrorCategory::ResourceLimit);
    assert_eq!(timed_error.code(), Some("transaction_deadline_exceeded"));
    assert_eq!(
        timed_error.diagnostic_path(),
        Some(
            &[crate::ErrorPathSegment::Query(
                crate::QueryDiagnosticPathKind::ProviderEvidence
            )][..]
        )
    );

    let cancellation = type_bridge_orm::AnswerCancellation::default();
    cancellation.cancel();
    let mut cancelled_session = db
        .query_with_resources(
            type_bridge_orm::QueryExecutionResourceLimits::default(),
            cancellation,
        )
        .unwrap();
    let cancelled_record = cancelled_session.exact::<Record>().unwrap();
    let cancelled_error = cancelled_session
        .query(cancelled_record)
        .unwrap()
        .count()
        .await
        .expect_err("pre-cancellation must fail before opening a transaction");
    assert_eq!(cancelled_error.category(), crate::ErrorCategory::Cancelled);
    assert_eq!(cancelled_error.code(), Some("provider_cancelled"));
    assert_eq!(state.lock().unwrap().opened, 0);
}

#[test]
fn rust_generated_materialization_observes_the_same_absolute_deadline() {
    use type_bridge_orm::match_request::MatchRow;

    SLOW_MATERIALIZATION_CALLS.store(0, Ordering::SeqCst);
    let (db, _state) = test_db();
    let mut session = db.query().unwrap();
    let record = session.exact::<SlowRecord>().unwrap();
    let query = session.query(record).unwrap();
    let thing = serde_json::json!({
        "concept_id": "0x01",
        "declared_descriptor": "entity:record",
        "concrete_descriptor": "entity:record",
        "kind": "entity",
        "attributes": [
            {
                "field": {"owner": "entity:record", "name": "name"},
                "values": [{"String": "Alice"}]
            },
            {
                "field": {"owner": "entity:record", "name": "tally"},
                "values": [{"Long": 1}]
            }
        ],
        "roles": []
    });
    let row: MatchRow = serde_json::from_value(serde_json::json!({
        "slots": [{"kind": "one", "value": thing}]
    }))
    .unwrap();
    let deadline = type_bridge_orm::QueryExecutionDeadline::from_timeout_milliseconds(5);

    let error = query
        .materialize_rows(&[row], deadline)
        .expect_err("expiry during generated-model construction must discard the partial output");
    assert_eq!(SLOW_MATERIALIZATION_CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(error.category(), crate::ErrorCategory::ResourceLimit);
    assert_eq!(error.code(), Some("transaction_deadline_exceeded"));
}

#[tokio::test]
#[allow(clippy::type_complexity)]
async fn query_aggregates_build_reduce_requests_and_decode_typed_tuples() {
    use crate::__codegen::FieldToken;
    use crate::aggregate::{self, Agg};
    use type_bridge_orm::match_request::recording::{
        RecordingMatchExecutor, RecordingMatchResponse,
    };
    use type_bridge_orm::match_request::{ReducedValue, Reduction};

    const TALLY: FieldToken<Record, i64> = FieldToken::new(
        r#"{"attribute":"tally","owner":{"kind":"entity","label":"record"}}"#,
        "{}",
    );
    let (db, _state) = test_db();
    let mut session = db.query().unwrap();
    let record = session.exact::<Record>().unwrap();
    let tally = record.field(TALLY);
    let query = session.query(record).unwrap();

    let terms: (
        Agg<TestSchema, u64>,
        Agg<TestSchema, Option<f64>>,
        Agg<TestSchema, i64>,
    ) = (aggregate::count(), tally.mean(), tally.sum());
    let term_list = crate::aggregate::AggregateTuple::terms(&terms);
    assert_eq!(term_list.len(), 3);
    assert_eq!(term_list[0].0, Reduction::Count);
    assert!(term_list[0].1.is_none());
    assert_eq!(term_list[1].0, Reduction::Mean);
    assert!(term_list[1].1.is_some());
    assert_eq!(term_list[2].0, Reduction::Sum);

    let validated = query.validated_reduce(None, &term_list).unwrap();
    let registry = std::sync::Arc::clone(db.match_registry().unwrap());
    let mut executor = RecordingMatchExecutor::new(std::sync::Arc::clone(&registry));
    executor.push(RecordingMatchResponse::Reduction(vec![
        ReducedValue::Count(4),
        ReducedValue::Double(Some(10.25)),
        ReducedValue::Long(Some(41)),
    ]));
    let result = executor.execute(&validated).unwrap();
    let rows = match result.for_request(&validated).unwrap() {
        type_bridge_orm::match_request::MatchResult::Reduction { rows, .. } => rows.clone(),
        _ => panic!("expected a reduction result"),
    };
    let decoded = <(
        Agg<TestSchema, u64>,
        Agg<TestSchema, Option<f64>>,
        Agg<TestSchema, i64>,
    ) as crate::aggregate::AggregateTuple<TestSchema>>::decode(rows[0].values())
    .unwrap();
    assert_eq!(decoded, (4, Some(10.25), 41));

    // An absent total sum fails closed at decode; absent mean decodes None.
    executor.push(RecordingMatchResponse::Reduction(vec![
        ReducedValue::Count(0),
        ReducedValue::Double(None),
        ReducedValue::Long(None),
    ]));
    let empty_result = executor.execute(&validated).unwrap();
    let rows = match empty_result.for_request(&validated).unwrap() {
        type_bridge_orm::match_request::MatchResult::Reduction { rows, .. } => rows.clone(),
        _ => panic!("expected a reduction result"),
    };
    let error = <(
        Agg<TestSchema, u64>,
        Agg<TestSchema, Option<f64>>,
        Agg<TestSchema, i64>,
    ) as crate::aggregate::AggregateTuple<TestSchema>>::decode(rows[0].values())
    .unwrap_err();
    assert!(matches!(error, crate::Error::ModelValidation { .. }));
    let partial = <(
        Agg<TestSchema, u64>,
        Agg<TestSchema, Option<f64>>,
    ) as crate::aggregate::AggregateTuple<TestSchema>>::decode(&rows[0].values()[..2])
    .unwrap();
    assert_eq!(partial, (0, None));

    // Grouped aggregates validate with a distinct group binding.
    let mut grouped_session = db.query().unwrap();
    let grouped_record = grouped_session.exact::<Record>().unwrap();
    let group_binding = grouped_session.exact::<Record>().unwrap();
    let grouped_query = grouped_session
        .query(grouped_record)
        .unwrap()
        .where_(
            grouped_record
                .field(TALLY)
                .eq_field(group_binding.field(TALLY)),
        )
        .unwrap();
    let grouped = grouped_query.group_by(group_binding).unwrap();
    let _ = grouped;
    let grouped_terms: (Agg<TestSchema, u64>,) = (aggregate::count(),);
    let grouped_validated = grouped_query
        .validated_reduce(
            Some(group_binding.key()),
            &crate::aggregate::AggregateTuple::terms(&grouped_terms),
        )
        .unwrap();
    executor.push(RecordingMatchResponse::EmptyGroupedReduction);
    let grouped_result = executor.execute(&grouped_validated).unwrap();
    match grouped_result.for_request(&grouped_validated).unwrap() {
        type_bridge_orm::match_request::MatchResult::Reduction { group, rows, .. } => {
            assert!(group.is_some());
            assert!(rows.is_empty());
        }
        _ => panic!("expected a grouped reduction result"),
    }
}
