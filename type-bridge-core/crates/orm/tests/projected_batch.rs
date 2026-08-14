use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, ProjectionHandler};
use type_bridge_contract::schema::{DocumentId, OwnsFactId};
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCategory, SdkDiagnosticDetailValue, SdkDiagnosticName, SdkDiagnosticPathSegment,
};
use type_bridge_contract::temporal::{CanonicalDateTimeTz, TimeZoneDesignator};
use type_bridge_contract::value::{CanonicalString, CanonicalValue, DecimalValue};
use type_bridge_orm::{
    AnswerCancellation, InstalledRuntimeProjection, MAX_QUERY_ITEMS,
    PreparedProjectedBatchInvocation, ProjectedAttributeValue, ProjectedBatch,
    ProjectedBatchInvocationControl, ProjectedBatchOperation, ProjectedBatchRow, ProjectedCreate,
    ProjectedReference, QueryExecutionResourceLimits,
};
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};

const SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  tag: { value: string }
  note: { value: string }
  amount: { value: decimal }
  observed: { value: datetime-tz }
entities:
  abstract-item:
    abstract: true
  person:
    owns:
      identifier: { key: true }
      tag: { card: { min: 0 } }
  log:
    owns:
      note: { card: 1 }
  balance:
    owns:
      amount: { key: true }
  observation:
    owns:
      observed: { key: true }
relations:
  membership:
    owns:
      identifier: { key: true }
    relates:
      member: { card: { min: 0, max: 2 } }
plays:
  person:
    membership: [member]
"#;

fn installed(target: BindingTarget) -> InstalledRuntimeProjection {
    installed_from(target, SCHEMA)
}

fn installed_from(target: BindingTarget, source: &str) -> InstalledRuntimeProjection {
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("projected-batch.yaml").unwrap(), source)])
            .unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
    let (config, handler) = match target {
        BindingTarget::Python => (ProjectionConfig::python(), ProjectionHandler::python_v1()),
        BindingTarget::Rust => (ProjectionConfig::rust(), ProjectionHandler::rust_v1()),
        _ => panic!("focused fixture uses Python or Rust"),
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
        vec![
            (
                field(&person, "identifier"),
                vec![string_value(installed, "identifier", identifier)],
            ),
            (
                field(&person, "tag"),
                vec![
                    string_value(installed, "tag", "one"),
                    string_value(installed, "tag", "two"),
                ],
            ),
        ],
        vec![],
    )
    .unwrap()
}

