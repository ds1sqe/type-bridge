#!/usr/bin/env python3
"""Require band 9 to bind the newest stable official 3.12.x driver and protocol."""

from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

CRATE_NAME = "typedb-driver"
PROTOCOL_CRATE_NAME = "typedb-protocol"
EXPECTED_SERIES = (3, 12)
CRATES_IO_URL = f"https://crates.io/api/v1/crates/{CRATE_NAME}"
PROTOCOL_CRATES_IO_URL = f"https://crates.io/api/v1/crates/{PROTOCOL_CRATE_NAME}"
CRATES_IO_LOCK_SOURCE = "registry+https://github.com/rust-lang/crates.io-index"
MAX_METADATA_BYTES = 4 * 1024 * 1024
STABLE_VERSION = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
REGISTRY_DEPENDENCY_FIELDS = frozenset({"version", "optional", "default-features", "features"})


class ValidationError(RuntimeError):
    """The local pin or upstream release metadata violates the band-9 contract."""


def _read_json(path: Path) -> object:
    if not path.is_file() or path.is_symlink():
        raise ValidationError(f"metadata fixture is missing or non-regular: {path}")
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValidationError(f"could not read metadata fixture {path}: {error}") from error


def _download_json(url: str, description: str) -> object:
    request = urllib.request.Request(
        url,
        headers={"User-Agent": "ds1sqe/type-bridge release-pin-validator"},
    )
    try:
        with urllib.request.urlopen(request, timeout=20) as response:  # noqa: S310
            declared_length = response.headers.get("Content-Length")
            if declared_length is not None and int(declared_length) > MAX_METADATA_BYTES:
                raise ValidationError("crates.io metadata response exceeds the byte budget")
            payload = response.read(MAX_METADATA_BYTES + 1)
    except (OSError, ValueError, urllib.error.URLError) as error:
        raise ValidationError(f"could not query crates.io for {description}: {error}") from error
    if len(payload) > MAX_METADATA_BYTES:
        raise ValidationError("crates.io metadata response exceeds the byte budget")
    try:
        return json.loads(payload)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValidationError(
            f"crates.io returned invalid JSON for {description}: {error}"
        ) from error


def _stable_tuple(value: object) -> tuple[int, int, int] | None:
    if not isinstance(value, str):
        return None
    match = STABLE_VERSION.fullmatch(value)
    if match is None:
        return None
    try:
        return tuple(int(component) for component in match.groups())  # type: ignore[return-value]
    except ValueError:
        return None


def _read_toml(path: Path, description: str) -> dict[str, Any]:
    if not path.is_file() or path.is_symlink():
        raise ValidationError(f"{description} is missing or non-regular: {path}")
    try:
        document = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        raise ValidationError(f"could not parse {description} {path}: {error}") from error
    if not isinstance(document, dict):
        raise ValidationError(f"{description} is not a TOML table: {path}")
    return document


def _driver_dependency_keys(dependencies: dict[str, Any]) -> set[str]:
    keys: set[str] = set()
    for key, value in dependencies.items():
        package = value.get("package", key) if isinstance(value, dict) else key
        if not isinstance(package, str):
            continue
        normalized = package.lower().replace("_", "-")
        if normalized == CRATE_NAME or normalized.startswith("type-bridge-typedb-driver-"):
            keys.add(key)
    return keys


def _feature_activated_driver_dependencies(
    features: dict[str, Any],
    dependencies: dict[str, Any],
    root: str,
) -> set[str]:
    driver_dependencies = _driver_dependency_keys(dependencies)
    activated: set[str] = set()
    pending = [root]
    visited: set[str] = set()
    while pending:
        feature = pending.pop()
        if feature in visited:
            continue
        visited.add(feature)
        entries = features.get(feature)
        if not isinstance(entries, list) or not all(isinstance(entry, str) for entry in entries):
            raise ValidationError(f"feature {feature!r} must be an array of feature activations")
        for entry in entries:
            dependency: str | None = None
            if entry.startswith("dep:"):
                dependency = entry.removeprefix("dep:")
            elif "/" in entry:
                dependency, _, _ = entry.partition("/")
                if dependency.endswith("?"):
                    continue
            elif entry in features:
                pending.append(entry)
                continue
            elif entry in dependencies:
                dependency = entry
            if dependency in driver_dependencies:
                activated.add(dependency)
    return activated


