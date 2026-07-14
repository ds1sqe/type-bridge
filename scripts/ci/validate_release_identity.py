#!/usr/bin/env python3
"""Bind a release tag to every public manifest before any crate publication."""

from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any


class ValidationError(RuntimeError):
    """Release metadata is incomplete or disagrees with the release tag."""


SEMVER_PATTERN = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)"
    r"(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)

# The fork crates preserve their upstream identities instead of inheriting the
# repository release version.  They are still covered by the gate and must be
# present at these exact versions in the publication plan.
VENDORED_PINS = {
    "type-bridge-typedb-driver-b7": "3.8.1",
    "type-bridge-typedb-protocol-b7": "3.7.0",
    "type-bridge-typedb-driver-b9": "3.12.0",
    "type-bridge-typedb-protocol-b9": "3.12.0",
}
PUBLISHED_CRATES = (
    "type-bridge-core-lib",
    "type-bridge-orm-derive",
    "type-bridge-typedb-protocol-b7",
    "type-bridge-typedb-driver-b7",
    "type-bridge-typedb-protocol-b9",
    "type-bridge-typedb-driver-b9",
    "type-bridge-typedb-runtime",
    "type-bridge-orm",
    "type-bridge-server",
)


@dataclass(frozen=True)
class CargoPackage:
    """The release-relevant identity of one workspace package."""

    manifest: Path
    name: str
    publishable: bool
    vendored: bool
    version: str


def read_toml(path: Path, *, label: str) -> dict[str, Any]:
    """Read one regular UTF-8 TOML object."""
    if not path.is_file() or path.is_symlink():
        raise ValidationError(f"{label} is missing or non-regular: {path}")
    try:
        payload = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        raise ValidationError(f"Could not parse {label} {path}: {error}") from error
    return payload


def read_json(path: Path, *, label: str) -> dict[str, Any]:
    """Read one regular UTF-8 JSON object."""
    if not path.is_file() or path.is_symlink():
        raise ValidationError(f"{label} is missing or non-regular: {path}")
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValidationError(f"Could not parse {label} {path}: {error}") from error
    if not isinstance(payload, dict):
        raise ValidationError(f"{label} must contain a JSON object: {path}")
    return payload


def stable_version(value: object, *, label: str) -> str:
    """Return one stable semantic version string."""
    if not isinstance(value, str) or SEMVER_PATTERN.fullmatch(value) is None:
        raise ValidationError(f"{label} is not a semantic version: {value!r}")
    if "-" in value.partition("+")[0]:
        raise ValidationError(f"{label} is a prerelease version: {value!r}")
    return value


def release_version(tag: str) -> str:
    """Return the stable version encoded by a strict ``v<semver>`` tag."""
    if not tag.startswith("v"):
        raise ValidationError(f"Release tag must have the form v<semver>: {tag!r}")
    version = stable_version(tag[1:], label="release tag")
    if tag != f"v{version}":
        raise ValidationError(f"Release tag is not canonical: {tag!r}")
    return version


def cargo_workspace_packages(workspace_manifest: Path) -> tuple[CargoPackage, ...]:
    """Read every explicitly listed Cargo workspace package."""
    workspace_manifest = workspace_manifest.resolve()
    workspace_root = workspace_manifest.parent
    payload = read_toml(workspace_manifest, label="Cargo workspace manifest")
    workspace = payload.get("workspace")
    if not isinstance(workspace, dict) or not isinstance(workspace.get("members"), list):
        raise ValidationError("Cargo workspace manifest must declare a members list")
    members = workspace["members"]
    if not members or not all(isinstance(member, str) and member for member in members):
        raise ValidationError("Cargo workspace members must be non-empty paths")

    packages: list[CargoPackage] = []
    names: set[str] = set()
    for member in members:
        if any(character in member for character in "*?["):
            raise ValidationError(
                f"Cargo workspace member globs are not accepted by the release gate: {member!r}"
            )
        manifest = (workspace_root / member / "Cargo.toml").resolve()
        try:
            relative = manifest.relative_to(workspace_root)
        except ValueError as error:
            raise ValidationError(f"Cargo workspace member escapes its root: {member!r}") from error
        package_payload = read_toml(manifest, label=f"Cargo package {member}").get("package")
        if not isinstance(package_payload, dict):
            raise ValidationError(f"Cargo workspace member has no [package]: {manifest}")
        name = package_payload.get("name")
        if not isinstance(name, str) or not name:
            raise ValidationError(f"Cargo package has no non-empty name: {manifest}")
        if name in names:
            raise ValidationError(f"Duplicate Cargo workspace package name: {name}")
        names.add(name)
        version = stable_version(
            package_payload.get("version"),
            label=f"Cargo package {name} version",
        )
        publish_setting = package_payload.get("publish", True)
        publishable = publish_setting is not False and publish_setting != []
        packages.append(
            CargoPackage(
                manifest=manifest,
                name=name,
                publishable=publishable,
                vendored=relative.parts[0] == "vendor",
                version=version,
            )
        )
    return tuple(packages)


