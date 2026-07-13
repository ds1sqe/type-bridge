#!/usr/bin/env python3
"""Audit every validated GNU wheel against its declared manylinux policy."""

from __future__ import annotations

import argparse
import hashlib
import importlib
import json
import re
import sys
from collections.abc import Sequence
from dataclasses import dataclass
from importlib import metadata as importlib_metadata
from pathlib import Path
from typing import Any

AUDITWHEEL_VERSION = "6.7.0"
LINUX_BUCKETS = frozenset({"linux-x86_64", "linux-aarch64"})
CORE_BUCKETS = LINUX_BUCKETS | frozenset({"macos-x86_64", "macos-arm64", "windows-x86_64"})
LEGACY_POLICIES = {
    "manylinux1": (2, 5),
    "manylinux2010": (2, 12),
    "manylinux2014": (2, 17),
}


class AuditError(RuntimeError):
    """The complete GNU wheel set is missing or violates its claimed policy."""


@dataclass(frozen=True, slots=True)
class AuditwheelResult:
    """Machine-checkable policy facts returned by auditwheel 6.7.0."""

    actual_policy: str
    blacklisted_symbols: tuple[str, ...]
    declared_policy: str
    external_libraries: tuple[str, ...]


def sha256_path(path: Path) -> str:
    """Hash one candidate without loading it into memory."""
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def read_manifest(path: Path) -> dict[str, Any]:
    """Read the exact artifact validator's successful JSON manifest."""
    if not path.is_file() or path.is_symlink():
        raise AuditError(f"Python artifact manifest is missing or unsafe: {path}")
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise AuditError(f"Could not read Python artifact manifest {path}: {error}") from error
    if not isinstance(payload, dict) or payload.get("status") != "ok":
        raise AuditError(f"Python artifact manifest is not successful: {path}")
    artifacts = payload.get("artifacts")
    if not isinstance(artifacts, list):
        raise AuditError(f"Python artifact manifest has no artifact list: {path}")
    return payload


def validated_linux_candidates(
    manifest: dict[str, Any],
    wheel_directory: Path,
) -> dict[str, Path]:
    """Bind both GNU candidates to the validator's filenames and SHA-256s."""
    if not wheel_directory.is_dir() or wheel_directory.is_symlink():
        raise AuditError(f"Core wheel directory is missing or unsafe: {wheel_directory}")
    entries = sorted(wheel_directory.iterdir(), key=lambda candidate: candidate.name)
    if any(not entry.is_file() or entry.is_symlink() for entry in entries):
        raise AuditError(f"Core wheel directory contains a non-regular entry: {wheel_directory}")

    artifacts = manifest["artifacts"]
    core_wheels: list[dict[str, Any]] = []
    for artifact in artifacts:
        if not isinstance(artifact, dict):
            raise AuditError("Python artifact manifest contains a non-object artifact")
        if artifact.get("package") == "type-bridge-core" and artifact.get("kind") == "wheel":
            core_wheels.append(artifact)
    if len(core_wheels) != 5:
        raise AuditError(f"Manifest must contain five core wheels, found {len(core_wheels)}")
    buckets = [artifact.get("bucket") for artifact in core_wheels]
    if (
        not all(isinstance(bucket, str) for bucket in buckets)
        or len(set(buckets)) != len(buckets)
        or set(buckets) != CORE_BUCKETS
    ):
        raise AuditError(
            f"Manifest core wheel matrix mismatch: expected={sorted(CORE_BUCKETS)}, "
            f"actual={sorted(map(str, buckets))}"
        )
    raw_manifest_filenames = [artifact.get("filename") for artifact in core_wheels]
    if not all(isinstance(filename, str) and filename for filename in raw_manifest_filenames):
        raise AuditError("Manifest contains an invalid core wheel filename")
    manifest_filenames: set[str] = {str(filename) for filename in raw_manifest_filenames}
    if len(manifest_filenames) != len(core_wheels):
        raise AuditError("Manifest repeats a core wheel filename")
    directory_filenames = {entry.name for entry in entries}
    if directory_filenames != manifest_filenames:
        raise AuditError(
            "Core wheel directory disagrees with validated manifest: "
            f"directory={sorted(directory_filenames)}, manifest={sorted(manifest_filenames)}"
        )

    candidates: dict[str, Path] = {}
    for artifact in core_wheels:
        bucket = artifact.get("bucket")
        if bucket not in LINUX_BUCKETS:
            continue
        filename = artifact["filename"]
        expected_digest = artifact.get("sha256")
        if (
            not isinstance(filename, str)
            or Path(filename).name != filename
            or not isinstance(expected_digest, str)
            or re.fullmatch(r"[0-9a-f]{64}", expected_digest) is None
        ):
            raise AuditError(f"Manifest has invalid GNU wheel identity: {artifact!r}")
        candidate = wheel_directory / filename
        actual_digest = sha256_path(candidate)
        if actual_digest != expected_digest:
            raise AuditError(
                f"GNU wheel changed after validation: {filename}; "
                f"manifest={expected_digest}, actual={actual_digest}"
            )
        if bucket in candidates:
            raise AuditError(f"Manifest repeats GNU wheel bucket: {bucket}")
        candidates[bucket] = candidate
    if candidates.keys() != LINUX_BUCKETS:
        raise AuditError(
            f"Manifest GNU wheel matrix mismatch: expected={sorted(LINUX_BUCKETS)}, "
            f"actual={sorted(candidates)}"
        )
    return candidates