def _validate_band9_feature(manifest: dict[str, Any], dependencies: dict[str, Any]) -> None:
    features = manifest.get("features")
    if not isinstance(features, dict):
        raise ValidationError("typedb-runtime manifest has no [features] table")
    default = features.get("default")
    if not isinstance(default, list) or "band9" not in default:
        raise ValidationError("typedb-runtime default features must include band9")
    band9 = features.get("band9")
    if not isinstance(band9, list) or band9.count(f"dep:{CRATE_NAME}") != 1:
        raise ValidationError(f"band9 must directly activate exactly dep:{CRATE_NAME}")
    activated = _feature_activated_driver_dependencies(features, dependencies, "band9")
    if activated != {CRATE_NAME}:
        rendered = ", ".join(sorted(activated)) or "none"
        raise ValidationError(
            f"band9 must activate only the official {CRATE_NAME} dependency; activated: {rendered}"
        )


def _manifest_pin(path: Path) -> tuple[str, tuple[int, int, int]]:
    manifest = _read_toml(path, "typedb-runtime manifest")
    dependencies = manifest.get("dependencies")
    if not isinstance(dependencies, dict) or CRATE_NAME not in dependencies:
        raise ValidationError(
            f"band 9 must use the exact dependency key {CRATE_NAME!r}; aliases are not allowed"
        )
    aliases = [
        key for key, value in dependencies.items() if key != CRATE_NAME and _is_driver_alias(value)
    ]
    if aliases:
        raise ValidationError(
            f"band 9 must use only the exact dependency key {CRATE_NAME!r}; "
            f"package aliases are not allowed: {', '.join(sorted(aliases))}"
        )
    dependency: Any = dependencies[CRATE_NAME]
    if isinstance(dependency, dict):
        unsupported = sorted(set(dependency) - REGISTRY_DEPENDENCY_FIELDS)
        if unsupported:
            fields = ", ".join(unsupported)
            raise ValidationError(
                f"{CRATE_NAME} must resolve directly from the default crates.io registry; "
                f"unsupported dependency fields: {fields}"
            )
        requirement = dependency.get("version")
    else:
        requirement = dependency
    if not isinstance(requirement, str) or not requirement.startswith("="):
        raise ValidationError(
            f"band 9 must use one concrete exact {CRATE_NAME} pin, not {requirement!r}"
        )
    version = requirement[1:]
    parsed = _stable_tuple(version)
    if parsed is None or parsed[:2] != EXPECTED_SERIES:
        raise ValidationError(
            f"band 9 must exact-pin a stable 3.12.x {CRATE_NAME} release, not {requirement!r}"
        )
    _validate_band9_feature(manifest, dependencies)
    return version, parsed


def _is_driver_alias(value: object) -> bool:
    return isinstance(value, dict) and value.get("package") == CRATE_NAME


