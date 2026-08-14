use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, ProjectionHandler};
use type_bridge_contract::schema::{DocumentId, OwnsFactId};
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCategory, SdkDiagnosticDetailValue, SdkDiagnosticName, SdkDiagnosticPathSegment,
};
use type_bridge_contract::temporal::{CanonicalDateTimeTz, TimeZoneDesignator};
use type_bridge_contract::value::{CanonicalString, CanonicalValue, DecimalValue};
use type_bridge_orm::projected_batch::ProjectedBatchBindingBudget;
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

fn operation_rows(
    installed: &InstalledRuntimeProjection,
    model: &TypeId,
    operation: ProjectedBatchOperation,
) -> Vec<ProjectedBatchRow> {
    let creates = if model.kind() == TypeKind::Entity {
        ["first", "second"].map(|identifier| person_create(installed, identifier))
    } else {
        ["first", "second"].map(|identifier| membership_create(installed, identifier))
    };
    match operation {
        ProjectedBatchOperation::Insert | ProjectedBatchOperation::Put => {
            creates.into_iter().map(ProjectedBatchRow::Create).collect()
        }
        ProjectedBatchOperation::Update => ["0x10", "0x11"]
            .into_iter()
            .zip(creates)
            .map(|(iid, replacement)| ProjectedBatchRow::Update {
                iid: iid.to_owned(),
                replacement,
            })
            .collect(),
        ProjectedBatchOperation::Delete => ["0x10", "0x11"]
            .into_iter()
            .map(|iid| ProjectedBatchRow::Delete {
                iid: iid.to_owned(),
            })
            .collect(),
    }
}

fn mismatched_row(
    installed: &InstalledRuntimeProjection,
    model: &TypeId,
    operation: ProjectedBatchOperation,
) -> ProjectedBatchRow {
    match operation {
        ProjectedBatchOperation::Insert | ProjectedBatchOperation::Put => {
            ProjectedBatchRow::Delete { iid: "0x12".into() }
        }
        ProjectedBatchOperation::Update | ProjectedBatchOperation::Delete => {
            let create = if model.kind() == TypeKind::Entity {
                person_create(installed, "mismatched")
            } else {
                membership_create(installed, "mismatched")
            };
            ProjectedBatchRow::Create(create)
        }
    }
}

#[test]
fn binding_budget_matches_all_operation_model_prefixes_and_failure_ordinals() {
    let installed = installed(BindingTarget::Python);
    for model in [
        type_id(TypeKind::Entity, "person"),
        type_id(TypeKind::Relation, "membership"),
    ] {
        for operation in [
            ProjectedBatchOperation::Insert,
            ProjectedBatchOperation::Put,
            ProjectedBatchOperation::Update,
            ProjectedBatchOperation::Delete,
        ] {
            let rows = operation_rows(&installed, &model, operation);
            let mut budget =
                ProjectedBatchBindingBudget::try_new(&installed, model.clone(), operation).unwrap();
            assert!(budget.is_empty());
            assert_eq!(budget.resource_measure(), Default::default());

            budget.try_add_row(&installed, &rows[0]).unwrap();
            let prefix = ProjectedBatch::try_new(
                &installed,
                model.clone(),
                operation,
                vec![rows[0].clone()],
            )
            .unwrap();
            assert_eq!(budget.len(), 1);
            assert_eq!(budget.resource_measure(), prefix.resource_measure());

            let wrong = mismatched_row(&installed, &model, operation);
            let incremental_error = budget.try_add_row(&installed, &wrong).unwrap_err();
            let full_error = ProjectedBatch::try_new(
                &installed,
                model.clone(),
                operation,
                vec![rows[0].clone(), wrong],
            )
            .unwrap_err();
            assert_eq!(incremental_error, full_error);
            assert_eq!(budget.len(), 1);
            assert_eq!(budget.resource_measure(), prefix.resource_measure());

            budget.try_add_row(&installed, &rows[1]).unwrap();
            let complete =
                ProjectedBatch::try_new(&installed, model.clone(), operation, rows.clone())
                    .unwrap();
            assert_eq!(budget.len(), rows.len());
            assert_eq!(budget.resource_measure(), complete.resource_measure());
            for (index, row) in rows.iter().enumerate() {
                assert_eq!(complete.row_at(index), Some((index, row)));
            }
        }
    }
}

