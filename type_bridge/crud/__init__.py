"""CRUD operations for TypeDB entities and relations.

This module provides managers and query builders for performing
CRUD (Create, Read, Update, Delete) operations on TypeDB entities
and relations with type safety.

The module is organized into submodules:
- entity: Entity-related CRUD operations
- relation: Relation-related CRUD operations
- typedb_manager: Unified TypeDBManager (v2 architecture)
- strategies: Model-specific strategies for TypeDBManager
- utils: Shared utilities
- base: Base type definitions
"""

# Public API re-exports
from .entity import EntityManager, EntityQuery, GroupByQuery
from .exceptions import (
    EntityNotFoundError,
    KeyAttributeError,
    NotFoundError,
    NotUniqueError,
    RelationNotFoundError,
)
from .relation import RelationGroupByQuery, RelationManager, RelationQuery
from .strategies import EntityStrategy, ModelStrategy, RelationStrategy
from .typedb_manager import TypeDBManager

__all__ = [
    # Unified manager (v2)
    "TypeDBManager",
    "ModelStrategy",
    "EntityStrategy",
    "RelationStrategy",
    # Entity operations (legacy, will be deprecated)
    "EntityManager",
    "EntityQuery",
    "GroupByQuery",
    # Relation operations (legacy, will be deprecated)
    "RelationManager",
    "RelationQuery",
    "RelationGroupByQuery",
    # Exceptions
    "NotFoundError",
    "EntityNotFoundError",
    "RelationNotFoundError",
    "NotUniqueError",
    "KeyAttributeError",
]
