from collections.abc import Iterator
from dataclasses import dataclass
from datetime import UTC, date, datetime, timedelta
from decimal import Decimal
from typing import assert_type

from generated_v2 import (
    Actor,
    Aliases,
    BoundVar,
    Container,
    Counter,
    CounterValue,
    CrudEvent,
    CrudHook,
    Employment,
    Event,
    EventRef,
    FieldToken,
    FooBar,
    FunctionRef,
    HookCancelled,
    Identifier,
    Interaction,
    Membership,
    Nickname,
    Party,
    Person,
    PersonRef,
    PlainActivity,
    PlayerStats,
    Predicate,
    ProjectedModelManager,
    ProjectedModelNotFoundError,
    Query,
    QuerySession,
    RemoteQuery,
    RemoteQuerySession,
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
    find_events,
)

from type_bridge.query import Query as RawQuery
from type_bridge.query import QueryBuilder
from type_bridge.session import Database


@dataclass(frozen=True, slots=True)
class EmploymentRow:
    person: Person
    employment: Employment


type PersonRoleBinding = BoundVar[Person] | SubtypeBoundVar[Actor] | SubtypeBoundVar[Person]
type EventRoleBinding = BoundVar[Event] | SubtypeBoundVar[Event]
type MembershipRoleBinding = (
    BoundVar[Person]
    | BoundVar[Robot]
    | SubtypeBoundVar[Actor]
    | SubtypeBoundVar[Person]
    | SubtypeBoundVar[Robot]
)

assert_type(
    Employment.employee,
    RoleToken[Employment, Person, PersonRoleBinding],
)
assert_type(Container.item, RoleToken[Container, Event, EventRoleBinding])
assert_type(Event.subject, RoleToken[Event, Person, PersonRoleBinding])
assert_type(
    Membership.member,
    RoleToken[Membership, Person | Robot, MembershipRoleBinding],
)
assert_type(Person.identifier, FieldToken[Person, Identifier])
assert_type(Person.nickname, FieldToken[Person, Nickname])
assert_type(Person.aliases, FieldToken[Person, Aliases])

