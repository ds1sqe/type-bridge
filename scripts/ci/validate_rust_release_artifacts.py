#!/usr/bin/env python3
"""Validate the exact Rust release-candidate archives before publication."""

from __future__ import annotations

import argparse
import copy
import hashlib
import io
import json
import re
import stat
import sys
import tarfile
import tomllib
from collections.abc import Sequence
from pathlib import Path, PurePosixPath
from typing import Any

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]


class ValidationError(RuntimeError):
    """A Rust release archive is missing, unsafe, or incorrectly licensed."""


SEMVER_PATTERN = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)"
    r"(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)
MIT_LICENSE = "MIT"
APACHE_2_LICENSE = "Apache-2.0"
MPL_2_LICENSE = "MPL-2.0"
CANONICAL_LICENSE_DIGESTS = {
    MIT_LICENSE: "9b3a3f225f7e7b2396656019bdff1e9517792c92b0100add4c88ac2c5e07c63f",
    APACHE_2_LICENSE: "a6cba85bc92e0cff7a450b1d873c0eaa2e9fc96bf472df0247a26bec77bf3ff9",
    MPL_2_LICENSE: "3f3d9e0024b1921b067d6f7f88deb4a60cbe7a78e76c64e3f1d7fc3b779b9d04",
}
FIRST_PARTY_PACKAGES = (
    "type-bridge-contract",
    "type-bridge-core-lib",
    "type-bridge-schema",
    "type-bridge-query",
    "type-bridge-schema-migration",
    "type-bridge-toml-transpiler",
    "type-bridge-schema-compat",
    "type-bridge-schema-codegen",
    "type-bridge-orm-derive",
    "type-bridge-typedb-runtime",
    "type-bridge-orm",
    "type-bridge-migration",
    "type-bridge-schema-migration-typedb",
    "type-bridge-workspace",
    "type-bridge-cli",
    "type-bridge",
)
VENDORED_PACKAGES = {
    "type-bridge-typedb-protocol-b8": ("3.11.0", MPL_2_LICENSE),
    "type-bridge-typedb-driver-b8": ("3.11.5", APACHE_2_LICENSE),
}
VENDORED_SOURCE_MANIFESTS = {
    "type-bridge-typedb-protocol-b8": Path("type-bridge-core/vendor/typedb-protocol-b8/Cargo.toml"),
    "type-bridge-typedb-driver-b8": Path("type-bridge-core/vendor/typedb-driver-b8/Cargo.toml"),
}
MAX_ARCHIVE_BYTES = 64 * 1024 * 1024
MAX_ARCHIVE_FILES = 20_000
MAX_ARCHIVE_MEMBER_BYTES = 64 * 1024 * 1024
MAX_ARCHIVE_EXPANDED_BYTES = 256 * 1024 * 1024


def stable_version(value: object, *, label: str) -> str:
    """Return one stable semantic version."""
    if not isinstance(value, str) or SEMVER_PATTERN.fullmatch(value) is None:
        raise ValidationError(f"{label} is not a semantic version: {value!r}")
    if "-" in value.partition("+")[0]:
        raise ValidationError(f"{label} is a prerelease version: {value!r}")
    return value


def expected_packages(release_version: str) -> dict[str, tuple[str, str]]:
    """Return the closed identity, version, and license map for new archives."""
    version = stable_version(release_version, label="expected Rust release version")
    packages = {name: (version, MIT_LICENSE) for name in FIRST_PARTY_PACKAGES}
    packages.update(VENDORED_PACKAGES)
    return packages


def read_archive(path: Path) -> bytes:
    """Read one regular bounded archive without following a symlink."""
    try:
        file_stat = path.lstat()
    except OSError as error:
        raise ValidationError(f"Could not inspect Rust release archive {path}: {error}") from error
    if stat.S_ISLNK(file_stat.st_mode) or not stat.S_ISREG(file_stat.st_mode):
        raise ValidationError(f"Rust release archive is linked or non-regular: {path}")
    if file_stat.st_size < 0 or file_stat.st_size > MAX_ARCHIVE_BYTES:
        raise ValidationError(
            "Rust release archive exceeds the compressed byte budget: "
            f"path={path}, size={file_stat.st_size}, maximum={MAX_ARCHIVE_BYTES}"
        )
    try:
        body = path.read_bytes()
    except OSError as error:
        raise ValidationError(f"Could not read Rust release archive {path}: {error}") from error
    if len(body) != file_stat.st_size:
        raise ValidationError(f"Rust release archive changed while it was read: {path}")
    return body


