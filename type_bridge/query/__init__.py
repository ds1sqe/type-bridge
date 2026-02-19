"""Query builder for TypeQL."""

from __future__ import annotations

import logging
from typing import TYPE_CHECKING, Any

from type_bridge.query.ast import (
    DeleteClause,
    FetchClause,
    FetchWildcard,
    InsertClause,
    MatchClause,
    RawPattern,
    RawStatement,
)
from type_bridge.query.compiler import QueryCompiler
from type_bridge.query.parser import parse_typeql_query as parse_typeql_query

if TYPE_CHECKING:
    from type_bridge.models import Entity, Relation

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
        logger.debug("Building TypeQL query")
        parts = []

        # Match clause
        if self.match_clause.patterns:
            parts.append(self.compiler.compile(self.match_clause))

        # Delete clause
        if self.delete_clause.statements:
            parts.append(self.compiler.compile(self.delete_clause))

        # Insert clause
        if self.insert_clause.statements:
            parts.append(self.compiler.compile(self.insert_clause))

        # Sort, offset, and limit modifiers (must come BEFORE fetch in TypeQL 3.x)
        modifier_parts = []
        if self.sort_clauses:
            sort_items = [f"{var} {dir}" for var, dir in self.sort_clauses]
            modifier_parts.append(f"sort {', '.join(sort_items)};")

        # Order matters: form modifiers, put offset then limit
        if self.offset_val is not None:
            modifier_parts.append(f"offset {self.offset_val};")
        if self.limit_val is not None:
            modifier_parts.append(f"limit {self.limit_val};")

        if modifier_parts:
            parts.append("\n".join(modifier_parts))

        # Fetch clause
        if self.fetch_clause.items:
            parts.append(self.compiler.compile(self.fetch_clause))

        query = "\n".join(parts)
        logger.debug(f"Built query: {query}")
        return query

    def __str__(self) -> str:
        """String representation of query."""
        return self.build()


class QueryBuilder:
    """Helper class for building queries with model classes."""

    @staticmethod
    def match_entity(model_class: type[Entity], var: str = "$e", **filters: Any) -> Query:
        """Create a match query for an entity.

        Args:
            model_class: The entity model class
            var: Variable name to use
            filters: Attribute filters (field_name: value)

        Returns:
            Query object
        """
        from type_bridge.crud.patterns import build_entity_match_pattern

        logger.debug(
            f"QueryBuilder.match_entity: {model_class.__name__}, var={var}, filters={filters}"
        )
        query = Query()
        pattern = build_entity_match_pattern(model_class, var, filters or None)
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

        Raises:
            ValueError: If a role name is not defined in the model
        """
        from type_bridge.crud.patterns import build_relation_match_pattern

        logger.debug(
            f"QueryBuilder.match_relation: {model_class.__name__}, var={var}, "
            f"role_players={role_players}"
        )
        query = Query()
        pattern = build_relation_match_pattern(model_class, var, role_players)
        query.match(pattern)
        return query
