"""Provider-free acceptance for Python's generated Serialization codec."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

pytestmark = pytest.mark.integration

ROOT = Path(__file__).resolve().parents[3]
PRODUCER = (
    ROOT / "type-bridge-core/crates/schema-codegen/tests/acceptance/sdk_serialization_codec.py"
)


def run_producer(stage: Path, output: Path, operational: Path) -> None:
    environment = os.environ.copy()
    environment["PYTHONPATH"] = os.pathsep.join(
        [str(stage), environment.get("PYTHONPATH", "")]
    ).rstrip(os.pathsep)
    environment["TYPE_BRIDGE_SDK_SERIALIZATION_CORPUS"] = str(output)
    environment["TYPE_BRIDGE_SDK_SERIALIZATION_OPERATIONAL_EVIDENCE"] = str(operational)
    subprocess.run([sys.executable, str(PRODUCER)], cwd=ROOT, env=environment, check=True)


def test_sdk_serialization_canonical_codec(tmp_path: Path) -> None:
    stage = tmp_path / "generated"
    # Offline and byte-checked against the frozen serialization vectors, so it stays on
    # the catalog's own profile on every lane.
    fixture_environment = os.environ.copy()
    fixture_environment["TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE"] = "typedb-3.12.1/v1"
    subprocess.run(
        [
            str(ROOT / "scripts/ci/prepare_generated_live_fixture.sh"),
            "python",
            str(stage),
        ],
        cwd=ROOT,
        env=fixture_environment,
        check=True,
    )
    first = Path(
        os.environ.get(
            "TYPE_BRIDGE_SDK_SERIALIZATION_PYTHON_CORPUS",
            tmp_path / "python-serialization-corpus.json",
        )
    )
    repeated = tmp_path / "python-serialization-corpus-repeat.json"
    operational = Path(
        os.environ.get(
            "TYPE_BRIDGE_SDK_SERIALIZATION_PYTHON_OPERATIONAL_EVIDENCE",
            tmp_path / "python-serialization-operational.json",
        )
    )
    repeated_operational = tmp_path / "python-serialization-operational-repeat.json"
    run_producer(stage, first, operational)
    run_producer(stage, repeated, repeated_operational)
    assert first.read_bytes() == repeated.read_bytes()
    assert operational.read_bytes() == repeated_operational.read_bytes()
    corpus = json.loads(first.read_bytes())
    assert corpus["format"] == "typebridge.sdk-serialization-provider-free-corpus/v1"
    assert corpus["binding"] == "python"
    assert len(corpus["record_b64"]) == 9
    assert corpus["archive_b64"]
    evidence = json.loads(operational.read_bytes())
    assert evidence["format"] == "typebridge.sdk-serialization-operational-evidence/v1"
    assert evidence["binding"] == "python"
    assert evidence["diagnostic"] == {
        "category": "invalid_input",
        "code": "projected_record_schema_mismatch",
        "path": ["declared_schema_identity"],
        "payload_absent": True,
    }
    assert evidence["lifecycle"]["sibling_usable"] is True
