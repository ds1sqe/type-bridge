"""Benchmarks: Python vs Rust validation performance."""

from __future__ import annotations

import pytest

from type_bridge.reserved_words import is_reserved_word
from type_bridge.validation import _is_xid_continue, _is_xid_start

pytestmark = pytest.mark.benchmark


# ---------------------------------------------------------------------------
# Python-only validation (direct calls, no delegation)
# ---------------------------------------------------------------------------


def _validate_python(name: str, context: str) -> bool:
    """Pure-Python validation path (mirrors validation.py fallback)."""
    if not name:
        return False
    if is_reserved_word(name):
        return False
    if not _is_xid_start(name[0]):
        return False
    for ch in name[1:]:
        if not _is_xid_continue(ch):
            return False
    return True


def _validate_variable_python(name: str) -> bool:
    """Pure-Python variable validation path."""
    if not name.startswith("$"):
        return False
    if len(name) == 1:
        return False
    return True


# ---------------------------------------------------------------------------
# Single name
# ---------------------------------------------------------------------------


def test_validate_single_name_python(benchmark):
    benchmark(_validate_python, "person", "entity")


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_validate_single_name_rust(benchmark, rust_validator):
    benchmark(rust_validator.validate_type_name, "person", "entity")


# ---------------------------------------------------------------------------
# Long name (100+ characters)
# ---------------------------------------------------------------------------

_LONG_NAME = "a" * 100 + "-type"


def test_validate_long_name_python(benchmark):
    benchmark(_validate_python, _LONG_NAME, "entity")


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_validate_long_name_rust(benchmark, rust_validator):
    benchmark(rust_validator.validate_type_name, _LONG_NAME, "entity")


# ---------------------------------------------------------------------------
# Unicode name
# ---------------------------------------------------------------------------


def test_validate_unicode_name_python(benchmark):
    benchmark(_validate_python, "\u00e9l\u00e8ve", "entity")


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_validate_unicode_name_rust(benchmark, rust_validator):
    benchmark(rust_validator.validate_type_name, "\u00e9l\u00e8ve", "entity")


# ---------------------------------------------------------------------------
# Reserved word (expected rejection)
# ---------------------------------------------------------------------------


def test_validate_reserved_word_python(benchmark):
    benchmark(_validate_python, "match", "entity")


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_validate_reserved_word_rust(benchmark, rust_validator):
    def _validate_reserved():
        try:
            rust_validator.validate_type_name("match", "entity")
        except ValueError:
            pass

    benchmark(_validate_reserved)


# ---------------------------------------------------------------------------
# Batch: 1000 names
# ---------------------------------------------------------------------------


def test_validate_batch_python(benchmark, type_names):
    def _batch():
        for name in type_names:
            _validate_python(name, "entity")

    benchmark(_batch)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_validate_batch_rust(benchmark, rust_validator, type_names):
    def _batch():
        for name in type_names:
            rust_validator.validate_type_name(name, "entity")

    benchmark(_batch)


# ---------------------------------------------------------------------------
# Variable name validation
# ---------------------------------------------------------------------------


def test_validate_variable_valid_python(benchmark):
    benchmark(_validate_variable_python, "$person")


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_validate_variable_valid_rust(benchmark, rust_validator):
    benchmark(rust_validator.validate_variable_name, "$person", "entity")


def test_validate_variable_invalid_python(benchmark):
    benchmark(_validate_variable_python, "person")


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_validate_variable_invalid_rust(benchmark, rust_validator):
    def _validate():
        try:
            rust_validator.validate_variable_name("person", "entity")
        except ValueError:
            pass

    benchmark(_validate)


# ---------------------------------------------------------------------------
# Multi-context type validation
# ---------------------------------------------------------------------------


def test_validate_relation_context_python(benchmark):
    benchmark(_validate_python, "employment", "relation")


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_validate_relation_context_rust(benchmark, rust_validator):
    benchmark(rust_validator.validate_type_name, "employment", "relation")


def test_validate_attribute_context_python(benchmark):
    benchmark(_validate_python, "first-name", "attribute")


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_validate_attribute_context_rust(benchmark, rust_validator):
    benchmark(rust_validator.validate_type_name, "first-name", "attribute")


# ---------------------------------------------------------------------------
# Invalid name rejection
# ---------------------------------------------------------------------------


def test_validate_empty_name_python(benchmark):
    benchmark(_validate_python, "", "entity")


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_validate_empty_name_rust(benchmark, rust_validator):
    def _validate():
        try:
            rust_validator.validate_type_name("", "entity")
        except ValueError:
            pass

    benchmark(_validate)


def test_validate_digit_start_python(benchmark):
    benchmark(_validate_python, "1st-entity", "entity")


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_validate_digit_start_rust(benchmark, rust_validator):
    def _validate():
        try:
            rust_validator.validate_type_name("1st-entity", "entity")
        except ValueError:
            pass

    benchmark(_validate)


