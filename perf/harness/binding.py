"""Write one ``perf-result/v1`` file from offline binding reports.

Usage::

    python -m perf.harness.binding --report FILE [--report FILE]...
        [--sdk NAME]... --out FILE

A report is the ``type-bridge-perf-binding/v1`` JSON an SDK driver writes
with ``--binding``: per op, the wall time of every measured call. The ops
need no TypeDB: ``build_purchase`` builds the purchase plan with one given
row, ``build_rows_1k`` builds the 1k-row insert (encoding its given rows),
and ``encode_rows_1k`` encodes that plan's canonical bytes. Staging and
answer decoding are not among them: every SDK's database handle connects
when it opens, and an answer exists only after a commit.

The Rust report is the baseline and is required. Each op becomes:

- ``binding.<sdk>.<op>`` in ns: ``value`` and ``p50`` are the median call,
  with ``p90``, ``p99``, ``n`` and the raw ``samples``;
- ``binding.<sdk>.<op>.overhead`` in ns, for every SDK but Rust: its median
  minus Rust's, the host-language cost of the binding. A result holds only
  measures (``>= 0``), so a host path cheaper than Rust's records 0; the
  console line prints the signed difference. C's ``encode_rows_1k`` copies
  the bytes its built plan already holds, where the others encode.
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import sys
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path

from perf.harness.micro import collect_run
from perf.harness.pipeline import SDKS, percentile
from perf.harness.producer import write_result
from perf.harness.schema import Metric, metric_id

REPORT_FORMAT = "type-bridge-perf-binding/v1"
# The offline ops every driver's ``--binding`` mode times, in report order.
OPS = ("build_purchase", "build_rows_1k", "encode_rows_1k")
BASELINE = "rust"


@dataclass(frozen=True)
class Op:
    name: str
    samples: list[int]


def read_report(path: Path) -> tuple[str, list[Op]]:
    """The binding and ops of a report; raises ``ValueError`` when malformed."""
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError(f"{path}: cannot read the binding report: {error}") from error
    if not isinstance(data, dict) or data.get("format") != REPORT_FORMAT:
        raise ValueError(f"{path}: expected format {REPORT_FORMAT!r}")
    binding = data.get("binding")
    if binding not in SDKS:
        raise ValueError(f"{path}: binding must be one of {', '.join(SDKS)}")
    raw = data.get("ops")
    if not isinstance(raw, list):
        raise ValueError(f"{path}: ops must be a list")
    ops = []
    for index, entry in enumerate(raw):
        where = f"{path}: ops[{index}]"
        if not isinstance(entry, dict):
            raise ValueError(f"{where}: must be an object")
        name, samples = entry.get("name"), entry.get("samples_ns")
        if not isinstance(samples, list) or not samples:
            raise ValueError(f"{where}.samples_ns: must be a non-empty list")
        if not all(isinstance(s, int) and not isinstance(s, bool) and s >= 0 for s in samples):
            raise ValueError(f"{where}.samples_ns: must hold non-negative integers")
        ops.append(Op(name=str(name), samples=samples))
    if [op.name for op in ops] != list(OPS):
        raise ValueError(f"{path}: ops must be {', '.join(OPS)}, in that order")
    return binding, ops


def convert(reports: Mapping[str, Sequence[Op]]) -> tuple[list[Metric], list[str]]:
    """The metrics of every report, with each binding's overhead over Rust,
    and one printable line per op."""
    if BASELINE not in reports:
        raise ValueError("the rust report is the baseline and is required")
    baseline = {op.name: statistics.median(op.samples) for op in reports[BASELINE]}
    metrics: list[Metric] = []
    lines: list[str] = []
    for binding, ops in reports.items():
        for op in ops:
            samples = [float(sample) for sample in op.samples]
            p50 = statistics.median(samples)
            identity = metric_id("binding", binding, op.name)
            metrics.append(
                Metric(
                    id=identity,
                    family="binding",
                    unit="ns",
                    value=p50,
                    n=len(samples),
                    samples=samples,
                    p50=p50,
                    p90=percentile(samples, 0.90),
                    p99=percentile(samples, 0.99),
                )
            )
            delta = p50 - baseline[op.name]
            lines.append(
                f"{identity}: p50 {p50 / 1e3:.1f} us"
                + ("" if binding == BASELINE else f", {delta / 1e3:+.1f} us over rust")
            )
            if binding != BASELINE:
                metrics.append(
                    Metric(
                        id=f"{identity}.overhead",
                        family="binding",
                        unit="ns",
                        value=max(0.0, delta),
                    )
                )
    return metrics, lines


def main(argv: Sequence[str] | None = None, env: Mapping[str, str] = os.environ) -> int:
    parser = argparse.ArgumentParser(
        prog="python -m perf.harness.binding",
        description="Convert offline binding reports into one perf-result/v1 file.",
    )
    parser.add_argument(
        "--report", type=Path, action="append", required=True, help="one SDK driver's output"
    )
    parser.add_argument(
        "--sdk", action="append", choices=SDKS, help="an SDK the reports must cover"
    )
    parser.add_argument("--out", type=Path, required=True, help="result file to write")
    parser.add_argument("--started-at", help="RFC 3339 run start; defaults to now")
    arguments = parser.parse_args(argv)
    try:
        reports: dict[str, list[Op]] = {}
        for report in arguments.report:
            binding, ops = read_report(report)
            if binding in reports:
                raise ValueError(f"{report}: a second report for {binding}")
            reports[binding] = ops
        if arguments.sdk and sorted(reports) != sorted(set(arguments.sdk)):
            raise ValueError(
                f"reports cover {sorted(reports)}, expected {sorted(set(arguments.sdk))}"
            )
        metrics, lines = convert(reports)
        write_result(collect_run(env, started_at=arguments.started_at), metrics, arguments.out)
    except (ValueError, OSError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    for line in lines:
        print(line)
    print(f"{arguments.out}: {len(metrics)} metric(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
