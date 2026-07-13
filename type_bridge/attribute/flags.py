"""Flag system for TypeDB attribute annotations."""

import re
from dataclasses import dataclass, field
from enum import Enum
from typing import Annotated, Any, TypeVar

T = TypeVar("T")

# Key marker type
type Key[T] = Annotated[T, "key"]
# Unique marker type
type Unique[T] = Annotated[T, "unique"]
# Ordered marker type — declares this owns clause as a list attribute (`owns attr[]`)
type Ordered[T] = Annotated[T, "ordered"]
# Distinct marker type — requires the Ordered marker; emits `@distinct` on the owns clause
type Distinct[T] = Annotated[T, "distinct"]


class Doc:
    """Documentation marker for the TypeDB 3.12+ ``@doc("...")`` annotation.

    Use with ``Flag(Doc("..."))`` on an ownership, or pass ``doc="..."`` to
    ``TypeFlags`` / ``AttributeFlags`` for type-level documentation.

    Example:
        class Person(Entity):
            name: Name = Flag(Key, Doc("full legal name"))
    """

    def __init__(self, text: str):
        self.text = text


class Meta:
    """Metadata marker for the TypeDB 3.12+ ``@meta("key", "value")`` annotation.

    TypeDB stores at most one value per key per subject; both key and value
    must be strings. Use with ``Flag(Meta("key", "value"))`` on an ownership,
    or pass ``meta={"key": "value"}`` to ``TypeFlags`` / ``AttributeFlags``
    for type-level metadata.

    Example:
        class Person(Entity):
            name: Name = Flag(Meta("column", "name"))
    """

    def __init__(self, key: str, value: str):
        self.key = key
        self.value = value


class TypeNameCase(Enum):
    """Type name case formatting options for Entity and Relation types.

    Options:
        LOWERCASE: Convert class name to lowercase (default)
                   Example: PersonName → personname
        CLASS_NAME: Keep class name as-is (PascalCase)
                    Example: PersonName → PersonName
        SNAKE_CASE: Convert class name to snake_case
                    Example: PersonName → person_name
    """

    LOWERCASE = "lowercase"
    CLASS_NAME = "classname"
    SNAKE_CASE = "snake_case"


def _to_snake_case(name: str) -> str:
    """Convert a PascalCase or camelCase string to snake_case.

    Args:
        name: The class name to convert

    Returns:
        The snake_case version of the name

    Examples:
        >>> _to_snake_case("PersonName")
        'person_name'
        >>> _to_snake_case("HTTPResponse")
        'http_response'
        >>> _to_snake_case("SimpleClass")
        'simple_class'
    """
    # Insert an underscore before any uppercase letter that follows a lowercase letter
    # or a digit, or before uppercase letters that are followed by lowercase letters
    s1 = re.sub("(.)([A-Z][a-z]+)", r"\1_\2", name)
    # Insert an underscore before any uppercase letter that follows a lowercase letter or digit
    return re.sub("([a-z0-9])([A-Z])", r"\1_\2", s1).lower()


def format_type_name(class_name: str, case: TypeNameCase) -> str:
    """Format a class name according to the specified case style.

    Args:
        class_name: The Python class name
        case: The case formatting style to apply

    Returns:
        The formatted type name

    Examples:
        >>> format_type_name("PersonName", TypeNameCase.LOWERCASE)
        'personname'
        >>> format_type_name("PersonName", TypeNameCase.CLASS_NAME)
        'PersonName'
        >>> format_type_name("PersonName", TypeNameCase.SNAKE_CASE)
        'person_name'
    """
    if case == TypeNameCase.LOWERCASE:
        return class_name.lower()
    elif case == TypeNameCase.CLASS_NAME:
        return class_name
    elif case == TypeNameCase.SNAKE_CASE:
        return _to_snake_case(class_name)
    else:
        # Default to lowercase for unknown cases
        return class_name.lower()


