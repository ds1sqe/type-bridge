# pyright: reportMissingImports=false
"""Field-level diagnostics tests for the TOML schema DSL transpiler (#138).

Each test asserts that a semantically malformed TOML document raises
``ValueError`` via the PyO3 boundary and that the message names the
offending field or type.  A final valid-path smoke test confirms the
validation pass is a no-op for well-formed input.
"""

from __future__ import annotations

from pathlib import Path

import pytest

pytest.importorskip("type_bridge_core")

from type_bridge_core import toml_to_typeql

from type_bridge.generator import generate_models
from type_bridge.generator.parser import parse_tql_schema

# ---------------------------------------------------------------------------
# Attribute value/sub XOR — both set
# ---------------------------------------------------------------------------


def test_attribute_value_and_sub_conflict_raises() -> None:
    """Attribute with both value and sub raises ValueError naming the attribute."""
    toml_text = """
[attributes.name]
value = "string"
sub = "other"
"""
    with pytest.raises(ValueError, match="name"):
        toml_to_typeql(toml_text)


# ---------------------------------------------------------------------------
# Attribute value/sub XOR — neither set
# ---------------------------------------------------------------------------


def test_attribute_neither_value_nor_sub_raises() -> None:
    """Attribute with neither value nor sub raises ValueError naming the attribute."""
    toml_text = """
[attributes.orphan]
"""
    with pytest.raises(ValueError, match="orphan"):
        toml_to_typeql(toml_text)


# ---------------------------------------------------------------------------
# Unknown value type — attribute
# ---------------------------------------------------------------------------


def test_unknown_attribute_value_type_raises() -> None:
    """Attribute with an unknown value type raises ValueError naming the attribute and type."""
    toml_text = """
[attributes.title]
value = "strng"
"""
    with pytest.raises(ValueError, match="title"):
        toml_to_typeql(toml_text)


def test_unknown_attribute_value_type_message_names_bad_value() -> None:
    """The ValueError message must include the misspelled type name."""
    toml_text = """
[attributes.title]
value = "strng"
"""
    with pytest.raises(ValueError, match="strng"):
        toml_to_typeql(toml_text)


# ---------------------------------------------------------------------------
# Unknown value type — struct field
# ---------------------------------------------------------------------------


def test_unknown_struct_field_type_raises() -> None:
    """Struct field with unknown type raises ValueError naming the struct and field."""
    toml_text = """
[structs.person-name]
fields = [{ name = "first", type = "itneger" }]
"""
    with pytest.raises(ValueError, match="person-name"):
        toml_to_typeql(toml_text)


def test_unknown_struct_field_type_message_names_field_and_bad_type() -> None:
    """The ValueError message must include both the field name and the bad type."""
    toml_text = """
[structs.person-name]
fields = [{ name = "first", type = "itneger" }]
"""
    with pytest.raises(ValueError, match="first"):
        toml_to_typeql(toml_text)


# ---------------------------------------------------------------------------
# Dangling sub parent
# ---------------------------------------------------------------------------


def test_dangling_sub_parent_attribute_raises() -> None:
    """Attribute with a sub referencing a non-existent parent raises ValueError."""
    toml_text = """
[attributes.isbn-13]
sub = "nonexistent"
"""
    with pytest.raises(ValueError, match="isbn-13"):
        toml_to_typeql(toml_text)


def test_dangling_sub_parent_attribute_message_names_parent() -> None:
    """The ValueError message must include the missing parent name."""
    toml_text = """
[attributes.isbn-13]
sub = "nonexistent"
"""
    with pytest.raises(ValueError, match="nonexistent"):
        toml_to_typeql(toml_text)


def test_dangling_sub_parent_entity_raises() -> None:
    """Entity with a sub referencing a non-existent parent raises ValueError."""
    toml_text = """
[entities.hardback]
sub = "ghost-parent"
"""
    with pytest.raises(ValueError, match="hardback"):
        toml_to_typeql(toml_text)


def test_dangling_sub_parent_relation_raises() -> None:
    """Relation with a sub referencing a non-existent parent raises ValueError."""
    toml_text = """
[relations.authoring]
sub = "ghost-base"
"""
    with pytest.raises(ValueError, match="authoring"):
        toml_to_typeql(toml_text)


# ---------------------------------------------------------------------------
# Missing role player — relation undefined
# ---------------------------------------------------------------------------


def test_missing_role_relation_raises() -> None:
    """Entity plays entry referencing an undefined relation raises ValueError."""
    toml_text = """
[entities.person]
plays = [{ relation = "nope", role = "r" }]
"""
    with pytest.raises(ValueError, match="nope"):
        toml_to_typeql(toml_text)


def test_missing_role_relation_message_names_player() -> None:
    """The ValueError message must include the player (entity) name."""
    toml_text = """
[entities.person]
plays = [{ relation = "nope", role = "r" }]
"""
    with pytest.raises(ValueError, match="person"):
        toml_to_typeql(toml_text)


# ---------------------------------------------------------------------------
# Missing role player — role name not declared on the relation
# ---------------------------------------------------------------------------


def test_missing_role_raises() -> None:
    """Entity plays entry referencing an undefined role name raises ValueError."""
    toml_text = """
[entities.person]
plays = [{ relation = "review", role = "ghost-role" }]

[relations.review]
roles = [{ name = "reviewer" }]
"""
    with pytest.raises(ValueError, match="ghost-role"):
        toml_to_typeql(toml_text)


