"""Read-only archive migration recovery and introspection surfaces.

Canonical Split-YAML workspace migrations own all new planning and writes.
The Python package retains only frozen-history loading, state inspection,
sidecar conversion, and schema introspection needed for one-way adoption.
"""

from __future__ import annotations

from importlib import import_module
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from type_bridge.migration.exceptions import SchemaConflictError, SchemaValidationError
    from type_bridge.migration.introspection import (
        IntrospectedAttribute,
        IntrospectedEntity,
        IntrospectedOwnership,
        IntrospectedRelation,
        IntrospectedRole,
        IntrospectedSchema,
        SchemaIntrospector,
    )
    from type_bridge.migration.loader import LoadedMigration, MigrationLoader, MigrationLoadError
    from type_bridge.migration.sidecar import SidecarConversionError, generate_sidecars
    from type_bridge.migration.state import (
        MigrationRecord,
        MigrationRunRecord,
        MigrationState,
        MigrationStateManager,
    )
    from type_bridge.migration.state_schema import (
        MIGRATION_STATE_SCHEMA,
        MigrationStateSchema,
        is_migration_state_type,
        migration_state_schema,
        without_migration_state_schema,
    )
    from type_bridge.migration.utils import type_exists

_LAZY_EXPORTS: dict[str, tuple[str, str]] = {
    "SchemaConflictError": (
        "type_bridge.migration.exceptions",
        "SchemaConflictError",
    ),
    "SchemaValidationError": (
        "type_bridge.migration.exceptions",
        "SchemaValidationError",
    ),
    "MigrationState": ("type_bridge.migration.state", "MigrationState"),
    "MigrationStateManager": (
        "type_bridge.migration.state",
        "MigrationStateManager",
    ),
    "MigrationRecord": ("type_bridge.migration.state", "MigrationRecord"),
    "MigrationRunRecord": (
        "type_bridge.migration.state",
        "MigrationRunRecord",
    ),
    "MigrationStateSchema": (
        "type_bridge.migration.state_schema",
        "MigrationStateSchema",
    ),
    "MIGRATION_STATE_SCHEMA": (
        "type_bridge.migration.state_schema",
        "MIGRATION_STATE_SCHEMA",
    ),
    "migration_state_schema": (
        "type_bridge.migration.state_schema",
        "migration_state_schema",
    ),
    "is_migration_state_type": (
        "type_bridge.migration.state_schema",
        "is_migration_state_type",
    ),
    "without_migration_state_schema": (
        "type_bridge.migration.state_schema",
        "without_migration_state_schema",
    ),
    "MigrationLoader": ("type_bridge.migration.loader", "MigrationLoader"),
    "LoadedMigration": ("type_bridge.migration.loader", "LoadedMigration"),
    "MigrationLoadError": (
        "type_bridge.migration.loader",
        "MigrationLoadError",
    ),
    "SidecarConversionError": (
        "type_bridge.migration.sidecar",
        "SidecarConversionError",
    ),
    "generate_sidecars": ("type_bridge.migration.sidecar", "generate_sidecars"),
    "SchemaIntrospector": (
        "type_bridge.migration.introspection",
        "SchemaIntrospector",
    ),
    "IntrospectedSchema": (
        "type_bridge.migration.introspection",
        "IntrospectedSchema",
    ),
    "IntrospectedEntity": (
        "type_bridge.migration.introspection",
        "IntrospectedEntity",
    ),
    "IntrospectedRelation": (
        "type_bridge.migration.introspection",
        "IntrospectedRelation",
    ),
    "IntrospectedAttribute": (
        "type_bridge.migration.introspection",
        "IntrospectedAttribute",
    ),
    "IntrospectedOwnership": (
        "type_bridge.migration.introspection",
        "IntrospectedOwnership",
    ),
    "IntrospectedRole": (
        "type_bridge.migration.introspection",
        "IntrospectedRole",
    ),
    "type_exists": ("type_bridge.migration.utils", "type_exists"),
}


def __getattr__(name: str) -> Any:
    target = _LAZY_EXPORTS.get(name)
    if target is None:
        from type_bridge.migration._archive_imports import archive_attribute

        return archive_attribute(__name__, name)
    module_name, attribute_name = target
    value = getattr(import_module(module_name), attribute_name)
    globals()[name] = value
    return value


__all__ = [
    "SchemaConflictError",
    "SchemaValidationError",
    "MigrationState",
    "MigrationStateManager",
    "MigrationRecord",
    "MigrationRunRecord",
    "MigrationStateSchema",
    "MIGRATION_STATE_SCHEMA",
    "migration_state_schema",
    "is_migration_state_type",
    "without_migration_state_schema",
    "MigrationLoader",
    "LoadedMigration",
    "MigrationLoadError",
    "SidecarConversionError",
    "generate_sidecars",
    "SchemaIntrospector",
    "IntrospectedSchema",
    "IntrospectedEntity",
    "IntrospectedRelation",
    "IntrospectedAttribute",
    "IntrospectedOwnership",
    "IntrospectedRole",
    "type_exists",
]
