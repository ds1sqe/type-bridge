"""Shared utilities for CRUD operations."""

from datetime import date, datetime, timedelta
from decimal import Decimal as DecimalType
from typing import TYPE_CHECKING, Any

import isodate
from isodate import Duration as IsodateDuration

from type_bridge.attribute import AttributeFlags
from type_bridge.crud.exceptions import KeyAttributeError

if TYPE_CHECKING:
    from type_bridge.models import Entity, Relation

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
                raw_value = unwrap_attribute(key_value)
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


def hydrate_attributes(
    entity_class: type["Entity"],
    raw_data: dict[str, Any],
    wrap_values: bool = False,
) -> tuple[dict[str, Any], tuple[tuple[str, Any], ...]]:
    """Hydrate attributes from TypeDB fetch result.

    Extracts attribute values from a raw TypeDB result dictionary,
    mapping TypeDB attribute names to Python field names. Handles
    multi-value attributes and collects key values for deduplication.

    Args:
        entity_class: The entity class to extract attributes for
        raw_data: Raw attribute data from TypeDB fetch result
        wrap_values: If True, wrap multi-value attributes in Attribute instances

    Returns:
        Tuple of (attrs_dict, key_values_tuple):
        - attrs_dict: Dictionary mapping field names to values
        - key_values_tuple: Tuple of (attr_name, value) for key attributes

    Examples:
        >>> result = {"name": "Alice", "age": 30}
        >>> attrs, keys = hydrate_attributes(Person, result)
        >>> attrs
        {"name": "Alice", "age": 30}
        >>> keys
        (("name", "Alice"),)  # if name is a key attribute
    """
    attrs: dict[str, Any] = {}
    key_values: list[tuple[str, Any]] = []

    for field_name, attr_info in entity_class.get_all_attributes().items():
        attr_class = attr_info.typ
        attr_name = attr_class.get_attribute_name()
        is_multi = is_multi_value_attribute(attr_info.flags)

        if attr_name in raw_data:
            raw_value = raw_data[attr_name]

            if is_multi and isinstance(raw_value, list):
                # Wrap list values in Attribute instances if requested
                if wrap_values:
                    attrs[field_name] = [attr_class(v) for v in raw_value]
                else:
                    attrs[field_name] = raw_value
            else:
                attrs[field_name] = raw_value

            # Collect key values for deduplication (convert lists to tuples for hashability)
            if attr_info.flags.is_key:
                hashable_value = tuple(raw_value) if isinstance(raw_value, list) else raw_value
                key_values.append((attr_name, hashable_value))
        else:
            # Default for missing attributes
            if is_multi or attr_info.flags.has_explicit_card:
                attrs[field_name] = []
            else:
                attrs[field_name] = None

    return attrs, tuple(sorted(key_values))


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


def process_iid_type_results(
    results: list[dict[str, Any]],
    key_attrs: dict[str, Any],
    key_attr_names: list[str],
    known_key_values: dict[str, Any],
) -> dict[tuple[tuple[str, Any], ...], tuple[str, str]]:
    """Process fetch results to build IID/type map.

    Shared logic for extracting IID and type information from TypeDB
    fetch results, building a map keyed by key attribute values.

    Args:
        results: List of fetch result dictionaries from TypeDB
        key_attrs: Dictionary of key attributes (field_name -> attr_info)
        key_attr_names: List of TypeDB attribute names for key attributes
        known_key_values: Dictionary of key values already known from filters

    Returns:
        Dictionary mapping key_values_tuple to (iid, type_name) tuple
    """
    iid_type_map: dict[tuple[tuple[str, Any], ...], tuple[str, str]] = {}

    for result in results:
        # Get IID/type from fetch result
        iid = result.get("_iid")
        type_name = result.get("_type")
        if not iid or not type_name:
            continue

        # Build map key from key attributes or known values
        if key_attrs:
            if known_key_values and len(known_key_values) == len(key_attrs):
                # All key values known from filters
                map_key = tuple(sorted(known_key_values.items()))
            else:
                # Extract key values from fetch result
                key_values: list[tuple[str, Any]] = []
                for attr_name in key_attr_names:
                    if attr_name in result:
                        val = result[attr_name]
                        key_values.append((attr_name, val))
                if not key_values:
                    continue
                map_key = tuple(sorted(key_values))
        else:
            # No key attributes, use IID as the map key
            map_key = (("_iid", iid),)

        iid_type_map[map_key] = (iid, type_name)

    return iid_type_map


