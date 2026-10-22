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
    /// `(query source, given rows)` per `given` query, in order.
    given_queries: Vec<(String, usize)>,
    opened: Vec<TxType>,
    commits: usize,
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
    log: Arc<Mutex<DispatchLog>>,
}

struct CountingTransaction {
    inner: Box<dyn TransactionOps>,
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
            Ok(Box::new(CountingTransaction { inner, log }) as Box<dyn TransactionOps>)
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
        DriverBackend::supports_given_rows(&self.inner)
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
        self.inner.supports_given_rows()
    }
    fn schema_snapshot(&mut self) -> BoxFuture<'_, Result<Option<String>, OrmError>> {
        self.inner.schema_snapshot()
    }
    fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        self.inner.query(typeql)
    }
    fn query_canonical<'a>(
        &'a mut self,
        typeql: &str,
    ) -> BoxFuture<'a, Result<QueryResult, OrmError>> {
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
    ) -> BoxFuture<'a, Result<QueryResult, OrmError>> {
        if let Some(rows) = &rows {
            self.record(typeql, rows);
        }
        self.inner
            .query_materialized_bounded(typeql, rows, limits, canonical)
    }
    fn query_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        self.inner.query_bounded(typeql, limits, consumer)
    }
    fn query_v2_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        limits: AnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        self.inner.query_v2_bounded(typeql, limits, consumer)
    }
    fn query_v2_with_rows_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        rows: GivenRowsSpec,
        limits: AnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
        self.record(typeql, &rows);
        self.inner
            .query_v2_with_rows_bounded(typeql, rows, limits, consumer)
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
        self.inner.rollback()
    }
    fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.inner.close()
    }
}

impl CountingTransaction {
    fn record(&self, typeql: &str, rows: &GivenRowsSpec) {
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
            log: Arc::clone(&log),
        }),
        database.database_name(),
    );
    Some((database, counted, installed, ids, log))
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
            timeout_milliseconds: 600_000,
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

/// A batch under every limit is one mutation and one attach query.
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
    assert_eq!(calls.len(), 2, "one mutation stage and one attach stage");
    assert_eq!(calls[0].1.len(), 1, "the mutation fires once");
    assert_eq!(calls[1].1.len(), 1, "the attach fires once");
    assert_eq!(person_count(&database, &ids).await, 1_000);
    database.delete_database().await.unwrap();
}

/// Rows past 4,096 split into same-shape chunks on one Write with one commit.
///
/// 5,000 rows, not 10,000: the attach stage costs grow with rows times
/// stored instances, and a 10,000-row batch passes the provider's
/// transaction timeout on the attach stage alone.
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
    assert_eq!(calls.len(), 2, "one mutation stage and one attach stage");
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