@dataclass
class TypeFlags:
    """Metadata flags for Entity and Relation classes.

    Args:
        name: TypeDB type name (if None, uses class name with case formatting)
        abstract: Whether this is an abstract type
        base: Whether this is a Python base class that should not appear in TypeDB schema
        case: Case formatting for auto-generated type names (default: CLASS_NAME)

    Example:
        class Person(Entity):
            flags = TypeFlags(name="person")
            name: Name

        class PersonName(Entity):
            flags = TypeFlags()  # → PersonName (default CLASS_NAME)
            name: Name

        class PersonName(Entity):
            flags = TypeFlags(case=TypeNameCase.SNAKE_CASE)  # → person_name
            name: Name

        class AbstractPerson(Entity):
            flags = TypeFlags(abstract=True)
            name: Name

        class BaseEntity(Entity):
            flags = TypeFlags(base=True)  # Python base class only
            # Children skip this in TypeDB hierarchy
    """

    name: str | None = None
    abstract: bool = False
    base: bool = False
    case: TypeNameCase = TypeNameCase.CLASS_NAME
    doc: str | None = None
    meta: dict[str, str] = field(default_factory=dict)

    def __init__(
        self,
        name: str | None = None,
        abstract: bool = False,
        base: bool = False,
        case: TypeNameCase = TypeNameCase.CLASS_NAME,
        doc: str | None = None,
        meta: dict[str, str] | None = None,
    ):
        """Initialize TypeFlags.

        Args:
            name: TypeDB type name (if None, uses class name with case formatting)
            abstract: Whether this is an abstract type
            base: Whether this is a Python base class that should not appear in TypeDB schema
            case: Case formatting for auto-generated type names (default: CLASS_NAME)
            doc: TypeDB 3.12+ ``@doc("...")`` documentation for the type
            meta: TypeDB 3.12+ ``@meta("key", "value")`` annotations, one value per key
        """
        self.name = name
        self.abstract = abstract
        self.base = base
        self.case = case
        self.doc = doc
        self.meta = dict(meta) if meta else {}


class Card:
    """Cardinality marker for TypeDB ``@card`` annotations.

    Use ``Card`` with ``Flag(Card(...))`` for multi-value attribute ownership,
    with ``Role(..., cardinality=Card(...))`` for relates-side cardinality, and
    with ``Role(..., plays_cardinality=Card(...))`` for plays-side cardinality.
    For optional single-value attributes, use ``Type | None`` instead.

    Args:
        min: Minimum cardinality (default: None, which means unspecified)
        max: Maximum cardinality (default: None, which means unbounded)

    Examples:
        tags: list[Tag] = Flag(Card(min=2))      # owns tag @card(2..)
        jobs: list[Job] = Flag(Card(1, 5))       # owns job @card(1..5)
        owner: Role[Company] = Role(
            "owner",
            Company,
            cardinality=Card(1, 1),              # relates owner @card(1..1)
            plays_cardinality=Card(0, 1),        # plays rel:owner @card(0..1)
        )

        # INCORRECT - use Optional[Type] instead:
        # age: Age = Flag(Card(min=0, max=1))    # ❌ Wrong!
        age: Optional[Age]                        # ✓ Correct
    """

    def __init__(self, *args: int, min: int | None = None, max: int | None = None):
        """Initialize cardinality marker.

        Supports both positional and keyword arguments:
        - Card(1, 5) → min=1, max=5
        - Card(min=2) → min=2, max=None (unbounded)
        - Card(max=5) → min=0, max=5 (defaults min to 0)
        - Card(min=0, max=10) → min=0, max=10
        """
        self.min: int | None = None
        self.max: int | None = None
        if args:
            # Positional arguments: Card(1, 5) or Card(2)
            if len(args) == 1:
                self.min = args[0]
                self.max = max  # Use keyword arg if provided
            elif len(args) == 2:
                self.min = args[0]
                self.max = args[1]
            else:
                raise ValueError("Card accepts at most 2 positional arguments")
        else:
            # Keyword arguments only
            # If only max is specified, default min to 0
            if min is None and max is not None:
                self.min = 0
                self.max = max
            else:
                self.min = min
                self.max = max


