#!/usr/bin/env python3
"""Validate complete, warning-free rustdoc for every first-party public Cargo package."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

try:
    from cargo_release_inventory import (
        CargoReleaseInventory,
        InventoryError,
        load_inventory,
    )
except ModuleNotFoundError:
    from scripts.ci.cargo_release_inventory import (
        CargoReleaseInventory,
        InventoryError,
        load_inventory,
    )

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
WORKSPACE_MANIFEST = Path("type-bridge-core/Cargo.toml")
LIBRARY_TARGET_KINDS = frozenset({"lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"})
CommandRunner = Callable[..., subprocess.CompletedProcess[str]]


class RustdocValidationError(RuntimeError):
    """The public Cargo rustdoc inventory cannot be validated."""


@dataclass(frozen=True)
class RustdocTarget:
    """One inventory-selected package target that docs.rs must be able to build."""

    package_name: str
    selector: tuple[str, ...]


@dataclass(frozen=True)
class RustdocFailure:
    """One failed rustdoc probe and its captured diagnostic output."""

    package_name: str
    phase: str
    command: tuple[str, ...]
    returncode: int | None
    stdout: str
    stderr: str


def cargo_prefix(*, cargo: str, toolchain: str | None) -> tuple[str, ...]:
    """Return a Cargo command prefix with an optional rustup toolchain override."""
    if not cargo:
        raise RustdocValidationError("Cargo executable cannot be empty")
    if toolchain is None:
        return (cargo,)
    if not toolchain or toolchain != toolchain.strip() or toolchain.startswith("+"):
        raise RustdocValidationError(f"invalid Rust toolchain override: {toolchain!r}")
    return (cargo, f"+{toolchain}")


def cargo_metadata_command(
    *, cargo: str, workspace_manifest: Path, toolchain: str | None = None
) -> tuple[str, ...]:
    """Return the locked, workspace-local Cargo metadata command."""
    return (
        *cargo_prefix(cargo=cargo, toolchain=toolchain),
        "metadata",
        "--locked",
        "--no-deps",
        "--format-version",
        "1",
        "--manifest-path",
        str(workspace_manifest),
    )


def load_workspace_metadata(
    *,
    cargo: str,
    workspace_manifest: Path,
    repository_root: Path,
    toolchain: str | None = None,
    runner: CommandRunner = subprocess.run,
) -> Mapping[str, Any]:
    """Load the exact locked workspace graph used to resolve documentation targets."""
    command = cargo_metadata_command(
        cargo=cargo,
        workspace_manifest=workspace_manifest,
        toolchain=toolchain,
    )
    try:
        completed = runner(
            command,
            check=False,
            capture_output=True,
            text=True,
            cwd=repository_root,
        )
    except OSError as error:
        raise RustdocValidationError(f"could not execute Cargo metadata: {error}") from error
    if completed.returncode != 0:
        diagnostic = completed.stderr.strip() or completed.stdout.strip() or "no diagnostics"
        raise RustdocValidationError(
            f"Cargo metadata failed with exit code {completed.returncode}: {diagnostic}"
        )
    try:
        metadata = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise RustdocValidationError(f"Cargo metadata returned invalid JSON: {error}") from error
    if not isinstance(metadata, dict):
        raise RustdocValidationError("Cargo metadata root must be an object")
    return metadata


def _target_kinds(target: object, *, package_name: str) -> frozenset[str]:
    if not isinstance(target, dict):
        raise RustdocValidationError(f"{package_name} has a non-object Cargo target")
    raw_kinds = target.get("kind")
    if (
        not isinstance(raw_kinds, list)
        or not raw_kinds
        or not all(isinstance(kind, str) and kind for kind in raw_kinds)
    ):
        raise RustdocValidationError(f"{package_name} has a malformed Cargo target kind")
    return frozenset(raw_kinds)


def _target_name(target: object, *, package_name: str) -> str:
    if not isinstance(target, dict):
        raise RustdocValidationError(f"{package_name} has a non-object Cargo target")
    name = target.get("name")
    if not isinstance(name, str) or not name:
        raise RustdocValidationError(f"{package_name} has an unnamed Cargo target")
    return name


def _target_is_documented(target: object, *, package_name: str) -> bool:
    if not isinstance(target, dict):
        raise RustdocValidationError(f"{package_name} has a non-object Cargo target")
    documented = target.get("doc")
    if not isinstance(documented, bool):
        raise RustdocValidationError(f"{package_name} has a malformed Cargo target doc flag")
    return documented


def plan_rustdoc_targets(
    inventory: CargoReleaseInventory,
    metadata: Mapping[str, Any],
    *,
    workspace_root: Path,
) -> tuple[RustdocTarget, ...]:
    """Resolve every first-party inventory entry to one unambiguous Cargo target."""
    raw_packages = metadata.get("packages")
    if not isinstance(raw_packages, list):
        raise RustdocValidationError("Cargo metadata packages must be an array")

    packages: dict[str, Mapping[str, Any]] = {}
    for raw_package in raw_packages:
        if not isinstance(raw_package, dict):
            raise RustdocValidationError("Cargo metadata contains a non-object package")
        name = raw_package.get("name")
        if not isinstance(name, str) or not name:
            raise RustdocValidationError("Cargo metadata contains an unnamed package")
        if name in packages:
            raise RustdocValidationError(f"Cargo metadata contains duplicate package {name!r}")
        packages[name] = raw_package

    planned: list[RustdocTarget] = []
    for package in inventory.first_party_packages:
        if package.docs_target == "none":
            raise RustdocValidationError(
                f"first-party public package {package.name} cannot have docs-target = 'none'"
            )
        actual = packages.get(package.name)
        if actual is None:
            raise RustdocValidationError(
                f"first-party public package {package.name} is missing from Cargo metadata"
            )

        manifest_path = actual.get("manifest_path")
        if not isinstance(manifest_path, str) or not manifest_path:
            raise RustdocValidationError(f"{package.name} has no Cargo manifest path")
        expected_manifest = (workspace_root / package.manifest).resolve()
        if Path(manifest_path).resolve() != expected_manifest:
            raise RustdocValidationError(
                f"{package.name} manifest disagrees with the Cargo inventory: "
                f"expected {expected_manifest}, found {manifest_path}"
            )

        raw_targets = actual.get("targets")
        if not isinstance(raw_targets, list):
            raise RustdocValidationError(f"{package.name} Cargo targets must be an array")
        if package.docs_target == "lib":
            matching = [
                target
                for target in raw_targets
                if _target_kinds(target, package_name=package.name) & LIBRARY_TARGET_KINDS
            ]
            if len(matching) != 1:
                raise RustdocValidationError(
                    f"{package.name} docs-target = 'lib' requires exactly one library target; "
                    f"found {len(matching)}"
                )
            if not _target_is_documented(matching[0], package_name=package.name):
                raise RustdocValidationError(
                    f"{package.name} inventory-selected library target has doc = false"
                )
            planned.append(RustdocTarget(package.name, ("--lib",)))
        elif package.docs_target == "bin":
            matching = [
                target
                for target in raw_targets
                if "bin" in _target_kinds(target, package_name=package.name)
            ]
            if len(matching) != 1:
                raise RustdocValidationError(
                    f"{package.name} docs-target = 'bin' requires exactly one binary target; "
                    f"found {len(matching)}"
                )
            if not _target_is_documented(matching[0], package_name=package.name):
                raise RustdocValidationError(
                    f"{package.name} inventory-selected binary target has doc = false"
                )
            target_name = _target_name(matching[0], package_name=package.name)
            planned.append(RustdocTarget(package.name, ("--bin", target_name)))
        else:
            raise RustdocValidationError(
                f"{package.name} has unsupported docs target {package.docs_target!r}"
            )

    if len(planned) != 17:
        raise RustdocValidationError(
            f"public rustdoc inventory must resolve exactly 17 targets; found {len(planned)}"
        )
    return tuple(planned)


def rustdoc_command(
    target: RustdocTarget,
    *,
    cargo: str,
    workspace_manifest: Path,
    toolchain: str | None = None,
) -> tuple[str, ...]:
    """Return one all-feature rustdoc probe denying warnings and missing API docs."""
    return (
        *cargo_prefix(cargo=cargo, toolchain=toolchain),
        "rustdoc",
        "--locked",
        "--quiet",
        "--all-features",
        "--manifest-path",
        str(workspace_manifest),
        "-p",
        target.package_name,
        *target.selector,
        "--",
        "-D",
        "warnings",
        "-D",
        "missing_docs",
    )


def doctest_command(
    target: RustdocTarget,
    *,
    cargo: str,
    workspace_manifest: Path,
    toolchain: str | None = None,
) -> tuple[str, ...]:
    """Return one all-feature library doctest probe."""
    return (
        *cargo_prefix(cargo=cargo, toolchain=toolchain),
        "test",
        "--locked",
        "--quiet",
        "--all-features",
        "--manifest-path",
        str(workspace_manifest),
        "-p",
        target.package_name,
        "--doc",
    )


def documentation_environment() -> dict[str, str]:
    """Return the deterministic environment shared by strict docs and doctests."""
    environment = os.environ.copy()
    environment.pop("CARGO_ENCODED_RUSTDOCFLAGS", None)
    environment.pop("RUSTDOCFLAGS", None)
    environment.setdefault("PYO3_USE_ABI3_FORWARD_COMPATIBILITY", "1")
    return environment


def _run_probes(
    targets: Sequence[RustdocTarget],
    *,
    phase: str,
    commands: Sequence[tuple[str, ...]],
    repository_root: Path,
    runner: CommandRunner,
) -> tuple[RustdocFailure, ...]:
    if len(commands) != len(targets):
        raise RustdocValidationError(
            f"{phase} command count disagrees with target count: "
            f"commands={len(commands)}, targets={len(targets)}"
        )
    environment = documentation_environment()
    failures: list[RustdocFailure] = []
    for target, command in zip(targets, commands, strict=True):
        try:
            completed = runner(
                command,
                check=False,
                capture_output=True,
                text=True,
                cwd=repository_root,
                env=environment,
            )
        except OSError as error:
            failures.append(
                RustdocFailure(target.package_name, phase, command, None, "", str(error))
            )
            print(f"{phase}-failed {target.package_name}", flush=True)
            continue
        if completed.returncode == 0:
            print(f"{phase}-clean {target.package_name}", flush=True)
            continue
        failures.append(
            RustdocFailure(
                target.package_name,
                phase,
                command,
                completed.returncode,
                completed.stdout,
                completed.stderr,
            )
        )
        print(f"{phase}-failed {target.package_name}", flush=True)
    return tuple(failures)


def run_rustdoc_probes(
    targets: Sequence[RustdocTarget],
    *,
    cargo: str,
    workspace_manifest: Path,
    repository_root: Path,
    toolchain: str | None = None,
    runner: CommandRunner = subprocess.run,
) -> tuple[RustdocFailure, ...]:
    """Run every planned probe, retaining all failures instead of stopping early."""
    commands = tuple(
        rustdoc_command(
            target,
            cargo=cargo,
            workspace_manifest=workspace_manifest,
            toolchain=toolchain,
        )
        for target in targets
    )
    return _run_probes(
        targets,
        phase="rustdoc",
        commands=commands,
        repository_root=repository_root,
        runner=runner,
    )


def run_doctest_probes(
    targets: Sequence[RustdocTarget],
    *,
    cargo: str,
    workspace_manifest: Path,
    repository_root: Path,
    toolchain: str | None = None,
    runner: CommandRunner = subprocess.run,
) -> tuple[RustdocFailure, ...]:
    """Run every first-party library doctest suite independently."""
    commands = tuple(
        doctest_command(
            target,
            cargo=cargo,
            workspace_manifest=workspace_manifest,
            toolchain=toolchain,
        )
        for target in targets
    )
    return _run_probes(
        targets,
        phase="doctest",
        commands=commands,
        repository_root=repository_root,
        runner=runner,
    )


def _print_failures(failures: Sequence[RustdocFailure]) -> None:
    for failure in failures:
        status = "could not execute" if failure.returncode is None else f"exit {failure.returncode}"
        print(
            f"\n[{failure.package_name} {failure.phase}] {status}: {' '.join(failure.command)}",
            file=sys.stderr,
        )
        if failure.stdout.strip():
            print(failure.stdout.rstrip(), file=sys.stderr)
        if failure.stderr.strip():
            print(failure.stderr.rstrip(), file=sys.stderr)


def build_parser() -> argparse.ArgumentParser:
    """Build the closed public-rustdoc validator CLI."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository-root", type=Path, default=REPOSITORY_ROOT)
    parser.add_argument("--cargo", default="cargo")
    parser.add_argument("--toolchain", help="rustup toolchain, such as 1.88.0")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Validate the inventory and every selected public rustdoc target."""
    args = build_parser().parse_args(argv)
    repository_root = args.repository_root.resolve()
    workspace_manifest = (repository_root / WORKSPACE_MANIFEST).resolve()
    try:
        inventory = load_inventory()
    except InventoryError as error:
        raise RustdocValidationError(f"invalid Cargo release inventory: {error}") from error
    metadata = load_workspace_metadata(
        cargo=args.cargo,
        workspace_manifest=workspace_manifest,
        repository_root=repository_root,
        toolchain=args.toolchain,
    )
    targets = plan_rustdoc_targets(
        inventory,
        metadata,
        workspace_root=workspace_manifest.parent,
    )
    failures = run_rustdoc_probes(
        targets,
        cargo=args.cargo,
        workspace_manifest=workspace_manifest,
        repository_root=repository_root,
        toolchain=args.toolchain,
    )
    doctest_failures = run_doctest_probes(
        targets,
        cargo=args.cargo,
        workspace_manifest=workspace_manifest,
        repository_root=repository_root,
        toolchain=args.toolchain,
    )
    failures = (*failures, *doctest_failures)
    if failures:
        _print_failures(failures)
        print(
            f"Cargo documentation validation failed for {len(failures)} probes "
            f"across {len(targets)} targets",
            file=sys.stderr,
        )
        return 1
    print(
        f"Cargo rustdoc and doctest validation passed for all {len(targets)} "
        "first-party public targets"
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RustdocValidationError as error:
        print(f"Cargo documentation validation failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
