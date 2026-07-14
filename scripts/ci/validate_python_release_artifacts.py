#!/usr/bin/env python3
"""Validate the exact Python distribution set a release will publish.

The validator uses only the Python standard library. It rejects missing or
extra artifacts, unsafe/corrupt archives, filename/metadata disagreement,
invalid wheel RECORD hashes, platform-matrix drift, and source-tree bytecode.
"""

from __future__ import annotations

import argparse
import base64
import configparser
import csv
import hashlib
import io
import json
import re
import stat
import sys
import tarfile
import tomllib
import zipfile
from collections.abc import Iterable, Sequence
from dataclasses import dataclass
from email import policy
from email.message import Message
from email.parser import BytesParser
from pathlib import Path, PurePosixPath
from typing import Any

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
CORE_WHEEL_BUCKETS = frozenset(
    {
        "linux-x86_64",
        "linux-aarch64",
        "macos-x86_64",
        "macos-arm64",
        "windows-x86_64",
    }
)


class ValidationError(RuntimeError):
    """A release artifact set is incomplete, unsafe, or internally inconsistent."""


@dataclass(frozen=True, slots=True)
class PackageSpec:
    """Repository-owned metadata and required public package members."""

    key: str
    name: str
    distribution: str
    version: str
    requires_python: str
    dependencies: tuple[str, ...]
    optional_dependencies: tuple[tuple[str, tuple[str, ...]], ...]
    entry_points: tuple[tuple[str, tuple[tuple[str, str], ...]], ...]
    pure: bool
    wheel_members: frozenset[str]
    sdist_members: frozenset[str]

    @property
    def scripts(self) -> tuple[tuple[str, str], ...]:
        """Return the console-script subset for compatibility with callers."""
        return dict(self.entry_points).get("console_scripts", ())


@dataclass(frozen=True, slots=True)
class WheelFilename:
    """The five canonical wheel filename components used by this release."""

    distribution: str
    version: str
    python_tag: str
    abi_tag: str
    platform_tag: str

    @property
    def tags(self) -> frozenset[str]:
        """Expand compressed filename tags into their PEP 427 Cartesian product."""
        return frozenset(
            f"{python_tag}-{abi_tag}-{platform_tag}"
            for python_tag in self.python_tag.split(".")
            for abi_tag in self.abi_tag.split(".")
            for platform_tag in self.platform_tag.split(".")
        )


def normalize_name(value: str) -> str:
    """Return the PEP 503 comparison form for a distribution name."""
    return re.sub(r"[-_.]+", "-", value).lower()


def normalized_distribution(value: str) -> str:
    """Return the wheel/sdist filename form used by the project builders."""
    return normalize_name(value).replace("-", "_")


def normalize_requires_python(value: str) -> tuple[str, ...]:
    """Compare comma-separated bounds without depending on clause order/spacing."""
    return tuple(sorted(part.replace(" ", "") for part in value.split(",") if part.strip()))


def normalize_extra(value: str) -> str:
    """Return the PEP 685 comparison form for an extra name."""
    return normalize_name(value)


def split_requirement_marker(value: str) -> tuple[str, str]:
    """Split one repository requirement into requirement and marker clauses."""
    requirement, separator, marker = value.partition(";")
    if not requirement.strip() or (separator and not marker.strip()):
        raise ValidationError(f"Invalid dependency requirement: {value!r}")
    return requirement.strip(), marker.strip()


def normalize_requirement(value: str) -> str:
    """Canonicalize the repository's PEP 508 dependency strings using stdlib."""
    requirement, marker = split_requirement_marker(value)
    match = re.fullmatch(
        r"([A-Za-z0-9][A-Za-z0-9._-]*)(?:\[([^]]+)\])?\s*(.*)",
        requirement,
    )
    if match is None:
        raise ValidationError(f"Unsupported dependency requirement: {value!r}")
    name, raw_extras, raw_constraint = match.groups()
    extras = ""
    if raw_extras is not None:
        normalized_extras = sorted(
            normalize_extra(extra.strip()) for extra in raw_extras.split(",")
        )
        if not all(normalized_extras) or len(set(normalized_extras)) != len(normalized_extras):
            raise ValidationError(f"Invalid dependency extras: {value!r}")
        extras = f"[{','.join(normalized_extras)}]"
    constraint = raw_constraint.strip()
    if constraint.startswith("(") and constraint.endswith(")"):
        constraint = constraint[1:-1].strip()
    if constraint.startswith("@"):
        constraint = "@" + constraint[1:].strip()
    elif constraint:
        clauses = [re.sub(r"\s+", "", clause) for clause in constraint.split(",")]
        if not all(clauses):
            raise ValidationError(f"Invalid dependency constraints: {value!r}")
        constraint = ",".join(sorted(clauses))
    normalized = f"{normalize_name(name)}{extras}{constraint}"
    if marker:
        normalized_marker = re.sub(r"\s+", "", marker).replace('"', "'").lower()
        atomic_parentheses = re.compile(r"\(([^()]*)\)")
        while True:
            collapsed = atomic_parentheses.sub(
                lambda match: (
                    match.group(0)
                    if re.search(r"\b(?:and|or)\b", match.group(1))
                    else match.group(1)
                ),
                normalized_marker,
            )
            if collapsed == normalized_marker:
                break
            normalized_marker = collapsed
        normalized = f"{normalized};{normalized_marker}"
    return normalized


