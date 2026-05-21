"""TypeDB driver re-exports for convenience.

This module re-exports the TypeDB driver components so users can import everything
from type_bridge instead of mixing imports from typedb.driver.

Example:
    from type_bridge import Database, Credentials, TypeDB

    # Instead of:
    # from typedb.driver import Credentials, TypeDB
    # from type_bridge import Database
"""

from typedb.driver import Credentials, DriverOptions, TransactionType, TypeDB


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
