"""Positive Pyright fixture for owner-aware typed references."""

from __future__ import annotations

from dataclasses import dataclass
from typing import NamedTuple, assert_type

from type_bridge import (
    Boolean,
    Database,
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    Role,
    String,
    TransactionContext,
    TypeFlags,
)
from type_bridge.fields import FieldDescriptor, FieldRef, NumericFieldRef, StringFieldRef
from type_bridge.typed import (
    BoundRole,
    BoundVar,
    Collected,
    Predicate,
    Query,
    QueryOrder,
    QuerySession,
    RoleRef,
    Selection,
)
from type_bridge.typed.page import Page


class Name(String):
    pass


class Age(Integer):
    pass


class Active(Boolean):
    pass


class Person(Entity):
    flags = TypeFlags(name="typed-positive-person")
    name: Name = Flag(Key)
    age: Age | None = None
    active: Active | None = None


class Employee(Person):
    flags = TypeFlags(name="typed-positive-employee")


class Company(Entity):
    flags = TypeFlags(name="typed-positive-company")
    name: Name = Flag(Key)


class Bot(Entity):
    flags = TypeFlags(name="typed-positive-bot")


class Employment(Relation):
    flags = TypeFlags(name="typed-positive-employment")
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)


class SpecializedEmployment(Employment):
    flags = TypeFlags(name="typed-positive-specialized-employment")


class Interaction(Relation):
    flags = TypeFlags(name="typed-positive-interaction")
    actor: Role[Person | Bot] = Role.multi("actor", Person, Bot)


@dataclass(frozen=True, slots=True)
class PersonOnly:
    person: Person


@dataclass(frozen=True, slots=True)
class PersonWork:
    person: Person
    employments: tuple[Employment, ...]
    companies: tuple[Company, ...]


class PersonWorkTuple(NamedTuple):
    person: Person
    employments: tuple[Employment, ...]
    companies: tuple[Company, ...]


# Bound variables derive the field owner from their model, so hand-written
# Pydantic models use attribute-class tokens without descriptor casts.
person_name = Name
person_age = Age
person_active = Active
employee_name = Name
type LegacyFieldRef = FieldRef[Name]
type LegacyStringFieldRef = StringFieldRef[Name]
type LegacyNumericFieldRef = NumericFieldRef[Age]
type LegacyFieldDescriptor = FieldDescriptor[Name]
type LegacyRoleRef = RoleRef[Person]
legacy_person_name: StringFieldRef[Name] = StringFieldRef("name", Name, Person)
legacy_employee_role: RoleRef[Person] = Employment.employee
assert_type(Employment.employee, RoleRef[Person, Employment])
assert_type(
    SpecializedEmployment.employee,
    RoleRef[Person, SpecializedEmployment],
)


def positive_reference_contract(connection: Database | TransactionContext) -> None:
    session = QuerySession(connection)
    first = session.var(Person)
    second = session.exact(Person)
    employee = session.subtypes(Employee)
    company = session.var(Company)
    employment = session.var(Employment)
    specialized_employment = session.var(SpecializedEmployment)
    interaction = session.var(Interaction)
    bot = session.var(Bot)

    assert_type(first, BoundVar[Person])
    assert_type(second, BoundVar[Person])
    assert_type(first.collect(), Collected[Person])

    selected: Selection[Person] = first
    collected: Selection[tuple[Person, ...]] = first.collect()
    del selected, collected

    assert_type(first.field(person_name).eq(Name("Alice")), Predicate)
    assert_type(first.field(person_name).neq(second.field(person_name)), Predicate)
    assert_type(first.field(person_name).eq_field(second.field(person_name)), Predicate)
    assert_type(first.field(person_name).contains(Name("lic")), Predicate)
    assert_type(first.field(person_age).gte(Age(18)), Predicate)
    assert_type(first.field(person_age).asc(), QueryOrder)
    assert_type(first.field(person_active).eq(Active(True)), Predicate)
    assert_type(employee.field(employee_name).eq(Name("Alice")), Predicate)
    employee_role = employment.role(Employment.employee)
    assert_type(employee_role, BoundRole[Person])
    assert_type(employee_role.is_(first), Predicate)
    assert_type(employee_role.connects(first), Predicate)
    assert_type(employment.role(Employment.employer).connects(company), Predicate)
    assert_type(
        specialized_employment.role(Employment.employee).is_(first),
        Predicate,
    )
    assert_type(interaction.role(Interaction.actor).connects(first), Predicate)
    assert_type(interaction.role(Interaction.actor).connects(bot), Predicate)


