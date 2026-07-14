"""Runtime coverage for the immutable native-backed #174 query facade."""

from __future__ import annotations

from collections.abc import Callable

import pytest
import type_bridge_core

from tests.unit.typed_query._support import (
    corpus_error,
    diagnostic_session,
    invoke_untyped,
)
from type_bridge import (
    Entity,
    Flag,
    Key,
    Relation,
    Role,
    String,
    TypeFlags,
)
from type_bridge import (
    Query as LegacyQuery,
)
from type_bridge.session import Database
from type_bridge.typed import (
    Query,
    QuerySession,
    TypedQueryConnectionError,
    TypedQueryWindowError,
)


class QueryName(String):
    pass


class QueryPerson(Entity):
    flags = TypeFlags(name="typed-query-person")
    name: QueryName = Flag(Key)


class QueryCompany(Entity):
    flags = TypeFlags(name="typed-query-company")
    name: QueryName = Flag(Key)


class QueryEmployment(Relation):
    flags = TypeFlags(name="typed-query-employment")
    employee: Role[QueryPerson] = Role("employee", QueryPerson)
    employer: Role[QueryCompany] = Role("employer", QueryCompany)


def _assert_connection_required(callback: Callable[[], object]) -> None:
    with pytest.raises(TypedQueryConnectionError) as raised:
        callback()
    assert raised.value.category == "invalid_plan"
    assert raised.value.code == "execution_connection_required"


def test_query_arities_repeated_models_duplicate_handle_and_cap() -> None:
    session = diagnostic_session()
    people = [session.var(QueryPerson) for _ in range(17)]

    with pytest.raises(type_bridge_core.MatchRequestError) as missing:
        invoke_untyped(session.query)
    assert missing.value.category == "invalid_plan"
    assert missing.value.code == "empty_output"

    one = session.query(people[0])
    two = session.query(people[0], people[1])
    five = session.query(
        people[0],
        people[1],
        people[2],
        people[3],
        people[4],
    )
    sixteen = session.query(
        people[0],
        people[1],
        people[2],
        people[3],
        people[4],
        people[5],
        people[6],
        people[7],
        people[8],
        people[9],
        people[10],
        people[11],
        people[12],
        people[13],
        people[14],
        people[15],
    )

    assert isinstance(one, Query)
    assert isinstance(two, Query)
    assert isinstance(five, Query)
    assert isinstance(sixteen, Query)

    with pytest.raises(type_bridge_core.MatchRequestError) as duplicate:
        session.query(people[0], people[0])
    assert (duplicate.value.category, duplicate.value.code) == corpus_error(
        "selection.duplicate-handle"
    )

    connected_sixteen = sixteen
    for person in people[1:16]:
        connected_sixteen = connected_sixteen.allow_cross_join(people[0], person)
    _assert_connection_required(lambda: connected_sixteen.rows(limit=16))

    with pytest.raises(type_bridge_core.MatchRequestError) as oversized:
        invoke_untyped(session.query, *people)
    assert (oversized.value.category, oversized.value.code) == corpus_error(
        "selection.seventeen-slot-rejection"
    )


def test_hidden_bindings_predicates_and_siblings_are_persistent() -> None:
    session = diagnostic_session()
    person = session.var(QueryPerson)
    company = session.var(QueryCompany)
    employment = session.var(QueryEmployment)
    base = session.query(person, company).match(employment)

    employee = employment.role(QueryEmployment.employee).connects(person)
    employer = employment.role(QueryEmployment.employer).connects(company)
    partial = base.where(employee)
    complete = base.where(employee, employer)

    for disconnected in (base, partial):
        with pytest.raises(type_bridge_core.MatchRequestError) as raised:
            disconnected.rows(limit=10)
        assert (raised.value.category, raised.value.code) == corpus_error("topology.disconnected")

    _assert_connection_required(lambda: complete.rows(limit=10))


def test_boolean_binding_rules_match_the_semantic_corpus() -> None:
    session = diagnostic_session()
    person = session.var(QueryPerson)
    company = session.var(QueryCompany)
    person_name = person.field(QueryName).eq(QueryName("Alice"))
    company_name = company.field(QueryName).eq(QueryName("Acme"))

    partial_or = session.query(person).match(company).where(person_name | company_name)
    with pytest.raises(type_bridge_core.MatchRequestError) as partial:
        partial_or.rows(limit=1)
    assert (partial.value.category, partial.value.code) == corpus_error(
        "boolean.or-definite-binding"
    )

    with pytest.raises(type_bridge_core.MatchRequestError) as unattached:
        session.query(person).where(~company_name)
    assert (unattached.value.category, unattached.value.code) == corpus_error(
        "boolean.not-unattached-reference"
    )