def _reject_workspace_overrides(lock_path: Path) -> None:
    workspace_root = lock_path.parent
    workspace_manifest = workspace_root / "Cargo.toml"
    workspace = _read_toml(workspace_manifest, "workspace manifest")

    patches = workspace.get("patch")
    if isinstance(patches, dict):
        for source, source_patches in patches.items():
            if not isinstance(source_patches, dict):
                continue
            for key, value in source_patches.items():
                if key == CRATE_NAME or _is_driver_alias(value):
                    raise ValidationError(
                        f"workspace {workspace_manifest} overrides the official crates.io "
                        f"{CRATE_NAME} package through [patch.{source}]"
                    )

    replacements = workspace.get("replace")
    if isinstance(replacements, dict) and replacements:
        # Cargo accepts both short ``name:version`` keys and fully-qualified
        # package IDs here.  Reject the deprecated mechanism wholesale rather
        # than trying to duplicate Cargo's package-ID parser and leaving a
        # source-substitution bypass.
        raise ValidationError(
            f"workspace {workspace_manifest} uses [replace]; {CRATE_NAME} source policy "
            "requires direct crates.io resolution"
        )

    config_roots = [workspace_root]
    try:
        repository_root = Path.cwd().resolve(strict=True)
        resolved_workspace = workspace_root.resolve(strict=True)
    except OSError as error:
        raise ValidationError(
            f"could not resolve workspace source-policy paths: {error}"
        ) from error
    if resolved_workspace.is_relative_to(repository_root):
        parent = resolved_workspace.parent
        while parent.is_relative_to(repository_root):
            config_roots.append(parent)
            if parent == repository_root:
                break
            parent = parent.parent

    for root in config_roots:
        for filename in ("config.toml", "config"):
            config_path = root / ".cargo" / filename
            if not config_path.exists() and not config_path.is_symlink():
                continue
            config = _read_toml(config_path, "Cargo source configuration")
            if "include" in config:
                raise ValidationError(
                    f"Cargo source configuration {config_path} includes another config; "
                    f"{CRATE_NAME} source policy must be self-contained"
                )
            if "paths" in config:
                raise ValidationError(
                    f"Cargo source configuration {config_path} supplies local package paths; "
                    f"{CRATE_NAME} must use the official registry directly"
                )
            registry = config.get("registry")
            if isinstance(registry, dict) and "default" in registry:
                raise ValidationError(
                    f"Cargo source configuration {config_path} changes the default registry; "
                    "release publication must remain bound to crates.io"
                )
            sources = config.get("source")
            if isinstance(sources, dict) and "crates-io" in sources:
                raise ValidationError(
                    f"Cargo source configuration {config_path} overrides crates.io; "
                    f"{CRATE_NAME} must use the official registry directly"
                )
            registries = config.get("registries")
            crates_io_registry = (
                registries.get("crates-io") if isinstance(registries, dict) else None
            )
            if isinstance(crates_io_registry, dict) and "index" in crates_io_registry:
                raise ValidationError(
                    f"Cargo source configuration {config_path} supplies a custom crates.io "
                    f"index; {CRATE_NAME} must use the official registry directly"
                )
            config_patches = config.get("patch")
            if isinstance(config_patches, dict):
                for source_patches in config_patches.values():
                    if not isinstance(source_patches, dict):
                        continue
                    for key, value in source_patches.items():
                        if key == CRATE_NAME or _is_driver_alias(value):
                            raise ValidationError(
                                f"Cargo source configuration {config_path} patches the "
                                f"official crates.io {CRATE_NAME} package"
                            )


def _locked_package(
    packages: list[object],
    *,
    crate: str,
    version: str,
    upstream_checksum: str,
) -> dict[str, Any]:
    crate_packages = [
        package
        for package in packages
        if isinstance(package, dict) and package.get("name") == crate
    ]
    if len(crate_packages) != 1:
        raise ValidationError(
            f"workspace Cargo.lock must contain exactly one {crate} package, "
            f"found {len(crate_packages)}"
        )
    package = crate_packages[0]
    if package.get("version") != version:
        if crate == CRATE_NAME:
            raise ValidationError(
                f"workspace Cargo.lock resolves {crate} {package.get('version')!r}, "
                f"but band 9 pins ={version}"
            )
        raise ValidationError(
            f"workspace Cargo.lock resolves {crate} {package.get('version')!r}, "
            f"but the official {CRATE_NAME} selects ={version}"
        )
    if package.get("source") != CRATES_IO_LOCK_SOURCE:
        raise ValidationError(
            f"workspace Cargo.lock does not bind {crate} ={version} to the official "
            "crates.io source"
        )
    checksum = package.get("checksum")
    if not isinstance(checksum, str) or not checksum.strip():
        raise ValidationError(
            f"workspace Cargo.lock has no crates.io checksum for {crate} ={version}"
        )
    if SHA256.fullmatch(checksum) is None:
        raise ValidationError(
            f"workspace Cargo.lock has no canonical crates.io SHA-256 checksum for "
            f"{crate} ={version}"
        )
    if checksum != upstream_checksum:
        raise ValidationError(
            f"workspace Cargo.lock checksum for {crate} ={version} does not match the "
            "canonical crates.io checksum"
        )
    return package


