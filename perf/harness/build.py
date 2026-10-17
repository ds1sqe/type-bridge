"""Measure cold release builds and convert stable Cargo HTML unit timings."""

from __future__ import annotations

import argparse
import json
import math
import os
import platform
import re
import subprocess
import sys
import tomllib
from collections import defaultdict
from collections.abc import Sequence
from dataclasses import replace
from pathlib import Path

from perf.harness.micro import REPO_ROOT, collect_run
from perf.harness.producer import timed, write_result
from perf.harness.schema import Metric, metric_id

MANIFEST = REPO_ROOT / "type-bridge-core/Cargo.toml"


def convert(report: Path) -> dict[str, float]:
    """Sum units (including build scripts) by crate from Cargo's stable HTML."""
    text = report.read_text(encoding="utf-8")
    marker = re.search(r"(?:const|let|var)\s+UNIT_DATA\s*=\s*", text)
    if marker is None:
        raise ValueError(f"{report}: missing Cargo UNIT_DATA")
    units, _ = json.JSONDecoder().raw_decode(text[marker.end() :])
    if not isinstance(units, list) or not units:
        raise ValueError(f"{report}: no Cargo timing units")
    totals: dict[str, float] = defaultdict(float)
    for unit in units:
        name, duration = unit["name"], unit["duration"]
        if (
            not isinstance(name, str)
            or not name
            or not isinstance(duration, (int, float))
            or isinstance(duration, bool)
        ):
            raise ValueError(f"{report}: invalid Cargo timing unit")
        duration = float(duration)
        if duration < 0 or not math.isfinite(duration):
            raise ValueError(f"{report}: invalid Cargo unit duration")
        totals[name] += duration
    return dict(totals)


def metrics(walls: dict[str, float], units: dict[str, float]) -> list[Metric]:
    result = [
        Metric(metric_id("build", group, "wall"), "build", "s", value)
        for group, value in sorted(walls.items())
    ]
    result += [
        Metric(metric_id("build", "unit", name), "build", "s", value)
        for name, value in sorted(units.items(), key=lambda item: (-item[1], item[0]))[:20]
    ]
    return result


def public_packages() -> list[str]:
    inventory = tomllib.loads((REPO_ROOT / "scripts/ci/cargo_release_inventory.toml").read_text())
    return [p["name"] for p in inventory["packages"] if p["classification"] == "public-first-party"]


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--group", action="append", choices=["public", "python", "node"])
    parser.add_argument("--package", action="append", help="public-group subset for a local smoke")
    parser.add_argument("--target-dir", type=Path, required=True, help="fresh cold-build root")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--started-at")
    parser.add_argument("--offline", action="store_true", help="use cached Cargo dependencies")
    args = parser.parse_args(argv)
    try:
        run = collect_run(os.environ, started_at=args.started_at)
        walls: dict[str, float] = {}
        units: dict[str, float] = defaultdict(float)
        groups = args.group or ["public", "python", "node"]
        if args.package and groups != ["public"]:
            raise ValueError("--package requires exactly --group public")
        for group in groups:
            target = (args.target_dir / group).resolve()
            # Never delete a user's target or silently measure a warm build.
            target.mkdir(parents=True, exist_ok=False)
            environment = dict(os.environ, CARGO_TARGET_DIR=str(target))
            if group == "python":
                environment["PYO3_PYTHON"] = sys.executable
            packages = {
                "public": args.package or public_packages(),
                "python": ["type-bridge-core"],
                "node": ["type-bridge-node"],
            }[group]
            command = [
                "cargo",
                "build",
                "--release",
                "--locked",
                "--timings",
                "--manifest-path",
                str(MANIFEST),
            ]
            if args.offline:
                command += ["--offline"]
            for package in packages:
                command += ["-p", package]
            if group == "python":
                command += ["--features", "pyo3/extension-module"]
            walls[group] = timed(command, args.out.parent / f"build-{group}.log", env=environment)
            if group == "python":
                # Include maturin's wheel assembly in the wall measurement. The
                # preceding cold Cargo build supplies stable HTML unit timings.
                walls[group] += timed(
                    [
                        "maturin",
                        "build",
                        *(["--offline"] if args.offline else []),
                        "--release",
                        "--locked",
                        "--manifest-path",
                        str(MANIFEST.parent / "crates/python/Cargo.toml"),
                        "-i",
                        sys.executable,
                        "--out",
                        str(target / "wheels"),
                    ],
                    args.out.parent / "build-python-wheel.log",
                    env=environment,
                    cwd=MANIFEST.parent,
                )
            for name, duration in convert(target / "cargo-timings/cargo-timing.html").items():
                units[name] += duration
        if "python" in groups:
            run = replace(run, toolchains=replace(run.toolchains, python=platform.python_version()))
        write_result(run, metrics(walls, dict(units)), args.out)
    except (OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
