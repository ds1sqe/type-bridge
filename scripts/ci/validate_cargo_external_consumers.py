#!/usr/bin/env python3
"""Accept the packaged Cargo graph through source-independent consumers."""

from __future__ import annotations

import argparse
import io
import json
import os
import re
import stat
import subprocess
import sys
import tarfile
import tempfile
import tomllib
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

try:
    from cargo_release_inventory import (
        CargoInventoryPackage,
        CargoReleaseInventory,
        InventoryError,
        load_inventory,
    )
except ModuleNotFoundError:
    from scripts.ci.cargo_release_inventory import (
        CargoInventoryPackage,
        CargoReleaseInventory,
        InventoryError,
        load_inventory,
    )

try:
    from cargo_release_candidate import CandidateError, validate_candidate_bundle
except ModuleNotFoundError:
    from scripts.ci.cargo_release_candidate import CandidateError, validate_candidate_bundle

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_ARTIFACTS_DIRECTORY = Path("type-bridge-core/target/package")
MAX_ARCHIVE_BYTES = 64 * 1024 * 1024
MAX_ARCHIVE_FILES = 20_000
MAX_ARCHIVE_MEMBER_BYTES = 64 * 1024 * 1024
MAX_ARCHIVE_EXPANDED_BYTES = 256 * 1024 * 1024
LIBRARY_TARGET_KINDS = frozenset({"lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"})
CRATES_IO_SOURCE = "registry+https://github.com/rust-lang/crates.io-index"
CommandRunner = Callable[..., subprocess.CompletedProcess[str]]


class ExternalConsumerError(RuntimeError):
    """The exact packaged Cargo graph failed external-consumer acceptance."""


@dataclass(frozen=True)
class ExtractedPackage:
    """One validated package extracted from its exact ``.crate`` archive."""

    package: CargoInventoryPackage
    archive: Path
    root: Path


def cargo_prefix(*, cargo: str, toolchain: str | None) -> tuple[str, ...]:
    """Return a Cargo command prefix with an optional exact toolchain."""
    if not cargo or cargo != cargo.strip():
        raise ExternalConsumerError(f"invalid Cargo executable: {cargo!r}")
    if toolchain is None:
        return (cargo,)
    if re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", toolchain) is None:
        raise ExternalConsumerError(
            f"Cargo toolchain must be an exact numeric Rust version: {toolchain!r}"
        )
    return (cargo, f"+{toolchain}")


def expected_archive_names(inventory: CargoReleaseInventory) -> dict[str, CargoInventoryPackage]:
    """Return the closed public archive filename inventory."""
    return {
        f"{package.name}-{package.version}.crate": package for package in inventory.public_packages
    }


def validate_archive_inventory(
    artifacts_directory: Path,
    inventory: CargoReleaseInventory,
) -> dict[str, CargoInventoryPackage]:
    """Require exactly the closed set of public ``.crate`` archives."""
    try:
        directory_stat = artifacts_directory.lstat()
    except OSError as error:
        raise ExternalConsumerError(
            f"could not inspect Cargo archive directory {artifacts_directory}: {error}"
        ) from error
    if stat.S_ISLNK(directory_stat.st_mode) or not stat.S_ISDIR(directory_stat.st_mode):
        raise ExternalConsumerError(
            f"Cargo archive directory is linked or non-directory: {artifacts_directory}"
        )
    expected = expected_archive_names(inventory)
    actual = {
        entry.name for entry in artifacts_directory.iterdir() if entry.name.endswith(".crate")
    }
    if actual != set(expected):
        raise ExternalConsumerError(
            "packaged Cargo archive inventory drifted: "
            f"missing={sorted(set(expected) - actual)!r}, "
            f"unexpected={sorted(actual - set(expected))!r}"
        )
    return expected


def _read_regular_archive(path: Path) -> bytes:
    """Read one bounded regular archive without following a symlink."""
    try:
        archive_stat = path.lstat()
    except OSError as error:
        raise ExternalConsumerError(f"could not inspect Cargo archive {path}: {error}") from error
    if stat.S_ISLNK(archive_stat.st_mode) or not stat.S_ISREG(archive_stat.st_mode):
        raise ExternalConsumerError(f"Cargo archive is linked or non-regular: {path}")
    if archive_stat.st_size < 0 or archive_stat.st_size > MAX_ARCHIVE_BYTES:
        raise ExternalConsumerError(
            "Cargo archive exceeds the compressed byte budget: "
            f"path={path}, size={archive_stat.st_size}, maximum={MAX_ARCHIVE_BYTES}"
        )
    try:
        body = path.read_bytes()
    except OSError as error:
        raise ExternalConsumerError(f"could not read Cargo archive {path}: {error}") from error
    if len(body) != archive_stat.st_size:
        raise ExternalConsumerError(f"Cargo archive changed while it was read: {path}")
    return body


