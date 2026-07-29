use std::env;
use std::sync::Arc;

use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::projection::BindingTarget;
use type_bridge_contract::projection_wire::decode_runtime_projection_verified;
use type_bridge_generated_schema::{
    Aliases, CanonicalDouble, Container, ContainerCreate, ContainerItemPlayer, ContainerType, Date,
    DateTime, DateTimeTz, Decimal, Duration, Employment, EmploymentCreate, EmploymentType,
    EncodedScalar, Event, EventCreate, EventRef, EventSubjectPlayer, EventType, HydratedPlayer,
    HydratedRow, HydrationCapability, Identifier, MaterializeModel, Membership, MembershipCreate,
    MembershipMemberRef, MembershipType, Model, Nickname, PROJECTION_FINGERPRINT_JSON, Person,
    PersonCreate, PersonRef, PersonType, RUNTIME_PROJECTION_JSON, SEMANTIC_SCHEMA_FINGERPRINT_JSON,
    Score, ValBool, ValConstrained, ValDate, ValDatetime, ValDatetimeTz, ValDecimal, ValDouble,
    ValDuration, materialize_model_for_test, plays_event_container_item,
};
use type_bridge_orm::{
    AttributeValue, ConnectOptions, Database, DynamicAttributeMap, DynamicEntityManager,
    DynamicEntityRow, DynamicRelationManager, DynamicRelationRow, DynamicRolePlayer,
    DynamicRolePlayerInput, InstalledRuntimeProjection, TxType, ensure_database_exists,
};

const PROVIDER_SCHEMA: &str = include_str!("provider-3.12.1.tql");

fn connect_options() -> ConnectOptions {
    let mut options = ConnectOptions::default();
    options.http_port = env::var("TYPEDB_HTTP_PORT")
        .unwrap_or_else(|_| "8000".to_owned())
        .parse()
        .expect("TYPEDB_HTTP_PORT is a valid nonzero u16");
    options.tls = env::var("TYPE_BRIDGE_RUST_PROJECTION_TLS").as_deref() == Ok("1");
    options
}

fn exact_type_id(kind: TypeKind, label: &str, generated_json: &str) -> TypeId {
    let id = TypeId::new(kind, label).expect("fixture type ID is valid");
    let canonical = to_canonical_json(&id).expect("fixture type ID canonicalizes");
    assert_eq!(
        generated_json,
        std::str::from_utf8(&canonical).expect("canonical IDs are UTF-8")
    );
    id
}

fn string_values(row: &DynamicEntityRow, attribute: &str) -> Vec<String> {
    let mut values = row
        .attributes
        .iter()
        .filter(|(name, _)| name == attribute)
        .map(|(_, value)| match value {
            AttributeValue::String(value) => value.clone(),
            other => panic!("{attribute} hydrated with the wrong value type: {other:?}"),
        })
        .collect::<Vec<_>>();
    values.sort();
    values
}

fn only_role<'a>(row: &'a DynamicRelationRow, role: &str) -> &'a DynamicRolePlayer {
    let players = row
        .role_players
        .iter()
        .filter(|player| player.role_name == role)
        .collect::<Vec<_>>();
    assert_eq!(players.len(), 1, "expected exactly one `{role}` player");
    players[0]
}

