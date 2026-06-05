# pyright: reportMissingImports=false
"""Round-trip equivalence harness for the TOML schema DSL (#138).

This is the epic's correctness oracle: TOML fixtures and their hand-written
TQL mirrors must parse to structurally-equivalent ParsedSchema objects, and
``generate_models`` must produce byte-for-byte identical packages from both.

Sub-plans 02–06 extend the fixture corpus and reuse the helper defined here.
"""

from __future__ import annotations

from pathlib import Path

import pytest

pytest.importorskip("type_bridge_core")

from type_bridge_core import toml_to_typeql  # type: ignore[import]

from type_bridge.generator import generate_models
from type_bridge.generator.models import (
    AttributeSpec,
    EntitySpec,
    FunctionSpec,
    ParsedSchema,
    RelationSpec,
    StructSpec,
)
from type_bridge.generator.parser import parse_tql_schema

FIXTURES_DIR = Path(__file__).parent / "fixtures"


# ---------------------------------------------------------------------------
# Structural-equivalence helper — test-only, does NOT touch models.py
# ---------------------------------------------------------------------------


def _assert_attribute_equivalent(name: str, a: AttributeSpec, b: AttributeSpec) -> None:
    """Assert two AttributeSpec objects are equivalent field-by-field."""
    assert a.name == b.name, f"[{name}] name mismatch: {a.name!r} != {b.name!r}"
    assert a.value_type == b.value_type, (
        f"[{name}] value_type: {a.value_type!r} != {b.value_type!r}"
    )
    assert a.parent == b.parent, f"[{name}] parent: {a.parent!r} != {b.parent!r}"
    assert a.abstract == b.abstract, f"[{name}] abstract: {a.abstract!r} != {b.abstract!r}"
    assert a.independent == b.independent, (
        f"[{name}] independent: {a.independent!r} != {b.independent!r}"
    )
    assert a.regex == b.regex, f"[{name}] regex: {a.regex!r} != {b.regex!r}"
    assert a.allowed_values == b.allowed_values, f"[{name}] allowed_values mismatch"
    assert a.range_min == b.range_min, f"[{name}] range_min mismatch"
    assert a.range_max == b.range_max, f"[{name}] range_max mismatch"
    assert a.docstring == b.docstring, f"[{name}] docstring mismatch"
    # annotations: dict compared key-by-key
    assert set(a.annotations.keys()) == set(b.annotations.keys()), (
        f"[{name}] annotations keys mismatch: {set(a.annotations.keys())} != {set(b.annotations.keys())}"
    )
    for k in a.annotations:
        assert a.annotations[k] == b.annotations[k], f"[{name}] annotation[{k!r}] mismatch"


def _assert_entity_equivalent(name: str, a: EntitySpec, b: EntitySpec) -> None:
    """Assert two EntitySpec objects are equivalent field-by-field."""
    assert a.name == b.name, f"[{name}] name mismatch"
    assert a.parent == b.parent, f"[{name}] parent: {a.parent!r} != {b.parent!r}"
    assert a.abstract == b.abstract, f"[{name}] abstract mismatch"
    assert a.docstring == b.docstring, f"[{name}] docstring mismatch"
    # owns/plays/keys/uniques/cascades: UNORDERED — compare as sets
    assert set(a.owns) == set(b.owns), f"[{name}] owns (as set): {set(a.owns)} != {set(b.owns)}"
    assert set(a.plays) == set(b.plays), f"[{name}] plays (as set) mismatch"
    assert set(a.keys) == set(b.keys), f"[{name}] keys (as set) mismatch"
    assert set(a.uniques) == set(b.uniques), f"[{name}] uniques (as set) mismatch"
    assert set(a.cascades) == set(b.cascades), f"[{name}] cascades (as set) mismatch"
    # owns_order: ORDERED — order must match (transpiler must preserve TOML array order)
    assert a.owns_order == b.owns_order, (
        f"[{name}] owns_order (ordered list): {a.owns_order} != {b.owns_order}"
    )
    # cardinalities: dict key-by-key
    assert set(a.cardinalities.keys()) == set(b.cardinalities.keys()), (
        f"[{name}] cardinalities keys mismatch"
    )
    for k in a.cardinalities:
        assert a.cardinalities[k] == b.cardinalities[k], f"[{name}] cardinality[{k!r}] mismatch"
    # plays_cardinalities: dict key-by-key
    assert set(a.plays_cardinalities.keys()) == set(b.plays_cardinalities.keys()), (
        f"[{name}] plays_cardinalities keys mismatch"
    )
    for k in a.plays_cardinalities:
        assert a.plays_cardinalities[k] == b.plays_cardinalities[k], (
            f"[{name}] plays_cardinality[{k!r}] mismatch"
        )
    # subkeys: dict key-by-key
    assert set(a.subkeys.keys()) == set(b.subkeys.keys()), f"[{name}] subkeys keys mismatch"
    for k in a.subkeys:
        assert a.subkeys[k] == b.subkeys[k], f"[{name}] subkeys[{k!r}] mismatch"
    # annotations: dict key-by-key
    assert set(a.annotations.keys()) == set(b.annotations.keys()), (
        f"[{name}] annotations keys mismatch"
    )
    for k in a.annotations:
        assert a.annotations[k] == b.annotations[k], f"[{name}] annotation[{k!r}] mismatch"