#[test]
fn binding_budget_authority_brand_and_row_model_match_full_construction() {
    let python = installed(BindingTarget::Python);
    let person = type_id(TypeKind::Entity, "person");
    let constructor_cases = [
        (
            type_id(TypeKind::Attribute, "identifier"),
            ProjectedBatchOperation::Delete,
        ),
        (
            type_id(TypeKind::Entity, "missing"),
            ProjectedBatchOperation::Delete,
        ),
        (
            type_id(TypeKind::Entity, "abstract-item"),
            ProjectedBatchOperation::Delete,
        ),
        (
            type_id(TypeKind::Entity, "log"),
            ProjectedBatchOperation::Put,
        ),
    ];
    for (model, operation) in constructor_cases {
        let incremental_error =
            ProjectedBatchBindingBudget::try_new(&python, model.clone(), operation).unwrap_err();
        let full_error = ProjectedBatch::try_new(&python, model, operation, vec![]).unwrap_err();
        assert_eq!(incremental_error, full_error);
    }

    let rust = installed(BindingTarget::Rust);
    let alternate_source = SCHEMA.replace(
        "      tag: { card: { min: 0 } }",
        "      tag: { card: { min: 0, max: 3 } }",
    );
    let alternate = installed_from(BindingTarget::Python, &alternate_source);
    for foreign in [&rust, &alternate] {
        let row = ProjectedBatchRow::Create(person_create(&python, "foreign"));
        let mut budget = ProjectedBatchBindingBudget::try_new(
            &python,
            person.clone(),
            ProjectedBatchOperation::Insert,
        )
        .unwrap();
        let incremental_error = budget.try_add_row(foreign, &row).unwrap_err();
        let full_error = ProjectedBatch::try_new(
            foreign,
            person.clone(),
            ProjectedBatchOperation::Insert,
            vec![row],
        )
        .unwrap_err();
        assert_eq!(incremental_error, full_error);
        assert!(budget.is_empty());
    }

    let first = ProjectedBatchRow::Create(person_create(&python, "first"));
    let wrong_model = ProjectedBatchRow::Create(scalar_key_create(
        &python,
        "log",
        "note",
        CanonicalValue::String(CanonicalString::new("wrong").unwrap()),
    ));
    let mut budget = ProjectedBatchBindingBudget::try_new(
        &python,
        person.clone(),
        ProjectedBatchOperation::Insert,
    )
    .unwrap();
    budget.try_add_row(&python, &first).unwrap();
    let prefix_measure = budget.resource_measure();
    let incremental_error = budget.try_add_row(&python, &wrong_model).unwrap_err();
    let full_error = ProjectedBatch::try_new(
        &python,
        person,
        ProjectedBatchOperation::Insert,
        vec![first, wrong_model],
    )
    .unwrap_err();
    assert_eq!(incremental_error, full_error);
    assert_eq!(budget.len(), 1);
    assert_eq!(budget.resource_measure(), prefix_measure);
}