def get_key_attrs(
    model_class: type["Entity"],
) -> tuple[dict[str, Any], list[str]]:
    """Get key attributes and their TypeDB names from a model class.

    Args:
        model_class: The entity class to extract key attributes from

    Returns:
        Tuple of (key_attrs dict, key_attr_names list)
    """
    owned_attrs = model_class.get_all_attributes()
    key_attrs = {
        field_name: attr_info
        for field_name, attr_info in owned_attrs.items()
        if attr_info.flags.is_key
    }
    key_attr_names = [attr_info.typ.get_attribute_name() for attr_info in key_attrs.values()]
    return key_attrs, key_attr_names


def build_known_key_values(
    key_attrs: dict[str, Any],
    filters: dict[str, Any],
) -> dict[str, Any]:
    """Extract known key values from filters.

    Args:
        key_attrs: Dictionary of key attributes (field_name -> attr_info)
        filters: Filter dictionary (field_name -> value)

    Returns:
        Dictionary of known key values (attr_name -> raw_value)
    """
    known_key_values: dict[str, Any] = {}
    for field_name, attr_info in key_attrs.items():
        attr_name = attr_info.typ.get_attribute_name()
        if field_name in filters:
            filter_value = filters[field_name]
            filter_value = unwrap_attribute(filter_value)
            known_key_values[attr_name] = filter_value
    return known_key_values


def build_iid_type_fetch_clause(
    key_attr_names: list[str],
    known_key_values: dict[str, Any],
    key_attrs: dict[str, Any],
    var: str = "$e",
) -> str:
    """Build fetch clause for IID and type retrieval.

    Args:
        key_attr_names: List of TypeDB attribute names for key attributes
        known_key_values: Dictionary of key values already known from filters
        key_attrs: Dictionary of key attributes
        var: Variable name for the entity (default: "$e")

    Returns:
        Fetch clause string
    """
    fetch_items = [f'"_iid": iid({var})', '"_type": label($t)']

    # Add key attributes to fetch (only if not all known from filters)
    if not (known_key_values and len(known_key_values) == len(key_attrs)):
        for attr_name in key_attr_names:
            fetch_items.append(f'"{attr_name}": {var}.{attr_name}')

    return f"fetch {{\n  {', '.join(fetch_items)}\n}}"


def match_entity_type(
    attrs: dict[str, Any],
    iid_type_map: dict[tuple[tuple[str, Any], ...], tuple[str, str]],
    model_class: type["Entity"],
    owned_attrs: dict[str, Any] | None = None,
) -> tuple[type["Entity"], str | None]:
    """Match entity attributes to IID/type and resolve the correct class.

    Uses key attributes to look up the corresponding IID/type from the map
    (in-memory, no database query), then resolves the actual Python class
    for polymorphic instantiation.

    Args:
        attrs: Extracted attributes for the entity
        iid_type_map: Map from key_values_tuple to (iid, type_name)
        model_class: The base model class for resolution
        owned_attrs: Optional attribute metadata. If None, fetched from model_class

    Returns:
        Tuple of (resolved_class, iid) where resolved_class is the
        concrete subclass if found, otherwise model_class
    """
    # If no type info available, use model_class
    if not iid_type_map:
        return model_class, None

    # Get owned_attrs if not provided
    attrs_dict: dict[str, Any] = (
        owned_attrs if owned_attrs is not None else model_class.get_all_attributes()
    )

    # Get key attributes for matching
    key_attrs = {
        field_name: attr_info
        for field_name, attr_info in attrs_dict.items()
        if attr_info.flags.is_key
    }

    if not key_attrs:
        # No key attributes - can't match reliably, use first available
        if iid_type_map:
            iid, type_name = next(iter(iid_type_map.values()))
            resolved_class = resolve_entity_class(model_class, type_name)
            return resolved_class, iid
        return model_class, None

    # Build key signature from attrs for in-memory lookup
    key_values: list[tuple[str, Any]] = []
    for field_name, attr_info in key_attrs.items():
        value = attrs.get(field_name)
        if value is not None:
            value = unwrap_attribute(value)
            attr_name = attr_info.typ.get_attribute_name()
            key_values.append((attr_name, value))

    if not key_values:
        return model_class, None

    # Look up in the map using key values (no database query!)
    map_key = tuple(sorted(key_values))
    if map_key in iid_type_map:
        iid, type_name = iid_type_map[map_key]
        resolved_class = resolve_entity_class(model_class, type_name)
        return resolved_class, iid

    return model_class, None


def modify_match_for_type_binding(match_str: str, type_name: str) -> str:
    """Modify match clause to bind exact type for label() retrieval.

    Changes: "$e isa person" to "$e isa! $t; $t sub person"
    This allows using label($t) to get the exact type name.

    Args:
        match_str: Original match clause
        type_name: TypeDB type name

    Returns:
        Modified match clause with type binding
    """
    match_str = match_str.replace(f"$e isa {type_name}", "$e isa! $t")
    return f"{match_str}; $t sub {type_name}"


