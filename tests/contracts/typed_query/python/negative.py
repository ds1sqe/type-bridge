"""Intentional Pyright errors, checked only by ``check_negative.py``."""

from __future__ import annotations

from tests.utils.handwritten import Entity, String, TypeFlags
from type_bridge.session import Database
from type_bridge.typed import Query, QuerySession


class Name(String):
    pass


class Person(Entity):
    flags = TypeFlags(name="negative-contract-person")
    name: Name


def invalid_arities(database: Database) -> None:
    session = QuerySession(database)
    person = session.var(Person)

    session.query()  # contract-error: zero-selections
    session.query(  # contract-error: seventeen-selections
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

    # Python's static type system cannot distinguish two values of the same
    # BoundVar[Person] type. Selecting one handle twice is therefore a canonical
    # Rust runtime error, not a static diagnostic in the public contract.
    runtime_duplicate_check: Query[Person, Person] = session.query(person, person)
    del runtime_duplicate_check

    person.field(Name).like(Name("A"))  # contract-error: like-absent