def _validate_protocol_lock_edge(
    driver: dict[str, Any],
    *,
    protocol_version: str,
) -> None:
    dependencies = driver.get("dependencies")
    if not isinstance(dependencies, list) or not all(
        isinstance(dependency, str) for dependency in dependencies
    ):
        raise ValidationError(
            f"workspace Cargo.lock {CRATE_NAME} row has no canonical dependency array"
        )
    matches = [
        dependency
        for dependency in dependencies
        if dependency == PROTOCOL_CRATE_NAME or dependency.startswith(f"{PROTOCOL_CRATE_NAME} ")
    ]
    if len(matches) != 1:
        raise ValidationError(
            f"workspace Cargo.lock {CRATE_NAME} row must contain exactly one "
            f"{PROTOCOL_CRATE_NAME} dependency edge, found {len(matches)}"
        )
    accepted = {
        PROTOCOL_CRATE_NAME,
        f"{PROTOCOL_CRATE_NAME} {protocol_version} ({CRATES_IO_LOCK_SOURCE})",
    }
    if matches[0] not in accepted:
        raise ValidationError(
            f"workspace Cargo.lock {CRATE_NAME} dependency edge does not resolve the "
            f"official {PROTOCOL_CRATE_NAME} ={protocol_version} row"
        )


def _validate_lock(
    path: Path,
    *,
    driver_version: str,
    driver_checksum: str,
    protocol_version: str,
    protocol_checksum: str,
) -> None:
    lock = _read_toml(path, "workspace Cargo.lock")
    packages = lock.get("package")
    if packages is None:
        packages = []
    elif not isinstance(packages, list):
        raise ValidationError(f"workspace Cargo.lock has no package array: {path}")
    driver = _locked_package(
        packages,
        crate=CRATE_NAME,
        version=driver_version,
        upstream_checksum=driver_checksum,
    )
    _locked_package(
        packages,
        crate=PROTOCOL_CRATE_NAME,
        version=protocol_version,
        upstream_checksum=protocol_checksum,
    )
    _validate_protocol_lock_edge(driver, protocol_version=protocol_version)


def _latest_upstream(
    metadata: object,
) -> tuple[str, tuple[int, int, int], dict[str, Any]]:
    if not isinstance(metadata, dict) or not isinstance(metadata.get("versions"), list):
        raise ValidationError("crates.io metadata has no versions array")
    candidates: list[tuple[tuple[int, int, int], str, dict[str, Any]]] = []
    for entry in metadata["versions"]:
        if not isinstance(entry, dict) or entry.get("yanked") is not False:
            continue
        version = entry.get("num")
        parsed = _stable_tuple(version)
        if parsed is not None and parsed[:2] == EXPECTED_SERIES:
            candidates.append((parsed, version, entry))
    if not candidates:
        raise ValidationError("crates.io reports no non-yanked stable typedb-driver 3.12.x release")
    latest = max(candidate[0] for candidate in candidates)
    selected = [candidate for candidate in candidates if candidate[0] == latest]
    if len(selected) != 1:
        raise ValidationError(
            "crates.io metadata must contain exactly one record for the newest non-yanked "
            f"stable {CRATE_NAME} 3.12.x release, found {len(selected)}"
        )
    parsed, version, entry = selected[0]
    return version, parsed, entry


