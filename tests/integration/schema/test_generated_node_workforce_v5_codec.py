"""Provider-free acceptance for Node's generated Workforce V5 codec."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

pytestmark = pytest.mark.integration

ROOT = Path(__file__).resolve().parents[3]
CORE = ROOT / "type-bridge-core"
NODE_PACKAGE = CORE / "crates/node"
PRODUCER = CORE / "crates/schema-codegen/tests/typescript_acceptance/workforce_v5_codec.mjs"


def native_artifact() -> Path:
    target = CORE / "target/debug"
    if sys.platform == "win32":
        return target / "type_bridge_node.dll"
    if sys.platform == "darwin":
        return target / "libtype_bridge_node.dylib"
    return target / "libtype_bridge_node.so"


def install_runtime(stage: Path) -> None:
    destination = stage / "node_modules/@type-bridge/node"
    destination.mkdir(parents=True)
    shutil.copy2(NODE_PACKAGE / "package.json", destination / "package.json")
    shutil.copytree(NODE_PACKAGE / "dist", destination / "dist")
    artifact = native_artifact()
    if not artifact.is_file():
        raise AssertionError("build the Node native addon before Workforce V5 acceptance")
    shutil.copy2(artifact, destination / "type_bridge_node.node")


def run_producer(stage: Path, output: Path, operational: Path) -> None:
    environment = os.environ.copy()
    environment["TYPE_BRIDGE_GENERATED_NODE_PACKAGE"] = str(
        stage / "generated_ordered/dist/index.js"
    )
    environment["TYPE_BRIDGE_GENERATED_NODE_FOREIGN_PACKAGE"] = str(
        stage / "generated_ordered_foreign/dist/index.js"
    )
    environment["TYPE_BRIDGE_WORKFORCE_V5_CORPUS"] = str(output)
    environment["TYPE_BRIDGE_WORKFORCE_V5_OPERATIONAL_EVIDENCE"] = str(operational)
    subprocess.run(["node", str(PRODUCER)], cwd=ROOT, env=environment, check=True)


def test_generated_node_workforce_v5_canonical_codec(tmp_path: Path) -> None:
    stage = tmp_path / "generated"
    subprocess.run(
        [
            str(ROOT / "scripts/ci/prepare_generated_live_fixture.sh"),
            "node",
            str(stage),
        ],
        cwd=ROOT,
        check=True,
    )
    install_runtime(stage)
    first = Path(
        os.environ.get(
            "TYPE_BRIDGE_WORKFORCE_V5_NODE_CORPUS",
            tmp_path / "node-workforce-v5-corpus.json",
        )
    )
    repeated = tmp_path / "node-workforce-v5-corpus-repeat.json"
    operational = Path(
        os.environ.get(
            "TYPE_BRIDGE_WORKFORCE_V5_NODE_OPERATIONAL_EVIDENCE",
            tmp_path / "node-workforce-v5-operational.json",
        )
    )
    repeated_operational = tmp_path / "node-workforce-v5-operational-repeat.json"
    run_producer(stage, first, operational)
    run_producer(stage, repeated, repeated_operational)
    assert first.read_bytes() == repeated.read_bytes()
    assert operational.read_bytes() == repeated_operational.read_bytes()
    corpus = json.loads(first.read_bytes())
    assert corpus["format"] == "typebridge.workforce-v5-provider-free-corpus/v1"
    assert corpus["binding"] == "node"
    assert len(corpus["record_b64"]) == 9
    assert corpus["archive_b64"]
    evidence = json.loads(operational.read_bytes())
    assert evidence["format"] == "typebridge.workforce-v5-operational-evidence/v1"
    assert evidence["binding"] == "node"
    assert evidence["diagnostic"] == {
        "category": "invalid_input",
        "code": "projected_record_schema_mismatch",
        "path": ["declared_schema_identity"],
        "payload_absent": True,
    }
    assert evidence["lifecycle"]["sibling_usable"] is True
