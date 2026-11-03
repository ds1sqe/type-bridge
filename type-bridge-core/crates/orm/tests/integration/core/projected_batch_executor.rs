//! Ignored exact TypeDB 3.12.1 journey for the common projected batch executor.

use std::sync::{Arc, Mutex};

use crate::common::rust_binding::{server_supports_v2_conformance, setup_db, unique_label};
use crate::common::typedb::connect_options_from_env;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, ProjectionHandler};
use type_bridge_contract::schema::{DocumentId, OwnsFactId};
use type_bridge_contract::temporal::{CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration};
use type_bridge_contract::value::{CanonicalDouble, CanonicalString, CanonicalValue, DecimalValue};
use type_bridge_core_lib::ast::{
    TypedFetchRows, TypedHydrateThings, TypedPageRematch, TypedRootScan,
};
use type_bridge_orm::session::backend::{
    AnswerConsumer, AnswerLimits, BoundedAnswerLimits, BoundedAnswerStats, BoxFuture,
    DriverBackend, GivenRowsSpec, QueryResult, SchemaFencedReadTransaction, TransactionOps,
};
use type_bridge_orm::session::real_driver::RealBackend;
use type_bridge_orm::session::staged::TransactionOptions;
use type_bridge_orm::{
    AnswerCancellation, InstalledRuntimeProjection, ProjectedAttributeValue, ProjectedBatch,
    ProjectedBatchExecutor, ProjectedBatchInvocationControl, ProjectedBatchOperation,
    ProjectedBatchResult, ProjectedBatchRow, ProjectedCreate, ProjectedReference, ProjectedThing,
    QueryExecutionResourceLimits, TxType,
};
use type_bridge_orm::{CapabilitySet, ClassifiedCommitError, Database, OrmError};
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
      {string}: {{ card: {{ min: 0, max: 1 }} }}
      {boolean}: {{ card: {{ min: 0, max: 1 }} }}
      {integer}: {{ card: {{ min: 0, max: 1 }} }}
      {double}: {{ card: {{ min: 0, max: 1 }} }}
      {date}: {{ card: {{ min: 0, max: 1 }} }}
      {datetime}: {{ card: {{ min: 0, max: 1 }} }}
      {datetime_tz}: {{ card: {{ min: 0, max: 1 }} }}
      {decimal}: {{ card: {{ min: 0, max: 1 }} }}
      {duration}: {{ card: {{ min: 0, max: 1 }} }}
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

/// The TypeQL schema for the live fixture labels.
fn schema_text(ids: &LiveIds) -> String {
    format!(
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
    )
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
    let schema = schema_text(&ids);
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

// ── Split-batch live journeys ─────────────────────────────────────────

/// Provider calls one counted connection observed.
#[derive(Debug, Default)]
struct DispatchLog {
    statements: Vec<String>,
    /// `(query source, given rows)` per `given` query, in order.
    given_queries: Vec<(String, usize)>,
    opened: Vec<TxType>,
    commits: usize,
    rollbacks: usize,
    fail_at: Option<usize>,
}

impl DispatchLog {
    /// Calls per distinct source, in first-seen order: `(source, rows per call)`.
    fn by_source(&self) -> Vec<(String, Vec<usize>)> {
        let mut grouped: Vec<(String, Vec<usize>)> = Vec::new();
        for (source, rows) in &self.given_queries {
            match grouped.iter_mut().find(|(seen, _)| seen == source) {
                Some((_, calls)) => calls.push(*rows),
                None => grouped.push((source.clone(), vec![*rows])),
            }
        }
        grouped
    }
}

/// Forwards every provider call to the real backend and records each one.
struct CountingBackend {
    inner: RealBackend,
    literal: bool,
    log: Arc<Mutex<DispatchLog>>,
}

struct CountingTransaction {
    inner: Box<dyn TransactionOps>,
    literal: bool,
    log: Arc<Mutex<DispatchLog>>,
}

impl CountingBackend {
    fn counted<'a>(
        &'a self,
        tx_type: TxType,
        opened: BoxFuture<'a, Result<Box<dyn TransactionOps>, OrmError>>,
    ) -> BoxFuture<'a, Result<Box<dyn TransactionOps>, OrmError>> {
        let log = Arc::clone(&self.log);
        Box::pin(async move {
            let inner = opened.await?;
            log.lock().unwrap().opened.push(tx_type);
            Ok(Box::new(CountingTransaction {
                inner,
                literal: self.literal,
                log,
            }) as Box<dyn TransactionOps>)
        })
    }
}

impl DriverBackend for CountingBackend {
    fn match_capabilities(&self) -> CapabilitySet {
        self.inner.match_capabilities()
    }
    fn open_transaction(
        &self,
        database: &str,
        tx_type: TxType,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
        self.counted(tx_type, self.inner.open_transaction(database, tx_type))
    }
    fn open_with_options(
        &self,
        database: &str,
        tx_type: TxType,
        options: &TransactionOptions,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
        self.counted(
            tx_type,
            self.inner.open_with_options(database, tx_type, options),
        )
    }
    #[allow(deprecated)]
    fn open_schema_fenced_read_transaction(
        &self,
        database: &str,
        timeout: std::time::Duration,
    ) -> BoxFuture<'_, Result<SchemaFencedReadTransaction, OrmError>> {
        self.inner
            .open_schema_fenced_read_transaction(database, timeout)
    }
    fn is_open(&self) -> bool {
        self.inner.is_open()
    }
    fn close_connection(&self) -> Result<(), OrmError> {
        self.inner.close_connection()
    }
    fn server_version(&self) -> Option<type_bridge_core_lib::version::Version> {
        self.inner.server_version()
    }
    fn supports_given_rows(&self) -> bool {
        !self.literal && DriverBackend::supports_given_rows(&self.inner)
    }
    fn database_exists(&self, database: &str) -> BoxFuture<'_, Result<bool, OrmError>> {
        self.inner.database_exists(database)
    }
    fn create_database(&self, database: &str) -> BoxFuture<'_, Result<(), OrmError>> {
        self.inner.create_database(database)
    }
    fn delete_database(&self, database: &str) -> BoxFuture<'_, Result<(), OrmError>> {
        self.inner.delete_database(database)
    }
    fn schema_text(&self, database: &str) -> BoxFuture<'_, Result<String, OrmError>> {
        self.inner.schema_text(database)
    }
}

impl TransactionOps for CountingTransaction {
    fn supports_given_rows(&self) -> bool {
        !self.literal && self.inner.supports_given_rows()
    }
    fn schema_snapshot(&mut self) -> BoxFuture<'_, Result<Option<String>, OrmError>> {
        self.inner.schema_snapshot()
    }
    fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        self.log.lock().unwrap().statements.push(typeql.to_owned());
        self.inner.query(typeql)
    }
    fn query_canonical<'a>(
        &'a mut self,
        typeql: &str,
    ) -> BoxFuture<'a, Result<QueryResult, OrmError>> {
        self.log.lock().unwrap().statements.push(typeql.to_owned());
        self.inner.query_canonical(typeql)
    }
    fn query_with_rows(
        &mut self,
        typeql: &str,
        rows: GivenRowsSpec,
    ) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        self.record(typeql, &rows);
        self.inner.query_with_rows(typeql, rows)
    }
    fn query_with_rows_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        rows: GivenRowsSpec,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        self.record(typeql, &rows);
        self.inner
            .query_with_rows_bounded(typeql, rows, limits, consumer)
    }
    fn query_materialized_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        rows: Option<GivenRowsSpec>,
        limits: type_bridge_orm::session::backend::AnswerLimits,
        canonical: bool,
        statement_display: Option<&'a str>,
    ) -> BoxFuture<'a, Result<QueryResult, OrmError>> {
        if let Some(rows) = &rows {
            self.record(typeql, rows);
        } else {
            self.log.lock().unwrap().statements.push(typeql.to_owned());
        }
        let fail = self.fail_requested();
        after_counted(
            self.inner.query_materialized_bounded(
                typeql,
                rows,
                limits,
                canonical,
                statement_display,
            ),
            fail,
        )
    }
    fn query_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        self.log.lock().unwrap().statements.push(typeql.to_owned());
        self.inner.query_bounded(typeql, limits, consumer)
    }
    fn query_v2_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        limits: AnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        self.log.lock().unwrap().statements.push(typeql.to_owned());
        let fail = self.fail_requested();
        after_counted(self.inner.query_v2_bounded(typeql, limits, consumer), fail)
    }
    fn query_v2_with_rows_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        rows: GivenRowsSpec,
        limits: AnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        self.record(typeql, &rows);
        let fail = self.fail_requested();
        after_counted(
            self.inner
                .query_v2_with_rows_bounded(typeql, rows, limits, consumer),
            fail,
        )
    }
    fn query_typed_bounded<'a>(
        &'a mut self,
        query: &'a TypedFetchRows,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        self.inner.query_typed_bounded(query, limits, consumer)
    }
    fn supports_exactly_one_tuple_proof(&self) -> bool {
        self.inner.supports_exactly_one_tuple_proof()
    }
    fn query_tuple_typed_bounded<'a>(
        &'a mut self,
        query: &'a TypedFetchRows,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        self.inner
            .query_tuple_typed_bounded(query, limits, consumer)
    }
    fn query_root_typed_bounded<'a>(
        &'a mut self,
        query: &'a TypedRootScan,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        self.inner.query_root_typed_bounded(query, limits, consumer)
    }
    fn rematch_page_typed_bounded<'a>(
        &'a mut self,
        query: &'a TypedPageRematch,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        self.inner
            .rematch_page_typed_bounded(query, limits, consumer)
    }
    fn hydrate_typed_bounded<'a>(
        &'a mut self,
        query: &'a TypedHydrateThings,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        self.inner.hydrate_typed_bounded(query, limits, consumer)
    }
    fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.log.lock().unwrap().commits += 1;
        self.inner.commit()
    }
    fn commit_classified(&mut self) -> BoxFuture<'_, Result<(), ClassifiedCommitError>> {
        self.log.lock().unwrap().commits += 1;
        self.inner.commit_classified()
    }
    fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.log.lock().unwrap().rollbacks += 1;
        self.inner.rollback()
    }
    fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.inner.close()
    }
}

fn after_counted<'a, T: Send + 'a>(
    future: BoxFuture<'a, Result<T, OrmError>>,
    fail: bool,
) -> BoxFuture<'a, Result<T, OrmError>> {
    Box::pin(async move {
        let answer = future.await?;
        if fail {
            Err(OrmError::Transaction("injected final-piece failure".into()))
        } else {
            Ok(answer)
        }
    })
}

impl CountingTransaction {
    fn fail_requested(&self) -> bool {
        let log = self.log.lock().unwrap();
        log.fail_at == Some(log.statements.len())
    }
    fn record(&self, typeql: &str, rows: &GivenRowsSpec) {
        self.log.lock().unwrap().statements.push(typeql.to_owned());
        self.log
            .lock()
            .unwrap()
            .given_queries
            .push((typeql.to_owned(), rows.rows.len()));
    }
}

/// A fresh fixture database whose connection counts provider calls.
async fn counted_fixture(
    tag: &str,
) -> Option<(
    Database,
    Database,
    InstalledRuntimeProjection,
    LiveIds,
    Arc<Mutex<DispatchLog>>,
)> {
    let database = setup_db().await;
    if !server_supports_v2_conformance(&database) {
        database.delete_database().await.unwrap();
        return None;
    }
    let (installed, ids) = installed(&unique_label(tag));
    database
        .execute_raw(&schema_text(&ids), TxType::Schema)
        .await
        .unwrap();
    let (counted, log) = counted_connection(&database, false).await;
    Some((database, counted, installed, ids, log))
}

async fn counted_connection(
    database: &Database,
    literal: bool,
) -> (Database, Arc<Mutex<DispatchLog>>) {
    let address = std::env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1730".into());
    let username = std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".into());
    let password = std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".into());
    let inner = RealBackend::connect(&address, &username, &password, connect_options_from_env())
        .await
        .unwrap();
    let log = Arc::new(Mutex::new(DispatchLog::default()));
    let counted = Database::with_backend(
        Box::new(CountingBackend {
            inner,
            literal,
            log: Arc::clone(&log),
        }),
        database.database_name(),
    );
    (counted, log)
}

#[tokio::test]
#[ignore = "requires a live TypeDB server"]
async fn keyed_relation_put_replaces_players_live() {
    let _guard = crate::common::integration_test_guard().await;
    let Some((database, counted, installed, ids, log)) = counted_fixture("keyed-put").await else {
        return;
    };
    let executor = type_bridge_orm::ProjectedCrudExecutor::new(&installed);
    let first = executor
        .insert_entity(&counted, &person_create(&installed, &ids, "first", false))
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    let second = executor
        .insert_entity(&counted, &person_create(&installed, &ids, "second", false))
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    let role = RoleId::new(ids.membership.label().as_str(), "member").unwrap();
    let input = |player: &str| {
        relation_create(
            &installed,
            &ids.membership,
            role.clone(),
            &ids.key,
            "same-key",
            vec![reference(&installed, &ids.person, player)],
        )
    };
    let original = executor
        .put_relation(&counted, &input(first.iid()))
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    assert_role_players(&original, &role, &[(&ids.person, first.iid())]);
    *log.lock().unwrap() = DispatchLog::default();
    let started = std::time::Instant::now();
    let replaced = executor
        .put_relation(&counted, &input(second.iid()))
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    eprintln!(
        "keyed relation put: statements={} elapsed={:?}",
        log.lock().unwrap().statements.len(),
        started.elapsed()
    );
    assert_eq!(log.lock().unwrap().statements.len(), 1);
    assert_eq!(replaced.iid(), original.iid());
    assert_role_players(&replaced, &role, &[(&ids.person, second.iid())]);
    let persisted = executor
        .get_relation_by_iid(&counted, &ids.membership, original.iid())
        .await
        .unwrap()
        .unwrap();
    assert_role_players(&persisted, &role, &[(&ids.person, second.iid())]);
    *log.lock().unwrap() = DispatchLog::default();
    let controlled = executor
        .put_relation_controlled(
            &counted,
            &input(first.iid()),
            QueryExecutionResourceLimits::default(),
            AnswerCancellation::default(),
        )
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    assert_eq!(log.lock().unwrap().statements.len(), 1);
    assert_eq!(controlled, original);
    *log.lock().unwrap() = DispatchLog::default();
    let fresh = executor
        .put_relation_controlled(
            &counted,
            &relation_create(
                &installed,
                &ids.membership,
                role.clone(),
                &ids.key,
                "new-key",
                vec![reference(&installed, &ids.person, second.iid())],
            ),
            QueryExecutionResourceLimits::default(),
            AnswerCancellation::default(),
        )
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    assert_eq!(log.lock().unwrap().statements.len(), 1);
    assert_ne!(fresh.iid(), original.iid());
    assert_role_players(&fresh, &role, &[(&ids.person, second.iid())]);
    let (literal, literal_log) = counted_connection(&database, true).await;
    let literal_put = executor
        .put_relation_controlled(
            &literal,
            &input(second.iid()),
            QueryExecutionResourceLimits::default(),
            AnswerCancellation::default(),
        )
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    assert_eq!(literal_put.iid(), original.iid());
    assert_role_players(&literal_put, &role, &[(&ids.person, second.iid())]);
    {
        let observed = literal_log.lock().unwrap();
        assert_eq!(observed.statements.len(), 1);
        assert_eq!(observed.opened, [TxType::Write]);
        assert_eq!((observed.commits, observed.rollbacks), (1, 0));
    }
    let mut put_times = Vec::new();
    let mut update_times = Vec::new();
    let mut get_times = Vec::new();
    for iteration in 0..35 {
        let player = if iteration % 2 == 0 {
            first.iid()
        } else {
            second.iid()
        };
        let replacement = input(player);
        let started = std::time::Instant::now();
        let put = executor
            .put_relation_controlled(
                &database,
                &replacement,
                QueryExecutionResourceLimits::default(),
                AnswerCancellation::default(),
            )
            .await
            .unwrap()
            .into_items()
            .pop()
            .expect("one written root");
        let put_elapsed = started.elapsed().as_micros();
        assert_eq!(put.iid(), original.iid());
        let started = std::time::Instant::now();
        let updated = executor
            .update_relation_controlled(
                &database,
                original.iid(),
                &replacement,
                QueryExecutionResourceLimits::default(),
                AnswerCancellation::default(),
            )
            .await
            .unwrap()
            .into_items()
            .pop()
            .expect("one written root");
        let update_elapsed = started.elapsed().as_micros();
        assert_eq!(updated.iid(), original.iid());
        let started = std::time::Instant::now();
        let fetched = executor
            .get_relation_by_iid_controlled(
                &database,
                &ids.membership,
                original.iid(),
                QueryExecutionResourceLimits::default(),
                AnswerCancellation::default(),
            )
            .await
            .unwrap()
            .unwrap();
        let get_elapsed = started.elapsed().as_micros();
        assert_role_players(&fetched, &role, &[(&ids.person, player)]);
        if iteration >= 5 {
            put_times.push(put_elapsed);
            update_times.push(update_elapsed);
            get_times.push(get_elapsed);
        }
    }
    for (operation, times) in [
        ("put", &mut put_times),
        ("update", &mut update_times),
        ("get_by_iid", &mut get_times),
    ] {
        times.sort_unstable();
        eprintln!(
            "NATIVE_MANAGER_BENCH server={:?} operation={operation} samples=30 warmup=5 median_us={}",
            database.server_version(),
            (times[14] + times[15]) / 2
        );
    }
    database.delete_database().await.unwrap();
}

#[derive(Debug, PartialEq)]
struct PolymorphicSnapshot {
    iid: Option<String>,
    document: serde_json::Map<String, serde_json::Value>,
}

impl type_bridge_orm::_relation::TypeBridgeRelation for PolymorphicSnapshot {
    const TYPE_NAME: &'static str = "poly-root";
    fn owned_attributes() -> &'static [type_bridge_orm::_entity::OwnedAttributeInfo] {
        &[]
    }
    fn role_info() -> &'static [type_bridge_orm::_relation::RoleInfo] {
        &[]
    }
    fn iid(&self) -> Option<&str> {
        self.iid.as_deref()
    }
    fn set_iid(&mut self, iid: String) {
        self.iid = Some(iid);
    }
    fn to_attribute_values(&self) -> Vec<(&'static str, type_bridge_orm::AttributeValue)> {
        vec![]
    }
    fn to_role_player_refs(&self) -> Vec<type_bridge_orm::_relation::RolePlayerRef> {
        vec![]
    }
    fn from_document(
        document: &serde_json::Map<String, serde_json::Value>,
    ) -> type_bridge_orm::Result<Self> {
        Ok(Self {
            iid: None,
            document: document.clone(),
        })
    }
}

#[tokio::test]
#[ignore = "requires a live TypeDB server"]
async fn polymorphic_returning_delete_keeps_complete_subtype_snapshots_live() {
    let _guard = crate::common::integration_test_guard().await;
    let database = setup_db().await;
    database.delete_database().await.unwrap();
    database.create_database().await.unwrap();
    let yaml = r#"format: typebridge.schema/v2
attributes:
  poly-key: { value: string }
entities:
  poly-person:
    owns:
      poly-key: { key: true }
  poly-employee:
    sub: poly-person
relations:
  poly-root:
    owns:
      poly-key: { key: true }
    relates:
      member: { card: { min: 1, max: 2 } }
  poly-child:
    sub: poly-root
    relates:
      extra: { card: { min: 0, max: 2 } }
  poly-choice:
    owns:
      poly-key: { key: true }
    relates:
      left: { card: { min: 0, max: 2 } }
      right: { card: { min: 0, max: 2 } }
plays:
  poly-person:
    poly-root: [member]
    poly-child: [extra]
    poly-choice: [left, right]
"#;
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("polymorphic.yaml").unwrap(), yaml)]).unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
    )
    .unwrap();
    let installed = InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Rust,
            &ProjectionConfig::rust(),
            &[ProjectionHandler::rust_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    database
        .execute_raw(
            r#"define
attribute poly-key, value string;
entity poly-person, owns poly-key @key, plays poly-root:member, plays poly-child:extra, plays poly-choice:left, plays poly-choice:right;
entity poly-employee sub poly-person;
relation poly-root, owns poly-key @key, relates member @card(1..2);
relation poly-child sub poly-root, relates extra @card(0..2);
relation poly-choice, owns poly-key @key, relates left @card(0..2), relates right @card(0..2);
"#,
            TxType::Schema,
        )
        .await
        .unwrap();
    database
        .execute_raw(
            r#"insert $p isa! poly-person, has poly-key "p";
$employee isa! poly-employee, has poly-key "employee";
$r isa! poly-root, has poly-key "root", links (member: $p);
$c isa! poly-child, has poly-key "child", links (member: $p, extra: $p);
$e isa! poly-child, has poly-key "empty-child", links (member: $p);
"#,
            TxType::Write,
        )
        .await
        .unwrap();
    let (counted, log) = counted_connection(&database, false).await;
    let manager = type_bridge_orm::_manager::RelationManager::<PolymorphicSnapshot>::from_registry(
        &counted,
        installed.manager_registry().unwrap(),
    )
    .unwrap();
    let mut before = manager.all().await.unwrap();
    before.sort_by(|a, b| {
        a.document["poly-key"]
            .as_str()
            .cmp(&b.document["poly-key"].as_str())
    });
    assert_eq!(before.len(), 3);
    *log.lock().unwrap() = DispatchLog::default();
    let snapshots = manager.delete_many_returning(&before).await.unwrap();
    eprintln!(
        "polymorphic returning delete: statements={} snapshots={snapshots:?}",
        log.lock().unwrap().statements.len()
    );
    assert_eq!(log.lock().unwrap().statements.len(), 2);
    assert_eq!(snapshots, before);
    assert!(manager.all().await.unwrap().is_empty());
    disjoint_role_insert(&counted, &installed, &log).await;
    database.delete_database().await.unwrap();
}

