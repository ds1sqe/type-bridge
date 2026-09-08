#!/usr/bin/env python3
"""Validate generated Python consumers against two prebuilt candidate wheels."""

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
FIXTURE_ROOT = REPOSITORY_ROOT / "tests/compat/generated_python"
ACCEPTANCE_ROOT = REPOSITORY_ROOT / "type-bridge-core/crates/schema-codegen/tests/acceptance"
DOCUMENTED_EXAMPLES = REPOSITORY_ROOT / "tests/contracts/typed_query/python/documented_examples.py"
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
        "type_bridge/_runtime_projection.py",
        "type_bridge/_rust_runtime.py",
        "type_bridge/py.typed",
        "type_bridge/query/__init__.py",
        "type_bridge/query_v2.py",
        "type_bridge/session.py",
    }
)
CORE_REQUIRED_MEMBERS = frozenset(
    {
        "type_bridge_core/__init__.py",
        "type_bridge_core/__init__.pyi",
        "type_bridge_core/py.typed",
    }
)
NEGATIVE_MARKER = re.compile(r"# E: (?P<marker>[a-z][a-z0-9_]*):(?P<rule>report[A-Za-z]+)$")


class RunnerError(RuntimeError):
    """A candidate artifact or its generated consumer failed acceptance."""


def clean_environment(source: dict[str, str] | None = None) -> dict[str, str]:
    environment = dict(os.environ if source is None else source)
    for key in IMPORT_ENVIRONMENT_KEYS:
        environment.pop(key, None)
    environment["PYTHONNOUSERSITE"] = "1"
    environment["PYTHONDONTWRITEBYTECODE"] = "1"
    return environment


def absolute_executable(raw_path: str | os.PathLike[str], *, label: str) -> Path:
    path = Path(raw_path).expanduser().absolute()
    if not path.is_file():
        raise RunnerError(f"Prepared {label} executable does not exist: {path}")
    if os.name != "nt" and not os.access(path, os.X_OK):
        raise RunnerError(f"Prepared {label} is not executable: {path}")
    return path


def wheel_path(raw_path: str | os.PathLike[str], *, label: str) -> Path:
    path = Path(raw_path).expanduser().resolve()
    if not path.is_file() or path.suffix != ".whl" or not zipfile.is_zipfile(path):
        raise RunnerError(f"Prebuilt {label} wheel is missing or unreadable: {path}")
    return path


def safe_member_name(info: zipfile.ZipInfo) -> str:
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


def wheel_members(path: Path) -> frozenset[str]:
    with zipfile.ZipFile(path) as archive:
        members: set[str] = set()
        for info in archive.infolist():
            if info.is_dir():
                continue
            name = safe_member_name(info)
            if name in members:
                raise RunnerError(f"Wheel contains duplicate member {name!r}: {path}")
            members.add(name)
        return frozenset(members)


def inspect_wheels(root_wheel: Path, core_wheel: Path) -> dict[str, Any]:
    root_members = wheel_members(root_wheel)
    core_members = wheel_members(core_wheel)
    missing_root = sorted(ROOT_REQUIRED_MEMBERS - root_members)
    missing_core = sorted(CORE_REQUIRED_MEMBERS - core_members)
    if missing_root:
        raise RunnerError(f"Root wheel is missing generated-runtime files: {missing_root}")
    if missing_core:
        raise RunnerError(f"Core wheel is missing generated-runtime files: {missing_core}")
    native = sorted(
        member
        for member in core_members
        if member.startswith("type_bridge_core/type_bridge_core.")
        and member.endswith((".so", ".pyd"))
    )
    if len(native) != 1:
        raise RunnerError(f"Core wheel must contain exactly one native extension: {native}")
    overlap = root_members & core_members
    if overlap:
        raise RunnerError(f"Candidate wheels overlap: {sorted(overlap)}")
    return {
        "core_member_count": len(core_members),
        "native_extension": native[0],
        "root_member_count": len(root_members),
    }


def extract_wheels(wheels: Iterable[Path], destination: Path) -> None:
    destination.mkdir(parents=True, exist_ok=False)
    extracted: set[str] = set()
    for wheel in wheels:
        with zipfile.ZipFile(wheel) as archive:
            for info in archive.infolist():
                name = safe_member_name(info)
                target = destination.joinpath(*PurePosixPath(name).parts)
                if info.is_dir():
                    target.mkdir(parents=True, exist_ok=True)
                    continue
                if name in extracted:
                    raise RunnerError(f"Wheel extraction collision for {name!r}")
                extracted.add(name)
                target.parent.mkdir(parents=True, exist_ok=True)
                with archive.open(info) as source, target.open("wb") as output:
                    shutil.copyfileobj(source, output)