def _assert_relation_equivalent(name: str, a: RelationSpec, b: RelationSpec) -> None:
    """Assert two RelationSpec objects are equivalent field-by-field."""
    assert a.name == b.name, f"[{name}] name mismatch"
    assert a.parent == b.parent, f"[{name}] parent mismatch"
    assert a.abstract == b.abstract, f"[{name}] abstract mismatch"
    assert a.docstring == b.docstring, f"[{name}] docstring mismatch"
    # roles: ORDERED list
    assert len(a.roles) == len(b.roles), f"[{name}] roles length: {len(a.roles)} != {len(b.roles)}"
    for i, (ra, rb) in enumerate(zip(a.roles, b.roles)):
        assert ra.name == rb.name, f"[{name}] roles[{i}].name mismatch"
        assert ra.overrides == rb.overrides, f"[{name}] roles[{i}].overrides mismatch"
        assert ra.cardinality == rb.cardinality, f"[{name}] roles[{i}].cardinality mismatch"
        assert ra.distinct == rb.distinct, f"[{name}] roles[{i}].distinct mismatch"
    # owns/keys/uniques/cascades: UNORDERED sets
    assert set(a.owns) == set(b.owns), f"[{name}] owns (as set) mismatch"
    assert set(a.keys) == set(b.keys), f"[{name}] keys (as set) mismatch"
    assert set(a.uniques) == set(b.uniques), f"[{name}] uniques (as set) mismatch"
    assert set(a.cascades) == set(b.cascades), f"[{name}] cascades (as set) mismatch"
    # owns_order: ORDERED list
    assert a.owns_order == b.owns_order, f"[{name}] owns_order (ordered list) mismatch"
    # cardinalities: dict key-by-key
    assert set(a.cardinalities.keys()) == set(b.cardinalities.keys()), (
        f"[{name}] cardinalities keys mismatch"
    )
    for k in a.cardinalities:
        assert a.cardinalities[k] == b.cardinalities[k], f"[{name}] cardinality[{k!r}] mismatch"
    # subkeys: dict key-by-key
    assert set(a.subkeys.keys()) == set(b.subkeys.keys()), f"[{name}] subkeys keys mismatch"
    for k in a.subkeys:
        assert a.subkeys[k] == b.subkeys[k], f"[{name}] subkeys[{k!r}] mismatch"
    # annotations: dict key-by-key
    assert set(a.annotations.keys()) == set(b.annotations.keys()), (
        f"[{name}] annotations keys mismatch"
    )
    for k in a.annotations:
        assert a.annotations[k] == b.annotations[k], f"[{name}] annotation[{k!r}] mismatch"


def _assert_function_equivalent(name: str, a: FunctionSpec, b: FunctionSpec) -> None:
    """Assert two FunctionSpec objects are equivalent."""
    assert a.name == b.name, f"[{name}] name mismatch"
    assert a.docstring == b.docstring, f"[{name}] docstring mismatch"
    assert len(a.parameters) == len(b.parameters), f"[{name}] parameters length mismatch"
    for i, (pa, pb) in enumerate(zip(a.parameters, b.parameters)):
        assert pa.name == pb.name, f"[{name}] parameters[{i}].name mismatch"
        assert pa.type == pb.type, f"[{name}] parameters[{i}].type mismatch"
    assert a.return_type.is_stream == b.return_type.is_stream, (
        f"[{name}] return_type.is_stream mismatch"
    )
    assert len(a.return_type.types) == len(b.return_type.types), (
        f"[{name}] return_type.types length mismatch"
    )
    for i, (ta, tb) in enumerate(zip(a.return_type.types, b.return_type.types)):
        assert ta.name == tb.name, f"[{name}] return_type.types[{i}].name mismatch"
        assert ta.optional == tb.optional, f"[{name}] return_type.types[{i}].optional mismatch"


