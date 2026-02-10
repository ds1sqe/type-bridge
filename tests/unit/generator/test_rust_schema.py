"""Tests for the Rust TypeSchema (type_bridge_core.TypeSchema).

These tests verify that the Rust parser + inheritance resolution produce
equivalent results to the Python parser, and that the PyO3 bindings work
correctly.
"""

from __future__ import annotations

import json
from typing import Any

import pytest

HAS_RUST_CORE = False
TypeSchema: Any = None
try:
    from type_bridge_core import TypeSchema  # type: ignore[no-redef]

    HAS_RUST_CORE = True
except ImportError:
    pass

pytestmark = pytest.mark.skipif(not HAS_RUST_CORE, reason="Rust core not available")


# ---------------------------------------------------------------------------
# Parsing basics
# ---------------------------------------------------------------------------


class TestRustSchemaBasicParsing:
    """Basic parsing through the Rust core."""

    def test_simple_attribute(self) -> None:
        schema = TypeSchema.from_typeql("define\nattribute name, value string;")
        assert "name" in schema.attributes
        attr = schema.attributes["name"]
        assert attr["name"] == "name"
        assert attr["value_type"] == "string"
        assert attr["parent"] is None
        assert attr["is_abstract"] is False

    def test_attribute_all_value_types(self) -> None:
        tql = "\n".join(
            [
                "define",
                "attribute a1, value string;",
                "attribute a2, value long;",
                "attribute a3, value double;",
                "attribute a4, value boolean;",
                "attribute a5, value datetime;",
                "attribute a6, value datetime-tz;",
                "attribute a7, value date;",
                "attribute a8, value decimal;",
                "attribute a9, value duration;",
            ]
        )
        schema = TypeSchema.from_typeql(tql)
        expected = {
            "a1": "string",
            "a2": "long",
            "a3": "double",
            "a4": "boolean",
            "a5": "datetime",
            "a6": "datetime-tz",
            "a7": "date",
            "a8": "decimal",
            "a9": "duration",
        }
        for name, vt in expected.items():
            assert schema.attributes[name]["value_type"] == vt

    def test_attribute_with_regex(self) -> None:
        schema = TypeSchema.from_typeql(
            'define\nattribute email, value string, @regex("^[a-z]+@[a-z]+\\.[a-z]+$");'
        )
        attr = schema.attributes["email"]
        assert attr["regex"] == "^[a-z]+@[a-z]+\\.[a-z]+$"

    def test_attribute_with_values(self) -> None:
        schema = TypeSchema.from_typeql(
            'define\nattribute status, value string, @values("active", "inactive");'
        )
        attr = schema.attributes["status"]
        assert attr["allowed_values"] == ["active", "inactive"]

    def test_simple_entity(self) -> None:
        schema = TypeSchema.from_typeql(
            "define\nattribute name, value string;\nentity person, owns name;"
        )
        assert "person" in schema.entities
        ent = schema.entities["person"]
        assert ent["name"] == "person"
        assert len(ent["owns"]) == 1
        assert ent["owns"][0]["name"] == "name"

    def test_entity_with_key(self) -> None:
        schema = TypeSchema.from_typeql(
            "define\nattribute email, value string;\nentity user, owns email @key;"
        )
        ent = schema.entities["user"]
        assert ent["owns"][0]["is_key"] is True

    def test_entity_with_plays(self) -> None:
        schema = TypeSchema.from_typeql("define\nentity person, plays employment:employee;")
        ent = schema.entities["person"]
        assert len(ent["plays"]) == 1
        assert ent["plays"][0]["role_ref"] == "employment:employee"

    def test_simple_relation(self) -> None:
        schema = TypeSchema.from_typeql(
            "define\nrelation employment, relates employee, relates employer;"
        )
        assert "employment" in schema.relations
        rel = schema.relations["employment"]
        assert len(rel["roles"]) == 2
        role_names = {r["name"] for r in rel["roles"]}
        assert role_names == {"employee", "employer"}

    def test_abstract_entity(self) -> None:
        schema = TypeSchema.from_typeql("define\nentity base-entity @abstract;")
        assert schema.is_abstract("base-entity")

    def test_not_abstract(self) -> None:
        schema = TypeSchema.from_typeql("define\nentity person;")
        assert not schema.is_abstract("person")

    def test_empty_input_is_valid(self) -> None:
        # An empty string (no define block) parses to an empty schema
        schema = TypeSchema.from_typeql("")
        assert schema.entities == {}
        assert schema.relations == {}
        assert schema.attributes == {}