def safe_archive_files(archive: bytes, *, root: str, label: str) -> dict[str, bytes]:
    """Read one gzip tar inventory without extracting it onto the filesystem."""
    if len(archive) > MAX_ARCHIVE_BYTES:
        raise ValidationError(
            f"{label} exceeds the compressed byte budget: maximum={MAX_ARCHIVE_BYTES}"
        )
    files: dict[str, bytes] = {}
    seen: set[str] = set()
    expanded_bytes = 0
    try:
        with tarfile.open(fileobj=io.BytesIO(archive), mode="r|gz") as crate:
            member_count = 0
            for member in crate:
                member_count += 1
                if member_count > MAX_ARCHIVE_FILES:
                    raise ValidationError(
                        f"{label} exceeds the member-count budget: maximum={MAX_ARCHIVE_FILES}"
                    )
                name = member.name
                if "\\" in name or name.startswith("/") or "\x00" in name:
                    raise ValidationError(f"{label} contains an unsafe path: {name!r}")
                normalized = name.rstrip("/")
                parts = normalized.split("/")
                if (
                    not normalized
                    or any(part in ("", ".", "..") for part in parts)
                    or parts[0] != root
                ):
                    raise ValidationError(f"{label} contains an unsafe path: {name!r}")
                if normalized in seen:
                    raise ValidationError(f"{label} contains a duplicate member: {name!r}")
                seen.add(normalized)
                if member.isdir():
                    continue
                if not member.isfile():
                    raise ValidationError(
                        f"{label} contains a symlink or non-regular member: {name!r}"
                    )
                if len(parts) == 1:
                    raise ValidationError(f"{label} root is unexpectedly a regular file")
                if member.size < 0 or member.size > MAX_ARCHIVE_MEMBER_BYTES:
                    raise ValidationError(
                        f"{label} member exceeds the per-file byte budget: "
                        f"path={name!r}, size={member.size}"
                    )
                expanded_bytes += member.size
                if expanded_bytes > MAX_ARCHIVE_EXPANDED_BYTES:
                    raise ValidationError(
                        f"{label} exceeds the expanded byte budget: "
                        f"maximum={MAX_ARCHIVE_EXPANDED_BYTES}"
                    )
                source = crate.extractfile(member)
                if source is None:
                    raise ValidationError(f"{label} regular member is unreadable: {name!r}")
                body = source.read(member.size + 1)
                if len(body) != member.size:
                    raise ValidationError(f"{label} member size disagrees with its body: {name!r}")
                relative = PurePosixPath(*parts[1:]).as_posix()
                if relative in files:
                    raise ValidationError(f"{label} contains a duplicate file: {relative!r}")
                files[relative] = body
    except ValidationError:
        raise
    except (EOFError, OSError, tarfile.TarError) as error:
        raise ValidationError(f"Could not safely read {label}: {error}") from error
    if not files:
        raise ValidationError(f"{label} contains no regular files")
    return files


