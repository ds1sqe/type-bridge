use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::limits::MAX_CANONICAL_STRING_BYTES;
use type_bridge_contract::projection::{
    BindingTarget, CSymbolPrefix, ProjectionConfig, ProjectionHandler,
};
use type_bridge_contract::schema::{DocumentId, OwnsFactId};
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCategory, SdkDiagnosticDetailValue, SdkDiagnosticName, SdkDiagnosticPathSegment,
};
use type_bridge_contract::value::{CanonicalString, CanonicalValue};
use type_bridge_orm::{
    InstalledRuntimeProjection, ProjectedAttributeValue, ProjectedCreate, ProjectedReference,
    ProjectedRolePlayer, ProjectedThing,
};
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};

const SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  identifier:
    value:
      type: string
      regex: "^[a-z]+$"
  email: { value: string }
  nickname: { value: string }
entities:
  actor:
    abstract: true
    owns:
      identifier: { key: true }
  person:
    sub: actor
    owns:
      email: { key: true }
      nickname:
        card: { min: 0 }
        regex: "^nickname-.+$"
  organization:
    owns:
      identifier: { key: true }
  account:
    owns:
      identifier: { key: true }
  premium-account:
    sub: account
relations:
  membership:
    relates:
      member: { card: 1 }
      observer: { card: { min: 0, max: 2 } }
      premium: { card: { min: 0, max: 1 } }
plays:
  actor:
    membership: [member]
  person:
    membership: [observer]
  organization:
    membership: [observer]
  premium-account:
    membership: [premium]
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

const CONSTRAINT_SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
  nickname:
    value:
      type: string
      regex: "^[A-Z][a-z]+$"
      values: [Ada, Dana]
  handle: { value: string }
  robot-id: { value: integer }
  val-constrained:
    value:
      type: integer
      range: { min: 0, max: 80 }
entities:
  person:
    owns:
      identifier: { key: true }
      nickname: { card: 1 }
      handle: { unique: true, card: { min: 0, max: 2 } }
      val-constrained: { card: 1, range: { min: 20, max: 80 } }
  robot:
    owns:
      robot-id: { key: true }
      val-constrained: { card: 1, range: { min: 0, max: 50 } }
"#;

fn installed(target: BindingTarget, source: &str) -> InstalledRuntimeProjection {
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("projected-model.yaml").unwrap(), source)])
            .unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
    let (config, handlers) = match target {
        BindingTarget::Python => (
            ProjectionConfig::python(),
            vec![ProjectionHandler::python_v1()],
        ),
        BindingTarget::TypeScript => (
            ProjectionConfig::typescript(),
            vec![ProjectionHandler::typescript_v1()],
        ),
        BindingTarget::Rust => (ProjectionConfig::rust(), vec![ProjectionHandler::rust_v1()]),
        _ => panic!("the focused fixture does not need another binding target"),
    };
    let runtime = project(&resolved, target, &config, &handlers, &[]).unwrap();
    InstalledRuntimeProjection::try_new(runtime).unwrap()
}

fn installed_c(source: &str, symbol_prefix: &str) -> InstalledRuntimeProjection {
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("projected-model.yaml").unwrap(), source)])
            .unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
    let runtime = project(
        &resolved,
        BindingTarget::C,
        &ProjectionConfig::c(CSymbolPrefix::new(symbol_prefix).unwrap()),
        &[ProjectionHandler::c_v2()],
        &[],
    )
    .unwrap();
    InstalledRuntimeProjection::try_new(runtime).unwrap()
}

fn installed_ordered() -> InstalledRuntimeProjection {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("projected-model-ordered.yaml").unwrap(),
        ORDERED_SCHEMA,
    )])
    .unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
    let runtime = project(
        &resolved,
        BindingTarget::Python,
        &ProjectionConfig::python(),
        &[ProjectionHandler::python_v2()],
        &[],
    )
    .unwrap();
    InstalledRuntimeProjection::try_new(runtime).unwrap()
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
    value: impl Into<String>,
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

fn person_fields(
    installed: &InstalledRuntimeProjection,
) -> Vec<(OwnsFactId, Vec<ProjectedAttributeValue>)> {
    let person = type_id(TypeKind::Entity, "person");
    vec![
        (
            field(&person, "identifier"),
            vec![string_value(installed, "identifier", "ada")],
        ),
        (
            field(&person, "email"),
            vec![string_value(installed, "email", "ada@example.test")],
        ),
    ]
}

fn iid_reference(
    installed: &InstalledRuntimeProjection,
    type_id: TypeId,
    iid: &str,
) -> ProjectedReference {
    ProjectedReference::try_new(installed, type_id, Some(iid.to_owned()), vec![]).unwrap()
}

