"""Validate ``perf-result/v1`` files before they are stored or rendered.

Usage: ``python -m perf.harness.validate FILES...``. Errors and warnings are
printed per file; the exit code is 1 when any file has an error.

Every message starts with the path of the offending field, for example
``metrics[2].unit``.
"""

from __future__ import annotations

import argparse
import json
import math
import re
import sys
from collections.abc import Sequence
from dataclasses import MISSING, dataclass, field, fields
from datetime import datetime
from pathlib import Path
from typing import Any, TypeGuard

from perf.harness.schema import (
    COMMIT,
    FAMILIES,
    FORMAT,
    METRIC_ID,
    STAGE_NAME,
    TRIGGERS,
    UNITS,
    Metric,
    Run,
    Runner,
    Toolchains,
)


@dataclass
class Findings:
    errors: list[str] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)


def validate(path: str | Path) -> list[str]:
    """Return the errors in the result file at ``path``; empty means valid."""
    return check_file(path).errors


def check_file(path: str | Path) -> Findings:
    """Errors and warnings for the result file at ``path``."""
    try:
        data = json.loads(Path(path).read_text(encoding="utf-8"))
    except OSError as error:
        return Findings(errors=[f"<file>: cannot read: {error.strerror or error}"])
    except (json.JSONDecodeError, UnicodeDecodeError) as error:
        return Findings(errors=[f"<file>: not valid JSON: {error}"])
    return check(data)


def check(data: object) -> Findings:
    """Errors and warnings for an already parsed result document."""
    findings = Findings()
    document = _object(data, "<document>", findings)
    if document is None:
        return findings
    _keys(document, "<document>", {"format", "run", "metrics"}, set(), findings)
    if "format" in document and document["format"] != FORMAT:
        findings.errors.append(f"format: expected {FORMAT!r}, got {document['format']!r}")
    if "run" in document:
        _check_run(document["run"], findings)
    if "metrics" in document:
        _check_metrics(document["metrics"], findings)
    return findings


def _check_run(value: object, findings: Findings) -> None:
    run = _object(value, "run", findings)
    if run is None:
        return
    _keys(run, "run", *_field_names(Run), findings)
    _pattern(run, "commit", "run", COMMIT, findings)
    _string(run, "branch", "run", findings)
    _string(run, "version", "run", findings)
    _choice(run, "trigger", "run", TRIGGERS, findings)
    _string(run, "typedb", "run", findings, nullable=True)
    if _string(run, "started_at", "run", findings):
        _timestamp(run["started_at"], "run.started_at", findings)

    if "runner" in run and (runner := _object(run["runner"], "run.runner", findings)) is not None:
        _keys(runner, "run.runner", *_field_names(Runner), findings)
        _string(runner, "image", "run.runner", findings)
        _string(runner, "cpu", "run.runner", findings)
        cores = runner.get("cores")
        if "cores" in runner and not (_is_int(cores) and cores >= 1):
            findings.errors.append(f"run.runner.cores: expected an integer >= 1, got {cores!r}")

    if (
        "toolchains" in run
        and (toolchains := _object(run["toolchains"], "run.toolchains", findings)) is not None
    ):
        names, _ = _field_names(Toolchains)
        _keys(toolchains, "run.toolchains", names, set(), findings)
        for name in sorted(names & toolchains.keys()):
            _string(toolchains, name, "run.toolchains", findings, nullable=True)


def _check_metrics(value: object, findings: Findings) -> None:
    if not isinstance(value, list) or not value:
        findings.errors.append(f"metrics: expected a non-empty array, got {value!r}")
        return
    seen: dict[str, int] = {}
    for index, item in enumerate(value):
        where = f"metrics[{index}]"
        metric = _object(item, where, findings)
        if metric is None:
            continue
        _keys(metric, where, *_field_names(Metric), findings)
        if _pattern(metric, "id", where, METRIC_ID, findings):
            metric_id = metric["id"]
            if metric_id in seen:
                findings.errors.append(
                    f"{where}.id: duplicate id {metric_id!r}, first at metrics[{seen[metric_id]}]"
                )
            else:
                seen[metric_id] = index
        _choice(metric, "family", where, FAMILIES, findings)
        _choice(metric, "unit", where, UNITS, findings)
        _number(metric, "value", where, findings)
        for name in ("p50", "p90", "p99"):
            _number(metric, name, where, findings)
        if "n" in metric and not (_is_int(metric["n"]) and metric["n"] >= 1):
            findings.errors.append(f"{where}.n: expected an integer >= 1, got {metric['n']!r}")
        if "samples" in metric:
            samples = metric["samples"]
            if not isinstance(samples, list) or not all(_is_measure(s) for s in samples):
                findings.errors.append(
                    f"{where}.samples: expected an array of finite numbers >= 0, got {samples!r}"
                )
        if "ci95" in metric:
            _check_ci95(metric["ci95"], where, findings)
        if "stages" in metric:
            _check_stages(metric, where, findings)


def _check_ci95(interval: object, where: str, findings: Findings) -> None:
    if not (isinstance(interval, list) and len(interval) == 2 and all(map(_is_finite, interval))):
        findings.errors.append(
            f"{where}.ci95: expected [lower, upper], two finite numbers, got {interval!r}"
        )
    elif interval[0] > interval[1]:
        findings.errors.append(
            f"{where}.ci95: lower bound {interval[0]} exceeds upper bound {interval[1]}"
        )