def _dependency_tables(manifest: Mapping[str, Any]) -> tuple[tuple[str, Mapping[str, Any]], ...]:
    """Return every root and target-specific Cargo dependency table."""
    tables: list[tuple[str, Mapping[str, Any]]] = []
    for section in ("dependencies", "dev-dependencies", "build-dependencies"):
        value = manifest.get(section)
        if value is None:
            continue
        if not isinstance(value, dict):
            raise ExternalConsumerError(f"packaged Cargo manifest [{section}] is not a table")
        tables.append((section, value))
    targets = manifest.get("target")
    if targets is not None:
        if not isinstance(targets, dict):
            raise ExternalConsumerError("packaged Cargo manifest [target] is not a table")
        for target_name, target in targets.items():
            if not isinstance(target, dict):
                raise ExternalConsumerError(
                    f"packaged Cargo manifest target {target_name!r} is not a table"
                )
            for section in ("dependencies", "dev-dependencies", "build-dependencies"):
                value = target.get(section)
                if value is None:
                    continue
                if not isinstance(value, dict):
                    raise ExternalConsumerError(
                        f"packaged Cargo manifest [target.{target_name}.{section}] is not a table"
                    )
                tables.append((f"target.{target_name}.{section}", value))
    return tuple(tables)


def validate_packaged_manifest(
    body: bytes,
    *,
    package: CargoInventoryPackage,
) -> None:
    """Require a normalized registry-form manifest for one archive."""
    try:
        manifest = tomllib.loads(body.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        raise ExternalConsumerError(
            f"could not parse packaged manifest for {package.name}: {error}"
        ) from error
    package_table = manifest.get("package")
    if not isinstance(package_table, dict):
        raise ExternalConsumerError(f"packaged manifest for {package.name} has no [package]")
    actual_identity = (package_table.get("name"), package_table.get("version"))
    expected_identity = (package.name, package.version)
    if actual_identity != expected_identity:
        raise ExternalConsumerError(
            f"packaged manifest identity drifted: actual={actual_identity!r}, "
            f"expected={expected_identity!r}"
        )
    if "patch" in manifest or "replace" in manifest:
        raise ExternalConsumerError(
            f"packaged manifest for {package.name} contains an embedded source override"
        )
    for section, dependencies in _dependency_tables(manifest):
        for dependency, specification in dependencies.items():
            if isinstance(specification, str):
                if not specification:
                    raise ExternalConsumerError(
                        f"{package.name} {section} dependency {dependency!r} has no version"
                    )
                continue
            if not isinstance(specification, dict):
                raise ExternalConsumerError(
                    f"{package.name} {section} dependency {dependency!r} is malformed"
                )
            forbidden = sorted({"path", "git"} & set(specification))
            if forbidden:
                raise ExternalConsumerError(
                    f"{package.name} {section} dependency {dependency!r} is not registry-form: "
                    f"forbidden={forbidden!r}"
                )
            version = specification.get("version")
            if not isinstance(version, str) or not version:
                raise ExternalConsumerError(
                    f"{package.name} {section} dependency {dependency!r} has no registry version"
                )


def extract_archive(
    archive_path: Path,
    *,
    package: CargoInventoryPackage,
    destination: Path,
) -> ExtractedPackage:
    """Safely extract one exact Cargo archive into an isolated directory."""
    archive = _read_regular_archive(archive_path)
    root_name = f"{package.name}-{package.version}"
    root = destination / root_name
    if root.exists() or root.is_symlink():
        raise ExternalConsumerError(f"Cargo extraction root already exists: {root}")
    files: dict[PurePosixPath, bytes] = {}
    seen: set[str] = set()
    expanded_bytes = 0
    try:
        with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as crate:
            for member_count, member in enumerate(crate, start=1):
                if member_count > MAX_ARCHIVE_FILES:
                    raise ExternalConsumerError(
                        f"Cargo archive {archive_path.name} exceeds the member-count budget"
                    )
                name = member.name
                if "\\" in name or name.startswith("/") or "\x00" in name:
                    raise ExternalConsumerError(
                        f"Cargo archive {archive_path.name} contains an unsafe path: {name!r}"
                    )
                normalized = name.rstrip("/")
                parts = normalized.split("/")
                if (
                    not normalized
                    or any(part in ("", ".", "..") for part in parts)
                    or parts[0] != root_name
                ):
                    raise ExternalConsumerError(
                        f"Cargo archive {archive_path.name} contains an unsafe path: {name!r}"
                    )
                if normalized in seen:
                    raise ExternalConsumerError(
                        f"Cargo archive {archive_path.name} contains a duplicate member: {name!r}"
                    )
                seen.add(normalized)
                if member.isdir():
                    continue
                if not member.isfile():
                    raise ExternalConsumerError(
                        f"Cargo archive {archive_path.name} contains a linked or non-file member: "
                        f"{name!r}"
                    )
                if len(parts) == 1:
                    raise ExternalConsumerError(
                        f"Cargo archive {archive_path.name} has a file at its package root"
                    )
                if member.size < 0 or member.size > MAX_ARCHIVE_MEMBER_BYTES:
                    raise ExternalConsumerError(
                        f"Cargo archive member exceeds its byte budget: {name!r}"
                    )
                expanded_bytes += member.size
                if expanded_bytes > MAX_ARCHIVE_EXPANDED_BYTES:
                    raise ExternalConsumerError(
                        f"Cargo archive {archive_path.name} exceeds its expanded byte budget"
                    )
                source = crate.extractfile(member)
                if source is None:
                    raise ExternalConsumerError(
                        f"Cargo archive {archive_path.name} contains an unreadable file: {name!r}"
                    )
                body = source.read(member.size + 1)
                if len(body) != member.size:
                    raise ExternalConsumerError(
                        f"Cargo archive member size disagrees with its body: {name!r}"
                    )
                relative = PurePosixPath(*parts[1:])
                if relative in files:
                    raise ExternalConsumerError(
                        f"Cargo archive {archive_path.name} contains a duplicate file: "
                        f"{relative.as_posix()!r}"
                    )
                files[relative] = body
    except ExternalConsumerError:
        raise
    except (EOFError, OSError, tarfile.TarError) as error:
        raise ExternalConsumerError(
            f"could not safely read Cargo archive {archive_path.name}: {error}"
        ) from error
    manifest_body = files.get(PurePosixPath("Cargo.toml"))
    if manifest_body is None:
        raise ExternalConsumerError(f"Cargo archive {archive_path.name} has no Cargo.toml")
    validate_packaged_manifest(manifest_body, package=package)
    root.mkdir(parents=True)
    for relative, body in files.items():
        target = root.joinpath(*relative.parts)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(body)
    return ExtractedPackage(package=package, archive=archive_path, root=root)


def extract_public_archives(
    artifacts_directory: Path,
    *,
    inventory: CargoReleaseInventory,
    destination: Path,
) -> dict[str, ExtractedPackage]:
    """Validate and extract the complete public archive inventory."""
    expected = validate_archive_inventory(artifacts_directory, inventory)
    extracted: dict[str, ExtractedPackage] = {}
    for filename, package in expected.items():
        extracted[package.name] = extract_archive(
            artifacts_directory / filename,
            package=package,
            destination=destination,
        )
    return extracted


def write_patch_config(root: Path, extracted: Mapping[str, ExtractedPackage]) -> Path:
    """Patch crates.io to exact extracted archives in temporary Cargo config only."""
    config_directory = root / ".cargo"
    config_directory.mkdir(parents=True)
    lines = ["[patch.crates-io]"]
    for name in sorted(extracted):
        package_root = extracted[name].root.resolve()
        lines.append(f"{json.dumps(name)} = {{ path = {json.dumps(str(package_root))} }}")
    config = config_directory / "config.toml"
    config.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return config


def _write_base_consumer(root: Path, *, name: str, dependencies: Sequence[str]) -> Path:
    """Write one registry-only temporary Cargo consumer."""
    source = root / "src"
    source.mkdir(parents=True)
    manifest_lines = [
        "[package]",
        f"name = {json.dumps(name)}",
        'version = "0.0.0"',
        'edition = "2024"',
        "publish = false",
        "",
        "[dependencies]",
        *dependencies,
    ]
    manifest = root / "Cargo.toml"
    manifest.write_text("\n".join(manifest_lines) + "\n", encoding="utf-8")
    (source / "main.rs").write_text("fn main() {}\n", encoding="utf-8")
    return manifest


def write_surface_consumer(
    root: Path,
    *,
    inventory: CargoReleaseInventory,
) -> Path:
    """Write a consumer with exact registry dependencies on all 17 first-party crates."""
    dependencies = [
        f"{json.dumps(package.name)} = {json.dumps('=' + package.version)}"
        for package in inventory.first_party_packages
    ]
    return _write_base_consumer(
        root,
        name="type-bridge-packaged-surface-consumer",
        dependencies=dependencies,
    )


def write_server_consumer(
    root: Path,
    *,
    version: str,
    features: Sequence[str],
) -> Path:
    """Write a registry-form server library feature consumer."""
    feature_fragment = ""
    if features:
        feature_fragment = f", features = {json.dumps(list(features))}"
    dependency = (
        '"type-bridge-server" = { '
        f"version = {json.dumps('=' + version)}, default-features = false{feature_fragment} }}"
    )
    manifest = _write_base_consumer(
        root,
        name=f"type-bridge-server-{'-'.join(features) if features else 'no-default'}-consumer",
        dependencies=[dependency],
    )
    (root / "src/main.rs").write_text(
        "use ::type_bridge_server as _;\n\nfn main() {}\n",
        encoding="utf-8",
    )
    return manifest


def _run_command(
    command: Sequence[str],
    *,
    cwd: Path,
    environment: Mapping[str, str],
    capture_output: bool,
    runner: CommandRunner = subprocess.run,
) -> subprocess.CompletedProcess[str]:
    """Run one acceptance command and convert every failure to a closed error."""
    try:
        result = runner(
            list(command),
            cwd=cwd,
            env=dict(environment),
            check=False,
            text=True,
            capture_output=capture_output,
        )
    except OSError as error:
        raise ExternalConsumerError(f"could not execute {command[0]!r}: {error}") from error
    if result.returncode != 0:
        details = ""
        if capture_output:
            details = f"\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        raise ExternalConsumerError(
            f"external-consumer command failed with exit {result.returncode}: "
            f"{' '.join(command)}{details}"
        )
    return result


