"""The purchase workspace every SDK driver measures against.

``perf/drivers/workspace/schema`` is the Split-YAML schema the Rust driver
compiles in (``crates/rust/tests/support/purchase_plan.rs``); a unit test
keeps the two equal. A driver copies it into its work directory, makes the
one migration with the ``type-bridge`` CLI, and applies it to a fresh
database per pipeline, so every SDK runs on the same live schema the Rust
pipelines use.

The Node and C drivers call it as a command for their setup::

    python -m perf.drivers.workspace --root DIR --cli PATH create
    python -m perf.drivers.workspace --root DIR --cli PATH apply|drop DATABASE

``create`` copies the workspace into ``DIR/workspace``, makes the migration
and writes ``declared-schema.json`` there. TypeDB comes from the same
``TYPEDB_*`` variables the drivers read.
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path

SCHEMA = Path(__file__).parent / "workspace" / "schema"
# Mirrors `purchase_plan::{SCOPE, PROFILE}`.
SCOPE = "rust-plan-purchase"
PROFILE = "typedb-3.13.6/v1"

MANIFEST = """format: typebridge.workspace/v1
schema:
  root: schema/schema.yaml
  ownership: exclusive
  managed-scope: {scope}
compatibility:
  semantic-profile: {profile}
migrations:
  directory: migrations/v2
  app-label: perf
environments:
  perf:
    database: {database}
    uri: {address}
    http-port: '{port}'
    tls: 'false'
    migrate: 'true'
    credential:
      username: env:TYPEDB_USERNAME
      password: env:TYPEDB_PASSWORD
"""


@dataclass(frozen=True)
class Workspace:
    """A copied purchase workspace driven by one ``type-bridge`` CLI binary."""

    root: Path
    cli: Path
    address: str
    http_port: int

    @classmethod
    def create(cls, work_dir: Path, cli: Path, address: str, http_port: int) -> Workspace:
        root = work_dir / "workspace"
        if root.exists():
            raise ValueError(f"work directory {work_dir} already holds a workspace")
        shutil.copytree(SCHEMA, root / "schema")
        workspace = cls(root, cli, address, http_port)
        workspace.point_at("unused")
        workspace.run("migration", "make", "--name", "initial")
        return workspace

    def point_at(self, database: str) -> None:
        """Write the manifest with its one environment naming ``database``."""
        (self.root / "typebridge.yaml").write_text(
            MANIFEST.format(
                scope=SCOPE,
                profile=PROFILE,
                database=database,
                address=self.address,
                port=self.http_port,
            ),
            encoding="utf-8",
        )

    def run(self, *command: str) -> None:
        subprocess.run(
            [str(self.cli), "--manifest", str(self.root / "typebridge.yaml"), *command],
            check=True,
            stdout=subprocess.DEVNULL,
        )

    def apply(self, database: str) -> None:
        """Create ``database`` with the purchase schema by applying the chain."""
        self.point_at(database)
        self.run("migration", "apply", "--environment", "perf")

    def declared(self) -> bytes:
        """The canonical declared schema the query-only authority checks."""
        self.run("schema", "export-declared")
        return (self.root / "declared-schema.json").read_bytes()


def drop(address: str, http_port: int, username: str, password: str, database: str) -> None:
    """Delete a managed database and its migration journal, if they exist."""
    from type_bridge_core import PyRustDatabase

    for name in (database, f"{database}__tbv2_journal"):
        db = PyRustDatabase.connect(
            address, name, username=username, password=password, http_port=http_port
        )
        try:
            if db.database_exists():
                db.delete_database()
        finally:
            db.close()


def main(argv: Sequence[str] | None = None, env: Mapping[str, str] = os.environ) -> int:
    parser = argparse.ArgumentParser(prog="python -m perf.drivers.workspace")
    parser.add_argument("--root", type=Path, required=True, help="driver work directory")
    parser.add_argument("--cli", type=Path, required=True, help="type-bridge CLI binary")
    parser.add_argument("action", choices=["create", "apply", "drop"])
    parser.add_argument("database", nargs="?")
    arguments = parser.parse_args(argv)
    address = env.get("TYPEDB_ADDRESS") or "localhost:1729"
    http_port = int(env.get("TYPEDB_HTTP_PORT") or "8000")
    if arguments.action == "create":
        Workspace.create(arguments.root, arguments.cli, address, http_port).declared()
        return 0
    if not arguments.database:
        parser.error(f"{arguments.action} needs a DATABASE")
    workspace = Workspace(arguments.root / "workspace", arguments.cli, address, http_port)
    if arguments.action == "apply":
        workspace.apply(arguments.database)
    else:
        drop(address, http_port, env["TYPEDB_USERNAME"], env["TYPEDB_PASSWORD"], arguments.database)
    return 0


if __name__ == "__main__":
    sys.exit(main())
