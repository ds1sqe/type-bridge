from collections.abc import Iterator
from typing import assert_type

from generated_v2 import (
    Aliases,
    Container,
    Employment,
    Event,
    EventRef,
    FieldToken,
    FunctionRef,
    Identifier,
    Membership,
    Nickname,
    Person,
    PersonRef,
    PlayerStats,
    Robot,
    RoleToken,
    find_events,
)

assert_type(Employment.employee, RoleToken[Employment, Person])
assert_type(Container.item, RoleToken[Container, Event])
assert_type(Event.subject, RoleToken[Event, Person])
assert_type(Membership.member, RoleToken[Membership, Person | Robot])
assert_type(Person.identifier, FieldToken[Person])
assert_type(Person.nickname, FieldToken[Person])
assert_type(Person.aliases, FieldToken[Person])

person = Person(
    identifier=Identifier("person-1"),
    nickname=Nickname("alice"),
    aliases=[Aliases("a"), Aliases("b")],
)
assert_type(person.identifier, Identifier)
assert_type(person.nickname, Nickname | None)
assert_type(person.aliases, tuple[Aliases, ...])

event = Event(subject=person)
assert_type(event.subject, Person)

employment = Employment(employee=person)
assert_type(employment.employee, Person)

container = Container(item=[EventRef("event-iid")])
assert_type(container.item, tuple[EventRef, ...])

person_reference = PersonRef("person-iid", identifier=Identifier("person-1"))
assert_type(person_reference, PersonRef)

stats = PlayerStats(wins=3, nickname=None)
assert_type(stats, PlayerStats)

assert_type(
    find_events,
    FunctionRef[[Event], Iterator[Event]],
)
