"""Entity CRUD operations manager."""

import logging
import re
from typing import TYPE_CHECKING, Any, cast

from typedb.driver import TransactionType

from type_bridge.attribute.string import String
from type_bridge.expressions import AttributeExistsExpr, BooleanExpr, Expression
from type_bridge.models import Entity
from type_bridge.query import QueryBuilder

from ..base import E
from ..exceptions import EntityNotFoundError, KeyAttributeError
from ..model_manager import ModelManager
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
    match_entity_type,
    modify_match_for_type_binding,
    process_iid_type_results,
    resolve_entity_class,
    unwrap_attribute,
)

if TYPE_CHECKING:
    from .group_by import GroupByQuery
    from .query import EntityQuery

logger = logging.getLogger(__name__)


class EntityManager[E: Entity](ModelManager[E]):
    """Manager for entity CRUD operations.

    Type-safe manager that preserves entity type information.
    Inherits connection management and common operations from BaseManager.
    """

    def update_many(self, entities: list[E]) -> list[E]:
        """Update multiple entities within a single transaction.

        Uses an existing transaction when supplied, otherwise opens one write
        transaction and reuses it for all updates.

        Optimized to use batched TypeQL queries for improved performance.

        Args:
            entities: Entity instances to update

        Returns:
            The list of updated entities
        """
        if not entities:
            logger.debug("update_many called with empty list")
            return []

        logger.debug(f"Updating {len(entities)} entities: {self.model_class.__name__}")

        # Build batched query
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

        # Match section (all matches combined)
        if match_parts:
            query_sections.append("match")
            query_sections.extend(match_parts)

        # Delete section
        if delete_parts:
            query_sections.append("delete")
            query_sections.extend(delete_parts)

        # Insert section
        if insert_parts:
            query_sections.append("insert")
            query_sections.extend(insert_parts)

        # Update section (TypeQL supports 'update' as a clause which acts as combined match-delete-insert for simple properties)
        # Note: In standard TypeQL, 'update' clause might not be standard in all versions/drivers or mixed with delete/insert.
        # But here 'update' clause in self._build... returns "has attr new_val" parts.
        # Standard TypeQL is: match... delete... insert...
        # 'update' keyword usage in Type DB is syntactic sugar for replace.
        # If we have both delete/insert clauses AND update clauses, we should check if they can be mixed.
        # Usually 'update' stands alone with 'match'.
        # If we have mix, we should probably emit 'update' clause as 'insert' (overwriting) or similar?
        # Actually, looking at `update()` implementation above: it appends "update ...".
        # If we have multiple clauses, TypeDB parser generally accepts: match ... delete ... insert ... update ...
        # But 'update' keyword behavior: "The `update` clause is a convenience clause that allows...".
        # It updates 1-1 attributes.
        # Safe bet: If we use `update`, include it.
        if update_parts:
            query_sections.append("update")
            query_parts_str = "\n".join(update_parts)
            # update clause expects "$e has ...;" lines.
            query_sections.append(query_parts_str)

        full_query = "\n".join(query_sections)

        if not full_query:
            return entities

        logger.debug(f"Update many query length: {len(full_query)}")

        self._execute(full_query, TransactionType.WRITE)

        logger.info(f"Updated {len(entities)} entities: {self.model_class.__name__}")
        return entities

    def get(self, **filters) -> list[E]:
        """Get entities matching filters.

        Returns entities with their actual concrete type, enabling polymorphic
        queries. When querying a supertype, entities are instantiated as their
        actual subtype class if the subclass is defined in Python.

        Args:
            filters: Attribute filters

        Returns:
            List of matching entities with _iid populated and correct concrete type
        """
        logger.debug(f"Get entities: {self.model_class.__name__}, filters={filters}")
        query = QueryBuilder.match_entity(self.model_class, **filters)
        query.fetch("$e")  # Fetch all attributes with $e.*
        query_str = query.build()
        logger.debug(f"Get query: {query_str}")

        results = self._execute(query_str, TransactionType.READ)
        logger.debug(f"Query returned {len(results)} results")

        if not results:
            return []

        # Get IIDs and types for polymorphic instantiation
        iid_type_map = self._get_iids_and_types(**filters)

        # Convert results to entity instances with correct concrete type
        entities = []
        for result in results:
            # First, resolve the entity class using key attributes from base class
            base_attrs = self._extract_attributes(result)
            entity_class, iid = self._match_entity_type(base_attrs, iid_type_map)

            # Then extract attributes using the resolved class (includes subtype attributes)
            attrs = self._extract_attributes(result, entity_class)

            # Create entity with the resolved class
            entity = entity_class(**attrs)
            if iid:
                object.__setattr__(entity, "_iid", iid)
            entities.append(entity)

        logger.info(f"Retrieved {len(entities)} entities: {self.model_class.__name__}")
        return entities

    def get_by_iid(self, iid: str) -> E | None:
        """Get a single entity by its TypeDB Internal ID (IID).

        Returns the entity with its actual concrete type, enabling polymorphic
        queries. When querying a supertype by IID, the entity is instantiated
        as its actual subtype class if the subclass is defined in Python.

        Args:
            iid: TypeDB IID hex string (e.g., '0x1e00000000000000000000')

        Returns:
            Entity instance with _iid populated and correct concrete type, or None

        Example:
            entity = manager.get_by_iid("0x1e00000000000000000000")
            if entity:
                print(f"Found: {entity.__class__.__name__}")  # Actual subtype
        """
        logger.debug(f"Get entity by IID: {self.model_class.__name__}, iid={iid}")

        # Validate IID format
        if not iid or not iid.startswith("0x"):
            raise ValueError(f"Invalid IID format: {iid}. Expected hex string like '0x1e00...'")

        # Two queries: one for type (using label() on type variable), one for attributes
        # TypeQL's label() works on TYPE variables, so we bind the exact type with isa!
        # TypeQL doesn't allow mixing "key": value entries with $e.* in fetch

        # Query 1: Get type name using label($t) where $t is bound via isa!
        base_type = self.model_class.get_type_name()
        type_query = (
            f"match\n$e isa! $t, iid {iid}; $t sub {base_type};\n"
            f'fetch {{\n  "_type": label($t)\n}};'
        )
        logger.debug(f"Type lookup query: {type_query}")
        type_results = self._execute(type_query, TransactionType.READ)

        if not type_results:
            logger.debug(f"No entity found with IID {iid}")
            return None

        type_name = type_results[0].get("_type")

        # Query 2: Fetch all attributes
        fetch_query = f"match\n$e isa {base_type}, iid {iid};\nfetch {{ $e.* }};"
        logger.debug(f"Get by IID attributes query: {fetch_query}")
        results = self._execute(fetch_query, TransactionType.READ)

        if not results:
            logger.debug(f"No entity found with IID {iid}")
            return None

        result = results[0]

        # Resolve the correct class
        entity_class: type[E] = (
            cast(type[E], resolve_entity_class(self.model_class, type_name))
            if type_name
            else self.model_class
        )

        # Extract attributes using the resolved class (includes subtype attributes)
        attrs = self._extract_attributes(result, entity_class)
        entity = entity_class(**attrs)

        # Set the IID - always use the input parameter since fetch { $e.* } doesn't return IID
        object.__setattr__(entity, "_iid", iid)

        logger.info(f"Retrieved entity by IID: {entity_class.__name__}")
        return entity

    def filter(self, *expressions: Any, **filters: Any) -> "EntityQuery[E]":
        """Create a query for filtering entities.

        Supports both expression-based and dictionary-based filtering.

        Args:
            *expressions: Expression objects (Age.gt(Age(30)), etc.)
            **filters: Attribute filters (exact match) - age=30, name="Alice"

        Returns:
            EntityQuery for chaining

        Examples:
            # Expression-based (advanced filtering)
            manager.filter(Age.gt(Age(30)))
            manager.filter(Age.gt(Age(18)), Age.lt(Age(65)))

            # Dictionary-based (exact match - legacy)
            manager.filter(age=30, name="Alice")

            # Mixed
            manager.filter(Age.gt(Age(30)), status="active")

        Raises:
            ValueError: If expression references attribute type not owned by entity
        """
        logger.debug(
            f"Creating filter query: {self.model_class.__name__}, "
            f"expressions={len(expressions)}, filters={filters}"
        )
        # Import here to avoid circular dependency
        from .query import EntityQuery

        base_filters: dict[str, Any] = {}
        lookup_expressions: list[Any] = []

        if filters:
            base_filters, lookup_expressions = self._parse_lookup_filters(filters)

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

        query = EntityQuery(
            self._connection,
            self.model_class,
            base_filters if base_filters else None,
        )
        if expressions:
            query._expressions.extend(expressions)
        if lookup_expressions:
            query._expressions.extend(lookup_expressions)
        return query

    def group_by(self, *fields: Any) -> "GroupByQuery[E]":
        """Create a group-by query for aggregating by field values.

        Args:
            *fields: Field references to group by (Person.city, Person.department, etc.)

        Returns:
            GroupByQuery for aggregation

        Example:
            # Group by single field
            result = manager.group_by(Person.city).aggregate(Person.age.avg())

            # Group by multiple fields
            result = manager.group_by(Person.city, Person.department).aggregate(
                Person.salary.avg()
            )
        """
        # Import here to avoid circular dependency
        from .group_by import GroupByQuery

        return GroupByQuery(self._connection, self.model_class, {}, [], fields)

    def _parse_lookup_filters(self, filters: dict[str, Any]) -> tuple[dict[str, Any], list[Any]]:
        """Parse Django-style lookup filters into base filters and expressions."""
        from type_bridge.expressions.iid import IidExpr

        owned_attrs = self.model_class.get_all_attributes()
        base_filters: dict[str, Any] = {}
        expressions: list[Any] = []

        for raw_key, raw_value in filters.items():
            # Handle special iid__in lookup (IID is not an attribute)
            if raw_key == "iid__in":
                if not isinstance(raw_value, (list, tuple, set)):
                    raise ValueError("iid__in lookup requires an iterable of IID strings")
                iids = list(raw_value)
                if not iids:
                    raise ValueError("iid__in lookup requires a non-empty iterable")
                iid_exprs: list[Expression] = [IidExpr(iid) for iid in iids]
                if len(iid_exprs) == 1:
                    expressions.append(iid_exprs[0])
                else:
                    expressions.append(BooleanExpr("or", iid_exprs))
                continue

            if "__" not in raw_key:
                if raw_key not in owned_attrs:
                    raise ValueError(
                        f"Unknown filter field '{raw_key}' for {self.model_class.__name__}"
                    )
                if "__" in raw_key:
                    raise ValueError(
                        "Attribute names cannot contain '__' when using lookup filters"
                    )
                base_filters[raw_key] = raw_value
                continue

            field_name, lookup = raw_key.split("__", 1)
            if field_name not in owned_attrs:
                raise ValueError(
                    f"Unknown filter field '{field_name}' for {self.model_class.__name__}"
                )
            if "__" in field_name:
                raise ValueError("Attribute names cannot contain '__' when using lookup filters")

            attr_info = owned_attrs[field_name]
            attr_type = attr_info.typ

            # Normalize raw_value into Attribute instance for comparison/string ops
            def _wrap(value: Any):
                if isinstance(value, attr_type):
                    return value
                return attr_type(value)

            if lookup in ("exact", "eq"):
                base_filters[field_name] = raw_value
                continue

            if lookup in ("gt", "gte", "lt", "lte"):
                if not hasattr(attr_type, lookup):
                    raise ValueError(f"Lookup '{lookup}' not supported for {attr_type.__name__}")
                wrapped = _wrap(raw_value)
                expressions.append(getattr(attr_type, lookup)(wrapped))
                continue

            if lookup == "in":
                if not isinstance(raw_value, (list, tuple, set)):
                    raise ValueError("__in lookup requires an iterable of values")
                values = list(raw_value)
                if not values:
                    raise ValueError("__in lookup requires a non-empty iterable")
                eq_exprs: list[Expression] = [attr_type.eq(_wrap(v)) for v in values]
                # Create flat OR disjunction (avoids nested binary tree that causes
                # TypeDB query planner stack overflow with many values)
                if len(eq_exprs) == 1:
                    expressions.append(eq_exprs[0])
                else:
                    expressions.append(BooleanExpr("or", eq_exprs))
                continue

            if lookup == "isnull":
                if not isinstance(raw_value, bool):
                    raise ValueError("__isnull lookup expects a boolean")
                expressions.append(AttributeExistsExpr(attr_type, present=not raw_value))
                continue

            if lookup in ("contains", "startswith", "endswith", "regex"):
                if not issubclass(attr_type, String):
                    raise ValueError(
                        f"String lookup '{lookup}' requires a String attribute (got {attr_type.__name__})"
                    )
                # Normalize to raw string
                raw_str = str(unwrap_attribute(raw_value))

                if lookup == "contains":
                    expressions.append(attr_type.contains(attr_type(raw_str)))
                elif lookup == "regex":
                    expressions.append(attr_type.regex(attr_type(raw_str)))
                elif lookup == "startswith":
                    pattern = f"^{re.escape(raw_str)}.*"
                    expressions.append(attr_type.regex(attr_type(pattern)))
                elif lookup == "endswith":
                    pattern = f".*{re.escape(raw_str)}$"
                    expressions.append(attr_type.regex(attr_type(pattern)))
                continue

            raise ValueError(f"Unsupported lookup operator '{lookup}'")

        return base_filters, expressions

    def delete_many(self, entities: list[E], *, strict: bool = False) -> list[E]:
        """Delete multiple entities within a single transaction.

        Uses an existing transaction when supplied, otherwise opens one write
        transaction and reuses it for all deletes.

        Optimized to use batched TypeQL queries for entities with defined @key attributes.
        Uses Disjunctive Batching (OR-pattern) so that missing entities are ignored
        by default (idempotent behavior).

        Entities without @key attributes fall back to individual deletion to ensure
        uniqueness safety checks.

        Args:
            entities: Entity instances to delete
            strict: If True, raises EntityNotFoundError when any entity doesn't exist.
                   If False (default), silently ignores missing entities (idempotent).

        Returns:
            List of entities that were actually deleted (subset of input if some
            entities didn't exist in the database)

        Raises:
            EntityNotFoundError: If strict=True and any entity doesn't exist
        """
        if not entities:
            logger.debug("delete_many called with empty list")
            return []

        logger.debug(f"Deleting {len(entities)} entities: {self.model_class.__name__}")

        # Get key attributes for existence checking
        owned_attrs = self.model_class.get_all_attributes()
        key_attrs = {
            field_name: attr_info
            for field_name, attr_info in owned_attrs.items()
            if attr_info.flags.is_key
        }

        # Separate keyed and non-keyed entities
        keyed_entities: list[E] = []
        unbatchable_entities: list[E] = []

        for entity in entities:
            if key_attrs:
                keyed_entities.append(entity)
            else:
                unbatchable_entities.append(entity)

        # Track which entities actually exist (for return value and strict mode)
        existing_entities: list[E] = []
        missing_entities: list[E] = []

        # Check existence for keyed entities
        if keyed_entities and key_attrs:
            existing_keys = self._get_existing_entity_keys(keyed_entities, key_attrs)

            for entity in keyed_entities:
                entity_key = self._build_entity_key(entity, key_attrs)
                if entity_key in existing_keys:
                    existing_entities.append(entity)
                else:
                    missing_entities.append(entity)

        # For unbatchable entities, we'll check during serial deletion
        # (they use delete() which raises EntityNotFoundError)

        # Strict mode: raise if any entities don't exist
        if strict and missing_entities:
            missing_keys = [self._build_entity_key(e, key_attrs) for e in missing_entities]
            raise EntityNotFoundError(
                f"Cannot delete: {len(missing_entities)} entity(ies) not found "
                f"with given key attributes. Missing keys: {missing_keys}"
            )

        # Build batch delete for existing keyed entities
        match_blocks = []
        var_name = "$e"

        for entity in existing_entities:
            part = self._build_delete_query_part(entity, var_name)
            if part:
                m_part, _ = part
                match_blocks.append(m_part)

        # Execute batch if we have blocks
        if match_blocks:
            or_clauses = [f"{{ {block} }}" for block in match_blocks]
            match_section = " or ".join(or_clauses)

            query = f"match\n{match_section};\ndelete\n{var_name};"

            logger.debug(f"Delete many batched query length: {len(query)}")
            self._execute(query, TransactionType.WRITE)

        # Handle unbatchable entities serially
        deleted_unbatchable: list[E] = []
        if unbatchable_entities:
            logger.debug(f"Deleting {len(unbatchable_entities)} unbatchable entities serially")
            if self._executor.has_transaction:
                for entity in unbatchable_entities:
                    try:
                        self.delete(entity)
                        deleted_unbatchable.append(entity)
                    except EntityNotFoundError:
                        if strict:
                            raise
                        # Idempotent: skip missing entities
            else:
                assert self._executor.database is not None
                with self._executor.database.transaction(TransactionType.WRITE) as tx_ctx:
                    temp_manager = EntityManager(tx_ctx, self.model_class)
                    for entity in unbatchable_entities:
                        try:
                            temp_manager.delete(entity)
                            deleted_unbatchable.append(entity)
                        except EntityNotFoundError:
                            if strict:
                                raise
                            # Idempotent: skip missing entities

        # Combine results: existing keyed entities + successfully deleted unbatchable
        deleted = existing_entities + deleted_unbatchable
        logger.info(f"Deleted {len(deleted)} entities: {self.model_class.__name__}")
        return deleted

    def _build_delete_query_part(self, entity: E, var_name: str) -> tuple[str, str] | None:
        """Build the TypeQL query parts for deleting an entity.

        Only builds query for entities with defined @key attributes.

        Returns:
            Tuple of (match_clause, delete_clause) or None if no keys defined.
        """
        owned_attrs = self.model_class.get_all_attributes()

        # Extract key attributes from entity for matching
        match_filters = {}
        has_keys = False

        for field_name, attr_info in owned_attrs.items():
            if attr_info.flags.is_key:
                has_keys = True
                key_value = getattr(entity, field_name, None)
                if key_value is None:
                    # Key attribute exists on model but value is None on entity
                    raise KeyAttributeError(
                        entity_type=self.model_class.__name__,
                        operation="delete",
                        field_name=field_name,
                    )
                # Extract value from Attribute instance if needed
                key_value = unwrap_attribute(key_value)
                attr_name = attr_info.typ.get_attribute_name()
                match_filters[attr_name] = key_value

        if not has_keys:
            return None

        # Build match clause
        parts = [f"{var_name} isa {self.model_class.get_type_name()}"]
        for attr_name, attr_value in match_filters.items():
            parts.append(f"has {attr_name} {format_value(attr_value)}")

        match_clause = ", ".join(parts) + ";"

        # Build delete clause (deletes the entity and all attributes)
        delete_clause = f"{var_name};"

        return match_clause, delete_clause

    def _build_entity_key(
        self, entity: E, key_attrs: dict[str, Any]
    ) -> tuple[tuple[str, Any], ...]:
        """Build a hashable key tuple from entity's key attributes.

        Args:
            entity: Entity instance
            key_attrs: Dictionary of field_name -> attr_info for key attributes

        Returns:
            Sorted tuple of (attr_name, value) pairs
        """
        key_values: list[tuple[str, Any]] = []
        for field_name, attr_info in key_attrs.items():
            value = getattr(entity, field_name, None)
            if value is not None:
                value = unwrap_attribute(value)
                attr_name = attr_info.typ.get_attribute_name()
                key_values.append((attr_name, value))
        return tuple(sorted(key_values))

    def _get_existing_entity_keys(
        self, entities: list[E], key_attrs: dict[str, Any]
    ) -> set[tuple[tuple[str, Any], ...]]:
        """Query database to find which entities exist.

        Builds a disjunctive query to check existence of all entities at once.

        Args:
            entities: List of entities to check
            key_attrs: Dictionary of field_name -> attr_info for key attributes

        Returns:
            Set of key tuples for entities that exist in the database
        """
        if not entities or not key_attrs:
            return set()

        # Build disjunctive match query to find existing entities
        var_name = "$e"
        or_clauses = []

        for entity in entities:
            # Build match clause for this entity's key attributes
            parts = [f"{var_name} isa {self.model_class.get_type_name()}"]
            for field_name, attr_info in key_attrs.items():
                value = getattr(entity, field_name, None)
                if value is not None:
                    value = unwrap_attribute(value)
                    attr_name = attr_info.typ.get_attribute_name()
                    parts.append(f"has {attr_name} {format_value(value)}")

            or_clauses.append(f"{{ {', '.join(parts)}; }}")

        # Construct query: match { P1 } or { P2 } ...; fetch key attrs
        match_section = " or ".join(or_clauses)

        # Build fetch clause for key attributes
        key_attr_names = [attr_info.typ.get_attribute_name() for attr_info in key_attrs.values()]
        fetch_attrs = ", ".join([f'"{name}": {var_name}.{name}' for name in key_attr_names])
        query = f"match\n{match_section};\nfetch {{\n  {fetch_attrs}\n}};"

        logger.debug(f"Existence check query: {query[:200]}...")
        results = self._execute(query, TransactionType.READ)

        # Build set of existing keys
        existing_keys: set[tuple[tuple[str, Any], ...]] = set()
        for result in results:
            key_values: list[tuple[str, Any]] = []
            for attr_name in key_attr_names:
                if attr_name in result:
                    key_values.append((attr_name, result[attr_name]))
            if key_values:
                existing_keys.add(tuple(sorted(key_values)))

        logger.debug(f"Found {len(existing_keys)} existing entities out of {len(entities)}")
        return existing_keys

    def _build_update_query_parts(
        self, entity: E, var_name: str = "$e"
    ) -> tuple[str, str, str, str]:
        """Build the TypeQL query parts for updating an entity.

        Delegates to shared utility function that handles IID-based and key-based matching.

        Returns:
            Tuple of (match_clause, delete_clause, insert_clause, update_clause)
        """
        return build_entity_update_query_parts(entity, self.model_class, var_name)

    def _extract_attributes(
        self, result: dict[str, Any], entity_class: type[E] | None = None
    ) -> dict[str, Any]:
        """Extract attributes from query result.

        Args:
            result: Query result dictionary
            entity_class: Optional entity class to use for attribute extraction.
                          If None, uses self.model_class. For polymorphic queries,
                          pass the resolved subclass to get all its attributes.

        Returns:
            Dictionary of attributes
        """
        # Use provided class or default to model_class
        target_class = entity_class if entity_class is not None else self.model_class
        # Use shared hydration utility (ignore key values for entity extraction)
        attrs, _ = hydrate_attributes(target_class, result)
        return attrs

    def _get_iids_and_types(
        self, **filters: Any
    ) -> dict[tuple[tuple[str, Any], ...], tuple[str, str]]:
        """Get IIDs and type names for entities matching filters.

        Uses a single fetch query with iid() and label() functions to get
        entity IIDs and types. Key attributes are also fetched to build
        the lookup map.

        Args:
            **filters: Attribute filters (same as get())

        Returns:
            Dictionary mapping key_values_tuple to (iid, type_name) tuple
        """
        # Get key attributes using shared utility
        key_attrs, key_attr_names = get_key_attrs(self.model_class)

        # Build match query with filters
        query = QueryBuilder.match_entity(self.model_class, **filters)
        match_str = query.build().rstrip().rstrip(";")

        # Modify match to bind exact type for label() retrieval
        type_name = self.model_class.get_type_name()
        match_str = modify_match_for_type_binding(match_str, type_name)

        # Track key values we already know from filters
        known_key_values = build_known_key_values(key_attrs, filters)

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
    ) -> tuple[type[E], str | None]:
        """Match entity attributes to IID/type and resolve the correct class.

        Uses key attributes to look up the corresponding IID/type from the map
        (in-memory, no database query), then resolves the actual Python class
        for polymorphic instantiation.

        Args:
            attrs: Extracted attributes for the entity
            iid_type_map: Map from key_values_tuple to (iid, type_name)

        Returns:
            Tuple of (resolved_class, iid) where resolved_class is the
            concrete subclass if found, otherwise self.model_class
        """
        resolved_class, iid = match_entity_type(attrs, iid_type_map, self.model_class)
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
        logger.debug(f"Batched IID lookup query: {query_str}")

        results = self._execute(query_str, TransactionType.READ)
        if not results:
            return

        # Build IID map and assign to entities
        iid_map = build_entity_iid_map(results, key_attr_names)
        assign_entity_iids(entities, iid_map, key_attrs)
