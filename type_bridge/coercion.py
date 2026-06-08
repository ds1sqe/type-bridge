"""Value coercion utilities powered by Rust.

Provides ``coerce_value()`` for schema-aware type coercion and
``format_value()`` for TypeQL literal formatting, both backed by the
Rust ``type_bridge_core`` extension when available.
"""

from __future__ import annotations

from typing import Any

try:
    from type_bridge_core import ValueCoercer as ValueCoercer
    from type_bridge_core import coerce_value as coerce_value
    from type_bridge_core import format_value as format_value

    RUST_AVAILABLE = True
except ImportError:
    RUST_AVAILABLE = False
    ValueCoercer = None
    coerce_value = None

    def format_value(value: Any) -> str:
        """Fallback to Python implementation."""
        from type_bridge.crud.formatting import format_value as _py_format_value

        return _py_format_value(value)
