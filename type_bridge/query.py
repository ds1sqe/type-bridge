"""Query builder for TypeQL."""

import logging

from type_bridge.crud.utils import format_value
from type_bridge.models import Entity, Relation

logger = logging.getLogger(__name__)


class Query:
    """Builder for TypeQL queries."""

    def __init__(self):
        """Initialize query builder."""
        self._match_clauses: list[str] = []
        self._fetch_specs: dict[str, list[str]] = {}  # var -> [attributes]
        self._delete_clauses: list[str] = []
        self._insert_clauses: list[str] = []
        self._sort_clauses: list[tuple[str, str]] = []  # [(variable, direction)]
        self._limit: int | None = None
        self._offset: int | None = None

    def match(self, pattern: str) -> "Query":
        """Add a match clause.

        Args:
            pattern: TypeQL match pattern

        Returns:
            Self for chaining
        """
        self._match_clauses.append(pattern)
        return self

    def fetch(self, variable: str, *attributes: str) -> "Query":
        """Add variables and attributes to fetch.

        In TypeQL 3.x, fetch uses the syntax:
        fetch { $e.* }  (fetch all attributes)

        Args:
            variable: Variable name to fetch (e.g., "$e")
            attributes: Not used in TypeQL 3.x (kept for API compatibility)

        Returns:
            Self for chaining

        Example:
            query.fetch("$e")  # Fetches all attributes
        """
        self._fetch_specs[variable] = list(attributes)
        return self

    def delete(self, pattern: str) -> "Query":
        """Add a delete clause.

        Args:
            pattern: TypeQL delete pattern

        Returns:
            Self for chaining
        """
        self._delete_clauses.append(pattern)
        return self

    def insert(self, pattern: str) -> "Query":
        """Add an insert clause.

        Args:
            pattern: TypeQL insert pattern

        Returns:
            Self for chaining
        """
        self._insert_clauses.append(pattern)
        return self

    def limit(self, limit: int) -> "Query":
        """Set query limit.

        Args:
            limit: Maximum number of results

        Returns:
            Self for chaining
        """
        self._limit = limit
        return self

    def offset(self, offset: int) -> "Query":
        """Set query offset.

        Args:
            offset: Number of results to skip

        Returns:
            Self for chaining
        """
        self._offset = offset
        return self

    def sort(self, variable: str, direction: str = "asc") -> "Query":
        """Add sorting to the query.

        Args:
            variable: Variable to sort by
            direction: Sort direction ("asc" or "desc")

        Returns:
            Self for chaining

        Example:
            Query().match("$p isa person").fetch("$p").sort("$p", "asc")
        """
        if direction not in ("asc", "desc"):
            raise ValueError(f"Invalid sort direction: {direction}. Must be 'asc' or 'desc'")
        self._sort_clauses.append((variable, direction))
        return self

    def build(self) -> str:
        """Build the final TypeQL query string.

        Returns:
            Complete TypeQL query
        """
        logger.debug("Building TypeQL query")
        parts = []

        # Match clause
        if self._match_clauses:
            match_body = "; ".join(self._match_clauses)
            parts.append(f"match\n{match_body};")

        # Delete clause
        if self._delete_clauses:
            delete_body = "; ".join(self._delete_clauses)
            parts.append(f"delete\n{delete_body};")

        # Insert clause
        if self._insert_clauses:
            insert_body = "; ".join(self._insert_clauses)
            parts.append(f"insert\n{insert_body};")

        # Sort, offset, and limit modifiers (must come BEFORE fetch in TypeQL 3.x)
        # IMPORTANT: offset must come BEFORE limit for pagination to work correctly
        if self._sort_clauses:
            # TypeQL uses comma-separated sort variables: sort $var1 asc, $var2 desc;
            sort_items = [f"{var} {direction}" for var, direction in self._sort_clauses]
            parts.append(f"sort {', '.join(sort_items)};")
        if self._offset is not None:
            parts.append(f"offset {self._offset};")
        if self._limit is not None:
            parts.append(f"limit {self._limit};")

        # Fetch clause (TypeQL 3.x syntax: fetch { $var.* })
        if self._fetch_specs:
            fetch_items = []
            for var in self._fetch_specs.keys():
                fetch_items.append(f"  {var}.*")
            # Use actual newline, not \n literal
            fetch_body = ",\n".join(fetch_items)
            parts.append(f"fetch {{\n{fetch_body}\n}};")

        query = "\n".join(parts)
        logger.debug(f"Built query: {query}")
        return query

    def __str__(self) -> str:
        """String representation of query."""
        return self.build()


class QueryBuilder:
    """Helper class for building queries with model classes."""

    @staticmethod
    def match_entity(model_class: type[Entity], var: str = "$e", **filters) -> Query:
        """Create a match query for an entity.

        Args:
            model_class: The entity model class
            var: Variable name to use
            filters: Attribute filters (field_name: value)

        Returns:
            Query object
        """
        logger.debug(
            f"QueryBuilder.match_entity: {model_class.__name__}, var={var}, filters={filters}"
        )
        query = Query()

        # Basic entity match
        pattern_parts = [f"{var} isa {model_class.get_type_name()}"]

        # Add attribute filters (including inherited attributes)
        owned_attrs = model_class.get_all_attributes()
        for field_name, field_value in filters.items():
            if field_name in owned_attrs:
                attr_info = owned_attrs[field_name]
                attr_name = attr_info.typ.get_attribute_name()
                formatted_value = format_value(field_value)
                pattern_parts.append(f"has {attr_name} {formatted_value}")

        pattern = ", ".join(pattern_parts)
        query.match(pattern)

        return query

    @staticmethod
    def insert_entity(instance: Entity, var: str = "$e") -> Query:
        """Create an insert query for an entity instance.

        Args:
            instance: Entity instance
            var: Variable name to use

        Returns:
            Query object
        """
        logger.debug(f"QueryBuilder.insert_entity: {instance.__class__.__name__}, var={var}")
        query = Query()
        insert_pattern = instance.to_insert_query(var)
        query.insert(insert_pattern)
        return query

    @staticmethod
    def match_relation(
        model_class: type[Relation], var: str = "$r", role_players: dict[str, str] | None = None
    ) -> Query:
        """Create a match query for a relation.

        Args:
            model_class: The relation model class
            var: Variable name to use
            role_players: Dict mapping role names to player variables

        Returns:
            Query object
        """
        logger.debug(
            f"QueryBuilder.match_relation: {model_class.__name__}, var={var}, "
            f"role_players={role_players}"
        )
        query = Query()

        # Basic relation match
        pattern_parts = [f"{var} isa {model_class.get_type_name()}"]

        # Add role players
        if role_players:
            for role_name, player_var in role_players.items():
                pattern_parts.append(f"({role_name}: {player_var})")

        pattern = ", ".join(pattern_parts)
        query.match(pattern)

        return query
