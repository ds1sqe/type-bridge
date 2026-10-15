"""Cross-language golden for a representative retained query request."""

import socket
from collections.abc import Callable
from pathlib import Path

import pytest
from type_bridge_core import (
    InvalidQueryRequestError,
    MatchRequestError,
    MatchSessionHandle,
    ProxyConnection,
    _QueryDescriptorRegistry,
)

from tests.unit.query.test_query_v2_remote_runtime import (
    PLAN_CAPABILITIES,
    _advertisement,
    _authority,
)
from tests.unit.typed_query._corpus import corpus_error

GOLDEN = (
    Path(__file__).parents[3]
    / "type-bridge-core"
    / "crates"
    / "orm"
    / "tests"
    / "fixtures"
    / "match_request"
    / "public-named-page.json"
)


def _registry() -> _QueryDescriptorRegistry:
    registry = _QueryDescriptorRegistry()
    for type_name, attr_name in (
        ("person", "person-name"),
        ("company", "company-name"),
    ):
        registry.register_entity(
            {
                "type_name": type_name,
                "is_abstract": False,
                "parent_type": None,
                "owned_attributes": [
                    {
                        "field_name": "name",
                        "attr_name": attr_name,
                        "value_type": "string",
                        "annotations": ["Key"],
                        "is_optional": False,
                        "is_ordered": False,
                    }
                ],
            }
        )
    registry.register_relation(
        {
            "type_name": "employment",
            "is_abstract": False,
            "parent_type": None,
            "owned_attributes": [],
            "roles": [
                {
                    "role_name": "employee",
                    "player_type_names": ["person"],
                    "cardinality": [1, 1],
                },
                {
                    "role_name": "employer",
                    "player_type_names": ["company"],
                    "cardinality": [1, 1],
                },
            ],
        }
    )
    return registry


def test_public_named_page_matches_the_rust_and_node_golden() -> None:
    registry = _registry()
    session = MatchSessionHandle(registry)
    person = session.exact("person")
    company = session.exact("company")
    employment = session.exact("employment")
    person_order = person.field("name").order("ascending", "reject")
    company_order = company.field("name").order("ascending", "reject")
    companies = company.collect().distinct(True).order_by(company_order)
    shape = session.named(["person", "companies"], [person.one(), companies])
    connected = (
        employment.role("employee")
        .connects(person)
        .and_(employment.role("employer").connects(company))
    )
    query = session.query(shape).add_hidden(employment).where_predicate(connected)

    assert (
        query.page_by_diagnostic(
            person,
            [person_order],
            10,
            10,
            True,
        )
        == GOLDEN.read_text(encoding="utf-8").strip()
    )


@pytest.mark.parametrize(
    ("name", "value", "error_type", "message"),
    [
        ("min_depth", True, TypeError, "min_depth must be an exact Python int"),
        ("max_depth", 1.5, TypeError, "max_depth must be an exact Python int"),
        (
            "min_depth",
            -1,
            ValueError,
            "min_depth must be an integer from 0 to 255",
        ),
        (
            "max_depth",
            256,
            ValueError,
            "max_depth must be an integer from 0 to 255",
        ),
        (
            "min_depth",
            10**100,
            ValueError,
            "min_depth must be an integer from 0 to 255",
        ),
    ],
)
def test_native_reachable_requires_exact_bounded_pyints(
    name: str,
    value: object,
    error_type: type[Exception],
    message: str,
) -> None:
    session = MatchSessionHandle(_registry())
    source = session.exact("person")
    target = session.exact("company")
    arguments = {
        "relation_type": "employment",
        "role_from": "employee",
        "role_to": "employer",
        "source": source,
        "target": target,
        "min_depth": 0,
        "max_depth": 1,
    }
    arguments[name] = value

    with pytest.raises(error_type, match=f"^{message}$"):
        session.reachable(**arguments)


def _reachable(session: MatchSessionHandle, min_depth: int, max_depth: int) -> object:
    return session.reachable(
        relation_type="employment",
        role_from="employee",
        role_to="employer",
        source=session.exact("person"),
        target=session.exact("company"),
        min_depth=min_depth,
        max_depth=max_depth,
    )


