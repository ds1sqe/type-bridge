"""Positive public typing consumer compiled only against extracted wheels."""

from __future__ import annotations

from collections.abc import Awaitable
from dataclasses import dataclass
from typing import assert_type

from generated_owner_models import (
    GeneratedEmployment,
    GeneratedName,
    GeneratedParty,
    GeneratedPerson,
)

from type_bridge import (
    Database,
    Entity,
    Flag,
    Key,
    Relation,
    Role,
    String,
    TransactionContext,
    TypeFlags,
)
from type_bridge.fields import FieldDescriptor, FieldRef, NumericFieldRef, StringFieldRef
from type_bridge.query import Query as RawQuery
from type_bridge.query_v2 import (
    AuthoredQueryInvocation,
    AuthoredQueryPlan,
    QueryPlanBuilder,
    QueryV2Authority,
)
from type_bridge.typed import (
    BoundRole,
    Page,
    Predicate,
    Query,
    QuerySession,
    RemoteQuery,
    RemoteQuerySession,
    RoleRef,
)


class ArtifactName(String):
    pass


class ArtifactCode(String):
    pass


class Person(Entity):
    flags = TypeFlags(name="artifact-positive-person")
    name: ArtifactName = Flag(Key)


class Company(Entity):
    flags = TypeFlags(name="artifact-positive-company")
    name: ArtifactName = Flag(Key)


class Employment(Relation):
    flags = TypeFlags(name="artifact-positive-employment")
    code: ArtifactCode = Flag(Key)
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)


class SpecializedEmployment(Employment):
    flags = TypeFlags(name="artifact-positive-specialized-employment")


@dataclass(frozen=True, slots=True)
class PersonWork:
    person: Person
    employments: tuple[Employment, ...]
    companies: tuple[Company, ...]


person_name = ArtifactName
employment_code = ArtifactCode
type LegacyFieldRef = FieldRef[ArtifactName]
type LegacyStringFieldRef = StringFieldRef[ArtifactName]
type LegacyNumericFieldRef = NumericFieldRef[ArtifactName]
type LegacyFieldDescriptor = FieldDescriptor[ArtifactName]
type LegacyRoleRef = RoleRef[Person]
legacy_person_name: StringFieldRef[ArtifactName] = StringFieldRef("name", ArtifactName, Person)
legacy_employee_role: RoleRef[Person] = Employment.employee
assert_type(Employment.employee, RoleRef[Person, Employment])

# Bindgen-emitted descriptors require no consumer cast and rebind inherited
# references to the concrete generated subtype.
assert_type(
    GeneratedPerson.name,
    StringFieldRef[GeneratedName, GeneratedPerson],
)
assert_type(
    GeneratedEmployment.employee,
    RoleRef[GeneratedPerson, GeneratedEmployment],
)
generated_value = GeneratedPerson(name=GeneratedName("Alice"))
assert_type(generated_value.name, GeneratedName)


