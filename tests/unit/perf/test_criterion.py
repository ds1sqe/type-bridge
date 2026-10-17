"""Criterion output to ``perf-result/v1`` micro metrics.

``fixtures/criterion/core/validate_type_name_simple/new`` is real Criterion
0.5 output, captured from one short run of
``cargo bench -p type-bridge-core-lib --bench validation``.
"""

from __future__ import annotations

import json
import shutil
from pathlib import Path

import pytest

from perf.harness.criterion import CriterionError, bench_id, convert
from perf.harness.schema import metric_id

FIXTURES = Path(__file__).parent / "fixtures"
CAPTURED = FIXTURES / "criterion"
NEW = CAPTURED / "core/validate_type_name_simple/new"


def _copy(tmp_path: Path) -> Path:
    root = tmp_path / "criterion"
    shutil.copytree(CAPTURED, root)
    return root


def _edit(path: Path, **changes: object) -> None:
    data = json.loads(path.read_text(encoding="utf-8"))
    data.update(changes)
    path.write_text(json.dumps(data), encoding="utf-8")


def test_captured_output_converts() -> None:
    estimates = json.loads((NEW / "estimates.json").read_text(encoding="utf-8"))
    interval = estimates["mean"]["confidence_interval"]

    [metric] = convert(CAPTURED)

    assert metric.id == "micro.core.validate_type_name.simple"
    assert metric.family == "micro"
    assert metric.unit == "ns"
    assert metric.value == estimates["mean"]["point_estimate"]
    assert metric.ci95 == [interval["lower_bound"], interval["upper_bound"]]
    assert metric.samples is None
    assert metric.p50 == estimates["median"]["point_estimate"]
    assert metric.n == 100


def test_nested_group_joins_with_underscore(tmp_path: Path) -> None:
    root = _copy(tmp_path)
    (root / "core").rename(root / "orm")
    _edit(
        root / "orm/validate_type_name_simple/new/benchmark.json",
        full_id="serde/attribute_value/string_roundtrip",
    )

    [metric] = convert(root)

    assert metric.id == "micro.orm.serde_attribute_value.string_roundtrip"


def test_only_new_results_are_read(tmp_path: Path) -> None:
    root = _copy(tmp_path)
    bench = root / "core/validate_type_name_simple"
    shutil.copytree(bench / "new", bench / "base")
    _edit(bench / "base/benchmark.json", full_id="stale/baseline")

    assert [metric.id for metric in convert(root)] == ["micro.core.validate_type_name.simple"]


def test_sample_count_is_optional(tmp_path: Path) -> None:
    root = _copy(tmp_path)
    (root / "core/validate_type_name_simple/new/sample.json").unlink()

    [metric] = convert(root)

    assert metric.n is None


def test_bench_id_normalizes_segments() -> None:
    assert bench_id("core", "Compile/Simple-Match") == "micro.core.compile.simple_match"
    assert metric_id("micro", "orm", "query codegen") == "micro.orm.query_codegen"


@pytest.mark.parametrize("full_id", ["simple", "validate//"])
def test_bench_id_needs_group_and_bench(full_id: str) -> None:
    with pytest.raises(CriterionError, match=repr(full_id)):
        bench_id("core", full_id)


def test_malformed_estimates_are_rejected(tmp_path: Path) -> None:
    root = _copy(tmp_path)
    _edit(root / "core/validate_type_name_simple/new/estimates.json", mean={})

    with pytest.raises(CriterionError, match="estimates.json: malformed estimates"):
        convert(root)


def test_invalid_json_is_rejected(tmp_path: Path) -> None:
    root = _copy(tmp_path)
    (root / "core/validate_type_name_simple/new/benchmark.json").write_text("{", encoding="utf-8")

    with pytest.raises(CriterionError, match="benchmark.json: not valid JSON"):
        convert(root)


def test_empty_or_missing_root_is_rejected(tmp_path: Path) -> None:
    with pytest.raises(CriterionError, match="not a directory"):
        convert(tmp_path / "absent")
    (tmp_path / "core").mkdir()
    with pytest.raises(CriterionError, match="no Criterion results"):
        convert(tmp_path)
