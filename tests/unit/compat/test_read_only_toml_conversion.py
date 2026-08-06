"""Frozen acceptance for the retained read-only TOML converter."""

from __future__ import annotations

import warnings
from pathlib import Path

import pytest

pytest.importorskip("type_bridge_core")

from type_bridge_core import toml_to_typeql

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
def test_frozen_toml_conversion_reproduces_exact_typeql(stem: str) -> None:
    source = (FIXTURES / f"{stem}.toml").read_text(encoding="utf-8")
    expected = (FIXTURES / f"{stem}.tql").read_text(encoding="utf-8")
    assert toml_to_typeql(source) == expected


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
    assert first.startswith("define\n")


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
