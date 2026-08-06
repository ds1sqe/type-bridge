"""Owner-aware typed-query references backed exclusively by native handles.

The package-root V1 ``Query`` remains separate and unchanged.
"""

from type_bridge.fields.base import _QueryFieldRef as FieldRef
from type_bridge.fields.role import _QueryRoleRef as RoleRef
from type_bridge.typed._remote_terminal import RemoteQueryExchange
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
from type_bridge.typed.remote_limits import RemoteQueryLimits
from type_bridge.typed.remote_query import RemoteQuery
from type_bridge.typed.remote_session import RemoteQuerySession
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
    "RemoteQuery",
    "RemoteQueryExchange",
    "RemoteQueryLimits",
    "RemoteQuerySession",
    "RoleRef",
    "Selection",
    "TypedQueryCapabilityError",
    "TypedQueryConnectionError",
    "TypedQueryMaterializationError",
    "TypedQueryWindowError",
]
