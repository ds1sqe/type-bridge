"""Python half of ``tests/contracts/diagnostic-json.json``.

A native exception carries one attribute per key of the core's diagnostic
JSON form, so the attributes rebuild that JSON exactly as Node's native
error message and the C diagnostics JSON give it. The one marshalling step:
``count``, ``byte_count``, and ``signed`` detail values are Python ``int``
on the exception and decimal strings in the JSON form.
"""

import json
from pathlib import Path

import pytest
from type_bridge_core import (
    MatchRequestError,
    MatchSessionHandle,
    QueryV2Authority,
    QueryV2Error,
    canonical_enum_spelling,
)

from tests.unit.typed_query.test_diagnostic_golden import _registry

GOLDEN = json.loads(
    (Path(__file__).parents[3] / "tests" / "contracts" / "diagnostic-json.json").read_text()
)
SDK_KEYS = (
    "version",
    "category",
    "sdk_category",
    "query_category",
    "code",
    "message",
    "path",
    "details",
)
CONTRACT_KEYS = ("category", "code", "message", "path", "details")


def _expected(case_id: str) -> dict[str, object]:
    return next(case["diagnostic"] for case in GOLDEN["cases"] if case["id"] == case_id)


INTEGER_DETAIL_KINDS = frozenset({"count", "byte_count", "signed"})


def _json_details(details: dict[str, dict[str, object]]) -> dict[str, dict[str, object]]:
    """Rebuild the JSON form of exception ``details``: integers become decimal strings."""
    form = {}
    for name, detail in details.items():
        if detail["kind"] in INTEGER_DETAIL_KINDS:
            assert type(detail["value"]) is int, (name, detail)
            detail = {**detail, "value": str(detail["value"])}
        form[name] = detail
    return form


def _form(error: BaseException, keys: tuple[str, ...]) -> dict[str, object]:
    form = {key: getattr(error, key) for key in keys}
    if isinstance(error, MatchRequestError):
        form["details"] = _json_details(form["details"])
    return form


def test_group_key_cap_is_the_golden_sdk_json() -> None:
    session = MatchSessionHandle(_registry())
    person = session.exact("person")
    query = session.query(session.positional([person.one()]))
    with pytest.raises(MatchRequestError) as raised:
        query.check_group_fields([person.field("name")] * 17)
    assert _form(raised.value, SDK_KEYS) == _expected("group_keys.seventeen")


def test_unknown_enum_value_is_the_golden_sdk_json() -> None:
    with pytest.raises(MatchRequestError) as raised:
        canonical_enum_spelling("sort_direction", "sideways")
    assert _form(raised.value, SDK_KEYS) == _expected("unknown_enum_value.sort_direction")


def test_malformed_schema_authority_is_the_golden_contract_json() -> None:
    with pytest.raises(QueryV2Error) as raised:
        QueryV2Authority(b"{", "scope", "profile")
    assert _form(raised.value, CONTRACT_KEYS) == _expected("contract.malformed_canonical_json")
