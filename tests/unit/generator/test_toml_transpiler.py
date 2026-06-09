# pyright: reportMissingImports=false
"""Smoke tests for the PyO3 toml_to_typeql binding (no DB required)."""

from __future__ import annotations

import pytest

pytest.importorskip("type_bridge_core")

from type_bridge_core import TypeSchema, toml_to_typeql

from type_bridge.generator.models import Cardinality
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

    def test_abstract_subtype_heads_reach_rust_schema(self) -> None:
        """TOML abstract subtypes must emit both tokens and parse through Rust TypeSchema."""
        toml_text = """
[attributes.payload]
value = "string"

[attributes.text-payload]
abstract = true
sub = "payload"

[entities.content]
abstract = true

[entities.page]
abstract = true
sub = "content"

[relations.interaction]
abstract = true

[relations.content-engagement]
abstract = true
sub = "interaction"
"""
        typeql = toml_to_typeql(toml_text)
        assert "attribute text-payload @abstract, sub payload;" in typeql
        assert "entity page @abstract, sub content;" in typeql
        assert "relation content-engagement @abstract, sub interaction;" in typeql

        schema = TypeSchema.from_typeql(typeql)
        assert schema.attributes["text-payload"]["is_abstract"] is True
        assert schema.attributes["text-payload"]["parent"] == "payload"
        assert schema.entities["page"]["is_abstract"] is True
        assert schema.entities["page"]["parent"] == "content"
        assert schema.relations["content-engagement"]["is_abstract"] is True
        assert schema.relations["content-engagement"]["parent"] == "interaction"

    def test_entity_plays_cardinality_reaches_generator_parser(self) -> None:
        """Entity plays card emits @card and populates ParsedSchema.plays_cardinalities."""
        toml_text = """
[entities.post]
plays = [
    { relation = "posting", role = "post", card = "1" },
    { relation = "reaction", role = "parent", card = "0..5" },
    { relation = "commenting", role = "parent", card = "1.." },
]

[relations.posting]
roles = [{ name = "post" }]

[relations.reaction]
roles = [{ name = "parent" }]

[relations.commenting]
roles = [{ name = "parent" }]
"""
        typeql = toml_to_typeql(toml_text)
        assert "plays posting:post @card(1)" in typeql
        assert "plays reaction:parent @card(0..5)" in typeql
        assert "plays commenting:parent @card(1..)" in typeql

        schema = parse_tql_schema(typeql)
        post = schema.entities["post"]
        assert post.plays_cardinalities["posting:post"] == Cardinality(1, 1)
        assert post.plays_cardinalities["reaction:parent"] == Cardinality(0, 5)
        assert post.plays_cardinalities["commenting:parent"] == Cardinality(1, None)

    def test_relation_plays_reaches_rust_schema(self) -> None:
        """Relation-level plays must survive through Rust TypeSchema, not only TypeQL text."""
        toml_text = """
[relations.publication]
plays = [
    { relation = "contribution", role = "work" },
    { relation = "review", role = "reviewed", card = "0..5" },
]

[relations.contribution]
roles = [{ name = "work" }]

[relations.review]
roles = [{ name = "reviewed" }]
"""
        typeql = toml_to_typeql(toml_text)
        assert (
            "relation publication, plays contribution:work, plays review:reviewed @card(0..5);"
        ) in typeql

        schema = TypeSchema.from_typeql(typeql)
        plays = {
            entry["role_ref"]: entry["cardinality"]
            for entry in schema.get_all_plays_roles("publication")
        }
        assert plays["contribution:work"] is None
        assert plays["review:reviewed"] == {"min": 0, "max": 5}


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
