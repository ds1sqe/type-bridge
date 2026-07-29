from collections.abc import Iterator
from datetime import UTC, date, datetime, timedelta
from decimal import Decimal
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
    Score,
    ValBool,
    ValConstrained,
    ValDate,
    ValDatetime,
    ValDatetimeTz,
    ValDecimal,
    ValDouble,
    ValDuration,
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
    score=Score(3),
    val_bool=ValBool(True),
    val_constrained=ValConstrained(20),
    val_date=ValDate(date(2026, 7, 29)),
    val_datetime=ValDatetime(datetime(2026, 7, 29)),
    val_datetime_tz=ValDatetimeTz(datetime(2026, 7, 29, tzinfo=UTC)),
    val_decimal=ValDecimal(Decimal("3.5")),
    val_double=ValDouble(3.5),
    val_duration=ValDuration(timedelta(seconds=3)),
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