@dataclass
class AttributeFlags:
    """Metadata for attribute ownership and type configuration.

    Represents TypeDB ownership annotations like @key, @card(min..max), @unique,
    and allows overriding the attribute type name with explicit name or case formatting.

    Example:
        class Person(Entity):
            name: Name = Flag(Key)                    # @key (implies @card(1..1))
            email: Email = Flag(Unique)               # @unique @card(1..1)
            age: Optional[Age]                        # @card(0..1) - no Flag needed
            tags: list[Tag] = Flag(Card(min=2))       # @card(2..)
            jobs: list[Job] = Flag(Card(1, 5))        # @card(1..5)

        # Override attribute type name explicitly
        class Name(String):
            flags = AttributeFlags(name="name")

        # Or use case formatting
        class PersonName(String):
            flags = AttributeFlags(case=TypeNameCase.SNAKE_CASE)  # -> person_name
    """

    is_key: bool = False
    is_unique: bool = False
    is_ordered: bool = False
    is_distinct: bool = False
    card_min: int | None = None
    card_max: int | None = None
    has_explicit_card: bool = False  # Track if Card(...) was explicitly used
    name: str | None = None  # Override attribute type name explicitly
    case: "TypeNameCase | None" = None  # Case formatting for type name
    doc: str | None = None  # TypeDB 3.12+ @doc("...") annotation
    meta: dict[str, str] = field(default_factory=dict)  # TypeDB 3.12+ @meta annotations

    def to_typeql_annotations(self) -> list[str]:
        """Convert to TypeQL annotations like @key, @card(0..5).

        Rules:
        - @key implies @card(1..1), so never output @card with @key
        - @unique with @card(1..1) is redundant, so omit @card in that case
        - Otherwise, always output @card if cardinality is specified
        - @doc / @meta follow the constraint annotations, matching the
          annotation order of TypeDB's schema export

        Returns:
            List of TypeQL annotation strings
        """
        from type_bridge.typeql.annotations import (
            format_card_annotation,
            format_doc_meta_annotations,
        )

        annotations = []
        if self.is_key:
            annotations.append("@key")
        if self.is_unique:
            annotations.append("@unique")

        # Only output @card if:
        # 1. Not a @key (since @key always implies @card(1..1))
        # 2. Not (@unique with default @card(1..1))
        should_output_card = self.card_min is not None or self.card_max is not None

        if should_output_card and not self.is_key:
            # Check if it's @unique with default (1,1) - if so, omit @card
            is_default_card = self.card_min == 1 and self.card_max == 1
            if not (self.is_unique and is_default_card):
                card_annotation = format_card_annotation(self.card_min, self.card_max)
                if card_annotation:
                    annotations.append(card_annotation)

        annotations.extend(format_doc_meta_annotations(self.doc, self.meta))
        return annotations


def Flag(*annotations: Any) -> Annotated[Any, AttributeFlags]:
    """Create attribute flags for Key, Unique, and Card markers.

    Usage:
        field: Type = Flag(Key)                   # @key (implies @card(1..1))
        field: Type = Flag(Unique)                # @unique @card(1..1)
        field: list[Type] = Flag(Card(min=2))     # @card(2..)
        field: list[Type] = Flag(Card(1, 5))      # @card(1..5)
        field: Type = Flag(Key, Unique)           # @key @unique
        field: list[Type] = Flag(Key, Card(min=1)) # @key @card(1..)

    For optional single values, use Optional[Type] instead:
        field: Optional[Type]  # @card(0..1) - no Flag needed

    Args:
        *annotations: Variable number of Key, Unique, or Card marker instances

    Returns:
        AttributeFlags instance with the specified flags

    Example:
        class Person(Entity):
            flags = TypeFlags(name="person")
            name: Name = Flag(Key)                    # @key (implies @card(1..1))
            email: Email = Flag(Key, Unique)          # @key @unique
            age: Optional[Age]                        # @card(0..1)
            tags: list[Tag] = Flag(Card(min=2))       # @card(2..)
            jobs: list[Job] = Flag(Card(1, 5))        # @card(1..5)
    """
    flags = AttributeFlags()
    has_card = False

    for ann in annotations:
        if ann is Key:
            flags.is_key = True
        elif ann is Unique:
            flags.is_unique = True
        elif ann is Ordered:
            flags.is_ordered = True
        elif ann is Distinct:
            flags.is_distinct = True
        elif isinstance(ann, Card):
            # Extract cardinality from Card instance
            flags.card_min = ann.min
            flags.card_max = ann.max
            flags.has_explicit_card = True
            has_card = True
        elif isinstance(ann, Doc):
            flags.doc = ann.text
        elif isinstance(ann, Meta):
            flags.meta[ann.key] = ann.value

    # Distinct requires Ordered: the TypeDB list form (`owns attr[]`) is the
    # only form the engine accepts @distinct on.
    if flags.is_distinct and not flags.is_ordered:
        raise ValueError(
            "Flag(Distinct) requires Flag(Ordered): @distinct is only valid on a "
            "list attribute (`owns attr[]`). Use Flag(Ordered, Distinct) together."
        )

    # If Key was used but no Card, set default card(1,1)
    if flags.is_key and not has_card:
        flags.card_min = 1
        flags.card_max = 1

    return flags
