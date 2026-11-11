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


@pytest.fixture
def variable_names() -> list[str]:
    """500 variable names for batch validation."""
    names: list[str] = []
    for i in range(200):
        names.append(f"$entity-var-{i}")
    for i in range(150):
        names.append(f"$long-variable-name-for-testing-purpose-{i}")
    for i in range(100):
        names.append(f"$r{i}")
    for i in range(50):
        names.append(f"$\u00e9l\u00e8ve-{i}")
    return names


@pytest.fixture
def type_names_5000() -> list[str]:
    """5000 type names for large batch validation."""
    names: list[str] = []
    for i in range(2000):
        names.append(f"entity-type-{i}")
    for i in range(1500):
        names.append(f"my-long-attribute-name-for-testing-{i}")
    for i in range(1000):
        names.append(f"relation-{i}-data")
    for i in range(500):
        names.append(f"\u00e9l\u00e8ve-{i}")
    return names
