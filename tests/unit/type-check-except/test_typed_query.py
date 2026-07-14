"""Intentional static failures for the checked #174 query facade."""

from __future__ import annotations

from dataclasses import dataclass

from type_bridge import Database, Entity, TypeFlags
from type_bridge.typed import QuerySession


class QueryPerson(Entity):
    flags = TypeFlags(name="typed-query-negative-person")


@dataclass(frozen=True)
class QueryPersonRow:
    person: QueryPerson


def invalid_query_contract(database: Database) -> None:
    QuerySession()  # typed-query-error: missing-connection
    QuerySession("database")  # typed-query-error: invalid-connection
    session = QuerySession(database)
    person = session.var(QueryPerson)

    session.query()  # typed-query-error: zero-selections
    session.query(  # typed-query-error: seventeen-selections
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

    session.query(person, person).page_by(  # typed-query-error: multi-slot-page
        person, limit=10
    )
    session.query("person")  # typed-query-error: no-untyped-fallback
    person.collect().distinct(False)  # typed-query-error: distinct-has-no-toggle
    session.query_as(
        QueryPersonRow,
        person="person",  # typed-query-error: named-selection-required
    )
