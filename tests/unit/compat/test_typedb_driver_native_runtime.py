"""Exercise native constructors from the interpreter-selected TypeDB driver.

Import-only checks do not catch native-extension incompatibilities. This smoke
test deliberately crosses the FFI boundary without opening a network
connection, so every supported interpreter proves its retained driver wheel
is usable.
"""

from __future__ import annotations

import typedb
from typedb.driver import Credentials, DriverTlsConfig

from type_bridge.typedb_driver import create_driver_options, driver_version


def test_interpreter_selected_driver_native_constructors() -> None:
    credentials = Credentials("admin", "password")
    tls_config = DriverTlsConfig.disabled()
    options = create_driver_options(is_tls_enabled=False)

    assert credentials is not None
    assert tls_config is not None
    assert options is not None

    version = tuple(int(part) for part in driver_version().split(".")[:2])
    assert version == (3, 14)
    assert typedb is not None
