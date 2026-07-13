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
    import type_bridge.version as _version  # local import avoids circular dependency

    installed = driver_version()
    b = _version.band(installed)

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
        min_v = _version.min_supported_version()
        max_l = _version.max_supported_line()
        raise _version.UnsupportedVersionError(
            f"Installed typedb-driver {installed!r} has no known protocol band; "
            f"supported driver lines fall in {min_v}–{max_l}.x. "
            f"Install a supported driver version (e.g. `pip install typedb-driver~=3.10`)."
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

    Every TypeBridge transaction executes through the embedded Rust driver
    (the Python ORM backend was retired), so the server must be
    protocol-compatible with this version as well as with the installed
    Python driver.  Delegates to ``type_bridge_core.embedded_driver_version``.

    Returns the band-8 (3.11.x) pin for back-compat.  Use
    :func:`embedded_driver_versions` to get all compiled-in bands.
    """
    return _core.embedded_driver_version()


def embedded_driver_versions() -> dict[int, str]:
    """Return all driver versions compiled into the Rust runtime, keyed by band.

    Delegates to ``type_bridge_core.embedded_driver_versions``.  The default
    build returns ``{7: "3.8.1", 8: "3.11.5"}``; a single-band build returns
    only the one entry for its compiled band.
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
