"""Cross-language golden for a representative public native-handle request."""

from pathlib import Path

from type_bridge_core import MatchSessionHandle, PyDescriptorRegistry

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


def _registry() -> PyDescriptorRegistry:
    registry = PyDescriptorRegistry()
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
