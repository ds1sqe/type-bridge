mod common;

use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::projection::{
    BindingTarget, ProjectedTokenIdentity, ProjectionConfig, ProjectionHandler,
};
use type_bridge_contract::schema::{DocumentId, OwnsFactId};
use type_bridge_contract::sdk_diagnostic::SdkDiagnosticCategory;
use type_bridge_contract::value::{CanonicalDouble, CanonicalString, CanonicalValue, DecimalValue};
use type_bridge_orm::{
    AnswerCancellation, ComparisonOp, Database, InstalledRuntimeProjection,
    ProjectedAttributeValue, ProjectedManagerComparison, ProjectedManagerFilter,
    ProjectedManagerFilterExecutor, QueryExecutionResourceLimits, SessionHandle,
};
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};

use common::MockBackend;

const SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  tenant: { value: string }
  external__id: { value: string }
  age: { value: integer }
  ratio: { value: double }
  enabled: { value: boolean }
  birthday: { value: date }
  observed-at: { value: datetime }
  zoned-at: { value: datetime-tz }
  amount: { value: decimal }
  elapsed: { value: duration }
entities:
  person:
    owns:
      identifier: { key: true }
      external__id: { card: { min: 0 } }
      age: { card: { min: 0 } }
      ratio: { card: { min: 0 } }
      enabled: { card: { min: 0 } }
      birthday: { card: { min: 0 } }
      observed-at: { card: { min: 0 } }
      zoned-at: { card: { min: 0 } }
      amount: { card: { min: 0 } }
      elapsed: { card: { min: 0 } }
  membership:
    owns:
      tenant: { key: true }
      identifier: { key: true }
  unkeyed:
    owns:
      identifier: { unique: true, card: { min: 0 } }
  duration-keyed:
    owns:
      elapsed: { key: true }
"#;

fn installed(target: BindingTarget) -> InstalledRuntimeProjection {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("projected-manager-filter.yaml").unwrap(),
        SCHEMA,
    )])
    .unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
    let (config, handler) = match target {
        BindingTarget::Python => (ProjectionConfig::python(), ProjectionHandler::python_v1()),
        BindingTarget::Rust => (ProjectionConfig::rust(), ProjectionHandler::rust_v1()),
        _ => panic!("focused manager fixture uses Python or Rust"),
    };
    InstalledRuntimeProjection::try_new(
        project(&resolved, target, &config, &[handler], &[]).unwrap(),
    )
    .unwrap()
}

fn type_id(kind: TypeKind, label: &str) -> TypeId {
    TypeId::new(kind, label).unwrap()
}

fn field(owner: &TypeId, attribute: &str) -> OwnsFactId {
    OwnsFactId::new(owner.clone(), AttributeId::new(attribute).unwrap()).unwrap()
}