# ============================================================================
# Entity IID Population Utilities
# ============================================================================


def build_entity_iid_query(
    entities: list[Any],
    model_class: type["Entity"],
    key_attrs: dict[str, Any],
) -> tuple[str, list[str]] | None:
    """Build batched IID lookup query for entities.

    Constructs a TypeQL fetch query using OR clauses to match all entities
    by their key attributes in a single database round-trip.

    Args:
        entities: List of entity instances to look up IIDs for
        model_class: The entity class (must have get_type_name())
        key_attrs: Dictionary of key attributes (field_name -> attr_info)

    Returns:
        Tuple of (query_str, key_attr_names) if valid query can be built,
        otherwise None (e.g., if no entities or no key attributes)
    """
    if not entities or not key_attrs:
        return None

    type_name = model_class.get_type_name()
    key_attr_names = [attr_info.typ.get_attribute_name() for attr_info in key_attrs.values()]

    # Build batched disjunctive query for all entities
    or_clauses = []
    for entity in entities:
        match_parts = [f"$e isa {type_name}"]
        for field_name, attr_info in key_attrs.items():
            value = getattr(entity, field_name, None)
            if value is not None:
                value = unwrap_attribute(value)
                attr_name = attr_info.typ.get_attribute_name()
                formatted_value = format_value(value)
                match_parts.append(f"has {attr_name} {formatted_value}")
        or_clauses.append(f"{{ {', '.join(match_parts)}; }}")

    if not or_clauses:
        return None

    # Build fetch clause: iid and key attributes (for matching)
    fetch_items = ['"_iid": iid($e)']
    for attr_name in key_attr_names:
        fetch_items.append(f'"{attr_name}": $e.{attr_name}')
    fetch_clause = f"fetch {{\n  {', '.join(fetch_items)}\n}}"

    query_str = f"match\n{' or '.join(or_clauses)};\n{fetch_clause};"
    return query_str, key_attr_names


def build_entity_iid_map(
    results: list[dict[str, Any]],
    key_attr_names: list[str],
) -> dict[tuple[tuple[str, Any], ...], str]:
    """Build IID map from query results.

    Processes fetch query results to create a lookup map from key attribute
    values to IIDs for fast in-memory matching.

    Args:
        results: List of fetch result dictionaries from TypeDB
        key_attr_names: List of TypeDB attribute names for key attributes

    Returns:
        Dictionary mapping key_values_tuple to IID string
    """
    iid_map: dict[tuple[tuple[str, Any], ...], str] = {}

    for result in results:
        iid = result.get("_iid")
        if not iid:
            continue

        # Extract key attribute values
        key_values: list[tuple[str, Any]] = []
        for attr_name in key_attr_names:
            if attr_name in result:
                key_values.append((attr_name, result[attr_name]))
        if key_values:
            iid_map[tuple(sorted(key_values))] = iid

    return iid_map


def assign_entity_iids(
    entities: list[Any],
    iid_map: dict[tuple[tuple[str, Any], ...], str],
    key_attrs: dict[str, Any],
) -> None:
    """Assign IIDs to entities using the IID map.

    Performs in-memory lookup to match entities to their IIDs by key
    attribute values, then sets the _iid field on each entity.

    Args:
        entities: List of entity instances to assign IIDs to
        iid_map: Dictionary mapping key_values_tuple to IID string
        key_attrs: Dictionary of key attributes (field_name -> attr_info)
    """
    for entity in entities:
        key_values: list[tuple[str, Any]] = []
        for field_name, attr_info in key_attrs.items():
            value = getattr(entity, field_name, None)
            if value is not None:
                value = unwrap_attribute(value)
                attr_name = attr_info.typ.get_attribute_name()
                key_values.append((attr_name, value))

        if key_values:
            map_key = tuple(sorted(key_values))
            if map_key in iid_map:
                object.__setattr__(entity, "_iid", iid_map[map_key])


# ============================================================================
# Polymorphic Role Player Utilities
# ============================================================================


def resolve_entity_class_from_label(
    type_label: str | None,
    allowed_entity_classes: tuple[type["Entity"], ...],
) -> type["Entity"]:
    """Resolve the correct Python entity class from a TypeDB type label.

    Used for polymorphic role players where the declared role type is abstract
    but the actual entity is a concrete subtype.

    Args:
        type_label: The TypeDB type label (from label() function), e.g., "person"
        allowed_entity_classes: Tuple of allowed entity classes for this role

    Returns:
        The matching Python entity class, or the first allowed class as fallback
    """
    if not type_label:
        # Fallback to first allowed class if no type label
        return allowed_entity_classes[0]

    # Build a mapping of type names to classes, including subclasses
    type_name_to_class: dict[str, type[Entity]] = {}

    def collect_subclasses(cls: type["Entity"]) -> None:
        """Recursively collect all subclasses and their type names."""
        type_name = cls.get_type_name()
        if type_name:
            type_name_to_class[type_name] = cls
        for subclass in cls.__subclasses__():
            collect_subclasses(subclass)

    for cls in allowed_entity_classes:
        collect_subclasses(cls)

    # Look up the class by type label
    if type_label in type_name_to_class:
        return type_name_to_class[type_label]

    # Fallback to first allowed class
    return allowed_entity_classes[0]


