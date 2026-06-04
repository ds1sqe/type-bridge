"""Base types and classes for CRUD operations."""

from abc import ABC, abstractmethod
from typing import TYPE_CHECKING, Any, TypeVar

from type_bridge.models import Entity, Relation
from type_bridge.session import Connection, ConnectionExecutor
from type_bridge.typedb_driver import TransactionType

if TYPE_CHECKING:
    from type_bridge.models.base import TypeDBType

# Type variables bound to Entity and Relation
E = TypeVar("E", bound=Entity)
R = TypeVar("R", bound=Relation)

# Type variable for generic base model
T = TypeVar("T", bound="TypeDBType")


class BaseQuery[T](ABC):
    """Abstract base class for chainable query operations.

    Provides shared implementation for common query methods used by both
    EntityQuery and RelationQuery.
    """

    _connection: Connection
    _executor: ConnectionExecutor
    model_class: type[T]
    filters: dict[str, Any]
    _expressions: list[Any]
    _limit_value: int | None
    _offset_value: int | None

    def __init__(
        self,
        connection: Connection,
        model_class: type[T],
        filters: dict[str, Any] | None = None,
    ):
        """Initialize base query.

        Args:
            connection: Database, Transaction, or TransactionContext
            model_class: Model class (Entity or Relation subclass)
            filters: Attribute filters (exact match) - optional, defaults to empty dict
        """
        self._connection = connection
        self._executor = ConnectionExecutor(connection)
        self.model_class = model_class
        self.filters = filters or {}
        self._expressions: list[Any] = []
        self._limit_value: int | None = None
        self._offset_value: int | None = None

    def limit(self, limit: int) -> "BaseQuery[T]":
        """Limit number of results.

        Args:
            limit: Maximum number of results

        Returns:
            Self for chaining
        """
        self._limit_value = limit
        return self

    def offset(self, offset: int) -> "BaseQuery[T]":
        """Skip number of results.

        Args:
            offset: Number of results to skip

        Returns:
            Self for chaining
        """
        self._offset_value = offset
        return self

    @abstractmethod
    def execute(self) -> list[T]:
        """Execute the query and return results.

        Returns:
            List of matching model instances
        """
        ...

    def first(self) -> T | None:
        """Get first matching result.

        Returns:
            First result or None
        """
        results = self.limit(1).execute()
        return results[0] if results else None

    def count(self) -> int:
        """Count matching results.

        Returns:
            Number of matching results
        """
        return len(self.execute())

    def _execute(self, query: str, tx_type: TransactionType) -> list[dict[str, Any]]:
        """Execute a query using the connection executor."""
        return self._executor.execute(query, tx_type)
