"""Base types and classes for CRUD operations."""

import re
from abc import ABC, abstractmethod
from typing import TYPE_CHECKING, Any, TypeVar

from typedb.driver import TransactionType

from type_bridge.models import Entity, Relation
from type_bridge.session import Connection, ConnectionExecutor

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


def parse_aggregate_results(
    results: list[dict[str, Any]],
) -> dict[str, Any]:
    """Parse aggregation results from TypeDB reduce query.

    Handles both legacy string format and new dict format (TypeDB 3.8.0+).

    Args:
        results: Raw results from TypeDB reduce query

    Returns:
        Dictionary mapping aggregate variable names to their values
    """
    if not results:
        return {}

    result = results[0] if results else {}
    output = {}

    if "result" in result:
        # Legacy format: TypeDB reduce returns results as a formatted string
        # Format: '|  $var_name: Value(type: value)  |'
        result_str = result["result"]
        # Parse variable names and values from the formatted string
        # Pattern: $variable_name: Value(type: actual_value)
        pattern = r"\$([a-zA-Z_][a-zA-Z0-9_]*)\s*:\s*Value\([^:]+:\s*([^)]+)\)"
        matches = re.findall(pattern, result_str)

        for var_name, value_str in matches:
            # Try to convert the value to appropriate Python type
            try:
                # Try float first (covers both int and float)
                if "." in value_str:
                    value: Any = float(value_str)
                else:
                    value = int(value_str)
            except ValueError:
                # Keep as string if conversion fails
                value = value_str.strip()

            output[var_name] = value
    else:
        # New format (TypeDB 3.8.0+): results are proper dicts with extracted values
        # Format: {"var_name": {"value": actual_value}, ...}
        for var_name, concept_data in result.items():
            if isinstance(concept_data, dict):
                value = concept_data.get("value")
            else:
                # Direct value
                value = concept_data
            output[var_name] = value

    return output


def parse_grouped_aggregate_results(
    results: list[dict[str, Any]],
    group_vars: list[str],
) -> dict[Any, dict[str, Any]]:
    """Parse grouped aggregation results from TypeDB reduce groupby query.

    Handles both legacy string format and new dict format (TypeDB 3.8.0+).

    Args:
        results: Raw results from TypeDB reduce groupby query
        group_vars: List of group variable names (e.g., ["$group0", "$group1"])

    Returns:
        Dictionary mapping group keys to aggregation results
    """
    output: dict[Any, dict[str, Any]] = {}

    for result in results:
        # Handle both old string format and new dict format
        if "result" in result:
            # Legacy string parsing (fallback)
            result_str = result["result"]

            # Parse both Value(...) and Attribute(...) formats
            value_pattern = r"\$([a-zA-Z_][a-zA-Z0-9_]*)\s*:\s*Value\([^:]+:\s*([^)]+)\)"
            attr_pattern = r'\$([a-zA-Z_][a-zA-Z0-9_]*)\s*:\s*Attribute\([^:]+:\s*"([^"]+)"\)'

            value_matches = re.findall(value_pattern, result_str)
            attr_matches = re.findall(attr_pattern, result_str)

            all_matches = [(name, val) for name, val in value_matches] + [
                (name, val) for name, val in attr_matches
            ]

            group_keys: list[Any] = []
            group_aggs: dict[str, Any] = {}

            for var_name, value_str in all_matches:
                try:
                    if "." in value_str:
                        value: Any = float(value_str)
                    else:
                        value = int(value_str)
                except ValueError:
                    value = value_str.strip().strip('"')

                is_group_var = False
                for group_var in group_vars:
                    if group_var.lstrip("$") == var_name:
                        group_keys.append(value)
                        is_group_var = True
                        break

                if not is_group_var:
                    group_aggs[var_name] = value
        else:
            # New dict format (TypeDB 3.8.0+ with proper value extraction)
            group_keys = []
            group_aggs = {}

            for var_name, concept_data in result.items():
                # Extract the actual value from concept data
                if isinstance(concept_data, dict):
                    value = concept_data.get("value")
                else:
                    # Direct value (e.g., from _Value concept)
                    value = concept_data

                # Check if this is a group variable
                is_group_var = False
                for group_var in group_vars:
                    if group_var.lstrip("$") == var_name:
                        group_keys.append(value)
                        is_group_var = True
                        break

                if not is_group_var:
                    # This is an aggregation result
                    group_aggs[var_name] = value

        # Create group key (single value or tuple)
        if len(group_keys) == 1:
            group_key: Any = group_keys[0]
        else:
            group_key = tuple(group_keys)

        output[group_key] = group_aggs

    return output