def build_role_player_fetch_items(
    role_info: dict[str, tuple[str, tuple[type["Entity"], ...]]],
) -> list[str]:
    """Build fetch items for role players with their IIDs and type labels.

    TypeQL 3.x doesn't allow mixing iid() with .* in the same nested block,
    so we fetch the IID as a separate key alongside the nested attributes.
    We also fetch the type label to correctly identify polymorphic role players.

    Args:
        role_info: Dict mapping role_name -> (role_var, allowed_entity_types)

    Returns:
        List of fetch item strings like:
            '"employee_iid": iid($employee)'
            '"employee_type": label($employee_type)'
            '"employee": { $employee.* }'

    Note: The caller must add type variable bindings to the match clause:
        $employee isa $employee_type;
    """
    fetch_items = []
    for role_name, (role_var, _) in role_info.items():
        # Fetch IID separately (can't be mixed with .* in nested block)
        fetch_items.append(f'"{role_name}_iid": iid({role_var})')
        # Fetch type label for polymorphic role player resolution
        fetch_items.append(f'"{role_name}_type": label({role_var}_type)')
        # Fetch all attributes for the role player
        fetch_items.append(f'"{role_name}": {{\n    {role_var}.*\n  }}')
    return fetch_items


def group_results_by_iid(results: list[dict[str, Any]]) -> dict[str, list[dict[str, Any]]]:
    """Group query results by relation/entity IID.

    TypeDB returns one row per role player combination for multi-cardinality
    roles. This utility groups those rows by IID so they can be merged into
    a single relation instance.

    Args:
        results: List of result dictionaries from TypeDB fetch

    Returns:
        Dictionary mapping IID -> list of result rows with that IID
    """
    grouped: dict[str, list[dict[str, Any]]] = {}
    for result in results:
        iid = result.get("_iid")
        if iid:
            if iid not in grouped:
                grouped[iid] = []
            grouped[iid].append(result)
    return grouped


def extract_relation_attributes(
    model_class: type["Relation"],
    result: dict[str, Any],
) -> dict[str, Any]:
    """Extract relation attributes from a query result.

    Handles multi-value attributes by wrapping them in Attribute instances.
    Single values are passed directly to let Pydantic handle conversion.

    Args:
        model_class: The relation class to extract attributes for
        result: The result dictionary from TypeDB fetch

    Returns:
        Dictionary mapping field_name -> attribute value (ready for model constructor)
    """
    attrs: dict[str, Any] = {}
    all_attrs = model_class.get_all_attributes()

    for field_name, attr_info in all_attrs.items():
        attr_class = attr_info.typ
        attr_name = attr_class.get_attribute_name()
        if attr_name in result:
            raw_value = result[attr_name]
            # Multi-value attributes need explicit conversion from list of raw values
            if is_multi_value_attribute(attr_info.flags) and isinstance(raw_value, list):
                # Convert each raw value to Attribute instance
                attrs[field_name] = [attr_class(v) for v in raw_value]
            else:
                # Single value - let Pydantic handle conversion via model constructor
                attrs[field_name] = raw_value
        else:
            # For list fields (has_explicit_card), default to empty list
            # For other optional fields, explicitly set to None
            if attr_info.flags.has_explicit_card:
                attrs[field_name] = []
            else:
                attrs[field_name] = None

    return attrs


