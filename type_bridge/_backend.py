"""Backend selection for TypeBridge managers."""

from __future__ import annotations

import os
from typing import Any

BACKEND_ENV_VAR = "TYPE_BRIDGE_BACKEND"
PYTHON_BACKEND = "python"
RUST_BACKEND = "rust"


def selected_backend() -> str:
    """Return the configured manager backend."""
    value = os.environ.get(BACKEND_ENV_VAR, PYTHON_BACKEND).strip().lower()
    if value == "":
        return PYTHON_BACKEND
    if value not in {PYTHON_BACKEND, RUST_BACKEND}:
        raise ValueError(
            f"{BACKEND_ENV_VAR} must be '{PYTHON_BACKEND}' or '{RUST_BACKEND}', got {value!r}"
        )
    return value


def manager_class(default_manager: type[Any]) -> type[Any]:
    """Return the manager class for the configured backend."""
    if selected_backend() == PYTHON_BACKEND:
        return default_manager

    from type_bridge.crud.rust_manager import RustTypeDBManager

    return RustTypeDBManager
