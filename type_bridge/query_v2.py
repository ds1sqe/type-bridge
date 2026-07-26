"""Typed low-level authoring for canonical V2 query plans.

The public classes are the native Rust-owned handles themselves.  This module
only gives them their stable Python namespace; it does not maintain a Python
AST, serialize plans, or repeat contract validation.
"""

from type_bridge_core import (
    AuthoredQueryInvocation,
    AuthoredQueryPlan,
    QueryPlanBuilder,
    QueryV2Authority,
)

__all__ = [
    "AuthoredQueryInvocation",
    "AuthoredQueryPlan",
    "QueryPlanBuilder",
    "QueryV2Authority",
]