def _assert_struct_equivalent(name: str, a: StructSpec, b: StructSpec) -> None:
    """Assert two StructSpec objects are equivalent."""
    assert a.name == b.name, f"[{name}] name mismatch"
    assert a.docstring == b.docstring, f"[{name}] docstring mismatch"
    assert len(a.fields) == len(b.fields), f"[{name}] fields length mismatch"
    for i, (fa, fb) in enumerate(zip(a.fields, b.fields)):
        assert fa.name == fb.name, f"[{name}] fields[{i}].name mismatch"
        assert fa.value_type == fb.value_type, f"[{name}] fields[{i}].value_type mismatch"
        assert fa.optional == fb.optional, f"[{name}] fields[{i}].optional mismatch"
    # annotations: dict key-by-key
    assert set(a.annotations.keys()) == set(b.annotations.keys()), (
        f"[{name}] annotations keys mismatch"
    )
    for k in a.annotations:
        assert a.annotations[k] == b.annotations[k], f"[{name}] annotation[{k!r}] mismatch"


def assert_parsed_equivalent(a: ParsedSchema, b: ParsedSchema) -> None:
    """Assert two ParsedSchema objects are structurally equivalent.

    Comparison semantics:
    - Top-level maps compared by KEY SET first, then per-spec.
    - Within each spec: scalar fields by ``==``; UNORDERED-set fields
      (owns/plays/keys/uniques/cascades) compared as ``set(...)``; ORDERED-list
      fields (owns_order, relation roles) compared as lists; dict fields
      (annotations/cardinalities/subkeys/plays_cardinalities) compared
      key-by-key; docstring by ``==``.

    This helper is intentionally order-lenient on set-typed fields so that
    benign TQL whitespace/declaration-order variations do not cause spurious
    failures. The byte-identical package smoke (test_generate_models_byte_identical)
    is the stricter invariant-2 proof.
    """
    # --- attributes ---
    assert set(a.attributes.keys()) == set(b.attributes.keys()), (
        f"attributes key sets differ: {set(a.attributes.keys())} != {set(b.attributes.keys())}"
    )
    for name in a.attributes:
        _assert_attribute_equivalent(name, a.attributes[name], b.attributes[name])

    # --- entities ---
    assert set(a.entities.keys()) == set(b.entities.keys()), (
        f"entities key sets differ: {set(a.entities.keys())} != {set(b.entities.keys())}"
    )
    for name in a.entities:
        _assert_entity_equivalent(name, a.entities[name], b.entities[name])

    # --- relations ---
    assert set(a.relations.keys()) == set(b.relations.keys()), (
        f"relations key sets differ: {set(a.relations.keys())} != {set(b.relations.keys())}"
    )
    for name in a.relations:
        _assert_relation_equivalent(name, a.relations[name], b.relations[name])

    # --- functions ---
    assert set(a.functions.keys()) == set(b.functions.keys()), (
        f"functions key sets differ: {set(a.functions.keys())} != {set(b.functions.keys())}"
    )
    for name in a.functions:
        _assert_function_equivalent(name, a.functions[name], b.functions[name])

    # --- structs ---
    assert set(a.structs.keys()) == set(b.structs.keys()), (
        f"structs key sets differ: {set(a.structs.keys())} != {set(b.structs.keys())}"
    )
    for name in a.structs:
        _assert_struct_equivalent(name, a.structs[name], b.structs[name])


