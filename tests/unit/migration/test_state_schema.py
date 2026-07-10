"""Public migration-state schema contract and filtering tests."""

from __future__ import annotations

from dataclasses import FrozenInstanceError

import pytest

import type_bridge.migration as migration
import type_bridge.migration.state_schema as state_schema_module
from type_bridge.migration import (
    MIGRATION_STATE_SCHEMA,
    MigrationStateSchema,
    is_migration_state_type,
    migration_state_schema,
    without_migration_state_schema,
)
from type_bridge.migration.introspection import (
    IntrospectedAttribute,
    IntrospectedEntity,
    IntrospectedOwnership,
    IntrospectedRelation,
    IntrospectedRole,
    IntrospectedSchema,
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
    assert migration.MigrationStateManager.ENTITY_NAME in MIGRATION_STATE_SCHEMA.entities


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
def test_predicate_recognizes_every_canonical_label(kind: str, labels: frozenset[str]) -> None:
    for label in labels:
        assert is_migration_state_type(kind=kind, label=label)  # type: ignore[arg-type]

    assert not is_migration_state_type(kind=kind, label="application-label")  # type: ignore[arg-type]


def test_predicate_is_keyword_only() -> None:
    with pytest.raises(TypeError):
        is_migration_state_type("entity", "type_bridge_migration")  # type: ignore[call-arg]


def test_predicate_rejects_unknown_schema_kind() -> None:
    with pytest.raises(ValueError, match="kind"):
        is_migration_state_type(kind="owner", label="type_bridge_migration")  # type: ignore[arg-type]


def test_without_migration_state_schema_filters_every_object_kind(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    synthetic_contract = MigrationStateSchema(
        entities=frozenset({"state-entity"}),
        relations=frozenset({"state-relation"}),
        attributes=frozenset({"state-attribute"}),
        roles=frozenset({"app-relation:state-role"}),
    )
    monkeypatch.setattr(state_schema_module, "MIGRATION_STATE_SCHEMA", synthetic_contract)

    schema = IntrospectedSchema(
        entities={
            "state-entity": IntrospectedEntity(name="state-entity"),
            "app-entity": IntrospectedEntity(name="app-entity"),
        },
        relations={
            "state-relation": IntrospectedRelation(
                name="state-relation",
                roles={"state-role": IntrospectedRole(name="state-role")},
            ),
            "app-relation": IntrospectedRelation(
                name="app-relation",
                roles={
                    "state-role": IntrospectedRole(name="state-role"),
                    "app-role": IntrospectedRole(name="app-role"),
                },
            ),
        },
        attributes={
            "state-attribute": IntrospectedAttribute(name="state-attribute", value_type="string"),
            "app-attribute": IntrospectedAttribute(name="app-attribute", value_type="string"),
        },
        ownerships=[
            IntrospectedOwnership("state-entity", "app-attribute"),
            IntrospectedOwnership("state-relation", "app-attribute"),
            IntrospectedOwnership("app-entity", "state-attribute"),
            IntrospectedOwnership("app-entity", "app-attribute"),
        ],
    )

    filtered = without_migration_state_schema(schema)

    assert filtered is not schema
    assert set(filtered.entities) == {"app-entity"}
    assert set(filtered.relations) == {"app-relation"}
    assert set(filtered.relations["app-relation"].roles) == {"app-role"}
    assert set(filtered.attributes) == {"app-attribute"}
    assert filtered.ownerships == [IntrospectedOwnership("app-entity", "app-attribute")]

    assert set(schema.entities) == {"state-entity", "app-entity"}
    assert set(schema.relations) == {"state-relation", "app-relation"}
    assert set(schema.relations["app-relation"].roles) == {"state-role", "app-role"}
    assert set(schema.attributes) == {"state-attribute", "app-attribute"}
    assert len(schema.ownerships) == 4


def test_public_migration_package_exports_state_schema_contract() -> None:
    expected = {
        "MIGRATION_STATE_SCHEMA",
        "MigrationStateSchema",
        "is_migration_state_type",
        "migration_state_schema",
        "without_migration_state_schema",
    }

    assert expected <= set(migration.__all__)
    for name in expected:
        assert getattr(migration, name) is globals()[name]
