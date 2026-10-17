"""Benchmarks: name, pattern and statement validation through the native core."""

from __future__ import annotations

# ---------------------------------------------------------------------------
# Single name
# ---------------------------------------------------------------------------


def test_validate_single_name(benchmark, validator):
    benchmark(validator.validate_type_name, "person", "entity")


# ---------------------------------------------------------------------------
# Long name (100+ characters)
# ---------------------------------------------------------------------------

_LONG_NAME = "a" * 100 + "-type"


def test_validate_long_name(benchmark, validator):
    benchmark(validator.validate_type_name, _LONG_NAME, "entity")


# ---------------------------------------------------------------------------
# Unicode name
# ---------------------------------------------------------------------------


def test_validate_unicode_name(benchmark, validator):
    benchmark(validator.validate_type_name, "\u00e9l\u00e8ve", "entity")


# ---------------------------------------------------------------------------
# Reserved word (expected rejection)
# ---------------------------------------------------------------------------


def test_validate_reserved_word(benchmark, validator):
    def _validate_reserved():
        try:
            validator.validate_type_name("match", "entity")
        except ValueError:
            pass

    benchmark(_validate_reserved)


# ---------------------------------------------------------------------------
# Batch: 1000 names
# ---------------------------------------------------------------------------


def test_validate_batch(benchmark, validator, type_names):
    def _batch():
        for name in type_names:
            validator.validate_type_name(name, "entity")

    benchmark(_batch)


# ---------------------------------------------------------------------------
# Variable name validation
# ---------------------------------------------------------------------------


def test_validate_variable_valid(benchmark, validator):
    benchmark(validator.validate_variable_name, "$person", "entity")


def test_validate_variable_invalid(benchmark, validator):
    def _validate():
        try:
            validator.validate_variable_name("person", "entity")
        except ValueError:
            pass

    benchmark(_validate)


# ---------------------------------------------------------------------------
# Multi-context type validation
# ---------------------------------------------------------------------------


def test_validate_relation_context(benchmark, validator):
    benchmark(validator.validate_type_name, "employment", "relation")


def test_validate_attribute_context(benchmark, validator):
    benchmark(validator.validate_type_name, "first-name", "attribute")


# ---------------------------------------------------------------------------
# Invalid name rejection
# ---------------------------------------------------------------------------


def test_validate_empty_name(benchmark, validator):
    def _validate():
        try:
            validator.validate_type_name("", "entity")
        except ValueError:
            pass

    benchmark(_validate)


def test_validate_digit_start(benchmark, validator):
    def _validate():
        try:
            validator.validate_type_name("1st-entity", "entity")
        except ValueError:
            pass

    benchmark(_validate)


# ---------------------------------------------------------------------------
# Batch scaling: variable names (500) and large type names (5000)
# ---------------------------------------------------------------------------


def test_validate_batch_variable(benchmark, validator, variable_names):
    def _batch():
        for name in variable_names:
            validator.validate_variable_name(name, "entity")

    benchmark(_batch)


def test_validate_batch_5000(benchmark, validator, type_names_5000):
    def _batch():
        for name in type_names_5000:
            validator.validate_type_name(name, "entity")

    benchmark(_batch)


# ---------------------------------------------------------------------------
# Role context validation
# ---------------------------------------------------------------------------


def test_validate_role_context(benchmark, validator):
    benchmark(validator.validate_type_name, "employee", "role")


# ---------------------------------------------------------------------------
# Pattern validation
# ---------------------------------------------------------------------------


def test_validate_pattern_simple(benchmark, validator):
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
    benchmark(validator.validate_pattern, pattern)


def test_validate_pattern_complex(benchmark, validator):
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
    benchmark(validator.validate_pattern, pattern)


# ---------------------------------------------------------------------------
# Statement validation
# ---------------------------------------------------------------------------


def test_validate_statement_simple(benchmark, validator):
    """Validate a simple Isa statement via Rust depythonize."""
    stmt = {
        "type": "Isa",
        "data": {"variable": "$p", "type_name": "person"},
    }
    benchmark(validator.validate_statement, stmt)


def test_validate_statement_relation(benchmark, validator):
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
    benchmark(validator.validate_statement, stmt)