def assert_roundtrip_equivalent(toml_path: Path, tql_path: Path) -> None:
    """Assert that a TOML fixture and its TQL mirror parse to equivalent IR.

    Reads both files, transpiles the TOML to TypeQL via ``toml_to_typeql``,
    parses both through ``parse_tql_schema``, and asserts structural equivalence
    via ``assert_parsed_equivalent``.
    """
    toml_text = toml_path.read_text(encoding="utf-8")
    tql_text = tql_path.read_text(encoding="utf-8")

    parsed_from_toml = parse_tql_schema(toml_to_typeql(toml_text))
    parsed_from_tql = parse_tql_schema(tql_text)

    assert_parsed_equivalent(parsed_from_toml, parsed_from_tql)


# ---------------------------------------------------------------------------
# Tests: round-trip equivalence (IR oracle)
# ---------------------------------------------------------------------------


class TestAttributeOwnsRoundtrip:
    """Round-trip equivalence for the attributes+owns fixture."""

    def test_attributes_owns_roundtrip_equivalent(self) -> None:
        """attributes_owns.toml and attributes_owns.tql must parse to equivalent IR."""
        assert_roundtrip_equivalent(
            FIXTURES_DIR / "attributes_owns.toml",
            FIXTURES_DIR / "attributes_owns.tql",
        )

    def test_toml_has_expected_attributes(self) -> None:
        """Parsed TOML fixture must contain 'name' and 'age' attributes."""
        toml_text = (FIXTURES_DIR / "attributes_owns.toml").read_text(encoding="utf-8")
        schema = parse_tql_schema(toml_to_typeql(toml_text))
        assert "name" in schema.attributes
        assert "age" in schema.attributes
        assert schema.attributes["name"].value_type == "string"
        assert schema.attributes["age"].value_type == "long"

    def test_toml_has_expected_entity(self) -> None:
        """Parsed TOML fixture must contain 'person' entity owning both attributes."""
        toml_text = (FIXTURES_DIR / "attributes_owns.toml").read_text(encoding="utf-8")
        schema = parse_tql_schema(toml_to_typeql(toml_text))
        assert "person" in schema.entities
        person = schema.entities["person"]
        assert set(person.owns) == {"name", "age"}

    def test_toml_owns_order_preserved(self) -> None:
        """owns_order from TOML must match the TOML array declaration order."""
        toml_text = (FIXTURES_DIR / "attributes_owns.toml").read_text(encoding="utf-8")
        schema = parse_tql_schema(toml_to_typeql(toml_text))
        person = schema.entities["person"]
        # TOML declares owns = ["name", "age"] — this order must be preserved
        assert person.owns_order == ["name", "age"]


# ---------------------------------------------------------------------------
# Integration smoke: byte-identical package generation (strong Inv-2 proof)
# ---------------------------------------------------------------------------


class TestGenerateModelsByteIdentical:
    """End-to-end smoke: generate_models on the TOML and TQL fixtures produces
    byte-for-byte identical model module files.

    The renderer is a deterministic function of ParsedSchema.  Equivalent
    inputs → identical IR → identical output.  This is the strong invariant-2
    proof through the REAL renderer (not just the IR layer).

    Exclusions:
    - ``schema.tql``: this is a copy of the input schema source (not a generated
      model module).  The TOML path writes the transpiler's TypeQL output;
      the TQL path writes the TQL file content.  These are identical when the
      TQL fixture content exactly matches ``toml_to_typeql`` output (which is
      true for the attributes_owns fixtures), but ``schema.tql`` is explicitly
      not a model module and is excluded from the model-module byte-compare for
      clarity.  The model modules (attributes.py, entities.py, relations.py,
      __init__.py, registry.py) are the invariant-2 proof targets.
    """

    # Model module files produced by generate_models (excluding schema copy)
    MODEL_FILES = ["attributes.py", "entities.py", "relations.py", "__init__.py", "registry.py"]

    def test_generate_models_byte_identical(self, tmp_path: Path) -> None:
        """generate_models on attributes_owns.toml vs .tql produces identical model files."""
        toml_path = FIXTURES_DIR / "attributes_owns.toml"
        tql_path = FIXTURES_DIR / "attributes_owns.tql"

        out_toml = tmp_path / "out_toml"
        out_tql = tmp_path / "out_tql"

        generate_models(toml_path, out_toml)
        generate_models(tql_path, out_tql)

        for filename in self.MODEL_FILES:
            toml_file = out_toml / filename
            tql_file = out_tql / filename
            assert toml_file.exists(), f"TOML output missing: {filename}"
            assert tql_file.exists(), f"TQL output missing: {filename}"
            toml_content = toml_file.read_text(encoding="utf-8")
            tql_content = tql_file.read_text(encoding="utf-8")
            assert toml_content == tql_content, (
                f"{filename} differs between TOML and TQL generate_models outputs.\n"
                f"--- TOML output ({filename}) ---\n{toml_content}\n"
                f"--- TQL output ({filename}) ---\n{tql_content}\n"
            )


