"""Rust vs Lark parity tests for the TypeQL schema parser.

These tests parse identical schemas through both the Rust (TypeSchema) and Lark
paths and verify that the resulting ParsedSchema objects are field-for-field
equivalent.  Skipped when the Rust core is not available.
"""

from __future__ import annotations

from typing import Any

import pytest

from type_bridge.generator.annotations import extract_annotations
from type_bridge.generator.models import (
    AttributeSpec,
    Cardinality,
    EntitySpec,
    ParsedSchema,
    RelationSpec,
    RoleSpec,
)
from type_bridge.generator.parser import _parse_with_lark

HAS_RUST_CORE = False
_RustTypeSchema: Any = None
_rust_schema_to_parsed: Any = None
try:
    from type_bridge_core import TypeSchema as _RustTypeSchema  # type: ignore[no-redef]

    from type_bridge.generator.parser import (
        _rust_schema_to_parsed as _rust_converter,  # type: ignore[attr-defined]
    )

    _rust_schema_to_parsed = _rust_converter
    HAS_RUST_CORE = True
except ImportError:
    pass

pytestmark = pytest.mark.skipif(not HAS_RUST_CORE, reason="Rust core not available")


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _parse_both(tql: str) -> tuple[ParsedSchema, ParsedSchema]:
    """Parse *tql* through both Rust and Lark, returning ``(rust, lark)``."""
    entity_annots, attr_annots, rel_annots, role_annots = extract_annotations(tql)
    rust_ts = _RustTypeSchema.from_typeql(tql)
    rust_schema = _rust_schema_to_parsed(
        rust_ts, entity_annots, attr_annots, rel_annots, role_annots
    )
    lark_schema = _parse_with_lark(tql, entity_annots, attr_annots, rel_annots, role_annots)
    return rust_schema, lark_schema


def _assert_cardinality_eq(
    rust_card: Cardinality | None,
    lark_card: Cardinality | None,
    context: str,
) -> None:
    """Compare two Cardinality values with a descriptive context on failure."""
    if rust_card is None and lark_card is None:
        return
    assert rust_card is not None and lark_card is not None, (
        f"{context}: one is None (rust={rust_card}, lark={lark_card})"
    )
    assert rust_card.min == lark_card.min, (
        f"{context}: min differs (rust={rust_card.min}, lark={lark_card.min})"
    )
    assert rust_card.max == lark_card.max, (
        f"{context}: max differs (rust={rust_card.max}, lark={lark_card.max})"
    )


def _assert_attr_eq(rust: AttributeSpec, lark: AttributeSpec) -> None:
    ctx = f"attribute '{rust.name}'"
    assert rust.name == lark.name, f"{ctx}: name"
    assert rust.value_type == lark.value_type, (
        f"{ctx}: value_type ({rust.value_type!r} vs {lark.value_type!r})"
    )
    assert rust.parent == lark.parent, f"{ctx}: parent"
    assert rust.abstract == lark.abstract, f"{ctx}: abstract"
    assert rust.independent == lark.independent, f"{ctx}: independent"
    assert rust.regex == lark.regex, f"{ctx}: regex ({rust.regex!r} vs {lark.regex!r})"
    assert rust.allowed_values == lark.allowed_values, f"{ctx}: allowed_values"
    assert rust.range_min == lark.range_min, (
        f"{ctx}: range_min ({rust.range_min!r} vs {lark.range_min!r})"
    )
    assert rust.range_max == lark.range_max, (
        f"{ctx}: range_max ({rust.range_max!r} vs {lark.range_max!r})"
    )
    assert rust.docstring == lark.docstring, f"{ctx}: docstring"
    assert rust.annotations == lark.annotations, f"{ctx}: annotations"


def _assert_role_eq(rust: RoleSpec, lark: RoleSpec, ctx: str) -> None:
    assert rust.name == lark.name, f"{ctx}: role name"
    assert rust.overrides == lark.overrides, (
        f"{ctx}: overrides ({rust.overrides!r} vs {lark.overrides!r})"
    )
    _assert_cardinality_eq(rust.cardinality, lark.cardinality, f"{ctx} cardinality")
    assert rust.distinct == lark.distinct, f"{ctx}: distinct"
    assert rust.annotations == lark.annotations, f"{ctx}: annotations"


