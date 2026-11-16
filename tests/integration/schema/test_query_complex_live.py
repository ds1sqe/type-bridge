"""Keep the shared complex-query fixture in the ordinary integration tier."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest

from tests.utils import typedb_lifecycle
from type_bridge import Database

pytestmark = pytest.mark.integration
ROOT = Path(__file__).resolve().parents[3]


def test_complex_queries_in_every_generated_sdk(docker_typedb: None, tmp_path: Path) -> None:
    address = typedb_lifecycle.TEST_DB_ADDRESS
    port = typedb_lifecycle.TEST_DB_HTTP_PORT
    connection = Database(address=address, database="complex_query_version_probe", http_port=port)
    connection.connect()
    try:
        version = connection.detected_server_version()
    finally:
        connection.close()
    assert version is not None
    environment = dict(os.environ)
    environment.setdefault("CARGO_TARGET_DIR", str(ROOT / "type-bridge-core/target"))
    for name in ("TYPE_BRIDGE_COMPLEX_WIDTH", "TYPE_BRIDGE_COMPLEX_RAW_QUERY"):
        environment.pop(name, None)
    subprocess.run(
        [
            sys.executable,
            str(ROOT / "tests/live/query_complex/run.py"),
            "--server",
            f"{version},{address},{port}",
            "--database-prefix",
            f"{typedb_lifecycle.TEST_DB_NAME}_complex_",
            "--size",
            "16",
            "--output",
            str(tmp_path / "complex-query"),
        ],
        cwd=ROOT,
        env=environment,
        check=True,
        timeout=1800,
    )