fn assert_code(
    diagnostic: &type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic,
    category: SdkDiagnosticCategory,
    code: &str,
) {
    assert_eq!(diagnostic.category(), category);
    assert_eq!(diagnostic.code().as_str(), code);
}

fn assert_duplicate_details(
    diagnostic: &type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic,
    first_index: u64,
    duplicate_index: u64,
) {
    assert_eq!(
        diagnostic
            .details()
            .get(&SdkDiagnosticName::new("first_index").unwrap()),
        Some(&SdkDiagnosticDetailValue::Count(first_index)),
    );
    assert_eq!(
        diagnostic
            .details()
            .get(&SdkDiagnosticName::new("duplicate_index").unwrap()),
        Some(&SdkDiagnosticDetailValue::Count(duplicate_index)),
    );
}

fn assert_signed_range_details(
    diagnostic: &type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic,
    actual: i64,
    bound_name: &str,
    bound: i64,
) {
    assert_eq!(diagnostic.details().len(), 2);
    assert_eq!(
        diagnostic
            .details()
            .get(&SdkDiagnosticName::new("actual").unwrap()),
        Some(&SdkDiagnosticDetailValue::Signed(actual)),
    );
    assert_eq!(
        diagnostic
            .details()
            .get(&SdkDiagnosticName::new(bound_name).unwrap()),
        Some(&SdkDiagnosticDetailValue::Signed(bound)),
    );
    let absent_bound = if bound_name == "minimum" {
        "maximum"
    } else {
        "minimum"
    };
    assert!(
        !diagnostic
            .details()
            .contains_key(&SdkDiagnosticName::new(absent_bound).unwrap())
    );
}

fn projected_strings(values: &[ProjectedAttributeValue]) -> Vec<&str> {
    values
        .iter()
        .map(|value| match value.value() {
            CanonicalValue::String(value) => value.as_str(),
            other => panic!("ordered string fixture changed domain: {other:?}"),
        })
        .collect()
}

#[test]
fn canonical_scalars_and_create_fields_use_exact_semantic_tokens() {
    let installed = installed(BindingTarget::Python, SCHEMA);
    let identifier = type_id(TypeKind::Attribute, "identifier");
    let wrong_domain =
        ProjectedAttributeValue::try_new(&installed, identifier.clone(), CanonicalValue::Long(1))
            .unwrap_err();
    assert_code(
        &wrong_domain,
        SdkDiagnosticCategory::InvalidInput,
        "wrong_scalar_domain",
    );
    assert_eq!(
        wrong_domain.path(),
        [SdkDiagnosticPathSegment::Type(identifier.clone())]
    );

    let regex = ProjectedAttributeValue::try_new(
        &installed,
        identifier,
        CanonicalValue::String(CanonicalString::new("Ada").unwrap()),
    )
    .unwrap_err();
    assert_code(
        &regex,
        SdkDiagnosticCategory::InvalidInput,
        "regex_constraint_violation",
    );

    let person = type_id(TypeKind::Entity, "person");
    let create = ProjectedCreate::try_new(
        &installed,
        person.clone(),
        person_fields(&installed),
        vec![],
    )
    .unwrap();
    assert_eq!(create.type_id(), &person);
    assert_eq!(create.fields().len(), 3);
    assert!(create.fields()[&field(&person, "nickname")].is_empty());

    let bad_alias = ProjectedCreate::try_new(
        &installed,
        person.clone(),
        {
            let mut fields = person_fields(&installed);
            fields.push((
                field(&person, "nickname"),
                vec![string_value(&installed, "nickname", "wrong")],
            ));
            fields
        },
        vec![],
    )
    .unwrap_err();
    assert_code(
        &bad_alias,
        SdkDiagnosticCategory::InvalidInput,
        "regex_constraint_violation",
    );

    let missing = ProjectedCreate::try_new(
        &installed,
        person.clone(),
        vec![person_fields(&installed).remove(0)],
        vec![],
    )
    .unwrap_err();
    assert_code(
        &missing,
        SdkDiagnosticCategory::InvalidInput,
        "missing_required_field",
    );

    let duplicate_field = field(&person, "identifier");
    let duplicate = ProjectedCreate::try_new(
        &installed,
        person.clone(),
        vec![
            (
                duplicate_field.clone(),
                vec![string_value(&installed, "identifier", "ada")],
            ),
            (
                duplicate_field,
                vec![string_value(&installed, "identifier", "grace")],
            ),
        ],
        vec![],
    )
    .unwrap_err();
    assert_code(
        &duplicate,
        SdkDiagnosticCategory::InvalidInput,
        "duplicate_field_token",
    );

    let organization = type_id(TypeKind::Entity, "organization");
    let unknown = ProjectedCreate::try_new(
        &installed,
        person,
        vec![
            person_fields(&installed).remove(0),
            (
                field(&organization, "identifier"),
                vec![string_value(&installed, "identifier", "acme")],
            ),
        ],
        vec![],
    )
    .unwrap_err();
    assert_code(
        &unknown,
        SdkDiagnosticCategory::InvalidInput,
        "field_not_creatable",
    );
}

