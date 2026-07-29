use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig};
use type_bridge_contract::schema::DocumentId;
use type_bridge_orm::{AttributeValue, DynamicEntityRow, InstalledRuntimeProjection};
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
use type_bridge_schema_codegen::RustEmitter;

use super::*;
use crate::__codegen::{self, EncodedReference};

const RECORD_ID: &str = r#"{"kind":"entity","label":"record"}"#;
const BASE_ID: &str = r#"{"kind":"entity","label":"base"}"#;
const OTHER_ID: &str = r#"{"kind":"entity","label":"other"}"#;
const LINK_ID: &str = r#"{"kind":"relation","label":"link"}"#;
const GHOST_ID: &str = r#"{"kind":"entity","label":"ghost"}"#;

const INHERITED: &str = r#"{"attribute":"inherited","owner":{"kind":"entity","label":"base"}}"#;
const ACTIVE: &str = r#"{"attribute":"active","owner":{"kind":"entity","label":"record"}}"#;
const AMOUNT: &str = r#"{"attribute":"amount","owner":{"kind":"entity","label":"record"}}"#;
const BOUNDED: &str = r#"{"attribute":"bounded","owner":{"kind":"entity","label":"record"}}"#;
const COUNT: &str = r#"{"attribute":"count","owner":{"kind":"entity","label":"record"}}"#;
const DAY: &str = r#"{"attribute":"day","owner":{"kind":"entity","label":"record"}}"#;
const MOMENT: &str = r#"{"attribute":"moment","owner":{"kind":"entity","label":"record"}}"#;
const RATIO: &str = r#"{"attribute":"ratio","owner":{"kind":"entity","label":"record"}}"#;
const SPAN: &str = r#"{"attribute":"span","owner":{"kind":"entity","label":"record"}}"#;
const TEXT: &str = r#"{"attribute":"text","owner":{"kind":"entity","label":"record"}}"#;
const UNBOUNDED: &str = r#"{"attribute":"unbounded","owner":{"kind":"entity","label":"record"}}"#;
const ZONED: &str = r#"{"attribute":"zoned","owner":{"kind":"entity","label":"record"}}"#;
const OPTIONAL_TEXT: &str =
    r#"{"attribute":"optional-text","owner":{"kind":"entity","label":"record"}}"#;
const OTHER_ONLY: &str = r#"{"attribute":"other-only","owner":{"kind":"entity","label":"other"}}"#;

#[test]
fn discovered_entity_labels_use_one_authority_path() {
    let installed = fixture();
    let (id, _descriptor) = resolve_discovered_entity("record", &installed).unwrap();
    assert_eq!(id.kind(), TypeKind::Entity);
    assert_eq!(id.label().as_str(), "record");
    assert!(resolve_discovered_entity("missing", &installed).is_err());
    assert!(resolve_discovered_entity("", &installed).is_err());
}

#[test]
fn generated_validation_paths_are_empty_aware() {
    assert!(split_path("").is_empty());
    assert_eq!(split_path("fields.0.value"), vec!["fields", "0", "value"]);
}

fn fixture() -> InstalledRuntimeProjection {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("entity-codec.yaml").unwrap(),
        r#"format: typebridge.schema/v2
attributes:
  active: { value: boolean }
  amount: { value: decimal }
  bounded: { value: decimal }
  count: { value: integer }
  day: { value: date }
  inherited: { value: string }
  moment: { value: datetime }
  optional-text: { value: string }
  other-only: { value: string }
  ratio: { value: double }
  span: { value: duration }
  text: { value: string }
  unbounded: { value: string }
  zoned: { value: datetime-tz }
entities:
  base:
    abstract: true
    owns:
      inherited: { card: 1 }
  record:
    sub: base
    owns:
      active: { card: 1 }
      amount: { card: 1 }
      bounded: { card: { min: 0, max: 3 } }
      count: { card: 1 }
      day: { card: 1 }
      moment: { card: 1 }
      optional-text: { card: { min: 0, max: 1 } }
      ratio: { card: 1 }
      span: { card: 1 }
      text: { card: 1 }
      unbounded: { card: { min: 0 } }
      zoned: { card: 1 }
  other:
    owns:
      other-only: { card: 1 }
relations:
  link: {}
"#,
    )])
    .unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    let emitter = RustEmitter::new();
    let projection = project(
        &resolved,
        BindingTarget::Rust,
        &ProjectionConfig::rust(),
        &emitter.generator_handlers(),
        &emitter.code_resources().unwrap(),
    )
    .unwrap();
    InstalledRuntimeProjection::try_new(projection).unwrap()
}

