"""Type resolution and attribute handling utilities."""

from typing import TYPE_CHECKING, Any

from type_bridge.attribute import AttributeFlags
from type_bridge.crud.formatting import unwrap_attribute

if TYPE_CHECKING:
    from type_bridge.models import Entity


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
