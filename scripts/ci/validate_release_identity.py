#!/usr/bin/env python3
"""Bind a release tag, license boundary, and package provenance before publication."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import tomllib
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

try:
    from cargo_release_inventory import load_inventory as load_cargo_release_inventory
    from python_release_contract import (
        ContractError as PythonReleaseContractError,
    )
    from python_release_contract import (
        validate_node_package_lockstep,
        validate_python_package_version,
        validate_root_python_manifest_lockstep,
    )
    from validate_latest_typedb_driver_pin import EXPECTED_SERIES as TYPEDB_BAND9_SERIES
except ModuleNotFoundError:
    from scripts.ci.cargo_release_inventory import (
        load_inventory as load_cargo_release_inventory,
    )
    from scripts.ci.python_release_contract import (
        ContractError as PythonReleaseContractError,
    )
    from scripts.ci.python_release_contract import (
        validate_node_package_lockstep,
        validate_python_package_version,
        validate_root_python_manifest_lockstep,
    )
    from scripts.ci.validate_latest_typedb_driver_pin import (
        EXPECTED_SERIES as TYPEDB_BAND9_SERIES,
    )


class ValidationError(RuntimeError):
    """Release metadata is incomplete or disagrees with the release tag."""


class RustPublicationBlockedError(ValidationError):
    """The source identity is valid, but its Rust crates.io graph is not publishable."""

    def __init__(self, message: str, report: dict[str, Any]) -> None:
        super().__init__(message)
        self.report = report


SEMVER_PATTERN = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)"
    r"(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)

CARGO_RELEASE_INVENTORY = load_cargo_release_inventory()
PUBLISHED_CRATES = tuple(package.name for package in CARGO_RELEASE_INVENTORY.public_packages)
UNPUBLISHED_CORE_CRATES: tuple[str, ...] = ()
KNOWN_PUBLICATION_BLOCKER_EDGES: frozenset[tuple[str, str]] = frozenset()
IMMUTABLE_BASELINE_CRATES: tuple[str, ...] = ()
PREEXISTING_CRATES = tuple(package.name for package in CARGO_RELEASE_INVENTORY.immutable_packages)
NEW_COMPATIBILITY_CRATES: tuple[str, ...] = ()
PACKAGED_RELEASE_CRATES = PUBLISHED_CRATES
PACKAGING_PATCH_CRATES = PUBLISHED_CRATES
EXPECTED_NEW_CRATES = tuple(crate for crate in PUBLISHED_CRATES if crate not in PREEXISTING_CRATES)
RELEASE_CRATES_GRAPH = PurePosixPath("scripts/ci/release_crates_graph.sh")
CARGO_CANDIDATE_BUILDER = PurePosixPath("scripts/ci/cargo_release_candidate.py")
CARGO_CANDIDATE_PUBLISHER = PurePosixPath("scripts/ci/publish_cargo_release_candidate.py")
TYPEDB_RUNTIME_PACKAGE = "type-bridge-typedb-runtime"
TYPEDB_BAND9_DEPENDENCY = "typedb-driver"
TARGET_RELEASE_VERSION = CARGO_RELEASE_INVENTORY.release_version
ARTIFACT_CONTRACT_CARGO_INCLUSIVE = "cargo-inclusive"
ARTIFACT_CONTRACT_PYTHON_NPM_ONLY = "python-npm-only"
ARTIFACT_CONTRACT_SOURCE_GIT_SERVER_OCI = "source-git-server-oci"
ARTIFACT_CONTRACTS = (
    ARTIFACT_CONTRACT_CARGO_INCLUSIVE,
    ARTIFACT_CONTRACT_PYTHON_NPM_ONLY,
    ARTIFACT_CONTRACT_SOURCE_GIT_SERVER_OCI,
)
RELEASE_CHANNEL_CANDIDATE = "candidate"
RELEASE_CHANNEL_STABLE = "stable"
RELEASE_CHANNEL_IDENTITIES = {
    # Candidate acceptance verifies the exact stable bytes before the tag exists.
    RELEASE_CHANNEL_CANDIDATE: (TARGET_RELEASE_VERSION, TARGET_RELEASE_VERSION),
    RELEASE_CHANNEL_STABLE: (TARGET_RELEASE_VERSION, TARGET_RELEASE_VERSION),
}
UNPUBLISHED_BINDING_CRATES = tuple(
    package.name for package in CARGO_RELEASE_INVENTORY.private_packages
)
PYTHON_NPM_UNPUBLISHED_CRATES = UNPUBLISHED_CORE_CRATES + UNPUBLISHED_BINDING_CRATES
TYPEDB_RUNTIME_BAND9_PIN_PATTERN = re.compile(
    r'^pub const PINNED_DRIVER_VERSION_B9: &str = "([^"]+)";$',
    re.MULTILINE,
)
CRATES_IO_SOURCE = "registry+https://github.com/rust-lang/crates.io-index"
TYPEDB_BAND9_COMPONENTS = ("typedb-driver", "typedb-protocol")
HISTORICAL_BAND9_PACKAGES = {
    "vendor/typedb-driver-b9": "type-bridge-typedb-driver-b9",
    "vendor/typedb-protocol-b9": "type-bridge-typedb-protocol-b9",
}
FORBIDDEN_BAND9_DEPENDENCY_NAMES = frozenset(
    {
        *HISTORICAL_BAND9_PACKAGES.values(),
        *(Path(relative).name for relative in HISTORICAL_BAND9_PACKAGES),
    }
)
SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")
MIT_LICENSE = "MIT"
APACHE_2_LICENSE = "Apache-2.0"
MPL_2_LICENSE = "MPL-2.0"
ED25519_DALEK_BSD_LICENSE = "ed25519-dalek-2.2.0 BSD-3-Clause"
CURVE25519_DALEK_BSD_LICENSE = "curve25519-dalek-4.1.3 BSD-3-Clause"
NATIVE_NOTICE_BEGIN = "<!-- BEGIN GENERATED RUST DEPENDENCY NOTICE -->"
NATIVE_NOTICE_END = "<!-- END GENERATED RUST DEPENDENCY NOTICE -->"
NATIVE_NOTICE_CARGO_ABOUT_VERSION = "0.9.1"
NATIVE_NOTICE_RUST_TOOLCHAIN = "1.94.1"
TYPEBRIDGE_REPOSITORY = "https://github.com/ds1sqe/type-bridge"
WORKSPACE_LICENSE_FILE = "LICENSE"
VENDOR_LICENSE_FILE = "LICENSE"
VENDOR_README = "README.md"
CANONICAL_LICENSE_DIGESTS = {
    MIT_LICENSE: "9b3a3f225f7e7b2396656019bdff1e9517792c92b0100add4c88ac2c5e07c63f",
    APACHE_2_LICENSE: "a6cba85bc92e0cff7a450b1d873c0eaa2e9fc96bf472df0247a26bec77bf3ff9",
    MPL_2_LICENSE: "3f3d9e0024b1921b067d6f7f88deb4a60cbe7a78e76c64e3f1d7fc3b779b9d04",
    ED25519_DALEK_BSD_LICENSE: ("7b6a19666b1304f2dec9202b0dd2d92ca220558aa23f07d4c5e86dbd271050b9"),
    CURVE25519_DALEK_BSD_LICENSE: (
        "6737ef630c5e038c2c1d1f45e25f00e51e9493dab7fbfb6b4a3a178e76c8187b"
    ),
}


@dataclass(frozen=True)
class WorkspacePathDependency:
    """One non-development path dependency declared by a workspace package."""

    manifest: Path
    name: str
    requirement: str | None


@dataclass(frozen=True)
class CargoPackage:
    """The release-relevant identity of one workspace package."""

    manifest: Path
    name: str
    path_dependencies: tuple[WorkspacePathDependency, ...]
    publish_explicitly_false: bool
    publishable: bool
    vendored: bool
    version: str


@dataclass(frozen=True)
class LockedTypeDbComponent:
    """One official TypeDB package resolved in the native band-9 graph."""

    checksum: str
    name: str
    source: str
    version: str


# Compatibility packages use independent immutable package versions instead of
# inheriting the repository release version. They are still covered by the gate
# and must be present at these exact downstream versions in the publication plan.
HISTORICAL_BAND9_LICENSES = {
    "vendor/typedb-driver-b9": APACHE_2_LICENSE,
    "vendor/typedb-protocol-b9": MPL_2_LICENSE,
}
HISTORICAL_BAND9_DESCRIPTIONS = {
    "vendor/typedb-driver-b9": (
        "Historical quarantined TypeDB driver 3.12.0 snapshot; workspace-excluded and "
        "forbidden for current TypeBridge consumption"
    ),
    "vendor/typedb-protocol-b9": (
        "Historical quarantined TypeDB protocol 3.12.0 snapshot; workspace-excluded and "
        "forbidden for current TypeBridge consumption"
    ),
}
HISTORICAL_BAND9_README_HEADING = "# Historical quarantined snapshot — do not consume"
LEGACY_VENDOR_ROOT_ENTRIES = frozenset({"Cargo.toml", "LICENSE", "README.md", "src"})


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


def read_text(path: Path, *, label: str) -> str:
    """Read one regular UTF-8 text file without accepting a symlink."""
    if not path.is_file() or path.is_symlink():
        raise ValidationError(f"{label} is missing or non-regular: {path}")
    try:
        return path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        raise ValidationError(f"Could not read {label} {path}: {error}") from error


def read_bytes(path: Path, *, label: str) -> bytes:
    """Read one regular binary file without accepting a symlink."""
    if not path.is_file() or path.is_symlink():
        raise ValidationError(f"{label} is missing or non-regular: {path}")
    try:
        return path.read_bytes()
    except OSError as error:
        raise ValidationError(f"Could not read {label} {path}: {error}") from error


def semantic_version(value: object, *, label: str) -> str:
    """Return one canonical semantic version string."""
    if not isinstance(value, str) or SEMVER_PATTERN.fullmatch(value) is None:
        raise ValidationError(f"{label} is not a semantic version: {value!r}")
    return value


def stable_version(value: object, *, label: str) -> str:
    """Return one stable semantic version string."""
    value = semantic_version(value, label=label)
    if "-" in value.partition("+")[0]:
        raise ValidationError(f"{label} is a prerelease version: {value!r}")
    return value


def release_identity_versions(tag: str, release_channel: str) -> tuple[str, str]:
    """Return the exact SemVer and PEP 440 identities authorized for one channel."""
    identity = RELEASE_CHANNEL_IDENTITIES.get(release_channel)
    if identity is None:
        raise ValidationError(
            f"Unknown release channel {release_channel!r}; "
            f"expected one of {sorted(RELEASE_CHANNEL_IDENTITIES)!r}"
        )
    semver_version, python_version = identity
    expected_tag = f"v{semver_version}"
    if tag != expected_tag:
        raise ValidationError(
            "Rust SSOT V2 release identity is not armed for this channel: "
            f"tag={tag!r}, release_channel={release_channel!r}, expected={expected_tag!r}"
        )
    return semver_version, python_version


def resolved_cargo_license(
    package: dict[str, Any],
    *,
    workspace_license: str,
    label: str,
) -> str:
    """Resolve a direct Cargo license or the exact ``license.workspace`` form."""
    declared = package.get("license")
    if isinstance(declared, str):
        return declared
    if declared == {"workspace": True}:
        return workspace_license
    raise ValidationError(
        f"{label} must declare a direct license or license.workspace = true: actual={declared!r}"
    )


def validate_python_manifest_license(path: Path, *, label: str) -> None:
    """Require one PEP 621 project to retain the MIT license identity."""
    project = read_toml(path.resolve(), label=label).get("project")
    if not isinstance(project, dict):
        raise ValidationError(f"{label} has no [project] table: {path}")
    declared = project.get("license")
    if declared not in (MIT_LICENSE, {"text": MIT_LICENSE}):
        raise ValidationError(
            f"{label} license must remain MIT: actual={declared!r}, expected={MIT_LICENSE!r}"
        )


def validate_cargo_license_boundary(
    workspace_manifest: Path,
    packages: Sequence[CargoPackage],
) -> dict[str, str]:
    """Bind first-party and compatibility Cargo packages to their license families."""
    workspace_manifest = workspace_manifest.resolve()
    workspace_root = workspace_manifest.parent
    payload = read_toml(workspace_manifest, label="Cargo workspace manifest")
    workspace = payload.get("workspace")
    if not isinstance(workspace, dict):
        raise ValidationError("Cargo workspace manifest has no [workspace] table")
    workspace_package = workspace.get("package")
    if not isinstance(workspace_package, dict):
        raise ValidationError("Cargo workspace manifest has no [workspace.package] table")
    workspace_license = workspace_package.get("license")
    if workspace_license != MIT_LICENSE:
        raise ValidationError(
            "Cargo workspace license must remain MIT: "
            f"actual={workspace_license!r}, expected={MIT_LICENSE!r}"
        )
    workspace_license_file = workspace_package.get("license-file")
    if workspace_license_file != WORKSPACE_LICENSE_FILE:
        raise ValidationError(
            "Cargo workspace license-file must name the canonical root MIT license: "
            f"actual={workspace_license_file!r}, expected={WORKSPACE_LICENSE_FILE!r}"
        )
    canonical_license_bytes(
        workspace_root / workspace_license_file,
        license_id=MIT_LICENSE,
        label="Cargo workspace MIT license-file",
    )

    resolved: dict[str, str] = {}
    for package in packages:
        payload = read_toml(package.manifest, label=f"Cargo package {package.name}")
        package_table = payload.get("package")
        if not isinstance(package_table, dict):
            raise ValidationError(f"Cargo workspace member has no [package]: {package.manifest}")
        actual = resolved_cargo_license(
            package_table,
            workspace_license=workspace_license,
            label=f"Cargo package {package.name}",
        )
        if actual != MIT_LICENSE:
            raise ValidationError(
                f"Cargo package {package.name} effective license drifted: "
                f"actual={actual!r}, expected={MIT_LICENSE!r}"
            )
        if not package.vendored:
            if package_table.get("license") != {"workspace": True}:
                raise ValidationError(
                    f"Cargo package {package.name} must declare license.workspace = true"
                )
            if package_table.get("license-file") != {"workspace": True}:
                raise ValidationError(
                    f"Cargo package {package.name} must declare license-file.workspace = true"
                )
        resolved[package.name] = actual

    for relative, expected_license in HISTORICAL_BAND9_LICENSES.items():
        manifest = workspace_root / relative / "Cargo.toml"
        payload = read_toml(manifest, label=f"historical band-9 manifest {relative}")
        package = payload.get("package")
        if not isinstance(package, dict):
            raise ValidationError(f"Historical band-9 manifest has no [package]: {manifest}")
        actual = package.get("license")
        if actual != expected_license:
            raise ValidationError(
                f"Historical band-9 package {package.get('name')!r} license drifted: "
                f"actual={actual!r}, expected={expected_license!r}"
            )
        if package.get("license-file") != VENDOR_LICENSE_FILE:
            raise ValidationError(
                f"Historical band-9 package {package.get('name')!r} must use its local LICENSE file"
            )
        canonical_license_bytes(
            manifest.parent / VENDOR_LICENSE_FILE,
            license_id=expected_license,
            label=f"Historical band-9 package {package.get('name')!r} license-file",
        )
        name = package.get("name")
        if isinstance(name, str):
            resolved[name] = expected_license
    return resolved


def normalized_cargo_name(value: object) -> str | None:
    """Normalize Cargo's hyphen/underscore-equivalent dependency spelling."""
    if not isinstance(value, str) or not value:
        return None
    return value.replace("_", "-")


