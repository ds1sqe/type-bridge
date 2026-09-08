"""Hostile tests for packaged Cargo external-consumer acceptance."""

from __future__ import annotations

import importlib.util
import io
import subprocess
import sys
import tarfile
import tomllib
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[3]
INVENTORY_MODULE = ROOT / "scripts/ci/cargo_release_inventory.py"
VALIDATOR_MODULE = ROOT / "scripts/ci/validate_cargo_external_consumers.py"
RELEASE_WORKFLOW = ROOT / ".github/workflows/release.yml"
VALIDATOR_COMMAND = "python scripts/ci/validate_cargo_external_consumers.py"


def load_module(name: str, path: Path) -> ModuleType:
    """Load one standalone CI helper without making scripts a package."""
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


inventory_module = load_module("cargo_release_inventory", INVENTORY_MODULE)
validator = load_module("validate_cargo_external_consumers", VALIDATOR_MODULE)


def write_archive(
    path: Path,
    *,
    package: Any,
    member_name: str | None = None,
    member_type: bytes | None = None,
    manifest: str | None = None,
) -> None:
    """Write one minimal synthetic ``.crate`` archive."""
    root = f"{package.name}-{package.version}"
    body = (
        manifest or f'[package]\nname = "{package.name}"\nversion = "{package.version}"\n'
    ).encode()
    with tarfile.open(path, mode="w:gz") as archive:
        root_info = tarfile.TarInfo(root)
        root_info.type = tarfile.DIRTYPE
        archive.addfile(root_info)
        info = tarfile.TarInfo(member_name or f"{root}/Cargo.toml")
        info.size = len(body)
        if member_type is not None:
            info.type = member_type
        archive.addfile(info, io.BytesIO(body))


def extracted_inventory(tmp_path: Path) -> dict[str, Any]:
    """Return synthetic extracted roots for the public inventory."""
    extracted: dict[str, Any] = {}
    for package in inventory_module.load_inventory().public_packages:
        root = tmp_path / f"{package.name}-{package.version}"
        root.mkdir(parents=True)
        (root / "Cargo.toml").write_text("[package]\n", encoding="utf-8")
        extracted[package.name] = validator.ExtractedPackage(
            package=package,
            archive=tmp_path / f"{package.name}-{package.version}.crate",
            root=root,
        )
    return extracted


def surface_metadata(
    manifest: Path,
    extracted: dict[str, Any],
) -> dict[str, Any]:
    """Return a complete synthetic external-consumer resolution."""
    inventory = inventory_module.load_inventory()
    consumer_id = "path+file:///consumer#type-bridge-packaged-surface-consumer@0.0.0"
    packages: list[dict[str, Any]] = [
        {
            "name": "type-bridge-packaged-surface-consumer",
            "version": "0.0.0",
            "id": consumer_id,
            "manifest_path": str(manifest),
            "dependencies": [
                {
                    "name": package.name,
                    "req": f"={package.version}",
                    "source": validator.CRATES_IO_SOURCE,
                    "path": None,
                }
                for package in inventory.first_party_packages
            ],
            "targets": [{"name": "consumer", "kind": ["bin"]}],
            "source": None,
        }
    ]
    nodes: list[dict[str, Any]] = [{"id": consumer_id, "features": []}]
    for package in inventory.public_packages:
        package_id = f"path+file:///{package.name}#{package.version}"
        kind = ["proc-macro"] if package.name == "type-bridge-orm-derive" else ["lib"]
        packages.append(
            {
                "name": package.name,
                "version": package.version,
                "id": package_id,
                "manifest_path": str(extracted[package.name].root / "Cargo.toml"),
                "dependencies": [],
                "targets": [{"name": package.name.replace("-", "_"), "kind": kind}],
                "source": None,
            }
        )
        nodes.append({"id": package_id, "features": []})
    return {"packages": packages, "resolve": {"nodes": nodes}}


def test_closed_archive_inventory_requires_all_19_exact_files(tmp_path: Path) -> None:
    inventory = inventory_module.load_inventory()
    expected = validator.expected_archive_names(inventory)
    for filename in expected:
        (tmp_path / filename).touch()

    assert validator.validate_archive_inventory(tmp_path, inventory) == expected

    missing = next(iter(expected))
    (tmp_path / missing).unlink()
    with pytest.raises(validator.ExternalConsumerError, match="inventory drifted"):
        validator.validate_archive_inventory(tmp_path, inventory)

    (tmp_path / missing).touch()
    (tmp_path / "unexpected-2.1.0.crate").touch()
    with pytest.raises(validator.ExternalConsumerError, match="unexpected"):
        validator.validate_archive_inventory(tmp_path, inventory)

    linked = tmp_path.parent / f"{tmp_path.name}-link"
    linked.symlink_to(tmp_path, target_is_directory=True)
    with pytest.raises(validator.ExternalConsumerError, match="linked or non-directory"):
        validator.validate_archive_inventory(linked, inventory)


