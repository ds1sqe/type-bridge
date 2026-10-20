"""Captured pytest/Node timing conversion and successful-suite publication."""

import json
import subprocess
from pathlib import Path

import pytest

from perf.harness import suite
from perf.harness.validate import check_file

FIXTURE = Path(__file__).parent / "fixtures/suite"


def test_captured_pytest_calls_exclude_setup_and_teardown() -> None:
    durations = suite.pytest_durations((FIXTURE / "pytest-durations.txt").read_text())
    assert len(durations) == 8
    assert all(name.startswith("tests/unit/perf/test_size.py::") for name in durations)


def test_captured_node_leaf_tests_are_qualified_and_exclude_suites() -> None:
    text = (FIXTURE / "node-tap.txt").read_text()
    durations = suite.node_durations(text)
    assert set(durations) == {"alpha::same name", "alpha::other test", "beta::same name"}
    # Also accept the Node 20 TAP layout without the newer type field.
    assert (
        suite.node_durations("\n".join(line for line in text.splitlines() if "type:" not in line))
        == durations
    )
    assert durations["alpha::same name"] < 0.01


def test_slowest_ten_and_punctuation_collisions() -> None:
    text = "\n".join(f"{i}.00s call tests/t.py::test-{i}" for i in range(15))
    result = suite.convert("python", 123, text)
    assert len(result) == 11
    assert result[0].id == "suite.python.wall"
    assert result[1].value == 14
    assert result[-1].value == 5
    colliding = suite.convert("python", 2, "1.00s call test-a\n1.00s call test_a")
    assert colliding[1].id != colliding[2].id


def test_stable_rust_reports_wall_only() -> None:
    [metric] = suite.convert("rust", 5.5, "test x ... ok")
    assert (metric.id, metric.value) == ("suite.rust.wall", 5.5)


@pytest.mark.parametrize("name", ["python", "node"])
def test_missing_timing_observations_fail(name: str) -> None:
    with pytest.raises(ValueError, match="no per-test durations"):
        suite.convert(name, 1, "tests passed, unsupported timing layout")


def test_cli_runs_existing_suite_and_writes_valid_result(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    def command(argv: list[str], log: Path, **kwargs: object) -> float:
        assert "--durations=0" in argv
        log.write_bytes((FIXTURE / "pytest-durations.txt").read_bytes())
        return 12.5

    monkeypatch.setattr(suite, "timed", command)
    out = tmp_path / "suite.json"
    assert suite.main(["--suite", "python", "--out", str(out)]) == 0
    assert not check_file(out).errors
    assert json.loads(out.read_text())["run"]["toolchains"]["python"]


def test_failed_suite_is_not_published(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    def fail(*args: object, **kwargs: object) -> float:
        raise subprocess.CalledProcessError(1, "pytest")

    monkeypatch.setattr(suite, "timed", fail)
    out = tmp_path / "suite.json"
    assert suite.main(["--suite", "python", "--out", str(out)]) == 1
    assert not out.exists()


def test_workflow_producers_are_read_only_and_store_waits_for_all() -> None:
    import yaml

    workflow = yaml.safe_load((suite.REPO_ROOT / ".github/workflows/perf.yml").read_text())
    assert workflow["permissions"] == {"contents": "read"}
    jobs = workflow["jobs"]
    assert set(jobs["store"]["needs"]) == {
        "micro", "build", "size", "suite", "pipeline", "binding", "versions", "released",
    }  # fmt: skip
    assert jobs["suite"]["if"] == "github.event_name != 'pull_request'"
    for family in ["micro", "build", "size", "suite", "pipeline", "binding"]:
        assert jobs[family]["needs"] == "metadata"
        assert jobs[family]["env"]["PERF_STARTED_AT"] == "${{ needs.metadata.outputs.started_at }}"
        assert jobs[family].get("permissions", workflow["permissions"]) == {"contents": "read"}
        uploads = [
            step
            for step in jobs[family]["steps"]
            if step.get("uses", "").startswith("actions/upload-artifact@")
            and step.get("if") != "failure()"
        ]
        assert [step["with"]["name"] for step in uploads] == [f"perf-{family}"]
    # The suite job keeps its harness logs when it fails.
    logs = [step for step in jobs["suite"]["steps"] if step.get("if") == "failure()"]
    assert [step["with"]["name"] for step in logs] == ["perf-suite-logs"]
    assert "clippy" in next(
        step["with"]["components"]
        for step in jobs["suite"]["steps"]
        if step.get("uses", "").startswith("dtolnay/rust-toolchain@")
    )
    released = jobs["released"]
    assert released["needs"] == ["metadata", "versions"]
    assert released["env"]["PERF_STARTED_AT"] == "${{ needs.metadata.outputs.started_at }}"
    assert released.get("permissions", workflow["permissions"]) == {"contents": "read"}