fn type_id(kind: TypeKind, label: &str) -> TypeId {
    TypeId::new(kind, label).unwrap()
}

fn positive_fields() -> Vec<(&'static str, Vec<EncodedScalar>)> {
    vec![
        (INHERITED, vec![EncodedScalar::String("parent".into())]),
        (ACTIVE, vec![EncodedScalar::Boolean(true)]),
        (
            AMOUNT,
            vec![EncodedScalar::Decimal(Decimal::try_new("123.45").unwrap())],
        ),
        (
            BOUNDED,
            vec![
                EncodedScalar::Decimal(Decimal::try_new("1.5").unwrap()),
                EncodedScalar::Decimal(Decimal::try_new("2.5").unwrap()),
            ],
        ),
        (COUNT, vec![EncodedScalar::Long(42)]),
        (
            DAY,
            vec![EncodedScalar::Date(Date::try_new("2026-07-28").unwrap())],
        ),
        (
            MOMENT,
            vec![EncodedScalar::DateTime(
                DateTime::try_new("2026-07-28T03:55:00").unwrap(),
            )],
        ),
        (
            RATIO,
            vec![EncodedScalar::Double(
                CanonicalDouble::try_new(1.5).unwrap(),
            )],
        ),
        (
            SPAN,
            vec![EncodedScalar::Duration(Duration::try_new("P1D").unwrap())],
        ),
        (TEXT, vec![EncodedScalar::String("child".into())]),
        (
            UNBOUNDED,
            vec![
                EncodedScalar::String("first".into()),
                EncodedScalar::String("second".into()),
            ],
        ),
        (
            ZONED,
            vec![EncodedScalar::DateTimeTz(
                DateTimeTz::try_new("2026-07-28T03:55:00Z").unwrap(),
            )],
        ),
    ]
}

fn positive_create() -> EncodedCreate {
    EncodedCreate::new(RECORD_ID, positive_fields(), vec![])
}

fn positive_attributes() -> Vec<(String, AttributeValue)> {
    lower_encoded_entity_create(
        &positive_create(),
        &type_id(TypeKind::Entity, "record"),
        &fixture(),
    )
    .unwrap()
}

fn error_code(error: Error) -> (ModelValidationPhase, String, Vec<String>) {
    (
        error.model_validation_phase().unwrap(),
        error.code().unwrap().to_owned(),
        error.path().unwrap().to_vec(),
    )
}

fn assert_input(error: Error, code: &str) {
    let (phase, actual, _) = error_code(error);
    assert_eq!(phase, ModelValidationPhase::Input);
    assert_eq!(actual, code);
}

fn assert_hydration(error: Error, code: &str) {
    let (phase, actual, _) = error_code(error);
    assert_eq!(phase, ModelValidationPhase::Hydration);
    assert_eq!(actual, code);
}

fn record_row(attributes: Vec<(String, AttributeValue)>) -> DynamicEntityRow {
    DynamicEntityRow {
        iid: Some("0xAbC123".into()),
        type_name: Some("record".into()),
        attributes,
    }
}

