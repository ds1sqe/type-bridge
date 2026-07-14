"""Owner-aware typed-query references backed exclusively by native handles.

The package-root legacy ``Query`` remains separate and unchanged.
"""

from type_bridge.fields import FieldRef
from type_bridge.fields.role import RoleRef
from type_bridge.typed._terminal import (
    TypedQueryCapabilityError,
    TypedQueryConnectionError,
    TypedQueryWindowError,
)
from type_bridge.typed.page import Page
from type_bridge.typed.query import Query
from type_bridge.typed.references import (
    BoundField,
    BoundRole,
    BoundVar,
    Collected,
    Predicate,
    QueryOrder,
    Selection,
)
from type_bridge.typed.results import TypedQueryMaterializationError
from type_bridge.typed.session import QuerySession

__all__ = [
    "BoundField",
    "BoundRole",
    "BoundVar",
    "Collected",
    "FieldRef",
    "Page",
    "Predicate",
    "Query",
    "QueryOrder",
    "QuerySession",
    "RoleRef",
    "Selection",
    "TypedQueryCapabilityError",
    "TypedQueryConnectionError",
    "TypedQueryMaterializationError",
    "TypedQueryWindowError",
]
