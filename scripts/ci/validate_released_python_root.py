#!/usr/bin/env python3
"""Validate the immutable released Python root used by the reverse-compat gate."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import urllib.error
import urllib.request
import zipfile
from collections.abc import Sequence
from email import policy
from email.parser import BytesParser
from pathlib import Path
from typing import Any

RELEASED_ROOT_VERSION = "1.5.11"
RELEASED_ROOT_FILENAME = f"type_bridge-{RELEASED_ROOT_VERSION}-py3-none-any.whl"
RELEASED_ROOT_SHA256 = "f2e5ac0a59488f18d294295a2d08ab82b57f750d816e485b83273292d37a9d41"
RELEASED_ROOT_SIZE = 286_440
RELEASED_CORE_REQUIREMENT = f"type-bridge-core>={RELEASED_ROOT_VERSION}"
RELEASED_REQUIRES_PYTHON = frozenset({">=3.12", "<3.15"})
PYPI_PROJECT_URL = "https://pypi.org/pypi/type-bridge/json"
MAX_PYPI_RESPONSE_BYTES = 10 * 1024 * 1024


class ValidationError(RuntimeError):
    """The downloaded wheel is not the frozen published compatibility authority."""


def normalize_distribution_name(value: str) -> str:
    """Return the PEP 503 comparison form for one distribution name."""
    return re.sub(r"[-_.]+", "-", value).lower()


def sha256_path(path: Path) -> str:
    """Hash one regular artifact without loading it all into memory."""
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def core_requirements(requirements: Sequence[str]) -> tuple[str, ...]:
    """Return every requirement whose normalized name denotes type-bridge-core."""
    selected: list[str] = []
    for requirement in requirements:
        match = re.match(r"^\s*([A-Za-z0-9][A-Za-z0-9._-]*)", requirement)
        if match is not None and normalize_distribution_name(match.group(1)) == "type-bridge-core":
            selected.append(requirement)
    return tuple(selected)


def fetch_pypi_project() -> dict[str, Any]:
    """Read the authoritative project index with strict transport and byte bounds."""
    request = urllib.request.Request(
        PYPI_PROJECT_URL,
        headers={
            "Accept": "application/json",
            "User-Agent": "ds1sqe/type-bridge released-root compatibility gate",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            if response.geturl() != PYPI_PROJECT_URL:
                raise ValidationError(
                    "PyPI project authority redirected unexpectedly: "
                    f"actual={response.geturl()!r}, expected={PYPI_PROJECT_URL!r}"
                )
            body = response.read(MAX_PYPI_RESPONSE_BYTES + 1)
    except ValidationError:
        raise
    except (OSError, urllib.error.URLError) as error:
        raise ValidationError(f"Could not read PyPI project authority: {error}") from error
    if len(body) > MAX_PYPI_RESPONSE_BYTES:
        raise ValidationError("PyPI project authority exceeded the response byte budget")
    try:
        payload = json.loads(body.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValidationError(f"PyPI project authority returned invalid JSON: {error}") from error
    if not isinstance(payload, dict):
        raise ValidationError("PyPI project authority returned a non-object payload")
    return payload


def validate_pypi_authority(payload: dict[str, Any]) -> dict[str, Any]:
    """Bind the immutable 1.5.11 wheel record without pinning project latestness."""
    info = payload.get("info")
    releases = payload.get("releases")
    if not isinstance(info, dict) or not isinstance(releases, dict):
        raise ValidationError("PyPI project authority omitted info or releases")
    project_latest_version = info.get("version")
    if not isinstance(project_latest_version, str) or not project_latest_version:
        raise ValidationError("PyPI project authority omitted the project latest version")
    if releases.get("1.5.7") not in (None, []):
        raise ValidationError(
            "PyPI now exposes 1.5.7 files; re-evaluate the durable released-root authority"
        )
    release_files = releases.get(RELEASED_ROOT_VERSION)
    if not isinstance(release_files, list):
        raise ValidationError("PyPI project authority omitted the released-root file list")
    matching = [
        item
        for item in release_files
        if isinstance(item, dict) and item.get("filename") == RELEASED_ROOT_FILENAME
    ]
    if len(matching) != 1:
        raise ValidationError(
            "PyPI project authority must expose exactly one frozen released-root wheel"
        )
    wheel = matching[0]
    digests = wheel.get("digests")
    expected_url_suffix = f"/{RELEASED_ROOT_FILENAME}"
    if (
        not isinstance(digests, dict)
        or digests.get("sha256") != RELEASED_ROOT_SHA256
        or wheel.get("size") != RELEASED_ROOT_SIZE
        or wheel.get("packagetype") != "bdist_wheel"
        or wheel.get("python_version") != "py3"
        or wheel.get("yanked") is not False
        or not isinstance(wheel.get("url"), str)
        or not wheel["url"].startswith("https://files.pythonhosted.org/")
        or not wheel["url"].endswith(expected_url_suffix)
    ):
        raise ValidationError(
            "PyPI released-root wheel record disagrees with the frozen filename/hash/size policy"
        )
    return {
        "project_latest_version": project_latest_version,
        "released_root_version": RELEASED_ROOT_VERSION,
        "missing_durable_tag_files": "1.5.7",
        "status": "ok",
        "wheel": RELEASED_ROOT_FILENAME,
    }


def validate_released_root_wheel(
    wheel: Path,
    *,
    expected_filename: str = RELEASED_ROOT_FILENAME,
    expected_sha256: str = RELEASED_ROOT_SHA256,
    expected_size: int = RELEASED_ROOT_SIZE,
    expected_version: str = RELEASED_ROOT_VERSION,
    expected_core_requirement: str = RELEASED_CORE_REQUIREMENT,
) -> dict[str, Any]:
    """Prove one wheel matches the published root bytes and unbounded dependency contract."""
    if wheel.is_symlink() or not wheel.is_file():
        raise ValidationError(f"Released root wheel is missing or unsafe: {wheel}")
    if wheel.name != expected_filename:
        raise ValidationError(
            f"Released root wheel filename drifted: actual={wheel.name!r}, "
            f"expected={expected_filename!r}"
        )
    actual_size = wheel.stat().st_size
    if actual_size != expected_size:
        raise ValidationError(
            "Released root wheel size disagrees with the immutable PyPI file: "
            f"actual={actual_size}, expected={expected_size}"
        )
    actual_sha256 = sha256_path(wheel)
    if actual_sha256 != expected_sha256:
        raise ValidationError(
            "Released root wheel SHA-256 disagrees with the immutable PyPI file: "
            f"actual={actual_sha256!r}, expected={expected_sha256!r}"
        )

    try:
        with zipfile.ZipFile(wheel) as archive:
            if archive.testzip() is not None:
                raise ValidationError("Released root wheel has a corrupt member")
            names = archive.namelist()
            if len(names) != len(set(names)):
                raise ValidationError("Released root wheel repeats an archive member")
            dist_info = f"type_bridge-{expected_version}.dist-info"
            metadata_name = f"{dist_info}/METADATA"
            wheel_name = f"{dist_info}/WHEEL"
            if names.count(metadata_name) != 1 or names.count(wheel_name) != 1:
                raise ValidationError("Released root wheel has an invalid dist-info inventory")
            metadata_bytes = archive.read(metadata_name)
            wheel_bytes = archive.read(wheel_name)
    except (OSError, zipfile.BadZipFile, KeyError) as error:
        raise ValidationError(f"Could not read released root wheel {wheel}: {error}") from error

    metadata = BytesParser(policy=policy.default).parsebytes(metadata_bytes)
    if metadata.defects:
        raise ValidationError(f"Released root METADATA is malformed: {metadata.defects}")
    for field in ("Name", "Version", "Requires-Python"):
        values = list(map(str, metadata.get_all(field, [])))
        if len(values) != 1:
            raise ValidationError(f"Released root METADATA must declare {field} exactly once")
    if normalize_distribution_name(str(metadata["Name"])) != "type-bridge":
        raise ValidationError(f"Released root distribution name drifted: {metadata['Name']!r}")
    if str(metadata["Version"]) != expected_version:
        raise ValidationError(
            "Released root metadata version drifted: "
            f"actual={metadata['Version']!r}, expected={expected_version!r}"
        )
    requires_python = frozenset(
        clause.strip() for clause in str(metadata["Requires-Python"]).split(",")
    )
    if requires_python != RELEASED_REQUIRES_PYTHON:
        raise ValidationError(
            "Released root Python range drifted: "
            f"actual={sorted(requires_python)!r}, "
            f"expected={sorted(RELEASED_REQUIRES_PYTHON)!r}"
        )
    requirements = tuple(map(str, metadata.get_all("Requires-Dist", [])))
    actual_core_requirements = core_requirements(requirements)
    if actual_core_requirements != (expected_core_requirement,):
        raise ValidationError(
            "Released root must retain its one published unbounded core requirement: "
            f"actual={actual_core_requirements!r}, expected={(expected_core_requirement,)!r}"
        )

    wheel_metadata = BytesParser(policy=policy.default).parsebytes(wheel_bytes)
    if wheel_metadata.defects:
        raise ValidationError(
            f"Released root WHEEL metadata is malformed: {wheel_metadata.defects}"
        )
    if list(map(str, wheel_metadata.get_all("Root-Is-Purelib", []))) != ["true"]:
        raise ValidationError("Released root wheel must remain pure Python")
    if list(map(str, wheel_metadata.get_all("Tag", []))) != ["py3-none-any"]:
        raise ValidationError("Released root wheel must retain the py3-none-any tag")

    return {
        "core_requirement": expected_core_requirement,
        "filename": wheel.name,
        "requires_python": sorted(requires_python),
        "sha256": actual_sha256,
        "size": actual_size,
        "status": "ok",
        "version": expected_version,
    }


def build_parser() -> argparse.ArgumentParser:
    """Build the immutable released-root validator CLI."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--wheel", type=Path, required=True)
    parser.add_argument("--verify-pypi-authority", action="store_true")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Validate the downloaded PyPI wheel and print its bound contract."""
    args = build_parser().parse_args(argv)
    report = validate_released_root_wheel(args.wheel.resolve())
    if args.verify_pypi_authority:
        report["pypi_authority"] = validate_pypi_authority(fetch_pypi_project())
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ValidationError as error:
        print(f"Released Python root validation failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
