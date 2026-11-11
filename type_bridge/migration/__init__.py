"""Canonical workspace migration CLI and migration-state schema helpers."""

from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from type_bridge.migration.state_schema import (
        MIGRATION_STATE_SCHEMA,
        MigrationStateSchema,
        is_migration_state_type,
        migration_state_schema,
    )
    from type_bridge.migration.transaction import MigrationTransaction

__all__ = [
    "MigrationTransaction",
    "MIGRATION_STATE_SCHEMA",
    "MigrationStateSchema",
    "is_migration_state_type",
    "migration_state_schema",
]


def __getattr__(name: str) -> Any:
    """Keep state-schema projection work off the CLI startup path."""
    if name not in __all__:
        raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
    from importlib import import_module

    module = "transaction" if name == "MigrationTransaction" else "state_schema"
    value = getattr(import_module(f"type_bridge.migration.{module}"), name)
    globals()[name] = value
    return value
