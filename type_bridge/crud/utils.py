"""Shared utilities for CRUD operations."""

from datetime import date, datetime, timedelta
from decimal import Decimal as DecimalType
from typing import TYPE_CHECKING, Any

import isodate
from isodate import Duration as IsodateDuration

from type_bridge.attribute import AttributeFlags

if TYPE_CHECKING:
    from type_bridge.models import Entity

# Cache for subclass maps (keyed by class name for hashability)
_subclass_map_cache: dict[str, dict[str, type["Entity"]]] = {}


def format_value(value: Any) -> str:
    """Format a Python value for TypeQL.

    Handles extraction from Attribute instances and converts Python types
    to their TypeQL literal representation.

    Args:
        value: Python value to format (may be wrapped in Attribute instance)

    Returns:
        TypeQL-formatted string literal

    Examples:
        >>> format_value("hello")
        '"hello"'
        >>> format_value(42)
        '42'
        >>> format_value(True)
        'true'
        >>> format_value(Decimal("123.45"))
        '123.45dec'
    """
    # Extract value from Attribute instances first
    if hasattr(value, "value"):
        value = value.value

    if isinstance(value, str):
        # Escape backslashes first, then double quotes for TypeQL string literals
        escaped = value.replace("\\", "\\\\").replace('"', '\\"')
        return f'"{escaped}"'
    elif isinstance(value, bool):
        return "true" if value else "false"
    elif isinstance(value, DecimalType):
        # TypeDB decimal literals use 'dec' suffix
        return f"{value}dec"
    elif isinstance(value, (int, float)):
        return str(value)
    elif isinstance(value, datetime):
        # TypeDB datetime/datetimetz literals are unquoted ISO 8601 strings
        return value.isoformat()
    elif isinstance(value, date):
        # TypeDB date literals are unquoted ISO 8601 date strings
        return value.isoformat()
    elif isinstance(value, (IsodateDuration, timedelta)):
        # TypeDB duration literals are unquoted ISO 8601 duration strings
        return isodate.duration_isoformat(value)
    else:
        # For other types, convert to string and escape
        str_value = str(value)
        escaped = str_value.replace("\\", "\\\\").replace('"', '\\"')
        return f'"{escaped}"'


def is_multi_value_attribute(flags: AttributeFlags) -> bool:
    """Check if attribute is multi-value based on cardinality.

    Multi-value attributes have either:
    - Unbounded cardinality (card_max is None)
    - Maximum cardinality > 1

    Single-value attributes have:
    - Maximum cardinality == 1 (including 0..1 and 1..1)

    Args:
        flags: AttributeFlags instance containing cardinality information

    Returns:
        True if multi-value (card_max is None or > 1), False if single-value

    Examples:
        >>> flags = AttributeFlags(card_min=0, card_max=1)
        >>> is_multi_value_attribute(flags)
        False
        >>> flags = AttributeFlags(card_min=0, card_max=5)
        >>> is_multi_value_attribute(flags)
        True
        >>> flags = AttributeFlags(card_min=2, card_max=None)
        >>> is_multi_value_attribute(flags)
        True
    """
    # Single-value: card_max == 1 (including 0..1 and 1..1)
    # Multi-value: card_max is None (unbounded) or > 1
    if flags.card_max is None:
        # Unbounded means multi-value
        return True
    return flags.card_max > 1


def resolve_entity_class(
    base_class: type["Entity"],
    type_name: str,
) -> type["Entity"]:
    """Resolve a TypeDB type name to the corresponding Python entity class.

    Searches through the class hierarchy starting from base_class to find
    a subclass that matches the given TypeDB type name. This enables
    polymorphic queries where a supertype query returns entities of
    different concrete subtypes.

    Args:
        base_class: The base entity class (e.g., the queried supertype)
        type_name: TypeDB type name to resolve (e.g., "user_story")

    Returns:
        The matching entity class, or base_class if no match found

    Example:
        # If querying Artifact and TypeDB returns a "user_story" entity:
        resolved = resolve_entity_class(Artifact, "user_story")
        # resolved is UserStory class (subclass of Artifact)
    """
    # Check if base class matches
    if base_class.get_type_name() == type_name:
        return base_class

    # Build subclass map and search (using cache)
    cache_key = f"{base_class.__module__}.{base_class.__name__}"
    if cache_key not in _subclass_map_cache:
        _subclass_map_cache[cache_key] = _build_subclass_map(base_class)
    subclass_map = _subclass_map_cache[cache_key]
    return subclass_map.get(type_name, base_class)


def build_metadata_fetch(var: str) -> str:
    """Build a fetch clause that retrieves only IID and type metadata.

    Uses TypeQL 3.8.0 built-in functions iid() and label() to fetch
    the internal ID and type label. This is used for queries that need
    to identify entities/relations without fetching all attributes.

    Note: TypeQL grammar doesn't allow mixing "key": value entries with $e.*
    in the same fetch clause, so metadata-only fetch is separate from
    attribute fetch.

    Args:
        var: Variable name (with or without $)

    Returns:
        Fetch clause string like 'fetch { "_iid": iid($e), "_type": label($e) }'

    Example:
        >>> build_metadata_fetch("e")
        'fetch {\\n  "_iid": iid($e), "_type": label($e)\\n}'
    """
    if not var.startswith("$"):
        var = f"${var}"

    return f'fetch {{\n  "_iid": iid({var}), "_type": label({var})\n}}'


def _build_subclass_map(base_class: type["Entity"]) -> dict[str, type["Entity"]]:
    """Build a mapping from TypeDB type names to entity classes.

    Recursively collects all subclasses of the given base class and maps
    their TypeDB type names to the Python classes.

    Args:
        base_class: The base entity class to start from

    Returns:
        Dictionary mapping TypeDB type names to entity classes
    """
    result: dict[str, type[Entity]] = {}

    def collect_subclasses(cls: type["Entity"]) -> None:
        # Add this class to the map
        try:
            type_name = cls.get_type_name()
            result[type_name] = cls
        except Exception:
            pass

        # Recursively collect from subclasses
        for subclass in cls.__subclasses__():
            collect_subclasses(subclass)

    collect_subclasses(base_class)
    return result