#[test]
fn scalar_constraints_keep_stable_annotation_order_and_exact_long_bound_details() {
    let installed = installed(BindingTarget::Python, CONSTRAINT_SCHEMA);
    let nickname = type_id(TypeKind::Attribute, "nickname");
    let regex = ProjectedAttributeValue::try_new(
        &installed,
        nickname.clone(),
        CanonicalValue::String(CanonicalString::new("ada").unwrap()),
    )
    .unwrap_err();
    assert_code(
        &regex,
        SdkDiagnosticCategory::InvalidInput,
        "regex_constraint_violation",
    );
    assert_eq!(
        regex.path(),
        [SdkDiagnosticPathSegment::Type(nickname.clone())],
    );
    assert!(regex.details().is_empty());

    let values = ProjectedAttributeValue::try_new(
        &installed,
        nickname.clone(),
        CanonicalValue::String(CanonicalString::new("Alan").unwrap()),
    )
    .unwrap_err();
    assert_code(
        &values,
        SdkDiagnosticCategory::InvalidInput,
        "values_constraint_violation",
    );
    assert_eq!(values.path(), [SdkDiagnosticPathSegment::Type(nickname)],);
    assert!(values.details().is_empty());

    let constrained = type_id(TypeKind::Attribute, "val-constrained");
    let upper =
        ProjectedAttributeValue::try_new(&installed, constrained.clone(), CanonicalValue::Long(81))
            .unwrap_err();
    assert_code(
        &upper,
        SdkDiagnosticCategory::InvalidInput,
        "range_constraint_violation",
    );
    assert_eq!(upper.path(), [SdkDiagnosticPathSegment::Type(constrained)],);
    assert_signed_range_details(&upper, 81, "maximum", 80);
}

#[test]
fn owner_long_ranges_preserve_exact_create_and_hydration_diagnostics() {
    let installed = installed(BindingTarget::Python, CONSTRAINT_SCHEMA);
    let person = type_id(TypeKind::Entity, "person");
    let robot = type_id(TypeKind::Entity, "robot");
    let constrained_person = field(&person, "val-constrained");
    let constrained_robot = field(&robot, "val-constrained");
    let person_fields = |value| {
        vec![
            (
                field(&person, "identifier"),
                vec![string_value(&installed, "identifier", "data-ada")],
            ),
            (
                field(&person, "nickname"),
                vec![string_value(&installed, "nickname", "Ada")],
            ),
            (
                constrained_person.clone(),
                vec![long_value(&installed, "val-constrained", value)],
            ),
        ]
    };
    let robot_fields = |value| {
        vec![
            (
                field(&robot, "robot-id"),
                vec![long_value(&installed, "robot-id", -7)],
            ),
            (
                constrained_robot.clone(),
                vec![long_value(&installed, "val-constrained", value)],
            ),
        ]
    };

    let create_lower =
        ProjectedCreate::try_new(&installed, person.clone(), person_fields(19), vec![])
            .unwrap_err();
    assert_code(
        &create_lower,
        SdkDiagnosticCategory::InvalidInput,
        "range_constraint_violation",
    );
    assert_eq!(
        create_lower.path(),
        [
            SdkDiagnosticPathSegment::Type(person.clone()),
            SdkDiagnosticPathSegment::Field(constrained_person.clone()),
        ],
    );
    assert_signed_range_details(&create_lower, 19, "minimum", 20);

    let create_upper =
        ProjectedCreate::try_new(&installed, robot.clone(), robot_fields(51), vec![]).unwrap_err();
    assert_code(
        &create_upper,
        SdkDiagnosticCategory::InvalidInput,
        "range_constraint_violation",
    );
    assert_eq!(
        create_upper.path(),
        [
            SdkDiagnosticPathSegment::Type(robot.clone()),
            SdkDiagnosticPathSegment::Field(constrained_robot.clone()),
        ],
    );
    assert_signed_range_details(&create_upper, 51, "maximum", 50);

    let hydration_lower = ProjectedThing::try_new(
        &installed,
        person.clone(),
        "0x10".into(),
        person_fields(19),
        vec![],
    )
    .unwrap_err();
    assert_code(
        &hydration_lower,
        SdkDiagnosticCategory::Integrity,
        "range_constraint_violation",
    );
    assert_eq!(
        hydration_lower.path(),
        [
            SdkDiagnosticPathSegment::Type(person),
            SdkDiagnosticPathSegment::Field(constrained_person),
        ],
    );
    assert_signed_range_details(&hydration_lower, 19, "minimum", 20);

    let hydration_upper = ProjectedThing::try_new(
        &installed,
        robot.clone(),
        "0x20".into(),
        robot_fields(51),
        vec![],
    )
    .unwrap_err();
    assert_code(
        &hydration_upper,
        SdkDiagnosticCategory::Integrity,
        "range_constraint_violation",
    );
    assert_eq!(
        hydration_upper.path(),
        [
            SdkDiagnosticPathSegment::Type(robot),
            SdkDiagnosticPathSegment::Field(constrained_robot),
        ],
    );
    assert_signed_range_details(&hydration_upper, 51, "maximum", 50);
}

