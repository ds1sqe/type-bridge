"""Closed-contract tests for the byte-exact Cargo candidate bundle."""

from __future__ import annotations

import hashlib
import io
import json
import os
import struct
import subprocess
import tarfile
from pathlib import Path, PurePosixPath

import pytest

from scripts.ci import cargo_release_candidate as candidate
from scripts.ci.cargo_release_inventory import (
    CargoInventoryPackage,
    CargoReleaseInventory,
    load_inventory,
)

ROOT = Path(__file__).resolve().parents[3]


def package(
    name: str,
    *,
    classification: str,
    order: int,
    checksum: str | None = None,
) -> CargoInventoryPackage:
    return CargoInventoryPackage(
        name=name,
        manifest=f"crates/{name}/Cargo.toml",
        classification=classification,
        role="supporting",
        version_policy="fixed",
        version="1.2.3",
        publish_order=order,
        docs_target="lib",
        registry_checksum=checksum,
    )


def crate_bytes(package_name: str) -> bytes:
    manifest = f'''[package]
name = "{package_name}"
version = "1.2.3"
authors = ["Ada"]
description = "candidate fixture"
documentation = "https://docs.rs/{package_name}/1.2.3"
readme = "README.md"
license = "MIT"
license-file = "LICENSE"
repository = "https://example.invalid/repository"
rust-version = "1.88"

[features]
default = ["dep:alias"]

[dependencies.alias]
package = "actual-dependency"
version = "1"
features = ["serde"]
optional = true
default-features = false

[target.'cfg(unix)'.build-dependencies.build-helper]
version = "=2.0.0"
'''.encode()
    members = {
        "Cargo.toml": manifest,
        "LICENSE": b"fixture license\n",
        "README.md": b"# Candidate fixture\n",
        "src/lib.rs": b"pub fn fixture() {}\n",
    }
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode="w:gz", format=tarfile.PAX_FORMAT) as archive:
        for relative, body in members.items():
            info = tarfile.TarInfo(f"{package_name}-1.2.3/{relative}")
            info.size = len(body)
            info.mtime = 0
            archive.addfile(info, io.BytesIO(body))
    return stream.getvalue()


def write_bundle(tmp_path: Path) -> tuple[Path, CargoReleaseInventory]:
    first = package("first-party", classification="public-first-party", order=1)
    first_archive = crate_bytes(first.name)
    immutable_archive = crate_bytes("immutable-input")
    immutable = package(
        "immutable-input",
        classification="public-immutable",
        order=2,
        checksum=hashlib.sha256(immutable_archive).hexdigest(),
    )
    inventory = CargoReleaseInventory(
        release_version="1.2.3",
        first_party_msrv="1.88",
        candidate_toolchain="1.94.1",
        repository="https://example.invalid/repository",
        packages=(first, immutable),
    )
    root = tmp_path / "candidate"
    root.mkdir()
    entries: list[dict[str, object]] = []
    for item, archive in ((first, first_archive), (immutable, immutable_archive)):
        archive_name = f"{item.name}-{item.version}.crate"
        (root / archive_name).write_bytes(archive)
        if item.immutable:
            metadata_sha = request_name = request_sha = request_size = None
        else:
            files = candidate.safe_crate_files(archive, package=item)
            manifest = candidate._parse_toml(  # noqa: SLF001 - independent fixture authority
                files[PurePosixPath("Cargo.toml")], label=item.name
            )
            metadata = candidate.publish_metadata_from_manifest(manifest, files)
            metadata_body, request = candidate.encode_publish_request(metadata, archive)
            request_name = f"{item.name}-{item.version}.put"
            (root / request_name).write_bytes(request)
            metadata_sha = hashlib.sha256(metadata_body).hexdigest()
            request_sha = hashlib.sha256(request).hexdigest()
            request_size = len(request)
        entries.append(
            {
                "archive": archive_name,
                "archive_sha256": hashlib.sha256(archive).hexdigest(),
                "archive_size": len(archive),
                "classification": item.classification,
                "metadata_sha256": metadata_sha,
                "name": item.name,
                "publish_order": item.publish_order,
                "request_body": request_name,
                "request_body_sha256": request_sha,
                "request_body_size": request_size,
                "version": item.version,
            }
        )
    inventory_sha = hashlib.sha256(
        (ROOT / "scripts/ci/cargo_release_inventory.toml").read_bytes()
    ).hexdigest()
    manifest = {
        "cargo_toolchain": inventory.candidate_toolchain,
        "inventory_sha256": inventory_sha,
        "packages": entries,
        "release_version": inventory.release_version,
        "schema_version": candidate.BUNDLE_SCHEMA_VERSION,
    }
    (root / candidate.BUNDLE_MANIFEST).write_bytes(
        candidate.canonical_json_bytes(manifest, newline=True)
    )
    return root, inventory


