"""Command-ledger and publication tests for the Sdk V1/V2 runner."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
CI = ROOT / "scripts/ci"
SPEC = importlib.util.spec_from_file_location(
    "sdk_v1_v2_artifact_runner", CI / "run_sdk_v1_v2_artifact.py"
)
assert SPEC is not None and SPEC.loader is not None
RUNNER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = RUNNER
SPEC.loader.exec_module(RUNNER)


def test_fragment_ledger_covers_each_binding_and_exact_validator(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    commands: list[list[str]] = []
    monkeypatch.setattr(RUNNER, "run", lambda command, **_kwargs: commands.append(command))
    RUNNER.emit_fragments(tmp_path, {}, "0" * 64)
    assert (
        sum(any("sdk_v2_proof_fragments.py" in item for item in command) for command in commands)
        == 4
    )
    assert [
        "uv",
        "run",
        "python",
        "type-bridge-core/crates/schema-codegen/tests/acceptance/check.py",
    ] in commands
    source = (CI / "run_sdk_v1_v2_artifact.py").read_text(encoding="utf-8")
    for marker in (
        "python_direct_cancellation_fragment_is_measured_from_owned_execution",
        "node_direct_cancellation_fragment_is_measured_from_owned_execution",
        "sdk_v2_rust_deterministic_proof_fragment",
        "sdk_v2_c_deterministic_proof_fragment",
    ):
        assert source.count(marker) == 1


def test_comparator_ledger_preserves_v1_three_and_v2_four(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    commands: list[list[str]] = []
    monkeypatch.setattr(RUNNER, "run", lambda command, **_kwargs: commands.append(command))
    RUNNER.compare(tmp_path, {})
    assert len(commands) == 2
    assert len(commands[0]) == 2 + len(RUNNER.V1_BINDINGS)
    assert len(commands[1]) == 2 + len(RUNNER.V2_BINDINGS)


def test_publication_is_one_create_new_nested_artifact(tmp_path: Path) -> None:
    scratch = tmp_path / "scratch"
    for version, bindings in (("v1", RUNNER.V1_BINDINGS), ("v2", RUNNER.V2_BINDINGS)):
        directory = scratch / version
        directory.mkdir(parents=True)
        for binding in bindings:
            (directory / f"{binding}.json").write_text(f"{version}-{binding}\n", encoding="utf-8")
    output = tmp_path / "artifact"
    RUNNER.publish(scratch, output)
    assert sorted(path.relative_to(output).as_posix() for path in output.rglob("*.json")) == [
        "v1/node.json",
        "v1/python.json",
        "v1/rust.json",
        "v2/c.json",
        "v2/node.json",
        "v2/python.json",
        "v2/rust.json",
    ]
    with pytest.raises(RUNNER.RunnerError, match="new directory"):
        RUNNER.publish(scratch, output)


def test_live_ledger_names_all_real_report_producers() -> None:
    source = (CI / "run_sdk_v1_v2_artifact.py").read_text(encoding="utf-8")
    for marker in (
        "test_generated_projection_round_trips_live_models",
        "generated package round-trips exact models on TypeDB",
        "generated_rust_projection_round_trips_exact_live_models",
        "live_c17_generated_person_and_membership_crud_round_trips_exact_3_12_3",
    ):
        assert source.count(marker) == 1
