"""Advisory comparisons use validated stored-run fixtures."""

import copy
import json
from pathlib import Path

import pytest
import yaml

from perf.harness import compare

FIXTURE = Path(__file__).parent / "fixtures/micro.json"


def document(metrics: list[dict]) -> dict:
    result = json.loads(FIXTURE.read_text())
    result["metrics"] = metrics
    return result


def metric(name: str, value: float, unit: str = "ns") -> dict:
    return {"id": f"micro.test.{name}", "family": "micro", "unit": unit, "value": value}


@pytest.mark.parametrize("unit,threshold", compare.THRESHOLDS.items())
def test_threshold_and_direction(unit: str, threshold: float) -> None:
    base = document([metric("x", 100, unit)])
    assert "unchanged |" in compare.render(base, document([metric("x", 100 + threshold, unit)]))
    report = compare.render(base, document([metric("x", 101 + threshold, unit)]))
    assert ("improvement |" if unit == "ops_per_s" else "regression |") in report
    report = compare.render(base, document([metric("x", 99 - threshold, unit)]))
    assert ("regression |" if unit == "ops_per_s" else "improvement |") in report


def test_regressions_first_missing_ids_and_changed_units() -> None:
    base = document([metric("a", 100), metric("z", 100), metric("removed", 1), metric("units", 1)])
    head = document(
        [metric("a", 50), metric("z", 150), metric("added", 1), metric("units", 1, "bytes")]
    )
    report = compare.render(base, head)
    assert report.index("| micro.test.z") < report.index("| micro.test.a")
    assert "## Only in head" in report and "micro.test.added" in report
    assert "## Only in base" in report and "micro.test.removed" in report
    assert "unit changed" in report


def test_total_row_cap_including_one_sided_metrics() -> None:
    base = document([metric(f"x{i}", 100) for i in range(205)])
    head = copy.deepcopy(base)
    head["metrics"] += [metric("added", 1)]
    report = compare.render(base, head)
    assert report.count("| micro.test.") == 200
    assert "+6 more" in report


def test_empty_base_and_zero_baseline() -> None:
    assert "Only in head" in compare.render(document([]), document([metric("new", 1)]))
    assert "+∞" in compare.render(document([metric("x", 0)]), document([metric("x", 1)]))
    assert "unchanged |" in compare.render(document([metric("x", 0)]), document([metric("x", 0)]))


def test_cli_is_advisory_even_for_regressions_or_missing_files(tmp_path: Path) -> None:
    base, head, out = tmp_path / "base.json", tmp_path / "head.json", tmp_path / "summary.md"
    base.write_text(json.dumps(document([metric("x", 100)])))
    head.write_text(json.dumps(document([metric("x", 200)])))
    assert compare.main([str(base), str(head), "--out", str(out)]) == 0
    assert "regression" in out.read_text()
    assert compare.main([str(tmp_path / "missing"), str(head), "--out", str(out)]) == 0
    assert "unavailable" in out.read_text()


def test_pr_job_is_read_only_and_measures_both_revisions() -> None:
    root = Path(__file__).resolve().parents[3]
    jobs = yaml.safe_load((root / ".github/workflows/perf.yml").read_text())["jobs"]
    job = jobs["compare"]
    assert job["if"] == "github.event_name == 'pull_request'"
    assert job["permissions"] == {"contents": "read"}
    checkouts = [
        step["with"]
        for step in job["steps"]
        if step.get("uses", "").startswith("actions/checkout@")
    ]
    assert [step["path"] for step in checkouts] == ["base", "head"]
    assert all(step["persist-credentials"] is False for step in checkouts)
    commands = "\n".join(step.get("run", "") for step in job["steps"])
    assert "for side in base head" in commands and "GITHUB_STEP_SUMMARY" in commands
    assert "perf.harness.store" not in commands and "git push" not in commands
