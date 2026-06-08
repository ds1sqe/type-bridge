# pyright: reportMissingImports=false
"""Smoke tests for the PyO3 toml_to_typeql binding (no DB required)."""

from __future__ import annotations

import pytest

pytest.importorskip("type_bridge_core")

from type_bridge_core import toml_to_typeql

from type_bridge.generator.parser import parse_tql_schema

# ---------------------------------------------------------------------------
# Fixture: the attribute+owns authoring slice
# ---------------------------------------------------------------------------

_SLICE_TOML = """\
[attributes.name]
value = "string"

[attributes.age]
value = "long"

[entities.person]
owns = ["name", "age"]
"""


# ---------------------------------------------------------------------------
# Happy-path: PyO3 boundary + parser acceptance
# ---------------------------------------------------------------------------


class TestTomlToTypeqlBinding:
    """Verify the PyO3 wrapper calls through to the Rust transpiler correctly."""

    def test_returns_string(self) -> None:
        result = toml_to_typeql(_SLICE_TOML)
        assert isinstance(result, str)

    def test_emitted_typeql_contains_define(self) -> None:
        result = toml_to_typeql(_SLICE_TOML)
        assert "define" in result

    def test_emitted_typeql_contains_attributes(self) -> None:
        result = toml_to_typeql(_SLICE_TOML)
        assert "attribute name" in result
        assert "attribute age" in result

    def test_emitted_typeql_contains_entity_owns(self) -> None:
        result = toml_to_typeql(_SLICE_TOML)
        assert "entity person" in result
        assert "owns name" in result
        assert "owns age" in result

    def test_parse_tql_schema_accepts_transpiled_output(self) -> None:
        """PyO3 boundary + parser: emitted TypeQL must parse without error."""
        typeql = toml_to_typeql(_SLICE_TOML)
        schema = parse_tql_schema(typeql)
        assert schema is not None

    def test_parsed_schema_has_expected_attributes(self) -> None:
        typeql = toml_to_typeql(_SLICE_TOML)
        schema = parse_tql_schema(typeql)
        assert "name" in schema.attributes
        assert "age" in schema.attributes
        assert schema.attributes["name"].value_type == "string"
        assert schema.attributes["age"].value_type == "long"

    def test_parsed_schema_has_expected_entity(self) -> None:
        typeql = toml_to_typeql(_SLICE_TOML)
        schema = parse_tql_schema(typeql)
        assert "person" in schema.entities
        person = schema.entities["person"]
        assert "name" in person.owns
        assert "age" in person.owns

    def test_person_owns_both_attributes(self) -> None:
        """Entity 'person' must own both 'name' and 'age' — end-to-end proof."""
        typeql = toml_to_typeql(_SLICE_TOML)
        schema = parse_tql_schema(typeql)
        person = schema.entities["person"]
        assert set(person.owns) >= {"name", "age"}


# ---------------------------------------------------------------------------
# Error path: malformed TOML raises ValueError (TranspileError → PyValueError)
# ---------------------------------------------------------------------------


class TestTomlToTypeqlErrors:
    """Verify that parse failures surface as Python ValueError."""

    def test_malformed_toml_raises_value_error(self) -> None:
        with pytest.raises(ValueError):
            toml_to_typeql("[[not valid toml ][")

    def test_unknown_key_raises_value_error(self) -> None:
        bad_toml = '[attributes.name]\nvaleu = "string"\n'
        with pytest.raises(ValueError):
            toml_to_typeql(bad_toml)

    def test_empty_string_does_not_crash(self) -> None:
        # Empty TOML is valid TOML (empty document); should return a valid define block
        result = toml_to_typeql("")
        assert isinstance(result, str)
