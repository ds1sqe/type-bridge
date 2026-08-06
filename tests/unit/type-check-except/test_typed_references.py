"""Intentional owner/operator/player errors for the #173 Pyright fixture."""

from __future__ import annotations

from tests.utils.handwritten import (
    Boolean,
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    Role,
    String,
    TypeFlags,
)
from type_bridge import Database
from type_bridge.typed import QuerySession


class Name(String):
    pass


class OtherName(String):
    pass


class Age(Integer):
    pass


class Active(Boolean):
    pass


class Salary(Integer):
    pass


class Person(Entity):
    flags = TypeFlags(name="typed-negative-person")
    name: Name = Flag(Key)
    age: Age | None = None
    active: Active | None = None


class Company(Entity):
    flags = TypeFlags(name="typed-negative-company")
    name: OtherName = Flag(Key)


class Employee(Person):
    flags = TypeFlags(name="typed-negative-employee")
    salary: Salary | None = None


class Employment(Relation):
    flags = TypeFlags(name="typed-negative-employment")
    employee: Role[Person] = Role("employee", Person)


class ParallelEmployment(Relation):
    flags = TypeFlags(name="typed-negative-parallel-employment")
    employee: Role[Person] = Role("employee", Person)


class SpecializedEmployment(Employment):
    flags = TypeFlags(name="typed-negative-specialized-employment")


person_name = Name
person_age = Age
person_active = Active
company_name = OtherName


def invalid_reference_contract(database: Database) -> None:
    session = QuerySession(database)
    person = session.var(Person)
    company = session.var(Company)
    employment = session.var(Employment)
    parallel_employment = session.var(ParallelEmployment)
    foreign_name = company.field(company_name)
    employee_role = employment.role(Employment.employee)

    person.field(Company.name)  # typed-ref-error: cross-owner-field
    person.field(person_active).lt(Active(False))  # typed-ref-error: invalid-operator
    person.field(person_name).eq(OtherName("foreign"))  # typed-ref-error: wrong-literal
    person.field(person_name).eq(foreign_name)  # typed-ref-error: wrong-field
    person.field(person_name).eq_field(foreign_name)  # typed-ref-error: wrong-field-alias
    person.role(Employment.employee)  # typed-ref-error: cross-owner-role
    parallel_employment.role(Employment.employee)  # typed-ref-error: relation-role-owner
    person.field(Employee.salary)  # typed-ref-error: subtype-field-on-base-binding
    employment.role(
        SpecializedEmployment.employee  # typed-ref-error: subtype-role-on-base-binding
    )
    employee_role.connects(employment)  # typed-ref-error: wrong-player
    employee_role.is_(employment)  # typed-ref-error: wrong-player-alias
    employee_role.connects(
        person.collect()  # typed-ref-error: collection-player
    )
    session.var("person")  # typed-ref-error: string-model

    # Same-model cross-session values have the same valid static type. The
    # runtime fixture proves their native lineage is rejected.