def load_metadata(
    manifest: Path,
    *,
    cargo: tuple[str, ...],
    environment: Mapping[str, str],
    runner: CommandRunner = subprocess.run,
) -> Mapping[str, Any]:
    """Resolve one generated registry consumer and return Cargo metadata."""
    result = _run_command(
        (
            *cargo,
            "metadata",
            "--format-version",
            "1",
            "--manifest-path",
            str(manifest),
        ),
        cwd=manifest.parent,
        environment=environment,
        capture_output=True,
        runner=runner,
    )
    try:
        payload = json.loads(result.stdout)
    except (TypeError, json.JSONDecodeError) as error:
        raise ExternalConsumerError(f"Cargo metadata returned invalid JSON: {error}") from error
    if not isinstance(payload, dict):
        raise ExternalConsumerError("Cargo metadata root is not an object")
    return payload


def _resolved_package(
    metadata: Mapping[str, Any],
    *,
    name: str,
    version: str,
) -> Mapping[str, Any]:
    """Return one uniquely resolved package identity."""
    packages = metadata.get("packages")
    if not isinstance(packages, list):
        raise ExternalConsumerError("Cargo metadata packages is not an array")
    matches = [
        package
        for package in packages
        if isinstance(package, dict)
        and package.get("name") == name
        and package.get("version") == version
    ]
    if len(matches) != 1:
        raise ExternalConsumerError(
            f"Cargo metadata did not resolve exactly one {name}@{version}: count={len(matches)}"
        )
    return matches[0]


