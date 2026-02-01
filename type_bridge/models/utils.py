"""Utility functions and dataclasses for TypeDB model classes."""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime as datetime_type
from typing import Any, Literal, get_args, get_origin

from pydantic import BeforeValidator

from type_bridge.attribute import (
    Attribute,
    AttributeFlags,
    Boolean,
    DateTime,
    Double,
    Integer,
    String,
)
from type_bridge.validation import validate_type_name as validate_reserved_word


def attribute_wrapper(attr_class: type[Attribute]):
    """Create a validator that wraps raw values in the attribute class.

    Returns a Pydantic BeforeValidator that converts raw values (str, int, etc.)
    into instances of the specified Attribute class.
    """

    def wrap(v: Any) -> Any:
        # If it's already an Attribute instance, return as is
        if isinstance(v, attr_class):
            return v
        # If None, return None (Pydantic handles Optional separately)
        if v is None:
            return None
        # Wrap the raw value
        return attr_class(v)

    return BeforeValidator(wrap)


@dataclass
class MatchClauseInfo:
    """Information needed to build a TypeQL match clause for an instance.

    Used by ModelManager to build unified CRUD queries for both entities and relations.

    Attributes:
        main_clause: The primary match clause (e.g., "$e isa person, has Name \"Alice\"")
        extra_clauses: Additional match clauses (e.g., role player matches for relations)
        var_name: The main variable name (e.g., "$e" or "$r")
    """

    main_clause: str
    extra_clauses: list[str]
    var_name: str


@dataclass
class WriteQueryInfo:
    """Information needed to build a TypeQL write query for an instance.

    Supports both entities (which only need insert/put pattern) and relations
    (which need match clause for role players + insert/put pattern).

    Attributes:
        match_clause: Optional match clause (for relations with role players)
        write_pattern: The insert/put pattern
    """

    match_clause: str | None
    write_pattern: str


@dataclass
class FieldInfo:
    """Information extracted from a field type annotation.

    Attributes:
        attr_type: The Attribute subclass (e.g., Name, Age)
        card_min: Minimum cardinality (None means use default)
        card_max: Maximum cardinality (None means unbounded)
        is_key: Whether this field is marked as @key
        is_unique: Whether this field is marked as @unique
    """

    attr_type: type[Attribute] | None = None
    card_min: int | None = 1
    card_max: int | None = 1
    is_key: bool = False
    is_unique: bool = False


@dataclass
class ModelAttrInfo:
    """Metadata for an attribute owned by an Entity or Relation.

    Attributes:
        typ: The Attribute subclass (e.g., Name, Age)
        flags: The AttributeFlags with key/unique/card annotations
    """

    typ: type[Attribute]
    flags: AttributeFlags


