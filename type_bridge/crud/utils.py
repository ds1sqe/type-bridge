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


def unwrap_attribute(value: Any) -> Any:
    """Extract raw value from Attribute instance.

    This utility consolidates the common pattern of extracting the underlying
    value from Attribute instances before processing.

    Args:
        value: Value that may be an Attribute instance or raw value

    Returns:
        The raw value (value.value if Attribute, otherwise value unchanged)

    Examples:
        >>> unwrap_attribute(Name("Alice"))
        "Alice"
        >>> unwrap_attribute("Alice")
        "Alice"
        >>> unwrap_attribute(42)
        42
    """
    if hasattr(value, "value"):
        return value.value
    return value


def normalize_role_players(
    role_players: dict[str, Any],
) -> tuple[dict[str, list[Any]], dict[str, list[str]]]:
    """Normalize role players to always be lists for uniform handling.

    Handles both single entities and lists of entities (for multi-cardinality roles).
    Also generates unique variable names for each player in the match clause.

    Args:
        role_players: Dict mapping role_name -> entity or list of entities

    Returns:
        Tuple of:
        - normalized_players: Dict mapping role_name -> list of entities
        - var_mapping: Dict mapping role_name -> list of variable names

    Examples:
        >>> normalize_role_players({"employee": alice, "employer": company})
        ({"employee": [alice], "employer": [company]},
         {"employee": ["employee"], "employer": ["employer"]})

        >>> normalize_role_players({"member": [alice, bob]})
        ({"member": [alice, bob]},
         {"member": ["member_0", "member_1"]})
    """
    normalized_players: dict[str, list[Any]] = {}
    var_mapping: dict[str, list[str]] = {}

    for role_name, entity_or_list in role_players.items():
        # Normalize to list
        entities = entity_or_list if isinstance(entity_or_list, list) else [entity_or_list]
        normalized_players[role_name] = entities

        # Generate variable names
        var_names = []
        for i in range(len(entities)):
            var_name = f"{role_name}_{i}" if len(entities) > 1 else role_name
            var_names.append(var_name)
        var_mapping[role_name] = var_names

    return normalized_players, var_mapping


def build_role_player_match(var_name: str, entity: Any, entity_type_name: str) -> str:
    """Build a match clause for a role player entity.

    Prefers IID-based matching when available (more precise and faster),
    falls back to key attribute matching, and raises a clear error if
    neither is available.

    This is the canonical implementation used by RelationManager and RelationQuery.

    Args:
        var_name: The variable name to use (without $)
        entity: The entity instance
        entity_type_name: The TypeDB type name for the entity

    Returns:
        A TypeQL match clause string like "$var_name isa type, iid 0x..."
        or "$var_name isa type, has key_attr value"

    Raises:
        ValueError: If entity has neither _iid nor key attributes
    """
    # Prefer IID-based matching when available (more precise and faster)
    entity_iid = getattr(entity, "_iid", None)
    if entity_iid:
        return f"${var_name} isa {entity_type_name}, iid {entity_iid}"

    # Fall back to key attribute matching
    key_attrs = {
        field_name: attr_info
        for field_name, attr_info in entity.__class__.get_all_attributes().items()
        if attr_info.flags.is_key
    }

    for field_name, attr_info in key_attrs.items():
        value = getattr(entity, field_name)
        if value is not None:
            attr_class = attr_info.typ
            attr_name = attr_class.get_attribute_name()
            formatted_value = format_value(value)
            return f"${var_name} isa {entity_type_name}, has {attr_name} {formatted_value}"

    # Neither IID nor key attributes available
    raise ValueError(
        f"Role player '{var_name}' ({entity.__class__.__name__}) cannot be identified: "
        f"no _iid set and no @key attributes defined. Either fetch the entity from the "
        f"database first (to populate _iid) or add Flag(Key) to an attribute."
    )


def extract_entity_key(entity: Any) -> tuple[str, str, Any] | None:
    """Extract the first key attribute from an entity for matching.

    This is used to build match clauses based on key attributes when IID
    is not available.

    Args:
        entity: The entity instance

    Returns:
        Tuple of (field_name, attr_typeql_name, raw_value) if a key attribute
        with a non-None value is found, otherwise None.

    Examples:
        >>> extract_entity_key(person)  # person has name as @key
        ("name", "name", "Alice")
    """
    for field_name, attr_info in entity.__class__.get_all_attributes().items():
        if attr_info.flags.is_key:
            key_value = getattr(entity, field_name, None)
            if key_value is not None:
                attr_name = attr_info.typ.get_attribute_name()
                # Unwrap Attribute instance
                raw_value = key_value.value if hasattr(key_value, "value") else key_value
                return (field_name, attr_name, raw_value)
    return None


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