@pytest.mark.parametrize(
    ("case_id", "min_depth", "max_depth"),
    [
        ("reachability.reversed-bounds", 3, 2),
        ("reachability.depth-ceiling", 0, 65),
    ],
)
def test_native_reachable_depth_rules_are_core_corpus_codes(
    case_id: str, min_depth: int, max_depth: int
) -> None:
    session = MatchSessionHandle(_registry())

    with pytest.raises(MatchRequestError) as raised:
        _reachable(session, min_depth, max_depth)
    assert (raised.value.category, raised.value.code) == corpus_error(case_id)


def test_native_group_fields_are_capped_at_the_group_by_call() -> None:
    session = MatchSessionHandle(_registry())
    person = session.exact("person")
    query = session.query(session.positional([person.one()]))
    name = person.field("name")

    query.check_group_fields([name] * 16)
    with pytest.raises(MatchRequestError) as raised:
        query.check_group_fields([name] * 17)
    assert (raised.value.category, raised.value.code) == corpus_error(
        "aggregate.seventeen-group-keys"
    )
    assert raised.value.details["maximum"] == {"kind": "count", "value": 16}


def _closed_proxy() -> ProxyConnection:
    # A pinned advertisement connects without reaching the endpoint, and the
    # core refuses the plan before any request is sent.
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    return ProxyConnection.connect(
        _authority(),
        f"http://127.0.0.1:{port}",
        advertisement=_advertisement(PLAN_CAPABILITIES),
    )


def _value_error_calls() -> dict[str, Callable[[], object]]:
    session = MatchSessionHandle(_registry())
    person = session.exact("person")
    name = person.field("name")
    query = session.query(session.positional([person.one()]))
    proxy = _closed_proxy()
    return {
        "bounds.public-invalid-limit": lambda: query.execute_fetch_rows_proxy(
            proxy, [], 0, 0, "bounded_many"
        ),
        "bounds.window-overflow": lambda: query.execute_fetch_rows_proxy(
            proxy, [], 2**64 - 2, 2, "bounded_many"
        ),
        "selection.seventeen-slot-rejection": lambda: session.positional(
            [session.exact("person").one() for _ in range(17)]
        ),
        "aggregate.empty-terms": lambda: query.execute_reduce_by_proxy(proxy, person, None, [], []),
        "aggregate.seventeen-terms": lambda: query.execute_reduce_by_proxy(
            proxy, person, None, ["count"] * 17, [None] * 17
        ),
        "aggregate.seventeen-group-keys": lambda: query.check_group_fields([name] * 17),
        "reachability.reversed-bounds": lambda: _reachable(session, 3, 2),
        "reachability.depth-ceiling": lambda: _reachable(session, 0, 65),
    }


@pytest.mark.parametrize("case_id", sorted(_value_error_calls()))
def test_refusals_that_were_value_errors_stay_value_errors(case_id: str) -> None:
    # type-bridge 2.2 raised ValueError for these; the core's refusal keeps
    # that base beside MatchRequestError.
    with pytest.raises(ValueError) as raised:
        _value_error_calls()[case_id]()
    assert type(raised.value) is InvalidQueryRequestError
    assert isinstance(raised.value, MatchRequestError)
    assert (raised.value.category, raised.value.code) == corpus_error(case_id)


def test_other_plan_refusals_are_not_value_errors() -> None:
    session = MatchSessionHandle(_registry())
    with pytest.raises(MatchRequestError) as raised:
        session.exact("missing")
    assert raised.value.code == "unknown_descriptor"
    assert not isinstance(raised.value, ValueError)


def test_count_details_are_python_ints() -> None:
    session = MatchSessionHandle(_registry())
    person = session.exact("person")
    query = session.query(session.positional([person.one()]))
    with pytest.raises(MatchRequestError) as raised:
        query.execute_reduce_by_proxy(_closed_proxy(), person, None, ["count"] * 17, [None] * 17)
    assert raised.value.details["actual"] == {"kind": "count", "value": 17}
    assert raised.value.details["maximum"] == {"kind": "count", "value": 16}
    maximum = raised.value.details["maximum"]
    assert isinstance(maximum, dict)
    assert type(maximum["value"]) is int
