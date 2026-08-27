#!/usr/bin/env python3
"""Compile/import one V6 generated surface as an isolated current-SDK consumer."""

from __future__ import annotations

import argparse
import hashlib
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path, PurePosixPath
from typing import Any, NoReturn

import workforce_v6_surfaces as surfaces

ROOT = Path(__file__).resolve().parents[2]
NODE = ROOT / "type-bridge-core/crates/node"
RUST = ROOT / "type-bridge-core/crates/rust"
FORMAT = "typebridge.workforce-v6-surface-consumer/v1"


class ConsumerError(ValueError):
    """Stable generated-surface consumer rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


def reject(code: str, message: str) -> NoReturn:
    raise ConsumerError(code, message)


def run(command: list[str], *, cwd: Path, env: dict[str, str] | None = None) -> None:
    result = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    if result.returncode != 0:
        reject(
            "surface_consumer_failed",
            f"consumer command failed ({' '.join(command)}):\n{result.stdout}",
        )


def extract_surface(archive: Path, destination: Path) -> dict[str, Any]:
    manifest = surfaces.validate_surface(archive, destination.name)
    files = surfaces._archive_files(archive)
    files.pop(PurePosixPath("surface-manifest.json"))
    destination.mkdir()
    for relative, body in files.items():
        path = destination / relative.as_posix()
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(body)
        os.chmod(path, 0o644)
    return manifest


def python_consumer(surface: Path, root: Path) -> list[str]:
    package = root / "python"
    extract_surface(surface, package)
    environment = os.environ.copy()
    environment["PYTHONPATH"] = str(root)
    run(
        [
            sys.executable,
            "-c",
            "import python as generated; "
            "assert generated.PROJECTION_FINGERPRINT_JSON; "
            "assert generated.SEMANTIC_SCHEMA_FINGERPRINT_JSON",
        ],
        cwd=root,
        env=environment,
    )
    run([sys.executable, "-m", "compileall", "-q", str(package)], cwd=root, env=environment)
    return ["public-package-import", "bytecode-compile"]


def node_consumer(surface: Path, root: Path) -> list[str]:
    package = root / "node"
    extract_surface(surface, package)
    installed = package / "node_modules/@type-bridge/node"
    installed.mkdir(parents=True)
    shutil.copy2(NODE / "package.json", installed / "package.json")
    shutil.copytree(NODE / "dist", installed / "dist")
    compiler = NODE / "node_modules/.bin/tsc"
    if not compiler.is_file():
        reject("missing_consumer_tool", "the pinned Node TypeScript compiler is unavailable")
    run([str(compiler), "--project", "tsconfig.json", "--pretty", "false"], cwd=package)
    environment = os.environ.copy()
    environment["TYPE_BRIDGE_NODE_NATIVE_PATH"] = str(NODE / "type_bridge_node.linux-x64-gnu.node")
    run(
        [
            "node",
            "--input-type=module",
            "-e",
            "import('./dist/index.js').then(m=>{if(!m.PROJECTION_FINGERPRINT_JSON)process.exit(2)})",
        ],
        cwd=package,
        env=environment,
    )
    return ["strict-typescript-compile", "public-esm-import"]


def rust_consumer(surface: Path, root: Path) -> list[str]:
    package = root / "rust"
    extract_surface(surface, package)
    config = package / ".cargo/config.toml"
    config.parent.mkdir()
    config.write_text(
        "[patch.crates-io]\n"
        f'type-bridge = {{ path = "{RUST.as_posix()}" }}\n'
        "[net]\noffline = true\n",
        encoding="utf-8",
    )
    environment = os.environ.copy()
    environment["CARGO_TARGET_DIR"] = str(root / "rust-target")
    run(
        [
            "cargo",
            "check",
            "--offline",
            "--manifest-path",
            str(package / "Cargo.toml"),
            "--all-targets",
        ],
        cwd=package,
        env=environment,
    )
    return ["offline-cargo-check", "all-targets"]


def consume(archive: Path, binding: str) -> dict[str, Any]:
    manifest = surfaces.validate_surface(archive, binding)
    with tempfile.TemporaryDirectory(prefix=f"typebridge-v6-{binding}-consumer-") as value:
        temporary = Path(value)
        checks = {
            "python": python_consumer,
            "node": node_consumer,
            "rust": rust_consumer,
        }[binding](archive, temporary)
    return {
        "format": FORMAT,
        "binding": binding,
        "surface-sha256": hashlib.sha256(archive.read_bytes()).hexdigest(),
        "source-commit": manifest["source-commit"],
        "cli-candidate-id": manifest["cli-candidate-id"],
        "runtime-provenance": "current-sdk-regression",
        "checks": checks,
        "cleanup": {"temporary-consumer-absent": True},
        "publication-authority": False,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("--binding", required=True, choices=surfaces.BINDING_DIRECTORIES)
    parser.add_argument("--output", type=Path)
    arguments = parser.parse_args()
    try:
        report = consume(arguments.archive, arguments.binding)
        body = surfaces.canonical_json(report)
        if arguments.output is None:
            print(body.decode())
        else:
            if not arguments.output.is_absolute() or arguments.output.exists():
                reject("invalid_output_path", "consumer report output must be absolute and new")
            arguments.output.write_bytes(body)
    except (ConsumerError, surfaces.SurfaceError, OSError) as error:
        code = getattr(error, "code", "consumer_failure")
        print(f"Workforce V6 surface consumer rejected [{code}]: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
