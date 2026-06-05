"""CRUD operations for TypeDB entities and relations."""

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
from .rust_manager import (
    RustTypeDBGroupByQuery as GroupByQuery,
)
from .rust_manager import (
    RustTypeDBManager as TypeDBManager,
)
from .rust_manager import (
    RustTypeDBQuery as TypeDBQuery,
)

__all__ = [
    # Cross-type lookup
    "has_lookup",
    # Unified manager
    "TypeDBManager",
    "TypeDBQuery",
    "GroupByQuery",
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
