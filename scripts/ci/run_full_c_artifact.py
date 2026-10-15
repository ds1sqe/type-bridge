#!/usr/bin/env python3
"""Run every real SDK suite producer against one captured C artifact set."""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

import c_release_policy as policy
import sdk_profile_matrix as profile_matrix


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
    for suite in tuple(profile_matrix.SUITES)[:-1]:
        run(
            "run_sdk_conformance.py",
            suite,
            "--output",
            str(output / suite),
            *(() if suite == "administration" else provider),
        )
    files = {
        filename: inputs / artifact / filename
        for artifact, filenames in policy.ARTIFACTS.items()
        for filename in filenames
    }
    run(
        "run_sdk_conformance.py",
        "artifacts",
        "--predecessors",
        str(output),
        "--acceptance",
        str(files["c-artifact-acceptance-report.json"]),
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
        str(output / "artifacts"),
    )


if __name__ == "__main__":
    main()