#[test]
fn binding_budget_preserves_authority_control_and_row_precedence_before_brand_fence() {
    let python = installed(BindingTarget::Python);
    let rust = installed(BindingTarget::Rust);
    let person = type_id(TypeKind::Entity, "person");

    let mut wrong_form = ProjectedBatchBindingBudget::try_new(
        &python,
        person.clone(),
        ProjectedBatchOperation::Insert,
    )
    .unwrap();
    let row = ProjectedBatchRow::Delete { iid: "0x10".into() };
    let incremental = wrong_form.try_add_row(&rust, &row).unwrap_err();
    let authoritative = ProjectedBatch::try_new(
        &rust,
        person.clone(),
        ProjectedBatchOperation::Insert,
        vec![row],
    )
    .unwrap_err();
    assert_eq!(incremental, authoritative);
    assert_eq!(incremental.code().as_str(), "batch_row_operation_mismatch");
    assert!(wrong_form.is_empty());

    let unkeyed_source = SCHEMA.replace(
        "      identifier: { key: true }\n      tag:",
        "      identifier: { card: 1 }\n      tag:",
    );
    let unkeyed = installed_from(BindingTarget::Python, &unkeyed_source);
    let mut put =
        ProjectedBatchBindingBudget::try_new(&python, person.clone(), ProjectedBatchOperation::Put)
            .unwrap();
    let row = ProjectedBatchRow::Create(person_create(&unkeyed, "foreign"));
    let incremental = put.try_add_row(&unkeyed, &row).unwrap_err();
    let authoritative = ProjectedBatch::try_new(
        &unkeyed,
        person.clone(),
        ProjectedBatchOperation::Put,
        vec![row],
    )
    .unwrap_err();
    assert_eq!(incremental, authoritative);
    assert_eq!(incremental.code().as_str(), "put_requires_projected_key");
    assert!(put.is_empty());

    let cancellation = AnswerCancellation::default();
    cancellation.cancel();
    let control = ProjectedBatchInvocationControl::capture(full_limits(), cancellation);
    let mut cancelled = ProjectedBatchBindingBudget::try_new_for_invocation(
        &python,
        person.clone(),
        ProjectedBatchOperation::Delete,
        &control,
    )
    .unwrap();
    let error = cancelled
        .try_add_row(&rust, &ProjectedBatchRow::Delete { iid: "0x10".into() })
        .unwrap_err();
    assert_eq!(error.code().as_str(), "provider_cancelled");
    assert!(cancelled.is_empty());

    let mut fenced = ProjectedBatchBindingBudget::try_new(
        &python,
        person.clone(),
        ProjectedBatchOperation::Delete,
    )
    .unwrap();
    let valid_foreign = ProjectedBatchRow::Delete { iid: "0x10".into() };
    ProjectedBatch::try_new(
        &rust,
        person.clone(),
        ProjectedBatchOperation::Delete,
        vec![valid_foreign.clone()],
    )
    .unwrap();
    let error = fenced.try_add_row(&rust, &valid_foreign).unwrap_err();
    assert_eq!(error.code().as_str(), "generated_token_package_mismatch");
    assert_eq!(
        error.path(),
        [
            SdkDiagnosticPathSegment::Argument(name("rows")),
            SdkDiagnosticPathSegment::Index(0),
            SdkDiagnosticPathSegment::Type(person),
        ]
    );
    assert!(fenced.is_empty());
}

#[test]
fn binding_budget_defers_duplicate_identity_to_authoritative_construction() {
    let installed = installed(BindingTarget::Python);
    let person = type_id(TypeKind::Entity, "person");
    let key_rows = vec![
        ProjectedBatchRow::Create(person_create(&installed, "same")),
        ProjectedBatchRow::Create(person_create(&installed, "same")),
    ];
    let mut key_budget = ProjectedBatchBindingBudget::try_new(
        &installed,
        person.clone(),
        ProjectedBatchOperation::Insert,
    )
    .unwrap();
    for row in &key_rows {
        key_budget.try_add_row(&installed, row).unwrap();
    }
    assert_eq!(key_budget.len(), key_rows.len());
    assert_eq!(
        ProjectedBatch::try_new(
            &installed,
            person.clone(),
            ProjectedBatchOperation::Insert,
            key_rows,
        )
        .unwrap_err()
        .code()
        .as_str(),
        "duplicate_batch_key"
    );

    let target_rows = vec![
        ProjectedBatchRow::Delete { iid: "0xAb".into() },
        ProjectedBatchRow::Delete { iid: "0xab".into() },
    ];
    let mut target_budget = ProjectedBatchBindingBudget::try_new(
        &installed,
        person.clone(),
        ProjectedBatchOperation::Delete,
    )
    .unwrap();
    for row in &target_rows {
        target_budget.try_add_row(&installed, row).unwrap();
    }
    assert_eq!(target_budget.len(), target_rows.len());
    assert_eq!(
        ProjectedBatch::try_new(
            &installed,
            person,
            ProjectedBatchOperation::Delete,
            target_rows,
        )
        .unwrap_err()
        .code()
        .as_str(),
        "duplicate_batch_target"
    );
}

