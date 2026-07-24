#!/usr/bin/env python3
"""Run the legacy Python compatibility probe in an isolated consumer.

The runner never builds an artifact. It either installs one or more supplied
prebuilt artifacts into a temporary virtual environment, or uses an already
prepared Python interpreter. In both modes the probe runs from a temporary
directory outside the checkout with Python isolated mode enabled.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from collections.abc import Sequence
from pathlib import Path
from typing import Any

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_PROBE = REPOSITORY_ROOT / "tests/compat/legacy_python/probe.py"
IMPORT_ENVIRONMENT_KEYS = (
    "PYTHONHOME",
    "PYTHONPATH",
    "VIRTUAL_ENV",
    "UV_PROJECT_ENVIRONMENT",
    "__PYVENV_LAUNCHER__",
)


class RunnerError(RuntimeError):
    """A compatibility runner setup or probe failure."""


def clean_environment(source: dict[str, str] | None = None) -> dict[str, str]:
    """Return an environment without ambient Python import overrides."""
    environment = dict(os.environ if source is None else source)
    for key in IMPORT_ENVIRONMENT_KEYS:
        environment.pop(key, None)
    environment["PYTHONNOUSERSITE"] = "1"
    environment["PYTHONDONTWRITEBYTECODE"] = "1"
    return environment


def validate_artifacts(raw_paths: Sequence[str | os.PathLike[str]]) -> tuple[Path, ...]:
    """Resolve supplied artifact paths and reject missing/non-file inputs."""
    artifacts: list[Path] = []
    for raw_path in raw_paths:
        path = Path(raw_path).expanduser().resolve()
        if not path.is_file():
            raise RunnerError(f"Artifact does not exist or is not a file: {path}")
        artifacts.append(path)
    if not artifacts:
        raise RunnerError("At least one prebuilt artifact is required in artifact mode")
    return tuple(artifacts)


def environment_python(venv: Path) -> Path:
    """Return the Python executable for a virtual environment."""
    if os.name == "nt":
        return venv / "Scripts/python.exe"
    return venv / "bin/python"


def absolute_executable(path: Path) -> Path:
    """Make an executable path absolute without dereferencing venv symlinks."""
    return path.expanduser().absolute()


def run_checked(
    command: Sequence[str | os.PathLike[str]],
    *,
    cwd: Path,
    environment: dict[str, str],
) -> subprocess.CompletedProcess[str]:
    """Run a setup command and raise a concise error on failure."""
    completed = subprocess.run(
        [os.fspath(part) for part in command],
        cwd=cwd,
        env=environment,
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        rendered = " ".join(os.fspath(part) for part in command)
        raise RunnerError(
            f"Command failed ({completed.returncode}): {rendered}\n"
            f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )
    return completed


def prepare_artifact_environment(
    *,
    consumer_root: Path,
    bootstrap_python: Path,
    artifacts: Sequence[Path],
    wheelhouse: Path | None,
    no_index: bool,
    environment: dict[str, str],
) -> Path:
    """Create a virtual environment and install only the requested artifacts."""
    venv = consumer_root / "venv"
    run_checked(
        [bootstrap_python, "-I", "-m", "venv", venv],
        cwd=consumer_root,
        environment=environment,
    )
    python = environment_python(venv)
    install_command: list[str | os.PathLike[str]] = [
        python,
        "-I",
        "-m",
        "pip",
        "install",
        "--disable-pip-version-check",
        "--no-input",
    ]
    if no_index:
        install_command.append("--no-index")
    if wheelhouse is not None:
        install_command.extend(("--find-links", wheelhouse))
    install_command.extend(artifacts)
    run_checked(install_command, cwd=consumer_root, environment=environment)
    return python


def parse_probe_report(stdout: str) -> dict[str, Any]:
    """Parse the probe's final JSON line and require its success marker."""
    lines = [line for line in stdout.splitlines() if line.strip()]
    if not lines:
        raise RunnerError("Compatibility probe produced no report")
    try:
        report = json.loads(lines[-1])
    except json.JSONDecodeError as exc:
        raise RunnerError("Compatibility probe did not end with a JSON report") from exc
    if not isinstance(report, dict) or report.get("status") != "ok":
        raise RunnerError(f"Compatibility probe reported failure: {report!r}")
    return report


def validate_distribution_versions(
    report: dict[str, Any],
    *,
    expected_root_version: str,
    expected_core_version: str,
) -> None:
    """Bind the frozen probe result to the intended released-root/candidate-core pair."""
    expected = {
        "type-bridge": expected_root_version,
        "type-bridge-core": expected_core_version,
    }
    if report.get("package_version") != expected_root_version:
        raise RunnerError(
            "Compatibility probe imported the wrong type_bridge.__version__: "
            f"actual={report.get('package_version')!r}, expected={expected_root_version!r}"
        )
    for field in ("locations", "locations_after_probe"):
        locations = report.get(field)
        distributions = locations.get("distributions") if isinstance(locations, dict) else None
        if not isinstance(distributions, dict):
            raise RunnerError(
                f"Compatibility probe omitted installed distribution identities from {field}"
            )
        actual: dict[str, object] = {}
        for name in expected:
            record = distributions.get(name)
            actual[name] = record.get("version") if isinstance(record, dict) else None
        if actual != expected:
            raise RunnerError(
                "Compatibility probe used the wrong released-root/candidate-core pair: "
                f"field={field!r}, actual={actual!r}, expected={expected!r}"
            )