def validate_metadata(
    metadata: Mapping[str, Any],
    *,
    consumer_manifest: Path,
    direct_dependencies: Mapping[str, str],
    required_packages: Sequence[CargoInventoryPackage],
    extracted: Mapping[str, ExtractedPackage],
) -> dict[str, str]:
    """Bind registry declarations and resolved package IDs to extracted archives."""
    raw_packages = metadata.get("packages")
    if not isinstance(raw_packages, list):
        raise ExternalConsumerError("Cargo metadata packages is not an array")
    consumers = [
        package
        for package in raw_packages
        if isinstance(package, dict)
        and isinstance(package.get("manifest_path"), str)
        and Path(package["manifest_path"]).resolve() == consumer_manifest.resolve()
        and package.get("version") == "0.0.0"
    ]
    if len(consumers) != 1:
        raise ExternalConsumerError("Cargo metadata consumer manifest identity drifted")
    consumer = consumers[0]
    raw_dependencies = consumer.get("dependencies")
    if not isinstance(raw_dependencies, list):
        raise ExternalConsumerError("Cargo metadata consumer dependencies is not an array")
    dependencies: dict[str, Mapping[str, Any]] = {}
    for dependency in raw_dependencies:
        if not isinstance(dependency, dict):
            continue
        dependency_name = dependency.get("name")
        if isinstance(dependency_name, str):
            dependencies[dependency_name] = dependency
    if set(dependencies) != set(direct_dependencies):
        raise ExternalConsumerError(
            "Cargo consumer direct dependency inventory drifted: "
            f"actual={sorted(dependencies)!r}, expected={sorted(direct_dependencies)!r}"
        )
    for name, version in direct_dependencies.items():
        dependency = dependencies[name]
        if dependency.get("req") != f"={version}":
            raise ExternalConsumerError(
                f"Cargo consumer dependency {name} is not exact: {dependency.get('req')!r}"
            )
        if dependency.get("source") != CRATES_IO_SOURCE or dependency.get("path") is not None:
            raise ExternalConsumerError(f"Cargo consumer dependency {name} is not registry-form")

    resolve = metadata.get("resolve")
    if not isinstance(resolve, dict) or not isinstance(resolve.get("nodes"), list):
        raise ExternalConsumerError("Cargo metadata has no resolved graph")
    resolved_ids = {
        node.get("id")
        for node in resolve["nodes"]
        if isinstance(node, dict) and isinstance(node.get("id"), str)
    }
    library_names: dict[str, str] = {}
    for package in required_packages:
        resolved = _resolved_package(metadata, name=package.name, version=package.version)
        expected_manifest = (extracted[package.name].root / "Cargo.toml").resolve()
        actual_package_manifest = resolved.get("manifest_path")
        if (
            not isinstance(actual_package_manifest, str)
            or Path(actual_package_manifest).resolve() != expected_manifest
            or resolved.get("source") is not None
        ):
            raise ExternalConsumerError(
                f"Cargo resolved {package.name}@{package.version} outside its extracted archive"
            )
        package_id = resolved.get("id")
        if not isinstance(package_id, str) or package_id not in resolved_ids:
            raise ExternalConsumerError(
                f"Cargo metadata package {package.name}@{package.version} is not in the resolve graph"
            )
        if package.classification != "public-first-party":
            continue
        raw_targets = resolved.get("targets")
        if not isinstance(raw_targets, list):
            raise ExternalConsumerError(f"Cargo metadata targets are missing for {package.name}")
        targets = [
            target
            for target in raw_targets
            if isinstance(target, dict)
            and isinstance(target.get("kind"), list)
            and LIBRARY_TARGET_KINDS.intersection(target["kind"])
        ]
        if len(targets) != 1 or not isinstance(targets[0].get("name"), str):
            raise ExternalConsumerError(
                f"Cargo package {package.name} does not expose exactly one library crate root"
            )
        library_names[package.name] = targets[0]["name"]
    return library_names