def is_forbidden_band9_name(value: object) -> bool:
    """Return whether a package name or dependency alias denotes a retired b9 fork."""
    normalized = normalized_cargo_name(value)
    return normalized in FORBIDDEN_BAND9_DEPENDENCY_NAMES


def non_dev_dependency_tables(
    manifest: dict[str, Any],
    *,
    label: str,
) -> tuple[tuple[str, dict[str, Any]], ...]:
    """Return normal/build dependency tables, including target-specific tables."""
    tables: list[tuple[str, dict[str, Any]]] = []
    for section_name in ("dependencies", "build-dependencies"):
        section = manifest.get(section_name, {})
        if not isinstance(section, dict):
            raise ValidationError(f"{label} has a non-table [{section_name}] section")
        tables.append((section_name, section))

    targets = manifest.get("target", {})
    if not isinstance(targets, dict):
        raise ValidationError(f"{label} has a non-table [target] section")
    for target_name, target in targets.items():
        if not isinstance(target, dict):
            raise ValidationError(f"{label} has a non-table target {target_name!r}")
        for section_name in ("dependencies", "build-dependencies"):
            section = target.get(section_name, {})
            if not isinstance(section, dict):
                raise ValidationError(
                    f"{label} has a non-table [target.{target_name!r}.{section_name}] section"
                )
            tables.append((f"target.{target_name!r}.{section_name}", section))
    return tuple(tables)


def validate_dependency_table_has_no_historical_band9(
    section: dict[str, Any],
    *,
    manifest: Path,
    section_name: str,
    forbidden_manifests: frozenset[Path],
) -> None:
    """Reject every name, rename, or path that can select a retired b9 fork."""
    for dependency_alias, specification in section.items():
        dependency_package = (
            specification.get("package", dependency_alias)
            if isinstance(specification, dict)
            else dependency_alias
        )
        if is_forbidden_band9_name(dependency_alias) or is_forbidden_band9_name(dependency_package):
            raise ValidationError(
                f"Cargo manifest {manifest} [{section_name}] references a forbidden "
                "historical band-9 dependency name or alias: "
                f"alias={dependency_alias!r}, package={dependency_package!r}"
            )
        if not isinstance(specification, dict):
            continue
        dependency_path = specification.get("path")
        if not isinstance(dependency_path, str) or not dependency_path:
            continue
        dependency_manifest = (manifest.parent / dependency_path / "Cargo.toml").resolve()
        if dependency_manifest in forbidden_manifests:
            raise ValidationError(
                f"Cargo manifest {manifest} [{section_name}] references a forbidden "
                f"historical band-9 path: alias={dependency_alias!r}, path={dependency_path!r}"
            )