fn token(owner: &TypeId, attribute: &str) -> ProjectedTokenIdentity {
    ProjectedTokenIdentity::Field {
        owner: owner.clone(),
        field: field(owner, attribute),
    }
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

fn long_value(
    installed: &InstalledRuntimeProjection,
    attribute: &str,
    value: i64,
) -> ProjectedAttributeValue {
    ProjectedAttributeValue::try_new(
        installed,
        type_id(TypeKind::Attribute, attribute),
        CanonicalValue::Long(value),
    )
    .unwrap()
}

fn boolean_value(
    installed: &InstalledRuntimeProjection,
    attribute: &str,
    value: bool,
) -> ProjectedAttributeValue {
    ProjectedAttributeValue::try_new(
        installed,
        type_id(TypeKind::Attribute, attribute),
        CanonicalValue::Boolean(value),
    )
    .unwrap()
}

fn projected_value(
    installed: &InstalledRuntimeProjection,
    attribute: &str,
    value: CanonicalValue,
) -> ProjectedAttributeValue {
    ProjectedAttributeValue::try_new(installed, type_id(TypeKind::Attribute, attribute), value)
        .unwrap()
}

#[test]
fn filters_are_persistent_and_admit_the_closed_scalar_operators() {
    let installed = installed(BindingTarget::Python);
    let person = type_id(TypeKind::Entity, "person");
    let empty = ProjectedManagerFilter::try_new(&installed, person.clone()).unwrap();
    assert!(empty.is_empty());

    let mut current = empty.clone();
    for comparison in [
        ProjectedManagerComparison::Eq,
        ProjectedManagerComparison::Ne,
        ProjectedManagerComparison::Lt,
        ProjectedManagerComparison::Lte,
        ProjectedManagerComparison::Gt,
        ProjectedManagerComparison::Gte,
    ] {
        current = current
            .try_and(
                &installed,
                &token(&person, "age"),
                comparison,
                &long_value(&installed, "age", 42),
            )
            .unwrap();
    }

    assert!(empty.is_empty());
    assert_eq!(current.len(), 6);
    assert_eq!(current.resource_measure().items(), 6);
    assert_eq!(current.resource_measure().attribute_values(), 6);
    assert!(current.resource_measure().bytes() > 6);

    let sibling = empty
        .try_and(
            &installed,
            &token(&person, "external__id"),
            ProjectedManagerComparison::Eq,
            &string_value(&installed, "external__id", "literal__value"),
        )
        .unwrap();
    assert_eq!(sibling.len(), 1);
    assert_eq!(sibling.model(), &person);
}

#[test]
fn equality_covers_all_nine_domains_and_ordering_covers_every_ordered_domain() {
    let installed = installed(BindingTarget::Python);
    let person = type_id(TypeKind::Entity, "person");
    let empty = ProjectedManagerFilter::try_new(&installed, person.clone()).unwrap();
    let domains = [
        (
            "identifier",
            CanonicalValue::String(CanonicalString::new("data-ada").unwrap()),
            true,
        ),
        ("age", CanonicalValue::Long(42), true),
        (
            "ratio",
            CanonicalValue::Double(CanonicalDouble::new(1.5).unwrap()),
            true,
        ),
        ("enabled", CanonicalValue::Boolean(true), false),
        (
            "birthday",
            CanonicalValue::Date("1815-12-10".parse().unwrap()),
            true,
        ),
        (
            "observed-at",
            CanonicalValue::DateTime("2026-08-14T12:34:56".parse().unwrap()),
            true,
        ),
        (
            "zoned-at",
            CanonicalValue::DateTimeTz("2026-08-14T12:34:56Z".parse().unwrap()),
            true,
        ),
        (
            "amount",
            CanonicalValue::Decimal(DecimalValue::new("12.50").unwrap()),
            true,
        ),
        (
            "elapsed",
            CanonicalValue::Duration("P1DT2S".parse().unwrap()),
            true,
        ),
    ];

    for (attribute, canonical, ordered) in domains {
        let value = projected_value(&installed, attribute, canonical);
        for comparison in [
            ProjectedManagerComparison::Eq,
            ProjectedManagerComparison::Ne,
        ] {
            empty
                .try_and(&installed, &token(&person, attribute), comparison, &value)
                .unwrap();
        }
        if ordered {
            for comparison in [
                ProjectedManagerComparison::Lt,
                ProjectedManagerComparison::Lte,
                ProjectedManagerComparison::Gt,
                ProjectedManagerComparison::Gte,
            ] {
                empty
                    .try_and(&installed, &token(&person, attribute), comparison, &value)
                    .unwrap();
            }
        }
    }
}

#[test]
fn token_package_owner_domain_and_boolean_order_fail_with_frozen_categories() {
    let python = installed(BindingTarget::Python);
    let rust = installed(BindingTarget::Rust);
    let person = type_id(TypeKind::Entity, "person");
    let membership = type_id(TypeKind::Entity, "membership");
    let filter = ProjectedManagerFilter::try_new(&python, person.clone()).unwrap();

    let wrong_owner = filter
        .try_and(
            &python,
            &token(&membership, "identifier"),
            ProjectedManagerComparison::Eq,
            &string_value(&python, "identifier", "ada"),
        )
        .unwrap_err();
    assert_eq!(wrong_owner.category(), SdkDiagnosticCategory::Integrity);
    assert_eq!(wrong_owner.code().as_str(), "field_owner_mismatch");

    let wrong_domain = filter
        .try_and(
            &python,
            &token(&person, "identifier"),
            ProjectedManagerComparison::Eq,
            &long_value(&python, "age", 42),
        )
        .unwrap_err();
    assert_eq!(wrong_domain.category(), SdkDiagnosticCategory::InvalidInput);
    assert_eq!(wrong_domain.code().as_str(), "wrong_scalar_domain");

    let wrong_package = filter
        .try_and(
            &python,
            &token(&person, "identifier"),
            ProjectedManagerComparison::Eq,
            &string_value(&rust, "identifier", "ada"),
        )
        .unwrap_err();
    assert_eq!(wrong_package.category(), SdkDiagnosticCategory::Integrity);
    assert_eq!(
        wrong_package.code().as_str(),
        "generated_token_package_mismatch"
    );

    let boolean_order = filter
        .try_and(
            &python,
            &token(&person, "enabled"),
            ProjectedManagerComparison::Gt,
            &boolean_value(&python, "enabled", false),
        )
        .unwrap_err();
    assert_eq!(
        boolean_order.category(),
        SdkDiagnosticCategory::InvalidInput
    );
    assert_eq!(boolean_order.code().as_str(), "invalid_operator_for_type");
}

#[tokio::test]
async fn first_requires_every_effective_key_before_opening_a_transaction() {
    let installed = installed(BindingTarget::Python);
    let membership = type_id(TypeKind::Entity, "membership");
    let unkeyed = type_id(TypeKind::Entity, "unkeyed");
    let backend = MockBackend::new(Vec::new());
    let queries = backend.queries.clone();
    let database = Database::with_backend(Box::new(backend), "manager-filter");
    let executor = ProjectedManagerFilterExecutor::new(&installed);

    let partial = ProjectedManagerFilter::try_new(&installed, membership.clone())
        .unwrap()
        .try_and(
            &installed,
            &token(&membership, "identifier"),
            ProjectedManagerComparison::Eq,
            &string_value(&installed, "identifier", "ada"),
        )
        .unwrap();
    let error = executor
        .first(
            &database,
            &partial,
            QueryExecutionResourceLimits::default(),
            AnswerCancellation::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.category(), SdkDiagnosticCategory::InvalidInput);
    assert_eq!(error.code().as_str(), "manager_first_requires_identity");

    let no_keys = ProjectedManagerFilter::try_new(&installed, unkeyed).unwrap();
    let error = executor
        .first(
            &database,
            &no_keys,
            QueryExecutionResourceLimits::default(),
            AnswerCancellation::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code().as_str(), "manager_first_requires_identity");
    assert!(queries.lock().unwrap().is_empty());
}

#[tokio::test]
async fn first_accepts_repeated_semantically_equal_duration_key_evidence() {
    let installed = installed(BindingTarget::Python);
    let duration_keyed = type_id(TypeKind::Entity, "duration-keyed");
    let duration = projected_value(
        &installed,
        "elapsed",
        CanonicalValue::Duration("P1DT2S".parse().unwrap()),
    );
    let filter = ProjectedManagerFilter::try_new(&installed, duration_keyed.clone())
        .unwrap()
        .try_and(
            &installed,
            &token(&duration_keyed, "elapsed"),
            ProjectedManagerComparison::Eq,
            &duration,
        )
        .unwrap()
        .try_and(
            &installed,
            &token(&duration_keyed, "elapsed"),
            ProjectedManagerComparison::Eq,
            &duration,
        )
        .unwrap();
    let backend = MockBackend::new(Vec::new());
    let queries = backend.queries.clone();
    let database = Database::with_backend(Box::new(backend), "manager-filter");
    let error = ProjectedManagerFilterExecutor::new(&installed)
        .first(
            &database,
            &filter,
            QueryExecutionResourceLimits::default(),
            AnswerCancellation::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.category(),
        SdkDiagnosticCategory::UnsupportedCapability
    );
    assert_eq!(error.code().as_str(), "missing_provider_capability");
    assert!(queries.lock().unwrap().is_empty());
}

#[test]
fn validated_exact_count_query_converts_but_richer_predicates_do_not() {
    let installed = installed(BindingTarget::Python);
    let person = type_id(TypeKind::Entity, "person");
    let registry = std::sync::Arc::new(installed.match_registry().unwrap());
    let session = SessionHandle::new(registry);
    let root = session.exact("person").unwrap();
    let shape = session.positional([root.one()]).unwrap();
    let base = session.query(shape).unwrap();
    let identifier = root.field("identifier").unwrap();
    let exact = base
        .where_predicates_in_order([
            identifier.compare_value(
                ComparisonOp::Equal,
                type_bridge_orm::AttributeValue::String("ada".into()),
            ),
            identifier.compare_value(
                ComparisonOp::NotEqual,
                type_bridge_orm::AttributeValue::String("grace".into()),
            ),
        ])
        .unwrap()
        .validate_count_by(&root)
        .unwrap();
    let converted =
        ProjectedManagerFilter::try_from_validated_exact_query(&installed, &exact).unwrap();
    assert_eq!(converted.model(), &person);
    assert_eq!(converted.len(), 2);

    let nested = base
        .where_predicate(identifier.compare_value(
            ComparisonOp::Equal,
            type_bridge_orm::AttributeValue::String("ada".into()),
        ))
        .unwrap()
        .where_predicate(identifier.compare_value(
            ComparisonOp::NotEqual,
            type_bridge_orm::AttributeValue::String("grace".into()),
        ))
        .unwrap()
        .where_predicate(identifier.compare_value(
            ComparisonOp::GreaterThanOrEqual,
            type_bridge_orm::AttributeValue::String("ada".into()),
        ))
        .unwrap()
        .validate_count_by(&root)
        .unwrap();
    let converted =
        ProjectedManagerFilter::try_from_validated_exact_query(&installed, &nested).unwrap();
    assert_eq!(converted.model(), &person);
    assert_eq!(converted.len(), 3);

    let rich = base
        .where_predicate(identifier.compare_value(
            ComparisonOp::Contains,
            type_bridge_orm::AttributeValue::String("a".into()),
        ))
        .unwrap()
        .validate_count_by(&root)
        .unwrap();
    let error =
        ProjectedManagerFilter::try_from_validated_exact_query(&installed, &rich).unwrap_err();
    assert_eq!(error.category(), SdkDiagnosticCategory::InvalidInput);
    assert_eq!(error.code().as_str(), "manager_filter_query_shape_invalid");
}

#[test]
fn manager_query_validation_flattens_the_full_boolean_term_ceiling() {
    let installed = installed(BindingTarget::Python);
    let registry = std::sync::Arc::new(installed.match_registry().unwrap());
    let session = SessionHandle::new(registry);
    let root = session.exact("person").unwrap();
    let shape = session.positional([root.one()]).unwrap();
    let mut query = session.query(shape).unwrap();
    let identifier = root.field("identifier").unwrap();
    for index in 0..type_bridge_contract::limits::MAX_BOOLEAN_TERMS {
        query = query
            .where_predicate(identifier.compare_value(
                ComparisonOp::NotEqual,
                type_bridge_orm::AttributeValue::String(format!("excluded-{index}")),
            ))
            .unwrap();
    }
    let validated = query.validate_manager_count_by(&root).unwrap();
    let converted =
        ProjectedManagerFilter::try_from_validated_exact_query(&installed, &validated).unwrap();
    assert_eq!(
        converted.len(),
        type_bridge_contract::limits::MAX_BOOLEAN_TERMS
    );

    let over_limit = query
        .where_predicate(identifier.compare_value(
            ComparisonOp::NotEqual,
            type_bridge_orm::AttributeValue::String("one-too-many".into()),
        ))
        .unwrap()
        .validate_manager_count_by(&root)
        .unwrap_err();
    assert!(
        over_limit
            .to_string()
            .contains("manager_filter_predicate_limit")
    );
}
