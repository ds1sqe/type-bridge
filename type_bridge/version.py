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
    """Raised when the driver/server pair is below the admission floor or spans
    incompatible protocol bands.

    The exception message contains both detected version strings and a
    human-readable remediation hint (e.g. which ``typedb-driver`` version to
    install) so callers never need to parse protocol numbers.

    This is a subclass of ``type_bridge_core.VersionError``, so existing
    ``except type_bridge_core.VersionError`` catches work unchanged while
    callers that import only ``type_bridge`` can target this type directly.
    """


def ensure_supported(driver: str, server: str) -> None:
    """Assert that a driver/server pair meets the admission floor and protocol band.

    Translates ``type_bridge_core.VersionError`` into
    :class:`UnsupportedVersionError` so all version-gate failures surface as the
    documented TypeBridge exception type.  Contains no comparison logic — the
    decision is entirely core's.

    Args:
        driver: Installed driver version string (e.g. ``"3.11.5"``).
        server: Detected server version string (e.g. ``"3.11.5"``).

    Raises:
        UnsupportedVersionError: When the pair is below the admission floor or crosses
            protocol bands.  The message names both versions and a remediation
            hint.
    """
    try:
        _check_supported(driver, server)
    except type_bridge_core.VersionError as exc:
        raise UnsupportedVersionError(str(exc)) from exc


def ensure_runtime_supported(server: str) -> None:
    """Check the embedded runtime's version admission for the detected server.

    Every TypeBridge transaction executes through the Rust runtime's own
    typedb-driver, and the default build embeds drivers covering the full
    measured lines (3.11–3.14; confirmed 3.12, 3.13 and 3.14 servers normally
    negotiate embedded band 9, with band 8 retained for discovery/fallback).
    Newer versions are admitted using the newest measured assumptions and
    are not tested or supported by this TypeBridge release. The runtime emits
    one warning per connection to a newer line. Core makes
    the decision using band-set intersection —
    the band set is derived from the compiled features in the Rust runtime
    (``check_server_supported`` reads them directly), so any admitted server
    is accepted by the default build.  This wrapper only reframes the failure
    — the "install a different typedb-driver" remediation does not apply to a
    driver compiled into the wheel.

    Args:
        server: Detected server version string.

    Raises:
        UnsupportedVersionError: When the server is below the admission floor
            or (in a non-default single-band build) its band is not compiled in.
            The message names the admission floor and gives wheel-appropriate
            remediation.
    """
    try:
        _check_server_supported(server)
    except type_bridge_core.VersionError as exc:
        min_v = min_supported_version()
        raise UnsupportedVersionError(
            f"TypeBridge's embedded runtime does not support server {server}: {exc} "
            f"(minimum admitted version {min_v}; use a type-bridge release "
            f"with a compatible embedded protocol driver)"
        ) from exc


__all__ = [
    "UnsupportedVersionError",
    "band",
    "ensure_runtime_supported",
    "ensure_supported",
    "max_supported_line",
    "min_supported_version",
]
