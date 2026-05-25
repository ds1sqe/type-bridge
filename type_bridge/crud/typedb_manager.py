"""Unified TypeDB Manager using AST and Strategies.

This module provides TypeDBManager, a unified CRUD manager that replaces
the separate EntityManager and RelationManager with a single generic
implementation using the Strategy pattern.
"""

from __future__ import annotations

import logging
from typing import TYPE_CHECKING, Any, Self, cast

from typedb.driver import TransactionType

from type_bridge.crud.formatting import format_value
from type_bridge.crud.hooks import CrudEvent, HookRunner
from type_bridge.crud.strategies import EntityStrategy, ModelStrategy, RelationStrategy
from type_bridge.crud.types import is_multi_value_attribute
from type_bridge.models import Entity, Relation
from type_bridge.query.ast import (
    DeleteClause,
    DeleteThingStatement,
    FetchAttribute,
    FetchAttributeList,
    FetchClause,
    FetchFunction,
    FetchItem,
    FetchNestedWildcard,
    FunctionCallValue,
    MatchClause,
    Pattern,
    ReduceAssignment,
    ReduceClause,
)
from type_bridge.query.compiler import QueryCompiler
from type_bridge.session import Connection, ConnectionExecutor

if TYPE_CHECKING:
    from type_bridge.models.base import TypeDBType

logger = logging.getLogger(__name__)


