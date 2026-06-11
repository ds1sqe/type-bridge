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
from type_bridge_core import check_server_supported as _check_server_supported
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


def ensure_runtime_supported(server: str) -> None:
    """Assert the embedded Rust runtime can serve the detected server.

    Every TypeBridge transaction executes through the Rust runtime's own
    typedb-driver, and the default build embeds drivers for the full supported
    window (3.8–3.11).  Core makes the decision using band-set membership —
    the band set is derived from the compiled features in the Rust runtime
    (``check_server_supported`` reads them directly), so any in-window server
    is accepted by the default build.  This wrapper only reframes the failure
    — the "install a different typedb-driver" remediation does not apply to a
    driver compiled into the wheel.

    Args:
        server: Detected server version string.

    Raises:
        UnsupportedVersionError: When the server is outside the supported window
            or (in a non-default single-band build) its band is not compiled in.
            The message names the supported window and gives wheel-appropriate
            remediation.
    """
    try:
        _check_server_supported(server)
    except type_bridge_core.VersionError as exc:
        min_v = min_supported_version()
        max_l = max_supported_line()
        raise UnsupportedVersionError(
            f"TypeBridge's embedded runtime does not support server {server}: {exc} "
            f"(use a TypeDB server in the supported window {min_v}–{max_l}.x, "
            f"or upgrade to a type-bridge release that covers this server line)"
        ) from exc


__all__ = [
    "UnsupportedVersionError",
    "band",
    "ensure_runtime_supported",
    "ensure_supported",
    "max_supported_line",
    "min_supported_version",
]