def run_command(
    command: Sequence[str | os.PathLike[str]],
    *,
    cwd: Path,
    environment: dict[str, str],
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [os.fspath(part) for part in command],
        cwd=cwd,
        env=environment,
        text=True,
        capture_output=True,
        check=False,
    )


def require_success(completed: subprocess.CompletedProcess[str], *, description: str) -> None:
    if completed.returncode != 0:
        raise RunnerError(
            f"{description} failed with exit {completed.returncode}\n"
            f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )


def parse_json_output(stdout: str, *, description: str) -> dict[str, Any]:
    lines = [line for line in stdout.splitlines() if line.strip()]
    if not lines:
        raise RunnerError(f"{description} produced no report")
    try:
        report = json.loads(lines[-1])
    except json.JSONDecodeError as error:
        raise RunnerError(f"{description} did not end with JSON") from error
    if not isinstance(report, dict) or report.get("status") != "ok":
        raise RunnerError(f"{description} reported failure: {report!r}")
    return report


def copy_tree_without_links(source: Path, destination: Path) -> None:
    if not source.is_dir() or source.is_symlink():
        raise RunnerError(f"Generated fixture root is invalid: {source}")
    links = [path for path in source.rglob("*") if path.is_symlink()]
    if links:
        raise RunnerError(f"Generated fixture contains symbolic links: {links}")
    shutil.copytree(source, destination)


def copy_consumers(consumer_root: Path) -> dict[str, Path]:
    sources = {
        "runtime.py": FIXTURE_ROOT / "runtime.py",
        "runtime_check.py": ACCEPTANCE_ROOT / "runtime_check.py",
        "positive.py": ACCEPTANCE_ROOT / "positive.py",
        "negative.py": ACCEPTANCE_ROOT / "negative.py",
        "documented_examples.py": DOCUMENTED_EXAMPLES,
    }
    copied: dict[str, Path] = {}
    for name, source in sources.items():
        if not source.is_file() or source.is_symlink():
            raise RunnerError(f"Generated artifact consumer is missing: {source}")
        target = consumer_root / name
        shutil.copy2(source, target)
        copied[name] = target
    return copied


def interpreter_language_version(
    python: Path,
    *,
    cwd: Path,
    environment: dict[str, str],
) -> str:
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
        raise RunnerError(f"Consumer Python returned an invalid version: {version!r}")
    return version


def expected_diagnostics(path: Path) -> dict[int, str]:
    expected: dict[int, str] = {}
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines()):
        match = NEGATIVE_MARKER.search(line)
        if match is not None:
            expected[line_number] = match["rule"]
    if not expected:
        raise RunnerError(f"Negative fixture has no diagnostic markers: {path}")
    return expected


