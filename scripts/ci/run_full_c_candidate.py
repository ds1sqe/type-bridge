#!/usr/bin/env python3
"""Run every real V1–V6 producer against one captured C candidate set."""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from pathlib import Path

import c_release_policy as policy


def run(script: str, *arguments: str) -> None:
    subprocess.run(
        [sys.executable, str(policy.ROOT / "scripts/ci" / script), *arguments],
        cwd=policy.ROOT,
        check=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inputs", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--address", default="127.0.0.1:1729")
    parser.add_argument("--http-port", default="8000")
    arguments = parser.parse_args()
    output = arguments.output.resolve()
    if output.exists() or output == policy.ROOT or policy.ROOT in output.parents:
        raise ValueError("FULL-C output must be a new directory outside the checkout")
    output.mkdir(parents=True)
    inputs = arguments.inputs.resolve()
    provider = ("--address", arguments.address, "--http-port", arguments.http_port)
    run("run_workforce_v1_v2_candidate.py", "--output", str(output / "v12"), *provider)
    for version in (1, 2):
        shutil.move(str(output / "v12" / f"v{version}"), output / f"v{version}")
    (output / "v12").rmdir()
    for version in (3, 4, 5):
        script = (
            "run_workforce_v4_live.py" if version == 4 else f"run_workforce_v{version}_candidate.py"
        )
        run(script, "--output", str(output / f"v{version}"), *(() if version == 4 else provider))
    files = {
        filename: inputs / artifact / filename
        for artifact, filenames in policy.ARTIFACTS.items()
        for filename in filenames
    }
    run(
        "run_workforce_v6_candidate.py",
        "--predecessors",
        str(output),
        "--phase4",
        str(files["c-artifact-phase4-report.json"]),
        "--provider",
        str(files["c-artifact-clean-consumer.json"]),
        "--live",
        str(files["c-artifact-live-journey.json"]),
        "--cli",
        str(files[policy.CLI]),
        "--runtime",
        str(files[policy.RUNTIME]),
        "--generated",
        str(files[policy.GENERATED]),
        "--output",
        str(output / "v6"),
    )


if __name__ == "__main__":
    main()
