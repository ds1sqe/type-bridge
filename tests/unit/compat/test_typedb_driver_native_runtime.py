"""Exercise native constructors from the interpreter-selected TypeDB driver.

Import-only checks do not catch native-extension incompatibilities. This smoke
test deliberately crosses the FFI boundary without opening a network
connection, so the CPython 3.12–3.13/driver-3.11 and CPython
3.14/driver-3.12 CI lanes prove that their selected wheel is usable.
"""

from __future__ import annotations

import sys

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
    expected_minor = (3, 12) if sys.version_info >= (3, 14) else (3, 11)
    assert version == expected_minor
    assert typedb is not None
