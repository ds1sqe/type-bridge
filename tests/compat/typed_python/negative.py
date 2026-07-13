"""Intentional public typing failures compiled only against extracted wheels."""

from __future__ import annotations

from dataclasses import dataclass

from generated_owner_models import GeneratedParty, GeneratedPerson

from type_bridge import Database, Entity, Relation, Role, String, TypeFlags
from type_bridge.typed import QuerySession


class ArtifactName(String):
    pass


class Person(Entity):
    flags = TypeFlags(name="artifact-negative-person")
    name: ArtifactName


class Company(Entity):
    flags = TypeFlags(name="artifact-negative-company")
    name: ArtifactName


class Employment(Relation):
    flags = TypeFlags(name="artifact-negative-employment")
    employee: Role[Person] = Role("employee", Person)


class SpecializedEmployment(Employment):
    flags = TypeFlags(name="artifact-negative-specialized-employment")


@dataclass(frozen=True, slots=True)
class PersonRow:
    person: Person


def invalid_public_shapes(database: Database) -> None:
    """Keep every expected consumer rejection on a marked source line."""
    QuerySession()  # artifact-type-error: missing-connection
    QuerySession("database")  # artifact-type-error: invalid-connection
    session = QuerySession(database)
    person = session.var(Person)
    company = session.var(Company)
    employment = session.var(Employment)
    generated_party = session.var(GeneratedParty)

    session.query()  # artifact-type-error: zero-selections
    session.query(  # artifact-type-error: seventeen-selections
        person,
        person,
        person,
        person,
        person,
        person,
        person,
        person,
        person,
        person,
        person,
        person,
        person,
        person,
        person,
        person,
        person,
    )
    session.query(person, company).page_by(  # artifact-type-error: multi-slot-page
        person, limit=10
    )
    session.query("person")  # artifact-type-error: untyped-selection
    person.collect().distinct(False)  # artifact-type-error: distinct-toggle
    session.query_as(
        PersonRow,
        person="person",  # artifact-type-error: named-selection
    )
    company.field(GeneratedPerson.name)  # artifact-type-error: foreign-field-owner
    generated_party.field(GeneratedPerson.name)  # artifact-type-error: subtype-field-on-base
    employment.role(
        SpecializedEmployment.employee  # artifact-type-error: subtype-role-on-base
    )
    employment.role(Employment.employee).connects(company)  # artifact-type-error: wrong-role-player