#[test]
fn keys_and_unique_facts_keep_their_local_preflight_boundaries() {
    let installed = installed(BindingTarget::Python, CONSTRAINT_SCHEMA);
    let person = type_id(TypeKind::Entity, "person");
    let identifier = field(&person, "identifier");
    let handle = field(&person, "handle");
    assert!(
        installed.projection().models()[&person]
            .query_tokens()
            .fields()[&handle]
            .is_unique(),
        "the provider-enforced unique fact must remain in the installed projection",
    );
    let missing_key = ProjectedCreate::try_new(
        &installed,
        person.clone(),
        vec![
            (
                field(&person, "nickname"),
                vec![string_value(&installed, "nickname", "Ada")],
            ),
            (
                field(&person, "val-constrained"),
                vec![long_value(&installed, "val-constrained", 20)],
            ),
        ],
        vec![],
    )
    .unwrap_err();
    assert_code(
        &missing_key,
        SdkDiagnosticCategory::InvalidInput,
        "missing_required_field",
    );
    assert_eq!(
        missing_key.path(),
        [
            SdkDiagnosticPathSegment::Type(person.clone()),
            SdkDiagnosticPathSegment::Field(identifier.clone()),
        ],
    );
    assert_eq!(
        missing_key
            .details()
            .get(&SdkDiagnosticName::new("actual_count").unwrap()),
        Some(&SdkDiagnosticDetailValue::Count(0)),
    );

    let person_fields = |identifier_value: &str, nickname: &str, handles: Vec<&str>| {
        vec![
            (
                identifier.clone(),
                vec![string_value(&installed, "identifier", identifier_value)],
            ),
            (
                field(&person, "nickname"),
                vec![string_value(&installed, "nickname", nickname)],
            ),
            (
                handle.clone(),
                handles
                    .into_iter()
                    .map(|value| string_value(&installed, "handle", value))
                    .collect(),
            ),
            (
                field(&person, "val-constrained"),
                vec![long_value(&installed, "val-constrained", 38)],
            ),
        ]
    };
    let first = ProjectedCreate::try_new(
        &installed,
        person.clone(),
        person_fields("data-ada", "Ada", vec!["shared", "shared"]),
        vec![],
    )
    .unwrap();
    assert_eq!(first.fields()[&handle].len(), 2);
    let second = ProjectedCreate::try_new(
        &installed,
        person.clone(),
        person_fields("data-dana", "Dana", vec!["shared"]),
        vec![],
    )
    .unwrap();
    assert_eq!(second.fields()[&handle].len(), 1);
}

#[test]
fn references_preserve_iid_precedence_and_exactly_one_key_fallback() {
    let installed = installed(BindingTarget::Python, SCHEMA);
    let person = type_id(TypeKind::Entity, "person");
    let identifier = field(&person, "identifier");
    let email = field(&person, "email");

    let by_key = ProjectedReference::try_new(
        &installed,
        person.clone(),
        None,
        vec![(
            identifier.clone(),
            string_value(&installed, "identifier", "ada"),
        )],
    )
    .unwrap();
    assert_eq!(by_key.iid(), None);
    assert_eq!(by_key.keys().len(), 1);

    let ambiguous = ProjectedReference::try_new(
        &installed,
        person.clone(),
        None,
        vec![
            (identifier, string_value(&installed, "identifier", "ada")),
            (email, string_value(&installed, "email", "ada@example.test")),
        ],
    )
    .unwrap_err();
    assert_code(
        &ambiguous,
        SdkDiagnosticCategory::InvalidInput,
        "ambiguous_reference_keys",
    );

    let invalid_iid =
        ProjectedReference::try_new(&installed, person, Some("person-1".into()), vec![])
            .unwrap_err();
    assert_code(
        &invalid_iid,
        SdkDiagnosticCategory::InvalidInput,
        "noncanonical_iid",
    );
}

