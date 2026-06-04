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

from enum import Enum
from typing import TYPE_CHECKING, Any

_MISSING_DRIVER_MESSAGE = (
    "The Python TypeDB driver is required for this operation. Install "
    "type-bridge with the 'python-backend' extra to use the transition "
    "Python backend or direct typedb.driver APIs."
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
    """Raise the transition-extra error for direct Python driver use."""
    raise ImportError(_MISSING_DRIVER_MESSAGE)


def create_driver_options(is_tls_enabled: bool = False) -> DriverOptions:
    """Create TypeDB driver options across supported driver versions."""
    try:
        return DriverOptions(is_tls_enabled=is_tls_enabled)
    except TypeError:
        typedb_driver = __import__("typedb.driver", fromlist=["DriverTlsConfig"])
        driver_tls_config = getattr(typedb_driver, "DriverTlsConfig")

        tls_config = (
            driver_tls_config.enabled_with_native_root_ca()
            if is_tls_enabled
            else driver_tls_config.disabled()
        )
        return DriverOptions(tls_config)


__all__ = [
    "Credentials",
    "DriverOptions",
    "TransactionType",
    "TypeDB",
    "create_driver_options",
]
