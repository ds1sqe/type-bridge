"""Grouped aggregation queries for entities."""

import logging
from typing import Any

from typedb.driver import TransactionType

from type_bridge.models import Entity
from type_bridge.query import QueryBuilder
from type_bridge.session import Connection, ConnectionExecutor

from ..base import parse_grouped_aggregate_results

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

        # Parse grouped results using shared utility
        output = parse_grouped_aggregate_results(results, group_vars)

        logger.info(f"GroupBy aggregation complete: {len(output)} groups")
        return output

    def _execute(self, query: str, tx_type: TransactionType) -> list[dict[str, Any]]:
        """Execute a query using an existing transaction if provided."""
        return self._executor.execute(query, tx_type)
