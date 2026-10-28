"""Runtime selection for TypeBridge managers."""

from __future__ import annotations

import os

BACKEND_ENV_VAR = "TYPE_BRIDGE_BACKEND"
PYTHON_BACKEND = "python"
RUST_BACKEND = "rust"


def selected_backend() -> str:
    """Return the configured manager backend.

    The Python ORM backend was retired in #125. The environment
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
