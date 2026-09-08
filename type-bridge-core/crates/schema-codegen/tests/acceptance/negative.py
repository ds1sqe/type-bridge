from datetime import UTC, date, datetime, timedelta
from decimal import Decimal

from generated_v2 import (
    Actor,
    BoundVar,
    Container,
    Employment,
    Event,
    EventRef,
    FooBar,
    Identifier,
    Membership,
    Person,
    PersonRef,
    Robot,
    RoleToken,
    Score,
    SubtypeBoundVar,
    ValBool,
    ValConstrained,
    ValDate,
    ValDatetime,
    ValDatetimeTz,
    ValDecimal,
    ValDouble,
    ValDuration,
    aggregate,
)

from type_bridge.session import Database


def person(identifier: Identifier) -> Person:
    return Person(
        identifier=identifier,
        score=Score(3),
        foo__bar=FooBar(7),
        val_bool=ValBool(True),
        val_constrained=ValConstrained(20),
        val_date=ValDate(date(2026, 7, 29)),
        val_datetime=ValDatetime(datetime(2026, 7, 29)),
        val_datetime_tz=ValDatetimeTz(datetime(2026, 7, 29, tzinfo=UTC)),
        val_decimal=ValDecimal(Decimal("3.5")),
        val_double=ValDouble(3.5),
        val_duration=ValDuration(timedelta(seconds=3)),
    )


type PersonRoleBinding = BoundVar[Person] | SubtypeBoundVar[Actor] | SubtypeBoundVar[Person]


def accepts_employment_role(
    role: RoleToken[Employment, Person, PersonRoleBinding],
) -> None:
    del role


Event()  # E: missing_event_subject:reportCallIssue
Employment()  # E: missing_required:reportCallIssue
Employment(
    employee=PersonRef(  # E: reference_as_complete:reportArgumentType
        "person-iid", identifier=Identifier("person-1")
    )
)
Employment(
    employee=person(Identifier("person-1")),
    member=person(Identifier("person-2")),  # E: specialized_keyword:reportCallIssue
)
accepts_employment_role(Membership.member)  # E: wrong_owner:reportArgumentType
Container(item=EventRef("event-iid"))  # E: scalar_for_sequence:reportArgumentType
Employment(
    employee=[  # E: sequence_for_scalar:reportArgumentType
        person(Identifier("person-1"))
    ]
)
person(7)  # E: wrong_scalar:reportArgumentType

query_session = Person.query(Database(address="localhost:1729", database="generated-query"))
person_var = query_session.exact(Person)
employment_var = query_session.exact(Employment)
exact_actor_var = query_session.exact(Actor)
exact_robot_var = query_session.exact(Robot)
subtype_robot_var = query_session.subtypes(Robot)
person_var.field(Employment.employee)  # E: wrong_query_owner:reportArgumentType
person_var.field(Person.score).gte(Identifier("wrong"))  # E: wrong_query_value:reportArgumentType
employment_var.role(Employment.employee).connects(
    query_session.exact(Event)  # E: wrong_query_player:reportArgumentType
)
query_session.exact(Event).role(Event.subject).connects(
    exact_actor_var  # E: exact_abstract_query_player:reportArgumentType
)
query_session.exact(Event).role(Event.subject).connects(
    exact_robot_var  # E: unrelated_exact_query_player:reportArgumentType
)
query_session.exact(Event).role(Event.subject).connects(
    subtype_robot_var  # E: unrelated_subtype_root:reportArgumentType
)
query_session.query(  # E: too_many_query_slots:reportCallIssue
    person_var,
    person_var,
    person_var,
    person_var,
    person_var,
    person_var,
    person_var,
    person_var,
    person_var,
    person_var,
    person_var,
    person_var,
    person_var,
    person_var,
    person_var,
    person_var,
    person_var,
)
aggregate.mean(person_var.field(Person.identifier))  # E: non_numeric_aggregate:reportArgumentType
query_session.query(person_var).aggregate(  # E: empty_aggregate:reportCallIssue
    person_var,
)
query_session.query(person_var).aggregate(  # E: too_many_aggregate_terms:reportCallIssue
    person_var,
    aggregate.count(),
    aggregate.count(),
    aggregate.count(),
    aggregate.count(),
    aggregate.count(),
    aggregate.count(),
    aggregate.count(),
    aggregate.count(),
    aggregate.count(),
    aggregate.count(),
    aggregate.count(),
    aggregate.count(),
    aggregate.count(),
    aggregate.count(),
    aggregate.count(),
    aggregate.count(),
    aggregate.count(),
)
