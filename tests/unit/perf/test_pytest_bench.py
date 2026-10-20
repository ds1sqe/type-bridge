"""pytest-benchmark JSON -> ``binding.python.*`` metrics."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from perf.harness.pytest_bench import PytestBenchError, bench_id, convert

FIXTURE = Path(__file__).parent / "fixtures" / "pytest-benchmark" / "benchmarks.json"


def test_converts_every_benchmark_in_ns() -> None:
    bench = convert(FIXTURE)

    assert bench.python == "3.13.11"
    [compile_metric, validate_metric] = bench.metrics
    assert compile_metric.id == "binding.python.compilation.compile_simple_match"
    assert compile_metric.family == "binding"
    assert compile_metric.unit == "ns"
    assert compile_metric.value == pytest.approx(3214.3594791599805)
    assert compile_metric.p50 == pytest.approx(3172.08468914032)
    assert compile_metric.n == 6072
    assert validate_metric.id == "binding.python.validation.validate_single_name"


@pytest.mark.parametrize(
    ("fullname", "expected"),
    [
        (
            "benchmarks/test_compilation_bench.py::test_compile_e2e_batch",
            "binding.python.compilation.compile_e2e_batch",
        ),
        (
            "test_validation_bench.py::test_validate_batch_5000",
            "binding.python.validation.validate_batch_5000",
        ),
    ],
)
def test_bench_id(fullname: str, expected: str) -> None:
    assert bench_id(fullname) == expected


@pytest.mark.parametrize(
    "fullname",
    [
        "benchmarks/conftest.py::test_compile",
        "benchmarks/test_compilation_bench.py::compile_simple",
        "benchmarks/test_compilation_bench.py",
    ],
)
def test_bench_id_rejects_other_layouts(fullname: str) -> None:
    with pytest.raises(PytestBenchError, match="test_<group>_bench.py"):
        bench_id(fullname)


def _write(tmp_path: Path, data: object) -> Path:
    path = tmp_path / "bench.json"
    path.write_text(json.dumps(data), encoding="utf-8")
    return path


def test_no_benchmarks_is_an_error(tmp_path: Path) -> None:
    path = _write(tmp_path, {"machine_info": {"python_version": "3.13.5"}, "benchmarks": []})

    with pytest.raises(PytestBenchError, match="no benchmarks"):
        convert(path)


def test_duplicate_ids_are_an_error(tmp_path: Path) -> None:
    data = json.loads(FIXTURE.read_text(encoding="utf-8"))
    data["benchmarks"].append(data["benchmarks"][0])

    with pytest.raises(PytestBenchError, match="duplicate metric ids"):
        convert(_write(tmp_path, data))


def test_missing_stats_are_an_error(tmp_path: Path) -> None:
    data = json.loads(FIXTURE.read_text(encoding="utf-8"))
    del data["benchmarks"][0]["stats"]["median"]

    with pytest.raises(PytestBenchError, match="malformed benchmark entry"):
        convert(_write(tmp_path, data))


def test_other_json_is_an_error(tmp_path: Path) -> None:
    with pytest.raises(PytestBenchError, match="not pytest-benchmark JSON"):
        convert(_write(tmp_path, {"benchmarks": []}))
    with pytest.raises(PytestBenchError, match="cannot read"):
        convert(tmp_path / "absent.json")
