"""Write one ``perf-result/v1`` file for the micro and Python binding benchmarks.

Usage::

    python -m perf.harness.micro --criterion-dir DIR --iai-dir DIR \
        --pytest-benchmark FILE --out FILE

Each directory holds one subdirectory per crate (see ``perf.harness.criterion``
and ``perf.harness.iai``). ``--pytest-benchmark`` is the JSON of the Python
binding benches in ``benchmarks/`` (see ``perf.harness.pytest_bench``). Any
input may be omitted, but at least one is required. The ``run`` block
comes from the GitHub Actions environment when present and from git and the
host otherwise. The result is validated before it is written; the exit code
is 1 when conversion or validation fails.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import subprocess
import sys
import tomllib
from collections.abc import Mapping, Sequence
from dataclasses import replace
from datetime import UTC, datetime
from pathlib import Path
from typing import cast

from perf.harness import criterion, iai, pytest_bench
from perf.harness.schema import TRIGGERS, Metric, PerfResult, Run, Runner, Toolchains, Trigger
from perf.harness.validate import check

REPO_ROOT = Path(__file__).resolve().parents[2]


def collect_run(env: Mapping[str, str], *, started_at: str | None = None) -> Run:
    """The ``run`` block shared by producers on this host."""
    return Run(
        commit=env.get("GITHUB_SHA") or _git("rev-parse", "HEAD"),
        # GITHUB_HEAD_REF is set only on pull requests, where GITHUB_REF_NAME
        # is the synthetic merge ref.
        branch=env.get("GITHUB_HEAD_REF")
        or env.get("GITHUB_REF_NAME")
        or _git("rev-parse", "--abbrev-ref", "HEAD"),
        version=_project_version(),
        trigger=_trigger(env),
        runner=Runner(image=_image(env), cpu=_cpu(), cores=os.cpu_count() or 1),
        toolchains=Toolchains(rust=_rustc_version(), python=None, node=None, cc=None),
        typedb=None,
        started_at=started_at
        or env.get("PERF_STARTED_AT")
        or datetime.now(UTC).isoformat(timespec="seconds"),
    )


def build(
    run: Run,
    *,
    criterion_dir: Path | None = None,
    iai_dir: Path | None = None,
    pytest_benchmark: Path | None = None,
) -> PerfResult:
    metrics: list[Metric] = []
    if criterion_dir is not None:
        metrics += criterion.convert(criterion_dir)
    if iai_dir is not None:
        metrics += iai.convert(iai_dir)
    if pytest_benchmark is not None:
        bench = pytest_bench.convert(pytest_benchmark)
        metrics += bench.metrics
        # The binding benches ran on that interpreter, not on this one.
        run = replace(run, toolchains=replace(run.toolchains, python=bench.python))
    return PerfResult(run=run, metrics=metrics)


def _trigger(env: Mapping[str, str]) -> Trigger:
    event = env.get("GITHUB_EVENT_NAME") or "local"
    if event not in TRIGGERS:
        raise ValueError(f"GITHUB_EVENT_NAME {event!r} is not one of {', '.join(TRIGGERS)}")
    return cast(Trigger, event)


def _image(env: Mapping[str, str]) -> str:
    # Hosted runners export the image name and its dated build.
    if (name := env.get("ImageOS")) and (build := env.get("ImageVersion")):
        return f"{name}/{build}"
    return platform.platform()


def _cpu() -> str:
    try:
        for line in Path("/proc/cpuinfo").read_text(encoding="utf-8").splitlines():
            key, _, value = line.partition(":")
            if key.strip() == "model name" and value.strip():
                return value.strip()
    except OSError:
        pass
    return platform.processor() or platform.machine() or "unknown"


def _rustc_version() -> str:
    # ``rustc 1.94.1 (e408947bf 2026-03-25)`` -> ``1.94.1``
    return _run("rustc", "--version").split()[1]


def _project_version() -> str:
    pyproject = tomllib.loads((REPO_ROOT / "pyproject.toml").read_text(encoding="utf-8"))
    return pyproject["project"]["version"]


def _git(*args: str) -> str:
    return _run("git", "-C", str(REPO_ROOT), *args)


def _run(*command: str) -> str:
    return subprocess.run(command, check=True, capture_output=True, text=True).stdout.strip()


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="python -m perf.harness.micro",
        description=(
            "Convert Criterion, gungraun and pytest-benchmark output into one perf-result/v1 file."
        ),
    )
    parser.add_argument("--criterion-dir", type=Path, help="CRITERION_HOME per crate")
    parser.add_argument("--iai-dir", type=Path, help="GUNGRAUN_HOME per crate")
    parser.add_argument(
        "--pytest-benchmark", type=Path, help="--benchmark-json output of benchmarks/"
    )
    parser.add_argument("--out", type=Path, required=True, help="result file to write")
    parser.add_argument("--started-at", help="RFC 3339 run start; defaults to now")
    arguments = parser.parse_args(argv)
    if (
        arguments.criterion_dir is None
        and arguments.iai_dir is None
        and arguments.pytest_benchmark is None
    ):
        parser.error("give at least one of --criterion-dir, --iai-dir, --pytest-benchmark")

    try:
        result = build(
            collect_run(os.environ, started_at=arguments.started_at),
            criterion_dir=arguments.criterion_dir,
            iai_dir=arguments.iai_dir,
            pytest_benchmark=arguments.pytest_benchmark,
        )
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1

    document = result.to_dict()
    findings = check(document)
    for message in findings.errors:
        print(f"error: {message}", file=sys.stderr)
    for message in findings.warnings:
        print(f"warning: {message}", file=sys.stderr)
    if findings.errors:
        return 1
    arguments.out.parent.mkdir(parents=True, exist_ok=True)
    arguments.out.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    print(f"{arguments.out}: {len(result.metrics)} metric(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
