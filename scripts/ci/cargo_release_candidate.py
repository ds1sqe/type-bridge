#!/usr/bin/env python3
"""Build and validate the byte-exact Cargo release-candidate bundle."""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import re
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import urllib.error
import urllib.request
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

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
CORE_WORKSPACE = REPOSITORY_ROOT / "type-bridge-core"
BUNDLE_MANIFEST = "cargo-release-candidate.json"
BUNDLE_SCHEMA_VERSION = 1
CRATES_IO_SOURCE = "registry+https://github.com/rust-lang/crates.io-index"
# crates.io rejects `.crate` uploads larger than 10 MiB. Keep the accepted
# bundle at the registry's exact ceiling so an oversized later package cannot
# be discovered only after earlier packages have been published.
MAX_ARCHIVE_BYTES = 10 * 1024 * 1024
MAX_REQUEST_BYTES = MAX_ARCHIVE_BYTES + 4 * 1024 * 1024
MAX_ARCHIVE_FILES = 20_000
MAX_ARCHIVE_MEMBER_BYTES = 64 * 1024 * 1024
MAX_ARCHIVE_EXPANDED_BYTES = 256 * 1024 * 1024
CommandRunner = Callable[..., subprocess.CompletedProcess[str]]


class CandidateError(RuntimeError):
    """The Cargo candidate could not be built or failed its byte contract."""


@dataclass(frozen=True)
class CandidatePackage:
    """One checksum-bound package in an accepted candidate bundle."""

    package: CargoInventoryPackage
    archive: Path
    archive_sha256: str
    request_body: Path | None
    request_body_sha256: str | None
    metadata_sha256: str | None


@dataclass(frozen=True)
class CandidateBundle:
    """A validated closed candidate bundle and its canonical manifest digest."""

    root: Path
    manifest: Path
    manifest_sha256: str
    packages: tuple[CandidatePackage, ...]


def sha256_bytes(body: bytes) -> str:
    """Return the lowercase SHA-256 of one immutable body."""
    return hashlib.sha256(body).hexdigest()


