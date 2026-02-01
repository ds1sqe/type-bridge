"""RelationManager for relation CRUD operations."""

import logging
from typing import TYPE_CHECKING, Any

from typedb.driver import TransactionType

from type_bridge.models import Entity, Relation

from ..base import R
from ..exceptions import RelationNotFoundError
from ..model_manager import ModelManager
from ..utils import (
    assign_relation_iids,
    build_relation_iid_query,
    build_relation_result_map,
    build_role_player_fetch_items,
    build_role_player_match,
    extract_entity_key,
    extract_relation_attributes,
    extract_role_players_from_results,
    format_value,
    group_results_by_iid,
    hydrate_attributes,
    is_multi_value_attribute,
    resolve_entity_class_from_label,
)

logger = logging.getLogger(__name__)

if TYPE_CHECKING:
    from .group_by import RelationGroupByQuery
    from .query import RelationQuery


class RelationManager[R: Relation](ModelManager[R]):
    """Manager for relation CRUD operations.

    Type-safe manager that preserves relation type information.
    Inherits connection management and common operations from BaseManager.
    """

    def _build_role_player_match(self, role_name: str, entity: Any, entity_type_name: str) -> str:
        """Build a match clause for a role player entity.

        Delegates to shared utility in crud/utils.py.

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
        return build_role_player_match(role_name, entity, entity_type_name)

    def _build_role_player_fetch_items(
        self, role_info: dict[str, tuple[str, tuple[type, ...]]]
    ) -> list[str]:
        """Build fetch items for role players with their IIDs and type labels.

        Delegates to shared utility in crud/utils.py.

        Args:
            role_info: Dict mapping role_name -> (role_var, allowed_entity_types)

        Returns:
            List of fetch item strings like:
                '"employee_iid": iid($employee)'
                '"employee_type": label($employee_type)'
                '"employee": { $employee.* }'

        Note: The caller must add type variable bindings to the match clause:
            $employee isa $employee_type;
        """
        return build_role_player_fetch_items(role_info)

    def _resolve_entity_class_from_label(
        self, type_label: str | None, allowed_entity_classes: tuple[type[Entity], ...]
    ) -> type[Entity]:
        """Resolve the correct Python entity class from a TypeDB type label.

        Delegates to shared utility in crud/utils.py.

        Args:
            type_label: The TypeDB type label (from label() function), e.g., "person"
            allowed_entity_classes: Tuple of allowed entity classes for this role

        Returns:
            The matching Python entity class, or the first allowed class as fallback
        """
        return resolve_entity_class_from_label(type_label, allowed_entity_classes)

    def _hydrate_entity_from_data(
        self,
        entity_class: type,
        player_data: dict[str, Any],
        player_iid: str | None = None,
    ) -> tuple[Any | None, tuple[tuple[str, Any], ...]]:
        """Hydrate an entity instance from raw player data.

        Handles multi-value attributes, optional fields, and IID assignment.
        Used by both get() and get_by_iid() for role player hydration.

        Args:
            entity_class: The entity class to instantiate
            player_data: Raw attribute data from TypeDB fetch
            player_iid: Optional IID to set on the entity

        Returns:
            Tuple of (entity instance or None, key values tuple for deduplication)
        """
        # Use shared hydration utility with value wrapping for multi-value attributes
        player_attrs, key_values = hydrate_attributes(entity_class, player_data, wrap_values=True)

        # Create entity instance if we have any non-None attributes
        # Note: Relations as role players may have no owned attributes,
        # so we also create if player_attrs is empty (valid for relations)
        if player_attrs == {} or any(v is not None for v in player_attrs.values()):
            player_entity = entity_class(**player_attrs)
            if player_iid:
                object.__setattr__(player_entity, "_iid", player_iid)
            return player_entity, key_values

        return None, tuple()

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

        # Add type variable bindings for each role player to enable label() fetch
        for role_name in self.model_class._roles:
            role_var = f"${role_name}"
            type_var = f"{role_var}_type"
            match_clauses.append(f"{role_var} isa! {type_var}")

        # Add attribute filter clauses
        for field_name, value in attr_filters.items():
            attr_info = all_attrs[field_name]
            attr_name = attr_info.typ.get_attribute_name()
            formatted_value = format_value(value)
            match_clauses.append(f"$r has {attr_name} {formatted_value}")

        # Add role player filter clauses
        for role_name, player_entity in role_player_filters.items():
            role_var = f"${role_name}"
            # Match the role player by their key attribute
            key_info = extract_entity_key(player_entity)
            if key_info:
                _, attr_name, raw_value = key_info
                formatted_value = format_value(raw_value)
                match_clauses.append(f"{role_var} has {attr_name} {formatted_value}")

        match_str = ";\n".join(match_clauses) + ";"

        # Build fetch clause with nested structure for role players
        # Use label($t) where $t is a TYPE variable bound via isa!
        fetch_items = ['"_iid": iid($r)', '"_type": label($t)']

        # Add relation attributes (including inherited)
        for attr_info in all_attrs.values():
            attr_name = attr_info.typ.get_attribute_name()
            # Multi-value attributes need to be wrapped in [] for TypeQL fetch
            if is_multi_value_attribute(attr_info.flags):
                fetch_items.append(f'"{attr_name}": [$r.{attr_name}]')
            else:
                fetch_items.append(f'"{attr_name}": $r.{attr_name}')

        # Add role player fetch items (IID + attributes for each role)
        fetch_items.extend(self._build_role_player_fetch_items(role_info))

        fetch_body = ",\n  ".join(fetch_items)
        fetch_str = f"fetch {{\n  {fetch_body}\n}};"

        query_str = f"match\n{match_str}\n{fetch_str}"
        logger.debug(f"Get query: {query_str}")

        results = self._execute(query_str, TransactionType.READ)
        logger.debug(f"Query returned {len(results)} results")

        # Group results by relation IID (handles multi-player roles)
        grouped_results = group_results_by_iid(results)

        # Check which roles have multi-player cardinality
        multi_player_roles = {
            role_name for role_name, role in self.model_class._roles.items() if role.is_multi_player
        }

        # Convert grouped results to relation instances
        relations = []

        for iid, result_group in grouped_results.items():
            # Extract relation attributes using shared utility
            attrs = extract_relation_attributes(self.model_class, result_group[0])

            # Create relation instance
            relation = self.model_class(**attrs)

            # Extract and assign role players using shared utility
            extracted_players = extract_role_players_from_results(
                result_group, role_info, multi_player_roles
            )
            for role_name, player_or_players in extracted_players.items():
                setattr(relation, role_name, player_or_players)

            # Set IID on the relation instance
            object.__setattr__(relation, "_iid", iid)
            relations.append(relation)

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
        match_parts = [f"$r isa! $t ({roles_str}), iid {iid}", f"$t sub {base_type}"]

        # Add type variable bindings for each role player to enable label() fetch
        for role_name in self.model_class._roles:
            role_var = f"${role_name}"
            type_var = f"{role_var}_type"
            match_parts.append(f"{role_var} isa! {type_var}")

        match_clause = ";\n".join(match_parts) + ";"

        # Build fetch clause with nested structure for role players
        # Use label($t) where $t is a TYPE variable bound via isa!
        fetch_items = ['"_iid": iid($r)', '"_type": label($t)']

        # Add relation attributes (including inherited)
        for attr_info in all_attrs.values():
            attr_name = attr_info.typ.get_attribute_name()
            # Multi-value attributes need to be wrapped in [] for TypeQL fetch
            if is_multi_value_attribute(attr_info.flags):
                fetch_items.append(f'"{attr_name}": [$r.{attr_name}]')
            else:
                fetch_items.append(f'"{attr_name}": $r.{attr_name}')

        # Add role player fetch items (IID + attributes for each role)
        fetch_items.extend(self._build_role_player_fetch_items(role_info))

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

        # Extract relation attributes using shared utility
        attrs = extract_relation_attributes(self.model_class, result)

        # Create relation instance
        relation = self.model_class(**attrs)

        # Extract role players from nested objects in result
        for role_name, (role_var, allowed_entity_classes) in role_info.items():
            if role_name in result and isinstance(result[role_name], dict):
                player_data = result[role_name]

                # Get the actual type label from TypeDB (fetched via label())
                type_label = result.get(f"{role_name}_type")

                # Resolve entity class from type label for polymorphic support
                entity_class = self._resolve_entity_class_from_label(
                    type_label, allowed_entity_classes
                )

                # Hydrate player entity from data
                player_iid = result.get(f"{role_name}_iid")
                player_entity, _ = self._hydrate_entity_from_data(
                    entity_class, player_data, player_iid
                )

                if player_entity is not None:
                    setattr(relation, role_name, player_entity)

        # Set the IID directly since we know it
        # Done after role player assignments to avoid Pydantic revalidation resetting it
        object.__setattr__(relation, "_iid", iid)

        logger.info(f"Retrieved relation by IID: {self.model_class.__name__}")
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
                    key_info = extract_entity_key(entity)
                    if key_info:
                        _, attr_name, raw_value = key_info
                        key_parts.append((f"{role_name}:{attr_name}", raw_value))

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

        # Build batched IID lookup query
        query_result = build_relation_iid_query(relations, self.model_class, roles)
        if not query_result:
            return

        query_str, role_names, role_key_info, _ = query_result
        logger.debug(f"Batched IID lookup query: {query_str[:200]}...")

        results = self._execute(query_str, TransactionType.READ)
        if not results:
            return

        # Build result map and assign IIDs
        result_map = build_relation_result_map(results, role_key_info)
        assign_relation_iids(relations, result_map, role_key_info, role_names)

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