def execute_probe(
    *,
    python: Path,
    probe: Path,
    source_root: Path,
    consumer_root: Path,
    environment: dict[str, str],
) -> dict[str, Any]:
    """Copy and execute the probe outside the repository."""
    copied_probe = consumer_root / "legacy_python_probe.py"
    shutil.copy2(probe, copied_probe)
    completed = run_checked(
        [python, "-I", copied_probe, "--source-root", source_root],
        cwd=consumer_root,
        environment=environment,
    )
    report = parse_probe_report(completed.stdout)
    report["consumer_root"] = str(consumer_root)
    report["python"] = str(python)
    return report


def build_parser() -> argparse.ArgumentParser:
    """Build the command-line parser."""
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument(
        "--artifact",
        action="append",
        default=[],
        metavar="PATH",
        help="Prebuilt wheel/sdist path; repeat for the root and native artifacts",
    )
    mode.add_argument(
        "--python",
        type=Path,
        help="Already prepared consumer Python; no environment creation or install",
    )
    parser.add_argument(
        "--bootstrap-python",
        type=Path,
        default=Path(sys.executable),
        help="Python used to create the artifact-mode virtual environment",
    )
    parser.add_argument(
        "--wheelhouse",
        type=Path,
        help="Optional dependency wheelhouse passed to pip --find-links",
    )
    parser.add_argument(
        "--no-index",
        action="store_true",
        help="Disable package indexes while installing supplied artifacts",
    )
    parser.add_argument(
        "--work-directory",
        type=Path,
        help="Parent for the temporary consumer (defaults to the system temp area)",
    )
    parser.add_argument(
        "--keep-consumer",
        action="store_true",
        help="Keep the temporary consumer directory for diagnostics",
    )
    parser.add_argument(
        "--expected-root-version",
        help="Require the installed type-bridge distribution to have this version",
    )
    parser.add_argument(
        "--expected-core-version",
        help="Require the installed type-bridge-core distribution to have this version",
    )
    parser.add_argument("--probe", type=Path, default=DEFAULT_PROBE, help=argparse.SUPPRESS)
    parser.add_argument("--source-root", type=Path, default=REPOSITORY_ROOT)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Run the isolated compatibility consumer."""
    args = build_parser().parse_args(argv)
    if (args.expected_root_version is None) != (args.expected_core_version is None):
        raise RunnerError(
            "--expected-root-version and --expected-core-version must be provided together"
        )
    source_root = args.source_root.resolve()
    probe = args.probe.resolve()
    if not probe.is_file():
        raise RunnerError(f"Compatibility probe does not exist: {probe}")

    work_directory = args.work_directory.resolve() if args.work_directory else None
    if work_directory is not None:
        work_directory.mkdir(parents=True, exist_ok=True)
    consumer_root = Path(
        tempfile.mkdtemp(prefix="type-bridge-legacy-python-", dir=work_directory)
    ).resolve()
    environment = clean_environment()

    try:
        if args.python is not None:
            # Do not resolve this path: POSIX venv Python is commonly a symlink
            # to the base interpreter, and dereferencing it discards the venv's
            # site-packages when the command is launched.
            python = absolute_executable(args.python)
            if not python.is_file():
                raise RunnerError(f"Prepared Python does not exist: {python}")
        else:
            artifacts = validate_artifacts(args.artifact)
            wheelhouse = args.wheelhouse.resolve() if args.wheelhouse else None
            if wheelhouse is not None and not wheelhouse.is_dir():
                raise RunnerError(f"Wheelhouse does not exist or is not a directory: {wheelhouse}")
            python = prepare_artifact_environment(
                consumer_root=consumer_root,
                bootstrap_python=args.bootstrap_python.expanduser().resolve(),
                artifacts=artifacts,
                wheelhouse=wheelhouse,
                no_index=args.no_index,
                environment=environment,
            )

        report = execute_probe(
            python=python,
            probe=probe,
            source_root=source_root,
            consumer_root=consumer_root,
            environment=environment,
        )
        if args.expected_root_version is not None and args.expected_core_version is not None:
            validate_distribution_versions(
                report,
                expected_root_version=args.expected_root_version,
                expected_core_version=args.expected_core_version,
            )
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0
    finally:
        if args.keep_consumer:
            print(f"Kept compatibility consumer at {consumer_root}", file=sys.stderr)
        else:
            shutil.rmtree(consumer_root, ignore_errors=True)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RunnerError as error:
        print(f"legacy Python compatibility runner: {error}", file=sys.stderr)
        raise SystemExit(1) from error
