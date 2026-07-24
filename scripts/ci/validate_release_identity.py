#!/usr/bin/env python3
"""Bind a release tag, license boundary, and package provenance before publication."""

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
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any
from urllib import error as urllib_error
from urllib import request as urllib_request

try:
    from python_release_contract import (
        ContractError as PythonReleaseContractError,
    )
    from python_release_contract import (
        validate_node_package_lockstep,
        validate_python_package_version,
        validate_root_python_manifest_lockstep,
    )
except ModuleNotFoundError:
    from scripts.ci.python_release_contract import (
        ContractError as PythonReleaseContractError,
    )
    from scripts.ci.python_release_contract import (
        validate_node_package_lockstep,
        validate_python_package_version,
        validate_root_python_manifest_lockstep,
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

PUBLISHED_CRATES = (
    "type-bridge-core-lib",
    "type-bridge-toml-transpiler",
    "type-bridge-orm-derive",
    "type-bridge-typedb-protocol-b7",
    "type-bridge-typedb-driver-b7",
    "type-bridge-typedb-protocol-b8",
    "type-bridge-typedb-driver-b8",
    "type-bridge-typedb-runtime",
    "type-bridge-orm",
    "type-bridge-migration",
    "type-bridge-server",
)
UNPUBLISHED_V2_CRATES = (
    "type-bridge-contract",
    "type-bridge-schema",
    "type-bridge-query",
    "type-bridge-schema-migration",
    "type-bridge-schema-migration-typedb",
    "type-bridge-schema-codegen",
    "type-bridge-schema-compat",
    "type-bridge-workspace",
    "type-bridge-cli",
)
KNOWN_PUBLICATION_BLOCKER_EDGES = frozenset(
    {
        ("type-bridge-core-lib", "type-bridge-contract"),
        ("type-bridge-orm", "type-bridge-contract"),
        ("type-bridge-orm", "type-bridge-query"),
        ("type-bridge-orm", "type-bridge-schema"),
        ("type-bridge-orm", "type-bridge-schema-compat"),
        ("type-bridge-migration", "type-bridge-contract"),
        ("type-bridge-migration", "type-bridge-schema-compat"),
        ("type-bridge-server", "type-bridge-contract"),
        ("type-bridge-server", "type-bridge-query"),
        ("type-bridge-server", "type-bridge-schema"),
        ("type-bridge-server", "type-bridge-schema-migration-typedb"),
    }
)
IMMUTABLE_BASELINE_CRATES = (
    "type-bridge-typedb-protocol-b7",
    "type-bridge-typedb-driver-b7",
)
OWNER_GATED_COMPATIBILITY_CRATES = (
    "type-bridge-typedb-protocol-b8",
    "type-bridge-typedb-driver-b8",
)
PREEXISTING_CRATES = IMMUTABLE_BASELINE_CRATES + OWNER_GATED_COMPATIBILITY_CRATES
PACKAGED_RELEASE_CRATES = tuple(
    crate for crate in PUBLISHED_CRATES if crate not in IMMUTABLE_BASELINE_CRATES
)
EXPECTED_NEW_CRATES = tuple(crate for crate in PUBLISHED_CRATES if crate not in PREEXISTING_CRATES)
TYPEDB_RUNTIME_PACKAGE = "type-bridge-typedb-runtime"
TYPEDB_BAND7_DEPENDENCY = "type-bridge-typedb-driver-b7"
TYPEDB_BAND8_DEPENDENCY = "type-bridge-typedb-driver-b8"
TYPEDB_BAND9_DEPENDENCY = "typedb-driver"
TARGET_RELEASE_VERSION = "2.0.0"
CANDIDATE_RELEASE_VERSION = "2.0.0-rc.0"
CANDIDATE_PYTHON_VERSION = "2.0.0rc0"
ARTIFACT_CONTRACT_CARGO_INCLUSIVE = "cargo-inclusive"
ARTIFACT_CONTRACT_PYTHON_NPM_ONLY = "python-npm-only"
ARTIFACT_CONTRACTS = (
    ARTIFACT_CONTRACT_CARGO_INCLUSIVE,
    ARTIFACT_CONTRACT_PYTHON_NPM_ONLY,
)
RELEASE_CHANNEL_CANDIDATE = "candidate"
RELEASE_CHANNEL_STABLE = "stable"
RELEASE_CHANNEL_IDENTITIES = {
    RELEASE_CHANNEL_CANDIDATE: (CANDIDATE_RELEASE_VERSION, CANDIDATE_PYTHON_VERSION),
    RELEASE_CHANNEL_STABLE: (TARGET_RELEASE_VERSION, TARGET_RELEASE_VERSION),
}
UNPUBLISHED_BINDING_CRATES = (
    "type-bridge-core",
    "type-bridge-node",
)
PYTHON_NPM_UNPUBLISHED_CRATES = UNPUBLISHED_V2_CRATES + UNPUBLISHED_BINDING_CRATES
TYPEDB_RUNTIME_BAND7_PIN_PATTERN = re.compile(
    r'^pub const PINNED_DRIVER_VERSION_B7: &str = "([^"]+)";$',
    re.MULTILINE,
)
TYPEDB_RUNTIME_BAND8_PIN_PATTERN = re.compile(
    r'^pub const PINNED_DRIVER_VERSION: &str = "([^"]+)";$',
    re.MULTILINE,
)
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
PREEXISTING_LEGACY_MANIFEST_DIGESTS = {
    "type-bridge-typedb-driver-b7": (
        "0b55ed816e74578b5170c724e70ffe3f061d0ffc35d8700260a36e6dd86bc4c3"
    ),
    "type-bridge-typedb-protocol-b7": (
        "90182fa55887b9af166344df8fa37457ea80988b4b1c1630c6f980ed38dedc4a"
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


@dataclass(frozen=True)
class LegacyTypeDbComponent:
    """Immutable package, upstream, archive, and license identity for one package."""

    archive_checksum: str
    band: int
    downstream_name: str
    downstream_version: str
    license: str
    license_status: str
    manifest_path: str
    upstream_commit: str
    upstream_name: str
    upstream_version: str

    @property
    def archive_url(self) -> str:
        """Return the immutable upstream crates.io archive URL."""
        return (
            f"https://static.crates.io/crates/{self.upstream_name}/"
            f"{self.upstream_name}-{self.upstream_version}.crate"
        )

    @property
    def vendor_directory(self) -> str:
        """Return the workspace-relative vendor directory."""
        return str(PurePosixPath(self.manifest_path).parent)


LEGACY_TYPEDB_COMPONENTS = (
    LegacyTypeDbComponent(
        archive_checksum="bf5f617f8d670dd75dc752ae6f42e2bf28ca612ab4feae353c2c89d052adfab0",
        band=7,
        downstream_name="type-bridge-typedb-driver-b7",
        downstream_version="3.8.1",
        license=APACHE_2_LICENSE,
        license_status=(
            "Apache-2.0 namespaced packaging-only package; source behavior unchanged; "
            "already published"
        ),
        manifest_path="vendor/typedb-driver-b7/Cargo.toml",
        upstream_commit="8e8d4a43da32adc1c56084f4d34174bebd0ce34a",
        upstream_name="typedb-driver",
        upstream_version="3.8.1",
    ),
    LegacyTypeDbComponent(
        archive_checksum="0062374abd0c14afa55e5b1d8e095ac110830da29943ad43f6c6b5d5912a811f",
        band=7,
        downstream_name="type-bridge-typedb-protocol-b7",
        downstream_version="3.7.0",
        license=MPL_2_LICENSE,
        license_status=(
            "MPL-2.0 namespaced packaging-only package; generated protocol source unchanged; "
            "already published"
        ),
        manifest_path="vendor/typedb-protocol-b7/Cargo.toml",
        upstream_commit="3b75931f30f2b5cecf192515bb95071cd98a6e10",
        upstream_name="typedb-protocol",
        upstream_version="3.7.0",
    ),
    LegacyTypeDbComponent(
        archive_checksum="71c456fc6fb8f9112236fc088569cbe47f620443629ef8c81b1d79aec7b49fc6",
        band=8,
        downstream_name="type-bridge-typedb-driver-b8",
        downstream_version="3.11.5",
        license=APACHE_2_LICENSE,
        license_status=(
            "Apache-2.0 namespaced packaging-only package; source behavior unchanged; "
            "registry publication requires separate explicit TypeBridge owner authorization"
        ),
        manifest_path="vendor/typedb-driver-b8/Cargo.toml",
        upstream_commit="7e669e41d9fee22fde8d5e60be7edbf00c6ec64b",
        upstream_name="typedb-driver",
        upstream_version="3.11.5",
    ),
    LegacyTypeDbComponent(
        archive_checksum="f051694ab18c9fb31f15e4567421b55a70e7dddbc1af60a6a1c4cf73ffe8d5e8",
        band=8,
        downstream_name="type-bridge-typedb-protocol-b8",
        downstream_version="3.11.0",
        license=MPL_2_LICENSE,
        license_status=(
            "MPL-2.0 namespaced packaging-only package; generated protocol source unchanged; "
            "registry publication requires separate explicit TypeBridge owner authorization"
        ),
        manifest_path="vendor/typedb-protocol-b8/Cargo.toml",
        upstream_commit="1db5bdd6579352d31343da28be41844ed07da1b5",
        upstream_name="typedb-protocol",
        upstream_version="3.11.0",
    ),
)

# Compatibility packages use independent immutable package versions instead of
# inheriting the repository release version. They are still covered by the gate
# and must be present at these exact downstream versions in the publication plan.
VENDORED_PINS = {
    component.downstream_name: component.downstream_version
    for component in LEGACY_TYPEDB_COMPONENTS
}
VENDORED_LICENSES = {
    component.downstream_name: component.license for component in LEGACY_TYPEDB_COMPONENTS
}
LEGACY_VENDOR_DESCRIPTIONS = {
    "type-bridge-typedb-driver-b7": (
        "Renamed vendor of upstream typedb-driver 3.8.1 (TypeDB protocol band 7), "
        "republished unmodified for type-bridge dual-band server support"
    ),
    "type-bridge-typedb-protocol-b7": (
        "Renamed vendor of upstream typedb-protocol 3.7.0 (TypeDB protocol band 7), "
        "republished unmodified for type-bridge dual-band server support"
    ),
    "type-bridge-typedb-driver-b8": (
        "Renamed package of upstream typedb-driver 3.11.5 (TypeDB protocol band 8); "
        "source-unmodified compatibility package with registry publication gated by separate "
        "explicit TypeBridge owner authorization"
    ),
    "type-bridge-typedb-protocol-b8": (
        "Renamed vendor of upstream typedb-protocol 3.11.0 (TypeDB protocol band 8); "
        "source-unmodified compatibility package with registry publication gated by separate "
        "explicit TypeBridge owner authorization"
    ),
}
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
MAX_LEGACY_ARCHIVE_BYTES = 32 * 1024 * 1024
MAX_LEGACY_ARCHIVE_FILES = 10_000
MAX_LEGACY_ARCHIVE_MEMBER_BYTES = 32 * 1024 * 1024
MAX_LEGACY_ARCHIVE_EXPANDED_BYTES = 128 * 1024 * 1024
LEGACY_ARCHIVE_HTTP_TIMEOUT_SECONDS = 30


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


def download_legacy_crate_archive(
    component: LegacyTypeDbComponent,
    *,
    opener: Callable[..., Any] = urllib_request.urlopen,
) -> bytes:
    """Download one immutable crates.io archive with strict time and byte bounds."""
    request = urllib_request.Request(
        component.archive_url,
        headers={
            "Accept": "application/octet-stream",
            "User-Agent": "ds1sqe/type-bridge legacy-provenance-gate",
        },
        method="GET",
    )
    try:
        with opener(request, timeout=LEGACY_ARCHIVE_HTTP_TIMEOUT_SECONDS) as response:
            status = getattr(response, "status", None)
            if status != 200:
                raise ValidationError(
                    f"Could not download {component.archive_url}: HTTP status {status!r}"
                )
            final_url = response.geturl()
            if final_url != component.archive_url:
                raise ValidationError(
                    "Legacy TypeDB archive download redirected away from its immutable URL: "
                    f"actual={final_url!r}, expected={component.archive_url!r}"
                )
            declared_length = response.headers.get("Content-Length")
            if declared_length is not None:
                try:
                    parsed_length = int(declared_length, 10)
                except ValueError as error:
                    raise ValidationError(
                        f"Legacy TypeDB archive has an invalid Content-Length: {declared_length!r}"
                    ) from error
                if parsed_length < 0 or parsed_length > MAX_LEGACY_ARCHIVE_BYTES:
                    raise ValidationError(
                        "Legacy TypeDB archive exceeds the compressed byte budget: "
                        f"declared={parsed_length}, maximum={MAX_LEGACY_ARCHIVE_BYTES}"
                    )

            chunks: list[bytes] = []
            total = 0
            while True:
                remaining = MAX_LEGACY_ARCHIVE_BYTES - total
                chunk = response.read(min(64 * 1024, remaining + 1))
                if not chunk:
                    break
                total += len(chunk)
                if total > MAX_LEGACY_ARCHIVE_BYTES:
                    raise ValidationError(
                        "Legacy TypeDB archive exceeds the compressed byte budget while reading: "
                        f"maximum={MAX_LEGACY_ARCHIVE_BYTES}"
                    )
                chunks.append(chunk)
    except ValidationError:
        raise
    except (OSError, urllib_error.URLError) as error:
        raise ValidationError(
            f"Could not download immutable legacy TypeDB archive {component.archive_url}: {error}"
        ) from error
    return b"".join(chunks)


def verified_crate_archive_files(
    component: LegacyTypeDbComponent,
    archive: bytes,
) -> dict[str, bytes]:
    """Verify and read a crates.io tarball without extracting paths onto the filesystem."""
    if len(archive) > MAX_LEGACY_ARCHIVE_BYTES:
        raise ValidationError(
            "Legacy TypeDB archive exceeds the compressed byte budget before parsing: "
            f"actual={len(archive)}, maximum={MAX_LEGACY_ARCHIVE_BYTES}"
        )
    actual_checksum = hashlib.sha256(archive).hexdigest()
    if actual_checksum != component.archive_checksum:
        raise ValidationError(
            f"Legacy TypeDB archive checksum mismatch for {component.upstream_name} "
            f"{component.upstream_version}: actual={actual_checksum!r}, "
            f"expected={component.archive_checksum!r}"
        )
    expected_root = f"{component.upstream_name}-{component.upstream_version}"
    files: dict[str, bytes] = {}
    seen_members: set[str] = set()
    expanded_bytes = 0
    try:
        # Stream members so an oversized declared file is rejected from its
        # header before tarfile seeks through and decompresses its body.
        with tarfile.open(fileobj=io.BytesIO(archive), mode="r|gz") as crate:
            member_count = 0
            for member in crate:
                member_count += 1
                if member_count > MAX_LEGACY_ARCHIVE_FILES:
                    raise ValidationError(
                        "Legacy TypeDB archive exceeds the member-count budget: "
                        f"maximum={MAX_LEGACY_ARCHIVE_FILES}"
                    )
                name = member.name
                if "\\" in name or name.startswith("/") or "\x00" in name:
                    raise ValidationError(f"Legacy TypeDB archive has an unsafe path: {name!r}")
                normalized = name.rstrip("/")
                parts = normalized.split("/")
                if (
                    not normalized
                    or any(part in ("", ".", "..") for part in parts)
                    or parts[0] != expected_root
                ):
                    raise ValidationError(f"Legacy TypeDB archive has an unsafe path: {name!r}")
                if normalized in seen_members:
                    raise ValidationError(
                        f"Legacy TypeDB archive contains a duplicate member: {name!r}"
                    )
                seen_members.add(normalized)
                if member.isdir():
                    continue
                if not member.isfile():
                    raise ValidationError(
                        f"Legacy TypeDB archive contains a symlink or non-regular member: {name!r}"
                    )
                if len(parts) == 1:
                    raise ValidationError(
                        f"Legacy TypeDB archive root is unexpectedly a regular file: {name!r}"
                    )
                if member.size < 0 or member.size > MAX_LEGACY_ARCHIVE_MEMBER_BYTES:
                    raise ValidationError(
                        "Legacy TypeDB archive member exceeds the per-file byte budget: "
                        f"path={name!r}, size={member.size}"
                    )
                expanded_bytes += member.size
                if expanded_bytes > MAX_LEGACY_ARCHIVE_EXPANDED_BYTES:
                    raise ValidationError(
                        "Legacy TypeDB archive exceeds the expanded byte budget: "
                        f"maximum={MAX_LEGACY_ARCHIVE_EXPANDED_BYTES}"
                    )
                source = crate.extractfile(member)
                if source is None:
                    raise ValidationError(
                        f"Legacy TypeDB archive regular member is unreadable: {name!r}"
                    )
                body = source.read(member.size + 1)
                if len(body) != member.size:
                    raise ValidationError(
                        f"Legacy TypeDB archive member size disagrees with its body: {name!r}"
                    )
                relative = PurePosixPath(*parts[1:]).as_posix()
                if relative in files:
                    raise ValidationError(
                        f"Legacy TypeDB archive contains a duplicate file path: {relative!r}"
                    )
                files[relative] = body
    except ValidationError:
        raise
    except (EOFError, OSError, tarfile.TarError) as error:
        raise ValidationError(
            f"Could not safely read legacy TypeDB archive {component.archive_url}: {error}"
        ) from error
    if not files:
        raise ValidationError(
            f"Legacy TypeDB archive contains no regular files: {component.archive_url}"
        )
    return files


def local_legacy_vendor_files(component_root: Path) -> dict[str, bytes]:
    """Read one local compatibility tree while rejecting links and special files."""
    if component_root.is_symlink():
        raise ValidationError(
            f"Legacy TypeDB vendor directory is linked and therefore rejected: {component_root}"
        )
    component_root = component_root.resolve()
    if not component_root.is_dir():
        raise ValidationError(
            f"Legacy TypeDB vendor directory is missing or non-directory: {component_root}"
        )
    root_entries = {entry.name for entry in component_root.iterdir()}
    if root_entries != LEGACY_VENDOR_ROOT_ENTRIES:
        raise ValidationError(
            "Legacy TypeDB vendor root inventory drifted: "
            f"path={component_root}, actual={sorted(root_entries)!r}, "
            f"expected={sorted(LEGACY_VENDOR_ROOT_ENTRIES)!r}"
        )

    files: dict[str, bytes] = {}
    file_count = 0
    total_bytes = 0

    def visit(directory: Path) -> None:
        nonlocal file_count, total_bytes
        for entry in sorted(directory.iterdir(), key=lambda candidate: candidate.name):
            relative = entry.relative_to(component_root).as_posix()
            try:
                file_stat = entry.lstat()
            except OSError as error:
                raise ValidationError(
                    f"Could not inspect legacy vendor path {entry}: {error}"
                ) from error
            mode = file_stat.st_mode
            if stat.S_ISLNK(mode):
                raise ValidationError(f"Legacy TypeDB vendor tree contains a symlink: {relative!r}")
            if stat.S_ISDIR(mode):
                visit(entry)
                continue
            if not stat.S_ISREG(mode):
                raise ValidationError(
                    f"Legacy TypeDB vendor tree contains a non-regular path: {relative!r}"
                )
            file_count += 1
            if file_count > MAX_LEGACY_ARCHIVE_FILES:
                raise ValidationError(
                    "Legacy TypeDB vendor tree exceeds the file-count budget: "
                    f"maximum={MAX_LEGACY_ARCHIVE_FILES}"
                )
            size = file_stat.st_size
            if size < 0 or size > MAX_LEGACY_ARCHIVE_MEMBER_BYTES:
                raise ValidationError(
                    "Legacy TypeDB vendor file exceeds the per-file byte budget: "
                    f"path={relative!r}, size={size}"
                )
            total_bytes += size
            if total_bytes > MAX_LEGACY_ARCHIVE_EXPANDED_BYTES:
                raise ValidationError(
                    "Legacy TypeDB vendor tree exceeds the total byte budget: "
                    f"maximum={MAX_LEGACY_ARCHIVE_EXPANDED_BYTES}"
                )
            files[relative] = read_bytes(entry, label=f"legacy vendor file {relative}")

    visit(component_root)
    return files


def legacy_vendor_readme_disclosure(component: LegacyTypeDbComponent) -> bytes:
    """Return the exact downstream prefix allowed before an upstream README."""
    if component.band == 7 and component.upstream_name in {"typedb-driver", "typedb-protocol"}:
        # These packages already exist on crates.io. Preserve their immutable
        # READMEs byte-for-byte; ownership and repackaging disclosure live in
        # their bound manifests plus the release-wide notices.
        return b""
    if component.upstream_name == "typedb-driver" and component.band == 8:
        return (
            "# type-bridge compatibility packaging notice\n\n"
            "This package is an unofficial renamed compatibility package maintained by\n"
            "type-bridge so TypeDB driver protocol band 8 can coexist with other bands in\n"
            "one Cargo graph. The upstream project, original source, and ownership remain\n"
            "**TypeDB**, from\n"
            "[`typedb/typedb-driver`](https://github.com/typedb/typedb-driver). This package\n"
            f"is based exactly on the crates.io `typedb-driver` {component.upstream_version} archive:\n\n"
            "This source checkout does not authorize registry publication. Any first\n"
            "publication requires separate explicit TypeBridge owner authorization. If\n"
            "distributed, this exact source-unmodified package is the authorized\n"
            "compatibility artifact, and its paired protocol package must precede it.\n\n"
            f"- Archive: <{component.archive_url}>\n"
            f"- SHA-256: `{component.archive_checksum}`\n"
            f"- Upstream license retained: {component.license}\n"
            "- Downstream package/version: "
            f"`{component.downstream_name}` {component.downstream_version}\n\n"
            "The Rust source and `LICENSE` are byte-identical to that upstream archive.\n"
            "Only `Cargo.toml` package metadata and this disclosure differ. The complete\n"
            "original TypeDB README follows below, unchanged. TypeDB is not responsible for\n"
            "the downstream packaging changes.\n\n"
            "---\n\n"
        ).encode()
    if component.upstream_name == "typedb-protocol" and component.band == 8:
        return (
            "# type-bridge compatibility packaging notice\n\n"
            "This unofficial downstream package preserves the upstream TypeDB protocol\n"
            "implementation and changes only package metadata/name so TypeDB protocol band\n"
            "8 can coexist with other bands and pair with the type-bridge compatibility\n"
            "driver package. The upstream project and original source remain **TypeDB**, from\n"
            "[`typedb/typedb-protocol`](https://github.com/typedb/typedb-protocol). It is\n"
            "based exactly on the crates.io "
            f"`typedb-protocol` {component.upstream_version} archive:\n\n"
            "This source checkout does not authorize registry publication. Any first\n"
            "publication requires separate explicit TypeBridge owner authorization. If\n"
            "distributed, this exact source-unmodified package is the authorized\n"
            "compatibility artifact and must precede the paired driver package.\n\n"
            f"- Archive: <{component.archive_url}>\n"
            f"- SHA-256: `{component.archive_checksum}`\n"
            f"- Upstream license retained: {component.license}\n"
            "- Downstream package/version: "
            f"`{component.downstream_name}` {component.downstream_version}\n\n"
            "The generated Rust protocol source and `LICENSE` remain byte-identical to the\n"
            "upstream archive. Only `Cargo.toml` package metadata and this disclosure differ.\n"
            "The complete original TypeDB README follows below, unchanged. TypeDB is not\n"
            "responsible for the downstream packaging changes.\n\n"
            "---\n\n"
        ).encode()
    raise ValidationError(f"Unknown legacy TypeDB README policy: {component.downstream_name}")


def parse_legacy_cargo_manifest(body: bytes, *, label: str) -> dict[str, Any]:
    """Parse one compatibility manifest without accepting invalid UTF-8 or TOML."""
    try:
        manifest = tomllib.loads(body.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        raise ValidationError(f"Could not parse {label}: {error}") from error
    if not isinstance(manifest, dict):
        raise ValidationError(f"{label} is not a TOML table")
    return manifest


def normalize_dependency_specs(
    manifest: dict[str, Any],
    *,
    label: str,
) -> dict[str, Any]:
    """Normalize Cargo's equivalent string and table dependency spellings."""
    normalized = copy.deepcopy(manifest)

    def normalize_table(container: dict[str, Any], section: str, section_label: str) -> None:
        table = container.get(section)
        if table is None:
            return
        if not isinstance(table, dict):
            raise ValidationError(f"{label} has a non-table [{section_label}] section")
        for dependency, specification in tuple(table.items()):
            if isinstance(specification, str):
                table[dependency] = {"version": specification}
            elif not isinstance(specification, dict):
                raise ValidationError(
                    f"{label} has an invalid {section_label} dependency {dependency!r}"
                )

    dependency_sections = ("dependencies", "dev-dependencies", "build-dependencies")
    for section in dependency_sections:
        normalize_table(normalized, section, section)

    targets = normalized.get("target")
    if targets is not None:
        if not isinstance(targets, dict):
            raise ValidationError(f"{label} has a non-table [target] section")
        for target_name, target in targets.items():
            if not isinstance(target, dict):
                raise ValidationError(f"{label} has an invalid target table {target_name!r}")
            for section in dependency_sections:
                normalize_table(target, section, f"target.{target_name}.{section}")
    return normalized


def legacy_protocol_component(band: int) -> LegacyTypeDbComponent:
    """Return the one exact protocol package paired with a legacy driver band."""
    matches = tuple(
        component
        for component in LEGACY_TYPEDB_COMPONENTS
        if component.band == band and component.upstream_name == "typedb-protocol"
    )
    if len(matches) != 1:
        raise ValidationError(f"Legacy TypeDB band {band} must have exactly one protocol component")
    return matches[0]


def expected_legacy_cargo_manifest(
    component: LegacyTypeDbComponent,
    upstream_manifest: dict[str, Any],
) -> dict[str, Any]:
    """Apply the complete allowlisted rename-only transform to an upstream manifest."""
    expected = copy.deepcopy(upstream_manifest)
    package = expected.get("package")
    if not isinstance(package, dict):
        raise ValidationError(
            f"Legacy TypeDB upstream manifest has no [package]: {component.archive_url}"
        )
    if package.pop("authors", None) != []:
        raise ValidationError(
            f"Legacy TypeDB upstream authors metadata drifted: {component.downstream_name}"
        )
    if package.pop("licenseFile", None) != VENDOR_LICENSE_FILE:
        raise ValidationError(
            f"Legacy TypeDB upstream licenseFile metadata drifted: {component.downstream_name}"
        )
    if "license-file" in package:
        raise ValidationError(
            f"Legacy TypeDB upstream unexpectedly has license-file: {component.downstream_name}"
        )
    package["name"] = component.downstream_name
    package["description"] = LEGACY_VENDOR_DESCRIPTIONS[component.downstream_name]
    package["repository"] = TYPEBRIDGE_REPOSITORY
    if component.band == 8:
        package["license-file"] = VENDOR_LICENSE_FILE
    elif component.band != 7:
        raise ValidationError(f"Unknown legacy TypeDB band: {component.band}")

    library = expected.get("lib")
    if not isinstance(library, dict):
        raise ValidationError(
            f"Legacy TypeDB upstream manifest has no [lib]: {component.downstream_name}"
        )
    library["doctest"] = False

    rust_lints = {"unused": "allow", "dead_code": "allow"}
    if component.upstream_name == "typedb-driver":
        rust_lints["private_interfaces"] = "allow"
    expected["lints"] = {"rust": rust_lints, "clippy": {"all": "allow"}}

    if component.upstream_name == "typedb-driver":
        dependencies = expected.get("dependencies")
        if not isinstance(dependencies, dict):
            raise ValidationError(
                f"Legacy TypeDB upstream driver has no [dependencies]: {component.downstream_name}"
            )
        protocol_dependency = dependencies.get("typedb-protocol")
        if not isinstance(protocol_dependency, dict):
            raise ValidationError(
                f"Legacy TypeDB upstream driver has no typedb-protocol table: "
                f"{component.downstream_name}"
            )
        if protocol_dependency.pop("features", None) != []:
            raise ValidationError(
                f"Legacy TypeDB upstream typedb-protocol features drifted: "
                f"{component.downstream_name}"
            )
        protocol = legacy_protocol_component(component.band)
        expected_protocol_version = f"={protocol.upstream_version}"
        if protocol_dependency.get("version") != expected_protocol_version:
            raise ValidationError(
                f"Legacy TypeDB upstream typedb-protocol version drifted: "
                f"package={component.downstream_name!r}, "
                f"actual={protocol_dependency.get('version')!r}, "
                f"expected={expected_protocol_version!r}"
            )
        protocol_dependency["package"] = protocol.downstream_name
        protocol_dependency["path"] = f"../{PurePosixPath(protocol.vendor_directory).name}"
        if "dev-dependencies" in expected:
            raise ValidationError(
                f"Legacy TypeDB upstream unexpectedly has dev-dependencies: "
                f"{component.downstream_name}"
            )
        expected["dev-dependencies"] = {"rand": "0.8", "serde_json": "1"}
    elif component.upstream_name != "typedb-protocol":
        raise ValidationError(f"Unknown legacy TypeDB package: {component.upstream_name}")

    return normalize_dependency_specs(
        expected,
        label=f"expected legacy manifest {component.downstream_name}",
    )


def validate_legacy_component_tree(
    workspace_root: Path,
    component: LegacyTypeDbComponent,
    archive: bytes,
) -> tuple[str, ...]:
    """Compare one active legacy package with its exact checksummed upstream archive."""
    upstream = verified_crate_archive_files(component, archive)
    local = local_legacy_vendor_files(workspace_root / component.vendor_directory)
    local_manifest = local["Cargo.toml"]
    pinned_manifest_digest = PREEXISTING_LEGACY_MANIFEST_DIGESTS.get(component.downstream_name)
    if pinned_manifest_digest is not None:
        actual_manifest_digest = hashlib.sha256(local_manifest).hexdigest()
        if actual_manifest_digest != pinned_manifest_digest:
            raise ValidationError(
                "Pre-existing legacy TypeDB manifest drifted from its immutable registry "
                f"payload: package={component.downstream_name!r}, "
                f"actual_sha256={actual_manifest_digest!r}, "
                f"expected_sha256={pinned_manifest_digest!r}"
            )
    upstream_manifest_body = upstream.get("Cargo.toml")
    if upstream_manifest_body is None:
        raise ValidationError(
            f"Legacy TypeDB upstream archive has no Cargo.toml: {component.archive_url}"
        )
    expected_manifest = expected_legacy_cargo_manifest(
        component,
        parse_legacy_cargo_manifest(
            upstream_manifest_body,
            label=f"upstream legacy manifest {component.downstream_name}",
        ),
    )
    actual_manifest = normalize_dependency_specs(
        parse_legacy_cargo_manifest(
            local_manifest,
            label=f"local legacy manifest {component.downstream_name}",
        ),
        label=f"local legacy manifest {component.downstream_name}",
    )
    if actual_manifest != expected_manifest:
        raise ValidationError(
            "Legacy TypeDB manifest exceeds the packaging-only transform: "
            f"{component.downstream_name}"
        )
    upstream_source = {path: body for path, body in upstream.items() if path.startswith("src/")}
    local_source = {path: body for path, body in local.items() if path.startswith("src/")}
    if not upstream_source:
        raise ValidationError(
            f"Legacy TypeDB upstream archive has no src tree: {component.archive_url}"
        )
    if set(local_source) != set(upstream_source):
        raise ValidationError(
            f"Legacy TypeDB source path inventory drifted for {component.downstream_name}: "
            f"missing={sorted(set(upstream_source) - set(local_source))!r}, "
            f"unexpected={sorted(set(local_source) - set(upstream_source))!r}"
        )
    upstream_license = upstream.get("LICENSE")
    if upstream_license is None or local["LICENSE"] != upstream_license:
        raise ValidationError(
            f"Legacy TypeDB LICENSE must remain byte-identical to upstream: "
            f"{component.downstream_name}"
        )
    upstream_readme = upstream.get(VENDOR_README)
    if upstream_readme is None:
        raise ValidationError(
            f"Legacy TypeDB upstream archive has no README.md: {component.archive_url}"
        )
    disclosure = legacy_vendor_readme_disclosure(component)
    expected_readme = disclosure + upstream_readme
    actual_readme = local[VENDOR_README]
    if actual_readme != expected_readme:
        if disclosure and not actual_readme.startswith(disclosure):
            raise ValidationError(
                f"Legacy TypeDB downstream README disclosure drifted: {component.downstream_name}"
            )
        if disclosure:
            raise ValidationError(
                "Legacy TypeDB downstream README must retain the exact upstream suffix: "
                f"{component.downstream_name}"
            )
        raise ValidationError(
            "Pre-existing band-7 package README must remain byte-identical to upstream: "
            f"{component.downstream_name}"
        )

    changed = {path for path in upstream_source if local_source[path] != upstream_source[path]}
    expected_license = {
        "typedb-driver": APACHE_2_LICENSE,
        "typedb-protocol": MPL_2_LICENSE,
    }.get(component.upstream_name)
    if expected_license is None or component.license != expected_license:
        raise ValidationError(
            f"Unknown legacy TypeDB provenance policy: {component.downstream_name}"
        )
    if changed:
        raise ValidationError(
            f"Legacy TypeDB source must remain byte-identical to upstream: "
            f"package={component.downstream_name!r}, changed={sorted(changed)!r}"
        )
    return ()


def validate_legacy_vendor_provenance(
    workspace_manifest: Path,
    *,
    archive_loader: Callable[[LegacyTypeDbComponent], bytes] = download_legacy_crate_archive,
) -> dict[str, list[str]]:
    """Fail closed unless all active legacy trees match their immutable upstreams."""
    workspace_root = workspace_manifest.resolve().parent
    result: dict[str, list[str]] = {}
    for component in LEGACY_TYPEDB_COMPONENTS:
        archive = archive_loader(component)
        result[component.downstream_name] = list(
            validate_legacy_component_tree(workspace_root, component, archive)
        )
    return result


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

    compatibility_components_by_manifest = {
        (workspace_root / component.manifest_path).resolve(): component
        for component in LEGACY_TYPEDB_COMPONENTS
    }
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
        component = compatibility_components_by_manifest.get(package.manifest.resolve())
        # License follows the source tree, not a mutable Cargo package name.
        # Renaming a compatibility package cannot relicense TypeDB-derived
        # Apache/MPL source as first-party MIT code.
        expected = component.license if component is not None else MIT_LICENSE
        if actual != expected:
            raise ValidationError(
                f"Cargo package {package.name} effective license drifted: "
                f"actual={actual!r}, expected={expected!r}"
            )
        if component is not None:
            expected_license_file = (
                None
                if component.downstream_name in IMMUTABLE_BASELINE_CRATES
                else VENDOR_LICENSE_FILE
            )
            if package_table.get("license-file") != expected_license_file:
                raise ValidationError(
                    f"Cargo package {package.name} license-file drifted from its immutable "
                    "compatibility-package contract: "
                    f"actual={package_table.get('license-file')!r}, "
                    f"expected={expected_license_file!r}"
                )
            canonical_license_bytes(
                package.manifest.parent / VENDOR_LICENSE_FILE,
                license_id=component.license,
                label=f"Cargo package {package.name} license-file",
            )
        elif not package.vendored:
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


def validate_legacy_vendor_component_identities(
    workspace_manifest: Path,
    packages: Sequence[CargoPackage],
) -> tuple[str, ...]:
    """Bind every active legacy package path and all crates.io-facing metadata."""
    workspace_root = workspace_manifest.resolve().parent
    expected_by_manifest = {
        (workspace_root / component.manifest_path).resolve(): component
        for component in LEGACY_TYPEDB_COMPONENTS
    }
    if len(expected_by_manifest) != len(LEGACY_TYPEDB_COMPONENTS):
        raise ValidationError("Legacy TypeDB component manifest identities are not unique")

    actual_by_manifest = {
        package.manifest.resolve(): package for package in packages if package.vendored
    }
    missing = sorted(
        str(path.relative_to(workspace_root))
        for path in expected_by_manifest.keys() - actual_by_manifest
    )
    unexpected = sorted(
        str(path.relative_to(workspace_root))
        for path in actual_by_manifest.keys() - expected_by_manifest
    )
    if missing or unexpected:
        raise ValidationError(
            "Cargo workspace legacy vendor members drifted from their canonical paths: "
            f"missing={missing!r}, unexpected={unexpected!r}"
        )

    identities: list[str] = []
    for manifest, component in expected_by_manifest.items():
        payload = read_toml(manifest, label=f"legacy vendor manifest {component.manifest_path}")
        package_table = payload.get("package")
        if not isinstance(package_table, dict):
            raise ValidationError(f"Legacy vendor manifest has no [package]: {manifest}")
        actual_path = str(manifest.relative_to(workspace_root))
        expected_path = component.manifest_path
        if actual_path != expected_path:
            raise ValidationError(
                f"Legacy TypeDB component {component.downstream_name} path drifted: "
                f"actual={actual_path!r}, expected={expected_path!r}"
            )
        expected_metadata = {
            "name": component.downstream_name,
            "version": component.downstream_version,
            "license": component.license,
            "license-file": (
                None
                if component.downstream_name in IMMUTABLE_BASELINE_CRATES
                else VENDOR_LICENSE_FILE
            ),
            "description": LEGACY_VENDOR_DESCRIPTIONS[component.downstream_name],
            "homepage": f"https://github.com/typedb/{component.upstream_name}",
            "repository": TYPEBRIDGE_REPOSITORY,
            "readme": VENDOR_README,
        }
        actual_metadata = {key: package_table.get(key) for key in expected_metadata}
        if actual_metadata != expected_metadata:
            raise ValidationError(
                f"Legacy TypeDB component {component.downstream_name} immutable package metadata "
                f"drifted: actual={actual_metadata!r}, expected={expected_metadata!r}"
            )
        identity_tuple = (
            component.manifest_path,
            component.downstream_name,
            component.downstream_version,
            component.license,
        )
        identities.append("|".join(identity_tuple))
    return tuple(identities)


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


def workflow_publish_sequence(workflow: Path) -> tuple[str, ...]:
    """Return the literal ordered crate arguments passed to the publish helper."""
    if not workflow.is_file() or workflow.is_symlink():
        raise ValidationError(f"Release workflow is missing or non-regular: {workflow}")
    source = workflow.read_text(encoding="utf-8")
    return tuple(re.findall(r'publish-crate\.sh ([A-Za-z0-9_-]+)["\']?', source))


def workflow_preflight_sequences(workflow: Path) -> tuple[tuple[str, ...], tuple[str, ...]]:
    """Return the local-patch and full-package sequences from release preflight."""
    if not workflow.is_file() or workflow.is_symlink():
        raise ValidationError(f"Release workflow is missing or non-regular: {workflow}")
    source = workflow.read_text(encoding="utf-8")
    command = 'cargo package --locked --allow-dirty --all-features -p "$crate" "${patches[@]}"'
    lines = source.splitlines()
    command_lines = [index for index, line in enumerate(lines) if command in line]
    if len(command_lines) != 1:
        raise ValidationError(
            "Release workflow must contain exactly one full cargo-package preflight loop"
        )
    command_line = command_lines[0]
    try:
        do_line = max(index for index in range(command_line) if lines[index].strip() == "do")
        for_line = max(
            index for index in range(do_line) if lines[index].strip() == "for crate in \\"
        )
    except ValueError as error:
        raise ValidationError("Cargo-package preflight loop is malformed") from error

    packages: list[str] = []
    for line in lines[for_line + 1 : do_line]:
        token = line.strip()
        if token.endswith("\\"):
            token = token[:-1].rstrip()
        if re.fullmatch(r"[A-Za-z0-9_-]+", token) is None:
            raise ValidationError(f"Cargo-package preflight contains an invalid crate: {token!r}")
        packages.append(token)
    patches = tuple(re.findall(r"patch\.crates-io\.([A-Za-z0-9_-]+)\.path=", source))
    return patches, tuple(packages)


def workflow_registry_preflight_sequences(
    workflow: Path,
) -> tuple[tuple[str, ...], tuple[str, ...]]:
    """Return checksum-only and expected-new crates.io key preflights."""
    if not workflow.is_file() or workflow.is_symlink():
        raise ValidationError(f"Release workflow is missing or non-regular: {workflow}")
    source = workflow.read_text(encoding="utf-8")
    preexisting = tuple(re.findall(r"--verify-preexisting ([A-Za-z0-9_-]+)", source))
    command = 'bash ../scripts/ci/publish_crate_idempotently.sh --preflight "$crate"'
    lines = source.splitlines()
    command_lines = [index for index, line in enumerate(lines) if command in line]
    if len(command_lines) != 1:
        raise ValidationError(
            "Release workflow must contain exactly one crates.io key-preflight loop"
        )
    preexisting_marker = "--verify-preexisting "
    package_marker = (
        'cargo package --locked --allow-dirty --all-features -p "$crate" "${patches[@]}"'
    )
    if (
        source.count(preexisting_marker) != len(PREEXISTING_CRATES)
        or source.count(package_marker) != 1
    ):
        raise ValidationError("Release workflow crates.io preflight markers are malformed")
    if not (
        source.rindex(preexisting_marker) < source.index(package_marker) < source.index(command)
    ):
        raise ValidationError(
            "Release workflow crates.io preflights are not ordered before publication"
        )
    command_line = command_lines[0]
    try:
        do_line = max(index for index in range(command_line) if lines[index].strip() == "do")
        for_line = max(
            index for index in range(do_line) if lines[index].strip() == "for crate in \\"
        )
    except ValueError as error:
        raise ValidationError("Crates.io key-preflight loop is malformed") from error

    candidates: list[str] = []
    for line in lines[for_line + 1 : do_line]:
        token = line.strip()
        if token.endswith("\\"):
            token = token[:-1].rstrip()
        if re.fullmatch(r"[A-Za-z0-9_-]+", token) is None:
            raise ValidationError(f"Crates.io key-preflight contains an invalid crate: {token!r}")
        candidates.append(token)
    return preexisting, tuple(candidates)


def validate_native_notice_workflow(workflow: Path) -> None:
    """Require the pinned dependency-notice freshness gate before identity validation."""
    if not workflow.is_file() or workflow.is_symlink():
        raise ValidationError(f"Release workflow is missing or non-regular: {workflow}")
    source = workflow.read_text(encoding="utf-8")
    install_marker = f"tool: cargo-about@{NATIVE_NOTICE_CARGO_ABOUT_VERSION}"
    check_marker = "python scripts/ci/generate_native_dependency_notice.py --check"
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
    if source.count(identity_marker) != 1:
        raise ValidationError("Release workflow identity-validator marker is malformed")
    if (
        not source.index(install_marker)
        < source.index(check_marker)
        < source.index(identity_marker)
    ):
        raise ValidationError(
            "Release workflow must install cargo-about and check the native notice before "
            "release identity validation"
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
        "rust-crates": "Rust crate artifact upload",
        "target/package": "Cargo archive directory",
        "type-bridge-typedb-driver-b8": "owner-gated band-8 registry path",
        "type-bridge-typedb-protocol-b8": "owner-gated band-8 registry path",
        "validate_fresh_typedb_runtime_package.sh": "published-Cargo consumer probe",
        "validate_rust_release_artifacts.py": "Rust release artifact validator",
    }
    present = [description for marker, description in forbidden_markers.items() if marker in source]
    if present:
        raise ValidationError(
            "Python/npm-only release workflow contains forbidden crates.io paths: "
            f"{sorted(present)!r}"
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


def validate_typedb_runtime_driver_pins(package: CargoPackage) -> tuple[str, str, str]:
    """Bind all three driver requirements to their runtime constants."""
    manifest = read_toml(package.manifest, label="TypeDB runtime Cargo manifest")
    dependencies = manifest.get("dependencies")
    if not isinstance(dependencies, dict):
        raise ValidationError("TypeDB runtime Cargo manifest has no [dependencies] table")
    specifications: dict[str, tuple[dict[str, Any], re.Pattern[str], str]] = {}
    for dependency, pattern, label in (
        (TYPEDB_BAND7_DEPENDENCY, TYPEDB_RUNTIME_BAND7_PIN_PATTERN, "band-7 package"),
        (TYPEDB_BAND8_DEPENDENCY, TYPEDB_RUNTIME_BAND8_PIN_PATTERN, "band-8 package"),
        (TYPEDB_BAND9_DEPENDENCY, TYPEDB_RUNTIME_BAND9_PIN_PATTERN, "band-9 upstream"),
    ):
        specification = dependencies.get(dependency)
        if not isinstance(specification, dict):
            raise ValidationError(f"TypeDB runtime must declare {dependency} as a dependency table")
        requirement = specification.get("version")
        if not isinstance(requirement, str):
            raise ValidationError(
                f"TypeDB runtime {dependency} dependency has no version requirement"
            )
        dependency_package = specification.get("package", dependency)
        if dependency_package != dependency:
            raise ValidationError(
                f"TypeDB runtime {label} dependency has the wrong package identity: "
                f"actual={dependency_package!r}, expected={dependency!r}"
            )
        specifications[dependency] = (specification, pattern, label)

    source = package.manifest.parent / "src/lib.rs"
    if not source.is_file() or source.is_symlink():
        raise ValidationError(f"TypeDB runtime source is missing or non-regular: {source}")
    try:
        source_text = source.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        raise ValidationError(f"Could not read TypeDB runtime source {source}: {error}") from error
    pinned_versions: list[str] = []
    for dependency in (
        TYPEDB_BAND7_DEPENDENCY,
        TYPEDB_BAND8_DEPENDENCY,
        TYPEDB_BAND9_DEPENDENCY,
    ):
        specification, pattern, label = specifications[dependency]
        matches = pattern.findall(source_text)
        if len(matches) != 1:
            raise ValidationError(f"TypeDB runtime source must define the {label} pin exactly once")
        pinned_version = stable_version(
            matches[0], label=f"TypeDB runtime pinned {label} driver version"
        )
        expected_requirement = f"={pinned_version}"
        requirement = specification["version"]
        if requirement != expected_requirement:
            raise ValidationError(
                f"TypeDB runtime {dependency} dependency must exactly match its runtime pin: "
                f"actual={requirement!r}, expected={expected_requirement!r}"
            )
        pinned_versions.append(pinned_version)
    return pinned_versions[0], pinned_versions[1], pinned_versions[2]


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
        if re.fullmatch(r"3\.12\.(?:0|[1-9]\d*)", version) is None:
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
    driver_b7 = canonical_license_bytes(
        workspace_root / "vendor/typedb-driver-b7/LICENSE",
        license_id=APACHE_2_LICENSE,
        label="band-7 driver Apache-2.0 LICENSE",
    )
    driver_b8 = read_bytes(
        workspace_root / "vendor/typedb-driver-b8/LICENSE",
        label="band-8 driver Apache-2.0 LICENSE",
    )
    if driver_b8 != driver_b7:
        raise ValidationError("Band-7 and band-8 driver LICENSE files must be byte-identical")
    protocol_b7 = canonical_license_bytes(
        workspace_root / "vendor/typedb-protocol-b7/LICENSE",
        license_id=MPL_2_LICENSE,
        label="band-7 protocol MPL-2.0 LICENSE",
    )
    protocol_b8 = read_bytes(
        workspace_root / "vendor/typedb-protocol-b8/LICENSE",
        label="band-8 protocol MPL-2.0 LICENSE",
    )
    if protocol_b8 != protocol_b7:
        raise ValidationError("Band-7 and band-8 protocol LICENSE files must be byte-identical")

    headings = (
        "## TypeBridge-authored portions — MIT License",
        "## TypeDB driver portions — Apache License 2.0",
        "## TypeDB protocol portions — Mozilla Public License 2.0",
        "## ed25519-dalek 2.2.0 — BSD 3-Clause License",
        "## curve25519-dalek 4.1.3 — BSD 3-Clause License",
    )
    expected_bodies = (mit, driver_b7, protocol_b7, None, None)
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


def validate_legacy_notice_provenance(notice: str) -> None:
    """Require every band-7/band-8 package and archive row to be exact."""
    compact_notice = " ".join(notice.split())
    required_disclosures = (
        "The band-7 packages are unofficial namespaced, already-published packaging-only "
        "republications.",
        "The band-8 compatibility copies are source-unmodified, and their registry "
        "publication requires separate explicit TypeBridge owner authorization.",
        "Registry publication of the band-8 packages is never implied by this source "
        "checkout or native distribution and requires separate explicit TypeBridge "
        "owner authorization.",
        "If distributed, these exact source-unmodified packages are the authorized "
        "compatibility artifacts, with the protocol package preceding the driver package.",
    )
    for disclosure in required_disclosures:
        if disclosure not in compact_notice:
            raise ValidationError(
                f"Native notice band-8 registry disposition is missing: {disclosure!r}"
            )
    forbidden_disclosure = (
        "The renamed crates are also distributed as immutable crates.io source packages"
    )
    if forbidden_disclosure in compact_notice:
        raise ValidationError(
            "Native notice falsely describes unpublished band-8 keys as distributed"
        )
    for transient_claim in (
        "band-8 namespaced registry keys are currently absent",
        "downstream package key is currently unpublished",
        "unpublished owner-gated packaging candidate",
    ):
        if transient_claim in compact_notice.lower():
            raise ValidationError(
                f"Native notice freezes transient band-8 registry state: {transient_claim!r}"
            )
    expected_component_rows = tuple(
        (
            str(component.band),
            f"`{component.downstream_name}` {component.downstream_version}",
            (
                f"TypeDB `{component.upstream_name}` tag [{component.upstream_version}]"
                f"(https://github.com/typedb/{component.upstream_name}/tree/"
                f"{component.upstream_commit}) (commit `{component.upstream_commit}`)"
            ),
            component.license_status,
        )
        for component in LEGACY_TYPEDB_COMPONENTS
    )
    actual_component_rows = tuple(
        tuple(cell.strip() for cell in row)
        for row in re.findall(
            r"^\| (7|8) \| ([^|]+) \| ([^|]+) \| ([^|]+) \|$",
            notice,
            re.MULTILINE,
        )
    )
    if actual_component_rows != expected_component_rows:
        raise ValidationError(
            "Native notice band-7/band-8 component provenance drifted: "
            f"actual={actual_component_rows!r}, expected={expected_component_rows!r}"
        )

    expected_keys = {
        (component.upstream_name, component.upstream_version)
        for component in LEGACY_TYPEDB_COMPONENTS
    }
    expected_archive_rows = tuple(
        (
            component.archive_url,
            component.upstream_name,
            component.upstream_version,
            component.archive_checksum,
        )
        for component in LEGACY_TYPEDB_COMPONENTS
    )
    all_archive_rows = re.findall(
        r"^\| <(https://static\.crates\.io/crates/(typedb-(?:driver|protocol))/"
        r"\2-([^/>]+)\.crate)> \| `([^`]+)` \|$",
        notice,
        re.MULTILINE,
    )
    actual_archive_rows = tuple(
        row for row in all_archive_rows if (row[1], row[2]) in expected_keys
    )
    if actual_archive_rows != expected_archive_rows:
        raise ValidationError(
            "Native notice band-7/band-8 archive provenance drifted: "
            f"actual={actual_archive_rows!r}, expected={expected_archive_rows!r}"
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
    validate_legacy_notice_provenance(notice)

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
            if version.startswith("3.12.")
        ]
        expected_archive = (component.version, component.checksum)
        if archive_rows != [expected_archive]:
            raise ValidationError(
                f"Native notice {component.name} 3.12.x archive provenance disagrees with "
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
        "No pre-existing namespaced registry key is assumed by this source checkout",
        "publication is never authorized merely by the checkout.",
        "Any first publication requires separate explicit TypeBridge owner authorization.",
        "If distributed under that authorization, these exact source-unmodified packages "
        "are the authorized compatibility artifacts, with protocol preceding driver.",
        "source checkout grants no publication authority; separately authorized "
        "distribution uses the exact source-unmodified protocol-before-driver packages",
        "TypeDB remains the upstream project and original source",
        "TypeBridge-authored crates and bindings remain MIT.",
        "Files derived from the TypeDB drivers retain Apache-2.0",
        "files derived from the TypeDB protocols retain MPL-2.0",
    ):
        if phrase not in compact_vendor_readme:
            raise ValidationError(f"Vendor provenance is missing: {phrase!r}")
    for transient_claim in (
        "band-8 namespaced registry keys are currently absent",
        "downstream package key is currently unpublished",
        "unpublished owner-gated packaging candidate",
    ):
        if transient_claim in compact_vendor_readme.lower():
            raise ValidationError(
                f"Vendor provenance freezes transient band-8 registry state: {transient_claim!r}"
            )

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
    legacy_vendor_identities = validate_legacy_vendor_component_identities(
        workspace_manifest,
        packages,
    )
    by_name = {package.name: package for package in packages}
    missing_v2_crates = sorted(set(UNPUBLISHED_V2_CRATES) - set(by_name))
    if missing_v2_crates:
        raise ValidationError(
            "Authoritative unpublished V2 crates are absent from the workspace: "
            f"{missing_v2_crates!r}"
        )
    non_explicit_v2_publish_settings = sorted(
        name
        for name in PYTHON_NPM_UNPUBLISHED_CRATES
        if name not in by_name or not by_name[name].publish_explicitly_false
    )
    if non_explicit_v2_publish_settings:
        raise ValidationError(
            "Authoritative V2 and binding crates must declare package.publish = false exactly: "
            f"{non_explicit_v2_publish_settings!r}"
        )
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

    runtime_package = by_name.get(TYPEDB_RUNTIME_PACKAGE)
    if runtime_package is None:
        raise ValidationError(f"TypeDB runtime package is absent: {TYPEDB_RUNTIME_PACKAGE}")
    (
        typedb_runtime_band7_driver_pin,
        typedb_runtime_driver_pin,
        typedb_runtime_band9_driver_pin,
    ) = validate_typedb_runtime_driver_pins(runtime_package)
    locked_band9_components = validate_native_band9_provenance(
        workspace_manifest,
        expected_driver_version=typedb_runtime_band9_driver_pin,
        root_license=root_python_manifest.resolve().parent / "LICENSE",
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
    validate_native_notice_workflow(release_workflow.resolve())
    if artifact_contract == ARTIFACT_CONTRACT_CARGO_INCLUSIVE:
        actual_publish_sequence = workflow_publish_sequence(release_workflow.resolve())
        if actual_publish_sequence != PUBLISHED_CRATES:
            raise ValidationError(
                "Cargo publish sequence is incomplete or reordered: "
                f"actual={actual_publish_sequence!r}, expected={PUBLISHED_CRATES!r}"
            )
        preflight_patches, preflight_packages = workflow_preflight_sequences(
            release_workflow.resolve()
        )
        if preflight_patches != PUBLISHED_CRATES:
            raise ValidationError(
                "Cargo preflight patch sequence is incomplete or reordered: "
                f"actual={preflight_patches!r}, expected={PUBLISHED_CRATES!r}"
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
    else:
        validate_python_npm_only_workflow(release_workflow.resolve())
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
                if target.name not in UNPUBLISHED_V2_CRATES:
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
    if artifact_contract == ARTIFACT_CONTRACT_PYTHON_NPM_ONLY:
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
        "cargo_licenses": cargo_licenses,
        "cargo_manifest_publishable_crates": list(PUBLISHED_CRATES),
        "cargo_packages": {package.name: package.version for package in packages},
        "cargo_publication_plan": (
            list(PUBLISHED_CRATES) if artifact_contract == ARTIFACT_CONTRACT_CARGO_INCLUSIVE else []
        ),
        "crates_io_mutation": crates_io_mutation,
        "historical_band9_quarantine": list(quarantined_band9_packages),
        "legacy_vendor_identities": list(legacy_vendor_identities),
        "cargo_manifest_dependency_order": {
            name: [dependency.name for dependency in by_name[name].path_dependencies]
            for name in PUBLISHED_CRATES
        },
        "rust_publication_blockers": publication_blockers,
        "unpublished_crates": sorted(set(by_name) - expected_publishable),
        "unpublished_v2_crates": list(UNPUBLISHED_V2_CRATES),
        "status": ("blocked" if crates_io_mutation and publication_blockers else "ok"),
        "tag": tag,
        "node_package_lock_version": node_lock_version,
        "python_core_requirement": python_core_requirement,
        "python_package_version": python_package_version,
        "typedb_runtime_band7_driver_pin": typedb_runtime_band7_driver_pin,
        "typedb_runtime_driver_pin": typedb_runtime_driver_pin,
        "typedb_runtime_band9_driver_pin": typedb_runtime_band9_driver_pin,
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
        "workspace-internal V2 crates: "
        f"{', '.join(edges)}. An owner-approved packaging decision is required; "
        "the nine V2 crates must remain publish=false.",
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
    report["legacy_vendor_provenance"] = validate_legacy_vendor_provenance(args.workspace)
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ValidationError as error:
        print(f"Release identity validation failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
