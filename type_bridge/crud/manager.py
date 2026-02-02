"""Base manager for CRUD operations."""

import logging
from abc import ABC, abstractmethod
from typing import TYPE_CHECKING, Any

from typedb.driver import TransactionType

from type_bridge.session import Connection, ConnectionExecutor

if TYPE_CHECKING:
    from type_bridge.models.base import TypeDBType

logger = logging.getLogger(__name__)


class BaseManager[T: "TypeDBType"](ABC):
    """Abstract base manager for CRUD operations.

    Provides shared functionality for TypeDBManager:
    - Connection and executor management
    - Query execution helpers
    - Common operations like all(), _execute()

    TypeDBManager implements all abstract methods.
    """

    _connection: Connection
    _executor: ConnectionExecutor
    model_class: type[T]

    def __init__(
        self,
        connection: Connection,
        model_class: type[T],
    ) -> None:
        """Initialize the manager.

        Args:
            connection: Database, Transaction, or TransactionContext
            model_class: Model class (Entity or Relation subclass)
        """
        self._connection = connection
        self._executor = ConnectionExecutor(connection)
        self.model_class = model_class

    def _execute(self, query: str, tx_type: TransactionType) -> list[dict[str, Any]]:
        """Execute a query using the connection executor.

        Args:
            query: TypeQL query string
            tx_type: Transaction type (READ or WRITE)

        Returns:
            List of result dictionaries
        """
        return self._executor.execute(query, tx_type)

    def all(self) -> list[T]:
        """Get all instances of this type.

        Syntactic sugar for get() with no filters.

        Returns:
            List of all instances
        """
        logger.debug(f"Getting all instances: {self.model_class.__name__}")
        return self.get()

    # =========================================================================
    # Abstract methods - must be implemented by subclasses
    # =========================================================================

    @abstractmethod
    def insert(self, instance: T) -> T:
        """Insert an instance into the database.

        Args:
            instance: Instance to insert

        Returns:
            The inserted instance (with _iid populated if successful)
        """
        ...

    @abstractmethod
    def get(self, **filters: Any) -> list[T]:
        """Get instances with optional filters.

        Args:
            **filters: Attribute filters (exact match)

        Returns:
            List of matching instances
        """
        ...

    @abstractmethod
    def update(self, instance: T) -> T:
        """Update an instance in the database.

        Args:
            instance: Instance with updated values

        Returns:
            The updated instance
        """
        ...

    @abstractmethod
    def delete(self, instance: T) -> T:
        """Delete an instance from the database.

        Args:
            instance: Instance to delete

        Returns:
            The deleted instance
        """
        ...