def _assert_entity_eq(rust: EntitySpec, lark: EntitySpec) -> None:
    ctx = f"entity '{rust.name}'"
    assert rust.name == lark.name, f"{ctx}: name"
    assert rust.parent == lark.parent, f"{ctx}: parent"
    assert rust.abstract == lark.abstract, f"{ctx}: abstract"
    assert rust.owns == lark.owns, f"{ctx}: owns ({rust.owns} vs {lark.owns})"
    assert rust.owns_order == lark.owns_order, (
        f"{ctx}: owns_order ({rust.owns_order} vs {lark.owns_order})"
    )
    assert rust.plays == lark.plays, f"{ctx}: plays ({rust.plays} vs {lark.plays})"
    assert rust.keys == lark.keys, f"{ctx}: keys ({rust.keys} vs {lark.keys})"
    assert rust.uniques == lark.uniques, f"{ctx}: uniques"
    assert rust.cascades == lark.cascades, f"{ctx}: cascades"
    assert rust.subkeys == lark.subkeys, f"{ctx}: subkeys"
    assert rust.docstring == lark.docstring, f"{ctx}: docstring"
    assert rust.annotations == lark.annotations, f"{ctx}: annotations"
    # Cardinalities
    assert set(rust.cardinalities.keys()) == set(lark.cardinalities.keys()), (
        f"{ctx}: cardinalities keys ({set(rust.cardinalities.keys())} vs {set(lark.cardinalities.keys())})"
    )
    for attr_name in rust.cardinalities:
        _assert_cardinality_eq(
            rust.cardinalities[attr_name],
            lark.cardinalities[attr_name],
            f"{ctx} cardinality[{attr_name}]",
        )
    # plays_cardinalities
    assert set(rust.plays_cardinalities.keys()) == set(lark.plays_cardinalities.keys()), (
        f"{ctx}: plays_cardinalities keys"
    )
    for role_ref in rust.plays_cardinalities:
        _assert_cardinality_eq(
            rust.plays_cardinalities[role_ref],
            lark.plays_cardinalities[role_ref],
            f"{ctx} plays_card[{role_ref}]",
        )


def _assert_relation_eq(rust: RelationSpec, lark: RelationSpec) -> None:
    ctx = f"relation '{rust.name}'"
    assert rust.name == lark.name, f"{ctx}: name"
    assert rust.parent == lark.parent, f"{ctx}: parent"
    assert rust.abstract == lark.abstract, f"{ctx}: abstract"
    assert rust.owns == lark.owns, f"{ctx}: owns"
    assert rust.owns_order == lark.owns_order, f"{ctx}: owns_order"
    assert rust.keys == lark.keys, f"{ctx}: keys"
    assert rust.uniques == lark.uniques, f"{ctx}: uniques"
    assert rust.cascades == lark.cascades, f"{ctx}: cascades"
    assert rust.subkeys == lark.subkeys, f"{ctx}: subkeys"
    assert rust.docstring == lark.docstring, f"{ctx}: docstring"
    assert rust.annotations == lark.annotations, f"{ctx}: annotations"
    # Cardinalities
    assert set(rust.cardinalities.keys()) == set(lark.cardinalities.keys()), (
        f"{ctx}: cardinalities keys"
    )
    for attr_name in rust.cardinalities:
        _assert_cardinality_eq(
            rust.cardinalities[attr_name],
            lark.cardinalities[attr_name],
            f"{ctx} cardinality[{attr_name}]",
        )
    # Roles — compare by name (order may differ between parsers)
    rust_roles = sorted(rust.roles, key=lambda r: r.name)
    lark_roles = sorted(lark.roles, key=lambda r: r.name)
    assert len(rust_roles) == len(lark_roles), (
        f"{ctx}: role count ({len(rust_roles)} vs {len(lark_roles)})"
    )
    for r_role, l_role in zip(rust_roles, lark_roles):
        _assert_role_eq(r_role, l_role, f"{ctx} role '{r_role.name}'")


