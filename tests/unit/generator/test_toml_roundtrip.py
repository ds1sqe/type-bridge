# pyright: reportMissingImports=false
"""Round-trip equivalence harness for the TOML schema DSL (#138).

This is the format's correctness oracle: TOML fixtures and their hand-written
TQL mirrors must parse to structurally-equivalent ParsedSchema objects, and
``generate_models`` must produce byte-for-byte identical packages from both.

Each schema feature adds a fixture pair to the corpus and reuses the helper
defined here, which compares every ParsedSchema spec type.
"""

from __future__ import annotations

from pathlib import Path

import pytest

pytest.importorskip("type_bridge_core")

from type_bridge_core import TypeSchema, toml_to_typeql

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
REPO_ROOT = Path(__file__).resolve().parents[3]
INTEGRATION_FIXTURES_DIR = REPO_ROOT / "tests" / "integration" / "generator" / "fixtures"


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
    assert set(a.ordered_owns) == set(b.ordered_owns), (
        f"[{name}] ordered_owns (as set): {set(a.ordered_owns)} != {set(b.ordered_owns)}"
    )
    assert set(a.distinct_owns) == set(b.distinct_owns), (
        f"[{name}] distinct_owns (as set): {set(a.distinct_owns)} != {set(b.distinct_owns)}"
    )
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
        assert ra.ordered == rb.ordered, f"[{name}] roles[{i}].ordered mismatch"
        assert ra.distinct == rb.distinct, f"[{name}] roles[{i}].distinct mismatch"
        assert ra.is_abstract == rb.is_abstract, f"[{name}] roles[{i}].is_abstract mismatch"
    # owns/keys/uniques/cascades: UNORDERED sets
    assert set(a.owns) == set(b.owns), f"[{name}] owns (as set) mismatch"
    assert set(a.keys) == set(b.keys), f"[{name}] keys (as set) mismatch"
    assert set(a.uniques) == set(b.uniques), f"[{name}] uniques (as set) mismatch"
    assert set(a.cascades) == set(b.cascades), f"[{name}] cascades (as set) mismatch"
    assert set(a.ordered_owns) == set(b.ordered_owns), (
        f"[{name}] ordered_owns (as set): {set(a.ordered_owns)} != {set(b.ordered_owns)}"
    )
    assert set(a.distinct_owns) == set(b.distinct_owns), (
        f"[{name}] distinct_owns (as set): {set(a.distinct_owns)} != {set(b.distinct_owns)}"
    )
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


# ---------------------------------------------------------------------------
# Tests: annotation / inheritance round-trip equivalence
# ---------------------------------------------------------------------------


