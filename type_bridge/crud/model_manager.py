"""Unified model manager for CRUD operations.

This module provides a unified ModelManager that handles CRUD operations
for both entities and relations using the model-level abstraction methods.
"""

import logging
from abc import abstractmethod
from typing import TYPE_CHECKING, Any

from typedb.driver import TransactionType

from type_bridge.crud.formatting import format_value
from type_bridge.crud.manager import BaseManager

if TYPE_CHECKING:
    from type_bridge.models.base import TypeDBType

logger = logging.getLogger(__name__)


class ModelManager[T: "TypeDBType"](BaseManager[T]):
    """Unified manager for CRUD operations on both entities and relations.

    Provides shared implementations for insert, put, delete, and update
    operations using the model-level abstraction methods (get_match_clause_info,
    get_write_query_info).

    Subclasses must implement:
    - get(): Retrieve instances with optional filters
    - filter(): Create a chainable query
    - group_by(): Create a grouped aggregation query
    - _build_fetch_query(): Build fetch query specific to entity/relation
    """

    # =========================================================================
    # Write operations (insert, put)
    # =========================================================================

    def insert(self, instance: T) -> T:
        """Insert an instance into the database.

        Uses the model's get_write_query_info() method to build the query,
        supporting both entities (no match clause) and relations (with
        role player match clauses).

        Args:
            instance: Instance to insert

        Returns:
            The inserted instance (with _iid populated if successful)
        """
        logger.debug(f"Inserting: {self.model_class.__name__}")

        write_info = instance.get_write_query_info("$x")

        # Build the query
        if write_info.match_clause:
            query = f"match\n{write_info.match_clause};\ninsert\n{write_info.write_pattern};"
        else:
            query = f"insert\n{write_info.write_pattern};"

        logger.debug(f"Insert query: {query}")
        self._execute(query, TransactionType.WRITE)

        logger.info(f"Inserted: {self.model_class.__name__}")

        # Populate IID by fetching the instance back
        self._populate_iid_after_write(instance)

        return instance

    def put(self, instance: T) -> T:
        """Put an instance into the database (insert if not exists).

        Uses TypeQL's PUT clause for idempotent insertion.

        Args:
            instance: Instance to put

        Returns:
            The instance
        """
        logger.debug(f"Put: {self.model_class.__name__}")

        write_info = instance.get_write_query_info("$x")

        # Build the query with "put" instead of "insert"
        if write_info.match_clause:
            query = f"match\n{write_info.match_clause};\nput\n{write_info.write_pattern};"
        else:
            query = f"put\n{write_info.write_pattern};"

        logger.debug(f"Put query: {query}")
        self._execute(query, TransactionType.WRITE)

        logger.info(f"Put: {self.model_class.__name__}")

        # Populate IID by fetching the instance back
        self._populate_iid_after_write(instance)

        return instance

    def _populate_iid_after_write(self, instance: T) -> None:
        """Populate _iid on instance by querying it back.

        Uses the model's get_match_clause_info() if available (for entities with
        key attrs or relations with role players), or builds a match using all
        non-None attributes.
        """
        # Try to get match clause using the model's method
        try:
            match_info = instance.get_match_clause_info("$x")
            all_clauses = [match_info.main_clause] + match_info.extra_clauses
            match_clause = ";\n".join(all_clauses)
        except ValueError:
            # No IID and no keys/role players - build match from all attributes
            match_clause = self._build_attribute_match(instance, "$x")

        if not match_clause:
            logger.debug(f"Cannot fetch IID: no match clause for {self.model_class.__name__}")
            return

        query = f'match\n{match_clause};\nfetch {{ "_iid": iid($x) }};'
        logger.debug(f"Fetch IID query: {query}")

        try:
            results = self._execute(query, TransactionType.READ)
            if results and len(results) == 1:
                iid = results[0].get("_iid")
                if iid:
                    object.__setattr__(instance, "_iid", iid)
                    logger.debug(f"Populated _iid {iid} for {self.model_class.__name__}")
            elif results and len(results) > 1:
                logger.warning(
                    f"Multiple matches after write for {self.model_class.__name__}, "
                    f"_iid not populated. Consider adding @key attribute for uniqueness."
                )
        except Exception as e:
            logger.warning(f"Failed to fetch _iid after write: {e}")

    def _build_attribute_match(self, instance: T, var_name: str) -> str:
        """Build a match clause using all non-None attributes.

        Fallback for instances without IID or key attributes.
        """
        type_name = self.model_class.get_type_name()
        parts = [f"{var_name} isa {type_name}"]

        for field_name, attr_info in self.model_class.get_all_attributes().items():
            value = getattr(instance, field_name, None)
            if value is not None:
                attr_name = attr_info.typ.get_attribute_name()
                if isinstance(value, list):
                    for item in value:
                        parts.append(f"has {attr_name} {format_value(item)}")
                else:
                    parts.append(f"has {attr_name} {format_value(value)}")

        return ", ".join(parts)

    # =========================================================================
    # Delete operations
    # =========================================================================

    def delete(self, instance: T) -> T:
        """Delete an instance from the database.

        Prefers IID-based matching when _iid is set on the instance.
        Falls back to key attributes (entities) or role players (relations).

        Args:
            instance: Instance to delete

        Returns:
            The deleted instance
        """
        logger.debug(f"Deleting: {self.model_class.__name__}")

        match_info = instance.get_match_clause_info("$x")
        all_clauses = [match_info.main_clause] + match_info.extra_clauses
        match_clause = ";\n".join(all_clauses)

        query = f"match\n{match_clause};\ndelete\n$x;"
        logger.debug(f"Delete query: {query}")

        self._execute(query, TransactionType.WRITE)
        logger.info(f"Deleted: {self.model_class.__name__}")

        return instance

    # =========================================================================
    # Update operations
    # =========================================================================

    def update(self, instance: T) -> T:
        """Update an instance in the database.

        Uses the model's get_match_clause_info() to identify the instance,
        then updates all attributes to match the current state.

        Args:
            instance: Instance with updated values

        Returns:
            The updated instance
        """
        logger.debug(f"Updating: {self.model_class.__name__}")

        match_info = instance.get_match_clause_info("$x")
        all_attrs = self.model_class.get_all_attributes()

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

            is_multi = attr_info.flags.has_explicit_card

            if value is None:
                # Mark for deletion (if attribute exists)
                single_value_deletes.append(attr_name)
            elif is_multi and isinstance(value, list):
                multi_value_updates[attr_name] = value
            else:
                single_value_updates[attr_name] = value

        # Build match clause - join base clauses with semicolons
        base_clauses = [match_info.main_clause] + match_info.extra_clauses
        base_match = ";\n".join(base_clauses)

        # Collect try blocks (already have trailing semicolons)
        try_blocks: list[str] = []

        # Add bindings for multi-value attributes with guards
        for attr_name, values in multi_value_updates.items():
            keep_literals = [format_value(v) for v in dict.fromkeys(values)]
            guard_lines = [f"not {{ ${attr_name} == {lit}; }};" for lit in keep_literals]
            try_block = "\n".join([
                "try {",
                f"  $x has {attr_name} ${attr_name};",
                *[f"  {g}" for g in guard_lines],
                "};",
            ])
            try_blocks.append(try_block)

        # Add bindings for single-value updates (delete old + insert new)
        for attr_name in single_value_updates:
            try_blocks.append(f"try {{ $x has {attr_name} $old_{attr_name}; }};")

        # Add bindings for single-value deletes
        for attr_name in single_value_deletes:
            try_blocks.append(f"try {{ $x has {attr_name} ${attr_name}; }};")

        # Combine base match with try blocks
        if try_blocks:
            match_clause = base_match + ";\n" + "\n".join(try_blocks)
        else:
            match_clause = base_match
        query_parts = [f"match\n{match_clause}"]

        # Build delete clause
        delete_parts = []
        for attr_name in multi_value_updates:
            delete_parts.append(f"try {{ ${attr_name} of $x; }};")
        for attr_name in single_value_updates:
            delete_parts.append(f"try {{ $old_{attr_name} of $x; }};")
        for attr_name in single_value_deletes:
            delete_parts.append(f"try {{ ${attr_name} of $x; }};")

        if delete_parts:
            query_parts.append("delete\n" + "\n".join(delete_parts))

        # Build insert clause
        insert_parts = []
        for attr_name, values in multi_value_updates.items():
            for value in values:
                insert_parts.append(f"$x has {attr_name} {format_value(value)};")
        for attr_name, value in single_value_updates.items():
            insert_parts.append(f"$x has {attr_name} {format_value(value)};")

        if insert_parts:
            query_parts.append("insert\n" + "\n".join(insert_parts))

        full_query = "\n".join(query_parts)
        logger.debug(f"Update query: {full_query}")

        self._execute(full_query, TransactionType.WRITE)
        logger.info(f"Updated: {self.model_class.__name__}")

        return instance

    # =========================================================================
    # Abstract methods - subclasses must implement
    # =========================================================================

    @abstractmethod
    def get(self, **filters: Any) -> list[T]:
        """Get instances with optional filters.

        Args:
            **filters: Attribute filters (exact match)

        Returns:
            List of matching instances
        """
        ...