def _version_checksum(
    entry: dict[str, Any],
    *,
    crate: str,
    version: str,
    expected_license: str,
) -> str:
    if entry.get("yanked") is not False:
        raise ValidationError(f"crates.io reports {crate} ={version} as yanked")
    if entry.get("license") != expected_license:
        raise ValidationError(
            f"crates.io reports {crate} ={version} license {entry.get('license')!r}; "
            f"expected {expected_license}"
        )
    checksum = entry.get("checksum")
    if not isinstance(checksum, str) or SHA256.fullmatch(checksum) is None:
        raise ValidationError(
            f"crates.io reports no canonical SHA-256 checksum for {crate} ={version}"
        )
    return checksum


def _protocol_requirement(metadata: object) -> str:
    if not isinstance(metadata, dict) or not isinstance(metadata.get("dependencies"), list):
        raise ValidationError(f"crates.io metadata has no {CRATE_NAME} dependencies array")
    edges = [
        entry
        for entry in metadata["dependencies"]
        if isinstance(entry, dict) and entry.get("crate_id") == PROTOCOL_CRATE_NAME
    ]
    if len(edges) != 1:
        raise ValidationError(
            f"crates.io metadata for {CRATE_NAME} must contain exactly one "
            f"{PROTOCOL_CRATE_NAME} dependency, found {len(edges)}"
        )
    edge = edges[0]
    if edge.get("optional") is not False or edge.get("kind") != "normal":
        raise ValidationError(
            f"crates.io metadata for {CRATE_NAME} must select {PROTOCOL_CRATE_NAME} "
            "through one nonoptional normal dependency"
        )
    requirement = edge.get("req")
    if not isinstance(requirement, str) or not requirement.startswith("="):
        raise ValidationError(
            f"crates.io metadata for {CRATE_NAME} must exact-pin {PROTOCOL_CRATE_NAME}, "
            f"not {requirement!r}"
        )
    version = requirement[1:]
    if _stable_tuple(version) is None:
        raise ValidationError(
            f"crates.io metadata for {CRATE_NAME} must exact-pin a stable "
            f"{PROTOCOL_CRATE_NAME} release, not {requirement!r}"
        )
    return version


def _protocol_upstream(metadata: object, version: str) -> dict[str, Any]:
    if not isinstance(metadata, dict) or not isinstance(metadata.get("versions"), list):
        raise ValidationError(f"crates.io metadata for {PROTOCOL_CRATE_NAME} has no versions array")
    selected = [
        entry
        for entry in metadata["versions"]
        if isinstance(entry, dict) and entry.get("num") == version
    ]
    if len(selected) != 1:
        raise ValidationError(
            f"crates.io metadata must contain exactly one {PROTOCOL_CRATE_NAME} ={version} "
            f"record, found {len(selected)}"
        )
    return selected[0]


def _driver_upstream(metadata: object, version: str) -> dict[str, Any]:
    if not isinstance(metadata, dict) or not isinstance(metadata.get("versions"), list):
        raise ValidationError(f"crates.io metadata for {CRATE_NAME} has no versions array")
    selected = [
        entry
        for entry in metadata["versions"]
        if isinstance(entry, dict) and entry.get("num") == version
    ]
    if len(selected) != 1:
        raise ValidationError(
            f"crates.io metadata must contain exactly one {CRATE_NAME} ={version} "
            f"record, found {len(selected)}"
        )
    return selected[0]


def _metadata_documents(
    metadata_path: Path | None,
    dependencies_path: Path | None,
    protocol_metadata_path: Path | None,
    driver_version: str,
) -> tuple[object, object, object]:
    fixture_paths = (metadata_path, dependencies_path, protocol_metadata_path)
    if any(path is not None for path in fixture_paths):
        if not all(path is not None for path in fixture_paths):
            raise ValidationError(
                "fixture mode requires --metadata, --dependencies, and --protocol-metadata"
            )
        assert metadata_path is not None
        assert dependencies_path is not None
        assert protocol_metadata_path is not None
        return (
            _read_json(metadata_path),
            _read_json(dependencies_path),
            _read_json(protocol_metadata_path),
        )

    driver_metadata = _download_json(CRATES_IO_URL, CRATE_NAME)
    dependencies = _download_json(
        f"{CRATES_IO_URL}/{driver_version}/dependencies",
        f"{CRATE_NAME} ={driver_version} dependencies",
    )
    protocol_metadata = _download_json(PROTOCOL_CRATES_IO_URL, PROTOCOL_CRATE_NAME)
    return driver_metadata, dependencies, protocol_metadata