def validate_server_features(
    metadata: Mapping[str, Any],
    *,
    version: str,
    expected_v2: bool,
) -> None:
    """Require the server feature probe to exclude every TypeDB band."""
    server = _resolved_package(metadata, name="type-bridge-server", version=version)
    package_id = server.get("id")
    resolve = metadata.get("resolve")
    if not isinstance(package_id, str) or not isinstance(resolve, dict):
        raise ExternalConsumerError("Cargo server metadata has no resolved package ID")
    nodes = resolve.get("nodes")
    if not isinstance(nodes, list):
        raise ExternalConsumerError("Cargo server metadata has no resolve nodes")
    matches = [node for node in nodes if isinstance(node, dict) and node.get("id") == package_id]
    if len(matches) != 1 or not isinstance(matches[0].get("features"), list):
        raise ExternalConsumerError("Cargo server feature resolution is ambiguous")
    features = set(matches[0]["features"])
    forbidden = {"default", "typedb", "band8", "band9"} & features
    if forbidden:
        raise ExternalConsumerError(
            f"Cargo server isolated feature probe activated forbidden features: {sorted(forbidden)!r}"
        )
    if ("v2-query" in features) is not expected_v2:
        raise ExternalConsumerError(
            f"Cargo server v2-query feature resolution drifted: actual={sorted(features)!r}"
        )