def validate_manifest_has_no_historical_band9(
    payload: dict[str, Any],
    *,
    manifest: Path,
    workspace_root: Path,
    label: str,
) -> tuple[tuple[str, dict[str, Any]], ...]:
    """Validate all release-relevant dependency tables in one workspace manifest."""
    tables = non_dev_dependency_tables(payload, label=label)
    forbidden_manifests = frozenset(
        (workspace_root / relative / "Cargo.toml").resolve()
        for relative in HISTORICAL_BAND9_PACKAGES
    )
    for section_name, section in tables:
        validate_dependency_table_has_no_historical_band9(
            section,
            manifest=manifest,
            section_name=section_name,
            forbidden_manifests=forbidden_manifests,
        )
    return tables


def validate_historical_band9_quarantine(workspace_manifest: Path) -> tuple[str, ...]:
    """Prove the retired downstream b9 packages cannot enter a release graph."""
    workspace_manifest = workspace_manifest.resolve()
    workspace_root = workspace_manifest.parent
    payload = read_toml(workspace_manifest, label="Cargo workspace manifest")
    workspace = payload.get("workspace")
    if not isinstance(workspace, dict):
        raise ValidationError("Cargo workspace manifest has no [workspace] table")

    members = workspace.get("members")
    if not isinstance(members, list) or not all(isinstance(member, str) for member in members):
        raise ValidationError("Cargo workspace manifest must declare a members list")
    excludes = workspace.get("exclude")
    if not isinstance(excludes, list) or not all(isinstance(entry, str) for entry in excludes):
        raise ValidationError("Cargo workspace manifest must declare an exclude list")

    member_manifests = {(workspace_root / member / "Cargo.toml").resolve() for member in members}
    required_excludes = set(HISTORICAL_BAND9_PACKAGES)
    missing_excludes = sorted(required_excludes - set(excludes))
    if missing_excludes:
        raise ValidationError(
            "Historical band-9 vendor packages must remain explicitly excluded from the "
            f"workspace: missing={missing_excludes!r}"
        )

    forbidden_manifests = frozenset(
        (workspace_root / relative / "Cargo.toml").resolve()
        for relative in HISTORICAL_BAND9_PACKAGES
    )
    included = sorted(str(path) for path in forbidden_manifests & member_manifests)
    if included:
        raise ValidationError(
            f"Historical band-9 vendor packages must not be workspace members: {included!r}"
        )

    validate_manifest_has_no_historical_band9(
        payload,
        manifest=workspace_manifest,
        workspace_root=workspace_root,
        label="Cargo workspace manifest",
    )
    workspace_dependencies = workspace.get("dependencies", {})
    if not isinstance(workspace_dependencies, dict):
        raise ValidationError("Cargo workspace manifest has a non-table [workspace.dependencies]")
    validate_dependency_table_has_no_historical_band9(
        workspace_dependencies,
        manifest=workspace_manifest,
        section_name="workspace.dependencies",
        forbidden_manifests=forbidden_manifests,
    )

    for relative, expected_name in HISTORICAL_BAND9_PACKAGES.items():
        historical_manifest = workspace_root / relative / "Cargo.toml"
        historical = read_toml(
            historical_manifest,
            label=f"historical band-9 package {expected_name}",
        )
        package = historical.get("package")
        if not isinstance(package, dict):
            raise ValidationError(
                f"Historical band-9 manifest has no [package]: {historical_manifest}"
            )
        if package.get("name") != expected_name:
            raise ValidationError(
                "Historical band-9 package identity drifted: "
                f"actual={package.get('name')!r}, expected={expected_name!r}"
            )
        if package.get("publish") is not False:
            raise ValidationError(
                f"Historical band-9 package must remain publish=false: {expected_name}"
            )
        expected_license = HISTORICAL_BAND9_LICENSES[relative]
        if package.get("license") != expected_license:
            raise ValidationError(
                f"Historical band-9 package {expected_name} license drifted: "
                f"actual={package.get('license')!r}, expected={expected_license!r}"
            )
        expected_description = HISTORICAL_BAND9_DESCRIPTIONS[relative]
        if package.get("description") != expected_description:
            raise ValidationError(
                f"Historical band-9 package description must retain its quarantine warning: "
                f"actual={package.get('description')!r}, expected={expected_description!r}"
            )
        if package.get("readme") != "README.md":
            raise ValidationError(
                f"Historical band-9 package must retain its quarantine README: {expected_name}"
            )
        readme = read_text(
            historical_manifest.parent / "README.md",
            label=f"historical band-9 README {expected_name}",
        )
        if not readme.startswith(f"{HISTORICAL_BAND9_README_HEADING}\n\n"):
            raise ValidationError(
                f"Historical band-9 README must begin with the quarantine warning: {expected_name}"
            )
        compact_readme = " ".join(readme.split())
        for phrase in (
            "publish = false",
            "forbidden for TypeBridge consumption",
            "official upstream crates.io packages",
            "not a current/2.0 release input",
            "type-bridge-typedb-driver-b9@3.12.0",
            "type-bridge-typedb-protocol-b9@3.12.0",
            "must remain non-yanked for released 1.5.x compatibility",
            "must never republish or yank them",
            "TypeDB remains the original upstream owner",
            package.get("license"),
        ):
            if not isinstance(phrase, str) or phrase not in compact_readme:
                raise ValidationError(
                    f"Historical band-9 README {expected_name} is missing: {phrase!r}"
                )

    lock_path = workspace_root / "Cargo.lock"
    lock = read_toml(lock_path, label="Cargo lockfile")
    rows = lock.get("package")
    if not isinstance(rows, list) or not all(isinstance(row, dict) for row in rows):
        raise ValidationError("Cargo lockfile must contain a package array")
    for row in rows:
        if is_forbidden_band9_name(row.get("name")):
            raise ValidationError(
                "Cargo lockfile contains a forbidden historical band-9 package: "
                f"{row.get('name')!r}"
            )
        dependencies = row.get("dependencies", [])
        if not isinstance(dependencies, list) or not all(
            isinstance(dependency, str) for dependency in dependencies
        ):
            raise ValidationError(
                f"Cargo lockfile package {row.get('name')!r} has an invalid dependency array"
            )
        for dependency in dependencies:
            dependency_name = dependency.partition(" ")[0]
            if is_forbidden_band9_name(dependency_name):
                raise ValidationError(
                    "Cargo lockfile contains a forbidden historical band-9 dependency: "
                    f"owner={row.get('name')!r}, dependency={dependency!r}"
                )
    return tuple(HISTORICAL_BAND9_PACKAGES.values())


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
        manifest_payload = read_toml(manifest, label=f"Cargo package {member}")
        package_payload = manifest_payload.get("package")
        if not isinstance(package_payload, dict):
            raise ValidationError(f"Cargo workspace member has no [package]: {manifest}")
        name = package_payload.get("name")
        if not isinstance(name, str) or not name:
            raise ValidationError(f"Cargo package has no non-empty name: {manifest}")
        if name in names:
            raise ValidationError(f"Duplicate Cargo workspace package name: {name}")
        if is_forbidden_band9_name(name):
            raise ValidationError(
                f"Forbidden historical band-9 package is a workspace member: {name}"
            )
        names.add(name)
        version = semantic_version(
            package_payload.get("version"),
            label=f"Cargo package {name} version",
        )
        publish_setting = package_payload.get("publish", True)
        publishable = publish_setting is not False and publish_setting != []
        path_dependencies: list[WorkspacePathDependency] = []
        dependency_tables = validate_manifest_has_no_historical_band9(
            manifest_payload,
            manifest=manifest,
            workspace_root=workspace_root,
            label=f"Cargo package {name}",
        )
        for section_name, section in dependency_tables:
            for dependency_name, specification in section.items():
                if not isinstance(specification, dict) or "path" not in specification:
                    continue
                dependency_path = specification.get("path")
                if not isinstance(dependency_path, str) or not dependency_path:
                    raise ValidationError(
                        f"Cargo package {name} has an invalid path dependency: {dependency_name}"
                    )
                dependency_manifest = (manifest.parent / dependency_path / "Cargo.toml").resolve()
                try:
                    dependency_manifest.relative_to(workspace_root)
                except ValueError as error:
                    raise ValidationError(
                        f"Cargo package {name} path dependency escapes the workspace: "
                        f"{dependency_name}"
                    ) from error
                dependency_package = specification.get("package", dependency_name)
                if not isinstance(dependency_package, str) or not dependency_package:
                    raise ValidationError(
                        f"Cargo package {name} has an invalid package rename: {dependency_name}"
                    )
                requirement = specification.get("version")
                if requirement is not None and not isinstance(requirement, str):
                    raise ValidationError(
                        f"Cargo package {name} has an invalid path dependency version: "
                        f"{dependency_package}"
                    )
                path_dependencies.append(
                    WorkspacePathDependency(
                        manifest=dependency_manifest,
                        name=dependency_package,
                        requirement=requirement,
                    )
                )
        packages.append(
            CargoPackage(
                manifest=manifest,
                name=name,
                path_dependencies=tuple(path_dependencies),
                publish_explicitly_false=publish_setting is False,
                publishable=publishable,
                vendored=relative.parts[0] == "vendor",
                version=version,
            )
        )
    return tuple(packages)


