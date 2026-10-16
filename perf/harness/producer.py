"""Validated result output and wall timing shared by command producers."""

from __future__ import annotations

import json
import subprocess
import time
from pathlib import Path

from perf.harness.schema import Metric, PerfResult, Run
from perf.harness.validate import check


def write_result(run: Run, metrics: list[Metric], out: Path) -> None:
    document = PerfResult(run=run, metrics=metrics).to_dict()
    findings = check(document)
    if findings.errors:
        raise ValueError("; ".join(findings.errors))
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")


def timed(command: list[str], log: Path, *, env: dict[str, str] | None = None) -> float:
    """Time the complete successful command, retaining its combined output."""
    log.parent.mkdir(parents=True, exist_ok=True)
    with log.open("w", encoding="utf-8") as output:
        started = time.perf_counter()
        subprocess.run(command, env=env, stdout=output, stderr=subprocess.STDOUT, check=True)
        return time.perf_counter() - started