struct BatchChoiceRelation {
    iid: Option<String>,
    key: Option<String>,
    players: Vec<type_bridge_orm::_relation::RolePlayerRef>,
    document: Option<serde_json::Map<String, serde_json::Value>>,
}
impl type_bridge_orm::_relation::TypeBridgeRelation for BatchChoiceRelation {
    const TYPE_NAME: &'static str = "poly-choice";
    fn owned_attributes() -> &'static [type_bridge_orm::_entity::OwnedAttributeInfo] {
        &[]
    }
    fn role_info() -> &'static [type_bridge_orm::_relation::RoleInfo] {
        &[]
    }
    fn iid(&self) -> Option<&str> {
        self.iid.as_deref()
    }
    fn set_iid(&mut self, iid: String) {
        self.iid = Some(iid);
    }
    fn to_attribute_values(&self) -> Vec<(&'static str, type_bridge_orm::AttributeValue)> {
        self.key
            .as_ref()
            .map(|key| {
                (
                    "poly-key",
                    type_bridge_orm::AttributeValue::String(key.clone()),
                )
            })
            .into_iter()
            .collect()
    }
    fn to_role_player_refs(&self) -> Vec<type_bridge_orm::_relation::RolePlayerRef> {
        self.players.clone()
    }
    fn from_document(
        document: &serde_json::Map<String, serde_json::Value>,
    ) -> type_bridge_orm::Result<Self> {
        Ok(Self {
            iid: None,
            key: document
                .get("poly-key")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            players: vec![],
            document: Some(document.clone()),
        })
    }
}

async fn disjoint_role_insert(
    database: &Database,
    installed: &InstalledRuntimeProjection,
    log: &Arc<Mutex<DispatchLog>>,
) {
    use type_bridge_orm::_dynamic::{DynamicAttributeMap, DynamicRolePlayerInput};
    use type_bridge_orm::_manager::DynamicRelationManager;
    use type_bridge_orm::AttributeValue;
    let registry = installed.manager_registry().unwrap();
    let descriptor = registry.relation("poly-choice").unwrap();
    let raw = DynamicRelationManager::from_registry(
        database,
        installed.manager_registry().unwrap(),
        Arc::clone(&descriptor),
    );
    let manager = DynamicRelationManager::from_registry(database, registry, descriptor);
    let player = |role: &str| DynamicRolePlayerInput {
        role_name: role.into(),
        player_type_name: "poly-person".into(),
        iid: None,
        key: Some(("poly-key".into(), AttributeValue::String("p".into()))),
    };
    let items: Vec<(DynamicAttributeMap, Vec<DynamicRolePlayerInput>)> = vec![
        (
            vec![("poly-key".into(), AttributeValue::String("left".into()))],
            vec![player("left")],
        ),
        (
            vec![("poly-key".into(), AttributeValue::String("right".into()))],
            vec![player("right")],
        ),
        (
            vec![("poly-key".into(), AttributeValue::String("both".into()))],
            vec![player("left"), player("right")],
        ),
    ];
    let mut old_iids = Vec::new();
    for (attributes, players) in &items {
        let result = raw.insert(attributes, players).await.unwrap();
        old_iids.push(
            result
                .item()
                .expect("one baseline root")
                .iid
                .clone()
                .unwrap(),
        );
    }
    let mut baseline = Vec::new();
    for iid in &old_iids {
        let mut row = manager.get_by_iid_exact(iid).await.unwrap();
        row[0].iid = None;
        baseline.push(serde_json::to_value(row).unwrap());
    }
    manager.delete_many_by_iid_exact(&old_iids).await.unwrap();
    *log.lock().unwrap() = DispatchLog::default();
    let iids = manager.insert_many(&items).await.unwrap();
    assert_eq!(log.lock().unwrap().statements.len(), 1);
    assert_eq!(iids.len(), 3);
    for (iid, expected) in iids.iter().zip(&baseline) {
        let mut row = manager.get_by_iid_exact(iid).await.unwrap();
        row[0].iid = None;
        assert_eq!(&serde_json::to_value(row).unwrap(), expected);
    }
    let first_player = manager.get_by_iid_exact(&iids[0]).await.unwrap()[0].role_players[0]
        .player_iid
        .clone()
        .unwrap();
    let mut aliased_players = vec![player("left"), player("left")];
    aliased_players[0].iid = Some(first_player.clone());
    aliased_players[0].key = None;
    *log.lock().unwrap() = DispatchLog::default();
    let aliased = manager
        .put_exact(&items[0].0, &aliased_players)
        .await
        .unwrap();
    assert_eq!(aliased.count(), 1);
    assert_eq!(
        aliased.item().unwrap().iid.as_deref(),
        Some(iids[0].as_str())
    );
    {
        let observed = log.lock().unwrap();
        assert_eq!(observed.statements.len(), 1);
        assert_eq!((observed.commits, observed.rollbacks), (1, 0));
    }
    let mut unchanged = manager.get_by_iid_exact(&iids[0]).await.unwrap();
    unchanged[0].iid = None;
    assert_eq!(serde_json::to_value(unchanged).unwrap(), baseline[0]);
    let mut put_items = items.clone();
    put_items[0].1[0].iid = Some(first_player);
    put_items[0].1[0].key = None;
    put_items[2].0[0].1 = AttributeValue::String("new-mixed".into());
    let mut old_put = Vec::new();
    for (attributes, players) in &put_items {
        let result = raw.put_exact(attributes, players).await.unwrap();
        old_put.push(
            result
                .item()
                .expect("one baseline root")
                .iid
                .clone()
                .unwrap(),
        );
    }
    let mut put_baseline = Vec::new();
    for iid in &old_put {
        let mut row = manager.get_by_iid_exact(iid).await.unwrap();
        row[0].iid = None;
        put_baseline.push(serde_json::to_value(row).unwrap());
    }
    manager
        .delete_many_by_iid_exact(&old_put[2..])
        .await
        .unwrap();
    *log.lock().unwrap() = DispatchLog::default();
    let put = manager.put_many_exact(&put_items).await.unwrap();
    {
        let log = log.lock().unwrap();
        assert_eq!(log.statements.len(), 1);
        assert_eq!(log.opened, [TxType::Write]);
        assert_eq!(log.commits, 1);
    }
    assert_eq!(&put[..2], &iids[..2]);
    assert_ne!(put[2], iids[2]);
    for (iid, expected) in put.iter().zip(&put_baseline) {
        let mut row = manager.get_by_iid_exact(iid).await.unwrap();
        row[0].iid = None;
        assert_eq!(&serde_json::to_value(row).unwrap(), expected);
    }
    eprintln!(
        "mixed IID/key put_many: one plan statement and transaction; existing IIDs preserved and all fields/players equal prior answers"
    );
    manager.delete_many_by_iid_exact(&iids).await.unwrap();
    let (literal, literal_log) = counted_connection(database, true).await;
    let literal_manager = DynamicRelationManager::from_registry(
        &literal,
        installed.manager_registry().unwrap(),
        installed
            .manager_registry()
            .unwrap()
            .relation("poly-choice")
            .unwrap(),
    );
    let iids = literal_manager.insert_many(&items).await.unwrap();
    assert_eq!(literal_log.lock().unwrap().statements.len(), 1);
    for (iid, expected) in iids.iter().zip(&baseline) {
        let mut row = literal_manager.get_by_iid_exact(iid).await.unwrap();
        row[0].iid = None;
        assert_eq!(&serde_json::to_value(row).unwrap(), expected);
    }
    let typed = type_bridge_orm::_manager::RelationManager::<BatchChoiceRelation>::from_registry(
        database,
        installed.manager_registry().unwrap(),
    )
    .unwrap();
    let mut inserts = ["left", "right", "both"]
        .into_iter()
        .map(|kind| BatchChoiceRelation {
            iid: None,
            key: Some(format!("generic-{kind}")),
            document: None,
            players: if kind == "both" {
                vec!["left", "right"]
            } else {
                vec![kind]
            }
            .into_iter()
            .map(|role| type_bridge_orm::_relation::RolePlayerRef {
                role,
                entity_type_name: "poly-person",
                iid: None,
                key: Some(("poly-key", AttributeValue::String("p".into()))),
            })
            .collect(),
        })
        .collect::<Vec<_>>();
    *log.lock().unwrap() = DispatchLog::default();
    let created = typed.insert_many(&mut inserts).await.unwrap();
    {
        let observed = log.lock().unwrap();
        assert_eq!(observed.statements.len(), 1);
        assert_eq!(observed.opened, [TxType::Write]);
        assert_eq!(observed.commits, 1);
    }
    assert_eq!(
        inserts
            .iter()
            .map(|model| model.iid.clone().unwrap())
            .collect::<Vec<_>>(),
        created
    );
    inserts[1].iid = None;
    inserts[2].iid = None;
    inserts[2].players.clear();
    *log.lock().unwrap() = DispatchLog::default();
    typed.delete_many(&inserts).await.unwrap();
    {
        let observed = log.lock().unwrap();
        assert_eq!(observed.statements.len(), 1);
        assert_eq!(observed.opened, [TxType::Write]);
        assert_eq!(observed.commits, 1);
    }
    for iid in &created {
        assert!(manager.get_by_iid_exact(iid).await.unwrap().is_empty());
    }
    eprintln!(
        "generic relation insert_many/delete_many: one statement and transaction each, mixed IID/key/player selectors"
    );
    let subtype_items = ["employee-left", "employee-right"]
        .into_iter()
        .map(|key| {
            (
                vec![("poly-key".into(), AttributeValue::String(key.into()))],
                vec![DynamicRolePlayerInput {
                    role_name: "left".into(),
                    player_type_name: "poly-person".into(),
                    iid: None,
                    key: Some(("poly-key".into(), AttributeValue::String("employee".into()))),
                }],
            )
        })
        .collect::<Vec<_>>();
    *log.lock().unwrap() = DispatchLog::default();
    let subtype_iids = manager.insert_many(&subtype_items).await.unwrap();
    assert_eq!(log.lock().unwrap().statements.len(), 1);
    for iid in &subtype_iids {
        let stored = manager.get_by_iid_exact(iid).await.unwrap();
        assert_eq!(
            stored[0].role_players[0].player_type_name.as_deref(),
            Some("poly-employee")
        );
    }
    let selectors = vec![
        BatchChoiceRelation {
            iid: None,
            key: Some("employee-right".into()),
            players: vec![type_bridge_orm::_relation::RolePlayerRef {
                role: "left",
                entity_type_name: "poly-person",
                iid: None,
                key: Some(("poly-key", AttributeValue::String("employee".into()))),
            }],
            document: None,
        },
        BatchChoiceRelation {
            iid: None,
            key: Some("missing".into()),
            players: vec![],
            document: None,
        },
        BatchChoiceRelation {
            iid: Some(subtype_iids[0].clone()),
            key: Some("stale-selector-ignored-for-iid".into()),
            players: vec![],
            document: None,
        },
    ];
    *log.lock().unwrap() = DispatchLog {
        fail_at: Some(2),
        ..Default::default()
    };
    assert!(typed.delete_many_returning(&selectors).await.is_err());
    {
        let observed = log.lock().unwrap();
        assert_eq!(observed.statements.len(), 2);
        assert_eq!(observed.opened, [TxType::Write]);
        assert_eq!((observed.commits, observed.rollbacks), (0, 1));
    }
    for iid in &subtype_iids {
        assert_eq!(manager.get_by_iid_exact(iid).await.unwrap().len(), 1);
    }
    *log.lock().unwrap() = DispatchLog::default();
    let snapshots = typed.delete_many_returning(&selectors).await.unwrap();
    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0].iid.as_ref(), Some(&subtype_iids[1]));
    assert_eq!(snapshots[1].iid.as_ref(), Some(&subtype_iids[0]));
    assert_eq!(snapshots[0].key.as_deref(), Some("employee-right"));
    assert_eq!(snapshots[1].key.as_deref(), Some("employee-left"));
    for snapshot in &snapshots {
        assert!(
            snapshot
                .document
                .as_ref()
                .unwrap()
                .contains_key("role_players")
        );
    }
    {
        let observed = log.lock().unwrap();
        assert_eq!(observed.statements.len(), 2);
        assert_eq!(observed.opened, [TxType::Write]);
        assert_eq!((observed.commits, observed.rollbacks), (1, 0));
    }
    for iid in &subtype_iids {
        assert!(manager.get_by_iid_exact(iid).await.unwrap().is_empty());
    }
    eprintln!(
        "generic relation returning delete: mixed IID/key/base-player selectors, subtype snapshots, missing omitted, input order and final-delete rollback; two statements in one transaction"
    );

    eprintln!(
        "disjoint and mixed role insert: one statement, given and literal; all committed fields and players equal grouped baseline"
    );
}

fn person_rows(
    installed: &InstalledRuntimeProjection,
    ids: &LiveIds,
    count: usize,
    key: impl Fn(usize) -> String,
) -> ProjectedBatch {
    ProjectedBatch::try_new(
        installed,
        ids.person.clone(),
        ProjectedBatchOperation::Insert,
        (0..count)
            .map(|row| ProjectedBatchRow::Create(person_create(installed, ids, &key(row), false)))
            .collect(),
    )
    .unwrap()
}

/// The default control with a deadline that fits a split debug-build batch.
fn split_control() -> ProjectedBatchInvocationControl {
    ProjectedBatchInvocationControl::capture(
        QueryExecutionResourceLimits {
            timeout_milliseconds: type_bridge_orm::QueryTimeout::from_milliseconds(600_000),
            ..QueryExecutionResourceLimits::default()
        },
        AnswerCancellation::default(),
    )
}

async fn person_count(database: &Database, ids: &LiveIds) -> usize {
    let rows = database
        .execute_raw(
            &format!("match $p isa {};\nselect $p;", ids.person.label()),
            TxType::Read,
        )
        .await
        .unwrap();
    match rows {
        QueryResult::Rows(rows) => rows.len(),
        other => panic!("person probe returned {other:?}"),
    }
}

/// A batch under every limit is one complete mutation pipeline.
#[tokio::test]
#[ignore = "requires the exact TypeDB CE 3.12.1 live fixture"]
async fn projected_batch_thousand_rows_fire_one_mutation_live() {
    let _guard = crate::common::integration_test_guard().await;
    let Some((database, counted, installed, ids, log)) = counted_fixture("batch-1k").await else {
        return;
    };
    let batch = person_rows(&installed, &ids, 1_000, |row| format!("p-{row}"));
    let inserted = things(
        ProjectedBatchExecutor::new(&installed)
            .execute(&counted, &batch, control())
            .await
            .unwrap(),
    );
    assert_eq!(inserted.len(), 1_000);
    let log = std::mem::take(&mut *log.lock().unwrap());
    let calls = log.by_source();
    eprintln!(
        "1k: opened={:?} commits={} calls={:?}",
        log.opened,
        log.commits,
        calls.iter().map(|(_, rows)| rows).collect::<Vec<_>>()
    );
    assert_eq!(log.opened, vec![TxType::Write]);
    assert_eq!(log.commits, 1);
    assert_eq!(calls.len(), 1, "one complete mutation pipeline");
    assert_eq!(calls[0].1, vec![1_000], "the pipeline fires once");
    assert_eq!(person_count(&database, &ids).await, 1_000);
    database.delete_database().await.unwrap();
}

/// Rows past 4,096 split into same-shape chunks on one Write with one commit.
#[tokio::test]
#[ignore = "requires the exact TypeDB CE 3.12.1 live fixture"]
async fn projected_batch_past_row_limit_splits_in_one_transaction_live() {
    let _guard = crate::common::integration_test_guard().await;
    let Some((database, counted, installed, ids, log)) = counted_fixture("batch-split").await
    else {
        return;
    };
    let batch = person_rows(&installed, &ids, 5_000, |row| format!("p-{row}"));
    let inserted = things(
        ProjectedBatchExecutor::new(&installed)
            .execute(&counted, &batch, split_control())
            .await
            .unwrap(),
    );
    assert_eq!(inserted.len(), 5_000);
    let log = std::mem::take(&mut *log.lock().unwrap());
    let calls = log.by_source();
    eprintln!(
        "split: opened={:?} commits={} calls={:?}",
        log.opened,
        log.commits,
        calls.iter().map(|(_, rows)| rows).collect::<Vec<_>>()
    );
    assert_eq!(log.opened, vec![TxType::Write]);
    assert_eq!(log.commits, 1);
    assert_eq!(
        calls.len(),
        1,
        "one complete mutation pipeline across canonical chunks"
    );
    for (_, chunks) in &calls {
        assert_eq!(chunks, &vec![4_096, 904]);
    }
    assert_eq!(person_count(&database, &ids).await, 5_000);
    database.delete_database().await.unwrap();
}

