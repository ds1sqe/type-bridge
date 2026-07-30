use generated::*;
use type_bridge::__codegen::{
    CanonicalDouble, Date, DateTime, DateTimeTz, Decimal, Duration, HydratedPlayer,
    HydratedRow, IntoEncodedScalar, materialize_model_for_test,
};

#[derive(type_bridge::SelectedRow)]
struct PersonEventRow {
    person: Person,
    event: Event,
}

#[derive(type_bridge::SelectedRow)]
struct PersonGraph {
    person: Person,
    events: Vec<Event>,
}

fn selected_shapes_compile(
    session: &type_bridge::QuerySession<'_, AppSchema>,
    person: type_bridge::Binding<AppSchema, Person>,
    event: type_bridge::Binding<AppSchema, Event>,
) -> type_bridge::Result<()> {
    let tuple = session.query((person, event))?;
    let _ = tuple.count_by(person);
    let named = PersonEventRow::select(person, event)?;
    let named_query = session.query(named)?;
    let _ = named_query.exists_by(event);
    let graph = PersonGraph::select(person, event.collect().distinct())?;
    let graph_query = session.query(graph)?;
    let _ = graph_query.page_by(person, type_bridge::PageOptions::new(10));
    Ok(())
}

fn bounded_reachability_compiles(
    session: &type_bridge::QuerySession<'_, AppSchema>,
    source: type_bridge::Binding<AppSchema, Person>,
    target: type_bridge::Binding<AppSchema, Person>,
) -> type_bridge::Result<()> {
    let reachable = session.reachable(
        NetworkLinkType::TOKEN,
        NetworkLinkType::origin,
        NetworkLinkType::destination,
        source,
        target,
        1,
        2,
    )?;
    let _ = session.query((source, target))?.where_(reachable)?;
    Ok(())
}

async fn reusable_read_context_compiles(
    database: &type_bridge::Database<AppSchema>,
) -> type_bridge::Result<()> {
    let read = database.read().await?;
    let mut session = read.query();
    let person = session.exact::<Person>()?;
    let query = session.query(person)?;
    let _ = query.count().await?;
    let _ = query.exists().await?;
    let _ = query.count().await?;
    drop(query);
    drop(session);
    read.close().await
}

struct RemoteTransport;

impl type_bridge::RemoteQueryTransport for RemoteTransport {
    fn capabilities(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = type_bridge::Result<Vec<u8>>>
                + Send
                + '_
        >
    > {
        Box::pin(async {
            Err(type_bridge::Error::Other {
                message: "compile-only transport".into(),
                source: None,
            })
        })
    }

    fn exchange<'a>(
        &'a self,
        _request: &'a [u8],
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = type_bridge::Result<Vec<u8>>>
                + Send
                + 'a
        >
    > {
        Box::pin(async {
            Err(type_bridge::Error::Other {
                message: "compile-only transport".into(),
                source: None,
            })
        })
    }
}

async fn remote_generated_outputs_compile() -> type_bridge::Result<()> {
    let options = type_bridge::RemoteConnectionOptions::new(
        "application-scope",
        "typedb-3.12.1/v1",
        type_bridge::RemoteQueryLimits::new(100, 1 << 20, 100, 1000, 1000, 1000),
        RemoteTransport,
    );
    let remote: type_bridge::RemoteDatabase<AppSchema> =
        type_bridge::RemoteDatabase::connect(options)
            .await?
            .with_schema(SCHEMA)?;
    let mut session = remote.query()?;
    let person = session.exact::<Person>()?;
    let event = session.exact::<Event>()?;
    let tuple = session.query((person, event))?;
    let _rows: Vec<(Person, Event)> =
        tuple.rows(type_bridge::RowsOptions::new(10)).await?;
    let graph = PersonGraph::select(person, event.collect())?;
    let page: type_bridge::Page<PersonGraph> = session
        .query(graph)?
        .page_by(person, type_bridge::PageOptions::new(10))
        .await?;
    let _ = page.items();
    Ok(())
}

fn assert_nominal_upcast<T: NominalUpcast<Membership>>() {}
fn assert_role_upcast<T: RoleUpcast<EmploymentEmployeePlayer, MembershipMemberPlayer>>() {}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    assert_nominal_upcast::<Employment>();
    assert_role_upcast::<Employment>();

    // Test signed zeroes by bits
    let neg_zero = CanonicalDouble::try_new(-0.0)?;
    let pos_zero = CanonicalDouble::try_new(0.0)?;
    assert_ne!(neg_zero.to_bits(), pos_zero.to_bits());

    let identifier = Identifier::new("person-1")?;
    let nickname = Nickname::new("Ada")?;
    let alias = Aliases::new("engineer")?;
    let score = Score::new(42i64)?;
    let v_double = ValDouble::new(CanonicalDouble::try_new(3.14)?)?;
    let v_decimal = ValDecimal::new(Decimal::try_new("123.45")?)?;
    let v_bool = ValBool::new(true)?;
    let v_date = ValDate::new(Date::try_new("2026-07-28")?)?;
    let v_datetime = ValDatetime::new(DateTime::try_new("2026-07-28T03:55:00")?)?;
    let v_datetimetz = ValDatetimeTz::new(DateTimeTz::try_new("2026-07-28T03:55:00Z")?)?;
    let v_duration = ValDuration::new(Duration::try_new("P1D")?)?;
    let v_constrained = ValConstrained::new(50i64)?;

