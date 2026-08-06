"""Type resolution and attribute handling utilities."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from type_bridge.attribute.flags import _QueryAttributeFlags as AttributeFlags
from type_bridge.crud.formatting import unwrap_attribute

if TYPE_CHECKING:
    from type_bridge.models.base import _QueryTypeDBType as TypeDBType
    from type_bridge.models.utils import _QueryModelAttrInfo as ModelAttrInfo


def wrap_attribute_value(
    value: Any,
    attr_info: ModelAttrInfo,
    *,
    use_pydantic_validate: bool = True,
) -> Any:
    """Wrap raw values in Attribute instances with consistent behavior.

    This is the unified attribute wrapping function used by all hydration paths.
    It ensures consistent handling of:
    - Multi-value attributes (lists)
    - Single-value attributes
    - Type coercion (via _pydantic_validate when available)
    - Null handling

    Args:
        value: The raw value to wrap (or list of values for multi-value attrs)
        attr_info: ModelAttrInfo containing attribute class and flags
        use_pydantic_validate: If True, use _pydantic_validate() for type coercion
                              (e.g., parsing ISO datetime strings). Default True.

    Returns:
        Wrapped attribute value(s), or None/empty list for missing values

    Examples:
        >>> wrap_attribute_value("Alice", name_attr_info)
        Name("Alice")
        >>> wrap_attribute_value(["tag1", "tag2"], tags_attr_info)
        [Tag("tag1"), Tag("tag2")]
    """
    attr_class = attr_info.typ

    def wrap_single(item: Any) -> Any:
        """Wrap a single value in an Attribute instance."""
        if isinstance(item, attr_class):
            return item
        # Use _pydantic_validate if available (handles type coercion like datetime parsing)
        if use_pydantic_validate and hasattr(attr_class, "_pydantic_validate"):
            return attr_class._pydantic_validate(item)
        return attr_class(item)

    # Handle None
    if value is None:
        return None

    # Multi-value attribute (has_explicit_card or is_multi_value_attribute)
    is_multi = is_multi_value_attribute(attr_info.flags)

    if is_multi or attr_info.flags.has_explicit_card:
        # Normalize to list
        items = value if isinstance(value, list) else [value]
        wrapped = [wrap_single(item) for item in items if item is not None]
        return wrapped if wrapped else []

    # Single-value but got a list (defensive handling)
    if isinstance(value, list):
        # Take first non-None value
        for item in value:
            if item is not None:
                return wrap_single(item)
        return None

    # Single value
    return wrap_single(value)


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
    entity_class: type[TypeDBType],
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
        wrap_values: If True, wrap attributes in Attribute instances using
                     the unified wrap_attribute_value() helper

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

            if wrap_values:
                # Use unified wrapping helper for consistent behavior
                attrs[field_name] = wrap_attribute_value(
                    raw_value, attr_info, use_pydantic_validate=True
                )
            elif is_multi and isinstance(raw_value, list):
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
