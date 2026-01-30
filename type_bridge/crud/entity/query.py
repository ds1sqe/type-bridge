"""Chainable query operations for entities."""

import logging
import re
from typing import TYPE_CHECKING, Any, cast

from typedb.driver import TransactionType

from type_bridge.models import Entity
from type_bridge.query import Query, QueryBuilder
from type_bridge.session import Connection, ConnectionExecutor

from ..base import E
from ..exceptions import KeyAttributeError
from ..utils import (
    format_value,
    is_multi_value_attribute,
    resolve_entity_class,
)

logger = logging.getLogger(__name__)

if TYPE_CHECKING:
    from .group_by import GroupByQuery


class EntityQuery[E: Entity]:
    """Chainable query for entities.

    Type-safe query builder that preserves entity type information.
    Supports both dictionary filters (exact match) and expression-based filters.
    """

    def __init__(
        self,
        connection: Connection,
        model_class: type[E],
        filters: dict[str, Any] | None = None,
    ):
        """Initialize entity query.

        Args:
            connection: Database, Transaction, or TransactionContext
            model_class: Entity model class
            filters: Attribute filters (exact match) - optional, defaults to empty dict
        """
        self._connection = connection
        self._executor = ConnectionExecutor(connection)
        self.model_class = model_class
        self.filters = filters or {}
        self._expressions: list[Any] = []  # Store Expression objects
        self._limit_value: int | None = None
        self._offset_value: int | None = None
        self._order_by_fields: list[tuple[str, str]] = []  # [(field_name, direction)]

    def filter(self, *expressions: Any) -> "EntityQuery[E]":
        """Add expression-based filters to the query.

        Args:
            *expressions: Expression objects (ComparisonExpr, StringExpr, etc.)

        Returns:
            Self for chaining

        Example:
            query = Person.manager(db).filter(
                Age.gt(Age(30)),
                Name.contains(Name("Alice"))
            )

        Raises:
            ValueError: If expression references attribute type not owned by entity
        """
        # Validate expressions reference owned attribute types (including inherited)
        if expressions:
            owned_attrs = self.model_class.get_all_attributes()
            owned_attr_types = {attr_info.typ for attr_info in owned_attrs.values()}

            for expr in expressions:
                # Get attribute types from expression
                expr_attr_types = expr.get_attribute_types()

                # Check if all attribute types are owned by entity
                for attr_type in expr_attr_types:
                    if attr_type not in owned_attr_types:
                        raise ValueError(
                            f"{self.model_class.__name__} does not own attribute type {attr_type.__name__}. "
                            f"Available attribute types: {', '.join(t.__name__ for t in owned_attr_types)}"
                        )

        self._expressions.extend(expressions)
        return self

    def limit(self, limit: int) -> "EntityQuery[E]":
        """Limit number of results.

        Args:
            limit: Maximum number of results

        Returns:
            Self for chaining
        """
        self._limit_value = limit
        return self

    def offset(self, offset: int) -> "EntityQuery[E]":
        """Skip number of results.

        Args:
            offset: Number of results to skip

        Returns:
            Self for chaining
        """
        self._offset_value = offset
        return self

    def order_by(self, *fields: str) -> "EntityQuery[E]":
        """Sort query results by one or more fields.

        Args:
            *fields: Field names to sort by. Prefix with '-' for descending order.

        Returns:
            Self for chaining

        Raises:
            ValueError: If field name does not correspond to an owned attribute
            ValueError: If attempting to sort by a multi-value attribute

        Example:
            # Ascending
            query.order_by('name')

            # Descending
            query.order_by('-age')

            # Multiple fields
            query.order_by('department', '-salary')
        """
        owned_attrs = self.model_class.get_all_attributes()

        for field in fields:
            # Parse direction prefix
            if field.startswith("-"):
                direction = "desc"
                field_name = field[1:]
            else:
                direction = "asc"
                field_name = field

            # Validate field exists
            if field_name not in owned_attrs:
                raise ValueError(
                    f"Unknown sort field '{field_name}' for {self.model_class.__name__}. "
                    f"Available fields: {list(owned_attrs.keys())}"
                )

            # Reject multi-value attributes
            if is_multi_value_attribute(owned_attrs[field_name].flags):
                raise ValueError(
                    f"Cannot sort by multi-value attribute '{field_name}'. "
                    "Multi-value attributes can have multiple values per entity."
                )

            self._order_by_fields.append((field_name, direction))

        return self

    def execute(self) -> list[E]:
        """Execute the query.

        Returns entities with their actual concrete type, enabling polymorphic
        queries. When querying a supertype, entities are instantiated as their
        actual subtype class if the subclass is defined in Python.

        Returns:
            List of matching entities with _iid populated and correct concrete type
        """
        logger.debug(
            f"Executing EntityQuery: {self.model_class.__name__}, "
            f"filters={self.filters}, expressions={len(self._expressions)}"
        )
        query = QueryBuilder.match_entity(self.model_class, **self.filters)

        # Apply expression-based filters
        for expr in self._expressions:
            # Generate TypeQL pattern from expression
            pattern = expr.to_typeql("$e")
            query.match(pattern)

        query.fetch("$e")  # Fetch all attributes with $e.*

        # Apply sorting - either user-specified or auto-select for pagination
        owned_attrs = self.model_class.get_all_attributes()

        if self._order_by_fields:
            # User-specified sort fields
            for i, (field_name, direction) in enumerate(self._order_by_fields):
                attr_info = owned_attrs[field_name]
                attr_name = attr_info.typ.get_attribute_name()
                sort_var = f"$sort_{i}"
                query.match(f"$e has {attr_name} {sort_var}")
                query.sort(sort_var, direction)
        elif self._limit_value is not None or self._offset_value is not None:
            # TypeDB 3.x requires sorting for pagination to work reliably
            # Auto-select a sort attribute when using limit or offset
            sort_attr = None

            # Try to find a key attribute first (keys are always present and unique)
            for field_name, attr_info in owned_attrs.items():
                if attr_info.flags.is_key:
                    sort_attr = attr_info.typ.get_attribute_name()
                    break

            # If no key found, try to find any required attribute
            if sort_attr is None:
                for field_name, attr_info in owned_attrs.items():
                    if attr_info.flags.card_min is not None and attr_info.flags.card_min >= 1:
                        sort_attr = attr_info.typ.get_attribute_name()
                        break

            # Add sort clause with attribute variable
            if sort_attr:
                query.match(f"$e has {sort_attr} $sort_attr")
                query.sort("$sort_attr", "asc")

        if self._limit_value is not None:
            query.limit(self._limit_value)
        if self._offset_value is not None:
            query.offset(self._offset_value)

        query_str = query.build()
        logger.debug(f"EntityQuery: {query_str}")
        results = self._execute(query_str, TransactionType.READ)
        logger.debug(f"Query returned {len(results)} results")

        if not results:
            return []

        # Get IIDs and types for polymorphic instantiation
        iid_type_map = self._get_iids_and_types()

        # Convert results to entity instances with correct concrete type
        entities = []
        base_attrs = self.model_class.get_all_attributes()
        for result in results:
            # First, extract base attributes for matching
            base_attr_values = {}
            for field_name, attr_info in base_attrs.items():
                attr_class = attr_info.typ
                attr_name = attr_class.get_attribute_name()
                if attr_name in result:
                    base_attr_values[field_name] = result[attr_name]
                else:
                    if attr_info.flags.has_explicit_card:
                        base_attr_values[field_name] = []
                    else:
                        base_attr_values[field_name] = None

            # Find matching IID/type and resolve class
            entity_class, iid = self._match_entity_type(base_attr_values, iid_type_map, base_attrs)

            # Now extract all attributes using the resolved class (includes subtype attrs)
            resolved_attrs = entity_class.get_all_attributes()
            attrs = {}
            for field_name, attr_info in resolved_attrs.items():
                attr_class = attr_info.typ
                attr_name = attr_class.get_attribute_name()
                if attr_name in result:
                    attrs[field_name] = result[attr_name]
                else:
                    if attr_info.flags.has_explicit_card:
                        attrs[field_name] = []
                    else:
                        attrs[field_name] = None

            entity = entity_class(**attrs)
            if iid:
                object.__setattr__(entity, "_iid", iid)
            entities.append(entity)

        logger.info(f"EntityQuery executed: {len(entities)} entities returned")
        return entities

    def _get_iids_and_types(self) -> dict[tuple[tuple[str, Any], ...], tuple[str, str]]:
        """Get IIDs and type names for entities matching current query.

        Uses a single fetch query with iid() and label() functions to get
        entity IIDs and types alongside attributes in one query.

        Returns:
            Dictionary mapping key_values_tuple to (iid, type_name) tuple
        """
        # Get key attributes for building the lookup key
        owned_attrs = self.model_class.get_all_attributes()
        key_attrs = {
            field_name: attr_info
            for field_name, attr_info in owned_attrs.items()
            if attr_info.flags.is_key
        }

        # Build match query with filters and expressions, adding type variable for label()
        # TypeQL's label() function works on TYPE variables, not instance variables
        # So we use: $e isa! $t (exact type match) then label($t)
        query = QueryBuilder.match_entity(self.model_class, **self.filters)

        for expr in self._expressions:
            pattern = expr.to_typeql("$e")
            query.match(pattern)

        match_str = query.build().rstrip().rstrip(";")

        # Modify match to bind exact type:
        # Original: "$e isa person, has Name X"
        # Changed:  "$e isa! $t, has Name X; $t sub person"
        type_name = self.model_class.get_type_name()
        match_str = match_str.replace(f"$e isa {type_name}", "$e isa! $t")
        match_str = f"{match_str}; $t sub {type_name}"

        # Track key values we already know from filters
        known_key_values: dict[str, Any] = {}
        for field_name, attr_info in key_attrs.items():
            attr_name = attr_info.typ.get_attribute_name()
            if field_name in self.filters:
                filter_value = self.filters[field_name]
                if hasattr(filter_value, "value"):
                    filter_value = filter_value.value
                known_key_values[attr_name] = filter_value

        # Build fetch items: iid, label (on type var), and key attributes
        # TypeQL doesn't allow mixing "key": value entries with $e.*
        # Note: label($t) works because $t is a TYPE variable bound via isa!
        fetch_items = ['"_iid": iid($e)', '"_type": label($t)']

        # Add key attributes to fetch (only if not all known from filters)
        key_attr_names = [attr_info.typ.get_attribute_name() for attr_info in key_attrs.values()]
        if not (known_key_values and len(known_key_values) == len(key_attrs)):
            for attr_name in key_attr_names:
                fetch_items.append(f'"{attr_name}": $e.{attr_name}')

        fetch_clause = f"fetch {{\n  {', '.join(fetch_items)}\n}}"
        query_str = f"{match_str};\n{fetch_clause};"
        logger.debug(f"IID/type query: {query_str}")
        results = self._execute(query_str, TransactionType.READ)

        iid_type_map: dict[tuple[tuple[str, Any], ...], tuple[str, str]] = {}

        for result in results:
            # Get IID/type from fetch result (provided by iid($e) and label($e))
            iid = result.get("_iid")
            type_name = result.get("_type")
            if not iid or not type_name:
                continue

            # Build map key from key attributes or known values
            if key_attrs:
                if known_key_values and len(known_key_values) == len(key_attrs):
                    # All key values known from filters
                    map_key = tuple(sorted(known_key_values.items()))
                else:
                    # Extract key values from fetch result
                    key_values: list[tuple[str, Any]] = []
                    for attr_name in key_attr_names:
                        if attr_name in result:
                            val = result[attr_name]
                            key_values.append((attr_name, val))
                    if not key_values:
                        continue
                    map_key = tuple(sorted(key_values))
            else:
                # No key attributes, use IID as the map key
                map_key = (("_iid", iid),)

            iid_type_map[map_key] = (iid, type_name)

        logger.debug(f"Found {len(iid_type_map)} IID/type mappings")
        return iid_type_map

    def _match_entity_type(
        self,
        attrs: dict[str, Any],
        iid_type_map: dict[tuple[tuple[str, Any], ...], tuple[str, str]],
        owned_attrs: dict[str, Any],
    ) -> tuple[type[E], str | None]:
        """Match entity attributes to IID/type and resolve the correct class.

        Uses key attributes to look up the corresponding IID/type from the map
        (in-memory, no database query), then resolves the actual Python class
        for polymorphic instantiation.

        Args:
            attrs: Extracted attributes for the entity
            iid_type_map: Map from key_values_tuple to (iid, type_name)
            owned_attrs: Attribute metadata for the model class

        Returns:
            Tuple of (resolved_class, iid) where resolved_class is the
            concrete subclass if found, otherwise self.model_class
        """
        # If no type info available, use model_class
        if not iid_type_map:
            return self.model_class, None

        # Get key attributes for matching
        key_attrs = {
            field_name: attr_info
            for field_name, attr_info in owned_attrs.items()
            if attr_info.flags.is_key
        }

        if not key_attrs:
            # No key attributes - can't match reliably, use first available
            if iid_type_map:
                iid, type_name = next(iter(iid_type_map.values()))
                resolved_class = cast(type[E], resolve_entity_class(self.model_class, type_name))
                return resolved_class, iid
            return self.model_class, None

        # Build key signature from attrs for in-memory lookup
        key_values: list[tuple[str, Any]] = []
        for field_name, attr_info in key_attrs.items():
            value = attrs.get(field_name)
            if value is not None:
                if hasattr(value, "value"):
                    value = value.value
                attr_name = attr_info.typ.get_attribute_name()
                key_values.append((attr_name, value))

        if not key_values:
            return self.model_class, None

        # Look up in the map using key values (no database query!)
        map_key = tuple(sorted(key_values))
        if map_key in iid_type_map:
            iid, type_name = iid_type_map[map_key]
            resolved_class = cast(type[E], resolve_entity_class(self.model_class, type_name))
            return resolved_class, iid

        return self.model_class, None

    def _populate_iids(self, entities: list[E]) -> None:
        """Populate _iid field on entities by querying TypeDB.

        Uses a single batched fetch query with iid() to get IIDs for all
        entities at once. Optimized to use O(1) queries instead of O(N) queries.

        Args:
            entities: List of entities to populate IIDs for
        """
        if not entities:
            return

        # Get key attributes for matching
        owned_attrs = self.model_class.get_all_attributes()
        key_attrs = {
            field_name: attr_info
            for field_name, attr_info in owned_attrs.items()
            if attr_info.flags.is_key
        }

        if not key_attrs:
            # No key attributes - cannot reliably match IIDs to entities
            logger.debug("No key attributes found, skipping IID population")
            return

        # Build batched disjunctive query for all entities
        or_clauses = []
        for entity in entities:
            match_parts = [f"$e isa {self.model_class.get_type_name()}"]
            for field_name, attr_info in key_attrs.items():
                value = getattr(entity, field_name, None)
                if value is not None:
                    if hasattr(value, "value"):
                        value = value.value
                    attr_name = attr_info.typ.get_attribute_name()
                    formatted_value = format_value(value)
                    match_parts.append(f"has {attr_name} {formatted_value}")

            or_clauses.append(f"{{ {', '.join(match_parts)}; }}")

        if not or_clauses:
            return

        # Build fetch items: iid and key attributes (for matching)
        # TypeQL doesn't allow mixing "key": value entries with $e.*
        key_attr_names = [attr_info.typ.get_attribute_name() for attr_info in key_attrs.values()]
        fetch_items = ['"_iid": iid($e)']
        for attr_name in key_attr_names:
            fetch_items.append(f'"{attr_name}": $e.{attr_name}')
        fetch_clause = f"fetch {{\n  {', '.join(fetch_items)}\n}}"

        query_str = f"match\n{' or '.join(or_clauses)};\n{fetch_clause};"
        logger.debug(f"Batched IID lookup query: {query_str[:200]}...")

        results = self._execute(query_str, TransactionType.READ)

        if not results:
            return

        iid_map: dict[tuple[tuple[str, Any], ...], str] = {}

        # Extract IID and key values from single fetch result
        for result in results:
            iid = result.get("_iid")
            if not iid:
                continue

            # Build key from fetch result
            key_values: list[tuple[str, Any]] = []
            for attr_name in key_attr_names:
                if attr_name in result:
                    key_values.append((attr_name, result[attr_name]))

            if key_values:
                iid_map[tuple(sorted(key_values))] = iid

        # Assign IIDs to entities using in-memory lookup
        for entity in entities:
            key_values = []
            for field_name, attr_info in key_attrs.items():
                value = getattr(entity, field_name, None)
                if value is not None:
                    if hasattr(value, "value"):
                        value = value.value
                    attr_name = attr_info.typ.get_attribute_name()
                    key_values.append((attr_name, value))

            if key_values:
                map_key = tuple(sorted(key_values))
                if map_key in iid_map:
                    object.__setattr__(entity, "_iid", iid_map[map_key])
                    logger.debug(
                        f"Set IID {iid_map[map_key]} for entity {self.model_class.__name__}"
                    )

    def first(self) -> E | None:
        """Get first matching entity.

        Returns:
            First entity or None
        """
        results = self.limit(1).execute()
        return results[0] if results else None

    def count(self) -> int:
        """Count matching entities.

        Returns:
            Number of matching entities
        """
        return len(self.execute())

    def delete(self) -> int:
        """Delete all entities matching the current filters.

        Builds and executes a delete query based on the current filter state.
        Uses a single transaction for atomic deletion.

        Returns:
            Number of entities deleted

        Example:
            # Delete all persons over 65
            count = Person.manager(db).filter(Age.gt(Age(65))).delete()
            print(f"Deleted {count} persons")

            # Delete with multiple filters
            count = Person.manager(db).filter(
                Age.lt(Age(18)),
                Status.eq(Status("inactive"))
            ).delete()
        """
        # Build match clause
        query = Query()
        pattern_parts = [f"$e isa {self.model_class.get_type_name()}"]

        # Add dictionary-based filters (exact match)
        owned_attrs = self.model_class.get_all_attributes()
        for field_name, field_value in self.filters.items():
            if field_name in owned_attrs:
                attr_info = owned_attrs[field_name]
                attr_name = attr_info.typ.get_attribute_name()
                formatted_value = format_value(field_value)
                pattern_parts.append(f"has {attr_name} {formatted_value}")

        # Combine base pattern
        pattern = ", ".join(pattern_parts)
        query.match(pattern)

        # Add expression-based filters
        for expr in self._expressions:
            expr_pattern = expr.to_typeql("$e")
            query.match(expr_pattern)

        # Add delete clause
        query.delete("$e")

        # Execute in single transaction
        query_str = query.build()
        logger.debug(f"Delete query: {query_str}")
        results = self._execute(query_str, TransactionType.WRITE)
        count = len(results) if results else 0
        logger.info(f"Deleted {count} entities via filter")

        return count

    def update_with(self, func: Any) -> list[E]:
        """Update entities by applying a function to each matching entity.

        Fetches all matching entities, applies the provided function to each one,
        then saves all updates in a single batched query. If the function raises an
        error on any entity, stops immediately and raises the error.

        Args:
            func: Callable that takes an entity and modifies it in-place.
                  Can be a lambda or regular function.

        Returns:
            List of updated entities

        Example:
            # Increment age for all persons over 30
            updated = Person.manager(db).filter(Age.gt(Age(30))).update_with(
                lambda person: setattr(person, 'age', Age(person.age.value + 1))
            )

            # Complex update with function
            def promote(person):
                person.status = Status("promoted")
                if person.salary:
                    person.salary = Salary(int(person.salary.value * 1.1))

            promoted = Person.manager(db).filter(
                Department.eq(Department("Engineering"))
            ).update_with(promote)

        Raises:
            Any exception raised by the function during processing
        """
        # Fetch all matching entities
        entities = self.execute()

        # Return empty list if no matches
        if not entities:
            return []

        # Apply function to each entity (stop and raise if error)
        for entity in entities:
            func(entity)

        # Build batched update query for all entities
        batched_query = self._build_batched_update_query(entities)

        if not batched_query:
            return entities

        # Execute the batched query
        self._executor.execute(batched_query, TransactionType.WRITE)

        return entities

    def _build_batched_update_query(self, entities: list[E]) -> str:
        """Build a single batched TypeQL query to update multiple entities.

        Uses conjunctive batching pattern similar to update_many in EntityManager.

        Args:
            entities: List of entity instances to update

        Returns:
            Single TypeQL query string that updates all entities
        """
        if not entities:
            return ""

        match_parts = []
        delete_parts = []
        insert_parts = []
        update_parts = []

        for i, entity in enumerate(entities):
            var_name = f"$e{i}"
            m_part, d_part, i_part, u_part = self._build_update_query_parts(entity, var_name)

            if m_part:
                match_parts.append(m_part)
            if d_part:
                delete_parts.append(d_part)
            if i_part:
                insert_parts.append(i_part)
            if u_part:
                update_parts.append(u_part)

        # Construct full query
        query_sections = []

        if match_parts:
            query_sections.append("match")
            query_sections.extend(match_parts)

        if delete_parts:
            query_sections.append("delete")
            query_sections.extend(delete_parts)

        if insert_parts:
            query_sections.append("insert")
            query_sections.extend(insert_parts)

        if update_parts:
            query_sections.append("update")
            query_sections.append("\n".join(update_parts))

        return "\n".join(query_sections)

    def _build_update_query_parts(
        self, entity: E, var_name: str = "$e"
    ) -> tuple[str, str, str, str]:
        """Build the TypeQL query parts for updating an entity.

        Returns:
            Tuple of (match_clause, delete_clause, insert_clause, update_clause)
        """
        owned_attrs = self.model_class.get_all_attributes()

        # Extract key attributes for matching
        match_filters = {}
        for field_name, attr_info in owned_attrs.items():
            if attr_info.flags.is_key:
                key_value = getattr(entity, field_name, None)
                if key_value is None:
                    raise KeyAttributeError(
                        entity_type=self.model_class.__name__,
                        operation="update",
                        field_name=field_name,
                    )
                if hasattr(key_value, "value"):
                    key_value = key_value.value
                attr_name = attr_info.typ.get_attribute_name()
                match_filters[attr_name] = key_value

        if not match_filters:
            raise KeyAttributeError(
                entity_type=self.model_class.__name__,
                operation="update",
                all_fields=list(owned_attrs.keys()),
            )

        # Separate single-value and multi-value updates
        single_value_updates = {}
        single_value_deletes = set()
        multi_value_updates = {}

        for field_name, attr_info in owned_attrs.items():
            if attr_info.flags.is_key:
                continue

            attr_class = attr_info.typ
            attr_name = attr_class.get_attribute_name()
            flags = attr_info.flags

            current_value = getattr(entity, field_name, None)

            # Extract raw values
            if current_value is not None:
                if isinstance(current_value, list):
                    raw_values = []
                    for item in current_value:
                        if hasattr(item, "value"):
                            raw_values.append(item.value)
                        else:
                            raw_values.append(item)
                    current_value = raw_values
                elif hasattr(current_value, "value"):
                    current_value = current_value.value

            is_multi_value = is_multi_value_attribute(flags)

            if is_multi_value:
                if current_value is None:
                    current_value = []
                multi_value_updates[attr_name] = current_value
            else:
                if current_value is not None:
                    single_value_updates[attr_name] = current_value
                elif flags.card_min == 0:
                    single_value_deletes.add(attr_name)

        # Build Match Clause
        match_statements = []
        entity_match_parts = [f"{var_name} isa {self.model_class.get_type_name()}"]
        for attr_name, attr_value in match_filters.items():
            formatted_value = format_value(attr_value)
            entity_match_parts.append(f"has {attr_name} {formatted_value}")
        match_statements.append(", ".join(entity_match_parts) + ";")

        # Add match for multi-value attributes with guards
        if multi_value_updates:
            for attr_name, values in multi_value_updates.items():
                keep_literals = [format_value(v) for v in dict.fromkeys(values)]
                attr_var = f"${attr_name}_{var_name.replace('$', '')}"
                guard_lines = [f"not {{ {attr_var} == {literal}; }};" for literal in keep_literals]
                try_block = "\n".join(
                    [
                        "try {",
                        f"  {var_name} has {attr_name} {attr_var};",
                        *[f"  {g}" for g in guard_lines],
                        "};",
                    ]
                )
                match_statements.append(try_block)

        # Add match for single-value deletes
        if single_value_deletes:
            for attr_name in single_value_deletes:
                attr_var = f"${attr_name}_{var_name.replace('$', '')}"
                match_statements.append(f"try {{ {var_name} has {attr_name} {attr_var}; }};")

        match_clause = "\n".join(match_statements)

        # Build Delete Clause
        delete_parts = []
        if multi_value_updates:
            for attr_name in multi_value_updates:
                attr_var = f"${attr_name}_{var_name.replace('$', '')}"
                delete_parts.append(f"try {{ {attr_var} of {var_name}; }};")

        if single_value_deletes:
            for attr_name in single_value_deletes:
                attr_var = f"${attr_name}_{var_name.replace('$', '')}"
                delete_parts.append(f"try {{ {attr_var} of {var_name}; }};")

        delete_clause = "\n".join(delete_parts)

        # Build Insert Clause
        insert_parts = []
        for attr_name, values in multi_value_updates.items():
            for value in values:
                formatted_value = format_value(value)
                insert_parts.append(f"{var_name} has {attr_name} {formatted_value};")

        insert_clause = "\n".join(insert_parts)

        # Build Update Clause
        update_parts = []
        if single_value_updates:
            for attr_name, value in single_value_updates.items():
                formatted_value = format_value(value)
                update_parts.append(f"{var_name} has {attr_name} {formatted_value};")

        update_clause = "\n".join(update_parts)

        return match_clause, delete_clause, insert_clause, update_clause

    def aggregate(self, *aggregates: Any) -> dict[str, Any]:
        """Execute aggregation queries.

        Performs database-side aggregations for efficiency.

        Args:
            *aggregates: AggregateExpr objects (Person.age.avg(), Person.score.sum(), etc.)

        Returns:
            Dictionary mapping aggregate keys to results

        Examples:
            # Single aggregation
            result = manager.filter().aggregate(Person.age.avg())
            avg_age = result['avg_age']

            # Multiple aggregations
            result = manager.filter(Person.city.eq(City("NYC"))).aggregate(
                Person.age.avg(),
                Person.score.sum(),
                Person.salary.max()
            )
            avg_age = result['avg_age']
            total_score = result['sum_score']
            max_salary = result['max_salary']
        """
        from type_bridge.expressions import AggregateExpr

        if not aggregates:
            raise ValueError("At least one aggregation expression required")

        # Build base match query with filters
        query = QueryBuilder.match_entity(self.model_class, **self.filters)

        # Apply expression-based filters
        for expr in self._expressions:
            pattern = expr.to_typeql("$e")
            query.match(pattern)

        # Build reduce query with aggregations
        # TypeQL 3.x syntax: reduce $result = function($var);
        # First, we need to bind all the fields being aggregated in the match clause
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

        # Convert match to reduce query
        match_clause = query.build().replace("fetch", "get").split("fetch")[0]
        reduce_query = f"{match_clause}\nreduce {', '.join(reduce_clauses)};"

        results = self._execute(reduce_query, TransactionType.READ)

        # Parse aggregation results
        # TypeDB 3.x reduce operator returns results as formatted strings
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
                        value = float(value_str)
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

    def _execute(self, query: str, tx_type: TransactionType) -> list[dict[str, Any]]:
        """Execute a query using an existing transaction if available."""
        return self._executor.execute(query, tx_type)

    def group_by(self, *fields: Any) -> "GroupByQuery[E]":
        """Group entities by field values.

        Args:
            *fields: FieldRef objects to group by

        Returns:
            GroupByQuery for chained aggregations

        Example:
            result = manager.group_by(Person.city).aggregate(Person.age.avg())
        """
        # Import here to avoid circular dependency
        from .group_by import GroupByQuery

        return GroupByQuery(
            self._connection,
            self.model_class,
            self.filters,
            self._expressions,
            fields,
        )
