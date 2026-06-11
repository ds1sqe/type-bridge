"""TypeDB version gate shim.

Re-exports the window/band surface from ``type_bridge_core`` (SSOT in
``crates/core/src/version.rs``) and defines ``UnsupportedVersionError`` as the
documented TypeBridge exception type.

No window literals, no comparison logic, and no band numbers live here — all
decisions are delegated to core.
"""

from __future__ import annotations

import type_bridge_core

# Plain re-exports — same style as type_bridge/coercion.py.  The raw core
# decision is intentionally NOT re-exported: ensure_supported() is the
# documented gate entry point (callers wanting the untranslated form can
# import type_bridge_core directly).
from type_bridge_core import band as band
from type_bridge_core import check_supported as _check_supported
from type_bridge_core import max_supported_line as max_supported_line
from type_bridge_core import min_supported_version as min_supported_version


class UnsupportedVersionError(type_bridge_core.VersionError):
    """Raised when the driver/server pair falls outside the support window or spans
    incompatible protocol bands.

    The exception message contains both detected version strings and a
    human-readable remediation hint (e.g. which ``typedb-driver`` version to
    install) so callers never need to parse protocol numbers.

    This is a subclass of ``type_bridge_core.VersionError``, so existing
    ``except type_bridge_core.VersionError`` catches work unchanged while
    callers that import only ``type_bridge`` can target this type directly.
    """


def ensure_supported(driver: str, server: str) -> None:
    """Assert that a driver/server version pair is within the support window and band.

    Translates ``type_bridge_core.VersionError`` into
    :class:`UnsupportedVersionError` so all version-gate failures surface as the
    documented TypeBridge exception type.  Contains no comparison logic — the
    decision is entirely core's.

    Args:
        driver: Installed driver version string (e.g. ``"3.10.0"``).
        server: Detected server version string (e.g. ``"3.10.4"``).

    Raises:
        UnsupportedVersionError: When the pair violates the window or crosses
            protocol bands.  The message names both versions and a remediation
            hint.
    """
    try:
        _check_supported(driver, server)
    except type_bridge_core.VersionError as exc:
        raise UnsupportedVersionError(str(exc)) from exc


def ensure_runtime_supported(embedded_driver: str, server: str) -> None:
    """Assert the embedded Rust runtime driver is compatible with the server.

    Every TypeBridge transaction executes through the Rust runtime's own
    typedb-driver, so its protocol band must match the server independently of
    the installed Python driver.  Core makes the decision; this wrapper only
    reframes the failure — the usual "install a different typedb-driver"
    remediation does not apply to a driver compiled into the wheel.

    Args:
        embedded_driver: Version compiled into the Rust runtime
            (``typedb_driver.embedded_driver_version()``).
        server: Detected server version string.

    Raises:
        UnsupportedVersionError: When the embedded driver and server violate
            the window or cross protocol bands.  The message names the
            embedded driver explicitly and gives wheel-appropriate remediation.
    """
    try:
        _check_supported(embedded_driver, server)
    except type_bridge_core.VersionError as exc:
        raise UnsupportedVersionError(
            f"TypeBridge's embedded runtime driver {embedded_driver} is not "
            f"compatible with server {server}: {exc} "
            f"(the runtime driver is compiled into type-bridge — use a "
            f"type-bridge release matching your server line, or a server "
            f"matching this release)"
        ) from exc


__all__ = [
    "UnsupportedVersionError",
    "band",
    "ensure_runtime_supported",
    "ensure_supported",
    "max_supported_line",
    "min_supported_version",
]
