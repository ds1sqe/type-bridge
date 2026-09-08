"""Command and cleanup contracts for the four-binding Sdk V5 runner."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
CI = ROOT / "scripts/ci"
sys.path.insert(0, str(CI))
SPEC = importlib.util.spec_from_file_location(
    "sdk_v5_artifact_runner", CI / "run_sdk_v5_artifact.py"
)
assert SPEC is not None and SPEC.loader is not None
RUNNER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = RUNNER
SPEC.loader.exec_module(RUNNER)


def test_cleanup_evidence_is_exact_and_non_inferential(tmp_path: Path) -> None:
    path = tmp_path / "cleanup.json"
    RUNNER.write_cleanup(path, "node")
    assert json.loads(path.read_bytes()) == {
        "binding": "node",
        "format": "typebridge.sdk-v5-cleanup-evidence/v1",
        "managed_database_absent": True,
        "partial_output_absent": True,
        "temporary_evidence_absent": True,
    }
    assert path.read_bytes() == RUNNER.conformance.canonical_json_bytes(
        json.loads(path.read_bytes())
    )


def test_assembly_runs_each_real_composer_and_one_fan_in(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    commands: list[list[str]] = []
    monkeypatch.setattr(
        RUNNER,
        "run",
        lambda command, **_kwargs: commands.append(command),
    )
    RUNNER.assemble(tmp_path / "evidence", tmp_path / "reports", {})
    assert len(commands) == 9
    assert (
        sum(
            any(item.endswith("compose_sdk_v5_evidence.py") for item in command)
            for command in commands
        )
        == 4
    )
    assert (
        sum(
            any(item.endswith("assemble_sdk_conformance_v5.py") for item in command)
            for command in commands
        )
        == 4
    )
    assert any(item.endswith("compare_sdk_conformance_v5.py") for item in commands[-1])


def test_provider_free_fan_in_names_each_binding(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    commands: list[list[str]] = []
    monkeypatch.setattr(
        RUNNER,
        "run",
        lambda command, **_kwargs: commands.append(command),
    )
    RUNNER.provider_free(tmp_path, {})
    fan_in = [command for command in commands if any("compare_sdk_v5_" in item for item in command)]
    assert len(fan_in) == 2
    for command in fan_in:
        for binding in RUNNER.BINDINGS:
            assert command.count(f"--{binding}") == 1


def test_runner_names_all_four_live_producer_paths() -> None:
    source = (CI / "run_sdk_v5_artifact.py").read_text(encoding="utf-8")
    for marker in (
        "test_generated_canonical_serialization_v5_live",
        "node.generated_canonical_serialization_v5_live",
        "generated_rust_projection_round_trips_exact_live_models",
        "live_c17_generated_person_and_membership_crud_round_trips_exact_3_12_3",
    ):
        assert source.count(marker) == 1