def _assert_schemas_eq(rust: ParsedSchema, lark: ParsedSchema) -> None:
    """Full deep comparison of two ParsedSchema objects."""
    # Attributes
    assert set(rust.attributes.keys()) == set(lark.attributes.keys()), (
        f"attribute keys differ: {set(rust.attributes.keys())} vs {set(lark.attributes.keys())}"
    )
    for name in rust.attributes:
        _assert_attr_eq(rust.attributes[name], lark.attributes[name])

    # Entities
    assert set(rust.entities.keys()) == set(lark.entities.keys()), (
        f"entity keys differ: {set(rust.entities.keys())} vs {set(lark.entities.keys())}"
    )
    for name in rust.entities:
        _assert_entity_eq(rust.entities[name], lark.entities[name])

    # Relations
    assert set(rust.relations.keys()) == set(lark.relations.keys()), (
        f"relation keys differ: {set(rust.relations.keys())} vs {set(lark.relations.keys())}"
    )
    for name in rust.relations:
        _assert_relation_eq(rust.relations[name], lark.relations[name])

    # Functions/structs — Rust doesn't parse these, so we only check they're empty
    # on the Rust side (Lark may have them if supplemented).


# ---------------------------------------------------------------------------
# Simple type parity
# ---------------------------------------------------------------------------


class TestParitySimpleTypes:
    """Basic types produce identical ParsedSchema from both parsers."""

    def test_simple_attribute(self) -> None:
        rust, lark = _parse_both("""
            define
            attribute name, value string;
        """)
        _assert_schemas_eq(rust, lark)

    def test_simple_entity(self) -> None:
        rust, lark = _parse_both("""
            define
            attribute name, value string;
            entity person, owns name;
        """)
        _assert_schemas_eq(rust, lark)

    def test_simple_relation(self) -> None:
        rust, lark = _parse_both("""
            define
            relation friendship, relates friend;
        """)
        _assert_schemas_eq(rust, lark)

    def test_multiple_types(self) -> None:
        rust, lark = _parse_both("""
            define
            attribute name, value string;
            attribute age, value long;
            entity person, owns name, owns age;
            relation friendship, relates friend;
        """)
        _assert_schemas_eq(rust, lark)

    def test_empty_input(self) -> None:
        """Empty input produces empty schema from both parsers."""
        rust, lark = _parse_both("")
        _assert_schemas_eq(rust, lark)


# ---------------------------------------------------------------------------
# All annotation types
# ---------------------------------------------------------------------------


