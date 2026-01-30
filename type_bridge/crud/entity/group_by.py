"""Grouped aggregation queries for entities."""

import logging
import re
from typing import Any

from typedb.driver import TransactionType

from type_bridge.models import Entity
from type_bridge.query import QueryBuilder
from type_bridge.session import Connection, ConnectionExecutor

logger = logging.getLogger(__name__)


class GroupByQuery[E: Entity]:
    """Query for grouped aggregations.

    Allows grouping entities by field values and computing aggregations per group.
    """

    def __init__(
        self,
        connection: Connection,
        model_class: type[E],
        filters: dict[str, Any],
        expressions: list[Any],
        group_fields: tuple[Any, ...],
    ):
        """Initialize grouped query.

        Args:
            connection: Database, Transaction, or TransactionContext
            model_class: Entity model class
            filters: Dict-based filters
            expressions: Expression-based filters
            group_fields: Fields to group by
        """
        self._executor = ConnectionExecutor(connection)
        self.model_class = model_class
        self.filters = filters
        self._expressions = expressions
        self.group_fields = group_fields

    def aggregate(self, *aggregates: Any) -> dict[Any, dict[str, Any]]:
        """Execute grouped aggregation.

        Args:
            *aggregates: AggregateExpr objects

        Returns:
            Dictionary mapping group values to aggregation results

        Example:
            # Group by city, compute average age per city
            result = manager.group_by(Person.city).aggregate(Person.age.avg())
            # Returns: {
            #   "NYC": {"avg_age": 35.5},
            #   "LA": {"avg_age": 28.3}
            # }
        """
        from type_bridge.expressions import AggregateExpr

        if not aggregates:
            raise ValueError("At least one aggregation expression required")

        logger.debug(
            f"GroupByQuery.aggregate: {self.model_class.__name__}, "
            f"group_fields={len(self.group_fields)}, aggregates={len(aggregates)}"
        )
        # Build base match query
        query = QueryBuilder.match_entity(self.model_class, **self.filters)

        # Apply expression filters
        for expr in self._expressions:
            pattern = expr.to_typeql("$e")
            query.match(pattern)

        # Add group-by fields to match
        group_vars = []
        for i, field in enumerate(self.group_fields):
            var_name = f"$group{i}"
            attr_name = field.attr_type.get_attribute_name()
            query.match(f"$e has {attr_name} {var_name}")
            group_vars.append(var_name)

        # Build reduce query with group-by
        # First, bind all the fields being aggregated in the match clause
        reduce_clauses = []
        for agg in aggregates:
            if not isinstance(agg, AggregateExpr):
                raise TypeError(f"Expected AggregateExpr, got {type(agg).__name__}")

            # If this aggregation is on a specific attr_type (not count), add binding pattern
            if agg.attr_type is not None:
                attr_name = agg.attr_type.get_attribute_name()
                attr_var = f"${attr_name.lower()}"
                query.match(f"$e has {attr_name} {attr_var}")

            # Generate reduce clause: $result_var = function($var)
            result_var = f"${agg.get_fetch_key()}"
            reduce_clauses.append(f"{result_var} = {agg.to_typeql('$e')}")

        # TypeQL 3.x group-by syntax:
        # match ... reduce $result = function($var) groupby $group_var;
        match_clause = query.build().replace("fetch", "get").split("fetch")[0]
        group_clause = ", ".join(group_vars)
        reduce_clause = ", ".join(reduce_clauses)
        reduce_query = f"{match_clause}\nreduce {reduce_clause} groupby {group_clause};"
        logger.debug(f"GroupBy query: {reduce_query}")

        results = self._execute(reduce_query, TransactionType.READ)
        logger.debug(f"GroupBy query returned {len(results)} results")

        # Parse grouped results
        # Results are now proper dicts with extracted values (TypeDB 3.8.0+)
        output = {}
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
                group_aggs = {}

                for var_name, value_str in all_matches:
                    try:
                        if "." in value_str:
                            value = float(value_str)
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

        logger.info(f"GroupBy aggregation complete: {len(output)} groups")
        return output

    def _execute(self, query: str, tx_type: TransactionType) -> list[dict[str, Any]]:
        """Execute a query using an existing transaction if provided."""
        return self._executor.execute(query, tx_type)
