"""Unified TypeDB Manager using AST and Strategies."""

from __future__ import annotations

import logging
from typing import TYPE_CHECKING, Any, Generic, TypeVar

from typedb.driver import TransactionType

from type_bridge.crud.formatting import format_value
from type_bridge.crud.strategies import EntityStrategy, ModelStrategy, RelationStrategy
from type_bridge.crud.types import is_multi_value_attribute
from type_bridge.models import Entity, Relation
from type_bridge.query.ast import MatchClause
from type_bridge.query.compiler import QueryCompiler
from type_bridge.session import Connection, ConnectionExecutor

if TYPE_CHECKING:
    from type_bridge.models.base import TypeDBType

logger = logging.getLogger(__name__)

T = TypeVar("T", bound="TypeDBType")


class TypeDBManager(Generic[T]):
    """Unified CRUD manager for TypeDB entities and relations."""

    def __init__(self, connection: Connection, model_class: type[T]):
        self._connection = connection
        self._executor = ConnectionExecutor(connection)
        self.model_class = model_class
        self.compiler = QueryCompiler()

        # Select strategy
        if issubclass(model_class, Entity):
            self.strategy: ModelStrategy = EntityStrategy()
        elif issubclass(model_class, Relation):
            self.strategy = RelationStrategy()
        else:
            raise TypeError(f"Unsupported model type: {model_class}")

    def _execute(self, query: str, tx_type: TransactionType) -> list[dict[str, Any]]:
        return self._executor.execute(query, tx_type)

    def insert(self, instance: T) -> T:
        """Insert a new instance."""
        var = "$x"
        match_clause, insert_clause = self.strategy.build_insert(instance, var)

        query_parts = []
        if match_clause:
            query_parts.append(self.compiler.compile(match_clause))

        query_parts.append(self.compiler.compile(insert_clause))

        query = "\n".join(query_parts)

        # Execute (WRITE transaction)
        self._execute(query, TransactionType.WRITE)

        # TODO: Retrieve and set IID on instance
        return instance

    def get(self, **filters) -> list[T]:
        """Get instances matching filters."""
        var = "$x"
        match_clause = self.strategy.build_match_all(self.model_class, var, filters)

        # We need to fetch attributes to rehydrate the objects
        # TypeQL 3.x fetch syntax: fetch { $x.* }
        from type_bridge.query.ast import FetchClause

        fetch_clause = FetchClause(items=[f"{var}.*"])

        query = self.compiler.compile_batch([match_clause, fetch_clause])

        # Execute (READ transaction)
        results = self._execute(query, TransactionType.READ)

        # Hydrate objects
        # This duplicates logic from old manager's hydration.
        # We should use a shared hydration utility or port it here.
        # For now, simplistic hydration to verify architectural flow.

        instances = []
        for result in results:
            # result is a dict of attributes (field_name -> value)
            # TODO: Robust hydration including nested attributes and types
            # Note: _execute wrapper handles some extraction, assume
            # standard TypeDB JSON-like output or ConceptMap
            try:
                # Assuming result is { "name": "Alice", "age": 30 }
                instance = self.model_class.from_dict(result, strict=False)
                instances.append(instance)
            except Exception as e:
                logger.error(f"Failed to hydrate instance: {e}")

        return instances

    def update(self, instance: T) -> T:
        """Update an instance in the database.

        Uses the Strategy pattern to identify the instance, then updates
        all non-key attributes to match the current state.

        Args:
            instance: Instance with updated values

        Returns:
            The updated instance
        """
        var = "$x"
        constraints = self.strategy.identify(instance)
        all_attrs = self.model_class.get_all_attributes()

        # Build the base match clause using AST
        from type_bridge.query.ast import EntityPattern, RelationPattern

        if issubclass(self.model_class, Entity):
            pattern = EntityPattern(
                variable=var,
                type_name=self.model_class.get_type_name(),
                constraints=constraints,
            )
        else:
            pattern = RelationPattern(
                variable=var,
                type_name=self.model_class.get_type_name(),
                role_players=[],
                constraints=constraints,
            )

        base_match = self.compiler.compile(MatchClause(patterns=[pattern]))
        # Remove "match\n" prefix as we'll rebuild it
        base_match_body = base_match[6:] if base_match.startswith("match\n") else base_match

        # Separate single-value and multi-value attributes
        single_value_updates: dict[str, Any] = {}
        multi_value_updates: dict[str, list[Any]] = {}
        single_value_deletes: list[str] = []

        for field_name, attr_info in all_attrs.items():
            # Skip key attributes - they identify the instance, can't be changed
            if attr_info.flags.is_key:
                continue

            value = getattr(instance, field_name, None)
            attr_name = attr_info.typ.get_attribute_name()
            is_multi = is_multi_value_attribute(attr_info.flags)

            if value is None:
                # Mark for deletion (if attribute exists)
                single_value_deletes.append(attr_name)
            elif is_multi and isinstance(value, list):
                multi_value_updates[attr_name] = value
            else:
                single_value_updates[attr_name] = value

        # Build try blocks for match clause
        try_blocks: list[str] = []

        # Add bindings for multi-value attributes with guards
        for attr_name, values in multi_value_updates.items():
            keep_literals = [format_value(v) for v in dict.fromkeys(values)]
            guard_lines = [f"not {{ ${attr_name} == {lit}; }};" for lit in keep_literals]
            try_block = "\n".join(
                [
                    "try {",
                    f"  {var} has {attr_name} ${attr_name};",
                    *[f"  {g}" for g in guard_lines],
                    "};",
                ]
            )
            try_blocks.append(try_block)

        # Add bindings for single-value updates (delete old + insert new)
        for attr_name in single_value_updates:
            try_blocks.append(f"try {{ {var} has {attr_name} $old_{attr_name}; }};")

        # Add bindings for single-value deletes
        for attr_name in single_value_deletes:
            try_blocks.append(f"try {{ {var} has {attr_name} ${attr_name}; }};")

        # Combine base match with try blocks
        if try_blocks:
            match_clause_str = base_match_body + "\n" + "\n".join(try_blocks)
        else:
            match_clause_str = base_match_body
        query_parts = [f"match\n{match_clause_str}"]

        # Build delete clause
        delete_parts = []
        for attr_name in multi_value_updates:
            delete_parts.append(f"try {{ ${attr_name} of {var}; }};")
        for attr_name in single_value_updates:
            delete_parts.append(f"try {{ $old_{attr_name} of {var}; }};")
        for attr_name in single_value_deletes:
            delete_parts.append(f"try {{ ${attr_name} of {var}; }};")

        if delete_parts:
            query_parts.append("delete\n" + "\n".join(delete_parts))

        # Build insert clause
        insert_parts = []
        for attr_name, values in multi_value_updates.items():
            for value in values:
                insert_parts.append(f"{var} has {attr_name} {format_value(value)};")
        for attr_name, value in single_value_updates.items():
            insert_parts.append(f"{var} has {attr_name} {format_value(value)};")

        if insert_parts:
            query_parts.append("insert\n" + "\n".join(insert_parts))

        full_query = "\n".join(query_parts)
        logger.debug(f"Update query: {full_query}")

        self._execute(full_query, TransactionType.WRITE)
        logger.info(f"Updated: {self.model_class.__name__}")

        return instance

    def delete(self, instance: T) -> None:
        """Delete an instance."""
        var = "$x"
        constraints = self.strategy.identify(instance)

        from type_bridge.query.ast import (
            DeleteClause,
            DeleteThingStatement,
            EntityPattern,
            RelationPattern,
        )

        # Build match pattern based on model type
        if issubclass(self.model_class, Entity):
            pattern = EntityPattern(
                variable=var,
                type_name=self.model_class.get_type_name(),
                constraints=constraints,
            )
        else:
            # Relation - IID-based match doesn't need role players
            pattern = RelationPattern(
                variable=var,
                type_name=self.model_class.get_type_name(),
                role_players=[],
                constraints=constraints,
            )

        match_clause = MatchClause(patterns=[pattern])
        delete_clause = DeleteClause(statements=[DeleteThingStatement(variable=var)])

        query = self.compiler.compile(match_clause) + "\n" + self.compiler.compile(delete_clause)

        self._execute(query, TransactionType.WRITE)