def canonical_json_bytes(payload: object, *, newline: bool = False) -> bytes:
    """Serialize one JSON value with a single stable byte representation."""
    body = json.dumps(
        payload,
        ensure_ascii=False,
        allow_nan=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    return body + (b"\n" if newline else b"")


def _read_regular(path: Path, *, maximum: int, label: str) -> bytes:
    """Read one bounded regular file without following a symlink."""
    try:
        file_stat = path.lstat()
    except OSError as error:
        raise CandidateError(f"could not inspect {label} {path}: {error}") from error
    if stat.S_ISLNK(file_stat.st_mode) or not stat.S_ISREG(file_stat.st_mode):
        raise CandidateError(f"{label} is linked or non-regular: {path}")
    if file_stat.st_size < 0 or file_stat.st_size > maximum:
        raise CandidateError(
            f"{label} exceeds its byte budget: path={path}, size={file_stat.st_size}, "
            f"maximum={maximum}"
        )
    try:
        body = path.read_bytes()
    except OSError as error:
        raise CandidateError(f"could not read {label} {path}: {error}") from error
    if len(body) != file_stat.st_size:
        raise CandidateError(f"{label} changed while it was read: {path}")
    return body


def safe_crate_files(
    archive: bytes,
    *,
    package: CargoInventoryPackage,
) -> dict[PurePosixPath, bytes]:
    """Read an exact ``.crate`` without extracting attacker-controlled paths."""
    if len(archive) > MAX_ARCHIVE_BYTES:
        raise CandidateError(f"archive for {package.name} exceeds its byte budget")
    root = f"{package.name}-{package.version}"
    files: dict[PurePosixPath, bytes] = {}
    seen: set[str] = set()
    expanded = 0
    try:
        with tarfile.open(fileobj=io.BytesIO(archive), mode="r|gz") as crate:
            for count, member in enumerate(crate, start=1):
                if count > MAX_ARCHIVE_FILES:
                    raise CandidateError(f"archive for {package.name} has too many members")
                name = member.name
                if "\\" in name or name.startswith("/") or "\x00" in name:
                    raise CandidateError(
                        f"archive for {package.name} contains an unsafe path: {name!r}"
                    )
                normalized = name.rstrip("/")
                parts = normalized.split("/")
                if (
                    not normalized
                    or any(part in ("", ".", "..") for part in parts)
                    or parts[0] != root
                ):
                    raise CandidateError(
                        f"archive for {package.name} contains an unsafe path: {name!r}"
                    )
                if normalized in seen:
                    raise CandidateError(
                        f"archive for {package.name} contains a duplicate member: {name!r}"
                    )
                seen.add(normalized)
                if member.isdir():
                    continue
                if not member.isfile() or len(parts) == 1:
                    raise CandidateError(
                        f"archive for {package.name} contains a linked or invalid member: {name!r}"
                    )
                if member.size < 0 or member.size > MAX_ARCHIVE_MEMBER_BYTES:
                    raise CandidateError(
                        f"archive member for {package.name} exceeds its byte budget: {name!r}"
                    )
                expanded += member.size
                if expanded > MAX_ARCHIVE_EXPANDED_BYTES:
                    raise CandidateError(
                        f"archive for {package.name} exceeds its expanded byte budget"
                    )
                source = crate.extractfile(member)
                if source is None:
                    raise CandidateError(
                        f"archive for {package.name} contains an unreadable member: {name!r}"
                    )
                body = source.read(member.size + 1)
                if len(body) != member.size:
                    raise CandidateError(f"archive member size disagrees with its body: {name!r}")
                relative = PurePosixPath(*parts[1:])
                if relative in files:
                    raise CandidateError(
                        f"archive for {package.name} contains a duplicate file: {relative}"
                    )
                files[relative] = body
    except CandidateError:
        raise
    except (EOFError, OSError, tarfile.TarError) as error:
        raise CandidateError(
            f"could not safely read archive for {package.name}: {error}"
        ) from error
    if PurePosixPath("Cargo.toml") not in files:
        raise CandidateError(f"archive for {package.name} has no Cargo.toml")
    return files


def _parse_toml(body: bytes, *, label: str) -> dict[str, Any]:
    try:
        payload = tomllib.loads(body.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        raise CandidateError(f"could not parse {label}: {error}") from error
    if not isinstance(payload, dict):
        raise CandidateError(f"{label} is not a TOML table")
    return payload


def _normalized_requirement(requirement: object, *, label: str) -> str:
    if not isinstance(requirement, str) or not requirement:
        raise CandidateError(f"{label} has no version requirement")
    return f"^{requirement}" if requirement[0].isdigit() else requirement


def _dependency_metadata_from_manifest(manifest: Mapping[str, Any]) -> list[dict[str, Any]]:
    """Independently translate normalized Cargo dependency tables to publish JSON."""
    dependencies: list[dict[str, Any]] = []

    def append_table(
        table: object,
        *,
        kind: str,
        target: str | None,
        label: str,
    ) -> None:
        if table is None:
            return
        if not isinstance(table, dict):
            raise CandidateError(f"packaged manifest [{label}] is not a table")
        for alias, raw in table.items():
            if not isinstance(alias, str) or not alias:
                raise CandidateError(f"packaged manifest [{label}] has an invalid dependency")
            if isinstance(raw, str):
                specification: dict[str, Any] = {"version": raw}
            elif isinstance(raw, dict):
                specification = raw
            else:
                raise CandidateError(
                    f"packaged manifest dependency {alias!r} in [{label}] is malformed"
                )
            if "path" in specification or "git" in specification:
                raise CandidateError(
                    f"packaged manifest dependency {alias!r} is not registry-normalized"
                )
            if specification.get("registry") is not None:
                raise CandidateError(
                    f"packaged manifest dependency {alias!r} targets another registry"
                )
            original = specification.get("package", alias)
            if not isinstance(original, str) or not original:
                raise CandidateError(f"packaged manifest dependency {alias!r} has no package name")
            features = specification.get("features", [])
            if not isinstance(features, list) or not all(
                isinstance(feature, str) for feature in features
            ):
                raise CandidateError(f"packaged manifest dependency {alias!r} has bad features")
            optional = specification.get("optional", False)
            default_features = specification.get("default-features", True)
            if not isinstance(optional, bool) or not isinstance(default_features, bool):
                raise CandidateError(f"packaged manifest dependency {alias!r} has bad booleans")
            dependencies.append(
                {
                    "default_features": default_features,
                    "explicit_name_in_toml": alias if original != alias else None,
                    "features": features,
                    "kind": kind,
                    "name": original,
                    "optional": optional,
                    "registry": None,
                    "target": target,
                    "version_req": _normalized_requirement(
                        specification.get("version"),
                        label=f"packaged dependency {alias!r}",
                    ),
                }
            )

    for section, kind in (
        ("dependencies", "normal"),
        ("dev-dependencies", "dev"),
        ("build-dependencies", "build"),
    ):
        append_table(manifest.get(section), kind=kind, target=None, label=section)
    targets = manifest.get("target")
    if targets is not None:
        if not isinstance(targets, dict):
            raise CandidateError("packaged manifest [target] is not a table")
        for target, target_table in targets.items():
            if not isinstance(target, str) or not isinstance(target_table, dict):
                raise CandidateError("packaged manifest has a malformed target table")
            for section, kind in (
                ("dependencies", "normal"),
                ("dev-dependencies", "dev"),
                ("build-dependencies", "build"),
            ):
                append_table(
                    target_table.get(section),
                    kind=kind,
                    target=target,
                    label=f"target.{target}.{section}",
                )
    return sorted(
        dependencies,
        key=lambda value: (
            str(value["kind"]),
            str(value["target"] or ""),
            str(value["explicit_name_in_toml"] or value["name"]),
            str(value["name"]),
            str(value["version_req"]),
        ),
    )


def publish_metadata_from_manifest(
    manifest: Mapping[str, Any],
    files: Mapping[PurePosixPath, bytes],
) -> dict[str, Any]:
    """Build the expected registry metadata directly from a normalized archive."""
    package = manifest.get("package")
    if not isinstance(package, dict):
        raise CandidateError("packaged manifest has no [package] table")
    readme_file = package.get("readme")
    if readme_file is not None and not isinstance(readme_file, str):
        raise CandidateError("packaged manifest package.readme is malformed")
    readme: str | None = None
    if readme_file is not None:
        readme_body = files.get(PurePosixPath(readme_file))
        if readme_body is None:
            raise CandidateError(f"packaged README is missing: {readme_file!r}")
        try:
            readme = readme_body.decode("utf-8")
        except UnicodeDecodeError as error:
            raise CandidateError(f"packaged README is not UTF-8: {readme_file!r}") from error
    features = manifest.get("features", {})
    badges = manifest.get("badges", {})
    if not isinstance(features, dict) or not isinstance(badges, dict):
        raise CandidateError("packaged features or badges are not tables")
    authors = package.get("authors", [])
    keywords = package.get("keywords", [])
    categories = package.get("categories", [])
    for label, value in (
        ("authors", authors),
        ("keywords", keywords),
        ("categories", categories),
    ):
        if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
            raise CandidateError(f"packaged package.{label} is malformed")
    return {
        "authors": authors,
        "badges": badges,
        "categories": categories,
        "deps": _dependency_metadata_from_manifest(manifest),
        "description": package.get("description"),
        "documentation": package.get("documentation"),
        "features": features,
        "homepage": package.get("homepage"),
        "keywords": keywords,
        "license": package.get("license"),
        "license_file": package.get("license-file"),
        "links": package.get("links"),
        "name": package.get("name"),
        "readme": readme,
        "readme_file": readme_file,
        "repository": package.get("repository"),
        "rust_version": package.get("rust-version"),
        "vers": package.get("version"),
    }


def publish_metadata_from_cargo(
    package: Mapping[str, Any],
    *,
    manifest: Mapping[str, Any],
    files: Mapping[PurePosixPath, bytes],
) -> dict[str, Any]:
    """Translate Cargo metadata into the registry publish-API object."""
    dependencies = package.get("dependencies")
    if not isinstance(dependencies, list):
        raise CandidateError("Cargo metadata dependencies are not an array")
    translated: list[dict[str, Any]] = []
    for dependency in dependencies:
        if not isinstance(dependency, dict):
            raise CandidateError("Cargo metadata contains a malformed dependency")
        source = dependency.get("source")
        registry = dependency.get("registry")
        if source != CRATES_IO_SOURCE or registry is not None or dependency.get("path") is not None:
            raise CandidateError(
                f"Cargo metadata dependency is not crates.io registry-form: "
                f"{dependency.get('name')!r}"
            )
        kind = dependency.get("kind") or "normal"
        if kind not in {"normal", "dev", "build"}:
            raise CandidateError(f"Cargo metadata dependency kind is invalid: {kind!r}")
        translated.append(
            {
                "default_features": dependency.get("uses_default_features"),
                "explicit_name_in_toml": dependency.get("rename"),
                "features": dependency.get("features"),
                "kind": kind,
                "name": dependency.get("name"),
                "optional": dependency.get("optional"),
                "registry": None,
                "target": dependency.get("target"),
                "version_req": dependency.get("req"),
            }
        )
    translated.sort(
        key=lambda value: (
            str(value["kind"]),
            str(value["target"] or ""),
            str(value["explicit_name_in_toml"] or value["name"]),
            str(value["name"]),
            str(value["version_req"]),
        )
    )
    expected = publish_metadata_from_manifest(manifest, files)
    cargo_payload = {
        "authors": package.get("authors", []),
        "badges": expected["badges"],
        "categories": package.get("categories", []),
        "deps": translated,
        "description": package.get("description"),
        "documentation": package.get("documentation"),
        "features": package.get("features", {}),
        "homepage": package.get("homepage"),
        "keywords": package.get("keywords", []),
        "license": package.get("license"),
        "license_file": expected["license_file"],
        "links": package.get("links"),
        "name": package.get("name"),
        "readme": expected["readme"],
        "readme_file": expected["readme_file"],
        "repository": package.get("repository"),
        "rust_version": package.get("rust_version"),
        "vers": package.get("version"),
    }
    if cargo_payload != expected:
        raise CandidateError(
            "Cargo metadata publish translation disagrees with the packaged manifest"
        )
    return cargo_payload


def encode_publish_request(metadata: Mapping[str, Any], archive: bytes) -> tuple[bytes, bytes]:
    """Return canonical metadata bytes and Cargo's exact publish request frame."""
    metadata_body = canonical_json_bytes(metadata)
    if len(metadata_body) > 0xFFFFFFFF or len(archive) > 0xFFFFFFFF:
        raise CandidateError("publish request component exceeds the u32 framing limit")
    body = (
        struct.pack("<I", len(metadata_body))
        + metadata_body
        + struct.pack("<I", len(archive))
        + archive
    )
    if len(body) > MAX_REQUEST_BYTES:
        raise CandidateError("publish request exceeds its byte budget")
    return metadata_body, body


def decode_publish_request(body: bytes) -> tuple[bytes, bytes]:
    """Strictly split one Cargo publish request into metadata and archive bytes."""
    if len(body) < 8 or len(body) > MAX_REQUEST_BYTES:
        raise CandidateError("publish request has an invalid size")
    metadata_length = struct.unpack("<I", body[:4])[0]
    metadata_end = 4 + metadata_length
    if metadata_end + 4 > len(body):
        raise CandidateError("publish request metadata length exceeds its body")
    archive_length = struct.unpack("<I", body[metadata_end : metadata_end + 4])[0]
    archive_start = metadata_end + 4
    if archive_start + archive_length != len(body):
        raise CandidateError("publish request archive length disagrees with its body")
    return body[4:metadata_end], body[archive_start:]


def _validate_metadata_bytes(
    metadata_body: bytes,
    *,
    archive: bytes,
    package: CargoInventoryPackage,
) -> dict[str, Any]:
    try:
        metadata = json.loads(metadata_body.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise CandidateError(
            f"publish metadata for {package.name} is invalid JSON: {error}"
        ) from error
    if not isinstance(metadata, dict):
        raise CandidateError(f"publish metadata for {package.name} is not an object")
    if canonical_json_bytes(metadata) != metadata_body:
        raise CandidateError(f"publish metadata for {package.name} is not canonical JSON")
    identity = (metadata.get("name"), metadata.get("vers"))
    if identity != (package.name, package.version):
        raise CandidateError(f"publish metadata identity drifted for {package.name}: {identity!r}")
    files = safe_crate_files(archive, package=package)
    manifest = _parse_toml(files[PurePosixPath("Cargo.toml")], label=package.name)
    expected = publish_metadata_from_manifest(manifest, files)
    if metadata != expected:
        raise CandidateError(
            f"publish metadata for {package.name} disagrees with its normalized archive"
        )
    return metadata


def _required_entry_string(entry: Mapping[str, Any], key: str, *, label: str) -> str:
    value = entry.get(key)
    if not isinstance(value, str) or not value:
        raise CandidateError(f"{label}.{key} must be a non-empty string")
    return value


def validate_candidate_bundle(
    root: Path,
    *,
    expected_release_version: str,
    expected_manifest_sha256: str | None = None,
    inventory: CargoReleaseInventory | None = None,
) -> CandidateBundle:
    """Validate the complete candidate before any registry operation."""
    inventory = inventory or load_inventory()
    if expected_release_version != inventory.release_version:
        raise CandidateError(
            "candidate release version disagrees with inventory: "
            f"actual={expected_release_version!r}, expected={inventory.release_version!r}"
        )
    try:
        root_stat = root.lstat()
    except OSError as error:
        raise CandidateError(f"could not inspect candidate bundle {root}: {error}") from error
    if stat.S_ISLNK(root_stat.st_mode) or not stat.S_ISDIR(root_stat.st_mode):
        raise CandidateError(f"candidate bundle is linked or non-directory: {root}")
    manifest_path = root / BUNDLE_MANIFEST
    manifest_body = _read_regular(
        manifest_path,
        maximum=4 * 1024 * 1024,
        label="candidate manifest",
    )
    manifest_sha256 = sha256_bytes(manifest_body)
    if expected_manifest_sha256 is not None:
        if re.fullmatch(r"[0-9a-f]{64}", expected_manifest_sha256) is None:
            raise CandidateError("expected candidate manifest SHA-256 is malformed")
        if manifest_sha256 != expected_manifest_sha256:
            raise CandidateError(
                "candidate manifest checksum mismatch: "
                f"actual={manifest_sha256}, expected={expected_manifest_sha256}"
            )
    try:
        payload = json.loads(manifest_body.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise CandidateError(f"candidate manifest is invalid JSON: {error}") from error
    if (
        not isinstance(payload, dict)
        or canonical_json_bytes(payload, newline=True) != manifest_body
    ):
        raise CandidateError("candidate manifest is not canonical newline-terminated JSON")
    required_root_keys = {
        "schema_version",
        "release_version",
        "cargo_toolchain",
        "inventory_sha256",
        "packages",
    }
    if set(payload) != required_root_keys:
        raise CandidateError(f"candidate manifest keys drifted: actual={sorted(payload)!r}")
    if payload.get("schema_version") != BUNDLE_SCHEMA_VERSION:
        raise CandidateError("candidate manifest schema version is unsupported")
    if payload.get("release_version") != inventory.release_version:
        raise CandidateError("candidate manifest release version drifted")
    if payload.get("cargo_toolchain") != inventory.candidate_toolchain:
        raise CandidateError("candidate manifest Cargo toolchain drifted")
    inventory_body = _read_regular(
        Path(__file__).with_name("cargo_release_inventory.toml"),
        maximum=1024 * 1024,
        label="Cargo release inventory",
    )
    if payload.get("inventory_sha256") != sha256_bytes(inventory_body):
        raise CandidateError("candidate manifest inventory checksum drifted")
    raw_entries = payload.get("packages")
    if not isinstance(raw_entries, list) or len(raw_entries) != len(inventory.public_packages):
        raise CandidateError("candidate manifest package inventory has the wrong size")

    expected_filenames = {BUNDLE_MANIFEST}
    accepted: list[CandidatePackage] = []
    entry_keys = {
        "archive",
        "archive_sha256",
        "archive_size",
        "classification",
        "metadata_sha256",
        "name",
        "publish_order",
        "request_body",
        "request_body_sha256",
        "request_body_size",
        "version",
    }
    for package, raw_entry in zip(inventory.public_packages, raw_entries, strict=True):
        if not isinstance(raw_entry, dict) or set(raw_entry) != entry_keys:
            raise CandidateError(f"candidate entry keys drifted for {package.name}")
        identity = (
            raw_entry.get("name"),
            raw_entry.get("version"),
            raw_entry.get("classification"),
            raw_entry.get("publish_order"),
        )
        expected_identity = (
            package.name,
            package.version,
            package.classification,
            package.publish_order,
        )
        if identity != expected_identity:
            raise CandidateError(
                f"candidate entry identity drifted: actual={identity!r}, "
                f"expected={expected_identity!r}"
            )
        archive_name = _required_entry_string(raw_entry, "archive", label=package.name)
        if archive_name != f"{package.name}-{package.version}.crate":
            raise CandidateError(f"candidate archive filename drifted for {package.name}")
        expected_filenames.add(archive_name)
        archive_path = root / archive_name
        archive = _read_regular(
            archive_path,
            maximum=MAX_ARCHIVE_BYTES,
            label=f"candidate archive for {package.name}",
        )
        archive_sha256 = sha256_bytes(archive)
        if (
            raw_entry.get("archive_size") != len(archive)
            or raw_entry.get("archive_sha256") != archive_sha256
        ):
            raise CandidateError(f"candidate archive checksum binding failed for {package.name}")
        safe_crate_files(archive, package=package)
        if package.immutable:
            if archive_sha256 != package.registry_checksum:
                raise CandidateError(
                    f"immutable registry archive checksum drifted for {package.name}"
                )
            if any(
                raw_entry.get(key) is not None
                for key in (
                    "metadata_sha256",
                    "request_body",
                    "request_body_sha256",
                    "request_body_size",
                )
            ):
                raise CandidateError(f"immutable package {package.name} has a publish request")
            request_path = None
            request_sha256 = None
            metadata_sha256 = None
        else:
            request_name = _required_entry_string(
                raw_entry,
                "request_body",
                label=package.name,
            )
            if request_name != f"{package.name}-{package.version}.put":
                raise CandidateError(f"candidate request filename drifted for {package.name}")
            expected_filenames.add(request_name)
            request_path = root / request_name
            request = _read_regular(
                request_path,
                maximum=MAX_REQUEST_BYTES,
                label=f"candidate publish request for {package.name}",
            )
            request_sha256 = sha256_bytes(request)
            if (
                raw_entry.get("request_body_size") != len(request)
                or raw_entry.get("request_body_sha256") != request_sha256
            ):
                raise CandidateError(
                    f"candidate publish-request checksum binding failed for {package.name}"
                )
            metadata_body, embedded_archive = decode_publish_request(request)
            if embedded_archive != archive:
                raise CandidateError(
                    f"candidate publish request does not embed the exact archive for {package.name}"
                )
            metadata_sha256 = sha256_bytes(metadata_body)
            if raw_entry.get("metadata_sha256") != metadata_sha256:
                raise CandidateError(
                    f"candidate metadata checksum binding failed for {package.name}"
                )
            _validate_metadata_bytes(metadata_body, archive=archive, package=package)
        accepted.append(
            CandidatePackage(
                package=package,
                archive=archive_path,
                archive_sha256=archive_sha256,
                request_body=request_path,
                request_body_sha256=request_sha256,
                metadata_sha256=metadata_sha256,
            )
        )

    actual_filenames: set[str] = set()
    for entry in root.iterdir():
        try:
            entry_stat = entry.lstat()
        except OSError as error:
            raise CandidateError(f"could not inspect candidate entry {entry}: {error}") from error
        if stat.S_ISLNK(entry_stat.st_mode) or not stat.S_ISREG(entry_stat.st_mode):
            raise CandidateError(f"candidate entry is linked or non-regular: {entry}")
        actual_filenames.add(entry.name)
    if actual_filenames != expected_filenames:
        raise CandidateError(
            "candidate file inventory drifted: "
            f"missing={sorted(expected_filenames - actual_filenames)!r}, "
            f"unexpected={sorted(actual_filenames - expected_filenames)!r}"
        )
    return CandidateBundle(
        root=root,
        manifest=manifest_path,
        manifest_sha256=manifest_sha256,
        packages=tuple(accepted),
    )


def _run(
    command: Sequence[str],
    *,
    cwd: Path,
    environment: Mapping[str, str],
    runner: CommandRunner,
    capture_output: bool = False,
) -> subprocess.CompletedProcess[str]:
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
        raise CandidateError(f"could not execute {command[0]!r}: {error}") from error
    if result.returncode != 0:
        details = ""
        if capture_output:
            details = f"\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        raise CandidateError(
            f"candidate command failed with exit {result.returncode}: {' '.join(command)}{details}"
        )
    return result


def _cargo_prefix(cargo: str, toolchain: str) -> tuple[str, ...]:
    if not cargo or cargo != cargo.strip():
        raise CandidateError(f"invalid Cargo executable: {cargo!r}")
    if re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", toolchain) is None:
        raise CandidateError(f"invalid exact Cargo toolchain: {toolchain!r}")
    return cargo, f"+{toolchain}"


def _download_registry_archive(package: CargoInventoryPackage) -> bytes:
    """Download one immutable crates.io archive with a hard byte bound."""
    url = f"https://crates.io/api/v1/crates/{package.name}/{package.version}/download"
    request = urllib.request.Request(
        url,
        headers={"User-Agent": "ds1sqe/type-bridge Cargo candidate builder"},
    )
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            length = response.headers.get("Content-Length")
            if length is not None and (not length.isdigit() or int(length) > MAX_ARCHIVE_BYTES):
                raise CandidateError(
                    f"registry archive Content-Length is invalid for {package.name}: {length!r}"
                )
            archive = response.read(MAX_ARCHIVE_BYTES + 1)
    except (OSError, urllib.error.URLError) as error:
        raise CandidateError(
            f"could not download registry archive for {package.name}: {error}"
        ) from error
    if len(archive) > MAX_ARCHIVE_BYTES:
        raise CandidateError(f"registry archive exceeds its byte budget for {package.name}")
    if sha256_bytes(archive) != package.registry_checksum:
        raise CandidateError(f"registry archive checksum mismatch for {package.name}")
    safe_crate_files(archive, package=package)
    return archive


def stage_archive(
    directory_source: Path,
    *,
    package: CargoInventoryPackage,
    archive: bytes,
) -> tuple[Path, dict[PurePosixPath, bytes]]:
    """Add exact archive contents and Cargo checksums to a directory source."""
    files = safe_crate_files(archive, package=package)
    root = directory_source / f"{package.name}-{package.version}"
    if root.exists() or root.is_symlink():
        raise CandidateError(f"directory source already contains staged package: {root.name}")
    root.mkdir()
    file_checksums: dict[str, str] = {}
    for relative, body in files.items():
        target = root.joinpath(*relative.parts)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(body)
        file_checksums[relative.as_posix()] = sha256_bytes(body)
    checksum = {
        "files": dict(sorted(file_checksums.items())),
        "package": sha256_bytes(archive),
    }
    (root / ".cargo-checksum.json").write_bytes(canonical_json_bytes(checksum))
    return root, files


def _validate_packaged_lock(
    files: Mapping[PurePosixPath, bytes],
    *,
    package: CargoInventoryPackage,
    staged_checksums: Mapping[tuple[str, str], str],
) -> None:
    lock_body = files.get(PurePosixPath("Cargo.lock"))
    if lock_body is None:
        raise CandidateError(f"packaged candidate {package.name} has no Cargo.lock")
    lock = _parse_toml(lock_body, label=f"{package.name} Cargo.lock")
    patch = lock.get("patch")
    if patch is not None:
        raise CandidateError(f"packaged candidate {package.name} contains patch state")
    raw_packages = lock.get("package")
    if not isinstance(raw_packages, list):
        raise CandidateError(f"packaged candidate {package.name} lock has no package list")
    by_identity = {
        (entry.get("name"), entry.get("version")): entry
        for entry in raw_packages
        if isinstance(entry, dict)
    }
    for identity, checksum in staged_checksums.items():
        entry = by_identity.get(identity)
        if entry is None:
            continue
        if entry.get("source") != CRATES_IO_SOURCE or entry.get("checksum") != checksum:
            raise CandidateError(
                f"packaged lock for {package.name} does not bind staged registry package "
                f"{identity[0]}@{identity[1]}"
            )


def _cargo_metadata_package(
    root: Path,
    *,
    cargo: tuple[str, ...],
    config: Path,
    environment: Mapping[str, str],
    runner: CommandRunner,
) -> Mapping[str, Any]:
    result = _run(
        (
            *cargo,
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--manifest-path",
            str(root / "Cargo.toml"),
            "--config",
            str(config),
        ),
        cwd=root,
        environment=environment,
        runner=runner,
        capture_output=True,
    )
    try:
        payload = json.loads(result.stdout)
    except (TypeError, json.JSONDecodeError) as error:
        raise CandidateError(f"Cargo metadata returned invalid JSON: {error}") from error
    packages = payload.get("packages") if isinstance(payload, dict) else None
    if not isinstance(packages, list) or len(packages) != 1 or not isinstance(packages[0], dict):
        raise CandidateError("Cargo metadata did not return exactly one packaged crate")
    return packages[0]


def _same_regular_tree(left: Path, right: Path) -> bool:
    left_names = sorted(path.name for path in left.iterdir())
    right_names = sorted(path.name for path in right.iterdir())
    if left_names != right_names:
        return False
    for name in left_names:
        left_path = left / name
        right_path = right / name
        try:
            left_stat = left_path.lstat()
            right_stat = right_path.lstat()
        except OSError:
            return False
        if not stat.S_ISREG(left_stat.st_mode) or not stat.S_ISREG(right_stat.st_mode):
            return False
        if _read_regular(left_path, maximum=MAX_REQUEST_BYTES, label=name) != _read_regular(
            right_path,
            maximum=MAX_REQUEST_BYTES,
            label=name,
        ):
            return False
    return True


def build_candidate_bundle(
    output: Path,
    *,
    core_workspace: Path = CORE_WORKSPACE,
    cargo_executable: str = "cargo",
    inventory: CargoReleaseInventory | None = None,
    runner: CommandRunner = subprocess.run,
    downloader: Callable[[CargoInventoryPackage], bytes] = _download_registry_archive,
) -> CandidateBundle:
    """Build all first-party archives against one incrementally staged registry."""
    inventory = inventory or load_inventory()
    cargo = _cargo_prefix(cargo_executable, inventory.candidate_toolchain)
    if core_workspace.is_symlink() or not core_workspace.is_dir():
        raise CandidateError(f"Cargo workspace is missing or linked: {core_workspace}")
    output_parent = output.absolute().parent
    output_parent.mkdir(parents=True, exist_ok=True)
    environment = os.environ.copy()
    environment["PYO3_USE_ABI3_FORWARD_COMPATIBILITY"] = "1"
    with tempfile.TemporaryDirectory(
        prefix=".cargo-release-candidate-",
        dir=output_parent,
    ) as temporary:
        work = Path(temporary)
        directory_source = work / "directory-source"
        target = work / "target"
        bundle = work / "bundle"
        bundle.mkdir()
        environment["CARGO_TARGET_DIR"] = str(target)

        version = _run(
            (*cargo, "--version"),
            cwd=core_workspace,
            environment=environment,
            runner=runner,
            capture_output=True,
        ).stdout
        if re.search(rf"\bcargo {re.escape(inventory.candidate_toolchain)}\b", version) is None:
            raise CandidateError(
                "Cargo executable did not report the inventory-pinned candidate toolchain"
            )
        _run(
            (*cargo, "vendor", "--locked", "--versioned-dirs", str(directory_source)),
            cwd=core_workspace,
            environment=environment,
            runner=runner,
        )
        config = work / "cargo-config.toml"
        config.write_text(
            "[source.crates-io]\n"
            'replace-with = "release-stage"\n\n'
            "[source.release-stage]\n"
            f"directory = {json.dumps(str(directory_source.resolve()))}\n",
            encoding="utf-8",
        )
        immutable_archives = {
            package.name: downloader(package) for package in inventory.immutable_packages
        }
        staged_checksums: dict[tuple[str, str], str] = {}
        manifest_entries: list[dict[str, Any]] = []

        for package in inventory.public_packages:
            archive_name = f"{package.name}-{package.version}.crate"
            if package.immutable:
                archive = immutable_archives[package.name]
                files = safe_crate_files(archive, package=package)
                staged_root, _ = stage_archive(
                    directory_source,
                    package=package,
                    archive=archive,
                )
                del staged_root, files
                metadata_sha256 = None
                request_name = None
                request_sha256 = None
                request_size = None
            else:
                packaged_path = target / "package" / archive_name
                if packaged_path.exists() or packaged_path.is_symlink():
                    raise CandidateError(f"fresh target unexpectedly contains {packaged_path}")
                _run(
                    (
                        *cargo,
                        "package",
                        "--locked",
                        "--allow-dirty",
                        "--all-features",
                        "-p",
                        package.name,
                        "--config",
                        str(config),
                    ),
                    cwd=core_workspace,
                    environment=environment,
                    runner=runner,
                )
                archive = _read_regular(
                    packaged_path,
                    maximum=MAX_ARCHIVE_BYTES,
                    label=f"packaged archive for {package.name}",
                )
                staged_root, files = stage_archive(
                    directory_source,
                    package=package,
                    archive=archive,
                )
                _validate_packaged_lock(
                    files,
                    package=package,
                    staged_checksums=staged_checksums,
                )
                manifest = _parse_toml(
                    files[PurePosixPath("Cargo.toml")],
                    label=f"{package.name} Cargo.toml",
                )
                cargo_package = _cargo_metadata_package(
                    staged_root,
                    cargo=cargo,
                    config=config,
                    environment=environment,
                    runner=runner,
                )
                metadata = publish_metadata_from_cargo(
                    cargo_package,
                    manifest=manifest,
                    files=files,
                )
                metadata_body, request_body = encode_publish_request(metadata, archive)
                metadata_sha256 = sha256_bytes(metadata_body)
                request_name = f"{package.name}-{package.version}.put"
                request_path = bundle / request_name
                request_path.write_bytes(request_body)
                request_sha256 = sha256_bytes(request_body)
                request_size = len(request_body)
            archive_sha256 = sha256_bytes(archive)
            if (
                package.registry_checksum is not None
                and archive_sha256 != package.registry_checksum
            ):
                raise CandidateError(f"immutable archive drifted for {package.name}")
            (bundle / archive_name).write_bytes(archive)
            staged_checksums[(package.name, package.version)] = archive_sha256
            manifest_entries.append(
                {
                    "archive": archive_name,
                    "archive_sha256": archive_sha256,
                    "archive_size": len(archive),
                    "classification": package.classification,
                    "metadata_sha256": metadata_sha256,
                    "name": package.name,
                    "publish_order": package.publish_order,
                    "request_body": request_name,
                    "request_body_sha256": request_sha256,
                    "request_body_size": request_size,
                    "version": package.version,
                }
            )

        inventory_path = Path(__file__).with_name("cargo_release_inventory.toml")
        manifest = {
            "cargo_toolchain": inventory.candidate_toolchain,
            "inventory_sha256": sha256_bytes(inventory_path.read_bytes()),
            "packages": manifest_entries,
            "release_version": inventory.release_version,
            "schema_version": BUNDLE_SCHEMA_VERSION,
        }
        (bundle / BUNDLE_MANIFEST).write_bytes(canonical_json_bytes(manifest, newline=True))
        accepted = validate_candidate_bundle(
            bundle,
            expected_release_version=inventory.release_version,
            inventory=inventory,
        )
        if output.exists() or output.is_symlink():
            existing = validate_candidate_bundle(
                output,
                expected_release_version=inventory.release_version,
                inventory=inventory,
            )
            if not _same_regular_tree(bundle, output):
                raise CandidateError(
                    f"existing candidate bundle differs from the rebuilt candidate: {output}"
                )
            return existing
        os.rename(bundle, output)
        return CandidateBundle(
            root=output,
            manifest=output / BUNDLE_MANIFEST,
            manifest_sha256=accepted.manifest_sha256,
            packages=tuple(
                CandidatePackage(
                    package=value.package,
                    archive=output / value.archive.name,
                    archive_sha256=value.archive_sha256,
                    request_body=(
                        output / value.request_body.name if value.request_body is not None else None
                    ),
                    request_body_sha256=value.request_body_sha256,
                    metadata_sha256=value.metadata_sha256,
                )
                for value in accepted.packages
            ),
        )


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    build = subparsers.add_parser("build", help="build the exact candidate bundle")
    build.add_argument("--output", type=Path, required=True)
    build.add_argument("--core-workspace", type=Path, default=CORE_WORKSPACE)
    build.add_argument("--cargo", default="cargo")
    validate = subparsers.add_parser("validate", help="validate an existing candidate bundle")
    validate.add_argument("--bundle", type=Path, required=True)
    validate.add_argument("--expected-release-version", required=True)
    validate.add_argument("--expected-manifest-sha256")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if args.command == "build":
        candidate = build_candidate_bundle(
            args.output,
            core_workspace=args.core_workspace.resolve(),
            cargo_executable=args.cargo,
        )
    else:
        candidate = validate_candidate_bundle(
            args.bundle,
            expected_release_version=args.expected_release_version,
            expected_manifest_sha256=args.expected_manifest_sha256,
        )
    print(
        json.dumps(
            {
                "archives": len(candidate.packages),
                "bundle": str(candidate.root),
                "manifest": candidate.manifest.name,
                "manifest_sha256": candidate.manifest_sha256,
                "status": "ok",
            },
            indent=2,
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (CandidateError, InventoryError) as error:
        print(f"Cargo release candidate failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
