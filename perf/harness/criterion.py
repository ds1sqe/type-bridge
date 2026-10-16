"""Criterion wall-time results as ``perf-result/v1`` micro metrics.

``root`` holds one directory per crate, each the ``CRITERION_HOME`` of that
crate's ``cargo bench`` run::

    <root>/<crate>/<bench dir>/new/benchmark.json   # the bench id
    <root>/<crate>/<bench dir>/new/estimates.json   # mean, median, CIs
    <root>/<crate>/<bench dir>/new/sample.json      # optional: sample count

A Criterion id ``<group>/<bench>`` becomes ``micro.<crate>.<group>.<bench>``.
A group that itself contains ``/`` (``serde/attribute_value``) joins its parts
with ``_``, so every id has exactly four parts and matches the id the
instruction-count bench reports (see ``perf.harness.iai``).

Each metric has unit ``ns``, the mean point estimate as ``value``, the median
as ``p50``, the sample count as ``n``, and the mean's confidence interval
(Criterion's default is 95%) as ``samples = [lower, upper]``.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from perf.harness.schema import Metric, metric_id


class CriterionError(ValueError):
    """Criterion output is missing, malformed or cannot be named."""


def convert(root: Path) -> list[Metric]:
    """Metrics for every benchmark under ``root``, sorted by id."""
    if not root.is_dir():
        raise CriterionError(f"{root}: not a directory")
    metrics: list[Metric] = []
    for crate in sorted(path for path in root.iterdir() if path.is_dir()):
        for benchmark in sorted(crate.rglob("new/benchmark.json")):
            metrics.append(_metric(crate.name, benchmark.parent))
    if not metrics:
        raise CriterionError(f"{root}: no Criterion results (<crate>/**/new/benchmark.json)")
    return sorted(metrics, key=lambda metric: metric.id)


def bench_id(crate: str, full_id: str) -> str:
    """The metric id for Criterion's ``full_id`` in ``crate``."""
    *group, bench = full_id.split("/")
    if not group:
        raise CriterionError(f"Criterion id {full_id!r} has no '<group>/' prefix")
    try:
        return metric_id("micro", crate, "_".join(group), bench)
    except ValueError as error:
        raise CriterionError(f"Criterion id {full_id!r}: {error}") from error


def _metric(crate: str, new: Path) -> Metric:
    benchmark = _load(new / "benchmark.json")
    estimates = _load(new / "estimates.json")
    full_id = benchmark.get("full_id")
    if not isinstance(full_id, str):
        raise CriterionError(f"{new / 'benchmark.json'}: no full_id")
    try:
        mean = estimates["mean"]
        interval = mean["confidence_interval"]
        value = float(mean["point_estimate"])
        lower = float(interval["lower_bound"])
        upper = float(interval["upper_bound"])
        median = float(estimates["median"]["point_estimate"])
    except (KeyError, TypeError, ValueError) as error:
        raise CriterionError(f"{new / 'estimates.json'}: malformed estimates: {error}") from error

    sample = new / "sample.json"
    n = None
    if sample.is_file():
        n = len(_load(sample).get("iters") or []) or None
    return Metric(
        id=bench_id(crate, full_id),
        family="micro",
        unit="ns",
        value=value,
        n=n,
        samples=[lower, upper],
        p50=median,
    )


def _load(path: Path) -> dict[str, Any]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except OSError as error:
        raise CriterionError(f"{path}: cannot read: {error.strerror or error}") from error
    except json.JSONDecodeError as error:
        raise CriterionError(f"{path}: not valid JSON: {error}") from error
    if not isinstance(data, dict):
        raise CriterionError(f"{path}: expected a JSON object")
    return data
