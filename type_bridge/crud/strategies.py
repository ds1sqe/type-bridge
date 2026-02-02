"""Strategies for handling different TypeDB model types in the unified manager."""

from __future__ import annotations

import logging
from abc import ABC, abstractmethod
from typing import TYPE_CHECKING, Any, Literal

from type_bridge.models.utils import get_base_type_for_attribute
from type_bridge.query.ast import (
    Constraint,
    HasConstraint,
    IidConstraint,
    InsertClause,
    LiteralValue,
    MatchClause,
    RolePlayer,
)

if TYPE_CHECKING:
    from type_bridge.attribute.base import Attribute
    from type_bridge.models import Entity, Relation
    from type_bridge.models.base import TypeDBType

logger = logging.getLogger(__name__)


class ModelStrategy(ABC):
    """Abstract strategy for handling model-specific logic."""

    @abstractmethod
    def identify(self, instance: TypeDBType) -> list[Constraint]:
        """Generate identification constraints (IID or keys/roles)."""
        pass

    @abstractmethod
    def build_insert(
        self, instance: TypeDBType, var: str
    ) -> tuple[MatchClause | None, InsertClause]:
        """Generate insert AST and optional match prerequisites."""
        pass

    @abstractmethod
    def build_match_all(
        self, model_class: type[TypeDBType], var: str, filters: dict[str, Any]
    ) -> MatchClause:
        """Generate match AST for filtering."""
        pass

    def _get_iid_constraint(self, instance: TypeDBType) -> IidConstraint | None:
        """Helper to check for IID."""
        if instance._iid:
            return IidConstraint(iid=instance._iid)
        return None

    def _convert_value(self, value: Any, attr_class: type[Attribute]) -> LiteralValue:
        """Helper to convert Python value to AST LiteralValue."""
        # This logic mimics to_ast in models
        py_type = get_base_type_for_attribute(attr_class)
        val_type_map: dict[type, Literal["string", "long", "double", "boolean", "datetime", "date"]] = {
            str: "string",
            int: "long",
            float: "double",
            bool: "boolean",
        }
        # Add datetime support if needed, requires import
        from datetime import datetime

        val_type_map[datetime] = "datetime"

        ast_type: Literal["string", "long", "double", "boolean", "datetime", "date"] = val_type_map.get(
            py_type, "string"
        ) if py_type else "string"

        # Refine type based on actual value
        if ast_type == "string" and isinstance(value, bool):
            ast_type = "boolean"
        elif ast_type == "string" and isinstance(value, (int, float)):
            ast_type = "double" if isinstance(value, float) else "long"

        return LiteralValue(value=value, value_type=ast_type)


class EntityStrategy(ModelStrategy):
    """Strategy for handling Entity models."""

    def identify(self, instance: Entity) -> list[Constraint]:
        # 1. Prefer IID
        iid_constraint = self._get_iid_constraint(instance)
        if iid_constraint:
            return [iid_constraint]

        # 2. Fallback to @key attributes
        constraints: list[Constraint] = []
        key_attrs = {
            name: info for name, info in instance.get_all_attributes().items() if info.flags.is_key
        }

        if not key_attrs:
            raise ValueError(
                f"Entity '{instance.__class__.__name__}' cannot be identified: "
                f"no _iid set and no @key attributes defined."
            )

        for field_name, attr_info in key_attrs.items():
            value = getattr(instance, field_name, None)
            if value is None:
                raise ValueError(
                    f"Cannot identify {instance.__class__.__name__}: "
                    f"key attribute '{field_name}' is None"
                )

            # Unwrap
            raw_val = value.value if hasattr(value, "value") else value
            attr_name = attr_info.typ.get_attribute_name()

            constraints.append(
                HasConstraint(
                    attr_name=attr_name, value=self._convert_value(raw_val, attr_info.typ)
                )
            )

        return constraints

    def build_insert(self, instance: Entity, var: str) -> tuple[MatchClause | None, InsertClause]:
        # Entity insert requires no match prerequisites (unless we support nested inserts later)
        if hasattr(instance, "to_ast"):
            return None, instance.to_ast(var)
        raise NotImplementedError("Model does not implement to_ast")

    def build_match_all(
        self, model_class: type[Entity], var: str, filters: dict[str, Any]
    ) -> MatchClause:
        from type_bridge.query.ast import EntityPattern, HasConstraint, MatchClause

        constraints = []
        owned_attrs = model_class.get_all_attributes()

        for field_name, value in filters.items():
            if field_name not in owned_attrs:
                # Could be a special filter or lookup? For now assume exact match on attrs
                continue

            attr_info = owned_attrs[field_name]
            attr_name = attr_info.typ.get_attribute_name()

            constraints.append(
                HasConstraint(attr_name=attr_name, value=self._convert_value(value, attr_info.typ))
            )

        pattern = EntityPattern(
            variable=var, type_name=model_class.get_type_name(), constraints=constraints
        )
        return MatchClause(patterns=[pattern])