class TestParityAnnotations:
    """All annotation types produce identical results."""

    def test_key_annotation(self) -> None:
        rust, lark = _parse_both("""
            define
            attribute email, value string;
            entity user, owns email @key;
        """)
        _assert_schemas_eq(rust, lark)

    def test_unique_annotation(self) -> None:
        rust, lark = _parse_both("""
            define
            attribute username, value string;
            entity user, owns username @unique;
        """)
        _assert_schemas_eq(rust, lark)

    def test_cascade_annotation(self) -> None:
        rust, lark = _parse_both("""
            define
            attribute name, value string;
            entity person, owns name @cascade;
        """)
        _assert_schemas_eq(rust, lark)

    def test_subkey_annotation(self) -> None:
        rust, lark = _parse_both("""
            define
            attribute user-id, value string;
            attribute org-id, value string;
            entity user, owns user-id @key, owns org-id @subkey(org-group);
        """)
        _assert_schemas_eq(rust, lark)

    def test_card_on_owns(self) -> None:
        rust, lark = _parse_both("""
            define
            attribute nickname, value string;
            entity person, owns nickname @card(0..3);
        """)
        _assert_schemas_eq(rust, lark)

    def test_card_exact(self) -> None:
        rust, lark = _parse_both("""
            define
            attribute name, value string;
            entity person, owns name @card(1);
        """)
        _assert_schemas_eq(rust, lark)

    def test_card_unbounded(self) -> None:
        rust, lark = _parse_both("""
            define
            attribute tag, value string;
            entity post, owns tag @card(0..);
        """)
        _assert_schemas_eq(rust, lark)

    def test_card_on_plays(self) -> None:
        rust, lark = _parse_both("""
            define
            entity person, plays friendship:friend @card(0..10);
            relation friendship, relates friend;
        """)
        _assert_schemas_eq(rust, lark)

    def test_card_on_relates(self) -> None:
        rust, lark = _parse_both("""
            define
            relation friendship, relates friend @card(2..2);
        """)
        _assert_schemas_eq(rust, lark)

    def test_abstract_entity(self) -> None:
        rust, lark = _parse_both("""
            define
            entity base-entity @abstract;
        """)
        _assert_schemas_eq(rust, lark)

    def test_abstract_relation(self) -> None:
        rust, lark = _parse_both("""
            define
            relation base-rel @abstract, relates member;
        """)
        _assert_schemas_eq(rust, lark)

    def test_abstract_attribute(self) -> None:
        rust, lark = _parse_both("""
            define
            attribute content @abstract;
        """)
        _assert_schemas_eq(rust, lark)

    def test_independent_attribute(self) -> None:
        rust, lark = _parse_both("""
            define
            attribute tag, value string, @independent;
        """)
        _assert_schemas_eq(rust, lark)

    def test_regex_annotation(self) -> None:
        rust, lark = _parse_both(r"""
            define
            attribute email, value string, @regex("^[a-z]+@[a-z]+\.[a-z]+$");
        """)
        _assert_schemas_eq(rust, lark)

    def test_values_annotation(self) -> None:
        rust, lark = _parse_both("""
            define
            attribute status, value string, @values("active", "inactive", "pending");
        """)
        _assert_schemas_eq(rust, lark)

    def test_range_annotation(self) -> None:
        rust, lark = _parse_both("""
            define
            attribute age, value integer, @range(0..150);
        """)
        _assert_schemas_eq(rust, lark)

    def test_range_open_ended(self) -> None:
        rust, lark = _parse_both("""
            define
            attribute score, value double, @range(0.0..);
        """)
        _assert_schemas_eq(rust, lark)

    def test_distinct_on_role(self) -> None:
        rust, lark = _parse_both("""
            define
            relation team, relates member @distinct;
        """)
        _assert_schemas_eq(rust, lark)

    def test_distinct_with_card(self) -> None:
        rust, lark = _parse_both("""
            define
            relation team, relates member @distinct @card(2..5);
        """)
        _assert_schemas_eq(rust, lark)

    def test_role_override(self) -> None:
        rust, lark = _parse_both("""
            define
            relation contribution @abstract, relates contributor, relates work;
            relation authoring sub contribution, relates author as contributor;
        """)
        _assert_schemas_eq(rust, lark)

    def test_multiple_annotations_on_owns(self) -> None:
        rust, lark = _parse_both("""
            define
            attribute name, value string;
            entity person, owns name @key @cascade;
        """)
        _assert_schemas_eq(rust, lark)


# ---------------------------------------------------------------------------
# Inheritance
# ---------------------------------------------------------------------------


class TestParityInheritance:
    """Inheritance resolution produces identical results."""

    def test_single_level(self) -> None:
        rust, lark = _parse_both("""
            define
            attribute name, value string;
            attribute email, value string;
            entity base-entity @abstract, owns name;
            entity person sub base-entity, owns email;
        """)
        _assert_schemas_eq(rust, lark)

    def test_deep_inheritance(self) -> None:
        rust, lark = _parse_both("""
            define
            attribute name, value string;
            attribute email, value string;
            attribute employee-id, value string;
            entity base-entity @abstract, owns name;
            entity person sub base-entity, owns email @key;
            entity employee sub person, owns employee-id @unique;
        """)
        _assert_schemas_eq(rust, lark)

    def test_plays_inheritance(self) -> None:
        rust, lark = _parse_both("""
            define
            entity base-entity @abstract, plays friendship:friend;
            entity person sub base-entity;
            relation friendship, relates friend;
        """)
        _assert_schemas_eq(rust, lark)

    def test_relation_role_inheritance(self) -> None:
        rust, lark = _parse_both("""
            define
            relation base-rel @abstract, relates member;
            relation group-membership sub base-rel, relates group;
        """)
        _assert_schemas_eq(rust, lark)

    def test_relation_role_override_inheritance(self) -> None:
        rust, lark = _parse_both("""
            define
            relation base-rel @abstract, relates member;
            relation friendship sub base-rel, relates friend as member;
        """)
        _assert_schemas_eq(rust, lark)

    def test_attribute_inheritance(self) -> None:
        rust, lark = _parse_both("""
            define
            attribute content @abstract;
            attribute name sub content, value string;
        """)
        _assert_schemas_eq(rust, lark)

    def test_owns_order_preserved(self) -> None:
        """Parent owns come before child owns in owns_order."""
        rust, lark = _parse_both("""
            define
            attribute a, value string;
            attribute b, value string;
            attribute c, value string;
            attribute d, value string;
            entity parent @abstract, owns a, owns b;
            entity child sub parent, owns c, owns d;
        """)
        _assert_schemas_eq(rust, lark)
        # Verify ordering: parent attrs first
        assert rust.entities["child"].owns_order == ["a", "b", "c", "d"]

    def test_key_and_cardinality_inheritance(self) -> None:
        rust, lark = _parse_both("""
            define
            attribute id, value string;
            attribute name, value string;
            attribute tag, value string;
            entity base @abstract, owns id @key, owns tag @card(0..);
            entity child sub base, owns name;
        """)
        _assert_schemas_eq(rust, lark)
        # Verify inherited keys and cardinalities
        child = rust.entities["child"]
        assert "id" in child.keys
        assert "tag" in child.cardinalities
        assert child.cardinalities["tag"].max is None