def _workflow_release_graph_source(workflow: Path) -> str:
    """Return the exact-candidate publisher bound to the ordinary workflow."""
    if not workflow.is_file() or workflow.is_symlink():
        raise ValidationError(f"Release workflow is missing or non-regular: {workflow}")
    source = workflow.read_text(encoding="utf-8")
    for mode in ("preflight", "publish"):
        command = f"python {CARGO_CANDIDATE_PUBLISHER.as_posix()} {mode}"
        if source.count(command) != 1:
            raise ValidationError(
                f"Release workflow must invoke the exact Cargo candidate exactly once with {mode}"
            )
    publisher = workflow.resolve().parents[2] / CARGO_CANDIDATE_PUBLISHER
    if not publisher.is_file() or publisher.is_symlink():
        raise ValidationError(f"Cargo candidate publisher is missing or non-regular: {publisher}")
    return publisher.read_text(encoding="utf-8")


def _bash_crate_array(source: str, name: str) -> tuple[str, ...]:
    """Return one strict, literal Bash array of crate names."""
    matches = tuple(
        re.finditer(
            rf"(?m)^{re.escape(name)}=\(\n(?P<body>(?:  [A-Za-z0-9_-]+\n)+)\)$",
            source,
        )
    )
    if len(matches) != 1:
        raise ValidationError(f"Cargo release graph must define exactly one {name} array")
    return tuple(line.strip() for line in matches[0].group("body").splitlines())


def workflow_publish_sequence(workflow: Path) -> tuple[str, ...]:
    """Return the authoritative ordered crate publication sequence."""
    source = _workflow_release_graph_source(workflow)
    markers = (
        "for candidate in bundle.packages:",
        "if candidate.package.immutable:",
        "upload_exact_request(",
        "settle_registry_state(",
    )
    if any(source.count(marker) < 1 for marker in markers):
        raise ValidationError("exact Cargo candidate publication loop is malformed")
    if "cargo publish" in source or "cargo package" in source:
        raise ValidationError("exact Cargo candidate publisher must not invoke Cargo packaging")
    return EXPECTED_NEW_CRATES


def workflow_preflight_sequences(workflow: Path) -> tuple[tuple[str, ...], tuple[str, ...]]:
    """Return the patch-free staged candidate package sequence."""
    _workflow_release_graph_source(workflow)
    workflow_source = workflow.read_text(encoding="utf-8")
    build_marker = f"python {CARGO_CANDIDATE_BUILDER.as_posix()} build"
    if workflow_source.count(build_marker) != 1:
        raise ValidationError("release workflow must build one exact Cargo candidate")
    builder = workflow.resolve().parents[2] / CARGO_CANDIDATE_BUILDER
    if not builder.is_file() or builder.is_symlink():
        raise ValidationError(f"Cargo candidate builder is missing or non-regular: {builder}")
    source = builder.read_text(encoding="utf-8")
    required = (
        '"vendor", "--locked", "--versioned-dirs"',
        '"package",',
        "for package in inventory.public_packages:",
        "stage_archive(",
        "_validate_packaged_lock(",
    )
    if any(source.count(marker) < 1 for marker in required):
        raise ValidationError("Cargo candidate staged package loop is malformed")
    if "patch.crates-io" in source:
        raise ValidationError("Cargo candidate builder contains a crates.io patch override")
    return (), PACKAGED_RELEASE_CRATES


def workflow_registry_preflight_sequences(
    workflow: Path,
) -> tuple[tuple[str, ...], tuple[str, ...]]:
    """Return immutable inputs and candidate keys bound by the exact flight."""
    _workflow_release_graph_source(workflow)
    source = workflow.read_text(encoding="utf-8")
    preflight = f"python {CARGO_CANDIDATE_PUBLISHER.as_posix()} preflight"
    upload = "uses: actions/upload-artifact@"
    download = "uses: actions/download-artifact@"
    bundle = "cargo-release-candidate"
    if source.count(preflight) != 1:
        raise ValidationError("Cargo candidate registry preflight is malformed")
    if source.count(upload) < 1 or source.count(download) < 1 or source.count(bundle) < 8:
        raise ValidationError("Cargo candidate artifact handoff is incomplete")
    return PREEXISTING_CRATES, EXPECTED_NEW_CRATES


def validate_native_notice_workflow(workflow: Path) -> None:
    """Require the pinned dependency-notice freshness gate before identity validation."""
    if not workflow.is_file() or workflow.is_symlink():
        raise ValidationError(f"Release workflow is missing or non-regular: {workflow}")
    source = workflow.read_text(encoding="utf-8")
    install_marker = f"tool: cargo-about@{NATIVE_NOTICE_CARGO_ABOUT_VERSION}"
    check_marker = "python scripts/ci/generate_native_dependency_notice.py --check"
    rustdoc_marker = "python scripts/ci/validate_cargo_rustdoc.py"
    cargo_graph_marker = f"python {CARGO_CANDIDATE_BUILDER.as_posix()} build"
    rust_artifact_marker = "python scripts/ci/validate_rust_release_artifacts.py"
    identity_marker = "python scripts/ci/validate_release_identity.py"
    if source.count(install_marker) != 1:
        raise ValidationError(
            "Release workflow must install exactly one pinned cargo-about "
            f"{NATIVE_NOTICE_CARGO_ABOUT_VERSION}"
        )
    if source.count(check_marker) != 1:
        raise ValidationError(
            "Release workflow must run exactly one native dependency-notice freshness gate"
        )
    if source.count(rustdoc_marker) != 1:
        raise ValidationError("Release workflow must run exactly one public Cargo rustdoc gate")
    if source.count(cargo_graph_marker) != 1:
        raise ValidationError("Release workflow must package the Cargo graph exactly once")
    if source.count(rust_artifact_marker) != 1:
        raise ValidationError("Release workflow must run exactly one Rust archive-content gate")
    if source.count(identity_marker) != 1:
        raise ValidationError("Release workflow identity-validator marker is malformed")
    if (
        not source.index(install_marker)
        < source.index(check_marker)
        < source.index(rustdoc_marker)
        < source.index(cargo_graph_marker)
        < source.index(rust_artifact_marker)
        < source.index(identity_marker)
    ):
        raise ValidationError(
            "Release workflow must check notices and rustdoc, package the Cargo graph, and "
            "validate exact archives before release identity validation"
        )
    if f"toolchain: {NATIVE_NOTICE_RUST_TOOLCHAIN}" not in source[: source.index(check_marker)]:
        raise ValidationError(
            "Release workflow must pin the native notice gate to Rust "
            f"{NATIVE_NOTICE_RUST_TOOLCHAIN}"
        )


def validate_python_npm_only_workflow(workflow: Path) -> None:
    """Require the Python/npm release lane to contain no crates.io artifact path."""
    if not workflow.is_file() or workflow.is_symlink():
        raise ValidationError(f"Release workflow is missing or non-regular: {workflow}")
    source = workflow.read_text(encoding="utf-8")
    forbidden_markers = {
        "CARGO_REGISTRY_TOKEN": "crates.io credential",
        "--verify-preexisting": "crates.io checksum preflight",
        "cargo package": "Cargo package artifact",
        "cargo publish": "Cargo publication command",
        "patch.crates-io": "crates.io package patch",
        "publish-crates": "Cargo publication job or dependency",
        "publish_crate_idempotently.sh": "Cargo publication helper",
        "cargo_release_candidate.py": "Cargo release candidate builder",
        "publish_cargo_release_candidate.py": "exact Cargo publication helper",
        "cargo-release-candidate": "exact Cargo candidate artifact",
        "rust-crates": "Rust crate artifact upload",
        "target/package": "Cargo archive directory",
        "validate_fresh_typedb_runtime_package.sh": "published-Cargo consumer probe",
        "validate_rust_release_artifacts.py": "Rust release artifact validator",
    }
    present = [description for marker, description in forbidden_markers.items() if marker in source]
    if present:
        raise ValidationError(
            "Python/npm-only release workflow contains forbidden crates.io paths: "
            f"{sorted(present)!r}"
        )


def validate_source_git_server_oci_workflow(workflow: Path) -> None:
    """Require the final source/Git Rust SDK plus server-OCI artifact graph."""
    validate_python_npm_only_workflow(workflow)
    source = workflow.read_text(encoding="utf-8")
    required_markers = {
        "build-server-oci": "server OCI build job",
        "accept-server-oci": "exact server OCI acceptance job",
        "publish-server-oci": "stable-only server OCI publication job",
        "ghcr.io/ds1sqe/type-bridge-server": "public server image identity",
        "type=oci,dest=": "immutable OCI-layout output",
        "scripts/ci/run_pinned_skopeo.py": "pinned Skopeo runtime",
        "copy --preserve-digests": "accepted-byte registry import",
        "TYPE_BRIDGE_SERVER_IMAGE": "exact-image V1/V2 runtime acceptance",
        "linux/amd64": "amd64 platform",
        "linux/arm64": "arm64 platform",
        "cosign sign --yes": "keyless image signature",
        "actions/attest-build-provenance": "registry provenance attestation",
        "actions/attest-sbom": "registry SBOM attestation",
    }
    missing = [
        description for marker, description in required_markers.items() if marker not in source
    ]
    if missing:
        raise ValidationError(
            f"Source/Git plus server-OCI workflow is incomplete: {sorted(missing)!r}"
        )


