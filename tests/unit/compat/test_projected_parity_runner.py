"""Fail-closed coverage for the provider-free four-binding Projected fan-in."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
RUNNER_PATH = ROOT / "scripts/ci/run_projected_parity.py"
CHECK = ROOT / "scripts/check.sh"
CI = ROOT / ".github/workflows/ci.yml"


def load_runner():
    spec = importlib.util.spec_from_file_location("projected_parity_runner", RUNNER_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_command_plan_runs_four_real_producers_then_one_comparator(tmp_path: Path) -> None:
    runner = load_runner()
    layout = runner.Layout.under(tmp_path.resolve())
    plan = runner.command_plan(layout)

    assert [command.label for command in plan] == [
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
        "produce the Rust report",
        "build the C shared library",
        "produce the C report",
        "compare all four reports",
    ]
    reports = tuple(str(path) for path in layout.report_paths())
    assert len(set(reports)) == 4
    assert all(Path(path).is_absolute() and not Path(path).exists() for path in reports)
    assert plan[3].environment == {
        "TYPE_BRIDGE_PROJECTED_PARITY_REPORT": reports[0],
        "TYPE_BRIDGE_PROJECTED_PYTHON_PACKAGE_ROOT": str(layout.python),
        "TYPE_BRIDGE_PROJECTED_REPOSITORY_ROOT": str(ROOT),
    }
    assert plan[9].environment == {
        "TYPE_BRIDGE_PROJECTED_PARITY_REPORT": reports[1],
        "TYPE_BRIDGE_PROJECTED_REPOSITORY_ROOT": str(ROOT),
    }
    assert plan[10].environment == {"TYPE_BRIDGE_PROJECTED_RUST_REPORT": reports[2]}
    assert plan[12].environment == {
        "TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER": "1",
        "TYPE_BRIDGE_C_SHARED_LIBRARY": str(runner._c_shared_library()),
        "TYPE_BRIDGE_PROJECTED_PARITY_REPORT_C": reports[3],
    }
    assert plan[-1].arguments[1] == str(runner.COMPARATOR)
    assert plan[-1].arguments[2:] == reports
    assert "generated_rust_projected_parity_producer" in plan[10].arguments
    assert (
        "generated_c17_projected_parity_producer_is_exact_and_comparator_accepted"
        in plan[12].arguments
    )


def test_runner_uses_one_stage_and_exact_foreign_mutation_without_expected_evidence() -> None:
    source = RUNNER_PATH.read_text(encoding="utf-8")

    assert source.count("tempfile.TemporaryDirectory(") == 1
    assert "member: { card: { min: 0, max: 2 }, doc: membership player }" in source
    assert "member: { card: { min: 0, max: 3 }, doc: membership player }" in source
    assert "expected_observations" not in source
    assert "expected_report" not in source
    assert "TYPEDB_ADDRESS" not in source
    assert "compare_projected_parity.py" in source


def test_runner_fails_closed_when_a_required_tool_is_missing(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runner = load_runner()
    monkeypatch.setattr(
        runner.shutil,
        "which",
        lambda executable: None if executable == "clang" else f"/tools/{executable}",
    )

    with pytest.raises(runner.RunnerError, match="unavailable: clang"):
        runner._require_tools()


def test_runner_surfaces_subprocess_failure(monkeypatch: pytest.MonkeyPatch) -> None:
    runner = load_runner()
    command = runner.CommandSpec("hostile producer", ("false",), ROOT)
    monkeypatch.setattr(
        runner.subprocess,
        "run",
        lambda *args, **kwargs: subprocess.CompletedProcess(
            args[0], 38, stdout="producer output", stderr="producer failure"
        ),
    )

    with pytest.raises(runner.RunnerError, match="hostile producer failed with exit 38"):
        runner._run(command)


def test_check_and_ci_persist_the_same_job_fan_in() -> None:
    check = CHECK.read_text(encoding="utf-8")
    ci = CI.read_text(encoding="utf-8")

    assert "run_projected_parity()" in check
    assert "projected-parity) run_projected_parity" in check
    assert "run_projected_parity" in check.split("all)", maxsplit=1)[1]
    assert "projected-parity:" in ci
    assert "runs-on: ubuntu-latest" in ci.split("  projected-parity:", maxsplit=1)[1]
    for source in (check, ci):
        assert "scripts/ci/run_projected_parity.py" in source
