"""Schema comparison and diff classes for TypeDB schema management."""

from dataclasses import dataclass, field

from type_bridge.attribute.base import Attribute
from type_bridge.models import Entity, Relation


@dataclass
class AttributeFlagChange:
    """Represents a change in attribute flags (e.g., cardinality change)."""

    name: str
    old_flags: str
    new_flags: str


@dataclass
class RolePlayerChange:
    """Represents a change in role player types.

    Tracks when entity types are added to or removed from a role's allowed players.

    Example:
        If a role changes from Role[Person] to Role[Person, Company]:
        - added_player_types = ["company"]
        - removed_player_types = []
    """

    role_name: str
    added_player_types: list[str] = field(default_factory=list)
    removed_player_types: list[str] = field(default_factory=list)

    def has_changes(self) -> bool:
        """Check if there are any player type changes."""
        return bool(self.added_player_types or self.removed_player_types)


@dataclass
class RoleCardinalityChange:
    """Represents a change in role cardinality constraints.

    Tracks cardinality (min, max) changes on roles.
    None values indicate unbounded.
    """

    role_name: str
    old_cardinality: tuple[int | None, int | None]  # (min, max)
    new_cardinality: tuple[int | None, int | None]  # (min, max)


@dataclass
class EntityChanges:
    """Represents changes to an entity type."""

    added_attributes: list[str] = field(default_factory=list)
    removed_attributes: list[str] = field(default_factory=list)
    modified_attributes: list[AttributeFlagChange] = field(default_factory=list)

    def has_changes(self) -> bool:
        """Check if there are any changes."""
        return bool(self.added_attributes or self.removed_attributes or self.modified_attributes)


@dataclass
class RelationChanges:
    """Represents changes to a relation type.

    Tracks:
    - Role additions/removals
    - Role player type changes (which entities can play each role)
    - Role cardinality changes
    - Attribute additions/removals/modifications
    """

    added_roles: list[str] = field(default_factory=list)
    removed_roles: list[str] = field(default_factory=list)
    modified_role_players: list[RolePlayerChange] = field(default_factory=list)
    modified_role_cardinality: list[RoleCardinalityChange] = field(default_factory=list)
    added_attributes: list[str] = field(default_factory=list)
    removed_attributes: list[str] = field(default_factory=list)
    modified_attributes: list[AttributeFlagChange] = field(default_factory=list)

    def has_changes(self) -> bool:
        """Check if there are any changes."""
        return bool(
            self.added_roles
            or self.removed_roles
            or self.modified_role_players
            or self.modified_role_cardinality
            or self.added_attributes
            or self.removed_attributes
            or self.modified_attributes
        )


