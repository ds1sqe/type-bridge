//! Ignored exact TypeDB 3.12.1 journey for the common projected batch executor.

use crate::common::rust_binding::{server_supports_v2_conformance, setup_db, unique_label};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, ProjectionHandler};
use type_bridge_contract::schema::{DocumentId, OwnsFactId};
use type_bridge_contract::temporal::{CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration};
use type_bridge_contract::value::{CanonicalDouble, CanonicalString, CanonicalValue, DecimalValue};
use type_bridge_orm::session::backend::QueryResult;
use type_bridge_orm::{
    AnswerCancellation, InstalledRuntimeProjection, ProjectedAttributeValue, ProjectedBatch,
    ProjectedBatchExecutor, ProjectedBatchInvocationControl, ProjectedBatchOperation,
    ProjectedBatchResult, ProjectedBatchRow, ProjectedCreate, ProjectedReference, ProjectedThing,
    QueryExecutionResourceLimits, TxType,
};
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};

struct LiveIds {
    person: TypeId,
    membership: TypeId,
    container: TypeId,
    key: AttributeId,
    string: AttributeId,
    boolean: AttributeId,
    integer: AttributeId,
    double: AttributeId,
    date: AttributeId,
    datetime: AttributeId,
    datetime_tz: AttributeId,
    decimal: AttributeId,
    duration: AttributeId,
}

fn field(owner: &TypeId, attribute: &AttributeId) -> OwnsFactId {
    OwnsFactId::new(owner.clone(), attribute.clone()).unwrap()
}

fn scalar(
    installed: &InstalledRuntimeProjection,
    attribute: &AttributeId,
    value: CanonicalValue,
) -> ProjectedAttributeValue {
    ProjectedAttributeValue::try_new(
        installed,
        TypeId::new(TypeKind::Attribute, attribute.label().as_str()).unwrap(),
        value,
    )
    .unwrap()
}

fn string_scalar(
    installed: &InstalledRuntimeProjection,
    attribute: &AttributeId,
    value: &str,
) -> ProjectedAttributeValue {
    scalar(
        installed,
        attribute,
        CanonicalValue::String(CanonicalString::new(value).unwrap()),
    )
}

fn person_create(
    installed: &InstalledRuntimeProjection,
    ids: &LiveIds,
    key_value: &str,
    all_domains: bool,
) -> ProjectedCreate {
    let mut fields = vec![(
        field(&ids.person, &ids.key),
        vec![string_scalar(installed, &ids.key, key_value)],
    )];
    if all_domains {
        let local: CanonicalDateTime = "2026-08-14T10:30:00".parse().unwrap();
        fields.extend([
            (
                field(&ids.person, &ids.string),
                vec![string_scalar(installed, &ids.string, "text")],
            ),
            (
                field(&ids.person, &ids.boolean),
                vec![scalar(
                    installed,
                    &ids.boolean,
                    CanonicalValue::Boolean(true),
                )],
            ),
            (
                field(&ids.person, &ids.integer),
                vec![scalar(installed, &ids.integer, CanonicalValue::Long(42))],
            ),
            (
                field(&ids.person, &ids.double),
                vec![scalar(
                    installed,
                    &ids.double,
                    CanonicalValue::Double(CanonicalDouble::new(1.5).unwrap()),
                )],
            ),
            (
                field(&ids.person, &ids.date),
                vec![scalar(
                    installed,
                    &ids.date,
                    CanonicalValue::Date("2026-08-14".parse().unwrap()),
                )],
            ),
            (
                field(&ids.person, &ids.datetime),
                vec![scalar(
                    installed,
                    &ids.datetime,
                    CanonicalValue::DateTime(local),
                )],
            ),
            (
                field(&ids.person, &ids.datetime_tz),
                vec![scalar(
                    installed,
                    &ids.datetime_tz,
                    CanonicalValue::DateTimeTz(
                        CanonicalDateTimeTz::new_named_resolved(local, "Europe/Amsterdam", 7_200)
                            .unwrap(),
                    ),
                )],
            ),
            (
                field(&ids.person, &ids.decimal),
                vec![scalar(
                    installed,
                    &ids.decimal,
                    CanonicalValue::Decimal(DecimalValue::new("12.30dec").unwrap()),
                )],
            ),
            (
                field(&ids.person, &ids.duration),
                vec![scalar(
                    installed,
                    &ids.duration,
                    CanonicalValue::Duration(CanonicalDuration::new(false, 1, 2, 3, 4).unwrap()),
                )],
            ),
        ]);
    }
    ProjectedCreate::try_new(installed, ids.person.clone(), fields, vec![]).unwrap()
}

