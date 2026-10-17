"""Write one ``perf-result/v1`` file from a live pipeline report.

Usage::

    python -m perf.harness.pipeline --report FILE --typedb VERSION --out FILE

The report is the JSON that ``type-bridge-perf-pipelines`` (the
``type-bridge-perf`` crate) writes: per pipeline, the wall time of every
measured iteration and the exclusive time of each stage span inside it. Each
pipeline becomes two metrics:

- ``pipeline.<binding>.<pipeline>`` in ns: ``value`` and ``p50`` are the
  median iteration, with ``p90``, ``p99``, ``n``, the raw ``samples``, and
  ``stages`` mapping each span name to its median;
- ``pipeline.<binding>.<pipeline>.throughput`` in ``ops_per_s``: logical
  operations (rows for a batch) over the summed iteration time.

Time inside no stage span is SDK-side work; it is not a stage, so the stages
sum to at most about the median. For each pipeline the converter prints the
attributed share and the stage-sum gap: how far the stage medians plus the
median unattributed time are from the median iteration. A gap above 5% means
the stage medians do not describe a typical iteration.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import statistics
import sys
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, replace
from pathlib import Path
from typing import TypeGuard

from perf.harness.micro import collect_run
from perf.harness.producer import write_result
from perf.harness.schema import STAGE_NAME, Metric, metric_id

REPORT_FORMAT = "type-bridge-perf-pipelines/v1"
# The acceptance bound on the stage-sum gap.
STAGE_GAP_LIMIT = 0.05


@dataclass(frozen=True)
class Sample:
    total_ns: int
    stages: dict[str, int]


@dataclass(frozen=True)
class Pipeline:
    name: str
    operations: int
    samples: list[Sample]


@dataclass(frozen=True)
class Summary:
    """The figures printed per pipeline."""

    metric: str
    p50: float
    attributed: float
    gap: float


def percentile(values: Sequence[float], fraction: float) -> float:
    """Linear interpolation between closest ranks, as numpy's default."""
    if not values:
        raise ValueError("no values")
    ordered = sorted(values)
    position = (len(ordered) - 1) * fraction
    low = math.floor(position)
    high = math.ceil(position)
    return ordered[low] + (ordered[high] - ordered[low]) * (position - low)


def read_report(path: Path) -> tuple[str, list[Pipeline]]:
    """The binding and pipelines of a report; raises ``ValueError`` when malformed."""
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError(f"{path}: cannot read the pipeline report: {error}") from error
    if not isinstance(data, dict) or data.get("format") != REPORT_FORMAT:
        raise ValueError(f"{path}: expected format {REPORT_FORMAT!r}")
    binding = data.get("binding")
    if not isinstance(binding, str) or not binding:
        raise ValueError(f"{path}: binding must be a non-empty string")
    raw = data.get("pipelines")
    if not isinstance(raw, list) or not raw:
        raise ValueError(f"{path}: pipelines must be a non-empty list")
    return binding, [_pipeline(entry, f"pipelines[{index}]") for index, entry in enumerate(raw)]


def _pipeline(entry: object, where: str) -> Pipeline:
    if not isinstance(entry, dict):
        raise ValueError(f"{where}: must be an object")
    name, operations, samples = entry.get("name"), entry.get("operations"), entry.get("samples")
    if not isinstance(name, str) or not name:
        raise ValueError(f"{where}.name: must be a non-empty string")
    if not _count(operations) or operations < 1:
        raise ValueError(f"{where}.operations: must be a positive integer")
    if not isinstance(samples, list) or not samples:
        raise ValueError(f"{where}.samples: must be a non-empty list")
    return Pipeline(
        name=name,
        operations=operations,
        samples=[
            _sample(sample, f"{where}.samples[{index}]") for index, sample in enumerate(samples)
        ],
    )


def _sample(entry: object, where: str) -> Sample:
    if not isinstance(entry, dict):
        raise ValueError(f"{where}: must be an object")
    total, stages = entry.get("total_ns"), entry.get("stages")
    if not _count(total):
        raise ValueError(f"{where}.total_ns: must be a non-negative integer")
    if not isinstance(stages, dict):
        raise ValueError(f"{where}.stages: must be an object")
    for stage, nanos in stages.items():
        if not STAGE_NAME.fullmatch(stage) or not _count(nanos):
            raise ValueError(f"{where}.stages: {stage!r} needs a dotted name and integer ns")
    return Sample(total_ns=total, stages=dict(stages))


def _count(value: object) -> TypeGuard[int]:
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def convert(binding: str, pipelines: Sequence[Pipeline]) -> tuple[list[Metric], list[Summary]]:
    metrics: list[Metric] = []
    summaries: list[Summary] = []
    for pipeline in pipelines:
        totals = [float(sample.total_ns) for sample in pipeline.samples]
        names = sorted({stage for sample in pipeline.samples for stage in sample.stages})
        # A stage absent from an iteration took no time in it.
        stages = {
            stage: statistics.median(sample.stages.get(stage, 0) for sample in pipeline.samples)
            for stage in names
        }
        unattributed = statistics.median(
            max(0, sample.total_ns - sum(sample.stages.values())) for sample in pipeline.samples
        )
        p50 = statistics.median(totals)
        identity = metric_id("pipeline", binding, pipeline.name)
        metrics.append(
            Metric(
                id=identity,
                family="pipeline",
                unit="ns",
                value=p50,
                n=len(totals),
                samples=totals,
                p50=p50,
                p90=percentile(totals, 0.90),
                p99=percentile(totals, 0.99),
                stages=stages or None,
            )
        )
        metrics.append(
            Metric(
                id=f"{identity}.throughput",
                family="pipeline",
                unit="ops_per_s",
                value=pipeline.operations * len(totals) * 1e9 / sum(totals) if sum(totals) else 0,
            )
        )
        attributed = sum(stages.values())
        summaries.append(
            Summary(
                metric=identity,
                p50=p50,
                attributed=attributed / p50 if p50 else 0.0,
                gap=abs(attributed + unattributed - p50) / p50 if p50 else 0.0,
            )
        )
    return metrics, summaries


def main(argv: Sequence[str] | None = None, env: Mapping[str, str] = os.environ) -> int:
    parser = argparse.ArgumentParser(
        prog="python -m perf.harness.pipeline",
        description="Convert a live pipeline report into one perf-result/v1 file.",
    )
    parser.add_argument("--report", type=Path, required=True, help="pipeline driver output")
    parser.add_argument("--typedb", required=True, help="TypeDB server version measured")
    parser.add_argument("--out", type=Path, required=True, help="result file to write")
    parser.add_argument("--started-at", help="RFC 3339 run start; defaults to now")
    arguments = parser.parse_args(argv)
    try:
        binding, pipelines = read_report(arguments.report)
        metrics, summaries = convert(binding, pipelines)
        run = replace(collect_run(env, started_at=arguments.started_at), typedb=arguments.typedb)
        write_result(run, metrics, arguments.out)
    except (ValueError, OSError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    for summary in summaries:
        flag = "" if summary.gap <= STAGE_GAP_LIMIT else "  (stage-sum gap above 5%)"
        print(
            f"{summary.metric}: p50 {summary.p50 / 1e6:.3f} ms, "
            f"{summary.attributed:.0%} in stages, stage-sum gap {summary.gap:.1%}{flag}"
        )
    print(f"{arguments.out}: {len(metrics)} metric(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
