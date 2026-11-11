"""Frozen legacy ledger labels retained for migration import."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Final, Literal

from type_bridge import _native


@dataclass(frozen=True)
class MigrationStateSchema:
    """Immutable label projection of the frozen legacy migration ledger.

    Role labels are qualified as ``relation:role`` so an application relation
    can use the same unqualified role name without being classified as
    TypeBridge infrastructure.
    """

    entities: frozenset[str]
    relations: frozenset[str]
    attributes: frozenset[str]
    roles: frozenset[str]


def migration_state_schema() -> dict[str, Any]:
    """Return the frozen legacy ledger descriptor from the canonical Rust importer."""
    return _native.migration_state_schema()


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
"""Immutable labels for the legacy ledger read by migration import."""


def is_migration_state_type(
    *,
    kind: Literal["entity", "relation", "attribute", "role"],
    label: str,
) -> bool:
    """Return whether ``label`` belongs to the frozen legacy migration ledger.

    Role labels must use the qualified ``relation:role`` form.
    """
    return _native.is_migration_state_type(kind, label)
