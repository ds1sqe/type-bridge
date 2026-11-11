"""Benchmarks: type and variable name validation through the native core."""

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
