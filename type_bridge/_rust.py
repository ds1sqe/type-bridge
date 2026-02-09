"""Rust core availability introspection."""

try:
    import type_bridge_core  # type: ignore[import-not-found]

    RUST_AVAILABLE: bool = True
    """True when the optional ``type_bridge_core`` Rust extension is installed."""
except ImportError:
    type_bridge_core = None  # type: ignore[assignment]
    RUST_AVAILABLE: bool = False