def extract_role_players_from_results(
    result_group: list[dict[str, Any]],
    role_info: dict[str, tuple[str, tuple[type["Entity"], ...]]],
    multi_player_roles: set[str],
) -> dict[str, Any]:
    """Extract and deduplicate role players from grouped query results.

    For multi-player roles, collects all unique players from all rows in the group.
    For single-player roles, takes the first player found.

    Args:
        result_group: List of result rows with the same relation IID
        role_info: Dict mapping role_name -> (role_var, allowed_entity_classes)
        multi_player_roles: Set of role names that allow multiple players

    Returns:
        Dictionary mapping role_name -> player entity or list of player entities
    """
    role_players: dict[str, Any] = {}

    for role_name, (role_var, allowed_entity_classes) in role_info.items():
        is_multi = role_name in multi_player_roles
        collected_players: list[Any] = []
        seen_player_keys: set[tuple[Any, ...]] = set()

        for result in result_group:
            if role_name in result and isinstance(result[role_name], dict):
                player_data = result[role_name]

                # Get the actual type label from TypeDB (fetched via label())
                type_label = result.get(f"{role_name}_type")

                # Resolve entity class from type label for polymorphic support
                entity_class = resolve_entity_class_from_label(type_label, allowed_entity_classes)

                # Hydrate player entity using shared utility
                player_iid = result.get(f"{role_name}_iid")
                player_attrs, key_values = hydrate_attributes(
                    entity_class, player_data, wrap_values=True
                )

                # Create entity instance if we have any non-None attributes
                # Note: Relations as role players may have no owned attributes
                if player_attrs == {} or any(v is not None for v in player_attrs.values()):
                    # Deduplicate players based on their attribute values
                    if key_values not in seen_player_keys:
                        seen_player_keys.add(key_values)
                        player_entity = entity_class(**player_attrs)
                        if player_iid:
                            object.__setattr__(player_entity, "_iid", player_iid)
                        collected_players.append(player_entity)

        # Store collected players
        if collected_players:
            if is_multi:
                role_players[role_name] = collected_players
            else:
                role_players[role_name] = collected_players[0]

    return role_players


# ============================================================================
# Relation IID Population Utilities
# ============================================================================


def build_relation_iid_query(
    relations: list[Any],
    model_class: type["Relation"],
    roles: dict[str, Any],
) -> (
    tuple[str, list[str], dict[str, tuple[str, str]], list[dict[str, tuple[str, Any, Any]]]] | None
):
    """Build batched IID lookup query for relations.

    Constructs a TypeQL select query using OR clauses to match all relations
    by their role players in a single database round-trip.

    Args:
        relations: List of relation instances to look up IIDs for
        model_class: The relation class (must have get_type_name())
        roles: Dictionary of roles from model_class._roles

    Returns:
        Tuple of (query_str, role_names, role_key_info, relation_key_data) if valid,
        otherwise None.
        - query_str: The TypeQL select query
        - role_names: List of role field names
        - role_key_info: Dict mapping role_name -> (key_var_name, attr_name)
        - relation_key_data: List of per-relation role key info for correlation
    """
    if not relations:
        return None

    role_names = list(roles.keys())
    type_name = model_class.get_type_name()

    # Track which roles have key attributes and their attribute names
    role_key_info: dict[str, tuple[str, str]] = {}
    relation_key_data: list[dict[str, tuple[str, Any, Any]]] = []

    or_clauses = []
    for relation in relations:
        role_parts = []
        match_statements = []
        per_relation_key_info: dict[str, tuple[str, Any, Any]] = {}

        for role_name, role in roles.items():
            entity = getattr(relation, role_name, None)
            if entity is None:
                continue

            # Validate entity is a TypeDBType instance
            entity_class = entity.__class__
            if not hasattr(entity_class, "get_all_attributes"):
                continue

            role_var = f"${role_name}"
            role_parts.append(f"{role.role_name}: {role_var}")

            key_info = extract_entity_key(entity)
            if key_info:
                _, attr_name, raw_value = key_info
                formatted_value = format_value(raw_value)
                # Use a variable for the key attribute to capture its value
                key_var = f"${role_name}_key"
                match_statements.append(
                    f"{role_var} has {attr_name} {key_var}; {key_var} == {formatted_value}"
                )
                # Track the key variable for this role
                if role_name not in role_key_info:
                    role_key_info[role_name] = (f"{role_name}_key", attr_name)
                per_relation_key_info[role_name] = (attr_name, raw_value, entity)

        if not role_parts:
            continue

        roles_str = ", ".join(role_parts)
        relation_match = f"$r isa {type_name} ({roles_str})"
        clause_parts = [relation_match] + match_statements
        or_clauses.append(f"{{ {'; '.join(clause_parts)}; }}")
        relation_key_data.append(per_relation_key_info)

    if not or_clauses:
        return None

    # Build select variables - shared across all branches
    select_vars = ["$r"] + [f"${role_name}" for role_name in role_names]
    for key_var_name, _ in role_key_info.values():
        select_vars.append(f"${key_var_name}")

    query_str = f"match\n{' or '.join(or_clauses)};\nselect {', '.join(select_vars)};"
    return query_str, role_names, role_key_info, relation_key_data


