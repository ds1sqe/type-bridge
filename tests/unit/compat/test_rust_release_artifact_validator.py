"""Hostile coverage for exact packaged Rust release candidates."""

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
        validator.APACHE_2_LICENSE: ROOT / "type-bridge-core/vendor/typedb-driver-b8/LICENSE",
        validator.MPL_2_LICENSE: ROOT / "type-bridge-core/vendor/typedb-protocol-b8/LICENSE",
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


def synthetic_vendor_manifests(
    name: str,
    version: str,
    license_id: str,
) -> tuple[bytes, bytes]:
    """Return one source manifest and Cargo's expected normalized equivalent."""
    source = f"""
[package]
name = "{name}"
version = "{version}"
edition = "2024"
license = "{license_id}"
license-file = "LICENSE"
readme = "README.md"
description = "Synthetic compatibility package"

[features]
compat = []

[lib]
path = "src/lib.rs"
doctest = false

[dependencies.tonic]
version = "0.12"
features = ["tls"]

[dev-dependencies]
rand = "0.8"

[lints.rust]
unused = "allow"
""".lstrip().encode()
    normalized = f"""
[package]
name = "{name}"
version = "{version}"
edition = "2024"
license = "{license_id}"
license-file = "LICENSE"
readme = "README.md"
description = "Synthetic compatibility package"
build = false
autolib = false
autobins = false
autoexamples = false
autotests = false
autobenches = false

[features]
compat = []

[lib]
name = "{name.replace("-", "_")}"
path = "src/lib.rs"
doctest = false

[dependencies.tonic]
version = "0.12"
features = ["tls"]

[dev-dependencies.rand]
version = "0.8"

[lints.rust]
unused = "allow"
""".lstrip().encode()
    return source, normalized


def write_release_set(directory: Path, release_version: str = "9.8.7") -> Path:
    """Write the complete closed archive set expected by the gate."""
    directory.mkdir()
    repository_root = directory.parent / "repository"
    for name, (version, license_id) in validator.expected_packages(release_version).items():
        if name in validator.VENDORED_SOURCE_MANIFESTS:
            source, normalized = synthetic_vendor_manifests(name, version, license_id)
            source_path = repository_root / validator.VENDORED_SOURCE_MANIFESTS[name]
            source_path.parent.mkdir(parents=True, exist_ok=True)
            source_path.write_bytes(source)
            archive = crate_bytes(
                name,
                version,
                license_id,
                manifest_body=normalized,
                manifest_orig=source,
            )
        else:
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
    assert len(report["artifacts"]) == 19
    assert {entry["license"] for entry in report["artifacts"]} == {
        "MIT",
        "Apache-2.0",
        "MPL-2.0",
    }


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
    assert expected["type-bridge-typedb-protocol-b8"] == ("3.11.0", "MPL-2.0")
    assert expected["type-bridge-typedb-driver-b8"] == ("3.11.5", "Apache-2.0")


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


def test_b8_packaged_original_manifest_is_repository_exact(tmp_path: Path) -> None:
    artifacts = tmp_path / "package"
    repository_root = write_release_set(artifacts)
    name = "type-bridge-typedb-driver-b8"
    version, license_id = validator.expected_packages("9.8.7")[name]
    source, normalized = synthetic_vendor_manifests(name, version, license_id)
    replace_archive(
        artifacts,
        "9.8.7",
        name,
        manifest_body=normalized,
        manifest_orig=source + b"# post-package drift\n",
    )

    with pytest.raises(validator.ValidationError, match="Cargo.toml.orig drifted"):
        validator.validate_release_artifacts(
            artifacts,
            expected_release_version="9.8.7",
            repository_root=repository_root,
        )


@pytest.mark.parametrize(
    ("needle", "replacement"),
    (
        ("compat = []", 'compat = ["hostile"]'),
        ('path = "src/lib.rs"', 'path = "src/hostile.rs"'),
        ('features = ["tls"]', 'features = ["tls", "hostile"]'),
        ("build = false", 'build = "build.rs"'),
        ('unused = "allow"', 'unused = "deny"'),
    ),
)
def test_b8_normalized_manifest_behavior_is_closed(
    tmp_path: Path,
    needle: str,
    replacement: str,
) -> None:
    artifacts = tmp_path / "package"
    repository_root = write_release_set(artifacts)
    name = "type-bridge-typedb-driver-b8"
    version, license_id = validator.expected_packages("9.8.7")[name]
    source, normalized = synthetic_vendor_manifests(name, version, license_id)
    assert needle.encode() in normalized
    replace_archive(
        artifacts,
        "9.8.7",
        name,
        manifest_body=normalized.replace(needle.encode(), replacement.encode(), 1),
        manifest_orig=source,
    )

    with pytest.raises(validator.ValidationError, match="packaging-only transform"):
        validator.validate_release_artifacts(
            artifacts,
            expected_release_version="9.8.7",
            repository_root=repository_root,
        )


def test_b8_normalized_manifest_cannot_add_build_dependency(tmp_path: Path) -> None:
    artifacts = tmp_path / "package"
    repository_root = write_release_set(artifacts)
    name = "type-bridge-typedb-protocol-b8"
    version, license_id = validator.expected_packages("9.8.7")[name]
    source, normalized = synthetic_vendor_manifests(name, version, license_id)
    replace_archive(
        artifacts,
        "9.8.7",
        name,
        manifest_body=normalized + b'\n[build-dependencies.cc]\nversion = "1"\n',
        manifest_orig=source,
    )

    with pytest.raises(validator.ValidationError, match="packaging-only transform"):
        validator.validate_release_artifacts(
            artifacts,
            expected_release_version="9.8.7",
            repository_root=repository_root,
        )
