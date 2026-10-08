"""Python half of the enum-spelling parity corpus.

Every enum the native module accepts as text is parsed by the Rust core; an
unknown spelling is one ``UnknownEnumValueError`` (a ``MatchRequestError``
and a ``ValueError``) with the corpus code and details.
"""

import json
from collections.abc import Callable
from pathlib import Path

import pytest
from type_bridge_core import (
    MatchRequestError,
    MatchSessionHandle,
    UnknownEnumValueError,
    _QueryDescriptorRegistry,
    canonical_enum_spelling,
)

_CORPUS = json.loads(
    (Path(__file__).parents[2] / "contracts" / "enum-spellings-v1.json").read_text(encoding="utf-8")
)
_CASES = {case["id"]: case for case in _CORPUS["cases"]}


def _assert_case(error: UnknownEnumValueError, case_id: str) -> None:
    expected = _CASES[case_id]["expected"]
    assert isinstance(error, MatchRequestError)
    assert isinstance(error, ValueError)
    assert (error.category, error.code, error.details) == (
        expected["category"],
        expected["code"],
        expected["details"],
    )


@pytest.mark.parametrize("case_id", sorted(_CASES))
def test_corpus_case_is_the_core_diagnostic(case_id: str) -> None:
    case = _CASES[case_id]
    with pytest.raises(UnknownEnumValueError) as raised:
        canonical_enum_spelling(case["kind"], case["value"])
    _assert_case(raised.value, case_id)


def test_aliases_resolve_to_their_canonical_spelling() -> None:
    assert canonical_enum_spelling("manager_lookup", "exact") == "eq"
    assert canonical_enum_spelling("manager_lookup", "like") == "regex"
    assert canonical_enum_spelling("owner_lookup", "exact") == "eq"
    assert canonical_enum_spelling("value_type", "datetime-tz") == "datetime-tz"


def _registry() -> _QueryDescriptorRegistry:
    registry = _QueryDescriptorRegistry()
    registry.register_entity(
        {
            "type_name": "person",
            "is_abstract": False,
            "parent_type": None,
            "owned_attributes": [
                {
                    "field_name": "name",
                    "attr_name": "person-name",
                    "value_type": "string",
                    "annotations": ["Key"],
                    "is_optional": False,
                    "is_ordered": False,
                }
            ],
        }
    )
    return registry


def _calls() -> dict[str, Callable[[], object]]:
    session = MatchSessionHandle(_registry())
    person = session.exact("person")
    name = person.field("name")
    query = session.query(session.positional([person.one()]))
    return {
        "comparison_operator.unknown": lambda: name.compare_field("equals", name),
        "sort_direction.unknown": lambda: name.order("desc", "reject"),
        "missing_order.unknown": lambda: name.order("ascending", "none"),
        "row_cardinality.unknown": lambda: query.fetch_rows_diagnostic([], 0, 1, "many"),
        "reducer.unknown": lambda: query.reduce_by_diagnostic(person, None, ["variance"], [None]),
    }


@pytest.mark.parametrize("case_id", sorted(_calls()))
def test_binding_calls_raise_the_corpus_diagnostic(case_id: str) -> None:
    with pytest.raises(UnknownEnumValueError) as raised:
        _calls()[case_id]()
    _assert_case(raised.value, case_id)