# ---------------------------------------------------------------------------
# Tests: relation + roles + plays round-trip equivalence
# ---------------------------------------------------------------------------


class TestRelationRoundtrip:
    """Round-trip equivalence for the relations+roles+plays fixture.

    Reuses ``assert_roundtrip_equivalent`` (unchanged from 01) and adds focused
    assertions on relation roles, per-role cardinality, and entity plays.
    """

    def test_relations_roles_roundtrip_equivalent(self) -> None:
        """relations_roles.toml and relations_roles.tql must parse to equivalent IR."""
        assert_roundtrip_equivalent(
            FIXTURES_DIR / "relations_roles.toml",
            FIXTURES_DIR / "relations_roles.tql",
        )

    def test_review_relation_roles_order_and_cardinality(self) -> None:
        """Parsed TOML must contain review relation with roles in TOML array order
        and with the correct per-role cardinalities."""
        from type_bridge.generator.models import Cardinality

        toml_text = (FIXTURES_DIR / "relations_roles.toml").read_text(encoding="utf-8")
        schema = parse_tql_schema(toml_to_typeql(toml_text))

        assert "review" in schema.relations, "expected 'review' relation in parsed schema"
        review = schema.relations["review"]

        # roles are an ordered list — document comes before reviewer
        assert len(review.roles) == 2, f"expected 2 roles, got {len(review.roles)}"
        assert review.roles[0].name == "document", (
            f"expected first role 'document', got {review.roles[0].name!r}"
        )
        assert review.roles[1].name == "reviewer", (
            f"expected second role 'reviewer', got {review.roles[1].name!r}"
        )

        # cardinalities: document @card(1..1), reviewer @card(1..3)
        assert review.roles[0].cardinality == Cardinality(min=1, max=1), (
            f"document role cardinality mismatch: {review.roles[0].cardinality!r}"
        )
        assert review.roles[1].cardinality == Cardinality(min=1, max=3), (
            f"reviewer role cardinality mismatch: {review.roles[1].cardinality!r}"
        )

        # relation owns score
        assert "score" in review.owns, "expected review relation to own 'score'"

    def test_person_plays_review_reviewer(self) -> None:
        """Parsed TOML must show person plays review:reviewer."""
        toml_text = (FIXTURES_DIR / "relations_roles.toml").read_text(encoding="utf-8")
        schema = parse_tql_schema(toml_to_typeql(toml_text))

        assert "person" in schema.entities, "expected 'person' entity in parsed schema"
        person = schema.entities["person"]
        assert "review:reviewer" in person.plays, (
            f"expected 'review:reviewer' in person.plays, got {person.plays!r}"
        )

    def test_generate_models_relation_byte_identical(self, tmp_path: Path) -> None:
        """generate_models on relations_roles.toml vs .tql produces identical model files,
        including relations.py — strong Inv-2 proof that the relation feature flows
        through the full renderer."""
        toml_path = FIXTURES_DIR / "relations_roles.toml"
        tql_path = FIXTURES_DIR / "relations_roles.tql"

        out_toml = tmp_path / "out_toml"
        out_tql = tmp_path / "out_tql"

        generate_models(toml_path, out_toml)
        generate_models(tql_path, out_tql)

        model_files = [
            "attributes.py",
            "entities.py",
            "relations.py",
            "__init__.py",
            "registry.py",
        ]
        for filename in model_files:
            toml_file = out_toml / filename
            tql_file = out_tql / filename
            assert toml_file.exists(), f"TOML output missing: {filename}"
            assert tql_file.exists(), f"TQL output missing: {filename}"
            toml_content = toml_file.read_text(encoding="utf-8")
            tql_content = tql_file.read_text(encoding="utf-8")
            assert toml_content == tql_content, (
                f"{filename} differs between TOML and TQL generate_models outputs.\n"
                f"--- TOML output ({filename}) ---\n{toml_content}\n"
                f"--- TQL output ({filename}) ---\n{tql_content}\n"
            )
