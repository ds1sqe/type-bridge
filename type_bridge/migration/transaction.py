"""Callback-scoped execution on the engine's migration transaction."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from type_bridge_core import PyTransactionContext


class MigrationTransaction:
    """Use generated managers/queries and raw TypeQL in one migration step.

    The engine owns commit, rollback and close. This handle and managers bound
    to it become invalid when the callback finishes.
    """

    def __init__(self, native_transaction: PyTransactionContext) -> None:
        self._native_tx = native_transaction

    def execute_query(self, query: str) -> Any:
        """Run raw TypeQL and return the ordinary raw API's answer."""
        return self._native_tx.execute_query(query)