def pyright_report(completed: subprocess.CompletedProcess[str]) -> dict[str, Any]:
    try:
        report = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise RunnerError(
            f"Pyright did not produce JSON\nstdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        ) from error
    if not isinstance(report, dict) or not isinstance(report.get("generalDiagnostics"), list):
        raise RunnerError(f"Pyright report has an invalid shape: {report!r}")
    return report


def execute_pyright(
    *,
    python: Path,
    pyright: Path,
    consumers: dict[str, Path],
    artifact_root: Path,
    generated_root: Path,
    consumer_root: Path,
    environment: dict[str, str],
    python_version: str,
) -> dict[str, Any]:
    results: dict[str, Any] = {}
    for label in ("positive", "documented_examples", "negative"):
        fixture = consumers[f"{label}.py"]
        config = consumer_root / f"pyrightconfig.{label}.json"
        config.write_text(
            json.dumps(
                {
                    "include": [fixture.name],
                    "pythonVersion": python_version,
                    "typeCheckingMode": "strict",
                    "reportMissingModuleSource": "none",
                    "executionEnvironments": [
                        {
                            "root": str(consumer_root),
                            "extraPaths": [str(generated_root), str(artifact_root)],
                        }
                    ],
                },
                indent=2,
            ),
            encoding="utf-8",
        )
        completed = run_command(
            [
                pyright,
                "--outputjson",
                "--project",
                config,
                "--pythonpath",
                python,
                fixture,
                generated_root / "generated_v2",
            ],
            cwd=consumer_root,
            environment=environment,
        )
        report = pyright_report(completed)
        errors = [
            diagnostic
            for diagnostic in report["generalDiagnostics"]
            if diagnostic.get("severity") == "error"
        ]
        if label != "negative":
            if completed.returncode != 0 or errors:
                raise RunnerError(
                    f"{label} generated artifact Pyright consumer failed:\n"
                    f"{json.dumps(report, indent=2)}\n{completed.stderr}"
                )
            results[label] = {"errors": 0}
            continue

        expected = expected_diagnostics(fixture)
        actual: dict[int, list[str]] = {}
        foreign: list[dict[str, Any]] = []
        for diagnostic in errors:
            if Path(str(diagnostic.get("file", ""))).resolve() != fixture.resolve():
                foreign.append(diagnostic)
                continue
            line = int(diagnostic["range"]["start"]["line"])
            actual.setdefault(line, []).append(str(diagnostic.get("rule", "")))
        if (
            completed.returncode == 0
            or foreign
            or set(actual) != set(expected)
            or any(actual[line] != [rule] for line, rule in expected.items())
        ):
            raise RunnerError(
                "negative generated artifact Pyright consumer drifted:\n"
                f"expected={expected!r}\nactual={actual!r}\nforeign={foreign!r}\n"
                f"{json.dumps(report, indent=2)}\n{completed.stderr}"
            )
        results[label] = {"errors": len(errors), "lines": sorted(line + 1 for line in actual)}
    return results


def execute_runtime(
    *,
    python: Path,
    consumers: dict[str, Path],
    artifact_root: Path,
    generated_root: Path,
    source_root: Path,
    consumer_root: Path,
    environment: dict[str, str],
) -> dict[str, Any]:
    completed = run_command(
        [
            python,
            "-I",
            consumers["runtime.py"],
            "--artifact-root",
            artifact_root,
            "--generated-root",
            generated_root,
            "--runtime-check",
            consumers["runtime_check.py"],
            "--source-root",
            source_root,
        ],
        cwd=consumer_root,
        environment=environment,
    )
    require_success(completed, description="generated artifact runtime probe")
    return parse_json_output(completed.stdout, description="generated artifact runtime probe")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root-wheel", required=True)
    parser.add_argument("--core-wheel", required=True)
    parser.add_argument("--generated-stage", required=True, type=Path)
    parser.add_argument("--python", required=True)
    parser.add_argument("--pyright", required=True)
    parser.add_argument("--work-directory", type=Path)
    parser.add_argument("--keep-consumer", action="store_true")
    parser.add_argument("--source-root", type=Path, default=REPOSITORY_ROOT)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    root_wheel = wheel_path(args.root_wheel, label="root")
    core_wheel = wheel_path(args.core_wheel, label="core")
    python = absolute_executable(args.python, label="Python")
    pyright = absolute_executable(args.pyright, label="Pyright")
    generated_stage = args.generated_stage.expanduser().resolve()
    source_root = args.source_root.expanduser().resolve()
    work_directory = args.work_directory.resolve() if args.work_directory else None
    if work_directory is not None:
        work_directory.mkdir(parents=True, exist_ok=True)
    consumer_root = Path(
        tempfile.mkdtemp(prefix="type-bridge-generated-python-", dir=work_directory)
    ).resolve()
    environment = clean_environment()
    try:
        wheel_report = inspect_wheels(root_wheel, core_wheel)
        artifact_root = consumer_root / "artifact"
        extract_wheels((root_wheel, core_wheel), artifact_root)
        generated_root = consumer_root / "generated"
        copy_tree_without_links(generated_stage, generated_root)
        for required in (
            generated_root / "generated_v2/__init__.py",
            generated_root / "generated_variant/__init__.py",
            generated_root / "schema-authority.json",
        ):
            if not required.is_file():
                raise RunnerError(f"Generated fixture is incomplete: {required}")
        consumers = copy_consumers(consumer_root)
        python_version = interpreter_language_version(
            python, cwd=consumer_root, environment=environment
        )
        runtime = execute_runtime(
            python=python,
            consumers=consumers,
            artifact_root=artifact_root,
            generated_root=generated_root,
            source_root=source_root,
            consumer_root=consumer_root,
            environment=environment,
        )
        typing = execute_pyright(
            python=python,
            pyright=pyright,
            consumers=consumers,
            artifact_root=artifact_root,
            generated_root=generated_root,
            consumer_root=consumer_root,
            environment=environment,
            python_version=python_version,
        )
        print(
            json.dumps(
                {
                    "python_version": python_version,
                    "runtime": runtime,
                    "status": "ok",
                    "typing": typing,
                    "wheels": wheel_report,
                },
                indent=2,
                sort_keys=True,
            )
        )
        return 0
    finally:
        if args.keep_consumer:
            print(f"Kept generated consumer at {consumer_root}", file=sys.stderr)
        else:
            shutil.rmtree(consumer_root, ignore_errors=True)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RunnerError as error:
        print(f"generated Python artifact runner: {error}", file=sys.stderr)
        raise SystemExit(1) from error
