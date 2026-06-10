"""Strategies for handling different TypeDB model types in the unified manager."""

from __future__ import annotations

import logging
from abc import ABC, abstractmethod
from typing import TYPE_CHECKING, Any

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


class ModelStrategy[T: "TypeDBType"](ABC):
    """Abstract strategy for handling model-specific logic."""

    @abstractmethod
    def identify(self, instance: T) -> list[Constraint]:
        """Generate identification constraints (IID or keys/roles)."""
        pass

    @abstractmethod
    def build_insert(self, instance: T, var: str) -> tuple[MatchClause | None, InsertClause]:
        """Generate insert AST and optional match prerequisites."""
        pass

    @abstractmethod
    def build_match_all(
        self, model_class: type[T], var: str, filters: dict[str, Any]
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
        from type_bridge.models.utils import get_ast_value_type

        ast_type = get_ast_value_type(attr_class)

        # Refine type based on actual value
        if ast_type == "string" and isinstance(value, bool):
            ast_type = "boolean"
        elif ast_type == "string" and isinstance(value, (int, float)):
            ast_type = "double" if isinstance(value, float) else "long"

        return LiteralValue(value=value, value_type=ast_type)


class EntityStrategy(ModelStrategy["Entity"]):
    """Strategy for handling Entity models."""

    def identify(self, instance: Entity) -> list[Constraint]:
        """Generate identification constraints for an entity.

        Delegates to the entity's _build_identification_constraints() method
        to avoid duplicating the IID/key attribute logic.
        """
        return instance._build_identification_constraints()

    def build_insert(self, instance: Entity, var: str) -> tuple[MatchClause | None, InsertClause]:
        # Entity insert requires no match prerequisites (unless we support nested inserts later)
        if hasattr(instance, "to_ast"):
            return None, instance.to_ast(var)
        raise NotImplementedError("Model does not implement to_ast")

    def build_match_all(
        self, model_class: type[Entity], var: str, filters: dict[str, Any]
    ) -> MatchClause:
        from type_bridge.query.ast import EntityPattern, MatchClause

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


class RelationStrategy(ModelStrategy["Relation"]):
    """Strategy for handling Relation models."""

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

        for role_name, _ in instance.__class__._roles.items():
            player_or_list = instance.__dict__.get(role_name)
            if player_or_list is not None:
                players = player_or_list if isinstance(player_or_list, list) else [player_or_list]
                for i, player in enumerate(players):
                    # Reconstruct variable name used in to_ast
                    var_name = f"{role_name}_{i}" if len(players) > 1 else role_name
                    player_var = f"${var_name}"

                    # Identify the player - could be Entity or Relation
                    # Use the appropriate strategy based on player type
                    if hasattr(player, "_build_identification_constraints"):
                        # Entity: use the shared identification method
                        constraints = player._build_identification_constraints()
                    else:
                        # Relation as role player: can only identify by IID
                        constraints = self.identify(player)

                    pattern = EntityPattern(
                        variable=player_var,
                        type_name=player.get_type_name(),
                        constraints=constraints,
                    )
                    patterns.append(pattern)

        match_clause = MatchClause(patterns=patterns) if patterns else None

        return match_clause, insert_clause

    def build_match_all(
        self, model_class: type[Relation], var: str, filters: dict[str, Any]
    ) -> MatchClause:
        from type_bridge.query.ast import (
            EntityPattern,
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
                    HasConstraint(
                        attr_name=attr_name, value=self._convert_value(value, attr_info.typ)
                    )
                )
            # Check if it's a role player filter (Entity or Relation)
            elif field_name in roles and hasattr(value, "get_type_name"):
                # Generate a unique variable for the role player
                player_var = f"${field_name}_filter"
                role = roles[field_name]

                # Add role player to the relation pattern
                role_players.append(RolePlayer(role=role.role_name, player_var=player_var))

                # Build entity pattern for the role player
                # Handle both Entity (has _build_identification_constraints) and Relation (IID only)
                if hasattr(value, "_build_identification_constraints"):
                    player_constraints = value._build_identification_constraints()
                else:
                    # Relation as role player: can only identify by IID
                    player_constraints = self.identify(value)
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