fn relation_create(
    installed: &InstalledRuntimeProjection,
    relation: &TypeId,
    role: RoleId,
    key: &AttributeId,
    key_value: &str,
    players: Vec<ProjectedReference>,
) -> ProjectedCreate {
    ProjectedCreate::try_new(
        installed,
        relation.clone(),
        vec![(
            field(relation, key),
            vec![string_scalar(installed, key, key_value)],
        )],
        vec![(role, players)],
    )
    .unwrap()
}

fn reference(
    installed: &InstalledRuntimeProjection,
    type_id: &TypeId,
    iid: &str,
) -> ProjectedReference {
    ProjectedReference::try_new(installed, type_id.clone(), Some(iid.to_owned()), vec![]).unwrap()
}

fn key_reference(
    installed: &InstalledRuntimeProjection,
    type_id: &TypeId,
    key: &AttributeId,
    value: &str,
) -> ProjectedReference {
    ProjectedReference::try_new(
        installed,
        type_id.clone(),
        None,
        vec![(field(type_id, key), string_scalar(installed, key, value))],
    )
    .unwrap()
}

fn control() -> ProjectedBatchInvocationControl {
    ProjectedBatchInvocationControl::capture(
        QueryExecutionResourceLimits::default(),
        AnswerCancellation::default(),
    )
}

fn things(result: ProjectedBatchResult) -> Vec<type_bridge_orm::ProjectedThing> {
    match result {
        ProjectedBatchResult::Things(things) => things,
        ProjectedBatchResult::Deleted => panic!("write unexpectedly returned delete result"),
        _ => panic!("unknown projected batch result"),
    }
}

fn field_value<'thing>(
    thing: &'thing ProjectedThing,
    owner: &TypeId,
    attribute: &AttributeId,
) -> &'thing CanonicalValue {
    let values = thing
        .fields()
        .get(&field(owner, attribute))
        .expect("projected field slot");
    assert_eq!(values.len(), 1, "expected one projected field value");
    values[0].value()
}

fn assert_field_empty(thing: &ProjectedThing, owner: &TypeId, attribute: &AttributeId) {
    assert!(
        thing
            .fields()
            .get(&field(owner, attribute))
            .expect("projected field slot")
            .is_empty(),
        "expected projected field to be empty"
    );
}

fn assert_role_players(thing: &ProjectedThing, role: &RoleId, expected: &[(&TypeId, &str)]) {
    let mut actual = thing
        .roles()
        .get(role)
        .expect("projected role slot")
        .iter()
        .map(|player| (player.type_id().clone(), player.iid().to_owned()))
        .collect::<Vec<_>>();
    let mut expected = expected
        .iter()
        .map(|(type_id, iid)| ((*type_id).clone(), (*iid).to_owned()))
        .collect::<Vec<_>>();
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected);
}