#[test]
fn all_domains_optional_absence_sequences_and_inheritance_roundtrip_losslessly() {
    let installed = fixture();
    let record = type_id(TypeKind::Entity, "record");
    let lowered = lower_encoded_entity_create(&positive_create(), &record, &installed).unwrap();

    assert!(!lowered.iter().any(|(name, _)| name == "optional-text"));
    assert_eq!(
        lowered
            .iter()
            .filter(|(name, _)| name == "bounded")
            .map(|(_, value)| value)
            .collect::<Vec<_>>(),
        vec![
            &AttributeValue::Decimal("1.5".into()),
            &AttributeValue::Decimal("2.5".into())
        ]
    );
    assert_eq!(
        lowered
            .iter()
            .filter(|(name, _)| name == "unbounded")
            .map(|(_, value)| value)
            .collect::<Vec<_>>(),
        vec![
            &AttributeValue::String("first".into()),
            &AttributeValue::String("second".into())
        ]
    );
    for (name, expected) in [
        ("inherited", vec![AttributeValue::String("parent".into())]),
        ("active", vec![AttributeValue::Boolean(true)]),
        ("amount", vec![AttributeValue::Decimal("123.45".into())]),
        ("count", vec![AttributeValue::Long(42)]),
        ("day", vec![AttributeValue::Date("2026-07-28".into())]),
        (
            "moment",
            vec![AttributeValue::DateTime("2026-07-28T03:55:00".into())],
        ),
        ("ratio", vec![AttributeValue::Double(1.5)]),
        ("span", vec![AttributeValue::Duration("P1D".into())]),
        ("text", vec![AttributeValue::String("child".into())]),
        (
            "zoned",
            vec![AttributeValue::DateTimeTZ("2026-07-28T03:55:00Z".into())],
        ),
    ] {
        assert_eq!(
            lowered
                .iter()
                .filter(|(actual, _)| actual == name)
                .map(|(_, value)| value.clone())
                .collect::<Vec<_>>(),
            expected,
        );
    }

    let expected_provider_order = installed
        .projection()
        .models()
        .get(&record)
        .unwrap()
        .create()
        .fields()
        .iter()
        .flat_map(|field| {
            let token = installed
                .projection()
                .models()
                .get(&record)
                .unwrap()
                .query_tokens()
                .fields()
                .get(field.token())
                .unwrap();
            lowered
                .iter()
                .filter(move |(name, _)| name == token.id().attribute().label().as_str())
                .map(|(name, _)| name.as_str())
        })
        .collect::<Vec<_>>();
    assert_eq!(
        lowered
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        expected_provider_order,
    );

    let mut optional_create = positive_fields();
    optional_create.push((OPTIONAL_TEXT, vec![EncodedScalar::String("present".into())]));
    let optional_lowered = lower_encoded_entity_create(
        &EncodedCreate::new(RECORD_ID, optional_create, vec![]),
        &record,
        &installed,
    )
    .unwrap();
    assert_eq!(
        optional_lowered
            .iter()
            .find(|(name, _)| name == "optional-text")
            .map(|(_, value)| value),
        Some(&AttributeValue::String("present".into())),
    );
    let mut optional_attributes = positive_attributes();
    optional_attributes.push((
        "optional-text".into(),
        AttributeValue::String("present".into()),
    ));
    let optional_hydrated =
        hydrate_entity(record_row(optional_attributes), &record, &installed).unwrap();
    assert!(
        optional_hydrated
            .fields()
            .iter()
            .any(|(identity, values)| identity == OPTIONAL_TEXT
                && values == &vec![EncodedScalar::String("present".into())])
    );

    let hydrated = hydrate_entity(record_row(lowered), &record, &installed).unwrap();
    assert_eq!(hydrated.type_id_json(), RECORD_ID);
    assert_eq!(hydrated.iid(), "0xAbC123");
    assert!(hydrated.roles().is_empty());
    assert!(hydrated.fields().iter().any(|(identity, values)| {
        identity == INHERITED && values == &vec![EncodedScalar::String("parent".into())]
    }));
    for (identity, expected) in positive_fields() {
        assert_eq!(
            hydrated
                .fields()
                .iter()
                .find(|(actual, _)| actual == identity)
                .map(|(_, values)| values),
            Some(&expected),
            "hydration must preserve every encoded value for {identity}",
        );
    }
    let expected_hydrated_order = installed
        .projection()
        .models()
        .get(&record)
        .unwrap()
        .complete_read()
        .fields()
        .iter()
        .filter_map(|field| {
            let token = installed
                .projection()
                .models()
                .get(&record)
                .unwrap()
                .query_tokens()
                .fields()
                .get(field.token())
                .unwrap();
            let identity = String::from_utf8(
                canonical_owns_identity(
                    token.declaring_id(),
                    ModelValidationPhase::Hydration,
                    vec![],
                )
                .unwrap(),
            )
            .unwrap();
            positive_fields()
                .iter()
                .any(|(expected, _)| *expected == identity)
                .then_some(identity)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        hydrated
            .fields()
            .iter()
            .map(|(identity, _)| identity.clone())
            .collect::<Vec<_>>(),
        expected_hydrated_order,
    );
    assert_eq!(
        hydrated
            .fields()
            .iter()
            .find(|(identity, _)| identity == BOUNDED)
            .unwrap()
            .1,
        vec![
            EncodedScalar::Decimal(Decimal::try_new("1.5").unwrap()),
            EncodedScalar::Decimal(Decimal::try_new("2.5").unwrap())
        ]
    );
    assert_eq!(
        hydrated
            .fields()
            .iter()
            .find(|(identity, _)| identity == UNBOUNDED)
            .unwrap()
            .1,
        vec![
            EncodedScalar::String("first".into()),
            EncodedScalar::String("second".into())
        ]
    );
}

#[test]
fn create_rejects_every_identity_shape_role_cardinality_and_domain_failure() {
    let installed = fixture();
    let record = type_id(TypeKind::Entity, "record");

    assert_input(
        lower_encoded_entity_create(
            &EncodedCreate::new("{}", positive_fields(), vec![]),
            &record,
            &installed,
        )
        .unwrap_err(),
        "invalid_model_identity",
    );
    assert_input(
        lower_encoded_entity_create(
            &EncodedCreate::new(OTHER_ID, positive_fields(), vec![]),
            &record,
            &installed,
        )
        .unwrap_err(),
        "wrong_model_identity",
    );
    assert_input(
        lower_encoded_entity_create(
            &EncodedCreate::new(LINK_ID, vec![], vec![]),
            &type_id(TypeKind::Relation, "link"),
            &installed,
        )
        .unwrap_err(),
        "wrong_model_kind",
    );
    assert_input(
        lower_encoded_entity_create(
            &EncodedCreate::new(GHOST_ID, vec![], vec![]),
            &type_id(TypeKind::Entity, "ghost"),
            &installed,
        )
        .unwrap_err(),
        "model_not_projected",
    );
    assert_input(
        lower_encoded_entity_create(
            &EncodedCreate::new(BASE_ID, vec![], vec![]),
            &type_id(TypeKind::Entity, "base"),
            &installed,
        )
        .unwrap_err(),
        "model_not_constructible",
    );
    assert_input(
        lower_encoded_entity_create(
            &EncodedCreate::new(
                RECORD_ID,
                positive_fields(),
                vec![(
                    "role",
                    vec![
                        EncodedReference::try_new(
                            RECORD_ID,
                            Some("0x1".into()),
                            vec![],
                            &crate::__codegen::ValidationPath::root(),
                        )
                        .unwrap(),
                    ],
                )],
            ),
            &record,
            &installed,
        )
        .unwrap_err(),
        "entity_roles_not_allowed",
    );

    let mut missing = positive_fields();
    missing.retain(|(identity, _)| *identity != TEXT);
    assert_input(
        lower_encoded_entity_create(
            &EncodedCreate::new(RECORD_ID, missing, vec![]),
            &record,
            &installed,
        )
        .unwrap_err(),
        "missing_field_evidence",
    );
    let mut duplicate_identity = positive_fields();
    duplicate_identity.push((TEXT, vec![EncodedScalar::String("again".into())]));
    assert_input(
        lower_encoded_entity_create(
            &EncodedCreate::new(RECORD_ID, duplicate_identity, vec![]),
            &record,
            &installed,
        )
        .unwrap_err(),
        "duplicate_field_evidence",
    );
    let mut unexpected = positive_fields();
    unexpected.push(("not-json", vec![]));
    assert_input(
        lower_encoded_entity_create(
            &EncodedCreate::new(RECORD_ID, unexpected, vec![]),
            &record,
            &installed,
        )
        .unwrap_err(),
        "unexpected_field_evidence",
    );
    let mut unrelated = positive_fields();
    unrelated.push((OTHER_ONLY, vec![EncodedScalar::String("other".into())]));
    assert_input(
        lower_encoded_entity_create(
            &EncodedCreate::new(RECORD_ID, unrelated, vec![]),
            &record,
            &installed,
        )
        .unwrap_err(),
        "unexpected_field_evidence",
    );
    let mut duplicate_scalar = positive_fields();
    duplicate_scalar
        .iter_mut()
        .find(|(identity, _)| *identity == TEXT)
        .unwrap()
        .1
        .push(EncodedScalar::String("again".into()));
    assert_input(
        lower_encoded_entity_create(
            &EncodedCreate::new(RECORD_ID, duplicate_scalar, vec![]),
            &record,
            &installed,
        )
        .unwrap_err(),
        "duplicate_scalar_evidence",
    );
    let mut too_many = positive_fields();
    too_many
        .iter_mut()
        .find(|(identity, _)| *identity == BOUNDED)
        .unwrap()
        .1
        .extend([
            EncodedScalar::Decimal(Decimal::try_new("3.5").unwrap()),
            EncodedScalar::Decimal(Decimal::try_new("4.5").unwrap()),
        ]);
    assert_input(
        lower_encoded_entity_create(
            &EncodedCreate::new(RECORD_ID, too_many, vec![]),
            &record,
            &installed,
        )
        .unwrap_err(),
        "cardinality_violation",
    );
    let mut wrong_domain = positive_fields();
    wrong_domain
        .iter_mut()
        .find(|(identity, _)| *identity == COUNT)
        .unwrap()
        .1 = vec![EncodedScalar::String("42".into())];
    assert_input(
        lower_encoded_entity_create(
            &EncodedCreate::new(RECORD_ID, wrong_domain, vec![]),
            &record,
            &installed,
        )
        .unwrap_err(),
        "wrong_scalar_domain",
    );
}

#[test]
fn selected_query_tokens_outside_create_are_classified_as_non_create() {
    assert_eq!(
        classify_create_membership(true, false),
        "non_create_field_evidence"
    );
    assert_eq!(classify_create_membership(true, true), "accepted");
    assert_eq!(
        classify_create_membership(false, false),
        "unexpected_field_evidence"
    );
}

#[test]
fn hydration_rejects_identity_shape_attribute_cardinality_and_domain_failures() {
    let installed = fixture();
    let record = type_id(TypeKind::Entity, "record");
    let attributes = positive_attributes();

    assert_hydration(
        hydrate_entity(
            record_row(vec![]),
            &type_id(TypeKind::Entity, "base"),
            &installed,
        )
        .unwrap_err(),
        "model_not_constructible",
    );
    assert_hydration(
        hydrate_entity(
            record_row(vec![]),
            &type_id(TypeKind::Relation, "link"),
            &installed,
        )
        .unwrap_err(),
        "wrong_model_kind",
    );
    assert_hydration(
        hydrate_entity(
            record_row(vec![]),
            &type_id(TypeKind::Entity, "ghost"),
            &installed,
        )
        .unwrap_err(),
        "model_not_projected",
    );

    let mut row = record_row(attributes.clone());
    row.iid = None;
    assert_hydration(
        hydrate_entity(row, &record, &installed).unwrap_err(),
        "missing_iid",
    );
    let mut row = record_row(attributes.clone());
    row.iid = Some("abc".into());
    assert_hydration(
        hydrate_entity(row, &record, &installed).unwrap_err(),
        "noncanonical_iid",
    );
    let mut row = record_row(attributes.clone());
    row.type_name = None;
    assert_hydration(
        hydrate_entity(row, &record, &installed).unwrap_err(),
        "missing_concrete_type",
    );
    let mut row = record_row(attributes.clone());
    row.type_name = Some("other".into());
    assert_hydration(
        hydrate_entity(row, &record, &installed).unwrap_err(),
        "wrong_concrete_type",
    );
    assert_hydration(
        hydrate_entity(
            record_row(vec![("unknown".into(), AttributeValue::String("x".into()))]),
            &record,
            &installed,
        )
        .unwrap_err(),
        "unexpected_provider_attribute",
    );

    let mut missing = attributes.clone();
    missing.retain(|(name, _)| name != "text");
    assert_hydration(
        hydrate_entity(record_row(missing), &record, &installed).unwrap_err(),
        "missing_field_evidence",
    );
    let mut duplicate = attributes.clone();
    duplicate.push(("text".into(), AttributeValue::String("again".into())));
    assert_hydration(
        hydrate_entity(record_row(duplicate), &record, &installed).unwrap_err(),
        "duplicate_scalar_evidence",
    );
    let mut wrong_domain = attributes.clone();
    *wrong_domain
        .iter_mut()
        .find(|(name, _)| name == "count")
        .unwrap() = ("count".into(), AttributeValue::String("42".into()));
    assert_hydration(
        hydrate_entity(record_row(wrong_domain), &record, &installed).unwrap_err(),
        "wrong_scalar_domain",
    );
    let mut over = attributes.clone();
    over.extend([
        ("bounded".into(), AttributeValue::Decimal("3.5".into())),
        ("bounded".into(), AttributeValue::Decimal("4.5".into())),
    ]);
    assert_hydration(
        hydrate_entity(record_row(over), &record, &installed).unwrap_err(),
        "cardinality_violation",
    );

    for (name, bad, code) in [
        (
            "amount",
            AttributeValue::Decimal("1.00".into()),
            "noncanonical_decimal",
        ),
        (
            "day",
            AttributeValue::Date("2026-02-30".into()),
            "noncanonical_date",
        ),
        (
            "moment",
            AttributeValue::DateTime("bad".into()),
            "noncanonical_datetime",
        ),
        (
            "zoned",
            AttributeValue::DateTimeTZ("bad".into()),
            "noncanonical_datetime_tz",
        ),
        (
            "span",
            AttributeValue::Duration("bad".into()),
            "noncanonical_duration",
        ),
        (
            "ratio",
            AttributeValue::Double(f64::NAN),
            "noncanonical_double",
        ),
    ] {
        let mut invalid = attributes.clone();
        *invalid
            .iter_mut()
            .find(|(actual, _)| actual == name)
            .unwrap() = (name.into(), bad);
        assert_hydration(
            hydrate_entity(record_row(invalid), &record, &installed).unwrap_err(),
            code,
        );
    }

    let mut indexed = attributes;
    indexed.retain(|(name, _)| name != "bounded");
    indexed.push(("bounded".into(), AttributeValue::Decimal("1.00".into())));
    let error = hydrate_entity(record_row(indexed), &record, &installed).unwrap_err();
    assert_eq!(
        error_code(error),
        (
            ModelValidationPhase::Hydration,
            "noncanonical_decimal".into(),
            vec!["bounded[0]".into()]
        )
    );
}

#[test]
fn opaque_field_tokens_require_selected_canonical_declaring_identity() {
    let installed = fixture();
    let record = type_id(TypeKind::Entity, "record");
    for identity in [
        "text",
        r#"{"owner":{"kind":"entity","label":"record"},"attribute":"text"}"#,
    ] {
        let mut fields = positive_fields();
        fields[0] = (identity, vec![EncodedScalar::String("bad".into())]);
        let error = lower_encoded_entity_create(
            &EncodedCreate::new(RECORD_ID, fields, vec![]),
            &record,
            &installed,
        )
        .unwrap_err();
        assert_eq!(
            error_code(error),
            (
                ModelValidationPhase::Input,
                "unexpected_field_evidence".into(),
                vec!["fields[0]".into()]
            )
        );
    }
}

#[test]
fn ambiguity_guard_and_generated_validation_mapping_preserve_stable_evidence() {
    let error = require_unambiguous_provider_attribute("text", 7, 2).unwrap_err();
    assert_eq!(
        error_code(error),
        (
            ModelValidationPhase::Hydration,
            "ambiguous_provider_attribute".into(),
            vec!["attributes[7]".into()]
        )
    );

    struct RejectedInput;
    impl __codegen::sealed::Sealed for RejectedInput {}
    impl IntoEncodedCreate for RejectedInput {
        fn into_encoded_create(self) -> std::result::Result<EncodedCreate, ValidationError> {
            Err(ValidationError::new(
                "outer.children[2].name",
                "fixture_rejected",
            ))
        }
    }

    let error = lower_entity_create(
        RejectedInput,
        &type_id(TypeKind::Entity, "record"),
        &fixture(),
    )
    .unwrap_err();
    assert!(std::error::Error::source(&error).is_some());
    assert_eq!(
        error_code(error),
        (
            ModelValidationPhase::Input,
            "fixture_rejected".into(),
            vec!["outer".into(), "children[2]".into(), "name".into()]
        )
    );
}

#[test]
fn hydration_evidence_owns_every_semantic_identity() {
    let mut type_identity = RECORD_ID.to_owned();
    let mut field_identity = TEXT.to_owned();
    let mut role_identity = "role-id".to_owned();
    let mut key_identity = "key-id".to_owned();
    let player = crate::__codegen::HydratedPlayer::from_owned(
        type_identity.clone(),
        Some("0x1".into()),
        vec![(key_identity.clone(), EncodedScalar::String("key".into()))],
    );
    let row = HydratedRow::from_owned(
        type_identity.clone(),
        "0x2".into(),
        vec![(
            field_identity.clone(),
            vec![EncodedScalar::String("value".into())],
        )],
        vec![(role_identity.clone(), vec![player])],
    );
    type_identity.clear();
    field_identity.clear();
    role_identity.clear();
    key_identity.clear();

    assert_eq!(row.type_id_json(), RECORD_ID);
    assert_eq!(row.fields()[0].0, TEXT);
    assert_eq!(row.roles()[0].0, "role-id");
    assert_eq!(row.roles()[0].1[0].type_id_json(), RECORD_ID);
    assert_eq!(row.roles()[0].1[0].keys()[0].0, "key-id");
}
