"""RelationQuery for chainable relation queries."""

import logging
from typing import TYPE_CHECKING, Any

from typedb.driver import TransactionType

from type_bridge.models import Relation
from type_bridge.query import Query
from type_bridge.session import Connection, ConnectionExecutor

from ..base import R
from ..utils import format_value, is_multi_value_attribute

logger = logging.getLogger(__name__)

if TYPE_CHECKING:
    from .group_by import RelationGroupByQuery


class RelationQuery[R: Relation]:
    """Chainable query for relations.

    Type-safe query builder that preserves relation type information.
    Supports both dictionary filters (exact match) and expression-based filters.
    """

    def __init__(
        self,
        connection: Connection,
        model_class: type[R],
        filters: dict[str, Any] | None = None,
    ):
        """Initialize relation query.

        Args:
            connection: Database, Transaction, or TransactionContext
            model_class: Relation model class
            filters: Attribute and role player filters (exact match) - optional
        """
        self._connection = connection
        self._executor = ConnectionExecutor(connection)
        self.model_class = model_class
        self.filters = filters or {}
        self._expressions: list[Any] = []  # Store Expression objects
        self._role_player_expressions: dict[str, list[Any]] = {}  # role_name -> expressions
        self._limit_value: int | None = None
        self._offset_value: int | None = None
        # [(field_name, direction, role_name or None)]
        self._order_by_fields: list[tuple[str, str, str | None]] = []

    def filter(self, *expressions: Any, **filters: Any) -> "RelationQuery[R]":
        """Add filters to the query.

        Supports expression-based and Django-style filtering for chained queries.

        Args:
            *expressions: Expression objects (ComparisonExpr, StringExpr, RolePlayerExpr, etc.)
            **filters: Django-style filters for attributes and role-player lookups
                - Attribute filters: position="Engineer", salary=100000
                - Role player filters: employee=person_entity
                - Role-player lookups: employee__age__gt=30, employer__name__contains="Tech"

        Returns:
            Self for chaining

        Example:
            # Filter by relation's own attributes
            query = Employment.manager(db).filter(
                Salary.gt(Salary(100000)),
                Position.contains(Position("Engineer"))
            )

            # Filter by role-player attributes (type-safe syntax)
            query = Employment.manager(db).filter(
                Employment.employee.age.gt(Age(30))
            )

            # Django-style lookups (can be chained)
            query = query.filter(employer__industry__contains="Tech")

            # Combined in one call
            query = manager.filter(
                Employment.employee.age.gte(Age(25)),
                salary__gt=50000
            )

        Raises:
            ValueError: If expression references attribute type not owned by relation
                       or role-player expression references unknown role
        """
        from type_bridge.expressions import RolePlayerExpr

        from .lookup import parse_role_lookup_filters

        # Parse Django-style filters if provided
        if filters:
            attr_filters, role_player_filters, role_expressions, attr_expressions = (
                parse_role_lookup_filters(self.model_class, filters)
            )

            # Add attr_filters to self.filters
            self.filters.update(attr_filters)

            # Add role_player_filters to self.filters
            self.filters.update(role_player_filters)

            # Add role_expressions to _role_player_expressions
            for role_name, exprs in role_expressions.items():
                if role_name not in self._role_player_expressions:
                    self._role_player_expressions[role_name] = []
                self._role_player_expressions[role_name].extend(exprs)

            # Add attr_expressions to _expressions
            self._expressions.extend(attr_expressions)

        # Separate RolePlayerExpr from regular expressions
        regular_expressions = []
        role_player_expr_list = []

        if expressions:
            for expr in expressions:
                if isinstance(expr, RolePlayerExpr):
                    role_player_expr_list.append(expr)
                else:
                    regular_expressions.append(expr)

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

        # Add regular expressions
        self._expressions.extend(regular_expressions)

        # Add RolePlayerExpr to role_player_expressions
        for expr in role_player_expr_list:
            if expr.role_name not in self._role_player_expressions:
                self._role_player_expressions[expr.role_name] = []
            self._role_player_expressions[expr.role_name].append(expr)

        return self

    def limit(self, limit: int) -> "RelationQuery[R]":
        """Limit number of results.

        Note: Requires sorting for stable pagination. A required attribute will be
        automatically selected for sorting.

        Args:
            limit: Maximum number of results

        Returns:
            Self for chaining
        """
        self._limit_value = limit
        return self

    def offset(self, offset: int) -> "RelationQuery[R]":
        """Skip number of results.

        Note: Requires sorting for stable pagination. A required attribute will be
        automatically selected for sorting.

        Args:
            offset: Number of results to skip

        Returns:
            Self for chaining
        """
        self._offset_value = offset
        return self

    def order_by(self, *fields: str) -> "RelationQuery[R]":
        """Sort query results by one or more fields.

        Args:
            *fields: Field names to sort by. Prefix with '-' for descending order.
                     Use 'role__attr' syntax for role-player attributes.

        Returns:
            Self for chaining

        Raises:
            ValueError: If field name does not exist
            ValueError: If role name does not exist
            ValueError: If role player does not have the specified attribute

        Example:
            # Relation attribute
            query.order_by('salary')
            query.order_by('-salary')

            # Role-player attribute
            query.order_by('employee__age')
            query.order_by('-employee__age')

            # Multiple
            query.order_by('employee__age', '-salary')
        """
        all_attrs = self.model_class.get_all_attributes()
        roles = self.model_class._roles

        for field in fields:
            # Parse direction prefix
            if field.startswith("-"):
                direction = "desc"
                field_spec = field[1:]
            else:
                direction = "asc"
                field_spec = field

            # Check if this is a role-player attribute (contains '__')
            if "__" in field_spec:
                parts = field_spec.split("__", 1)
                if len(parts) != 2:
                    raise ValueError(
                        f"Invalid sort field format '{field_spec}'. "
                        "Use 'role__attr' for role-player attributes."
                    )

                role_name, attr_name = parts

                # Validate role exists
                if role_name not in roles:
                    raise ValueError(
                        f"Unknown role '{role_name}' for {self.model_class.__name__}. "
                        f"Available roles: {list(roles.keys())}"
                    )

                # Validate attribute exists on at least one player type
                role = roles[role_name]
                all_player_attrs: set[str] = set()
                for player_type in role.player_entity_types:
                    player_attrs = player_type.get_all_attributes()
                    all_player_attrs.update(player_attrs.keys())

                if attr_name not in all_player_attrs:
                    raise ValueError(
                        f"Role '{role_name}' players do not have attribute '{attr_name}'. "
                        f"Available attributes: {sorted(all_player_attrs)}"
                    )

                self._order_by_fields.append((attr_name, direction, role_name))
            else:
                # Direct relation attribute
                if field_spec not in all_attrs:
                    raise ValueError(
                        f"Unknown sort field '{field_spec}' for {self.model_class.__name__}. "
                        f"Available fields: {list(all_attrs.keys())}"
                    )

                # Reject multi-value attributes
                if is_multi_value_attribute(all_attrs[field_spec].flags):
                    raise ValueError(
                        f"Cannot sort by multi-value attribute '{field_spec}'. "
                        "Multi-value attributes can have multiple values per relation."
                    )

                self._order_by_fields.append((field_spec, direction, None))

        return self

    def execute(self) -> list[R]:
        """Execute the query.

        Returns:
            List of matching relations
        """
        logger.debug(
            f"Executing RelationQuery: {self.model_class.__name__}, "
            f"filters={self.filters}, expressions={len(self._expressions)}"
        )

        # Get all attributes (including inherited)
        all_attrs = self.model_class.get_all_attributes()

        # Separate attribute filters from role player filters
        attr_filters = {}
        role_player_filters = {}

        for key, value in self.filters.items():
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

        # Use isa! to bind exact type to $t for label() function
        roles_str = ", ".join(role_parts)
        base_type = self.model_class.get_type_name()
        match_clauses = [f"$r isa! $t ({roles_str})", f"$t sub {base_type}"]

        # Add dict-based attribute filters
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

        # Apply expression-based filters
        for expr in self._expressions:
            # Generate TypeQL pattern from expression
            pattern = expr.to_typeql("$r")
            match_clauses.append(pattern)

        # Apply role player expression-based filters
        for role_name, expressions in self._role_player_expressions.items():
            role_var = f"${role_name}"
            for expr in expressions:
                # Generate TypeQL pattern using role player variable
                pattern = expr.to_typeql(role_var)
                match_clauses.append(pattern)

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
        for role_name, (role_var, allowed_entity_classes) in role_info.items():
            fetch_items.append(f'"{role_name}": {{\n    {role_var}.*\n  }}')

        fetch_body = ",\n  ".join(fetch_items)

        # Apply sorting - either user-specified or auto-select for pagination
        sort_clause = ""
        sort_match_clauses: list[str] = []

        if self._order_by_fields:
            # User-specified sort fields
            sort_parts = []
            for i, (field_name, direction, role_name) in enumerate(self._order_by_fields):
                sort_var = f"$sort_{i}"

                if role_name is not None:
                    # Role-player attribute: get attribute name from player type
                    role = self.model_class._roles[role_name]
                    # Find attribute info from first player type that has it
                    attr_name = None
                    for player_type in role.player_entity_types:
                        player_attrs = player_type.get_all_attributes()
                        if field_name in player_attrs:
                            attr_info = player_attrs[field_name]
                            attr_name = attr_info.typ.get_attribute_name()
                            break

                    if attr_name:
                        role_var = f"${role_name}"
                        sort_match_clauses.append(f"{role_var} has {attr_name} {sort_var}")
                        sort_parts.append(f"{sort_var} {direction}")
                else:
                    # Relation attribute
                    attr_info = all_attrs[field_name]
                    attr_name = attr_info.typ.get_attribute_name()
                    sort_match_clauses.append(f"$r has {attr_name} {sort_var}")
                    sort_parts.append(f"{sort_var} {direction}")

            if sort_parts:
                sort_clause = "\nsort " + ", ".join(sort_parts) + ";"
        elif self._limit_value is not None or self._offset_value is not None:
            # Auto-select a sort attribute for pagination (required for stable limit/offset)
            sort_attr = None
            already_matched_attrs = set()
            for field_name in attr_filters.keys():
                attr_info = all_attrs[field_name]
                already_matched_attrs.add(attr_info.typ.get_attribute_name())

            # Try to find a required attribute that isn't already matched
            for field_name, attr_info in all_attrs.items():
                attr_name = attr_info.typ.get_attribute_name()
                if attr_name not in already_matched_attrs:
                    if attr_info.flags.is_key or (
                        attr_info.flags.card_min is not None and attr_info.flags.card_min >= 1
                    ):
                        sort_attr = attr_name
                        break

            if sort_attr:
                sort_match_clauses.append(f"$r has {sort_attr} $sort_attr")
                sort_clause = "\nsort $sort_attr;"

        # Add sort bindings to match clause
        if sort_match_clauses:
            match_str = match_str.rstrip(";")
            match_str += ";\n" + ";\n".join(sort_match_clauses) + ";"

        # Add limit/offset clauses
        pagination_clause = ""
        if self._offset_value is not None:
            pagination_clause += f"\noffset {self._offset_value};"
        if self._limit_value is not None:
            pagination_clause += f"\nlimit {self._limit_value};"

        fetch_str = f"fetch {{\n  {fetch_body}\n}};"
        query_str = f"match\n{match_str}{sort_clause}{pagination_clause}\n{fetch_str}"
        logger.debug(f"RelationQuery: {query_str}")

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

        logger.info(f"RelationQuery executed: {len(relations)} relations returned")
        return relations

    def _populate_iids(self, relations: list[R]) -> None:
        """Populate _iid field on relations and their role players by querying TypeDB.

        Since fetch queries cannot return IIDs, this method uses a single
        batched disjunctive query to get IIDs for all relations at once.

        Optimized to use O(1) queries instead of O(N) queries.

        Args:
            relations: List of relations to populate IIDs for
        """
        if not relations:
            return

        roles = self.model_class._roles
        role_names = list(roles.keys())

        # Build batched disjunctive query for all relations
        # Use shared variable names across all branches for TypeQL compatibility
        or_clauses = []
        # Track relation key info for correlation
        relation_key_data: list[dict[str, tuple[str, Any, Any]]] = []

        for relation in relations:
            # Build match clause with role players using shared variable names
            role_parts = []
            match_statements = []
            role_key_info: dict[
                str, tuple[str, Any, Any]
            ] = {}  # role_name -> (attr_name, value, entity)

            for role_name, role in roles.items():
                entity = getattr(relation, role_name, None)
                if entity is None:
                    logger.debug(f"Skipping role {role_name} for relation without role player")
                    continue

                role_var = f"${role_name}"
                role_parts.append(f"{role.role_name}: {role_var}")

                # Match the role player by their key attributes
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
                            match_statements.append(f"{role_var} has {attr_name} {formatted_value}")
                            role_key_info[role_name] = (attr_name, key_value, entity)
                            break

            if not role_parts:
                continue

            roles_str = ", ".join(role_parts)
            relation_match = f"$r isa {self.model_class.get_type_name()} ({roles_str})"

            # Build clause for this relation
            query_parts = [relation_match] + match_statements
            or_clauses.append(f"{{ {'; '.join(query_parts)}; }}")

            relation_key_data.append(role_key_info)

        if not or_clauses:
            return

        # Build select variables - shared across all branches
        select_vars = ["$r"] + [f"${role_name}" for role_name in role_names]

        # Single disjunctive query for all relations
        query_str = f"match\n{' or '.join(or_clauses)};\nselect {', '.join(select_vars)};"
        logger.debug(f"Batched IID lookup query: {query_str}")

        results = self._execute(query_str, TransactionType.READ)

        if not results:
            return

        # Build a map from role player key values to (relation, role_key_info) for correlation
        # Key: tuple of sorted (role_name, attr_name, value) tuples
        key_to_relation: dict[tuple[tuple[str, str, Any], ...], tuple[Any, dict]] = {}
        for relation, role_key_info in zip(relations, relation_key_data):
            key_parts: list[tuple[str, str, Any]] = []
            for role_name, (attr_name, value, _) in role_key_info.items():
                key_parts.append((role_name, attr_name, value))
            if key_parts:
                map_key = tuple(sorted(key_parts))
                key_to_relation[map_key] = (relation, role_key_info)

        # Process results - results come back in the same order as or_clauses
        # Each result corresponds to one relation
        for idx, result in enumerate(results):
            if idx >= len(relation_key_data):
                break

            relation = relations[idx]
            role_key_info = relation_key_data[idx]

            # Extract relation IID
            if "r" in result and isinstance(result["r"], dict):
                relation_iid = result["r"].get("_iid")
                if relation_iid:
                    object.__setattr__(relation, "_iid", relation_iid)
                    logger.debug(f"Set IID {relation_iid} for relation {self.model_class.__name__}")

            # Extract role player IIDs
            for role_name, (_, _, entity) in role_key_info.items():
                if role_name in result and isinstance(result[role_name], dict):
                    player_iid = result[role_name].get("_iid")
                    if player_iid:
                        object.__setattr__(entity, "_iid", player_iid)
                        logger.debug(
                            f"Set IID {player_iid} for role player {role_name} "
                            f"({entity.__class__.__name__})"
                        )

    def first(self) -> R | None:
        """Get first matching relation.

        Returns:
            First relation or None
        """
        results = self.limit(1).execute()
        return results[0] if results else None

    def count(self) -> int:
        """Count matching relations.

        Returns:
            Number of matching relations
        """
        return len(self.execute())

    def delete(self) -> int:
        """Delete all relations matching the current filters.

        Builds and executes a delete query based on the current filter state.
        Uses a single transaction for atomic deletion.

        Returns:
            Number of relations deleted

        Example:
            # Delete all high-salary employments
            count = Employment.manager(db).filter(Salary.gt(Salary(150000))).delete()
            logger.info(f"Deleted {count} employments")

            # Delete with multiple filters
            count = Employment.manager(db).filter(
                Position.eq(Position("Intern")),
                Salary.lt(Salary(30000))
            ).delete()
        """

        # Get all attributes (including inherited)
        all_attrs = self.model_class.get_all_attributes()

        # Separate attribute filters from role player filters
        attr_filters = {}
        role_player_filters = {}

        for key, value in self.filters.items():
            if key in self.model_class._roles:
                # This is a role player filter
                role_player_filters[key] = value
            elif key in all_attrs:
                # This is an attribute filter
                attr_filters[key] = value
            else:
                raise ValueError(f"Unknown filter: {key}")

        # Build match clause with role players
        query = Query()
        role_parts = []
        role_info = {}  # role_name -> (var, allowed_entity_classes)
        for role_name, role in self.model_class._roles.items():
            role_var = f"${role_name}"
            role_parts.append(f"{role.role_name}: {role_var}")
            role_info[role_name] = (role_var, role.player_entity_types)

        roles_str = ", ".join(role_parts)

        # Build relation match with inline attribute filters
        relation_parts = [f"$r isa {self.model_class.get_type_name()} ({roles_str})"]

        # Add dict-based attribute filters
        for field_name, value in attr_filters.items():
            attr_info = all_attrs[field_name]
            attr_name = attr_info.typ.get_attribute_name()
            formatted_value = format_value(value)
            relation_parts.append(f"has {attr_name} {formatted_value}")

        # Combine relation and attribute filters with commas
        pattern = ", ".join(relation_parts)
        query.match(pattern)

        # Add role player filter clauses
        for role_name, player_entity in role_player_filters.items():
            role_var = f"${role_name}"
            allowed_entity_classes = role_info[role_name][1]

            # Determine the matching entity class for this player
            entity_class = player_entity.__class__
            if not isinstance(player_entity, allowed_entity_classes):
                allowed_names = ", ".join(cls.__name__ for cls in allowed_entity_classes)
                raise TypeError(
                    f"Role '{role_name}' expects types ({allowed_names}), got {entity_class.__name__}"
                )

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
                        query.match(f"{role_var} has {attr_name} {formatted_value}")
                        break

        # Add expression-based filters
        for expr in self._expressions:
            expr_pattern = expr.to_typeql("$r")
            query.match(expr_pattern)

        # Add role player expression-based filters
        for role_name, expressions in self._role_player_expressions.items():
            role_var = f"${role_name}"
            for expr in expressions:
                expr_pattern = expr.to_typeql(role_var)
                query.match(expr_pattern)

        # Add delete clause
        query.delete("$r")

        # Execute in single transaction
        query_str = query.build()
        logger.debug(f"Delete query: {query_str}")
        results = self._execute(query_str, TransactionType.WRITE)
        count = len(results) if results else 0
        logger.info(f"Deleted {count} relations via filter")

        return count

    def update_with(self, func: Any) -> list[R]:
        """Update relations by applying a function to each matching relation.

        Fetches all matching relations, applies the provided function to each one,
        then saves all updates in a single batched query. If the function raises an
        error on any relation, stops immediately and raises the error.

        Optimized to use O(1) queries instead of O(N) queries.

        Args:
            func: Callable that takes a relation and modifies it in-place.
                  Can be a lambda or regular function.

        Returns:
            List of updated relations

        Example:
            # Increase salary for all engineers by 10%
            updated = Employment.manager(db).filter(
                Position.eq(Position("Engineer"))
            ).update_with(
                lambda emp: setattr(emp, 'salary', Salary(int(emp.salary.value * 1.1)))
            )

            # Complex update with function
            def promote(employment):
                employment.position = Position("Senior " + employment.position.value)
                if employment.salary:
                    employment.salary = Salary(int(employment.salary.value * 1.2))

            promoted = Employment.manager(db).filter(
                Position.contains(Position("Engineer"))
            ).update_with(promote)

        Raises:
            Any exception raised by the function during processing
        """
        # Fetch all matching relations
        relations = self.execute()

        # Return empty list if no matches
        if not relations:
            return []

        # Store original attribute values before applying function
        # This is needed to match the relation uniquely in the update query
        owned_attrs = self.model_class.get_all_attributes()
        original_values = []

        for relation in relations:
            original = {}
            for field_name, attr_info in owned_attrs.items():
                current_value = getattr(relation, field_name, None)
                # Store the original value (before function modifies it)
                original[field_name] = current_value
            original_values.append(original)

        # Apply function to each relation (stop and raise if error)
        for relation in relations:
            func(relation)

        # Build batched update query for all relations
        query_str = self._build_batched_update_query(relations, original_values)

        if query_str:
            self._execute(query_str, TransactionType.WRITE)

        return relations

    def _build_batched_update_query(
        self, relations: list[R], original_values: list[dict[str, Any]]
    ) -> str:
        """Build a single batched TypeQL query to update multiple relations.

        Uses conjunctive batching pattern for efficient bulk updates.

        Args:
            relations: List of relation instances with NEW values
            original_values: List of dicts with original values for matching

        Returns:
            Single TypeQL query string that updates all relations
        """
        if not relations:
            return ""

        match_parts = []
        delete_parts = []
        insert_parts = []
        update_parts = []

        for idx, (relation, original) in enumerate(zip(relations, original_values)):
            var_name = f"$r{idx}"
            m_part, d_part, i_part, u_part = self._build_update_query_parts(
                relation, original, var_name, idx
            )

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
        self,
        relation: R,
        original_values: dict[str, Any],
        var_name: str = "$r",
        idx: int = 0,
    ) -> tuple[str, str, str, str]:
        """Build the TypeQL query parts for updating a single relation.

        Args:
            relation: Relation instance with NEW values to update to
            original_values: Dict of field_name -> original value (for matching)
            var_name: Variable name to use for this relation (e.g., "$r0")
            idx: Index for unique variable names

        Returns:
            Tuple of (match_clause, delete_clause, insert_clause, update_clause)
        """
        # Get all attributes (including inherited) to determine cardinality
        owned_attrs = self.model_class.get_all_attributes()

        # Extract role players from relation for matching
        role_players = {}
        roles = self.model_class._roles

        for role_name in roles:
            entity = getattr(relation, role_name, None)
            if entity is None:
                msg = f"Role '{role_name}' is required for update"
                raise ValueError(msg)
            role_players[role_name] = entity

        # Separate single-value and multi-value updates from NEW values
        single_value_updates = {}
        multi_value_updates: dict[str, list[Any]] = {}

        # Also separate original values for matching
        original_single_values = {}
        original_multi_values: dict[str, list[Any]] = {}

        for field_name, attr_info in owned_attrs.items():
            attr_class = attr_info.typ
            attr_name = attr_class.get_attribute_name()
            flags = attr_info.flags

            # Get NEW value from relation
            new_value = getattr(relation, field_name, None)

            # Get ORIGINAL value for matching
            orig_value = original_values.get(field_name)

            # Extract raw values from Attribute instances (for NEW values)
            if new_value is not None:
                if isinstance(new_value, list):
                    raw_values = []
                    for item in new_value:
                        if hasattr(item, "value"):
                            raw_values.append(item.value)
                        else:
                            raw_values.append(item)
                    new_value = raw_values
                elif hasattr(new_value, "value"):
                    new_value = new_value.value

            # Extract raw values from Attribute instances (for ORIGINAL values)
            if orig_value is not None:
                if isinstance(orig_value, list):
                    raw_orig_values = []
                    for item in orig_value:
                        if hasattr(item, "value"):
                            raw_orig_values.append(item.value)
                        else:
                            raw_orig_values.append(item)
                    orig_value = raw_orig_values
                elif hasattr(orig_value, "value"):
                    orig_value = orig_value.value

            # Determine if multi-value
            if is_multi_value_attribute(flags):
                # Multi-value: store as list (even if empty)
                if new_value is None:
                    new_value = []
                multi_value_updates[attr_name] = new_value
                if orig_value is None:
                    orig_value = []
                original_multi_values[attr_name] = orig_value
            else:
                # Single-value: skip None values for optional attributes
                if new_value is not None:
                    single_value_updates[attr_name] = new_value
                if orig_value is not None:
                    original_single_values[attr_name] = orig_value

        # Build match clause with role players
        role_parts = []
        match_statements = []

        for role_name, entity in role_players.items():
            role_var = f"${role_name}_{idx}"
            role = roles[role_name]
            role_parts.append(f"{role.role_name}: {role_var}")

            # Match the role player by their key attributes (including inherited)
            entity_class = entity.__class__
            player_owned_attrs = entity_class.get_all_attributes()
            for field_name, attr_info in player_owned_attrs.items():
                if attr_info.flags.is_key:
                    key_value = getattr(entity, field_name, None)
                    if key_value is not None:
                        attr_name = attr_info.typ.get_attribute_name()
                        # Extract value from Attribute instance if needed
                        if hasattr(key_value, "value"):
                            key_value = key_value.value
                        formatted_value = format_value(key_value)
                        match_statements.append(f"{role_var} has {attr_name} {formatted_value};")
                        break

        roles_str = ", ".join(role_parts)
        relation_match_parts = [f"{var_name} isa {self.model_class.get_type_name()} ({roles_str})"]

        # IMPORTANT: Match by ORIGINAL attribute values to uniquely identify the relation
        # This is crucial because multiple relations can have same role players
        for attr_name, orig_value in original_single_values.items():
            formatted_value = format_value(orig_value)
            relation_match_parts.append(f"has {attr_name} {formatted_value}")

        relation_match = ", ".join(relation_match_parts) + ";"
        match_statements.insert(0, relation_match)

        # Add match statements to bind multi-value attributes for deletion
        if multi_value_updates:
            for attr_name in multi_value_updates:
                match_statements.append(f"{var_name} has {attr_name} ${attr_name}_{idx};")

        match_clause = "\n".join(match_statements)

        # Build delete clause (for multi-value attributes)
        delete_clause = ""
        if multi_value_updates:
            delete_lines = []
            for attr_name in multi_value_updates:
                delete_lines.append(f"${attr_name}_{idx} of {var_name};")
            delete_clause = "\n".join(delete_lines)

        # Build insert clause (for multi-value attributes)
        insert_clause = ""
        if multi_value_updates:
            insert_lines = []
            for attr_name, values in multi_value_updates.items():
                for value in values:
                    formatted_value = format_value(value)
                    insert_lines.append(f"{var_name} has {attr_name} {formatted_value};")
            if insert_lines:
                insert_clause = "\n".join(insert_lines)

        # Build update clause (for single-value attributes)
        update_clause = ""
        if single_value_updates:
            update_lines = []
            for attr_name, value in single_value_updates.items():
                formatted_value = format_value(value)
                update_lines.append(f"{var_name} has {attr_name} {formatted_value};")
            update_clause = "\n".join(update_lines)

        return match_clause, delete_clause, insert_clause, update_clause

    def aggregate(self, *aggregates: Any) -> dict[str, Any]:
        """Execute aggregation queries.

        Performs database-side aggregations for efficiency.

        Args:
            *aggregates: AggregateExpr objects (Employment.salary.avg(), etc.)

        Returns:
            Dictionary mapping aggregate keys to results

        Examples:
            # Single aggregation
            result = manager.filter().aggregate(Employment.salary.avg())
            avg_salary = result['avg_salary']

            # Multiple aggregations
            result = manager.filter(Employment.position.eq(Position("Engineer"))).aggregate(
                Employment.salary.avg(),
                Employment.salary.sum(),
                Employment.salary.max()
            )
            avg_salary = result['avg_salary']
            total_salary = result['sum_salary']
            max_salary = result['max_salary']
        """
        from type_bridge.expressions import AggregateExpr

        if not aggregates:
            raise ValueError("At least one aggregation expression required")

        # Get all attributes (including inherited)
        all_attrs = self.model_class.get_all_attributes()

        # Separate attribute filters from role player filters
        attr_filters = {}
        role_player_filters = {}

        for key, value in self.filters.items():
            if key in self.model_class._roles:
                role_player_filters[key] = value
            elif key in all_attrs:
                attr_filters[key] = value
            else:
                raise ValueError(f"Unknown filter: {key}")

        # Build base match clause
        role_parts = []
        role_info = {}
        for role_name, role in self.model_class._roles.items():
            role_var = f"${role_name}"
            role_parts.append(f"{role.role_name}: {role_var}")
            role_info[role_name] = (role_var, role.player_entity_type)

        roles_str = ", ".join(role_parts)
        match_clauses = [f"$r isa {self.model_class.get_type_name()} ({roles_str})"]

        # Add dict-based attribute filters
        for field_name, value in attr_filters.items():
            attr_info = all_attrs[field_name]
            attr_name = attr_info.typ.get_attribute_name()
            formatted_value = format_value(value)
            match_clauses.append(f"$r has {attr_name} {formatted_value}")

        # Add role player filter clauses
        for role_name, player_entity in role_player_filters.items():
            role_var = f"${role_name}"
            entity_class = role_info[role_name][1]

            player_owned_attrs = entity_class.get_all_attributes()
            for field_name, attr_info in player_owned_attrs.items():
                if attr_info.flags.is_key:
                    key_value = getattr(player_entity, field_name, None)
                    if key_value is not None:
                        attr_name = attr_info.typ.get_attribute_name()
                        if hasattr(key_value, "value"):
                            key_value = key_value.value
                        formatted_value = format_value(key_value)
                        match_clauses.append(f"{role_var} has {attr_name} {formatted_value}")
                        break

        # Apply expression-based filters
        for expr in self._expressions:
            pattern = expr.to_typeql("$r")
            match_clauses.append(pattern)

        match_clause = ";\n".join(match_clauses) + ";"

        # Build reduce query with aggregations
        reduce_clauses = []
        for agg in aggregates:
            if not isinstance(agg, AggregateExpr):
                raise TypeError(f"Expected AggregateExpr, got {type(agg).__name__}")

            # If this aggregation is on a specific attr_type (not count), add binding pattern
            if agg.attr_type is not None:
                attr_name = agg.attr_type.get_attribute_name()
                attr_var = f"${attr_name.lower()}"
                match_clause = match_clause.rstrip(";")
                match_clause += f";\n$r has {attr_name} {attr_var};"

            # Generate reduce clause: $result_var = function($var)
            result_var = f"${agg.get_fetch_key()}"
            reduce_clauses.append(f"{result_var} = {agg.to_typeql('$r')}")

        # Convert match to reduce query
        reduce_query = f"match\n{match_clause}\nreduce {', '.join(reduce_clauses)};"

        results = self._execute(reduce_query, TransactionType.READ)

        # Parse aggregation results
        if not results:
            return {}

        result = results[0] if results else {}

        # TypeDB reduce returns results as a formatted string in 'result' key
        import re

        output = {}
        if "result" in result:
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

        return output

    def _execute(self, query: str, tx_type: TransactionType) -> list[dict[str, Any]]:
        """Execute a query using an existing transaction if available."""
        return self._executor.execute(query, tx_type)

    def group_by(self, *fields: Any) -> "RelationGroupByQuery[R]":
        """Group relations by field values.

        Args:
            *fields: FieldRef objects to group by

        Returns:
            RelationGroupByQuery for chained aggregations

        Example:
            result = manager.group_by(Employment.position).aggregate(Employment.salary.avg())
        """
        # Import here to avoid circular dependency
        from .group_by import RelationGroupByQuery

        return RelationGroupByQuery(
            self._connection,
            self.model_class,
            self.filters,
            self._expressions,
            fields,
        )