def extract_metadata(field_type: type) -> FieldInfo:
    """Extract attribute type, cardinality, and key/unique metadata from a type annotation.

    Handles:
    - Optional[Name] → FieldInfo(Name, 0, 1, False, False)
    - Key[Name] → FieldInfo(Name, 1, 1, True, False)
    - Unique[Email] → FieldInfo(Email, 1, 1, False, True)
    - Name → FieldInfo(Name, 1, 1, False, False)
    - list[Tag] → FieldInfo(Tag, None, None, False, False) - cardinality set by Flag(Card(...))

    Args:
        field_type: The type annotation from __annotations__

    Returns:
        FieldInfo with extracted metadata
    """
    origin = get_origin(field_type)
    args = get_args(field_type)

    # Default cardinality: exactly one (1,1)
    info = FieldInfo(card_min=1, card_max=1)

    # Handle Union types (Optional[T] or Literal[...] | T)
    from types import UnionType

    if origin is UnionType or str(origin) == "typing.Union":
        # Check if it's Optional (has None in args)
        has_none = type(None) in args or None in args

        if has_none:
            # Optional[T] is Union[T, None]
            for arg in args:
                if arg is not type(None) and arg is not None:
                    # Recursively extract from the non-None type
                    nested_info = extract_metadata(arg)
                    if nested_info.attr_type:
                        # Optional means 0 or 1
                        nested_info.card_min = 0
                        nested_info.card_max = 1
                        return nested_info
        else:
            # Not Optional - might be Literal[...] | AttributeType
            # Look for an Attribute subclass in the union args
            for arg in args:
                try:
                    if isinstance(arg, type) and issubclass(arg, Attribute):
                        # Found the Attribute type - use it
                        info.attr_type = arg
                        info.card_min = 1
                        info.card_max = 1
                        return info
                except TypeError:
                    continue

    # Handle list[Type] annotations
    if origin is list and len(args) >= 1:
        # Extract the attribute type from list[AttributeType]
        list_item_type = args[0]
        try:
            if isinstance(list_item_type, type) and issubclass(list_item_type, Attribute):
                # Found an Attribute type in the list
                info.attr_type = list_item_type
                # Don't set card_min/card_max here - let Flag(Card(...)) handle it
                # or use default multi-value cardinality
                return info
        except TypeError:
            pass

    # Handle Key[T] and Unique[T] type aliases
    elif origin is not None:
        origin_name = str(origin)

        # Check for Key/Unique type aliases
        if "Key" in origin_name and len(args) >= 1:
            info.is_key = True
            info.card_min, info.card_max = 1, 1
            info.attr_type = args[0]
            # Check if attr_type is an Attribute subclass
            try:
                if isinstance(info.attr_type, type) and issubclass(info.attr_type, Attribute):
                    return info
            except TypeError:
                pass
        elif "Unique" in origin_name and len(args) >= 1:
            info.is_unique = True
            info.card_min, info.card_max = 1, 1
            info.attr_type = args[0]
            # Check if attr_type is an Attribute subclass
            try:
                if isinstance(info.attr_type, type) and issubclass(info.attr_type, Attribute):
                    return info
            except TypeError:
                pass

    # Handle plain Attribute types
    else:
        try:
            if isinstance(field_type, type) and issubclass(field_type, Attribute):
                info.attr_type = field_type
                return info
        except TypeError:
            pass

    return info


def get_base_type_for_attribute(attr_cls: type[Attribute]) -> type | None:
    """Get the base Python type for an Attribute class.

    Args:
        attr_cls: The Attribute subclass (e.g., Name which inherits from String)

    Returns:
        The corresponding base Python type (str, int, float, bool, datetime)
    """
    # Check the MRO (method resolution order) to find the base Attribute type
    for base in attr_cls.__mro__:
        if base is String:
            return str
        elif base is Integer:
            return int
        elif base is Double:
            return float
        elif base is Boolean:
            return bool
        elif base is DateTime:
            return datetime_type
    return None


# TypeDB built-in type names that cannot be used
TYPEDB_BUILTIN_TYPES = {"thing", "entity", "relation", "attribute"}


def validate_type_name(
    type_name: str,
    class_name: str,
    context: Literal["entity", "relation", "attribute", "role"] = "entity",
) -> None:
    """Validate that a type name doesn't conflict with TypeDB built-ins or TypeQL keywords.

    Args:
        type_name: The type name to validate
        class_name: The Python class name (for error messages)
        context: The type context ("entity", "relation", "attribute", "role")

    Raises:
        ValueError: If type name conflicts with a TypeDB built-in type
        ReservedWordError: If type name is a TypeQL reserved word
    """
    # First check TypeDB built-in types (thing, entity, relation, attribute)
    if type_name.lower() in TYPEDB_BUILTIN_TYPES:
        raise ValueError(
            f"Type name '{type_name}' for class '{class_name}' conflicts with TypeDB built-in type. "
            f"Built-in types are: {', '.join(sorted(TYPEDB_BUILTIN_TYPES))}. "
            f"Please use a different type_name in TypeFlags or rename your class."
        )

    # Then check TypeQL reserved words
    # This will raise ReservedWordError if type_name is reserved
    validate_reserved_word(type_name, context)