    let person_create = PersonCreate::new(
        vec![alias.clone(), alias.clone()],
        identifier.clone(),
        Some(nickname),
        score.clone(),
        v_bool.clone(),
        v_constrained.clone(),
        v_date.clone(),
        v_datetime.clone(),
        v_datetimetz.clone(),
        v_decimal.clone(),
        v_double.clone(),
        v_duration.clone(),
    )?;
    assert_eq!(person_create.aliases().len(), 2);

    let id_scalar = identifier.value().into_encoded_scalar();
    let score_scalar = score.value().into_encoded_scalar();
    let double_scalar = v_double.value().into_encoded_scalar();
    let decimal_scalar = v_decimal.value().into_encoded_scalar();
    let bool_scalar = v_bool.value().into_encoded_scalar();
    let date_scalar = v_date.value().into_encoded_scalar();
    let datetime_scalar = v_datetime.value().into_encoded_scalar();
    let datetimetz_scalar = v_datetimetz.value().into_encoded_scalar();
    let duration_scalar = v_duration.value().into_encoded_scalar();
    let constrained_scalar = v_constrained.value().into_encoded_scalar();

    let person_row = HydratedRow::new(
        Person::TYPE_ID_JSON,
        "person-iid-1".to_owned(),
        vec![
            (PersonType::identifier.owns_id_json(), vec![id_scalar.clone()]),
            (PersonType::score.owns_id_json(), vec![score_scalar]),
            (PersonType::val_double.owns_id_json(), vec![double_scalar]),
            (PersonType::val_decimal.owns_id_json(), vec![decimal_scalar]),
            (PersonType::val_bool.owns_id_json(), vec![bool_scalar]),
            (PersonType::val_date.owns_id_json(), vec![date_scalar]),
            (PersonType::val_datetime.owns_id_json(), vec![datetime_scalar]),
            (PersonType::val_datetime_tz.owns_id_json(), vec![datetimetz_scalar]),
            (PersonType::val_duration.owns_id_json(), vec![duration_scalar]),
            (PersonType::val_constrained.owns_id_json(), vec![constrained_scalar]),
        ],
        vec![],
    );
    let person: Person = materialize_model_for_test(&person_row)?;
    assert_eq!(person.iid(), "person-iid-1");

    let person_ref = person.reference();
    assert_eq!(person_ref.iid(), Some("person-iid-1"));

    let key_ref = PersonRef::from_key(identifier)?;
    assert_eq!(key_ref.iid(), None);
    assert_eq!(key_ref.identifier().unwrap().value(), "person-1");

    let person_player_evidence = HydratedPlayer::new(
        Person::TYPE_ID_JSON,
        Some("person-iid-1".to_owned()),
        vec![(PersonType::identifier.owns_id_json(), id_scalar)],
    );

    let event_row = HydratedRow::new(
        Event::TYPE_ID_JSON,
        "event-iid-1".to_owned(),
        vec![],
        vec![(EventType::subject.role_id_json(), vec![person_player_evidence.clone()])],
    );
    let event: Event = materialize_model_for_test(&event_row)?;
    assert_eq!(event.iid(), "event-iid-1");

    let event_reference = EventRef::from_iid("event-iid")?;
    let container = ContainerCreate::new(vec![event_reference])?;
    assert_eq!(container.item().len(), 1);

    let employment = EmploymentCreate::new(person_ref)?;
    assert_eq!(employment.employee().identifier().unwrap().value(), "person-1");

    let emp_row = HydratedRow::new(
        Employment::TYPE_ID_JSON,
        "emp-iid-1".to_owned(),
        vec![],
        vec![(EmploymentType::employee.role_id_json(), vec![person_player_evidence])],
    );
    let emp_read: Employment = materialize_model_for_test(&emp_row)?;
    let family = MembershipFamily::Employment(emp_read);
    assert_eq!(family.iid(), "emp-iid-1");
    assert!(family.as_employment().is_some());

    let role: RoleToken<Container, ContainerItemPlayer> = ContainerType::item;
    assert!(role.role_id_json().contains("item"));
    let _: PlaysToken<Event, Container, ContainerItemPlayer> = plays_event_container_item;
    let _: FunctionToken<AppSchema, (Event,), Stream<Event>> = find_events;

    let stats = PlayerStats::try_new(Some("stable".to_owned()), 3);
    assert_eq!(*stats.wins(), 3);
    assert_eq!(PLAYING_FACTS.len(), 8);
    assert!(RUNTIME_PROJECTION_JSON.contains("validated-create-input"));
    assert!(MODEL_LINK_COMPONENTS.iter().any(|component| component.len() > 1));
    Ok(())
}