/// A key clash in the last chunk rolls back the chunks before it.
#[tokio::test]
#[ignore = "requires the exact TypeDB CE 3.12.1 live fixture"]
async fn projected_batch_split_key_clash_in_last_chunk_stores_nothing_live() {
    let _guard = crate::common::integration_test_guard().await;
    let Some((database, counted, installed, ids, log)) = counted_fixture("batch-split-clash").await
    else {
        return;
    };
    let executor = ProjectedBatchExecutor::new(&installed);
    executor
        .execute(
            &database,
            &person_rows(&installed, &ids, 1, |_| "taken".to_owned()),
            control(),
        )
        .await
        .unwrap();
    let batch = person_rows(&installed, &ids, 5_000, |row| {
        if row == 4_500 {
            "taken".to_owned()
        } else {
            format!("p-{row}")
        }
    });
    let error = executor
        .execute(&counted, &batch, split_control())
        .await
        .expect_err("a key clash must fail the whole batch");
    let log = std::mem::take(&mut *log.lock().unwrap());
    let calls = log.by_source();
    eprintln!(
        "clash: code={} opened={:?} commits={} calls={:?}",
        error.code().as_str(),
        log.opened,
        log.commits,
        calls.iter().map(|(_, rows)| rows).collect::<Vec<_>>()
    );
    assert_eq!(error.code().as_str(), "provider_operation_failed");
    assert_eq!(log.opened, vec![TxType::Write]);
    assert_eq!(log.commits, 0);
    assert_eq!(
        calls[0].1,
        vec![4_096, 904],
        "the clash is in the last mutation chunk"
    );
    assert_eq!(
        person_count(&database, &ids).await,
        1,
        "only the earlier person remains"
    );
    database.delete_database().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a live TypeDB server"]
async fn wide_manager_rows_match_prior_answers_and_last_piece_is_atomic_live() {
    use type_bridge_orm::_dynamic::{DynamicAttributeMap, DynamicRolePlayerInput};
    use type_bridge_orm::_manager::{DynamicEntityManager, DynamicRelationManager};
    use type_bridge_orm::{AttributeValue, Filter};
    let _guard = crate::common::integration_test_guard().await;
    let database = setup_db().await;
    let prefix = unique_label("wide-manager");
    let key = format!("{prefix}-key");
    let tag = format!("{prefix}-tag");
    let person_name = format!("{prefix}-person");
    let relation_name = format!("{prefix}-relation");
    let yaml = format!(
        "format: typebridge.schema/v2\nattributes:\n  {key}: {{ value: string }}\n  {tag}: {{ value: string }}\nentities:\n  {person_name}:\n    owns:\n      {key}: {{ key: true }}\n      {tag}: {{ card: {{ min: 0 }} }}\nrelations:\n  {relation_name}:\n    owns:\n      {key}: {{ key: true }}\n    relates:\n      member: {{ card: {{ min: 0 }} }}\nplays:\n  {person_name}:\n    {relation_name}: [member]\n"
    );
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("wide-manager.yaml").unwrap(), yaml.as_str())])
            .unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
    )
    .unwrap();
    let installed = InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Rust,
            &ProjectionConfig::rust(),
            &[ProjectionHandler::rust_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    database.execute_raw(&format!("define attribute {key}, value string; attribute {tag}, value string; entity {person_name}, owns {key} @key, owns {tag} @card(0..), plays {relation_name}:member; relation {relation_name}, owns {key} @key, relates member @card(0..);"), TxType::Schema).await.unwrap();
    let registry = installed.manager_registry().unwrap();
    let person_type = TypeId::new(TypeKind::Entity, &person_name).unwrap();
    let relation_type = TypeId::new(TypeKind::Relation, &relation_name).unwrap();
    let key_id = AttributeId::new(&key).unwrap();
    let tag_id = AttributeId::new(&tag).unwrap();
    let member = RoleId::new(&relation_name, "member").unwrap();
    let raw_person = DynamicEntityManager::from_registry(
        &database,
        installed.manager_registry().unwrap(),
        registry.entity(&person_name).unwrap(),
    );
    let raw_relation = DynamicRelationManager::from_registry(
        &database,
        installed.manager_registry().unwrap(),
        registry.relation(&relation_name).unwrap(),
    );
    let fields = |value: &str, count: usize| -> DynamicAttributeMap {
        std::iter::once((key.clone(), AttributeValue::String(value.into())))
            .chain(
                (0..count)
                    .map(|index| (tag.clone(), AttributeValue::String(format!("v-{index:04}")))),
            )
            .collect()
    };
    let baseline_iid = raw_person
        .insert(&fields("baseline", 300))
        .await
        .unwrap()
        .item()
        .expect("one written root")
        .iid
        .clone()
        .unwrap();
    let baseline = raw_person.get_by_iid(&baseline_iid).await.unwrap().unwrap();
    let baseline_values = baseline
        .attributes
        .iter()
        .filter(|(name, _)| name == &tag)
        .map(|(_, value)| format!("{value:?}"))
        .collect::<std::collections::BTreeSet<_>>();
    let player_iids = raw_person
        .insert_many(
            &(0..300)
                .map(|index| fields(&format!("player-{index}"), 0))
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
    let role_inputs = player_iids
        .iter()
        .map(|iid| DynamicRolePlayerInput {
            role_name: "member".into(),
            player_type_name: person_name.clone(),
            iid: Some(iid.clone()),
            key: None,
        })
        .collect::<Vec<_>>();
    let baseline_relation_iid = raw_relation
        .insert(
            &vec![(key.clone(), AttributeValue::String("baseline-role".into()))],
            &role_inputs,
        )
        .await
        .unwrap()
        .item()
        .expect("one written root")
        .iid
        .clone()
        .unwrap();
    let mut baseline_role = raw_relation
        .get_by_iid(&baseline_relation_iid)
        .await
        .unwrap();
    assert_eq!(baseline_role.len(), 1);
    let baseline_role = baseline_role.remove(0);
    let baseline_players = baseline_role
        .role_players
        .iter()
        .map(|player| player.player_iid.clone().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    let wide_input = |relation: bool, value: &str| {
        if relation {
            relation_create(
                &installed,
                &relation_type,
                member.clone(),
                &key_id,
                value,
                player_iids
                    .iter()
                    .map(|iid| reference(&installed, &person_type, iid))
                    .collect(),
            )
        } else {
            ProjectedCreate::try_new(
                &installed,
                person_type.clone(),
                vec![
                    (
                        field(&person_type, &key_id),
                        vec![string_scalar(&installed, &key_id, value)],
                    ),
                    (
                        field(&person_type, &tag_id),
                        (0..300)
                            .map(|index| {
                                string_scalar(&installed, &tag_id, &format!("v-{index:04}"))
                            })
                            .collect(),
                    ),
                ],
                vec![],
            )
            .unwrap()
        }
    };
    for literal in [false, true] {
        let (counted, log) = counted_connection(&database, literal).await;
        let dynamic = DynamicEntityManager::from_registry(
            &counted,
            registry.clone(),
            registry.entity(&person_name).unwrap(),
        );
        for count in [254, 300] {
            *log.lock().unwrap() = DispatchLog::default();
            let value = format!("dynamic-{literal}-{count}");
            let iids = dynamic.insert_many(&[fields(&value, count)]).await.unwrap();
            assert_eq!(
                log.lock().unwrap().statements.len(),
                if count == 254 { 1 } else { 2 }
            );
            assert_eq!(log.lock().unwrap().opened, [TxType::Write]);
            assert_eq!(log.lock().unwrap().commits, 1);
            if count == 300 {
                let actual = raw_person.get_by_iid(&iids[0]).await.unwrap().unwrap();
                let actual = actual
                    .attributes
                    .iter()
                    .filter(|(name, _)| name == &tag)
                    .map(|(_, value)| format!("{value:?}"))
                    .collect::<std::collections::BTreeSet<_>>();
                assert_eq!(actual, baseline_values);
            }
        }
        let failed_key = format!("dynamic-field-final-failure-{literal}");
        *log.lock().unwrap() = DispatchLog {
            fail_at: Some(2),
            ..Default::default()
        };
        assert!(
            dynamic
                .insert_many(&[fields(&failed_key, 300)])
                .await
                .is_err()
        );
        assert!(
            raw_person
                .get(&[Filter::eq(&key, AttributeValue::String(failed_key))])
                .await
                .unwrap()
                .is_empty()
        );
        {
            let observed = log.lock().unwrap();
            assert_eq!(observed.statements.len(), 2);
            assert_eq!(observed.opened, [TxType::Write]);
            assert_eq!((observed.commits, observed.rollbacks), (0, 1));
        }
        let dynamic_relation = DynamicRelationManager::from_registry(
            &counted,
            registry.clone(),
            registry.relation(&relation_name).unwrap(),
        );
        for fail in [false, true] {
            let value = format!("dynamic-role-{literal}-{fail}");
            *log.lock().unwrap() = DispatchLog {
                fail_at: fail.then_some(3),
                ..Default::default()
            };
            let result = dynamic_relation
                .insert_many(&[(
                    vec![(key.clone(), AttributeValue::String(value.clone()))],
                    role_inputs.clone(),
                )])
                .await;
            {
                let observed = log.lock().unwrap();
                assert_eq!(observed.statements.len(), 3);
                assert_eq!(observed.opened, [TxType::Write]);
                assert_eq!(
                    (observed.commits, observed.rollbacks),
                    if fail { (0, 1) } else { (1, 0) }
                );
            }
            if fail {
                assert!(result.is_err());
                assert!(
                    raw_relation
                        .get(&[Filter::eq(&key, AttributeValue::String(value))])
                        .await
                        .unwrap()
                        .is_empty()
                );
            } else {
                let iid = result.unwrap().remove(0);
                let mut rows = raw_relation.get_by_iid(&iid).await.unwrap();
                assert_eq!(rows.len(), 1);
                let row = rows.remove(0);
                assert_eq!(
                    row.role_players
                        .iter()
                        .map(|player| player.player_iid.clone().unwrap())
                        .collect::<std::collections::BTreeSet<_>>(),
                    baseline_players
                );
            }
        }
        for relation in [false, true] {
            for fail in [false, true] {
                let value = format!("projected-{literal}-{relation}-{fail}");
                let input = wide_input(relation, &value);
                *log.lock().unwrap() = DispatchLog {
                    fail_at: fail.then_some(if relation { 3 } else { 2 }),
                    ..Default::default()
                };
                let batch = ProjectedBatch::try_new(
                    &installed,
                    if relation {
                        relation_type.clone()
                    } else {
                        person_type.clone()
                    },
                    ProjectedBatchOperation::Insert,
                    vec![ProjectedBatchRow::Create(input)],
                )
                .unwrap();
                let result = ProjectedBatchExecutor::new(&installed)
                    .execute(&counted, &batch, control())
                    .await;
                assert_eq!(
                    log.lock().unwrap().statements.len(),
                    if relation { 3 } else { 2 }
                );
                assert_eq!(log.lock().unwrap().opened, [TxType::Write]);
                if fail {
                    assert!(result.is_err());
                    {
                        let observed = log.lock().unwrap();
                        assert_eq!((observed.commits, observed.rollbacks), (0, 1));
                    }
                    if relation {
                        assert!(
                            raw_relation
                                .get(&[Filter::eq(&key, AttributeValue::String(value))])
                                .await
                                .unwrap()
                                .is_empty()
                        );
                    } else {
                        assert!(
                            raw_person
                                .get(&[Filter::eq(&key, AttributeValue::String(value))])
                                .await
                                .unwrap()
                                .is_empty()
                        );
                    }
                } else {
                    let output = things(result.unwrap());
                    assert_eq!(log.lock().unwrap().commits, 1);
                    if relation {
                        assert_eq!(
                            output[0].roles()[&member]
                                .iter()
                                .map(|player| player.iid().to_owned())
                                .collect::<std::collections::BTreeSet<_>>(),
                            baseline_players
                        );
                    } else {
                        assert_eq!(output[0].fields()[&field(&person_type, &tag_id)].len(), 300);
                    }
                }
            }
        }
        let crud = type_bridge_orm::ProjectedCrudExecutor::new(&installed);
        for relation in [false, true] {
            for borrowed in [false, true] {
                for fail in [false, true] {
                    let value = format!("controlled-put-{literal}-{relation}-{borrowed}-{fail}");
                    let input = wide_input(relation, &value);
                    *log.lock().unwrap() = DispatchLog {
                        fail_at: fail.then_some(if relation { 3 } else { 2 }),
                        ..Default::default()
                    };
                    let result = if borrowed {
                        let transaction = counted.transaction_context(TxType::Write).await.unwrap();
                        let result = if relation {
                            crud.put_relation_in_transaction_controlled(
                                &transaction,
                                &input,
                                QueryExecutionResourceLimits::default(),
                                AnswerCancellation::default(),
                            )
                            .await
                        } else {
                            crud.put_entity_in_transaction_controlled(
                                &transaction,
                                &input,
                                QueryExecutionResourceLimits::default(),
                                AnswerCancellation::default(),
                            )
                            .await
                        };
                        if fail {
                            assert!(transaction.commit_sdk().await.is_err());
                            transaction.rollback().await.unwrap();
                        } else {
                            transaction.commit_sdk().await.unwrap();
                        }
                        result
                    } else if relation {
                        crud.put_relation_controlled(
                            &counted,
                            &input,
                            QueryExecutionResourceLimits::default(),
                            AnswerCancellation::default(),
                        )
                        .await
                    } else {
                        crud.put_entity_controlled(
                            &counted,
                            &input,
                            QueryExecutionResourceLimits::default(),
                            AnswerCancellation::default(),
                        )
                        .await
                    };
                    {
                        let observed = log.lock().unwrap();
                        assert_eq!(
                            observed.statements.len(),
                            if relation { 3 } else { 2 },
                            "controlled Put relation={relation} borrowed={borrowed} fail={fail}"
                        );
                        assert_eq!(observed.opened, [TxType::Write]);
                        assert_eq!(
                            (observed.commits, observed.rollbacks),
                            if fail { (0, 1) } else { (1, 0) }
                        );
                    }
                    if fail {
                        assert!(result.is_err());
                        if relation {
                            assert!(
                                raw_relation
                                    .get(&[Filter::eq(&key, AttributeValue::String(value))])
                                    .await
                                    .unwrap()
                                    .is_empty()
                            );
                        } else {
                            assert!(
                                raw_person
                                    .get(&[Filter::eq(&key, AttributeValue::String(value))])
                                    .await
                                    .unwrap()
                                    .is_empty()
                            );
                        }
                    } else {
                        let result = result.unwrap();
                        let thing = result.item().expect("one written root");
                        if relation {
                            assert_eq!(
                                thing.roles()[&member]
                                    .iter()
                                    .map(|player| player.iid().to_owned())
                                    .collect::<std::collections::BTreeSet<_>>(),
                                baseline_players
                            );
                        } else {
                            let actual = thing.fields()[&field(&person_type, &tag_id)]
                                .iter()
                                .map(|value| {
                                    let CanonicalValue::String(text) = value.value() else {
                                        panic!("expected string field")
                                    };
                                    text.as_str().to_owned()
                                })
                                .collect::<std::collections::BTreeSet<_>>();
                            assert_eq!(
                                actual,
                                (0..300).map(|index| format!("v-{index:04}")).collect()
                            );
                        }
                        if !borrowed {
                            *log.lock().unwrap() = DispatchLog::default();
                            let repeated = if relation {
                                crud.put_relation_controlled(
                                    &counted,
                                    &input,
                                    QueryExecutionResourceLimits::default(),
                                    AnswerCancellation::default(),
                                )
                                .await
                            } else {
                                crud.put_entity_controlled(
                                    &counted,
                                    &input,
                                    QueryExecutionResourceLimits::default(),
                                    AnswerCancellation::default(),
                                )
                                .await
                            }
                            .unwrap();
                            assert_eq!(
                                repeated.item().expect("one written root").iid(),
                                thing.iid()
                            );
                            let observed = log.lock().unwrap();
                            assert_eq!(observed.statements.len(), if relation { 3 } else { 2 });
                            assert_eq!(observed.opened, [TxType::Write]);
                            assert_eq!((observed.commits, observed.rollbacks), (1, 0));
                        }
                    }
                }
            }
        }
        if !literal {
            for relation in [false, true] {
                let input = wide_input(relation, &format!("controlled-statement-limit-{relation}"));
                *log.lock().unwrap() = DispatchLog::default();
                let limits = QueryExecutionResourceLimits {
                    statements: Some(1),
                    ..Default::default()
                };
                let result = if relation {
                    crud.put_relation_controlled(
                        &counted,
                        &input,
                        limits,
                        AnswerCancellation::default(),
                    )
                    .await
                } else {
                    crud.put_entity_controlled(
                        &counted,
                        &input,
                        limits,
                        AnswerCancellation::default(),
                    )
                    .await
                };
                assert_eq!(
                    result.unwrap_err().code().as_str(),
                    "projected_batch_statement_limit"
                );
                let observed = log.lock().unwrap();
                assert!(observed.statements.is_empty());
                assert!(observed.opened.is_empty());
            }
        }
    }
    database.delete_database().await.unwrap();
}

struct PartialUpdateEntity {
    iid: Option<String>,
    fields: Vec<(&'static str, type_bridge_orm::AttributeValue)>,
}
impl type_bridge_orm::_entity::TypeBridgeEntity for PartialUpdateEntity {
    const TYPE_NAME: &'static str = "partial-person";
    fn owned_attributes() -> &'static [type_bridge_orm::_entity::OwnedAttributeInfo] {
        &[]
    }
    fn iid(&self) -> Option<&str> {
        self.iid.as_deref()
    }
    fn set_iid(&mut self, iid: String) {
        self.iid = Some(iid);
    }
    fn to_attribute_values(&self) -> Vec<(&'static str, type_bridge_orm::AttributeValue)> {
        self.fields.clone()
    }
    fn from_document(
        document: &serde_json::Map<String, serde_json::Value>,
    ) -> type_bridge_orm::Result<Self> {
        let mut fields = Vec::new();
        for name in [
            "partial-key",
            "partial-name",
            "partial-tag",
            "partial-email",
        ] {
            match document.get(name) {
                Some(serde_json::Value::String(value)) => {
                    fields.push((name, type_bridge_orm::AttributeValue::String(value.clone())))
                }
                Some(serde_json::Value::Array(values)) => {
                    for value in values {
                        fields.push((
                            name,
                            type_bridge_orm::AttributeValue::String(
                                value.as_str().expect("string fixture field").into(),
                            ),
                        ));
                    }
                }
                None => {}
                other => panic!("unexpected fixture field {name}: {other:?}"),
            }
        }
        Ok(Self { iid: None, fields })
    }
}

#[tokio::test]
#[ignore = "requires a live TypeDB server"]
async fn conditional_partial_update_matches_looped_answers_live() {
    use type_bridge_orm::_dynamic::DynamicAttributeMap;
    use type_bridge_orm::_manager::DynamicEntityManager;
    use type_bridge_orm::AttributeValue;
    let _guard = crate::common::integration_test_guard().await;
    let database = setup_db().await;
    database.delete_database().await.unwrap();
    database.create_database().await.unwrap();
    let key = String::from("partial-key");
    let name = String::from("partial-name");
    let tag = String::from("partial-tag");
    let person = String::from("partial-person");
    let email = String::from("partial-email");
    let yaml = format!(
        "format: typebridge.schema/v2\nattributes:\n  {key}: {{ value: string }}\n  {name}: {{ value: string }}\n  {tag}: {{ value: string }}\n  {email}: {{ value: string }}\nentities:\n  {person}:\n    owns:\n      {key}: {{ key: true }}\n      {name}: {{ card: {{ min: 0, max: 1 }} }}\n      {tag}: {{ card: {{ min: 0 }} }}\n      {email}: {{ unique: true, card: {{ min: 0, max: 1 }} }}\n"
    );
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("partial.yaml").unwrap(), yaml.as_str())])
            .unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
    )
    .unwrap();
    let installed = InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Rust,
            &ProjectionConfig::rust(),
            &[ProjectionHandler::rust_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    database.execute_raw(&format!("define attribute {key}, value string; attribute {name}, value string; attribute {tag}, value string; attribute {email}, value string; entity {person}, owns {key} @key, owns {name} @card(0..1), owns {tag} @card(0..), owns {email} @unique @card(0..1);"), TxType::Schema).await.unwrap();
    let registry = installed.manager_registry().unwrap();
    let manager = DynamicEntityManager::from_registry(
        &database,
        registry.clone(),
        registry.entity(&person).unwrap(),
    );
    let initial = |value: &str| -> DynamicAttributeMap {
        vec![
            (key.clone(), AttributeValue::String(value.into())),
            (name.clone(), AttributeValue::String("old-name".into())),
            (tag.clone(), AttributeValue::String("old-1".into())),
            (tag.clone(), AttributeValue::String("old-2".into())),
        ]
    };
    let iids = manager
        .insert_many(&[initial("a"), initial("b"), initial("c")])
        .await
        .unwrap();
    let a = vec![(name.clone(), AttributeValue::String("new-name".into()))];
    let b = vec![
        (key.clone(), AttributeValue::String("b".into())),
        (tag.clone(), AttributeValue::String("x".into())),
        (tag.clone(), AttributeValue::String("y".into())),
    ];
    let tx = database.transaction_context(TxType::Write).await.unwrap();
    let looped = DynamicEntityManager::with_registry_transaction(
        tx.clone(),
        registry.clone(),
        registry.entity(&person).unwrap(),
    );
    looped.update(Some(&iids[0]), &a).await.unwrap();
    looped.update(None, &b).await.unwrap();
    looped.update(Some(&iids[2]), &vec![]).await.unwrap();
    tx.commit().await.unwrap();
    let mut expected = Vec::new();
    for iid in &iids {
        let row = manager.get_by_iid_exact(iid).await.unwrap().unwrap();
        expected.push(
            row.attributes
                .iter()
                .map(|(name, value)| format!("{name}={value:?}"))
                .collect::<std::collections::BTreeSet<_>>(),
        );
    }
    for (iid, value) in iids.iter().zip(["a", "b", "c"]) {
        manager.update_exact(iid, &initial(value)).await.unwrap();
    }
    let (counted, log) = counted_connection(&database, false).await;
    let query = format!(
        r#"match
{{ $root isa! {person}, iid {aid}; let $row = 0; }} or
{{ $root isa! {person}, has {key} "b"; let $row = 1; }} or
{{ $root isa! {person}, iid {cid}; let $row = 2; }};
let $root_iid = iid($root);
try {{ $name_owner isa! {person}; iid($name_owner) == $root_iid; $row == 0; }};
try {{ $tag_owner isa! {person}; iid($tag_owner) == $root_iid; $row == 1; }};
match try {{ $name_owner has {name} $old_name; $old_name isa! {name}; }};
try {{ $tag_owner has {tag} $old_tag; $old_tag isa! {tag}; }};
delete try {{ has $old_name of $name_owner; }}; try {{ has $old_tag of $tag_owner; }};
insert try {{ $name_owner has {name} "new-name"; }};
try {{ $tag_owner has {tag} "x"; }}; try {{ $tag_owner has {tag} "y"; }};
select $root; distinct;
"#,
        aid = iids[0],
        cid = iids[2]
    );
    counted.execute_raw(&query, TxType::Write).await.unwrap();
    {
        let observed = log.lock().unwrap();
        assert_eq!(observed.statements.len(), 1);
        assert_eq!(observed.opened, [TxType::Write]);
        assert_eq!(observed.commits, 1);
    }
    for (iid, expected) in iids.iter().zip(&expected) {
        let row = manager.get_by_iid_exact(iid).await.unwrap().unwrap();
        let actual = row
            .attributes
            .iter()
            .map(|(name, value)| format!("{name}={value:?}"))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(&actual, expected);
    }
    eprintln!(
        "conditional partial update: one statement/transaction; IID A name only, key B tags only, omitted C unchanged; equals looped updates"
    );
    for (iid, value) in iids.iter().zip(["a", "b", "c"]) {
        manager.update_exact(iid, &initial(value)).await.unwrap();
    }
    let put_items = vec![
        vec![
            (key.clone(), AttributeValue::String("a".into())),
            (name.clone(), AttributeValue::String("new-name".into())),
        ],
        b.clone(),
        vec![(key.clone(), AttributeValue::String("c".into()))],
        vec![
            (key.clone(), AttributeValue::String("new".into())),
            (name.clone(), AttributeValue::String("fresh".into())),
        ],
    ];
    let tx = database.transaction_context(TxType::Write).await.unwrap();
    let looped = DynamicEntityManager::with_registry_transaction(
        tx.clone(),
        registry.clone(),
        registry.entity(&person).unwrap(),
    );
    let mut old_put = Vec::new();
    for fields in &put_items {
        old_put.push(
            looped
                .put(fields)
                .await
                .unwrap()
                .item()
                .expect("one written root")
                .iid
                .clone()
                .unwrap(),
        );
    }
    tx.commit().await.unwrap();
    let mut put_expected = Vec::new();
    for iid in &old_put {
        put_expected.push(
            manager
                .get_by_iid_exact(iid)
                .await
                .unwrap()
                .unwrap()
                .attributes
                .iter()
                .map(|(name, value)| format!("{name}={value:?}"))
                .collect::<std::collections::BTreeSet<_>>(),
        );
    }
    manager.delete_by_iid_exact(&old_put[3]).await.unwrap();
    for literal in [false, true] {
        for (iid, value) in iids.iter().zip(["a", "b", "c"]) {
            manager.update_exact(iid, &initial(value)).await.unwrap();
        }
        let (put_db, put_log) = counted_connection(&database, literal).await;
        let put_manager = DynamicEntityManager::from_registry(
            &put_db,
            registry.clone(),
            registry.entity(&person).unwrap(),
        );
        let result = put_manager.put_many(&put_items).await.unwrap();
        {
            let observed = put_log.lock().unwrap();
            assert_eq!(observed.statements.len(), 1);
            assert_eq!(observed.opened, [TxType::Write]);
            assert_eq!(observed.commits, 1);
        }
        assert_eq!(result[..3], iids[..]);
        assert_ne!(result[3], old_put[3]);
        for (iid, expected) in result.iter().zip(&put_expected) {
            let row = manager.get_by_iid_exact(iid).await.unwrap().unwrap();
            assert_eq!(
                &row.attributes
                    .iter()
                    .map(|(name, value)| format!("{name}={value:?}"))
                    .collect::<std::collections::BTreeSet<_>>(),
                expected
            );
        }
        manager.delete_by_iid_exact(&result[3]).await.unwrap();
    }
    for (iid, value) in iids.iter().zip(["a", "b", "c"]) {
        manager.update_exact(iid, &initial(value)).await.unwrap();
    }
    eprintln!(
        "keyed-partial entity put_many: Given/literal one statement/TX, hit/new identities and all committed fields equal per-row put"
    );
    let typed = type_bridge_orm::_manager::EntityManager::<PartialUpdateEntity>::from_registry(
        &counted,
        registry.clone(),
    )
    .unwrap();
    let updates = vec![
        PartialUpdateEntity {
            iid: Some(iids[0].clone()),
            fields: vec![("partial-name", AttributeValue::String("new-name".into()))],
        },
        PartialUpdateEntity {
            iid: None,
            fields: vec![
                ("partial-key", AttributeValue::String("b".into())),
                ("partial-tag", AttributeValue::String("x".into())),
                ("partial-tag", AttributeValue::String("y".into())),
            ],
        },
        PartialUpdateEntity {
            iid: Some(iids[2].clone()),
            fields: vec![],
        },
    ];
    *log.lock().unwrap() = DispatchLog::default();
    typed.update_many(&updates).await.unwrap();
    {
        let observed = log.lock().unwrap();
        assert_eq!(observed.statements.len(), 1);
        assert_eq!(observed.opened, [TxType::Write]);
        assert_eq!(observed.commits, 1);
    }
    for (iid, expected) in iids.iter().zip(&expected) {
        let row = manager.get_by_iid_exact(iid).await.unwrap().unwrap();
        assert_eq!(
            &row.attributes
                .iter()
                .map(|(name, value)| format!("{name}={value:?}"))
                .collect::<std::collections::BTreeSet<_>>(),
            expected
        );
    }
    *log.lock().unwrap() = DispatchLog::default();
    let missing = format!("{}ff", &iids[0][..iids[0].len() - 2]);
    let repeated = vec![
        PartialUpdateEntity {
            iid: Some(iids[0].clone()),
            fields: vec![("partial-name", AttributeValue::String("first".into()))],
        },
        PartialUpdateEntity {
            iid: Some(missing),
            fields: vec![("partial-name", AttributeValue::String("ignored".into()))],
        },
        PartialUpdateEntity {
            iid: Some(iids[0].clone()),
            fields: vec![("partial-name", AttributeValue::String("last".into()))],
        },
    ];
    typed.update_many(&repeated).await.unwrap();
    {
        let observed = log.lock().unwrap();
        assert_eq!(observed.statements.len(), 1);
        assert_eq!(observed.opened, [TxType::Write]);
        assert_eq!(observed.commits, 1);
    }
    assert!(
        manager
            .get_by_iid_exact(&iids[0])
            .await
            .unwrap()
            .unwrap()
            .attributes
            .iter()
            .any(|(name, value)| name == "partial-name"
                && value == &AttributeValue::String("last".into()))
    );
    manager
        .update(
            Some(&iids[0]),
            &vec![(email.clone(), AttributeValue::String("ea".into()))],
        )
        .await
        .unwrap();
    manager
        .update(
            Some(&iids[1]),
            &vec![(email.clone(), AttributeValue::String("eb".into()))],
        )
        .await
        .unwrap();
    let tx = database.transaction_context(TxType::Write).await.unwrap();
    let looped = DynamicEntityManager::with_registry_transaction(
        tx.clone(),
        registry.clone(),
        registry.entity(&person).unwrap(),
    );
    let old_failure = looped
        .update(
            Some(&iids[0]),
            &vec![(email.clone(), AttributeValue::String("eb".into()))],
        )
        .await
        .unwrap_err()
        .failure()
        .unwrap();
    tx.rollback().await.unwrap();
    let swap = vec![
        PartialUpdateEntity {
            iid: Some(iids[0].clone()),
            fields: vec![("partial-email", AttributeValue::String("eb".into()))],
        },
        PartialUpdateEntity {
            iid: Some(iids[1].clone()),
            fields: vec![("partial-email", AttributeValue::String("ea".into()))],
        },
    ];
    *log.lock().unwrap() = DispatchLog::default();
    let failure = typed
        .update_many(&swap)
        .await
        .unwrap_err()
        .failure()
        .unwrap();
    assert_eq!(failure.code(), old_failure.code());
    assert_eq!(failure.code(), Some("CNT9"));
    {
        let observed = log.lock().unwrap();
        assert_eq!(observed.statements.len(), 1);
        assert_eq!((observed.commits, observed.rollbacks), (0, 1));
    }
    for (iid, email) in iids.iter().take(2).zip(["ea", "eb"]) {
        assert!(
            manager
                .get_by_iid_exact(iid)
                .await
                .unwrap()
                .unwrap()
                .attributes
                .iter()
                .any(|(name, value)| name == "partial-email"
                    && value == &AttributeValue::String(email.into()))
        );
    }
    for literal in [false, true] {
        let (wide_counted, wide_log) = counted_connection(&database, literal).await;
        let wide_typed =
            type_bridge_orm::_manager::EntityManager::<PartialUpdateEntity>::from_registry(
                &wide_counted,
                registry.clone(),
            )
            .unwrap();
        let wide_updates = vec![
            PartialUpdateEntity {
                iid: Some("0xffff".into()),
                fields: (0..300)
                    .map(|index| {
                        (
                            "partial-tag",
                            AttributeValue::String(format!("missing-wide-{index}")),
                        )
                    })
                    .collect(),
            },
            PartialUpdateEntity {
                iid: Some(iids[0].clone()),
                fields: vec![(
                    "partial-name",
                    AttributeValue::String("after-wide-missing".into()),
                )],
            },
        ];
        let tx = database.transaction_context(TxType::Write).await.unwrap();
        let looped = DynamicEntityManager::with_registry_transaction(
            tx.clone(),
            installed.manager_registry().unwrap(),
            registry.entity(&person).unwrap(),
        );
        for update in &wide_updates {
            let fields = update
                .fields
                .iter()
                .map(|(name, value)| ((*name).to_owned(), value.clone()))
                .collect();
            looped.update(update.iid.as_deref(), &fields).await.unwrap();
        }
        let expected = looped
            .get_by_iid_exact(&iids[0])
            .await
            .unwrap()
            .unwrap()
            .attributes;
        tx.rollback().await.unwrap();
        wide_typed.update_many(&wide_updates).await.unwrap();
        assert_eq!(
            manager
                .get_by_iid_exact(&iids[0])
                .await
                .unwrap()
                .unwrap()
                .attributes,
            expected
        );
        assert!(manager.get_by_iid_exact("0xffff").await.unwrap().is_none());
        let observed = wide_log.lock().unwrap();
        assert_eq!(
            observed.statements.len(),
            2,
            "the absent seed skips its attach piece and the later row still runs"
        );
        assert_eq!(observed.opened, [TxType::Write]);
        assert_eq!((observed.commits, observed.rollbacks), (1, 0));
    }
    *log.lock().unwrap() = DispatchLog::default();
    typed.delete_many(&updates).await.unwrap();
    {
        let observed = log.lock().unwrap();
        assert_eq!(observed.statements.len(), 1);
        assert_eq!(observed.opened, [TxType::Write]);
        assert_eq!(observed.commits, 1);
    }
    for iid in &iids {
        assert!(manager.get_by_iid_exact(iid).await.unwrap().is_none());
    }
    let mut inserts = vec![
        PartialUpdateEntity {
            iid: None,
            fields: vec![("partial-key", AttributeValue::String("d".into()))],
        },
        PartialUpdateEntity {
            iid: None,
            fields: vec![("partial-key", AttributeValue::String("e".into()))],
        },
    ];
    *log.lock().unwrap() = DispatchLog::default();
    let created = typed.insert_many(&mut inserts).await.unwrap();
    {
        let observed = log.lock().unwrap();
        assert_eq!(observed.statements.len(), 1);
        assert_eq!(observed.opened, [TxType::Write]);
        assert_eq!(observed.commits, 1);
    }
    assert_eq!(
        inserts
            .iter()
            .map(|model| model.iid.clone().unwrap())
            .collect::<Vec<_>>(),
        created
    );
    eprintln!(
        "generic entity insert_many/update_many/delete_many: one statement and transaction each; mixed IID/key partial updates equal looped answers"
    );
    let selectors = vec![
        PartialUpdateEntity {
            iid: None,
            fields: vec![("partial-key", AttributeValue::String("e".into()))],
        },
        PartialUpdateEntity {
            iid: Some(format!("{}ff", &created[0][..created[0].len() - 2])),
            fields: vec![],
        },
        PartialUpdateEntity {
            iid: Some(created[0].clone()),
            fields: vec![("partial-name", AttributeValue::String("stale".into()))],
        },
    ];
    *log.lock().unwrap() = DispatchLog {
        fail_at: Some(2),
        ..Default::default()
    };
    assert!(typed.delete_many_returning(&selectors).await.is_err());
    {
        let observed = log.lock().unwrap();
        assert_eq!(observed.statements.len(), 2);
        assert_eq!(observed.opened, [TxType::Write]);
        assert_eq!((observed.commits, observed.rollbacks), (0, 1));
    }
    for iid in &created {
        assert!(manager.get_by_iid_exact(iid).await.unwrap().is_some());
    }
    *log.lock().unwrap() = DispatchLog::default();
    let snapshots = typed.delete_many_returning(&selectors).await.unwrap();
    {
        let observed = log.lock().unwrap();
        assert_eq!(observed.statements.len(), 2);
        assert_eq!(observed.opened, [TxType::Write]);
        assert_eq!((observed.commits, observed.rollbacks), (1, 0));
    }
    assert_eq!(
        snapshots
            .iter()
            .map(|model| model.iid.clone().unwrap())
            .collect::<Vec<_>>(),
        vec![created[1].clone(), created[0].clone()]
    );
    assert!(
        snapshots[0]
            .fields
            .contains(&("partial-key", AttributeValue::String("e".into())))
    );
    assert!(
        snapshots[1]
            .fields
            .contains(&("partial-key", AttributeValue::String("d".into())))
    );
    for iid in &created {
        assert!(manager.get_by_iid_exact(iid).await.unwrap().is_none());
    }
    eprintln!(
        "generic returning deletion: mixed key/IID selectors, missing omitted, requested order, capture+delete only, final-delete rollback"
    );
    database.delete_database().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a live TypeDB server"]
async fn nonexact_put_many_preserves_sequential_answers_live() {
    use type_bridge_orm::_dynamic::{DynamicAttributeMap, DynamicRolePlayerInput};
    use type_bridge_orm::_manager::{DynamicEntityManager, DynamicRelationManager};
    use type_bridge_orm::{AttributeValue, Filter};
    let _guard = crate::common::integration_test_guard().await;
    let database = setup_db().await;
    database.delete_database().await.unwrap();
    database.create_database().await.unwrap();
    let yaml = r#"format: typebridge.schema/v2
attributes:
  seq-key: { value: string }
  seq-name: { value: string }
  seq-tag: { value: string }
entities:
  seq-person:
    owns:
      seq-key: { key: true }
  seq-note:
    owns:
      seq-name: { card: { min: 0, max: 1 } }
      seq-tag: { card: { min: 0 } }
  seq-keyed:
    owns:
      seq-key: { key: true }
      seq-name: { card: { min: 0, max: 1 } }
      seq-tag: { card: { min: 0 } }
relations:
  seq-edge:
    owns:
      seq-key: { key: true }
    relates:
      node: { card: { min: 0, max: 1 } }
      prior: { card: { min: 0, max: 1 } }
plays:
  seq-person:
    seq-edge: [node]
  seq-edge:
    seq-edge: [prior]
"#;
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("sequence.yaml").unwrap(), yaml)]).unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
    )
    .unwrap();
    let installed = InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Rust,
            &ProjectionConfig::rust(),
            &[ProjectionHandler::rust_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    database.execute_raw("define attribute seq-key, value string; attribute seq-name, value string; attribute seq-tag, value string; entity seq-person, owns seq-key @key, plays seq-edge:node; entity seq-note, owns seq-name @card(0..1), owns seq-tag @card(0..); entity seq-keyed, owns seq-key @key, owns seq-name @card(0..1), owns seq-tag @card(0..); relation seq-edge, owns seq-key @key, relates node @card(0..1), relates prior @card(0..1), plays seq-edge:prior;",TxType::Schema).await.unwrap();
    let registry = installed.manager_registry().unwrap();
    let fields = |values: &[(&str, &str)]| -> DynamicAttributeMap {
        values
            .iter()
            .map(|(field, value)| ((*field).into(), AttributeValue::String((*value).into())))
            .collect()
    };
    for literal in [false, true] {
        for (case, kind, items, puts) in [
            (
                "repeat-keyless",
                "seq-note",
                vec![
                    fields(&[("seq-name", "repeat")]),
                    fields(&[("seq-name", "repeat")]),
                ],
                2,
            ),
            (
                "interleaved",
                "seq-note",
                vec![
                    fields(&[("seq-name", "a")]),
                    fields(&[("seq-tag", "t")]),
                    fields(&[("seq-name", "a")]),
                    fields(&[("seq-name", "b")]),
                ],
                4,
            ),
            (
                "repeat-keyed",
                "seq-keyed",
                vec![
                    fields(&[("seq-key", "repeat"), ("seq-name", "first")]),
                    fields(&[("seq-key", "repeat"), ("seq-name", "last")]),
                ],
                2,
            ),
            (
                "independent-keyless",
                "seq-note",
                vec![
                    fields(&[("seq-name", "independent-a")]),
                    fields(&[("seq-name", "independent-b")]),
                ],
                1,
            ),
            (
                "independent-keyed",
                "seq-keyed",
                vec![
                    fields(&[("seq-key", "a"), ("seq-name", "a")]),
                    fields(&[("seq-key", "b"), ("seq-name", "b")]),
                ],
                1,
            ),
        ] {
            let descriptor = registry.entity(kind).unwrap();
            let manager = DynamicEntityManager::from_registry(
                &database,
                registry.clone(),
                descriptor.clone(),
            );
            let tx = database.transaction_context(TxType::Write).await.unwrap();
            let looped = DynamicEntityManager::with_registry_transaction(
                tx.clone(),
                registry.clone(),
                descriptor.clone(),
            );
            let mut old = Vec::new();
            for item in &items {
                old.push(
                    looped
                        .put(item)
                        .await
                        .unwrap()
                        .item()
                        .expect("one written root")
                        .iid
                        .clone()
                        .unwrap(),
                );
            }
            tx.commit().await.unwrap();
            let mut expected = Vec::new();
            for iid in &old {
                expected.push(
                    manager
                        .get_by_iid_exact(iid)
                        .await
                        .unwrap()
                        .unwrap()
                        .attributes
                        .iter()
                        .map(|(name, value)| format!("{name}={value:?}"))
                        .collect::<std::collections::BTreeSet<_>>(),
                );
            }
            let unique = old
                .iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            manager.delete_many_by_iid_exact(&unique).await.unwrap();
            let (counted, log) = counted_connection(&database, literal).await;
            let many = DynamicEntityManager::from_registry(&counted, registry.clone(), descriptor);
            let actual = many
                .put_many(&items)
                .await
                .unwrap_or_else(|error| panic!("{case} literal={literal}: {error:?}"));
            {
                let observed = log.lock().unwrap();
                assert_eq!(observed.statements.len(), 1, "{case}");
                assert_eq!(observed.opened, [TxType::Write]);
                assert_eq!(observed.commits, 1);
                assert_eq!(
                    observed.statements[0]
                        .split_whitespace()
                        .filter(|word| *word == "put")
                        .count(),
                    puts,
                    "{case}"
                );
            }
            assert_eq!(actual.len(), old.len());
            for left in 0..actual.len() {
                for right in 0..actual.len() {
                    assert_eq!(
                        actual[left] == actual[right],
                        old[left] == old[right],
                        "{case}: IID aliasing"
                    );
                }
            }
            for (iid, expected) in actual.iter().zip(&expected) {
                assert_eq!(
                    &manager
                        .get_by_iid_exact(iid)
                        .await
                        .unwrap()
                        .unwrap()
                        .attributes
                        .iter()
                        .map(|(name, value)| format!("{name}={value:?}"))
                        .collect::<std::collections::BTreeSet<_>>(),
                    expected,
                    "{case}"
                );
            }
            let unique = actual
                .iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            manager.delete_many_by_iid_exact(&unique).await.unwrap();
        }
        for fail in [false, true] {
            let key = if fail { "wide-failure" } else { "wide-success" };
            let items = (0..130)
                .map(|index| {
                    vec![
                        ("seq-key".into(), AttributeValue::String(key.into())),
                        (
                            "seq-name".into(),
                            AttributeValue::String(format!("value-{index}")),
                        ),
                    ]
                })
                .collect::<Vec<_>>();
            let (counted, log) = counted_connection(&database, literal).await;
            *log.lock().unwrap() = DispatchLog {
                fail_at: fail.then_some(2),
                ..Default::default()
            };
            let manager = DynamicEntityManager::from_registry(
                &counted,
                registry.clone(),
                registry.entity("seq-keyed").unwrap(),
            );
            let result = manager.put_many(&items).await;
            {
                let observed = log.lock().unwrap();
                assert_eq!(observed.statements.len(), 2);
                assert_eq!(observed.opened, [TxType::Write]);
                assert_eq!(
                    (observed.commits, observed.rollbacks),
                    if fail { (0, 1) } else { (1, 0) }
                );
            }
            let reader = DynamicEntityManager::from_registry(
                &database,
                registry.clone(),
                registry.entity("seq-keyed").unwrap(),
            );
            if fail {
                assert!(result.is_err());
                assert!(
                    reader
                        .get(&[Filter::eq("seq-key", AttributeValue::String(key.into()))])
                        .await
                        .unwrap()
                        .is_empty()
                );
            } else {
                let ids = result.unwrap();
                assert_eq!(ids.len(), 130);
                assert!(ids.iter().all(|iid| iid == &ids[0]));
                let row = reader.get_by_iid_exact(&ids[0]).await.unwrap().unwrap();
                assert!(row.attributes.iter().any(|(name, value)| name == "seq-name"
                    && value == &AttributeValue::String("value-129".into())));
                reader.delete_by_iid_exact(&ids[0]).await.unwrap();
            }
        }
        let person = DynamicEntityManager::from_registry(
            &database,
            registry.clone(),
            registry.entity("seq-person").unwrap(),
        );
        let person_iid = person
            .put(&fields(&[("seq-key", "p")]))
            .await
            .unwrap()
            .item()
            .expect("one written root")
            .iid
            .clone()
            .unwrap();
        let edges = DynamicRelationManager::from_registry(
            &database,
            registry.clone(),
            registry.relation("seq-edge").unwrap(),
        );
        let node = DynamicRolePlayerInput {
            role_name: "node".into(),
            player_type_name: "seq-person".into(),
            iid: Some(person_iid),
            key: None,
        };
        let prior = |key: &str| DynamicRolePlayerInput {
            role_name: "prior".into(),
            player_type_name: "seq-edge".into(),
            iid: None,
            key: Some(("seq-key".into(), AttributeValue::String(key.into()))),
        };
        // Exact many calls must see relation rows written earlier in this call.
        for put_exact in [false, true] {
            let prefix = if put_exact {
                "exact-chain"
            } else {
                "insert-chain"
            };
            let keys = (0..3)
                .map(|index| format!("{prefix}-{index}"))
                .collect::<Vec<_>>();
            let inputs = vec![
                (fields(&[("seq-key", &keys[0])]), vec![node.clone()]),
                (fields(&[("seq-key", &keys[1])]), vec![prior(&keys[0])]),
                (fields(&[("seq-key", &keys[2])]), vec![prior(&keys[1])]),
            ];
            let tx = database.transaction_context(TxType::Write).await.unwrap();
            let looped = DynamicRelationManager::with_registry_transaction(
                tx.clone(),
                registry.clone(),
                registry.relation("seq-edge").unwrap(),
            );
            let mut loop_ids = Vec::new();
            for (fields, players) in &inputs {
                loop_ids.push(if put_exact {
                    looped
                        .put_exact(fields, players)
                        .await
                        .unwrap()
                        .item()
                        .expect("one written root")
                        .iid
                        .clone()
                        .unwrap()
                } else {
                    looped
                        .insert(fields, players)
                        .await
                        .unwrap()
                        .item()
                        .expect("one written root")
                        .iid
                        .clone()
                        .unwrap()
                });
            }
            let mut expected = Vec::new();
            for iid in &loop_ids {
                let rows = looped.get_by_iid_exact(iid).await.unwrap();
                let mut document = serde_json::to_value(&rows[0]).unwrap();
                document["iid"] = serde_json::Value::Null;
                for player in document["role_players"].as_array_mut().unwrap() {
                    player["player_iid"] = serde_json::Value::Null;
                }
                expected.push(document);
            }
            tx.rollback().await.unwrap();
            let (counted, log) = counted_connection(&database, literal).await;
            let many = DynamicRelationManager::from_registry(
                &counted,
                registry.clone(),
                registry.relation("seq-edge").unwrap(),
            );
            let ids = if put_exact {
                many.put_many_exact(&inputs).await.unwrap()
            } else {
                many.insert_many(&inputs).await.unwrap()
            };
            let mut actual = Vec::new();
            for (index, iid) in ids.iter().enumerate() {
                let rows = edges.get_by_iid_exact(iid).await.unwrap();
                if index > 0 {
                    assert_eq!(
                        rows[0].role_players[0].player_iid.as_deref(),
                        Some(ids[index - 1].as_str())
                    );
                }
                let mut document = serde_json::to_value(&rows[0]).unwrap();
                document["iid"] = serde_json::Value::Null;
                for player in document["role_players"].as_array_mut().unwrap() {
                    player["player_iid"] = serde_json::Value::Null;
                }
                actual.push(document);
            }
            assert_eq!(
                actual, expected,
                "{prefix}: batched answers equal the old loop"
            );
            {
                let observed = log.lock().unwrap();
                assert_eq!(observed.statements.len(), 1);
                assert_eq!(observed.opened, [TxType::Write]);
                assert_eq!((observed.commits, observed.rollbacks), (1, 0));
            }
            edges.delete_many_by_iid_exact(&ids).await.unwrap();
            let anchor = edges
                .put_exact(
                    &fields(&[("seq-key", "explicit-anchor")]),
                    std::slice::from_ref(&node),
                )
                .await
                .unwrap()
                .item()
                .expect("one written root")
                .iid
                .clone()
                .unwrap();
            let relation = TypeId::new(TypeKind::Relation, "seq-edge").unwrap();
            let key = AttributeId::new("seq-key").unwrap();
            let prior_role = RoleId::new("seq-edge", "prior").unwrap();
            let operation = if put_exact {
                ProjectedBatchOperation::Put
            } else {
                ProjectedBatchOperation::Insert
            };
            let batch = ProjectedBatch::try_new(
                &installed,
                relation.clone(),
                operation,
                (0..2)
                    .map(|index| {
                        ProjectedBatchRow::Create(relation_create(
                            &installed,
                            &relation,
                            prior_role.clone(),
                            &key,
                            &format!("projected-{prefix}-{index}"),
                            vec![key_reference(
                                &installed,
                                &relation,
                                &key,
                                "explicit-anchor",
                            )],
                        ))
                    })
                    .collect(),
            )
            .unwrap();
            *log.lock().unwrap() = DispatchLog::default();
            let written = things(
                ProjectedBatchExecutor::new(&installed)
                    .execute(&counted, &batch, control())
                    .await
                    .unwrap(),
            );
            for thing in &written {
                assert_role_players(thing, &prior_role, &[(&relation, &anchor)]);
            }
            {
                let observed = log.lock().unwrap();
                assert_eq!(observed.statements.len(), 1);
                assert_eq!(observed.opened, [TxType::Write]);
                assert_eq!((observed.commits, observed.rollbacks), (1, 0));
            }
            let mut cleanup = written
                .iter()
                .map(|thing| thing.iid().to_owned())
                .collect::<Vec<_>>();
            cleanup.push(anchor);
            edges.delete_many_by_iid_exact(&cleanup).await.unwrap();
        }
        let chain = vec![
            (fields(&[("seq-key", "a")]), vec![node.clone()]),
            (fields(&[("seq-key", "b")]), vec![prior("a")]),
            (fields(&[("seq-key", "c")]), vec![prior("b")]),
        ];
        let a = edges
            .put(&chain[0].0, &chain[0].1)
            .await
            .unwrap()
            .item()
            .expect("one written root")
            .iid
            .clone()
            .unwrap();
        let tx = database.transaction_context(TxType::Write).await.unwrap();
        let looped = DynamicRelationManager::with_registry_transaction(
            tx.clone(),
            registry.clone(),
            registry.relation("seq-edge").unwrap(),
        );
        let mut old = Vec::new();
        for (fields, players) in &chain {
            old.push(
                looped
                    .put(fields, players)
                    .await
                    .unwrap()
                    .item()
                    .expect("one written root")
                    .iid
                    .clone()
                    .unwrap(),
            );
        }
        tx.commit().await.unwrap();
        assert_eq!(old[0], a);
        edges.delete_many_by_iid_exact(&old[1..]).await.unwrap();
        let (counted, log) = counted_connection(&database, literal).await;
        let many = DynamicRelationManager::from_registry(
            &counted,
            registry.clone(),
            registry.relation("seq-edge").unwrap(),
        );
        let result = many.put_many(&chain).await.unwrap();
        {
            let observed = log.lock().unwrap();
            assert_eq!(observed.statements.len(), 1);
            assert_eq!(observed.opened, [TxType::Write]);
            assert_eq!(observed.commits, 1);
        }
        assert_eq!(result[0], a);
        assert_eq!(
            result
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            3
        );
        for row in 1..3 {
            let stored = edges.get_by_iid_exact(&result[row]).await.unwrap();
            assert_eq!(stored.len(), 1);
            assert_eq!(stored[0].role_players.len(), 1);
            assert_eq!(
                stored[0].role_players[0].player_iid.as_deref(),
                Some(result[row - 1].as_str())
            );
        }
        edges.delete_many_by_iid_exact(&result[1..]).await.unwrap();
        let conflict = vec![
            (fields(&[("seq-key", "temporary")]), vec![prior("a")]),
            (fields(&[("seq-key", "a")]), vec![prior("temporary")]),
        ];
        let tx = database.transaction_context(TxType::Write).await.unwrap();
        let looped = DynamicRelationManager::with_registry_transaction(
            tx.clone(),
            registry.clone(),
            registry.relation("seq-edge").unwrap(),
        );
        looped
            .put(&conflict[0].0, &conflict[0].1)
            .await
            .unwrap()
            .item()
            .expect("one written root")
            .iid
            .clone()
            .unwrap();
        let before = looped
            .put(&conflict[1].0, &conflict[1].1)
            .await
            .unwrap_err()
            .failure()
            .unwrap();
        tx.rollback().await.unwrap();
        *log.lock().unwrap() = DispatchLog::default();
        let after = many
            .put_many(&conflict)
            .await
            .unwrap_err()
            .failure()
            .unwrap();
        assert_eq!(after.code(), before.code());
        assert_eq!(after.code(), Some("CNT9"));
        {
            let observed = log.lock().unwrap();
            assert_eq!(observed.statements.len(), 1);
            assert_eq!((observed.commits, observed.rollbacks), (0, 1));
        }
        assert!(
            edges
                .get(&[Filter::eq(
                    "seq-key",
                    AttributeValue::String("temporary".into())
                )])
                .await
                .unwrap()
                .is_empty()
        );
        *log.lock().unwrap() = DispatchLog {
            fail_at: Some(1),
            ..Default::default()
        };
        assert!(many.put_many(&chain).await.is_err());
        {
            let observed = log.lock().unwrap();
            assert_eq!(observed.statements.len(), 1);
            assert_eq!((observed.commits, observed.rollbacks), (0, 1));
        }
        assert!(
            edges
                .get(&[Filter::eq("seq-key", AttributeValue::String("b".into()))])
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            edges
                .get(&[Filter::eq("seq-key", AttributeValue::String("c".into()))])
                .await
                .unwrap()
                .is_empty()
        );
        edges.delete_by_iid_exact(&a).await.unwrap();
        eprintln!(
            "non-exact put_many literal={literal}: matrix and sequential recipes, repeated keyed/keyless IIDs, interleaved rows, self-playing relation chain, CNT9 and rollback all equal looped semantics"
        );
    }
    database.delete_database().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a live TypeDB server"]
async fn wide_sequential_and_full_match_puts_preserve_loop_answers_live() {
    use type_bridge_orm::_dynamic::DynamicAttributeMap;
    use type_bridge_orm::_manager::DynamicEntityManager;
    use type_bridge_orm::_manager::EntityManager;
    use type_bridge_orm::AttributeValue;
    let _guard = crate::common::integration_test_guard().await;
    let database = setup_db().await;
    database.delete_database().await.unwrap();
    database.create_database().await.unwrap();
    let yaml = "format: typebridge.schema/v2\nattributes:\n  partial-key: { value: string }\n  partial-tag: { value: string }\nentities:\n  partial-person:\n    owns:\n      partial-key: { key: true }\n      partial-tag: { card: { min: 0 } }\n  partial-note:\n    owns:\n      partial-tag: { card: { min: 0 } }\n";
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("wide-sequence.yaml").unwrap(), yaml)]).unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
    )
    .unwrap();
    let installed = InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Rust,
            &ProjectionConfig::rust(),
            &[ProjectionHandler::rust_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    database.execute_raw("define attribute partial-key, value string; attribute partial-tag, value string; entity partial-person, owns partial-key @key, owns partial-tag @card(0..); entity partial-note, owns partial-tag @card(0..);", TxType::Schema).await.unwrap();
    let registry = installed.manager_registry().unwrap();
    let keyed = registry.entity("partial-person").unwrap();
    let note = registry.entity("partial-note").unwrap();
    let tag = |value: String| ("partial-tag".to_owned(), AttributeValue::String(value));
    let values = |prefix: &str| -> DynamicAttributeMap {
        (0..300)
            .map(|index| tag(format!("{prefix}-{index:04}")))
            .collect()
    };
    let normalized = |fields: DynamicAttributeMap| {
        fields
            .into_iter()
            .map(|(name, value)| format!("{name}={value:?}"))
            .collect::<std::collections::BTreeSet<_>>()
    };
    for literal in [false, true] {
        let raw_note = DynamicEntityManager::from_registry(
            &database,
            installed.manager_registry().unwrap(),
            note.clone(),
        );
        let full = values(&format!("full-{literal}"));
        let hit = raw_note
            .insert(&full)
            .await
            .unwrap()
            .item()
            .expect("one written root")
            .iid
            .clone()
            .unwrap();
        let mut near = full.clone();
        near[299] = tag(format!("near-{literal}"));
        let items = vec![full.clone(), near.clone()];
        let tx = database.transaction_context(TxType::Write).await.unwrap();
        let looped = DynamicEntityManager::with_registry_transaction(
            tx.clone(),
            installed.manager_registry().unwrap(),
            note.clone(),
        );
        let mut expected = Vec::new();
        for fields in &items {
            let iid = looped
                .put(fields)
                .await
                .unwrap()
                .item()
                .expect("one written root")
                .iid
                .clone()
                .unwrap();
            expected.push((
                iid.clone(),
                normalized(looped.get_by_iid(&iid).await.unwrap().unwrap().attributes),
            ));
        }
        assert_eq!(expected[0].0, hit);
        assert_ne!(expected[1].0, hit);
        tx.rollback().await.unwrap();
        let (counted, log) = counted_connection(&database, literal).await;
        let manager = DynamicEntityManager::from_registry(&counted, registry.clone(), note.clone());
        let before_put = raw_note.count().await.unwrap();
        let actual = manager.put_many(&items).await.unwrap();
        assert_eq!(
            raw_note.count().await.unwrap(),
            before_put + 1,
            "the hit branch must not allocate"
        );
        assert_eq!(actual[0], hit);
        assert_ne!(actual[1], hit, "a one-field near miss must allocate");
        for (iid, (_, fields)) in actual.iter().zip(&expected) {
            assert_eq!(
                normalized(raw_note.get_by_iid(iid).await.unwrap().unwrap().attributes),
                *fields
            );
        }
        let statements = {
            let observed = log.lock().unwrap();
            assert_eq!(observed.opened, [TxType::Write]);
            assert_eq!((observed.commits, observed.rollbacks), (1, 0));
            // Each row now has a fixed probe/allocation/attachment/final-read stack.
            assert_eq!(observed.statements.len(), 8);
            observed.statements.len()
        };
        let before = raw_note.count().await.unwrap();
        let mut failing = full.clone();
        failing[299] = tag(format!("rolled-back-{literal}"));
        *log.lock().unwrap() = DispatchLog {
            fail_at: Some(statements),
            ..Default::default()
        };
        assert!(manager.put_many(&[full, failing]).await.is_err());
        assert_eq!(raw_note.count().await.unwrap(), before);
        {
            let observed = log.lock().unwrap();
            assert_eq!(observed.statements.len(), statements);
            assert_eq!(observed.opened, [TxType::Write]);
            assert_eq!((observed.commits, observed.rollbacks), (0, 1));
        }

        let key = (
            "partial-key".to_owned(),
            AttributeValue::String(format!("overlap-{literal}")),
        );
        let raw_keyed = DynamicEntityManager::from_registry(
            &database,
            installed.manager_registry().unwrap(),
            keyed.clone(),
        );
        let initial = vec![key.clone(), tag("old".into())];
        let iid = raw_keyed
            .insert(&initial)
            .await
            .unwrap()
            .item()
            .expect("one written root")
            .iid
            .clone()
            .unwrap();
        let mut wide = values(&format!("overlap-{literal}"));
        wide.push(key.clone());
        let rows = vec![
            vec![key.clone(), tag("first".into())],
            wide,
            vec![key.clone(), tag("last".into())],
        ];
        // Preserve the raw answer quirk as evidence while comparing committed data.
        let tx = database.transaction_context(TxType::Write).await.unwrap();
        let mut raw_answer_counts = Vec::new();
        for fields in &rows {
            let query = type_bridge_orm::_manager::query_builder::build_dynamic_entity_put(
                &keyed, fields, "$e",
            )
            .unwrap();
            let QueryResult::Documents(documents) = tx.query(&query).await.unwrap() else {
                panic!("raw Put documents");
            };
            raw_answer_counts.push(documents.len());
            assert!(
                documents
                    .iter()
                    .all(|document| document["iid"] == serde_json::json!(iid))
            );
        }
        tx.commit().await.unwrap();
        assert_eq!(raw_answer_counts, [1, 1, 300]);
        let raw_committed = normalized(
            raw_keyed
                .get_by_iid(&iid)
                .await
                .unwrap()
                .unwrap()
                .attributes,
        );
        raw_keyed.update(Some(&iid), &initial).await.unwrap();
        let tx = database.transaction_context(TxType::Write).await.unwrap();
        let looped = DynamicEntityManager::with_registry_transaction(
            tx.clone(),
            installed.manager_registry().unwrap(),
            keyed.clone(),
        );
        for fields in &rows {
            looped.update(Some(&iid), fields).await.unwrap();
        }
        tx.commit().await.unwrap();
        let expected = normalized(
            raw_keyed
                .get_by_iid(&iid)
                .await
                .unwrap()
                .unwrap()
                .attributes,
        );
        assert_eq!(raw_committed, expected);
        raw_keyed.update(Some(&iid), &initial).await.unwrap();
        eprintln!(
            "old keyed raw Put literal={literal}: committed={raw_committed:?}, fetch rows={raw_answer_counts:?} all IID={iid}; clear/attach recipe committed={expected:?}, answers=[(), (), ()]; duplicate fetch rows are a pre-existing raw-path quirk"
        );
        for update in [false, true] {
            for fail in [false, true] {
                *log.lock().unwrap() = DispatchLog {
                    fail_at: fail.then_some(4),
                    ..Default::default()
                };
                let result = if update {
                    let typed = EntityManager::<PartialUpdateEntity>::from_registry(
                        &counted,
                        registry.clone(),
                    )
                    .unwrap();
                    let updates = rows
                        .iter()
                        .map(|fields| PartialUpdateEntity {
                            iid: Some(iid.clone()),
                            fields: fields
                                .iter()
                                .map(|(name, value)| {
                                    (
                                        if name == "partial-key" {
                                            "partial-key"
                                        } else {
                                            "partial-tag"
                                        },
                                        value.clone(),
                                    )
                                })
                                .collect(),
                        })
                        .collect::<Vec<_>>();
                    typed.update_many(&updates).await
                } else {
                    let manager = DynamicEntityManager::from_registry(
                        &counted,
                        registry.clone(),
                        keyed.clone(),
                    );
                    manager
                        .put_many(&rows)
                        .await
                        .map(|iids| assert_eq!(iids, vec![iid.clone(); 3]))
                };
                if fail {
                    assert!(result.is_err());
                } else {
                    result.unwrap();
                }
                assert_eq!(
                    normalized(
                        raw_keyed
                            .get_by_iid(&iid)
                            .await
                            .unwrap()
                            .unwrap()
                            .attributes
                    ),
                    if fail {
                        normalized(initial.clone())
                    } else {
                        expected.clone()
                    }
                );
                {
                    let observed = log.lock().unwrap();
                    assert_eq!(observed.statements.len(), 4);
                    assert_eq!(observed.opened, [TxType::Write]);
                    assert_eq!(
                        (observed.commits, observed.rollbacks),
                        if fail { (0, 1) } else { (1, 0) }
                    );
                }
                raw_keyed.update(Some(&iid), &initial).await.unwrap();
            }
        }
        eprintln!(
            "wide sequential literal={literal}: full match reuses IID, near miss allocates; {statements} bounded match/write statements, later-piece rollback; overlapping partial put/update use four statements and equal old loops"
        );
    }
}

