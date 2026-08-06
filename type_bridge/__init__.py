"""Python SDK for generated TypeBridge applications and retained query APIs."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from type_bridge.proxy import ProxyDatabase, ProxyError
from type_bridge.query import Query, QueryBuilder
from type_bridge.session import (
    Connection,
    Database,
    TransactionContext,
)
from type_bridge.typedb_driver import Credentials, TransactionType, TypeDB, create_driver_options

if TYPE_CHECKING:
    from type_bridge.crud.exceptions import (
        EntityNotFoundError,
        KeyAttributeError,
        NotUniqueError,
        RelationNotFoundError,
    )
    from type_bridge.crud.hooks import CrudEvent, CrudHook, HookCancelled
    from type_bridge.migration.exceptions import SchemaConflictError, SchemaValidationError
    from type_bridge.migration.introspection import SchemaIntrospector

__version__ = "2.1.0"


_LAZY_EXPORTS: dict[str, tuple[str, str]] = {
    "CrudEvent": ("type_bridge.crud.hooks", "CrudEvent"),
    "CrudHook": ("type_bridge.crud.hooks", "CrudHook"),
    "HookCancelled": ("type_bridge.crud.hooks", "HookCancelled"),
    "EntityNotFoundError": ("type_bridge.crud.exceptions", "EntityNotFoundError"),
    "RelationNotFoundError": ("type_bridge.crud.exceptions", "RelationNotFoundError"),
    "NotUniqueError": ("type_bridge.crud.exceptions", "NotUniqueError"),
    "KeyAttributeError": ("type_bridge.crud.exceptions", "KeyAttributeError"),
    "SchemaIntrospector": ("type_bridge.migration.introspection", "SchemaIntrospector"),
    "SchemaConflictError": ("type_bridge.migration.exceptions", "SchemaConflictError"),
    "SchemaValidationError": ("type_bridge.migration.exceptions", "SchemaValidationError"),
}


def __getattr__(name: str) -> Any:
    """Load retained compatibility identities without importing authoring eagerly."""
    target = _LAZY_EXPORTS.get(name)
    if target is None:
        from type_bridge.migration._archive_imports import archive_attribute

        return archive_attribute(__name__, name)

    from importlib import import_module

    module_name, attribute_name = target
    value = getattr(import_module(module_name), attribute_name)
    globals()[name] = value
    return value


__all__ = [
    # Database and session
    "Connection",
    "Database",
    "TransactionContext",
    # TypeDB driver (re-exported for convenience)
    "Credentials",
    "TransactionType",
    "TypeDB",
    "create_driver_options",
    # Query
    "Query",
    "QueryBuilder",
    # Hooks
    "CrudEvent",
    "CrudHook",
    "HookCancelled",
    # Proxy
    "ProxyDatabase",
    "ProxyError",
    # CRUD Exceptions
    "EntityNotFoundError",
    "RelationNotFoundError",
    "NotUniqueError",
    "KeyAttributeError",
    # Schema
    "SchemaIntrospector",
    # Schema exceptions
    "SchemaConflictError",
    "SchemaValidationError",
]
