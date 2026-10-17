"""Frozen acceptance for the retained read-only TOML converter."""

from __future__ import annotations

import warnings
from pathlib import Path

import pytest

pytest.importorskip("type_bridge_core")

from type_bridge_core import _typeql_queries_equivalent, toml_to_typeql

FIXTURES = Path(__file__).parents[1] / "generator" / "fixtures"
EXACT_PAIRS = (
    "annotations_inheritance",
    "attributes_owns",
    "bookstore_corpus",
    "functions_structs",
    "relations_roles",
    "role_cardinality",
    "social_media",
)
ALL_TOML = tuple(sorted(FIXTURES.glob("*.toml")))


@pytest.mark.parametrize("stem", EXACT_PAIRS)
def test_toml_conversion_preserves_fixture_schema_ast(stem: str) -> None:
    source = (FIXTURES / f"{stem}.toml").read_text(encoding="utf-8")
    expected = (FIXTURES / f"{stem}.tql").read_text(encoding="utf-8")
    if stem == "functions_structs":
        # Keep the released fixture bytes; correct its known old struct grammar
        # only in the comparison input for the approved TypeQL 3 syntax fix.
        expected = expected.replace(
            "struct player-stats, value wins integer, value losses integer, value nickname string?;",
            "struct player-stats:wins value integer, losses value integer, nickname value string?;",
        )
    # Approved old-invalid-output correction: long/int/bool are TOML aliases,
    # not TypeDB builtin domains. Preserve the released fixture file itself.
    expected = expected.replace("value long", "value integer")
    expected = expected.replace("value int;", "value integer;")
    expected = expected.replace("value bool;", "value boolean;")
    assert _typeql_queries_equivalent(toml_to_typeql(source), expected)


@pytest.mark.parametrize("source_path", ALL_TOML, ids=lambda path: path.stem)
def test_frozen_toml_conversion_is_deterministic_and_warning_free(
    source_path: Path,
) -> None:
    source = source_path.read_text(encoding="utf-8")
    with warnings.catch_warnings():
        warnings.simplefilter("error")
        first = toml_to_typeql(source)
        second = toml_to_typeql(source)
    assert first == second
    assert first.startswith("define ")


@pytest.mark.parametrize(
    ("source", "message"),
    (
        ('[attributes.name]\nvaleu = "string"\n', "valeu"),
        ('[attributes.name]\nvalue = "strng"\n', "strng"),
        ('[attributes.child]\nsub = "missing"\n', "missing"),
        (
            '[entities.person]\nplays = [{ relation = "missing", role = "member" }]\n',
            "missing",
        ),
        ("[[not valid toml ][", "TOML"),
    ),
)
def test_frozen_toml_conversion_rejects_invalid_input(source: str, message: str) -> None:
    with pytest.raises(ValueError, match=message):
        toml_to_typeql(source)