def write_surface_source(root: Path, library_names: Mapping[str, str]) -> None:
    """Touch every first-party public crate root from external Rust source."""
    names = sorted(library_names.values())
    if len(names) != 17 or len(set(names)) != 17:
        raise ExternalConsumerError(
            f"surface consumer requires 17 unique crate roots: actual={names!r}"
        )
    lines = ["#![allow(unused_imports)]", ""]
    lines.extend(f"use ::{name} as _;" for name in names)
    lines.extend(("", "fn main() {}", ""))
    (root / "src/main.rs").write_text("\n".join(lines), encoding="utf-8")


def _cargo_check(
    manifest: Path,
    *,
    cargo: tuple[str, ...],
    environment: Mapping[str, str],
    runner: CommandRunner,
) -> None:
    """Compile one generated consumer against its locked external graph."""
    _run_command(
        (*cargo, "check", "--locked", "--manifest-path", str(manifest)),
        cwd=manifest.parent,
        environment=environment,
        capture_output=False,
        runner=runner,
    )


def _install_and_run_binary(
    *,
    cargo: tuple[str, ...],
    package_root: Path,
    binary: str,
    expected_version: str,
    install_root: Path,
    work_root: Path,
    environment: Mapping[str, str],
    runner: CommandRunner,
) -> None:
    """Install one binary from its extracted archive and run its version surface."""
    _run_command(
        (
            *cargo,
            "install",
            "--locked",
            "--debug",
            "--root",
            str(install_root),
            "--path",
            str(package_root),
            "--bin",
            binary,
        ),
        cwd=work_root,
        environment=environment,
        capture_output=False,
        runner=runner,
    )
    suffix = ".exe" if os.name == "nt" else ""
    executable = install_root / "bin" / f"{binary}{suffix}"
    result = _run_command(
        (str(executable), "--version"),
        cwd=work_root,
        environment=environment,
        capture_output=True,
        runner=runner,
    )
    expected = f"{binary} {expected_version}"
    if result.stdout.strip() != expected or result.stderr.strip():
        raise ExternalConsumerError(
            f"installed {binary} version output drifted: "
            f"stdout={result.stdout!r}, stderr={result.stderr!r}, expected={expected!r}"
        )


