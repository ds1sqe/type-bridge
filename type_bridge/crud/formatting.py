"""Value formatting utilities for TypeQL."""

from typing import Any

from type_bridge_core import format_value as _rust_format_value

_USE_RUST = True


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
    # Use Rust implementation when available for performance
    return _rust_format_value(value)


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
