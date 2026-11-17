"""Canonical generated-contract enum spelling through the native core."""

from type_bridge_core import canonical_enum_spelling


def test_canonical_spelling(benchmark):
    assert benchmark(canonical_enum_spelling, "value_type", "datetime-tz") == "datetime-tz"


def test_alias(benchmark):
    assert benchmark(canonical_enum_spelling, "manager_lookup", "exact") == "eq"