def dependency_metadata(spec: PackageSpec) -> tuple[tuple[str, ...], tuple[str, ...]]:
    """Return canonical Provides-Extra and complete Requires-Dist contracts."""
    extras: list[str] = []
    requirements = list(spec.dependencies)
    for extra, dependencies in spec.optional_dependencies:
        normalized_extra = normalize_extra(extra)
        extras.append(normalized_extra)
        for dependency in dependencies:
            requirement, marker = split_requirement_marker(dependency)
            extra_marker = f"extra == '{normalized_extra}'"
            if not marker:
                combined_marker = extra_marker
            elif re.search(r"\bor\b", marker, flags=re.IGNORECASE):
                combined_marker = f"({marker}) and {extra_marker}"
            else:
                combined_marker = f"{marker} and {extra_marker}"
            requirements.append(f"{requirement}; {combined_marker}")
    return tuple(sorted(extras)), tuple(sorted(map(normalize_requirement, requirements)))


def source_package_members(
    repository_root: Path,
    package_path: str,
    *,
    archive_prefix: str | None = None,
) -> frozenset[str]:
    """Derive the complete clean package inventory from repository source files."""
    package_root = repository_root / package_path
    prefix = PurePosixPath(archive_prefix or package_path)
    if not package_root.is_dir() or package_root.is_symlink():
        raise ValidationError(f"Source package directory is missing or unsafe: {package_root}")
    members: set[str] = set()
    for candidate in sorted(package_root.rglob("*")):
        source_relative = candidate.relative_to(package_root)
        relative = prefix / PurePosixPath(source_relative.as_posix())
        if candidate.is_symlink():
            raise ValidationError(f"Source package contains a symbolic link: {relative}")
        if "__pycache__" in relative.parts or candidate.suffix in {".pyc", ".pyo"}:
            continue
        if candidate.suffix in {".so", ".pyd", ".dylib"}:
            # Local editable/native builds place the generated extension beside
            # the authored Python package. The wheel inventory adds exactly one
            # platform-checked extension separately below.
            continue
        if candidate.is_dir():
            continue
        if not candidate.is_file():
            raise ValidationError(f"Source package contains a non-regular file: {relative}")
        members.add(relative.as_posix())
    expected_init = (prefix / "__init__.py").as_posix()
    if expected_init not in members:
        raise ValidationError(f"Source package has no __init__.py: {package_root}")
    return frozenset(members)


def project_entry_points(
    project: dict[str, Any],
    *,
    source: str,
) -> tuple[tuple[str, tuple[tuple[str, str], ...]], ...]:
    """Return the complete PEP 621 entry-point inventory for one project."""
    groups: dict[str, tuple[tuple[str, str], ...]] = {}

    def add_group(group: str, raw_entries: object, *, field: str) -> None:
        if not isinstance(raw_entries, dict) or not all(
            isinstance(name, str) and bool(name) and isinstance(target, str) and bool(target)
            for name, target in raw_entries.items()
        ):
            raise ValidationError(f"Invalid {field} in {source}")
        if group in groups:
            raise ValidationError(f"Duplicate entry-point group {group!r} in {source}")
        if raw_entries:
            groups[group] = tuple(sorted(raw_entries.items()))

    add_group("console_scripts", project.get("scripts", {}), field="project.scripts")
    add_group("gui_scripts", project.get("gui-scripts", {}), field="project.gui-scripts")
    raw_groups = project.get("entry-points", {})
    if not isinstance(raw_groups, dict):
        raise ValidationError(f"Invalid project.entry-points in {source}")
    for group, entries in raw_groups.items():
        if not isinstance(group, str) or not group:
            raise ValidationError(f"Invalid project.entry-points group in {source}")
        if group in {"console_scripts", "gui_scripts"}:
            raise ValidationError(
                f"Reserved entry-point group {group!r} must use its PEP 621 table in {source}"
            )
        add_group(group, entries, field=f"project.entry-points.{group}")
    return tuple(sorted(groups.items()))


def load_package_specs(repository_root: Path, expected_version: str) -> dict[str, PackageSpec]:
    """Load authoritative package metadata from both checked-out pyprojects."""
    root_members = source_package_members(repository_root, "type_bridge")
    core_members = source_package_members(
        repository_root,
        "type-bridge-core/python/type_bridge_core",
        archive_prefix="type_bridge_core",
    )
    definitions = {
        "core": (
            repository_root / "type-bridge-core" / "pyproject.toml",
            False,
            core_members,
            frozenset({"PKG-INFO", "pyproject.toml", "Cargo.toml", "crates/python/Cargo.toml"})
            | frozenset(f"python/{name}" for name in core_members),
        ),
        "root": (
            repository_root / "pyproject.toml",
            True,
            root_members,
            root_members | {"PKG-INFO", "pyproject.toml"},
        ),
    }
    specs: dict[str, PackageSpec] = {}
    for key, (pyproject, pure, wheel_members, sdist_members) in definitions.items():
        try:
            project = tomllib.loads(pyproject.read_text(encoding="utf-8"))["project"]
            name = str(project["name"])
            version = str(project["version"])
            requires_python = str(project["requires-python"])
            raw_dependencies = project.get("dependencies", [])
            if not isinstance(raw_dependencies, list) or not all(
                isinstance(value, str) for value in raw_dependencies
            ):
                raise ValidationError(f"Invalid project.dependencies in {pyproject}")
            dependencies = tuple(raw_dependencies)
            raw_optional = project.get("optional-dependencies", {})
            if not isinstance(raw_optional, dict):
                raise ValidationError(f"Invalid project.optional-dependencies in {pyproject}")
            optional_dependencies: list[tuple[str, tuple[str, ...]]] = []
            seen_extras: set[str] = set()
            for extra, values in raw_optional.items():
                if (
                    not isinstance(extra, str)
                    or not isinstance(values, list)
                    or not all(isinstance(value, str) for value in values)
                ):
                    raise ValidationError(f"Invalid project.optional-dependencies in {pyproject}")
                normalized_extra = normalize_extra(extra)
                if not normalized_extra or normalized_extra in seen_extras:
                    raise ValidationError(f"Duplicate or invalid extra {extra!r} in {pyproject}")
                seen_extras.add(normalized_extra)
                optional_dependencies.append((extra, tuple(values)))
            optional_dependencies.sort(key=lambda item: normalize_extra(item[0]))
            entry_points = project_entry_points(project, source=str(pyproject))
        except (KeyError, OSError, tomllib.TOMLDecodeError) as error:
            raise ValidationError(
                f"Could not read release metadata from {pyproject}: {error}"
            ) from error
        if version != expected_version:
            raise ValidationError(
                f"Release tag version {expected_version!r} disagrees with {pyproject}: {version!r}"
            )
        specs[key] = PackageSpec(
            key=key,
            name=name,
            distribution=normalized_distribution(name),
            version=version,
            requires_python=requires_python,
            dependencies=dependencies,
            optional_dependencies=tuple(optional_dependencies),
            entry_points=entry_points,
            pure=pure,
            wheel_members=wheel_members,
            sdist_members=sdist_members,
        )
    return specs


