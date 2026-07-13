"""Relation class for TypeDB relations."""

from __future__ import annotations

import logging
from typing import (
    TYPE_CHECKING,
    Any,
    ClassVar,
    Self,
    TypeVar,
    cast,
    dataclass_transform,
)

from pydantic import ConfigDict, model_validator

from type_bridge.attribute import AttributeFlags, TypeFlags
from type_bridge.crud.utils import extract_entity_key, unwrap_attribute
from type_bridge.models.base import TypeDBType
from type_bridge.models.role import Role
from type_bridge.models.utils import MatchClauseInfo

if TYPE_CHECKING:
    from type_bridge.query.ast import InsertClause, Pattern

logger = logging.getLogger(__name__)

# Type variable for self type
R = TypeVar("R", bound="Relation")


@dataclass_transform(kw_only_default=True, field_specifiers=(AttributeFlags,))
class Relation(TypeDBType):
    """Base class for TypeDB relations with Pydantic validation.

    Relations can own attributes and have role players.
    Use TypeFlags to configure type name and abstract status.
    Supertype is determined automatically from Python inheritance.

    This class inherits from TypeDBType and Pydantic's BaseModel, providing:
    - Automatic validation of attribute values
    - JSON serialization/deserialization
    - Type checking and coercion
    - Field metadata via Pydantic's Field()

    Example:
        class Position(String):
            pass

        class Salary(Integer):
            pass

        class Employment(Relation):
            flags = TypeFlags(name="employment")

            employee: Role[Person] = Role("employee", Person)
            employer: Role[Company] = Role("employer", Company)

            position: Position
            salary: Salary | None
    """

    # Pydantic configuration (extends TypeDBType config)
    model_config = ConfigDict(
        arbitrary_types_allowed=True,
        validate_assignment=True,
        extra="allow",
        ignored_types=(TypeFlags, Role),
        revalidate_instances="always",
    )

    # Relation-specific metadata
    _type_context = "relation"
    _roles: ClassVar[dict[str, Role]] = {}

    def __init_subclass__(cls, **kwargs: Any) -> None:
        """Initialize relation subclass."""
        super().__init_subclass__(**kwargs)
        logger.debug(f"Initializing Relation subclass: {cls.__name__}")

        from type_bridge.models.schema_scanner import SchemaScanner

        scanner = SchemaScanner(cls)
        cls._roles = scanner.scan_roles()
        cls._owned_attrs = scanner.scan_attributes(is_relation=True)

        from type_bridge.models.registry import ModelRegistry

        ModelRegistry.register_attribute_owners(cls)

    @classmethod
    def __pydantic_init_subclass__(cls, **kwargs: Any) -> None:
        """Called by Pydantic after model class initialization.

        This is the right place to restore Role descriptors because:
        1. __init_subclass__ runs before Pydantic's metaclass finishes
        2. Pydantic removes Role instances from class __dict__ during construction
        3. __pydantic_init_subclass__ runs after Pydantic's setup is complete

        This restores Role descriptors so class-level access (Employment.employee)
        returns a RoleRef for type-safe query building.
        """
        super().__pydantic_init_subclass__(**kwargs)

        # Restore Role descriptors using type.__setattr__ to bypass any Pydantic interception
        for role_name, role in cls._roles.items():
            type.__setattr__(cls, role_name, role)

    def __setattr__(self, name: str, value: Any) -> None:
        """Reject instance assignment to roles that declare no player."""
        role = self.__class__._roles.get(name)
        if role is not None and role.is_relates_only:
            raise TypeError(
                f"Role '{role.role_name}' is relates-only; it declares no player "
                "to bind on this relation"
            )
        super().__setattr__(name, value)

    @classmethod
    def _get_base_type_class(cls) -> type[Relation]:
        """Return Relation as the base type class for supertype resolution."""
        return Relation

    @classmethod
    def _get_manager_class(cls) -> type:
        from type_bridge.crud import TypeDBManager

        return TypeDBManager

    @classmethod
    def get_roles(cls) -> dict[str, Role]:
        """Get all roles defined on this relation.

        Returns:
            Dictionary mapping role names to Role instances
        """
        return cls._roles

    @model_validator(mode="before")
    @classmethod
    def _normalize_unset_roles(cls, values: Any) -> Any:
        """Prevent class-level RoleRef defaults from leaking into instances."""
        if not isinstance(values, dict):
            return values

        from type_bridge.fields.role import RoleRef

        data = dict(values)
        for role_name in cls._roles:
            value = data.get(role_name)
            if role_name not in data or isinstance(value, RoleRef):
                data[role_name] = None
        return data

    @model_validator(mode="after")
    def _normalize_unbound_role_defaults(self) -> Self:
        """Replace leaked class-level RoleRef defaults with None.

        Role players are optional at construction time so relations can be
        built incrementally or hydrated from partial results. Completeness
        for a non-optional role is enforced where it matters — at insert and
        match time (see to_ast and the query builders), which raise when a
        required player is still unset.
        """
        from type_bridge.fields.role import RoleRef

        for role_name in self.__class__._roles:
            if isinstance(self.__dict__.get(role_name), RoleRef):
                cast("dict[str, Any]", self.__dict__)[role_name] = None
        return self

    def to_ast(self, var: str = "$r") -> InsertClause:
        """Generate AST InsertClause for this relation instance.

        Args:
            var: Variable name to use

        Returns:
            InsertClause containing statements
        """
        from type_bridge.query.ast import InsertClause, RelationStatement, Statement
        from type_bridge.query.ast import RolePlayer as AstRolePlayer

        type_name = self.get_type_name()

        # Build role players
        role_players_ast = []
        for role_name, role in self.__class__._roles.items():
            # Get the entity from the instance
            entity_or_list = self.__dict__.get(role_name)
            if entity_or_list is not None:
                # Normalize to list for uniform handling
                entities = entity_or_list if isinstance(entity_or_list, list) else [entity_or_list]
                for i, entity in enumerate(entities):
                    # Generate unique variable name for each player
                    # Note: This assumes the manager will bind these variables in a match clause
                    # or they are already bound. For now, we generate the usage.
                    var_name = f"{role_name}_{i}" if len(entities) > 1 else role_name

                    # We assume variables are passed in or coordinated.
                    # Since to_ast is called on the instance, we need a convention.
                    # This implies the Manager/Compiler must coordinate variable names between Match and Insert.
                    # For now, let's use a standard derived name format that the Manager can also replicate.
                    player_var = f"${var_name}"

                    role_players_ast.append(
                        AstRolePlayer(role=role.role_name, player_var=player_var)
                    )

        # Collect attribute statements using shared helper from TypeDBType
        inline_attributes = self._build_attribute_statements(var)

        statements: list[Statement] = [
            RelationStatement(
                variable=var,
                type_name=type_name,
                role_players=role_players_ast,
                include_variable=False,  # TypeDB 3.x insert doesn't use variable for relations
                attributes=inline_attributes,
            )
        ]

        return InsertClause(statements=statements)

    def get_match_clause_info(self, var_name: str = "$r") -> MatchClauseInfo:
        """Get match clause info for this relation instance.

        Prefers IID-based matching when available (most precise).
        Falls back to role player matching.

        Args:
            var_name: Variable name to use in the match clause

        Returns:
            MatchClauseInfo with the match clause and role player clauses

        Raises:
            ValueError: If any role player cannot be identified
        """
        type_name = self.get_type_name()

        # Prefer IID-based matching when available
        relation_iid = getattr(self, "_iid", None)
        if relation_iid:
            main_clause = f"{var_name} isa {type_name}, iid {relation_iid}"
            return MatchClauseInfo(main_clause=main_clause, extra_clauses=[], var_name=var_name)

        # Fall back to role player matching
        roles = self.__class__._roles
        role_parts = []
        extra_clauses = []

        for role_name, role in roles.items():
            entity_or_list = self.__dict__.get(role_name)
            if entity_or_list is None:
                raise ValueError(f"Role player '{role_name}' is required for matching")

            # Normalize to list for uniform handling
            entities = entity_or_list if isinstance(entity_or_list, list) else [entity_or_list]

            for i, entity in enumerate(entities):
                player_var = f"${role_name}_{i}" if len(entities) > 1 else f"${role_name}"
                role_parts.append(f"{role.role_name}: {player_var}")

                # Get match clause for the role player entity
                player_match = entity.get_match_clause_info(player_var)
                extra_clauses.append(player_match.main_clause)
                extra_clauses.extend(player_match.extra_clauses)

        roles_str = ", ".join(role_parts)
        main_clause = f"{var_name} isa {type_name} ({roles_str})"

        return MatchClauseInfo(
            main_clause=main_clause, extra_clauses=extra_clauses, var_name=var_name
        )

    def get_match_patterns(self, var_name: str = "$r") -> list[Pattern]:
        """Get AST patterns for matching this relation instance.

        Returns a list of patterns: the main RelationPattern plus EntityPatterns
        for each role player (when matching by role players, not IID).

        Args:
            var_name: Variable name to use in the pattern

        Returns:
            List of Pattern AST nodes

        Raises:
            ValueError: If any role player cannot be identified
        """
        from type_bridge.query.ast import (
            IidConstraint,
            RelationPattern,
            RolePlayer,
        )

        type_name = self.get_type_name()
        patterns: list[Pattern] = []

        # Prefer IID-based matching when available
        relation_iid = getattr(self, "_iid", None)
        if relation_iid:
            main_pattern = RelationPattern(
                variable=var_name,
                type_name=type_name,
                role_players=[],
                constraints=[IidConstraint(iid=relation_iid)],
            )
            return [main_pattern]

        # Fall back to role player matching
        roles = self.__class__._roles
        role_player_nodes: list[RolePlayer] = []

        for role_name, role in roles.items():
            entity_or_list = self.__dict__.get(role_name)
            if entity_or_list is None:
                raise ValueError(f"Role player '{role_name}' is required for matching")

            # Normalize to list for uniform handling
            entities = entity_or_list if isinstance(entity_or_list, list) else [entity_or_list]

            for i, player in enumerate(entities):
                player_var = f"${role_name}_{i}" if len(entities) > 1 else f"${role_name}"
                role_player_nodes.append(RolePlayer(role=role.role_name, player_var=player_var))

                # Get AST pattern for the role player (could be Entity or Relation)
                if hasattr(player, "get_match_pattern"):
                    # Entity: returns single pattern
                    patterns.append(player.get_match_pattern(player_var))
                else:
                    # Relation: returns list of patterns
                    patterns.extend(player.get_match_patterns(player_var))

        # Build main relation pattern
        main_pattern = RelationPattern(
            variable=var_name,
            type_name=type_name,
            role_players=role_player_nodes,
            constraints=[],
        )
        # Insert main pattern first, then role player patterns
        patterns.insert(0, main_pattern)

        return patterns

    @classmethod
    def to_schema_definition(cls) -> str | None:
        """Generate TypeQL schema definition for this relation.

        Returns:
            TypeQL schema definition string, or None if this is a base class
        """
        from type_bridge.typeql.annotations import (
            format_card_annotation,
            format_doc_meta_annotations,
            format_type_annotations,
        )

        # Base classes don't appear in TypeDB schema
        if cls.is_base():
            return None

        type_name = cls.get_type_name()
        lines = []

        # Define relation type with supertype from Python inheritance
        # TypeDB 3.x syntax: relation name @abstract, sub parent,
        supertype = cls.get_supertype()
        type_annotations = format_type_annotations(abstract=cls.is_abstract())
        type_annotations.extend(format_doc_meta_annotations(cls.get_doc(), cls.get_meta()))

        relation_def = f"relation {type_name}"
        if type_annotations:
            relation_def += " " + " ".join(type_annotations)
        if supertype:
            relation_def += f", sub {supertype}"

        lines.append(relation_def)

        # Add roles with optional cardinality constraints
        for role in cls._roles.values():
            role_def = f"    relates {role.role_name}"
            # Add cardinality annotation if not default (1..1)
            if role.cardinality is not None:
                card_annotation = format_card_annotation(role.cardinality.min, role.cardinality.max)
                if card_annotation:
                    role_def += f" {card_annotation}"
            for annotation in format_doc_meta_annotations(role.doc, role.meta):
                role_def += f" {annotation}"
            lines.append(role_def)

        # Add attribute ownerships using shared helper
        lines.extend(cls._build_owns_lines())

        # Join with commas, but end with semicolon (no comma before semicolon)
        return ",\n".join(lines) + ";"

    def __repr__(self) -> str:
        """Developer-friendly string representation of relation."""
        parts = []
        # Show role players
        for role_name in self._roles:
            player = getattr(self, role_name, None)
            if player is not None:
                parts.append(f"{role_name}={player!r}")
        # Show attributes
        for field_name in self._owned_attrs:
            value = getattr(self, field_name, None)
            if value is not None:
                parts.append(f"{field_name}={value!r}")
        return f"{self.__class__.__name__}({', '.join(parts)})"

    def __str__(self) -> str:
        """User-friendly string representation of relation."""
        parts = []

        # Show role players first (more important)
        role_parts = []
        for role_name, role in self._roles.items():
            player = getattr(self, role_name, None)
            # Only show role players that are actual entity instances (have _owned_attrs)
            if player is not None and hasattr(player, "_owned_attrs"):
                # Get a simple representation of the player (their key attribute)
                key_info = extract_entity_key(player)
                if key_info:
                    _, _, raw_value = key_info
                    role_parts.append(f"{role_name}={raw_value}")

        if role_parts:
            parts.append("(" + ", ".join(role_parts) + ")")

        # Show attributes
        attr_parts = []
        for field_name, attr_info in self._owned_attrs.items():
            value = getattr(self, field_name, None)
            if value is None:
                continue

            # Extract actual value from Attribute instance
            display_value = unwrap_attribute(value)

            attr_parts.append(f"{field_name}={display_value}")

        if attr_parts:
            parts.append("[" + ", ".join(attr_parts) + "]")

        if parts:
            return f"{self.get_type_name()}{' '.join(parts)}"
        else:
            return f"{self.get_type_name()}()"