def _release_workflow_job(source: str, name: str) -> str:
    """Return one active top-level release job from workflow source."""
    jobs_marker = "\njobs:\n"
    if source.count(jobs_marker) != 1:
        raise ValidationError("Release workflow jobs mapping is malformed")
    jobs = source.split(jobs_marker, maxsplit=1)[1]
    header = re.search(rf"^  {re.escape(name)}:\n", jobs, re.MULTILINE)
    if header is None:
        raise ValidationError(f"Release workflow is missing job {name!r}")
    next_header = re.search(r"^  [a-z][a-z0-9-]*:\n", jobs[header.end() :], re.MULTILINE)
    end = header.end() + next_header.start() if next_header is not None else len(jobs)
    return jobs[header.start() : end]


def _release_workflow_step(job: str, name: str) -> str:
    """Return one active named step from a top-level release job."""
    marker = f"      - name: {name}\n"
    if job.count(marker) != 1:
        raise ValidationError(f"Release workflow step {name!r} is missing or duplicated")
    start = job.index(marker)
    next_step = re.search(r"^      - name: .+$", job[start + len(marker) :], re.MULTILINE)
    end = start + len(marker) + next_step.start() if next_step is not None else len(job)
    return job[start:end]


def validate_server_oci_release_channels(workflow: Path) -> None:
    """Bind tag preflight, publishers, OCI aliases, and signatures to exact channels."""
    if not workflow.is_file() or workflow.is_symlink():
        raise ValidationError(f"Release workflow is missing or non-regular: {workflow}")
    source = workflow.read_text(encoding="utf-8")
    preamble = source.split("\njobs:\n", maxsplit=1)[0]
    alias_selector = "  SERVER_OCI_MINOR_ALIAS: '2.3'"
    if preamble.count(alias_selector) != 1:
        raise ValidationError("Release workflow has no exact OCI minor-alias selector")

    tag_preflight = _release_workflow_job(source, "release-tag-preflight")
    freeze_tag = _release_workflow_step(tag_preflight, "Freeze exact annotated tag object")
    tag_requirements = {
        "needs: [channel-preflight]": "stable channel preflight",
        "needs.channel-preflight.result == 'success'": "stable channel acceptance",
        "outputs:\n      tag_object: ${{ steps.freeze-tag.outputs.tag_object }}": (
            "frozen tag-object job output"
        ),
    }
    malformed = [label for marker, label in tag_requirements.items() if marker not in tag_preflight]
    freeze_requirements = {
        'test "$(jq -r \'.object.type\' <<<"$tag_ref_json")" = tag': ("annotated-tag requirement"),
        'test "$(jq -r \'.object.type\' <<<"$tag_json")" = commit': ("tag target kind"),
        'test "$(jq -r \'.object.sha\' <<<"$tag_json")" = "$RELEASE_REVISION"': (
            "tag target revision"
        ),
        'printf \'tag_object=%s\\n\' "$tag_object" >> "$GITHUB_OUTPUT"': (
            "frozen tag-object output"
        ),
    }
    malformed.extend(
        label for marker, label in freeze_requirements.items() if freeze_tag.count(marker) != 1
    )

    mutating_jobs = (
        "publish-crates",
        "publish-server-oci",
        "publish-node-npm",
        "publish-core-pypi",
        "publish-python-pypi",
        "github-release",
    )
    for name in mutating_jobs:
        job = _release_workflow_job(source, name)
        needs_match = re.search(r"^    needs: .+$", job, re.MULTILINE)
        needs = needs_match.group(0) if needs_match is not None else ""
        if "release-tag-preflight" not in needs:
            malformed.append(f"{name} frozen-tag dependency")
        if "needs.release-tag-preflight.result == 'success'" not in job:
            malformed.append(f"{name} frozen-tag success guard")
        step_name = (
            "Revalidate frozen release tag"
            if name == "publish-crates"
            else "Revalidate immutable release tag"
        )
        step = _release_workflow_step(job, step_name)
        for marker, label in {
            "EXPECTED_RELEASE_TAG_OBJECT: "
            "${{ needs.release-tag-preflight.outputs.tag_object }}": "expected tag object",
            'test "$tag_object" = "$EXPECTED_RELEASE_TAG_OBJECT"': "tag-object equality",
            'test "$(jq -r \'.object.sha\' <<<"$tag_json")" = "$RELEASE_REVISION"': (
                "tag target revision"
            ),
        }.items():
            if step.count(marker) != 1:
                malformed.append(f"{name} {label}")

    crates = _release_workflow_job(source, "publish-crates")
    crates_needs_match = re.search(r"^    needs: .+$", crates, re.MULTILINE)
    crates_needs = crates_needs_match.group(0) if crates_needs_match is not None else ""
    if "publish-node-npm" not in crates_needs:
        malformed.append("npm-first Cargo publication dependency")
    if "needs.publish-node-npm.result == 'success'" not in crates:
        malformed.append("npm-first Cargo publication guard")

    server = _release_workflow_job(source, "publish-server-oci")
    alias_step = _release_workflow_step(server, "Create exact stable manifest and aliases")
    metadata_step = _release_workflow_step(server, "Record immutable OCI release identity")
    signature_step = _release_workflow_step(
        server, "Sign exact platform and multi-platform digests"
    )
    scoped_requirements = (
        (alias_step, 'for alias in "$SERVER_OCI_MINOR_ALIAS" 2 latest; do', "alias loop"),
        (
            metadata_step,
            '"aliases": [os.environ["SERVER_OCI_MINOR_ALIAS"], "2", "latest"],',
            "release metadata aliases",
        ),
        (metadata_step, 'alias_markdown = ", ".join(', "release-note aliases"),
        (
            signature_step,
            "COSIGN_CERTIFICATE_IDENTITY: 'https://github.com/${{ github.repository }}/.github/workflows/"
            "release.yml@refs/tags/${{ env.RELEASE_TAG }}'",
            "closed stable Cosign identities",
        ),
    )
    malformed.extend(
        label for block, marker, label in scoped_requirements if block.count(marker) != 1
    )
    if "--certificate-identity-regexp" in signature_step:
        malformed.append("non-exact Cosign identities")
    if '"$COSIGN_CERTIFICATE_IDENTITY"' not in signature_step:
        malformed.append("missing Cosign identities")
    stale_stable_markers = (
        "for alias in 2.0 2 latest; do",
        '"aliases": ["2.0", "2", "latest"],',
        "release.yml@refs/tags/v2[.]0[.]0$",
    )
    if any(marker in server for marker in stale_stable_markers):
        malformed.append("stale v2.0 stable OCI identity")
    if malformed:
        raise ValidationError(
            "Release workflow has malformed release/OCI channel identities: "
            f"{sorted(set(malformed))!r}"
        )


def validate_manifest_version(path: Path, version: str, *, label: str) -> None:
    """Require one PEP 621 project version to match the release."""
    project = read_toml(path.resolve(), label=label).get("project")
    if not isinstance(project, dict):
        raise ValidationError(f"{label} has no [project] table: {path}")
    actual = project.get("version")
    if not isinstance(actual, str) or not actual:
        raise ValidationError(f"{label} has no non-empty project version: {path}")
    if actual != version:
        raise ValidationError(
            f"{label} version disagrees with release identity: "
            f"actual={actual!r}, expected={version!r}"
        )


def validate_typedb_runtime_driver_pins(package: CargoPackage) -> str:
    """Bind the retained driver requirement and compatibility alias to its pin."""
    manifest = read_toml(package.manifest, label="TypeDB runtime Cargo manifest")
    dependencies = manifest.get("dependencies")
    if not isinstance(dependencies, dict):
        raise ValidationError("TypeDB runtime Cargo manifest has no [dependencies] table")
    if any(name.startswith("type-bridge-typedb-") for name in dependencies):
        raise ValidationError("TypeDB runtime must not depend on namespaced driver forks")
    features = manifest.get("features")
    if not isinstance(features, dict) or set(features) != {"default", "band9"}:
        raise ValidationError("TypeDB runtime must declare only the retained band feature")
    specification = dependencies.get(TYPEDB_BAND9_DEPENDENCY)
    if not isinstance(specification, dict):
        raise ValidationError("TypeDB runtime must declare typedb-driver as a dependency table")
    if specification.get("package", TYPEDB_BAND9_DEPENDENCY) != TYPEDB_BAND9_DEPENDENCY:
        raise ValidationError(
            "TypeDB runtime band-9 upstream dependency has the wrong package identity"
        )
    source = package.manifest.parent / "src/lib.rs"
    source_text = read_text(source, label="TypeDB runtime source")
    matches = TYPEDB_RUNTIME_BAND9_PIN_PATTERN.findall(source_text)
    if len(matches) != 1:
        raise ValidationError(
            "TypeDB runtime source must define the band-9 upstream pin exactly once"
        )
    pinned_version = stable_version(matches[0], label="TypeDB runtime pinned driver version")
    expected_requirement = f"={pinned_version}"
    requirement = specification.get("version")
    if requirement != expected_requirement:
        raise ValidationError(
            "TypeDB runtime typedb-driver dependency must exactly match its runtime pin: "
            f"actual={requirement!r}, expected={expected_requirement!r}"
        )
    if source_text.count("pub const PINNED_DRIVER_VERSION: &str = PINNED_DRIVER_VERSION_B9;") != 1:
        raise ValidationError("TypeDB runtime compatibility pin must alias its retained driver pin")
    return pinned_version


