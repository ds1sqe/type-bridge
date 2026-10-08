from dataclasses import dataclass
from typing import assert_type

from generated_v2 import (
    Container,
    Employment,
    Event,
    Identifier,
    Page,
    Person,
    Query,
    Score,
)

from type_bridge import Database

db = Database(address="localhost:1729", database="typed_query_example")
db.connect()
session = Person.query(db)
person = session.exact(Person)
event = session.exact(Event)
container = session.exact(Container)
employment = session.exact(Employment)

employee = employment.role(Employment.employee).connects(person)
subject = event.role(Event.subject).connects(person)
contained = container.role(Container.item).connects(event)

one_person: Query[Person] = session.query(person)
two_slots: Query[Person, Event] = session.query(person, event).where(subject)

colleague = session.exact(Person)
other_event = session.exact(Event)
five_slots: Query[Person, Event, Container, Event, Person] = session.query(
    person,
    event,
    container,
    other_event,
    colleague,
).where(
    subject,
    contained,
    other_event.role(Event.subject).connects(colleague),
    container.role(Container.item).connects(other_event),
)

person_pair: Query[Person, Person] = (
    session.query(person, colleague)
    .match(event, other_event, container)
    .where(
        subject,
        contained,
        other_event.role(Event.subject).connects(colleague),
        container.role(Container.item).connects(other_event),
    )
)


# Terminals are coroutines on every connection; composition is synchronous.
async def selection_shapes() -> None:
    assert_type(await one_person.one(), Person)
    assert_type(await one_person.rows(limit=20), list[Person])
    assert_type(await two_slots.one(), tuple[Person, Event])
    assert_type(await two_slots.rows(limit=20), list[tuple[Person, Event]])
    assert_type(
        await five_slots.rows(limit=20),
        list[tuple[Person, Event, Container, Event, Person]],
    )
    assert_type(await person_pair.rows(limit=20), list[tuple[Person, Person]])


# Generated QuerySession.query has checked overloads through 16 selections. A
# seventeenth selection is a static diagnostic and Rust rejects forged input.

adults = one_person.where(
    person.field(Person.score).gte(Score(18)),
    person.field(Person.identifier).starts_with(Identifier("Al")),
    person.field(Person.identifier).contains(Identifier("Research")),
    person.field(Person.identifier).ends_with(Identifier("Labs")),
    person.field(Person.identifier).regex(Identifier(r"^A[[:alpha:]]+$")),
)

# Percent is literal input here; it has no SQL wildcard meaning.
literal_percent = one_person.where(person.field(Person.identifier).contains(Identifier("50%")))

# Cross joins are explicit topology permission, never a boolean predicate.
independent_pairs: Query[Person, Container] = session.query(person, container).allow_cross_join(
    person, container
)


@dataclass(frozen=True, slots=True)
class PersonEvents:
    person: Person
    events: tuple[Event, ...]


work: Query[PersonEvents] = session.query_as(
    PersonEvents,
    person=person,
    events=event.collect().distinct(),
).where(subject)


async def terminals() -> None:
    ordered_people = await one_person.rows(
        limit=50,
        offset=0,
        order_by=(person.field(Person.identifier).asc(),),
    )
    assert_type(ordered_people, list[Person])

    person_count: int = await two_slots.count_by(person)
    any_person: bool = await two_slots.exists_by(person)
    assert_type((person_count, any_person), tuple[int, bool])

    page: Page[PersonEvents] = await work.page_by(
        person,
        limit=50,
        offset=0,
        order_by=(person.field(Person.identifier).asc(),),
        include_total=True,
    )
    assert_type(page.items, tuple[PersonEvents, ...])
    assert_type(page.total, int | None)


async def ownership() -> None:
    # Owned: rows() opens one read transaction and closes it on success or error.
    owned = Person.query(db)
    owned_person = owned.exact(Person)
    assert_type(await owned.query(owned_person).rows(limit=10), list[Person])

    # Borrowed: the surrounding context owns lifecycle and may run more work.
    with db.transaction("read") as tx:
        borrowed = Person.query(tx)
        borrowed_person = borrowed.exact(Person)
        first_page = await borrowed.query(borrowed_person).rows(limit=10)
        second_page = await borrowed.query(borrowed_person).rows(limit=10, offset=10)
        assert_type(first_page + second_page, list[Person])