#[test]
fn abstract_nominal_references_work_but_materialized_values_stay_concrete() {
    let installed = installed(BindingTarget::Python, SCHEMA);
    let actor = type_id(TypeKind::Entity, "actor");
    let actor_ref = iid_reference(&installed, actor.clone(), "0xa");
    assert_eq!(actor_ref.type_id(), &actor);

    let membership = type_id(TypeKind::Relation, "membership");
    let member = RoleId::new("membership", "member").unwrap();
    let create = ProjectedCreate::try_new(
        &installed,
        membership,
        vec![],
        vec![(member, vec![actor_ref.clone()])],
    )
    .unwrap();
    assert_eq!(create.roles().values().map(Vec::len).sum::<usize>(), 1);

    let abstract_create =
        ProjectedCreate::try_new(&installed, actor.clone(), vec![], vec![]).unwrap_err();
    assert_code(
        &abstract_create,
        SdkDiagnosticCategory::InvalidInput,
        "model_not_constructible",
    );
    let abstract_thing =
        ProjectedThing::try_new(&installed, actor.clone(), "0xa".into(), vec![], vec![])
            .unwrap_err();
    assert_code(
        &abstract_thing,
        SdkDiagnosticCategory::Integrity,
        "model_not_constructible",
    );
    let abstract_player = ProjectedRolePlayer::try_new(&installed, actor_ref).unwrap_err();
    assert_code(
        &abstract_player,
        SdkDiagnosticCategory::Integrity,
        "model_not_constructible",
    );
}

#[test]
fn relation_roles_enforce_domain_duplicates_and_cardinality() {
    let installed = installed(BindingTarget::Python, SCHEMA);
    let membership = type_id(TypeKind::Relation, "membership");
    let member = RoleId::new("membership", "member").unwrap();
    let observer = RoleId::new("membership", "observer").unwrap();
    let person = type_id(TypeKind::Entity, "person");
    let organization = type_id(TypeKind::Entity, "organization");

    let missing =
        ProjectedCreate::try_new(&installed, membership.clone(), vec![], vec![]).unwrap_err();
    assert_code(
        &missing,
        SdkDiagnosticCategory::InvalidInput,
        "missing_required_role",
    );

    let rejected = ProjectedCreate::try_new(
        &installed,
        membership.clone(),
        vec![],
        vec![(
            member.clone(),
            vec![iid_reference(&installed, organization.clone(), "0x1")],
        )],
    )
    .unwrap_err();
    assert_code(
        &rejected,
        SdkDiagnosticCategory::InvalidInput,
        "role_player_not_accepted",
    );

    let too_many = ProjectedCreate::try_new(
        &installed,
        membership.clone(),
        vec![],
        vec![
            (
                member.clone(),
                vec![iid_reference(&installed, person.clone(), "0x1")],
            ),
            (
                observer.clone(),
                vec![
                    iid_reference(&installed, person.clone(), "0x1"),
                    iid_reference(&installed, organization.clone(), "0x2"),
                    iid_reference(&installed, person, "0x3"),
                ],
            ),
        ],
    )
    .unwrap_err();
    assert_code(
        &too_many,
        SdkDiagnosticCategory::InvalidInput,
        "role_cardinality_violation",
    );

    let duplicate = ProjectedCreate::try_new(
        &installed,
        membership,
        vec![],
        vec![(member.clone(), vec![]), (member, vec![])],
    )
    .unwrap_err();
    assert_code(
        &duplicate,
        SdkDiagnosticCategory::InvalidInput,
        "duplicate_role_token",
    );
}

#[test]
fn ordered_distinct_construction_preserves_order_and_rejects_canonical_duplicates() {
    let installed = installed_ordered();
    let person = type_id(TypeKind::Entity, "person");
    let identifier = field(&person, "identifier");
    let alias = field(&person, "tag");
    let ordered_aliases = vec![
        string_value(&installed, "tag", "analyst"),
        string_value(&installed, "tag", "mathematician"),
    ];
    let create = ProjectedCreate::try_new(
        &installed,
        person.clone(),
        vec![
            (
                identifier.clone(),
                vec![string_value(&installed, "identifier", "data-ada")],
            ),
            (alias.clone(), ordered_aliases),
        ],
        vec![],
    )
    .unwrap();
    assert_eq!(
        projected_strings(&create.fields()[&alias]),
        ["analyst", "mathematician"],
    );

    let duplicate = ProjectedCreate::try_new(
        &installed,
        person.clone(),
        vec![
            (
                identifier,
                vec![string_value(&installed, "identifier", "data-ada")],
            ),
            (
                alias.clone(),
                vec![
                    string_value(&installed, "tag", "analyst"),
                    string_value(&installed, "tag", "analyst"),
                ],
            ),
        ],
        vec![],
    )
    .unwrap_err();
    assert_code(
        &duplicate,
        SdkDiagnosticCategory::InvalidInput,
        "ordered_distinct_duplicate",
    );
    assert_eq!(
        duplicate.path(),
        [
            SdkDiagnosticPathSegment::Type(person.clone()),
            SdkDiagnosticPathSegment::Field(alias),
            SdkDiagnosticPathSegment::Index(1),
        ],
    );
    assert_duplicate_details(&duplicate, 0, 1);
}