class RelationStrategy(ModelStrategy):
    """Strategy for handling Relation models."""

    def build_fetch_query(
        self,
        model_class: type[Relation],
        var: str,
        filters: dict[str, Any],
        expressions: list[Any],
        order_by: list[tuple[str, bool]] | None = None,
        limit: int | None = None,
        offset: int | None = None,
    ) -> str:
        """Build a complete fetch query for relations including role players.

        Uses the TypeDB 3.x pattern with isa! for type variable binding to enable
        label() fetching for polymorphic type resolution.

        Args:
            model_class: The relation class
            var: Variable name for the relation (e.g., "$r")
            filters: Attribute and role player filters
            expressions: Expression objects for filtering
            order_by: List of (field_name, descending) tuples for sorting
            limit: Maximum number of results
            offset: Number of results to skip

        Returns:
            Complete TypeQL query string
        """
        from type_bridge.crud.formatting import format_value
        from type_bridge.crud.types import is_multi_value_attribute
        from type_bridge.crud.utils import build_role_player_fetch_items
        from type_bridge.models import Entity

        roles = getattr(model_class, "_roles", {})
        owned_attrs = model_class.get_all_attributes()
        base_type = model_class.get_type_name()

        # Build role player variables
        role_vars: dict[str, str] = {}  # role_name -> var
        role_info: dict[str, tuple[str, tuple[type, ...]]] = {}  # for fetch items

        for role_name, role in roles.items():
            role_var = f"${role_name}"
            role_vars[role_name] = role_var
            role_info[role_name] = (role_var, role.player_entity_types)

        # Build match clause using isa! pattern for type variable binding
        # This enables label($t) to fetch the actual type name
        role_player_parts = [f"{roles[rn].role_name}: {rv}" for rn, rv in role_vars.items()]
        roles_str = ", ".join(role_player_parts)

        # Use isa! to bind exact type to $t for label() function
        match_clauses = [f"{var} isa! $t ({roles_str})", f"$t sub {base_type}"]

        # Add type variable bindings for each role player to enable label() fetch
        for role_name in roles:
            role_var = f"${role_name}"
            type_var = f"{role_var}_type"
            match_clauses.append(f"{role_var} isa! {type_var}")

        # Add attribute filter clauses to relation match
        for field_name, value in filters.items():
            if field_name in owned_attrs:
                attr_info = owned_attrs[field_name]
                attr_name = attr_info.typ.get_attribute_name()
                raw_val = value.value if hasattr(value, "value") else value
                match_clauses.append(f"{var} has {attr_name} {format_value(raw_val)}")

        # Add role player filter clauses
        for role_name, value in filters.items():
            if role_name in roles and isinstance(value, Entity):
                entity = value
                entity_type = entity.get_type_name()
                role_var = f"${role_name}"

                # Get entity identification (IID or keys)
                if entity._iid:
                    match_clauses.append(f"{role_var} isa {entity_type}, iid {entity._iid}")
                else:
                    # Use key attributes
                    key_match = f"{role_var} isa {entity_type}"
                    for field_name, attr_info in entity.get_all_attributes().items():
                        if attr_info.flags.is_key:
                            attr_value = getattr(entity, field_name, None)
                            if attr_value is not None:
                                attr_name = attr_info.typ.get_attribute_name()
                                raw_val = attr_value.value if hasattr(attr_value, "value") else attr_value
                                key_match += f", has {attr_name} {format_value(raw_val)}"
                    match_clauses.append(key_match)

        match_clause = "match\n" + ";\n".join(match_clauses) + ";"

        # Add expression patterns
        if expressions:
            from type_bridge.expressions.role_player import RolePlayerExpr

            match_clause = match_clause.rstrip(";")
            for expr in expressions:
                # For RolePlayerExpr, use the role player variable instead of relation variable
                if isinstance(expr, RolePlayerExpr):
                    role_var = role_vars.get(expr.role_name, f"${expr.role_name}")
                    pattern = expr.to_typeql(role_var)
                else:
                    pattern = expr.to_typeql(var)
                match_clause += f";\n{pattern}"
            match_clause += ";"

        # Build fetch clause with type label and role player fetch items
        fetch_items = [f'"_iid": iid({var})', '"_type": label($t)']

        # Add relation attributes
        for field_name, attr_info in owned_attrs.items():
            attr_name = attr_info.typ.get_attribute_name()
            if is_multi_value_attribute(attr_info.flags):
                fetch_items.append(f'"{attr_name}": [{var}.{attr_name}]')
            else:
                fetch_items.append(f'"{attr_name}": {var}.{attr_name}')

        # Add role player fetch items (IID, type label, and attributes)
        fetch_items.extend(build_role_player_fetch_items(role_info))

        # Build modifiers (sort, offset, limit)
        # For relations, we need to bind sort attributes to variables in the match clause
        modifier_clauses = []
        sort_var_bindings = []
        if order_by:
            sort_parts = []
            for i, (field_name, desc) in enumerate(order_by):
                sort_var = f"$sort_{i}"
                direction = "desc" if desc else "asc"

                # Check for role player attribute syntax: role__attribute
                if "__" in field_name:
                    role_name, attr_field = field_name.split("__", 1)
                    if role_name in roles:
                        role_var = role_vars.get(role_name, f"${role_name}")
                        # Get the role player entity types to find the attribute
                        role = roles[role_name]
                        player_types = role.player_entity_types
                        # Look up the attribute in the player entity types
                        for player_type in player_types:
                            player_attrs = player_type.get_all_attributes()
                            if attr_field in player_attrs:
                                attr_name = player_attrs[attr_field].typ.get_attribute_name()
                                sort_var_bindings.append(f"{role_var} has {attr_name} {sort_var}")
                                sort_parts.append(f"{sort_var} {direction}")
                                break
                # Direct relation attribute
                elif field_name in owned_attrs:
                    attr_name = owned_attrs[field_name].typ.get_attribute_name()
                    # Add binding to match clause
                    sort_var_bindings.append(f"{var} has {attr_name} {sort_var}")
                    sort_parts.append(f"{sort_var} {direction}")
            if sort_parts:
                modifier_clauses.append(f"sort {', '.join(sort_parts)};")

        # Add sort variable bindings to match clause
        if sort_var_bindings:
            match_clause = match_clause.rstrip(";") + ";\n" + ";\n".join(sort_var_bindings) + ";"

        if offset is not None:
            modifier_clauses.append(f"offset {offset};")
        if limit is not None:
            modifier_clauses.append(f"limit {limit};")

        modifier_str = "\n".join(modifier_clauses)

        fetch_clause = "fetch {\n  " + ",\n  ".join(fetch_items) + "\n}"

        # Build query: match + modifiers + fetch
        if modifier_str:
            return match_clause + "\n" + modifier_str + "\n" + fetch_clause + ";"
        return match_clause + "\n" + fetch_clause + ";"

    def identify(self, instance: Relation) -> list[Constraint]:
        # 1. Prefer IID (most reliable for relations)
        iid_constraint = self._get_iid_constraint(instance)
        if iid_constraint:
            return [iid_constraint]

        # 2. Relations without IID are hard to identify uniquely
        # We'd need to match on role players + attributes, which is complex
        raise ValueError(
            f"Relation '{instance.__class__.__name__}' cannot be identified: "
            f"no _iid set. Fetch the relation from the database first to populate _iid."
        )

    def build_insert(self, instance: Relation, var: str) -> tuple[MatchClause | None, InsertClause]:
        from type_bridge.query.ast import EntityPattern, MatchClause

        # 1. Build Insert Clause
        insert_clause = instance.to_ast(var)

        # 2. Build Match Clause for Role Players
        patterns = []

        # We need to iterate over roles again to generate the corresponding match patterns
        # matching the variables used in to_ast.
        # This duplicates logic slightly from to_ast but is necessary for binding.

        # NOTE: to_ast uses a specific variable naming convention: f"${role_name}_{i}"

        for role_name, role in instance.__class__._roles.items():
            entity_or_list = instance.__dict__.get(role_name)
            if entity_or_list is not None:
                entities = entity_or_list if isinstance(entity_or_list, list) else [entity_or_list]
                for i, entity in enumerate(entities):
                    # Reconstruct variable name used in to_ast
                    var_name = f"{role_name}_{i}" if len(entities) > 1 else role_name
                    player_var = f"${var_name}"

                    # Identify the player entity
                    # Quick fix: Use the entity's _iid or key attributes directly here.
                    constraints = EntityStrategy().identify(entity)

                    pattern = EntityPattern(
                        variable=player_var,
                        type_name=entity.get_type_name(),
                        constraints=constraints,
                    )
                    patterns.append(pattern)

        match_clause = MatchClause(patterns=patterns) if patterns else None

        return match_clause, insert_clause

    def build_match_all(
        self, model_class: type[Relation], var: str, filters: dict[str, Any]
    ) -> MatchClause:
        from type_bridge.models import Entity
        from type_bridge.query.ast import (
            EntityPattern,
            HasConstraint,
            MatchClause,
            RelationPattern,
            RolePlayer,
        )

        constraints = []
        role_players: list[RolePlayer] = []
        entity_patterns: list[EntityPattern] = []
        owned_attrs = model_class.get_all_attributes()
        roles = getattr(model_class, "_roles", {})

        for field_name, value in filters.items():
            # Check if it's an attribute filter
            if field_name in owned_attrs:
                attr_info = owned_attrs[field_name]
                attr_name = attr_info.typ.get_attribute_name()

                constraints.append(
                    HasConstraint(attr_name=attr_name, value=self._convert_value(value, attr_info.typ))
                )
            # Check if it's a role player filter
            elif field_name in roles and isinstance(value, Entity):
                # Generate a unique variable for the role player
                player_var = f"${field_name}_filter"
                role = roles[field_name]

                # Add role player to the relation pattern
                role_players.append(RolePlayer(role=role.role_name, player_var=player_var))

                # Build entity pattern for the role player using its key attributes
                player_constraints = EntityStrategy().identify(value)
                entity_patterns.append(
                    EntityPattern(
                        variable=player_var,
                        type_name=value.get_type_name(),
                        constraints=player_constraints,
                    )
                )

        # Build the relation pattern with role players (if any)
        relation_pattern = RelationPattern(
            variable=var,
            type_name=model_class.get_type_name(),
            role_players=role_players,
            constraints=constraints,
        )

        # Combine all patterns: entity patterns first, then the relation
        all_patterns: list = entity_patterns + [relation_pattern]
        return MatchClause(patterns=all_patterns)
