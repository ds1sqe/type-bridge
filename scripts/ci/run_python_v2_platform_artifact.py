#!/usr/bin/env python3
"""Install one native wheel pair and author a V2 plan on its target runner."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import venv
from pathlib import Path

IMPORT_ENVIRONMENT_KEYS = (
    "PYTHONHOME",
    "PYTHONPATH",
    "VIRTUAL_ENV",
    "UV_PROJECT_ENVIRONMENT",
    "__PYVENV_LAUNCHER__",
)

PROBE = r"""
import importlib.metadata
import json
import sys
from pathlib import Path

expected_version = sys.argv[1]
source_root = Path(sys.argv[2]).resolve()
declared_path = Path(sys.argv[3]).resolve()

for distribution in ("type-bridge", "type-bridge-core"):
    actual = importlib.metadata.version(distribution)
    if actual != expected_version:
        raise SystemExit(
            f"{distribution} version mismatch: actual={actual!r}, "
            f"expected={expected_version!r}"
        )

import type_bridge
import type_bridge_core
from type_bridge.query_v2 import (
    AuthoredQueryInvocation,
    AuthoredQueryPlan,
    QueryPlanBuilder,
    QueryV2Authority,
)
from type_bridge_core import QueryV2Error

for module in (type_bridge, type_bridge_core):
    module_path = Path(module.__file__).resolve()
    environment = Path(sys.prefix).resolve()
    if module_path != environment and environment not in module_path.parents:
        if module_path == source_root or source_root in module_path.parents:
            raise SystemExit(
                f"{module.__name__} leaked to the source checkout: {module_path}"
            )
        raise SystemExit(
            f"{module.__name__} escaped the artifact environment: {module_path}"
        )

if QueryPlanBuilder is not type_bridge_core.QueryPlanBuilder:
    raise SystemExit("public QueryPlanBuilder is not the packaged native authority")
if QueryV2Authority is not type_bridge_core.QueryV2Authority:
    raise SystemExit("public QueryV2Authority is not the packaged native authority")

declared = declared_path.read_bytes().removesuffix(b"\n")
authority = QueryV2Authority(
    declared,
    "model-remote-parity",
    "typedb-3.12.1/v1",
)
builder = QueryPlanBuilder(authority)
person = builder.binding("person")
name = builder.binding("name")
wanted = builder.input("wanted_name", "string", False)
builder.match(
    (
        builder.isa(person, "entity", "parity-person", True),
        builder.has(person, name, "parity-person-name"),
        builder.value(
            "equal",
            builder.binding_operand(name),
            builder.input_operand(wanted),
        ),
    )
)
builder.select((person, name))
builder.require((name,))
builder.distinct()
builder.sort((builder.order(name, "ascending"),))
builder.limit(10)
plan: AuthoredQueryPlan = builder.finalize_rows((person, name))
invocation: AuthoredQueryInvocation = plan.rows((("Alice",),))

if plan.format != "typebridge.query-plan/v2":
    raise SystemExit(f"unexpected authored plan format: {plan.format!r}")
if len(plan.fingerprint) != 64:
    raise SystemExit(f"unexpected authored fingerprint: {plan.fingerprint!r}")
if invocation.plan_fingerprint != plan.fingerprint:
    raise SystemExit("authored invocation is not bound to its plan")
if not plan.canonical_bytes or not invocation.canonical_bytes:
    raise SystemExit("authored canonical bytes are empty")
if tuple(plan.required_capabilities) != tuple(sorted(plan.required_capabilities)):
    raise SystemExit("authored capabilities are not lexically ordered")

try:
    builder.binding("after_finalize")
except QueryV2Error as error:
    if error.code != "query_builder_finalized":
        raise SystemExit(f"unexpected finalized diagnostic: {error.code!r}") from error
else:
    raise SystemExit("finalized builder accepted a new binding")

