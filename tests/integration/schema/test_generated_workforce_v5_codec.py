"""Provider-free acceptance for Python's generated Workforce V5 codec."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

pytestmark = pytest.mark.integration

ROOT = Path(__file__).resolve().parents[3]
PRODUCER = ROOT / "type-bridge-core/crates/schema-codegen/tests/acceptance/workforce_v5_codec.py"


def run_producer(stage: Path, output: Path) -> None:
    environment = os.environ.copy()
    environment["PYTHONPATH"] = os.pathsep.join(
        [str(stage), environment.get("PYTHONPATH", "")]
    ).rstrip(os.pathsep)
    environment["TYPE_BRIDGE_WORKFORCE_V5_CORPUS"] = str(output)
    subprocess.run([sys.executable, str(PRODUCER)], cwd=ROOT, env=environment, check=True)


def test_generated_python_workforce_v5_canonical_codec(tmp_path: Path) -> None:
    stage = tmp_path / "generated"
    subprocess.run(
        [
            str(ROOT / "scripts/ci/prepare_generated_live_fixture.sh"),
            "python",
            str(stage),
        ],
        cwd=ROOT,
        check=True,
    )
    first = tmp_path / "python-workforce-v5-corpus.json"
    repeated = tmp_path / "python-workforce-v5-corpus-repeat.json"
    run_producer(stage, first)
    run_producer(stage, repeated)
    assert first.read_bytes() == repeated.read_bytes()
    corpus = json.loads(first.read_bytes())
    assert corpus["format"] == "typebridge.workforce-v5-provider-free-corpus/v1"
    assert corpus["binding"] == "python"
    assert len(corpus["record_b64"]) == 9
    assert corpus["archive_b64"]
