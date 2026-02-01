"""RelationGroupByQuery for grouped aggregations on relations."""

import logging
from typing import Any

from typedb.driver import TransactionType

from type_bridge.models import Relation
from type_bridge.session import Connection, ConnectionExecutor

from ..base import parse_grouped_aggregate_results
from ..utils import extract_entity_key, format_value

logger = logging.getLogger(__name__)


class RelationGroupByQuery[R: Relation]:
    """Query for grouped aggregations on relations.

    Allows grouping relations by field values and computing aggregations per group.
    """

    def __init__(
        self,
        connection: Connection,
        model_class: type[R],
        filters: dict[str, Any],
        expressions: list[Any],
        group_fields: tuple[Any, ...],
    ):
        """Initialize grouped query.

        Args:
            connection: Database, Transaction, or TransactionContext
            model_class: Relation model class
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
            # Group by position, compute average salary per position
            result = manager.group_by(Employment.position).aggregate(Employment.salary.avg())
            # Returns: {
            #   "Engineer": {"avg_salary": 100000},
            #   "Manager": {"avg_salary": 150000}
            # }
        """
        from type_bridge.expressions import AggregateExpr

        if not aggregates:
            raise ValueError("At least one aggregation expression required")

        logger.debug(
            f"RelationGroupByQuery.aggregate: {self.model_class.__name__}, "
            f"group_fields={len(self.group_fields)}, aggregates={len(aggregates)}"
        )
        # Get all attributes (including inherited)
        all_attrs = self.model_class.get_all_attributes()

        # Separate attribute filters from role player filters
        attr_filters = {}
        role_player_filters = {}

        for key, value in self.filters.items():
            if key in self.model_class._roles:
                role_player_filters[key] = value
            elif key in all_attrs:
                attr_filters[key] = value
            else:
                raise ValueError(f"Unknown filter: {key}")

        # Build base match clause
        role_parts = []
        role_info = {}
        for role_name, role in self.model_class._roles.items():
            role_var = f"${role_name}"
            role_parts.append(f"{role.role_name}: {role_var}")
            role_info[role_name] = (role_var, role.player_entity_types)

        roles_str = ", ".join(role_parts)
        match_clauses = [f"$r isa {self.model_class.get_type_name()} ({roles_str})"]

        # Add dict-based attribute filters
        for field_name, value in attr_filters.items():
            attr_info = all_attrs[field_name]
            attr_name = attr_info.typ.get_attribute_name()
            formatted_value = format_value(value)
            match_clauses.append(f"$r has {attr_name} {formatted_value}")

        # Add role player filter clauses
        for role_name, player_entity in role_player_filters.items():
            role_var = f"${role_name}"
            key_info = extract_entity_key(player_entity)
            if key_info:
                _, attr_name, raw_value = key_info
                formatted_value = format_value(raw_value)
                match_clauses.append(f"{role_var} has {attr_name} {formatted_value}")

        # Apply expression filters
        for expr in self._expressions:
            pattern = expr.to_typeql("$r")
            match_clauses.append(pattern)

        # Add group-by fields to match
        group_vars = []
        for i, field in enumerate(self.group_fields):
            var_name = f"$group{i}"
            attr_name = field.attr_type.get_attribute_name()
            match_clauses.append(f"$r has {attr_name} {var_name}")
            group_vars.append(var_name)

        match_clause = ";\n".join(match_clauses) + ";"

        # Build reduce query with group-by
        reduce_clauses = []
        for agg in aggregates:
            if not isinstance(agg, AggregateExpr):
                raise TypeError(f"Expected AggregateExpr, got {type(agg).__name__}")

            # If this aggregation is on a specific attr_type (not count), add binding pattern
            if agg.attr_type is not None:
                attr_name = agg.attr_type.get_attribute_name()
                attr_var = f"${attr_name.lower()}"
                match_clause = match_clause.rstrip(";")
                match_clause += f";\n$r has {attr_name} {attr_var};"

            # Generate reduce clause: $result_var = function($var)
            result_var = f"${agg.get_fetch_key()}"
            reduce_clauses.append(f"{result_var} = {agg.to_typeql('$r')}")

        # TypeQL 3.x group-by syntax:
        # match ... reduce $result = function($var) groupby $group_var;
        group_clause = ", ".join(group_vars)
        reduce_clause = ", ".join(reduce_clauses)
        reduce_query = f"match\n{match_clause}\nreduce {reduce_clause} groupby {group_clause};"
        logger.debug(f"RelationGroupBy query: {reduce_query}")

        results = self._execute(reduce_query, TransactionType.READ)
        logger.debug(f"RelationGroupBy query returned {len(results)} results")

        # Parse grouped results using shared utility
        output = parse_grouped_aggregate_results(results, group_vars)

        logger.info(f"RelationGroupBy aggregation complete: {len(output)} groups")
        return output

    def _execute(self, query: str, tx_type: TransactionType) -> list[dict[str, Any]]:
        """Execute a query using an existing transaction if provided."""
        return self._executor.execute(query, tx_type)
