#!/usr/bin/env python3
"""Run ordered generated Python/Node manager parity over custom-root TLS."""

from __future__ import annotations

import json
import os
import secrets
import shutil
import stat
import sys
import tempfile
from collections.abc import Mapping
from pathlib import Path

import run_phase5_manager_filter_live as live

TLS_ADDRESS_ENV = "TYPEDB_TLS_ADDRESS"
TLS_HTTP_PORT_ENV = "TYPEDB_TLS_HTTP_PORT"
TLS_ROOT_CA_ENV = "TYPEDB_TLS_ROOT_CA"

SELECTED_COMMANDS = frozenset(
    {
        "install the Python native facade",
        "generate the local Python package",
        "generate the foreign Python package",
        "produce the Python report",
        "build the Node package",
        "generate the local TypeScript package",
        "generate the foreign TypeScript package",
        "compile the local TypeScript package",
        "compile the foreign TypeScript package",
        "produce the Node report",
    }
)


def _required(environment: Mapping[str, str], name: str) -> str:
    value = environment.get(name)
    if value is None or value == "":
        raise live.RunnerError(f"{name} is required for ordered custom-root TLS parity")
    return value


def _root_ca(environment: Mapping[str, str]) -> Path:
    path = Path(_required(environment, TLS_ROOT_CA_ENV)).resolve()
    try:
        metadata = path.lstat()
    except OSError as error:
        raise live.RunnerError(f"{TLS_ROOT_CA_ENV} cannot be inspected: {error}") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        raise live.RunnerError(f"{TLS_ROOT_CA_ENV} must be one regular non-symlink file")
    return path


def _fixture(environment: Mapping[str, str]) -> live.Fixture:
    return live._required_fixture(
        {
            live.ADDRESS_ENV: _required(environment, TLS_ADDRESS_ENV),
            live.HTTP_PORT_ENV: _required(environment, TLS_HTTP_PORT_ENV),
        }
    )


def _require_tools() -> None:
    for executable in ("cargo", "maturin", "npm", "node"):
        if shutil.which(executable) is None:
            raise live.RunnerError(f"required ordered TLS tool is unavailable: {executable}")
    tsc = live.NODE_PACKAGE / "node_modules/.bin/tsc"
    if not tsc.is_file() or not os.access(tsc, os.X_OK):
        raise live.RunnerError("the TypeScript compiler is unavailable; run npm ci first")


def _parity_report(python_path: Path, node_path: Path) -> str:
    try:
        python_report = json.loads(python_path.read_bytes())
        node_report = json.loads(node_path.read_bytes())
    except (OSError, json.JSONDecodeError, RecursionError) as error:
        raise live.RunnerError(f"ordered TLS report could not be loaded: {error}") from error
    if not isinstance(python_report, dict) or not isinstance(node_report, dict):
        raise live.RunnerError("ordered TLS reports must be JSON objects")
    python_binding = python_report.pop("binding", None)
    node_binding = node_report.pop("binding", None)
    if python_binding != "python" or node_binding != "node" or python_report != node_report:
        raise live.RunnerError("ordered Python and Node TLS reports are not semantically identical")
    summary = {
        "bindings": ["node", "python"],
        "format": "typebridge.phase5-manager-filter-tls-summary/v1",
        "semantic_profile": python_report.get("semantic_profile"),
        "status": "passed",
        "transport": "custom_root_tls",
        "wrong_trust": "rejected",
    }
    return live._load_live_contract().canonical_json_bytes(summary).decode()


def run(environment: Mapping[str, str] = os.environ) -> str:
    fixture = _fixture(environment)
    root_ca = _root_ca(environment)
    contract = live._load_live_contract()
    for label, path in (
        ("Python producer", live.PYTHON_PRODUCER),
        ("Node producer", live.NODE_PRODUCER),
    ):
        contract.validate_producer_source(path.read_bytes())
    _require_tools()
    with tempfile.TemporaryDirectory(prefix="typebridge-phase5-manager-tls-") as temporary:
        layout = live.Layout.under(Path(temporary).resolve(), secrets.token_hex(12))
        live._prepare(layout)
        for command in live.command_plan(layout, fixture):
            if command.label not in SELECTED_COMMANDS:
                continue
            if command.environment is not None and command.label in {
                "produce the Python report",
                "produce the Node report",
            }:
                command.environment[TLS_ROOT_CA_ENV] = str(root_ca)
                if command.label == "produce the Node report":
                    command.environment["NODE_EXTRA_CA_CERTS"] = str(root_ca)
            live._run(command)
        live._require_regular_report(layout.python_report, "Python TLS producer", contract)
        live._require_regular_report(layout.node_report, "Node TLS producer", contract)
        return _parity_report(layout.python_report, layout.node_report)


def main() -> int:
    try:
        summary = run()
    except (live.RunnerError, OSError, ValueError) as error:
        print(f"Phase-5 ordered TLS parity failed: {error}", file=sys.stderr)
        return 1
    sys.stdout.write(summary)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
