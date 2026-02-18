"""Value formatting utilities for TypeQL."""

from datetime import date, datetime, timedelta
from decimal import Decimal as DecimalType
from typing import Any

import isodate
from isodate import Duration as IsodateDuration

try:
    from type_bridge_core import (
        format_value as _rust_format_value,  # type: ignore[import-not-found]
    )

    _USE_RUST = True
except ImportError:
    _rust_format_value = None
    _USE_RUST = False


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
    if _USE_RUST and _rust_format_value is not None:
        return _rust_format_value(value)

    # Python fallback below
    # Extract value from Attribute instances first
    if hasattr(value, "value"):
        value = value.value

    if isinstance(value, str):
        # Escape special characters for TypeQL string literals (JSON-style escaping)
        # Order matters: backslashes first, then other sequences
        escaped = (
            value.replace("\\", "\\\\")  # Backslashes
            .replace('"', '\\"')  # Double quotes
            .replace("\n", "\\n")  # Newlines
            .replace("\r", "\\r")  # Carriage returns
            .replace("\t", "\\t")  # Tabs
        )
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
        escaped = (
            str_value.replace("\\", "\\\\")  # Backslashes
            .replace('"', '\\"')  # Double quotes
            .replace("\n", "\\n")  # Newlines
            .replace("\r", "\\r")  # Carriage returns
            .replace("\t", "\\t")  # Tabs
        )
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