def declared_policy(path: Path, bucket: str) -> tuple[str, tuple[int, int]]:
    """Return the oldest compatibility promise in a compressed manylinux tag."""
    parts = path.name.removesuffix(".whl").split("-")
    if len(parts) != 5:
        raise AuditError(f"GNU wheel filename does not have five components: {path.name}")
    architecture = bucket.removeprefix("linux-")
    policies: list[tuple[int, int]] = []
    for tag in parts[4].split("."):
        pep600 = re.fullmatch(rf"manylinux_(\d+)_(\d+)_{re.escape(architecture)}", tag)
        legacy = re.fullmatch(
            rf"(manylinux(?:1|2010|2014))_{re.escape(architecture)}",
            tag,
        )
        if pep600 is not None:
            policies.append((int(pep600.group(1)), int(pep600.group(2))))
        elif legacy is not None:
            policies.append(LEGACY_POLICIES[legacy.group(1)])
        else:
            raise AuditError(f"Invalid manylinux token in validated GNU wheel: {tag!r}")
    floor = min(policies)
    return f"manylinux_{floor[0]}_{floor[1]}_{architecture}", floor


def auditwheel_result(path: Path, policy_name: str) -> AuditwheelResult:
    """Run the pinned auditwheel API with grafting and ISA exceptions disabled."""
    try:
        installed_version = importlib_metadata.version("auditwheel")
        if installed_version != AUDITWHEEL_VERSION:
            raise AuditError(
                f"auditwheel {AUDITWHEEL_VERSION} is required, found {installed_version}"
            )
        wheel_abi = importlib.import_module("auditwheel.wheel_abi")
        wheeltools = importlib.import_module("auditwheel.wheeltools")

        architecture = wheeltools.get_wheel_architecture(path.name)
        libc = wheeltools.get_wheel_libc(path.name)
        wheel_info = wheel_abi.analyze_wheel_abi(
            libc,
            architecture,
            path,
            frozenset(),
            disable_isa_ext_check=False,
            allow_graft=False,
            args_ldpaths=None,
        )
        policy = wheel_info.policies.get_policy_by_name(policy_name)
        external = wheel_info.external_refs[policy.name]
    except AuditError:
        raise
    except Exception as error:
        raise AuditError(f"auditwheel could not analyze {path.name}: {error}") from error
    blacklisted = tuple(
        sorted(
            f"{library}:{symbol}"
            for library, symbols in external.blacklist.items()
            for symbol in symbols
        )
    )
    return AuditwheelResult(
        actual_policy=wheel_info.overall_policy.name,
        blacklisted_symbols=blacklisted,
        declared_policy=policy.name,
        external_libraries=tuple(sorted(external.libs)),
    )


def validate_policy_result(
    result: AuditwheelResult,
    *,
    bucket: str,
    declared_floor: tuple[int, int],
    path: Path,
) -> None:
    """Require auditwheel to prove the candidate satisfies its filename promise."""
    architecture = bucket.removeprefix("linux-")
    match = re.fullmatch(
        rf"manylinux_(\d+)_(\d+)_{re.escape(architecture)}",
        result.actual_policy,
    )
    if match is None:
        raise AuditError(
            f"auditwheel gives {path.name} non-manylinux policy {result.actual_policy!r}"
        )
    actual_floor = (int(match.group(1)), int(match.group(2)))
    if actual_floor > declared_floor:
        raise AuditError(
            f"{path.name} claims manylinux_{declared_floor[0]}_{declared_floor[1]} "
            f"but auditwheel requires {result.actual_policy}"
        )
    if result.external_libraries or result.blacklisted_symbols:
        raise AuditError(
            f"{path.name} violates {result.declared_policy}: "
            f"external={result.external_libraries}, blacklist={result.blacklisted_symbols}"
        )


def audit_release_wheels(
    *,
    manifest_path: Path,
    wheel_directory: Path,
) -> dict[str, Any]:
    """Audit the complete, hash-bound GNU wheel subset."""
    manifest = read_manifest(manifest_path)
    candidates = validated_linux_candidates(manifest, wheel_directory)
    reports: list[dict[str, Any]] = []
    for bucket, path in sorted(candidates.items()):
        policy_name, floor = declared_policy(path, bucket)
        result = auditwheel_result(path, policy_name)
        validate_policy_result(result, bucket=bucket, declared_floor=floor, path=path)
        reports.append(
            {
                "actual_policy": result.actual_policy,
                "bucket": bucket,
                "declared_policy": result.declared_policy,
                "filename": path.name,
                "sha256": sha256_path(path),
            }
        )
    return {"artifacts": reports, "status": "ok"}


def build_parser() -> argparse.ArgumentParser:
    """Build the immutable manylinux audit CLI."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--core-wheels-dir", type=Path, required=True)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Audit both GNU release candidates and print a JSON report."""
    args = build_parser().parse_args(argv)
    report = audit_release_wheels(
        manifest_path=args.manifest.resolve(),
        wheel_directory=args.core_wheels_dir.resolve(),
    )
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except AuditError as error:
        print(f"manylinux release audit failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
