"""Hostile coverage for exact packaged Rust release artifacts."""

from __future__ import annotations

import importlib.util
import io
import sys
import tarfile
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[3]
VALIDATOR_PATH = ROOT / "scripts/ci/validate_rust_release_artifacts.py"


def load_module(name: str, path: Path) -> ModuleType:
    """Load one standalone CI validator without making scripts a package."""
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


validator = load_module("validate_rust_release_artifacts", VALIDATOR_PATH)


def canonical_license(license_id: str) -> bytes:
    """Return the repository's canonical body for one package family."""
    paths = {
        validator.MIT_LICENSE: ROOT / "LICENSE",
        validator.APACHE_2_LICENSE: ROOT / "type-bridge-core/vendor/typedb-driver-b9/LICENSE",
        validator.MPL_2_LICENSE: ROOT / "type-bridge-core/vendor/typedb-protocol-b9/LICENSE",
    }
    return paths[license_id].read_bytes()


def synthetic_readme(name: str) -> bytes:
    """Return the repository-exact README body for one synthetic package."""
    return f"# {name}\n\nSynthetic package landing documentation.\n".encode()


def crate_bytes(
    name: str,
    version: str,
    license_id: str,
    *,
    manifest_name: str | None = None,
    manifest_version: str | None = None,
    manifest_license: str | None = None,
    manifest_license_file: str = "LICENSE",
    manifest_readme: str = "README.md",
    manifest_body: bytes | None = None,
    manifest_orig: bytes | None = None,
    license_path: str = "LICENSE",
    license_body: bytes | None = None,
    readme_path: str | None = "README.md",
    readme_body: bytes | None = None,
    extra_files: dict[str, bytes] | None = None,
    symlink: tuple[str, str] | None = None,
) -> bytes:
    """Build one in-memory synthetic .crate archive."""
    root = f"{name}-{version}"
    manifest = (
        manifest_body
        or (
            "[package]\n"
            f'name = "{manifest_name or name}"\n'
            f'version = "{manifest_version or version}"\n'
            f'license = "{manifest_license or license_id}"\n'
            f'license-file = "{manifest_license_file}"\n'
            f'readme = "{manifest_readme}"\n'
        ).encode()
    )
    files = {
        "Cargo.toml": manifest,
        license_path: canonical_license(license_id) if license_body is None else license_body,
        "src/lib.rs": b"pub fn packaged() {}\n",
        **(extra_files or {}),
    }
    if readme_path is not None:
        files[readme_path] = synthetic_readme(name) if readme_body is None else readme_body
    if manifest_orig is not None:
        files["Cargo.toml.orig"] = manifest_orig
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode="w:gz") as archive:
        for relative, body in sorted(files.items()):
            member = tarfile.TarInfo(f"{root}/{relative}")
            member.size = len(body)
            archive.addfile(member, io.BytesIO(body))
        if symlink is not None:
            relative, target = symlink
            member = tarfile.TarInfo(f"{root}/{relative}")
            member.type = tarfile.SYMTYPE
            member.linkname = target
            archive.addfile(member)
    return stream.getvalue()


def write_release_set(directory: Path, release_version: str = "9.8.7") -> Path:
    """Write the complete closed archive set expected by the gate."""
    directory.mkdir()
    repository_root = directory.parent / "repository"
    for name, (version, license_id) in validator.expected_packages(release_version).items():
        archive = crate_bytes(name, version, license_id)
        readme_path = repository_root / validator.PUBLIC_SOURCE_READMES[name]
        readme_path.parent.mkdir(parents=True, exist_ok=True)
        readme_path.write_bytes(synthetic_readme(name))
        (directory / f"{name}-{version}.crate").write_bytes(archive)
    return repository_root


def replace_archive(
    directory: Path,
    release_version: str,
    name: str,
    **overrides: Any,
) -> Path:
    """Replace one expected archive with a hostile payload under the same filename."""
    version, license_id = validator.expected_packages(release_version)[name]
    path = directory / f"{name}-{version}.crate"
    path.write_bytes(crate_bytes(name, version, license_id, **overrides))
    return path


def test_complete_rust_release_archive_set_is_accepted(tmp_path: Path) -> None:
    artifacts = tmp_path / "package"
    repository_root = write_release_set(artifacts)

    report = validator.validate_release_artifacts(
        artifacts,
        expected_release_version="9.8.7",
        repository_root=repository_root,
    )

    assert report["status"] == "ok"
    assert len(report["artifacts"]) == 15
    assert {entry["license"] for entry in report["artifacts"]} == {"MIT"}


