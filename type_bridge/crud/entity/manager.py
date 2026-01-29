"""Entity CRUD operations manager."""

import logging
import re
from typing import TYPE_CHECKING, Any, cast

from typedb.driver import TransactionType

from type_bridge.attribute.string import String
from type_bridge.expressions import AttributeExistsExpr, BooleanExpr, Expression
from type_bridge.models import Entity
from type_bridge.query import QueryBuilder
from type_bridge.session import Connection, ConnectionExecutor

from ..base import E
from ..exceptions import EntityNotFoundError, KeyAttributeError, NotUniqueError
from ..utils import (
    format_value,
    is_multi_value_attribute,
    resolve_entity_class,
)

if TYPE_CHECKING:
    from .group_by import GroupByQuery
    from .query import EntityQuery

logger = logging.getLogger(__name__)


class EntityManager[E: Entity]:
    """Manager for entity CRUD operations.

    Type-safe manager that preserves entity type information.
    """

    def __init__(
        self,
        connection: Connection,
        model_class: type[E],
    ):
        """Initialize entity manager.

        Args:
            connection: Database, Transaction, or TransactionContext
            model_class: Entity model class
        """
        self._connection = connection
        self._executor = ConnectionExecutor(connection)
        self.model_class = model_class

    def insert(self, entity: E) -> E:
        """Insert an entity instance into the database.

        Args:
            entity: Entity instance to insert

        Returns:
            The inserted entity instance

        Example:
            # Create typed entity instance with wrapped attributes
            person = Person(
                name=Name("Alice"),
                age=Age(30),
                email=Email("alice@example.com")
            )
            Person.manager(db).insert(person)
        """
        logger.debug(f"Inserting entity: {self.model_class.__name__}")
        query = QueryBuilder.insert_entity(entity)
        query_str = query.build()
        logger.debug(f"Insert query: {query_str}")

        self._execute(query_str, TransactionType.WRITE)

        logger.info(f"Entity inserted: {self.model_class.__name__}")
        return entity

    def put(self, entity: E) -> E:
        """Put an entity instance into the database (insert if not exists).

        Uses TypeQL's PUT clause to ensure idempotent insertion. If the entity
        already exists (matching all attributes), no changes are made. If it doesn't
        exist, it's inserted.

        Args:
            entity: Entity instance to put

        Returns:
            The entity instance

        Example:
            # Create typed entity instance
            person = Person(
                name=Name("Alice"),
                age=Age(30),
                email=Email("alice@example.com")
            )
            # First call inserts, subsequent calls are idempotent
            Person.manager(db).put(person)
            Person.manager(db).put(person)  # No duplicate created
        """
        # Build PUT query similar to insert, but use "put" instead of "insert"
        logger.debug(f"Put entity: {self.model_class.__name__}")
        pattern = entity.to_insert_query("$e")
        query = f"put\n{pattern};"
        logger.debug(f"Put query: {query}")

        self._execute(query, TransactionType.WRITE)

        logger.info(f"Entity put: {self.model_class.__name__}")
        return entity

    def put_many(self, entities: list[E]) -> list[E]:
        """Put multiple entities into the database (insert if not exists).

        Uses TypeQL's PUT clause with all-or-nothing semantics:
        - If ALL entities match existing data, nothing is inserted
        - If ANY entity doesn't match, ALL entities in the pattern are inserted

        This means if one entity already exists, attempting to put it with new entities
        may cause a key constraint violation.

        Args:
            entities: List of entity instances to put

        Returns:
            List of entity instances

        Example:
            persons = [
                Person(name="Alice", email="alice@example.com"),
                Person(name="Bob", email="bob@example.com"),
            ]
            # First call inserts all, subsequent identical calls are idempotent
            Person.manager(db).put_many(persons)
        """
        if not entities:
            logger.debug("put_many called with empty list")
            return []

        logger.debug(f"Put {len(entities)} entities: {self.model_class.__name__}")
        # Build a single TypeQL PUT query with multiple patterns
        put_patterns = []
        for i, entity in enumerate(entities):
            # Use unique variable names for each entity
            var = f"$e{i}"
            pattern = entity.to_insert_query(var)
            put_patterns.append(pattern)

        # Combine all patterns into a single put query
        query = "put\n" + ";\n".join(put_patterns) + ";"
        logger.debug(f"Put many query: {query}")

        self._execute(query, TransactionType.WRITE)

        logger.info(f"Put {len(entities)} entities: {self.model_class.__name__}")
        return entities

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

    def insert_many(self, entities: list[E]) -> list[E]:
        """Insert multiple entities into the database in a single transaction.

        More efficient than calling insert() multiple times.

        Args:
            entities: List of entity instances to insert

        Returns:
            List of inserted entity instances

        Example:
            persons = [
                Person(name="Alice", email="alice@example.com"),
                Person(name="Bob", email="bob@example.com"),
                Person(name="Charlie", email="charlie@example.com"),
            ]
            Person.manager(db).insert_many(persons)
        """
        if not entities:
            logger.debug("insert_many called with empty list")
            return []

        logger.debug(f"Inserting {len(entities)} entities: {self.model_class.__name__}")
        # Build a single TypeQL query with multiple insert patterns
        insert_patterns = []
        for i, entity in enumerate(entities):
            # Use unique variable names for each entity
            var = f"$e{i}"
            pattern = entity.to_insert_query(var)
            insert_patterns.append(pattern)

        # Combine all patterns into a single insert query
        query = "insert\n" + ";\n".join(insert_patterns) + ";"
        logger.debug(f"Insert many query: {query}")

        self._execute(query, TransactionType.WRITE)

        logger.info(f"Inserted {len(entities)} entities: {self.model_class.__name__}")
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

        # Set the IID (from iid($e) in fetch or from input parameter)
        object.__setattr__(entity, "_iid", result.get("_iid", iid))

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
                raw_str = raw_value.value if hasattr(raw_value, "value") else str(raw_value)

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

    def all(self) -> list[E]:
        """Get all entities of this type.

        Returns:
            List of all entities
        """
        logger.debug(f"Getting all entities: {self.model_class.__name__}")
        return self.get()

    def delete(self, entity: E) -> E:
        """Delete an entity instance from the database.

        Uses @key attributes to identify the entity (same as update).
        If no @key attributes exist, matches by ALL attributes and only
        deletes if exactly 1 match is found.

        Args:
            entity: Entity instance to delete (must have key attributes set,
                    or match exactly one record if no keys)

        Returns:
            The deleted entity instance

        Raises:
            ValueError: If key attribute value is None
            EntityNotFoundError: If entity does not exist in database
            NotUniqueError: If no @key and multiple matches found

        Example:
            alice = Person(name=Name("Alice"), age=Age(30))
            person_manager.insert(alice)

            # Delete using the instance
            deleted = person_manager.delete(alice)
        """
        logger.debug(f"Deleting entity: {self.model_class.__name__}")
        owned_attrs = self.model_class.get_all_attributes()

        # Extract key attributes from entity for matching (same pattern as update)
        match_filters: dict[str, Any] = {}
        for field_name, attr_info in owned_attrs.items():
            if attr_info.flags.is_key:
                key_value = getattr(entity, field_name, None)
                if key_value is None:
                    raise KeyAttributeError(
                        entity_type=self.model_class.__name__,
                        operation="delete",
                        field_name=field_name,
                    )
                # Extract value from Attribute instance if needed
                if hasattr(key_value, "value"):
                    key_value = key_value.value
                attr_name = attr_info.typ.get_attribute_name()
                match_filters[attr_name] = key_value

        # Fallback: no @key attributes - match by ALL attributes
        if not match_filters:
            all_filters: dict[str, Any] = {}
            filter_kwargs: dict[str, Any] = {}
            for field_name, attr_info in owned_attrs.items():
                value = getattr(entity, field_name, None)
                if value is not None:
                    # Store field_name -> attribute value for filter()
                    filter_kwargs[field_name] = value
                    # Store attr_name -> raw value for TypeQL query
                    if hasattr(value, "value"):
                        value = value.value
                    attr_name = attr_info.typ.get_attribute_name()
                    all_filters[attr_name] = value

            # Count matches first - only delete if exactly 1
            # Use existing filter().count() mechanism
            count = self.filter(**filter_kwargs).count()

            if count == 0:
                raise EntityNotFoundError(
                    f"Cannot delete: entity '{self.model_class.get_type_name()}' "
                    "not found with given attributes."
                )
            if count > 1:
                raise NotUniqueError(
                    f"Cannot delete: found {count} matches. "
                    "Entity without @key must match exactly 1 record. "
                    "Use filter().delete() for bulk deletion."
                )
            match_filters = all_filters
        else:
            # For keyed entities, check existence before delete
            filter_kwargs: dict[str, Any] = {}
            for field_name, attr_info in owned_attrs.items():
                if attr_info.flags.is_key:
                    value = getattr(entity, field_name, None)
                    if value is not None:
                        filter_kwargs[field_name] = value

            count = self.filter(**filter_kwargs).count()
            if count == 0:
                raise EntityNotFoundError(
                    f"Cannot delete: entity '{self.model_class.get_type_name()}' "
                    "not found with given key attributes."
                )

        # Build TypeQL: match $e isa type, has key value; delete $e;
        parts = [f"$e isa {self.model_class.get_type_name()}"]
        for attr_name, attr_value in match_filters.items():
            parts.append(f"has {attr_name} {format_value(attr_value)}")

        query_str = f"match\n{', '.join(parts)};\ndelete\n$e;"
        logger.debug(f"Delete query: {query_str}")
        self._execute(query_str, TransactionType.WRITE)

        logger.info(f"Entity deleted: {self.model_class.__name__}")
        return entity

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
                if hasattr(key_value, "value"):
                    key_value = key_value.value
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
                if hasattr(value, "value"):
                    value = value.value
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
                    if hasattr(value, "value"):
                        value = value.value
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

    def update(self, entity: E) -> E:
        """Update an entity in the database based on its current state.

        Reads all attribute values from the entity instance and persists them to the database.
        Uses key attributes to identify the entity.

        For single-value attributes (@card(0..1) or @card(1..1)), uses TypeQL update clause.
        For multi-value attributes (e.g., @card(0..5), @card(2..)), deletes old values
        and inserts new ones.

        Args:
            entity: The entity instance to update (must have key attributes set)

        Returns:
            The same entity instance

        Example:
            # Fetch entity
            alice = person_manager.get(name="Alice")[0]

            # Modify attributes directly
            alice.age = 31
            alice.tags = ["python", "typedb", "ai"]

            # Update in database
            person_manager.update(alice)
        """
        logger.debug(f"Updating entity: {self.model_class.__name__}")

        match_clause, delete_clause, insert_clause, update_clause = self._build_update_query_parts(
            entity
        )

        # Combine and execute
        query_parts = []
        if match_clause:
            query_parts.append(f"match\n{match_clause}")
        if delete_clause:
            query_parts.append(f"delete\n{delete_clause}")
        if insert_clause:
            query_parts.append(f"insert\n{insert_clause}")
        if update_clause:
            query_parts.append(f"update\n{update_clause}")

        full_query = "\n".join(query_parts)
        logger.debug(f"Update query: {full_query}")

        self._execute(full_query, TransactionType.WRITE)

        logger.info(f"Entity updated: {self.model_class.__name__}")
        return entity

    def _build_update_query_parts(
        self, entity: E, var_name: str = "$e"
    ) -> tuple[str, str, str, str]:
        """Build the TypeQL query parts for updating an entity.

        Returns:
            Tuple of (match_clause, delete_clause, insert_clause, update_clause) structures
            containing the partial queries (without the keywords 'match', 'delete', etc.)
        """
        # Get all attributes (including inherited) to determine cardinality
        owned_attrs = self.model_class.get_all_attributes()

        # Extract key attributes from entity for matching
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
                # Extract value from Attribute instance if needed
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

        # Separate single-value and multi-value updates from entity state
        single_value_updates = {}
        single_value_deletes = set()  # Track single-value attributes to delete
        multi_value_updates = {}

        for field_name, attr_info in owned_attrs.items():
            # Skip key attributes (they're used for matching)
            if attr_info.flags.is_key:
                continue

            attr_class = attr_info.typ
            attr_name = attr_class.get_attribute_name()
            flags = attr_info.flags

            # Get current value from entity
            current_value = getattr(entity, field_name, None)

            # Extract raw values from Attribute instances
            if current_value is not None:
                if isinstance(current_value, list):
                    # Multi-value: extract value from each Attribute in list
                    raw_values = []
                    for item in current_value:
                        if hasattr(item, "value"):
                            raw_values.append(item.value)
                        else:
                            raw_values.append(item)
                    current_value = raw_values
                elif hasattr(current_value, "value"):
                    # Single-value: extract value from Attribute
                    current_value = current_value.value

            # Determine if multi-value
            is_multi_value = is_multi_value_attribute(flags)

            if is_multi_value:
                # Multi-value: store as list (even if empty)
                if current_value is None:
                    current_value = []
                multi_value_updates[attr_name] = current_value
            else:
                # Single-value: handle updates and deletions
                if current_value is not None:
                    single_value_updates[attr_name] = current_value
                else:
                    # Check if attribute is optional (card_min == 0)
                    if flags.card_min == 0:
                        # Optional attribute set to None - needs to be deleted
                        single_value_deletes.add(attr_name)

        # Build Match Clause
        match_statements = []
        entity_match_parts = [f"{var_name} isa {self.model_class.get_type_name()}"]
        for attr_name, attr_value in match_filters.items():
            formatted_value = format_value(attr_value)
            entity_match_parts.append(f"has {attr_name} {formatted_value}")
        match_statements.append(", ".join(entity_match_parts) + ";")

        # Add match statements to bind multi-value attributes for deletion with optional guards
        if multi_value_updates:
            for attr_name, values in multi_value_updates.items():
                keep_literals = [format_value(v) for v in dict.fromkeys(values)]
                # Use scoped variables for guards to avoid conflicts in batch queries
                # (Variables inside not {} or try {} are local scope in TypeQL generally,
                # but to be safe we use unique names if needed. Here we use literals so it's fine)
                guard_lines = [
                    f"not {{ ${attr_name} == {literal}; }};" for literal in keep_literals
                ]
                # Use variable name derived from entity var to be unique across batch
                attr_var = f"${attr_name}_{var_name.replace('$', '')}"

                try_block = "\n".join(
                    [
                        "try {",
                        f"  {var_name} has {attr_name} {attr_var};",
                        *[f"  {g.replace(f'${attr_name}', attr_var)}" for g in guard_lines],
                        "};",
                    ]
                )
                match_statements.append(try_block)

        # Add match statements to bind single-value attributes for deletion
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

        # Build Insert Clause (for multi-value attributes)
        insert_parts = []
        for attr_name, values in multi_value_updates.items():
            for value in values:
                formatted_value = format_value(value)
                insert_parts.append(f"{var_name} has {attr_name} {formatted_value};")

        insert_clause = "\n".join(insert_parts)

        # Build Update Clause (for single-value attributes)
        update_parts = []
        if single_value_updates:
            for attr_name, value in single_value_updates.items():
                formatted_value = format_value(value)
                update_parts.append(f"{var_name} has {attr_name} {formatted_value};")

        update_clause = "\n".join(update_parts)

        return match_clause, delete_clause, insert_clause, update_clause

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
        attrs = {}
        # Use provided class or default to model_class
        target_class = entity_class if entity_class is not None else self.model_class
        # Extract attributes from all attribute classes (including inherited)
        all_attrs = target_class.get_all_attributes()
        for field_name, attr_info in all_attrs.items():
            attr_class = attr_info.typ
            attr_name = attr_class.get_attribute_name()
            if attr_name in result:
                attrs[field_name] = result[attr_name]
            else:
                # For multi-value attributes, use empty list; for optional, use None
                is_multi_value = is_multi_value_attribute(attr_info.flags)
                attrs[field_name] = [] if is_multi_value else None
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
        # Get key attributes for building the lookup key
        owned_attrs = self.model_class.get_all_attributes()
        key_attrs = {
            field_name: attr_info
            for field_name, attr_info in owned_attrs.items()
            if attr_info.flags.is_key
        }

        # Build match query with filters, adding type variable for label()
        # TypeQL's label() function works on TYPE variables, not instance variables
        # So we use: $e isa! $t (exact type match) then label($t)
        query = QueryBuilder.match_entity(self.model_class, **filters)
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
            if field_name in filters:
                filter_value = filters[field_name]
                if hasattr(filter_value, "value"):
                    filter_value = filter_value.value
                known_key_values[attr_name] = filter_value

        # Build fetch items: iid, label (on type var), and key attributes
        # TypeQL doesn't allow mixing "key": value entries with $e.*, so we
        # explicitly list the items we need
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
        # If no type info available, use model_class
        if not iid_type_map:
            return self.model_class, None

        # Get key attributes for matching
        owned_attrs = self.model_class.get_all_attributes()
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
            # No key values found, use model_class
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
        logger.debug(f"Batched IID lookup query: {query_str}")

        results = self._execute(query_str, TransactionType.READ)

        if not results:
            return

        iid_map: dict[tuple[tuple[str, Any], ...], str] = {}

        # Extract IID and key values from single fetch result
        for result in results:
            iid = result.get("_iid")
            if not iid:
                continue

            # Extract key attribute values
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

    def _execute(self, query: str, tx_type: TransactionType) -> list[dict[str, Any]]:
        """Execute a query using existing transaction if provided."""
        return self._executor.execute(query, tx_type)