def test_inventory_pins_candidate_toolchain_and_exact_b8_registry_bytes() -> None:
    inventory = load_inventory()

    assert inventory.candidate_toolchain == "1.94.1"
    assert {value.name: value.registry_checksum for value in inventory.immutable_packages} == {
        "type-bridge-typedb-protocol-b8": (
            "e181af88e3742a13e35225c439f8a98968f014417b1814b18736743f6d799b16"
        ),
        "type-bridge-typedb-driver-b8": (
            "a2c4fe7da8c6c8d6a075bb667c916f8fceda416bbb844d0396f987cd48204d2e"
        ),
    }
    assert all(value.registry_checksum is None for value in inventory.first_party_packages)


def test_publish_frame_preserves_metadata_and_exact_archive_bytes() -> None:
    item = package("first-party", classification="public-first-party", order=1)
    archive = crate_bytes(item.name)
    files = candidate.safe_crate_files(archive, package=item)
    manifest = candidate._parse_toml(  # noqa: SLF001
        files[PurePosixPath("Cargo.toml")], label=item.name
    )
    metadata = candidate.publish_metadata_from_manifest(manifest, files)

    metadata_body, request = candidate.encode_publish_request(metadata, archive)
    decoded_metadata, decoded_archive = candidate.decode_publish_request(request)

    assert decoded_metadata == metadata_body
    assert decoded_archive == archive
    assert struct.unpack("<I", request[:4])[0] == len(metadata_body)
    assert metadata["deps"] == [
        {
            "default_features": True,
            "explicit_name_in_toml": None,
            "features": [],
            "kind": "build",
            "name": "build-helper",
            "optional": False,
            "registry": None,
            "target": "cfg(unix)",
            "version_req": "=2.0.0",
        },
        {
            "default_features": False,
            "explicit_name_in_toml": "alias",
            "features": ["serde"],
            "kind": "normal",
            "name": "actual-dependency",
            "optional": True,
            "registry": None,
            "target": None,
            "version_req": "^1",
        },
    ]


def test_candidate_bundle_binds_every_file_and_semantic_metadata(tmp_path: Path) -> None:
    root, inventory = write_bundle(tmp_path)
    accepted = candidate.validate_candidate_bundle(
        root,
        expected_release_version="1.2.3",
        inventory=inventory,
    )

    assert len(accepted.packages) == 2
    assert accepted.manifest_sha256 == hashlib.sha256(accepted.manifest.read_bytes()).hexdigest()

    archive = accepted.packages[0].archive
    archive.write_bytes(archive.read_bytes() + b"tamper")
    with pytest.raises(candidate.CandidateError, match="archive checksum binding failed"):
        candidate.validate_candidate_bundle(
            root,
            expected_release_version="1.2.3",
            inventory=inventory,
        )


