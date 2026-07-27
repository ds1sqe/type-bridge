use generated::*;

fn assert_nominal_upcast<T: NominalUpcast<Membership>>() {}
fn assert_role_upcast<T: RoleUpcast<EmploymentEmployeePlayer, MembershipMemberPlayer>>() {}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    assert_nominal_upcast::<Employment>();
    assert_role_upcast::<Employment>();

    let identifier = Identifier::from_parts("person-1".to_owned())?;
    let nickname = Nickname::from_parts("Ada".to_owned())?;
    let alias = Aliases::from_parts("engineer".to_owned())?;
    let person_create = PersonCreate::try_new(
        vec![alias.clone(), alias.clone()],
        identifier.clone(),
        Some(nickname),
    )?;
    assert_eq!(person_create.aliases().len(), 2);

    let too_many = PersonCreate::try_new(
        vec![alias.clone(), alias.clone(), alias.clone(), alias],
        identifier.clone(),
        None,
    ).unwrap_err();
    assert_eq!(too_many.code(), "cardinality_violation");

    let person = Person::from_parts(Vec::new(), identifier, None)?;
    let event = Event::from_parts(person.clone())?;
    let event_reference = EventRef::try_new("event-iid")?;
    let container = ContainerCreate::try_new(vec![Either::Right(event_reference)])?;
    assert_eq!(container.item().len(), 1);
    let employment = EmploymentCreate::try_new(person)?;
    assert_eq!(employment.employee().identifier().value(), "person-1");

    let role: RoleToken<Container, ContainerItemPlayer> = ContainerType::item;
    assert!(role.role_id_json().contains("item"));
    let _: PlaysToken<Event, Container, ContainerItemPlayer> = plays_event_container_item;
    let _: FunctionToken<(Event,), Stream<Event>> = find_events;

    let stats = PlayerStats::try_new(3, Some("stable".to_owned()));
    assert_eq!(*stats.wins(), 3);
    assert_eq!(PLAYING_FACTS.len(), 5);
    assert!(RUNTIME_PROJECTION_JSON.contains("validated-create-input"));
    assert!(MODEL_LINK_COMPONENTS.iter().any(|component| component.len() > 1));
    Ok(())
}
