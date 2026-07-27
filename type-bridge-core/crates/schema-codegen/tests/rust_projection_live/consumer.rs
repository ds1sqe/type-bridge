use std::env;
use std::sync::Arc;

use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::projection::BindingTarget;
use type_bridge_contract::projection_wire::decode_runtime_projection_verified;
use type_bridge_generated_schema::{
    Aliases, Container, ContainerCreate, ContainerType, Either, Employment, EmploymentCreate,
    EmploymentType, Event, EventCreate, EventRef, EventType, Identifier, Membership,
    MembershipCreate, MembershipType, Nickname, Person, PersonCreate, PersonRef, PersonType,
    PROJECTION_FINGERPRINT_JSON, RUNTIME_PROJECTION_JSON, SEMANTIC_SCHEMA_FINGERPRINT_JSON,
    plays_event_container_item,
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
    let installed =
        InstalledRuntimeProjection::try_new(runtime).expect("Rust projection installs");

    // Generated tokens are canonical identity evidence only. They deliberately do
    // not execute a query here; Query V2 and the typed Rust ORM companion/facade
    // that owns reusable input lowering and hydration belong to plan04.
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
    ensure_database_exists(
        &address,
        &database,
        &username,
        &password,
        connect_options(),
    )
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

    let person_create = PersonCreate::try_new(
        vec![
            Aliases::from_parts("alpha".to_owned()).expect("alias is valid"),
            Aliases::from_parts("beta".to_owned()).expect("alias is valid"),
        ],
        Identifier::from_parts("person-1".to_owned()).expect("identifier is valid"),
        Some(Nickname::from_parts("alice".to_owned()).expect("nickname is valid")),
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
    let person = Person::from_parts(
        person_create.aliases().to_vec(),
        person_create.identifier().clone(),
        person_create.nickname().cloned(),
    )
    .expect("provider-validated person has the generated complete shape");
    let person_ref = PersonRef::try_new(person_iid.clone(), person.identifier().clone())
        .expect("person reference is valid");
    assert_eq!(person_ref.iid(), person_iid);

    let no_attributes: DynamicAttributeMap = Vec::new();
    let person_player = |role_name: &str| DynamicRolePlayerInput {
        role_name: role_name.to_owned(),
        player_type_name: "person".to_owned(),
        iid: Some(person_iid.clone()),
        key: None,
    };

    let membership_create = MembershipCreate::try_new(Either::Left(person.clone()))
        .expect("membership create input is valid");
    assert!(matches!(membership_create.member(), Either::Left(_)));
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
    assert_eq!(membership_player.player_iid.as_deref(), Some(person_iid.as_str()));
    let membership = Membership::from_parts(membership_create.member().clone())
        .expect("membership has the generated complete shape");
    assert!(matches!(membership.member(), Either::Left(_)));

    let employment_create =
        EmploymentCreate::try_new(person.clone()).expect("employment create input is valid");
    assert_eq!(
        employment_create.employee().identifier().value(),
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
    assert_eq!(employment_player.player_iid.as_deref(), Some(person_iid.as_str()));
    assert!(
        membership_manager
            .get_by_iid_exact(&employment_iid)
            .await
            .expect("exact base lookup succeeds")
            .is_empty(),
        "an exact Membership read must not hydrate an Employment subtype"
    );
    let employment = Employment::from_parts(employment_create.employee().clone())
        .expect("employment has the generated specialized read shape");
    assert_eq!(employment.employee().identifier().value(), "person-1");

    let event_create = EventCreate::try_new(person.clone()).expect("event create input is valid");
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
    assert_eq!(event_player.player_iid.as_deref(), Some(person_iid.as_str()));
    let event = Event::from_parts(event_create.subject().clone())
        .expect("event has the generated complete shape");
    assert_eq!(event.subject().identifier().value(), "person-1");

    let event_ref = EventRef::try_new(event_iid.clone()).expect("event reference is valid");
    let container_create = ContainerCreate::try_new(vec![Either::Right(event_ref.clone())])
        .expect("container reference input is valid");
    let input_event_ref = match &container_create.item()[0] {
        Either::Right(reference) => reference,
        Either::Left(_) => panic!("container fixture must use the shallow reference input"),
    };
    let container_iid = container_manager
        .insert(
            &no_attributes,
            &[DynamicRolePlayerInput {
                role_name: "item".to_owned(),
                player_type_name: "event".to_owned(),
                iid: Some(input_event_ref.iid().to_owned()),
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
    assert_eq!(container_player.player_iid.as_deref(), Some(event_iid.as_str()));
    let shallow_event = EventRef::try_new(
        container_player
            .player_iid
            .clone()
            .expect("container item includes its relation IID"),
    )
    .expect("provider relation reference is valid");
    let container = Container::from_parts(vec![shallow_event])
        .expect("container has the generated reference-read shape");
    assert_eq!(container.item().len(), 1);
    assert_eq!(container.item()[0].iid(), event_iid);
}
