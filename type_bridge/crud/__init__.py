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
from .strategies import EntityStrategy, ModelStrategy, RelationStrategy
from .typedb_manager import GroupByQuery, TypeDBManager, TypeDBQuery

__all__ = [
    # Unified manager
    "TypeDBManager",
    "TypeDBQuery",
    "GroupByQuery",
    # Strategies
    "ModelStrategy",
    "EntityStrategy",
    "RelationStrategy",
    # Exceptions
    "NotFoundError",
    "EntityNotFoundError",
    "RelationNotFoundError",
    "NotUniqueError",
    "KeyAttributeError",
    "HydrationError",
]