#[tokio::main]
async fn main() {
    let runtime = decode_runtime_projection_verified(
        RUNTIME_PROJECTION_JSON.as_bytes(),
        SEMANTIC_SCHEMA_FINGERPRINT_JSON.as_bytes(),
        PROJECTION_FINGERPRINT_JSON.as_bytes(),
    )
    .expect("generated Rust projection evidence verifies");
    assert_eq!(runtime.target(), BindingTarget::Rust);
    let installed = InstalledRuntimeProjection::try_new(runtime).expect("Rust projection installs");

    // Generated tokens are canonical identity evidence only. They deliberately do
    // not execute a query here; Query V2 and the typed Rust ORM companion/facade
    // that owns reusable input lowering and hydration belong to Flight 3/query execution.
    let person_id = exact_type_id(TypeKind::Entity, "person", PersonType::TOKEN.type_id_json());
    let membership_id = exact_type_id(
        TypeKind::Relation,
        "membership",
        MembershipType::TOKEN.type_id_json(),
    );
    let employment_id = exact_type_id(
        TypeKind::Relation,
        "employment",
        EmploymentType::TOKEN.type_id_json(),
    );
    let event_id = exact_type_id(TypeKind::Relation, "event", EventType::TOKEN.type_id_json());
    let container_id = exact_type_id(
        TypeKind::Relation,
        "container",
        ContainerType::TOKEN.type_id_json(),
    );
    assert_eq!(
        PersonType::identifier.owns_id_json(),
        r#"{"attribute":"identifier","owner":{"kind":"entity","label":"person"}}"#
    );
    assert_eq!(
        MembershipType::member.role_id_json(),
        r#"{"declaring_relation":"membership","label":"member"}"#
    );
    assert_eq!(
        EmploymentType::employee.role_id_json(),
        r#"{"declaring_relation":"employment","label":"employee"}"#
    );
    assert_eq!(
        EventType::subject.role_id_json(),
        r#"{"declaring_relation":"event","label":"subject"}"#
    );
    assert_eq!(
        ContainerType::item.role_id_json(),
        r#"{"declaring_relation":"container","label":"item"}"#
    );
    assert_eq!(
        plays_event_container_item.plays_id_json(),
        r#"{"player":{"kind":"relation","label":"event"},"role":{"declaring_relation":"container","label":"item"}}"#
    );

    let address = env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1729".to_owned());
    let database = env::var("TYPE_BRIDGE_RUST_PROJECTION_INTG_DATABASE")
        .unwrap_or_else(|_| format!("type_bridge_rust_projection_live_{}", std::process::id()));
    let username = env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned());
    let password = env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned());
    ensure_database_exists(&address, &database, &username, &password, connect_options())
        .await
        .expect("live projection database is created");
    let db = Database::connect_with_options(
        &address,
        &database,
        &username,
        &password,
        connect_options(),
    )
    .await
    .expect("live projection database connects");
    db.execute_raw(PROVIDER_SCHEMA, TxType::Schema)
        .await
        .expect("shared TypeDB 3.12.1 provider schema defines");

    let person_manager = DynamicEntityManager::new(
        &db,
        Arc::new(
            installed
                .entity_descriptor(&person_id)
                .expect("person descriptor exists")
                .clone(),
        ),
    );
    let membership_manager = DynamicRelationManager::new(
        &db,
        Arc::new(
            installed
                .relation_descriptor(&membership_id)
                .expect("membership descriptor exists")
                .clone(),
        ),
    );
    let employment_manager = DynamicRelationManager::new(
        &db,
        Arc::new(
            installed
                .relation_descriptor(&employment_id)
                .expect("employment descriptor exists")
                .clone(),
        ),
    );
    let event_manager = DynamicRelationManager::new(
        &db,
        Arc::new(
            installed
                .relation_descriptor(&event_id)
                .expect("event descriptor exists")
                .clone(),
        ),
    );
    let container_manager = DynamicRelationManager::new(
        &db,
        Arc::new(
            installed
                .relation_descriptor(&container_id)
                .expect("container descriptor exists")
                .clone(),
        ),
    );

    let score = Score::new(42i64).expect("score is valid");
    let v_double = ValDouble::new(CanonicalDouble::try_new(3.14).expect("double is valid"))
        .expect("val_double is valid");
    let v_decimal = ValDecimal::new(Decimal::try_new("123.45").expect("decimal is valid"))
        .expect("val_decimal is valid");
    let v_bool = ValBool::new(true).expect("val_bool is valid");
    let v_date = ValDate::new(Date::try_new("2026-07-28").expect("date is valid"))
        .expect("val_date is valid");
    let v_datetime =
        ValDatetime::new(DateTime::try_new("2026-07-28T03:55:00").expect("datetime is valid"))
            .expect("val_datetime is valid");
    let v_datetimetz = ValDatetimeTz::new(
        DateTimeTz::try_new("2026-07-28T03:55:00Z").expect("datetimetz is valid"),
    )
    .expect("val_datetimetz is valid");
    let v_duration = ValDuration::new(Duration::try_new("P1D").expect("duration is valid"))
        .expect("val_duration is valid");
    let v_constrained = ValConstrained::new(50i64).expect("val_constrained is valid");

    let person_create = PersonCreate::try_new(
        vec![
            Aliases::new("alpha").expect("alias is valid"),
            Aliases::new("beta").expect("alias is valid"),
        ],
        Identifier::new("person-1").expect("identifier is valid"),
        Some(Nickname::new("alice").expect("nickname is valid")),
        score,
        v_bool,
        v_constrained,
        v_date,
        v_datetime,
        v_datetimetz,
        v_decimal,
        v_double,
        v_duration,
    )
    .expect("person create input is valid");
    let mut person_attributes: DynamicAttributeMap = person_create
        .aliases()
        .iter()
        .map(|alias| {
            (
                "aliases".to_owned(),
                AttributeValue::String(alias.value().clone()),
            )
        })
        .collect();
    person_attributes.push((
        "identifier".to_owned(),
        AttributeValue::String(person_create.identifier().value().clone()),
    ));
    if let Some(nickname) = person_create.nickname() {
        person_attributes.push((
            "nickname".to_owned(),
            AttributeValue::String(nickname.value().clone()),
        ));
    }
    person_attributes.push((
        "score".to_owned(),
        AttributeValue::Long(*person_create.score().value()),
    ));
    person_attributes.push((
        "val_double".to_owned(),
        AttributeValue::Double(f64::from_bits(person_create.val_double().value().to_bits())),
    ));
    person_attributes.push((
        "val_decimal".to_owned(),
        AttributeValue::Decimal(person_create.val_decimal().value().as_str().to_owned()),
    ));
    person_attributes.push((
        "val_bool".to_owned(),
        AttributeValue::Boolean(*person_create.val_bool().value()),
    ));
    person_attributes.push((
        "val_date".to_owned(),
        AttributeValue::Date(person_create.val_date().value().as_str().to_owned()),
    ));
    person_attributes.push((
        "val_datetime".to_owned(),
        AttributeValue::DateTime(person_create.val_datetime().value().as_str().to_owned()),
    ));
    person_attributes.push((
        "val_datetime_tz".to_owned(),
        AttributeValue::DateTimeTZ(person_create.val_datetime_tz().value().as_str().to_owned()),
    ));
    person_attributes.push((
        "val_duration".to_owned(),
        AttributeValue::Duration(person_create.val_duration().value().as_str().to_owned()),
    ));
    person_attributes.push((
        "val_constrained".to_owned(),
        AttributeValue::Long(*person_create.val_constrained().value()),
    ));

    let person_iid = person_manager
        .insert(&person_attributes)
        .await
        .expect("generated person input inserts");
    let person_row = person_manager
        .get_by_iid_exact(&person_iid)
        .await
        .expect("exact person IID read succeeds")
        .expect("inserted person exists");
    assert_eq!(person_row.type_name.as_deref(), Some("person"));
    assert_eq!(string_values(&person_row, "identifier"), ["person-1"]);
    assert_eq!(string_values(&person_row, "nickname"), ["alice"]);
    assert_eq!(string_values(&person_row, "aliases"), ["alpha", "beta"]);

    let person_hrow = HydratedRow::new(
        Person::TYPE_ID_JSON,
        person_iid.clone(),
        vec![
            (
                PersonType::aliases.owns_id_json(),
                vec![
                    EncodedScalar::String("alpha".to_owned()),
                    EncodedScalar::String("beta".to_owned()),
                ],
            ),
            (
                PersonType::identifier.owns_id_json(),
                vec![EncodedScalar::String("person-1".to_owned())],
            ),
            (
                PersonType::nickname.owns_id_json(),
                vec![EncodedScalar::String("alice".to_owned())],
            ),
            (
                PersonType::score.owns_id_json(),
                vec![EncodedScalar::Long(42)],
            ),
            (
                PersonType::val_bool.owns_id_json(),
                vec![EncodedScalar::Boolean(true)],
            ),
            (
                PersonType::val_constrained.owns_id_json(),
                vec![EncodedScalar::Long(50)],
            ),
            (
                PersonType::val_date.owns_id_json(),
                vec![EncodedScalar::Date(Date::try_new("2026-07-28").unwrap())],
            ),
            (
                PersonType::val_datetime.owns_id_json(),
                vec![EncodedScalar::DateTime(
                    DateTime::try_new("2026-07-28T03:55:00").unwrap(),
                )],
            ),
            (
                PersonType::val_datetime_tz.owns_id_json(),
                vec![EncodedScalar::DateTimeTz(
                    DateTimeTz::try_new("2026-07-28T03:55:00Z").unwrap(),
                )],
            ),
            (
                PersonType::val_decimal.owns_id_json(),
                vec![EncodedScalar::Decimal(Decimal::try_new("123.45").unwrap())],
            ),
            (
                PersonType::val_double.owns_id_json(),
                vec![EncodedScalar::Double(
                    CanonicalDouble::try_new(3.14).unwrap(),
                )],
            ),
            (
                PersonType::val_duration.owns_id_json(),
                vec![EncodedScalar::Duration(Duration::try_new("P1D").unwrap())],
            ),
        ],
        vec![],
    );
    let person: Person = materialize_model_for_test(&person_hrow)
        .expect("provider-validated person has the generated complete shape");
    let person_ref = person.reference();
    assert_eq!(person_ref.iid(), Some(person_iid.as_str()));

    let person_player_evidence = HydratedPlayer::new(
        Person::TYPE_ID_JSON,
        Some(person_iid.clone()),
        vec![(
            PersonType::identifier.owns_id_json(),
            EncodedScalar::String("person-1".to_owned()),
        )],
    );

    let no_attributes: DynamicAttributeMap = Vec::new();
    let person_player = |role_name: &str| DynamicRolePlayerInput {
        role_name: role_name.to_owned(),
        player_type_name: "person".to_owned(),
        iid: Some(person_iid.clone()),
        key: None,
    };

    let membership_create = MembershipCreate::new(MembershipMemberRef::Person(person_ref.clone()))
        .expect("membership create input is valid");
    assert!(matches!(
        membership_create.member(),
        MembershipMemberRef::Person(_)
    ));
    let membership_iid = membership_manager
        .insert(&no_attributes, &[person_player("member")])
        .await
        .expect("normal relation inserts");
    let membership_rows = membership_manager
        .get_by_iid_exact(&membership_iid)
        .await
        .expect("exact membership IID read succeeds");
    assert_eq!(membership_rows.len(), 1);
    let membership_player = only_role(&membership_rows[0], "member");
    assert_eq!(
        membership_player.player_iid.as_deref(),
        Some(person_iid.as_str())
    );
    let mem_hrow = HydratedRow::new(
        Membership::TYPE_ID_JSON,
        membership_iid.clone(),
        vec![],
        vec![(
            MembershipType::member.role_id_json(),
            vec![person_player_evidence.clone()],
        )],
    );
    let membership: Membership =
        materialize_model_for_test(&mem_hrow).expect("membership has the generated complete shape");
    assert_eq!(membership.iid(), membership_iid.as_str());

    let employment_create =
        EmploymentCreate::new(person_ref.clone()).expect("employment create input is valid");
    assert_eq!(
        employment_create
            .employee()
            .identifier()
            .expect("identifier present")
            .value(),
        "person-1"
    );
    let employment_iid = employment_manager
        .insert(&no_attributes, &[person_player("employee")])
        .await
        .expect("specialized relation inserts");
    let employment_rows = employment_manager
        .get_by_iid_exact(&employment_iid)
        .await
        .expect("exact employment IID read succeeds");
    assert_eq!(employment_rows.len(), 1);
    let employment_player = only_role(&employment_rows[0], "employee");
    assert_eq!(
        employment_player.player_iid.as_deref(),
        Some(person_iid.as_str())
    );
    assert!(
        membership_manager
            .get_by_iid_exact(&employment_iid)
            .await
            .expect("exact base lookup succeeds")
            .is_empty(),
        "an exact Membership read must not hydrate an Employment subtype"
    );
    let emp_hrow = HydratedRow::new(
        Employment::TYPE_ID_JSON,
        employment_iid.clone(),
        vec![],
        vec![(
            EmploymentType::employee.role_id_json(),
            vec![person_player_evidence.clone()],
        )],
    );
    let employment: Employment = materialize_model_for_test(&emp_hrow)
        .expect("employment has the generated specialized read shape");
    assert_eq!(employment.iid(), employment_iid.as_str());

    let _event_create = EventCreate::new(person_ref.clone()).expect("event create input is valid");
    let event_iid = event_manager
        .insert(&no_attributes, &[person_player("subject")])
        .await
        .expect("event relation inserts");
    let event_rows = event_manager
        .get_by_iid_exact(&event_iid)
        .await
        .expect("exact event IID read succeeds");
    assert_eq!(event_rows.len(), 1);
    let event_player = only_role(&event_rows[0], "subject");
    assert_eq!(
        event_player.player_iid.as_deref(),
        Some(person_iid.as_str())
    );
    let event_hrow = HydratedRow::new(
        Event::TYPE_ID_JSON,
        event_iid.clone(),
        vec![],
        vec![(
            EventType::subject.role_id_json(),
            vec![person_player_evidence.clone()],
        )],
    );
    let event: Event =
        materialize_model_for_test(&event_hrow).expect("event has the generated complete shape");
    let EventSubjectPlayer::Person(p) = event.subject();
    assert_eq!(
        p.identifier().expect("identifier present").value(),
        "person-1"
    );

    let event_ref = EventRef::from_iid(event_iid.clone()).expect("event reference is valid");
    let container_create =
        ContainerCreate::new(vec![event_ref.clone()]).expect("container reference input is valid");
    let input_event_ref = &container_create.item()[0];
    let container_iid = container_manager
        .insert(
            &no_attributes,
            &[DynamicRolePlayerInput {
                role_name: "item".to_owned(),
                player_type_name: "event".to_owned(),
                iid: input_event_ref.iid().map(|s| s.to_owned()),
                key: None,
            }],
        )
        .await
        .expect("relation-as-player container inserts");
    let container_rows = container_manager
        .get_by_iid_exact(&container_iid)
        .await
        .expect("exact container IID read succeeds");
    assert_eq!(container_rows.len(), 1);
    let container_player = only_role(&container_rows[0], "item");
    assert_eq!(container_player.player_type_name.as_deref(), Some("event"));
    assert_eq!(
        container_player.player_iid.as_deref(),
        Some(event_iid.as_str())
    );
    let event_player_evidence =
        HydratedPlayer::new(Event::TYPE_ID_JSON, Some(event_iid.clone()), vec![]);
    let container_hrow = HydratedRow::new(
        Container::TYPE_ID_JSON,
        container_iid.clone(),
        vec![],
        vec![(
            ContainerType::item.role_id_json(),
            vec![event_player_evidence],
        )],
    );
    let container: Container = materialize_model_for_test(&container_hrow)
        .expect("container has the generated reference-read shape");
    assert_eq!(container.item().len(), 1);
    let ContainerItemPlayer::Event(ref item) = container.item()[0];
    assert_eq!(item.iid(), Some(event_iid.as_str()));
    println!("F2B-03 internal dynamic regression: passed");
}