def resolved_official_band9_components(
    workspace_manifest: Path,
    *,
    expected_driver_version: str,
) -> tuple[LockedTypeDbComponent, LockedTypeDbComponent]:
    """Resolve the official driver and its protocol edge from ``Cargo.lock``."""
    lock_path = workspace_manifest.resolve().parent / "Cargo.lock"
    payload = read_toml(lock_path, label="Cargo lockfile")
    rows = payload.get("package")
    if not isinstance(rows, list) or not all(isinstance(row, dict) for row in rows):
        raise ValidationError("Cargo lockfile must contain a package array")

    def resolve(name: str) -> tuple[LockedTypeDbComponent, dict[str, Any]]:
        matches = [row for row in rows if row.get("name") == name]
        if len(matches) != 1:
            raise ValidationError(
                f"Cargo lockfile must resolve exactly one official band-9 {name}: "
                f"found={len(matches)}"
            )
        row = matches[0]
        version = stable_version(row.get("version"), label=f"Cargo lockfile {name} version")
        major, minor = TYPEDB_BAND9_SERIES
        if re.fullmatch(rf"{major}\.{minor}\.(?:0|[1-9]\d*)", version) is None:
            raise ValidationError(f"Cargo lockfile {name} is outside official band 9: {version!r}")
        source = row.get("source")
        if source != CRATES_IO_SOURCE:
            raise ValidationError(
                f"Cargo lockfile {name} must resolve from official crates.io: "
                f"actual={source!r}, expected={CRATES_IO_SOURCE!r}"
            )
        checksum = row.get("checksum")
        if not isinstance(checksum, str) or SHA256_PATTERN.fullmatch(checksum) is None:
            raise ValidationError(
                f"Cargo lockfile {name} has no canonical crates.io SHA-256 checksum"
            )
        return (
            LockedTypeDbComponent(
                checksum=checksum,
                name=name,
                source=source,
                version=version,
            ),
            row,
        )

    driver, driver_row = resolve(TYPEDB_BAND9_COMPONENTS[0])
    protocol, _ = resolve(TYPEDB_BAND9_COMPONENTS[1])
    if driver.version != expected_driver_version:
        raise ValidationError(
            "Cargo lockfile band-9 driver disagrees with the runtime pin: "
            f"actual={driver.version!r}, expected={expected_driver_version!r}"
        )

    dependencies = driver_row.get("dependencies")
    if not isinstance(dependencies, list) or not all(
        isinstance(dependency, str) for dependency in dependencies
    ):
        raise ValidationError("Cargo lockfile typedb-driver has no dependency identity array")
    protocol_edges = [
        dependency
        for dependency in dependencies
        if dependency == protocol.name or dependency.startswith(f"{protocol.name} ")
    ]
    expected_edges = {
        protocol.name,
        f"{protocol.name} {protocol.version}",
        f"{protocol.name} {protocol.version} ({protocol.source})",
    }
    if len(protocol_edges) != 1 or protocol_edges[0] not in expected_edges:
        raise ValidationError(
            "Cargo lockfile typedb-driver must resolve exactly the audited official "
            f"typedb-protocol package: actual={protocol_edges!r}"
        )
    return driver, protocol


def canonical_license_bytes(path: Path, *, license_id: str, label: str) -> bytes:
    """Read one canonical license body and reject any byte-level drift."""
    body = read_bytes(path, label=label)
    actual_digest = hashlib.sha256(body).hexdigest()
    expected_digest = CANONICAL_LICENSE_DIGESTS[license_id]
    if actual_digest != expected_digest:
        raise ValidationError(
            f"{label} is not the canonical {license_id} body: "
            f"actual_sha256={actual_digest!r}, expected_sha256={expected_digest!r}"
        )
    return body


def embedded_notice_license(
    notice: bytes,
    *,
    heading: str,
    next_heading: str | None,
) -> bytes:
    """Extract one license body whose Markdown heading is itself unambiguous."""
    marker = f"{heading}\n\n".encode()
    if notice.count(marker) != 1:
        raise ValidationError(f"Native third-party notice must contain exactly one {heading!r}")
    body = notice.split(marker, maxsplit=1)[1]
    if next_heading is None:
        return body
    delimiter = f"\n{next_heading}\n".encode()
    if body.count(delimiter) != 1:
        raise ValidationError(
            f"Native third-party notice must delimit {heading!r} with {next_heading!r}"
        )
    return body.split(delimiter, maxsplit=1)[0]


def validate_native_license_bodies(
    *,
    workspace_root: Path,
    root_license: Path,
    notice: bytes,
) -> None:
    """Bind both native notices to every canonical incorporated license text."""
    mit = canonical_license_bytes(
        root_license,
        license_id=MIT_LICENSE,
        label="root MIT LICENSE",
    )

    headings = (
        "## TypeBridge-authored portions — MIT License",
        "## TypeDB driver portions — Apache License 2.0",
        "## TypeDB protocol portions — Mozilla Public License 2.0",
        "## ed25519-dalek 2.2.0 — BSD 3-Clause License",
        "## curve25519-dalek 4.1.3 — BSD 3-Clause License",
    )
    expected_bodies = (mit, None, None, None, None)
    embedded_license_ids = (
        MIT_LICENSE,
        APACHE_2_LICENSE,
        MPL_2_LICENSE,
        ED25519_DALEK_BSD_LICENSE,
        CURVE25519_DALEK_BSD_LICENSE,
    )
    next_headings = (*headings[1:], NATIVE_NOTICE_BEGIN)
    for index, (heading, expected, license_id) in enumerate(
        zip(headings, expected_bodies, embedded_license_ids, strict=True)
    ):
        actual = embedded_notice_license(
            notice,
            heading=heading,
            next_heading=next_headings[index],
        )
        if expected is not None and actual != expected:
            raise ValidationError(
                f"Native third-party notice {heading!r} body must match its canonical LICENSE"
            )
        if (
            expected is None
            and hashlib.sha256(actual).hexdigest() != CANONICAL_LICENSE_DIGESTS[license_id]
        ):
            raise ValidationError(
                f"Native third-party notice {heading!r} body must match its canonical LICENSE"
            )


def validate_native_crypto_notice_provenance(workspace_root: Path, notice: str) -> None:
    """Bind reply-authentication notices to the exact resolved dalek versions."""
    expected = {
        "ed25519-dalek": (
            "2.2.0",
            "BSD-3-Clause; exact upstream license text reproduced below",
        ),
        "curve25519-dalek": (
            "4.1.3",
            "BSD-3-Clause; exact upstream license text, including the original "
            "Go-derived portions notice, reproduced below",
        ),
    }
    lock = read_toml(workspace_root / "Cargo.lock", label="Cargo lockfile")
    packages = lock.get("package")
    if not isinstance(packages, list):
        raise ValidationError("Cargo lockfile must contain a package array")
    for name, (version, license_cell) in expected.items():
        locked = [row for row in packages if row.get("name") == name]
        if len(locked) != 1 or locked[0].get("version") != version:
            raise ValidationError(
                f"Cargo.lock must resolve exactly {name} {version} for remote reply authentication"
            )
        rows = re.findall(
            rf"^\| `{re.escape(name)}` \| ([^ |`]+) \| ([^|]+) \|$",
            notice,
            re.MULTILINE,
        )
        if rows != [(version, license_cell)]:
            raise ValidationError(
                f"Native notice must identify exact {name} {version} license provenance"
            )


