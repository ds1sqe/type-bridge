"""TypeDB driver re-exports for convenience.

This module re-exports the TypeDB driver components so users can import everything
from type_bridge instead of mixing imports from typedb.driver.

Example:
    from type_bridge import Database, Credentials, TypeDB

    # Instead of:
    # from typedb.driver import Credentials, TypeDB
    # from type_bridge import Database
"""

from typedb.driver import Credentials, DriverOptions, DriverTlsConfig, TransactionType, TypeDB


def create_driver_options(is_tls_enabled: bool = False) -> DriverOptions:
    """Create TypeDB driver options for the configured TLS mode.

    TypeDB driver 3.11+ configures TLS via an explicit ``DriverTlsConfig`` rather
    than a boolean flag.
    """
    tls_config = (
        DriverTlsConfig.enabled_with_native_root_ca()
        if is_tls_enabled
        else DriverTlsConfig.disabled()
    )
    return DriverOptions(tls_config)


__all__ = [
    "Credentials",
    "DriverOptions",
    "TransactionType",
    "TypeDB",
    "create_driver_options",
]
