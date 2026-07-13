"""Schema comparison DTOs for TypeDB schema management.

Rust/Python shape reconciliation:
- Rust schema diffs serialize added/removed type collections as type-name lists;
  Python public DTOs keep sets of Entity/Relation/Attribute classes.
- Rust modified entity/relation maps are keyed by type-name strings; Python DTOs
  are keyed by the target-side model classes.
- Rust entity/relation modified_attributes are triples ``[name, old_flags,
  new_flags]``; Python uses ``AttributeFlagChange`` objects.
- Rust root modified_attributes are attribute type definition-change maps; Python
  uses ``AttributeTypeChange`` objects keyed by attribute class where available.
- Rust role player/cardinality changes serialize as dicts; Python keeps
  ``RolePlayerChange``/``RoleCardinalityChange`` DTOs.
"""

from dataclasses import dataclass, field
from typing import Any

from type_bridge.attribute.base import Attribute
from type_bridge.models import Entity, Relation


@dataclass
class AttributeFlagChange:
    """Represents a change in attribute flags (e.g., cardinality change)."""

    name: str
    old_flags: str
    new_flags: str


@dataclass
class AttributeTypeChange:
    """Represents a change in a standalone attribute type definition."""

    name: str
    changes: dict[str, Any]


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
class RoleAnnotationChange:
    """Represents a change in a role's @doc/@meta annotations (TypeDB 3.12+)."""

    role_name: str
    doc_changed: tuple[str | None, str | None] | None = None
    meta_changed: tuple[dict[str, str], dict[str, str]] | None = None


