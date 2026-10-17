"""The ``perf-result/v1`` validator, its model and the normative JSON Schema."""

from __future__ import annotations

import copy
import dataclasses
import json
import subprocess
import sys
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest

from perf.harness.schema import (
    FAMILIES,
    METRIC_ID,
    SOURCES,
    TRIGGERS,
    UNITS,
    Metric,
    PerfResult,
    Run,
    Runner,
    Toolchains,
)
from perf.harness.validate import check, check_file, main, validate

REPO_ROOT = Path(__file__).resolve().parents[3]
FIXTURES = Path(__file__).parent / "fixtures"
FIXTURE_FILES = sorted(FIXTURES.glob("*.json"))
SCHEMA = REPO_ROOT / "perf/schema/perf-result-v1.json"

type Document = dict[str, Any]


def _pipeline() -> Document:
    return json.loads((FIXTURES / "pipeline.json").read_text(encoding="utf-8"))


def _locate(document: Document, path: str) -> tuple[Any, Any]:
    """Container and key for a dotted ``path``; digit segments index lists."""
    keys: list[Any] = [int(part) if part.isdigit() else part for part in path.split(".")]
    target: Any = document
    for key in keys[:-1]:
        target = target[key]
    return target, keys[-1]


def _set(path: str, value: object) -> Callable[[Document], None]:
    def mutate(document: Document) -> None:
        target, key = _locate(document, path)
        target[key] = value

    return mutate


def _drop(path: str) -> Callable[[Document], None]:
    def mutate(document: Document) -> None:
        target, key = _locate(document, path)
        del target[key]

    return mutate


def _set_stage(span: str, value: object) -> Callable[[Document], None]:
    """Span names contain dots, so they cannot go through ``_set``."""

    def mutate(document: Document) -> None:
        document["metrics"][0]["stages"][span] = value

    return mutate


@pytest.mark.parametrize("path", FIXTURE_FILES, ids=lambda path: path.name)
def test_fixture_is_valid(path: Path) -> None:
    findings = check_file(path)
    assert findings.errors == []
    assert findings.warnings == []
    assert validate(path) == []


@pytest.mark.parametrize("path", FIXTURE_FILES, ids=lambda path: path.name)
def test_model_round_trips_a_fixture(path: Path) -> None:
    document = json.loads(path.read_text(encoding="utf-8"))
    result = PerfResult.from_dict(document)
    assert result.to_dict() == document
    assert check(result.to_dict()).errors == []


VIOLATIONS: list[tuple[str, Callable[[Document], None], str]] = [
    ("format", _set("format", "perf-result/v2"), "format:"),
    ("missing run", _drop("run"), "run: required field is missing"),
    ("unknown top field", _set("extra", 1), "extra: unknown field"),
    ("commit", _set("run.commit", "c7ee52d3"), "run.commit:"),
    ("branch", _set("run.branch", ""), "run.branch:"),
    ("version", _set("run.version", 2), "run.version:"),
    ("trigger", _set("run.trigger", "nightly"), "run.trigger:"),
    ("missing runner", _drop("run.runner"), "run.runner: required field is missing"),
    ("null runner", _set("run.runner", None), "run.runner: expected an object"),
    ("runner image", _drop("run.runner.image"), "run.runner.image:"),
    ("runner cpu", _set("run.runner.cpu", None), "run.runner.cpu:"),
    ("runner cores", _set("run.runner.cores", 0), "run.runner.cores:"),
    ("runner cores bool", _set("run.runner.cores", True), "run.runner.cores:"),
    ("toolchain", _drop("run.toolchains.cc"), "run.toolchains.cc:"),
    ("toolchain type", _set("run.toolchains.rust", 1.88), "run.toolchains.rust:"),
    ("typedb", _set("run.typedb", ""), "run.typedb:"),
    ("started_at", _set("run.started_at", "yesterday"), "run.started_at:"),
    ("started_at offset", _set("run.started_at", "2026-10-16T02:00:11"), "run.started_at:"),
    ("unknown run field", _set("run.host", "x"), "run.host: unknown field"),
    ("source outside release-matrix", _set("run.source", "registry"), "run.source:"),
    ("release-matrix without source", _set("run.trigger", "release-matrix"), "run.source:"),
    (
        "source value",
        lambda document: [
            _set("run.trigger", "release-matrix")(document),
            _set("run.source", "pypi")(document),
        ][0],
        "run.source:",
    ),
    ("metrics empty", _set("metrics", []), "metrics:"),
    ("metric not object", _set("metrics.0", "x"), "metrics[0]:"),
    ("id single segment", _set("metrics.0.id", "insert"), "metrics[0].id:"),
    ("id uppercase", _set("metrics.0.id", "Pipeline.insert"), "metrics[0].id:"),
    ("family", _set("metrics.1.family", "macro"), "metrics[1].family:"),
    ("unit", _set("metrics.2.unit", "ms"), "metrics[2].unit:"),
    ("unit ops/s", _set("metrics.1.unit", "ops/s"), "metrics[1].unit:"),
    ("missing value", _drop("metrics.2.value"), "metrics[2].value: required field is missing"),
    ("value negative", _set("metrics.2.value", -1), "metrics[2].value:"),
    ("value bool", _set("metrics.2.value", True), "metrics[2].value:"),
    ("value nan", _set("metrics.2.value", float("nan")), "metrics[2].value:"),
    ("n", _set("metrics.0.n", 0), "metrics[0].n:"),
    ("samples", _set("metrics.3.samples", [1, "2"]), "metrics[3].samples:"),
    ("ci95 length", _set("metrics.0.ci95", [1.0]), "metrics[0].ci95:"),
    ("ci95 type", _set("metrics.0.ci95", [1.0, "2"]), "metrics[0].ci95:"),
    ("ci95 infinite", _set("metrics.0.ci95", [1.0, float("inf")]), "metrics[0].ci95:"),
    ("ci95 reversed", _set("metrics.0.ci95", [2.0, 1.0]), "metrics[0].ci95:"),
    ("p90", _set("metrics.0.p90", "fast"), "metrics[0].p90:"),
    ("unknown metric field", _set("metrics.0.mean", 1), "metrics[0].mean: unknown field"),
    ("stages type", _set("metrics.0.stages", [1]), "metrics[0].stages:"),
    ("stages empty", _set("metrics.0.stages", {}), "metrics[0].stages:"),
    ("stage name", _set("metrics.0.stages.Compile", 1), "metrics[0].stages:"),
    ("stage value", _set_stage("tb.decode", -5), "metrics[0].stages.tb.decode:"),
    ("stages need ns", _set("metrics.1.stages", {"tb.decode": 1}), "metrics[1].stages:"),
]