def build_relation_result_map(
    results: list[dict[str, Any]],
    role_key_info: dict[str, tuple[str, str]],
) -> dict[tuple[tuple[str, Any], ...], dict[str, Any]]:
    """Build a lookup map from key attribute values to results.

    Processes select query results to create a lookup map for correlating
    relations to their query results.

    Args:
        results: List of select result dictionaries from TypeDB
        role_key_info: Dict mapping role_name -> (key_var_name, attr_name)

    Returns:
        Dictionary mapping key_values_tuple to result dict
    """
    result_map: dict[tuple[tuple[str, Any], ...], dict[str, Any]] = {}

    for result in results:
        key_parts: list[tuple[str, Any]] = []
        for role_name, (key_var_name, _) in role_key_info.items():
            if key_var_name in result:
                key_val = result[key_var_name]
                # Extract value from concept data dict
                if isinstance(key_val, dict):
                    key_val = key_val.get("value", key_val.get("result"))
                key_parts.append((role_name, key_val))
        if key_parts:
            result_map[tuple(sorted(key_parts))] = result

    return result_map


def assign_relation_iids(
    relations: list[Any],
    result_map: dict[tuple[tuple[str, Any], ...], dict[str, Any]],
    role_key_info: dict[str, tuple[str, str]],
    role_names: list[str],
) -> None:
    """Assign IIDs to relations and their role players.

    Performs in-memory lookup to match relations to their query results,
    then sets the _iid field on relations and their role player entities.

    Args:
        relations: List of relation instances to assign IIDs to
        result_map: Dictionary mapping key_values_tuple to result dict
        role_key_info: Dict mapping role_name -> (key_var_name, attr_name)
        role_names: List of role field names
    """
    for relation in relations:
        # Build the key for this relation from its role players' key attributes
        relation_key_parts: list[tuple[str, Any]] = []
        role_var_to_entity: dict[str, Any] = {}

        for role_name in role_names:
            entity = getattr(relation, role_name, None)
            if entity is not None:
                role_var_to_entity[role_name] = entity

                # Get the key attribute value for this role player
                if role_name in role_key_info:
                    key_info = extract_entity_key(entity)
                    if key_info:
                        _, _, raw_value = key_info
                        relation_key_parts.append((role_name, raw_value))

        # Look up the result by key values
        relation_key = tuple(sorted(relation_key_parts))
        matched_result = result_map.get(relation_key)

        if matched_result:
            # Extract relation IID
            if "r" in matched_result and isinstance(matched_result["r"], dict):
                relation_iid = matched_result["r"].get("_iid")
                if relation_iid:
                    object.__setattr__(relation, "_iid", relation_iid)

            # Extract role player IIDs
            for role_name, entity in role_var_to_entity.items():
                if role_name in matched_result and isinstance(matched_result[role_name], dict):
                    player_iid = matched_result[role_name].get("_iid")
                    if player_iid:
                        object.__setattr__(entity, "_iid", player_iid)


# ============================================================================
# Update Query Building Utilities
# ============================================================================


def extract_update_values(
    entity_or_relation: Any,
    owned_attrs: dict[str, Any],
    skip_key_attrs: bool = True,
) -> tuple[dict[str, Any], set[str], dict[str, list[Any]]]:
    """Extract attribute updates from an entity or relation instance.

    Separates single-value and multi-value attribute updates, and tracks
    optional attributes that need to be deleted (set to None).

    Args:
        entity_or_relation: The entity or relation instance
        owned_attrs: Dictionary of attribute info (field_name -> attr_info)
        skip_key_attrs: Whether to skip key attributes (True for entities)

    Returns:
        Tuple of (single_value_updates, single_value_deletes, multi_value_updates):
        - single_value_updates: Dict of attr_name -> value for single-value attrs
        - single_value_deletes: Set of attr_names for optional attrs set to None
        - multi_value_updates: Dict of attr_name -> list of values for multi-value attrs
    """
    single_value_updates: dict[str, Any] = {}
    single_value_deletes: set[str] = set()
    multi_value_updates: dict[str, list[Any]] = {}

    for field_name, attr_info in owned_attrs.items():
        # Skip key attributes if requested (used for entity matching)
        if skip_key_attrs and attr_info.flags.is_key:
            continue

        attr_class = attr_info.typ
        attr_name = attr_class.get_attribute_name()
        flags = attr_info.flags

        # Get current value from instance
        current_value = getattr(entity_or_relation, field_name, None)

        # Extract raw values from Attribute instances
        if current_value is not None:
            if isinstance(current_value, list):
                # Multi-value: extract value from each Attribute in list
                current_value = [unwrap_attribute(item) for item in current_value]
            else:
                # Single-value: extract value from Attribute
                current_value = unwrap_attribute(current_value)

        # Determine if multi-value
        if is_multi_value_attribute(flags):
            # Multi-value: store as list (even if empty)
            if current_value is None:
                current_value = []
            multi_value_updates[attr_name] = current_value
        else:
            # Single-value: handle updates and deletions
            if current_value is not None:
                single_value_updates[attr_name] = current_value
            elif flags.card_min == 0:
                # Optional attribute set to None - needs to be deleted
                single_value_deletes.add(attr_name)

    return single_value_updates, single_value_deletes, multi_value_updates