@dataclass
class EntityChanges:
    """Represents changes to an entity type."""

    added_attributes: list[str] = field(default_factory=list)
    removed_attributes: list[str] = field(default_factory=list)
    modified_attributes: list[AttributeFlagChange] = field(default_factory=list)
    doc_changed: tuple[str | None, str | None] | None = None
    meta_changed: tuple[dict[str, str], dict[str, str]] | None = None

    def has_changes(self) -> bool:
        """Check if there are any changes."""
        return bool(
            self.added_attributes
            or self.removed_attributes
            or self.modified_attributes
            or self.doc_changed
            or self.meta_changed
        )


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
    modified_role_annotations: list[RoleAnnotationChange] = field(default_factory=list)
    added_attributes: list[str] = field(default_factory=list)
    removed_attributes: list[str] = field(default_factory=list)
    modified_attributes: list[AttributeFlagChange] = field(default_factory=list)
    doc_changed: tuple[str | None, str | None] | None = None
    meta_changed: tuple[dict[str, str], dict[str, str]] | None = None

    def has_changes(self) -> bool:
        """Check if there are any changes."""
        return bool(
            self.added_roles
            or self.removed_roles
            or self.modified_role_players
            or self.modified_role_cardinality
            or self.modified_role_annotations
            or self.added_attributes
            or self.removed_attributes
            or self.modified_attributes
            or self.doc_changed
            or self.meta_changed
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
    modified_attributes: dict[type[Attribute], AttributeTypeChange] = field(default_factory=dict)

    # Detailed changes (entity/relation modifications)
    modified_entities: dict[type[Entity], EntityChanges] = field(default_factory=dict)
    modified_relations: dict[type[Relation], RelationChanges] = field(default_factory=dict)
    _rust_diff: dict[str, Any] | None = field(default=None, repr=False, compare=False)

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
            or self.modified_attributes
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

        if self.modified_attributes:
            lines.append(f"\nModified Attributes ({len(self.modified_attributes)}):")
            for attr, changes in sorted(
                self.modified_attributes.items(), key=lambda item: item[0].get_attribute_name()
            ):
                fields = ", ".join(_attribute_type_changed_fields(changes.changes))
                lines.append(f"  ~ {attr.get_attribute_name()}: {fields}")

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

    def to_rust_dict(self) -> dict[str, Any]:
        """Serialize this public DTO to the Rust ``SchemaDiff`` dict shape.

        Converts Python class-keyed sets/dicts into the string-keyed lists and
        nested dicts the PyO3 layer expects, reconciling field shapes as described
        in the module docstring.
        """
        return {
            "added_entities": sorted(_model_type_name(model) for model in self.added_entities),
            "removed_entities": sorted(_model_type_name(model) for model in self.removed_entities),
            "modified_entities": {
                _model_type_name(model): _entity_changes_to_rust(changes)
                for model, changes in self.modified_entities.items()
            },
            "added_relations": sorted(_model_type_name(model) for model in self.added_relations),
            "removed_relations": sorted(
                _model_type_name(model) for model in self.removed_relations
            ),
            "modified_relations": {
                _model_type_name(model): _relation_changes_to_rust(changes)
                for model, changes in self.modified_relations.items()
            },
            "added_attributes": sorted(attr.get_attribute_name() for attr in self.added_attributes),
            "removed_attributes": sorted(
                attr.get_attribute_name() for attr in self.removed_attributes
            ),
            "modified_attributes": {
                attr.get_attribute_name(): change.changes
                for attr, change in self.modified_attributes.items()
            },
        }


def from_rust_schema_diff(
    rust_diff: dict[str, Any],
    *,
    current_schema: Any | None = None,
    target_schema: Any | None = None,
) -> SchemaDiff:
    """Map the serialized Rust diff into the frozen Python DTO shape."""
    current_entities = _entities_by_name(current_schema)
    target_entities = _entities_by_name(target_schema)
    current_relations = _relations_by_name(current_schema)
    target_relations = _relations_by_name(target_schema)
    current_attributes = _attributes_by_name(current_schema)
    target_attributes = _attributes_by_name(target_schema)

    diff = SchemaDiff(
        added_entities={
            target_entities[name]
            for name in rust_diff.get("added_entities", [])
            if name in target_entities
        },
        removed_entities={
            current_entities[name]
            for name in rust_diff.get("removed_entities", [])
            if name in current_entities
        },
        added_relations={
            target_relations[name]
            for name in rust_diff.get("added_relations", [])
            if name in target_relations
        },
        removed_relations={
            current_relations[name]
            for name in rust_diff.get("removed_relations", [])
            if name in current_relations
        },
        added_attributes={
            target_attributes[name]
            for name in rust_diff.get("added_attributes", [])
            if name in target_attributes
        },
        removed_attributes={
            current_attributes[name]
            for name in rust_diff.get("removed_attributes", [])
            if name in current_attributes
        },
        _rust_diff=rust_diff,
    )

    for attr_name, changes in rust_diff.get("modified_attributes", {}).items():
        attr = target_attributes.get(attr_name) or current_attributes.get(attr_name)
        if attr is not None:
            diff.modified_attributes[attr] = AttributeTypeChange(
                name=attr_name,
                changes=dict(changes),
            )

    for type_name, changes in rust_diff.get("modified_entities", {}).items():
        entity = target_entities.get(type_name) or current_entities.get(type_name)
        if entity is not None:
            diff.modified_entities[entity] = _entity_changes_from_rust(changes)

    for type_name, changes in rust_diff.get("modified_relations", {}).items():
        relation = target_relations.get(type_name) or current_relations.get(type_name)
        if relation is not None:
            diff.modified_relations[relation] = _relation_changes_from_rust(changes)

    return diff


def _entity_changes_from_rust(changes: dict[str, Any]) -> EntityChanges:
    return EntityChanges(
        added_attributes=list(changes.get("added_attributes", [])),
        removed_attributes=list(changes.get("removed_attributes", [])),
        modified_attributes=[
            AttributeFlagChange(name=name, old_flags=old_flags, new_flags=new_flags)
            for name, old_flags, new_flags in changes.get("modified_attributes", [])
        ],
        doc_changed=_doc_changed_from_rust(changes.get("doc_changed")),
        meta_changed=_meta_changed_from_rust(changes.get("meta_changed")),
    )


def _doc_changed_from_rust(value: Any) -> tuple[str | None, str | None] | None:
    if value is None:
        return None
    old, new = value
    return (old, new)


def _meta_changed_from_rust(value: Any) -> tuple[dict[str, str], dict[str, str]] | None:
    if value is None:
        return None
    old, new = value
    return (dict(old), dict(new))


def _relation_changes_from_rust(changes: dict[str, Any]) -> RelationChanges:
    return RelationChanges(
        added_roles=list(changes.get("added_roles", [])),
        removed_roles=list(changes.get("removed_roles", [])),
        modified_role_players=[
            RolePlayerChange(
                role_name=change["role_name"],
                added_player_types=list(change.get("added_player_types", [])),
                removed_player_types=list(change.get("removed_player_types", [])),
            )
            for change in changes.get("modified_role_players", [])
        ],
        modified_role_cardinality=[
            RoleCardinalityChange(
                role_name=change["role_name"],
                old_cardinality=_cardinality_from_rust(change.get("old_cardinality")),
                new_cardinality=_cardinality_from_rust(change.get("new_cardinality")),
            )
            for change in changes.get("modified_role_cardinality", [])
        ],
        added_attributes=list(changes.get("added_attributes", [])),
        removed_attributes=list(changes.get("removed_attributes", [])),
        modified_attributes=[
            AttributeFlagChange(name=name, old_flags=old_flags, new_flags=new_flags)
            for name, old_flags, new_flags in changes.get("modified_attributes", [])
        ],
        modified_role_annotations=[
            RoleAnnotationChange(
                role_name=change["role_name"],
                doc_changed=_doc_changed_from_rust(change.get("doc_changed")),
                meta_changed=_meta_changed_from_rust(change.get("meta_changed")),
            )
            for change in changes.get("modified_role_annotations", [])
        ],
        doc_changed=_doc_changed_from_rust(changes.get("doc_changed")),
        meta_changed=_meta_changed_from_rust(changes.get("meta_changed")),
    )


def _entity_changes_to_rust(changes: EntityChanges) -> dict[str, Any]:
    return {
        "added_attributes": changes.added_attributes,
        "removed_attributes": changes.removed_attributes,
        "modified_attributes": [
            [change.name, change.old_flags, change.new_flags]
            for change in changes.modified_attributes
        ],
        "abstract_changed": None,
        "parent_changed": None,
        "doc_changed": changes.doc_changed,
        "meta_changed": changes.meta_changed,
    }


def _relation_changes_to_rust(changes: RelationChanges) -> dict[str, Any]:
    return {
        "added_attributes": changes.added_attributes,
        "removed_attributes": changes.removed_attributes,
        "modified_attributes": [
            [change.name, change.old_flags, change.new_flags]
            for change in changes.modified_attributes
        ],
        "added_roles": changes.added_roles,
        "removed_roles": changes.removed_roles,
        "modified_role_players": [
            {
                "role_name": change.role_name,
                "added_player_types": change.added_player_types,
                "removed_player_types": change.removed_player_types,
            }
            for change in changes.modified_role_players
        ],
        "modified_role_cardinality": [
            {
                "role_name": change.role_name,
                "old_cardinality": _cardinality_to_rust(change.old_cardinality),
                "new_cardinality": _cardinality_to_rust(change.new_cardinality),
            }
            for change in changes.modified_role_cardinality
        ],
        "modified_role_annotations": [
            {
                "role_name": change.role_name,
                "doc_changed": change.doc_changed,
                "meta_changed": change.meta_changed,
            }
            for change in changes.modified_role_annotations
        ],
        "abstract_changed": None,
        "parent_changed": None,
        "doc_changed": changes.doc_changed,
        "meta_changed": changes.meta_changed,
    }


def _attribute_type_changed_fields(changes: dict[str, Any]) -> list[str]:
    names = []
    for key, label in (
        ("value_type_changed", "value type"),
        ("parent_changed", "parent"),
        ("abstract_changed", "abstract"),
        ("independent_changed", "independent"),
        ("regex_changed", "regex"),
        ("allowed_values_changed", "values"),
        ("range_changed", "range"),
        ("doc_changed", "doc"),
        ("meta_changed", "meta"),
    ):
        if changes.get(key) is not None:
            names.append(label)
    return names


def _entities_by_name(schema: Any | None) -> dict[str, type[Entity]]:
    if schema is None:
        return {}
    return {entity.get_type_name(): entity for entity in schema.entities}


def _relations_by_name(schema: Any | None) -> dict[str, type[Relation]]:
    if schema is None:
        return {}
    return {relation.get_type_name(): relation for relation in schema.relations}


def _attributes_by_name(schema: Any | None) -> dict[str, type[Attribute]]:
    if schema is None:
        return {}
    return {attr.get_attribute_name(): attr for attr in schema.attribute_classes}


def _model_type_name(model: type[Entity] | type[Relation]) -> str:
    return model.get_type_name()


def _cardinality_from_rust(
    value: list[Any] | tuple[Any, Any] | None,
) -> tuple[int | None, int | None]:
    if value is None:
        return (None, None)
    return (value[0], value[1])


def _cardinality_to_rust(value: tuple[int | None, int | None]) -> list[int | None] | None:
    min_value, max_value = value
    if min_value is None and max_value is None:
        return None
    return [0 if min_value is None else min_value, max_value]
