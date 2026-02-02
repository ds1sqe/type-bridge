"""Query builder for TypeQL."""

from __future__ import annotations

import logging
from typing import TYPE_CHECKING

from type_bridge.query_parts import (
    DeleteBlock,
    FetchBlock,
    InsertBlock,
    MatchBlock,
    Modifiers,
)

if TYPE_CHECKING:
    from type_bridge.models import Entity, Relation

logger = logging.getLogger(__name__)


class Query:
    """Builder for TypeQL queries."""

    def __init__(self):
        """Initialize query builder."""
        self.match_block = MatchBlock()
        self.fetch_block = FetchBlock()
        self.delete_block = DeleteBlock()
        self.insert_block = InsertBlock()
        self.modifiers = Modifiers()

    def match(self, pattern: str) -> Query:
        """Add a match clause.

        Args:
            pattern: TypeQL match pattern

        Returns:
            Self for chaining
        """
        self.match_block.add(pattern)
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
        self.fetch_block.add(variable, list(attributes))
        return self

    def delete(self, pattern: str) -> Query:
        """Add a delete clause.

        Args:
            pattern: TypeQL delete pattern

        Returns:
            Self for chaining
        """
        self.delete_block.add(pattern)
        return self

    def insert(self, pattern: str) -> Query:
        """Add an insert clause.

        Args:
            pattern: TypeQL insert pattern

        Returns:
            Self for chaining
        """
        self.insert_block.add(pattern)
        return self

    def limit(self, limit: int) -> Query:
        """Set query limit.

        Args:
            limit: Maximum number of results

        Returns:
            Self for chaining
        """
        self.modifiers.limit(limit)
        return self

    def offset(self, offset: int) -> Query:
        """Set query offset.

        Args:
            offset: Number of results to skip

        Returns:
            Self for chaining
        """
        self.modifiers.offset(offset)
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
        self.modifiers.sort(variable, direction)
        return self

    def build(self) -> str:
        """Build the final TypeQL query string.

        Returns:
            Complete TypeQL query
        """
        logger.debug("Building TypeQL query")
        parts = []

        # Match clause
        match_str = self.match_block.build()
        if match_str:
            parts.append(match_str)

        # Delete clause
        delete_str = self.delete_block.build()
        if delete_str:
            parts.append(delete_str)

        # Insert clause
        insert_str = self.insert_block.build()
        if insert_str:
            parts.append(insert_str)

        # Sort, offset, and limit modifiers (must come BEFORE fetch in TypeQL 3.x)
        # IMPORTANT: offset must come BEFORE limit for pagination to work correctly
        modifier_str = self.modifiers.build()
        if modifier_str:
            parts.append(modifier_str)

        # Fetch clause (TypeQL 3.x syntax: fetch { $var.* })
        fetch_str = self.fetch_block.build()
        if fetch_str:
            parts.append(fetch_str)

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