def direct_files(directory: Path, *, label: str) -> tuple[Path, ...]:
    """Return an exact flat artifact directory and reject nested/non-file entries."""
    if not directory.is_dir():
        raise ValidationError(f"{label} directory does not exist: {directory}")
    entries = sorted(directory.iterdir(), key=lambda path: path.name)
    invalid = [path.name for path in entries if not path.is_file() or path.is_symlink()]
    if invalid:
        raise ValidationError(f"{label} directory contains non-files: {invalid}")
    return tuple(entries)


def parse_wheel_filename(path: Path) -> WheelFilename:
    """Parse the build-tag-free wheel names produced by this repository."""
    if path.suffix != ".whl":
        raise ValidationError(f"Expected a wheel, got {path.name}")
    parts = path.name.removesuffix(".whl").split("-")
    if len(parts) != 5:
        raise ValidationError(f"Wheel filename must have five components: {path.name}")
    return WheelFilename(*parts)


def parse_core_platform(platform_tag: str) -> tuple[str, tuple[int, int] | None]:
    """Map every compressed platform token and return its strict Linux policy."""
    legacy_manylinux = {
        "manylinux1": ((2, 5), frozenset({"x86_64"})),
        "manylinux2010": ((2, 12), frozenset({"x86_64"})),
        "manylinux2014": ((2, 17), frozenset({"x86_64", "aarch64"})),
    }
    buckets: list[str] = []
    linux_policies: list[tuple[int, int]] = []
    tags = platform_tag.split(".")
    if not tags or any(not tag for tag in tags):
        raise ValidationError(f"Invalid compressed core platform tag: {platform_tag!r}")
    for tag in tags:
        if tag.startswith(("linux_", "musllinux_")):
            raise ValidationError(f"GNU Linux core wheels must use manylinux tags, got {tag!r}")
        pep600 = re.fullmatch(r"manylinux_(\d+)_(\d+)_(x86_64|aarch64)", tag)
        legacy = re.fullmatch(r"(manylinux(?:1|2010|2014))_(x86_64|aarch64)", tag)
        macos = re.fullmatch(r"macosx_(\d+)_(\d+)_(x86_64|arm64)", tag)
        if pep600 is not None:
            major, minor, architecture = pep600.groups()
            policy_version = (int(major), int(minor))
            if policy_version[0] != 2:
                raise ValidationError(f"Unsupported manylinux policy in {platform_tag!r}: {tag}")
            buckets.append(f"linux-{architecture}")
            linux_policies.append(policy_version)
        elif legacy is not None:
            policy_name, architecture = legacy.groups()
            policy_version, architectures = legacy_manylinux[policy_name]
            if architecture not in architectures:
                raise ValidationError(f"Invalid legacy manylinux tag in {platform_tag!r}: {tag}")
            buckets.append(f"linux-{architecture}")
            linux_policies.append(policy_version)
        elif macos is not None:
            architecture = macos.group(3)
            buckets.append(f"macos-{architecture}")
        elif tag == "win_amd64":
            buckets.append("windows-x86_64")
        else:
            raise ValidationError(f"Unknown core wheel platform tag in {platform_tag!r}: {tag}")
    if len(set(buckets)) != 1:
        raise ValidationError(
            f"Compressed core platform tag mixes release buckets: {platform_tag!r} -> {buckets}"
        )
    bucket = buckets[0]
    if bucket not in CORE_WHEEL_BUCKETS:
        raise ValidationError(f"Unsupported core wheel platform bucket: {bucket}")
    if bucket.startswith("linux-"):
        if len(linux_policies) != len(tags):
            raise ValidationError(
                f"Compressed Linux platform tag mixes policy families: {platform_tag}"
            )
        return bucket, min(linux_policies)
    if linux_policies:
        raise ValidationError(f"Compressed platform tag mixes Linux and non-Linux: {platform_tag}")
    return bucket, None