def build_multi_value_match_clauses(
    multi_value_updates: dict[str, list[Any]],
    var_name: str = "$e",
) -> list[str]:
    """Build match clauses for multi-value attribute updates with guards.

    Creates try blocks that match existing attribute values while guarding
    against the values that should be kept (not deleted).

    Args:
        multi_value_updates: Dict of attr_name -> list of values to keep
        var_name: Variable name for the entity/relation (e.g., "$e", "$r")

    Returns:
        List of match clause strings (try blocks with guards)
    """
    match_statements = []
    for attr_name, values in multi_value_updates.items():
        keep_literals = [format_value(v) for v in dict.fromkeys(values)]
        attr_var = f"${attr_name}_{var_name.replace('$', '')}"
        guard_lines = [f"not {{ {attr_var} == {literal}; }};" for literal in keep_literals]
        try_block = "\n".join(
            [
                "try {",
                f"  {var_name} has {attr_name} {attr_var};",
                *[f"  {g}" for g in guard_lines],
                "};",
            ]
        )
        match_statements.append(try_block)
    return match_statements


def build_single_value_match_clauses(
    single_value_updates: dict[str, Any],
    var_name: str = "$e",
) -> list[str]:
    """Build match clauses for single-value attribute updates.

    Creates try blocks to bind existing attribute values for deletion.
    TypeDB 3.x requires delete-then-insert for attribute replacement.

    Args:
        single_value_updates: Dict of attr_name -> value to update
        var_name: Variable name for the entity/relation

    Returns:
        List of match clause strings (try blocks)
    """
    match_statements = []
    for attr_name in single_value_updates:
        match_statements.append(
            f"try {{ {var_name} has {attr_name} $old_{attr_name}_{var_name.replace('$', '')}; }};"
        )
    return match_statements


def build_delete_match_clauses(
    single_value_deletes: set[str],
    var_name: str = "$e",
) -> list[str]:
    """Build match clauses for single-value attribute deletions.

    Creates try blocks to bind existing attribute values for deletion.

    Args:
        single_value_deletes: Set of attr_names to delete
        var_name: Variable name for the entity/relation

    Returns:
        List of match clause strings (try blocks)
    """
    match_statements = []
    for attr_name in single_value_deletes:
        match_statements.append(
            f"try {{ {var_name} has {attr_name} ${attr_name}_{var_name.replace('$', '')}; }};"
        )
    return match_statements


def build_update_delete_clause(
    multi_value_updates: dict[str, list[Any]],
    single_value_updates: dict[str, Any],
    single_value_deletes: set[str],
    var_name: str = "$e",
) -> str:
    """Build the delete clause for update operations.

    Combines deletions for multi-value attrs, single-value updates,
    and explicit deletions.

    Args:
        multi_value_updates: Dict of attr_name -> list of values
        single_value_updates: Dict of attr_name -> value
        single_value_deletes: Set of attr_names to delete
        var_name: Variable name for the entity/relation

    Returns:
        Delete clause string (without "delete" keyword)
    """
    delete_parts = []
    var_suffix = var_name.replace("$", "")

    # Delete for multi-value attributes
    for attr_name in multi_value_updates:
        attr_var = f"${attr_name}_{var_suffix}"
        delete_parts.append(f"try {{ {attr_var} of {var_name}; }};")

    # Delete old values for single-value attributes being updated
    for attr_name in single_value_updates:
        delete_parts.append(f"try {{ $old_{attr_name}_{var_suffix} of {var_name}; }};")

    # Delete for explicit deletions
    for attr_name in single_value_deletes:
        delete_parts.append(f"try {{ ${attr_name}_{var_suffix} of {var_name}; }};")

    return "\n".join(delete_parts)


def build_update_insert_clause(
    multi_value_updates: dict[str, list[Any]],
    single_value_updates: dict[str, Any],
    var_name: str = "$e",
) -> str:
    """Build the insert clause for update operations.

    Creates has statements for all new attribute values.

    Args:
        multi_value_updates: Dict of attr_name -> list of values
        single_value_updates: Dict of attr_name -> value
        var_name: Variable name for the entity/relation

    Returns:
        Insert clause string (without "insert" keyword)
    """
    insert_parts = []

    # Insert multi-value attributes
    for attr_name, values in multi_value_updates.items():
        for value in values:
            formatted_value = format_value(value)
            insert_parts.append(f"{var_name} has {attr_name} {formatted_value};")

    # Insert single-value attributes
    for attr_name, value in single_value_updates.items():
        formatted_value = format_value(value)
        insert_parts.append(f"{var_name} has {attr_name} {formatted_value};")

    return "\n".join(insert_parts)