@pytest.mark.parametrize(
    ("mutate", "field"),
    [(mutate, field) for _, mutate, field in VIOLATIONS],
    ids=[name for name, _, _ in VIOLATIONS],
)
def test_violation_is_rejected_naming_the_field(
    mutate: Callable[[Document], None], field: str
) -> None:
    document = _pipeline()
    mutate(document)
    errors = check(document).errors
    assert errors, "violation was accepted"
    assert any(error.startswith(field) for error in errors), errors


def test_duplicate_ids_are_rejected() -> None:
    document = _pipeline()
    document["metrics"].append(copy.deepcopy(document["metrics"][2]))
    errors = check(document).errors
    assert errors == [
        "metrics[4].id: duplicate id 'size.python.wheel', first at metrics[2]",
    ]


def test_stage_sum_above_value_is_a_warning_not_an_error() -> None:
    document = _pipeline()
    document["metrics"][0]["stages"]["tb.commit"] = 500000
    findings = check(document)
    assert findings.errors == []
    assert len(findings.warnings) == 1
    assert findings.warnings[0].startswith("metrics[0].stages:")
    assert "pipeline.python.insert" in findings.warnings[0]


def test_document_must_be_an_object() -> None:
    assert check([]).errors == ["<document>: expected an object, got []"]


def test_unreadable_and_malformed_files_are_errors(tmp_path: Path) -> None:
    broken = tmp_path / "broken.json"
    broken.write_text("{", encoding="utf-8")
    assert validate(broken)[0].startswith("<file>: not valid JSON")
    assert validate(tmp_path / "absent.json")[0].startswith("<file>: cannot read")


def test_cli_exit_codes(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    assert main([str(path) for path in FIXTURE_FILES]) == 0

    corrupted = tmp_path / "corrupted.json"
    document = _pipeline()
    document["metrics"][1]["unit"] = "ms"
    corrupted.write_text(json.dumps(document), encoding="utf-8")
    capsys.readouterr()
    assert main([str(FIXTURE_FILES[0]), str(corrupted)]) == 1
    captured = capsys.readouterr()
    assert f"{corrupted}: error: metrics[1].unit:" in captured.err
    assert "1 of 2 file(s) valid" in captured.out


def test_cli_module_entry_point(tmp_path: Path) -> None:
    def run(*files: Path) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, "-m", "perf.harness.validate", *map(str, files)],
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            check=False,
        )

    assert run(*FIXTURE_FILES).returncode == 0
    corrupted = tmp_path / "corrupted.json"
    corrupted.write_text(
        (FIXTURES / "micro.json").read_text(encoding="utf-8").replace('"micro"', '"macro"'),
        encoding="utf-8",
    )
    failed = run(corrupted)
    assert failed.returncode == 1
    assert "metrics[0].family:" in failed.stderr


def test_python_model_matches_the_json_schema() -> None:
    schema = json.loads(SCHEMA.read_text(encoding="utf-8"))
    defs = schema["$defs"]
    metric = defs["metric"]["properties"]
    assert schema["properties"]["format"]["const"] == "perf-result/v1"
    assert tuple(metric["family"]["enum"]) == FAMILIES
    assert tuple(metric["unit"]["enum"]) == UNITS
    assert tuple(defs["run"]["properties"]["trigger"]["enum"]) == TRIGGERS
    assert defs["dottedName"]["pattern"] == METRIC_ID.pattern

    def names(model: type) -> set[str]:
        return {field.name for field in dataclasses.fields(model)}

    run = defs["run"]
    assert set(run["properties"]) == names(Run)
    assert set(run["required"]) == {
        f.name for f in dataclasses.fields(Run) if f.default is dataclasses.MISSING
    }
    assert tuple(run["properties"]["source"]["enum"]) == SOURCES
    for model, node in ((Runner, "runner"), (Toolchains, "toolchains")):
        sub = run["properties"][node]
        assert set(sub["required"]) == set(sub["properties"]) == names(model)
    required = {f.name for f in dataclasses.fields(Metric) if f.default is dataclasses.MISSING}
    assert set(defs["metric"]["required"]) == required
    assert set(metric) == names(Metric)