#[tokio::test]
#[ignore = "requires a live TypeDB server"]
async fn wide_relation_missing_player_refuses_before_allocation_live() {
    use type_bridge_orm::_dynamic::{DynamicAttributeMap, DynamicRolePlayerInput};
    use type_bridge_orm::_manager::{DynamicEntityManager, DynamicRelationManager};
    use type_bridge_orm::AttributeValue;
    let _guard = crate::common::integration_test_guard().await;
    let database = setup_db().await;
    database.delete_database().await.unwrap();
    database.create_database().await.unwrap();
    let yaml = "format: typebridge.schema/v2\nattributes:\n  guard-tag: { value: string }\nentities:\n  guard-person: {}\nrelations:\n  guard-relation:\n    owns:\n      guard-tag: { card: { min: 0 } }\n    relates:\n      member: { card: { min: 0 } }\nplays:\n  guard-person:\n    guard-relation: [member]\n";
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("wide-reference-guard.yaml").unwrap(), yaml)])
            .unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
    )
    .unwrap();
    let installed = InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Rust,
            &ProjectionConfig::rust(),
            &[ProjectionHandler::rust_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    database.execute_raw("define attribute guard-tag, value string; entity guard-person, plays guard-relation:member; relation guard-relation, owns guard-tag @card(0..), relates member @card(0..);", TxType::Schema).await.unwrap();
    let registry = installed.manager_registry().unwrap();
    let person = registry.entity("guard-person").unwrap();
    let relation = registry.relation("guard-relation").unwrap();
    let iid = DynamicEntityManager::from_registry(
        &database,
        installed.manager_registry().unwrap(),
        person,
    )
    .insert(&vec![])
    .await
    .unwrap()
    .item()
    .expect("one written root")
    .iid
    .clone()
    .unwrap();
    let players = [iid.as_str(), "0xffff"]
        .map(|iid| DynamicRolePlayerInput {
            role_name: "member".into(),
            player_type_name: "guard-person".into(),
            iid: Some(iid.into()),
            key: None,
        })
        .to_vec();
    let fields: DynamicAttributeMap = (0..300)
        .map(|index| {
            (
                "guard-tag".into(),
                AttributeValue::String(format!("tag-{index:04}")),
            )
        })
        .collect();
    let raw = DynamicRelationManager::from_registry(
        &database,
        installed.manager_registry().unwrap(),
        relation.clone(),
    );
    let tx = database.transaction_context(TxType::Write).await.unwrap();
    let looped = DynamicRelationManager::with_registry_transaction(
        tx.clone(),
        installed.manager_registry().unwrap(),
        relation.clone(),
    );
    assert!(looped.put(&fields, &players).await.is_err());
    tx.rollback().await.unwrap();
    assert_eq!(raw.count().await.unwrap(), 0);
    let other = DynamicEntityManager::from_registry(
        &database,
        installed.manager_registry().unwrap(),
        registry.entity("guard-person").unwrap(),
    )
    .insert(&vec![])
    .await
    .unwrap()
    .item()
    .expect("one written root")
    .iid
    .clone()
    .unwrap();
    let valid_players = [iid.as_str(), other.as_str()]
        .map(|iid| DynamicRolePlayerInput {
            role_name: "member".into(),
            player_type_name: "guard-person".into(),
            iid: Some(iid.into()),
            key: None,
        })
        .to_vec();
    let relation_type = TypeId::new(TypeKind::Relation, "guard-relation").unwrap();
    let person_type = TypeId::new(TypeKind::Entity, "guard-person").unwrap();
    let role = RoleId::new("guard-relation", "member").unwrap();
    let tag = AttributeId::new("guard-tag").unwrap();
    let assert_missing =
        |diagnostic: &type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic| {
            use type_bridge_contract::sdk_diagnostic::{
                SdkDiagnosticCategory, SdkDiagnosticName, SdkDiagnosticPathSegment as Path,
            };
            assert_eq!(
                diagnostic.code().as_str(),
                "projected_batch_reference_missing"
            );
            assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Integrity);
            assert_eq!(
                diagnostic.path(),
                &[
                    Path::Argument(SdkDiagnosticName::new("rows").unwrap()),
                    Path::Index(0),
                    Path::Type(relation_type.clone()),
                    Path::Role(role.clone()),
                    Path::Index(1),
                    Path::Type(person_type.clone())
                ]
            );
        };
    for literal in [false, true] {
        let (counted, log) = counted_connection(&database, literal).await;
        let manager =
            DynamicRelationManager::from_registry(&counted, registry.clone(), relation.clone());
        for operation in ["insert", "put", "match-put"] {
            *log.lock().unwrap() = DispatchLog::default();
            let items = [(fields.clone(), players.clone())];
            let result = match operation {
                "insert" => manager.insert_many(&items).await,
                "put" => manager.put_many_exact(&items).await,
                _ => manager.put_many(&items).await,
            };
            eprintln!(
                "wide missing-player literal={literal} operation={operation}: result={result:?}, committed_count={}",
                raw.count().await.unwrap()
            );
            let error = result.unwrap_err();
            let type_bridge_orm::OrmError::ExecutionDiagnostic(diagnostic) = error else {
                panic!("guard miss must retain its execution diagnostic");
            };
            assert_missing(&diagnostic);
            assert_eq!(raw.count().await.unwrap(), 0);
            let observed = log.lock().unwrap();
            assert_eq!(observed.opened, [TxType::Write]);
            assert_eq!((observed.commits, observed.rollbacks), (0, 1));
        }
        let create = ProjectedCreate::try_new(
            &installed,
            relation_type.clone(),
            vec![(
                field(&relation_type, &tag),
                (0..300)
                    .map(|index| string_scalar(&installed, &tag, &format!("tag-{index:04}")))
                    .collect(),
            )],
            vec![(
                role.clone(),
                vec![
                    reference(&installed, &person_type, &iid),
                    reference(&installed, &person_type, "0xffff"),
                ],
            )],
        )
        .unwrap();
        let batch = ProjectedBatch::try_new(
            &installed,
            relation_type.clone(),
            ProjectedBatchOperation::Insert,
            vec![ProjectedBatchRow::Create(create)],
        )
        .unwrap();
        *log.lock().unwrap() = DispatchLog::default();
        let failure = ProjectedBatchExecutor::new(&installed)
            .execute(&counted, &batch, control())
            .await
            .unwrap_err();
        assert_missing(&failure);
        assert_eq!(raw.count().await.unwrap(), 0);
        assert_eq!(log.lock().unwrap().statements.len(), 1);
        *log.lock().unwrap() = DispatchLog::default();
        let target = manager
            .insert_many(&[(fields.clone(), valid_players.clone())])
            .await
            .unwrap()
            .remove(0);
        assert_eq!(raw.count().await.unwrap(), 1);
        assert_eq!(log.lock().unwrap().statements.len(), 3);
        let before = raw.get_by_iid(&target).await.unwrap();
        *log.lock().unwrap() = DispatchLog::default();
        assert!(
            manager
                .update_many_and_get_exact(&[(target.clone(), vec![], players.clone())])
                .await
                .is_err()
        );
        assert_eq!(raw.get_by_iid(&target).await.unwrap(), before);
        // Force the update into pieces as well, retaining every old tag/player.
        *log.lock().unwrap() = DispatchLog::default();
        let failure = manager
            .update_many_and_get_exact(&[(target.clone(), fields.clone(), players.clone())])
            .await
            .unwrap_err();
        let OrmError::ExecutionDiagnostic(diagnostic) = failure else {
            panic!("wide update retains reference diagnostic");
        };
        assert_missing(&diagnostic);
        assert_eq!(raw.get_by_iid(&target).await.unwrap(), before);
        {
            let observed = log.lock().unwrap();
            assert_eq!((observed.commits, observed.rollbacks), (0, 1));
        }
        raw.delete_by_iid(&target).await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a live TypeDB server"]
async fn explicit_wide_updates_keep_invocation_start_key_references_live() {
    use type_bridge_orm::_manager::{DynamicEntityManager, DynamicRelationManager};
    let _guard = crate::common::integration_test_guard().await;
    let database = setup_db().await;
    database.delete_database().await.unwrap();
    database.create_database().await.unwrap();
    let yaml = "format: typebridge.schema/v2\nattributes:\n  snapshot-key: { value: string }\n  snapshot-tag: { value: string }\nentities:\n  snapshot-person: {}\nrelations:\n  snapshot-edge:\n    owns:\n      snapshot-key: { key: true }\n      snapshot-tag: { card: { min: 0 } }\n    relates:\n      anchor: { card: { min: 0, max: 1 } }\n      prior: { card: { min: 0, max: 1 } }\nplays:\n  snapshot-person:\n    snapshot-edge: [anchor]\n  snapshot-edge:\n    snapshot-edge: [prior]\n";
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("reference-snapshot.yaml").unwrap(), yaml)])
            .unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
    )
    .unwrap();
    let installed = InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Rust,
            &ProjectionConfig::rust(),
            &[ProjectionHandler::rust_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    database.execute_raw("define attribute snapshot-key, value string; attribute snapshot-tag, value string; entity snapshot-person, plays snapshot-edge:anchor; relation snapshot-edge, owns snapshot-key @key, owns snapshot-tag @card(0..), relates anchor @card(0..1), relates prior @card(0..1), plays snapshot-edge:prior;", TxType::Schema).await.unwrap();
    let registry = installed.manager_registry().unwrap();
    let person = TypeId::new(TypeKind::Entity, "snapshot-person").unwrap();
    let edge = TypeId::new(TypeKind::Relation, "snapshot-edge").unwrap();
    let key = AttributeId::new("snapshot-key").unwrap();
    let tag = AttributeId::new("snapshot-tag").unwrap();
    let anchor = RoleId::new("snapshot-edge", "anchor").unwrap();
    let prior = RoleId::new("snapshot-edge", "prior").unwrap();
    let player = DynamicEntityManager::from_registry(
        &database,
        installed.manager_registry().unwrap(),
        registry.entity("snapshot-person").unwrap(),
    )
    .insert(&vec![])
    .await
    .unwrap()
    .item()
    .expect("one written root")
    .iid
    .clone()
    .unwrap();
    let create = |value: &str, width: usize, predecessor: Option<ProjectedReference>| {
        let mut fields = vec![(
            field(&edge, &key),
            vec![string_scalar(&installed, &key, value)],
        )];
        if width > 0 {
            fields.push((
                field(&edge, &tag),
                (0..width)
                    .map(|index| string_scalar(&installed, &tag, &format!("t-{index:04}")))
                    .collect(),
            ));
        }
        let mut roles = vec![(
            anchor.clone(),
            vec![reference(&installed, &person, &player)],
        )];
        if let Some(predecessor) = predecessor {
            roles.push((prior.clone(), vec![predecessor]));
        }
        ProjectedCreate::try_new(&installed, edge.clone(), fields, roles).unwrap()
    };
    let crud = type_bridge_orm::ProjectedCrudExecutor::new(&installed);
    let first = crud
        .insert_relation(&database, &create("a", 0, None))
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    let second = crud
        .insert_relation(
            &database,
            &create("b", 0, Some(reference(&installed, &edge, first.iid()))),
        )
        .await
        .unwrap()
        .into_items()
        .pop()
        .expect("one written root");
    let raw = DynamicRelationManager::from_registry(
        &database,
        installed.manager_registry().unwrap(),
        registry.relation("snapshot-edge").unwrap(),
    );
    let mut failures = Vec::new();
    for literal in [false, true] {
        let (counted, log) = counted_connection(&database, literal).await;
        for by_iid in [false, true] {
            let predecessor = || {
                ProjectedReference::try_new(
                    &installed,
                    edge.clone(),
                    by_iid.then(|| first.iid().to_owned()),
                    vec![(field(&edge, &key), string_scalar(&installed, &key, "a"))],
                )
                .unwrap()
            };
            for self_playing in [false, true] {
                for width in [0, 300] {
                    *log.lock().unwrap() = DispatchLog::default();
                    let rows = if self_playing {
                        vec![ProjectedBatchRow::Update {
                            iid: first.iid().into(),
                            replacement: create("a2", width, Some(predecessor())),
                        }]
                    } else {
                        vec![
                            ProjectedBatchRow::Update {
                                iid: first.iid().into(),
                                replacement: create("a2", width, None),
                            },
                            ProjectedBatchRow::Update {
                                iid: second.iid().into(),
                                replacement: create("b", 0, Some(predecessor())),
                            },
                        ]
                    };
                    let batch = ProjectedBatch::try_new(
                        &installed,
                        edge.clone(),
                        ProjectedBatchOperation::Update,
                        rows,
                    )
                    .unwrap();
                    let tx = counted.transaction_context(TxType::Write).await.unwrap();
                    let result = ProjectedBatchExecutor::new(&installed)
                        .execute_in_transaction(&tx, &batch, control())
                        .await;
                    eprintln!(
                        "snapshot reference literal={literal} self={self_playing} width={width} by_iid={by_iid}: success={}, diagnostic={:?}",
                        result.is_ok(),
                        result.as_ref().err().map(|error| error.code())
                    );
                    if let Err(error) = &result {
                        failures.push(format!(
                            "literal={literal} self={self_playing} width={width}: {}",
                            error.code().as_str()
                        ));
                        tx.rollback().await.unwrap();
                        continue;
                    }
                    let statements = if width == 0 {
                        1
                    } else if self_playing {
                        if by_iid { 3 } else { 4 }
                    } else {
                        5
                    };
                    assert_eq!(log.lock().unwrap().statements.len(), statements);
                    let output = things(result.unwrap());
                    assert_eq!(
                        output[0].fields()[&field(&edge, &key)][0].value(),
                        &CanonicalValue::String(CanonicalString::new("a2").unwrap())
                    );
                    let referenced = if self_playing { &output[0] } else { &output[1] };
                    assert_eq!(referenced.roles()[&prior][0].iid(), first.iid());
                    assert_eq!(
                        output[0]
                            .fields()
                            .get(&field(&edge, &tag))
                            .map_or(0, Vec::len),
                        width
                    );
                    tx.rollback().await.unwrap();
                    assert_eq!(log.lock().unwrap().commits, 0);
                    assert_eq!(log.lock().unwrap().rollbacks, 1);
                    assert_eq!(
                        raw.get_by_iid(first.iid()).await.unwrap()[0].iid.as_deref(),
                        Some(first.iid())
                    );
                    if width > 0 {
                        *log.lock().unwrap() = DispatchLog {
                            fail_at: Some(statements),
                            ..Default::default()
                        };
                        let failure = ProjectedBatchExecutor::new(&installed)
                            .execute(&counted, &batch, control())
                            .await
                            .unwrap_err();
                        assert_eq!(failure.code().as_str(), "transaction_state_invalid");
                        {
                            let observed = log.lock().unwrap();
                            assert_eq!(observed.statements.len(), statements);
                            assert_eq!(observed.opened, [TxType::Write]);
                            assert_eq!((observed.commits, observed.rollbacks), (0, 1));
                        }
                        let original = raw.get_by_iid(first.iid()).await.unwrap();
                        assert!(
                            original[0].attributes.iter().any(|(name, value)| name
                                == "snapshot-key"
                                && value == &type_bridge_orm::AttributeValue::String("a".into()))
                        );
                    }
                }
            }
        }
        *log.lock().unwrap() = DispatchLog::default();
        let future = key_reference(&installed, &edge, &key, "future");
        let invocation_start = ProjectedBatch::try_new(
            &installed,
            edge.clone(),
            ProjectedBatchOperation::Insert,
            vec![
                ProjectedBatchRow::Create(create("future", 300, None)),
                ProjectedBatchRow::Create(create("future-child", 0, Some(future))),
            ],
        )
        .unwrap();
        let failure = ProjectedBatchExecutor::new(&installed)
            .execute(&counted, &invocation_start, control())
            .await
            .unwrap_err();
        assert_eq!(failure.code().as_str(), "projected_batch_reference_missing");
        assert_eq!(
            failure.category(),
            type_bridge_contract::sdk_diagnostic::SdkDiagnosticCategory::Integrity
        );
        assert_eq!(raw.count().await.unwrap(), 2);
        let observed = log.lock().unwrap();
        assert_eq!(observed.opened, [TxType::Write]);
        assert_eq!((observed.commits, observed.rollbacks), (0, 1));
        assert!(
            observed
                .statements
                .iter()
                .all(|source| !source.contains("insert ")
                    && !source.contains("put ")
                    && !source.contains("delete "))
        );
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
#[ignore = "requires a live TypeDB server"]
async fn generic_relation_updates_keep_prior_row_visibility_and_snapshots_live() {
    use type_bridge_orm::_dynamic::{DynamicAttributeMap, DynamicRolePlayerInput};
    use type_bridge_orm::_manager::DynamicRelationManager;
    let _guard = crate::common::integration_test_guard().await;
    let database = setup_db().await;
    database.delete_database().await.unwrap();
    database.create_database().await.unwrap();
    let yaml = "format: typebridge.schema/v2\nattributes:\n  mutable-lookup: { value: string }\nentities:\n  mutable-person: {}\nrelations:\n  mutable-edge:\n    owns:\n      mutable-lookup: { card: { min: 0, max: 1 } }\n    relates:\n      anchor: { card: { min: 0, max: 1 } }\n      prior: { card: { min: 0, max: 1 } }\nplays:\n  mutable-person:\n    mutable-edge: [anchor]\n  mutable-edge:\n    mutable-edge: [prior]\n";
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("mutable-reference.yaml").unwrap(), yaml)])
            .unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
    )
    .unwrap();
    let installed = InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Rust,
            &ProjectionConfig::rust(),
            &[ProjectionHandler::rust_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    database.execute_raw("define attribute mutable-lookup, value string; entity mutable-person, plays mutable-edge:anchor; relation mutable-edge, owns mutable-lookup @card(0..1), relates anchor @card(0..1), relates prior @card(0..1), plays mutable-edge:prior;", TxType::Schema).await.unwrap();
    let registry = installed.manager_registry().unwrap();
    let descriptor = registry.relation("mutable-edge").unwrap();
    let raw = DynamicRelationManager::from_registry(
        &database,
        installed.manager_registry().unwrap(),
        descriptor.clone(),
    );
    let fields = |value: &str| -> DynamicAttributeMap {
        vec![(
            "mutable-lookup".into(),
            type_bridge_orm::AttributeValue::String(value.into()),
        )]
    };
    let person = type_bridge_orm::_manager::DynamicEntityManager::from_registry(
        &database,
        installed.manager_registry().unwrap(),
        registry.entity("mutable-person").unwrap(),
    )
    .insert(&vec![])
    .await
    .unwrap()
    .item()
    .expect("one written root")
    .iid
    .clone()
    .unwrap();
    let anchor = || DynamicRolePlayerInput {
        role_name: "anchor".into(),
        player_type_name: "mutable-person".into(),
        iid: Some(person.clone()),
        key: None,
    };
    let first = raw
        .insert(&fields("old"), &[anchor()])
        .await
        .unwrap()
        .item()
        .expect("one written root")
        .iid
        .clone()
        .unwrap();
    let second = raw
        .insert(&fields("second"), &[anchor()])
        .await
        .unwrap()
        .item()
        .expect("one written root")
        .iid
        .clone()
        .unwrap();
    let player = |value: &str| DynamicRolePlayerInput {
        role_name: "prior".into(),
        player_type_name: "mutable-edge".into(),
        iid: None,
        key: Some((
            "mutable-lookup".into(),
            type_bridge_orm::AttributeValue::String(value.into()),
        )),
    };
    for literal in [false, true] {
        let (counted, log) = counted_connection(&database, literal).await;
        let manager =
            DynamicRelationManager::from_registry(&counted, registry.clone(), descriptor.clone());
        for case in ["new-value", "old-value", "overlap", "independent"] {
            let items = match case {
                "new-value" => vec![
                    (first.clone(), fields("new"), vec![anchor()]),
                    (
                        second.clone(),
                        fields("second"),
                        vec![anchor(), player("new")],
                    ),
                ],
                "old-value" => vec![
                    (first.clone(), fields("new"), vec![anchor()]),
                    (
                        second.clone(),
                        fields("second"),
                        vec![anchor(), player("old")],
                    ),
                ],
                "overlap" => vec![
                    (first.clone(), fields("new"), vec![anchor()]),
                    (first.clone(), fields("newer"), vec![anchor()]),
                ],
                _ => vec![
                    (first.clone(), fields("new"), vec![anchor()]),
                    (second.clone(), fields("newer"), vec![anchor()]),
                ],
            };
            let tx = database.transaction_context(TxType::Write).await.unwrap();
            let looped = DynamicRelationManager::with_registry_transaction(
                tx.clone(),
                installed.manager_registry().unwrap(),
                descriptor.clone(),
            );
            let mut expected = Vec::new();
            let mut old_failed = false;
            for (iid, fields, players) in &items {
                match looped.update_and_get_exact(iid, fields, players).await {
                    Ok(rows) => expected.push(rows.item().expect("one updated root").clone()),
                    Err(_) => {
                        old_failed = true;
                        break;
                    }
                }
            }
            assert_eq!(old_failed, case == "old-value");
            tx.rollback().await.unwrap();
            *log.lock().unwrap() = DispatchLog::default();
            let tx = counted.transaction_context(TxType::Write).await.unwrap();
            let borrowed = DynamicRelationManager::with_registry_transaction(
                tx.clone(),
                registry.clone(),
                descriptor.clone(),
            );
            let result = borrowed.update_many_and_get_exact(&items).await;
            if old_failed {
                assert!(
                    result.is_err(),
                    "an old-value lookup cannot resolve after the first update"
                );
            } else {
                assert_eq!(result.unwrap(), expected, "literal={literal} case={case}");
            }
            let statements = if case == "independent" { 1 } else { 2 };
            assert_eq!(log.lock().unwrap().statements.len(), statements);
            tx.rollback().await.unwrap();
            assert_eq!(
                raw.get_by_iid(&first).await.unwrap()[0].attributes,
                fields("old")
            );
            if case != "independent" && !old_failed {
                *log.lock().unwrap() = DispatchLog {
                    fail_at: Some(2),
                    ..Default::default()
                };
                assert!(manager.update_many_and_get_exact(&items).await.is_err());
                {
                    let observed = log.lock().unwrap();
                    assert_eq!(observed.statements.len(), 2);
                    assert_eq!(observed.opened, [TxType::Write]);
                    assert_eq!((observed.commits, observed.rollbacks), (0, 1));
                }
                assert_eq!(
                    raw.get_by_iid(&first).await.unwrap()[0].attributes,
                    fields("old")
                );
            }
        }
        *log.lock().unwrap() = DispatchLog::default();
        let committed = manager
            .update_many_and_get_exact(&[
                (first.clone(), fields("new"), vec![anchor()]),
                (
                    second.clone(),
                    fields("second"),
                    vec![anchor(), player("new")],
                ),
            ])
            .await
            .unwrap();
        assert_eq!(committed.len(), 2);
        {
            let observed = log.lock().unwrap();
            assert_eq!(observed.statements.len(), 2);
            assert_eq!(observed.opened, [TxType::Write]);
            assert_eq!((observed.commits, observed.rollbacks), (1, 0));
        }
        raw.update_exact(&second, &fields("second"), &[anchor()])
            .await
            .unwrap();
        raw.update_exact(&first, &fields("old"), &[anchor()])
            .await
            .unwrap();
    }
}
#[tokio::test]
#[ignore = "requires a live TypeDB server"]
async fn wide_relation_match_put_intersects_all_player_probe_chunks_live() {
    use type_bridge_orm::_dynamic::DynamicRolePlayerInput;
    use type_bridge_orm::_manager::{DynamicEntityManager, DynamicRelationManager};
    let _guard = crate::common::integration_test_guard().await;
    let database = setup_db().await;
    database.delete_database().await.unwrap();
    database.create_database().await.unwrap();
    let yaml = "format: typebridge.schema/v2\nattributes:\n  probe-key: { value: string }\nentities:\n  probe-person:\n    owns:\n      probe-key: { key: true }\nrelations:\n  probe-relation:\n    relates:\n      member: { card: { min: 0 } }\nplays:\n  probe-person:\n    probe-relation: [member]\n";
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("player-probes.yaml").unwrap(), yaml)]).unwrap();
    let resolved = resolve(
        &normalize_documents(&documents).unwrap(),
        &SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
    )
    .unwrap();
    let installed = InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Rust,
            &ProjectionConfig::rust(),
            &[ProjectionHandler::rust_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    database.execute_raw("define attribute probe-key, value string; entity probe-person, owns probe-key @key, plays probe-relation:member; relation probe-relation, relates member @card(0..);", TxType::Schema).await.unwrap();
    let registry = installed.manager_registry().unwrap();
    let people = DynamicEntityManager::from_registry(
        &database,
        registry.clone(),
        registry.entity("probe-person").unwrap(),
    );
    let iids = people
        .insert_many(
            &(0..101)
                .map(|index| {
                    vec![(
                        "probe-key".into(),
                        type_bridge_orm::AttributeValue::String(format!("player-{index}")),
                    )]
                })
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
    // Each key reference consumes a player binding, a key binding and an input.
    // One hundred players force two probes and grouped Boolean read patterns.
    let players = (0..101)
        .map(|index| DynamicRolePlayerInput {
            role_name: "member".into(),
            player_type_name: "probe-person".into(),
            iid: None,
            key: Some((
                "probe-key".into(),
                type_bridge_orm::AttributeValue::String(format!("player-{index}")),
            )),
        })
        .collect::<Vec<_>>();
    let descriptor = registry.relation("probe-relation").unwrap();
    let raw = DynamicRelationManager::from_registry(
        &database,
        installed.manager_registry().unwrap(),
        descriptor.clone(),
    );
    let full = players[..100].to_vec();
    let mut near = full.clone();
    near[99] = players[100].clone();
    let expected_players = |values: &[DynamicRolePlayerInput]| {
        values
            .iter()
            .map(|player| {
                let Some((_, type_bridge_orm::AttributeValue::String(key))) = &player.key else {
                    panic!("key reference");
                };
                let index: usize = key.strip_prefix("player-").unwrap().parse().unwrap();
                iids[index].clone()
            })
            .collect::<std::collections::BTreeSet<_>>()
    };
    for literal in [false, true] {
        let hit = raw
            .insert(&vec![], &full)
            .await
            .unwrap()
            .item()
            .expect("one written root")
            .iid
            .clone()
            .unwrap();
        let (counted, log) = counted_connection(&database, literal).await;
        let manager =
            DynamicRelationManager::from_registry(&counted, registry.clone(), descriptor.clone());
        let actual = manager
            .put_many(&[(vec![], full.clone()), (vec![], near.clone())])
            .await
            .unwrap();
        assert_eq!(
            actual[0], hit,
            "all probe chunks must agree before reusing a relation"
        );
        assert_ne!(actual[1], hit, "the last-player near miss must allocate");
        assert_eq!(raw.count().await.unwrap(), 2);
        for (iid, input) in actual.iter().zip([&full, &near]) {
            let stored = raw.get_by_iid(iid).await.unwrap();
            let stored = stored
                .iter()
                .flat_map(|row| {
                    row.role_players
                        .iter()
                        .map(|player| player.player_iid.clone().unwrap())
                })
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(stored, expected_players(input));
        }
        {
            let observed = log.lock().unwrap();
            let probes = observed
                .statements
                .iter()
                .filter(|query| query.contains("select $root0;"))
                .count();
            assert_eq!(
                probes, 4,
                "two bounded player probes for each full-match row"
            );
            // Each row has two probes; a hit reads its IID, a miss fits one Insert.
            assert_eq!(observed.statements.len(), 6);
            assert_eq!(
                observed
                    .statements
                    .iter()
                    .filter(|query| query.contains("insert"))
                    .count(),
                1
            );
            assert_eq!(observed.opened, [TxType::Write]);
            assert_eq!((observed.commits, observed.rollbacks), (1, 0));
        }
        for iid in actual {
            raw.delete_by_iid(&iid).await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires a live TypeDB server"]
async fn generic_manager_query_limits_choose_full_plans_or_existing_pieces_live() {
    use type_bridge_contract::limits::QueryLimits;
    use type_bridge_orm::_manager::DynamicEntityManager;
    let _guard = crate::common::integration_test_guard().await;
    let database = setup_db().await;
    database.delete_database().await.unwrap();
    database.create_database().await.unwrap();
    let yaml = "format: typebridge.schema/v2\nattributes:\n  configured-tag: { value: string }\nentities:\n  configured-note:\n    owns:\n      configured-tag: { card: { min: 0 } }\n";
    let docs =
        SchemaDocumentSet::parse([(DocumentId::new("configured-slots.yaml").unwrap(), yaml)])
            .unwrap();
    let resolved = resolve(
        &normalize_documents(&docs).unwrap(),
        &SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
    )
    .unwrap();
    let installed = InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Rust,
            &ProjectionConfig::rust(),
            &[ProjectionHandler::rust_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    database.execute_raw("define attribute configured-tag, value string; entity configured-note, owns configured-tag @card(0..);", TxType::Schema).await.unwrap();
    let registry = installed.manager_registry().unwrap();
    let descriptor = registry.entity("configured-note").unwrap();
    let raw = DynamicEntityManager::from_registry(
        &database,
        installed.manager_registry().unwrap(),
        descriptor.clone(),
    );
    for literal in [false, true] {
        for (bindings, terms) in [(256, 256), (1024, 256), (256, 512), (1024, 512)] {
            // Reuse identical values so deleted fixtures do not accumulate
            // distinct orphan attributes between policy cases.
            let mut limits = QueryLimits::V3;
            limits.structural.bindings = bindings;
            limits.structural.boolean_terms = terms;
            let (counted, log) = counted_connection(&database, literal).await;
            let counted = counted.with_query_limits(limits).unwrap();
            assert_eq!(counted.query_limits(), limits);
            let inherited = counted.transaction_context(TxType::Write).await.unwrap();
            assert_eq!(inherited.query_limits(), limits);
            inherited.rollback().await.unwrap();
            let manager =
                DynamicEntityManager::from_registry(&counted, registry.clone(), descriptor.clone());
            let fields = (0..300)
                .map(|index| {
                    (
                        "configured-tag".into(),
                        type_bridge_orm::AttributeValue::String(format!("value-{index}")),
                    )
                })
                .collect::<Vec<_>>();
            let hit = raw
                .insert(&fields)
                .await
                .unwrap()
                .item()
                .expect("one written root")
                .iid
                .clone()
                .unwrap();
            *log.lock().unwrap() = DispatchLog::default();
            eprintln!("query policy literal={literal} bindings={bindings} terms={terms}: full hit");
            let actual = manager
                .put_many(std::slice::from_ref(&fields))
                .await
                .unwrap();
            assert_eq!(actual.as_slice(), std::slice::from_ref(&hit));
            let expected = if bindings > 256 && terms > 256 { 1 } else { 4 };
            assert_eq!(log.lock().unwrap().statements.len(), expected);
            assert_eq!(log.lock().unwrap().commits, 1);
            let mut near = fields.clone();
            near[299].1 = type_bridge_orm::AttributeValue::String("near".into());
            *log.lock().unwrap() = DispatchLog::default();
            eprintln!(
                "query policy literal={literal} bindings={bindings} terms={terms}: near miss"
            );
            let created = manager
                .put_many(std::slice::from_ref(&near))
                .await
                .unwrap()
                .remove(0);
            assert_ne!(created, hit);
            let expected = if bindings > 256 {
                if terms > 256 { 1 } else { 4 }
            } else {
                5
            };
            assert_eq!(log.lock().unwrap().statements.len(), expected);
            assert_eq!(log.lock().unwrap().opened, [TxType::Write]);
            assert_eq!(log.lock().unwrap().commits, 1);
            let actual = raw
                .get_by_iid(&created)
                .await
                .unwrap()
                .unwrap()
                .attributes
                .into_iter()
                .map(|(name, value)| format!("{name}={value:?}"))
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(
                actual,
                near.into_iter()
                    .map(|(name, value)| format!("{name}={value:?}"))
                    .collect()
            );
            raw.delete_by_iid(&hit).await.unwrap();
            raw.delete_by_iid(&created).await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires a live TypeDB server"]
async fn staged_iid_reference_wide_attachment_and_last_failure_are_atomic_live() {
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
    use type_bridge_contract::query_plan::v3::{
        AttachItem, InsertThing, Operation, PlanSpec, QueryPlanV3, Stage, ThingDocument,
        WriteReturns,
    };
    use type_bridge_contract::query_plan::{
        InputColumn, InputColumnId, InputRow, QueryOperand, QueryOutput, QueryPattern,
    };
    use type_bridge_contract::query_remote::{RemoteFieldValue, RemoteValue};
    use type_bridge_contract::query_remote_v3::{StagedAnswerV3, StagedPlanV3, StagedResultRefV3};
    use type_bridge_contract::value::ValueTypeTag;
    let _guard = crate::common::integration_test_guard().await;
    let database = setup_db().await;
    database.delete_database().await.unwrap();
    database.create_database().await.unwrap();
    database.execute_raw("define attribute reference-value, value string; entity reference-note, owns reference-value @card(0..);", TxType::Schema).await.unwrap();
    let yaml = "format: typebridge.schema/v2\nattributes:\n  reference-value: { value: string }\nentities:\n  reference-note:\n    owns:\n      reference-value: { card: { min: 0 } }\n";
    let declared = Arc::new(
        normalize_documents(
            &SchemaDocumentSet::parse([(
                DocumentId::new("iid-reference-live.yaml").unwrap(),
                yaml,
            )])
            .unwrap(),
        )
        .unwrap(),
    );
    let context = type_bridge_schema::ManagedDeltaContext::new(
        ManagedScopeId::new("iid-reference-live").unwrap(),
        SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
        type_bridge_contract::capability::CapabilitySet::new(),
    );
    let managed = type_bridge_schema::managed_schema_state(&declared, &context).unwrap();
    let note = TypeId::new(TypeKind::Entity, "reference-note").unwrap();
    let field =
        OwnsFactId::new(note.clone(), AttributeId::new("reference-value").unwrap()).unwrap();
    let root = BindingId::new(0).unwrap();
    let source = QueryPlanV3::new(
        PlanSpec {
            bindings: vec![AssertionBinding::new(
                root,
                QueryVariable::new("root").unwrap(),
            )],
            inputs: vec![],
            given_rows: None,
            operation: Operation::Write {
                returns: WriteReturns::Rows,
            },
            pipeline: vec![Stage::Insert {
                things: vec![InsertThing {
                    binding: root,
                    type_id: note.clone(),
                    fields: vec![],
                    players: vec![],
                }],
            }],
            output: QueryOutput::Rows {
                columns: vec![root],
            },
            managed_semantics: managed.managed_semantic_schema().clone(),
            compatibility: None,
            functions: vec![],
        },
        &declared,
    )
    .unwrap();
    let discarded = BindingId::new(1).unwrap();
    let target = QueryPlanV3::new(
        PlanSpec {
            bindings: vec![
                AssertionBinding::new(root, QueryVariable::new("root").unwrap()),
                AssertionBinding::new(discarded, QueryVariable::new("discarded").unwrap()),
            ],
            inputs: vec![
                InputColumn::new(
                    InputColumnId::new(0),
                    QueryVariable::new("target").unwrap(),
                    ValueTypeTag::String,
                    false,
                ),
                InputColumn::new(
                    InputColumnId::new(1),
                    QueryVariable::new("value").unwrap(),
                    ValueTypeTag::String,
                    false,
                ),
            ],
            given_rows: None,
            operation: Operation::Write {
                returns: WriteReturns::Rows,
            },
            pipeline: vec![
                Stage::Match {
                    patterns: vec![
                        QueryPattern::Isa {
                            binding: root,
                            type_id: note.clone(),
                            include_subtypes: false,
                        }
                        .into(),
                        QueryPattern::Iid {
                            binding: root,
                            iid: QueryOperand::Input {
                                column: InputColumnId::new(0),
                            },
                        }
                        .into(),
                    ],
                },
                Stage::Attach {
                    items: vec![AttachItem::Has {
                        optional: false,
                        owner: root,
                        field,
                        operand: QueryOperand::Input {
                            column: InputColumnId::new(1),
                        },
                    }],
                },
                Stage::Select {
                    bindings: vec![root],
                },
                Stage::Distinct,
                // Group before Fetch: the provider can repeat documents after a Given window.
                Stage::Reduce {
                    assignments: vec![type_bridge_contract::query_plan::ReduceAssignment::new(
                        discarded,
                        type_bridge_contract::query_plan::Reducer::Count,
                        None,
                    )],
                    groups: vec![root],
                },
                Stage::Fetch(ThingDocument::new(root, vec![])),
            ],
            output: QueryOutput::Rows { columns: vec![] },
            managed_semantics: managed.managed_semantic_schema().clone(),
            compatibility: None,
            functions: vec![],
        },
        &declared,
    )
    .unwrap();
    let values = (0..300)
        .map(|index| {
            CanonicalValue::String(CanonicalString::new(format!("value-{index}")).unwrap())
        })
        .collect::<Vec<_>>();
    let stack = vec![
        StagedPlanV3::new(&source, vec![], &declared).unwrap(),
        StagedPlanV3::new_with_result_refs(
            &target,
            values
                .iter()
                .map(|value| InputRow::new(vec![None, Some(value.clone())]))
                .collect(),
            vec![StagedResultRefV3 {
                input: InputColumnId::new(0),
                source_plan: 0,
                source_column: 0,
                expected_types: vec![note],
            }],
            &declared,
        )
        .unwrap(),
    ];
    for literal in [false, true] {
        for fail in [false, true] {
            let (counted, log) = counted_connection(&database, literal).await;
            if fail {
                log.lock().unwrap().fail_at = Some(2);
            }
            let mut transaction = counted.transaction(
                TransactionOptions::default()
                    .with_answer_timeout(std::time::Duration::from_secs(30)),
            );
            let pending = transaction
                .stage_plan_stack(stack.clone(), Arc::clone(&declared))
                .unwrap();
            let result = transaction.commit().await;
            let QueryResult::Documents(roots) = database
                .execute_raw(
                    "match $root isa reference-note; fetch {\"iid\": iid($root)};",
                    TxType::Read,
                )
                .await
                .unwrap()
            else {
                panic!("committed roots")
            };
            let counts = {
                let log = log.lock().unwrap();
                (log.commits, log.rollbacks)
            };
            if fail {
                assert_eq!(result.unwrap_err().op_index, Some(1));
                assert!(pending.iter().all(|answer| answer.get().is_err()));
                assert!(roots.is_empty());
                assert_eq!(counts, (0, 1));
            } else {
                result.unwrap();
                assert_eq!(roots.len(), 1);
                let StagedAnswerV3::Rows { rows } =
                    pending[0].get().unwrap().to_staged_v3(&source).unwrap()
                else {
                    panic!("allocated IID")
                };
                let [RemoteValue::Thing { iid, .. }] = rows[0].as_slice() else {
                    panic!("one thing")
                };
                assert_eq!(roots[0]["iid"], iid.as_str());
                let StagedAnswerV3::Documents { documents } =
                    pending[1].get().unwrap().to_staged_v3(&target).unwrap()
                else {
                    panic!("final document")
                };
                assert_eq!(documents.len(), 1);
                let RemoteFieldValue::Document { fields } = &documents[0]["attributes"] else {
                    panic!("attributes")
                };
                let RemoteFieldValue::List { values: actual } = &fields["reference-value"] else {
                    panic!("values")
                };
                assert_eq!(
                    actual
                        .iter()
                        .cloned()
                        .collect::<std::collections::BTreeSet<_>>(),
                    values.iter().cloned().collect()
                );
                assert_eq!(counts, (1, 0));
                database
                    .execute_raw(
                        "match $root isa reference-note; delete $root;",
                        TxType::Write,
                    )
                    .await
                    .unwrap();
            }
            assert_eq!(log.lock().unwrap().opened, [TxType::Write]);
            assert_eq!(log.lock().unwrap().statements.len(), 2);
        }
    }
    for literal in [false, true] {
        for fail in [false, true] {
            let (counted, log) = counted_connection(&database, literal).await;
            log.lock().unwrap().fail_at = fail.then_some(2);
            let transaction = counted.transaction_context(TxType::Write).await.unwrap();
            let policy = QueryExecutionResourceLimits::default();
            let answers = transaction
                .execute_staged_plans(
                    &stack,
                    &declared,
                    &policy,
                    &AnswerCancellation::default(),
                    type_bridge_orm::QueryExecutionDeadline::for_limits(policy),
                )
                .await;
            if fail {
                assert!(answers.is_err());
                assert_eq!(
                    transaction.lifecycle_state().await,
                    type_bridge_orm::TransactionContextState::RollbackOnly
                );
                assert!(transaction.commit_sdk().await.is_err());
                transaction.rollback().await.unwrap();
            } else {
                let answers = answers.unwrap();
                assert_eq!(answers.len(), 2);
                let StagedAnswerV3::Documents { documents } =
                    answers[1].to_staged_v3(&target).unwrap()
                else {
                    panic!("borrowed final document")
                };
                let RemoteFieldValue::Document { fields } = &documents[0]["attributes"] else {
                    panic!("borrowed attributes")
                };
                let RemoteFieldValue::List { values: actual } = &fields["reference-value"] else {
                    panic!("borrowed values")
                };
                assert_eq!(
                    actual
                        .iter()
                        .cloned()
                        .collect::<std::collections::BTreeSet<_>>(),
                    values.iter().cloned().collect()
                );
                transaction.commit_sdk().await.unwrap();
            }
            let QueryResult::Documents(roots) = database
                .execute_raw(
                    "match $root isa reference-note; fetch {\"iid\": iid($root)};",
                    TxType::Read,
                )
                .await
                .unwrap()
            else {
                panic!("borrowed roots")
            };
            assert_eq!(roots.len(), usize::from(!fail));
            let counts = {
                let log = log.lock().unwrap();
                (
                    log.commits,
                    log.rollbacks,
                    log.statements.len(),
                    log.opened.clone(),
                )
            };
            assert_eq!(
                counts,
                (
                    usize::from(!fail),
                    usize::from(fail),
                    2,
                    vec![TxType::Write]
                )
            );
            if !fail {
                database
                    .execute_raw(
                        "match $root isa reference-note; delete $root;",
                        TxType::Write,
                    )
                    .await
                    .unwrap();
            }
        }
    }
    // A zero-row optional source is a static branch gate, without client IIDs.
    let note = TypeId::new(TypeKind::Entity, "reference-note").unwrap();
    let mut read = type_bridge_contract::query_plan::v3::PlanBuilder::new();
    let selected = read.exact(note.clone());
    read.returning(vec![selected]);
    let read = read
        .build(&declared, managed.managed_semantic_schema().clone())
        .unwrap();
    let ordinal = BindingId::new(1).unwrap();
    let gate = QueryPlanV3::new(
        PlanSpec {
            bindings: vec![
                AssertionBinding::new(root, QueryVariable::new("root").unwrap()),
                AssertionBinding::new(ordinal, QueryVariable::new("ordinal").unwrap()),
            ],
            inputs: vec![
                InputColumn::new(
                    InputColumnId::new(0),
                    QueryVariable::new("candidate_iid").unwrap(),
                    ValueTypeTag::String,
                    true,
                ),
                InputColumn::new(
                    InputColumnId::new(1),
                    QueryVariable::new("input_ordinal").unwrap(),
                    ValueTypeTag::Long,
                    false,
                ),
            ],
            given_rows: None,
            operation: Operation::Write {
                returns: WriteReturns::Rows,
            },
            pipeline: vec![
                Stage::Match {
                    patterns: vec![
                        QueryPattern::Not {
                            patterns: vec![QueryPattern::Value {
                    comparator: type_bridge_contract::migration_assertion::ValueComparator::Equal,
                    left: QueryOperand::Input { column: InputColumnId::new(0) },
                    right: QueryOperand::Input { column: InputColumnId::new(0) },
                }],
                        }
                        .into(),
                        type_bridge_contract::query_plan::v3::Pattern::Let {
                            binding: ordinal,
                            expr: type_bridge_contract::query_plan::v3::Expr::Operand(
                                QueryOperand::Input {
                                    column: InputColumnId::new(1),
                                },
                            ),
                        },
                    ],
                },
                Stage::Insert {
                    things: vec![InsertThing {
                        binding: root,
                        type_id: note.clone(),
                        fields: vec![],
                        players: vec![],
                    }],
                },
            ],
            output: QueryOutput::Rows {
                columns: vec![root, ordinal],
            },
            managed_semantics: managed.managed_semantic_schema().clone(),
            compatibility: None,
            functions: vec![],
        },
        &declared,
    )
    .unwrap();
    let mut present = type_bridge_contract::query_plan::v3::PlanBuilder::new();
    let candidate = present.input("candidate_iid", ValueTypeTag::String, true);
    let selected = present.exact(note.clone());
    present
        .by_iid_input(selected, candidate)
        .returning(vec![selected]);
    let present = present
        .build(&declared, managed.managed_semantic_schema().clone())
        .unwrap();
    let reference = StagedResultRefV3 {
        input: InputColumnId::new(0),
        source_plan: 0,
        source_column: 0,
        expected_types: vec![note.clone()],
    };
    let gates = vec![
        StagedPlanV3::new(&read, vec![], &declared).unwrap(),
        StagedPlanV3::new_with_result_refs(
            &gate,
            vec![InputRow::new(vec![None, Some(CanonicalValue::Long(0))])],
            vec![reference.clone()],
            &declared,
        )
        .unwrap(),
        StagedPlanV3::new_with_result_refs(
            &present,
            vec![InputRow::new(vec![None])],
            vec![reference],
            &declared,
        )
        .unwrap(),
    ];
    for literal in [false, true] {
        for existing in [false, true] {
            let (counted, log) = counted_connection(&database, literal).await;
            let mut transaction = counted.transaction(TransactionOptions::default());
            let pending = transaction
                .stage_plan_stack(gates.clone(), Arc::clone(&declared))
                .unwrap();
            transaction.commit().await.unwrap();
            let StagedAnswerV3::Rows { rows } =
                pending[1].get().unwrap().to_staged_v3(&gate).unwrap()
            else {
                panic!("conditional allocation rows")
            };
            assert_eq!(rows.len(), usize::from(!existing));
            // Matching the captured IID cannot observe the root allocated on a miss.
            let StagedAnswerV3::Rows { rows } =
                pending[2].get().unwrap().to_staged_v3(&present).unwrap()
            else {
                panic!("captured present rows")
            };
            assert_eq!(rows.len(), usize::from(existing));
            let QueryResult::Documents(roots) = database
                .execute_raw(
                    "match $root isa reference-note; fetch {\"iid\": iid($root)};",
                    TxType::Read,
                )
                .await
                .unwrap()
            else {
                panic!("conditional roots")
            };
            assert_eq!(roots.len(), 1);
            let counts = {
                let log = log.lock().unwrap();
                (log.commits, log.statements.len(), log.opened.clone())
            };
            assert_eq!(counts, (1, 3, vec![TxType::Write]));
        }
        database
            .execute_raw(
                "match $root isa reference-note; delete $root;",
                TxType::Write,
            )
            .await
            .unwrap();
    }
}
#[tokio::test]
#[ignore = "requires a live TypeDB server"]
async fn reduced_full_match_equals_stepwise_candidates_live() {
    use std::collections::BTreeSet;
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::migration_assertion::{
        AssertionBinding, BindingId, QueryVariable, ValueComparator,
    };
    use type_bridge_contract::query_plan::v3::{
        Expr, Operation, Pattern, PlanSpec, QueryPlanV3, Stage,
    };
    use type_bridge_contract::query_plan::{
        InputColumn, InputColumnId, InputRow, QueryOperand, QueryOutput, QueryPattern,
        ReduceAssignment, Reducer,
    };
    use type_bridge_contract::query_remote::RemoteValue;
    use type_bridge_contract::query_remote_v3::StagedAnswerV3;
    use type_bridge_contract::value::ValueTypeTag;
    use type_bridge_orm::_manager::DynamicEntityManager;
    use type_bridge_orm::Filter;
    use type_bridge_orm::{AttributeValue, QueryExecutionDeadline};

    let _guard = crate::common::integration_test_guard().await;
    let database = setup_db().await;
    database.delete_database().await.unwrap();
    database.create_database().await.unwrap();
    let yaml = "format: typebridge.schema/v2\nattributes:\n  reduced-tag: { value: string }\nentities:\n  reduced-note:\n    owns:\n      reduced-tag: { card: { min: 0 } }\n";
    let declared = normalize_documents(
        &SchemaDocumentSet::parse([(DocumentId::new("reduced-full-match.yaml").unwrap(), yaml)])
            .unwrap(),
    )
    .unwrap();
    let profile = SemanticProfileId::new("typedb-3.13.6/v1").unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    let installed = InstalledRuntimeProjection::try_new(
        project(
            &resolved,
            BindingTarget::Rust,
            &ProjectionConfig::rust(),
            &[ProjectionHandler::rust_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    let context = type_bridge_schema::ManagedDeltaContext::new(
        ManagedScopeId::new("reduced-full-match-live").unwrap(),
        profile,
        type_bridge_contract::capability::CapabilitySet::new(),
    );
    let managed = type_bridge_schema::managed_schema_state(&declared, &context).unwrap();
    database.execute_raw("define attribute reduced-tag, value string; entity reduced-note, owns reduced-tag @card(0..);", TxType::Schema).await.unwrap();
    let note = TypeId::new(TypeKind::Entity, "reduced-note").unwrap();
    let attribute = AttributeId::new("reduced-tag").unwrap();
    let descriptor = installed
        .manager_registry()
        .unwrap()
        .entity("reduced-note")
        .unwrap();
    let direct = DynamicEntityManager::from_registry(
        &database,
        installed.manager_registry().unwrap(),
        descriptor,
    );
    let full = (0..300)
        .map(|index| format!("value-{index:04}"))
        .collect::<Vec<_>>();
    let seed = direct
        .insert(
            &full
                .iter()
                .map(|value| ("reduced-tag".into(), AttributeValue::String(value.clone())))
                .collect(),
        )
        .await
        .unwrap()
        .item()
        .expect("one written root")
        .iid
        .clone()
        .unwrap();
    for case in ["hit", "last-near-miss", "absent"] {
        let mut wanted = full.clone();
        if case == "last-near-miss" {
            wanted[299] = "last-missing".into();
        }
        if case == "absent" {
            wanted
                .iter_mut()
                .for_each(|value| *value = format!("absent-{value}"));
        }
        let mut expected: Option<BTreeSet<String>> = None;
        for chunk in wanted.chunks(32) {
            let filters = chunk
                .iter()
                .map(|value| Filter::eq("reduced-tag", AttributeValue::String(value.clone())))
                .collect::<Vec<_>>();
            let found = direct
                .get(&filters)
                .await
                .unwrap()
                .into_iter()
                .map(|row| row.iid.expect("hydrated direct IID"))
                .collect::<BTreeSet<_>>();
            match &mut expected {
                Some(expected) => expected.retain(|iid| found.contains(iid)),
                None => expected = Some(found),
            }
        }
        let expected = expected.unwrap();
        assert_eq!(
            expected,
            if case == "hit" {
                BTreeSet::from([seed.clone()])
            } else {
                BTreeSet::new()
            }
        );
        let id = |index| BindingId::new(index).unwrap();
        let root = id(0);
        let field = id(1);
        let value = id(2);
        let count = id(3);
        let binding = |binding| QueryOperand::Binding { binding };
        let plan = QueryPlanV3::new(
            PlanSpec {
                bindings: ["root", "field", "wanted", "matched"]
                    .into_iter()
                    .enumerate()
                    .map(|(index, name)| {
                        AssertionBinding::new(id(index as u16), QueryVariable::new(name).unwrap())
                    })
                    .collect(),
                inputs: vec![InputColumn::new(
                    InputColumnId::new(0),
                    QueryVariable::new("input_value").unwrap(),
                    ValueTypeTag::String,
                    false,
                )],
                given_rows: Some(
                    wanted
                        .iter()
                        .map(|value| {
                            InputRow::new(vec![Some(CanonicalValue::String(
                                CanonicalString::new(value.clone()).unwrap(),
                            ))])
                        })
                        .collect(),
                ),
                operation: Operation::Rows,
                pipeline: vec![
                    Stage::Match {
                        patterns: vec![
                            QueryPattern::Isa {
                                binding: root,
                                type_id: note.clone(),
                                include_subtypes: true,
                            }
                            .into(),
                            QueryPattern::Has {
                                owner: root,
                                attribute: field,
                                attribute_id: attribute.clone(),
                            }
                            .into(),
                            QueryPattern::Value {
                                comparator: ValueComparator::Equal,
                                left: binding(field),
                                right: binding(value),
                            }
                            .into(),
                            Pattern::Let {
                                binding: value,
                                expr: Expr::Operand(QueryOperand::Input {
                                    column: InputColumnId::new(0),
                                }),
                            },
                        ],
                    },
                    Stage::Select {
                        bindings: vec![root, value],
                    },
                    Stage::Distinct,
                    Stage::Reduce {
                        assignments: vec![ReduceAssignment::new(count, Reducer::Count, None)],
                        groups: vec![root],
                    },
                    Stage::Match {
                        patterns: vec![
                            QueryPattern::Value {
                                comparator: ValueComparator::Equal,
                                left: binding(count),
                                right: QueryOperand::Literal {
                                    value: CanonicalValue::Long(300),
                                },
                            }
                            .into(),
                        ],
                    },
                    Stage::Select {
                        bindings: vec![root, count],
                    },
                ],
                output: QueryOutput::Rows {
                    columns: vec![root, count],
                },
                managed_semantics: managed.managed_semantic_schema().clone(),
                compatibility: None,
                functions: vec![],
            },
            &declared,
        )
        .unwrap();
        let (counted, log) = counted_connection(&database, false).await;
        let limits = QueryExecutionResourceLimits::default();
        let answers = counted
            .execute_plans(
                std::slice::from_ref(&plan),
                &limits,
                &AnswerCancellation::default(),
                QueryExecutionDeadline::for_limits(limits),
            )
            .await
            .unwrap();
        let StagedAnswerV3::Rows { rows } = answers[0].to_staged_v3(&plan).unwrap() else {
            panic!("typed rows")
        };
        let actual = rows
            .into_iter()
            .map(|row| {
                let [
                    RemoteValue::Thing { iid, .. },
                    RemoteValue::Value {
                        value: CanonicalValue::Long(count),
                    },
                ] = row.as_slice()
                else {
                    panic!("typed root and count: {row:?}")
                };
                assert_eq!(*count, 300);
                iid.clone()
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(actual, expected, "{case}");
        let observed = log.lock().unwrap();
        assert_eq!(observed.opened, [TxType::Read]);
        assert_eq!(observed.statements.len(), 1);
        assert_eq!(observed.commits, 0);
        eprintln!(
            "REDUCE_REMATCH case={case} candidates={} statements=1 opened=1 commits=0",
            actual.len()
        );
    }
    database.delete_database().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a live TypeDB server"]
async fn mixed_wide_full_match_puts_use_static_probes_and_rollback_last_piece_live() {
    use type_bridge_orm::_dynamic::DynamicAttributeMap;
    use type_bridge_orm::_manager::DynamicEntityManager;
    use type_bridge_orm::AttributeValue;
    let _guard = crate::common::integration_test_guard().await;
    let database = setup_db().await;
    database.delete_database().await.unwrap();
    database.create_database().await.unwrap();
    let yaml = "format: typebridge.schema/v2\nattributes:\n  mixed-tag: { value: string }\n  mixed-alias: { value: string }\n  mixed-number: { value: integer }\nentities:\n  mixed-note:\n    owns:\n      mixed-tag: { card: { min: 0 } }\n      mixed-alias: { card: { min: 0 } }\n      mixed-number: { card: { min: 0 } }\n";
    let declared = normalize_documents(
        &SchemaDocumentSet::parse([(DocumentId::new("mixed-wide-match.yaml").unwrap(), yaml)])
            .unwrap(),
    )
    .unwrap();
    let installed = InstalledRuntimeProjection::try_new(
        project(
            &resolve(
                &declared,
                &SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
            )
            .unwrap(),
            BindingTarget::Rust,
            &ProjectionConfig::rust(),
            &[ProjectionHandler::rust_v1()],
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    database.execute_raw("define attribute mixed-tag, value string; attribute mixed-alias, value string; attribute mixed-number, value integer; entity mixed-note, owns mixed-tag @card(0..), owns mixed-alias @card(0..), owns mixed-number @card(0..);", TxType::Schema).await.unwrap();
    let registry = installed.manager_registry().unwrap();
    let note = registry.entity("mixed-note").unwrap();
    let normalized = |fields: DynamicAttributeMap| {
        fields
            .into_iter()
            .map(|(name, value)| format!("{name}={value:?}"))
            .collect::<std::collections::BTreeSet<_>>()
    };
    for literal in [false, true] {
        let raw = DynamicEntityManager::from_registry(
            &database,
            installed.manager_registry().unwrap(),
            note.clone(),
        );
        let full = (0..100)
            .flat_map(|index| {
                [
                    (
                        "mixed-tag".to_owned(),
                        AttributeValue::String(format!("mixed-{literal}-{index}")),
                    ),
                    (
                        "mixed-alias".to_owned(),
                        AttributeValue::String(format!("mixed-{literal}-{index}")),
                    ),
                    ("mixed-number".to_owned(), AttributeValue::Long(index)),
                ]
            })
            .collect::<DynamicAttributeMap>();
        let hit = raw
            .insert(&full)
            .await
            .unwrap()
            .item()
            .expect("one written root")
            .iid
            .clone()
            .unwrap();
        let mut near = full.clone();
        near[299] = ("mixed-number".into(), AttributeValue::Long(10_000));
        let mut repeated = full.clone();
        repeated.extend(full[..3].iter().cloned());
        let expected = [normalized(full.clone()), normalized(near.clone())];
        let (counted, log) = counted_connection(&database, literal).await;
        let manager = DynamicEntityManager::from_registry(&counted, registry.clone(), note.clone());
        let before = raw.count().await.unwrap();
        let actual = manager.put_many(&[repeated.clone(), near]).await.unwrap();
        assert_eq!(actual[0], hit);
        assert_ne!(actual[1], hit);
        assert_eq!(raw.count().await.unwrap(), before + 1);
        for (iid, expected) in actual.iter().zip(expected) {
            assert_eq!(
                normalized(raw.get_by_iid(iid).await.unwrap().unwrap().attributes),
                expected
            );
        }
        {
            let observed = log.lock().unwrap();
            assert_eq!(observed.statements.len(), 8);
            assert_eq!(observed.opened, [TxType::Write]);
            assert_eq!((observed.commits, observed.rollbacks), (1, 0));
        }
        let before = raw.count().await.unwrap();
        let mut failing = full;
        failing[299] = ("mixed-number".into(), AttributeValue::Long(20_000));
        *log.lock().unwrap() = DispatchLog {
            fail_at: Some(8),
            ..Default::default()
        };
        assert!(manager.put_many(&[repeated, failing]).await.is_err());
        assert_eq!(raw.count().await.unwrap(), before);
        {
            let observed = log.lock().unwrap();
            assert_eq!(observed.statements.len(), 8);
            assert_eq!(observed.opened, [TxType::Write]);
            assert_eq!((observed.commits, observed.rollbacks), (0, 1));
        }
        eprintln!(
            "mixed scalar full-match literal={literal}: 300 field/value pairs, same values across fields, duplicate predicates; full hit reused, one-value near miss allocated, last piece rolled back; eight fixed statements per two-row call"
        );
    }
    database.delete_database().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the assigned live TypeDB profile"]
async fn dynamic_nonkey_single_write_results_follow_typeql_fanout_and_role_cardinality_live() {
    use type_bridge_orm::_dynamic::DynamicRolePlayerInput;
    use type_bridge_orm::_manager::{DynamicEntityManager, DynamicRelationManager};
    use type_bridge_orm::AttributeValue;
    let database = setup_db().await;
    if !server_supports_v2_conformance(&database) {
        database.delete_database().await.unwrap();
        return;
    }
    let yaml = r#"format: typebridge.schema/v2
attributes:
  fanout-key: { value: string }
  fanout-selector: { value: string }
  fanout-tag: { value: string }
entities:
  fanout-person:
    owns:
      fanout-key: { key: true }
      fanout-selector: { card: { min: 0, max: 1 } }
relations:
  fanout-edge:
    owns:
      fanout-tag: { card: { min: 0 } }
    relates:
      member: { card: 1 }
plays:
  fanout-person:
    fanout-edge: [member]
"#;
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("dynamic-fanout.yaml").unwrap(), yaml)])
            .unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let profile = SemanticProfileId::new("typedb-3.13.6/v1").unwrap();
    let resolved = resolve(&declared, &profile).unwrap();
    let projection = project(
        &resolved,
        BindingTarget::Python,
        &ProjectionConfig::python(),
        &[ProjectionHandler::python_v1()],
        &[],
    )
    .unwrap();
    let context = type_bridge_schema::ManagedDeltaContext::new(
        type_bridge_contract::managed_scope::ManagedScopeId::new("dynamic-fanout").unwrap(),
        profile,
        type_bridge_schema::schema_authority_capability_vocabulary(),
    );
    let authority = type_bridge_schema::build_schema_authority(
        &declared,
        declared.required_capabilities(),
        &context,
    )
    .unwrap();
    let installed = InstalledRuntimeProjection::try_new(projection)
        .unwrap()
        .with_schema_authority(&authority);
    let registry = installed.manager_registry().unwrap();
    database.execute_raw("define attribute fanout-key, value string; attribute fanout-selector, value string; attribute fanout-tag, value string; entity fanout-person, owns fanout-key @key, owns fanout-selector @card(0..1), plays fanout-edge:member; relation fanout-edge, owns fanout-tag @card(0..), relates member @card(1);", TxType::Schema).await.unwrap();
    let people = DynamicEntityManager::from_registry(
        &database,
        Arc::clone(&registry),
        registry.entity("fanout-person").unwrap(),
    );
    let mut player_iids = Vec::new();
    for key in ["alice", "bob"] {
        let result = people
            .insert(&vec![
                ("fanout-key".into(), AttributeValue::String(key.into())),
                (
                    "fanout-selector".into(),
                    AttributeValue::String("shared".into()),
                ),
            ])
            .await
            .unwrap();
        assert_eq!(result.count(), 1);
        player_iids.push(result.item().unwrap().iid.clone().unwrap());
    }
    let all_edges = DynamicRelationManager::from_registry(
        &database,
        Arc::clone(&registry),
        registry.relation("fanout-edge").unwrap(),
    );
    let player = |iid: Option<String>, key: Option<(&str, &str)>| DynamicRolePlayerInput {
        role_name: "member".into(),
        player_type_name: "fanout-person".into(),
        iid,
        key: key.map(|(field, value)| (field.into(), AttributeValue::String(value.into()))),
    };
    for literal in [false, true] {
        let (counted, log) = counted_connection(&database, literal).await;
        let entities = DynamicEntityManager::from_registry(
            &counted,
            Arc::clone(&registry),
            registry.entity("fanout-person").unwrap(),
        );
        let entities = if literal {
            entities.literal_values()
        } else {
            entities
        };
        let missing_iid = format!("{}ff", &player_iids[0][..player_iids[0].len() - 2]);
        let empty = entities
            .update_exact(
                &missing_iid,
                &vec![(
                    "fanout-selector".into(),
                    AttributeValue::String("absent".into()),
                )],
            )
            .await
            .unwrap();
        assert_eq!(empty.count(), 0);
        assert!(empty.items().is_empty());
        assert!(empty.item().is_none());
        {
            let observed = log.lock().unwrap();
            assert_eq!(observed.statements.len(), 1);
            assert_eq!(observed.opened, [TxType::Write]);
            assert_eq!((observed.commits, observed.rollbacks), (1, 0));
        }
        for borrowed in [false, true] {
            let before = all_edges.count().await.unwrap();
            *log.lock().unwrap() = DispatchLog::default();
            let tx = if borrowed {
                Some(counted.transaction_context(TxType::Write).await.unwrap())
            } else {
                None
            };
            let mut manager = if let Some(tx) = &tx {
                DynamicRelationManager::with_registry_transaction(
                    tx.clone(),
                    Arc::clone(&registry),
                    registry.relation("fanout-edge").unwrap(),
                )
            } else {
                DynamicRelationManager::from_registry(
                    &counted,
                    Arc::clone(&registry),
                    registry.relation("fanout-edge").unwrap(),
                )
            };
            if literal {
                manager = manager.literal_values();
            }
            let result = manager
                .insert(
                    &vec![],
                    &[player(None, Some(("fanout-selector", "shared")))],
                )
                .await
                .unwrap();
            assert_eq!(result.count(), 2);
            assert!(result.item().is_none());
            assert_eq!(
                result
                    .items()
                    .iter()
                    .map(|row| row.iid.clone().unwrap())
                    .collect::<std::collections::BTreeSet<_>>()
                    .len(),
                2
            );
            assert_eq!(
                result
                    .items()
                    .iter()
                    .map(|row| {
                        assert_eq!(row.role_players.len(), 1);
                        row.role_players[0].player_iid.clone().unwrap()
                    })
                    .collect::<std::collections::BTreeSet<_>>(),
                player_iids.iter().cloned().collect()
            );
            {
                let observed = log.lock().unwrap();
                assert_eq!(observed.statements.len(), 1);
                assert_eq!(observed.opened, [TxType::Write]);
                assert_eq!(observed.commits, usize::from(!borrowed));
                assert_eq!(observed.rollbacks, 0);
                for forbidden in ["count(", "limit 1", " != "] {
                    assert!(!observed.statements[0].contains(forbidden));
                }
            }
            if let Some(tx) = tx {
                tx.commit_sdk().await.unwrap();
            }
            assert_eq!(log.lock().unwrap().commits, 1);
            assert_eq!(all_edges.count().await.unwrap(), before + 2);
            eprintln!(
                "DYNAMIC_NONKEY_FANOUT literal={literal} borrowed={borrowed} count=2 statements=1 transactions=1"
            );
        }
        let manager = DynamicRelationManager::from_registry(
            &counted,
            Arc::clone(&registry),
            registry.relation("fanout-edge").unwrap(),
        );
        let manager = if literal {
            manager.literal_values()
        } else {
            manager
        };
        for key_alias in [false, true] {
            let first = player(Some(player_iids[0].clone()), None);
            let second = if key_alias {
                player(None, Some(("fanout-key", "alice")))
            } else {
                first.clone()
            };
            let players = [first, second];
            for operation in ["insert", "put_exact", "update_exact"] {
                let seed = manager
                    .insert(&vec![], &[player(Some(player_iids[0].clone()), None)])
                    .await
                    .unwrap();
                let iid = seed.item().unwrap().iid.clone().unwrap();
                *log.lock().unwrap() = DispatchLog::default();
                let result = match operation {
                    "insert" => manager.insert(&vec![], &players).await,
                    "put_exact" => manager.put_exact(&vec![], &players).await,
                    _ => manager.update_exact(&iid, &vec![], &players).await,
                }
                .unwrap();
                assert_eq!(result.count(), 1);
                assert_eq!(result.item().unwrap().role_players.len(), 1);
                assert_eq!(
                    result.item().unwrap().role_players[0].player_iid.as_deref(),
                    Some(player_iids[0].as_str())
                );
                let observed = log.lock().unwrap();
                assert_eq!(observed.statements.len(), 1);
                assert_eq!(observed.opened, [TxType::Write]);
                assert_eq!((observed.commits, observed.rollbacks), (1, 0));
            }
        }
        let before = all_edges.count().await.unwrap();
        let provider_error = database.execute_raw(&format!("match $a isa fanout-person, iid {}; $b isa fanout-person, iid {}; insert $r isa fanout-edge, links (member: $a, member: $b);", player_iids[0], player_iids[1]), TxType::Write).await.unwrap_err();
        assert!(
            provider_error
                .to_string()
                .to_lowercase()
                .contains("cardinality"),
            "real TypeDB role cardinality refusal: {provider_error}"
        );
        assert_eq!(all_edges.count().await.unwrap(), before);
        *log.lock().unwrap() = DispatchLog::default();
        assert!(
            manager
                .insert(
                    &vec![],
                    &[
                        player(Some(player_iids[0].clone()), None),
                        player(Some(player_iids[1].clone()), None)
                    ]
                )
                .await
                .is_err()
        );
        assert_eq!(log.lock().unwrap().statements.len(), 1);
        assert_eq!(all_edges.count().await.unwrap(), before);
        let seed = manager
            .insert(&vec![], &[player(Some(player_iids[0].clone()), None)])
            .await
            .unwrap();
        let existing_iid = seed.item().unwrap().iid.clone().unwrap();
        let absent_iid = format!("{}ff", &existing_iid[..existing_iid.len() - 2]);
        let before = all_edges.count().await.unwrap();
        for operation in [
            "insert",
            "put",
            "update_missing_player",
            "update_missing_root",
        ] {
            let missing = [player(None, Some(("fanout-selector", "missing")))];
            let valid = [player(Some(player_iids[0].clone()), None)];
            *log.lock().unwrap() = DispatchLog::default();
            let empty = match operation {
                "insert" => manager.insert(&vec![], &missing).await,
                "put" => manager.put(&vec![], &missing).await,
                "update_missing_player" => {
                    manager.update(Some(&existing_iid), &vec![], &missing).await
                }
                _ => manager.update(Some(&absent_iid), &vec![], &valid).await,
            }
            .unwrap();
            assert_eq!(empty.count(), 0, "{operation}");
            assert!(empty.items().is_empty());
            assert!(empty.item().is_none());
            {
                let observed = log.lock().unwrap();
                assert_eq!(observed.statements.len(), 1, "{operation}");
                assert_eq!(observed.opened, [TxType::Write]);
                assert_eq!((observed.commits, observed.rollbacks), (1, 0));
            }
            assert_eq!(all_edges.count().await.unwrap(), before);
        }
    }
    for literal in [false, true] {
        let (counted, log) = counted_connection(&database, literal).await;
        let manager = DynamicRelationManager::from_registry(
            &counted,
            Arc::clone(&registry),
            registry.relation("fanout-edge").unwrap(),
        );
        let manager = if literal {
            manager.literal_values()
        } else {
            manager
        };
        for operation in ["insert", "put_miss", "put_hit"] {
            let suffix = if operation == "insert" {
                "insert"
            } else {
                "put"
            };
            let attributes = (0..300)
                .map(|index| {
                    (
                        "fanout-tag".into(),
                        AttributeValue::String(format!("wide-{literal}-{suffix}-{index}")),
                    )
                })
                .collect::<Vec<_>>();
            let expected = attributes
                .iter()
                .map(|(_, value)| match value {
                    AttributeValue::String(value) => value.clone(),
                    other => panic!("expected string tag, got {other:?}"),
                })
                .collect::<std::collections::BTreeSet<_>>();
            let before = all_edges.count().await.unwrap();
            *log.lock().unwrap() = DispatchLog::default();
            let players = [player(None, Some(("fanout-selector", "shared")))];
            let result = if operation == "insert" {
                manager.insert(&attributes, &players).await
            } else {
                manager.put(&attributes, &players).await
            }
            .unwrap_or_else(|error| {
                panic!(
                    "wide {operation} literal={literal}: {error}; statements={:#?}",
                    log.lock().unwrap().statements
                )
            });
            assert_eq!(result.count(), 2);
            for row in result.items() {
                assert_eq!(row.role_players.len(), 1);
                assert_eq!(row.attributes.len(), 300);
                let values = row
                    .attributes
                    .iter()
                    .map(|(name, value)| {
                        assert_eq!(name, "fanout-tag");
                        match value {
                            AttributeValue::String(value) => value.clone(),
                            other => panic!("expected string tag, got {other:?}"),
                        }
                    })
                    .collect::<std::collections::BTreeSet<_>>();
                assert_eq!(values, expected);
            }
            {
                let observed = log.lock().unwrap();
                assert_eq!(
                    observed.statements.len(),
                    if operation == "insert" { 3 } else { 5 }
                );
                assert_eq!(observed.opened, [TxType::Write]);
                assert_eq!((observed.commits, observed.rollbacks), (1, 0));
                eprintln!(
                    "DYNAMIC_WIDE_NONKEY_FANOUT literal={literal} operation={operation} count=2 values=300 statements={} transactions=1",
                    observed.statements.len()
                );
            }
            assert_eq!(
                all_edges.count().await.unwrap(),
                before + if operation == "put_hit" { 0 } else { 2 }
            );
        }
        let attributes = (0..300)
            .map(|index| {
                (
                    "fanout-tag".into(),
                    AttributeValue::String(format!("wide-zero-{literal}-{index}")),
                )
            })
            .collect::<Vec<_>>();
        let seed = manager
            .insert(&vec![], &[player(Some(player_iids[0].clone()), None)])
            .await
            .unwrap();
        let existing_iid = seed.item().unwrap().iid.clone().unwrap();
        let absent_iid = format!("{}ff", &existing_iid[..existing_iid.len() - 2]);
        let before = all_edges.count().await.unwrap();
        for operation in [
            "insert",
            "put",
            "update_missing_player",
            "update_missing_root",
        ] {
            let missing = [player(None, Some(("fanout-selector", "missing")))];
            let valid = [player(Some(player_iids[0].clone()), None)];
            *log.lock().unwrap() = DispatchLog::default();
            let result = match operation {
                "insert" => manager.insert(&attributes, &missing).await,
                "put" => manager.put(&attributes, &missing).await,
                "update_missing_player" => {
                    manager
                        .update(Some(&existing_iid), &attributes, &missing)
                        .await
                }
                _ => manager.update(Some(&absent_iid), &attributes, &valid).await,
            }
            .unwrap();
            assert_eq!(result.count(), 0, "wide {operation}");
            assert!(result.items().is_empty());
            assert!(result.item().is_none());
            {
                let observed = log.lock().unwrap();
                assert_eq!(
                    observed.statements.len(),
                    match operation {
                        "insert" => 3,
                        "put" => 5,
                        _ => 2,
                    },
                    "wide {operation}"
                );
                assert_eq!(observed.opened, [TxType::Write]);
                assert_eq!((observed.commits, observed.rollbacks), (1, 0));
                eprintln!(
                    "DYNAMIC_WIDE_ZERO literal={literal} operation={operation} count=0 values=300 statements={} transactions=1",
                    observed.statements.len()
                );
            }
            assert_eq!(all_edges.count().await.unwrap(), before);
        }
    }
    database.delete_database().await.unwrap();
    database.close().unwrap();
}