# ---------------------------------------------------------------------------
# Inheritance resolution
# ---------------------------------------------------------------------------


class TestRustSchemaInheritance:
    """Verify that inheritance resolution works through the Rust core."""

    def test_entity_inherits_owns(self) -> None:
        tql = "\n".join(
            [
                "define",
                "attribute name, value string;",
                "attribute email, value string;",
                "entity base-entity @abstract, owns name;",
                "entity person sub base-entity, owns email;",
            ]
        )
        schema = TypeSchema.from_typeql(tql)
        owned = schema.get_all_owned_attributes("person")
        owned_names = {a["name"] for a in owned}
        assert "name" in owned_names
        assert "email" in owned_names

    def test_entity_inherits_plays(self) -> None:
        tql = "\n".join(
            [
                "define",
                "entity base-entity @abstract, plays employment:employee;",
                "entity person sub base-entity;",
            ]
        )
        schema = TypeSchema.from_typeql(tql)
        plays = schema.get_all_plays_roles("person")
        assert len(plays) == 1
        assert plays[0]["role_ref"] == "employment:employee"

    def test_relation_inherits_roles(self) -> None:
        tql = "\n".join(
            [
                "define",
                "relation base-rel @abstract, relates member;",
                "relation group-membership sub base-rel, relates group;",
            ]
        )
        schema = TypeSchema.from_typeql(tql)
        roles = schema.get_all_relates("group-membership")
        role_names = {r["name"] for r in roles}
        assert "member" in role_names
        assert "group" in role_names

    def test_relation_role_override(self) -> None:
        tql = "\n".join(
            [
                "define",
                "relation base-rel @abstract, relates member;",
                "relation friendship sub base-rel, relates friend as member;",
            ]
        )
        schema = TypeSchema.from_typeql(tql)
        roles = schema.get_all_relates("friendship")
        role_names = {r["name"] for r in roles}
        assert "friend" in role_names
        assert "member" not in role_names


# ---------------------------------------------------------------------------
# Query methods
# ---------------------------------------------------------------------------


class TestRustSchemaQueryMethods:
    """Test the query API exposed to Python."""

    BOOKSTORE = "\n".join(
        [
            "define",
            "attribute title, value string;",
            "attribute isbn, value string;",
            "attribute name, value string;",
            "entity book, owns title, owns isbn @key;",
            "entity author, owns name @key;",
            "relation authorship, relates book-role, relates author-role;",
        ]
    )

    def test_get_all_owned_attributes(self) -> None:
        schema = TypeSchema.from_typeql(self.BOOKSTORE)
        attrs = schema.get_all_owned_attributes("book")
        names = {a["name"] for a in attrs}
        assert names == {"title", "isbn"}

    def test_get_all_owned_attributes_nonexistent(self) -> None:
        schema = TypeSchema.from_typeql(self.BOOKSTORE)
        assert schema.get_all_owned_attributes("nonexistent") == []

    def test_get_all_plays_roles(self) -> None:
        tql = "\n".join(
            [
                "define",
                "entity person, plays employment:employee;",
                "relation employment, relates employee;",
            ]
        )
        schema = TypeSchema.from_typeql(tql)
        plays = schema.get_all_plays_roles("person")
        assert len(plays) == 1

    def test_get_all_relates(self) -> None:
        schema = TypeSchema.from_typeql(self.BOOKSTORE)
        roles = schema.get_all_relates("authorship")
        assert len(roles) == 2

    def test_is_abstract_true(self) -> None:
        schema = TypeSchema.from_typeql("define\nentity abstract-entity @abstract;")
        assert schema.is_abstract("abstract-entity") is True

    def test_is_abstract_false(self) -> None:
        schema = TypeSchema.from_typeql("define\nentity concrete;")
        assert schema.is_abstract("concrete") is False

    def test_is_abstract_nonexistent(self) -> None:
        schema = TypeSchema.from_typeql("define\nentity x;")
        assert schema.is_abstract("nope") is False


