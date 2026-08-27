#!/usr/bin/env python3
"""Assemble and atomically persist four real artifact-bound Workforce V6 reports."""

from __future__ import annotations

import argparse
import os
import secrets
import stat
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
import c_artifact_journey as artifact_journey  # noqa: E402
import compare_workforce_conformance_v6 as conformance  # noqa: E402
import standalone_cli_candidate as cli_candidate  # noqa: E402
from persist_binding_reports import PublishError, publish  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
BINDINGS = conformance.BINDINGS
SURFACE_CONSUMER_FORMAT = "typebridge.workforce-v6-surface-consumer/v1"


class RunnerError(RuntimeError):
    """One V6 input, producer, comparator, or publication step failed."""


def run(command: list[str], *, env: dict[str, str]) -> None:
    result = subprocess.run(command, cwd=ROOT, env=env, check=False)
    if result.returncode != 0:
        raise RunnerError(f"command failed with exit {result.returncode}: {' '.join(command)}")


def regular(path: Path, label: str) -> Path:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise RunnerError(f"{label} cannot be inspected") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        raise RunnerError(f"{label} must be a regular non-symlink file")
    return path.resolve()


def predecessor_paths(root: Path, binding: str) -> tuple[tuple[int, Path], ...]:
    return tuple(
        (
            version,
            regular(root / f"v{version}" / f"{binding}.json", f"V{version} {binding} report"),
        )
        for version in conformance.predecessor_versions(binding)
    )


def predecessor_arguments(paths: tuple[tuple[int, Path], ...]) -> list[str]:
    return [item for version, path in paths for item in ("--predecessor", f"{version}={path}")]


def c_surface_consumer(phase4: dict[str, Any], *, source_commit: str) -> dict[str, Any]:
    artifacts = phase4["artifacts"]
    return {
        "format": SURFACE_CONSUMER_FORMAT,
        "binding": "c",
        "source-commit": source_commit,
        "surface-sha256": artifacts["generated-package"]["sha256"],
        "cli-candidate-id": artifacts["cli"]["candidate-id"],
        "runtime-provenance": "candidate-c-runtime",
        "checks": ["phase4-c17-cpp17", "sanitizers", "loader-unload"],
        "cleanup": {"temporary-consumer-absent": True},
        "publication-authority": False,
    }


def assemble(
    directory: Path,
    *,
    predecessors: Path,
    phase4_path: Path,
    provider: Path,
    live: Path,
    cli: Path,
    runtime: Path,
    generated: Path,
    source_commit: str,
    phase4: dict[str, Any],
    environment: dict[str, str],
) -> tuple[Path, ...]:
    nonce = secrets.token_hex(32)
    surfaces = directory / "surfaces"
    consumers = directory / "consumers"
    evidence = directory / "evidence"
    reports = directory / "reports"
    for path in (surfaces, consumers, evidence, reports):
        path.mkdir()

    run(
        [
            sys.executable,
            "scripts/ci/workforce_v6_surfaces.py",
            "build",
            str(cli),
            "--output",
            str(surfaces),
        ],
        env=environment,
    )
    for binding in BINDINGS[:-1]:
        surface = surfaces / f"type-bridge-{binding}-generated-workforce-v6.tar.gz"
        run(
            [
                sys.executable,
                "scripts/ci/validate_workforce_v6_surface_consumer.py",
                str(surface),
                "--binding",
                binding,
                "--output",
                str(consumers / f"{binding}.json"),
            ],
            env=environment,
        )
    (consumers / "c.json").write_bytes(
        conformance.canonical_json_bytes(c_surface_consumer(phase4, source_commit=source_commit))
    )

    output: list[Path] = []
    for binding in BINDINGS:
        historical = predecessor_paths(predecessors, binding)
        historical_args = predecessor_arguments(historical)
        producer = evidence / f"{binding}.json"
        report = reports / f"{binding}.json"
        surface = (
            generated
            if binding == "c"
            else surfaces / f"type-bridge-{binding}-generated-workforce-v6.tar.gz"
        )
        run(
            [
                sys.executable,
                "scripts/ci/compose_workforce_v6_evidence.py",
                "--binding",
                binding,
                "--run-nonce",
                nonce,
                "--source-commit",
                source_commit,
                "--surface-consumer",
                str(consumers / f"{binding}.json"),
                "--phase4",
                str(phase4_path),
                *historical_args,
                "--output",
                str(producer),
            ],
            env=environment,
        )
        run(
            [
                sys.executable,
                "scripts/ci/assemble_workforce_conformance_v6.py",
                "--binding",
                binding,
                "--run-nonce",
                nonce,
                "--evidence",
                str(producer),
                "--phase4",
                str(phase4_path),
                "--provider",
                str(provider),
                "--live",
                str(live),
                "--cli",
                str(cli),
                "--runtime",
                str(runtime),
                "--generated",
                str(generated),
                "--generated-surface",
                str(surface),
                *historical_args,
                "--output",
                str(report),
            ],
            env=environment,
        )
        output.append(report)
    run(
        [
            sys.executable,
            "scripts/ci/compare_workforce_conformance_v6.py",
            *(str(path) for path in output),
        ],
        env=environment,
    )
    return tuple(output)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--predecessors", required=True, type=Path)
    parser.add_argument("--phase4", required=True, type=Path)
    parser.add_argument("--provider", required=True, type=Path)
    parser.add_argument("--live", required=True, type=Path)
    parser.add_argument("--cli", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--generated", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()
    try:
        inputs = {
            name: regular(getattr(arguments, name), name)
            for name in ("phase4", "provider", "live", "cli", "runtime", "generated")
        }
        manifest = cli_candidate.validate(inputs["cli"])
        phase4 = artifact_journey.load_canonical_report(inputs["phase4"])
        artifact_journey.validate_phase4_report(
            phase4,
            inputs["provider"],
            inputs["live"],
            inputs["cli"],
            inputs["runtime"],
            inputs["generated"],
        )
        with tempfile.TemporaryDirectory(prefix="typebridge-workforce-v6-") as value:
            reports = assemble(
                Path(value),
                predecessors=arguments.predecessors.resolve(),
                phase4_path=inputs["phase4"],
                provider=inputs["provider"],
                live=inputs["live"],
                cli=inputs["cli"],
                runtime=inputs["runtime"],
                generated=inputs["generated"],
                source_commit=manifest["source-commit"],
                phase4=phase4,
                environment=os.environ.copy(),
            )
            publish(reports, arguments.output, checkout=ROOT)
        return 0
    except (
        RunnerError,
        PublishError,
        artifact_journey.JourneyError,
        cli_candidate.CandidateError,
        KeyError,
        OSError,
    ) as error:
        print(f"Workforce V6 candidate run failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