#[test]
fn ordered_distinct_players_preserve_order_and_reject_projected_identity_duplicates() {
    let installed = installed_ordered();
    let collaboration = type_id(TypeKind::Relation, "collaboration");
    let participant = RoleId::new("collaboration", "participant").unwrap();
    let person = type_id(TypeKind::Entity, "person");
    let reference = |key: &str| {
        ProjectedReference::try_new(
            &installed,
            person.clone(),
            None,
            vec![(
                field(&person, "identifier"),
                string_value(&installed, "identifier", key),
            )],
        )
        .unwrap()
    };
    let create = ProjectedCreate::try_new(
        &installed,
        collaboration.clone(),
        vec![],
        vec![(
            participant.clone(),
            vec![reference("data-ada"), reference("data-dana")],
        )],
    )
    .unwrap();
    let keys = create.roles()[&participant]
        .iter()
        .map(|reference| {
            let value = reference.keys().values().next().unwrap();
            projected_strings(std::slice::from_ref(value))[0]
        })
        .collect::<Vec<_>>();
    assert_eq!(keys, ["data-ada", "data-dana"]);

    let duplicate = ProjectedCreate::try_new(
        &installed,
        collaboration.clone(),
        vec![],
        vec![(
            participant.clone(),
            vec![reference("data-ada"), reference("data-ada")],
        )],
    )
    .unwrap_err();
    assert_code(
        &duplicate,
        SdkDiagnosticCategory::InvalidInput,
        "ordered_distinct_duplicate",
    );
    assert_eq!(
        duplicate.path(),
        [
            SdkDiagnosticPathSegment::Type(collaboration),
            SdkDiagnosticPathSegment::Role(participant),
            SdkDiagnosticPathSegment::Index(1),
            SdkDiagnosticPathSegment::Type(person.clone()),
        ],
    );
    assert_duplicate_details(&duplicate, 0, 1);

    let hydrated_player = |iid: &str| {
        ProjectedRolePlayer::try_new(
            &installed,
            ProjectedReference::try_new(&installed, person.clone(), Some(iid.to_owned()), vec![])
                .unwrap(),
        )
        .unwrap()
    };
    let hydrated = ProjectedThing::try_new(
        &installed,
        type_id(TypeKind::Relation, "collaboration"),
        "0x20".into(),
        vec![],
        vec![(
            RoleId::new("collaboration", "participant").unwrap(),
            vec![hydrated_player("0x10"), hydrated_player("0x11")],
        )],
    )
    .unwrap();
    assert_eq!(
        hydrated.roles()[&RoleId::new("collaboration", "participant").unwrap()]
            .iter()
            .map(ProjectedRolePlayer::iid)
            .collect::<Vec<_>>(),
        ["0x10", "0x11"],
    );

    let duplicate = ProjectedThing::try_new(
        &installed,
        type_id(TypeKind::Relation, "collaboration"),
        "0x21".into(),
        vec![],
        vec![(
            RoleId::new("collaboration", "participant").unwrap(),
            vec![hydrated_player("0x10"), hydrated_player("0x10")],
        )],
    )
    .unwrap_err();
    assert_code(
        &duplicate,
        SdkDiagnosticCategory::Integrity,
        "ordered_distinct_duplicate",
    );
    assert_duplicate_details(&duplicate, 0, 1);
}