@dataclass
class SchemaDiff:
    """Container for schema comparison results.

    Represents the differences between two schemas for migration planning.
    """

    # Entity changes
    added_entities: set[type[Entity]] = field(default_factory=set)
    removed_entities: set[type[Entity]] = field(default_factory=set)

    # Relation changes
    added_relations: set[type[Relation]] = field(default_factory=set)
    removed_relations: set[type[Relation]] = field(default_factory=set)

    # Attribute changes
    added_attributes: set[type[Attribute]] = field(default_factory=set)
    removed_attributes: set[type[Attribute]] = field(default_factory=set)

    # Detailed changes (entity/relation modifications)
    modified_entities: dict[type[Entity], EntityChanges] = field(default_factory=dict)
    modified_relations: dict[type[Relation], RelationChanges] = field(default_factory=dict)

    def has_changes(self) -> bool:
        """Check if there are any schema differences.

        Returns:
            True if any changes exist, False otherwise
        """
        return bool(
            self.added_entities
            or self.removed_entities
            or self.added_relations
            or self.removed_relations
            or self.added_attributes
            or self.removed_attributes
            or self.modified_entities
            or self.modified_relations
        )

    def summary(self) -> str:
        """Generate a human-readable summary of changes.

        Returns:
            Formatted summary string
        """
        lines = []
        lines.append("Schema Comparison Summary")
        lines.append("=" * 50)

        if not self.has_changes():
            lines.append("No schema changes detected.")
            return "\n".join(lines)

        if self.added_entities:
            lines.append(f"\nAdded Entities ({len(self.added_entities)}):")
            for entity in sorted(self.added_entities, key=lambda e: e.__name__):
                lines.append(f"  + {entity.__name__}")

        if self.removed_entities:
            lines.append(f"\nRemoved Entities ({len(self.removed_entities)}):")
            for entity in sorted(self.removed_entities, key=lambda e: e.__name__):
                lines.append(f"  - {entity.__name__}")

        if self.added_relations:
            lines.append(f"\nAdded Relations ({len(self.added_relations)}):")
            for relation in sorted(self.added_relations, key=lambda r: r.__name__):
                lines.append(f"  + {relation.__name__}")

        if self.removed_relations:
            lines.append(f"\nRemoved Relations ({len(self.removed_relations)}):")
            for relation in sorted(self.removed_relations, key=lambda r: r.__name__):
                lines.append(f"  - {relation.__name__}")

        if self.added_attributes:
            lines.append(f"\nAdded Attributes ({len(self.added_attributes)}):")
            for attr in sorted(self.added_attributes, key=lambda a: a.get_attribute_name()):
                lines.append(f"  + {attr.get_attribute_name()}")

        if self.removed_attributes:
            lines.append(f"\nRemoved Attributes ({len(self.removed_attributes)}):")
            for attr in sorted(self.removed_attributes, key=lambda a: a.get_attribute_name()):
                lines.append(f"  - {attr.get_attribute_name()}")

        if self.modified_entities:
            lines.append(f"\nModified Entities ({len(self.modified_entities)}):")
            for entity, changes in self.modified_entities.items():
                lines.append(f"  ~ {entity.__name__}")
                if changes.added_attributes:
                    lines.append(f"    added_attributes: {changes.added_attributes}")
                if changes.removed_attributes:
                    lines.append(f"    removed_attributes: {changes.removed_attributes}")
                if changes.modified_attributes:
                    lines.append("    modified_attributes:")
                    for attr_change in changes.modified_attributes:
                        lines.append(f"      - {attr_change.name}:")
                        lines.append(f"          old: {attr_change.old_flags}")
                        lines.append(f"          new: {attr_change.new_flags}")

        if self.modified_relations:
            lines.append(f"\nModified Relations ({len(self.modified_relations)}):")
            for relation, rel_changes in self.modified_relations.items():
                relation_changes: RelationChanges = rel_changes
                lines.append(f"  ~ {relation.__name__}")
                if relation_changes.added_roles:
                    lines.append(f"    added_roles: {relation_changes.added_roles}")
                if relation_changes.removed_roles:
                    lines.append(f"    removed_roles: {relation_changes.removed_roles}")
                if relation_changes.modified_role_players:
                    lines.append("    modified_role_players:")
                    for rpc in relation_changes.modified_role_players:
                        lines.append(f"      - {rpc.role_name}:")
                        if rpc.added_player_types:
                            lines.append(f"          added: {rpc.added_player_types}")
                        if rpc.removed_player_types:
                            lines.append(f"          removed: {rpc.removed_player_types}")
                if relation_changes.modified_role_cardinality:
                    lines.append("    modified_role_cardinality:")
                    for rcc in relation_changes.modified_role_cardinality:
                        lines.append(f"      - {rcc.role_name}:")
                        lines.append(f"          old: {rcc.old_cardinality}")
                        lines.append(f"          new: {rcc.new_cardinality}")
                if relation_changes.added_attributes:
                    lines.append(f"    added_attributes: {relation_changes.added_attributes}")
                if relation_changes.removed_attributes:
                    lines.append(f"    removed_attributes: {relation_changes.removed_attributes}")
                if relation_changes.modified_attributes:
                    lines.append("    modified_attributes:")
                    for attr_change in relation_changes.modified_attributes:
                        lines.append(f"      - {attr_change.name}:")
                        lines.append(f"          old: {attr_change.old_flags}")
                        lines.append(f"          new: {attr_change.new_flags}")

        return "\n".join(lines)
