"""Command-ledger contract for the four-binding Sdk V3 runner."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
CI = ROOT / "scripts/ci"
sys.path.insert(0, str(CI))
SPEC = importlib.util.spec_from_file_location(
    "sdk_v3_artifact_runner", CI / "run_sdk_v3_artifact.py"
)
assert SPEC is not None and SPEC.loader is not None
RUNNER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = RUNNER
SPEC.loader.exec_module(RUNNER)


def capture(monkeypatch: pytest.MonkeyPatch) -> list[list[str]]:
    commands: list[list[str]] = []
    monkeypatch.setattr(RUNNER, "run", lambda command, **_kwargs: commands.append(command))
    return commands


def test_phase_report_ledger_uses_all_three_validated_fan_ins(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    commands = capture(monkeypatch)
    RUNNER.run_phase_reports(
        tmp_path,
        {"TYPEDB_ADDRESS": "127.0.0.1:1729", "TYPEDB_HTTP_PORT": "8000"},
    )
    assert len(commands) == 3
    for marker in (
        "run_projected_parity.py",
        "run_projected_live.py",
        "run_manager_filter_live.py",
    ):
        assert sum(any(item.endswith(marker) for item in command) for command in commands) == 1


def test_assembly_runs_four_compositors_four_assemblers_and_one_fan_in(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    commands = capture(monkeypatch)
    (tmp_path / "fragments").mkdir()
    RUNNER.assemble(tmp_path, {}, "0" * 64)
    assert len(commands) == 9
    assert sum(any("compose_sdk_v3" in item for item in command) for command in commands) == 4
    assert (
        sum(any("assemble_sdk_conformance_v3" in item for item in command) for command in commands)
        == 4
    )
    assert any("compare_sdk_conformance_v3" in item for item in commands[-1])


def test_runner_names_all_live_and_proof_producers() -> None:
    source = (CI / "run_sdk_v3_artifact.py").read_text(encoding="utf-8")
    for marker in (
        "test_generated_data_model_runtime_v3_live",
        "node.generated_data_model_runtime_v3_live",
        "generated_rust_projection_round_trips_exact_live_models",
        "generated_data_model_runtime_v3_live",
        "sdk_v3_python_data_plane_fragment",
        "sdk_v3_node_data_plane_fragment",
        "sdk_v3_rust_data_plane_fragment",
        "sdk_v3_c_data_plane_fragment",
    ):
        assert marker in source
