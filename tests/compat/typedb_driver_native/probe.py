#!/usr/bin/env python3
"""Exercise the optional TypeDB driver's native constructors from installed wheels."""

from __future__ import annotations

import argparse
import importlib.metadata
import json
import sys
from collections.abc import Sequence
from typing import Any


class ProbeError(RuntimeError):
    """The installed artifact set did not provide a usable optional driver."""


def driver_line(version: str) -> tuple[int, int]:
    """Return the numeric major/minor line from one distribution version."""
    try:
        major, minor, *_ = version.split(".")
        return int(major), int(minor)
    except (TypeError, ValueError) as error:
        raise ProbeError(f"Invalid typedb-driver version: {version!r}") from error


def main(argv: Sequence[str] | None = None) -> int:
    """Cross the native FFI boundary without opening a network connection."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--expected-version", required=True)
    args = parser.parse_args(argv)

    root_version = importlib.metadata.version("type-bridge")
    core_version = importlib.metadata.version("type-bridge-core")
    if (root_version, core_version) != (args.expected_version, args.expected_version):
        raise ProbeError(
            "Installed Python artifact versions disagree with the release tag: "
            f"root={root_version}, core={core_version}, expected={args.expected_version}"
        )

    import typedb
    from typedb.driver import Credentials, DriverOptions, DriverTlsConfig

    from type_bridge.typedb_driver import create_driver_options, driver_version

    credentials = Credentials("admin", "password")
    tls_config = DriverTlsConfig.disabled()
    driver_options_class: Any = DriverOptions
    options = driver_options_class(tls_config)
    bridge_options = create_driver_options(is_tls_enabled=False)
    if any(value is None for value in (typedb, credentials, tls_config, options, bridge_options)):
        raise ProbeError("A TypeDB native constructor returned an invalid object")

    installed_driver = driver_version()
    line = driver_line(installed_driver)
    if sys.version_info >= (3, 14):
        if installed_driver != "3.12.0":
            raise ProbeError(
                f"Python 3.14 must resolve typedb-driver 3.12.0, got {installed_driver}"
            )
    elif not (line[0] == 3 and 8 <= line[1] < 13):
        raise ProbeError(
            f"Python 3.13 resolved an unsupported typedb-driver line: {installed_driver}"
        )

    print(
        json.dumps(
            {
                "core_version": core_version,
                "driver_version": installed_driver,
                "python_version": f"{sys.version_info.major}.{sys.version_info.minor}",
                "root_version": root_version,
                "status": "ok",
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ProbeError as error:
        print(f"typedb-driver artifact smoke failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
