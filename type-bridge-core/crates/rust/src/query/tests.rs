use std::sync::{Arc, Mutex};

use crate::__codegen::{
    self, CompleteModel, EncodedCreate, EntityModel, HydratedRow, HydrationCapability,
    IntoEncodedCreate, MaterializeModel, Model, SubtypeRootModel, ThingModel, ValidationError,
};
use crate::schema::{Schema, sealed};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig};
use type_bridge_contract::schema::DocumentId;
use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, TransactionOps, TxType};
use type_bridge_orm::{Database as OrmDatabase, OrmError};
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
    assert!(ghost.to_string().contains("ghost"));
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
            assert!(error.to_string().contains("assignment"));
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
    let outputs = query.outputs_from_rows(&rows_request, &result).unwrap();
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
    let page = query.output_page(&validated, &result).unwrap();
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
