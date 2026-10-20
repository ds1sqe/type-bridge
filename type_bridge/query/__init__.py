"""Query builder for TypeQL."""

from __future__ import annotations

import logging
from typing import TYPE_CHECKING, Any

from type_bridge.query.ast import (
    Clause,
    DeleteClause,
    FetchClause,
    FetchWildcard,
    InsertClause,
    MatchClause,
    RawPattern,
    RawStatement,
    _NativePattern,
    _NativeStatement,
)
from type_bridge.query.compiler import QueryCompiler
from type_bridge.query.parser import parse_typeql_query as parse_typeql_query

if TYPE_CHECKING:
    from type_bridge._runtime_projection import (
        GeneratedEntityProjection,
        GeneratedRelationProjection,
    )

logger = logging.getLogger(__name__)


class Query:
    """Builder for TypeQL queries."""

    def __init__(self):
        """Initialize query builder."""
        self.match_clause = MatchClause(patterns=[])
        self.delete_clause = DeleteClause(statements=[])
        self.insert_clause = InsertClause(statements=[])
        self.fetch_clause = FetchClause(items=[])

        # Modifiers
        self.sort_clauses: list[tuple[str, str]] = []
        self.offset_val: int | None = None
        self.limit_val: int | None = None

        self.compiler = QueryCompiler()

    def match(self, pattern: str) -> Query:
        """Add a match clause.

        Args:
            pattern: TypeQL match pattern

        Returns:
            Self for chaining
        """
        if pattern:
            # Clean string pattern
            clean_pattern = pattern.strip().rstrip(";")
            # RawPattern doesn't strictly require 'variable' if handled by compiler as raw string
            # But we made it subclass Pattern again with 'variable' moved to subclasses.
            # RawPattern definition: content: str
            self.match_clause.patterns.append(RawPattern(content=clean_pattern))
        return self

    def fetch(self, variable: str, *attributes: str) -> Query:
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
        # For TypeQL 3.x, default to wildcard fetch
        # Use variable name (without $) as the key
        key = variable.lstrip("$")
        self.fetch_clause.items.append(FetchWildcard(key=key, var=variable))
        return self

    def delete(self, pattern: str) -> Query:
        """Add a delete clause.

        Args:
            pattern: TypeQL delete pattern

        Returns:
            Self for chaining
        """
        if pattern:
            clean_pattern = pattern.strip().rstrip(";")
            self.delete_clause.statements.append(RawStatement(content=clean_pattern))
        return self

    def insert(self, pattern: str) -> Query:
        """Add an insert clause.

        Args:
            pattern: TypeQL insert pattern

        Returns:
            Self for chaining
        """
        if pattern:
            clean_pattern = pattern.strip().rstrip(";")
            self.insert_clause.statements.append(RawStatement(content=clean_pattern))
        return self

    def limit(self, limit: int) -> Query:
        """Set query limit.

        Args:
            limit: Maximum number of results

        Returns:
            Self for chaining
        """
        self.limit_val = limit
        return self

    def offset(self, offset: int) -> Query:
        """Set query offset.

        Args:
            offset: Number of results to skip

        Returns:
            Self for chaining
        """
        self.offset_val = offset
        return self

    def sort(self, variable: str, direction: str = "asc") -> Query:
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
            raise ValueError(f"Invalid sort direction: {direction}")
        self.sort_clauses.append((variable, direction))
        return self

    def build(self) -> str:
        """Build the final TypeQL query string.

        Returns:
            Complete TypeQL query
        """
        clauses: list[Clause] = []
        if self.match_clause.patterns:
            clauses.append(self.match_clause)
        if self.delete_clause.statements:
            clauses.append(self.delete_clause)
        if self.insert_clause.statements:
            clauses.append(self.insert_clause)
        if self.fetch_clause.items:
            clauses.append(self.fetch_clause)
        return self.compiler._compile_query(
            clauses, self.sort_clauses, self.offset_val, self.limit_val
        )

    def __str__(self) -> str:
        """String representation of query."""
        return self.build()


class QueryBuilder:
    """Helper class for building raw TypeQL from installed generated models."""

    @staticmethod
    def match_entity(
        model_class: type[GeneratedEntityProjection],
        var: str = "$e",
        **filters: Any,
    ) -> Query:
        """Create a match query for an entity.

        Args:
            model_class: An exact class from an installed generated projection
            var: Variable name to use
            filters: Attribute filters (field_name: value)

        Returns:
            Query object
        """
        logger.debug(
            f"QueryBuilder.match_entity: {model_class.__name__}, var={var}, filters={filters}"
        )
        query = Query()
        from type_bridge._runtime_projection import _projected_query_builder_match_entity_clause_for

        clause = _projected_query_builder_match_entity_clause_for(model_class, var, filters)
        query.match_clause.patterns.extend(_NativePattern(data) for data in clause["Match"])
        return query

    @staticmethod
    def insert_entity(instance: GeneratedEntityProjection, var: str = "$e") -> Query:
        """Create an insert query for an entity instance.

        Args:
            instance: An exact value from an installed generated projection
            var: Variable name to use

        Returns:
            Query object
        """
        logger.debug(f"QueryBuilder.insert_entity: {instance.__class__.__name__}, var={var}")
        query = Query()
        from type_bridge._runtime_projection import (
            _projected_query_builder_insert_entity_clause_for,
        )

        clause = _projected_query_builder_insert_entity_clause_for(instance, var)
        query.insert_clause.statements.extend(_NativeStatement(data) for data in clause["Insert"])
        return query

    @staticmethod
    def match_relation(
        model_class: type[GeneratedRelationProjection],
        var: str = "$r",
        role_players: dict[str, str] | None = None,
    ) -> Query:
        """Create a match query for a relation.

        Args:
            model_class: An exact class from an installed generated projection
            var: Variable name to use
            role_players: Dict mapping role names to player variables

        Returns:
            Query object

        Raises:
            ValueError: If a role name is not defined in the model
        """
        logger.debug(
            f"QueryBuilder.match_relation: {model_class.__name__}, var={var}, "
            f"role_players={role_players}"
        )
        query = Query()
        from type_bridge._runtime_projection import (
            _projected_query_builder_match_relation_clause_for,
        )

        clause = _projected_query_builder_match_relation_clause_for(model_class, var, role_players)
        query.match_clause.patterns.extend(_NativePattern(data) for data in clause["Match"])
        return query