def test_cross_join_is_explicit_topology_and_builders_remain_opaque() -> None:
    session = diagnostic_session()
    person = session.var(QueryPerson)
    company = session.var(QueryCompany)
    base = session.query(person, company)
    allowed = base.allow_cross_join(person, company)

    with pytest.raises(type_bridge_core.MatchRequestError) as disconnected:
        base.rows(limit=10)
    assert (disconnected.value.category, disconnected.value.code) == corpus_error(
        "topology.disconnected"
    )

    _assert_connection_required(lambda: allowed.rows(limit=10))
    assert base is not allowed
    for query in (base, allowed):
        assert not hasattr(query, "__dict__")
        assert not hasattr(query, "plan")
        assert not hasattr(query, "diagnostic")


def test_terminals_preserve_native_operation_and_cardinality() -> None:
    session = diagnostic_session()
    person = session.var(QueryPerson)
    query = session.query(person).where(person.field(QueryName).eq(QueryName("Alice")))
    order = person.field(QueryName).asc()

    _assert_connection_required(query.one)
    _assert_connection_required(lambda: query.rows(limit=10, offset=1, order_by=(order,)))
    _assert_connection_required(
        lambda: query.page_by(
            person,
            limit=10,
            order_by=(order,),
            include_total=True,
        ),
    )
    _assert_connection_required(lambda: query.count_by(person))
    _assert_connection_required(lambda: query.exists_by(person))


def test_real_connection_ownership_is_retained_without_construction_io() -> None:
    with pytest.raises(TypeError, match="connection"):
        invoke_untyped(QuerySession)
    with pytest.raises(TypeError, match="Database or TransactionContext"):
        invoke_untyped(QuerySession, None)

    database = Database(server_version="3.11.0")
    transaction = database.transaction("read")

    owned_session = QuerySession(database)
    borrowed_session = QuerySession(transaction)
    owned = owned_session.var(QueryPerson)
    borrowed = borrowed_session.var(QueryPerson)

    owned_session.query(owned)
    with pytest.raises(TypedQueryConnectionError) as inactive:
        borrowed_session.query(borrowed).rows(limit=1)
    assert inactive.value.code == "inactive_transaction_context"
    assert not hasattr(database, "_rust_backend_database")
    assert transaction._rust_tx is None

    with pytest.raises(TypeError, match="Database or TransactionContext"):
        invoke_untyped(QuerySession, object())


def test_invalid_lineage_and_empty_builders_fail_before_terminal_adapter() -> None:
    session = diagnostic_session()
    person = session.var(QueryPerson)
    foreign = diagnostic_session().var(QueryPerson)
    query = session.query(person)

    with pytest.raises(type_bridge_core.MatchRequestError) as cross_session:
        session.query(foreign)
    assert cross_session.value.code == "cross_session_handle"

    with pytest.raises(TypeError, match="at least one BoundVar"):
        query.match()
    with pytest.raises(TypeError, match="at least one Predicate"):
        query.where()


@pytest.mark.parametrize(
    ("arguments", "code"),
    [
        ({"limit": 0}, "invalid_window_limit"),
        ({"limit": -1}, "invalid_window_limit"),
        ({"limit": 1, "offset": -1}, "invalid_window_offset"),
        ({"limit": 2**64}, "invalid_window_limit"),
        ({"limit": 2, "offset": 2**64 - 2}, "window_overflow"),
    ],
)
def test_window_checks_are_stable_before_native_unsigned_conversion(
    arguments: dict[str, int], code: str
) -> None:
    session = diagnostic_session()
    person = session.var(QueryPerson)
    query = session.query(person)

    with pytest.raises(TypedQueryWindowError) as raised:
        invoke_untyped(query.rows, **arguments)
    assert raised.value.category == "invalid_plan"
    assert raised.value.code == code

    case_id = (
        "bounds.public-invalid-offset"
        if code == "invalid_window_offset"
        else "bounds.window-overflow"
        if code == "window_overflow"
        else "bounds.public-invalid-limit"
    )
    assert (raised.value.category, raised.value.code) == corpus_error(case_id)


def test_legacy_query_import_remains_separate() -> None:
    assert LegacyQuery is not Query
