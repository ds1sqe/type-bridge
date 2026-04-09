"""CRUD operations for TypeDB entities and relations.

This module provides the unified TypeDBManager for performing
CRUD (Create, Read, Update, Delete) operations on TypeDB entities
and relations with type safety.
"""

from .exceptions import (
    EntityNotFoundError,
    HydrationError,
    KeyAttributeError,
    NotFoundError,
    NotUniqueError,
    RelationNotFoundError,
)
from .has_lookup import has_lookup
from .hooks import CrudEvent, CrudHook, HookCancelled
from .strategies import EntityStrategy, ModelStrategy, RelationStrategy
from .typedb_manager import GroupByQuery, TypeDBManager, TypeDBQuery

__all__ = [
    # Cross-type lookup
    "has_lookup",
    # Unified manager
    "TypeDBManager",
    "TypeDBQuery",
    "GroupByQuery",
    # Strategies
    "ModelStrategy",
    "EntityStrategy",
    "RelationStrategy",
    # Hooks
    "CrudEvent",
    "CrudHook",
    "HookCancelled",
    # Exceptions
    "NotFoundError",
    "EntityNotFoundError",
    "RelationNotFoundError",
    "NotUniqueError",
    "KeyAttributeError",
    "HydrationError",
]
