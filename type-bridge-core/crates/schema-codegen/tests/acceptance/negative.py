from generated_v2 import (
    Container,
    Employment,
    Event,
    EventRef,
    Identifier,
    Membership,
    Person,
    PersonRef,
    RoleToken,
)


def accepts_employment_role(role: RoleToken[Employment, Person]) -> None:
    del role


Event()  # E: missing_event_subject:reportCallIssue
Employment()  # E: missing_required:reportCallIssue
Employment(
    employee=PersonRef(  # E: reference_as_complete:reportArgumentType
        "person-iid", identifier=Identifier("person-1")
    )
)
Employment(
    employee=Person(identifier=Identifier("person-1")),
    member=Person(identifier=Identifier("person-2")),  # E: specialized_keyword:reportCallIssue
)
accepts_employment_role(Membership.member)  # E: wrong_owner:reportArgumentType
Container(item=EventRef("event-iid"))  # E: scalar_for_sequence:reportArgumentType
Employment(
    employee=[  # E: sequence_for_scalar:reportArgumentType
        Person(identifier=Identifier("person-1"))
    ]
)
Person(identifier=7)  # E: wrong_scalar:reportArgumentType
