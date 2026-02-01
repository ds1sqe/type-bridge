"""Chainable query operations for entities."""

import logging
from typing import TYPE_CHECKING, Any, cast

from typedb.driver import TransactionType

from type_bridge.models import Entity
from type_bridge.query import Query, QueryBuilder
from type_bridge.session import Connection

from ..base import BaseQuery, E, parse_aggregate_results
from ..utils import (
    assign_entity_iids,
    build_entity_iid_map,
    build_entity_iid_query,
    build_entity_update_query_parts,
    build_iid_type_fetch_clause,
    build_known_key_values,
    format_value,
    get_key_attrs,
    hydrate_attributes,
    is_multi_value_attribute,
    match_entity_type,
    modify_match_for_type_binding,
    process_iid_type_results,
)

logger = logging.getLogger(__name__)

if TYPE_CHECKING:
    from .group_by import GroupByQuery


class EntityQuery[E: Entity](BaseQuery[E]):
    """Chainable query for entities.

    Type-safe query builder that preserves entity type information.
    Supports both dictionary filters (exact match) and expression-based filters.
    """

    _order_by_fields: list[tuple[str, str]]

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
        super().__init__(connection, model_class, filters)
        self._order_by_fields = []  # [(field_name, direction)]

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
        super().limit(limit)
        return self

    def offset(self, offset: int) -> "EntityQuery[E]":
        """Skip number of results.

        Args:
            offset: Number of results to skip

        Returns:
            Self for chaining
        """
        super().offset(offset)
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
            # First, extract base attributes for matching (to find IID/type)
            base_attr_values, _ = hydrate_attributes(self.model_class, result)

            # Find matching IID/type and resolve class
            entity_class, iid = self._match_entity_type(base_attr_values, iid_type_map, base_attrs)

            # Now extract all attributes using the resolved class (includes subtype attrs)
            attrs, _ = hydrate_attributes(entity_class, result)

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
        # Get key attributes using shared utility
        key_attrs, key_attr_names = get_key_attrs(self.model_class)

        # Build match query with filters and expressions
        query = QueryBuilder.match_entity(self.model_class, **self.filters)

        for expr in self._expressions:
            pattern = expr.to_typeql("$e")
            query.match(pattern)

        match_str = query.build().rstrip().rstrip(";")

        # Modify match to bind exact type for label() retrieval
        type_name = self.model_class.get_type_name()
        match_str = modify_match_for_type_binding(match_str, type_name)

        # Track key values we already know from filters
        known_key_values = build_known_key_values(key_attrs, self.filters)

        # Build fetch clause
        fetch_clause = build_iid_type_fetch_clause(key_attr_names, known_key_values, key_attrs)
        query_str = f"{match_str};\n{fetch_clause};"
        logger.debug(f"IID/type query: {query_str}")
        results = self._execute(query_str, TransactionType.READ)

        # Process results using shared utility
        iid_type_map = process_iid_type_results(
            results, key_attrs, key_attr_names, known_key_values
        )

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
        resolved_class, iid = match_entity_type(attrs, iid_type_map, self.model_class, owned_attrs)
        return cast(type[E], resolved_class), iid

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
        key_attrs, _ = get_key_attrs(self.model_class)

        if not key_attrs:
            logger.debug("No key attributes found, skipping IID population")
            return

        # Build batched IID lookup query
        query_result = build_entity_iid_query(entities, self.model_class, key_attrs)
        if not query_result:
            return

        query_str, key_attr_names = query_result
        logger.debug(f"Batched IID lookup query: {query_str[:200]}...")

        results = self._execute(query_str, TransactionType.READ)
        if not results:
            return

        # Build IID map and assign to entities
        iid_map = build_entity_iid_map(results, key_attr_names)
        assign_entity_iids(entities, iid_map, key_attrs)

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

        Delegates to shared utility function that handles IID-based and key-based matching.

        Returns:
            Tuple of (match_clause, delete_clause, insert_clause, update_clause)
        """
        return build_entity_update_query_parts(entity, self.model_class, var_name)

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

        # Parse aggregation results using shared utility
        return parse_aggregate_results(results)

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
