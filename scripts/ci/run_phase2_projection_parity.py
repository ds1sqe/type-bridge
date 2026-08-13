#!/usr/bin/env python3
"""Produce and compare the four provider-free Phase-2 projection reports."""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CORE = ROOT / "type-bridge-core"
SCHEMA = ROOT / "tests/contracts/sdk_conformance/workforce-v3/schema-v3.yaml"
PYTHON_PRODUCER = CORE / "crates/schema-codegen/tests/acceptance/phase2_parity_check.py"
NODE_PRODUCER = CORE / "crates/schema-codegen/tests/typescript_acceptance/phase2_parity_check.mjs"
NODE_PACKAGE = CORE / "crates/node"
COMPARATOR = ROOT / "scripts/ci/compare_phase2_projection_parity.py"
FOREIGN_PLAYING_FACT = "member: { card: { min: 0, max: 2 }, doc: membership player }"
FOREIGN_PLAYING_REPLACEMENT = "member: { card: { min: 0, max: 3 }, doc: membership player }"


class RunnerError(RuntimeError):
    """The isolated producer fan-in could not complete safely."""


@dataclass(frozen=True, slots=True)
class Layout:
    root: Path
    python: Path
    node: Path
    reports: Path
    foreign_schema: Path
    python_report: Path
    node_report: Path
    rust_report: Path
    c_report: Path

    @classmethod
    def under(cls, root: Path) -> Layout:
        reports = root / "reports"
        return cls(
            root=root,
            python=root / "python",
            node=root / "node",
            reports=reports,
            foreign_schema=root / "workforce-v3-foreign.yaml",
            python_report=reports / "python.json",
            node_report=reports / "node.json",
            rust_report=reports / "rust.json",
            c_report=reports / "c.json",
        )

    def report_paths(self) -> tuple[Path, Path, Path, Path]:
        return (
            self.python_report,
            self.node_report,
            self.rust_report,
            self.c_report,
        )


@dataclass(frozen=True, slots=True)
class CommandSpec:
    label: str
    arguments: tuple[str, ...]
    cwd: Path
    environment: Mapping[str, str] | None = None


def _cargo_run_example(example: str, schema: Path, output: Path) -> tuple[str, ...]:
    return (
        "cargo",
        "run",
        "--locked",
        "--quiet",
        "--manifest-path",
        str(CORE / "Cargo.toml"),
        "--package",
        "type-bridge-schema-codegen",
        "--example",
        example,
        "--",
        str(schema),
        str(output),
    )


def command_plan(layout: Layout) -> tuple[CommandSpec, ...]:
    """Return the exact same-stage producer and comparator command ledger."""

    python_environment = {
        "TYPE_BRIDGE_PHASE2_PARITY_REPORT": str(layout.python_report),
        "TYPE_BRIDGE_PHASE2_PYTHON_PACKAGE_ROOT": str(layout.python),
        "TYPE_BRIDGE_PHASE2_REPOSITORY_ROOT": str(ROOT),
    }
    node_environment = {
        "TYPE_BRIDGE_PHASE2_PARITY_REPORT": str(layout.node_report),
        "TYPE_BRIDGE_PHASE2_REPOSITORY_ROOT": str(ROOT),
    }
    rust_environment = {
        "TYPE_BRIDGE_PHASE2_RUST_REPORT": str(layout.rust_report),
    }
    c_environment = {
        "TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER": "1",
        "TYPE_BRIDGE_PHASE2_PARITY_REPORT_C": str(layout.c_report),
    }
    tsc = NODE_PACKAGE / "node_modules/.bin/tsc"
    return (
        CommandSpec("install the Python native facade", ("maturin", "develop"), CORE),
        CommandSpec(
            "generate the local Python package",
            _cargo_run_example(
                "emit_python_acceptance",
                SCHEMA,
                layout.python / "generated_phase2",
            ),
            ROOT,
        ),
        CommandSpec(
            "generate the foreign Python package",
            _cargo_run_example(
                "emit_python_acceptance",
                layout.foreign_schema,
                layout.python / "generated_phase2_foreign",
            ),
            ROOT,
        ),
        CommandSpec(
            "produce the Python report",
            (sys.executable, str(layout.python / PYTHON_PRODUCER.name)),
            ROOT,
            python_environment,
        ),
        CommandSpec("build the Node package", ("npm", "run", "build"), NODE_PACKAGE),
        CommandSpec(
            "generate the local TypeScript package",
            _cargo_run_example(
                "emit_typescript_acceptance",
                SCHEMA,
                layout.node / "generated_phase2",
            ),
            ROOT,
        ),
        CommandSpec(
            "generate the foreign TypeScript package",
            _cargo_run_example(
                "emit_typescript_acceptance",
                layout.foreign_schema,
                layout.node / "generated_phase2_foreign",
            ),
            ROOT,
        ),
        CommandSpec(
            "compile the local TypeScript package",
            (str(tsc), "--project", str(layout.node / "generated_phase2/tsconfig.json")),
            ROOT,
        ),
        CommandSpec(
            "compile the foreign TypeScript package",
            (
                str(tsc),
                "--project",
                str(layout.node / "generated_phase2_foreign/tsconfig.json"),
            ),
            ROOT,
        ),
        CommandSpec(
            "produce the Node report",
            ("node", str(layout.node / NODE_PRODUCER.name)),
            ROOT,
            node_environment,
        ),
        CommandSpec(
            "produce the Rust report",
            (
                "cargo",
                "test",
                "--locked",
                "--manifest-path",
                str(CORE / "Cargo.toml"),
                "-p",
                "type-bridge-schema-codegen",
                "--test",
                "rust_acceptance",
                "generated_rust_phase2_projection_parity_producer",
                "--",
                "--exact",
            ),
            ROOT,
            rust_environment,
        ),
        CommandSpec(
            "build the C shared library",
            (
                "cargo",
                "build",
                "--locked",
                "--manifest-path",
                str(CORE / "Cargo.toml"),
                "-p",
                "type-bridge-c",
                "--lib",
            ),
            ROOT,
        ),
        CommandSpec(
            "produce the C report",
            (
                "cargo",
                "test",
                "--locked",
                "--manifest-path",
                str(CORE / "Cargo.toml"),
                "-p",
                "type-bridge-c",
                "--test",
                "phase2_projection_parity",
                "generated_c17_phase2_parity_producer_is_exact_and_comparator_accepted",
                "--",
                "--exact",
            ),
            ROOT,
            c_environment,
        ),
        CommandSpec(
            "compare all four reports",
            (
                sys.executable,
                str(COMPARATOR),
                *(str(path) for path in layout.report_paths()),
            ),
            ROOT,
        ),
    )


