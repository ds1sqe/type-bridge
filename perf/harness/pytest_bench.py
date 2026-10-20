"""pytest-benchmark results as ``perf-result/v1`` Python binding metrics.

The input is the file ``pytest benchmarks --benchmark-only --benchmark-json
FILE`` writes. A bench ``test_<name>`` in ``test_<group>_bench.py`` becomes
``binding.python.<group>.<name>``, for example
``binding.python.compilation.compile_simple_match``.

Each metric has unit ``ns``, the mean round time as ``value``, the median as
``p50`` and the round count as ``n``, matching the Criterion converter
(``perf.harness.criterion``).
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from perf.harness.schema import Metric, metric_id

_MODULE = re.compile(r"^test_(?P<group>\w+)_bench\.py$")
_NS_PER_S = 1e9


class PytestBenchError(ValueError):
    """pytest-benchmark output is missing, malformed or cannot be named."""


@dataclass(frozen=True)
class PytestBench:
    """The converted metrics and the interpreter version that ran them."""

    python: str
    metrics: list[Metric]


def convert(path: Path) -> PytestBench:
    """Metrics for every benchmark in the pytest-benchmark JSON at ``path``."""
    data = _load(path)
    try:
        python = str(data["machine_info"]["python_version"])
        benchmarks = data["benchmarks"]
    except (KeyError, TypeError) as error:
        raise PytestBenchError(f"{path}: not pytest-benchmark JSON: missing {error}") from error
    if not isinstance(benchmarks, list) or not benchmarks:
        raise PytestBenchError(f"{path}: no benchmarks")
    metrics = [_metric(path, benchmark) for benchmark in benchmarks]
    ids = [metric.id for metric in metrics]
    if duplicates := sorted({metric_id for metric_id in ids if ids.count(metric_id) > 1}):
        raise PytestBenchError(f"{path}: duplicate metric ids: {', '.join(duplicates)}")
    return PytestBench(python=python, metrics=sorted(metrics, key=lambda metric: metric.id))


def bench_id(fullname: str) -> str:
    """The metric id for a pytest-benchmark ``fullname`` (``<file>::test_<name>``)."""
    module, _, test = fullname.partition("::")
    match = _MODULE.fullmatch(Path(module).name)
    if match is None or not test.startswith("test_"):
        raise PytestBenchError(
            f"benchmark {fullname!r} is not 'test_<group>_bench.py::test_<name>'"
        )
    try:
        return metric_id("binding", "python", match["group"], test.removeprefix("test_"))
    except ValueError as error:
        raise PytestBenchError(f"benchmark {fullname!r}: {error}") from error


def _metric(path: Path, benchmark: Any) -> Metric:
    try:
        fullname = benchmark["fullname"]
        stats = benchmark["stats"]
        mean = float(stats["mean"]) * _NS_PER_S
        median = float(stats["median"]) * _NS_PER_S
        rounds = int(stats["rounds"])
    except (KeyError, TypeError, ValueError) as error:
        raise PytestBenchError(f"{path}: malformed benchmark entry: {error}") from error
    if not isinstance(fullname, str):
        raise PytestBenchError(f"{path}: benchmark fullname is not a string")
    return Metric(
        id=bench_id(fullname),
        family="binding",
        unit="ns",
        value=mean,
        n=rounds,
        p50=median,
    )


def _load(path: Path) -> dict[str, Any]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except OSError as error:
        raise PytestBenchError(f"{path}: cannot read: {error.strerror or error}") from error
    except json.JSONDecodeError as error:
        raise PytestBenchError(f"{path}: not valid JSON: {error}") from error
    if not isinstance(data, dict):
        raise PytestBenchError(f"{path}: expected a JSON object")
    return data
