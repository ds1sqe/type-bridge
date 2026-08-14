use generated::*;

#[derive(type_bridge::SelectedRow)]
struct WrongSelectedOutput {
    event: Event,
}

#[derive(type_bridge::SelectedRow)]
struct WrongCollectedOutput {
    events: Vec<Person>,
}

#[derive(type_bridge::SelectedRow)]
struct TooManySelectedOutputs {
    a: Person,
    b: Person,
    c: Person,
    d: Person,
    e: Person,
    f: Person,
    g: Person,
    h: Person,
    i: Person,
    j: Person,
    k: Person,
    l: Person,
    m: Person,
    n: Person,
    o: Person,
    p: Person,
    q: Person,
}

fn selected_output_type_is_static(
    person: type_bridge::Binding<AppSchema, Person>,
    event: type_bridge::Binding<AppSchema, Event>,
) {
    let _ = WrongSelectedOutput::select(person);
    let _ = WrongCollectedOutput::select(event.collect());
}

fn collections_have_page_terminals_only(
    session: &type_bridge::QuerySession<'_, AppSchema>,
    person: type_bridge::Binding<AppSchema, Person>,
    event: type_bridge::Binding<AppSchema, Event>,
) {
    let query = session.query((person, event.collect())).unwrap();
    let _ = query.rows(type_bridge::RowsOptions::new(10));
}

async fn active_read_borrow_prevents_close(read: type_bridge::ReadTransaction<'_, AppSchema>) {
    let mut session = read.query();
    let person = session.exact::<Person>().unwrap();
    let query = session.query(person).unwrap();
    read.close().await.unwrap();
    let _ = query.count().await;
}

fn manager_filter_owner_and_value_are_static(database: &type_bridge::Database<AppSchema>) {
    let identifier = Identifier::new("data-ada").unwrap();
    let _ = database.entities::<Person>().where_(
        RobotType::robot_id,
        type_bridge::ProjectedManagerComparison::Eq,
        &RobotId::new(7).unwrap(),
    );
    let _ = database.entities::<Person>().where_(
        PersonType::score,
        type_bridge::ProjectedManagerComparison::Eq,
        &identifier,
    );
}

fn canonical_filter_exposes_no_mutations(database: &type_bridge::Database<AppSchema>) {
    let identifier = Identifier::new("data-ada").unwrap();
    let filter = database
        .entities::<Person>()
        .where_(
            PersonType::identifier,
            type_bridge::ProjectedManagerComparison::Eq,
            &identifier,
        )
        .unwrap();
    let _ = filter.insert();
}

async fn active_manager_filter_borrow_prevents_close(
    read: type_bridge::ReadTransaction<'_, AppSchema>,
) {
    let filter = read.entities::<Person>().filter().unwrap();
    read.close().await.unwrap();
    let _ = filter.count().await;
}

fn requires_event_reference(_: EventRef) {}

fn complete_is_not_reference(value: Event) {
    requires_event_reference(value);
}

fn role_owner_is_nominal() {
    let _: RoleToken<Membership, MembershipMemberPlayer> = EventType::subject;
}

fn reachability_endpoint_roles_are_static(
    session: &type_bridge::QuerySession<'_, AppSchema>,
    person: type_bridge::Binding<AppSchema, Person>,
    event: type_bridge::Binding<AppSchema, Event>,
) {
    let _ = session.reachable(
        NetworkLinkType::TOKEN,
        NetworkLinkType::origin,
        NetworkLinkType::destination,
        person,
        event,
        1,
        2,
    );
}

fn required_create_inputs_are_static() {
    let _ = EventCreate::try_new();
}

fn function_call_domain_is_static(
    session: &type_bridge::QuerySession<'_, AppSchema>,
    person: type_bridge::Binding<AppSchema, Person>,
    wrong_domain: &type_bridge::FunctionCall<AppSchema, bool>,
) {
    let _ = qualifying_score(session, person, wrong_domain);
}

fn main() {}