def _require_tools() -> None:
    for executable in ("cargo", "maturin", "npm", "node", "gcc", "clang"):
        if shutil.which(executable) is None:
            raise RunnerError(f"required Phase-2 parity tool is unavailable: {executable}")
    tsc = NODE_PACKAGE / "node_modules/.bin/tsc"
    if not tsc.is_file() or not os.access(tsc, os.X_OK):
        raise RunnerError(
            "required TypeScript compiler is unavailable; run npm ci in "
            "type-bridge-core/crates/node"
        )


def _prepare(layout: Layout) -> None:
    layout.python.mkdir()
    layout.node.mkdir()
    layout.reports.mkdir()
    schema_source = SCHEMA.read_text(encoding="utf-8")
    foreign_source = schema_source.replace(
        FOREIGN_PLAYING_FACT,
        FOREIGN_PLAYING_REPLACEMENT,
        1,
    )
    if foreign_source == schema_source or foreign_source.count(FOREIGN_PLAYING_REPLACEMENT) != 1:
        raise RunnerError("the exact Workforce V3 foreign-package mutation did not apply once")
    layout.foreign_schema.write_text(foreign_source, encoding="utf-8")
    shutil.copy2(PYTHON_PRODUCER, layout.python / PYTHON_PRODUCER.name)
    shutil.copy2(NODE_PRODUCER, layout.node / NODE_PRODUCER.name)
    node_scope = layout.node / "node_modules/@type-bridge"
    node_scope.mkdir(parents=True)
    (node_scope / "node").symlink_to(NODE_PACKAGE, target_is_directory=True)
    for report in layout.report_paths():
        if report.exists() or report.is_symlink():
            raise RunnerError(f"report path unexpectedly exists before production: {report}")


def _run(command: CommandSpec) -> subprocess.CompletedProcess[str]:
    environment = os.environ.copy()
    if command.environment is not None:
        environment.update(command.environment)
    print(f"Phase-2 parity: {command.label}", file=sys.stderr, flush=True)
    try:
        completed = subprocess.run(
            command.arguments,
            cwd=command.cwd,
            env=environment,
            text=True,
            capture_output=True,
            check=False,
        )
    except OSError as error:
        raise RunnerError(f"could not launch {command.label}: {error}") from error
    if completed.returncode != 0:
        raise RunnerError(
            f"{command.label} failed with exit {completed.returncode}\n"
            f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )
    return completed


def run() -> str:
    _require_tools()
    with tempfile.TemporaryDirectory(prefix="typebridge-phase2-parity-") as temporary:
        layout = Layout.under(Path(temporary).resolve())
        _prepare(layout)
        producer_reports = {
            "produce the Python report": layout.python_report,
            "produce the Node report": layout.node_report,
            "produce the Rust report": layout.rust_report,
            "produce the C report": layout.c_report,
        }
        comparison = ""
        for command in command_plan(layout):
            report = producer_reports.get(command.label)
            if report is not None and (report.exists() or report.is_symlink()):
                raise RunnerError(f"report path exists before {command.label}: {report}")
            completed = _run(command)
            if report is not None:
                binding = command.label.removeprefix("produce the ").removesuffix(" report")
                if not report.is_file() or report.is_symlink():
                    raise RunnerError(f"{binding} producer did not publish a regular report")
            if command.label == "compare all four reports":
                comparison = completed.stdout
        if not comparison.endswith("\n"):
            raise RunnerError("the four-binding comparator did not emit a canonical summary line")
        return comparison


def main() -> int:
    try:
        summary = run()
    except RunnerError as error:
        print(f"Phase-2 provider-free parity fan-in failed: {error}", file=sys.stderr)
        return 1
    sys.stdout.write(summary)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
