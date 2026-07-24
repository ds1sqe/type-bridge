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
VERSION = "1.5.11"
PACKAGE_FILES = ["dist", "*.node", "README.md", "THIRD_PARTY_NOTICES.md"]
README_BYTES = (ROOT / "type-bridge-core/crates/node/README.md").read_bytes()
NOTICE_BYTES = (ROOT / "type-bridge-core/crates/node/THIRD_PARTY_NOTICES.md").read_bytes()


def write_json(path: Path, payload: dict[str, Any]) -> None:
    """Write one compact JSON fixture."""
    path.write_text(json.dumps(payload), encoding="utf-8")


def package_payload(**overrides: Any) -> dict[str, Any]:
    """Return the immutable release package contract with selected mutations."""
    payload: dict[str, Any] = {
        "name": PACKAGE_NAME,
        "version": VERSION,
        "license": validator.MIT_LICENSE,
        "files": list(PACKAGE_FILES),
    }
    payload.update(overrides)
    return payload


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
        validator.README: README_BYTES,
        validator.THIRD_PARTY_NOTICE: NOTICE_BYTES,
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
    artifact = tmp_path / "type-bridge-node-1.5.11.tgz"
    package = package_payload()
    write_json(repository, package)
    repository.with_name(validator.README).write_bytes(README_BYTES)
    repository.with_name(validator.THIRD_PARTY_NOTICE).write_bytes(NOTICE_BYTES)
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
        "allow_prerelease": False,
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
        (package_payload(name="@hostile/replacement"), f"v{VERSION}", "identity"),
        (package_payload(version="9.9.9"), f"v{VERSION}", "identity"),
        (package_payload(), "v9.9.9", "Release tag"),
    ],
)
def test_identity_or_tag_mismatch_hard_fails(
    tmp_path: Path,
    packed: dict[str, Any],
    tag: str,
    message: str,
) -> None:
    repository, artifact = release_fixture(tmp_path)
    write_tarball(artifact, packed)

    with pytest.raises(validator.ValidationError, match=message):
        validate(repository, artifact, tag=tag)


@pytest.mark.parametrize("license_value", [None, "ISC"])
def test_packed_package_license_must_be_explicit_mit(
    tmp_path: Path,
    license_value: str | None,
) -> None:
    repository, artifact = release_fixture(tmp_path)
    packed = package_payload(license=license_value)
    if license_value is None:
        packed.pop("license")
    write_tarball(artifact, packed)

    with pytest.raises(validator.ValidationError, match="packed package.json license"):
        validate(repository, artifact)


@pytest.mark.parametrize("license_value", [None, "Apache-2.0"])
def test_repository_package_license_must_be_explicit_mit(
    tmp_path: Path,
    license_value: str | None,
) -> None:
    repository, artifact = release_fixture(tmp_path)
    payload = package_payload(license=license_value)
    if license_value is None:
        payload.pop("license")
    write_json(repository, payload)

    with pytest.raises(validator.ValidationError, match="repository package.json license"):
        validate(repository, artifact)


@pytest.mark.parametrize(
    "files",
    [
        ["dist", "*.node", "README.md"],
        [*PACKAGE_FILES, "scripts"],
        [*PACKAGE_FILES, "README.md"],
    ],
)
def test_packed_package_files_contract_must_be_exact(
    tmp_path: Path,
    files: list[str],
) -> None:
    repository, artifact = release_fixture(tmp_path)
    write_tarball(artifact, package_payload(files=files))

    with pytest.raises(validator.ValidationError, match="packed package.json files contract"):
        validate(repository, artifact)


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
    artifact = tmp_path / "type-bridge-node-1.5.11-rc.1.tgz"
    package = package_payload(version="1.5.11-rc.1")
    write_json(repository, package)
    write_tarball(artifact, package)

    with pytest.raises(validator.ValidationError, match="prerelease version"):
        validator.validate_release_package(
            artifact=artifact,
            repository_package=repository,
            tag="v1.5.11-rc.1",
        )


def test_prerelease_version_is_accepted_only_for_nonpublishing_candidate(
    tmp_path: Path,
) -> None:
    repository = tmp_path / "package.json"
    artifact = tmp_path / "type-bridge-node-2.0.0-rc.0.tgz"
    package = package_payload(version="2.0.0-rc.0")
    write_json(repository, package)
    repository.with_name(validator.README).write_bytes(README_BYTES)
    repository.with_name(validator.THIRD_PARTY_NOTICE).write_bytes(NOTICE_BYTES)
    write_tarball(artifact, package)

    report = validator.validate_release_package(
        artifact=artifact,
        repository_package=repository,
        tag="v2.0.0-rc.0",
        allow_prerelease=True,
    )

    assert report["allow_prerelease"] is True
    assert report["version"] == "2.0.0-rc.0"