def test_archive_extraction_is_bounded_and_rejects_unsafe_members(tmp_path: Path) -> None:
    package = inventory_module.load_inventory().first_party_packages[0]
    safe = tmp_path / f"{package.name}-{package.version}.crate"
    write_archive(safe, package=package)
    destination = tmp_path / "safe"

    extracted = validator.extract_archive(safe, package=package, destination=destination)
    assert (extracted.root / "Cargo.toml").is_file()

    unsafe = tmp_path / "unsafe.crate"
    write_archive(
        unsafe,
        package=package,
        member_name=f"{package.name}-{package.version}/../escape",
    )
    with pytest.raises(validator.ExternalConsumerError, match="unsafe path"):
        validator.extract_archive(
            unsafe,
            package=package,
            destination=tmp_path / "unsafe-output",
        )

    linked = tmp_path / "linked.crate"
    linked.symlink_to(safe)
    with pytest.raises(validator.ExternalConsumerError, match="linked or non-regular"):
        validator.extract_archive(
            linked,
            package=package,
            destination=tmp_path / "linked-output",
        )


@pytest.mark.parametrize(
    "dependency",
    (
        'demo = { version = "1", path = "../source" }',
        'demo = { version = "1", git = "https://example.invalid/demo" }',
        'demo = { package = "renamed" }',
    ),
)
def test_packaged_manifests_must_be_registry_form(dependency: str) -> None:
    package = inventory_module.load_inventory().first_party_packages[0]
    manifest = (
        f'[package]\nname = "{package.name}"\nversion = "{package.version}"\n'
        f"[dependencies]\n{dependency}\n"
    ).encode()

    with pytest.raises(validator.ExternalConsumerError, match="registry-form|registry version"):
        validator.validate_packaged_manifest(manifest, package=package)


def test_generated_consumers_have_only_exact_registry_declarations(tmp_path: Path) -> None:
    inventory = inventory_module.load_inventory()
    surface = validator.write_surface_consumer(tmp_path / "surface", inventory=inventory)
    manifest = tomllib.loads(surface.read_text(encoding="utf-8"))

    assert manifest["dependencies"] == {
        package.name: f"={package.version}" for package in inventory.first_party_packages
    }
    assert len(manifest["dependencies"]) == 17
    assert "patch" not in manifest

    no_default = validator.write_server_consumer(
        tmp_path / "server-no-default",
        version=inventory.release_version,
        features=(),
    )
    v2_only = validator.write_server_consumer(
        tmp_path / "server-v2-only",
        version=inventory.release_version,
        features=("v2-query",),
    )
    assert tomllib.loads(no_default.read_text())["dependencies"]["type-bridge-server"] == {
        "version": "=2.1.0",
        "default-features": False,
    }
    assert tomllib.loads(v2_only.read_text())["dependencies"]["type-bridge-server"] == {
        "version": "=2.1.0",
        "default-features": False,
        "features": ["v2-query"],
    }


def test_only_temporary_cargo_config_patches_exact_extracted_roots(tmp_path: Path) -> None:
    extracted = extracted_inventory(tmp_path / "archives")
    root = tmp_path / "work"
    config = validator.write_patch_config(root, extracted)
    patches = tomllib.loads(config.read_text(encoding="utf-8"))["patch"]["crates-io"]

    assert set(patches) == set(extracted)
    assert len(patches) == 19
    for name, specification in patches.items():
        assert specification == {"path": str(extracted[name].root.resolve())}


def test_metadata_binds_all_public_packages_to_extracted_archives(tmp_path: Path) -> None:
    inventory = inventory_module.load_inventory()
    extracted = extracted_inventory(tmp_path / "archives")
    manifest = validator.write_surface_consumer(tmp_path / "consumer", inventory=inventory)
    metadata = surface_metadata(manifest, extracted)

    library_names = validator.validate_metadata(
        metadata,
        consumer_manifest=manifest,
        direct_dependencies={
            package.name: package.version for package in inventory.first_party_packages
        },
        required_packages=inventory.public_packages,
        extracted=extracted,
    )
    validator.write_surface_source(manifest.parent, library_names)

    source = (manifest.parent / "src/main.rs").read_text(encoding="utf-8")
    assert len(library_names) == 17
    assert source.count("use ::") == 17