class TestAnnotationInheritanceRoundtrip:
    """Round-trip equivalence for the annotations+inheritance fixture.

    Reuses ``assert_roundtrip_equivalent`` (unchanged from 01) to assert that
    ``annotations_inheritance.toml`` and its hand-written TQL mirror parse to
    structurally-equivalent IR.  Adds focused assertions on every 03 target
    field: abstract/sub on attributes+entities, @key/@unique/@card on owns,
    and @regex/@values/@range on attribute values.
    """

    def test_annotations_inheritance_roundtrip_equivalent(self) -> None:
        """annotations_inheritance.toml and .tql must parse to equivalent IR."""
        assert_roundtrip_equivalent(
            FIXTURES_DIR / "annotations_inheritance.toml",
            FIXTURES_DIR / "annotations_inheritance.tql",
        )

    def test_book_entity_abstract_and_owns_annotations(self) -> None:
        """Parsed TOML must yield book entity: abstract=True, keys={isbn-13},
        uniques={isbn-10}, cardinalities={isbn: Cardinality(0,2)}, and title in owns."""
        from type_bridge.generator.models import Cardinality

        toml_text = (FIXTURES_DIR / "annotations_inheritance.toml").read_text(encoding="utf-8")
        schema = parse_tql_schema(toml_to_typeql(toml_text))

        assert "book" in schema.entities, "expected 'book' entity in parsed schema"
        book = schema.entities["book"]
        assert book.abstract is True, f"expected book.abstract=True, got {book.abstract!r}"
        assert "isbn-13" in book.keys, f"expected isbn-13 in book.keys, got {book.keys!r}"
        assert "isbn-10" in book.uniques, f"expected isbn-10 in book.uniques, got {book.uniques!r}"
        assert "isbn" in book.cardinalities, "expected 'isbn' in book.cardinalities"
        assert book.cardinalities["isbn"] == Cardinality(min=0, max=2), (
            f"expected Cardinality(0,2) for isbn, got {book.cardinalities['isbn']!r}"
        )
        assert "title" in book.owns, f"expected 'title' in book.owns, got {book.owns!r}"

    def test_hardback_entity_sub_book(self) -> None:
        """Parsed TOML must yield hardback entity with parent='book'."""
        toml_text = (FIXTURES_DIR / "annotations_inheritance.toml").read_text(encoding="utf-8")
        schema = parse_tql_schema(toml_to_typeql(toml_text))

        assert "hardback" in schema.entities, "expected 'hardback' entity in parsed schema"
        hardback = schema.entities["hardback"]
        assert hardback.parent == "book", (
            f"expected hardback.parent='book', got {hardback.parent!r}"
        )

    def test_isbn_attribute_abstract_and_sub_child(self) -> None:
        """Parsed TOML must yield isbn attr abstract=True and isbn-13/isbn-10 with parent='isbn'."""
        toml_text = (FIXTURES_DIR / "annotations_inheritance.toml").read_text(encoding="utf-8")
        schema = parse_tql_schema(toml_to_typeql(toml_text))

        assert "isbn" in schema.attributes, "expected 'isbn' attribute"
        isbn = schema.attributes["isbn"]
        assert isbn.abstract is True, f"expected isbn.abstract=True, got {isbn.abstract!r}"

        assert "isbn-13" in schema.attributes, "expected 'isbn-13' attribute"
        isbn13 = schema.attributes["isbn-13"]
        assert isbn13.parent == "isbn", f"expected isbn-13.parent='isbn', got {isbn13.parent!r}"

        assert "isbn-10" in schema.attributes, "expected 'isbn-10' attribute"
        isbn10 = schema.attributes["isbn-10"]
        assert isbn10.parent == "isbn", f"expected isbn-10.parent='isbn', got {isbn10.parent!r}"

    def test_attribute_value_annotations(self) -> None:
        """Parsed TOML must yield status/reaction/age with correct value constraints."""
        toml_text = (FIXTURES_DIR / "annotations_inheritance.toml").read_text(encoding="utf-8")
        schema = parse_tql_schema(toml_to_typeql(toml_text))

        # @regex on status
        assert "status" in schema.attributes, "expected 'status' attribute"
        status = schema.attributes["status"]
        assert status.regex is not None, "expected status.regex to be set"
        assert "paid" in status.regex, f"expected regex to contain 'paid', got {status.regex!r}"

        # @values on reaction
        assert "reaction" in schema.attributes, "expected 'reaction' attribute"
        reaction = schema.attributes["reaction"]
        assert reaction.allowed_values is not None, "expected reaction.allowed_values to be set"
        assert "like" in reaction.allowed_values, (
            f"expected 'like' in reaction.allowed_values, got {reaction.allowed_values!r}"
        )

        # @range on age
        assert "age" in schema.attributes, "expected 'age' attribute"
        age = schema.attributes["age"]
        assert age.range_min == "0", f"expected age.range_min='0', got {age.range_min!r}"
        assert age.range_max == "150", f"expected age.range_max='150', got {age.range_max!r}"

    def test_generate_models_annotations_byte_identical(self, tmp_path: Path) -> None:
        """generate_models on annotations_inheritance.toml vs .tql produces identical
        model files — strong Inv-2 proof through the real renderer for the
        sub/abstract/annotation codegen paths."""
        toml_path = FIXTURES_DIR / "annotations_inheritance.toml"
        tql_path = FIXTURES_DIR / "annotations_inheritance.tql"

        out_toml = tmp_path / "out_toml"
        out_tql = tmp_path / "out_tql"

        generate_models(toml_path, out_toml)
        generate_models(tql_path, out_tql)

        model_files = [
            "attributes.py",
            "entities.py",
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


# ---------------------------------------------------------------------------
# Tests: functions + structs round-trip equivalence
# ---------------------------------------------------------------------------


class TestFunctionsStructsRoundtrip:
    """Round-trip equivalence for the functions and structs fixture.

    Reuses ``assert_roundtrip_equivalent`` (unchanged from 01) to assert that
    ``functions_structs.toml`` and its hand-written TQL mirror parse to
    structurally-equivalent IR.  Adds focused assertions on function parameters,
    return type (stream vs scalar), struct fields, and the optional field.

    Also verifies the body-discard invariant: ``FunctionSpec`` carries no body
    field regardless of which source (TOML or TQL) is used.
    """

    def test_functions_structs_roundtrip_equivalent(self) -> None:
        """functions_structs.toml and functions_structs.tql must parse to equivalent IR."""
        assert_roundtrip_equivalent(
            FIXTURES_DIR / "functions_structs.toml",
            FIXTURES_DIR / "functions_structs.tql",
        )

    def test_stream_return_function_parsed(self) -> None:
        """top-scorer must parse as a stream-return function with the correct parameter."""
        toml_text = (FIXTURES_DIR / "functions_structs.toml").read_text(encoding="utf-8")
        schema = parse_tql_schema(toml_to_typeql(toml_text))

        assert "top-scorer" in schema.functions, (
            f"expected 'top-scorer' in functions; got keys: {list(schema.functions.keys())}"
        )
        f = schema.functions["top-scorer"]
        assert f.name == "top-scorer", f"expected name='top-scorer', got {f.name!r}"
        assert len(f.parameters) == 1, f"expected 1 parameter, got {len(f.parameters)}"
        assert f.parameters[0].name == "g", (
            f"expected parameter name 'g', got {f.parameters[0].name!r}"
        )
        assert f.parameters[0].type == "game", (
            f"expected parameter type 'game', got {f.parameters[0].type!r}"
        )
        assert f.return_type.is_stream is True, (
            f"expected is_stream=True for top-scorer, got {f.return_type.is_stream!r}"
        )
        assert len(f.return_type.types) == 1, (
            f"expected 1 return type item, got {len(f.return_type.types)}"
        )
        assert f.return_type.types[0].name == "player", (
            f"expected return type 'player', got {f.return_type.types[0].name!r}"
        )

    def test_scalar_return_function_parsed(self) -> None:
        """max-score must parse as a scalar-return function."""
        toml_text = (FIXTURES_DIR / "functions_structs.toml").read_text(encoding="utf-8")
        schema = parse_tql_schema(toml_to_typeql(toml_text))

        assert "max-score" in schema.functions, (
            f"expected 'max-score' in functions; got keys: {list(schema.functions.keys())}"
        )
        f = schema.functions["max-score"]
        assert f.return_type.is_stream is False, (
            f"expected is_stream=False for max-score, got {f.return_type.is_stream!r}"
        )
        assert len(f.return_type.types) == 1, (
            f"expected 1 return type item, got {len(f.return_type.types)}"
        )
        assert f.return_type.types[0].name == "double", (
            f"expected return type 'double', got {f.return_type.types[0].name!r}"
        )

    def test_function_body_discarded_by_parser(self) -> None:
        """FunctionSpec must carry no body field — the parser discards it from
        both the TOML-transpiled and hand-written TQL sources."""
        toml_text = (FIXTURES_DIR / "functions_structs.toml").read_text(encoding="utf-8")
        schema_from_toml = parse_tql_schema(toml_to_typeql(toml_text))
        tql_text = (FIXTURES_DIR / "functions_structs.tql").read_text(encoding="utf-8")
        schema_from_tql = parse_tql_schema(tql_text)

        for func_name in ("top-scorer", "max-score"):
            for schema, source in ((schema_from_toml, "TOML"), (schema_from_tql, "TQL")):
                f = schema.functions[func_name]
                assert not hasattr(f, "body"), (
                    f"[{source}] FunctionSpec for {func_name!r} must not have a 'body' attribute"
                )

    def test_struct_fields_with_optional(self) -> None:
        """player-stats struct must have three fields; nickname must be optional."""
        toml_text = (FIXTURES_DIR / "functions_structs.toml").read_text(encoding="utf-8")
        schema = parse_tql_schema(toml_to_typeql(toml_text))

        assert "player-stats" in schema.structs, (
            f"expected 'player-stats' in structs; got keys: {list(schema.structs.keys())}"
        )
        s = schema.structs["player-stats"]
        assert len(s.fields) == 3, f"expected 3 fields, got {len(s.fields)}"

        field_map = {f.name: f for f in s.fields}
        assert "wins" in field_map, "expected 'wins' field"
        assert field_map["wins"].value_type == "integer", (
            f"expected wins.value_type='integer', got {field_map['wins'].value_type!r}"
        )
        assert field_map["wins"].optional is False, "wins must be non-optional"

        assert "nickname" in field_map, "expected 'nickname' field"
        assert field_map["nickname"].value_type == "string", (
            f"expected nickname.value_type='string', got {field_map['nickname'].value_type!r}"
        )
        assert field_map["nickname"].optional is True, "nickname must be optional"

    def test_generate_models_functions_structs_byte_identical(self, tmp_path: Path) -> None:
        """generate_models on functions_structs.toml vs .tql produces byte-for-byte
        identical model files — strong Inv-2 proof through the real renderer for the
        functions and structs codegen paths."""
        toml_path = FIXTURES_DIR / "functions_structs.toml"
        tql_path = FIXTURES_DIR / "functions_structs.tql"

        out_toml = tmp_path / "out_toml"
        out_tql = tmp_path / "out_tql"

        generate_models(toml_path, out_toml)
        generate_models(tql_path, out_tql)

        # functions.py and structs.py are conditionally generated (present when the
        # schema has functions/structs respectively); both fixture files have both.
        model_files = [
            "attributes.py",
            "entities.py",
            "relations.py",
            "functions.py",
            "structs.py",
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


# ---------------------------------------------------------------------------
# Tests: role cardinality round-trip equivalence
# ---------------------------------------------------------------------------


class TestRoleCardinalityRoundtrip:
    """Round-trip equivalence for the role-cardinality fixture.

    Asserts that ``role_cardinality.toml`` and its generated TQL mirror parse to
    structurally-equivalent IR.  Includes focused assertions for all four
    ``@card`` forms used in this corpus: 2..2 (exact), 1..1 (exact), 2..
    (unbounded), and 1..3 (range).
    """

    def test_role_cardinality_roundtrip_equivalent(self) -> None:
        """role_cardinality.toml and role_cardinality.tql must parse to equivalent IR."""
        assert_roundtrip_equivalent(
            FIXTURES_DIR / "role_cardinality.toml",
            FIXTURES_DIR / "role_cardinality.tql",
        )

    def test_all_four_card_forms_present(self) -> None:
        """Parsed TOML must yield relations carrying all four @card forms.

        is_similar_to: similar_memory @card(2..2)
        friendship:    friend @card(2..2)
        group_membership: group @card(1..1), member @card(2..)
        review:        document @card(1..1), reviewer @card(1..3)
        """
        from type_bridge.generator.models import Cardinality

        toml_text = (FIXTURES_DIR / "role_cardinality.toml").read_text(encoding="utf-8")
        schema = parse_tql_schema(toml_to_typeql(toml_text))

        # 2..2 — is_similar_to
        assert "is_similar_to" in schema.relations
        similar_roles = {r.name: r for r in schema.relations["is_similar_to"].roles}
        assert similar_roles["similar_memory"].cardinality == Cardinality(min=2, max=2), (
            f"expected Cardinality(2,2) on similar_memory, got "
            f"{similar_roles['similar_memory'].cardinality!r}"
        )

        # 2..2 — friendship
        assert "friendship" in schema.relations
        friend_roles = {r.name: r for r in schema.relations["friendship"].roles}
        assert friend_roles["friend"].cardinality == Cardinality(min=2, max=2), (
            f"expected Cardinality(2,2) on friend, got {friend_roles['friend'].cardinality!r}"
        )

        # 1..1 and 2.. — group_membership
        assert "group_membership" in schema.relations
        gm_roles = {r.name: r for r in schema.relations["group_membership"].roles}
        assert gm_roles["group"].cardinality == Cardinality(min=1, max=1), (
            f"expected Cardinality(1,1) on group role, got {gm_roles['group'].cardinality!r}"
        )
        assert gm_roles["member"].cardinality == Cardinality(min=2, max=None), (
            f"expected Cardinality(2,None) on member role, got {gm_roles['member'].cardinality!r}"
        )

        # 1..1 and 1..3 — review
        assert "review" in schema.relations
        rv_roles = {r.name: r for r in schema.relations["review"].roles}
        assert rv_roles["document"].cardinality == Cardinality(min=1, max=1), (
            f"expected Cardinality(1,1) on document role, got {rv_roles['document'].cardinality!r}"
        )
        assert rv_roles["reviewer"].cardinality == Cardinality(min=1, max=3), (
            f"expected Cardinality(1,3) on reviewer role, got {rv_roles['reviewer'].cardinality!r}"
        )

    def test_generate_models_role_cardinality_byte_identical(self, tmp_path: Path) -> None:
        """generate_models on role_cardinality.toml vs .tql produces identical model files."""
        toml_path = FIXTURES_DIR / "role_cardinality.toml"
        tql_path = FIXTURES_DIR / "role_cardinality.tql"

        out_toml = tmp_path / "out_toml"
        out_tql = tmp_path / "out_tql"

        generate_models(toml_path, out_toml)
        generate_models(tql_path, out_tql)

        model_files = ["attributes.py", "entities.py", "relations.py", "__init__.py", "registry.py"]
        for filename in model_files:
            toml_file = out_toml / filename
            tql_file = out_tql / filename
            if not toml_file.exists() and not tql_file.exists():
                continue
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
# Tests: social media round-trip equivalence
# ---------------------------------------------------------------------------


class TestSocialMediaRoundtrip:
    """Round-trip equivalence for the social-media fixture.

    Asserts that ``social_media.toml`` and its generated TQL mirror parse to
    structurally-equivalent IR.  Includes focused assertions on:
    - a ``relates X as Y`` super-role override (e.g. author as subject),
    - owns_order preservation on an entity with multiple ordered owns,
    - presence of abstract entities/relations and sub-type inheritance.
    """

    def test_social_media_roundtrip_equivalent(self) -> None:
        """social_media.toml and social_media.tql must parse to equivalent IR."""
        assert_roundtrip_equivalent(
            FIXTURES_DIR / "social_media.toml",
            FIXTURES_DIR / "social_media.tql",
        )

    def test_content_engagement_author_as_subject(self) -> None:
        """Parsed TOML must yield content-engagement relation with author role
        overriding subject (relates author as subject)."""
        toml_text = (FIXTURES_DIR / "social_media.toml").read_text(encoding="utf-8")
        schema = parse_tql_schema(toml_to_typeql(toml_text))

        assert "content-engagement" in schema.relations, (
            "expected 'content-engagement' in parsed relations"
        )
        ce = schema.relations["content-engagement"]
        role_map = {r.name: r for r in ce.roles}
        assert "author" in role_map, (
            f"expected 'author' role in content-engagement, got {list(role_map.keys())!r}"
        )
        assert role_map["author"].overrides == "subject", (
            f"expected author.overrides='subject', got {role_map['author'].overrides!r}"
        )

    def test_page_entity_owns_order_preserved(self) -> None:
        """Parsed TOML must yield page entity with owns_order matching TOML array order."""
        toml_text = (FIXTURES_DIR / "social_media.toml").read_text(encoding="utf-8")
        schema = parse_tql_schema(toml_to_typeql(toml_text))

        assert "page" in schema.entities, "expected 'page' entity in parsed schema"
        page = schema.entities["page"]
        # page sub content, so the inherited `id` (content's key) precedes page's
        # own declarations, which keep their TOML array order: name, bio, bio-version.
        expected_order = ["id", "name", "bio", "bio-version"]
        assert page.owns_order == expected_order, (
            f"expected page.owns_order={expected_order!r}, got {page.owns_order!r}"
        )

    def test_social_media_abstract_subtypes_preserved(self) -> None:
        """Full social-media TOML must preserve abstract subtypes across type families."""
        toml_text = (FIXTURES_DIR / "social_media.toml").read_text(encoding="utf-8")
        schema = parse_tql_schema(toml_to_typeql(toml_text))

        assert schema.entities["page"].abstract is True
        assert schema.entities["page"].parent == "content"
        assert schema.entities["post"].abstract is True
        assert schema.entities["post"].parent == "content"
        assert schema.relations["content-engagement"].abstract is True
        assert schema.relations["content-engagement"].parent == "interaction"
        assert schema.attributes["text-payload"].abstract is True
        assert schema.attributes["text-payload"].parent == "payload"
        assert schema.attributes["image-payload"].abstract is True
        assert schema.attributes["image-payload"].parent == "payload"

    def test_social_media_post_plays_cardinality_preserved(self) -> None:
        """The social-media post fixture carries plays posting:post @card(1)."""
        from type_bridge.generator.models import Cardinality

        toml_text = (FIXTURES_DIR / "social_media.toml").read_text(encoding="utf-8")
        schema = parse_tql_schema(toml_to_typeql(toml_text))

        post = schema.entities["post"]
        assert post.plays_cardinalities["posting:post"] == Cardinality(min=1, max=1)

    def test_generate_models_social_media_byte_identical(self, tmp_path: Path) -> None:
        """generate_models on social_media.toml vs .tql produces identical model files."""
        toml_path = FIXTURES_DIR / "social_media.toml"
        tql_path = FIXTURES_DIR / "social_media.tql"

        out_toml = tmp_path / "out_toml"
        out_tql = tmp_path / "out_tql"

        generate_models(toml_path, out_toml)
        generate_models(tql_path, out_tql)

        model_files = ["attributes.py", "entities.py", "relations.py", "__init__.py", "registry.py"]
        for filename in model_files:
            toml_file = out_toml / filename
            tql_file = out_tql / filename
            if not toml_file.exists() and not tql_file.exists():
                continue
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
# Tests: type-theoretic relation-plays corpus
# ---------------------------------------------------------------------------


class TestTypeTheoreticRelationPlays:
    """Relation-level plays coverage for the type-theoretic corpus.

    The Python generator IR does not currently retain relation-as-player plays,
    so these assertions intentionally use Rust ``TypeSchema`` as the oracle.
    """

    def test_type_theoretic_toml_transpiles_to_rust_schema(self) -> None:
        """The TOML mirror must be accepted by the Rust TypeSchema parser."""
        toml_text = (FIXTURES_DIR / "type_theoretic.toml").read_text(encoding="utf-8")
        typeql = toml_to_typeql(toml_text)
        schema = TypeSchema.from_typeql(typeql)

        assert "publication" in schema.relations
        assert "city" in schema.relations
        assert "world" in schema.entities

    def test_type_theoretic_relation_plays_preserved(self) -> None:
        """Relation-level plays must survive TOML -> TypeQL -> Rust TypeSchema."""
        toml_text = (FIXTURES_DIR / "type_theoretic.toml").read_text(encoding="utf-8")
        schema = TypeSchema.from_typeql(toml_to_typeql(toml_text))

        publication_plays = {
            entry["role_ref"] for entry in schema.get_all_plays_roles("publication")
        }
        assert publication_plays == {
            "contribution:work",
            "promotion-inclusion:item",
            "order-line:item",
            "review:reviewed",
            "recommendation:recommended",
        }

        city_plays = {entry["role_ref"] for entry in schema.get_all_plays_roles("city")}
        assert city_plays == {
            "publication:location",
            "user:location",
            "address:location",
        }

    def test_type_theoretic_toml_relation_plays_match_source_tql(self) -> None:
        """Compare relation-level plays against the source TQL through Rust TypeSchema."""
        toml_text = (FIXTURES_DIR / "type_theoretic.toml").read_text(encoding="utf-8")
        tql_text = (INTEGRATION_FIXTURES_DIR / "type_theoretic.tql").read_text(encoding="utf-8")

        toml_schema = TypeSchema.from_typeql(toml_to_typeql(toml_text))
        tql_schema = TypeSchema.from_typeql(tql_text)

        for relation in ("publication", "user", "order", "city", "country"):
            toml_plays = {entry["role_ref"] for entry in toml_schema.get_all_plays_roles(relation)}
            tql_plays = {entry["role_ref"] for entry in tql_schema.get_all_plays_roles(relation)}
            assert toml_plays == tql_plays, f"{relation} relation plays differ"


# ---------------------------------------------------------------------------
# Tests: bookstore corpus round-trip equivalence
# ---------------------------------------------------------------------------


class TestBookstoreRoundtrip:
    """Round-trip equivalence for the bookstore corpus fixture.

    Asserts that ``bookstore_corpus.toml`` and its generated TQL mirror parse to
    structurally-equivalent IR.  Includes focused assertions on:
    - stream-return and scalar-return functions with correct ``is_stream`` and params,
    - the body-discard invariant (FunctionSpec carries no ``body`` attribute),
    - sub-typed relations with ``relates ... as ...`` overrides,
    - entities with ``@key``, ``@unique``, ``@card`` owns annotations.
    """

    def test_bookstore_roundtrip_equivalent(self) -> None:
        """bookstore_corpus.toml and bookstore_corpus.tql must parse to equivalent IR."""
        assert_roundtrip_equivalent(
            FIXTURES_DIR / "bookstore_corpus.toml",
            FIXTURES_DIR / "bookstore_corpus.tql",
        )

    def test_contribution_contributor_role_is_abstract(self) -> None:
        """bookstore_corpus must round-trip with contributor role marked abstract.

        The `contribution` relation declares `relates contributor @abstract`; the
        child relations (`authoring`, `editing`, `illustrating`) override it via
        `relates X as contributor`.  The abstract flag must survive the TOML →
        TypeQL → ParsedSchema round-trip.
        """
        toml_text = (FIXTURES_DIR / "bookstore_corpus.toml").read_text(encoding="utf-8")
        schema = parse_tql_schema(toml_to_typeql(toml_text))

        assert "contribution" in schema.relations, "expected 'contribution' relation"
        role_map = {r.name: r for r in schema.relations["contribution"].roles}
        assert "contributor" in role_map, "expected 'contributor' role in contribution"
        assert role_map["contributor"].is_abstract is True, (
            f"expected contributor.is_abstract=True, got {role_map['contributor'].is_abstract!r}"
        )
        assert role_map["work"].is_abstract is False, "work role must NOT be abstract"

    def test_stream_and_scalar_functions_parsed(self) -> None:
        """Parsed TOML must yield at least one stream-return and one scalar-return function.

        Stream: is_review_verified_by_purchase, book_recommendations_for,
                book_recommendations_by_genre, book_recommendations_by_author,
                order_line_best_price, transitive_places
        Scalar: best_discount_for_item
        """
        toml_text = (FIXTURES_DIR / "bookstore_corpus.toml").read_text(encoding="utf-8")
        schema = parse_tql_schema(toml_to_typeql(toml_text))

        # At least one stream function
        stream_funcs = [name for name, fn in schema.functions.items() if fn.return_type.is_stream]
        assert len(stream_funcs) >= 1, (
            f"expected at least one stream-return function, got none; "
            f"functions: {list(schema.functions.keys())!r}"
        )

        # At least one scalar function
        scalar_funcs = [
            name for name, fn in schema.functions.items() if not fn.return_type.is_stream
        ]
        assert len(scalar_funcs) >= 1, (
            f"expected at least one scalar-return function, got none; "
            f"functions: {list(schema.functions.keys())!r}"
        )

        # Spot-check: is_review_verified_by_purchase is stream
        assert "is_review_verified_by_purchase" in schema.functions, (
            "expected 'is_review_verified_by_purchase' in functions"
        )
        f_stream = schema.functions["is_review_verified_by_purchase"]
        assert f_stream.return_type.is_stream is True, (
            f"expected is_stream=True for is_review_verified_by_purchase, "
            f"got {f_stream.return_type.is_stream!r}"
        )
        assert len(f_stream.parameters) == 1, (
            f"expected 1 parameter for is_review_verified_by_purchase, "
            f"got {len(f_stream.parameters)}"
        )
        assert f_stream.parameters[0].name == "review", (
            f"expected param name 'review', got {f_stream.parameters[0].name!r}"
        )
        assert f_stream.parameters[0].type == "review", (
            f"expected param type 'review', got {f_stream.parameters[0].type!r}"
        )

        # Spot-check: best_discount_for_item is scalar
        assert "best_discount_for_item" in schema.functions, (
            "expected 'best_discount_for_item' in functions"
        )
        f_scalar = schema.functions["best_discount_for_item"]
        assert f_scalar.return_type.is_stream is False, (
            f"expected is_stream=False for best_discount_for_item, "
            f"got {f_scalar.return_type.is_stream!r}"
        )
        assert len(f_scalar.parameters) == 2, (
            f"expected 2 parameters for best_discount_for_item, got {len(f_scalar.parameters)}"
        )

    def test_function_body_discarded_by_parser(self) -> None:
        """FunctionSpec must carry no body field — the parser discards it from
        both the TOML-transpiled and hand-written TQL sources."""
        toml_text = (FIXTURES_DIR / "bookstore_corpus.toml").read_text(encoding="utf-8")
        schema_from_toml = parse_tql_schema(toml_to_typeql(toml_text))
        tql_text = (FIXTURES_DIR / "bookstore_corpus.tql").read_text(encoding="utf-8")
        schema_from_tql = parse_tql_schema(tql_text)

        for func_name in ("is_review_verified_by_purchase", "best_discount_for_item"):
            for schema, source in ((schema_from_toml, "TOML"), (schema_from_tql, "TQL")):
                assert func_name in schema.functions, (
                    f"[{source}] expected {func_name!r} in functions"
                )
                f = schema.functions[func_name]
                assert not hasattr(f, "body"), (
                    f"[{source}] FunctionSpec for {func_name!r} must not have a 'body' attribute"
                )

    def test_generate_models_bookstore_byte_identical(self, tmp_path: Path) -> None:
        """generate_models on bookstore_corpus.toml vs .tql produces identical model files,
        including functions.py — strong Inv-2 proof through the full renderer."""
        toml_path = FIXTURES_DIR / "bookstore_corpus.toml"
        tql_path = FIXTURES_DIR / "bookstore_corpus.tql"

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
        # functions.py is conditionally generated when the schema has functions
        conditional_files = ["functions.py"]

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

        for filename in conditional_files:
            toml_file = out_toml / filename
            tql_file = out_tql / filename
            if toml_file.exists() or tql_file.exists():
                assert toml_file.exists(), f"TOML output missing conditional: {filename}"
                assert tql_file.exists(), f"TQL output missing conditional: {filename}"
                toml_content = toml_file.read_text(encoding="utf-8")
                tql_content = tql_file.read_text(encoding="utf-8")
                assert toml_content == tql_content, (
                    f"{filename} differs between TOML and TQL generate_models outputs.\n"
                    f"--- TOML output ({filename}) ---\n{toml_content}\n"
                    f"--- TQL output ({filename}) ---\n{tql_content}\n"
                )


# ---------------------------------------------------------------------------
# Tests: list-interface (ordered / distinct) round-trip equivalence
# ---------------------------------------------------------------------------


class TestListInterfaceRoundtrip:
    """Focused assertions that ordered_owns, distinct_owns, and role ordered/distinct
    flags survive the TOML -> TypeQL -> ParsedSchema round-trip.

    Uses the bookstore_corpus fixtures which contain:
    - ``book`` entity: ``owns tag[] @distinct`` (ordered + distinct)
    - ``rating`` relation: ``relates reviewer[] @distinct`` (ordered + distinct)
    """

    def test_book_entity_tag_is_ordered_and_distinct(self) -> None:
        """book.ordered_owns must contain 'tag'; book.distinct_owns must contain 'tag'."""
        toml_text = (FIXTURES_DIR / "bookstore_corpus.toml").read_text(encoding="utf-8")
        schema = parse_tql_schema(toml_to_typeql(toml_text))

        assert "book" in schema.entities, "expected 'book' entity"
        book = schema.entities["book"]
        assert "tag" in book.ordered_owns, (
            f"expected 'tag' in book.ordered_owns, got {book.ordered_owns!r}"
        )
        assert "tag" in book.distinct_owns, (
            f"expected 'tag' in book.distinct_owns, got {book.distinct_owns!r}"
        )

    def test_book_tql_entity_tag_is_ordered_and_distinct(self) -> None:
        """Same assertion reading from the TQL fixture directly."""
        tql_text = (FIXTURES_DIR / "bookstore_corpus.tql").read_text(encoding="utf-8")
        schema = parse_tql_schema(tql_text)

        assert "book" in schema.entities, "expected 'book' entity"
        book = schema.entities["book"]
        assert "tag" in book.ordered_owns, (
            f"[TQL] expected 'tag' in book.ordered_owns, got {book.ordered_owns!r}"
        )
        assert "tag" in book.distinct_owns, (
            f"[TQL] expected 'tag' in book.distinct_owns, got {book.distinct_owns!r}"
        )

    def test_rating_relation_reviewer_role_ordered_and_distinct(self) -> None:
        """rating relation: reviewer role must have ordered=True and distinct=True."""
        toml_text = (FIXTURES_DIR / "bookstore_corpus.toml").read_text(encoding="utf-8")
        schema = parse_tql_schema(toml_to_typeql(toml_text))

        assert "rating" in schema.relations, "expected 'rating' relation"
        rating = schema.relations["rating"]
        role_map = {r.name: r for r in rating.roles}
        assert "reviewer" in role_map, (
            f"expected 'reviewer' role in rating, got {list(role_map.keys())!r}"
        )
        reviewer = role_map["reviewer"]
        assert reviewer.ordered is True, f"expected reviewer.ordered=True, got {reviewer.ordered!r}"
        assert reviewer.distinct is True, (
            f"expected reviewer.distinct=True, got {reviewer.distinct!r}"
        )

    def test_rating_tql_reviewer_role_ordered_and_distinct(self) -> None:
        """Same assertion reading from the TQL fixture directly."""
        tql_text = (FIXTURES_DIR / "bookstore_corpus.tql").read_text(encoding="utf-8")
        schema = parse_tql_schema(tql_text)

        assert "rating" in schema.relations, "expected 'rating' relation"
        rating = schema.relations["rating"]
        role_map = {r.name: r for r in rating.roles}
        assert "reviewer" in role_map, (
            f"[TQL] expected 'reviewer' role in rating, got {list(role_map.keys())!r}"
        )
        reviewer = role_map["reviewer"]
        assert reviewer.ordered is True, (
            f"[TQL] expected reviewer.ordered=True, got {reviewer.ordered!r}"
        )
        assert reviewer.distinct is True, (
            f"[TQL] expected reviewer.distinct=True, got {reviewer.distinct!r}"
        )

    def test_non_list_owns_not_in_ordered_owns(self) -> None:
        """Attributes that are NOT declared as list must NOT appear in ordered_owns."""
        toml_text = (FIXTURES_DIR / "bookstore_corpus.toml").read_text(encoding="utf-8")
        schema = parse_tql_schema(toml_to_typeql(toml_text))

        book = schema.entities["book"]
        # title, page-count, genre, price are all non-list on book
        for attr in ("title", "page-count", "price"):
            assert attr not in book.ordered_owns, (
                f"expected '{attr}' NOT in book.ordered_owns (not a list attribute)"
            )


# ---------------------------------------------------------------------------
# Tests: example schema.toml smoke
# ---------------------------------------------------------------------------


class TestExampleSchemaToml:
    """Smoke test for the examples/basic/schema.toml fixture.

    Asserts that the example schema transpiles and generates a model package.
    This test exercises the full path: TOML DSL → TypeQL → ParsedSchema →
    generate_models output directory.
    """

    def test_example_schema_transpiles_and_generates(self, tmp_path: Path) -> None:
        """examples/basic/schema.toml must transpile to valid TypeQL and generate a package.

        Locates the file relative to this test module (tests/unit/generator/ → repo root
        is three parents up).
        """
        # tests/unit/generator/test_toml_roundtrip.py → parents[0]=generator,
        # parents[1]=unit, parents[2]=tests, parents[3]=repo root
        repo_root = Path(__file__).resolve().parents[3]
        schema_toml = repo_root / "examples" / "basic" / "schema.toml"

        assert schema_toml.exists(), (
            f"examples/basic/schema.toml not found at {schema_toml}; "
            "a separate agent authors this file"
        )

        toml_text = schema_toml.read_text(encoding="utf-8")
        parsed = parse_tql_schema(toml_to_typeql(toml_text))
        assert parsed is not None, "parse_tql_schema returned None for example schema"

        out_dir = tmp_path / "out"
        generate_models(schema_toml, out_dir)
        assert (out_dir / "__init__.py").exists(), (
            f"generate_models did not produce __init__.py in {out_dir}"
        )