fn installed(prefix: &str) -> (InstalledRuntimeProjection, LiveIds) {
    let label = |suffix: &str| format!("{prefix}-{suffix}");
    let yaml = format!(
        r#"format: typebridge.schema/v2
attributes:
  {key}: {{ value: string }}
  {string}: {{ value: string }}
  {boolean}: {{ value: boolean }}
  {integer}: {{ value: integer }}
  {double}: {{ value: double }}
  {date}: {{ value: date }}
  {datetime}: {{ value: datetime }}
  {datetime_tz}: {{ value: datetime-tz }}
  {decimal}: {{ value: decimal }}
  {duration}: {{ value: duration }}
entities:
  {person}:
    owns:
      {key}: {{ key: true }}
      {string}: {{ card: {{ min: 0 }} }}
      {boolean}: {{ card: {{ min: 0 }} }}
      {integer}: {{ card: {{ min: 0 }} }}
      {double}: {{ card: {{ min: 0 }} }}
      {date}: {{ card: {{ min: 0 }} }}
      {datetime}: {{ card: {{ min: 0 }} }}
      {datetime_tz}: {{ card: {{ min: 0 }} }}
      {decimal}: {{ card: {{ min: 0 }} }}
      {duration}: {{ card: {{ min: 0 }} }}
relations:
  {membership}:
    owns:
      {key}: {{ key: true }}
    relates:
      member: {{ card: {{ min: 0, max: 4 }} }}
  {container}:
    owns:
      {key}: {{ key: true }}
    relates:
      item: {{ card: {{ min: 0, max: 4 }} }}
plays:
  {person}:
    {membership}: [member]
    {container}: [item]
  {membership}:
    {container}: [item]
"#,
        key = label("key"),
        string = label("string"),
        boolean = label("boolean"),
        integer = label("integer"),
        double = label("double"),
        date = label("date"),
        datetime = label("datetime"),
        datetime_tz = label("datetime-tz"),
        decimal = label("decimal"),
        duration = label("duration"),
        person = label("person"),
        membership = label("membership"),
        container = label("container"),
    );
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("projected-batch-live.yaml").unwrap(),
        yaml.as_str(),
    )])
    .unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
    )
    .unwrap();
    let installed = InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &[ProjectionHandler::python_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    let ids = LiveIds {
        person: TypeId::new(TypeKind::Entity, label("person")).unwrap(),
        membership: TypeId::new(TypeKind::Relation, label("membership")).unwrap(),
        container: TypeId::new(TypeKind::Relation, label("container")).unwrap(),
        key: AttributeId::new(label("key")).unwrap(),
        string: AttributeId::new(label("string")).unwrap(),
        boolean: AttributeId::new(label("boolean")).unwrap(),
        integer: AttributeId::new(label("integer")).unwrap(),
        double: AttributeId::new(label("double")).unwrap(),
        date: AttributeId::new(label("date")).unwrap(),
        datetime: AttributeId::new(label("datetime")).unwrap(),
        datetime_tz: AttributeId::new(label("datetime-tz")).unwrap(),
        decimal: AttributeId::new(label("decimal")).unwrap(),
        duration: AttributeId::new(label("duration")).unwrap(),
    };
    (installed, ids)
}