def test_candidate_bundle_enforces_the_inclusive_crates_io_archive_ceiling(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    assert candidate.MAX_ARCHIVE_BYTES == 10 * 1024 * 1024
    root, inventory = write_bundle(tmp_path)
    payload = json.loads((root / candidate.BUNDLE_MANIFEST).read_text())
    fixture_ceiling = max(entry["archive_size"] for entry in payload["packages"])

    monkeypatch.setattr(candidate, "MAX_ARCHIVE_BYTES", fixture_ceiling)
    candidate.validate_candidate_bundle(
        root,
        expected_release_version="1.2.3",
        inventory=inventory,
    )

    monkeypatch.setattr(candidate, "MAX_ARCHIVE_BYTES", fixture_ceiling - 1)
    with pytest.raises(candidate.CandidateError, match="exceeds its byte budget"):
        candidate.validate_candidate_bundle(
            root,
            expected_release_version="1.2.3",
            inventory=inventory,
        )


def test_rehashed_request_cannot_disagree_with_packaged_manifest(tmp_path: Path) -> None:
    root, inventory = write_bundle(tmp_path)
    manifest_path = root / candidate.BUNDLE_MANIFEST
    payload = json.loads(manifest_path.read_text())
    entry = payload["packages"][0]
    request_path = root / entry["request_body"]
    metadata_body, archive = candidate.decode_publish_request(request_path.read_bytes())
    metadata = json.loads(metadata_body)
    metadata["description"] = "hostile but consistently rehashed"
    changed_metadata, changed_request = candidate.encode_publish_request(metadata, archive)
    request_path.write_bytes(changed_request)
    entry["metadata_sha256"] = hashlib.sha256(changed_metadata).hexdigest()
    entry["request_body_sha256"] = hashlib.sha256(changed_request).hexdigest()
    entry["request_body_size"] = len(changed_request)
    manifest_path.write_bytes(candidate.canonical_json_bytes(payload, newline=True))

    with pytest.raises(candidate.CandidateError, match="disagrees with its normalized archive"):
        candidate.validate_candidate_bundle(
            root,
            expected_release_version="1.2.3",
            inventory=inventory,
        )


def test_directory_source_checksum_binds_archive_and_each_file(tmp_path: Path) -> None:
    item = package("first-party", classification="public-first-party", order=1)
    archive = crate_bytes(item.name)
    source = tmp_path / "source"
    source.mkdir()

    root, files = candidate.stage_archive(source, package=item, archive=archive)
    checksum = json.loads((root / ".cargo-checksum.json").read_text())

    assert checksum["package"] == hashlib.sha256(archive).hexdigest()
    assert checksum["files"] == {
        path.as_posix(): hashlib.sha256(body).hexdigest() for path, body in files.items()
    }


def test_candidate_builder_is_patch_free_and_keeps_cargo_verification() -> None:
    source = (ROOT / "scripts/ci/cargo_release_candidate.py").read_text()

    assert '"vendor", "--locked", "--versioned-dirs"' in source
    assert '"package",' in source
    assert '"--locked",' in source
    assert '"--all-features",' in source
    assert "patch.crates-io" not in source
    assert "--no-verify" not in source
    assert "registry_checksum" in source


def test_cargo_metadata_isolates_archive_from_ancestor_workspace(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    (workspace / "Cargo.toml").write_text(
        "[workspace]\n"
        "members = []\n"
        'resolver = "3"\n\n'
        "[workspace.package]\n"
        'authors = ["ancestor workspace"]\n'
        'description = "must not affect packaged metadata"\n\n'
        "[workspace.lints.rust]\n"
        'unsafe_code = "forbid"\n'
    )
    temporary_root = workspace / "target"
    directory_source = temporary_root / "directory-source"
    directory_source.mkdir(parents=True)
    monkeypatch.setattr(candidate.tempfile, "tempdir", str(temporary_root))

    item = package("first-party", classification="public-first-party", order=1)
    archive = crate_bytes(item.name)
    staged_root, files = candidate.stage_archive(
        directory_source,
        package=item,
        archive=archive,
    )
    original = {
        path.relative_to(staged_root): path.read_bytes()
        for path in staged_root.rglob("*")
        if path.is_file()
    }
    config = temporary_root / "cargo-config.toml"
    config.write_text("")

    metadata = candidate._cargo_metadata_package(  # noqa: SLF001
        archive,
        package=item,
        cargo=("cargo", "+1.94.1"),
        config=config,
        environment=os.environ,
        runner=subprocess.run,
    )

    manifest = candidate._parse_toml(  # noqa: SLF001
        files[PurePosixPath("Cargo.toml")],
        label=f"{item.name} Cargo.toml",
    )
    assert candidate.publish_metadata_from_cargo(
        metadata,
        manifest=manifest,
        files=files,
    ) == candidate.publish_metadata_from_manifest(manifest, files)
    assert original == {
        path.relative_to(staged_root): path.read_bytes()
        for path in staged_root.rglob("*")
        if path.is_file()
    }
    assert not any(
        path.name.startswith("type-bridge-cargo-metadata-") for path in temporary_root.iterdir()
    )