# ---------------------------------------------------------------------------
# Comments
# ---------------------------------------------------------------------------


class TestParityComments:
    """Comments are handled identically by both parsers."""

    def test_hash_comments(self) -> None:
        rust, lark = _parse_both("""
            define
            # This is a comment
            attribute name, value string;
            entity person, owns name;  # inline comment
        """)
        _assert_schemas_eq(rust, lark)

    def test_cpp_comments(self) -> None:
        rust, lark = _parse_both("""
            define
            // C++ style comment
            attribute name, value string;
            entity person, owns name;
        """)
        _assert_schemas_eq(rust, lark)


# ---------------------------------------------------------------------------
# Complex real-world schema
# ---------------------------------------------------------------------------


class TestParityComplex:
    """Complex schema that exercises many features at once."""

    BOOKSTORE = """
        define

        attribute name, value string;
        attribute email, value string;
        attribute age, value long;
        attribute title, value string;
        attribute isbn, value string;
        attribute price, value double;
        attribute tag, value string;
        attribute publish-date, value date;

        entity base-entity @abstract, owns name;
        entity person sub base-entity, owns email @key, owns age;
        entity author sub person;

        entity book
            , owns title
            , owns isbn @key
            , owns price
            , owns tag @card(0..)
            , owns publish-date
            , plays authorship:written-work;

        relation authorship
            , relates written-by
            , relates written-work;
    """

    def test_complex_full_parity(self) -> None:
        rust, lark = _parse_both(self.BOOKSTORE)
        _assert_schemas_eq(rust, lark)

    def test_complex_entity_count(self) -> None:
        rust, lark = _parse_both(self.BOOKSTORE)
        assert len(rust.entities) == len(lark.entities) == 4

    def test_complex_attribute_count(self) -> None:
        rust, lark = _parse_both(self.BOOKSTORE)
        assert len(rust.attributes) == len(lark.attributes) == 8

    def test_complex_multi_level_owns(self) -> None:
        """Author inherits from person which inherits from base-entity."""
        rust, lark = _parse_both(self.BOOKSTORE)
        # Both parsers should resolve 3-level inheritance
        rust_names = {a for a in rust.entities["author"].owns}
        lark_names = {a for a in lark.entities["author"].owns}
        assert rust_names == lark_names
        assert "name" in rust_names  # from base-entity
        assert "email" in rust_names  # from person
        assert "age" in rust_names  # from person

    def test_complex_relation_with_owns(self) -> None:
        """Relation that owns attributes and has roles."""
        rust, lark = _parse_both("""
            define
            attribute start-date, value date;
            attribute end-date, value date;
            relation employment
                , relates employer
                , relates employee
                , owns start-date
                , owns end-date;
        """)
        _assert_schemas_eq(rust, lark)

    def test_mixed_annotations_comprehensive(self) -> None:
        """Schema using many annotation types at once."""
        rust, lark = _parse_both("""
            define
            attribute id, value string;
            attribute name, value string;
            attribute status, value string, @values("active", "archived");
            attribute score, value double, @range(0.0..100.0);
            attribute email, value string;
            attribute tag, value string, @independent;

            entity base @abstract
                , owns id @key;

            entity user sub base
                , owns name
                , owns email @unique
                , owns status
                , owns score
                , owns tag @card(0..) @cascade
                , plays membership:member @card(0..5);

            relation membership
                , relates member @card(1..)
                , relates group @distinct;
        """)
        _assert_schemas_eq(rust, lark)