class TypeDBManager[T: "TypeDBType"]:
    """Unified CRUD manager for TypeDB entities and relations."""

    def __init__(self, connection: Connection, model_class: type[T]):
        self._connection = connection
        self._executor = ConnectionExecutor(connection)
        self.model_class = model_class
        self.compiler = QueryCompiler()

        self._hook_runner = HookRunner()

        # Select strategy
        if issubclass(model_class, Entity):
            self.strategy: ModelStrategy = EntityStrategy()
        elif issubclass(model_class, Relation):
            self.strategy = RelationStrategy()
        else:
            raise TypeError(f"Unsupported model type: {model_class}")

    def add_hook(self, hook: Any) -> Self:
        """Register a lifecycle hook. Returns self for chaining."""
        self._hook_runner.add(hook)
        return self

    def remove_hook(self, hook: Any) -> None:
        """Unregister a lifecycle hook."""
        self._hook_runner.remove(hook)

    def _execute(self, query: str, tx_type: TransactionType) -> list[dict[str, Any]]:
        return self._executor.execute(query, tx_type)

    def _build_fetch_items(
        self,
        var: str,
        attrs: dict[str, Any],
        *,
        include_iid: bool = True,
        include_type: bool = True,
    ) -> list[FetchAttribute | FetchAttributeList | FetchFunction]:
        """Build typed fetch items for a query.

        Args:
            var: The variable to fetch from (e.g., "$ent")
            attrs: Dict of attribute info from get_all_attributes()
            include_iid: Whether to include _iid fetch
            include_type: Whether to include _type fetch (requires $t type variable)

        Returns:
            List of typed FetchItem nodes
        """
        items: list[FetchAttribute | FetchAttributeList | FetchFunction] = []

        if include_iid:
            items.append(FetchFunction(key="_iid", func_name="iid", var=var))

        if include_type:
            items.append(FetchFunction(key="_type", func_name="label", var="$t"))

        for _field_name, attr_info in attrs.items():
            attr_name = attr_info.typ.get_attribute_name()
            if is_multi_value_attribute(attr_info.flags):
                items.append(FetchAttributeList(key=attr_name, var=var, attr_name=attr_name))
            else:
                items.append(FetchAttribute(key=attr_name, var=var, attr_name=attr_name))

        return items

    def _compile_fetch(
        self, items: list[FetchAttribute | FetchAttributeList | FetchFunction]
    ) -> str:
        """Compile fetch items to a fetch clause string."""
        return self.compiler.compile(FetchClause(items=cast(list[FetchItem | str], items)))

    def _build_wildcard_fetch(
        self,
        var: str,
        *,
        include_iid: bool = True,
        include_type: bool = True,
    ) -> str:
        """Build a fetch clause using wildcard syntax for all attributes.

        Uses the nested wildcard syntax to fetch all attributes including
        subtype-specific ones, combined with iid() and label() functions.

        Generates: fetch {
          "_iid": iid($var),
          "_type": label($t),
          "attributes": { $var.* }
        };

        Args:
            var: The variable to fetch from (e.g., "$ent")
            include_iid: Whether to include _iid fetch
            include_type: Whether to include _type fetch (requires $t type variable)

        Returns:
            Compiled fetch clause string
        """
        items: list[FetchItem | str] = []

        if include_iid:
            items.append(FetchFunction(key="_iid", func_name="iid", var=var))

        if include_type:
            items.append(FetchFunction(key="_type", func_name="label", var="$t"))

        # Use nested wildcard to fetch all attributes including subtype-specific ones
        items.append(FetchNestedWildcard(key="attributes", var=var))

        return self.compiler.compile(FetchClause(items=items))

    def _build_count_reduce(self, var: str, result_var: str = "$count") -> str:
        """Build a reduce count clause string.

        Args:
            var: Variable to count (e.g., "$e")
            result_var: Result variable name (e.g., "$count")

        Returns:
            Compiled reduce clause like "reduce $count = count($e);"
        """
        reduce_clause = ReduceClause(
            assignments=[
                ReduceAssignment(
                    variable=result_var,
                    expression=FunctionCallValue(function="count", args=[var]),
                )
            ]
        )
        return self.compiler.compile(reduce_clause)

    def _build_iid_fetch(self, var: str) -> str:
        """Build a fetch clause that only fetches the IID.

        Args:
            var: Variable to fetch IID from (e.g., "$e")

        Returns:
            Compiled fetch clause like 'fetch { "iid": iid($e) };'
        """
        fetch_items: list[FetchAttribute | FetchAttributeList | FetchFunction] = [
            FetchFunction(key="iid", func_name="iid", var=var)
        ]
        return self._compile_fetch(fetch_items)

    def _extract_iid_from_result(self, results: list[dict[str, Any]]) -> str | None:
        """Extract IID value from query results.

        Args:
            results: Query results from execute()

        Returns:
            IID string if found, None otherwise
        """
        if not results or len(results) == 0:
            return None
        iid_result = results[0].get("iid")
        # Handle wrapped IID format from TypeDB driver
        if isinstance(iid_result, dict) and "value" in iid_result:
            iid_result = iid_result["value"]
        if isinstance(iid_result, str):
            return iid_result
        return None

    def _execute_insert_with_iid(
        self, instance: T, var: str, *, use_put: bool = False
    ) -> str | None:
        """Execute insert/put and fetch IID in a single query.

        Combines insert (or put) with fetch in one roundtrip using TypeDB 3.x
        query pipelining. After insert, the variable is bound to the new instance
        so we can directly fetch its IID.

        Args:
            instance: Instance to insert
            var: Variable name (e.g., "$x")
            use_put: If True, use 'put' instead of 'insert' for idempotent operation

        Returns:
            IID string if successful, None otherwise
        """
        match_clause, insert_clause = self.strategy.build_insert(instance, var)

        query_parts = []
        if match_clause:
            query_parts.append(self.compiler.compile(match_clause))

        # Compile insert clause
        insert_query = self.compiler.compile(insert_clause)
        if use_put:
            insert_query = insert_query.replace("insert\n", "put\n", 1)
        query_parts.append(insert_query)

        # Append fetch for IID - variable is already bound after insert
        query_parts.append(self._build_iid_fetch(var))

        query = "\n".join(query_parts)

        # Execute combined insert+fetch in single WRITE transaction
        results = self._execute(query, TransactionType.WRITE)
        return self._extract_iid_from_result(results)

    def _collect_all_subclass_attributes(self, model_class: type[T]) -> dict[str, bool]:
        """Collect all attributes from a class and all its subclasses.

        For polymorphic queries, we need to fetch attributes from all possible
        concrete subtypes, not just the queried supertype.

        Args:
            model_class: The base class to start from

        Returns:
            Dict mapping attribute names to is_multi_value flags
        """
        collected: dict[str, bool] = {}

        def collect_from_class(cls: type[TypeDBType]) -> None:
            """Recursively collect attributes from class and its subclasses."""
            # Skip base classes that don't define types
            if hasattr(cls, "get_all_attributes"):
                for field_name, attr_info in cls.get_all_attributes().items():
                    attr_name = attr_info.typ.get_attribute_name()
                    is_multi = is_multi_value_attribute(attr_info.flags)
                    # Only update if not seen or if multi-value (to not lose that info)
                    if attr_name not in collected:
                        collected[attr_name] = is_multi

            # Recurse into subclasses
            for subclass in cls.__subclasses__():
                collect_from_class(subclass)

        collect_from_class(model_class)
        return collected

    def insert(self, instance: T) -> T:
        """Insert a new instance and populate _iid.

        For entities, uses a single roundtrip (insert + fetch combined).
        For relations, uses two roundtrips (insert, then fetch) because
        TypeDB 3.x relation inserts don't bind the variable.
        """
        if self._hook_runner.has_hooks:
            self._hook_runner.run_pre(CrudEvent.PRE_INSERT, self.model_class, instance)

        var = "$x"

        # Relations use include_variable=False in to_ast(), so $x isn't bound
        # after insert. Use the two-query approach for relations.
        if isinstance(instance, Relation):
            match_clause, insert_clause = self.strategy.build_insert(instance, var)
            query_parts = []
            if match_clause:
                query_parts.append(self.compiler.compile(match_clause))
            query_parts.append(self.compiler.compile(insert_clause))
            self._execute("\n".join(query_parts), TransactionType.WRITE)
            self._fetch_and_set_iid(instance, var)

            if self._hook_runner.has_hooks:
                self._hook_runner.run_post(CrudEvent.POST_INSERT, self.model_class, instance)
            return instance

        # Entities: Combined insert + fetch IID in single query
        iid = self._execute_insert_with_iid(instance, var, use_put=False)
        if iid:
            object.__setattr__(instance, "_iid", iid)
            logger.debug(f"Set _iid on instance: {iid}")
        else:
            # Fallback to separate fetch (for edge cases like types without keys)
            self._fetch_and_set_iid(instance, var)

        if self._hook_runner.has_hooks:
            self._hook_runner.run_post(CrudEvent.POST_INSERT, self.model_class, instance)

        return instance

    def _fetch_and_set_iid(self, instance: T, var: str) -> None:
        """Fetch and set _iid on an instance after insert/put.

        Tries key-based matching first, then falls back to attribute-based matching.
        """
        try:
            # Build AST-based match + fetch query
            patterns: list[Pattern]
            if isinstance(instance, Entity):
                patterns = [instance.get_match_pattern(var)]
            elif isinstance(instance, Relation):
                patterns = instance.get_match_patterns(var)
            else:
                raise TypeError(f"Unexpected instance type: {type(instance)}")

            match_clause = MatchClause(patterns=patterns)
            match_str = self.compiler.compile(match_clause)
            iid_fetch_str = self._build_iid_fetch(var)
            fetch_query = f"{match_str}\n{iid_fetch_str}"
            results = self._execute(fetch_query, TransactionType.READ)

            if results and len(results) > 0:
                iid_result = results[0].get("iid")
                # Handle wrapped IID format from TypeDB driver
                if isinstance(iid_result, dict) and "value" in iid_result:
                    iid_result = iid_result["value"]
                if iid_result:
                    object.__setattr__(instance, "_iid", iid_result)
                    logger.debug(f"Set _iid on instance: {iid_result}")
        except ValueError:
            # Fall back to attribute-based matching for entities without keys
            # This is less precise but allows IID retrieval when there's exactly one match
            self._try_fetch_iid_by_attributes(instance, var)

    def _try_fetch_iid_by_attributes(self, instance: T, var: str) -> None:
        """Try to fetch IID by matching all attributes (fallback for types without keys).

        This is used when a type has no @key and no _iid. For entities, it matches by type
        and all attribute values. For relations, it matches by role players and attributes.
        If exactly one instance matches, we populate _iid.
        """
        if issubclass(self.model_class, Relation):
            self._try_fetch_relation_iid(instance, var)
            return

        if not issubclass(self.model_class, Entity):
            logger.debug(f"Attribute-based IID fetch not supported for {self.model_class.__name__}")
            return

        type_name = self.model_class.get_type_name()
        all_attrs = self.model_class.get_all_attributes()

        # Build constraints
        from type_bridge.crud.patterns import _get_literal_type
        from type_bridge.query.ast import (
            EntityPattern,
            HasConstraint,
            LiteralValue,
            MatchClause,
        )

        constraints = []
        has_attrs = False

        for field_name, attr_info in all_attrs.items():
            value = getattr(instance, field_name, None)
            if value is not None:
                attr_name = attr_info.typ.get_attribute_name()
                # Handle lists (multi-value attributes)
                if isinstance(value, list):
                    for item in value:
                        raw_val = item.value if hasattr(item, "value") else item
                        constraints.append(
                            HasConstraint(
                                attr_name=attr_name,
                                value=LiteralValue(
                                    value=raw_val, value_type=_get_literal_type(raw_val)
                                ),
                            )
                        )
                        has_attrs = True
                else:
                    raw_val = value.value if hasattr(value, "value") else value
                    constraints.append(
                        HasConstraint(
                            attr_name=attr_name,
                            value=LiteralValue(
                                value=raw_val, value_type=_get_literal_type(raw_val)
                            ),
                        )
                    )
                    has_attrs = True

        if not has_attrs:
            logger.debug(f"No attributes to match for {self.model_class.__name__}")
            return

        # Build match clause
        pattern = EntityPattern(variable=var, type_name=type_name, constraints=constraints)
        match_clause = MatchClause(patterns=[pattern])
        match_str = self.compiler.compile(match_clause)

        iid_fetch_str = self._build_iid_fetch(var)
        fetch_query = f"{match_str}\n{iid_fetch_str}"

        try:
            results = self._execute(fetch_query, TransactionType.READ)
            if results and len(results) == 1:
                iid_result = results[0].get("iid")
                if isinstance(iid_result, dict) and "value" in iid_result:
                    iid_result = iid_result["value"]
                if iid_result:
                    object.__setattr__(instance, "_iid", iid_result)
                    logger.debug(f"Set _iid via attribute matching: {iid_result}")
            elif results and len(results) > 1:
                logger.debug(
                    f"Multiple matches for {self.model_class.__name__} - "
                    f"cannot uniquely identify for IID population"
                )
        except Exception as e:
            logger.debug(f"Failed to fetch IID by attributes: {e}")

    def _try_fetch_relation_iid(self, instance: T, var: str) -> None:
        """Try to fetch IID for a relation by matching role players and attributes.

        Uses the relation's role players (by their IIDs) and attributes to find
        a unique match. If exactly one relation matches, we populate _iid.
        """
        type_name = self.model_class.get_type_name()
        roles = getattr(self.model_class, "_roles", {})
        all_attrs = self.model_class.get_all_attributes()

        # Build role players and nested match patterns
        from type_bridge.crud.patterns import _get_literal_type
        from type_bridge.query.ast import (
            Constraint,
            EntityPattern,
            HasConstraint,
            IidConstraint,
            LiteralValue,
            MatchClause,
            RelationPattern,
            RolePlayer,
        )

        role_players: list[RolePlayer] = []
        nested_patterns: list[EntityPattern] = []
        role_var_counter = 0

        for role_name, role in roles.items():
            entity_or_list = getattr(instance, role_name, None)
            if entity_or_list is not None:
                entities = entity_or_list if isinstance(entity_or_list, list) else [entity_or_list]
                for entity in entities:
                    role_var = f"$rp{role_var_counter}"
                    role_var_counter += 1
                    role_players.append(RolePlayer(role=role.role_name, player_var=role_var))

                    # Match role player by IID if available
                    if entity._iid:
                        nested_patterns.append(
                            EntityPattern(
                                variable=role_var,
                                type_name=entity.get_type_name(),
                                constraints=[IidConstraint(iid=entity._iid)],
                            )
                        )
                    else:
                        # Fall back to key attributes
                        key_constraints: list[Constraint] = []
                        for field_name, attr_info in entity.get_all_attributes().items():
                            if attr_info.flags.is_key:
                                value = getattr(entity, field_name, None)
                                if value is not None:
                                    attr_name = attr_info.typ.get_attribute_name()
                                    raw_val = value.value if hasattr(value, "value") else value
                                    key_constraints.append(
                                        HasConstraint(
                                            attr_name=attr_name,
                                            value=LiteralValue(
                                                value=raw_val, value_type=_get_literal_type(raw_val)
                                            ),
                                        )
                                    )
                        nested_patterns.append(
                            EntityPattern(
                                variable=role_var,
                                type_name=entity.get_type_name(),
                                constraints=key_constraints,
                            )
                        )

        if not role_players:
            logger.debug(f"No role players to match for {self.model_class.__name__}")
            return

        # Build attribute constraints for the relation itself
        rel_constraints: list[Constraint] = []
        for field_name, attr_info in all_attrs.items():
            value = getattr(instance, field_name, None)
            if value is not None:
                attr_name = attr_info.typ.get_attribute_name()
                if isinstance(value, list):
                    for item in value:
                        raw_val = item.value if hasattr(item, "value") else item
                        rel_constraints.append(
                            HasConstraint(
                                attr_name=attr_name,
                                value=LiteralValue(
                                    value=raw_val, value_type=_get_literal_type(raw_val)
                                ),
                            )
                        )
                else:
                    raw_val = value.value if hasattr(value, "value") else value
                    rel_constraints.append(
                        HasConstraint(
                            attr_name=attr_name,
                            value=LiteralValue(
                                value=raw_val, value_type=_get_literal_type(raw_val)
                            ),
                        )
                    )

        # Build the main relation pattern
        main_pattern = RelationPattern(
            variable=var,
            type_name=type_name,
            role_players=role_players,
            constraints=rel_constraints,
        )

        # Combine all patterns into a MatchClause
        all_patterns: list[Pattern] = [main_pattern, *nested_patterns]
        match_clause = MatchClause(patterns=all_patterns)
        match_str = self.compiler.compile(match_clause)

        iid_fetch_str = self._build_iid_fetch(var)
        fetch_query = f"{match_str}\n{iid_fetch_str}"

        try:
            results = self._execute(fetch_query, TransactionType.READ)
            if results and len(results) == 1:
                iid_result = results[0].get("iid")
                if isinstance(iid_result, dict) and "value" in iid_result:
                    iid_result = iid_result["value"]
                if iid_result:
                    object.__setattr__(instance, "_iid", iid_result)
                    logger.debug(f"Set relation _iid via role player matching: {iid_result}")
            elif results and len(results) > 1:
                logger.debug(
                    f"Multiple relations match for {self.model_class.__name__} - "
                    f"cannot uniquely identify for IID population"
                )
        except Exception as e:
            logger.debug(f"Failed to fetch relation IID: {e}")

    def get(self, **filters) -> list[T]:
        """Get instances matching filters."""
        from type_bridge.crud.role_players import resolve_entity_class_from_label
        from type_bridge.models.registry import ModelRegistry

        # Use descriptive variable name to avoid conflicts
        var = "$rel" if isinstance(self.strategy, RelationStrategy) else "$ent"

        # Check if this is a relation (needs special handling for role players)
        if isinstance(self.strategy, RelationStrategy):
            return self._get_relations(var, filters, [])

        # Entity path: fetch with polymorphic type resolution
        base_type = self.model_class.get_type_name()

        from type_bridge.query.ast import EntityPattern

        # Build match clause with isa! for type variable binding (enables polymorphic resolution)
        # This allows us to fetch the actual concrete type using label()
        match_clause: MatchClause = self.strategy.build_match_all(self.model_class, var, filters)

        # Modify the AST to use 'isa!' and capture type variable '$t'
        # We iterate through patterns to find the main entity pattern
        for pattern in match_clause.patterns:
            if (
                isinstance(pattern, EntityPattern)
                and pattern.variable == var
                and pattern.type_name == base_type
            ):
                # We found the main pattern.
                # Transform to: $ent isa! $t
                # And add: $t sub base_type

                pattern.type_name = "$t"
                pattern.is_strict = True

                # Add sub constraint as a new pattern
                from type_bridge.query.ast import SubTypePattern

                match_clause.patterns.append(SubTypePattern(variable="$t", parent_type=base_type))
                break

        match_str = self.compiler.compile(match_clause)

        # Build fetch clause using wildcard to get all attributes including subtype-specific ones
        fetch_clause_str = self._build_wildcard_fetch(var, include_iid=True, include_type=True)
        query = match_str + "\n" + fetch_clause_str

        results = self._execute(query, TransactionType.READ)

        # Hydrate entity instances with polymorphic type resolution
        instances = []
        for result in results:
            try:
                iid = result.pop("_iid", None)
                if isinstance(iid, dict) and "value" in iid:
                    iid = iid["value"]

                type_label = result.pop("_type", None)

                # Extract attributes from nested "attributes" key (wildcard fetch structure)
                attrs = result.pop("attributes", result)

                if type_label and type_label != base_type:
                    concrete_class = ModelRegistry.get(type_label)
                    if concrete_class is None:
                        concrete_class = resolve_entity_class_from_label(
                            type_label,
                            cast(tuple[type[Entity], ...], (self.model_class,)),
                        )
                else:
                    concrete_class = self.model_class

                assert concrete_class is not None, "Failed to resolve concrete class"
                entity_class = cast(type[Entity], concrete_class)
                instance = entity_class.from_dict(attrs, strict=False)
                if iid:
                    object.__setattr__(instance, "_iid", iid)
                instances.append(instance)
            except Exception as e:
                from type_bridge.crud.exceptions import HydrationError

                raise HydrationError(
                    model_type=self.model_class.__name__,
                    raw_data=result,
                    cause=e,
                ) from e

        return instances

    def _get_relations(
        self,
        var: str,
        filters: dict[str, Any],
        expressions: list[Any],
        order_by: list[tuple[str, bool]] | None = None,
        limit: int | None = None,
        offset: int | None = None,
    ) -> list[T]:
        """Get relations with role player hydration.

        Handles multi-cardinality roles by grouping results by IID and merging
        role players into lists where appropriate.
        """
        from type_bridge.crud.role_lookup import parse_role_lookup_filters
        from type_bridge.crud.role_players import (
            extract_relation_attributes,
            extract_role_players_from_results,
            group_results_by_iid,
        )

        # Extract special _iid filter if present
        iid_filter = None
        if filters and "_iid" in filters:
            iid_filter = filters.pop("_iid")

        # Parse Django-style filters (e.g., employer__industry__eq="Technology")
        # This extracts role player attribute lookups into expressions
        all_expressions = list(expressions)  # Copy to avoid mutating input
        relation_model = cast(type[Relation], self.model_class)
        if filters:
            attr_filters, role_player_filters, role_expressions, attr_expressions = (
                parse_role_lookup_filters(relation_model, filters)
            )
            # Merge parsed filters: direct attrs and entity instances
            parsed_filters = {**attr_filters, **role_player_filters}
            # Add role player expressions (these are RolePlayerExpr)
            for exprs in role_expressions.values():
                all_expressions.extend(exprs)
            # Add relation attribute expressions
            all_expressions.extend(attr_expressions)
        else:
            parsed_filters = {}

        # Build and execute query with modifiers
        assert isinstance(self.strategy, RelationStrategy), "Expected RelationStrategy"
        query = self.strategy.build_fetch_query(
            relation_model, var, parsed_filters, all_expressions, order_by, limit, offset
        )

        # Add IID filter to match clause if specified
        if iid_filter:
            # Insert iid constraint into match clause
            query = query.replace(f"{var} isa!", f"{var} iid {iid_filter}, isa!")
        logger.debug(f"Relation fetch query: {query}")
        results = self._execute(query, TransactionType.READ)

        # Unwrap IID values in all results first
        for result in results:
            iid = result.get("_iid")
            if isinstance(iid, dict) and "value" in iid:
                result["_iid"] = iid["value"]
            # Also unwrap role player IIDs
            for key in list(result.keys()):
                if key.endswith("_iid"):
                    val = result[key]
                    if isinstance(val, dict) and "value" in val:
                        result[key] = val["value"]

        # Group results by relation IID to handle multi-cardinality roles
        grouped = group_results_by_iid(results)

        # Build role info for the extraction utilities
        roles = getattr(self.model_class, "_roles", {})
        role_info: dict[str, tuple[str, tuple[type, ...]]] = {}
        multi_player_roles: set[str] = set()

        for role_name, role in roles.items():
            role_var = f"${role_name}"
            role_info[role_name] = (role_var, role.player_entity_types)
            # Check if role allows multiple players (has explicit cardinality)
            if role.is_multi_player:
                multi_player_roles.add(role_name)

        # Hydrate relation instances
        instances = []
        for relation_iid, result_group in grouped.items():
            try:
                # Extract relation attributes from first result (all should have same attrs)
                relation_attrs = extract_relation_attributes(relation_model, result_group[0])

                # Extract and deduplicate role players from all results in group
                role_players = extract_role_players_from_results(
                    result_group, role_info, multi_player_roles
                )

                # Build relation instance
                relation_kwargs = {**relation_attrs, **role_players}
                relation_instance = self.model_class(**relation_kwargs)
                object.__setattr__(relation_instance, "_iid", relation_iid)

                instances.append(relation_instance)
            except Exception as e:
                from type_bridge.crud.exceptions import HydrationError

                raise HydrationError(
                    model_type=self.model_class.__name__,
                    raw_data=result_group,
                    cause=e,
                ) from e

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
        if self._hook_runner.has_hooks:
            self._hook_runner.run_pre(CrudEvent.PRE_UPDATE, self.model_class, instance)

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

        if self._hook_runner.has_hooks:
            self._hook_runner.run_post(CrudEvent.POST_UPDATE, self.model_class, instance)

        return instance

    def delete(self, instance: T) -> T:
        """Delete an instance and return it."""
        if self._hook_runner.has_hooks:
            self._hook_runner.run_pre(CrudEvent.PRE_DELETE, self.model_class, instance)

        var = "$x"

        # Build AST-based match clause
        patterns: list[Pattern]
        if isinstance(instance, Entity):
            patterns = [instance.get_match_pattern(var)]
        elif isinstance(instance, Relation):
            patterns = instance.get_match_patterns(var)
        else:
            raise TypeError(f"Unexpected instance type: {type(instance)}")

        match_clause = MatchClause(patterns=patterns)
        delete_clause = DeleteClause(statements=[DeleteThingStatement(variable=var)])

        # Compile and execute
        match_str = self.compiler.compile(match_clause)
        delete_str = self.compiler.compile(delete_clause)
        query = f"{match_str}\n{delete_str}"

        self._execute(query, TransactionType.WRITE)

        if self._hook_runner.has_hooks:
            self._hook_runner.run_post(CrudEvent.POST_DELETE, self.model_class, instance)

        return instance

    def all(self) -> list[T]:
        """Fetch all instances of this type."""
        return self.get()

    def insert_many(self, instances: list[T]) -> list[T]:
        """Insert multiple instances in a single query (batched).

        For entities, combines all inserts into a single query for efficiency.
        For relations, falls back to individual inserts (due to match clause complexity).
        """
        if not instances:
            return instances

        # Check if all instances are entities (can be batched)
        # Relations need individual handling due to role player match clauses
        if all(isinstance(inst, Entity) for inst in instances):
            if self._hook_runner.has_hooks:
                for instance in instances:
                    self._hook_runner.run_pre(CrudEvent.PRE_INSERT, self.model_class, instance)

            result = self._batch_insert_entities(instances)

            if self._hook_runner.has_hooks:
                for instance in result:
                    self._hook_runner.run_post(CrudEvent.POST_INSERT, self.model_class, instance)

            return result

        # Fallback for relations or mixed types (hooks fire via self.insert)
        for instance in instances:
            self.insert(instance)
        return instances

    def _batch_insert_entities(self, instances: list[T], *, use_put: bool = False) -> list[T]:
        """Batch insert/put multiple entities in a single query.

        Combines all entity inserts into one query with a single fetch clause
        to retrieve all IIDs, reducing N roundtrips to 1.

        Args:
            instances: List of Entity instances to insert
            use_put: If True, use 'put' instead of 'insert' for idempotent operation

        Returns:
            The same list with _iid populated on each instance
        """
        from type_bridge.query.ast import FetchClause, FetchFunction, InsertClause

        if not instances:
            return instances

        # Build combined insert clause with unique variables
        all_statements = []
        var_to_instance: dict[str, T] = {}

        for i, instance in enumerate(instances):
            var = f"$x{i}"
            var_to_instance[var] = instance

            # Get insert clause for this instance
            insert_clause = instance.to_ast(var)
            all_statements.extend(insert_clause.statements)

        # Create combined insert clause
        combined_insert = InsertClause(statements=all_statements)
        query_str = self.compiler.compile(combined_insert)

        # Convert to 'put' if needed
        if use_put:
            query_str = query_str.replace("insert\n", "put\n", 1)

        # Build fetch clause for all IIDs
        fetch_items: list[FetchFunction] = [
            FetchFunction(key=var, func_name="iid", var=var) for var in var_to_instance
        ]
        fetch_clause = FetchClause(items=cast(list, fetch_items))
        fetch_str = self.compiler.compile(fetch_clause)

        # Execute combined query
        query = f"{query_str}\n{fetch_str}"
        results = self._execute(query, TransactionType.WRITE)

        # Parse results and set IIDs on instances
        if results:
            result = results[0]  # All IIDs come back in a single result row
            for var, instance in var_to_instance.items():
                iid_value = result.get(var)
                if isinstance(iid_value, dict) and "value" in iid_value:
                    iid_value = iid_value["value"]
                if iid_value:
                    object.__setattr__(instance, "_iid", iid_value)
                    logger.debug(f"Set _iid on instance {var}: {iid_value}")

        return instances

    def put(self, instance: T) -> T:
        """Insert or update an instance (idempotent) and populate _iid.

        Uses TypeQL's PUT clause for idempotent insertion.
        For entities, uses a single roundtrip. For relations, uses two.
        """
        if self._hook_runner.has_hooks:
            self._hook_runner.run_pre(CrudEvent.PRE_PUT, self.model_class, instance)

        var = "$x"

        # Relations use include_variable=False in to_ast(), so $x isn't bound.
        # Use the two-query approach for relations.
        if isinstance(instance, Relation):
            match_clause, insert_clause = self.strategy.build_insert(instance, var)
            query_parts = []
            if match_clause:
                query_parts.append(self.compiler.compile(match_clause))
            insert_query = self.compiler.compile(insert_clause)
            put_query = insert_query.replace("insert\n", "put\n", 1)
            query_parts.append(put_query)
            self._execute("\n".join(query_parts), TransactionType.WRITE)
            self._fetch_and_set_iid(instance, var)

            if self._hook_runner.has_hooks:
                self._hook_runner.run_post(CrudEvent.POST_PUT, self.model_class, instance)
            return instance

        # Entities: Combined put + fetch IID in single query
        iid = self._execute_insert_with_iid(instance, var, use_put=True)
        if iid:
            object.__setattr__(instance, "_iid", iid)
            logger.debug(f"Set _iid on instance: {iid}")
        else:
            # Fallback to separate fetch (for edge cases like types without keys)
            self._fetch_and_set_iid(instance, var)

        if self._hook_runner.has_hooks:
            self._hook_runner.run_post(CrudEvent.POST_PUT, self.model_class, instance)

        return instance

    def delete_many(self, instances: list[T], *, strict: bool = False) -> list[T]:
        """Delete multiple instances.

        Optimized for batch deletion: instances with IIDs are deleted in a single
        query using disjunctive matching (OR pattern), reducing N roundtrips to 1.

        Args:
            instances: List of instances to delete
            strict: If True, raise EntityNotFoundError if any entity doesn't exist.
                   In strict mode, checks all entities first and raises before any deletion.

        Returns:
            List of actually-deleted entities (excludes those that didn't exist)
        """
        if not instances:
            return instances

        from type_bridge.crud.exceptions import EntityNotFoundError

        # Separate instances by whether they have IIDs
        with_iids: list[T] = []
        without_iids: list[T] = []

        for instance in instances:
            if getattr(instance, "_iid", None):
                with_iids.append(instance)
            else:
                without_iids.append(instance)

        has_hooks = self._hook_runner.has_hooks

        # For strict mode, we need to check existence before deleting
        if strict:
            # Batch check existence for instances with IIDs
            existing_iids = self._batch_check_existence_by_iid(with_iids) if with_iids else set()
            not_found_with_iid = [inst for inst in with_iids if inst._iid not in existing_iids]

            # Check existence individually for instances without IIDs
            not_found_without_iid = [inst for inst in without_iids if not self._entity_exists(inst)]

            not_found = not_found_with_iid + not_found_without_iid
            if not_found:
                names = [str(e) for e in not_found]
                raise EntityNotFoundError(f"entity(ies) not found: {names}")

            # All exist - proceed with batch delete
            deleted: list[T] = []
            if with_iids:
                if has_hooks:
                    for inst in with_iids:
                        self._hook_runner.run_pre(CrudEvent.PRE_DELETE, self.model_class, inst)
                self._batch_delete_by_iid(with_iids)
                if has_hooks:
                    for inst in with_iids:
                        self._hook_runner.run_post(CrudEvent.POST_DELETE, self.model_class, inst)
                deleted.extend(with_iids)
            for inst in without_iids:
                self.delete(inst)  # hooks fire inside self.delete()
                deleted.append(inst)
            return deleted

        # Non-strict mode: batch delete instances with IIDs, individual for others
        deleted = []

        if with_iids:
            # Batch check which ones exist
            existing_iids = self._batch_check_existence_by_iid(with_iids)
            existing_instances = [inst for inst in with_iids if inst._iid in existing_iids]

            if existing_instances:
                if has_hooks:
                    for inst in existing_instances:
                        self._hook_runner.run_pre(CrudEvent.PRE_DELETE, self.model_class, inst)
                self._batch_delete_by_iid(existing_instances)
                if has_hooks:
                    for inst in existing_instances:
                        self._hook_runner.run_post(CrudEvent.POST_DELETE, self.model_class, inst)
                deleted.extend(existing_instances)

        # Handle instances without IIDs individually (hooks fire via self.delete)
        for inst in without_iids:
            if self._entity_exists(inst):
                self.delete(inst)
                deleted.append(inst)

        return deleted

    def _batch_check_existence_by_iid(self, instances: list[T]) -> set[str]:
        """Batch check existence of instances by IID using disjunctive matching.

        Args:
            instances: List of instances (must have _iid set)

        Returns:
            Set of IIDs that exist in the database
        """
        from type_bridge.query.ast import (
            FetchClause,
            FetchFunction,
            IidPattern,
            MatchClause,
            OrPattern,
        )

        if not instances:
            return set()

        var = "$x"

        # Build OR pattern for all IIDs
        alternatives: list[list[Pattern]] = []
        for inst in instances:
            iid = getattr(inst, "_iid", None)
            if iid:
                alternatives.append([IidPattern(variable=var, iid=iid)])

        if not alternatives:
            return set()

        # If only one IID, no need for OR pattern
        if len(alternatives) == 1:
            match_clause = MatchClause(patterns=alternatives[0])
        else:
            or_pattern = OrPattern(alternatives=alternatives)
            match_clause = MatchClause(patterns=[or_pattern])

        # Fetch IIDs of matching instances
        match_str = self.compiler.compile(match_clause)
        fetch_items: list[FetchFunction] = [FetchFunction(key="iid", func_name="iid", var=var)]
        fetch_clause = FetchClause(items=cast(list, fetch_items))
        fetch_str = self.compiler.compile(fetch_clause)

        query = f"{match_str}\n{fetch_str}"
        results = self._execute(query, TransactionType.READ)

        # Extract IIDs from results
        existing_iids: set[str] = set()
        for result in results:
            iid = result.get("iid")
            if isinstance(iid, dict) and "value" in iid:
                iid = iid["value"]
            if isinstance(iid, str):
                existing_iids.add(iid)

        return existing_iids

    def _batch_delete_by_iid(self, instances: list[T]) -> None:
        """Batch delete instances using disjunctive IID matching.

        Generates a single query with OR pattern to match all instances by IID,
        then deletes them in one operation.

        Args:
            instances: List of instances to delete (must have _iid set)
        """
        from type_bridge.query.ast import IidPattern, OrPattern

        if not instances:
            return

        var = "$x"

        # Build OR pattern for all IIDs
        alternatives: list[list[Pattern]] = []
        for inst in instances:
            iid = getattr(inst, "_iid", None)
            if iid:
                alternatives.append([IidPattern(variable=var, iid=iid)])

        if not alternatives:
            return

        # Build match clause with OR pattern
        if len(alternatives) == 1:
            match_clause = MatchClause(patterns=alternatives[0])
        else:
            or_pattern = OrPattern(alternatives=alternatives)
            match_clause = MatchClause(patterns=[or_pattern])

        # Build delete clause
        delete_clause = DeleteClause(statements=[DeleteThingStatement(variable=var)])

        # Compile and execute
        match_str = self.compiler.compile(match_clause)
        delete_str = self.compiler.compile(delete_clause)
        query = f"{match_str}\n{delete_str}"

        logger.debug(f"Batch delete query: {query}")
        self._execute(query, TransactionType.WRITE)

    def _entity_exists(self, instance: T) -> bool:
        """Check if an entity exists in the database."""
        var = "$x"
        try:
            # Build AST-based match clause
            patterns: list[Pattern]
            if isinstance(instance, Entity):
                patterns = [instance.get_match_pattern(var)]
            elif isinstance(instance, Relation):
                patterns = instance.get_match_patterns(var)
            else:
                raise TypeError(f"Unexpected instance type: {type(instance)}")

            match_clause = MatchClause(patterns=patterns)
            match_str = self.compiler.compile(match_clause)
            reduce_str = self._build_count_reduce(var)
            query = f"{match_str}\n{reduce_str}"

            results = self._execute(query, TransactionType.READ)
            if results and "count" in results[0]:
                count_val = results[0]["count"]
                if isinstance(count_val, dict) and "value" in count_val:
                    count_val = count_val["value"]
                if isinstance(count_val, (int, float)):
                    return int(count_val) > 0
                return int(str(count_val)) > 0
        except ValueError:
            # Can't identify entity (no IID, no keys) - assume doesn't exist
            pass
        return False

    def put_many(self, instances: list[T]) -> list[T]:
        """Put multiple instances (idempotent insert/update).

        Attempts batch operation first for efficiency. If a key constraint
        violation occurs (some entities exist with different data), falls back
        to individual operations which are idempotent.

        For entities, uses batch PUT when possible (N→1 roundtrips).
        For relations, uses individual operations (match clause complexity).
        """
        if not instances:
            return instances

        has_hooks = self._hook_runner.has_hooks

        # Check if all instances are entities (can attempt batch)
        if all(isinstance(inst, Entity) for inst in instances):
            if has_hooks:
                for instance in instances:
                    self._hook_runner.run_pre(CrudEvent.PRE_PUT, self.model_class, instance)

            try:
                result = self._batch_insert_entities(instances, use_put=True)
            except Exception as e:
                # Check if this is a key constraint violation
                error_str = str(e)
                if "unique" in error_str.lower() or "constraint" in error_str.lower():
                    logger.debug(
                        f"Batch put failed with constraint violation, falling back to individual: {e}"
                    )
                    # Fall back to individual operations.
                    # Note: pre-hooks may fire again via self.put() — acceptable
                    # since the batch operation was rolled back.
                    for instance in instances:
                        self.put(instance)
                    return instances
                # Re-raise other errors
                raise

            if has_hooks:
                for instance in result:
                    self._hook_runner.run_post(CrudEvent.POST_PUT, self.model_class, instance)

            return result

        # Fallback for relations or mixed types (hooks fire via self.put)
        for instance in instances:
            self.put(instance)
        return instances

    def update_many(self, instances: list[T]) -> list[T]:
        """Update multiple instances."""
        for instance in instances:
            self.update(instance)
        return instances

    def get_by_iid(self, iid: str) -> T | None:
        """Fetch an instance by its internal ID with polymorphic type resolution."""
        import re

        from type_bridge.crud.role_players import resolve_entity_class_from_label
        from type_bridge.models.registry import ModelRegistry
        from type_bridge.query.ast import (
            EntityPattern,
            IidConstraint,
            MatchClause,
            SubTypePattern,
        )

        # Validate IID format (TypeDB IIDs are hexadecimal strings starting with 0x)
        # Return None for invalid IIDs (graceful handling - treat as "not found")
        if not iid or not re.match(r"^0x[0-9a-fA-F]+$", iid):
            return None

        var = "$x"
        base_type = self.model_class.get_type_name()

        if issubclass(self.model_class, Entity):
            # Entity path: Build match clause with isa! for polymorphic type resolution
            # $x isa! $t, iid <iid>; $t sub base_type;
            entity_pattern = EntityPattern(
                variable=var,
                type_name="$t",  # Type variable for polymorphic resolution
                constraints=[IidConstraint(iid=iid)],
                is_strict=True,  # Use isa! for strict type matching
            )
            subtype_pattern = SubTypePattern(variable="$t", parent_type=base_type)
            match_clause = MatchClause(patterns=[entity_pattern, subtype_pattern])
            match_str = self.compiler.compile(match_clause)

            # Build fetch clause using wildcard to get all attributes including subtype-specific ones
            # No IID needed in fetch - we already have it from input
            fetch_clause_str = self._build_wildcard_fetch(var, include_iid=False, include_type=True)
            query = match_str + "\n" + fetch_clause_str

            results = self._execute(query, TransactionType.READ)

            if not results:
                return None

            result = results[0]
            try:
                type_label = result.pop("_type", None)

                # Extract attributes from nested "attributes" key (wildcard fetch structure)
                attrs = result.pop("attributes", result)

                # Resolve concrete class
                if type_label and type_label != base_type:
                    concrete_class = ModelRegistry.get(type_label)
                    if concrete_class is None:
                        concrete_class = resolve_entity_class_from_label(
                            type_label,
                            cast(tuple[type[Entity], ...], (self.model_class,)),
                        )
                else:
                    concrete_class = self.model_class

                assert concrete_class is not None, "Failed to resolve concrete class"
                entity_class = cast(type[Entity], concrete_class)
                instance = entity_class.from_dict(attrs, strict=False)
                object.__setattr__(instance, "_iid", iid)
                return cast(T | None, instance)
            except Exception as e:
                from type_bridge.crud.exceptions import HydrationError

                raise HydrationError(
                    model_type=self.model_class.__name__,
                    raw_data=result,
                    cause=e,
                ) from e
        else:
            # Relation path: Use _get_relations with IID filter
            # This properly handles role player hydration
            results = self._get_relations(var, {"_iid": iid}, [])
            if results:
                return results[0]
            return None

    def filter(self, *expressions: Any, **filters: Any) -> TypeDBQuery[T]:
        """Create a chainable query with filters.

        Args:
            *expressions: Expression objects (Person.age.gt(Age(30)), etc.)
            **filters: Attribute filters (exact match) - age=30, name="Alice"

        Returns:
            TypeDBQuery for chaining

        Raises:
            ValueError: If expressions reference attribute types not owned by the model
        """
        # Validate expressions reference owned attribute types
        if expressions:
            owned_attrs = self.model_class.get_all_attributes()
            owned_attr_types = {attr_info.typ for attr_info in owned_attrs.values()}

            from type_bridge.expressions.role_player import RolePlayerExpr

            for expr in expressions:
                # Skip RolePlayerExpr - they reference player attributes, not relation attributes
                if isinstance(expr, RolePlayerExpr):
                    continue

                # Get attribute types from expression
                expr_attr_types = expr.get_attribute_types()

                # Check if all attribute types are owned by the model
                for attr_type in expr_attr_types:
                    if attr_type not in owned_attr_types:
                        raise ValueError(
                            f"{self.model_class.__name__} does not own attribute type {attr_type.__name__}. "
                            f"Available attribute types: {', '.join(t.__name__ for t in owned_attr_types)}"
                        )

        return TypeDBQuery(self, filters, list(expressions))

    def count(self, **filters) -> int:
        """Count all instances of this type, optionally filtering."""
        var = "$x"
        match_clause = self.strategy.build_match_all(self.model_class, var, filters)
        match_str = self.compiler.compile(match_clause)
        reduce_str = self._build_count_reduce(var)
        query = match_str + "\n" + reduce_str

        results = self._execute(query, TransactionType.READ)
        if results and "count" in results[0]:
            count_val = results[0]["count"]
            # Handle wrapped value format from TypeDB driver
            if isinstance(count_val, dict) and "value" in count_val:
                return int(count_val["value"])
            if isinstance(count_val, (int, float)):
                return int(count_val)
            return int(str(count_val))
        return 0

    def group_by(self, *fields: Any) -> GroupByQuery[T]:
        """Group results by field values and compute aggregations.

        Args:
            *fields: Field descriptors to group by (e.g., Person.department)

        Returns:
            GroupByQuery for chained aggregations

        Example:
            # Group by department, compute average age per department
            result = manager.group_by(Person.department).aggregate(Person.age.avg())
            # Returns: {
            #   "Engineering": {"avg_age": 35.5},
            #   "Sales": {"avg_age": 28.3}
            # }
        """
        return GroupByQuery(self, {}, [], fields)