fn membership_create(installed: &InstalledRuntimeProjection, identifier: &str) -> ProjectedCreate {
    let person = type_id(TypeKind::Entity, "person");
    let membership = type_id(TypeKind::Relation, "membership");
    let person_key = field(&person, "identifier");
    let reference = ProjectedReference::try_new(
        installed,
        person,
        Some("0x10".to_owned()),
        vec![(person_key, string_value(installed, "identifier", "ada"))],
    )
    .unwrap();
    ProjectedCreate::try_new(
        installed,
        membership.clone(),
        vec![(
            field(&membership, "identifier"),
            vec![string_value(installed, "identifier", identifier)],
        )],
        vec![(
            RoleId::new("membership", "member").unwrap(),
            vec![reference],
        )],
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

fn scalar_key_create(
    installed: &InstalledRuntimeProjection,
    model_label: &str,
    attribute: &str,
    value: CanonicalValue,
) -> ProjectedCreate {
    let model = type_id(TypeKind::Entity, model_label);
    ProjectedCreate::try_new(
        installed,
        model.clone(),
        vec![(
            field(&model, attribute),
            vec![
                ProjectedAttributeValue::try_new(
                    installed,
                    type_id(TypeKind::Attribute, attribute),
                    value,
                )
                .unwrap(),
            ],
        )],
        vec![],
    )
    .unwrap()
}

fn full_limits() -> QueryExecutionResourceLimits {
    QueryExecutionResourceLimits::default()
}

fn prepare<'a>(
    installed: &InstalledRuntimeProjection,
    batch: &'a ProjectedBatch,
    limits: QueryExecutionResourceLimits,
) -> Result<
    PreparedProjectedBatchInvocation<'a>,
    type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic,
> {
    PreparedProjectedBatchInvocation::try_new(
        installed,
        batch,
        ProjectedBatchInvocationControl::capture(limits, AnswerCancellation::default()),
    )
}

#[test]
fn all_operations_validate_empty_authority_and_store_ordinals() {
    let installed = installed(BindingTarget::Python);
    let person = type_id(TypeKind::Entity, "person");
    for operation in [
        ProjectedBatchOperation::Insert,
        ProjectedBatchOperation::Put,
        ProjectedBatchOperation::Update,
        ProjectedBatchOperation::Delete,
    ] {
        let empty = ProjectedBatch::try_new(&installed, person.clone(), operation, vec![]).unwrap();
        assert!(empty.is_empty());
        assert_eq!(empty.resource_measure().statements(), 0);
    }

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
    assert_eq!(batch.row_at(0).map(|(ordinal, _)| ordinal), Some(0));
    assert_eq!(batch.row_at(1).map(|(ordinal, _)| ordinal), Some(1));
}

#[test]
fn relation_writes_reject_a_roleless_final_row_before_io_but_delete_is_valid() {
    let installed = installed(BindingTarget::Python);
    let membership = type_id(TypeKind::Relation, "membership");
    for operation in [
        ProjectedBatchOperation::Insert,
        ProjectedBatchOperation::Put,
        ProjectedBatchOperation::Update,
    ] {
        let row = if operation == ProjectedBatchOperation::Update {
            ProjectedBatchRow::Update {
                iid: "0x10".into(),
                replacement: roleless_membership_create(&installed, "empty"),
            }
        } else {
            ProjectedBatchRow::Create(roleless_membership_create(&installed, "empty"))
        };
        let error = ProjectedBatch::try_new(&installed, membership.clone(), operation, vec![row])
            .unwrap_err();
        assert_eq!(error.category(), SdkDiagnosticCategory::InvalidInput);
        assert_eq!(error.code().as_str(), "relation_requires_role_player");
        assert_eq!(
            error.path(),
            [
                SdkDiagnosticPathSegment::Argument(name("rows")),
                SdkDiagnosticPathSegment::Index(0),
                SdkDiagnosticPathSegment::Type(membership.clone()),
            ]
        );
    }

    ProjectedBatch::try_new(
        &installed,
        membership,
        ProjectedBatchOperation::Delete,
        vec![ProjectedBatchRow::Delete { iid: "0x10".into() }],
    )
    .unwrap();
}

#[test]
fn roleless_relation_failure_reports_the_exact_later_ordinal() {
    let installed = installed(BindingTarget::Python);
    let membership = type_id(TypeKind::Relation, "membership");
    let error = ProjectedBatch::try_new(
        &installed,
        membership.clone(),
        ProjectedBatchOperation::Insert,
        vec![
            ProjectedBatchRow::Create(membership_create(&installed, "valid")),
            ProjectedBatchRow::Create(roleless_membership_create(&installed, "empty")),
        ],
    )
    .unwrap_err();
    assert_eq!(error.code().as_str(), "relation_requires_role_player");
    assert_eq!(
        error.path(),
        [
            SdkDiagnosticPathSegment::Argument(name("rows")),
            SdkDiagnosticPathSegment::Index(1),
            SdkDiagnosticPathSegment::Type(membership),
        ]
    );
}

#[test]
fn empty_skips_zero_limits_cancellation_deadline_and_reservations() {
    let installed = installed(BindingTarget::Python);
    let empty = ProjectedBatch::try_new(
        &installed,
        type_id(TypeKind::Entity, "person"),
        ProjectedBatchOperation::Delete,
        vec![],
    )
    .unwrap();
    let cancelled = AnswerCancellation::default();
    cancelled.cancel();
    let prepared = PreparedProjectedBatchInvocation::try_new(
        &installed,
        &empty,
        ProjectedBatchInvocationControl::capture(
            QueryExecutionResourceLimits::tightened(0, 0, 0, 0, 0, 0, 0, 0),
            cancelled,
        ),
    )
    .unwrap();
    prepared.check_control().unwrap();

    let abstract_error = ProjectedBatch::try_new(
        &installed,
        type_id(TypeKind::Entity, "abstract-item"),
        ProjectedBatchOperation::Delete,
        vec![],
    )
    .unwrap_err();
    assert_eq!(abstract_error.code().as_str(), "model_not_constructible");
    let kind_error = ProjectedBatch::try_new(
        &installed,
        type_id(TypeKind::Attribute, "identifier"),
        ProjectedBatchOperation::Delete,
        vec![],
    )
    .unwrap_err();
    assert_eq!(kind_error.code().as_str(), "wrong_model_kind");
}

#[test]
fn row_form_model_brand_and_iid_fail_at_the_exact_ordinal() {
    let installed_projection = installed(BindingTarget::Python);
    let other = installed(BindingTarget::Rust);
    let person = type_id(TypeKind::Entity, "person");
    for index in 0..3 {
        let mut rows = vec![
            ProjectedBatchRow::Create(person_create(&installed_projection, "ada")),
            ProjectedBatchRow::Create(person_create(&installed_projection, "bob")),
            ProjectedBatchRow::Create(person_create(&installed_projection, "cam")),
        ];
        rows[index] = ProjectedBatchRow::Delete {
            iid: "0x10".to_owned(),
        };
        let error = ProjectedBatch::try_new(
            &installed_projection,
            person.clone(),
            ProjectedBatchOperation::Insert,
            rows,
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "batch_row_operation_mismatch");
        assert_eq!(
            error.path(),
            [
                SdkDiagnosticPathSegment::Argument(name("rows")),
                SdkDiagnosticPathSegment::Index(index as u64),
            ]
        );
    }

    let foreign = ProjectedBatch::try_new(
        &installed_projection,
        person.clone(),
        ProjectedBatchOperation::Insert,
        vec![ProjectedBatchRow::Create(person_create(&other, "ada"))],
    )
    .unwrap_err();
    assert_eq!(foreign.code().as_str(), "generated_token_package_mismatch");
    assert_eq!(
        foreign.path(),
        [
            SdkDiagnosticPathSegment::Argument(name("rows")),
            SdkDiagnosticPathSegment::Index(0),
            SdkDiagnosticPathSegment::Type(person.clone()),
        ]
    );

    let model_mismatch = ProjectedBatch::try_new(
        &installed_projection,
        person.clone(),
        ProjectedBatchOperation::Insert,
        vec![ProjectedBatchRow::Create(scalar_key_create(
            &installed_projection,
            "log",
            "note",
            CanonicalValue::String(CanonicalString::new("entry").unwrap()),
        ))],
    )
    .unwrap_err();
    assert_eq!(model_mismatch.code().as_str(), "batch_row_model_mismatch");
    assert_eq!(
        model_mismatch.path(),
        [
            SdkDiagnosticPathSegment::Argument(name("rows")),
            SdkDiagnosticPathSegment::Index(0),
            SdkDiagnosticPathSegment::Type(type_id(TypeKind::Entity, "log")),
        ]
    );
    assert!(model_mismatch.details().is_empty());

    let iid = ProjectedBatch::try_new(
        &installed_projection,
        person,
        ProjectedBatchOperation::Delete,
        vec![ProjectedBatchRow::Delete {
            iid: "not-an-iid".to_owned(),
        }],
    )
    .unwrap_err();
    assert_eq!(iid.code().as_str(), "noncanonical_iid");
    assert_eq!(
        iid.path(),
        [
            SdkDiagnosticPathSegment::Argument(name("rows")),
            SdkDiagnosticPathSegment::Index(0),
            SdkDiagnosticPathSegment::Argument(name("iid")),
        ]
    );
}

#[test]
fn duplicate_targets_precede_keys_and_duplicate_keys_report_exact_field() {
    let installed = installed(BindingTarget::Python);
    let person = type_id(TypeKind::Entity, "person");
    let target = ProjectedBatch::try_new(
        &installed,
        person,
        ProjectedBatchOperation::Update,
        vec![
            ProjectedBatchRow::Update {
                iid: "0xAb".to_owned(),
                replacement: person_create(&installed, "same"),
            },
            ProjectedBatchRow::Update {
                iid: "0xab".to_owned(),
                replacement: person_create(&installed, "same"),
            },
        ],
    )
    .unwrap_err();
    assert_eq!(target.code().as_str(), "duplicate_batch_target");
    assert_eq!(
        target.path(),
        [
            SdkDiagnosticPathSegment::Argument(name("rows")),
            SdkDiagnosticPathSegment::Index(1),
            SdkDiagnosticPathSegment::Argument(name("iid")),
        ]
    );
    assert_eq!(
        target.details().get(&name("first_conflicting_index")),
        Some(&SdkDiagnosticDetailValue::Count(0))
    );
    assert_eq!(target.details().len(), 1);

    let deleted = ProjectedBatch::try_new(
        &installed,
        type_id(TypeKind::Entity, "person"),
        ProjectedBatchOperation::Delete,
        vec![
            ProjectedBatchRow::Delete { iid: "0xAb".into() },
            ProjectedBatchRow::Delete { iid: "0xab".into() },
        ],
    )
    .unwrap_err();
    assert_eq!(deleted.code().as_str(), "duplicate_batch_target");
    assert_eq!(
        deleted.path(),
        [
            SdkDiagnosticPathSegment::Argument(name("rows")),
            SdkDiagnosticPathSegment::Index(1),
            SdkDiagnosticPathSegment::Argument(name("iid")),
        ]
    );
    assert_eq!(
        deleted.details().get(&name("first_conflicting_index")),
        Some(&SdkDiagnosticDetailValue::Count(0))
    );
    assert_eq!(deleted.details().len(), 1);

    let person = type_id(TypeKind::Entity, "person");
    let key = ProjectedBatch::try_new(
        &installed,
        person.clone(),
        ProjectedBatchOperation::Insert,
        vec![
            ProjectedBatchRow::Create(person_create(&installed, "same")),
            ProjectedBatchRow::Create(person_create(&installed, "same")),
        ],
    )
    .unwrap_err();
    assert_eq!(key.code().as_str(), "duplicate_batch_key");
    assert_eq!(
        key.path(),
        [
            SdkDiagnosticPathSegment::Argument(name("rows")),
            SdkDiagnosticPathSegment::Index(1),
            SdkDiagnosticPathSegment::Field(field(&person, "identifier")),
        ]
    );
    assert_eq!(
        key.details().get(&name("first_conflicting_index")),
        Some(&SdkDiagnosticDetailValue::Count(0))
    );
    assert_eq!(key.details().len(), 1);
}

#[test]
fn decimal_and_datetime_tz_keys_use_semantic_identity() {
    let installed = installed(BindingTarget::Python);
    let cases = [(
        "balance",
        "amount",
        CanonicalValue::Decimal(DecimalValue::new("1.00").unwrap()),
        CanonicalValue::Decimal(DecimalValue::new("1.0").unwrap()),
    )];
    for (model_label, attribute, left, right) in cases {
        let model = type_id(TypeKind::Entity, model_label);
        let error = ProjectedBatch::try_new(
            &installed,
            model.clone(),
            ProjectedBatchOperation::Insert,
            vec![
                ProjectedBatchRow::Create(scalar_key_create(
                    &installed,
                    model_label,
                    attribute,
                    left,
                )),
                ProjectedBatchRow::Create(scalar_key_create(
                    &installed,
                    model_label,
                    attribute,
                    right,
                )),
            ],
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "duplicate_batch_key");
        assert_eq!(
            error.path(),
            [
                SdkDiagnosticPathSegment::Argument(name("rows")),
                SdkDiagnosticPathSegment::Index(1),
                SdkDiagnosticPathSegment::Field(field(&model, attribute)),
            ]
        );
        assert_eq!(
            error.details(),
            &[(
                name("first_conflicting_index"),
                SdkDiagnosticDetailValue::Count(0),
            )]
            .into_iter()
            .collect()
        );
    }

    let model = type_id(TypeKind::Entity, "observation");
    let datetime = |local: &str, zone: TimeZoneDesignator| {
        CanonicalValue::DateTimeTz(
            CanonicalDateTimeTz::new_fixed(local.parse().unwrap(), zone).unwrap(),
        )
    };
    let duplicate = ProjectedBatch::try_new(
        &installed,
        model.clone(),
        ProjectedBatchOperation::Insert,
        vec![
            ProjectedBatchRow::Create(scalar_key_create(
                &installed,
                "observation",
                "observed",
                datetime("2024-07-01T10:00:00", TimeZoneDesignator::Utc),
            )),
            ProjectedBatchRow::Create(scalar_key_create(
                &installed,
                "observation",
                "observed",
                datetime("2024-07-01T11:00:00", TimeZoneDesignator::Utc),
            )),
            ProjectedBatchRow::Create(scalar_key_create(
                &installed,
                "observation",
                "observed",
                CanonicalValue::DateTimeTz(
                    CanonicalDateTimeTz::new_named_resolved(
                        "2024-07-01T12:00:00".parse().unwrap(),
                        "Europe/Amsterdam",
                        7_200,
                    )
                    .unwrap(),
                ),
            )),
        ],
    )
    .unwrap_err();
    assert_eq!(duplicate.code().as_str(), "duplicate_batch_key");
    assert_eq!(
        duplicate.path(),
        [
            SdkDiagnosticPathSegment::Argument(name("rows")),
            SdkDiagnosticPathSegment::Index(2),
            SdkDiagnosticPathSegment::Field(field(&model, "observed")),
        ]
    );
    assert_eq!(
        duplicate.details().get(&name("first_conflicting_index")),
        Some(&SdkDiagnosticDetailValue::Count(0))
    );

    ProjectedBatch::try_new(
        &installed,
        model,
        ProjectedBatchOperation::Insert,
        vec![
            ProjectedBatchRow::Create(scalar_key_create(
                &installed,
                "observation",
                "observed",
                CanonicalValue::DateTimeTz(
                    CanonicalDateTimeTz::new_fixed(
                        "2024-07-01T10:00:00".parse().unwrap(),
                        TimeZoneDesignator::Utc,
                    )
                    .unwrap(),
                ),
            )),
            ProjectedBatchRow::Create(scalar_key_create(
                &installed,
                "observation",
                "observed",
                CanonicalValue::DateTimeTz(
                    CanonicalDateTimeTz::new_fixed(
                        "2024-07-01T10:00:01".parse().unwrap(),
                        TimeZoneDesignator::Utc,
                    )
                    .unwrap(),
                ),
            )),
        ],
    )
    .unwrap();
}

#[test]
fn unkeyed_insert_and_update_keep_equal_rows_distinct_but_put_rejects() {
    let installed = installed(BindingTarget::Python);
    let log = type_id(TypeKind::Entity, "log");
    let create = || {
        ProjectedCreate::try_new(
            &installed,
            log.clone(),
            vec![(
                field(&log, "note"),
                vec![string_value(&installed, "note", "same")],
            )],
            vec![],
        )
        .unwrap()
    };
    ProjectedBatch::try_new(
        &installed,
        log.clone(),
        ProjectedBatchOperation::Insert,
        vec![
            ProjectedBatchRow::Create(create()),
            ProjectedBatchRow::Create(create()),
        ],
    )
    .unwrap();
    ProjectedBatch::try_new(
        &installed,
        log.clone(),
        ProjectedBatchOperation::Update,
        vec![
            ProjectedBatchRow::Update {
                iid: "0x10".into(),
                replacement: create(),
            },
            ProjectedBatchRow::Update {
                iid: "0x11".into(),
                replacement: create(),
            },
        ],
    )
    .unwrap();
    let error =
        ProjectedBatch::try_new(&installed, log, ProjectedBatchOperation::Put, vec![]).unwrap_err();
    assert_eq!(error.code().as_str(), "put_requires_projected_key");
}

#[test]
fn measures_do_not_conflate_structural_members_and_forecast_bulk_statements() {
    let installed = installed(BindingTarget::Python);
    let person = ProjectedBatch::try_new(
        &installed,
        type_id(TypeKind::Entity, "person"),
        ProjectedBatchOperation::Insert,
        vec![ProjectedBatchRow::Create(person_create(&installed, "ada"))],
    )
    .unwrap();
    let person_measure = person.resource_measure();
    assert_eq!(person_measure.items(), 1);
    assert_eq!(person_measure.graph_nodes(), 1);
    assert_eq!(person_measure.attribute_values(), 3);
    assert_eq!(person_measure.collection_members(), 3);
    assert_eq!(person_measure.role_players(), 0);
    assert_eq!(person_measure.statements(), 2);

    let relation = ProjectedBatch::try_new(
        &installed,
        type_id(TypeKind::Relation, "membership"),
        ProjectedBatchOperation::Insert,
        vec![ProjectedBatchRow::Create(membership_create(
            &installed,
            "membership",
        ))],
    )
    .unwrap();
    let relation_measure = relation.resource_measure();
    assert_eq!(relation_measure.graph_nodes(), 2);
    assert_eq!(relation_measure.attribute_values(), 2);
    assert_eq!(relation_measure.collection_members(), 3);
    assert_eq!(relation_measure.role_players(), 1);
    assert_eq!(relation_measure.statements(), 3);

    let delete = ProjectedBatch::try_new(
        &installed,
        type_id(TypeKind::Relation, "membership"),
        ProjectedBatchOperation::Delete,
        vec![ProjectedBatchRow::Delete { iid: "0x20".into() }],
    )
    .unwrap();
    assert_eq!(delete.resource_measure().bytes(), 4);
    assert_eq!(delete.resource_measure().statements(), 1);
}

#[test]
fn every_resource_dimension_is_checked_in_deterministic_order() {
    let installed = installed(BindingTarget::Python);
    let batch = ProjectedBatch::try_new(
        &installed,
        type_id(TypeKind::Relation, "membership"),
        ProjectedBatchOperation::Insert,
        vec![ProjectedBatchRow::Create(membership_create(
            &installed,
            "membership",
        ))],
    )
    .unwrap();
    let measure = batch.resource_measure();
    prepare(&installed, &batch, full_limits()).unwrap();
    prepare(
        &installed,
        &batch,
        QueryExecutionResourceLimits::tightened(
            30_000,
            measure.items(),
            measure.bytes(),
            measure.graph_nodes(),
            measure.attribute_values(),
            measure.collection_members(),
            measure.role_players(),
            measure.statements(),
        ),
    )
    .unwrap();

    let cases = [
        (
            "batch_item_limit",
            QueryExecutionResourceLimits::tightened(
                30_000,
                measure.items() - 1,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                3,
            ),
        ),
        (
            "batch_byte_limit",
            QueryExecutionResourceLimits::tightened(
                30_000,
                u64::MAX,
                measure.bytes() - 1,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                3,
            ),
        ),
        (
            "batch_graph_node_limit",
            QueryExecutionResourceLimits::tightened(
                30_000,
                u64::MAX,
                u64::MAX,
                measure.graph_nodes() - 1,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                3,
            ),
        ),
        (
            "batch_attribute_value_limit",
            QueryExecutionResourceLimits::tightened(
                30_000,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                measure.attribute_values() - 1,
                u64::MAX,
                u64::MAX,
                3,
            ),
        ),
        (
            "batch_collection_member_limit",
            QueryExecutionResourceLimits::tightened(
                30_000,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                measure.collection_members() - 1,
                u64::MAX,
                3,
            ),
        ),
        (
            "batch_role_player_limit",
            QueryExecutionResourceLimits::tightened(
                30_000,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                measure.role_players() - 1,
                3,
            ),
        ),
        (
            "batch_statement_limit",
            QueryExecutionResourceLimits::tightened(
                30_000,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                measure.statements() - 1,
            ),
        ),
    ];
    for (code, limits) in cases {
        let error = prepare(&installed, &batch, limits).unwrap_err();
        assert_eq!(error.category(), SdkDiagnosticCategory::ResourceLimit);
        assert_eq!(error.code().as_str(), code);
        let (dimension, actual, maximum) = match code {
            "batch_item_limit" => ("items", measure.items(), measure.items() - 1),
            "batch_byte_limit" => ("bytes", measure.bytes(), measure.bytes() - 1),
            "batch_graph_node_limit" => (
                "graph_nodes",
                measure.graph_nodes(),
                measure.graph_nodes() - 1,
            ),
            "batch_attribute_value_limit" => (
                "attribute_values",
                measure.attribute_values(),
                measure.attribute_values() - 1,
            ),
            "batch_collection_member_limit" => (
                "collection_members",
                measure.collection_members(),
                measure.collection_members() - 1,
            ),
            "batch_role_player_limit" => (
                "role_players",
                measure.role_players(),
                measure.role_players() - 1,
            ),
            "batch_statement_limit" => (
                "statements",
                u64::from(measure.statements()),
                u64::from(measure.statements() - 1),
            ),
            _ => unreachable!(),
        };
        assert_eq!(
            error.path(),
            [
                SdkDiagnosticPathSegment::Argument(name("limits")),
                SdkDiagnosticPathSegment::Argument(name(dimension)),
            ]
        );
        assert_eq!(error.details().len(), 2);
        if code == "batch_byte_limit" {
            assert_eq!(
                error.details().get(&name("actual_bytes")),
                Some(&SdkDiagnosticDetailValue::ByteCount(measure.bytes()))
            );
            assert_eq!(
                error.details().get(&name("maximum_bytes")),
                Some(&SdkDiagnosticDetailValue::ByteCount(measure.bytes() - 1))
            );
            assert_eq!(actual, measure.bytes());
            assert_eq!(maximum, measure.bytes() - 1);
        } else {
            assert_eq!(
                error.details().get(&name("actual")),
                Some(&SdkDiagnosticDetailValue::Count(actual))
            );
            assert_eq!(
                error.details().get(&name("maximum")),
                Some(&SdkDiagnosticDetailValue::Count(maximum))
            );
        }
    }

    let first = prepare(
        &installed,
        &batch,
        QueryExecutionResourceLimits::tightened(30_000, 0, 0, 0, 0, 0, 0, 0),
    )
    .unwrap_err();
    assert_eq!(first.code().as_str(), "batch_item_limit");
}

#[test]
fn cancellation_and_deadline_diagnostics_are_exact() {
    let installed = installed(BindingTarget::Python);
    let person = type_id(TypeKind::Entity, "person");
    let make_insert = || vec![ProjectedBatchRow::Create(person_create(&installed, "ada"))];
    let batch = ProjectedBatch::try_new(
        &installed,
        person,
        ProjectedBatchOperation::Insert,
        make_insert(),
    )
    .unwrap();
    let cancelled = AnswerCancellation::default();
    cancelled.cancel();
    let cancellation = PreparedProjectedBatchInvocation::try_new(
        &installed,
        &batch,
        ProjectedBatchInvocationControl::capture(
            QueryExecutionResourceLimits::tightened(0, 0, 0, 0, 0, 0, 0, 0),
            cancelled,
        ),
    )
    .unwrap_err();
    assert_eq!(cancellation.category(), SdkDiagnosticCategory::Cancelled);
    assert_eq!(cancellation.code().as_str(), "provider_cancelled");
    assert!(cancellation.path().is_empty());
    assert!(cancellation.details().is_empty());
    let deadline = PreparedProjectedBatchInvocation::try_new(
        &installed,
        &batch,
        ProjectedBatchInvocationControl::capture(
            QueryExecutionResourceLimits::tightened(
                0,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                u64::MAX,
                3,
            ),
            AnswerCancellation::default(),
        ),
    )
    .unwrap_err();
    assert_eq!(deadline.category(), SdkDiagnosticCategory::ResourceLimit);
    assert_eq!(deadline.code().as_str(), "transaction_deadline_exceeded");
    assert!(deadline.path().is_empty());
    assert!(deadline.details().is_empty());
}

#[test]
fn reusable_batch_rejects_a_foreign_installed_projection() {
    let python = installed(BindingTarget::Python);
    let rust = installed(BindingTarget::Rust);
    let batch = ProjectedBatch::try_new(
        &python,
        type_id(TypeKind::Entity, "person"),
        ProjectedBatchOperation::Delete,
        vec![ProjectedBatchRow::Delete { iid: "0x10".into() }],
    )
    .unwrap();
    batch.validate_for(&python).unwrap();
    let error = batch.validate_for(&rust).unwrap_err();
    assert_eq!(error.code().as_str(), "generated_token_package_mismatch");

    let cancelled = AnswerCancellation::default();
    cancelled.cancel();
    let control = || {
        ProjectedBatchInvocationControl::capture(
            QueryExecutionResourceLimits::tightened(0, 0, 0, 0, 0, 0, 0, 0),
            cancelled.clone(),
        )
    };
    let prepared_error =
        PreparedProjectedBatchInvocation::try_new(&rust, &batch, control()).unwrap_err();
    assert_eq!(
        prepared_error.code().as_str(),
        "generated_token_package_mismatch"
    );

    let empty = ProjectedBatch::try_new(
        &python,
        type_id(TypeKind::Entity, "person"),
        ProjectedBatchOperation::Delete,
        vec![],
    )
    .unwrap();
    let empty_error =
        PreparedProjectedBatchInvocation::try_new(&rust, &empty, control()).unwrap_err();
    assert_eq!(
        empty_error.code().as_str(),
        "generated_token_package_mismatch"
    );

    let alternate_source = SCHEMA.replace(
        "      tag: { card: { min: 0 } }",
        "      tag: { card: { min: 0, max: 3 } }",
    );
    assert_ne!(alternate_source, SCHEMA);
    let alternate = installed_from(BindingTarget::Python, &alternate_source);
    assert_ne!(
        alternate.projection().semantic_fingerprint(),
        empty.semantic_fingerprint()
    );
    let stale = empty.validate_for(&alternate).unwrap_err();
    assert_eq!(stale.code().as_str(), "generated_token_package_mismatch");
    assert_eq!(
        stale.path(),
        [SdkDiagnosticPathSegment::Type(type_id(
            TypeKind::Entity,
            "person"
        ))]
    );
    assert_eq!(stale.details().len(), 2);
    assert_eq!(
        stale.details().get(&name("expected_fingerprint")),
        Some(&SdkDiagnosticDetailValue::Fingerprint(
            alternate
                .projection()
                .semantic_fingerprint()
                .as_fingerprint()
                .clone()
        ))
    );
    assert_eq!(
        stale.details().get(&name("actual_fingerprint")),
        Some(&SdkDiagnosticDetailValue::Fingerprint(
            empty.semantic_fingerprint().as_fingerprint().clone()
        ))
    );
}

#[test]
fn hard_item_cap_rejects_before_normalization_work() {
    let installed = installed(BindingTarget::Python);
    let rows = (0..=MAX_QUERY_ITEMS)
        .map(|index| ProjectedBatchRow::Delete {
            iid: format!("0x{:x}", index + 1),
        })
        .collect();
    let error = ProjectedBatch::try_new(
        &installed,
        type_id(TypeKind::Entity, "person"),
        ProjectedBatchOperation::Delete,
        rows,
    )
    .unwrap_err();
    assert_eq!(error.code().as_str(), "batch_item_limit");
    assert_eq!(
        error.details().get(&name("actual")),
        Some(&SdkDiagnosticDetailValue::Count(MAX_QUERY_ITEMS + 1))
    );
    assert_eq!(
        error.details().get(&name("maximum")),
        Some(&SdkDiagnosticDetailValue::Count(MAX_QUERY_ITEMS))
    );
}

#[test]
fn plain_constructor_rejects_a_non_item_hard_limit() {
    let installed = installed(BindingTarget::Python);
    let rows = (0..21_846)
        .map(|index| {
            ProjectedBatchRow::Create(person_create(&installed, &format!("person-{index}")))
        })
        .collect();
    let error = ProjectedBatch::try_new(
        &installed,
        type_id(TypeKind::Entity, "person"),
        ProjectedBatchOperation::Insert,
        rows,
    )
    .unwrap_err();
    assert_eq!(error.code().as_str(), "batch_attribute_value_limit");
    assert_eq!(
        error.path(),
        [
            SdkDiagnosticPathSegment::Argument(name("limits")),
            SdkDiagnosticPathSegment::Argument(name("attribute_values")),
        ]
    );
}

#[test]
fn preparation_preserves_the_control_deadline_captured_before_batch_work() {
    let installed = installed(BindingTarget::Python);
    let control =
        ProjectedBatchInvocationControl::capture(full_limits(), AnswerCancellation::default());
    let captured = control.deadline().instant();
    let batch = ProjectedBatch::try_new_for_invocation(
        &installed,
        type_id(TypeKind::Entity, "person"),
        ProjectedBatchOperation::Delete,
        vec![ProjectedBatchRow::Delete { iid: "0x10".into() }],
        &control,
    )
    .unwrap();
    let prepared = PreparedProjectedBatchInvocation::try_new(&installed, &batch, control).unwrap();
    assert_eq!(prepared.deadline().instant(), captured);
    prepared.check_control().unwrap();
}

fn name(value: &'static str) -> SdkDiagnosticName {
    SdkDiagnosticName::new(value).unwrap()
}
