"""CRUD operations for TypeDB entities and relations.

This module provides managers and query builders for performing
CRUD (Create, Read, Update, Delete) operations on TypeDB entities
and relations with type safety.

The module is organized into submodules:
- entity: Entity-related CRUD operations
- relation: Relation-related CRUD operations
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

__all__ = [
    # Entity operations
    "EntityManager",
    "EntityQuery",
    "GroupByQuery",
    # Relation operations
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