#[test]
fn binding_budget_resource_crossing_precedes_duplicate_only_before_retention() {
    let installed = installed(BindingTarget::Python);
    let person = type_id(TypeKind::Entity, "person");
    let row = ProjectedBatchRow::Create(person_create(&installed, "same"));
    let prefix = ProjectedBatch::try_new(
        &installed,
        person.clone(),
        ProjectedBatchOperation::Insert,
        vec![row.clone()],
    )
    .unwrap();
    let limits = QueryExecutionResourceLimits::tightened(
        30_000,
        MAX_QUERY_ITEMS,
        prefix.resource_measure().bytes(),
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        3,
    );
    let control = ProjectedBatchInvocationControl::capture(limits, AnswerCancellation::default());
    let mut budget = ProjectedBatchBindingBudget::try_new_for_invocation(
        &installed,
        person.clone(),
        ProjectedBatchOperation::Insert,
        &control,
    )
    .unwrap();
    budget.try_add_row(&installed, &row).unwrap();
    let retained_measure = budget.resource_measure();
    let crossing = budget.try_add_row(&installed, &row).unwrap_err();
    assert_eq!(crossing.code().as_str(), "batch_byte_limit");
    assert_eq!(budget.len(), 1);
    assert_eq!(budget.resource_measure(), retained_measure);

    let authoritative = ProjectedBatch::try_new_for_invocation(
        &installed,
        person,
        ProjectedBatchOperation::Insert,
        vec![row.clone(), row],
        &control,
    )
    .unwrap_err();
    assert_eq!(authoritative.code().as_str(), "duplicate_batch_key");
}

#[test]
fn binding_budget_matches_every_invocation_resource_crossing() {
    let installed = installed(BindingTarget::Python);
    let membership = type_id(TypeKind::Relation, "membership");
    let rows = operation_rows(&installed, &membership, ProjectedBatchOperation::Insert);
    let prefix = ProjectedBatch::try_new(
        &installed,
        membership.clone(),
        ProjectedBatchOperation::Insert,
        vec![rows[0].clone()],
    )
    .unwrap();
    let complete = ProjectedBatch::try_new(
        &installed,
        membership.clone(),
        ProjectedBatchOperation::Insert,
        rows.clone(),
    )
    .unwrap();
    let measure = complete.resource_measure();
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
    ];
    for (code, limits) in cases {
        let full_control =
            ProjectedBatchInvocationControl::capture(limits, AnswerCancellation::default());
        let full_error = ProjectedBatch::try_new_for_invocation(
            &installed,
            membership.clone(),
            ProjectedBatchOperation::Insert,
            rows.clone(),
            &full_control,
        )
        .unwrap_err();
        assert_eq!(full_error.code().as_str(), code);

        let incremental_control =
            ProjectedBatchInvocationControl::capture(limits, AnswerCancellation::default());
        let mut budget = ProjectedBatchBindingBudget::try_new_for_invocation(
            &installed,
            membership.clone(),
            ProjectedBatchOperation::Insert,
            &incremental_control,
        )
        .unwrap();
        budget.try_add_row(&installed, &rows[0]).unwrap();
        let incremental_error = budget.try_add_row(&installed, &rows[1]).unwrap_err();
        assert_eq!(incremental_error, full_error);
        assert_eq!(budget.len(), 1);
        assert_eq!(budget.resource_measure(), prefix.resource_measure());
    }

    let statement_limits = QueryExecutionResourceLimits::tightened(
        30_000,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        u64::MAX,
        prefix.resource_measure().statements() - 1,
    );
    let full_control =
        ProjectedBatchInvocationControl::capture(statement_limits, AnswerCancellation::default());
    let full_error = ProjectedBatch::try_new_for_invocation(
        &installed,
        membership.clone(),
        ProjectedBatchOperation::Insert,
        vec![rows[0].clone()],
        &full_control,
    )
    .unwrap_err();
    assert_eq!(full_error.code().as_str(), "batch_statement_limit");
    let incremental_control =
        ProjectedBatchInvocationControl::capture(statement_limits, AnswerCancellation::default());
    let mut budget = ProjectedBatchBindingBudget::try_new_for_invocation(
        &installed,
        membership,
        ProjectedBatchOperation::Insert,
        &incremental_control,
    )
    .unwrap();
    let incremental_error = budget.try_add_row(&installed, &rows[0]).unwrap_err();
    assert_eq!(incremental_error, full_error);
    assert!(budget.is_empty());
    assert_eq!(budget.resource_measure(), Default::default());
}