def build_entity_update_query_parts(
    entity: Any,
    model_class: type["Entity"],
    var_name: str = "$e",
) -> tuple[str, str, str, str]:
    """Build the TypeQL query parts for updating an entity.

    Shared implementation used by both EntityManager and EntityQuery.
    Prefers IID-based matching when available, falls back to key attributes.

    Args:
        entity: The entity instance to update
        model_class: The entity's model class
        var_name: Variable name for the entity in the query

    Returns:
        Tuple of (match_clause, delete_clause, insert_clause, update_clause)

    Raises:
        KeyAttributeError: If entity has no _iid and no key attributes
    """
    owned_attrs = model_class.get_all_attributes()

    # Prefer IID-based matching when available (like relation CRUD)
    entity_iid = getattr(entity, "_iid", None)
    use_iid_matching = entity_iid is not None

    # Fall back to key attributes if no IID
    match_filters: dict[str, Any] = {}
    if not use_iid_matching:
        for field_name, attr_info in owned_attrs.items():
            if attr_info.flags.is_key:
                key_value = getattr(entity, field_name, None)
                if key_value is None:
                    raise KeyAttributeError(
                        entity_type=model_class.__name__,
                        operation="update",
                        field_name=field_name,
                    )
                key_value = unwrap_attribute(key_value)
                attr_name = attr_info.typ.get_attribute_name()
                match_filters[attr_name] = key_value

        if not match_filters:
            raise KeyAttributeError(
                entity_type=model_class.__name__,
                operation="update",
                all_fields=list(owned_attrs.keys()),
            )

    # Extract single/multi-value updates
    single_value_updates, single_value_deletes, multi_value_updates = extract_update_values(
        entity, owned_attrs, skip_key_attrs=True
    )

    # Build Match Clause
    match_statements = []
    entity_match_parts = [f"{var_name} isa {model_class.get_type_name()}"]
    if use_iid_matching:
        # Use IID for precise matching
        entity_match_parts.append(f"iid {entity_iid}")
    else:
        # Use key attributes for matching
        for attr_name, attr_value in match_filters.items():
            formatted_value = format_value(attr_value)
            entity_match_parts.append(f"has {attr_name} {formatted_value}")
    match_statements.append(", ".join(entity_match_parts) + ";")

    # Add match for multi-value attributes with guards
    if multi_value_updates:
        for attr_name, values in multi_value_updates.items():
            keep_literals = [format_value(v) for v in dict.fromkeys(values)]
            attr_var = f"${attr_name}_{var_name.replace('$', '')}"
            guard_lines = [f"not {{ {attr_var} == {literal}; }};" for literal in keep_literals]
            try_block = "\n".join(
                [
                    "try {",
                    f"  {var_name} has {attr_name} {attr_var};",
                    *[f"  {g}" for g in guard_lines],
                    "};",
                ]
            )
            match_statements.append(try_block)

    # Add match for single-value deletes
    if single_value_deletes:
        for attr_name in single_value_deletes:
            attr_var = f"${attr_name}_{var_name.replace('$', '')}"
            match_statements.append(f"try {{ {var_name} has {attr_name} {attr_var}; }};")

    match_clause = "\n".join(match_statements)

    # Build Delete Clause
    delete_parts = []
    if multi_value_updates:
        for attr_name in multi_value_updates:
            attr_var = f"${attr_name}_{var_name.replace('$', '')}"
            delete_parts.append(f"try {{ {attr_var} of {var_name}; }};")

    if single_value_deletes:
        for attr_name in single_value_deletes:
            attr_var = f"${attr_name}_{var_name.replace('$', '')}"
            delete_parts.append(f"try {{ {attr_var} of {var_name}; }};")

    delete_clause = "\n".join(delete_parts)

    # Build Insert Clause (for multi-value attributes)
    insert_parts = []
    for attr_name, values in multi_value_updates.items():
        for value in values:
            formatted_value = format_value(value)
            insert_parts.append(f"{var_name} has {attr_name} {formatted_value};")

    insert_clause = "\n".join(insert_parts)

    # Build Update Clause (for single-value attributes)
    update_parts = []
    if single_value_updates:
        for attr_name, value in single_value_updates.items():
            formatted_value = format_value(value)
            update_parts.append(f"{var_name} has {attr_name} {formatted_value};")

    update_clause = "\n".join(update_parts)

    return match_clause, delete_clause, insert_clause, update_clause