def _check_stages(metric: dict[str, Any], where: str, findings: Findings) -> None:
    stages = metric["stages"]
    if not isinstance(stages, dict) or not stages:
        findings.errors.append(f"{where}.stages: expected a non-empty object, got {stages!r}")
        return
    if metric.get("unit") != "ns":
        findings.errors.append(
            f"{where}.stages: stages are in ns, so unit must be 'ns', got {metric.get('unit')!r}"
        )
    valid = True
    for name, ns in stages.items():
        if not STAGE_NAME.fullmatch(name):
            findings.errors.append(f"{where}.stages: span name {name!r} is not a dotted name")
            valid = False
        if not _is_measure(ns):
            findings.errors.append(
                f"{where}.stages.{name}: expected a finite number >= 0, got {ns!r}"
            )
            valid = False
    value = metric.get("value")
    if valid and _is_measure(value) and (total := sum(stages.values())) > value:
        # Spans can overlap or include setup outside the timed region; flag it
        # for a human rather than drop the measurement.
        findings.warnings.append(
            f"{where}.stages: stage sum {total} exceeds value {value} ({metric.get('id')})"
        )


def _field_names(model: type) -> tuple[set[str], set[str]]:
    """Required and optional JSON keys of a schema dataclass."""
    required = {f.name for f in fields(model) if f.default is MISSING}
    return required, {f.name for f in fields(model)} - required


def _object(value: object, where: str, findings: Findings) -> dict[str, Any] | None:
    """``value`` as a JSON object, or ``None`` after recording why it is not."""
    if isinstance(value, dict):
        return value
    findings.errors.append(f"{where}: expected an object, got {value!r}")
    return None


def _keys(
    obj: dict[str, Any], where: str, required: set[str], optional: set[str], findings: Findings
) -> None:
    for name in sorted(required - obj.keys()):
        findings.errors.append(f"{_join(where, name)}: required field is missing")
    for name in sorted(obj.keys() - required - optional):
        findings.errors.append(f"{_join(where, name)}: unknown field")


def _string(
    obj: dict[str, Any], key: str, where: str, findings: Findings, *, nullable: bool = False
) -> bool:
    """True when ``obj[key]`` is a non-empty string."""
    if key not in obj:
        return False
    value = obj[key]
    if isinstance(value, str) and value:
        return True
    if not (nullable and value is None):
        expected = "a non-empty string or null" if nullable else "a non-empty string"
        findings.errors.append(f"{_join(where, key)}: expected {expected}, got {value!r}")
    return False


def _pattern(
    obj: dict[str, Any], key: str, where: str, pattern: re.Pattern[str], findings: Findings
) -> bool:
    if key not in obj:
        return False
    value = obj[key]
    if isinstance(value, str) and pattern.fullmatch(value):
        return True
    findings.errors.append(
        f"{_join(where, key)}: {value!r} does not match pattern {pattern.pattern}"
    )
    return False


def _choice(
    obj: dict[str, Any], key: str, where: str, choices: tuple[str, ...], findings: Findings
) -> None:
    if key in obj and obj[key] not in choices:
        findings.errors.append(
            f"{_join(where, key)}: {obj[key]!r} is not one of {', '.join(choices)}"
        )


def _number(obj: dict[str, Any], key: str, where: str, findings: Findings) -> None:
    if key in obj and not _is_measure(obj[key]):
        findings.errors.append(
            f"{_join(where, key)}: expected a finite number >= 0, got {obj[key]!r}"
        )


def _timestamp(value: str, where: str, findings: Findings) -> None:
    try:
        parsed = datetime.fromisoformat(value)
    except ValueError:
        findings.errors.append(f"{where}: {value!r} is not an RFC 3339 timestamp")
        return
    if parsed.tzinfo is None:
        findings.errors.append(f"{where}: {value!r} has no UTC offset")


def _is_int(value: object) -> TypeGuard[int]:
    return isinstance(value, int) and not isinstance(value, bool)


def _is_finite(value: object) -> TypeGuard[int | float]:
    """A finite JSON number (``bool`` is not a number here)."""
    return isinstance(value, int | float) and not isinstance(value, bool) and math.isfinite(value)


def _is_measure(value: object) -> TypeGuard[int | float]:
    """A finite, non-negative JSON number."""
    return _is_finite(value) and value >= 0


def _join(where: str, key: str) -> str:
    return key if where == "<document>" else f"{where}.{key}"


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="python -m perf.harness.validate", description="Validate perf-result/v1 files."
    )
    parser.add_argument("files", nargs="+", type=Path, metavar="FILE")
    arguments = parser.parse_args(argv)

    failed = 0
    for path in arguments.files:
        findings = check_file(path)
        for message in findings.errors:
            print(f"{path}: error: {message}", file=sys.stderr)
        for message in findings.warnings:
            print(f"{path}: warning: {message}", file=sys.stderr)
        failed += bool(findings.errors)
    total = len(arguments.files)
    print(f"{total - failed} of {total} file(s) valid")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
