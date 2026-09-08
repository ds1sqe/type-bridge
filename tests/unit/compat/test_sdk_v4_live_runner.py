"""Persistence contract for the exact four-binding Sdk V4 runner."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
RUNNER_PATH = ROOT / "scripts/ci/run_sdk_v4_live.py"
SPEC = importlib.util.spec_from_file_location("sdk_v4_live_runner", RUNNER_PATH)
assert SPEC is not None and SPEC.loader is not None
RUNNER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = RUNNER
SPEC.loader.exec_module(RUNNER)


def test_validated_reports_publish_with_binding_names(tmp_path: Path) -> None:
    sources = []
    for binding in ("python", "node", "rust", "c"):
        source = tmp_path / f"source-{binding}.json"
        source.write_bytes(f'{{"binding":"{binding}"}}'.encode())
        sources.append(source)
    output = tmp_path / "reports"
    RUNNER.persist_reports(sources, output)
    assert sorted(path.name for path in output.iterdir()) == [
        "c.json",
        "node.json",
        "python.json",
        "rust.json",
    ]
    for binding in ("python", "node", "rust", "c"):
        assert (output / f"{binding}.json").read_bytes() == (
            tmp_path / f"source-{binding}.json"
        ).read_bytes()


def test_persistence_is_create_new(tmp_path: Path) -> None:
    output = tmp_path / "reports"
    output.mkdir()
    with pytest.raises(RUNNER.RunnerError, match="new directory"):
        RUNNER.persist_reports([], output)