def safe_archive_name(raw_name: str, *, archive: Path) -> str:
    """Validate one portable, relative archive path."""
    path = PurePosixPath(raw_name)
    if (
        not raw_name
        or "\0" in raw_name
        or "\\" in raw_name
        or path.is_absolute()
        or re.match(r"^[A-Za-z]:", raw_name)
        or any(part in {"", ".", ".."} for part in raw_name.split("/"))
    ):
        raise ValidationError(f"Unsafe member path in {archive.name}: {raw_name!r}")
    return path.as_posix()


def safe_sdist_symlink_target(
    member_name: str,
    link_name: str,
    *,
    archive: Path,
) -> str:
    """Resolve one relative sdist symlink without allowing root traversal."""
    link = PurePosixPath(link_name)
    if (
        not link_name
        or "\0" in link_name
        or "\\" in link_name
        or link.is_absolute()
        or re.match(r"^[A-Za-z]:", link_name)
    ):
        raise ValidationError(
            f"Unsafe symbolic link in {archive.name}: {member_name!r} -> {link_name!r}"
        )
    resolved = list(PurePosixPath(member_name).parent.parts)
    for part in link.parts:
        if part in {"", "."}:
            continue
        if part == "..":
            if len(resolved) <= 1:
                raise ValidationError(
                    f"Symbolic link escapes sdist root in {archive.name}: "
                    f"{member_name!r} -> {link_name!r}"
                )
            resolved.pop()
        else:
            resolved.append(part)
    return PurePosixPath(*resolved).as_posix()


def parse_metadata(
    payload: bytes,
    *,
    source: str,
    required_fields: tuple[str, ...] = (
        "Metadata-Version",
        "Name",
        "Version",
        "Requires-Python",
    ),
) -> Message:
    """Parse RFC-compliant metadata and require the selected fields."""
    try:
        message = BytesParser(policy=policy.default).parsebytes(payload)
    except Exception as error:  # email defects vary by malformed input
        raise ValidationError(f"Could not parse metadata from {source}: {error}") from error
    if message.defects:
        raise ValidationError(f"Malformed metadata in {source}: {message.defects}")
    for field in required_fields:
        if not message.get(field):
            raise ValidationError(f"Metadata in {source} is missing {field}")
    return message


def validate_metadata(message: Message, spec: PackageSpec, *, source: str) -> None:
    """Match archive metadata to the repository package contract."""
    for field in ("Metadata-Version", "Name", "Version", "Requires-Python"):
        if len(message.get_all(field, [])) != 1:
            raise ValidationError(f"{source} must declare {field} exactly once")
    if normalize_name(str(message["Name"])) != normalize_name(spec.name):
        raise ValidationError(f"{source} names {message['Name']!r}, expected {spec.name!r}")
    if str(message["Version"]) != spec.version:
        raise ValidationError(
            f"{source} has version {message['Version']!r}, expected {spec.version!r}"
        )
    actual_python = normalize_requires_python(str(message["Requires-Python"]))
    expected_python = normalize_requires_python(spec.requires_python)
    if actual_python != expected_python:
        raise ValidationError(
            f"{source} has Requires-Python {message['Requires-Python']!r}, "
            f"expected {spec.requires_python!r}"
        )
    expected_extras, expected_dependencies = dependency_metadata(spec)
    actual_extras = tuple(
        sorted(normalize_extra(str(value)) for value in message.get_all("Provides-Extra", []))
    )
    if actual_extras != expected_extras:
        raise ValidationError(
            f"{source} Provides-Extra metadata disagrees: "
            f"actual={actual_extras}, expected={expected_extras}"
        )
    actual_dependencies = tuple(
        sorted(normalize_requirement(str(value)) for value in message.get_all("Requires-Dist", []))
    )
    if actual_dependencies != expected_dependencies:
        raise ValidationError(
            f"{source} Requires-Dist metadata disagrees: "
            f"actual={actual_dependencies}, expected={expected_dependencies}"
        )


def validate_wheel_scripts(
    archive: zipfile.ZipFile,
    infos: dict[str, zipfile.ZipInfo],
    dist_info: str,
    spec: PackageSpec,
    *,
    path: Path,
) -> None:
    """Match every wheel entry-point group exactly to repository PEP 621 metadata."""
    entry_points_name = f"{dist_info}/entry_points.txt"
    expected = spec.entry_points
    if entry_points_name not in infos:
        if expected:
            raise ValidationError(f"Wheel {path.name} is missing {entry_points_name}")
        return
    try:
        payload = archive.read(entry_points_name).decode("utf-8")
        parser = configparser.ConfigParser(
            interpolation=None,
            strict=True,
            empty_lines_in_values=False,
        )
        parser.optionxform = lambda optionstr: optionstr
        parser.read_string(payload)
    except (UnicodeDecodeError, configparser.Error) as error:
        raise ValidationError(f"Invalid entry_points.txt in {path.name}: {error}") from error
    if parser.defaults():
        raise ValidationError(f"Wheel {path.name} entry_points.txt has an invalid DEFAULT group")
    actual = tuple(
        sorted((section, tuple(sorted(parser.items(section)))) for section in parser.sections())
    )
    if actual != expected:
        raise ValidationError(
            f"Wheel {path.name} entry points disagree: actual={actual}, expected={expected}"
        )


def sha256_path(path: Path) -> str:
    """Hash a release artifact without loading it all into memory."""
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def zip_member_sha256(archive: zipfile.ZipFile, info: zipfile.ZipInfo) -> tuple[str, int]:
    """Return a wheel member's URL-safe RECORD digest and streamed size."""
    digest = hashlib.sha256()
    size = 0
    with archive.open(info) as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
            size += len(chunk)
    encoded = base64.urlsafe_b64encode(digest.digest()).rstrip(b"=").decode("ascii")
    return encoded, size