def parse_manifest(body: bytes, *, label: str) -> dict[str, Any]:
    """Parse one packaged Cargo manifest as a TOML object."""
    try:
        payload = tomllib.loads(body.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        raise ValidationError(f"Could not parse {label} Cargo.toml: {error}") from error
    package = payload.get("package")
    if not isinstance(package, dict):
        raise ValidationError(f"{label} Cargo.toml has no [package] table")
    return package


def parse_complete_manifest(body: bytes, *, label: str) -> dict[str, Any]:
    """Parse one complete Cargo manifest for behavioral comparison."""
    try:
        payload = tomllib.loads(body.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        raise ValidationError(f"Could not parse {label}: {error}") from error
    if not isinstance(payload, dict):
        raise ValidationError(f"{label} is not a TOML table")
    return payload


def read_source_manifest(path: Path, *, label: str) -> bytes:
    """Read one bounded regular repository manifest without following a symlink."""
    try:
        file_stat = path.lstat()
    except OSError as error:
        raise ValidationError(f"Could not inspect {label} {path}: {error}") from error
    if stat.S_ISLNK(file_stat.st_mode) or not stat.S_ISREG(file_stat.st_mode):
        raise ValidationError(f"{label} is linked or non-regular: {path}")
    if file_stat.st_size < 0 or file_stat.st_size > MAX_ARCHIVE_MEMBER_BYTES:
        raise ValidationError(
            f"{label} exceeds the source-manifest byte budget: "
            f"size={file_stat.st_size}, maximum={MAX_ARCHIVE_MEMBER_BYTES}"
        )
    try:
        body = path.read_bytes()
    except OSError as error:
        raise ValidationError(f"Could not read {label} {path}: {error}") from error
    if len(body) != file_stat.st_size:
        raise ValidationError(f"{label} changed while it was read: {path}")
    return body


def expected_normalized_vendor_manifest(
    source_manifest: dict[str, Any],
    *,
    label: str,
) -> dict[str, Any]:
    """Model Cargo's exact behavior-relevant normalization for a b8 source manifest."""
    expected = copy.deepcopy(source_manifest)
    package = expected.get("package")
    if not isinstance(package, dict):
        raise ValidationError(f"{label} has no [package] table")
    package_name = package.get("name")
    if not isinstance(package_name, str) or not package_name:
        raise ValidationError(f"{label} has no package name")
    for key in ("build", "autolib", "autobins", "autoexamples", "autotests", "autobenches"):
        if key in package:
            raise ValidationError(f"{label} unexpectedly declares package.{key}")
        package[key] = False

    library = expected.get("lib")
    if not isinstance(library, dict):
        raise ValidationError(f"{label} has no [lib] table")
    if "name" in library:
        raise ValidationError(f"{label} unexpectedly declares lib.name")
    library["name"] = package_name.replace("-", "_")

    dependency_sections = ("dependencies", "dev-dependencies", "build-dependencies")

    def normalize_table(container: dict[str, Any], section: str, section_label: str) -> None:
        table = container.get(section)
        if table is None:
            return
        if not isinstance(table, dict):
            raise ValidationError(f"{label} has a non-table [{section_label}] section")
        for dependency, specification in tuple(table.items()):
            if isinstance(specification, str):
                normalized: dict[str, Any] = {"version": specification}
            elif isinstance(specification, dict):
                normalized = copy.deepcopy(specification)
            else:
                raise ValidationError(
                    f"{label} has an invalid {section_label} dependency {dependency!r}"
                )
            if "path" in normalized:
                normalized.pop("path")
                if not isinstance(normalized.get("version"), str):
                    raise ValidationError(
                        f"{label} path dependency {dependency!r} has no registry version"
                    )
            table[dependency] = normalized

    for section in dependency_sections:
        normalize_table(expected, section, section)
    targets = expected.get("target")
    if targets is not None:
        if not isinstance(targets, dict):
            raise ValidationError(f"{label} has a non-table [target] section")
        for target_name, target in targets.items():
            if not isinstance(target, dict):
                raise ValidationError(f"{label} has an invalid target table {target_name!r}")
            for section in dependency_sections:
                normalize_table(target, section, f"target.{target_name}.{section}")
    return expected


def validate_vendored_manifest_payload(
    files: dict[str, bytes],
    *,
    name: str,
    source_manifest_path: Path,
) -> None:
    """Bind both Cargo manifests in a b8 archive to the reviewed repository source."""
    source_body = read_source_manifest(
        source_manifest_path,
        label=f"repository manifest for {name}",
    )
    original_body = files.get("Cargo.toml.orig")
    if original_body is None:
        raise ValidationError(f"Rust release archive {name} has no Cargo.toml.orig")
    if original_body != source_body:
        raise ValidationError(
            f"Rust release archive {name} Cargo.toml.orig drifted from its repository source"
        )
    normalized_body = files.get("Cargo.toml")
    if normalized_body is None:
        raise ValidationError(f"Rust release archive {name} has no Cargo.toml")
    source = parse_complete_manifest(source_body, label=f"repository manifest for {name}")
    actual = parse_complete_manifest(normalized_body, label=f"packaged manifest for {name}")
    expected = expected_normalized_vendor_manifest(
        source,
        label=f"repository manifest for {name}",
    )
    if actual != expected:
        raise ValidationError(
            f"Rust release archive {name} normalized manifest exceeds Cargo's "
            "packaging-only transform"
        )


def is_license_document(path: str) -> bool:
    """Return whether a member basename could represent a license document."""
    basename = PurePosixPath(path).name.upper()
    return basename in {"LICENSE", "LICENSE.TXT", "COPYING", "COPYING.TXT"} or basename.startswith(
        ("LICENSE-", "LICENSE.", "COPYING-", "COPYING.")
    )


def validate_archive(
    path: Path,
    *,
    name: str,
    version: str,
    license_id: str,
    source_manifest_path: Path | None = None,
) -> dict[str, str | int]:
    """Validate one exact package identity and its canonical root LICENSE."""
    archive = read_archive(path)
    root = f"{name}-{version}"
    label = f"Rust release archive {name}@{version}"
    files = safe_archive_files(archive, root=root, label=label)
    manifest_body = files.get("Cargo.toml")
    if manifest_body is None:
        raise ValidationError(f"{label} has no root Cargo.toml")
    package = parse_manifest(manifest_body, label=label)
    actual_identity = (package.get("name"), package.get("version"))
    expected_identity = (name, version)
    if actual_identity != expected_identity:
        raise ValidationError(
            f"{label} manifest identity drifted: "
            f"actual={actual_identity!r}, expected={expected_identity!r}"
        )
    if package.get("license") != license_id:
        raise ValidationError(
            f"{label} license metadata drifted: "
            f"actual={package.get('license')!r}, expected={license_id!r}"
        )
    if package.get("license-file") != "LICENSE":
        raise ValidationError(
            f"{label} license-file must reference the packaged root LICENSE: "
            f"actual={package.get('license-file')!r}"
        )
    if source_manifest_path is not None:
        validate_vendored_manifest_payload(
            files,
            name=name,
            source_manifest_path=source_manifest_path,
        )

    license_documents = sorted(path for path in files if is_license_document(path))
    if license_documents != ["LICENSE"]:
        raise ValidationError(
            f"{label} must contain exactly one root LICENSE document: actual={license_documents!r}"
        )
    actual_license_digest = hashlib.sha256(files["LICENSE"]).hexdigest()
    expected_license_digest = CANONICAL_LICENSE_DIGESTS[license_id]
    if actual_license_digest != expected_license_digest:
        raise ValidationError(
            f"{label} LICENSE is not the canonical {license_id} body: "
            f"actual_sha256={actual_license_digest!r}, "
            f"expected_sha256={expected_license_digest!r}"
        )
    return {
        "archive": path.name,
        "archive_sha256": hashlib.sha256(archive).hexdigest(),
        "files": len(files),
        "license": license_id,
        "name": name,
        "version": version,
    }


def validate_release_artifacts(
    artifacts_dir: Path,
    *,
    expected_release_version: str,
    repository_root: Path = REPOSITORY_ROOT,
) -> dict[str, Any]:
    """Validate the closed packaged Cargo release graph."""
    if artifacts_dir.is_symlink() or not artifacts_dir.is_dir():
        raise ValidationError(
            f"Rust release artifact directory is missing, linked, or non-directory: {artifacts_dir}"
        )
    expected = expected_packages(expected_release_version)
    expected_filenames = {
        f"{name}-{version}.crate": (name, version, license_id)
        for name, (version, license_id) in expected.items()
    }
    actual_entries = sorted(
        entry.name for entry in artifacts_dir.iterdir() if entry.name.endswith(".crate")
    )
    if set(actual_entries) != set(expected_filenames):
        raise ValidationError(
            "Rust release archive inventory drifted: "
            f"missing={sorted(set(expected_filenames) - set(actual_entries))!r}, "
            f"unexpected={sorted(set(actual_entries) - set(expected_filenames))!r}"
        )
    reports = []
    for filename in sorted(expected_filenames):
        name, version, license_id = expected_filenames[filename]
        reports.append(
            validate_archive(
                artifacts_dir / filename,
                name=name,
                version=version,
                license_id=license_id,
                source_manifest_path=(
                    repository_root / VENDORED_SOURCE_MANIFESTS[name]
                    if name in VENDORED_SOURCE_MANIFESTS
                    else None
                ),
            )
        )
    return {
        "artifacts": reports,
        "expected_release_version": expected_release_version,
        "status": "ok",
    }


def build_parser() -> argparse.ArgumentParser:
    """Build the Rust release-artifact validator CLI."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts-dir", type=Path, required=True)
    parser.add_argument("--expected-release-version", required=True)
    parser.add_argument("--repository-root", type=Path, default=REPOSITORY_ROOT)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Validate and print the exact Rust release archive report."""
    args = build_parser().parse_args(argv)
    report = validate_release_artifacts(
        args.artifacts_dir,
        expected_release_version=args.expected_release_version,
        repository_root=args.repository_root.resolve(),
    )
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ValidationError as error:
        print(f"Rust release artifact validation failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
