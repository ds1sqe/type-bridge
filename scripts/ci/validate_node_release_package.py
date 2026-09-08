#!/usr/bin/env python3
"""Validate an npm tarball's identity, MIT license, inventory, sources, and SRI."""

from __future__ import annotations

import argparse
import base64
import hashlib
import hmac
import json
import re
import sys
import tarfile
from collections.abc import Sequence
from pathlib import Path, PurePosixPath
from typing import Any


class ValidationError(RuntimeError):
    """The npm release candidate is unsafe or disagrees with its authority."""


SEMVER_PATTERN = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)"
    r"(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)

# One npm tarball carries every native target that ``typescript/native.ts``
# advertises.  Keeping this inventory exact prevents a package assembled on one
# runner from silently becoming a single-platform release.
EXPECTED_NATIVE_MODULES = frozenset(
    {
        "type_bridge_node.darwin-arm64.node",
        "type_bridge_node.darwin-x64.node",
        "type_bridge_node.linux-arm64-gnu.node",
        "type_bridge_node.linux-x64-gnu.node",
        "type_bridge_node.win32-arm64-msvc.node",
        "type_bridge_node.win32-x64-msvc.node",
    }
)
MIT_LICENSE = "MIT"
README = "README.md"
THIRD_PARTY_NOTICE = "THIRD_PARTY_NOTICES.md"
EXPECTED_PACKAGE_FILES = frozenset(
    {
        "dist",
        "*.node",
        README,
        THIRD_PARTY_NOTICE,
    }
)
HISTORICAL_BAND9_COMPONENT = re.compile(r"(?:^|-)typedb-(?:driver|protocol)-b9(?:-|$)")
REQUIRED_RUNTIME_MEMBERS = frozenset(
    {
        "dist/index.d.ts",
        "dist/index.js",
        "dist/native.d.ts",
        "dist/native.js",
        "dist/owned-bytes.d.ts",
        "dist/owned-bytes.js",
        "dist/public.d.ts",
        "dist/public.js",
        "dist/query-v2-internals.d.ts",
        "dist/query-v2-internals.js",
        "dist/query-v2.d.ts",
        "dist/query-v2.js",
        "dist/query.d.ts",
        "dist/query.js",
        "dist/runtime-handles.d.ts",
        "dist/runtime-handles.js",
        "dist/runtime-projection.d.ts",
        "dist/runtime-projection.js",
        "package.json",
        README,
        THIRD_PARTY_NOTICE,
    }
)
FORBIDDEN_RUNTIME_PREFIXES = (
    "dist/generator/",
    "dist/typed/",
    "dist/typescript/",
)
FORBIDDEN_RUNTIME_MEMBERS = frozenset(
    {
        "dist/attribute.d.ts",
        "dist/attribute.js",
        "dist/codec.d.ts",
        "dist/codec.js",
        "dist/flags.d.ts",
        "dist/flags.js",
        "dist/iid.d.ts",
        "dist/iid.js",
        "dist/manager.d.ts",
        "dist/manager.js",
        "dist/model.d.ts",
        "dist/model.js",
        "dist/parser.d.ts",
        "dist/parser.js",
    }
)
EXPECTED_MAIN = "dist/public.js"
EXPECTED_TYPES = "dist/public.d.ts"
EXPECTED_EXPORTS = {
    ".": {
        "types": "./dist/public.d.ts",
        "require": "./dist/public.js",
        "default": "./dist/public.js",
    },
    "./query-v2": {
        "types": "./dist/query-v2.d.ts",
        "require": "./dist/query-v2.js",
        "default": "./dist/query-v2.js",
    },
    "./runtime-projection": {
        "types": "./dist/runtime-projection.d.ts",
        "require": "./dist/runtime-projection.js",
        "default": "./dist/runtime-projection.js",
    },
}


def read_json_file(path: Path, *, label: str) -> dict[str, Any]:
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


def safe_member_name(raw_name: str, *, artifact: Path) -> str:
    """Validate one portable npm archive path rooted below package/."""
    path = PurePosixPath(raw_name)
    if (
        not raw_name
        or "\0" in raw_name
        or "\\" in raw_name
        or path.is_absolute()
        or re.match(r"^[A-Za-z]:", raw_name)
        or any(part in {"", ".", ".."} for part in raw_name.split("/"))
    ):
        raise ValidationError(f"Unsafe member path in {artifact.name}: {raw_name!r}")
    normalized = path.as_posix()
    if normalized != "package" and not normalized.startswith("package/"):
        raise ValidationError(
            f"Archive member is outside package/ in {artifact.name}: {raw_name!r}"
        )
    return normalized