# ---------------------------------------------------------------------------
# JSON serialization
# ---------------------------------------------------------------------------


class TestRustSchemaJsonRoundTrip:
    """Test JSON serialization round-trip through Python."""

    def test_json_round_trip(self) -> None:
        tql = "\n".join(
            [
                "define",
                "attribute name, value string;",
                "entity person, owns name;",
            ]
        )
        schema = TypeSchema.from_typeql(tql)
        json_str = schema.to_json()
        schema2 = TypeSchema.from_json(json_str)
        assert schema.entities == schema2.entities
        assert schema.attributes == schema2.attributes

    def test_to_json_is_valid_json(self) -> None:
        schema = TypeSchema.from_typeql("define\nattribute x, value long;\nentity y, owns x;")
        parsed = json.loads(schema.to_json())
        assert "entities" in parsed
        assert "relations" in parsed
        assert "attributes" in parsed


# ---------------------------------------------------------------------------
# Error handling
# ---------------------------------------------------------------------------


class TestRustSchemaErrors:
    """Test that errors are properly raised as Python exceptions."""

    def test_parse_error_no_define(self) -> None:
        with pytest.raises(ValueError, match="[Pp]arse"):
            TypeSchema.from_typeql("entity person;")

    def test_invalid_json(self) -> None:
        with pytest.raises(ValueError):
            TypeSchema.from_json("{invalid json")


# ---------------------------------------------------------------------------
# Complex schema (mimics real-world usage)
# ---------------------------------------------------------------------------


class TestRustSchemaComplex:
    """Complex schema parsing and resolution."""

    COMPLEX = "\n".join(
        [
            "define",
            "attribute name, value string;",
            "attribute email, value string;",
            "attribute age, value long;",
            "attribute title, value string;",
            "attribute isbn, value string;",
            "attribute price, value double;",
            "",
            "entity base-entity @abstract, owns name;",
            "entity person sub base-entity, owns email @key, owns age;",
            "entity author sub person;",
            "",
            "entity book, owns title, owns isbn @key, owns price;",
            "",
            "relation authorship,",
            "    relates written-by,",
            "    relates written-work;",
        ]
    )

    def test_parse_complex(self) -> None:
        schema = TypeSchema.from_typeql(self.COMPLEX)
        assert len(schema.entities) == 4
        assert len(schema.attributes) == 6
        assert len(schema.relations) == 1

    def test_multi_level_inheritance(self) -> None:
        schema = TypeSchema.from_typeql(self.COMPLEX)
        # author inherits from person which inherits from base-entity
        author_owns = schema.get_all_owned_attributes("author")
        own_names = {a["name"] for a in author_owns}
        # Should have: name (from base-entity), email + age (from person)
        assert "name" in own_names
        assert "email" in own_names
        assert "age" in own_names

    def test_comments_and_optional_commas(self) -> None:
        tql = "\n".join(
            [
                "define",
                "# This is a comment",
                "attribute name, value string;",
                "// Another comment",
                "entity person",
                "    owns name;  # inline comment",
            ]
        )
        schema = TypeSchema.from_typeql(tql)
        assert "person" in schema.entities
        assert "name" in schema.attributes

    def test_function_and_struct_skipped(self) -> None:
        tql = "\n".join(
            [
                "define",
                "attribute name, value string;",
                "entity person, owns name;",
                "fun get_name($p: person) -> name:",
                "    return $p.name;",
                "struct result { name: name };",
            ]
        )
        schema = TypeSchema.from_typeql(tql)
        assert "person" in schema.entities
        assert "name" in schema.attributes