def validate_native_band9_provenance(
    workspace_manifest: Path,
    *,
    expected_driver_version: str,
    root_license: Path,
) -> tuple[LockedTypeDbComponent, LockedTypeDbComponent]:
    """Bind packaged notices and vendor provenance to the resolved band-9 graph."""
    workspace_root = workspace_manifest.resolve().parent
    driver, protocol = resolved_official_band9_components(
        workspace_manifest,
        expected_driver_version=expected_driver_version,
    )
    components = (driver, protocol)

    python_notice_path = workspace_root / "python/type_bridge_core/THIRD_PARTY_NOTICES.md"
    node_notice_path = workspace_root / "crates/node/THIRD_PARTY_NOTICES.md"
    python_notice_bytes = read_bytes(python_notice_path, label="Python third-party notice")
    node_notice_bytes = read_bytes(node_notice_path, label="Node third-party notice")
    if python_notice_bytes != node_notice_bytes:
        raise ValidationError("Python and Node third-party notices must be byte-identical")
    try:
        notice = python_notice_bytes.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ValidationError("Native third-party notice is not UTF-8") from error
    if notice.count(NATIVE_NOTICE_BEGIN) != 1 or notice.count(NATIVE_NOTICE_END) != 1:
        raise ValidationError(
            "Native third-party notice must contain exactly one generated dependency appendix"
        )
    generated_appendix = notice.split(NATIVE_NOTICE_BEGIN, maxsplit=1)[1].split(
        NATIVE_NOTICE_END, maxsplit=1
    )[0]
    for phrase in (
        "## Locked Rust dependency closure",
        f"`cargo-about {NATIVE_NOTICE_CARGO_ABOUT_VERSION}`",
        f"under Rust `{NATIVE_NOTICE_RUST_TOOLCHAIN}`",
        "type-bridge-core/about.toml",
        "Closure fingerprint: `sha256:",
        "### Package inventory",
        "### Harvested license texts",
    ):
        if phrase not in generated_appendix:
            raise ValidationError(
                f"Native generated dependency appendix is missing contract marker {phrase!r}"
            )
    validate_native_license_bodies(
        workspace_root=workspace_root,
        root_license=root_license,
        notice=python_notice_bytes,
    )
    validate_native_crypto_notice_provenance(workspace_root, notice)

    license_cells = {
        "typedb-driver": "Apache-2.0, unmodified official package",
        "typedb-protocol": "MPL-2.0, unmodified official package",
    }
    for component in components:
        component_rows = re.findall(
            rf"^\| 9 \(default\) \| official `{re.escape(component.name)}` "
            r"([^ |`]+) \| ([^|]+) \| ([^|]+) \|$",
            notice,
            re.MULTILINE,
        )
        if len(component_rows) != 1:
            raise ValidationError(
                f"Native notice must contain exactly one band-9 {component.name} row"
            )
        documented_version, source_cell, license_cell = (
            value.strip() for value in component_rows[0]
        )
        if documented_version != component.version:
            raise ValidationError(
                f"Native notice {component.name} version disagrees with Cargo.lock: "
                f"actual={documented_version!r}, expected={component.version!r}"
            )
        expected_source = (
            f"TypeDB official crates.io package [{component.version}]"
            f"(https://crates.io/crates/{component.name}/{component.version})"
        )
        if source_cell != expected_source:
            raise ValidationError(
                f"Native notice {component.name} source must identify its official crates.io "
                f"package: actual={source_cell!r}, expected={expected_source!r}"
            )
        if license_cell != license_cells[component.name]:
            raise ValidationError(
                f"Native notice {component.name} license/status drifted: {license_cell!r}"
            )

        archive_rows = [
            (version, checksum)
            for version, checksum in re.findall(
                rf"^\| <https://static\.crates\.io/crates/{re.escape(component.name)}/"
                rf"{re.escape(component.name)}-([^/>]+)\.crate> \| `([^`]+)` \|$",
                notice,
                re.MULTILINE,
            )
        ]
        expected_archive = (component.version, component.checksum)
        if archive_rows != [expected_archive]:
            raise ValidationError(
                f"Native notice {component.name} band-9 archive provenance disagrees with "
                f"Cargo.lock: actual={archive_rows!r}, expected={[expected_archive]!r}"
            )

    normalized_notice = " ".join(notice.split())
    for phrase in (
        "TypeBridge-authored code remains licensed under the MIT License.",
        "There is no active, consumed, or release-input TypeBridge band-9 driver or protocol fork.",
        "TypeDB remains the original upstream owner and source",
        "## TypeDB driver portions — Apache License 2.0",
        "## TypeDB protocol portions — Mozilla Public License 2.0",
    ):
        if phrase not in normalized_notice:
            raise ValidationError(f"Native third-party notice is missing: {phrase!r}")

    vendor_readme = read_text(workspace_root / "vendor/README.md", label="vendor provenance")
    compact_vendor_readme = " ".join(vendor_readme.split())
    for phrase in (
        f"currently that is {driver.version}, exercised",
        "There is no active, consumed, or release-input TypeBridge band-9 fork.",
        "forbidden for consumption and are not release inputs.",
        "TypeDB remains the upstream project and original source",
        "TypeBridge-authored crates and bindings remain MIT.",
        "Files derived from the TypeDB drivers retain Apache-2.0",
        "files derived from the TypeDB protocols retain MPL-2.0",
    ):
        if phrase not in compact_vendor_readme:
            raise ValidationError(f"Vendor provenance is missing: {phrase!r}")
    band9_rows = re.findall(
        r"^\| 9 \| ([^|]+) \| ([^|]+) \| ([^|]+) \|$",
        vendor_readme,
        re.MULTILINE,
    )
    if len(band9_rows) != 1:
        raise ValidationError("Vendor provenance must contain exactly one band-9 package row")
    identity_cell, source_cell, disposition_cell = (value.strip() for value in band9_rows[0])
    if driver.version == protocol.version:
        expected_identities = {f"official `typedb-driver` and `typedb-protocol` {driver.version}"}
    else:
        expected_identities = {
            f"official `typedb-driver` {driver.version} and `typedb-protocol` {protocol.version}"
        }
    if identity_cell not in expected_identities:
        raise ValidationError(
            "Vendor provenance band-9 versions disagree with Cargo.lock: "
            f"actual={identity_cell!r}, expected={sorted(expected_identities)!r}"
        )
    if source_cell != "official crates.io packages":
        raise ValidationError(
            f"Vendor provenance band-9 source is not official crates.io: {source_cell!r}"
        )
    if disposition_cell != "consume upstream directly; never publish a TypeBridge fork":
        raise ValidationError(f"Vendor provenance band-9 disposition drifted: {disposition_cell!r}")
    return components