def test_expected_rust_archive_identity_set_is_closed() -> None:
    expected = validator.expected_packages("2.0.0")

    assert set(expected) == {
        *validator.FIRST_PARTY_PACKAGES,
        *validator.VENDORED_PACKAGES,
    }
    assert "type-bridge-typedb-protocol-b7" not in expected
    assert "type-bridge-typedb-driver-b7" not in expected
    assert expected["type-bridge-core-lib"] == ("2.0.0", "MIT")
    assert expected["type-bridge-contract"] == ("2.0.0", "MIT")
    assert expected["type-bridge-schema"] == ("2.0.0", "MIT")
    assert expected["type-bridge-query"] == ("2.0.0", "MIT")
    assert expected["type-bridge-schema-migration"] == ("2.0.0", "MIT")
    assert expected["type-bridge-schema-migration-typedb"] == ("2.0.0", "MIT")
    assert expected["type-bridge-schema-codegen"] == ("2.0.0", "MIT")
    assert expected["type-bridge-schema-compat"] == ("2.0.0", "MIT")
    assert expected["type-bridge-workspace"] == ("2.0.0", "MIT")
    assert expected["type-bridge-cli"] == ("2.0.0", "MIT")
    assert expected["type-bridge"] == ("2.0.0", "MIT")
    assert expected["type-bridge-server"] == ("2.0.0", "MIT")


@pytest.mark.parametrize("mutation", ("missing", "unexpected"))
def test_rust_archive_inventory_must_be_exact(tmp_path: Path, mutation: str) -> None:
    artifacts = tmp_path / "package"
    repository_root = write_release_set(artifacts)
    if mutation == "missing":
        next(artifacts.glob("*.crate")).unlink()
    else:
        (artifacts / "unexpected-1.0.0.crate").write_bytes(b"hostile")

    with pytest.raises(validator.ValidationError, match="archive inventory drifted"):
        validator.validate_release_artifacts(
            artifacts,
            expected_release_version="9.8.7",
            repository_root=repository_root,
        )


def test_packaged_manifest_identity_must_match_archive_key(tmp_path: Path) -> None:
    artifacts = tmp_path / "package"
    repository_root = write_release_set(artifacts)
    replace_archive(
        artifacts,
        "9.8.7",
        "type-bridge-core-lib",
        manifest_name="hostile-core-lib",
    )

    with pytest.raises(validator.ValidationError, match="manifest identity drifted"):
        validator.validate_release_artifacts(
            artifacts,
            expected_release_version="9.8.7",
            repository_root=repository_root,
        )


@pytest.mark.parametrize(
    ("overrides", "message"),
    [
        ({"manifest_license": "GPL-3.0"}, "license metadata drifted"),
        ({"manifest_license_file": "licenses/LICENSE"}, "packaged root LICENSE"),
        ({"license_path": "licenses/LICENSE"}, "exactly one root LICENSE"),
        ({"license_body": b"not the MIT license\n"}, "not the canonical MIT body"),
        ({"extra_files": {"docs/LICENSE.txt": b"conflicting\n"}}, "exactly one root LICENSE"),
    ],
)
def test_packaged_license_metadata_placement_and_body_are_closed(
    tmp_path: Path,
    overrides: dict[str, object],
    message: str,
) -> None:
    artifacts = tmp_path / "package"
    repository_root = write_release_set(artifacts)
    replace_archive(
        artifacts,
        "9.8.7",
        "type-bridge-core-lib",
        **overrides,
    )

    with pytest.raises(validator.ValidationError, match=message):
        validator.validate_release_artifacts(
            artifacts,
            expected_release_version="9.8.7",
            repository_root=repository_root,
        )


@pytest.mark.parametrize(
    ("overrides", "message"),
    [
        ({"manifest_readme": "OTHER.md"}, "root README.md"),
        ({"readme_path": None}, "no declared root README.md"),
        ({"readme_path": "docs/README.md"}, "no declared root README.md"),
        ({"readme_body": b"stale package landing page\n"}, "drifted from its repository source"),
    ],
)
def test_packaged_readme_metadata_placement_and_body_are_closed(
    tmp_path: Path,
    overrides: dict[str, object],
    message: str,
) -> None:
    artifacts = tmp_path / "package"
    repository_root = write_release_set(artifacts)
    replace_archive(
        artifacts,
        "9.8.7",
        "type-bridge-core-lib",
        **overrides,
    )

    with pytest.raises(validator.ValidationError, match=message):
        validator.validate_release_artifacts(
            artifacts,
            expected_release_version="9.8.7",
            repository_root=repository_root,
        )


def test_packaged_archive_traversal_member_hard_fails(tmp_path: Path) -> None:
    artifacts = tmp_path / "package"
    repository_root = write_release_set(artifacts)
    replace_archive(
        artifacts,
        "9.8.7",
        "type-bridge-core-lib",
        extra_files={"../escape": b"hostile\n"},
    )

    with pytest.raises(validator.ValidationError, match="unsafe path"):
        validator.validate_release_artifacts(
            artifacts,
            expected_release_version="9.8.7",
            repository_root=repository_root,
        )


def test_packaged_archive_symlink_member_hard_fails(tmp_path: Path) -> None:
    artifacts = tmp_path / "package"
    repository_root = write_release_set(artifacts)
    replace_archive(
        artifacts,
        "9.8.7",
        "type-bridge-core-lib",
        symlink=("src/linked.rs", "../../host-file"),
    )

    with pytest.raises(validator.ValidationError, match="symlink or non-regular"):
        validator.validate_release_artifacts(
            artifacts,
            expected_release_version="9.8.7",
            repository_root=repository_root,
        )