def validate_native_binary(
    archive: zipfile.ZipFile,
    info: zipfile.ZipInfo,
    bucket: str,
    *,
    path: Path,
) -> None:
    """Match an extension's executable format and CPU to its wheel platform."""
    with archive.open(info) as stream:
        header = stream.read(min(info.file_size, 4096))

    valid = False
    if bucket.startswith("linux-") and len(header) >= 20:
        expected_machine = 62 if bucket == "linux-x86_64" else 183
        valid = (
            header[:4] == b"\x7fELF"
            and header[4] == 2  # ELFCLASS64
            and header[5] == 1  # little endian
            and int.from_bytes(header[18:20], "little") == expected_machine
        )
    elif bucket.startswith("macos-") and len(header) >= 8:
        expected_cpu = 0x01000007 if bucket == "macos-x86_64" else 0x0100000C
        valid = (
            header[:4] == b"\xcf\xfa\xed\xfe"
            and int.from_bytes(header[4:8], "little") == expected_cpu
        )
    elif bucket == "windows-x86_64" and len(header) >= 64 and header[:2] == b"MZ":
        pe_offset = int.from_bytes(header[60:64], "little")
        required = pe_offset + 6
        if required <= min(info.file_size, 1024 * 1024):
            if required > len(header):
                with archive.open(info) as stream:
                    header = stream.read(required)
            valid = (
                len(header) >= required
                and header[pe_offset : pe_offset + 4] == b"PE\0\0"
                and int.from_bytes(header[pe_offset + 4 : required], "little") == 0x8664
            )
    if not valid:
        raise ValidationError(
            f"Native extension binary header does not match {bucket} in {path.name}: "
            f"{info.filename}"
        )


def validate_manylinux_elf_policy(
    archive: zipfile.ZipFile,
    info: zipfile.ZipInfo,
    policy_version: tuple[int, int],
    *,
    path: Path,
) -> None:
    """Reject ELF imports newer than the wheel's declared glibc compatibility floor."""
    required_versions: set[tuple[int, int]] = set()
    carry = b""
    with archive.open(info) as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            searchable = carry + chunk
            required_versions.update(
                (int(match.group(1)), int(match.group(2)))
                for match in re.finditer(rb"GLIBC_(\d+)\.(\d+)", searchable)
            )
            carry = searchable[-64:]
    incompatible = sorted(version for version in required_versions if version > policy_version)
    if incompatible:
        raise ValidationError(
            f"ELF symbols in {info.filename} exceed the manylinux_"
            f"{policy_version[0]}_{policy_version[1]} policy in {path.name}: {incompatible}"
        )


def validate_python_init_symbol(
    archive: zipfile.ZipFile,
    info: zipfile.ZipInfo,
    *,
    path: Path,
) -> None:
    """Require the native module's CPython initialization symbol."""
    expected = b"PyInit_type_bridge_core"
    carry = b""
    with archive.open(info) as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            searchable = carry + chunk
            if expected in searchable:
                return
            carry = searchable[-len(expected) :]
    raise ValidationError(
        f"Native extension lacks {expected.decode()} in {path.name}: {info.filename}"
    )


def validate_wheel_record(
    archive: zipfile.ZipFile,
    infos: dict[str, zipfile.ZipInfo],
    record_name: str,
    *,
    path: Path,
) -> None:
    """Require RECORD to enumerate and hash every regular wheel member exactly."""
    try:
        rows = list(csv.reader(io.StringIO(archive.read(record_name).decode("utf-8"))))
    except (UnicodeDecodeError, csv.Error, KeyError) as error:
        raise ValidationError(f"Invalid RECORD in {path.name}: {error}") from error
    records: dict[str, tuple[str, str]] = {}
    for row in rows:
        if len(row) != 3:
            raise ValidationError(f"Invalid RECORD row in {path.name}: {row!r}")
        name = safe_archive_name(row[0], archive=path)
        if name in records:
            raise ValidationError(f"Duplicate RECORD entry in {path.name}: {name}")
        records[name] = (row[1], row[2])
    if records.keys() != infos.keys():
        missing = sorted(infos.keys() - records.keys())
        extra = sorted(records.keys() - infos.keys())
        raise ValidationError(
            f"RECORD inventory mismatch in {path.name}: missing={missing}, extra={extra}"
        )
    for name, info in infos.items():
        hash_field, size_field = records[name]
        if name == record_name:
            if hash_field or size_field:
                raise ValidationError(f"RECORD must leave its own hash/size empty in {path.name}")
            continue
        if not hash_field.startswith("sha256=") or not size_field.isdecimal():
            raise ValidationError(f"RECORD lacks sha256/size for {name} in {path.name}")
        actual_hash, actual_size = zip_member_sha256(archive, info)
        if hash_field.removeprefix("sha256=") != actual_hash or int(size_field) != actual_size:
            raise ValidationError(f"RECORD digest/size mismatch for {name} in {path.name}")


