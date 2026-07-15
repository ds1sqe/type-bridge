"""Hostile tests for npm release identity and registry-integrity validation."""

from __future__ import annotations

import importlib.util
import io
import json
import sys
import tarfile
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[3]
VALIDATOR_PATH = ROOT / "scripts/ci/validate_node_release_package.py"


def load_module(name: str, path: Path) -> ModuleType:
    """Load one standalone CI validator without making scripts a package."""
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


validator = load_module("validate_node_release_package", VALIDATOR_PATH)
PACKAGE_NAME = "@type-bridge/node"
VERSION = "1.5.8"


def write_json(path: Path, payload: dict[str, Any]) -> None:
    """Write one compact JSON fixture."""
    path.write_text(json.dumps(payload), encoding="utf-8")


def write_tarball(
    path: Path,
    package: dict[str, Any],
    *,
    duplicate_package_json: bool = False,
    native_modules: set[str] | None = None,
    omit_runtime_member: str | None = None,
    extra_members: dict[str, bytes] | None = None,
    unsafe_member: bool = False,
) -> Path:
    """Write a minimal npm-shaped package archive."""
    payload = json.dumps(package).encode()
    runtime_members = {
        "dist/index.d.ts": b"export {};\n",
        "dist/index.js": b"module.exports = {};\n",
        "dist/native.d.ts": b"export declare function loadNative(): object;\n",
        "dist/native.js": b"exports.loadNative = () => ({});\n",
        "dist/typed/index.d.ts": b"export {};\n",
        "dist/typed/index.js": b"module.exports = {};\n",
    }
    if omit_runtime_member is not None:
        runtime_members.pop(omit_runtime_member)
    selected_native = (
        set(validator.EXPECTED_NATIVE_MODULES) if native_modules is None else native_modules
    )
    with tarfile.open(path, "w:gz") as archive:
        members = {
            "package/package.json": payload,
            **{f"package/{name}": content for name, content in runtime_members.items()},
            **{f"package/{name}": b"native\n" for name in selected_native},
            **{f"package/{name}": content for name, content in (extra_members or {}).items()},
        }
        for name, content in members.items():
            info = tarfile.TarInfo(name)
            info.size = len(content)
            archive.addfile(info, io.BytesIO(content))
        if duplicate_package_json:
            info = tarfile.TarInfo("package/package.json")
            info.size = len(payload)
            archive.addfile(info, io.BytesIO(payload))
        if unsafe_member:
            content = b"escape"
            info = tarfile.TarInfo("package/../escape")
            info.size = len(content)
            archive.addfile(info, io.BytesIO(content))
    return path


def release_fixture(tmp_path: Path) -> tuple[Path, Path]:
    """Return matching repository metadata and npm tarball paths."""
    repository = tmp_path / "package.json"
    artifact = tmp_path / "type-bridge-node-1.5.8.tgz"
    package = {"name": PACKAGE_NAME, "version": VERSION}
    write_json(repository, package)
    write_tarball(artifact, package)
    return repository, artifact


def validate(repository: Path, artifact: Path, **overrides: Any) -> dict[str, Any]:
    """Run the release validator with fixture defaults."""
    return validator.validate_release_package(
        artifact=artifact,
        repository_package=repository,
        tag=overrides.pop("tag", f"v{VERSION}"),
        **overrides,
    )


def test_matching_identity_tag_and_integrity_pass(tmp_path: Path) -> None:
    repository, artifact = release_fixture(tmp_path)
    integrity = validator.artifact_sri(artifact)

    report = validate(repository, artifact, registry_integrity=integrity)

    assert report == {
        "artifact": artifact.name,
        "integrity": integrity,
        "name": PACKAGE_NAME,
        "native_modules": sorted(validator.EXPECTED_NATIVE_MODULES),
        "registry_match": True,
        "status": "ok",
        "tag": f"v{VERSION}",
        "version": VERSION,
    }


@pytest.mark.parametrize(
    ("packed", "tag", "message"),
    [
        ({"name": "@hostile/replacement", "version": VERSION}, f"v{VERSION}", "identity"),
        ({"name": PACKAGE_NAME, "version": "9.9.9"}, f"v{VERSION}", "identity"),
        ({"name": PACKAGE_NAME, "version": VERSION}, "v9.9.9", "Release tag"),
    ],
)
def test_identity_or_tag_mismatch_hard_fails(
    tmp_path: Path,
    packed: dict[str, str],
    tag: str,
    message: str,
) -> None:
    repository, artifact = release_fixture(tmp_path)
    write_tarball(artifact, packed)

    with pytest.raises(validator.ValidationError, match=message):
        validate(repository, artifact, tag=tag)


