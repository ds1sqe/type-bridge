"""Relation class for TypeDB relations."""

from __future__ import annotations

import logging
from typing import (
    TYPE_CHECKING,
    Any,
    ClassVar,
    TypeVar,
    dataclass_transform,
)

from pydantic import ConfigDict

from type_bridge.attribute import AttributeFlags, TypeFlags
from type_bridge.crud.utils import extract_entity_key, unwrap_attribute
from type_bridge.models.base import TypeDBType
from type_bridge.models.role import Role
from type_bridge.models.utils import (
    MatchClauseInfo,
    WriteQueryInfo,
)

if TYPE_CHECKING:
    from type_bridge.crud import RelationManager

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

    def __init_subclass__(cls) -> None:
        """Initialize relation subclass."""
        super().__init_subclass__()
        logger.debug(f"Initializing Relation subclass: {cls.__name__}")

        from type_bridge.models.schema_scanner import SchemaScanner

        scanner = SchemaScanner(cls)
        cls._roles = scanner.scan_roles()
        cls._owned_attrs = scanner.scan_attributes(is_relation=True)

    @classmethod
    def _get_base_type_class(cls) -> type[Relation]:
        """Return Relation as the base type class for supertype resolution."""
        return Relation

    @classmethod
    def _get_manager_class(cls) -> type[RelationManager]:
        from type_bridge.crud import RelationManager

        return RelationManager

    @classmethod
    def get_roles(cls) -> dict[str, Role]:
        """Get all roles defined on this relation.

        Returns:
            Dictionary mapping role names to Role instances
        """
        return cls._roles

    def to_insert_query(self, var: str = "$r") -> str:
        """Generate TypeQL insert query for this relation instance.

        Args:
            var: Variable name to use

        Returns:
            TypeQL insert pattern for the relation

        Example:
            >>> employment = Employment(employee=alice, employer=tech_corp, position="Engineer")
            >>> employment.to_insert_query()
            '$r (employee: $alice, employer: $tech_corp) isa employment, has position "Engineer"'
        """
        type_name = self.get_type_name()

        # Build role players
        role_parts = []
        for role_name, role in self.__class__._roles.items():
            # Get the entity from the instance
            entity_or_list = self.__dict__.get(role_name)
            if entity_or_list is not None:
                # Normalize to list for uniform handling (multi-cardinality roles)
                entities = entity_or_list if isinstance(entity_or_list, list) else [entity_or_list]
                for i, entity in enumerate(entities):
                    # Generate unique variable name for each player
                    var_name = f"{role_name}_{i}" if len(entities) > 1 else role_name
                    role_parts.append(f"{role.role_name}: ${var_name}")

        # Start with relation pattern
        relation_pattern = f"{var} ({', '.join(role_parts)}) isa {type_name}"
        parts = [relation_pattern]

        # Add attribute ownerships
        for field_name, attr_info in self._owned_attrs.items():
            value = getattr(self, field_name, None)
            if value is not None:
                attr_class = attr_info.typ
                attr_name = attr_class.get_attribute_name()

                # Handle lists (multi-value attributes)
                if isinstance(value, list):
                    for item in value:
                        parts.append(f"has {attr_name} {self._format_value(item)}")
                else:
                    parts.append(f"has {attr_name} {self._format_value(value)}")

        return ", ".join(parts)

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

    def get_write_query_info(self, var_name: str = "$r") -> WriteQueryInfo:
        """Get write query info for this relation instance.

        Relations need a match clause for role players before the insert/put.

        Args:
            var_name: Variable name to use

        Returns:
            WriteQueryInfo with match clause for role players and write pattern
        """
        roles = self.__class__._roles
        match_parts = []
        role_parts = []
        role_var_map: dict[str, list[str]] = {}

        # Build match clauses for role players
        for role_name, role in roles.items():
            entity_or_list = self.__dict__.get(role_name)
            if entity_or_list is None:
                raise ValueError(f"Role player '{role_name}' is required for insert")

            # Normalize to list for uniform handling
            entities = entity_or_list if isinstance(entity_or_list, list) else [entity_or_list]
            role_var_map[role_name] = []

            for i, entity in enumerate(entities):
                player_var = f"${role_name}_{i}" if len(entities) > 1 else f"${role_name}"
                role_var_map[role_name].append(player_var)
                role_parts.append(f"{role.role_name}: {player_var}")

                # Get match clause for the role player entity
                player_match = entity.get_match_clause_info(player_var)
                match_parts.append(player_match.main_clause)
                match_parts.extend(player_match.extra_clauses)

        # Build match clause
        match_clause = ";\n".join(match_parts) if match_parts else None

        # Build write pattern
        # Note: In TypeDB 3.x insert, relations don't get a variable binding
        # (the var_name is ignored for the write pattern)
        type_name = self.get_type_name()
        roles_str = ", ".join(role_parts)
        write_parts = [f"({roles_str}) isa {type_name}"]

        # Add attributes (including inherited)
        for field_name, attr_info in self.get_all_attributes().items():
            value = getattr(self, field_name, None)
            if value is not None:
                attr_class = attr_info.typ
                attr_name = attr_class.get_attribute_name()

                # Handle lists (multi-value attributes)
                if isinstance(value, list):
                    for item in value:
                        write_parts.append(f"has {attr_name} {self._format_value(item)}")
                else:
                    write_parts.append(f"has {attr_name} {self._format_value(value)}")

        write_pattern = ", ".join(write_parts)

        return WriteQueryInfo(match_clause=match_clause, write_pattern=write_pattern)

    @classmethod
    def build_batch_write_query(cls, instances: list[Relation], keyword: str = "insert") -> str:
        """Build a batch write query for multiple relation instances.

        Deduplicates role players across all instances to avoid redundant
        match clauses when multiple relations share the same role players.

        Args:
            instances: List of relation instances
            keyword: Write keyword ("insert" or "put")

        Returns:
            Complete TypeQL query string with match and write clauses
        """
        if not instances:
            return ""

        roles = cls._roles

        # Collect all unique role players with deduplication
        # Maps player_key -> (player_var, match_clause)
        all_players: dict[tuple[str, Any], tuple[str, str]] = {}
        player_counter = 0

        def get_player_key_and_match(entity: Any) -> tuple[tuple[str, Any], str]:
            """Get deduplication key and match clause for a role player."""
            player_type = entity.get_type_name()

            # Prefer IID-based matching when available
            entity_iid = getattr(entity, "_iid", None)
            if entity_iid:
                player_key: tuple[str, Any] = ("iid", entity_iid)
                match_clause = f"isa {player_type}, iid {entity_iid}"
                return (player_key, match_clause)

            # Fall back to key attribute matching
            owned_attrs = entity.get_all_attributes()
            key_parts: list[str] = [f"isa {player_type}"]
            key_values: list[tuple[str, Any]] = []

            for field_name, attr_info in owned_attrs.items():
                if attr_info.flags.is_key:
                    value = getattr(entity, field_name, None)
                    if value is not None:
                        attr_name = attr_info.typ.get_attribute_name()
                        formatted = entity._format_value(value)
                        key_parts.append(f"has {attr_name} {formatted}")
                        key_values.append((attr_name, value))

            if key_values:
                player_key = ("keys", tuple(sorted(key_values)))
                match_clause = ", ".join(key_parts)
                return (player_key, match_clause)

            raise ValueError(
                f"Role player ({entity.__class__.__name__}) cannot be identified: "
                "no _iid set and no @key attributes defined."
            )

        # First pass: collect all unique players
        match_clauses = []
        for instance in instances:
            for role_name in roles:
                entity_or_list = instance.__dict__.get(role_name)
                if entity_or_list is None:
                    continue

                entities = entity_or_list if isinstance(entity_or_list, list) else [entity_or_list]
                for entity in entities:
                    player_key, match_parts = get_player_key_and_match(entity)
                    if player_key not in all_players:
                        player_var = f"$player{player_counter}"
                        player_counter += 1
                        all_players[player_key] = (player_var, match_parts)
                        match_clauses.append(f"{player_var} {match_parts}")

        # Second pass: build write patterns using the player variables
        write_patterns = []
        for instance in instances:
            role_parts = []
            for role_name, role in roles.items():
                entity_or_list = instance.__dict__.get(role_name)
                if entity_or_list is None:
                    raise ValueError(f"Missing role player for role: {role_name}")

                entities = entity_or_list if isinstance(entity_or_list, list) else [entity_or_list]
                for entity in entities:
                    player_key, _ = get_player_key_and_match(entity)
                    player_var, _ = all_players[player_key]
                    role_parts.append(f"{role.role_name}: {player_var}")

            # Build write pattern for this relation
            roles_str = ", ".join(role_parts)
            pattern_parts = [f"({roles_str}) isa {cls.get_type_name()}"]

            # Add attributes (including inherited)
            for field_name, attr_info in cls.get_all_attributes().items():
                value = getattr(instance, field_name, None)
                if value is not None:
                    attr_name = attr_info.typ.get_attribute_name()
                    if isinstance(value, list):
                        for item in value:
                            pattern_parts.append(f"has {attr_name} {instance._format_value(item)}")
                    else:
                        pattern_parts.append(f"has {attr_name} {instance._format_value(value)}")

            write_patterns.append(", ".join(pattern_parts))

        # Build complete query
        if match_clauses:
            match_section = "match\n" + ";\n".join(match_clauses) + ";"
            write_section = f"{keyword}\n" + ";\n".join(write_patterns) + ";"
            return f"{match_section}\n{write_section}"
        else:
            return f"{keyword}\n" + ";\n".join(write_patterns) + ";"

    @classmethod
    def to_schema_definition(cls) -> str | None:
        """Generate TypeQL schema definition for this relation.

        Returns:
            TypeQL schema definition string, or None if this is a base class
        """
        # Base classes don't appear in TypeDB schema
        if cls.is_base():
            return None

        type_name = cls.get_type_name()
        lines = []

        # Define relation type with supertype from Python inheritance
        # TypeDB 3.x syntax: relation name @abstract, sub parent,
        supertype = cls.get_supertype()
        is_abstract = cls.is_abstract()

        relation_def = f"relation {type_name}"
        if is_abstract:
            relation_def += " @abstract"
        if supertype:
            relation_def += f", sub {supertype}"

        lines.append(relation_def)

        # Add roles with optional cardinality constraints
        for role in cls._roles.values():
            role_def = f"    relates {role.role_name}"
            # Add cardinality annotation if not default (1..1)
            if role.cardinality is not None:
                card = role.cardinality
                if card.max is None:
                    role_def += f" @card({card.min}..)"
                else:
                    role_def += f" @card({card.min}..{card.max})"
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