def validate(
    manifest: Path,
    lock: Path,
    driver_metadata: object,
    dependency_metadata: object,
    protocol_metadata: object,
    *,
    committed_cutoff: bool = False,
) -> str:
    """Return the accepted exact version, or raise ``ValidationError``."""
    pin, parsed_pin = _manifest_pin(manifest)
    _reject_workspace_overrides(lock)
    if not committed_cutoff:
        latest, parsed_latest, _ = _latest_upstream(driver_metadata)
        if parsed_pin != parsed_latest:
            raise ValidationError(
                f"band 9 pins {CRATE_NAME} ={pin}, but the newest non-yanked stable "
                f"3.12.x release is {latest}"
            )
    driver_entry = _driver_upstream(driver_metadata, pin)
    driver_checksum = _version_checksum(
        driver_entry,
        crate=CRATE_NAME,
        version=pin,
        expected_license="Apache-2.0",
    )
    protocol_version = _protocol_requirement(dependency_metadata)
    protocol_entry = _protocol_upstream(protocol_metadata, protocol_version)
    protocol_checksum = _version_checksum(
        protocol_entry,
        crate=PROTOCOL_CRATE_NAME,
        version=protocol_version,
        expected_license="MPL-2.0",
    )
    _validate_lock(
        lock,
        driver_version=pin,
        driver_checksum=driver_checksum,
        protocol_version=protocol_version,
        protocol_checksum=protocol_checksum,
    )
    return pin


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--manifest",
        type=Path,
        default=Path("type-bridge-core/crates/typedb-runtime/Cargo.toml"),
    )
    parser.add_argument(
        "--metadata",
        type=Path,
        help="read crates.io driver-version JSON from a file instead of the live API",
    )
    parser.add_argument(
        "--dependencies",
        type=Path,
        help="read crates.io driver-dependency JSON from a file instead of the live API",
    )
    parser.add_argument(
        "--protocol-metadata",
        type=Path,
        help="read crates.io protocol-version JSON from a file instead of the live API",
    )
    parser.add_argument(
        "--lock",
        type=Path,
        default=Path("type-bridge-core/Cargo.lock"),
        help="workspace Cargo.lock whose official registry resolution must match the pin",
    )
    parser.add_argument(
        "--committed-cutoff",
        action="store_true",
        help=(
            "accept the committed exact official pin after the release graph cutoff exists; "
            "all provenance checks remain mandatory"
        ),
    )
    arguments = parser.parse_args(argv)
    try:
        selected_pin, _ = _manifest_pin(arguments.manifest)
        driver_metadata, dependency_metadata, protocol_metadata = _metadata_documents(
            arguments.metadata,
            arguments.dependencies,
            arguments.protocol_metadata,
            selected_pin,
        )
        pin = validate(
            arguments.manifest,
            arguments.lock,
            driver_metadata,
            dependency_metadata,
            protocol_metadata,
            committed_cutoff=arguments.committed_cutoff,
        )
    except ValidationError as error:
        print(f"typedb-driver pin validation failed: {error}", file=sys.stderr)
        return 1
    relationship = (
        "committed-cutoff official" if arguments.committed_cutoff else "newest stable upstream"
    )
    print(
        f"band 9 exact-pins {relationship} {CRATE_NAME} 3.12.x: ={pin}; "
        f"official {PROTOCOL_CRATE_NAME} provenance is bound"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