def test_registry_integrity_mismatch_hard_fails(tmp_path: Path) -> None:
    repository, artifact = release_fixture(tmp_path)
    wrong = "sha512-" + ("A" * 88)

    with pytest.raises(validator.ValidationError, match="dist.integrity disagrees"):
        validate(repository, artifact, registry_integrity=wrong)


def test_tarball_basename_must_match_scoped_package_identity(tmp_path: Path) -> None:
    repository, artifact = release_fixture(tmp_path)
    renamed = artifact.with_name("renamed-release.tgz")
    artifact.rename(renamed)

    with pytest.raises(validator.ValidationError, match="tarball filename disagrees"):
        validate(repository, renamed)


def test_registry_integrity_rejects_mixed_matching_and_mismatching_tokens(
    tmp_path: Path,
) -> None:
    repository, artifact = release_fixture(tmp_path)
    mixed = f"{validator.artifact_sri(artifact)} sha256-{'A' * 44}"

    with pytest.raises(validator.ValidationError, match="dist.integrity disagrees"):
        validate(repository, artifact, registry_integrity=mixed)


def test_prerelease_version_is_rejected_before_latest_publication(tmp_path: Path) -> None:
    repository = tmp_path / "package.json"
    artifact = tmp_path / "type-bridge-node-1.5.8-rc.1.tgz"
    package = {"name": PACKAGE_NAME, "version": "1.5.8-rc.1"}
    write_json(repository, package)
    write_tarball(artifact, package)

    with pytest.raises(validator.ValidationError, match="prerelease version"):
        validator.validate_release_package(
            artifact=artifact,
            repository_package=repository,
            tag="v1.5.8-rc.1",
        )


@pytest.mark.parametrize("hostility", ["duplicate", "unsafe"])
def test_hostile_archive_structure_hard_fails(tmp_path: Path, hostility: str) -> None:
    repository, artifact = release_fixture(tmp_path)
    write_tarball(
        artifact,
        {"name": PACKAGE_NAME, "version": VERSION},
        duplicate_package_json=hostility == "duplicate",
        unsafe_member=hostility == "unsafe",
    )

    with pytest.raises(validator.ValidationError, match="Duplicate|Unsafe"):
        validate(repository, artifact)


@pytest.mark.parametrize("kind", ["missing", "unexpected"])
def test_native_module_inventory_must_be_exact(tmp_path: Path, kind: str) -> None:
    repository, artifact = release_fixture(tmp_path)
    native_modules = set(validator.EXPECTED_NATIVE_MODULES)
    if kind == "missing":
        native_modules.remove("type_bridge_node.darwin-arm64.node")
    else:
        native_modules.add("type_bridge_node.freebsd-x64.node")
    write_tarball(
        artifact,
        {"name": PACKAGE_NAME, "version": VERSION},
        native_modules=native_modules,
    )

    with pytest.raises(validator.ValidationError, match="native module inventory"):
        validate(repository, artifact)


def test_required_compiled_runtime_must_be_present(tmp_path: Path) -> None:
    repository, artifact = release_fixture(tmp_path)
    write_tarball(
        artifact,
        {"name": PACKAGE_NAME, "version": VERSION},
        omit_runtime_member="dist/typed/index.d.ts",
    )

    with pytest.raises(validator.ValidationError, match="required runtime members"):
        validate(repository, artifact)


def test_stale_duplicate_typescript_output_hard_fails(tmp_path: Path) -> None:
    repository, artifact = release_fixture(tmp_path)
    write_tarball(
        artifact,
        {"name": PACKAGE_NAME, "version": VERSION},
        extra_members={"dist/typescript/index.js": b"stale duplicate\n"},
    )

    with pytest.raises(validator.ValidationError, match="stale duplicate runtime outputs"):
        validate(repository, artifact)


def test_fan_in_directory_inventory_is_exact(tmp_path: Path) -> None:
    for name in validator.EXPECTED_NATIVE_MODULES:
        (tmp_path / name).write_bytes(b"native")

    report = validator.validate_native_directory(tmp_path)

    assert report["native_modules"] == sorted(validator.EXPECTED_NATIVE_MODULES)
    (tmp_path / "type_bridge_node.win32-arm64-msvc.node").unlink()
    with pytest.raises(validator.ValidationError, match="Fan-in directory"):
        validator.validate_native_directory(tmp_path)