def assert_public_shapes(connection: Database | TransactionContext) -> None:
    """Pin exact scalar, tuple, collected, named, page, and terminal types."""
    raw = RawQuery()
    assert_type(raw, RawQuery)

    session = QuerySession(connection)
    generated_var = session.var(GeneratedPerson)
    generated_employment = session.var(GeneratedEmployment)
    assert_type(
        generated_var.field(GeneratedPerson.name).contains(GeneratedName("li")),
        Predicate,
    )
    assert_type(
        generated_var.field(GeneratedParty.name).contains(GeneratedName("li")),
        Predicate,
    )
    assert_type(
        generated_employment.role(GeneratedEmployment.employee),
        BoundRole[GeneratedPerson],
    )
    person = session.var(Person)
    second_person = session.var(Person)
    company = session.var(Company)
    employment = session.var(Employment)
    second_employment = session.var(Employment)
    specialized_employment = session.var(SpecializedEmployment)
    employee_role = employment.role(Employment.employee)
    assert_type(employee_role, BoundRole[Person])
    assert_type(employee_role.connects(person), Predicate)
    assert_type(employee_role.is_(person), Predicate)
    assert_type(
        specialized_employment.role(Employment.employee).is_(person),
        Predicate,
    )
    assert_type(
        person.field(person_name).eq_field(second_person.field(person_name)),
        Predicate,
    )

    scalar = session.query(person)
    assert_type(scalar, Query[Person])
    assert_type(scalar.one(), Person)
    assert_type(scalar.rows(limit=10), list[Person])
    assert_type(
        scalar.page_by(
            person,
            limit=10,
            order_by=[person.field(person_name).asc()],
            include_total=True,
        ),
        Page[Person],
    )
    assert_type(scalar.count_by(person), int)
    assert_type(scalar.exists_by(person), bool)

    pair = session.query(person, company)
    assert_type(pair, Query[Person, Company])
    assert_type(pair.one(), tuple[Person, Company])
    assert_type(pair.rows(limit=10), list[tuple[Person, Company]])

    repeated = session.query(person, second_person)
    assert_type(repeated, Query[Person, Person])
    assert_type(repeated.one(), tuple[Person, Person])
    assert_type(repeated.rows(limit=10), list[tuple[Person, Person]])

    five = session.query(person, employment, company, second_employment, second_person)
    assert_type(five, Query[Person, Employment, Company, Employment, Person])
    assert_type(
        five.one(),
        tuple[Person, Employment, Company, Employment, Person],
    )
    assert_type(
        five.rows(limit=10),
        list[tuple[Person, Employment, Company, Employment, Person]],
    )

    connected = pair.match(employment).where(
        employee_role.connects(person),
        employment.role(Employment.employer).connects(company),
    )
    assert_type(connected, Query[Person, Company])

    positional = session.query(
        person,
        employment.collect().order_by(employment.field(employment_code).asc()),
        company.collect().distinct(),
    )
    assert_type(
        positional,
        Query[Person, tuple[Employment, ...], tuple[Company, ...]],
    )
    assert_type(
        positional.page_by(
            person,
            limit=10,
            order_by=[person.field(person_name).asc()],
        ),
        Page[tuple[Person, tuple[Employment, ...], tuple[Company, ...]]],
    )

    named = session.query_as(
        PersonWork,
        person=person,
        employments=employment.collect().order_by(employment.field(employment_code).asc()),
        companies=company.collect().distinct(),
    )
    assert_type(named, Query[PersonWork])
    assert_type(
        named.page_by(
            person,
            limit=10,
            order_by=[person.field(person_name).asc()],
        ),
        Page[PersonWork],
    )


def assert_page_shape(page: Page[PersonWork]) -> None:
    """Pin the immutable envelope and lossless optional total surface."""
    assert_type(page.items, tuple[PersonWork, ...])
    assert_type(page.offset, int)
    assert_type(page.limit, int)
    assert_type(page.total, int | None)


def assert_v2_artifact_shapes(
    authority: QueryV2Authority,
    remote_session: RemoteQuerySession,
) -> None:
    """Pin low-level authoring and strict three-binding remote terminal types."""
    builder = QueryPlanBuilder(authority)
    person = builder.binding("person")
    name = builder.binding("name")
    wanted = builder.input("wanted_name", "string", False)
    builder.match(
        (
            builder.isa(person, "entity", "artifact-positive-person", True),
            builder.has(person, name, "artifact-positive-name"),
            builder.value(
                "equal",
                builder.binding_operand(name),
                builder.input_operand(wanted),
            ),
        )
    )
    plan: AuthoredQueryPlan = builder.finalize_rows((person, name))
    invocation: AuthoredQueryInvocation = plan.rows((("Alice",),))

    remote_person = remote_session.var(Person)
    remote_company = remote_session.var(Company)
    remote_employment = remote_session.var(Employment)
    remote = remote_session.query(
        remote_person,
        remote_company,
        remote_employment,
    ).where(
        remote_employment.role(Employment.employee).connects(remote_person),
        remote_employment.role(Employment.employer).connects(remote_company),
    )
    assert_type(remote, RemoteQuery[Person, Company, Employment])
    pending: Awaitable[list[tuple[Person, Company, Employment]]] = remote.rows(
        limit=10,
        order_by=(remote_person.field(person_name).asc(),),
    )
    del invocation, pending