#[test]
fn ordered_distinct_hydration_preserves_order_and_classifies_duplicates_as_integrity() {
    let installed = installed_ordered();
    let person = type_id(TypeKind::Entity, "person");
    let identifier = field(&person, "identifier");
    let alias = field(&person, "tag");
    let thing = ProjectedThing::try_new(
        &installed,
        person.clone(),
        "0x10".into(),
        vec![
            (
                identifier.clone(),
                vec![string_value(&installed, "identifier", "data-ada")],
            ),
            (
                alias.clone(),
                vec![
                    string_value(&installed, "tag", "analyst"),
                    string_value(&installed, "tag", "mathematician"),
                ],
            ),
        ],
        vec![],
    )
    .unwrap();
    assert_eq!(
        projected_strings(&thing.fields()[&alias]),
        ["analyst", "mathematician"],
    );

    let duplicate = ProjectedThing::try_new(
        &installed,
        person.clone(),
        "0x11".into(),
        vec![
            (
                identifier,
                vec![string_value(&installed, "identifier", "data-ada")],
            ),
            (
                alias.clone(),
                vec![
                    string_value(&installed, "tag", "analyst"),
                    string_value(&installed, "tag", "analyst"),
                ],
            ),
        ],
        vec![],
    )
    .unwrap_err();
    assert_code(
        &duplicate,
        SdkDiagnosticCategory::Integrity,
        "ordered_distinct_duplicate",
    );
    assert_eq!(
        duplicate.path(),
        [
            SdkDiagnosticPathSegment::Type(person),
            SdkDiagnosticPathSegment::Field(alias),
            SdkDiagnosticPathSegment::Index(1),
        ],
    );
    assert_duplicate_details(&duplicate, 0, 1);
}

#[test]
fn unordered_collections_retain_multiplicity_without_a_runtime_order_contract() {
    let installed = installed(BindingTarget::Python, SCHEMA);
    let person = type_id(TypeKind::Entity, "person");
    let nickname = field(&person, "nickname");
    let mut fields = person_fields(&installed);
    fields.push((
        nickname.clone(),
        vec![
            string_value(&installed, "nickname", "nickname-same"),
            string_value(&installed, "nickname", "nickname-same"),
        ],
    ));
    let create = ProjectedCreate::try_new(&installed, person, fields, vec![]).unwrap();
    assert_eq!(create.fields()[&nickname].len(), 2);
}

#[test]
fn hydrated_things_are_complete_nonrecursive_and_classify_result_failures_as_integrity() {
    let installed = installed(BindingTarget::Python, SCHEMA);
    let person = type_id(TypeKind::Entity, "person");
    let person_thing = ProjectedThing::try_new(
        &installed,
        person.clone(),
        "0x10".into(),
        person_fields(&installed),
        vec![],
    )
    .unwrap();
    assert_eq!(person_thing.iid(), "0x10");
    assert!(person_thing.fields()[&field(&person, "nickname")].is_empty());

    let missing_field = ProjectedThing::try_new(
        &installed,
        person.clone(),
        "0x11".into(),
        vec![person_fields(&installed).remove(0)],
        vec![],
    )
    .unwrap_err();
    assert_code(
        &missing_field,
        SdkDiagnosticCategory::Integrity,
        "missing_required_field",
    );

    let duplicate_field_id = field(&person, "identifier");
    let duplicate_field = ProjectedThing::try_new(
        &installed,
        person.clone(),
        "0x12".into(),
        vec![
            (
                duplicate_field_id.clone(),
                vec![string_value(&installed, "identifier", "ada")],
            ),
            (
                duplicate_field_id,
                vec![string_value(&installed, "identifier", "grace")],
            ),
        ],
        vec![],
    )
    .unwrap_err();
    assert_code(
        &duplicate_field,
        SdkDiagnosticCategory::Integrity,
        "duplicate_field_token",
    );

    let key_only = ProjectedReference::try_new(
        &installed,
        person.clone(),
        None,
        vec![(
            field(&person, "identifier"),
            string_value(&installed, "identifier", "ada"),
        )],
    )
    .unwrap();
    let missing_player_iid = ProjectedRolePlayer::try_new(&installed, key_only).unwrap_err();
    assert_code(
        &missing_player_iid,
        SdkDiagnosticCategory::Integrity,
        "hydrated_player_iid_missing",
    );

    let player =
        ProjectedRolePlayer::try_new(&installed, iid_reference(&installed, person, "0x10"))
            .unwrap();
    let membership = type_id(TypeKind::Relation, "membership");
    let member = RoleId::new("membership", "member").unwrap();
    let relation = ProjectedThing::try_new(
        &installed,
        membership.clone(),
        "0x20".into(),
        vec![],
        vec![(member, vec![player])],
    )
    .unwrap();
    assert_eq!(relation.roles().values().map(Vec::len).sum::<usize>(), 1);

    let duplicate_role = RoleId::new("membership", "member").unwrap();
    let duplicate_role = ProjectedThing::try_new(
        &installed,
        membership.clone(),
        "0x21".into(),
        vec![],
        vec![(duplicate_role.clone(), vec![]), (duplicate_role, vec![])],
    )
    .unwrap_err();
    assert_code(
        &duplicate_role,
        SdkDiagnosticCategory::Integrity,
        "duplicate_role_token",
    );

    let account = type_id(TypeKind::Entity, "account");
    let nominal_account = iid_reference(&installed, account, "0x30");
    let nominal_create = ProjectedCreate::try_new(
        &installed,
        membership.clone(),
        vec![],
        vec![
            (
                RoleId::new("membership", "member").unwrap(),
                vec![iid_reference(
                    &installed,
                    type_id(TypeKind::Entity, "person"),
                    "0x10",
                )],
            ),
            (
                RoleId::new("membership", "premium").unwrap(),
                vec![nominal_account.clone()],
            ),
        ],
    )
    .unwrap();
    assert_eq!(
        nominal_create.roles()[&RoleId::new("membership", "premium").unwrap()].len(),
        1
    );

    let concrete_account = ProjectedRolePlayer::try_new(&installed, nominal_account).unwrap();
    let exact_hydration = ProjectedThing::try_new(
        &installed,
        membership,
        "0x31".into(),
        vec![],
        vec![
            (
                RoleId::new("membership", "member").unwrap(),
                vec![
                    ProjectedRolePlayer::try_new(
                        &installed,
                        iid_reference(&installed, type_id(TypeKind::Entity, "person"), "0x10"),
                    )
                    .unwrap(),
                ],
            ),
            (
                RoleId::new("membership", "premium").unwrap(),
                vec![concrete_account],
            ),
        ],
    )
    .unwrap_err();
    assert_code(
        &exact_hydration,
        SdkDiagnosticCategory::Integrity,
        "hydrated_role_player_not_accepted",
    );
}

