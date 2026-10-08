"""Cross-language golden for a representative retained query request."""

from pathlib import Path

import pytest
from type_bridge_core import MatchRequestError, MatchSessionHandle, _QueryDescriptorRegistry

from tests.unit.typed_query._support import corpus_error

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
            OverflowError,
            "min_depth must be an integer from 0 to 255",
        ),
        (
            "max_depth",
            256,
            OverflowError,
            "max_depth must be an integer from 0 to 255",
        ),
        (
            "min_depth",
            10**100,
            OverflowError,
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