def validate_external_consumers(
    artifacts_directory: Path,
    *,
    expected_release_version: str,
    cargo_executable: str = "cargo",
    toolchain: str | None = None,
    runner: CommandRunner = subprocess.run,
) -> dict[str, Any]:
    """Run the complete packaged Cargo external-consumer acceptance lane."""
    try:
        inventory = load_inventory()
    except InventoryError as error:
        raise ExternalConsumerError(f"Cargo release inventory is invalid: {error}") from error
    if expected_release_version != inventory.release_version:
        raise ExternalConsumerError(
            "external-consumer release identity disagrees with the Cargo inventory: "
            f"actual={expected_release_version!r}, expected={inventory.release_version!r}"
        )
    cargo = cargo_prefix(cargo=cargo_executable, toolchain=toolchain)
    with tempfile.TemporaryDirectory(prefix="type-bridge-cargo-consumers-") as temporary:
        work_root = Path(temporary)
        archive_root = work_root / "archives"
        archive_root.mkdir()
        extracted = extract_public_archives(
            artifacts_directory.absolute(),
            inventory=inventory,
            destination=archive_root,
        )
        write_patch_config(work_root, extracted)
        environment = os.environ.copy()
        environment["CARGO_TARGET_DIR"] = str(work_root / "target")

        surface_root = work_root / "surface-consumer"
        surface_manifest = write_surface_consumer(surface_root, inventory=inventory)
        surface_metadata = load_metadata(
            surface_manifest,
            cargo=cargo,
            environment=environment,
            runner=runner,
        )
        library_names = validate_metadata(
            surface_metadata,
            consumer_manifest=surface_manifest,
            direct_dependencies={
                package.name: package.version for package in inventory.first_party_packages
            },
            required_packages=inventory.public_packages,
            extracted=extracted,
        )
        write_surface_source(surface_root, library_names)
        _cargo_check(
            surface_manifest,
            cargo=cargo,
            environment=environment,
            runner=runner,
        )

        server = extracted["type-bridge-server"]
        for label, features, expected_v2 in (
            ("server-no-default", (), False),
            ("server-v2-only", ("v2-query",), True),
        ):
            consumer_root = work_root / label
            manifest = write_server_consumer(
                consumer_root,
                version=inventory.release_version,
                features=features,
            )
            metadata = load_metadata(
                manifest,
                cargo=cargo,
                environment=environment,
                runner=runner,
            )
            validate_metadata(
                metadata,
                consumer_manifest=manifest,
                direct_dependencies={"type-bridge-server": inventory.release_version},
                required_packages=(server.package,),
                extracted=extracted,
            )
            validate_server_features(
                metadata,
                version=inventory.release_version,
                expected_v2=expected_v2,
            )
            _cargo_check(
                manifest,
                cargo=cargo,
                environment=environment,
                runner=runner,
            )

        install_root = work_root / "installed"
        _install_and_run_binary(
            cargo=cargo,
            package_root=extracted["type-bridge-cli"].root,
            binary="type-bridge",
            expected_version=inventory.release_version,
            install_root=install_root,
            work_root=work_root,
            environment=environment,
            runner=runner,
        )
        _install_and_run_binary(
            cargo=cargo,
            package_root=server.root,
            binary="type-bridge-server",
            expected_version=inventory.release_version,
            install_root=install_root,
            work_root=work_root,
            environment=environment,
            runner=runner,
        )

    return {
        "archives": len(inventory.public_packages),
        "first_party_packages": len(inventory.first_party_packages),
        "immutable_packages": len(inventory.immutable_packages),
        "release_version": inventory.release_version,
        "status": "ok",
    }


def validate_candidate_external_consumers(
    candidate_bundle: Path,
    *,
    expected_release_version: str,
    expected_manifest_sha256: str | None = None,
    cargo_executable: str = "cargo",
    toolchain: str | None = None,
    runner: CommandRunner = subprocess.run,
) -> dict[str, Any]:
    """Accept external consumers from a checksum-bound candidate bundle."""
    try:
        candidate = validate_candidate_bundle(
            candidate_bundle,
            expected_release_version=expected_release_version,
            expected_manifest_sha256=expected_manifest_sha256,
        )
    except CandidateError as error:
        raise ExternalConsumerError(f"Cargo candidate bundle is invalid: {error}") from error
    report = validate_external_consumers(
        candidate.root,
        expected_release_version=expected_release_version,
        cargo_executable=cargo_executable,
        toolchain=toolchain,
        runner=runner,
    )
    report["candidate_manifest_sha256"] = candidate.manifest_sha256
    return report


def build_parser() -> argparse.ArgumentParser:
    """Build the packaged Cargo external-consumer CLI."""
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group()
    source.add_argument("--artifacts-dir", type=Path)
    source.add_argument("--candidate-bundle", type=Path)
    parser.add_argument("--expected-release-version", required=True)
    parser.add_argument("--expected-manifest-sha256")
    parser.add_argument("--cargo", default="cargo")
    parser.add_argument("--toolchain")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Run acceptance and print its machine-readable report."""
    args = build_parser().parse_args(argv)
    if args.candidate_bundle is not None:
        report = validate_candidate_external_consumers(
            args.candidate_bundle,
            expected_release_version=args.expected_release_version,
            expected_manifest_sha256=args.expected_manifest_sha256,
            cargo_executable=args.cargo,
            toolchain=args.toolchain,
        )
    else:
        if args.expected_manifest_sha256 is not None:
            raise ExternalConsumerError("--expected-manifest-sha256 requires --candidate-bundle")
        report = validate_external_consumers(
            args.artifacts_dir or DEFAULT_ARTIFACTS_DIRECTORY,
            expected_release_version=args.expected_release_version,
            cargo_executable=args.cargo,
            toolchain=args.toolchain,
        )
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ExternalConsumerError as error:
        print(f"Packaged Cargo external-consumer acceptance failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
