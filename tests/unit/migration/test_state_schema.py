"""Public migration-state schema contract and filtering tests."""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import FrozenInstanceError
from typing import Literal

import pytest

import type_bridge.migration as migration
from type_bridge.migration import (
    MIGRATION_STATE_SCHEMA,
    MigrationStateSchema,
    is_migration_state_type,
    migration_state_schema,
)

EXPECTED_ENTITIES = frozenset(
    {
        "type_bridge_migration",
        "type_bridge_migration_run",
    }
)
EXPECTED_ATTRIBUTES = frozenset(
    {
        "migration_id",
        "migration_app_label",
        "migration_name",
        "migration_applied_at",
        "migration_checksum",
        "migration_run_id",
        "migration_direction",
        "migration_status",
        "migration_started_at",
        "migration_finished_at",
        "migration_error",
        "migration_executor_ip",
        "migration_executor_mac",
    }
)


class _UncheckedMigrationStatePredicate:
    """Checked runtime adapter for testing a hostile schema-kind value."""

    def __init__(self, candidate: object) -> None:
        if not callable(candidate):
            raise AssertionError("migration-state predicate is not callable")
        self._candidate: Callable[..., object] = candidate

    def __call__(self, *, kind: str, label: str) -> bool:
        return bool(self._candidate(kind=kind, label=label))


def _unchecked_migration_state_predicate() -> _UncheckedMigrationStatePredicate:
    return _UncheckedMigrationStatePredicate(is_migration_state_type)


def test_migration_state_schema_projects_the_canonical_rust_descriptor() -> None:
    descriptor = migration_state_schema()

    assert MIGRATION_STATE_SCHEMA == MigrationStateSchema(
        entities=frozenset(descriptor["entities"]),
        relations=frozenset(descriptor["relations"]),
        attributes=frozenset(descriptor["attributes"]),
        roles=frozenset(
            f"{relation_name}:{role['role_name']}"
            for relation_name, relation in descriptor["relations"].items()
            for role in relation.get("roles", [])
        ),
    )
    assert MIGRATION_STATE_SCHEMA.entities == EXPECTED_ENTITIES
    assert MIGRATION_STATE_SCHEMA.relations == frozenset()
    assert MIGRATION_STATE_SCHEMA.attributes == EXPECTED_ATTRIBUTES
    assert MIGRATION_STATE_SCHEMA.roles == frozenset()
    assert "type_bridge_migration" in MIGRATION_STATE_SCHEMA.entities


def test_migration_state_schema_projection_is_immutable() -> None:
    with pytest.raises(FrozenInstanceError):
        MIGRATION_STATE_SCHEMA.entities = frozenset()  # type: ignore[misc]

    with pytest.raises(AttributeError):
        MIGRATION_STATE_SCHEMA.entities.add("another-label")  # type: ignore[attr-defined]


@pytest.mark.parametrize(
    ("kind", "labels"),
    [
        ("entity", EXPECTED_ENTITIES),
        ("attribute", EXPECTED_ATTRIBUTES),
    ],
)
def test_predicate_recognizes_every_canonical_label(
    kind: Literal["entity", "attribute"], labels: frozenset[str]
) -> None:
    for label in labels:
        assert is_migration_state_type(kind=kind, label=label)

    assert not is_migration_state_type(kind=kind, label="application-label")


def test_predicate_is_keyword_only() -> None:
    with pytest.raises(TypeError):
        is_migration_state_type("entity", "type_bridge_migration")  # type: ignore[call-arg]


def test_predicate_rejects_unknown_schema_kind() -> None:
    with pytest.raises(ValueError, match="kind"):
        _unchecked_migration_state_predicate()(
            kind="owner",
            label="type_bridge_migration",
        )


def test_public_migration_package_exports_state_schema_contract() -> None:
    expected = {
        "MIGRATION_STATE_SCHEMA",
        "MigrationStateSchema",
        "is_migration_state_type",
        "migration_state_schema",
    }

    assert expected <= set(migration.__all__)
    for name in expected:
        assert getattr(migration, name) is globals()[name]
