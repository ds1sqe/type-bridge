"""Runtime selection for TypeBridge managers."""

from __future__ import annotations

import os
from typing import Any

BACKEND_ENV_VAR = "TYPE_BRIDGE_BACKEND"
PYTHON_BACKEND = "python"
RUST_BACKEND = "rust"


def selected_backend() -> str:
    """Return the configured manager backend.

    The Python ORM backend was retired in #125 Phase 4. The environment
    variable is retained only to reject stale transition settings clearly.
    """
    value = os.environ.get(BACKEND_ENV_VAR, RUST_BACKEND).strip().lower()
    if value in {"", RUST_BACKEND}:
        return RUST_BACKEND
    if value == PYTHON_BACKEND:
        raise ValueError(
            f"{BACKEND_ENV_VAR}=python is no longer supported; TypeBridge managers "
            "run through the Rust runtime"
        )
    raise ValueError(f"{BACKEND_ENV_VAR} must be '{RUST_BACKEND}' or unset, got {value!r}")


def manager_class(default_manager: type[Any]) -> type[Any]:
    """Return the canonical manager class after validating stale env settings."""
    selected_backend()
    if default_manager.__name__ == "RustTypeDBManager":
        return default_manager

    from type_bridge.crud.rust_manager import RustTypeDBManager

    return RustTypeDBManager