#[test]
fn binding_budget_reuses_one_control_and_empty_skips_it() {
    let installed = installed(BindingTarget::Python);
    let person = type_id(TypeKind::Entity, "person");
    let control =
        ProjectedBatchInvocationControl::capture(full_limits(), AnswerCancellation::default());
    let captured_deadline = control.deadline().instant();
    let row = ProjectedBatchRow::Create(person_create(&installed, "ada"));
    let mut budget = ProjectedBatchBindingBudget::try_new_for_invocation(
        &installed,
        person.clone(),
        ProjectedBatchOperation::Insert,
        &control,
    )
    .unwrap();
    budget.checkpoint().unwrap();
    budget.try_add_row(&installed, &row).unwrap();
    assert_eq!(budget.len(), 1);

    let batch = ProjectedBatch::try_new_for_invocation(
        &installed,
        person.clone(),
        ProjectedBatchOperation::Insert,
        vec![row],
        &control,
    )
    .unwrap();
    let prepared = PreparedProjectedBatchInvocation::try_new(&installed, &batch, control).unwrap();
    assert_eq!(prepared.deadline().instant(), captured_deadline);

    let cancelled = AnswerCancellation::default();
    cancelled.cancel();
    let cancelled_control = ProjectedBatchInvocationControl::capture(
        QueryExecutionResourceLimits::tightened(0, 0, 0, 0, 0, 0, 0, 0),
        cancelled,
    );
    let empty_budget = ProjectedBatchBindingBudget::try_new_for_invocation(
        &installed,
        person.clone(),
        ProjectedBatchOperation::Delete,
        &cancelled_control,
    )
    .unwrap();
    assert!(empty_budget.is_empty());
    assert_eq!(
        empty_budget.checkpoint().unwrap_err().code().as_str(),
        "provider_cancelled"
    );
    ProjectedBatch::try_new_for_invocation(
        &installed,
        person,
        ProjectedBatchOperation::Delete,
        vec![],
        &cancelled_control,
    )
    .unwrap();
}

#[test]
fn binding_budget_nonempty_cancellation_keeps_the_last_accepted_prefix() {
    let installed = installed(BindingTarget::Python);
    let person = type_id(TypeKind::Entity, "person");
    let cancellation = AnswerCancellation::default();
    let control = ProjectedBatchInvocationControl::capture(full_limits(), cancellation.clone());
    let mut budget = ProjectedBatchBindingBudget::try_new_for_invocation(
        &installed,
        person,
        ProjectedBatchOperation::Insert,
        &control,
    )
    .unwrap();
    let first = ProjectedBatchRow::Create(person_create(&installed, "first"));
    budget.try_add_row(&installed, &first).unwrap();
    let retained_measure = budget.resource_measure();

    cancellation.cancel();
    let error = budget
        .try_add_row(
            &installed,
            &ProjectedBatchRow::Create(person_create(&installed, "second")),
        )
        .unwrap_err();
    assert_eq!(error.code().as_str(), "provider_cancelled");
    assert_eq!(budget.len(), 1);
    assert_eq!(budget.resource_measure(), retained_measure);
}

#[test]
fn plain_binding_budget_rejects_the_first_row_beyond_the_hard_item_cap() {
    let installed = installed(BindingTarget::Python);
    let person = type_id(TypeKind::Entity, "person");
    let row = ProjectedBatchRow::Delete { iid: "0x10".into() };
    let mut budget =
        ProjectedBatchBindingBudget::try_new(&installed, person, ProjectedBatchOperation::Delete)
            .unwrap();
    let maximum = usize::try_from(MAX_QUERY_ITEMS).unwrap();
    for _ in 0..maximum {
        budget.try_add_row(&installed, &row).unwrap();
    }
    let retained_measure = budget.resource_measure();
    let error = budget.try_add_row(&installed, &row).unwrap_err();
    assert_eq!(error.code().as_str(), "batch_item_limit");
    assert_eq!(budget.len(), maximum);
    assert_eq!(budget.resource_measure(), retained_measure);
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
