"""Intentional static failures for the checked #174 query facade."""

from __future__ import annotations

from collections.abc import Awaitable
from dataclasses import dataclass

from tests.utils.handwritten import Entity, TypeFlags
from type_bridge import Database
from type_bridge.typed import (
    Query,
    QuerySession,
    RemoteQuery,
    RemoteQueryExchange,
    RemoteQuerySession,
)
from type_bridge.typed._remote_terminal import _RemoteRuntime, execute_remote_one


class QueryPerson(Entity):
    flags = TypeFlags(name="typed-query-negative-person")


@dataclass(frozen=True)
class QueryPersonRow:
    person: QueryPerson


async def invalid_remote_exchange_call(exchange: RemoteQueryExchange) -> None:
    await exchange(request=b"request")  # typed-query-error: exchange-positional-only


def invalid_remote_query_identity(
    direct_session: QuerySession,
    remote_session: RemoteQuerySession,
    runtime: _RemoteRuntime,
) -> None:
    direct_person = direct_session.var(QueryPerson)
    direct = direct_session.query(direct_person)
    remote_person = remote_session.var(QueryPerson)
    remote = remote_session.query(remote_person)

    direct_awaitable: Awaitable[QueryPerson] = (  # typed-query-error: direct-not-awaitable
        direct.one()
    )
    remote_value: QueryPerson = remote.one()  # typed-query-error: remote-await-required
    direct_query: Query[QueryPerson] = remote  # typed-query-error: remote-not-direct-query
    remote_query: RemoteQuery[QueryPerson] = direct  # typed-query-error: direct-not-remote-query
    execute_remote_one(remote, runtime)  # typed-query-error: remote-rejected-by-direct-helper
    del direct_awaitable, remote_value, direct_query, remote_query


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