def positive_query_contract(connection: Database | TransactionContext) -> None:
    session = QuerySession(connection)
    people = [session.var(Person) for _ in range(16)]
    companies = [session.var(Company) for _ in range(5)]
    employments = [session.var(Employment) for _ in range(5)]

    scalar = session.query(people[0])
    assert_type(scalar, Query[Person])
    assert_type(scalar.one(), Person)
    assert_type(scalar.rows(limit=20), list[Person])
    assert_type(scalar.page_by(people[0], limit=20), Page[Person])
    assert_type(scalar.count_by(people[0]), int)
    assert_type(scalar.exists_by(people[0]), bool)

    pair = session.query(people[0], companies[0])
    assert_type(pair, Query[Person, Company])
    assert_type(pair.one(), tuple[Person, Company])
    assert_type(pair.rows(limit=20), list[tuple[Person, Company]])

    repeated = session.query(people[0], people[1])
    assert_type(repeated, Query[Person, Person])
    assert_type(repeated.one(), tuple[Person, Person])
    assert_type(repeated.rows(limit=20), list[tuple[Person, Person]])

    five = session.query(people[0], employments[0], companies[0], employments[1], people[1])
    assert_type(five, Query[Person, Employment, Company, Employment, Person])
    assert_type(
        five.one(),
        tuple[Person, Employment, Company, Employment, Person],
    )
    assert_type(
        five.rows(limit=20),
        list[tuple[Person, Employment, Company, Employment, Person]],
    )

    sixteen = session.query(
        people[0],
        employments[0],
        companies[0],
        people[1],
        employments[1],
        companies[1],
        people[2],
        employments[2],
        companies[2],
        people[3],
        employments[3],
        companies[3],
        people[4],
        employments[4],
        companies[4],
        people[5],
    )
    assert_type(
        sixteen,
        Query[
            Person,
            Employment,
            Company,
            Person,
            Employment,
            Company,
            Person,
            Employment,
            Company,
            Person,
            Employment,
            Company,
            Person,
            Employment,
            Company,
            Person,
        ],
    )
    assert_type(
        sixteen.rows(limit=20),
        list[
            tuple[
                Person,
                Employment,
                Company,
                Person,
                Employment,
                Company,
                Person,
                Employment,
                Company,
                Person,
                Employment,
                Company,
                Person,
                Employment,
                Company,
                Person,
            ]
        ],
    )

    hidden = pair.match(employments[0]).where(
        employments[0].role(Employment.employee).connects(people[0]),
        employments[0].role(Employment.employer).connects(companies[0]),
    )
    assert_type(hidden, Query[Person, Company])
    assert_type(pair.allow_cross_join(people[0], companies[0]), Query[Person, Company])
    assert_type(
        scalar.where(people[0].field(person_name).eq(Name("Alice"))),
        Query[Person],
    )


def positive_connection_contract(connection: Database | TransactionContext) -> None:
    session = QuerySession(connection)
    person = session.var(Person)
    assert_type(session.query(person), Query[Person])


def positive_collected_and_named_contract(connection: Database | TransactionContext) -> None:
    session = QuerySession(connection)
    person = session.var(Person)
    employment = session.var(Employment)
    company = session.var(Company)

    positional = session.query(
        person,
        employment.collect(),
        company.collect().distinct(),
    )
    assert_type(
        positional,
        Query[Person, tuple[Employment, ...], tuple[Company, ...]],
    )
    assert_type(
        positional.page_by(person, limit=20),
        Page[tuple[Person, tuple[Employment, ...], tuple[Company, ...]]],
    )
    non_first_root = session.query(
        employment.collect(),
        person,
        company.collect(),
    )
    assert_type(
        non_first_root.page_by(person, limit=20),
        Page[tuple[tuple[Employment, ...], Person, tuple[Company, ...]]],
    )

    scalar_named = session.query_as(PersonOnly, person=person)
    assert_type(scalar_named, Query[PersonOnly])
    assert_type(scalar_named.one(), PersonOnly)
    assert_type(scalar_named.rows(limit=20), list[PersonOnly])
    assert_type(scalar_named.page_by(person, limit=20), Page[PersonOnly])

    named = session.query_as(
        PersonWork,
        person=person,
        employments=employment.collect(),
        companies=company.collect().distinct(),
    )
    assert_type(named, Query[PersonWork])
    assert_type(named.page_by(person, limit=20), Page[PersonWork])

    named_tuple = session.query_as(
        PersonWorkTuple,
        person=person,
        employments=employment.collect(),
        companies=company.collect().distinct(),
    )
    assert_type(named_tuple, Query[PersonWorkTuple])
    assert_type(named_tuple.page_by(person, limit=20), Page[PersonWorkTuple])
