"""TypeDB driver compatibility helpers.

This module re-exports the TypeDB driver components so users can import everything
from type_bridge instead of mixing imports from typedb.driver.

Example:
    from type_bridge import Database, Credentials, TypeDB

    # Instead of:
    # from typedb.driver import Credentials, TypeDB
    # from type_bridge import Database
"""

from __future__ import annotations

import importlib.metadata
import sys
from enum import Enum
from typing import TYPE_CHECKING, Any

import type_bridge_core as _core

# Re-exported from the Rust core so every Python default shares the same
# source as the Rust ConnectOptions/server defaults.
DEFAULT_HTTP_PORT: int = _core.DEFAULT_HTTP_PORT

_MISSING_DRIVER_MESSAGE = (
    "The Python TypeDB driver is required for this operation. Install "
    "type-bridge with the 'typedb-driver' extra to use direct typedb.driver "
    "APIs."
)

if TYPE_CHECKING:
    from typedb.driver import Credentials, DriverOptions, TransactionType, TypeDB
else:
    try:
        from typedb.driver import Credentials, DriverOptions, TransactionType, TypeDB
    except ModuleNotFoundError:

        class TransactionType(Enum):
            """Rust-safe fallback transaction type for the default backend."""

            READ = "read"
            WRITE = "write"
            SCHEMA = "schema"

        class _UnavailableDriverClass:
            def __init__(self, *args: Any, **kwargs: Any) -> None:
                del args, kwargs
                raise_missing_typedb_driver()

            def __getattr__(self, name: str) -> Any:
                del name
                raise_missing_typedb_driver()

        class _UnavailableTypeDB:
            def __getattr__(self, name: str) -> Any:
                del name
                raise_missing_typedb_driver()

        Credentials = _UnavailableDriverClass
        DriverOptions = _UnavailableDriverClass
        TypeDB = _UnavailableTypeDB()


def raise_missing_typedb_driver() -> None:
    """Raise the optional-driver error for direct Python driver use."""
    raise ImportError(_MISSING_DRIVER_MESSAGE)


def _load_tls_config() -> Any:
    """Import and return ``DriverTlsConfig`` from a band-8+ typedb driver.

    Isolated so tests can patch ``type_bridge.typedb_driver._load_tls_config``
    without needing a real band-8/band-9 driver installed.
    """
    from typedb.driver import DriverTlsConfig  # type: ignore[attr-defined]

    return DriverTlsConfig


def _ensure_driver_interpreter_supported(installed: str) -> int | None:
    """Return the protocol band after rejecting unsafe native-wheel pairs.

    Driver metadata import can succeed even when its bundled native library is
    incompatible with the running interpreter. Keep this check ahead of every
    native constructor so a manually installed 3.11 wheel cannot crash
    CPython 3.14 before TypeBridge can report the supported 3.12 path.
    """
    import type_bridge.version as _version  # local import avoids circular dependency

    driver_band = _version.band(installed)
    if sys.version_info >= (3, 14) and driver_band != 9:
        raise _version.UnsupportedVersionError(
            f"Installed typedb-driver {installed!r} has no compatible native wheel "
            "for CPython 3.14. Install `type-bridge[typedb-driver]` "
            "(driver 3.12.0) and target TypeDB 3.12."
        )
    return driver_band


def create_driver_options(is_tls_enabled: bool = False) -> DriverOptions:
    """Create TypeDB driver options using explicit band-keyed dispatch.

    The same band map that drives the version gate drives option construction:
    band-7 drivers (3.8/3.10) use the keyword form
    ``DriverOptions(is_tls_enabled=…)``; band-8 (3.11) and band-9 (3.12)
    drivers use the positional ``DriverOptions(tls_config)`` form.

    Args:
        is_tls_enabled: Whether to enable TLS for the driver connection.

    Returns:
        Configured ``DriverOptions`` instance.

    Raises:
        UnsupportedVersionError: When the installed driver version is outside
            the supported range (no known band).
    """
    installed = driver_version()
    b = _ensure_driver_interpreter_supported(installed)

    if b == 7:
        return DriverOptions(is_tls_enabled=is_tls_enabled)
    elif b in (8, 9):
        driver_tls_config = _load_tls_config()
        tls_config = (
            driver_tls_config.enabled_with_native_root_ca()
            if is_tls_enabled
            else driver_tls_config.disabled()
        )
        return DriverOptions(tls_config)
    else:
        import type_bridge.version as _version  # local import avoids circular dependency

        min_v = _version.min_supported_version()
        max_l = _version.max_supported_line()
        if sys.version_info >= (3, 14):
            remediation = (
                "Install `type-bridge[typedb-driver]` (driver 3.12.0 on "
                "CPython 3.14) and target TypeDB 3.12."
            )
        else:
            remediation = (
                "Install `type-bridge[typedb-driver]` and select a driver line "
                "accepted by the target server."
            )
        raise _version.UnsupportedVersionError(
            f"Installed typedb-driver {installed!r} has no known protocol band; "
            f"supported driver lines fall in {min_v}–{max_l}.x. "
            f"{remediation}"
        )


def driver_version() -> str:
    """Return the installed typedb-driver package version.

    This is the one version fact Python computes itself: a Python-runtime
    metadata query that Rust cannot observe.  The result matches whatever
    ``typedb-driver`` release is installed in the current environment.
    """
    return importlib.metadata.version("typedb-driver")


def embedded_driver_version() -> str:
    """Return the typedb-driver version compiled into the Rust runtime.

    Every ORM transaction executes through the embedded Rust drivers; their
    accepted-band set is gated independently of the optional installed Python
    driver. The latter is consulted only for direct ``typedb.driver`` access.
    Delegates to ``type_bridge_core.embedded_driver_version``.

    Returns the band-8 (3.11.x) pin for back-compat.  Use
    :func:`embedded_driver_versions` to get all compiled-in bands.
    """
    return _core.embedded_driver_version()


def embedded_driver_versions() -> dict[int, str]:
    """Return all driver versions compiled into the Rust runtime, keyed by band.

    Delegates to ``type_bridge_core.embedded_driver_versions``.  The default
    build returns ``{7: "3.8.1", 8: "3.11.5", 9: "3.12.0"}``; a
    single-band build returns only the one entry for its compiled band.
    """
    return _core.embedded_driver_versions()


def server_version(address: str, *, http_port: int = DEFAULT_HTTP_PORT, tls: bool = False) -> str:
    """Return the TypeDB server version by probing its HTTP API.

    Delegates entirely to ``type_bridge_core.server_version``; no HTTP code
    lives here.  ``address`` follows the connect-address form ``"host:1729"``;
    the core layer derives the HTTP host from it and handles TLS.

    Args:
        address: Connect address in ``"host:port"`` form (e.g. ``"localhost:1729"``).
        http_port: HTTP API port (default 8000).
        tls: Whether to use HTTPS for the version probe.

    Returns:
        Version string reported by the server (e.g. ``"3.10.4"``).

    Raises:
        type_bridge_core.VersionError: When the endpoint is unreachable or the
            response cannot be parsed.
    """
    return _core.server_version(address, http_port, tls)


__all__ = [
    "Credentials",
    "DriverOptions",
    "TransactionType",
    "TypeDB",
    "create_driver_options",
    "driver_version",
    "embedded_driver_version",
    "embedded_driver_versions",
    "server_version",
]