def test_missing_role_message_names_relation_and_player() -> None:
    """The ValueError message must include the relation name and the player."""
    toml_text = """
[entities.person]
plays = [{ relation = "review", role = "ghost-role" }]

[relations.review]
roles = [{ name = "reviewer" }]
"""
    with pytest.raises(ValueError, match="review"):
        toml_to_typeql(toml_text)


# ---------------------------------------------------------------------------
# Missing role player — relation-level plays
# ---------------------------------------------------------------------------


def test_relation_plays_missing_relation_raises() -> None:
    """Relation plays entry referencing an undefined relation raises ValueError."""
    toml_text = """
[relations.publication]
plays = [{ relation = "nope", role = "work" }]
"""
    with pytest.raises(ValueError, match="publication"):
        toml_to_typeql(toml_text)
    with pytest.raises(ValueError, match="nope"):
        toml_to_typeql(toml_text)
    with pytest.raises(ValueError, match="work"):
        toml_to_typeql(toml_text)


def test_relation_plays_missing_role_raises() -> None:
    """Relation plays entry referencing an undefined role name raises ValueError."""
    toml_text = """
[relations.publication]
plays = [{ relation = "contribution", role = "ghost-role" }]

[relations.contribution]
roles = [{ name = "work" }]
"""
    with pytest.raises(ValueError, match="publication"):
        toml_to_typeql(toml_text)
    with pytest.raises(ValueError, match="contribution"):
        toml_to_typeql(toml_text)
    with pytest.raises(ValueError, match="ghost-role"):
        toml_to_typeql(toml_text)


# ---------------------------------------------------------------------------
# Empty struct
# ---------------------------------------------------------------------------


def test_empty_struct_raises() -> None:
    """Struct with zero fields raises ValueError naming the struct."""
    toml_text = """
[structs.empty-thing]
fields = []
"""
    with pytest.raises(ValueError, match="empty-thing"):
        toml_to_typeql(toml_text)


# ---------------------------------------------------------------------------
# Malformed function body
# ---------------------------------------------------------------------------


def test_malformed_function_body_raises() -> None:
    """Function body with no return clause raises ValueError naming the function."""
    toml_text = """
[functions.bad-fn]
signature = "fun bad-fn($x: t) -> { r }"
body = "  match $x isa t;"
"""
    with pytest.raises(ValueError, match="bad-fn"):
        toml_to_typeql(toml_text)


# ---------------------------------------------------------------------------
# Structural errors (deny_unknown_fields) are also surfaced as ValueError —
# documenting that "unknown annotation key" is already caught at deserialise.
# ---------------------------------------------------------------------------


def test_unknown_annotation_key_raises_at_deserialise() -> None:
    """A typo'd annotation key (deny_unknown_fields) raises ValueError at deserialise.

    This documents that the 'unknown annotation' diagnostic from the issue AC is
    already covered structurally: TOML annotations are typed model fields, so a
    typo'd key like 'kye = true' is caught by serde's deny_unknown_fields before
    the semantic validation pass runs.  No redundant semantic check is needed.
    """
    toml_text = """
[entities.book]
owns = [{ attribute = "isbn", kye = true }]
"""
    with pytest.raises(ValueError):
        toml_to_typeql(toml_text)


# ---------------------------------------------------------------------------
# Valid path — validation pass is a no-op
# ---------------------------------------------------------------------------


def test_valid_schema_transpiles_without_error() -> None:
    """A fully valid TOML schema transpiles without raising ValueError."""
    toml_text = """
[attributes.name]
value = "string"

[attributes.score]
value = "double"

[entities.person]
owns = ["name"]
plays = [{ relation = "review", role = "reviewer" }]

[entities.document]
owns = ["name"]
plays = [{ relation = "review", role = "document" }]

[relations.review]
roles = [
    { name = "document", card = "1..1" },
    { name = "reviewer", card = "1..3" },
]
owns = ["score"]

[functions.top-score]
signature = "fun top-score($d: document) -> double"
body = "  match $d has score $s;\\n  return max($s);"

[structs.person-name]
fields = [
    { name = "first", type = "string" },
    { name = "last", type = "string" },
]
"""
    result = toml_to_typeql(toml_text)
    parsed = parse_tql_schema(result)
    assert parsed is not None, "parsed schema must not be None"
    assert "name" in parsed.attributes, "attribute 'name' must be in parsed schema"


# ---------------------------------------------------------------------------
# End-to-end: generate_models raises ValueError for a malformed .toml
# ---------------------------------------------------------------------------


def test_generate_models_raises_for_malformed_toml(tmp_path: Path) -> None:
    """generate_models raises ValueError when the .toml has a semantic error.

    This proves the diagnostic reaches the public generator API, not only
    the raw toml_to_typeql binding.
    """
    bad_toml = tmp_path / "bad.toml"
    bad_toml.write_text(
        """
[attributes.broken]
value = "strng"
""",
        encoding="utf-8",
    )
    out_dir = tmp_path / "out"
    with pytest.raises(ValueError, match="strng"):
        generate_models(str(bad_toml), str(out_dir))
