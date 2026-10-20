"""Time offline Rust, Python and Node suites and retain slow-test observations."""

from __future__ import annotations

import argparse
import hashlib
import os
import platform
import re
import subprocess
import sys
from collections import defaultdict
from collections.abc import Sequence
from dataclasses import replace
from pathlib import Path

from perf.harness.micro import REPO_ROOT, collect_run
from perf.harness.producer import timed, write_result
from perf.harness.schema import Metric, metric_id


def pytest_durations(text: str) -> dict[str, float]:
    """Read call durations, excluding setup/teardown from pytest --durations=0."""
    return {
        name: float(seconds)
        for seconds, name in re.findall(
            r"^\s*([0-9]+(?:\.[0-9]+)?)s\s+call\s+(.+?)\s*$", text, re.MULTILINE
        )
    }


def node_durations(text: str) -> dict[str, float]:
    """Read leaf-test TAP durations, qualifying equal names by parent suites.

    Node 20 need not emit the newer YAML 'type' key. Nested Subtest indentation
    identifies suites so their inclusive duration is not counted as a test.
    """
    frames: list[tuple[int, str, bool]] = []
    pending: str | None = None
    result: dict[str, float] = defaultdict(float)
    for line in text.splitlines():
        if match := re.match(r"^(\s*)# Subtest: (.+)$", line):
            indent, name = len(match[1]), match[2]
            frames = [frame for frame in frames if frame[0] < indent]
            frames = [(depth, label, True) for depth, label, _ in frames]
            frames.append((indent, name, False))
            pending = None
        elif match := re.match(r"^(\s*)(?:not )?ok \d+ - (.+)$", line):
            indent = len(match[1])
            frames = [frame for frame in frames if frame[0] <= indent]
            if frames and frames[-1][0] == indent:
                pending = None if frames[-1][2] else "::".join(f[1] for f in frames)
            else:
                pending = match[2]
        elif pending and (match := re.match(r"^\s+duration_ms:\s*([0-9.eE+-]+)\s*$", line)):
            result[pending] += float(match[1]) / 1000
            pending = None
    return dict(result)


def convert(name: str, wall: float, output: str) -> list[Metric]:
    result = [Metric(metric_id("suite", name, "wall"), "suite", "s", wall)]
    if name == "rust":
        # Stable libtest does not offer --report-time; do not enable nightly
        # flags or substitute total-suite timing for individual test durations.
        return result
    durations = {"python": pytest_durations, "node": node_durations}[name](output)
    if not durations:
        raise ValueError(f"{name}: no per-test durations in suite output")
    for test, duration in sorted(durations.items(), key=lambda item: (-item[1], item[0]))[:10]:
        # Keep readable labels without collapsing distinct punctuation/parameters.
        identity = test[:80] + "_" + hashlib.sha256(test.encode()).hexdigest()[:12]
        result.append(Metric(metric_id("suite", name, "slow", identity), "suite", "s", duration))
    return result


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", action="append", choices=["rust", "python", "node"])
    parser.add_argument("--pytest-target", default="tests/unit", help="subset for local smoke")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--started-at")
    args = parser.parse_args(argv)
    try:
        run = collect_run(os.environ, started_at=args.started_at)
        metrics: list[Metric] = []
        for suite in args.suite or ["rust", "python", "node"]:
            log = args.out.parent / f"suite-{suite}.log"
            environment = dict(os.environ)
            command = {
                "rust": [
                    "cargo",
                    "test",
                    "--workspace",
                    "--locked",
                    "--offline",
                    "--manifest-path",
                    str(REPO_ROOT / "type-bridge-core/Cargo.toml"),
                ],
                "python": [
                    sys.executable,
                    "-m",
                    "pytest",
                    args.pytest_target,
                    "-q",
                    "--durations=0",
                    "--durations-min=0",
                ],
                "node": [
                    "npm",
                    "--prefix",
                    str(REPO_ROOT / "type-bridge-core/crates/node"),
                    "run",
                    "test:unit",
                ],
            }[suite]
            if suite == "node":
                # The existing npm runner inherits this into its node --test child.
                environment["NODE_OPTIONS"] = (
                    environment.get("NODE_OPTIONS", "") + " --test-reporter=tap"
                ).strip()
                version = subprocess.check_output(["node", "--version"], text=True).strip()
                run = replace(run, toolchains=replace(run.toolchains, node=version))
            elif suite == "python":
                run = replace(
                    run, toolchains=replace(run.toolchains, python=platform.python_version())
                )
            wall = timed(command, log, env=environment, cwd=REPO_ROOT)
            metrics += convert(suite, wall, log.read_text(encoding="utf-8"))
        write_result(run, metrics, args.out)
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
