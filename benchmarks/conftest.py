"""Fixtures for the native validation benchmarks."""

import pytest
from type_bridge_core import ValidationEngine


@pytest.fixture
def validator() -> ValidationEngine:
    return ValidationEngine()


@pytest.fixture
def type_names() -> list[str]:
    """1000 type names: mix of simple, hyphenated, long, and unicode."""
    names: list[str] = []
    for i in range(400):
        names.append(f"entity-type-{i}")
    for i in range(300):
        names.append(f"my-long-attribute-name-for-testing-{i}")
    for i in range(200):
        names.append(f"relation-{i}-data")
    for i in range(100):
        names.append(f"\u00e9l\u00e8ve-{i}")  # unicode names
    return names