class TypeDBQuery[T: "TypeDBType"]:
    """Chainable query builder for TypeDBManager."""

    def __init__(
        self,
        manager: TypeDBManager[T],
        filters: dict[str, Any],
        expressions: list[Any] | None = None,
    ):
        self._manager = manager
        self._filters = filters
        self._expressions: list[Any] = expressions or []
        self._order_fields: list[tuple[str, bool]] = []  # (field, descending)
        self._limit_val: int | None = None
        self._offset_val: int | None = None

    def filter(self, *expressions: Any, **filters: Any) -> TypeDBQuery[T]:
        """Add additional filters to the query.

        Validates that expression attribute types are owned by the model class.
        """
        # Validate expressions reference owned attribute types
        if expressions:
            model_class = self._manager.model_class
            owned_attrs = model_class.get_all_attributes()
            owned_attr_types = {attr_info.typ for attr_info in owned_attrs.values()}

            # For relations, also include role player attribute types
            # (RolePlayerExpr inner expressions will be validated separately)
            from type_bridge.expressions.role_player import RolePlayerExpr

            for expr in expressions:
                # Skip RolePlayerExpr - they reference player attributes, not relation attributes
                if isinstance(expr, RolePlayerExpr):
                    continue

                # Get attribute types from expression
                expr_attr_types = expr.get_attribute_types()

                # Check if all attribute types are owned by the model
                for attr_type in expr_attr_types:
                    if attr_type not in owned_attr_types:
                        raise ValueError(
                            f"{model_class.__name__} does not own attribute type {attr_type.__name__}. "
                            f"Available attribute types: {', '.join(t.__name__ for t in owned_attr_types)}"
                        )

        self._expressions.extend(expressions)
        self._filters.update(filters)
        return self

    def order_by(self, *fields: str) -> TypeDBQuery[T]:
        """Order results by fields. Prefix with '-' for descending."""
        for field in fields:
            if field.startswith("-"):
                self._order_fields.append((field[1:], True))
            else:
                self._order_fields.append((field, False))
        return self

    def limit(self, n: int) -> TypeDBQuery[T]:
        """Limit number of results."""
        self._limit_val = n
        return self

    def offset(self, n: int) -> TypeDBQuery[T]:
        """Skip first n results."""
        self._offset_val = n
        return self

    def execute(self) -> list[T]:
        """Execute the query and return results."""
        from type_bridge.crud.role_players import resolve_entity_class_from_label
        from type_bridge.models.registry import ModelRegistry

        model_class = self._manager.model_class

        # Relations need special handling for role player fetching
        if isinstance(self._manager.strategy, RelationStrategy):
            return self._manager._get_relations(
                "$r",
                self._filters,
                self._expressions,
                order_by=self._order_fields if self._order_fields else None,
                limit=self._limit_val,
                offset=self._offset_val,
            )

        # Entity path with polymorphic type resolution
        var = "$x"
        base_type = model_class.get_type_name()

        # Parse Django-style lookup filters (e.g., name__startswith="Al")
        base_filters, lookup_expressions = self._parse_entity_lookup_filters(
            model_class, self._filters
        )
        all_expressions = list(self._expressions) + lookup_expressions

        from type_bridge.query.ast import EntityPattern, SubTypePattern

        # Build base match clause using parsed filters (exact match only)
        match_clause = self._manager.strategy.build_match_all(model_class, var, base_filters)

        # Add expression patterns to match clause (includes parsed lookup expressions)
        if all_expressions:
            for expr in all_expressions:
                match_clause.patterns.extend(expr.to_ast(var))

        # Add type variable binding for polymorphic resolution
        # Transform "$x isa type" to "$x isa! $t; $t sub type"

        for pattern in match_clause.patterns:
            if (
                isinstance(pattern, EntityPattern)
                and pattern.variable == var
                and pattern.type_name == base_type
            ):
                pattern.type_name = "$t"
                pattern.is_strict = True
                match_clause.patterns.append(SubTypePattern(variable="$t", parent_type=base_type))
                break

        # Add sort variable bindings to match clause (TypeDB 3.x requirement)
        # These must be added to the match clause BEFORE compilation
        modifier_clauses = []
        sort_parts = []
        all_attrs = model_class.get_all_attributes()
        if self._order_fields:
            for i, (field_name, desc) in enumerate(self._order_fields):
                if field_name in all_attrs:
                    attr_name = all_attrs[field_name].typ.get_attribute_name()
                    sort_var = f"$sort_{i}"
                    # Bind attribute to variable in match clause
                    from type_bridge.query.ast import HasPattern

                    match_clause.patterns.append(
                        HasPattern(thing_var=var, attr_type=attr_name, attr_var=sort_var)
                    )
                    direction = "desc" if desc else "asc"
                    sort_parts.append(f"{sort_var} {direction}")
            if sort_parts:
                modifier_clauses.append(f"sort {', '.join(sort_parts)};")

        match_str = self._manager.compiler.compile(match_clause)

        # Build fetch clause using wildcard to get all attributes including subtype-specific ones
        fetch_clause_str = self._manager._build_wildcard_fetch(
            var, include_iid=True, include_type=True
        )

        # offset must come BEFORE limit
        if self._offset_val is not None:
            modifier_clauses.append(f"offset {self._offset_val};")
        if self._limit_val is not None:
            modifier_clauses.append(f"limit {self._limit_val};")

        modifier_str = "\n".join(modifier_clauses)

        # Build final query: match ; modifiers ; fetch
        if modifier_str:
            query = match_str + "\n" + modifier_str + "\n" + fetch_clause_str
        else:
            query = match_str + "\n" + fetch_clause_str

        # Execute
        results = self._manager._execute(query, TransactionType.READ)

        # Hydrate objects with polymorphic type resolution
        instances = []
        entity_model = cast(type[Entity], model_class)
        for result in results:
            try:
                iid = result.pop("_iid", None)
                # Handle wrapped IID format from TypeDB driver
                if isinstance(iid, dict) and "value" in iid:
                    iid = iid["value"]

                # Get actual type label for polymorphic resolution
                type_label = result.pop("_type", None)

                # Extract attributes from nested "attributes" key (wildcard fetch structure)
                attrs = result.pop("attributes", result)

                # Resolve concrete class from type label
                if type_label and type_label != base_type:
                    concrete_class = ModelRegistry.get(type_label)
                    if concrete_class is None:
                        concrete_class = resolve_entity_class_from_label(
                            type_label, (entity_model,)
                        )
                else:
                    concrete_class = entity_model

                assert concrete_class is not None, "Failed to resolve concrete class"
                entity_class = cast(type[Entity], concrete_class)
                instance = entity_class.from_dict(attrs, strict=False)
                if iid:
                    object.__setattr__(instance, "_iid", iid)
                instances.append(instance)
            except Exception as e:
                from type_bridge.crud.exceptions import HydrationError

                raise HydrationError(
                    model_type=model_class.__name__,
                    raw_data=result,
                    cause=e,
                ) from e

        return instances

    @staticmethod
    def _parse_entity_lookup_filters(
        model_class: type[TypeDBType], filters: dict[str, Any]
    ) -> tuple[dict[str, Any], list[Any]]:
        """Parse Django-style lookup filters for entities.

        Converts filters like `name__startswith="Al"` into expressions.

        Args:
            model_class: Entity model class
            filters: Raw filter keyword arguments

        Returns:
            Tuple of (base_filters, expressions) where base_filters are exact matches
            and expressions are lookup expressions (startswith, contains, etc.)
        """
        from type_bridge.crud.lookup import build_lookup_expression
        from type_bridge.expressions import BooleanExpr, Expression, IidExpr

        owned_attrs = model_class.get_all_attributes()
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

            # No "__" means exact match
            if "__" not in raw_key:
                if raw_key not in owned_attrs:
                    raise ValueError(f"Unknown filter field '{raw_key}' for {model_class.__name__}")
                base_filters[raw_key] = raw_value
                continue

            # Parse field__lookup
            field_name, lookup = raw_key.split("__", 1)
            if field_name not in owned_attrs:
                raise ValueError(f"Unknown filter field '{field_name}' for {model_class.__name__}")

            attr_info = owned_attrs[field_name]
            attr_type = attr_info.typ

            # Handle exact/eq as base filter for efficiency
            if lookup in ("exact", "eq"):
                base_filters[field_name] = raw_value
                continue

            # Use shared lookup builder for lookups
            expr = build_lookup_expression(attr_type, lookup, raw_value)
            # Add to expressions list for AST processing later
            expressions.append(expr)

        return base_filters, expressions

    def all(self) -> list[T]:
        """Alias for execute()."""
        return self.execute()

    def first(self) -> T | None:
        """Get the first matching instance."""
        if isinstance(self._manager.strategy, RelationStrategy):
            results = self.execute()
            return results[0] if results else None

        # Optimize by limiting to 1
        original_limit = self._limit_val
        self._limit_val = 1
        results = self.execute()
        self._limit_val = original_limit
        return results[0] if results else None

    def count(self) -> int:
        """Count matching instances.

        Note: For relation queries with role player filters, TypeDB 3.x's reduce count
        may not work as expected. In those cases, we fall back to fetching IIDs and
        counting unique results.
        """
        model_class = self._manager.model_class

        # For relations with filters, fetch and count to avoid TypeDB count limitations
        if isinstance(self._manager.strategy, RelationStrategy) and self._filters:
            # Fetch only IIDs for efficiency
            results = self._manager._get_relations("$r", self._filters, self._expressions)
            return len(results)

        # Entity path or relation without filters - use reduce count
        var = "$x"

        # Build match clause with filters and expressions
        # Strategy is generic, so we need to cast appropriately based on strategy type
        if isinstance(self._manager.strategy, EntityStrategy):
            match_clause = self._manager.strategy.build_match_all(
                cast(type[Entity], model_class), var, self._filters
            )
        else:
            # RelationStrategy without filters
            assert isinstance(self._manager.strategy, RelationStrategy)
            match_clause = self._manager.strategy.build_match_all(
                cast(type[Relation], model_class), var, self._filters
            )

        if self._expressions:
            from type_bridge.query.ast import RawPattern

            for expr in self._expressions:
                # Add expressions as RawPatterns to the AST
                pattern_str = expr.to_typeql(var)
                match_clause.patterns.append(RawPattern(content=pattern_str))

        match_str = self._manager.compiler.compile(match_clause)

        # Explicitly count the variable to avoid counting joins
        reduce_str = self._manager._build_count_reduce(var)
        query = match_str + "\n" + reduce_str
        results = self._manager._execute(query, TransactionType.READ)
        if results and "count" in results[0]:
            count_val = results[0]["count"]
            # Handle wrapped value format from TypeDB driver
            if isinstance(count_val, dict) and "value" in count_val:
                return int(count_val["value"])
            if isinstance(count_val, (int, float)):
                return int(count_val)
            return int(str(count_val))
        return 0

    def delete(self) -> int:
        """Delete all matching instances and return count."""
        instances = self.execute()
        for instance in instances:
            self._manager.delete(instance)
        return len(instances)

    def exists(self) -> bool:
        """Check if any matching instances exist."""
        return self.count() > 0

    def update_with(self, func: Any) -> list[T]:
        """Update instances by applying a function to each.

        Uses atomic transaction semantics: if the function fails on any entity,
        no updates are persisted (all or nothing).

        Note: _iid is preserved during attribute modification by the wrap validator
        in TypeDBType base class.
        """
        instances = self.execute()
        if not instances:
            return []

        # Phase 1: Apply function to all instances FIRST
        # If any function call fails, no writes have happened yet
        for instance in instances:
            func(instance)

        # Phase 2: All functions succeeded, now persist all updates
        for instance in instances:
            self._manager.update(instance)

        return instances

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
        """
        from type_bridge.crud.aggregates import parse_aggregate_results
        from type_bridge.expressions import AggregateExpr
        from type_bridge.expressions.utils import generate_attr_var
        from type_bridge.query.ast import HasPattern, ReduceAssignment, ReduceClause

        if not aggregates:
            raise ValueError("At least one aggregation expression required")

        model_class = self._manager.model_class
        var = "$e"

        # Build base match clause with filters
        match_clause = self._manager.strategy.build_match_all(model_class, var, self._filters)

        # Add expression-based filters as AST patterns
        for expr in self._expressions:
            match_clause.patterns.extend(expr.to_ast(var))

        # Build reduce assignments for each aggregation
        reduce_assignments = []
        for agg in aggregates:
            if not isinstance(agg, AggregateExpr):
                raise TypeError(f"Expected AggregateExpr, got {type(agg).__name__}")

            # If this aggregation is on a specific attr_type (not count), add binding pattern
            if agg.attr_type is not None:
                attr_name = agg.attr_type.get_attribute_name()
                attr_var = generate_attr_var(var, agg.attr_type)
                match_clause.patterns.append(
                    HasPattern(thing_var=var, attr_type=attr_name, attr_var=attr_var)
                )

            # Build reduce assignment using FunctionCallValue
            result_var = f"${agg.get_fetch_key()}"
            # agg.to_typeql produces something like "mean($e_age)" - use it as expression string
            reduce_assignments.append(
                ReduceAssignment(variable=result_var, expression=agg.to_typeql(var))
            )

        # Compile and execute
        match_str = self._manager.compiler.compile(match_clause)
        reduce_clause = ReduceClause(assignments=reduce_assignments)
        reduce_str = self._manager.compiler.compile(reduce_clause)
        query = f"{match_str}\n{reduce_str}"

        results = self._manager._execute(query, TransactionType.READ)
        return parse_aggregate_results(results)

    def group_by(self, *fields: Any) -> GroupByQuery[T]:
        """Group results by field values.

        Args:
            *fields: FieldRef objects or field descriptors to group by (e.g., Person.department)

        Returns:
            GroupByQuery for chained aggregations

        Example:
            result = manager.group_by(Person.department).aggregate(Person.age.avg())
        """
        return GroupByQuery(
            self._manager,
            self._filters,
            self._expressions,
            fields,
        )


class GroupByQuery[T: "TypeDBType"]:
    """Query for grouped aggregations.

    Allows grouping entities by field values and computing aggregations per group.
    """

    def __init__(
        self,
        manager: TypeDBManager[T],
        filters: dict[str, Any],
        expressions: list[Any],
        group_fields: tuple[Any, ...],
    ):
        """Initialize grouped query.

        Args:
            manager: TypeDBManager instance
            filters: Dict-based filters
            expressions: Expression-based filters
            group_fields: Fields to group by (FieldRef or field descriptors)
        """
        self._manager = manager
        self._filters = filters
        self._expressions = expressions
        self._group_fields = group_fields

    def aggregate(self, *aggregates: Any) -> dict[Any, dict[str, Any]]:
        """Execute grouped aggregation.

        Args:
            *aggregates: AggregateExpr objects

        Returns:
            Dictionary mapping group values to aggregation results

        Example:
            # Group by department, compute average age per department
            result = manager.group_by(Person.department).aggregate(Person.age.avg())
            # Returns: {
            #   "Engineering": {"avg_age": 35.5},
            #   "Sales": {"avg_age": 28.3}
            # }
        """
        from type_bridge.crud.aggregates import parse_grouped_aggregate_results
        from type_bridge.expressions import AggregateExpr
        from type_bridge.expressions.utils import generate_attr_var
        from type_bridge.fields.base import FieldRef
        from type_bridge.query.ast import HasPattern, ReduceAssignment, ReduceClause

        if not aggregates:
            raise ValueError("At least one aggregation expression required")

        model_class = self._manager.model_class
        var = "$e"

        # Build base match clause with filters
        match_clause = self._manager.strategy.build_match_all(model_class, var, self._filters)

        # Add expression-based filters as AST patterns
        for expr in self._expressions:
            match_clause.patterns.extend(expr.to_ast(var))

        # Add group-by fields to match clause as AST patterns
        group_vars = []
        for i, field in enumerate(self._group_fields):
            var_name = f"$group{i}"
            # Field can be a FieldRef or a field descriptor
            if isinstance(field, FieldRef):
                attr_name = field.attr_type.get_attribute_name()
            else:
                # Assume it's a field descriptor with attr_type attribute
                attr_name = field.attr_type.get_attribute_name()
            match_clause.patterns.append(
                HasPattern(thing_var=var, attr_type=attr_name, attr_var=var_name)
            )
            group_vars.append(var_name)

        # Build reduce assignments for each aggregation
        reduce_assignments = []
        for agg in aggregates:
            if not isinstance(agg, AggregateExpr):
                raise TypeError(f"Expected AggregateExpr, got {type(agg).__name__}")

            # If this aggregation is on a specific attr_type (not count), add binding pattern
            if agg.attr_type is not None:
                attr_name = agg.attr_type.get_attribute_name()
                attr_var = generate_attr_var(var, agg.attr_type)
                match_clause.patterns.append(
                    HasPattern(thing_var=var, attr_type=attr_name, attr_var=attr_var)
                )

            # Build reduce assignment
            result_var = f"${agg.get_fetch_key()}"
            reduce_assignments.append(
                ReduceAssignment(variable=result_var, expression=agg.to_typeql(var))
            )

        # Compile and execute with group-by
        match_str = self._manager.compiler.compile(match_clause)
        group_clause = ", ".join(group_vars)
        reduce_clause = ReduceClause(assignments=reduce_assignments, group_by=group_clause)
        reduce_str = self._manager.compiler.compile(reduce_clause)
        query = f"{match_str}\n{reduce_str}"

        results = self._manager._execute(query, TransactionType.READ)
        return parse_grouped_aggregate_results(results, group_vars)
