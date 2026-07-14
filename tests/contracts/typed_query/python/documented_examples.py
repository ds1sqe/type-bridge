from dataclasses import dataclass
from typing import assert_type

from type_bridge import (
    Database,
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    Role,
    String,
    TypeFlags,
)
from type_bridge.typed import BoundRole, Page, Query, QuerySession, RoleRef


class Name(String):
    pass


class Age(Integer):
    pass


class Industry(String):
    pass


class Position(String):
    pass


class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)
    age: Age


class Company(Entity):
    flags = TypeFlags(name="company")
    name: Name = Flag(Key)
    industry: Industry


class Employment(Relation):
    flags = TypeFlags(name="employment")
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)
    position: Position


# Attribute classes are owner-derived field tokens. The bound variable supplies
# the model owner; no descriptor cast or public string is part of the API.
person_name = Name
person_age = Age
company_name = Name
company_industry = Industry

db = Database(address="localhost:1729", database="typed_query_example")
db.connect()
session = QuerySession(db)
person = session.var(Person)
employment = session.var(Employment)
company = session.var(Company)
assert_type(Employment.employee, RoleRef[Person, Employment])
assert_type(Employment.employer, RoleRef[Company, Employment])
assert_type(employment.role(Employment.employee), BoundRole[Person])

one_person: Query[Person] = session.query(person)
assert_type(one_person.one(), Person)
assert_type(one_person.rows(limit=20), list[Person])

two_slots: Query[Person, Company] = (
    session.query(person, company)
    .match(employment)
    .where(
        employment.role(Employment.employee).is_(person),
        employment.role(Employment.employer).is_(company),
    )
)
assert_type(two_slots.one(), tuple[Person, Company])
assert_type(two_slots.rows(limit=20), list[tuple[Person, Company]])

colleague = session.var(Person)
other_employment = session.var(Employment)
five_slots: Query[Person, Employment, Company, Employment, Person] = session.query(
    person,
    employment,
    company,
    other_employment,
    colleague,
).where(
    employment.role(Employment.employee).is_(person),
    employment.role(Employment.employer).is_(company),
    other_employment.role(Employment.employee).is_(colleague),
    other_employment.role(Employment.employer).is_(company),
)
assert_type(
    five_slots.rows(limit=20),
    list[tuple[Person, Employment, Company, Employment, Person]],
)

person_pair: Query[Person, Person] = (
    session.query(person, colleague)
    .match(
        employment,
        other_employment,
        company,
    )
    .where(
        employment.role(Employment.employee).is_(person),
        employment.role(Employment.employer).is_(company),
        other_employment.role(Employment.employee).is_(colleague),
        other_employment.role(Employment.employer).is_(company),
    )
)
assert_type(person_pair.rows(limit=20), list[tuple[Person, Person]])

# QuerySession.query has checked overloads through 16 selections. A seventeenth
# selection is a static diagnostic and Rust rejects forged unchecked requests.

adults_in_ai = two_slots.where(
    person.field(person_age).gte(Age(18)),
    company.field(company_industry).eq(Industry("AI")),
    person.field(person_name).starts_with(Name("Al")),
    company.field(company_name).contains(Name("Research")),
    company.field(company_name).ends_with(Name("Labs")),
    person.field(person_name).regex(Name(r"^A[[:alpha:]]+$")),
)

# Percent is literal input here; it has no SQL wildcard meaning.
literal_percent = one_person.where(person.field(person_name).contains(Name("50%")))

# Cross joins are explicit topology permission, never a boolean predicate.
independent_pairs: Query[Person, Company] = session.query(person, company).allow_cross_join(
    person, company
)

ordered_people = one_person.rows(
    limit=50,
    offset=0,
    order_by=(person.field(person_name).asc(),),
)

person_count: int = two_slots.count_by(person)
any_person: bool = two_slots.exists_by(person)


@dataclass(frozen=True, slots=True)
class PersonWork:
    person: Person
    employments: tuple[Employment, ...]
    companies: tuple[Company, ...]


work: Query[PersonWork] = session.query_as(
    PersonWork,
    person=person,
    employments=employment.collect(),
    companies=company.collect().distinct(),
).where(
    employment.role(Employment.employee).is_(person),
    employment.role(Employment.employer).is_(company),
)

page: Page[PersonWork] = work.page_by(
    person,
    limit=50,
    offset=0,
    order_by=(person.field(person_name).asc(),),
    include_total=True,
)
assert_type(page.items, tuple[PersonWork, ...])
assert_type(page.total, int | None)

# Owned: rows() opens one read transaction and closes it on success or error.
owned = QuerySession(db)
owned_person = owned.var(Person)
owned_rows = owned.query(owned_person).rows(limit=10)

# Borrowed: the surrounding context owns lifecycle and may run more work.
with db.transaction("read") as tx:
    borrowed = QuerySession(tx)
    borrowed_person = borrowed.var(Person)
    first_page = borrowed.query(borrowed_person).rows(limit=10)
    second_page = borrowed.query(borrowed_person).rows(limit=10, offset=10)