def validate_release_identity(
    *,
    tag: str,
    artifact_contract: str,
    release_channel: str,
    workspace_manifest: Path,
    root_python_manifest: Path,
    core_python_manifest: Path,
    node_package: Path,
    release_workflow: Path,
    root_python_init: Path | None = None,
    node_package_lock: Path | None = None,
) -> dict[str, Any]:
    """Validate all release identities and report Cargo publication blockers."""
    if artifact_contract not in ARTIFACT_CONTRACTS:
        raise ValidationError(
            f"Unknown release artifact contract {artifact_contract!r}; "
            f"expected one of {list(ARTIFACT_CONTRACTS)!r}"
        )
    version, python_version = release_identity_versions(tag, release_channel)
    validate_manifest_version(
        root_python_manifest,
        python_version,
        label="root Python manifest",
    )
    validate_manifest_version(
        core_python_manifest,
        python_version,
        label="core Python manifest",
    )
    try:
        python_core_requirement = validate_root_python_manifest_lockstep(
            root_python_manifest.resolve(),
            python_version,
        )
    except PythonReleaseContractError as error:
        raise ValidationError(str(error)) from error
    validate_python_manifest_license(root_python_manifest, label="root Python manifest")
    validate_python_manifest_license(core_python_manifest, label="core Python manifest")
    try:
        python_package_version = validate_python_package_version(
            (
                root_python_manifest.resolve().parent / "type_bridge/__init__.py"
                if root_python_init is None
                else root_python_init.resolve()
            ),
            python_version,
        )
    except PythonReleaseContractError as error:
        raise ValidationError(str(error)) from error
    node_payload = read_json(node_package.resolve(), label="Node package.json")
    node_version = semantic_version(
        node_payload.get("version"),
        label="Node package version",
    )
    if node_version != version:
        raise ValidationError(
            "Node package version disagrees with release tag: "
            f"actual={node_version!r}, expected={version!r}"
        )
    if node_payload.get("license") != MIT_LICENSE:
        raise ValidationError(
            "Node package license must remain MIT: "
            f"actual={node_payload.get('license')!r}, expected={MIT_LICENSE!r}"
        )
    try:
        node_lock_version = validate_node_package_lockstep(
            node_package.resolve(),
            (
                node_package.resolve().with_name("package-lock.json")
                if node_package_lock is None
                else node_package_lock.resolve()
            ),
            version,
        )
    except PythonReleaseContractError as error:
        raise ValidationError(str(error)) from error

    quarantined_band9_packages = validate_historical_band9_quarantine(workspace_manifest)
    packages = cargo_workspace_packages(workspace_manifest)
    cargo_licenses = validate_cargo_license_boundary(workspace_manifest, packages)
    by_name = {package.name: package for package in packages}
    missing_core_crates = sorted(set(UNPUBLISHED_CORE_CRATES) - set(by_name))
    if missing_core_crates:
        raise ValidationError(
            "Authoritative unpublished core crates are absent from the workspace: "
            f"{missing_core_crates!r}"
        )
    non_explicit_publish_settings = sorted(
        name
        for name in PYTHON_NPM_UNPUBLISHED_CRATES
        if name not in by_name or not by_name[name].publish_explicitly_false
    )
    if non_explicit_publish_settings:
        raise ValidationError(
            "Authoritative core and binding crates must declare package.publish = false exactly: "
            f"{non_explicit_publish_settings!r}"
        )
    for package in packages:
        if package.vendored:
            raise ValidationError(
                f"Unpinned workspace vendor crate: {package.name}@{package.version}"
            )
        expected = version
        if package.version != expected:
            raise ValidationError(
                f"Cargo package {package.name} version disagrees with its release identity: "
                f"actual={package.version!r}, expected={expected!r}"
            )

    runtime_package = by_name.get(TYPEDB_RUNTIME_PACKAGE)
    if runtime_package is None:
        raise ValidationError(f"TypeDB runtime package is absent: {TYPEDB_RUNTIME_PACKAGE}")
    typedb_runtime_driver_pin = validate_typedb_runtime_driver_pins(runtime_package)
    locked_band9_components = validate_native_band9_provenance(
        workspace_manifest,
        expected_driver_version=typedb_runtime_driver_pin,
        root_license=root_python_manifest.resolve().parent / "LICENSE",
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
    validate_native_notice_workflow(release_workflow.resolve())
    validate_server_oci_release_channels(release_workflow.resolve())
    if artifact_contract == ARTIFACT_CONTRACT_CARGO_INCLUSIVE:
        actual_publish_sequence = workflow_publish_sequence(release_workflow.resolve())
        if actual_publish_sequence != EXPECTED_NEW_CRATES:
            raise ValidationError(
                "Cargo publish sequence is incomplete or reordered: "
                f"actual={actual_publish_sequence!r}, expected={EXPECTED_NEW_CRATES!r}"
            )
        preflight_patches, preflight_packages = workflow_preflight_sequences(
            release_workflow.resolve()
        )
        if preflight_patches:
            raise ValidationError(
                "Cargo candidate preflight must not use crates.io patches: "
                f"actual={preflight_patches!r}"
            )
        if preflight_packages != PACKAGED_RELEASE_CRATES:
            raise ValidationError(
                "Cargo preflight package sequence is incomplete or reordered: "
                f"actual={preflight_packages!r}, expected={PACKAGED_RELEASE_CRATES!r}"
            )
        preexisting_preflights, candidate_preflights = workflow_registry_preflight_sequences(
            release_workflow.resolve()
        )
        if preexisting_preflights != PREEXISTING_CRATES:
            raise ValidationError(
                "Pre-existing crates.io checksum sequence is incomplete or reordered: "
                f"actual={preexisting_preflights!r}, expected={PREEXISTING_CRATES!r}"
            )
        if candidate_preflights != EXPECTED_NEW_CRATES:
            raise ValidationError(
                "Expected-new crates.io key-preflight sequence is incomplete or reordered: "
                f"actual={candidate_preflights!r}, expected={EXPECTED_NEW_CRATES!r}"
            )
    elif artifact_contract == ARTIFACT_CONTRACT_PYTHON_NPM_ONLY:
        validate_python_npm_only_workflow(release_workflow.resolve())
    else:
        validate_source_git_server_oci_workflow(release_workflow.resolve())
    for name in PUBLISHED_CRATES:
        package = by_name.get(name)
        if package is None or not package.publishable:
            raise ValidationError(f"Published crate is absent or publish=false: {name}")

    manifest_to_package = {package.manifest: package for package in packages}
    publish_position = {name: position for position, name in enumerate(PUBLISHED_CRATES)}
    publication_blockers: list[dict[str, str]] = []
    for package_name in PUBLISHED_CRATES:
        package = by_name[package_name]
        for dependency in package.path_dependencies:
            target = manifest_to_package.get(dependency.manifest)
            if target is None or target.name != dependency.name:
                actual = None if target is None else target.name
                raise ValidationError(
                    f"Published crate {package.name} has an unresolved workspace path dependency: "
                    f"declared={dependency.name!r}, actual={actual!r}, "
                    f"manifest={dependency.manifest}"
                )
            if dependency.requirement not in (target.version, f"={target.version}"):
                raise ValidationError(
                    f"Published crate {package.name} must declare the release version for "
                    f"workspace dependency {target.name}: actual={dependency.requirement!r}, "
                    f"expected={target.version!r}"
                )
            if not target.publishable or target.name not in publish_position:
                if target.name not in UNPUBLISHED_CORE_CRATES:
                    raise ValidationError(
                        f"Published crate {package.name} depends on unpublished workspace crate "
                        f"{target.name}"
                    )
                edge = (package.name, target.name)
                if edge not in KNOWN_PUBLICATION_BLOCKER_EDGES:
                    raise ValidationError(
                        f"Published crate {package.name} depends on unpublished workspace crate "
                        f"{target.name} outside the acknowledged blocked Rust graph"
                    )
                publication_blockers.append(
                    {
                        "package": package.name,
                        "unpublished_dependency": target.name,
                    }
                )
                continue
            if publish_position[target.name] >= publish_position[package.name]:
                raise ValidationError(
                    f"Cargo publish sequence places {package.name} before dependency {target.name}"
                )

    blocker_edges = [
        (entry["package"], entry["unpublished_dependency"]) for entry in publication_blockers
    ]
    if artifact_contract != ARTIFACT_CONTRACT_CARGO_INCLUSIVE:
        actual_blocker_edges = frozenset(blocker_edges)
        missing_blocker_edges = sorted(KNOWN_PUBLICATION_BLOCKER_EDGES - actual_blocker_edges)
        unexpected_blocker_edges = sorted(actual_blocker_edges - KNOWN_PUBLICATION_BLOCKER_EDGES)
        if (
            missing_blocker_edges
            or unexpected_blocker_edges
            or len(blocker_edges) != len(actual_blocker_edges)
        ):
            raise ValidationError(
                "Python/npm-only Rust publication-blocker graph drifted: "
                f"missing={missing_blocker_edges!r}, "
                f"unexpected={unexpected_blocker_edges!r}, "
                f"duplicate_count={len(blocker_edges) - len(actual_blocker_edges)}"
            )

    crates_io_mutation = artifact_contract == ARTIFACT_CONTRACT_CARGO_INCLUSIVE
    report = {
        "artifact_contract": artifact_contract,
        "public_artifact_set": (
            [
                "python-root",
                "python-native",
                "npm-package",
                "npm-native-packages",
                "rust-type-bridge-source-git-exact-revision",
                "ghcr.io/ds1sqe/type-bridge-server",
            ]
            if artifact_contract == ARTIFACT_CONTRACT_SOURCE_GIT_SERVER_OCI
            else ["python-root", "python-native", "npm-package", "npm-native-packages"]
        ),
        "cargo_licenses": cargo_licenses,
        "cargo_manifest_publishable_crates": list(PUBLISHED_CRATES),
        "cargo_packages": {package.name: package.version for package in packages},
        "cargo_publication_plan": (
            list(EXPECTED_NEW_CRATES)
            if artifact_contract == ARTIFACT_CONTRACT_CARGO_INCLUSIVE
            else []
        ),
        "crates_io_mutation": crates_io_mutation,
        "historical_band9_quarantine": list(quarantined_band9_packages),
        "cargo_manifest_dependency_order": {
            name: [dependency.name for dependency in by_name[name].path_dependencies]
            for name in PUBLISHED_CRATES
        },
        "rust_publication_blockers": publication_blockers,
        "unpublished_crates": sorted(set(by_name) - expected_publishable),
        "unpublished_v2_crates": list(UNPUBLISHED_CORE_CRATES),
        "status": ("blocked" if crates_io_mutation and publication_blockers else "ok"),
        "tag": tag,
        "node_package_lock_version": node_lock_version,
        "python_core_requirement": python_core_requirement,
        "python_package_version": python_package_version,
        "typedb_runtime_driver_pin": typedb_runtime_driver_pin,
        "typedb_runtime_band9_driver_pin": typedb_runtime_driver_pin,
        "typedb_runtime_band9_components": {
            component.name: {
                "checksum": component.checksum,
                "source": component.source,
                "version": component.version,
            }
            for component in locked_band9_components
        },
        "python_version": python_version,
        "release_channel": release_channel,
        "server_oci_stable_aliases": [
            ".".join(version.split(".")[:2]),
            version.split(".")[0],
            "latest",
        ],
        "server_oci_stable_signing_identity": (
            f"https://github.com/ds1sqe/type-bridge/.github/workflows/release.yml@refs/tags/v{version}"
        ),
        "version": version,
    }
    if artifact_contract == ARTIFACT_CONTRACT_CARGO_INCLUSIVE:
        require_unblocked_rust_publication(report)
    return report


def require_unblocked_rust_publication(report: dict[str, Any]) -> None:
    """Reject a release whose public crates depend on unpublished V2 crates."""
    blockers = report.get("rust_publication_blockers")
    if not isinstance(blockers, list):
        raise ValidationError("Release identity report has no Rust publication-blocker inventory")
    if not blockers:
        return
    edges = []
    for blocker in blockers:
        if not isinstance(blocker, dict):
            raise ValidationError(
                "Release identity report has a malformed Rust publication blocker"
            )
        package = blocker.get("package")
        dependency = blocker.get("unpublished_dependency")
        if not isinstance(package, str) or not isinstance(dependency, str):
            raise ValidationError(
                "Release identity report has a malformed Rust publication blocker"
            )
        edges.append(f"{package} -> {dependency}")
    raise RustPublicationBlockedError(
        "Rust crates.io publication is blocked because public crates depend on "
        f"unpublished workspace crates: {', '.join(edges)}.",
        report,
    )


def build_parser() -> argparse.ArgumentParser:
    """Build the release-identity validator CLI."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--artifact-contract", choices=ARTIFACT_CONTRACTS, required=True)
    parser.add_argument(
        "--release-channel",
        choices=tuple(RELEASE_CHANNEL_IDENTITIES),
        required=True,
    )
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--root-python", type=Path, required=True)
    parser.add_argument("--root-python-init", type=Path, required=True)
    parser.add_argument("--core-python", type=Path, required=True)
    parser.add_argument("--node-package", type=Path, required=True)
    parser.add_argument("--node-package-lock", type=Path, required=True)
    parser.add_argument("--release-workflow", type=Path, required=True)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Run identity plus live upstream provenance validation and print a report."""
    args = build_parser().parse_args(argv)
    report = validate_release_identity(
        tag=args.tag,
        artifact_contract=args.artifact_contract,
        release_channel=args.release_channel,
        workspace_manifest=args.workspace,
        root_python_manifest=args.root_python,
        root_python_init=args.root_python_init,
        core_python_manifest=args.core_python,
        node_package=args.node_package,
        node_package_lock=args.node_package_lock,
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