print(
    json.dumps(
        {
            "status": "ok",
            "version": expected_version,
            "plan_fingerprint": plan.fingerprint,
            "capabilities": list(plan.required_capabilities),
            "python": sys.version.split()[0],
        },
        sort_keys=True,
    )
)
"""


class ArtifactSmokeError(RuntimeError):
    """The target-native artifact pair failed its isolated authoring smoke."""


def one_wheel(directory: Path, pattern: str, *, label: str) -> Path:
    """Return exactly one direct regular wheel matching the closed pattern."""
    if directory.is_symlink() or not directory.is_dir():
        raise ArtifactSmokeError(f"{label} directory is missing or symbolic: {directory}")
    candidates = sorted(
        path.resolve()
        for path in directory.glob(pattern)
        if not path.is_symlink() and path.is_file()
    )
    if len(candidates) != 1:
        raise ArtifactSmokeError(
            f"Expected exactly one {label} matching {pattern!r}, found "
            f"{len(candidates)}: {[path.name for path in candidates]}"
        )
    return candidates[0]


def venv_python(environment: Path) -> Path:
    """Return the platform-specific interpreter in a newly created venv."""
    if os.name == "nt":
        return environment / "Scripts/python.exe"
    return environment / "bin/python"


def clean_environment() -> dict[str, str]:
    """Prevent ambient import overrides from contaminating the consumer."""
    environment = dict(os.environ)
    for key in IMPORT_ENVIRONMENT_KEYS:
        environment.pop(key, None)
    environment["PYTHONNOUSERSITE"] = "1"
    environment["PYTHONDONTWRITEBYTECODE"] = "1"
    return environment


def run_checked(
    command: list[str],
    *,
    cwd: Path,
    environment: dict[str, str],
    description: str,
) -> subprocess.CompletedProcess[str]:
    """Run one shell-free command and retain complete failure diagnostics."""
    completed = subprocess.run(
        command,
        cwd=cwd,
        env=environment,
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        raise ArtifactSmokeError(
            f"{description} failed with exit {completed.returncode}\n"
            f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )
    return completed


def sha256(path: Path) -> str:
    """Hash one immutable artifact without loading it into memory."""
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run(args: argparse.Namespace) -> dict[str, object]:
    """Create an isolated consumer, install the pair, and execute the probe."""
    root_dist_dir = args.root_dist_dir.absolute()
    core_dist_dir = args.core_dist_dir.absolute()
    root_wheel = one_wheel(root_dist_dir, "type_bridge-*.whl", label="root wheel")
    core_wheel = one_wheel(
        core_dist_dir,
        "type_bridge_core-*.whl",
        label="core wheel",
    )
    source_root_input = args.source_root.absolute()
    declared_schema_input = args.declared_schema.absolute()
    if source_root_input.is_symlink() or not source_root_input.is_dir():
        raise ArtifactSmokeError(f"Source root is missing or symbolic: {source_root_input}")
    if declared_schema_input.is_symlink() or not declared_schema_input.is_file():
        raise ArtifactSmokeError(
            f"Declared-schema fixture is missing or symbolic: {declared_schema_input}"
        )
    source_root = source_root_input.resolve()
    declared_schema = declared_schema_input.resolve()

    work_dir = args.work_dir.absolute()
    if work_dir.exists() or work_dir.is_symlink():
        raise ArtifactSmokeError(f"Artifact smoke work directory already exists: {work_dir}")
    work_dir.mkdir(parents=True)
    environment_dir = work_dir / "venv"
    venv.EnvBuilder(with_pip=True, clear=False, symlinks=False).create(environment_dir)
    python = venv_python(environment_dir)
    if not python.is_file():
        raise ArtifactSmokeError(f"Artifact interpreter was not created: {python}")

    environment = clean_environment()
    run_checked(
        [
            str(python),
            "-m",
            "pip",
            "install",
            "--disable-pip-version-check",
            "--no-input",
            str(core_wheel),
            str(root_wheel),
        ],
        cwd=work_dir,
        environment=environment,
        description="artifact pair installation",
    )
    completed = run_checked(
        [
            str(python),
            "-I",
            "-c",
            PROBE,
            args.expected_version,
            str(source_root),
            str(declared_schema),
        ],
        cwd=work_dir,
        environment=environment,
        description="target-native V2 authoring probe",
    )
    lines = [line for line in completed.stdout.splitlines() if line.strip()]
    if not lines:
        raise ArtifactSmokeError("Target-native V2 authoring probe produced no report")
    try:
        probe = json.loads(lines[-1])
    except json.JSONDecodeError as error:
        raise ArtifactSmokeError(
            f"Target-native V2 authoring probe returned invalid JSON: {lines[-1]!r}"
        ) from error
    if not isinstance(probe, dict) or probe.get("status") != "ok":
        raise ArtifactSmokeError(f"Target-native V2 authoring probe failed: {probe!r}")
    return {
        "status": "ok",
        "root_wheel": root_wheel.name,
        "root_sha256": sha256(root_wheel),
        "core_wheel": core_wheel.name,
        "core_sha256": sha256(core_wheel),
        "probe": probe,
    }


def main() -> int:
    """Parse the closed workflow contract and print one deterministic report."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root-dist-dir", type=Path, required=True)
    parser.add_argument("--core-dist-dir", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--expected-version", required=True)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--declared-schema", type=Path, required=True)
    args = parser.parse_args()
    try:
        report = run(args)
    except ArtifactSmokeError as error:
        print(f"Python V2 platform artifact smoke failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
