"""Data structures describing a parsed TypeDB schema.

These models represent the intermediate representation (IR) between raw TQL
and generated Python code. They capture all schema information needed for
code generation while abstracting away TQL syntax details.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from collections.abc import Iterable

# Type aliases for annotation values
# Scalar types: bool (flag), int, float, str
AnnotationScalar = bool | int | float | str
# Full annotation value: scalar or list of scalars (e.g., @tags(api, public))
AnnotationValue = AnnotationScalar | list[AnnotationScalar]


@dataclass(frozen=True, slots=True)
class Cardinality:
    """Cardinality constraint from @card annotation.

    Examples:
        @card(0..1) -> Cardinality(0, 1)  - optional single
        @card(1)    -> Cardinality(1, 1)  - required single
        @card(0..)  -> Cardinality(0, None) - optional multi
        @card(1..)  -> Cardinality(1, None) - required multi
        @card(1..3) -> Cardinality(1, 3)  - bounded multi
    """

    min: int
    max: int | None  # None means unbounded

    @property
    def is_required(self) -> bool:
        """True if at least one value is required."""
        return self.min >= 1

    @property
    def is_single(self) -> bool:
        """True if at most one value is allowed."""
        return self.max == 1

    @property
    def is_multi(self) -> bool:
        """True if multiple values are allowed."""
        return self.max is None or self.max > 1

    @property
    def is_optional_single(self) -> bool:
        """True if zero or one value (the default)."""
        return self.min == 0 and self.max == 1


@dataclass(frozen=True, slots=True)
class AttributeSpec:
    """Attribute definition extracted from a TypeDB schema.

    Attributes:
        name: The TypeDB attribute name (e.g., "isbn-13")
        value_type: The TypeDB value type (string, integer, double, etc.)
        parent: Parent attribute name for inheritance (sub)
        abstract: Whether this is an abstract attribute
        independent: Whether this is an independent attribute (@independent)
        regex: Regex constraint from @regex annotation
        allowed_values: Enum values from @values annotation
        range_min: Minimum value from @range annotation
        range_max: Maximum value from @range annotation
        docstring: Documentation extracted from comments
        annotations: Custom annotations from TQL comments (e.g., # @default(new))
    """

    name: str
    value_type: str  # string, integer, double, boolean, datetime, etc.
    parent: str | None = None
    abstract: bool = False
    independent: bool = False
    regex: str | None = None
    allowed_values: tuple[str, ...] | None = None
    range_min: str | None = None  # String to support integers, floats, and dates
    range_max: str | None = None
    docstring: str | None = None
    annotations: dict[str, AnnotationValue] = field(default_factory=dict)

    # Legacy fields for custom annotations (can be removed if not needed)
    default: object | None = None
    transform: str | None = None
    case: str | None = None


@dataclass(slots=True)
class EntitySpec:
    """Entity definition extracted from a TypeDB schema.

    Attributes:
        name: The TypeDB entity name (e.g., "person")
        parent: Parent entity name for inheritance (sub)
        owns: Set of owned attribute names
        owns_order: Ordered list preserving TQL declaration order
        plays: Set of role references (e.g., "friendship:friend")
        abstract: Whether this is an abstract entity
        keys: Attributes marked with @key
        uniques: Attributes marked with @unique
        cascades: Attributes marked with @cascade
        ordered_owns: Attributes declared as list (`owns attr[]`)
        distinct_owns: Attributes declared with @distinct on the owns clause
        subkeys: Attributes -> subkey group name mapping (from @subkey)
        cardinalities: Per-attribute cardinality constraints
        plays_cardinalities: Per-role cardinality constraints (e.g., plays friendship:friend @card(0..5))
        annotations: Custom annotations from TQL comments (e.g., # @prefix(PROJ))
    """

    name: str
    parent: str | None = None
    owns: set[str] = field(default_factory=set)
    owns_order: list[str] = field(default_factory=list)
    plays: set[str] = field(default_factory=set)
    abstract: bool = False
    keys: set[str] = field(default_factory=set)
    uniques: set[str] = field(default_factory=set)
    cascades: set[str] = field(default_factory=set)
    ordered_owns: set[str] = field(default_factory=set)
    distinct_owns: set[str] = field(default_factory=set)
    subkeys: dict[str, str] = field(default_factory=dict)
    cardinalities: dict[str, Cardinality] = field(default_factory=dict)
    plays_cardinalities: dict[str, Cardinality] = field(default_factory=dict)
    docstring: str | None = None
    annotations: dict[str, AnnotationValue] = field(default_factory=dict)

    # Legacy fields for custom annotations
    prefix: str | None = None
    internal: bool = False


@dataclass(slots=True)
class RoleSpec:
    """Role definition within a relation.

    Attributes:
        name: The role name (e.g., "author")
        overrides: Parent role this overrides (from "as" syntax)
        cardinality: Optional cardinality constraint on the role
        ordered: Whether this role is a list role (`relates name[]`)
        distinct: Whether this role has @distinct annotation
        is_abstract: Whether this role carries the @abstract annotation
        annotations: Custom annotations from TQL comments
    """

    name: str
    overrides: str | None = None  # For "relates author as contributor"
    cardinality: Cardinality | None = None
    ordered: bool = False
    distinct: bool = False
    is_abstract: bool = False
    annotations: dict[str, AnnotationValue] = field(default_factory=dict)


@dataclass(slots=True)
class RelationSpec:
    """Relation definition extracted from a TypeDB schema.

    Attributes:
        name: The TypeDB relation name (e.g., "authoring")
        parent: Parent relation name for inheritance (sub)
        roles: List of role specifications
        owns: Set of owned attribute names
        owns_order: Ordered list preserving TQL declaration order
        abstract: Whether this is an abstract relation
        cascades: Attributes marked with @cascade
        ordered_owns: Attributes declared as list (`owns attr[]`)
        distinct_owns: Attributes declared with @distinct on the owns clause
        subkeys: Attributes -> subkey group name mapping (from @subkey)
        annotations: Custom annotations from TQL comments
    """

    name: str
    parent: str | None = None
    roles: list[RoleSpec] = field(default_factory=list)
    owns: set[str] = field(default_factory=set)
    owns_order: list[str] = field(default_factory=list)
    abstract: bool = False
    keys: set[str] = field(default_factory=set)
    uniques: set[str] = field(default_factory=set)
    cascades: set[str] = field(default_factory=set)
    ordered_owns: set[str] = field(default_factory=set)
    distinct_owns: set[str] = field(default_factory=set)
    subkeys: dict[str, str] = field(default_factory=dict)
    cardinalities: dict[str, Cardinality] = field(default_factory=dict)
    docstring: str | None = None
    annotations: dict[str, AnnotationValue] = field(default_factory=dict)

    @property
    def role_names(self) -> list[str]:
        """Get list of role names for backward compatibility."""
        return [r.name for r in self.roles]

    @property
    def role_overrides(self) -> dict[str, str]:
        """Get role -> parent_role mapping for overrides."""
        return {r.name: r.overrides for r in self.roles if r.overrides}


@dataclass(slots=True)
class ParameterSpec:
    """Parameter definition for a TypeDB function.

    Attributes:
        name: The parameter name (e.g., "birth-date")
        type: The parameter type (e.g., "date")
    """

    name: str
    type: str


@dataclass(frozen=True, slots=True)
class ReturnTypeItem:
    """One return-type entry: a type token plus its optionality."""

    name: str
    optional: bool = False


@dataclass(slots=True)
class ReturnTypeSpec:
    """Structured function return type (stream flag + ordered items)."""

    is_stream: bool
    types: list[ReturnTypeItem]


@dataclass(slots=True)
class FunctionSpec:
    """Function definition extracted from a TypeDB schema.

    Attributes:
        name: The function name (e.g., "calculate-age")
        parameters: List of parameters
        return_type: Structured return type (stream flag + ordered items)
    """

    name: str
    parameters: list[ParameterSpec]
    return_type: ReturnTypeSpec
    docstring: str | None = None


@dataclass(frozen=True, slots=True)
class StructFieldSpec:
    """Field definition within a struct.

    Attributes:
        name: The field name (e.g., "first-name")
        value_type: The TypeDB value type (string, integer, etc.)
        optional: Whether the field is optional (marked with ?)
    """

    name: str
    value_type: str
    optional: bool = False


@dataclass(slots=True)
class StructSpec:
    """Struct definition extracted from a TypeDB schema.

    Structs are composite value types introduced in TypeDB 3.0.

    Attributes:
        name: The struct name (e.g., "person-name")
        fields: List of field specifications
        docstring: Documentation extracted from comments
        annotations: Custom annotations from TQL comments
    """

    name: str
    fields: list[StructFieldSpec] = field(default_factory=list)
    docstring: str | None = None
    annotations: dict[str, AnnotationValue] = field(default_factory=dict)


@dataclass
class ParsedSchema:
    """Container for all parsed schema components.

    This is the main output of the parser and input to the renderers.
    """

    attributes: dict[str, AttributeSpec] = field(default_factory=dict)
    entities: dict[str, EntitySpec] = field(default_factory=dict)
    relations: dict[str, RelationSpec] = field(default_factory=dict)
    functions: dict[str, FunctionSpec] = field(default_factory=dict)
    structs: dict[str, StructSpec] = field(default_factory=dict)

    def accumulate_inheritance(self) -> None:
        """Propagate inherited members down all type hierarchies."""
        _accumulate_entity_inheritance(self)
        _accumulate_relation_inheritance(self)


def _accumulate_entity_inheritance(schema: ParsedSchema) -> None:
    """Propagate owns/plays/keys/uniques/cardinalities down the entity hierarchy."""
    changed = True
    while changed:
        changed = False
        for entity in schema.entities.values():
            if not entity.parent or entity.parent not in schema.entities:
                continue
            parent = schema.entities[entity.parent]
            before = (
                len(entity.owns),
                len(entity.plays),
                len(entity.keys),
                len(entity.uniques),
                len(entity.cardinalities),
                len(entity.plays_cardinalities),
            )

            entity.owns |= parent.owns
            entity.plays |= parent.plays
            entity.keys |= parent.keys
            entity.uniques |= parent.uniques

            # Inherit parent cardinalities (child can override)
            for attr, card in parent.cardinalities.items():
                if attr not in entity.cardinalities:
                    entity.cardinalities[attr] = card

            # Inherit parent plays_cardinalities (child can override)
            for role, card in parent.plays_cardinalities.items():
                if role not in entity.plays_cardinalities:
                    entity.plays_cardinalities[role] = card

            # Prepend parent's owns_order (parent attrs first)
            parent_attrs = [a for a in parent.owns_order if a not in set(entity.owns_order)]
            if parent_attrs:
                entity.owns_order[:0] = parent_attrs

            after = (
                len(entity.owns),
                len(entity.plays),
                len(entity.keys),
                len(entity.uniques),
                len(entity.cardinalities),
                len(entity.plays_cardinalities),
            )
            changed = changed or after != before


def _accumulate_relation_inheritance(schema: ParsedSchema) -> None:
    """Propagate owns/roles/keys/uniques/cardinalities down the relation hierarchy."""
    changed = True
    while changed:
        changed = False
        for relation in schema.relations.values():
            if not relation.parent or relation.parent not in schema.relations:
                continue
            parent = schema.relations[relation.parent]
            before = (
                len(relation.owns),
                len(relation.roles),
                len(relation.keys),
                len(relation.uniques),
                len(relation.cardinalities),
            )

            # Inherit owns
            relation.owns |= parent.owns

            # Inherit keys and uniques
            relation.keys |= parent.keys
            relation.uniques |= parent.uniques

            # Inherit parent cardinalities (child can override)
            for attr, card in parent.cardinalities.items():
                if attr not in relation.cardinalities:
                    relation.cardinalities[attr] = card

            # Inherit roles - but child may override with "as"
            child_role_names = {r.name for r in relation.roles}
            overridden_parent_roles = set(relation.role_overrides.values())
            for parent_role in parent.roles:
                if (
                    parent_role.name not in child_role_names
                    and parent_role.name not in overridden_parent_roles
                ):
                    relation.roles.append(parent_role)

            # Prepend parent's owns_order
            parent_attrs = [a for a in parent.owns_order if a not in set(relation.owns_order)]
            if parent_attrs:
                relation.owns_order[:0] = parent_attrs

            after = (
                len(relation.owns),
                len(relation.roles),
                len(relation.keys),
                len(relation.uniques),
                len(relation.cardinalities),
            )
            changed = changed or before != after


def minimal_role_players(schema: ParsedSchema, relation: str, role: str) -> list[str]:
    """Return the minimal set of entity names that can play a given role.

    Removes ancestors when a descendant is also present, keeping MultiRole
    declarations as narrow as possible.
    """
    role_token = f"{relation}:{role}"
    players = [e.name for e in schema.entities.values() if role_token in e.plays]
    if not players:
        return []

    parent_map = {name: entity.parent for name, entity in schema.entities.items()}

    def is_ancestor(candidate: str, target: str) -> bool:
        current = parent_map.get(target)
        while current:
            if current == candidate:
                return True
            current = parent_map.get(current)
        return False

    unique_players = set(players)
    minimal = set(unique_players)
    for player in unique_players:
        for other in unique_players:
            if player != other and is_ancestor(other, player) and player in minimal:
                minimal.remove(player)
                break

    return sorted(minimal)


def to_tuple_literal(values: Iterable[str]) -> str:
    """Render a Python tuple literal of strings."""
    entries = [f'"{val}"' for val in values]
    if not entries:
        return "()"
    if len(entries) == 1:
        return f"({entries[0]},)"
    return f"({', '.join(entries)})"