#[tokio::test]
#[ignore = "requires the exact TypeDB CE 3.12.1 live fixture"]
async fn projected_batch_all_operations_domains_roles_and_mixed_players_live() {
    let _guard = crate::common::integration_test_guard().await;
    let database = setup_db().await;
    if !server_supports_v2_conformance(&database) {
        database.delete_database().await.unwrap();
        return;
    }
    let prefix = unique_label("projected-batch-live");
    let (installed, ids) = installed(&prefix);
    let schema = format!(
        r#"define
attribute {key}, value string;
attribute {string}, value string;
attribute {boolean}, value boolean;
attribute {integer}, value integer;
attribute {double}, value double;
attribute {date}, value date;
attribute {datetime}, value datetime;
attribute {datetime_tz}, value datetime-tz;
attribute {decimal}, value decimal;
attribute {duration}, value duration;
entity {person},
  owns {key} @key,
  owns {string} @card(0..1),
  owns {boolean} @card(0..1),
  owns {integer} @card(0..1),
  owns {double} @card(0..1),
  owns {date} @card(0..1),
  owns {datetime} @card(0..1),
  owns {datetime_tz} @card(0..1),
  owns {decimal} @card(0..1),
  owns {duration} @card(0..1),
  plays {membership}:member,
  plays {container}:item;
relation {membership},
  owns {key} @key,
  relates member @card(0..4),
  plays {container}:item;
relation {container},
  owns {key} @key,
  relates item @card(0..4);
"#,
        key = ids.key.label(),
        string = ids.string.label(),
        boolean = ids.boolean.label(),
        integer = ids.integer.label(),
        double = ids.double.label(),
        date = ids.date.label(),
        datetime = ids.datetime.label(),
        datetime_tz = ids.datetime_tz.label(),
        decimal = ids.decimal.label(),
        duration = ids.duration.label(),
        person = ids.person.label(),
        membership = ids.membership.label(),
        container = ids.container.label(),
    );
    database.execute_raw(&schema, TxType::Schema).await.unwrap();
    let executor = ProjectedBatchExecutor::new(&installed);

    let inserted = things(
        executor
            .execute(
                &database,
                &ProjectedBatch::try_new(
                    &installed,
                    ids.person.clone(),
                    ProjectedBatchOperation::Insert,
                    vec![
                        ProjectedBatchRow::Create(person_create(&installed, &ids, "p-0", true)),
                        ProjectedBatchRow::Create(person_create(&installed, &ids, "p-1", false)),
                    ],
                )
                .unwrap(),
                control(),
            )
            .await
            .unwrap(),
    );
    assert_eq!(inserted.len(), 2);
    let local: CanonicalDateTime = "2026-08-14T10:30:00".parse().unwrap();
    assert_eq!(
        field_value(&inserted[0], &ids.person, &ids.key),
        &CanonicalValue::String(CanonicalString::new("p-0").unwrap())
    );
    assert_eq!(
        field_value(&inserted[0], &ids.person, &ids.string),
        &CanonicalValue::String(CanonicalString::new("text").unwrap())
    );
    assert_eq!(
        field_value(&inserted[0], &ids.person, &ids.boolean),
        &CanonicalValue::Boolean(true)
    );
    assert_eq!(
        field_value(&inserted[0], &ids.person, &ids.integer),
        &CanonicalValue::Long(42)
    );
    assert_eq!(
        field_value(&inserted[0], &ids.person, &ids.double),
        &CanonicalValue::Double(CanonicalDouble::new(1.5).unwrap())
    );
    assert_eq!(
        field_value(&inserted[0], &ids.person, &ids.date),
        &CanonicalValue::Date("2026-08-14".parse().unwrap())
    );
    assert_eq!(
        field_value(&inserted[0], &ids.person, &ids.datetime),
        &CanonicalValue::DateTime(local)
    );
    assert_eq!(
        field_value(&inserted[0], &ids.person, &ids.datetime_tz),
        &CanonicalValue::DateTimeTz(
            CanonicalDateTimeTz::new_named_resolved(local, "Europe/Amsterdam", 7_200).unwrap()
        )
    );
    assert_eq!(
        field_value(&inserted[0], &ids.person, &ids.decimal),
        &CanonicalValue::Decimal(DecimalValue::new("12.30dec").unwrap())
    );
    assert_eq!(
        field_value(&inserted[0], &ids.person, &ids.duration),
        &CanonicalValue::Duration(CanonicalDuration::new(false, 1, 2, 3, 4).unwrap())
    );
    let first_person = inserted[0].iid().to_owned();
    let first_person_upper = format!("0x{}", first_person[2..].to_ascii_uppercase());
    let second_person = inserted[1].iid().to_owned();

    let put_people = things(
        executor
            .execute(
                &database,
                &ProjectedBatch::try_new(
                    &installed,
                    ids.person.clone(),
                    ProjectedBatchOperation::Put,
                    vec![
                        ProjectedBatchRow::Create(person_create(&installed, &ids, "p-0", false)),
                        ProjectedBatchRow::Create(person_create(&installed, &ids, "p-2", false)),
                    ],
                )
                .unwrap(),
                control(),
            )
            .await
            .unwrap(),
    );
    assert_eq!(put_people.len(), 2);
    assert_eq!(put_people[0].iid(), first_person);
    assert_eq!(
        field_value(&put_people[0], &ids.person, &ids.key),
        &CanonicalValue::String(CanonicalString::new("p-0").unwrap())
    );
    for attribute in [
        &ids.string,
        &ids.boolean,
        &ids.integer,
        &ids.double,
        &ids.date,
        &ids.datetime,
        &ids.datetime_tz,
        &ids.decimal,
        &ids.duration,
    ] {
        assert_field_empty(&put_people[0], &ids.person, attribute);
    }
    assert_ne!(put_people[1].iid(), first_person);
    assert_ne!(put_people[1].iid(), second_person);
    assert_eq!(
        field_value(&put_people[1], &ids.person, &ids.key),
        &CanonicalValue::String(CanonicalString::new("p-2").unwrap())
    );
    for attribute in [
        &ids.string,
        &ids.boolean,
        &ids.integer,
        &ids.double,
        &ids.date,
        &ids.datetime,
        &ids.datetime_tz,
        &ids.decimal,
        &ids.duration,
    ] {
        assert_field_empty(&put_people[1], &ids.person, attribute);
    }

    let updated = things(
        executor
            .execute(
                &database,
                &ProjectedBatch::try_new(
                    &installed,
                    ids.person.clone(),
                    ProjectedBatchOperation::Update,
                    vec![ProjectedBatchRow::Update {
                        iid: second_person.clone(),
                        replacement: person_create(&installed, &ids, "p-1-updated", true),
                    }],
                )
                .unwrap(),
                control(),
            )
            .await
            .unwrap(),
    );
    assert_eq!(updated[0].iid(), second_person);
    assert_eq!(
        field_value(&updated[0], &ids.person, &ids.key),
        &CanonicalValue::String(CanonicalString::new("p-1-updated").unwrap())
    );
    assert_eq!(
        field_value(&updated[0], &ids.person, &ids.datetime_tz),
        &CanonicalValue::DateTimeTz(
            CanonicalDateTimeTz::new_named_resolved(local, "Europe/Amsterdam", 7_200).unwrap()
        )
    );

    let member_role = RoleId::new(ids.membership.label().as_str(), "member").unwrap();
    let inserted_membership = things(
        executor
            .execute(
                &database,
                &ProjectedBatch::try_new(
                    &installed,
                    ids.membership.clone(),
                    ProjectedBatchOperation::Insert,
                    vec![ProjectedBatchRow::Create(relation_create(
                        &installed,
                        &ids.membership,
                        member_role.clone(),
                        &ids.key,
                        "m-0",
                        vec![key_reference(&installed, &ids.person, &ids.key, "p-0")],
                    ))],
                )
                .unwrap(),
                control(),
            )
            .await
            .unwrap(),
    );
    let membership_iid = inserted_membership[0].iid().to_owned();
    assert_eq!(
        field_value(&inserted_membership[0], &ids.membership, &ids.key),
        &CanonicalValue::String(CanonicalString::new("m-0").unwrap())
    );
    assert_role_players(
        &inserted_membership[0],
        &member_role,
        &[(&ids.person, &first_person)],
    );

    let put_memberships = things(
        executor
            .execute(
                &database,
                &ProjectedBatch::try_new(
                    &installed,
                    ids.membership.clone(),
                    ProjectedBatchOperation::Put,
                    vec![
                        ProjectedBatchRow::Create(relation_create(
                            &installed,
                            &ids.membership,
                            member_role.clone(),
                            &ids.key,
                            "m-0",
                            vec![reference(&installed, &ids.person, &second_person)],
                        )),
                        ProjectedBatchRow::Create(relation_create(
                            &installed,
                            &ids.membership,
                            member_role.clone(),
                            &ids.key,
                            "m-1",
                            vec![reference(&installed, &ids.person, &first_person)],
                        )),
                    ],
                )
                .unwrap(),
                control(),
            )
            .await
            .unwrap(),
    );
    assert_eq!(put_memberships.len(), 2);
    assert_eq!(put_memberships[0].iid(), membership_iid);
    assert_eq!(
        field_value(&put_memberships[0], &ids.membership, &ids.key),
        &CanonicalValue::String(CanonicalString::new("m-0").unwrap())
    );
    assert_role_players(
        &put_memberships[0],
        &member_role,
        &[(&ids.person, &second_person)],
    );
    let second_membership = put_memberships[1].iid().to_owned();
    assert_ne!(second_membership, membership_iid);
    assert_eq!(
        field_value(&put_memberships[1], &ids.membership, &ids.key),
        &CanonicalValue::String(CanonicalString::new("m-1").unwrap())
    );
    assert_role_players(
        &put_memberships[1],
        &member_role,
        &[(&ids.person, &first_person)],
    );

    let updated_membership = things(
        executor
            .execute(
                &database,
                &ProjectedBatch::try_new(
                    &installed,
                    ids.membership.clone(),
                    ProjectedBatchOperation::Update,
                    vec![ProjectedBatchRow::Update {
                        iid: membership_iid.clone(),
                        replacement: relation_create(
                            &installed,
                            &ids.membership,
                            member_role.clone(),
                            &ids.key,
                            "m-0-updated",
                            vec![reference(&installed, &ids.person, &first_person)],
                        ),
                    }],
                )
                .unwrap(),
                control(),
            )
            .await
            .unwrap(),
    );
    assert_eq!(updated_membership[0].iid(), membership_iid);
    assert_eq!(
        field_value(&updated_membership[0], &ids.membership, &ids.key),
        &CanonicalValue::String(CanonicalString::new("m-0-updated").unwrap())
    );
    assert_role_players(
        &updated_membership[0],
        &member_role,
        &[(&ids.person, &first_person)],
    );

    let item_role = RoleId::new(ids.container.label().as_str(), "item").unwrap();
    let containers = things(
        executor
            .execute(
                &database,
                &ProjectedBatch::try_new(
                    &installed,
                    ids.container.clone(),
                    ProjectedBatchOperation::Insert,
                    vec![ProjectedBatchRow::Create(relation_create(
                        &installed,
                        &ids.container,
                        item_role.clone(),
                        &ids.key,
                        "c-0",
                        vec![
                            reference(&installed, &ids.person, &first_person_upper),
                            reference(&installed, &ids.membership, &membership_iid),
                        ],
                    ))],
                )
                .unwrap(),
                control(),
            )
            .await
            .unwrap(),
    );
    assert_eq!(
        field_value(&containers[0], &ids.container, &ids.key),
        &CanonicalValue::String(CanonicalString::new("c-0").unwrap())
    );
    assert_role_players(
        &containers[0],
        &item_role,
        &[
            (&ids.person, &first_person),
            (&ids.membership, &membership_iid),
        ],
    );

    for (model, present) in [
        (ids.person.clone(), first_person),
        (ids.membership.clone(), membership_iid),
    ] {
        assert_eq!(
            executor
                .execute(
                    &database,
                    &ProjectedBatch::try_new(
                        &installed,
                        model.clone(),
                        ProjectedBatchOperation::Delete,
                        vec![
                            ProjectedBatchRow::Delete {
                                iid: present.clone(),
                            },
                            ProjectedBatchRow::Delete {
                                iid: "0xffffffffffffffffffffffffffffffff".to_owned(),
                            },
                        ],
                    )
                    .unwrap(),
                    control(),
                )
                .await
                .unwrap(),
            ProjectedBatchResult::Deleted
        );
        let probe = database
            .execute_raw(
                &format!(
                    "match\n$thing isa! {};\nlet $actual-iid = iid($thing);\n$actual-iid == \"{}\";\nselect $thing;",
                    model.label(),
                    present.to_ascii_lowercase()
                ),
                TxType::Read,
            )
            .await
            .expect("delete absence probe should execute");
        assert!(
            matches!(probe, QueryResult::Rows(rows) if rows.is_empty()),
            "present projected batch delete target must be absent"
        );
    }
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        database.delete_database(),
    )
    .await
    .expect("projected batch fixture cleanup timed out")
    .expect("projected batch fixture database should be deleted");
    assert!(!database.database_exists().await.unwrap());
}
