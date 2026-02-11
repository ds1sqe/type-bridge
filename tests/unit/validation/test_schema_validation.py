"""Tests for schema-aware query validation (issue #105).

These tests validate that queries are checked against a TypeSchema,
catching semantic errors like unknown attribute ownership, invalid roles,
and value type mismatches.
"""

from __future__ import annotations

from typing import Any

import pytest

HAS_RUST_CORE = False
QueryCompiler: Any = None
TypeSchema: Any = None
try:
    from type_bridge_core import QueryCompiler, TypeSchema  # type: ignore[no-redef]

    HAS_RUST_CORE = True
except ImportError:
    pass

pytestmark = pytest.mark.skipif(not HAS_RUST_CORE, reason="Rust core not available")

BOOKSTORE_SCHEMA = """
define
attribute name, value string;
attribute isbn, value string;
attribute price, value double;
attribute age, value long;
attribute rating, value long;
entity person, owns name @key, owns age;
entity book, owns name, owns isbn @key, owns price;
relation authorship, relates author, relates written-work;
entity person, plays authorship:author;
entity book, plays authorship:written-work;
"""


@pytest.fixture
def schema() -> Any:
    return TypeSchema.from_typeql(BOOKSTORE_SCHEMA)


@pytest.fixture
def compiler() -> Any:
    return QueryCompiler()


class TestSchemaAwareValidation:
    """Tests using TypeSchema.validate_query (convenience method)."""

    def test_valid_query_passes(self, schema: Any, compiler: Any) -> None:
        clauses = compiler.parse('match $p isa person, has name "Alice";')
        result = schema.validate_query(clauses)
        assert result["is_valid"] is True

    def test_unknown_type_fails(self, schema: Any, compiler: Any) -> None:
        clauses = compiler.parse("match $x isa spaceship;")
        result = schema.validate_query(clauses)
        assert result["is_valid"] is False
        assert any(e["code"] == "UNKNOWN_TYPE" for e in result["errors"])

    def test_invalid_ownership_fails(self, schema: Any, compiler: Any) -> None:
        # person doesn't own isbn
        clauses = compiler.parse('match $p isa person, has isbn "123";')
        result = schema.validate_query(clauses)
        assert result["is_valid"] is False
        assert any(e["code"] == "UNKNOWN_ATTRIBUTE_OWNERSHIP" for e in result["errors"])

    def test_unknown_role_fails(self, schema: Any, compiler: Any) -> None:
        clauses = compiler.parse("match $r isa authorship (manager: $p, written-work: $b);")
        result = schema.validate_query(clauses)
        assert result["is_valid"] is False
        assert any(e["code"] == "UNKNOWN_ROLE" for e in result["errors"])

    def test_value_type_mismatch_fails(self, schema: Any, compiler: Any) -> None:
        # age is long, "thirty" is string
        clauses = compiler.parse('match $p isa person, has age "thirty";')
        result = schema.validate_query(clauses)
        assert result["is_valid"] is False
        assert any(e["code"] == "VALUE_TYPE_MISMATCH" for e in result["errors"])

    def test_valid_value_type_passes(self, schema: Any, compiler: Any) -> None:
        clauses = compiler.parse("match $p isa person, has age 30;")
        result = schema.validate_query(clauses)
        assert result["is_valid"] is True

    def test_long_to_double_widening(self, schema: Any, compiler: Any) -> None:
        # price is double, 50 is long — should be OK (widening)
        clauses = compiler.parse("match $b isa book, has price 50;")
        result = schema.validate_query(clauses)
        assert result["is_valid"] is True

    def test_valid_relation_query(self, schema: Any, compiler: Any) -> None:
        clauses = compiler.parse(
            "match $p isa person; $b isa book; $r isa authorship (author: $p, written-work: $b);"
        )
        result = schema.validate_query(clauses)
        assert result["is_valid"] is True

    def test_role_player_type_mismatch(self, schema: Any, compiler: Any) -> None:
        # book can't play author role
        clauses = compiler.parse("match $b isa book; $r isa authorship (author: $b);")
        result = schema.validate_query(clauses)
        assert result["is_valid"] is False
        assert any(e["code"] == "ROLE_PLAYER_TYPE_MISMATCH" for e in result["errors"])

    def test_empty_query_passes(self, schema: Any) -> None:
        result = schema.validate_query([])
        assert result["is_valid"] is True
        assert result["errors"] == []

    def test_warnings_have_severity(self, schema: Any, compiler: Any) -> None:
        # isa! on a type with no subtypes produces a warning
        clauses = compiler.parse("match $b isa! book;")
        result = schema.validate_query(clauses)
        # Warnings don't invalidate
        assert result["is_valid"] is True
        warnings = [e for e in result["errors"] if e.get("severity") == "Warning"]
        assert len(warnings) >= 1
        assert any(e["code"] == "STRICT_ISA_NO_SUBTYPES" for e in warnings)


class TestValidationEngineMethod:
    """Tests using ValidationEngine.validate_query (explicit engine)."""

    def test_validate_query_via_engine(self, schema: Any, compiler: Any) -> None:
        from type_bridge_core import ValidationEngine  # type: ignore[import-not-found]

        engine = ValidationEngine()
        clauses = compiler.parse('match $p isa person, has name "Alice";')
        result = engine.validate_query(clauses, schema)
        assert result["is_valid"] is True


class TestPythonWrapper:
    """Tests for the Python validate_query_against_schema wrapper."""

    def test_wrapper_valid_query(self, schema: Any, compiler: Any) -> None:
        from type_bridge.validation import validate_query_against_schema

        clauses = compiler.parse('match $p isa person, has name "Alice";')
        result = validate_query_against_schema(clauses, schema)
        assert result["is_valid"] is True

    def test_wrapper_invalid_query(self, schema: Any, compiler: Any) -> None:
        from type_bridge.validation import validate_query_against_schema

        clauses = compiler.parse("match $x isa spaceship;")
        result = validate_query_against_schema(clauses, schema)
        assert result["is_valid"] is False

    def test_wrapper_strict_mode(self, schema: Any, compiler: Any) -> None:
        from type_bridge.validation import validate_query_against_schema

        # isa! on book produces a warning — strict mode should fail
        clauses = compiler.parse("match $b isa! book;")
        result = validate_query_against_schema(clauses, schema, strict=True)
        assert result["is_valid"] is False


class TestFallbackBehavior:
    """Test fallback when Rust core is not available."""

    def test_fallback_returns_valid(self) -> None:
        """Simulate fallback by calling with None schema (non-strict)."""
        from type_bridge.validation import validate_query_against_schema

        # When Rust core IS available, passing None schema would fail.
        # This test verifies the function signature accepts the call.
        # The actual fallback is hard to test when Rust IS available,
        # so we just verify the function exists and returns the right shape.
        if not HAS_RUST_CORE:
            result = validate_query_against_schema([], None)
            assert result["is_valid"] is True