#[test]
fn brands_fence_binding_targets_and_semantic_schemas() {
    let python = installed(BindingTarget::Python, SCHEMA);
    let typescript = installed(BindingTarget::TypeScript, SCHEMA);
    let person = type_id(TypeKind::Entity, "person");
    let foreign_target = string_value(&typescript, "identifier", "ada");
    let target_error = ProjectedCreate::try_new(
        &python,
        person.clone(),
        vec![
            (field(&person, "identifier"), vec![foreign_target]),
            (
                field(&person, "email"),
                vec![string_value(&python, "email", "ada@example.test")],
            ),
        ],
        vec![],
    )
    .unwrap_err();
    assert_code(
        &target_error,
        SdkDiagnosticCategory::Integrity,
        "binding_target_brand_mismatch",
    );

    let changed_schema = SCHEMA.replace("entities:\n", "entities:\n  robot: {}\n");
    let changed = installed(BindingTarget::Python, &changed_schema);
    let foreign_semantic = string_value(&changed, "identifier", "ada");
    let semantic_error = ProjectedCreate::try_new(
        &python,
        person.clone(),
        vec![
            (field(&person, "identifier"), vec![foreign_semantic]),
            (
                field(&person, "email"),
                vec![string_value(&python, "email", "ada@example.test")],
            ),
        ],
        vec![],
    )
    .unwrap_err();
    assert_code(
        &semantic_error,
        SdkDiagnosticCategory::Integrity,
        "semantic_brand_mismatch",
    );

    let first_c = installed_c(SCHEMA, "first");
    let second_c = installed_c(SCHEMA, "second");
    let c_person = type_id(TypeKind::Entity, "person");
    let foreign_projection = string_value(&second_c, "identifier", "ada");
    let projection_error = ProjectedCreate::try_new(
        &first_c,
        c_person.clone(),
        vec![
            (field(&c_person, "identifier"), vec![foreign_projection]),
            (
                field(&c_person, "email"),
                vec![string_value(&first_c, "email", "ada@example.test")],
            ),
        ],
        vec![],
    )
    .unwrap_err();
    assert_code(
        &projection_error,
        SdkDiagnosticCategory::Integrity,
        "projection_brand_mismatch",
    );
}

#[test]
fn total_payload_bytes_are_bounded_before_a_create_value_is_returned() {
    let installed = installed(BindingTarget::Python, SCHEMA);
    let person = type_id(TypeKind::Entity, "person");
    let large = format!(
        "nickname-{}",
        "x".repeat(MAX_CANONICAL_STRING_BYTES - "nickname-".len())
    );
    let aliases = (0..17)
        .map(|_| string_value(&installed, "nickname", large.clone()))
        .collect::<Vec<_>>();
    let mut fields = person_fields(&installed);
    fields.push((field(&person, "nickname"), aliases));
    let diagnostic = ProjectedCreate::try_new(&installed, person, fields, vec![]).unwrap_err();
    assert_code(
        &diagnostic,
        SdkDiagnosticCategory::ResourceLimit,
        "projected_model_byte_limit_exceeded",
    );
}