def workflow_publish_sequence(workflow: Path) -> tuple[str, ...]:
    """Return the literal ordered crate arguments passed to the publish helper."""
    if not workflow.is_file() or workflow.is_symlink():
        raise ValidationError(f"Release workflow is missing or non-regular: {workflow}")
    source = workflow.read_text(encoding="utf-8")
    return tuple(re.findall(r'publish-crate\.sh ([A-Za-z0-9_-]+)["\']?', source))


def validate_manifest_version(path: Path, version: str, *, label: str) -> None:
    """Require one PEP 621 project version to match the release."""
    project = read_toml(path.resolve(), label=label).get("project")
    if not isinstance(project, dict):
        raise ValidationError(f"{label} has no [project] table: {path}")
    actual = stable_version(project.get("version"), label=f"{label} version")
    if actual != version:
        raise ValidationError(
            f"{label} version disagrees with release tag: actual={actual!r}, expected={version!r}"
        )


def validate_release_identity(
    *,
    tag: str,
    workspace_manifest: Path,
    root_python_manifest: Path,
    core_python_manifest: Path,
    node_package: Path,
    release_workflow: Path,
) -> dict[str, Any]:
    """Validate all public manifests and the complete Cargo publish plan."""
    version = release_version(tag)
    validate_manifest_version(root_python_manifest, version, label="root Python manifest")
    validate_manifest_version(core_python_manifest, version, label="core Python manifest")
    node_version = stable_version(
        read_json(node_package.resolve(), label="Node package.json").get("version"),
        label="Node package version",
    )
    if node_version != version:
        raise ValidationError(
            "Node package version disagrees with release tag: "
            f"actual={node_version!r}, expected={version!r}"
        )

    packages = cargo_workspace_packages(workspace_manifest)
    by_name = {package.name: package for package in packages}
    for package in packages:
        if package.vendored:
            expected = VENDORED_PINS.get(package.name)
            if expected is None:
                raise ValidationError(
                    f"Unpinned workspace vendor crate: {package.name}@{package.version}"
                )
        else:
            expected = version
        if package.version != expected:
            raise ValidationError(
                f"Cargo package {package.name} version disagrees with its release identity: "
                f"actual={package.version!r}, expected={expected!r}"
            )

    missing_vendors = sorted(set(VENDORED_PINS) - set(by_name))
    if missing_vendors:
        raise ValidationError(
            f"Pinned vendor crates are absent from the workspace: {missing_vendors!r}"
        )
    publishable = frozenset(package.name for package in packages if package.publishable)
    expected_publishable = frozenset(PUBLISHED_CRATES)
    if publishable != expected_publishable:
        missing = sorted(expected_publishable - publishable)
        unexpected = sorted(publishable - expected_publishable)
        raise ValidationError(
            "Cargo publishable package set disagrees with the authoritative release plan: "
            f"missing={missing!r}, unexpected={unexpected!r}"
        )
    actual_publish_sequence = workflow_publish_sequence(release_workflow.resolve())
    if actual_publish_sequence != PUBLISHED_CRATES:
        raise ValidationError(
            "Cargo publish sequence is incomplete or reordered: "
            f"actual={actual_publish_sequence!r}, expected={PUBLISHED_CRATES!r}"
        )
    for name in PUBLISHED_CRATES:
        package = by_name.get(name)
        if package is None or not package.publishable:
            raise ValidationError(f"Published crate is absent or publish=false: {name}")

    return {
        "cargo_packages": {package.name: package.version for package in packages},
        "published_crates": list(PUBLISHED_CRATES),
        "unpublished_crates": sorted(set(by_name) - expected_publishable),
        "status": "ok",
        "tag": tag,
        "version": version,
    }


def build_parser() -> argparse.ArgumentParser:
    """Build the release-identity validator CLI."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--root-python", type=Path, required=True)
    parser.add_argument("--core-python", type=Path, required=True)
    parser.add_argument("--node-package", type=Path, required=True)
    parser.add_argument("--release-workflow", type=Path, required=True)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Run validation and print a machine-readable report."""
    args = build_parser().parse_args(argv)
    report = validate_release_identity(
        tag=args.tag,
        workspace_manifest=args.workspace,
        root_python_manifest=args.root_python,
        core_python_manifest=args.core_python,
        node_package=args.node_package,
        release_workflow=args.release_workflow,
    )
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ValidationError as error:
        print(f"Release identity validation failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