def validate_wheel(path: Path, spec: PackageSpec) -> dict[str, Any]:
    """Validate one wheel's archive, metadata, tags, package files, and RECORD."""
    filename = parse_wheel_filename(path)
    if filename.distribution != spec.distribution or filename.version != spec.version:
        raise ValidationError(
            f"Wheel identity {filename.distribution}-{filename.version} disagrees with "
            f"expected {spec.distribution}-{spec.version}: {path.name}"
        )
    if spec.pure:
        if (filename.python_tag, filename.abi_tag, filename.platform_tag) != (
            "py3",
            "none",
            "any",
        ):
            raise ValidationError(f"Root wheel is not universal py3-none-any: {path.name}")
        bucket = "universal"
        manylinux_policy = None
    else:
        if filename.python_tag != "cp312" or filename.abi_tag != "abi3":
            raise ValidationError(f"Core wheel must use the exact cp312-abi3 tags: {path.name}")
        bucket, manylinux_policy = parse_core_platform(filename.platform_tag)

    try:
        archive = zipfile.ZipFile(path)
    except (OSError, zipfile.BadZipFile) as error:
        raise ValidationError(f"Unreadable wheel {path}: {error}") from error
    with archive:
        corrupt_member = archive.testzip()
        if corrupt_member is not None:
            raise ValidationError(f"Wheel CRC failure in {path.name}: {corrupt_member}")
        infos: dict[str, zipfile.ZipInfo] = {}
        for info in archive.infolist():
            name = safe_archive_name(info.filename.rstrip("/"), archive=path)
            if name in infos:
                raise ValidationError(f"Duplicate wheel member in {path.name}: {name}")
            if info.flag_bits & 0x1:
                raise ValidationError(f"Encrypted wheel member in {path.name}: {name}")
            mode = info.external_attr >> 16
            if mode and stat.S_ISLNK(mode):
                raise ValidationError(f"Symbolic link in wheel {path.name}: {name}")
            if info.is_dir():
                continue
            if "__pycache__" in PurePosixPath(name).parts or name.endswith((".pyc", ".pyo")):
                raise ValidationError(f"Generated bytecode leaked into wheel {path.name}: {name}")
            infos[name] = info

        dist_info = f"{spec.distribution}-{spec.version}.dist-info"
        metadata_name = f"{dist_info}/METADATA"
        wheel_name = f"{dist_info}/WHEEL"
        record_name = f"{dist_info}/RECORD"
        for required in (metadata_name, wheel_name, record_name):
            if required not in infos:
                raise ValidationError(f"Wheel {path.name} is missing {required}")

        package_prefix = "type_bridge/" if spec.key == "root" else "type_bridge_core/"
        outside_install_payload = sorted(
            name
            for name in infos
            if not name.startswith(package_prefix) and not name.startswith(f"{dist_info}/")
        )
        if outside_install_payload:
            raise ValidationError(
                f"Wheel {path.name} contains unexpected install payload: {outside_install_payload}"
            )

        metadata = parse_metadata(
            archive.read(metadata_name), source=f"{path.name}:{metadata_name}"
        )
        validate_metadata(metadata, spec, source=f"{path.name}:{metadata_name}")
        validate_wheel_scripts(archive, infos, dist_info, spec, path=path)
        wheel_metadata = parse_metadata(
            archive.read(wheel_name),
            source=f"{path.name}:{wheel_name}",
            required_fields=("Wheel-Version", "Root-Is-Purelib", "Tag"),
        )
        for field in ("Wheel-Version", "Root-Is-Purelib"):
            if len(wheel_metadata.get_all(field, [])) != 1:
                raise ValidationError(f"Wheel {path.name} must declare {field} exactly once")
        if str(wheel_metadata.get("Wheel-Version")) != "1.0":
            raise ValidationError(f"Wheel {path.name} does not declare Wheel-Version 1.0")
        raw_pure = str(wheel_metadata.get("Root-Is-Purelib", "")).lower()
        if raw_pure not in {"true", "false"}:
            raise ValidationError(
                f"Wheel {path.name} has invalid Root-Is-Purelib value: {raw_pure!r}"
            )
        pure = raw_pure == "true"
        if pure != spec.pure:
            raise ValidationError(f"Wheel {path.name} Root-Is-Purelib disagrees with package")
        metadata_tags = frozenset(str(value) for value in wheel_metadata.get_all("Tag", []))
        if metadata_tags != filename.tags:
            raise ValidationError(
                f"Wheel tag metadata differs from filename in {path.name}: "
                f"metadata={sorted(metadata_tags)}, filename={sorted(filename.tags)}"
            )

        native_extensions = [name for name in infos if name.endswith((".so", ".pyd"))]
        if spec.key == "core":
            expected_native = (
                "type_bridge_core/type_bridge_core.pyd"
                if bucket == "windows-x86_64"
                else "type_bridge_core/type_bridge_core.abi3.so"
            )
            if native_extensions != [expected_native]:
                raise ValidationError(
                    f"Core wheel {path.name} must contain exactly {expected_native}: "
                    f"{native_extensions}"
                )
            expected_package_members = spec.wheel_members | {expected_native}
            validate_native_binary(archive, infos[expected_native], bucket, path=path)
            validate_python_init_symbol(archive, infos[expected_native], path=path)
            if manylinux_policy is not None:
                validate_manylinux_elf_policy(
                    archive,
                    infos[expected_native],
                    manylinux_policy,
                    path=path,
                )
        elif native_extensions:
            raise ValidationError(
                f"Pure root wheel unexpectedly contains native extensions: {native_extensions}"
            )
        else:
            expected_package_members = spec.wheel_members
        actual_package_members = frozenset(
            name for name in infos if name.startswith(package_prefix)
        )
        if actual_package_members != expected_package_members:
            missing_package = sorted(expected_package_members - actual_package_members)
            extra_package = sorted(actual_package_members - expected_package_members)
            raise ValidationError(
                f"Wheel {path.name} package inventory disagrees: "
                f"missing={missing_package}, extra={extra_package}"
            )
        validate_wheel_record(archive, infos, record_name, path=path)

    return {
        "bucket": bucket,
        "filename": path.name,
        "kind": "wheel",
        "package": spec.name,
        "sha256": sha256_path(path),
        "size": path.stat().st_size,
    }