def normalized_component(value: str) -> str:
    """Normalize separators in one package member component for identity checks."""
    return re.sub(r"[-_.]+", "-", value).lower()


def reject_historical_band9_member(name: str, *, artifact: Path) -> None:
    """Reject any archived path component that identifies a retired band-9 fork."""
    for component in PurePosixPath(name).parts:
        if HISTORICAL_BAND9_COMPONENT.search(normalized_component(component)) is not None:
            raise ValidationError(f"Historical band-9 fork payload in {artifact.name}: {name!r}")


def read_packed_package(
    artifact: Path,
) -> tuple[dict[str, Any], frozenset[str], bytes | None, bytes | None]:
    """Inspect the npm tarball and return metadata, files, README, and notice."""
    if not artifact.is_file() or artifact.is_symlink() or not artifact.name.endswith(".tgz"):
        raise ValidationError(f"npm artifact must be one regular .tgz file: {artifact}")
    try:
        archive = tarfile.open(artifact, mode="r:gz")
    except (OSError, tarfile.TarError) as error:
        raise ValidationError(f"Unreadable npm artifact {artifact}: {error}") from error
    with archive:
        members: dict[str, tarfile.TarInfo] = {}
        try:
            archive_members = archive.getmembers()
        except (OSError, tarfile.TarError) as error:
            raise ValidationError(f"Corrupt npm artifact {artifact}: {error}") from error
        for member in archive_members:
            name = safe_member_name(member.name.rstrip("/"), artifact=artifact)
            reject_historical_band9_member(name.removeprefix("package/"), artifact=artifact)
            if name in members:
                raise ValidationError(f"Duplicate npm archive member in {artifact.name}: {name}")
            if not (member.isdir() or member.isfile()):
                raise ValidationError(f"Non-regular npm archive member in {artifact.name}: {name}")
            members[name] = member
        package_json = members.get("package/package.json")
        if package_json is None or not package_json.isfile():
            raise ValidationError(f"{artifact.name} has no regular package/package.json")
        stream = archive.extractfile(package_json)
        if stream is None:
            raise ValidationError(f"Could not read package/package.json from {artifact.name}")
        try:
            payload = json.loads(stream.read().decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise ValidationError(
                f"Invalid package/package.json in {artifact.name}: {error}"
            ) from error
        if not isinstance(payload, dict):
            raise ValidationError(f"package/package.json must be an object in {artifact.name}")
        regular_files = frozenset(
            name.removeprefix("package/") for name, member in members.items() if member.isfile()
        )

        def packed_member_bytes(relative: str) -> bytes | None:
            member = members.get(f"package/{relative}")
            if member is None or not member.isfile():
                return None
            member_stream = archive.extractfile(member)
            if member_stream is None:
                raise ValidationError(f"Could not read package/{relative} from {artifact.name}")
            return member_stream.read()

        readme_bytes = packed_member_bytes(README)
        notice_bytes = packed_member_bytes(THIRD_PARTY_NOTICE)
        return payload, regular_files, readme_bytes, notice_bytes


def validate_runtime_inventory(artifact: Path, regular_files: frozenset[str]) -> None:
    """Require the complete public runtime and exactly the supported native set."""
    missing_runtime = sorted(REQUIRED_RUNTIME_MEMBERS - regular_files)
    if missing_runtime:
        raise ValidationError(
            f"{artifact.name} omits required runtime members: {missing_runtime!r}"
        )
    stale_runtime = sorted(
        name
        for name in regular_files
        if name in FORBIDDEN_RUNTIME_MEMBERS
        or any(name.startswith(prefix) for prefix in FORBIDDEN_RUNTIME_PREFIXES)
    )
    if stale_runtime:
        raise ValidationError(
            f"{artifact.name} contains stale duplicate runtime outputs: {stale_runtime!r}"
        )
    packed_native = frozenset(
        name for name in regular_files if PurePosixPath(name).suffix == ".node"
    )
    if packed_native != EXPECTED_NATIVE_MODULES:
        missing = sorted(EXPECTED_NATIVE_MODULES - packed_native)
        unexpected = sorted(packed_native - EXPECTED_NATIVE_MODULES)
        raise ValidationError(
            f"{artifact.name} has an invalid native module inventory: "
            f"missing={missing!r}, unexpected={unexpected!r}"
        )
    fixed_members = {
        "package.json",
        README,
        THIRD_PARTY_NOTICE,
        *EXPECTED_NATIVE_MODULES,
    }
    unexpected_members = sorted(
        name for name in regular_files if name not in fixed_members and not name.startswith("dist/")
    )
    if unexpected_members:
        raise ValidationError(
            f"{artifact.name} contains members outside the package.json files contract: "
            f"{unexpected_members!r}"
        )


def validate_native_directory(directory: Path) -> dict[str, Any]:
    """Require one fan-in directory to contain exactly the release native set."""
    if not directory.is_dir() or directory.is_symlink():
        raise ValidationError(f"Native module directory is missing or invalid: {directory}")
    native_modules = frozenset(
        entry.name
        for entry in directory.iterdir()
        if entry.name.endswith(".node") and entry.is_file() and not entry.is_symlink()
    )
    if native_modules != EXPECTED_NATIVE_MODULES:
        missing = sorted(EXPECTED_NATIVE_MODULES - native_modules)
        unexpected = sorted(native_modules - EXPECTED_NATIVE_MODULES)
        raise ValidationError(
            "Fan-in directory has an invalid native module inventory: "
            f"missing={missing!r}, unexpected={unexpected!r}"
        )
    return {
        "directory": str(directory),
        "native_modules": sorted(native_modules),
        "status": "ok",
    }


def package_identity(
    package: dict[str, Any],
    *,
    label: str,
    allow_prerelease: bool = False,
) -> tuple[str, str]:
    """Return one non-empty npm package name/version pair."""
    name = package.get("name")
    version = package.get("version")
    if not isinstance(name, str) or not name or not isinstance(version, str) or not version:
        raise ValidationError(f"{label} must declare non-empty string name and version")
    match = SEMVER_PATTERN.fullmatch(version)
    if match is None:
        raise ValidationError(f"{label} declares an invalid semantic version: {version!r}")
    if match.group(4) is not None and not allow_prerelease:
        raise ValidationError(
            f"{label} declares prerelease version {version!r}; "
            "this workflow publishes only stable versions to npm's latest tag"
        )
    return name, version


def validate_package_contract(package: dict[str, Any], *, label: str) -> None:
    """Require the immutable license and files contract in one package.json."""
    license_value = package.get("license")
    if license_value != MIT_LICENSE:
        raise ValidationError(
            f"{label} license must remain {MIT_LICENSE}: actual={license_value!r}"
        )
    raw_files = package.get("files")
    if (
        not isinstance(raw_files, list)
        or not all(isinstance(value, str) for value in raw_files)
        or len(raw_files) != len(set(raw_files))
        or frozenset(raw_files) != EXPECTED_PACKAGE_FILES
    ):
        raise ValidationError(
            f"{label} files contract disagrees: "
            f"actual={raw_files!r}, expected={sorted(EXPECTED_PACKAGE_FILES)!r}"
        )
    for field, expected in (
        ("main", EXPECTED_MAIN),
        ("types", EXPECTED_TYPES),
        ("exports", EXPECTED_EXPORTS),
    ):
        actual = package.get(field)
        if actual != expected:
            raise ValidationError(
                f"{label} {field} contract disagrees: actual={actual!r}, expected={expected!r}"
            )


def read_repository_member(repository_package: Path, name: str) -> bytes:
    """Read one required regular file beside the repository package.json."""
    path = repository_package.with_name(name)
    if not path.is_file() or path.is_symlink():
        raise ValidationError(f"repository {name} is missing or non-regular: {path}")
    try:
        return path.read_bytes()
    except OSError as error:
        raise ValidationError(f"Could not read repository {name} {path}: {error}") from error


def artifact_sri(path: Path, algorithm: str = "sha512") -> str:
    """Return an npm-compatible Subresource Integrity digest for one file."""
    digest = hashlib.new(algorithm)
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    encoded = base64.b64encode(digest.digest()).decode("ascii")
    return f"{algorithm}-{encoded}"


def packed_filename(name: str, version: str) -> str:
    """Return npm pack's deterministic tarball basename for a package identity."""
    flattened_name = name.removeprefix("@").replace("/", "-")
    return f"{flattened_name}-{version}.tgz"


def validate_registry_integrity(artifact: Path, registry_integrity: str) -> str:
    """Require at least one supported registry SRI token to match artifact bytes."""
    supported = {"sha1", "sha256", "sha384", "sha512"}
    comparisons: list[tuple[str, str]] = []
    for token in registry_integrity.split():
        algorithm, separator, encoded = token.partition("-")
        encoded = encoded.partition("?")[0]
        if not separator or algorithm not in supported or not encoded:
            continue
        try:
            base64.b64decode(encoded, validate=True)
        except ValueError:
            continue
        comparisons.append((token, artifact_sri(artifact, algorithm)))
    if not comparisons:
        raise ValidationError(
            f"Registry dist.integrity has no supported valid digest: {registry_integrity!r}"
        )
    mismatches = [
        token
        for token, actual in comparisons
        if not hmac.compare_digest(token.partition("?")[0], actual)
    ]
    if mismatches:
        actual = artifact_sri(artifact)
        raise ValidationError(
            f"Registry dist.integrity disagrees with {artifact.name}: "
            f"registry={registry_integrity!r}, mismatches={mismatches!r}, local={actual!r}"
        )
    return artifact_sri(artifact)


def validate_release_package(
    *,
    artifact: Path,
    repository_package: Path,
    tag: str,
    registry_integrity: str | None = None,
    allow_prerelease: bool = False,
) -> dict[str, Any]:
    """Validate archive identity against repository metadata and the release tag."""
    repository = read_json_file(repository_package, label="repository package.json")
    packed, regular_files, packed_readme, packed_notice = read_packed_package(artifact)
    validate_runtime_inventory(artifact, regular_files)
    validate_package_contract(repository, label="repository package.json")
    validate_package_contract(packed, label="packed package.json")
    repository_name, repository_version = package_identity(
        repository,
        label="repository package",
        allow_prerelease=allow_prerelease,
    )
    packed_name, packed_version = package_identity(
        packed,
        label="packed package",
        allow_prerelease=allow_prerelease,
    )
    if (packed_name, packed_version) != (repository_name, repository_version):
        raise ValidationError(
            "Packed npm identity disagrees with repository package.json: "
            f"packed={packed_name}@{packed_version}, "
            f"repository={repository_name}@{repository_version}"
        )
    expected_tag = f"v{repository_version}"
    if tag != expected_tag:
        raise ValidationError(
            f"Release tag disagrees with npm package version: tag={tag!r}, expected={expected_tag!r}"
        )
    expected_filename = packed_filename(repository_name, repository_version)
    if artifact.name != expected_filename:
        raise ValidationError(
            f"npm tarball filename disagrees with package identity: "
            f"artifact={artifact.name!r}, expected={expected_filename!r}"
        )
    repository_readme = read_repository_member(repository_package, README)
    if packed_readme != repository_readme:
        raise ValidationError(f"Packed npm {README} disagrees with repository source")
    repository_notice = read_repository_member(repository_package, THIRD_PARTY_NOTICE)
    if packed_notice != repository_notice:
        raise ValidationError(f"Packed npm {THIRD_PARTY_NOTICE} disagrees with repository source")
    integrity = artifact_sri(artifact)
    registry_match = False
    if registry_integrity is not None:
        validate_registry_integrity(artifact, registry_integrity)
        registry_match = True
    return {
        "artifact": artifact.name,
        "allow_prerelease": allow_prerelease,
        "integrity": integrity,
        "name": repository_name,
        "native_modules": sorted(EXPECTED_NATIVE_MODULES),
        "registry_match": registry_match,
        "status": "ok",
        "tag": tag,
        "version": repository_version,
    }


def build_parser() -> argparse.ArgumentParser:
    """Build the npm release validator CLI."""
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--artifact", type=Path)
    source.add_argument("--native-directory", type=Path)
    parser.add_argument("--repository-package", type=Path)
    parser.add_argument("--tag")
    parser.add_argument("--registry-integrity")
    parser.add_argument(
        "--allow-prerelease",
        action="store_true",
        help=(
            "accept a SemVer prerelease identity for non-publishing candidate validation; "
            "never use this flag in an npm publication job"
        ),
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Run npm release validation and print its machine-readable report."""
    args = build_parser().parse_args(argv)
    if args.native_directory is not None:
        if args.repository_package is not None or args.tag is not None:
            raise ValidationError("--repository-package and --tag are valid only with --artifact")
        report = validate_native_directory(args.native_directory.resolve())
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0
    if args.artifact is None or args.repository_package is None or args.tag is None:
        raise ValidationError("--artifact requires both --repository-package and --tag")
    report = validate_release_package(
        artifact=args.artifact.resolve(),
        repository_package=args.repository_package.resolve(),
        tag=args.tag,
        registry_integrity=args.registry_integrity,
        allow_prerelease=args.allow_prerelease,
    )
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ValidationError as error:
        print(f"Node release package validation failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