# ---------------------------------------------------------------------------
# Batch scaling: variable names (500) and large type names (5000)
# ---------------------------------------------------------------------------


def test_validate_batch_variable_python(benchmark, variable_names):
    def _batch():
        for name in variable_names:
            _validate_variable_python(name)

    benchmark(_batch)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_validate_batch_variable_rust(benchmark, rust_validator, variable_names):
    def _batch():
        for name in variable_names:
            rust_validator.validate_variable_name(name, "entity")

    benchmark(_batch)


def test_validate_batch_5000_python(benchmark, type_names_5000):
    def _batch():
        for name in type_names_5000:
            _validate_python(name, "entity")

    benchmark(_batch)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_validate_batch_5000_rust(benchmark, rust_validator, type_names_5000):
    def _batch():
        for name in type_names_5000:
            rust_validator.validate_type_name(name, "entity")

    benchmark(_batch)


# ---------------------------------------------------------------------------
# Role context validation
# ---------------------------------------------------------------------------


def test_validate_role_context_python(benchmark):
    benchmark(_validate_python, "employee", "role")


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_validate_role_context_rust(benchmark, rust_validator):
    benchmark(rust_validator.validate_type_name, "employee", "role")


# ---------------------------------------------------------------------------
# Pattern validation (Rust-only — no Python equivalent)
# ---------------------------------------------------------------------------


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_validate_pattern_simple_rust(benchmark, rust_validator):
    """Validate a simple entity pattern via Rust depythonize."""
    pattern = {
        "type": "Entity",
        "data": {
            "variable": "$p",
            "type_name": "person",
            "constraints": [
                {
                    "type": "Has",
                    "data": {
                        "attr_name": "name",
                        "value": {
                            "type": "Literal",
                            "data": {"value": "Alice", "value_type": "string"},
                        },
                    },
                }
            ],
            "is_strict": False,
        },
    }
    benchmark(rust_validator.validate_pattern, pattern)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_validate_pattern_complex_rust(benchmark, rust_validator):
    """Validate a complex nested pattern (Or + Relation + Not) via Rust."""
    pattern = {
        "type": "Or",
        "data": [
            [
                {
                    "type": "Entity",
                    "data": {
                        "variable": "$p",
                        "type_name": "person",
                        "constraints": [
                            {
                                "type": "Has",
                                "data": {
                                    "attr_name": "name",
                                    "value": {
                                        "type": "Literal",
                                        "data": {"value": "Alice", "value_type": "string"},
                                    },
                                },
                            },
                            {
                                "type": "Has",
                                "data": {
                                    "attr_name": "age",
                                    "value": {
                                        "type": "Literal",
                                        "data": {"value": 30, "value_type": "long"},
                                    },
                                },
                            },
                        ],
                        "is_strict": False,
                    },
                },
                {
                    "type": "Relation",
                    "data": {
                        "variable": "$r",
                        "type_name": "employment",
                        "role_players": [
                            {"role": "employee", "player_var": "$p"},
                            {"role": "employer", "player_var": "$c"},
                        ],
                        "constraints": [],
                    },
                },
            ],
            [
                {
                    "type": "Not",
                    "data": [
                        {
                            "type": "Entity",
                            "data": {
                                "variable": "$p",
                                "type_name": "retired-person",
                                "constraints": [],
                                "is_strict": True,
                            },
                        }
                    ],
                }
            ],
        ],
    }
    benchmark(rust_validator.validate_pattern, pattern)


# ---------------------------------------------------------------------------
# Statement validation (Rust-only — no Python equivalent)
# ---------------------------------------------------------------------------


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_validate_statement_simple_rust(benchmark, rust_validator):
    """Validate a simple Isa statement via Rust depythonize."""
    stmt = {
        "type": "Isa",
        "data": {"variable": "$p", "type_name": "person"},
    }
    benchmark(rust_validator.validate_statement, stmt)


@pytest.mark.skipif(
    not pytest.importorskip("type_bridge_core", reason="Rust core not installed"),
    reason="",
)
def test_validate_statement_relation_rust(benchmark, rust_validator):
    """Validate a Relation statement with role players and attributes via Rust."""
    stmt = {
        "type": "Relation",
        "data": {
            "variable": "$rel",
            "type_name": "employment",
            "role_players": [
                {"role": "employee", "player_var": "$p"},
                {"role": "employer", "player_var": "$c"},
            ],
            "include_variable": True,
            "attributes": [
                {
                    "type": "Has",
                    "data": {
                        "subject_var": "$rel",
                        "attr_name": "start-date",
                        "value": {
                            "type": "Literal",
                            "data": {"value": "2024-01-15", "value_type": "date"},
                        },
                    },
                },
                {
                    "type": "Has",
                    "data": {
                        "subject_var": "$rel",
                        "attr_name": "salary",
                        "value": {
                            "type": "Literal",
                            "data": {"value": 95000, "value_type": "long"},
                        },
                    },
                },
            ],
        },
    }
    benchmark(rust_validator.validate_statement, stmt)
