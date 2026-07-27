#!/usr/bin/env python3
"""Check the public typed facade directly from two prebuilt wheel artifacts.

This runner never builds a wheel, creates an environment, or installs a
package. It extracts the supplied root/native wheels into a temporary consumer
and uses explicitly supplied, already prepared Python and Pyright executables.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import zipfile
from collections.abc import Iterable, Sequence
from pathlib import Path, PurePosixPath
from typing import Any

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
FIXTURE_ROOT = REPOSITORY_ROOT / "tests/compat/typed_python"
IMPORT_ENVIRONMENT_KEYS = (
    "PYTHONHOME",
    "PYTHONPATH",
    "VIRTUAL_ENV",
    "UV_PROJECT_ENVIRONMENT",
    "__PYVENV_LAUNCHER__",
)
ROOT_REQUIRED_MEMBERS = frozenset(
    {
        "type_bridge/__init__.py",
        "type_bridge/py.typed",
        "type_bridge/query_v2.py",
        "type_bridge/typed/__init__.py",
        "type_bridge/typed/_descriptors.py",
        "type_bridge/typed/_remote_terminal.py",
        "type_bridge/typed/_terminal.py",
        "type_bridge/typed/page.py",
        "type_bridge/typed/query.py",
        "type_bridge/typed/references.py",
        "type_bridge/typed/remote_limits.py",
        "type_bridge/typed/remote_query.py",
        "type_bridge/typed/remote_session.py",
        "type_bridge/typed/results.py",
        "type_bridge/typed/session.py",
    }
)
CORE_REQUIRED_MEMBERS = frozenset(
    {
        "type_bridge_core/__init__.py",
        "type_bridge_core/__init__.pyi",
        "type_bridge_core/py.typed",
    }
)
NEGATIVE_MARKER = "# artifact-type-error:"


class RunnerError(RuntimeError):
    """The artifact consumer setup or one of its checks failed."""


def clean_environment(source: dict[str, str] | None = None) -> dict[str, str]:
    """Remove ambient import overrides while retaining the prepared tools."""
    environment = dict(os.environ if source is None else source)
    for key in IMPORT_ENVIRONMENT_KEYS:
        environment.pop(key, None)
    environment["PYTHONNOUSERSITE"] = "1"
    environment["PYTHONDONTWRITEBYTECODE"] = "1"
    return environment


def absolute_executable(raw_path: str | os.PathLike[str], *, label: str) -> Path:
    """Validate an explicit executable without dereferencing venv symlinks."""
    path = Path(raw_path).expanduser().absolute()
    if not path.is_file():
        raise RunnerError(f"Prepared {label} executable does not exist: {path}")
    if os.name != "nt" and not os.access(path, os.X_OK):
        raise RunnerError(f"Prepared {label} is not executable: {path}")
    return path


def wheel_path(raw_path: str | os.PathLike[str], *, label: str) -> Path:
    """Resolve and validate one explicitly supplied wheel path."""
    path = Path(raw_path).expanduser().resolve()
    if not path.is_file():
        raise RunnerError(f"Prebuilt {label} wheel does not exist: {path}")
    if path.suffix != ".whl":
        raise RunnerError(f"Prebuilt {label} artifact must be a .whl file: {path}")
    if not zipfile.is_zipfile(path):
        raise RunnerError(f"Prebuilt {label} wheel is not a readable ZIP archive: {path}")
    return path


def wheel_members(path: Path) -> frozenset[str]:
    """Return normalized file members and reject ambiguous archive paths."""
    with zipfile.ZipFile(path) as archive:
        members: set[str] = set()
        for info in archive.infolist():
            if info.is_dir():
                continue
            name = _safe_member_name(info)
            if name in members:
                raise RunnerError(f"Wheel contains duplicate member {name!r}: {path}")
            members.add(name)
        return frozenset(members)


def inspect_wheels(root_wheel: Path, core_wheel: Path) -> dict[str, Any]:
    """Require the public Python files, native stub, marker, and extension."""
    root_members = wheel_members(root_wheel)
    core_members = wheel_members(core_wheel)
    missing_root = sorted(ROOT_REQUIRED_MEMBERS - root_members)
    missing_core = sorted(CORE_REQUIRED_MEMBERS - core_members)
    if missing_root:
        raise RunnerError(f"Root wheel is missing typed facade files: {missing_root}")
    if missing_core:
        raise RunnerError(f"Core wheel is missing typed package files: {missing_core}")

    native_members = sorted(
        name
        for name in core_members
        if name.startswith("type_bridge_core/type_bridge_core.") and name.endswith((".so", ".pyd"))
    )
    if len(native_members) != 1:
        raise RunnerError(
            f"Core wheel must contain exactly one packaged native extension; found {native_members}"
        )
    if root_members & core_members:
        overlap = sorted(root_members & core_members)
        raise RunnerError(f"Root and core wheels contain overlapping files: {overlap}")

    return {
        "root_wheel": str(root_wheel),
        "core_wheel": str(core_wheel),
        "root_member_count": len(root_members),
        "core_member_count": len(core_members),
        "native_extension": native_members[0],
    }


def extract_wheels(wheels: Iterable[Path], destination: Path) -> None:
    """Safely extract non-overlapping wheel files into one import root."""
    destination.mkdir(parents=True, exist_ok=False)
    extracted: set[str] = set()
    for wheel in wheels:
        with zipfile.ZipFile(wheel) as archive:
            for info in archive.infolist():
                name = _safe_member_name(info)
                if info.is_dir():
                    (destination / name).mkdir(parents=True, exist_ok=True)
                    continue
                if name in extracted:
                    raise RunnerError(f"Wheel extraction collision for {name!r}")
                extracted.add(name)
                target = destination.joinpath(*PurePosixPath(name).parts)
                target.parent.mkdir(parents=True, exist_ok=True)
                with archive.open(info) as source, target.open("wb") as output:
                    shutil.copyfileobj(source, output)


def _safe_member_name(info: zipfile.ZipInfo) -> str:
    """Validate one wheel member as a portable relative regular-file path."""
    name = info.filename
    path = PurePosixPath(name)
    if (
        not name
        or "\\" in name
        or path.is_absolute()
        or any(part in {"", ".", ".."} for part in path.parts)
    ):
        raise RunnerError(f"Wheel contains unsafe member path: {name!r}")
    mode = info.external_attr >> 16
    if mode and stat.S_ISLNK(mode):
        raise RunnerError(f"Wheel contains unsupported symbolic link: {name!r}")
    return path.as_posix()


def run_command(
    command: Sequence[str | os.PathLike[str]],
    *,
    cwd: Path,
    environment: dict[str, str],
) -> subprocess.CompletedProcess[str]:
    """Run one consumer command without a shell or ambient import overrides."""
    return subprocess.run(
        [os.fspath(part) for part in command],
        cwd=cwd,
        env=environment,
        text=True,
        capture_output=True,
        check=False,
    )


def require_success(
    completed: subprocess.CompletedProcess[str],
    *,
    description: str,
) -> None:
    """Raise with complete captured diagnostics when a command fails."""
    if completed.returncode == 0:
        return
    raise RunnerError(
        f"{description} failed with exit {completed.returncode}\n"
        f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
    )


def parse_json_output(stdout: str, *, description: str) -> dict[str, Any]:
    """Parse the final non-empty output line as a successful JSON report."""
    lines = [line for line in stdout.splitlines() if line.strip()]
    if not lines:
        raise RunnerError(f"{description} produced no report")
    try:
        payload = json.loads(lines[-1])
    except json.JSONDecodeError as error:
        raise RunnerError(f"{description} did not end with JSON") from error
    if not isinstance(payload, dict) or payload.get("status") != "ok":
        raise RunnerError(f"{description} reported failure: {payload!r}")
    return payload


def execute_runtime(
    *,
    python: Path,
    runtime_fixture: Path,
    artifact_root: Path,
    source_root: Path,
    consumer_root: Path,
    environment: dict[str, str],
) -> dict[str, Any]:
    """Run the public runtime probe under isolated interpreter mode."""
    completed = run_command(
        [
            python,
            "-I",
            runtime_fixture,
            "--artifact-root",
            artifact_root,
            "--source-root",
            source_root,
        ],
        cwd=consumer_root,
        environment=environment,
    )
    require_success(completed, description="typed artifact runtime probe")
    return parse_json_output(completed.stdout, description="typed artifact runtime probe")


def execute_live(
    *,
    python: Path,
    live_fixture: Path,
    artifact_root: Path,
    source_root: Path,
    consumer_root: Path,
    contract_fixture: Path,
    address: str,
    database: str,
    http_port: int,
    username: str,
    password: str,
    environment: dict[str, str],
) -> dict[str, Any]:
    """Run the named collected-page parity probe against a live TypeDB."""
    completed = run_command(
        [
            python,
            "-I",
            live_fixture,
            "--artifact-root",
            artifact_root,
            "--source-root",
            source_root,
            "--fixture",
            contract_fixture,
            "--address",
            address,
            "--database",
            database,
            "--http-port",
            str(http_port),
            "--username",
            username,
            "--password",
            password,
        ],
        cwd=consumer_root,
        environment=environment,
    )
    require_success(completed, description="typed artifact live probe")
    return parse_json_output(completed.stdout, description="typed artifact live probe")


def pyright_config(
    *,
    fixture: Path,
    artifact_root: Path,
    consumer_root: Path,
    python_version: str,
) -> dict[str, Any]:
    """Return a consumer-only Pyright project rooted outside the checkout."""
    return {
        "include": [fixture.relative_to(consumer_root).as_posix()],
        "pythonVersion": python_version,
        "typeCheckingMode": "strict",
        "reportMissingModuleSource": "none",
        "executionEnvironments": [
            {
                "root": str(consumer_root),
                "extraPaths": [str(artifact_root)],
            }
        ],
    }


def interpreter_language_version(
    python: Path,
    *,
    cwd: Path,
    environment: dict[str, str],
) -> str:
    """Read the major.minor language level from the supplied consumer interpreter."""
    completed = run_command(
        [
            python,
            "-I",
            "-c",
            "import sys; print(f'{sys.version_info.major}.{sys.version_info.minor}')",
        ],
        cwd=cwd,
        environment=environment,
    )
    require_success(completed, description="consumer Python version probe")
    version = completed.stdout.strip()
    if re.fullmatch(r"\d+\.\d+", version) is None:
        raise RunnerError(f"consumer Python returned an invalid language version: {version!r}")
    return version


def pyright_errors(
    completed: subprocess.CompletedProcess[str],
) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    """Parse Pyright JSON and return only error diagnostics."""
    try:
        payload = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise RunnerError(
            f"Pyright did not produce JSON (exit {completed.returncode})\n"
            f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        ) from error
    if not isinstance(payload, dict):
        raise RunnerError(f"Pyright report is not an object: {payload!r}")
    diagnostics = payload.get("generalDiagnostics", [])
    if not isinstance(diagnostics, list):
        raise RunnerError("Pyright generalDiagnostics is not a list")
    errors = [
        diagnostic
        for diagnostic in diagnostics
        if isinstance(diagnostic, dict) and diagnostic.get("severity") == "error"
    ]
    return payload, errors


def execute_pyright(
    *,
    python: Path,
    pyright: Path,
    positive: Path,
    negative: Path,
    artifact_root: Path,
    consumer_root: Path,
    environment: dict[str, str],
    python_version: str,
) -> dict[str, Any]:
    """Run positive and marker-exact negative checks against wheel contents."""
    reports: dict[str, Any] = {}
    for label, fixture in (("positive", positive), ("negative", negative)):
        config = consumer_root / f"pyrightconfig.{label}.json"
        config.write_text(
            json.dumps(
                pyright_config(
                    fixture=fixture,
                    artifact_root=artifact_root,
                    consumer_root=consumer_root,
                    python_version=python_version,
                ),
                indent=2,
            ),
            encoding="utf-8",
        )
        completed = run_command(
            [pyright, "--outputjson", "--project", config, "--pythonpath", python],
            cwd=consumer_root,
            environment=environment,
        )
        payload, errors = pyright_errors(completed)
        if label == "positive":
            if completed.returncode != 0 or errors:
                raise RunnerError(
                    "positive typed artifact Pyright consumer failed:\n"
                    f"{json.dumps(payload, indent=2)}\n{completed.stderr}"
                )
            reports[label] = {"errors": 0}
            continue

        expected_lines = marker_lines(negative, NEGATIVE_MARKER)
        foreign = [
            diagnostic
            for diagnostic in errors
            if Path(str(diagnostic.get("file", ""))).resolve() != negative.resolve()
        ]
        actual_lines = {
            int(diagnostic["range"]["start"]["line"]) + 1
            for diagnostic in errors
            if Path(str(diagnostic.get("file", ""))).resolve() == negative.resolve()
        }
        if completed.returncode == 0 or foreign or actual_lines != expected_lines:
            raise RunnerError(
                "negative typed artifact Pyright consumer drifted:\n"
                f"expected lines: {sorted(expected_lines)}\n"
                f"actual lines: {sorted(actual_lines)}\n"
                f"foreign diagnostics: {foreign}\n"
                f"{json.dumps(payload, indent=2)}\n{completed.stderr}"
            )
        reports[label] = {
            "errors": len(errors),
            "lines": sorted(actual_lines),
            "rules": sorted({str(diagnostic.get("rule")) for diagnostic in errors}),
        }
    return reports


def marker_lines(path: Path, marker: str) -> set[int]:
    """Return the one-based lines carrying expected-diagnostic markers."""
    lines = {
        line_number
        for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1)
        if marker in line
    }
    if not lines:
        raise RunnerError(f"Negative fixture has no {marker!r} markers: {path}")
    return lines


def copy_fixtures(source: Path, consumer_root: Path) -> dict[str, Path]:
    """Copy the standalone consumers and generated-model fixture."""
    copied: dict[str, Path] = {}
    for name in (
        "runtime.py",
        "live.py",
        "positive.py",
        "negative.py",
        "generated_owner_models.py",
    ):
        fixture = source / name
        if not fixture.is_file():
            raise RunnerError(f"Typed artifact fixture does not exist: {fixture}")
        target = consumer_root / name
        shutil.copy2(fixture, target)
        copied[name] = target
    return copied


def build_parser() -> argparse.ArgumentParser:
    """Build the artifact-only command-line interface."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root-wheel", required=True, help="Prebuilt type-bridge .whl")
    parser.add_argument("--core-wheel", required=True, help="Prebuilt type-bridge-core .whl")
    parser.add_argument("--python", required=True, help="Prepared consumer Python executable")
    parser.add_argument("--pyright", required=True, help="Prepared Pyright executable")
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
    parser.add_argument("--source-root", type=Path, default=REPOSITORY_ROOT)
    parser.add_argument("--fixture-root", type=Path, default=FIXTURE_ROOT, help=argparse.SUPPRESS)
    parser.add_argument(
        "--live-address", help="Live TypeDB address for the optional artifact probe"
    )
    parser.add_argument("--live-database", help="Existing live parity database name")
    parser.add_argument("--live-http-port", type=int, help="Live TypeDB HTTP API port")
    parser.add_argument("--live-fixture", type=Path, help="Shared typed-query parity contract JSON")
    parser.add_argument("--live-username", default="admin")
    parser.add_argument("--live-password", default="password")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Stage and run every built-wheel typed-facade acceptance check."""
    args = build_parser().parse_args(argv)
    root_wheel = wheel_path(args.root_wheel, label="root")
    core_wheel = wheel_path(args.core_wheel, label="core")
    python = absolute_executable(args.python, label="Python")
    pyright = absolute_executable(args.pyright, label="Pyright")
    source_root = args.source_root.expanduser().resolve()
    fixture_root = args.fixture_root.expanduser().resolve()
    live_values = (
        args.live_address,
        args.live_database,
        args.live_http_port,
        args.live_fixture,
    )
    if any(value is not None for value in live_values) and not all(
        value is not None for value in live_values
    ):
        raise RunnerError(
            "Live artifact acceptance requires --live-address, --live-database, "
            "--live-http-port, and --live-fixture together"
        )
    artifact_report = inspect_wheels(root_wheel, core_wheel)

    work_directory = args.work_directory.resolve() if args.work_directory else None
    if work_directory is not None:
        work_directory.mkdir(parents=True, exist_ok=True)
    consumer_root = Path(
        tempfile.mkdtemp(prefix="type-bridge-typed-python-", dir=work_directory)
    ).resolve()
    environment = clean_environment()

    try:
        artifact_root = consumer_root / "wheel-site-packages"
        extract_wheels((root_wheel, core_wheel), artifact_root)
        fixtures = copy_fixtures(fixture_root, consumer_root)
        python_version = interpreter_language_version(
            python,
            cwd=consumer_root,
            environment=environment,
        )
        runtime = execute_runtime(
            python=python,
            runtime_fixture=fixtures["runtime.py"],
            artifact_root=artifact_root,
            source_root=source_root,
            consumer_root=consumer_root,
            environment=environment,
        )
        typing = execute_pyright(
            python=python,
            pyright=pyright,
            positive=fixtures["positive.py"],
            negative=fixtures["negative.py"],
            artifact_root=artifact_root,
            consumer_root=consumer_root,
            environment=environment,
            python_version=python_version,
        )
        live = None
        if args.live_fixture is not None:
            assert args.live_address is not None
            assert args.live_database is not None
            assert args.live_http_port is not None
            contract_fixture = args.live_fixture.expanduser().resolve()
            if not contract_fixture.is_file():
                raise RunnerError(f"Live typed-query fixture does not exist: {contract_fixture}")
            live = execute_live(
                python=python,
                live_fixture=fixtures["live.py"],
                artifact_root=artifact_root,
                source_root=source_root,
                consumer_root=consumer_root,
                contract_fixture=contract_fixture,
                address=args.live_address,
                database=args.live_database,
                http_port=args.live_http_port,
                username=args.live_username,
                password=args.live_password,
                environment=environment,
            )
        report = {
            "status": "ok",
            "artifacts": artifact_report,
            "runtime": runtime,
            "typing": typing,
            "python": str(python),
            "python_version": python_version,
            "pyright": str(pyright),
            "consumer_root": str(consumer_root),
        }
        if live is not None:
            report["live"] = live
        print(
            json.dumps(
                report,
                indent=2,
                sort_keys=True,
            )
        )
        return 0
    finally:
        if args.keep_consumer:
            print(f"Kept typed artifact consumer at {consumer_root}", file=sys.stderr)
        else:
            shutil.rmtree(consumer_root, ignore_errors=True)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RunnerError as error:
        print(f"typed Python artifact runner: {error}", file=sys.stderr)
        raise SystemExit(1) from error
