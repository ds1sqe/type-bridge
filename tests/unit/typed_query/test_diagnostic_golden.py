"""Cross-language golden for a representative retained query request."""

from pathlib import Path

import pytest
from type_bridge_core import MatchSessionHandle, _QueryDescriptorRegistry

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
            "min_depth must be an integer between 0 and 255",
        ),
        (
            "max_depth",
            256,
            ValueError,
            "max_depth must be an integer between 0 and 255",
        ),
        (
            "min_depth",
            10**100,
            ValueError,
            "min_depth must be an integer between 0 and 255",
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
