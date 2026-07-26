"""Explicit immutable budgets for one remote model-query exchange."""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class RemoteQueryLimits:
    """Caller-owned V2 response and hydration ceilings.

    Values are validated by the shared Rust contract when a
    :class:`~type_bridge.typed.RemoteQuerySession` snapshots its native
    execution context.
    """

    max_items: int
    max_bytes: int
    max_collection_members: int
    max_graph_nodes: int
    max_attribute_values: int
    max_role_players: int
    deadline_ms: int | None = None


__all__ = ["RemoteQueryLimits"]