def test_cli_prerelease_flag_is_explicit_and_defaults_off() -> None:
    parser = validator.build_parser()

    stable = parser.parse_args(["--artifact", "package.tgz", "--tag", "v2.0.0"])
    candidate = parser.parse_args(
        [
            "--artifact",
            "package.tgz",
            "--tag",
            "v2.0.0-rc.0",
            "--allow-prerelease",
        ]
    )

    assert stable.allow_prerelease is False
    assert candidate.allow_prerelease is True


@pytest.mark.parametrize("hostility", ["duplicate", "unsafe"])
def test_hostile_archive_structure_hard_fails(tmp_path: Path, hostility: str) -> None:
    repository, artifact = release_fixture(tmp_path)
    write_tarball(
        artifact,
        package_payload(),
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
        package_payload(),
        native_modules=native_modules,
    )

    with pytest.raises(validator.ValidationError, match="native module inventory"):
        validate(repository, artifact)


def test_required_compiled_runtime_must_be_present(tmp_path: Path) -> None:
    repository, artifact = release_fixture(tmp_path)
    write_tarball(
        artifact,
        package_payload(),
        omit_runtime_member="dist/typed/index.d.ts",
    )

    with pytest.raises(validator.ValidationError, match="required runtime members"):
        validate(repository, artifact)


def test_third_party_notice_must_be_present_and_exact(tmp_path: Path) -> None:
    repository, artifact = release_fixture(tmp_path)
    write_tarball(
        artifact,
        package_payload(),
        omit_runtime_member=validator.THIRD_PARTY_NOTICE,
    )
    with pytest.raises(validator.ValidationError, match="required runtime members"):
        validate(repository, artifact)

    write_tarball(
        artifact,
        package_payload(),
        extra_members={validator.THIRD_PARTY_NOTICE: b"incomplete notice\n"},
    )
    with pytest.raises(validator.ValidationError, match="disagrees with repository source"):
        validate(repository, artifact)


def test_readme_must_be_present_and_exact(tmp_path: Path) -> None:
    repository, artifact = release_fixture(tmp_path)
    write_tarball(
        artifact,
        package_payload(),
        omit_runtime_member=validator.README,
    )
    with pytest.raises(validator.ValidationError, match="required runtime members"):
        validate(repository, artifact)

    write_tarball(
        artifact,
        package_payload(),
        extra_members={validator.README: b"drifted readme\n"},
    )
    with pytest.raises(validator.ValidationError, match="README.md disagrees"):
        validate(repository, artifact)


def test_stale_duplicate_typescript_output_hard_fails(tmp_path: Path) -> None:
    repository, artifact = release_fixture(tmp_path)
    write_tarball(
        artifact,
        package_payload(),
        extra_members={"dist/typescript/index.js": b"stale duplicate\n"},
    )

    with pytest.raises(validator.ValidationError, match="stale duplicate runtime outputs"):
        validate(repository, artifact)


@pytest.mark.parametrize(
    "member",
    [
        "dist/vendor/typedb-driver-b9/Cargo.toml",
        "dist/vendor/typedb_protocol_b9/LICENSE",
        "dist/vendor/type-bridge-typedb-driver-b9/Cargo.toml",
        "dist/vendor/type_bridge_typedb_protocol_b9/LICENSE",
    ],
)
def test_tarball_rejects_historical_band9_payload_names(
    tmp_path: Path,
    member: str,
) -> None:
    repository, artifact = release_fixture(tmp_path)
    write_tarball(
        artifact,
        package_payload(),
        extra_members={member: b"hostile\n"},
    )

    with pytest.raises(validator.ValidationError, match="Historical band-9 fork payload"):
        validate(repository, artifact)


@pytest.mark.parametrize("member", ["LICENSE", "scripts/postinstall.js", "backdoor.js"])
def test_tarball_rejects_arbitrary_top_level_extras(
    tmp_path: Path,
    member: str,
) -> None:
    repository, artifact = release_fixture(tmp_path)
    write_tarball(
        artifact,
        package_payload(),
        extra_members={member: b"unexpected\n"},
    )

    with pytest.raises(validator.ValidationError, match="outside the package.json files contract"):
        validate(repository, artifact)


def test_fan_in_directory_inventory_is_exact(tmp_path: Path) -> None:
    for name in validator.EXPECTED_NATIVE_MODULES:
        (tmp_path / name).write_bytes(b"native")

    report = validator.validate_native_directory(tmp_path)

    assert report["native_modules"] == sorted(validator.EXPECTED_NATIVE_MODULES)
    (tmp_path / "type_bridge_node.win32-arm64-msvc.node").unlink()
    with pytest.raises(validator.ValidationError, match="Fan-in directory"):
        validator.validate_native_directory(tmp_path)