def test_metadata_rejects_source_tree_or_non_registry_resolution(tmp_path: Path) -> None:
    inventory = inventory_module.load_inventory()
    extracted = extracted_inventory(tmp_path / "archives")
    manifest = validator.write_surface_consumer(tmp_path / "consumer", inventory=inventory)
    metadata = surface_metadata(manifest, extracted)
    consumer = metadata["packages"][0]
    consumer["dependencies"][0]["source"] = None

    with pytest.raises(validator.ExternalConsumerError, match="not registry-form"):
        validator.validate_metadata(
            metadata,
            consumer_manifest=manifest,
            direct_dependencies={
                package.name: package.version for package in inventory.first_party_packages
            },
            required_packages=inventory.public_packages,
            extracted=extracted,
        )

    metadata = surface_metadata(manifest, extracted)
    first_party = inventory.first_party_packages[0]
    resolved = next(
        package for package in metadata["packages"] if package["name"] == first_party.name
    )
    resolved["manifest_path"] = str(ROOT / "type-bridge-core" / first_party.manifest)
    with pytest.raises(validator.ExternalConsumerError, match="outside its extracted archive"):
        validator.validate_metadata(
            metadata,
            consumer_manifest=manifest,
            direct_dependencies={
                package.name: package.version for package in inventory.first_party_packages
            },
            required_packages=inventory.public_packages,
            extracted=extracted,
        )


@pytest.mark.parametrize(
    ("features", "expected_v2", "accepted"),
    (
        ([], False, True),
        (["v2-query", "axum-transport"], True, True),
        (["default", "band8", "band9", "typedb", "v2-query"], True, False),
        ([], True, False),
    ),
)
def test_server_feature_probes_are_isolated(
    features: list[str], expected_v2: bool, accepted: bool
) -> None:
    package_id = "path+file:///server#type-bridge-server@2.1.0"
    metadata = {
        "packages": [
            {
                "name": "type-bridge-server",
                "version": "2.1.0",
                "id": package_id,
            }
        ],
        "resolve": {"nodes": [{"id": package_id, "features": features}]},
    }

    if accepted:
        validator.validate_server_features(metadata, version="2.1.0", expected_v2=expected_v2)
    else:
        with pytest.raises(validator.ExternalConsumerError):
            validator.validate_server_features(
                metadata,
                version="2.1.0",
                expected_v2=expected_v2,
            )


def test_installed_binary_version_must_be_exact(tmp_path: Path) -> None:
    commands: list[list[str]] = []

    def fake_runner(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        commands.append(command)
        if command[-1] == "--version":
            return subprocess.CompletedProcess(command, 0, "type-bridge 2.0.0\n", "")
        return subprocess.CompletedProcess(command, 0, "", "")

    with pytest.raises(validator.ExternalConsumerError, match="version output drifted"):
        validator._install_and_run_binary(
            cargo=("cargo", "+1.94.1"),
            package_root=tmp_path / "type-bridge-cli-2.1.0",
            binary="type-bridge",
            expected_version="2.1.0",
            install_root=tmp_path / "installed",
            work_root=tmp_path,
            environment={},
            runner=fake_runner,
        )

    assert commands[0][2:5] == ["install", "--locked", "--debug"]
    assert "--path" in commands[0]
    assert "publish" not in commands[0]


def test_release_wires_external_consumers_between_archive_and_identity_gates() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    job = workflow.split("  validate-release-identity:\n", maxsplit=1)[1].split(
        "\n  channel-preflight:", maxsplit=1
    )[0]
    archive = "python scripts/ci/validate_rust_release_artifacts.py"
    identity = "python scripts/ci/validate_release_identity.py"

    assert job.count(VALIDATOR_COMMAND) == 1
    assert job.index(archive) < job.index(VALIDATOR_COMMAND) < job.index(identity)
    assert "--candidate-bundle type-bridge-core/target/cargo-release-candidate" in job
    assert (
        '--expected-manifest-sha256 "${{ steps.cargo-candidate.outputs.manifest_sha256 }}"' in job
    )
    assert '--expected-release-version "$RELEASE_VERSION"' in job
    assert "--toolchain 1.94.1" in job

    source = VALIDATOR_MODULE.read_text(encoding="utf-8")
    assert '"metadata"' in source
    assert '"check"' in source
    assert '"install"' in source
    assert '"publish",' not in source
    assert "member.isfile()" in source
    assert "extractall" not in source