def validate_sdist(path: Path, spec: PackageSpec) -> dict[str, Any]:
    """Validate one gzipped source distribution and its embedded metadata."""
    expected_name = f"{spec.distribution}-{spec.version}.tar.gz"
    if path.name != expected_name:
        raise ValidationError(f"Expected sdist {expected_name}, got {path.name}")
    archive_root = path.name.removesuffix(".tar.gz")
    try:
        archive = tarfile.open(path, mode="r:gz")
    except (OSError, tarfile.TarError) as error:
        raise ValidationError(f"Unreadable sdist {path}: {error}") from error
    with archive:
        members: dict[str, tarfile.TarInfo] = {}
        symlink_targets: dict[str, str] = {}
        for member in archive.getmembers():
            name = safe_archive_name(member.name.rstrip("/"), archive=path)
            if PurePosixPath(name).parts[0] != archive_root:
                raise ValidationError(f"Sdist member escapes canonical root in {path.name}: {name}")
            relative = PurePosixPath(*PurePosixPath(name).parts[1:]).as_posix()
            if relative in members:
                raise ValidationError(f"Duplicate sdist member in {path.name}: {name}")
            if member.issym():
                target = safe_sdist_symlink_target(
                    name,
                    member.linkname,
                    archive=path,
                )
                symlink_targets[relative] = PurePosixPath(
                    *PurePosixPath(target).parts[1:]
                ).as_posix()
            elif not (member.isdir() or member.isfile()):
                raise ValidationError(f"Non-regular member in sdist {path.name}: {name}")
            if "__pycache__" in PurePosixPath(relative).parts or relative.endswith(
                (".pyc", ".pyo")
            ):
                raise ValidationError(
                    f"Generated bytecode leaked into sdist {path.name}: {relative}"
                )
            members[relative] = member

        for name, target in symlink_targets.items():
            target_member = members.get(target)
            if target_member is None or not target_member.isfile():
                raise ValidationError(
                    f"Symbolic link target is missing or non-regular in {path.name}: "
                    f"{name!r} -> {target!r}"
                )

        missing = sorted(spec.sdist_members - members.keys())
        if missing:
            raise ValidationError(f"Sdist {path.name} is missing release sources: {missing}")
        non_regular_required = sorted(
            name for name in spec.sdist_members if not members[name].isfile()
        )
        if non_regular_required:
            raise ValidationError(
                f"Sdist {path.name} has non-regular required sources: {non_regular_required}"
            )
        if spec.key == "root":
            package_prefix = "type_bridge/"
            expected_package_members = spec.wheel_members
        else:
            package_prefix = "python/type_bridge_core/"
            expected_package_members = frozenset(f"python/{name}" for name in spec.wheel_members)
        actual_package_members = frozenset(
            name
            for name, member in members.items()
            if name.startswith(package_prefix) and not member.isdir()
        )
        if actual_package_members != expected_package_members:
            missing_package = sorted(expected_package_members - actual_package_members)
            extra_package = sorted(actual_package_members - expected_package_members)
            raise ValidationError(
                f"Sdist {path.name} package inventory disagrees: "
                f"missing={missing_package}, extra={extra_package}"
            )
        package_info = members["PKG-INFO"]
        stream = archive.extractfile(package_info)
        if stream is None:
            raise ValidationError(f"Could not read PKG-INFO from {path.name}")
        metadata = parse_metadata(stream.read(), source=f"{path.name}:PKG-INFO")
        validate_metadata(metadata, spec, source=f"{path.name}:PKG-INFO")

        pyproject_stream = archive.extractfile(members["pyproject.toml"])
        if pyproject_stream is None:
            raise ValidationError(f"Could not read pyproject.toml from {path.name}")
        try:
            project = tomllib.loads(pyproject_stream.read().decode("utf-8"))["project"]
        except (KeyError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
            raise ValidationError(f"Invalid pyproject.toml in {path.name}: {error}") from error
        if normalize_name(str(project.get("name", ""))) != normalize_name(spec.name):
            raise ValidationError(f"Sdist pyproject name disagrees in {path.name}")
        if str(project.get("version", "")) != spec.version:
            raise ValidationError(f"Sdist pyproject version disagrees in {path.name}")
        actual_python = normalize_requires_python(str(project.get("requires-python", "")))
        if actual_python != normalize_requires_python(spec.requires_python):
            raise ValidationError(f"Sdist pyproject Requires-Python disagrees in {path.name}")
        project_dependencies = project.get("dependencies", [])
        if not isinstance(project_dependencies, list) or not all(
            isinstance(value, str) for value in project_dependencies
        ):
            raise ValidationError(f"Sdist pyproject dependencies are invalid in {path.name}")
        actual_dependencies = tuple(
            sorted(normalize_requirement(value) for value in project_dependencies)
        )
        expected_dependencies = tuple(sorted(map(normalize_requirement, spec.dependencies)))
        if actual_dependencies != expected_dependencies:
            raise ValidationError(f"Sdist pyproject dependencies disagree in {path.name}")
        project_optional = project.get("optional-dependencies", {})
        if not isinstance(project_optional, dict):
            raise ValidationError(
                f"Sdist pyproject optional-dependencies are invalid in {path.name}"
            )
        actual_optional: dict[str, tuple[str, ...]] = {}
        for extra, dependencies in project_optional.items():
            if (
                not isinstance(extra, str)
                or not isinstance(dependencies, list)
                or not all(isinstance(value, str) for value in dependencies)
            ):
                raise ValidationError(
                    f"Sdist pyproject optional-dependencies are invalid in {path.name}"
                )
            normalized_extra = normalize_extra(extra)
            if normalized_extra in actual_optional:
                raise ValidationError(
                    f"Sdist pyproject has duplicate normalized extra in {path.name}: {extra}"
                )
            actual_optional[normalized_extra] = tuple(
                sorted(normalize_requirement(value) for value in dependencies)
            )
        expected_optional = {
            normalize_extra(extra): tuple(
                sorted(normalize_requirement(value) for value in dependencies)
            )
            for extra, dependencies in spec.optional_dependencies
        }
        if actual_optional != expected_optional:
            raise ValidationError(f"Sdist pyproject optional-dependencies disagree in {path.name}")
        actual_entry_points = project_entry_points(
            project,
            source=f"{path.name}:pyproject.toml",
        )
        if actual_entry_points != spec.entry_points:
            raise ValidationError(
                f"Sdist pyproject entry points disagree in {path.name}: "
                f"actual={actual_entry_points}, expected={spec.entry_points}"
            )

    return {
        "bucket": "source",
        "filename": path.name,
        "kind": "sdist",
        "package": spec.name,
        "sha256": sha256_path(path),
        "size": path.stat().st_size,
    }


def require_suffixes(files: Iterable[Path], suffix: str, *, label: str) -> tuple[Path, ...]:
    """Reject non-distribution extras in an artifact download directory."""
    selected = tuple(path for path in files if path.name.endswith(suffix))
    extras = sorted(path.name for path in files if not path.name.endswith(suffix))
    if extras:
        raise ValidationError(f"{label} contains unexpected files: {extras}")
    return selected


def validate_release_set(
    *,
    core_wheels_dir: Path,
    core_sdist_dir: Path,
    root_dist_dir: Path,
    expected_version: str,
    repository_root: Path = REPOSITORY_ROOT,
) -> dict[str, Any]:
    """Validate exactly five core wheels, two sdists, and one root wheel."""
    specs = load_package_specs(repository_root, expected_version)
    core_wheels = require_suffixes(
        direct_files(core_wheels_dir, label="core wheels"), ".whl", label="core wheels"
    )
    core_sdists = require_suffixes(
        direct_files(core_sdist_dir, label="core sdist"), ".tar.gz", label="core sdist"
    )
    root_files = direct_files(root_dist_dir, label="root distributions")
    root_wheels = tuple(path for path in root_files if path.name.endswith(".whl"))
    root_sdists = tuple(path for path in root_files if path.name.endswith(".tar.gz"))
    root_extras = sorted(
        path.name for path in root_files if not path.name.endswith((".whl", ".tar.gz"))
    )
    if root_extras:
        raise ValidationError(f"Root distributions contain unexpected files: {root_extras}")
    if len(core_wheels) != 5 or len(core_sdists) != 1:
        raise ValidationError(
            f"Expected five core wheels and one core sdist; found "
            f"{len(core_wheels)} wheels and {len(core_sdists)} sdists"
        )
    if len(root_wheels) != 1 or len(root_sdists) != 1:
        raise ValidationError(
            f"Expected one root wheel and one root sdist; found "
            f"{len(root_wheels)} wheels and {len(root_sdists)} sdists"
        )

    reports = [validate_wheel(path, specs["core"]) for path in core_wheels]
    buckets = [str(report["bucket"]) for report in reports]
    if len(set(buckets)) != len(buckets) or frozenset(buckets) != CORE_WHEEL_BUCKETS:
        raise ValidationError(
            f"Core wheel platform matrix mismatch: expected={sorted(CORE_WHEEL_BUCKETS)}, "
            f"actual={sorted(buckets)}"
        )
    reports.append(validate_sdist(core_sdists[0], specs["core"]))
    reports.append(validate_wheel(root_wheels[0], specs["root"]))
    reports.append(validate_sdist(root_sdists[0], specs["root"]))
    return {
        "artifacts": sorted(reports, key=lambda report: str(report["filename"])),
        "expected_version": expected_version,
        "status": "ok",
    }


def build_parser() -> argparse.ArgumentParser:
    """Build the exact-artifact release validator CLI."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--core-wheels-dir", type=Path, required=True)
    parser.add_argument("--core-sdist-dir", type=Path, required=True)
    parser.add_argument("--root-dist-dir", type=Path, required=True)
    parser.add_argument("--expected-version", required=True)
    parser.add_argument("--repository-root", type=Path, default=REPOSITORY_ROOT)
    parser.add_argument("--manifest", type=Path)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Validate the release set and optionally persist its hash manifest."""
    args = build_parser().parse_args(argv)
    report = validate_release_set(
        core_wheels_dir=args.core_wheels_dir.resolve(),
        core_sdist_dir=args.core_sdist_dir.resolve(),
        root_dist_dir=args.root_dist_dir.resolve(),
        expected_version=args.expected_version,
        repository_root=args.repository_root.resolve(),
    )
    payload = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.manifest is not None:
        manifest = args.manifest.resolve()
        manifest.parent.mkdir(parents=True, exist_ok=True)
        manifest.write_text(payload, encoding="utf-8")
    print(payload, end="")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ValidationError as error:
        print(f"Python release artifact validation failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
