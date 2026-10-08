"""Python half of ``tests/contracts/diagnostic-json-v1.json``.

A native exception carries one attribute per key of the core's diagnostic
JSON form, so the attributes rebuild that JSON exactly as Node's native
error message and the C diagnostics JSON give it.
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
    (Path(__file__).parents[3] / "tests" / "contracts" / "diagnostic-json-v1.json").read_text()
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


def _form(error: BaseException, keys: tuple[str, ...]) -> dict[str, object]:
    return {key: getattr(error, key) for key in keys}


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
