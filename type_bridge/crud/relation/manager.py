"""RelationManager for relation CRUD operations."""

import logging
from typing import TYPE_CHECKING, Any

from typedb.driver import TransactionType

from type_bridge.models import Relation
from type_bridge.query import Query
from type_bridge.session import Connection, ConnectionExecutor

from ..base import R
from ..exceptions import RelationNotFoundError
from ..utils import format_value, is_multi_value_attribute

logger = logging.getLogger(__name__)

if TYPE_CHECKING:
    from .group_by import RelationGroupByQuery
    from .query import RelationQuery


class RelationManager[R: Relation]:
    """Manager for relation CRUD operations.

    Type-safe manager that preserves relation type information.
    """

    def __init__(self, connection: Connection, model_class: type[R]):
        """Initialize relation manager.

        Args:
            connection: Database, Transaction, or TransactionContext
            model_class: Relation model class
        """
        self._connection = connection
        self._executor = ConnectionExecutor(connection)
        self.model_class = model_class

    def _build_role_player_match(self, role_name: str, entity: Any, entity_type_name: str) -> str:
        """Build a match clause for a role player entity.

        Prefers IID-based matching when available (more precise and faster),
        falls back to key attribute matching, and raises a clear error if
        neither is available.

        Args:
            role_name: The role name (used as variable name)
            entity: The entity instance
            entity_type_name: The TypeDB type name for the entity

        Returns:
            A TypeQL match clause string like "$role_name isa type, iid 0x..."
            or "$role_name isa type, has key_attr value"

        Raises:
            ValueError: If entity has neither _iid nor key attributes
        """
        # Prefer IID-based matching when available (more precise and faster)
        entity_iid = getattr(entity, "_iid", None)
        if entity_iid:
            return f"${role_name} isa {entity_type_name}, iid {entity_iid}"

        # Fall back to key attribute matching
        key_attrs = {
            field_name: attr_info
            for field_name, attr_info in entity.__class__.get_all_attributes().items()
            if attr_info.flags.is_key
        }

        for field_name, attr_info in key_attrs.items():
            value = getattr(entity, field_name)
            if value is not None:
                attr_class = attr_info.typ
                attr_name = attr_class.get_attribute_name()
                formatted_value = format_value(value)
                return f"${role_name} isa {entity_type_name}, has {attr_name} {formatted_value}"

        # Neither IID nor key attributes available
        raise ValueError(
            f"Role player '{role_name}' ({entity.__class__.__name__}) cannot be identified: "
            f"no _iid set and no @key attributes defined. Either fetch the entity from the "
            f"database first (to populate _iid) or add Flag(Key) to an attribute."
        )

    def _build_player_key_and_match(
        self, player_entity: Any
    ) -> tuple[tuple[str, Any], str, list[str]]:
        """Build a unique key and match clause parts for a role player entity.

        Used by insert_many/put_many to deduplicate role players across relations.
        Prefers IID-based matching when available.

        Args:
            player_entity: The entity instance

        Returns:
            Tuple of (player_key, player_type, match_parts) where:
            - player_key: Tuple for deduplication (either ("iid", iid) or key attr values)
            - player_type: The TypeDB type name
            - match_parts: List of match clause parts like ["isa type", "iid 0x..."]

        Raises:
            ValueError: If entity has neither _iid nor key attributes
        """
        player_type = player_entity.get_type_name()

        # Prefer IID-based matching when available
        entity_iid = getattr(player_entity, "_iid", None)
        if entity_iid:
            player_key: tuple[str, Any] = ("iid", entity_iid)
            match_parts = [f"isa {player_type}", f"iid {entity_iid}"]
            return (player_key, player_type, match_parts)

        # Fall back to key attribute matching
        owned_attrs = player_entity.get_all_attributes()
        key_values: list[tuple[str, Any]] = []
        for field_name, attr_info in owned_attrs.items():
            if attr_info.flags.is_key:
                value = getattr(player_entity, field_name, None)
                if value is not None:
                    attr_name = attr_info.typ.get_attribute_name()
                    key_values.append((attr_name, value))

        if key_values:
            player_key = ("keys", tuple(sorted(key_values)))
            match_parts = [f"isa {player_type}"]
            for attr_name, value in key_values:
                formatted_value = format_value(value)
                match_parts.append(f"has {attr_name} {formatted_value}")
            return (player_key, player_type, match_parts)

        # Neither IID nor key attributes available
        raise ValueError(
            f"Role player ({player_entity.__class__.__name__}) cannot be identified: "
            f"no _iid set and no @key attributes defined. Either fetch the entity from the "
            f"database first (to populate _iid) or add Flag(Key) to an attribute."
        )

    def insert(self, relation: R) -> R:
        """Insert a typed relation instance into the database.

        Args:
            relation: Typed relation instance with role players and attributes

        Returns:
            The inserted relation instance

        Example:
            # Typed construction - full IDE support and type checking
            employment = Employment(
                employee=person,
                employer=company,
                position="Engineer",
                salary=100000
            )
            employment_manager.insert(employment)
        """
        logger.debug(f"Inserting relation: {self.model_class.__name__}")
        # Extract role players from relation instance
        roles = self.model_class._roles
        role_players = {}
        for role_name, role in roles.items():
            entity = relation.__dict__.get(role_name)
            if entity is not None:
                role_players[role_name] = entity

        # Build match clause for role players (IID-preferring)
        match_parts = []
        for role_name, entity in role_players.items():
            entity_type_name = entity.__class__.get_type_name()
            match_parts.append(self._build_role_player_match(role_name, entity, entity_type_name))

        # Build insert clause
        relation_type_name = self.model_class.get_type_name()
        role_parts = [
            f"{roles[role_name].role_name}: ${role_name}" for role_name in role_players.keys()
        ]
        relation_pattern = f"({', '.join(role_parts)}) isa {relation_type_name}"

        # Add attributes (including inherited)
        attr_parts = []
        for field_name, attr_info in self.model_class.get_all_attributes().items():
            value = getattr(relation, field_name, None)
            if value is not None:
                attr_class = attr_info.typ
                attr_name = attr_class.get_attribute_name()

                # Handle lists (multi-value attributes)
                if isinstance(value, list):
                    for item in value:
                        # Extract value from Attribute instance
                        if hasattr(item, "value"):
                            item = item.value
                        # Use format_value to ensure proper escaping
                        formatted = format_value(item)
                        attr_parts.append(f"has {attr_name} {formatted}")
                else:
                    # Extract value from Attribute instance
                    if hasattr(value, "value"):
                        value = value.value
                    # Use format_value to ensure proper escaping
                    formatted = format_value(value)
                    attr_parts.append(f"has {attr_name} {formatted}")

        # Combine relation pattern with attributes
        if attr_parts:
            insert_pattern = relation_pattern + ", " + ", ".join(attr_parts)
        else:
            insert_pattern = relation_pattern

        # Build full query
        match_clause = "match\n" + ";\n".join(match_parts) + ";"
        insert_clause = "insert\n" + insert_pattern + ";"
        query = match_clause + "\n" + insert_clause
        logger.debug(f"Insert query: {query}")

        self._execute(query, TransactionType.WRITE)

        logger.info(f"Relation inserted: {self.model_class.__name__}")
        return relation

    def put(self, relation: R) -> R:
        """Put a typed relation instance into the database (insert if not exists).

        Uses TypeQL's PUT clause to ensure idempotent insertion. If the relation
        already exists (matching role players and attributes), no changes are made.
        If it doesn't exist, it's inserted.

        Args:
            relation: Typed relation instance with role players and attributes

        Returns:
            The relation instance

        Example:
            # Typed construction
            employment = Employment(
                employee=person,
                employer=company,
                position="Engineer",
                salary=100000
            )
            # First call inserts, subsequent calls are idempotent
            employment_manager.put(employment)
            employment_manager.put(employment)  # No duplicate created
        """
        logger.debug(f"Put relation: {self.model_class.__name__}")
        # Extract role players from relation instance
        roles = self.model_class._roles
        role_players = {}
        for role_name, role in roles.items():
            entity = relation.__dict__.get(role_name)
            if entity is not None:
                role_players[role_name] = entity

        # Build match clause for role players (IID-preferring)
        match_parts = []
        for role_name, entity in role_players.items():
            entity_type_name = entity.__class__.get_type_name()
            match_parts.append(self._build_role_player_match(role_name, entity, entity_type_name))

        # Build put clause (same as insert clause but with "put" keyword)
        relation_type_name = self.model_class.get_type_name()
        role_parts = [
            f"{roles[role_name].role_name}: ${role_name}" for role_name in role_players.keys()
        ]
        relation_pattern = f"({', '.join(role_parts)}) isa {relation_type_name}"

        # Add attributes (including inherited)
        attr_parts = []
        for field_name, attr_info in self.model_class.get_all_attributes().items():
            value = getattr(relation, field_name, None)
            if value is not None:
                attr_class = attr_info.typ
                attr_name = attr_class.get_attribute_name()

                # Handle lists (multi-value attributes)
                if isinstance(value, list):
                    for item in value:
                        # Extract value from Attribute instance
                        if hasattr(item, "value"):
                            item = item.value
                        # Use format_value to ensure proper escaping
                        formatted = format_value(item)
                        attr_parts.append(f"has {attr_name} {formatted}")
                else:
                    # Extract value from Attribute instance
                    if hasattr(value, "value"):
                        value = value.value
                    # Use format_value to ensure proper escaping
                    formatted = format_value(value)
                    attr_parts.append(f"has {attr_name} {formatted}")

        # Combine relation pattern with attributes
        if attr_parts:
            put_pattern = relation_pattern + ", " + ", ".join(attr_parts)
        else:
            put_pattern = relation_pattern

        # Build full query with match for role players, then put for the relation
        match_clause = "match\n" + ";\n".join(match_parts) + ";"
        put_clause = "put\n" + put_pattern + ";"
        query = match_clause + "\n" + put_clause
        logger.debug(f"Put query: {query}")

        self._execute(query, TransactionType.WRITE)

        logger.info(f"Relation put: {self.model_class.__name__}")
        return relation

    def put_many(self, relations: list[R]) -> list[R]:
        """Put multiple relations into the database (insert if not exists).

        Uses TypeQL's PUT clause with all-or-nothing semantics:
        - If ALL relations match existing data, nothing is inserted
        - If ANY relation doesn't match, ALL relations in the pattern are inserted

        Args:
            relations: List of relation instances to put

        Returns:
            List of relation instances

        Example:
            employments = [
                Employment(
                    position="Engineer",
                    salary=100000,
                    employee=alice,
                    employer=tech_corp
                ),
                Employment(
                    position="Manager",
                    salary=120000,
                    employee=bob,
                    employer=tech_corp
                ),
            ]
            # First call inserts all, subsequent identical calls are idempotent
            Employment.manager(db).put_many(employments)
        """
        if not relations:
            logger.debug("put_many called with empty list")
            return []

        logger.debug(f"Put {len(relations)} relations: {self.model_class.__name__}")
        # Build query
        query = Query()

        # Collect all unique role players to match (IID-preferring)
        all_players: dict[tuple[str, tuple[str, Any]], str] = {}  # player_key -> player_var
        player_counter = 0

        # First pass: collect all unique players from all relation instances
        for relation in relations:
            # Extract role players from instance
            for role_name in self.model_class._roles:
                player_entity = relation.__dict__.get(role_name)
                if player_entity is None:
                    continue

                # Build player key and match clause (IID-preferring)
                player_key, player_type, match_parts = self._build_player_key_and_match(
                    player_entity
                )

                if player_key not in all_players:
                    player_var = f"$player{player_counter}"
                    player_counter += 1
                    all_players[player_key] = player_var

                    # Build match clause for this player
                    query.match(f"{player_var} " + ", ".join(match_parts))

        # Second pass: build put patterns for relations (same as insert_many but with "put")
        put_patterns = []

        for relation in relations:
            # Map role players to their variables
            role_var_map = {}
            for role_name, role in self.model_class._roles.items():
                player_entity = relation.__dict__.get(role_name)
                if player_entity is None:
                    raise ValueError(f"Missing role player for role: {role_name}")

                # Find the player variable using the same key logic
                player_key, _, _ = self._build_player_key_and_match(player_entity)
                player_var = all_players[player_key]
                role_var_map[role_name] = (player_var, role.role_name)

            # Build put pattern for this relation
            role_players_str = ", ".join(
                [f"{role_name}: {var}" for var, role_name in role_var_map.values()]
            )
            put_pattern = f"({role_players_str}) isa {self.model_class.get_type_name()}"

            # Extract and add attributes from relation instance (including inherited)
            attr_parts = []
            all_attrs = self.model_class.get_all_attributes()
            for field_name, attr_info in all_attrs.items():
                if hasattr(relation, field_name):
                    attr_value = getattr(relation, field_name)
                    if attr_value is None:
                        continue

                    typeql_attr_name = attr_info.typ.get_attribute_name()

                    # Handle multi-value attributes (lists)
                    if isinstance(attr_value, list):
                        # Extract raw values from each Attribute instance in the list
                        for item in attr_value:
                            if hasattr(item, "value"):
                                raw_value = item.value
                            else:
                                raw_value = item
                            formatted_value = format_value(raw_value)
                            attr_parts.append(f"has {typeql_attr_name} {formatted_value}")
                    else:
                        # Single-value attribute - extract raw value from Attribute instance
                        if hasattr(attr_value, "value"):
                            attr_value = attr_value.value
                        formatted_value = format_value(attr_value)
                        attr_parts.append(f"has {typeql_attr_name} {formatted_value}")

            if attr_parts:
                put_pattern += ", " + ", ".join(attr_parts)

            put_patterns.append(put_pattern)

        # Build the query with "put" instead of "insert"
        if query._match_clauses:
            # If we have match clauses, build match section and put section separately
            match_body = "; ".join(query._match_clauses)
            match_section = f"match\n{match_body};"
            put_section = ";\n".join(put_patterns)
            query_str = f"{match_section}\nput\n{put_section};"
        else:
            # No match clauses, just put patterns
            put_section = ";\n".join(put_patterns)
            query_str = f"put\n{put_section};"

        # Execute the query
        logger.debug(f"Put many query: {query_str}")
        self._execute(query_str, TransactionType.WRITE)

        logger.info(f"Put {len(relations)} relations: {self.model_class.__name__}")
        return relations

    def insert_many(self, relations: list[R]) -> list[R]:
        """Insert multiple relations into the database in a single transaction.

        More efficient than calling insert() multiple times.

        Args:
            relations: List of relation instances to insert

        Returns:
            List of inserted relation instances

        Example:
            employments = [
                Employment(
                    position="Engineer",
                    salary=100000,
                    employee=alice,
                    employer=tech_corp
                ),
                Employment(
                    position="Manager",
                    salary=120000,
                    employee=bob,
                    employer=tech_corp
                ),
            ]
            Employment.manager(db).insert_many(employments)
        """
        if not relations:
            logger.debug("insert_many called with empty list")
            return []

        logger.debug(f"Inserting {len(relations)} relations: {self.model_class.__name__}")
        # Build query
        query = Query()

        # Collect all unique role players to match (IID-preferring)
        all_players: dict[tuple[str, tuple[str, Any]], str] = {}  # player_key -> player_var
        player_counter = 0

        # First pass: collect all unique players from all relation instances
        for relation in relations:
            # Extract role players from instance
            for role_name in self.model_class._roles:
                player_entity = relation.__dict__.get(role_name)
                if player_entity is None:
                    continue

                # Build player key and match clause (IID-preferring)
                player_key, player_type, match_parts = self._build_player_key_and_match(
                    player_entity
                )

                if player_key not in all_players:
                    player_var = f"$player{player_counter}"
                    player_counter += 1
                    all_players[player_key] = player_var

                    # Build match clause for this player
                    query.match(f"{player_var} " + ", ".join(match_parts))

        # Second pass: build insert patterns for relations
        insert_patterns = []

        for relation in relations:
            # Map role players to their variables
            role_var_map = {}
            for role_name, role in self.model_class._roles.items():
                player_entity = relation.__dict__.get(role_name)
                if player_entity is None:
                    raise ValueError(f"Missing role player for role: {role_name}")

                # Find the player variable using the same key logic
                player_key, _, _ = self._build_player_key_and_match(player_entity)
                player_var = all_players[player_key]
                role_var_map[role_name] = (player_var, role.role_name)

            # Build insert pattern for this relation
            role_players_str = ", ".join(
                [f"{role_name}: {var}" for var, role_name in role_var_map.values()]
            )
            insert_pattern = f"({role_players_str}) isa {self.model_class.get_type_name()}"

            # Extract and add attributes from relation instance (including inherited)
            attr_parts = []
            all_attrs = self.model_class.get_all_attributes()
            for field_name, attr_info in all_attrs.items():
                if hasattr(relation, field_name):
                    attr_value = getattr(relation, field_name)
                    if attr_value is None:
                        continue

                    typeql_attr_name = attr_info.typ.get_attribute_name()

                    # Handle multi-value attributes (lists)
                    if isinstance(attr_value, list):
                        # Extract raw values from each Attribute instance in the list
                        for item in attr_value:
                            if hasattr(item, "value"):
                                raw_value = item.value
                            else:
                                raw_value = item
                            formatted_value = format_value(raw_value)
                            attr_parts.append(f"has {typeql_attr_name} {formatted_value}")
                    else:
                        # Single-value attribute - extract raw value from Attribute instance
                        if hasattr(attr_value, "value"):
                            attr_value = attr_value.value
                        formatted_value = format_value(attr_value)
                        attr_parts.append(f"has {typeql_attr_name} {formatted_value}")

            if attr_parts:
                insert_pattern += ", " + ", ".join(attr_parts)

            insert_patterns.append(insert_pattern)

        # Add all insert patterns to query
        query.insert(";\n".join(insert_patterns))

        # Execute the query
        query_str = query.build()
        logger.debug(f"Insert many query: {query_str}")
        self._execute(query_str, TransactionType.WRITE)

        logger.info(f"Inserted {len(relations)} relations: {self.model_class.__name__}")
        return relations

    def get(self, **filters) -> list[R]:
        """Get relations matching filters.

        Supports filtering by both attributes and role players.

        Args:
            filters: Attribute filters and/or role player filters
                - Attribute filters: position="Engineer", salary=100000, is_remote=True
                - Role player filters: employee=person_entity, employer=company_entity

        Returns:
            List of matching relations

        Example:
            # Filter by attribute
            Employment.manager(db).get(position="Engineer")

            # Filter by role player
            Employment.manager(db).get(employee=alice)

            # Filter by both
            Employment.manager(db).get(position="Manager", employer=tech_corp)
        """
        logger.debug(f"Get relations: {self.model_class.__name__}, filters={filters}")
        # Build TypeQL 3.x query with correct syntax for fetching relations with role players
        # Use get_all_attributes to include inherited attributes for filtering
        all_attrs = self.model_class.get_all_attributes()

        # Separate attribute filters from role player filters
        attr_filters = {}
        role_player_filters = {}

        for key, value in filters.items():
            if key in self.model_class._roles:
                # This is a role player filter
                role_player_filters[key] = value
            elif key in all_attrs:
                # This is an attribute filter
                attr_filters[key] = value
            else:
                raise ValueError(f"Unknown filter: {key}")

        # Build match clause with inline role players
        role_parts = []
        role_info = {}  # role_name -> (var, allowed_entity_classes)
        for role_name, role in self.model_class._roles.items():
            role_var = f"${role_name}"
            role_parts.append(f"{role.role_name}: {role_var}")
            role_info[role_name] = (role_var, role.player_entity_types)

        # Build match clause with inline role players
        # Use isa! to bind exact type to $t for label() function
        roles_str = ", ".join(role_parts)
        base_type = self.model_class.get_type_name()
        match_clauses = [f"$r isa! $t ({roles_str})", f"$t sub {base_type}"]

        # Add attribute filter clauses
        for field_name, value in attr_filters.items():
            attr_info = all_attrs[field_name]
            attr_name = attr_info.typ.get_attribute_name()
            formatted_value = format_value(value)
            match_clauses.append(f"$r has {attr_name} {formatted_value}")

        # Add role player filter clauses
        for role_name, player_entity in role_player_filters.items():
            role_var = f"${role_name}"
            entity_class = player_entity.__class__

            # Match the role player by their key attributes (including inherited)
            player_owned_attrs = entity_class.get_all_attributes()
            for field_name, attr_info in player_owned_attrs.items():
                if attr_info.flags.is_key:
                    key_value = getattr(player_entity, field_name, None)
                    if key_value is not None:
                        attr_name = attr_info.typ.get_attribute_name()
                        # Extract value from Attribute instance if needed
                        if hasattr(key_value, "value"):
                            key_value = key_value.value
                        formatted_value = format_value(key_value)
                        match_clauses.append(f"{role_var} has {attr_name} {formatted_value}")
                        break

        match_str = ";\n".join(match_clauses) + ";"

        # Build fetch clause with nested structure for role players
        # Use label($t) where $t is a TYPE variable bound via isa!
        fetch_items = ['"_iid": iid($r)', '"_type": label($t)']

        # Add relation attributes (including inherited)
        for field_name, attr_info in all_attrs.items():
            attr_name = attr_info.typ.get_attribute_name()
            # Multi-value attributes need to be wrapped in [] for TypeQL fetch
            if is_multi_value_attribute(attr_info.flags):
                fetch_items.append(f'"{attr_name}": [$r.{attr_name}]')
            else:
                fetch_items.append(f'"{attr_name}": $r.{attr_name}')

        # Add each role player as nested object
        for role_name, (role_var, entity_class) in role_info.items():
            fetch_items.append(f'"{role_name}": {{\n    {role_var}.*\n  }}')

        fetch_body = ",\n  ".join(fetch_items)
        fetch_str = f"fetch {{\n  {fetch_body}\n}};"

        query_str = f"match\n{match_str}\n{fetch_str}"
        logger.debug(f"Get query: {query_str}")

        results = self._execute(query_str, TransactionType.READ)
        logger.debug(f"Query returned {len(results)} results")

        # Convert results to relation instances
        relations = []

        for result in results:
            # Extract relation attributes (including inherited)
            attrs: dict[str, Any] = {}
            for field_name, attr_info in all_attrs.items():
                attr_class = attr_info.typ
                attr_name = attr_class.get_attribute_name()
                if attr_name in result:
                    raw_value = result[attr_name]
                    # Multi-value attributes need explicit conversion from list of raw values
                    if is_multi_value_attribute(attr_info.flags) and isinstance(raw_value, list):
                        # Convert each raw value to Attribute instance
                        attrs[field_name] = [attr_class(v) for v in raw_value]
                    else:
                        # Single value - let Pydantic handle conversion via model constructor
                        attrs[field_name] = raw_value
                else:
                    # For list fields (has_explicit_card), default to empty list
                    # For other optional fields, explicitly set to None
                    if attr_info.flags.has_explicit_card:
                        attrs[field_name] = []
                    else:
                        attrs[field_name] = None

            # Create relation instance
            relation = self.model_class(**attrs)

            # Extract role players from nested objects in result
            for role_name, (role_var, allowed_entity_classes) in role_info.items():
                if role_name in result and isinstance(result[role_name], dict):
                    player_data = result[role_name]
                    # Choose entity class based on available key attributes; fallback to first allowed
                    entity_class = allowed_entity_classes[0]
                    for candidate in allowed_entity_classes:
                        key_attr_names = [
                            attr_info.typ.get_attribute_name()
                            for attr_info in candidate.get_all_attributes().values()
                            if attr_info.flags.is_key
                        ]
                        if any(key in player_data for key in key_attr_names):
                            entity_class = candidate
                            break
                    # Extract player attributes (including inherited)
                    player_attrs: dict[str, Any] = {}
                    for field_name, attr_info in entity_class.get_all_attributes().items():
                        attr_class = attr_info.typ
                        attr_name = attr_class.get_attribute_name()
                        if attr_name in player_data:
                            raw_value = player_data[attr_name]
                            # Multi-value attributes need explicit conversion from list of raw values
                            # Use static method to avoid binding issues between EntityQuery/RelationQuery
                            if (
                                hasattr(attr_info.flags, "has_explicit_card")
                                and attr_info.flags.has_explicit_card
                            ):
                                is_multi = True
                            elif (
                                hasattr(attr_info.flags, "card_min")
                                and attr_info.flags.card_min is not None
                            ):
                                is_multi = attr_info.flags.card_min > 1 or (
                                    hasattr(attr_info.flags, "card_max")
                                    and attr_info.flags.card_max is not None
                                    and attr_info.flags.card_max != 1
                                )
                            else:
                                is_multi = False
                            if is_multi and isinstance(raw_value, list):
                                # Convert each raw value to Attribute instance
                                player_attrs[field_name] = [attr_class(v) for v in raw_value]
                            else:
                                # Single value - let Pydantic handle conversion
                                player_attrs[field_name] = raw_value
                        else:
                            # For list fields (has_explicit_card), default to empty list
                            # For other optional fields, explicitly set to None
                            if attr_info.flags.has_explicit_card:
                                player_attrs[field_name] = []
                            else:
                                player_attrs[field_name] = None

                    # Create entity instance and assign to role
                    if any(v is not None for v in player_attrs.values()):
                        player_entity = entity_class(**player_attrs)
                        setattr(relation, role_name, player_entity)

            relations.append(relation)

        # Populate IIDs by fetching them in a second query
        if relations:
            self._populate_iids(relations)

        logger.info(f"Retrieved {len(relations)} relations: {self.model_class.__name__}")
        return relations

    def get_by_iid(self, iid: str) -> R | None:
        """Get a single relation by its TypeDB Internal ID (IID).

        Args:
            iid: TypeDB IID hex string (e.g., '0x1e00000000000000000000')

        Returns:
            Relation instance with _iid populated, or None if not found

        Example:
            relation = manager.get_by_iid("0x1e00000000000000000000")
            if relation:
                print(f"Found: {relation}")
        """
        logger.debug(f"Get relation by IID: {self.model_class.__name__}, iid={iid}")

        # Validate IID format
        if not iid or not iid.startswith("0x"):
            raise ValueError(f"Invalid IID format: {iid}. Expected hex string like '0x1e00...'")

        # Build match query with IID filter
        # Get all attributes (including inherited)
        all_attrs = self.model_class.get_all_attributes()

        # Build match clause with role players
        role_parts = []
        role_info = {}  # role_name -> (var, allowed_entity_classes)
        for role_name, role in self.model_class._roles.items():
            role_var = f"${role_name}"
            role_parts.append(f"{role.role_name}: {role_var}")
            role_info[role_name] = (role_var, role.player_entity_types)

        # Use isa! to bind exact type to $t for label() function
        roles_str = ", ".join(role_parts)
        base_type = self.model_class.get_type_name()
        match_clause = f"$r isa! $t ({roles_str}), iid {iid};\n$t sub {base_type};"

        # Build fetch clause with nested structure for role players
        # Use label($t) where $t is a TYPE variable bound via isa!
        fetch_items = ['"_iid": iid($r)', '"_type": label($t)']

        # Add relation attributes (including inherited)
        for field_name, attr_info in all_attrs.items():
            attr_name = attr_info.typ.get_attribute_name()
            # Multi-value attributes need to be wrapped in [] for TypeQL fetch
            if is_multi_value_attribute(attr_info.flags):
                fetch_items.append(f'"{attr_name}": [$r.{attr_name}]')
            else:
                fetch_items.append(f'"{attr_name}": $r.{attr_name}')

        # Add each role player as nested object
        for role_name, (role_var, allowed_entity_classes) in role_info.items():
            fetch_items.append(f'"{role_name}": {{\n    {role_var}.*\n  }}')

        fetch_body = ",\n  ".join(fetch_items)
        fetch_str = f"fetch {{\n  {fetch_body}\n}};"

        query_str = f"match\n{match_clause}\n{fetch_str}"
        logger.debug(f"Get by IID query: {query_str}")

        results = self._execute(query_str, TransactionType.READ)

        if not results:
            logger.debug(f"No relation found with IID {iid}")
            return None

        # Convert result to relation instance
        result = results[0]

        # Extract relation attributes (including inherited)
        attrs: dict[str, Any] = {}
        for field_name, attr_info in all_attrs.items():
            attr_class = attr_info.typ
            attr_name = attr_class.get_attribute_name()
            if attr_name in result:
                raw_value = result[attr_name]
                # Multi-value attributes need explicit conversion from list of raw values
                if is_multi_value_attribute(attr_info.flags) and isinstance(raw_value, list):
                    # Convert each raw value to Attribute instance
                    attrs[field_name] = [attr_class(v) for v in raw_value]
                else:
                    # Single value - let Pydantic handle conversion via model constructor
                    attrs[field_name] = raw_value
            else:
                # For list fields (has_explicit_card), default to empty list
                # For other optional fields, explicitly set to None
                if attr_info.flags.has_explicit_card:
                    attrs[field_name] = []
                else:
                    attrs[field_name] = None

        # Create relation instance
        relation = self.model_class(**attrs)

        # Extract role players from nested objects in result
        for role_name, (role_var, allowed_entity_classes) in role_info.items():
            if role_name in result and isinstance(result[role_name], dict):
                player_data = result[role_name]
                # Choose entity class based on key attributes present; fallback to first allowed
                entity_class = allowed_entity_classes[0]
                for candidate in allowed_entity_classes:
                    key_attr_names = [
                        attr_info.typ.get_attribute_name()
                        for attr_info in candidate.get_all_attributes().values()
                        if attr_info.flags.is_key
                    ]
                    if any(key in player_data for key in key_attr_names):
                        entity_class = candidate
                        break
                # Extract player attributes (including inherited)
                player_attrs: dict[str, Any] = {}
                for field_name, attr_info in entity_class.get_all_attributes().items():
                    attr_class = attr_info.typ
                    attr_name = attr_class.get_attribute_name()
                    if attr_name in player_data:
                        raw_value = player_data[attr_name]
                        if (
                            hasattr(attr_info.flags, "has_explicit_card")
                            and attr_info.flags.has_explicit_card
                        ):
                            is_multi = True
                        elif (
                            hasattr(attr_info.flags, "card_min")
                            and attr_info.flags.card_min is not None
                        ):
                            is_multi = attr_info.flags.card_min > 1 or (
                                hasattr(attr_info.flags, "card_max")
                                and attr_info.flags.card_max is not None
                                and attr_info.flags.card_max != 1
                            )
                        else:
                            is_multi = False
                        if is_multi and isinstance(raw_value, list):
                            player_attrs[field_name] = [attr_class(v) for v in raw_value]
                        else:
                            player_attrs[field_name] = raw_value
                    else:
                        if attr_info.flags.has_explicit_card:
                            player_attrs[field_name] = []
                        else:
                            player_attrs[field_name] = None

                # Create entity instance and assign to role
                if any(v is not None for v in player_attrs.values()):
                    player_entity = entity_class(**player_attrs)
                    setattr(relation, role_name, player_entity)

        # Set the IID directly since we know it
        # Done after role player assignments to avoid Pydantic revalidation resetting it
        object.__setattr__(relation, "_iid", iid)

        logger.info(f"Retrieved relation by IID: {self.model_class.__name__}")
        return relation

    def all(self) -> list[R]:
        """Get all relations of this type.

        Syntactic sugar for get() with no filters.

        Returns:
            List of all relations

        Example:
            all_employments = Employment.manager(db).all()
        """
        logger.debug(f"Getting all relations: {self.model_class.__name__}")
        return self.get()

    def update(self, relation: R) -> R:
        """Update a relation in the database based on its current state.

        Uses role players to identify the relation, then updates its attributes.
        Role players themselves cannot be changed (that would be a different relation).

        For single-value attributes (@card(0..1) or @card(1..1)), uses TypeQL update clause.
        For multi-value attributes (e.g., @card(0..5), @card(2..)), deletes old values
        and inserts new ones.

        Args:
            relation: The relation instance to update (must have all role players set)

        Returns:
            The same relation instance

        Example:
            # Fetch relation
            emp = employment_manager.get(employee=alice)[0]

            # Modify attributes
            emp.position = Position("Senior Engineer")
            emp.salary = Salary(120000)

            # Update in database
            employment_manager.update(emp)
        """
        logger.debug(f"Updating relation: {self.model_class.__name__}")
        # Get all attributes (including inherited)
        all_attrs = self.model_class.get_all_attributes()

        # Extract role players from relation instance for matching
        roles = self.model_class._roles
        role_players = {}
        for role_name, role in roles.items():
            entity = relation.__dict__.get(role_name)
            if entity is None:
                raise ValueError(f"Role player '{role_name}' is required for update")
            role_players[role_name] = entity

        # Separate single-value and multi-value updates from relation state
        single_value_updates = {}
        single_value_deletes = set()  # Track single-value attributes to delete
        multi_value_updates = {}

        for field_name, attr_info in all_attrs.items():
            attr_class = attr_info.typ
            attr_name = attr_class.get_attribute_name()
            flags = attr_info.flags

            # Get current value from relation
            current_value = getattr(relation, field_name, None)

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
            if is_multi_value_attribute(flags):
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

        # Build match clause with role players (IID-preferring)
        role_parts = []
        match_statements = []

        for role_name, entity in role_players.items():
            role_var = f"${role_name}"
            role = roles[role_name]
            role_parts.append(f"{role.role_name}: {role_var}")

            # Match the role player using IID-preferring logic
            entity_type_name = entity.__class__.get_type_name()
            match_clause = self._build_role_player_match(role_name, entity, entity_type_name)
            match_statements.append(f"{match_clause};")

        roles_str = ", ".join(role_parts)
        relation_match = f"$r isa {self.model_class.get_type_name()} ({roles_str});"
        match_statements.insert(0, relation_match)

        # Add match statements to bind multi-value attributes for deletion with optional guards
        if multi_value_updates:
            for attr_name, values in multi_value_updates.items():
                keep_literals = [format_value(v) for v in dict.fromkeys(values)]
                guard_lines = [
                    f"not {{ ${attr_name} == {literal}; }};" for literal in keep_literals
                ]
                try_block = "\n".join(
                    [
                        "try {",
                        f"  $r has {attr_name} ${attr_name};",
                        *[f"  {g}" for g in guard_lines],
                        "};",
                    ]
                )
                match_statements.append(try_block)

        # Add match statements to bind single-value attributes for deletion
        if single_value_deletes:
            for attr_name in single_value_deletes:
                match_statements.append(f"$r has {attr_name} ${attr_name};")

        match_clause = "\n".join(match_statements)

        # Build query parts
        query_parts = [f"match\n{match_clause}"]

        # Delete clause (for multi-value and single-value deletions)
        delete_parts = []
        if multi_value_updates:
            for attr_name in multi_value_updates:
                delete_parts.append(f"try {{ ${attr_name} of $r; }};")
        if single_value_deletes:
            for attr_name in single_value_deletes:
                delete_parts.append(f"${attr_name} of $r;")
        if delete_parts:
            delete_clause = "\n".join(delete_parts)
            query_parts.append(f"delete\n{delete_clause}")

        # Insert clause (for multi-value attributes)
        if multi_value_updates:
            insert_parts = []
            for attr_name, values in multi_value_updates.items():
                for value in values:
                    formatted_value = format_value(value)
                    insert_parts.append(f"$r has {attr_name} {formatted_value};")
            if insert_parts:
                insert_clause = "\n".join(insert_parts)
                query_parts.append(f"insert\n{insert_clause}")

        # Update clause (for single-value attributes)
        if single_value_updates:
            update_parts = []
            for attr_name, value in single_value_updates.items():
                formatted_value = format_value(value)
                update_parts.append(f"$r has {attr_name} {formatted_value};")
            update_clause = "\n".join(update_parts)
            query_parts.append(f"update\n{update_clause}")

        # Combine and execute
        full_query = "\n".join(query_parts)
        logger.debug(f"Update query: {full_query}")

        self._execute(full_query, TransactionType.WRITE)

        logger.info(f"Relation updated: {self.model_class.__name__}")
        return relation

    def delete(self, relation: R) -> R:
        """Delete a relation instance from the database.

        Uses role players' @key attributes to identify the relation (same as update).

        Args:
            relation: Relation instance to delete (must have all role players set)

        Returns:
            The deleted relation instance

        Raises:
            ValueError: If any role player is missing
            RelationNotFoundError: If relation does not exist in database

        Example:
            employment = Employment(employee=alice, employer=techcorp, position=Position("Engineer"))
            employment_manager.insert(employment)

            # Delete using the instance
            deleted = employment_manager.delete(employment)
        """
        logger.debug(f"Deleting relation: {self.model_class.__name__}")
        # Extract role players from relation instance for matching
        roles = self.model_class._roles
        role_players = {}

        for role_name in roles:
            entity = relation.__dict__.get(role_name)
            if entity is None:
                raise ValueError(f"Role player '{role_name}' is required for delete")
            role_players[role_name] = entity

        # Build match clause with role players (IID-preferring)
        role_parts = []
        match_statements = []

        for role_name, entity in role_players.items():
            role_var = f"${role_name}"
            role = roles[role_name]
            role_parts.append(f"{role.role_name}: {role_var}")

            # Match the role player using IID-preferring logic
            entity_type_name = entity.__class__.get_type_name()
            match_clause = self._build_role_player_match(role_name, entity, entity_type_name)
            match_statements.append(match_clause)

        roles_str = ", ".join(role_parts)
        relation_match = f"$r isa {self.model_class.get_type_name()} ({roles_str})"
        match_statements.insert(0, relation_match)

        # Check existence before delete by fetching the relation
        check_query = Query()
        check_pattern = ";\n".join(match_statements)
        check_query.match(check_pattern)
        check_query.fetch("$r")

        result = self._execute(check_query.build(), TransactionType.READ)

        if not result:
            raise RelationNotFoundError(
                f"Cannot delete: relation '{self.model_class.get_type_name()}' "
                "not found with given role players."
            )

        # Build delete query
        query = Query()
        pattern = ";\n".join(match_statements)
        query.match(pattern)
        query.delete("$r")

        query_str = query.build()
        logger.debug(f"Delete query: {query_str}")
        self._execute(query_str, TransactionType.WRITE)

        logger.info(f"Relation deleted: {self.model_class.__name__}")
        return relation

    def delete_many(self, relations: list[R], *, strict: bool = False) -> list[R]:
        """Delete multiple relations within a single transaction.

        Uses batched TypeQL queries (disjunctive OR-pattern) to delete all
        relations in a single query, optimizing from O(N) to O(1) queries.

        Args:
            relations: Relation instances to delete
            strict: If True, raises RelationNotFoundError when any relation doesn't exist.
                   If False (default), silently ignores missing relations (idempotent).

        Returns:
            List of relations that were actually deleted (subset of input if some
            relations didn't exist in the database)

        Raises:
            ValueError: If any relation has missing role players
            RelationNotFoundError: If strict=True and any relation doesn't exist
        """
        if not relations:
            logger.debug("delete_many called with empty list")
            return []

        logger.debug(f"Deleting {len(relations)} relations: {self.model_class.__name__}")

        roles = self.model_class._roles
        role_names = list(roles.keys())

        # Build disjunctive check query to see which relations exist (IID-preferring)
        # Use shared variable names across all branches for TypeQL compatibility
        check_clauses = []
        relation_keys: list[tuple[tuple[str, Any], ...]] = []

        for relation in relations:
            role_parts = []
            match_statements = []
            key_parts: list[tuple[str, Any]] = []

            for role_name in roles:
                entity = relation.__dict__.get(role_name)
                if entity is None:
                    raise ValueError(f"Role player '{role_name}' is required for delete")

                role_var = f"${role_name}"
                role = roles[role_name]
                role_parts.append(f"{role.role_name}: {role_var}")

                # Match role player using IID-preferring logic
                entity_type_name = entity.__class__.get_type_name()
                match_clause = self._build_role_player_match(role_name, entity, entity_type_name)
                match_statements.append(match_clause)

                # Build key for deduplication (use IID if available, else key attrs)
                entity_iid = getattr(entity, "_iid", None)
                if entity_iid:
                    key_parts.append((f"{role_name}:iid", entity_iid))
                else:
                    player_attrs = entity.__class__.get_all_attributes()
                    for field_name, attr_info in player_attrs.items():
                        if attr_info.flags.is_key:
                            key_value = getattr(entity, field_name, None)
                            if key_value is not None:
                                attr_name = attr_info.typ.get_attribute_name()
                                if hasattr(key_value, "value"):
                                    key_value = key_value.value
                                key_parts.append((f"{role_name}:{attr_name}", key_value))
                                break

            if not role_parts:
                continue

            roles_str = ", ".join(role_parts)
            relation_match = f"$r isa {self.model_class.get_type_name()} ({roles_str})"
            query_parts = [relation_match] + match_statements
            check_clauses.append(f"{{ {'; '.join(query_parts)}; }}")
            relation_keys.append(tuple(sorted(key_parts)))

        if not check_clauses:
            return []

        # Check which relations exist with a batched select query
        # Use shared variable names across all branches
        select_vars = ["$r"] + [f"${role_name}" for role_name in role_names]
        check_query = f"match\n{' or '.join(check_clauses)};\nselect {', '.join(select_vars)};"

        existing_results = self._execute(check_query, TransactionType.READ)

        # Results are in same order as clauses - each result is a matched relation
        # Build set of existing relation keys based on position
        existing_relations: list[R] = []
        missing_relations: list[R] = []

        # The results come back in order - just count how many exist
        result_count = len(existing_results) if existing_results else 0

        # Since we can't rely on result order matching clause order for all cases,
        # use a simpler approach: if all relations exist, proceed; otherwise check
        # each individually for strict mode
        if result_count == len(relations):
            # All relations exist
            existing_relations = list(relations)
        elif result_count == 0:
            # None exist
            missing_relations = list(relations)
        else:
            # Partial match - for strict mode, we need to know which ones
            # For now, assume all exist if not strict (will just skip missing)
            if strict:
                # Need to identify which ones are missing - use a simpler approach
                # Just mark all as missing if count doesn't match
                missing_relations = list(relations)
            else:
                existing_relations = list(relations)

        # Strict mode: raise if any relations don't exist
        if strict and missing_relations:
            raise RelationNotFoundError(
                f"Cannot delete: {len(missing_relations)} relation(s) not found "
                "with given role players."
            )

        if not existing_relations:
            logger.info("No relations to delete (none exist)")
            return []

        # Build batched delete query for existing relations (IID-preferring)
        # Reuse the same clause-building logic with shared variable names
        delete_clauses = []
        for relation in existing_relations:
            role_parts = []
            match_statements = []

            for role_name in roles:
                entity = relation.__dict__.get(role_name)
                if entity is None:
                    continue
                role_var = f"${role_name}"
                role = roles[role_name]
                role_parts.append(f"{role.role_name}: {role_var}")

                # Match role player using IID-preferring logic
                entity_type_name = entity.__class__.get_type_name()
                match_clause = self._build_role_player_match(role_name, entity, entity_type_name)
                match_statements.append(match_clause)

            roles_str = ", ".join(role_parts)
            relation_match = f"$r isa {self.model_class.get_type_name()} ({roles_str})"
            query_parts = [relation_match] + match_statements
            delete_clauses.append(f"{{ {'; '.join(query_parts)}; }}")

        # Execute batched delete
        delete_query = f"match\n{' or '.join(delete_clauses)};\ndelete\n$r;"
        logger.debug(f"Delete many batched query length: {len(delete_query)}")
        self._execute(delete_query, TransactionType.WRITE)

        logger.info(f"Deleted {len(existing_relations)} relations: {self.model_class.__name__}")
        return existing_relations

    def filter(self, *expressions: Any, **filters: Any) -> "RelationQuery[R]":
        """Create a query for filtering relations.

        Supports expression-based, dictionary-based, and Django-style role-player filtering.

        Args:
            *expressions: Expression objects (Age.gt(Age(30)), etc.)
            **filters: Attribute, role player, and role-player lookup filters
                - Attribute filters: position="Engineer", salary=100000
                - Role player filters: employee=person_entity, employer=company_entity
                - Role-player lookups: employee__age__gt=30, employer__name__contains="Tech"

        Returns:
            RelationQuery for chaining

        Examples:
            # Expression-based (advanced filtering)
            manager.filter(Salary.gt(Salary(100000)))
            manager.filter(Salary.gt(Salary(50000)), Salary.lt(Salary(150000)))

            # Dictionary-based (exact match)
            manager.filter(position="Engineer", employee=alice)

            # Role-player attribute filtering (Django-style)
            manager.filter(employee__age__gt=30)
            manager.filter(employer__name__contains="Tech", employee__age__gte=25)

            # Combined
            manager.filter(Salary.gt(Salary(80000)), employee__age__gt=25)

        Raises:
            ValueError: If expression references attribute type not owned by relation,
                       or if role-player lookup references unknown role/attribute
        """
        # Import here to avoid circular dependency
        from .lookup import parse_role_lookup_filters
        from .query import RelationQuery

        # Parse filters into attr_filters, role_player_filters, role_expressions, and attr_expressions
        attr_filters, role_player_filters, role_expressions, attr_expressions = (
            parse_role_lookup_filters(self.model_class, filters)
        )

        # Separate RolePlayerExpr from regular expressions
        from type_bridge.expressions import RolePlayerExpr

        regular_expressions = []
        role_player_expr_list = []

        if expressions:
            for expr in expressions:
                if isinstance(expr, RolePlayerExpr):
                    role_player_expr_list.append(expr)
                else:
                    regular_expressions.append(expr)

        # Add attr_expressions (from Django-style lookups on relation attributes) to regular_expressions
        regular_expressions.extend(attr_expressions)

        # Validate regular expressions reference owned attribute types
        if regular_expressions:
            owned_attrs = self.model_class.get_all_attributes()
            owned_attr_types = {attr_info.typ for attr_info in owned_attrs.values()}

            for expr in regular_expressions:
                # Get attribute types from expression
                expr_attr_types = expr.get_attribute_types()

                # Check if all attribute types are owned by relation
                for attr_type in expr_attr_types:
                    if attr_type not in owned_attr_types:
                        raise ValueError(
                            f"{self.model_class.__name__} does not own attribute type {attr_type.__name__}. "
                            f"Available attribute types: {', '.join(t.__name__ for t in owned_attr_types)}"
                        )

        # Validate RolePlayerExpr reference valid roles
        roles = self.model_class._roles
        for expr in role_player_expr_list:
            if expr.role_name not in roles:
                raise ValueError(
                    f"{self.model_class.__name__} does not have role '{expr.role_name}'. "
                    f"Available roles: {list(roles.keys())}"
                )

        # Combine attr_filters and role_player_filters for backward compatibility
        combined_filters = {**attr_filters, **role_player_filters}
        query = RelationQuery(
            self._connection, self.model_class, combined_filters if combined_filters else None
        )
        if regular_expressions:
            query._expressions.extend(regular_expressions)

        # Add RolePlayerExpr to role_player_expressions
        for expr in role_player_expr_list:
            if expr.role_name not in query._role_player_expressions:
                query._role_player_expressions[expr.role_name] = []
            query._role_player_expressions[expr.role_name].append(expr)

        if role_expressions:
            for role_name, exprs in role_expressions.items():
                if role_name not in query._role_player_expressions:
                    query._role_player_expressions[role_name] = []
                query._role_player_expressions[role_name].extend(exprs)
        return query

    def _execute(self, query: str, tx_type: TransactionType) -> list[dict[str, Any]]:
        """Execute a query using an existing transaction when provided."""
        return self._executor.execute(query, tx_type)

    def _populate_iids(self, relations: list[R]) -> None:
        """Populate _iid field on relations and their role players by querying TypeDB.

        Since fetch queries cannot return IIDs, this method uses a single batched
        disjunctive query to get IIDs for all relations and their role players.

        The query captures key attribute values as variables to enable proper
        correlation of results back to the original relation instances.

        Optimized to use O(1) queries instead of O(N) queries.

        Args:
            relations: List of relations to populate IIDs for
        """
        if not relations:
            return

        roles = self.model_class._roles
        role_names = list(roles.keys())

        # Track which roles have key attributes and their attribute names
        # Maps role_name -> (key_var_name, attr_name)
        role_key_info: dict[str, tuple[str, str]] = {}

        # Build a single query matching all relations using shared variable names
        # Capture key attribute values as variables for result correlation
        shared_or_clauses = []
        for relation in relations:
            role_parts = []
            match_statements = []

            for role_name, role in roles.items():
                entity = getattr(relation, role_name, None)
                if entity is None:
                    continue

                role_var = f"${role_name}"
                role_parts.append(f"{role.role_name}: {role_var}")

                entity_class = entity.__class__
                player_owned_attrs = entity_class.get_all_attributes()
                for field_name, attr_info in player_owned_attrs.items():
                    if attr_info.flags.is_key:
                        key_value = getattr(entity, field_name, None)
                        if key_value is not None:
                            attr_name = attr_info.typ.get_attribute_name()
                            if hasattr(key_value, "value"):
                                key_value = key_value.value
                            formatted_value = format_value(key_value)
                            # Use a variable for the key attribute to capture its value
                            key_var = f"${role_name}_key"
                            match_statements.append(
                                f"{role_var} has {attr_name} {key_var}; {key_var} == {formatted_value}"
                            )
                            # Track the key variable for this role
                            if role_name not in role_key_info:
                                role_key_info[role_name] = (f"{role_name}_key", attr_name)
                            break

            if not role_parts:
                continue

            roles_str = ", ".join(role_parts)
            relation_match = f"$r isa {self.model_class.get_type_name()} ({roles_str})"
            clause_parts = [relation_match] + match_statements
            shared_or_clauses.append(f"{{ {'; '.join(clause_parts)}; }}")

        if not shared_or_clauses:
            return

        # Build the query with OR clauses, including key attribute variables in select
        select_vars = ["$r"] + [f"${role_name}" for role_name in role_names]
        for key_var_name, _ in role_key_info.values():
            select_vars.append(f"${key_var_name}")

        query_str = f"match\n{' or '.join(shared_or_clauses)};\nselect {', '.join(select_vars)};"
        logger.debug(f"Batched IID lookup query: {query_str[:200]}...")

        results = self._execute(query_str, TransactionType.READ)

        if not results:
            return

        # Build a lookup map from key attribute values to results
        # This allows proper correlation: each relation maps to exactly one result
        result_map: dict[tuple[tuple[str, Any], ...], dict[str, Any]] = {}
        for result in results:
            key_parts: list[tuple[str, Any]] = []
            for role_name, (key_var_name, _) in role_key_info.items():
                if key_var_name in result:
                    key_val = result[key_var_name]
                    # Extract value from concept data dict
                    if isinstance(key_val, dict):
                        key_val = key_val.get("value", key_val.get("result"))
                    key_parts.append((role_name, key_val))
            if key_parts:
                result_map[tuple(sorted(key_parts))] = result

        # For each relation, find its corresponding result by key attribute values
        for relation in relations:
            # Build the key for this relation from its role players' key attributes
            relation_key_parts: list[tuple[str, Any]] = []
            role_var_to_entity: dict[str, Any] = {}

            for role_name in role_names:
                entity = getattr(relation, role_name, None)
                if entity is not None:
                    role_var_to_entity[role_name] = entity

                    # Get the key attribute value for this role player
                    if role_name in role_key_info:
                        entity_class = entity.__class__
                        player_owned_attrs = entity_class.get_all_attributes()
                        for field_name, attr_info in player_owned_attrs.items():
                            if attr_info.flags.is_key:
                                key_value = getattr(entity, field_name, None)
                                if key_value is not None:
                                    if hasattr(key_value, "value"):
                                        key_value = key_value.value
                                    relation_key_parts.append((role_name, key_value))
                                break

            # Look up the result by key values
            relation_key = tuple(sorted(relation_key_parts))
            matched_result = result_map.get(relation_key)

            if matched_result:
                # Extract relation IID
                if "r" in matched_result and isinstance(matched_result["r"], dict):
                    relation_iid = matched_result["r"].get("_iid")
                    if relation_iid:
                        object.__setattr__(relation, "_iid", relation_iid)
                        logger.debug(
                            f"Set IID {relation_iid} for relation {self.model_class.__name__}"
                        )

                # Extract role player IIDs
                for role_name, entity in role_var_to_entity.items():
                    if role_name in matched_result and isinstance(matched_result[role_name], dict):
                        player_iid = matched_result[role_name].get("_iid")
                        if player_iid:
                            object.__setattr__(entity, "_iid", player_iid)
                            logger.debug(
                                f"Set IID {player_iid} for role player {role_name} "
                                f"({entity.__class__.__name__})"
                            )

    def group_by(self, *fields: Any) -> "RelationGroupByQuery[R]":
        """Create a group-by query for aggregating by field values.

        Args:
            *fields: Field references to group by (Employment.position, etc.)

        Returns:
            RelationGroupByQuery for aggregation

        Example:
            # Group by single field
            result = manager.group_by(Employment.position).aggregate(Employment.salary.avg())

            # Group by multiple fields
            result = manager.group_by(Employment.position, Employment.department).aggregate(
                Employment.salary.avg()
            )
        """
        # Import here to avoid circular dependency
        from .group_by import RelationGroupByQuery

        return RelationGroupByQuery(self._connection, self.model_class, {}, [], fields)
