use generated::*;

fn requires_event_reference(_: EventRef) {}

fn complete_is_not_reference(value: Event) {
    requires_event_reference(value);
}

fn role_owner_is_nominal() {
    let _: RoleToken<Membership, MembershipMemberPlayer> = EventType::subject;
}

fn required_create_inputs_are_static() {
    let _ = EventCreate::try_new();
}

fn main() {}
