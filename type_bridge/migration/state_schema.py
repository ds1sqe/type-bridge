"""Public boundary for TypeBridge-owned migration-state schema objects."""

from __future__ import annotations

from dataclasses import dataclass, replace
from typing import Any, Final, Literal

from type_bridge import _rust_runtime
from type_bridge.migration.introspection import IntrospectedSchema


@dataclass(frozen=True)
class MigrationStateSchema:
    """Immutable label projection of the canonical migration-state schema.

    Role labels are qualified as ``relation:role`` so an application relation
    can use the same unqualified role name without being classified as
    TypeBridge infrastructure.
    """

    entities: frozenset[str]
    relations: frozenset[str]
    attributes: frozenset[str]
    roles: frozenset[str]


def migration_state_schema() -> dict[str, Any]:
    """Return the full canonical migration-state schema descriptor from Rust."""
    return _rust_runtime.migration_state_schema()


def _label_projection(schema: dict[str, Any]) -> MigrationStateSchema:
    entities = frozenset(str(label) for label in schema.get("entities", {}))
    relations = frozenset(str(label) for label in schema.get("relations", {}))
    attributes = frozenset(str(label) for label in schema.get("attributes", {}))

    roles: set[str] = set()
    for relation_label, relation in schema.get("relations", {}).items():
        for role in relation.get("roles", []):
            roles.add(f"{relation_label}:{role['role_name']}")

    return MigrationStateSchema(
        entities=entities,
        relations=relations,
        attributes=attributes,
        roles=frozenset(roles),
    )


MIGRATION_STATE_SCHEMA: Final = _label_projection(migration_state_schema())
"""Immutable labels for all schema objects owned by TypeBridge migration state."""


def is_migration_state_type(
    *,
    kind: Literal["entity", "relation", "attribute", "role"],
    label: str,
) -> bool:
    """Return whether ``label`` is a TypeBridge migration-state schema object.

    Role labels must use the qualified ``relation:role`` form.
    """
    return _rust_runtime.is_migration_state_type(kind, label)


def without_migration_state_schema(schema: IntrospectedSchema) -> IntrospectedSchema:
    """Return a copy of ``schema`` without TypeBridge migration-state objects."""
    state_schema = MIGRATION_STATE_SCHEMA
    state_owners = state_schema.entities | state_schema.relations

    relations = {}
    for relation_name, relation in schema.relations.items():
        if relation_name in state_schema.relations:
            continue
        roles = {
            role_name: role
            for role_name, role in relation.roles.items()
            if f"{relation_name}:{role_name}" not in state_schema.roles
        }
        relations[relation_name] = replace(relation, roles=roles)

    return IntrospectedSchema(
        entities={
            name: entity
            for name, entity in schema.entities.items()
            if name not in state_schema.entities
        },
        relations=relations,
        attributes={
            name: attribute
            for name, attribute in schema.attributes.items()
            if name not in state_schema.attributes
        },
        ownerships=[
            ownership
            for ownership in schema.ownerships
            if ownership.owner_name not in state_owners
            and ownership.attribute_name not in state_schema.attributes
        ],
    )