person = Person(
    identifier=Identifier("person-1"),
    nickname=Nickname("alice"),
    aliases=[Aliases("a"), Aliases("b")],
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
counter = Counter(counter_value=CounterValue(1))
assert_type(counter.counter_value, CounterValue)
assert_type(person.identifier, Identifier)
assert_type(person.nickname, Nickname | None)
assert_type(person.aliases, tuple[Aliases, ...])
assert_type(QueryBuilder.match_entity(Person, identifier=Identifier("person-1")), RawQuery)
assert_type(QueryBuilder.insert_entity(person), RawQuery)
assert_type(
    QueryBuilder.match_relation(Membership, role_players={"member": "$person"}),
    RawQuery,
)

person_manager = Person.manager(Database(address="localhost:1729", database="generated-manager"))
assert_type(person_manager.put(person), Person)
assert_type(person_manager.insert_many([person]), list[Person])
assert_type(person_manager.put_many([person]), list[Person])
assert_type(person_manager.update(person), Person)
assert_type(person_manager.update_many([person]), list[Person])
assert_type(person_manager.delete(person), Person)
assert_type(person_manager.delete("0x1"), None)
assert_type(person_manager.delete_many([person]), list[Person])
plain_activity = PlainActivity(participant=person)
assert_type(
    PlainActivity.manager(Database(address="localhost:1729", database="generated-manager")).put(
        plain_activity
    ),
    PlainActivity,
)


def generated_owner_lookup_types(database: Database) -> None:
    entity_owners = Identifier.owners(
        database,
        "person-",
        kind="entity",
        lookup="startswith",
    )
    narrowed_owners = Party.has(database, Identifier, lookup="present")
    relation_owners = Identifier.owners(
        database,
        [Identifier("network-1"), Identifier("network-2")],
        kind="relation",
        lookup="in",
    )
    for owner in (*entity_owners, *narrowed_owners, *relation_owners):
        assert_type(owner.iid, str | None)


filtered_person_manager = person_manager.filter(score__gte=Score(3))
assert_type(person_manager.filter(score__in=[Score(3), Score(4)]), ProjectedModelManager[Person])
assert_type(person_manager.filter(aliases__isnull=True), ProjectedModelManager[Person])
assert_type(person_manager.filter(iid__in=["0x1", "0x2"]), ProjectedModelManager[Person])
assert_type(filtered_person_manager.all(), list[Person])
assert_type(filtered_person_manager.first(), Person | None)
assert_type(filtered_person_manager.count(), int)
assert_type(filtered_person_manager.exists(), bool)
assert_type(filtered_person_manager.delete(), int)


def update_person(value: Person) -> None:
    value.nickname = Nickname("updated")


assert_type(filtered_person_manager.update_with(update_person), list[Person])


class PersonHook(CrudHook[Person]):
    def should_run(self, event: CrudEvent, sender: type[Person]) -> bool:
        return event is CrudEvent.PRE_UPDATE and sender is Person

    def pre_update(self, sender: type[Person], instance: Person) -> None:
        if instance.score.value < 0:
            raise HookCancelled("negative score", event=CrudEvent.PRE_UPDATE, hook=self)


person_hook = PersonHook()
assert_type(person_manager.add_hook(person_hook), ProjectedModelManager[Person])
person_manager.remove_hook(person_hook)
assert_type(ProjectedModelNotFoundError("missing"), ProjectedModelNotFoundError)

query_session = Person.query(Database(address="localhost:1729", database="generated-query"))
assert_type(query_session, QuerySession)
person_var = query_session.exact(Person)
other_person_var = query_session.exact(Person)
employment_var = query_session.exact(Employment)
membership_var = query_session.exact(Membership)
party_var = query_session.subtypes(Party)
actor_var = query_session.subtypes(Actor)
interaction_var = query_session.exact(Interaction)
assert_type(person_var, BoundVar[Person])
assert_type(party_var, SubtypeBoundVar[Party])
assert_type(query_session.query(party_var).rows(limit=10), list[Party])
score_field = person_var.field(Person.score)
bool_field = person_var.field(Person.val_bool)
adult = score_field.gte(Score(18))
assert_type(adult, Predicate)
assert_type(score_field.is_present(), Predicate)
assert_type(score_field.is_missing(), Predicate)
assert_type(person_var.iid("0x1"), Predicate)
assert_type(person_var.iid_in(("0x1", "0x2")), Predicate)
people_query = query_session.query(person_var).where(adult)
assert_type(people_query, Query[Person])
assert_type(people_query.one(), Person)
assert_type(people_query.first(), Person | None)
assert_type(people_query.rows(limit=10), list[Person])
assert_type(people_query.count_by(person_var), int)
assert_type(people_query.exists_by(person_var), bool)
assert_type(
    people_query.aggregate(
        person_var,
        aggregate.count(),
        aggregate.sum(score_field),
        aggregate.mean(score_field),
    ),
    tuple[int, int, float | None],
)
assert_type(
    people_query.group_by(person_var, bool_field).aggregate(aggregate.count()),
    tuple[tuple[ValBool, tuple[int]], ...],
)
assert_type(
    people_query.group_by(person_var, bool_field, score_field).aggregate(aggregate.count()),
    tuple[tuple[tuple[ValBool, Score], tuple[int]], ...],
)

employee = employment_var.role(Employment.employee).connects(person_var)
membership_member = membership_var.role(Membership.member).connects(person_var)
interaction_actor = interaction_var.role(Interaction.actor).connects(actor_var)
assert_type(
    query_session.query(interaction_var).match(actor_var).where(interaction_actor),
    Query[Interaction],
)
assert_type(membership_member, Predicate)
reachable = query_session.reachable(
    person_var,
    other_person_var,
    Event,
    Event.subject,
    Event.subject,
    min_depth=0,
    max_depth=3,
)
joined = (
    query_session.query(person_var, employment_var)
    .match(other_person_var)
    .where(
        employee,
        reachable,
    )
)
assert_type(joined.one(), tuple[Person, Employment])
assert_type(
    joined.group_by(person_var, employment_var).aggregate(
        aggregate.count(),
        aggregate.max(score_field),
    ),
    tuple[tuple[Employment, tuple[int, int | None]], ...],
)
named = query_session.query_as(
    EmploymentRow,
    person=person_var,
    employment=employment_var,
).where(employee)
assert_type(named.one(), EmploymentRow)
collected = query_session.query(
    person_var,
    employment_var.collect(),
).where(employee)
page = collected.page_by(
    person_var,
    limit=10,
    order_by=(person_var.field(Person.identifier).asc(),),
    include_total=True,
)
assert_type(page.items, tuple[tuple[Person, tuple[Employment, ...]], ...])
assert_type(page.total, int | None)

sixteen = query_session.query(
    person_var,
    other_person_var,
    employment_var,
    person_var,
    other_person_var,
    employment_var,
    person_var,
    other_person_var,
    employment_var,
    person_var,
    other_person_var,
    employment_var,
    person_var,
    other_person_var,
    employment_var,
    person_var,
)
assert_type(
    sixteen.one(),
    tuple[
        Person,
        Person,
        Employment,
        Person,
        Person,
        Employment,
        Person,
        Person,
        Employment,
        Person,
        Person,
        Employment,
        Person,
        Person,
        Employment,
        Person,
    ],
)

membership_session = Membership.query(
    Database(address="localhost:1729", database="generated-query")
)
membership_var = membership_session.exact(Membership)
assert_type(membership_session.query(membership_var).one(), Membership)


async def check_remote(session: RemoteQuerySession) -> None:
    remote_person = session.exact(Person)
    remote_party = session.subtypes(Party)
    remote_employment = session.exact(Employment)
    remote_employee = remote_employment.role(Employment.employee).connects(remote_person)
    remote = session.query(remote_person, remote_employment).where(remote_employee)
    assert_type(remote, RemoteQuery[Person, Employment])
    assert_type(await remote.one(), tuple[Person, Employment])
    assert_type(await remote.first(), tuple[Person, Employment] | None)
    assert_type(await remote.rows(limit=10), list[tuple[Person, Employment]])
    assert_type(await remote.count_by(remote_person), int)
    assert_type(await remote.exists_by(remote_person), bool)
    assert_type(await session.query(remote_party).rows(limit=10), list[Party])
    remote_score = remote_person.field(Person.score)
    assert_type(
        await remote.aggregate(
            remote_person,
            aggregate.count(),
            aggregate.sum(remote_score),
        ),
        tuple[int, int],
    )
    assert_type(
        await remote.group_by(remote_person, remote_employment).aggregate(
            aggregate.mean(remote_score),
        ),
        tuple[tuple[Employment, tuple[float | None]], ...],
    )
    remote_named = session.query_as(
        EmploymentRow,
        person=remote_person,
        employment=remote_employment,
    ).where(remote_employee)
    assert_type(await remote_named.one(), EmploymentRow)
    remote_page = session.query(
        remote_person,
        remote_employment.collect(),
    ).where(remote_employee)
    assert_type(
        (
            await remote_page.page_by(
                remote_person,
                limit=10,
                order_by=(remote_person.field(Person.identifier).asc(),),
            )
        ).items,
        tuple[tuple[Person, tuple[Employment, ...]], ...],
    )


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
