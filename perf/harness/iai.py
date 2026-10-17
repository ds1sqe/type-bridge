"""gungraun (formerly iai-callgrind) instruction counts as ``perf-result/v1``
micro metrics.

``root`` holds one directory per crate, each the ``GUNGRAUN_HOME`` of that
crate's ``cargo bench`` run with ``GUNGRAUN_SAVE_SUMMARY=json``. Every
benchmark leaves one ``summary.json`` (summary format version 7) somewhere
below its crate directory.

A benchmark in ``library_benchmark_group`` ``<group>`` with bench id ``<bench>``
(``#[bench::<bench>(...)]``, or the function name when it has no id) becomes
``micro.<crate>.<group>.<bench>.instructions``: the Criterion metric id of the
same work plus ``.instructions``. The value is Callgrind's total ``Ir``
(instructions executed) for the measured function.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from perf.harness.schema import Metric, metric_id

SUMMARY_VERSION = "7"


class IaiError(ValueError):
    """A gungraun summary is missing, malformed or of an unknown version."""


def convert(root: Path) -> list[Metric]:
    """Metrics for every ``summary.json`` under ``root``, sorted by id."""
    if not root.is_dir():
        raise IaiError(f"{root}: not a directory")
    metrics: list[Metric] = []
    for crate in sorted(path for path in root.iterdir() if path.is_dir()):
        for summary in sorted(crate.rglob("summary.json")):
            metrics.append(_metric(crate.name, summary))
    if not metrics:
        raise IaiError(f"{root}: no gungraun summaries (<crate>/**/summary.json)")
    return sorted(metrics, key=lambda metric: metric.id)


def _metric(crate: str, path: Path) -> Metric:
    summary = _load(path)
    if summary.get("version") != SUMMARY_VERSION:
        raise IaiError(
            f"{path}: summary version {summary.get('version')!r}, expected {SUMMARY_VERSION!r}"
        )
    group = summary.get("group")
    bench = summary.get("id") or summary.get("function_name")
    if not isinstance(group, str) or not isinstance(bench, str):
        raise IaiError(f"{path}: no group or benchmark name")
    try:
        identifier = metric_id("micro", crate, group, bench, "instructions")
    except ValueError as error:
        raise IaiError(f"{path}: {error}") from error
    return Metric(
        id=identifier,
        family="micro",
        unit="instructions",
        value=_instructions(summary, path),
    )


def _instructions(summary: dict[str, Any], path: Path) -> int:
    for profile in summary.get("profiles") or []:
        if not isinstance(profile, dict) or profile.get("tool") != "Callgrind":
            continue
        try:
            value = profile["data"]["total"]["metrics"]["Ir"]["values"]["new"]
        except (KeyError, TypeError) as error:
            raise IaiError(f"{path}: Callgrind profile has no new Ir total: {error}") from error
        if not isinstance(value, int) or isinstance(value, bool) or value < 0:
            raise IaiError(f"{path}: Ir is not a count: {value!r}")
        return value
    raise IaiError(f"{path}: no Callgrind profile")


def _load(path: Path) -> dict[str, Any]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except OSError as error:
        raise IaiError(f"{path}: cannot read: {error.strerror or error}") from error
    except json.JSONDecodeError as error:
        raise IaiError(f"{path}: not valid JSON: {error}") from error
    if not isinstance(data, dict):
        raise IaiError(f"{path}: expected a JSON object")
    return data
