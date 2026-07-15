"""Hostile tests for the pre-publication release identity gate."""

from __future__ import annotations

import importlib.util
import shutil
import sys
import tomllib
from pathlib import Path
from types import ModuleType

import pytest

ROOT = Path(__file__).resolve().parents[3]
VALIDATOR_PATH = ROOT / "scripts/ci/validate_release_identity.py"


def load_module(name: str, path: Path) -> ModuleType:
    """Load one standalone CI validator without making scripts a package."""
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


validator = load_module("validate_release_identity", VALIDATOR_PATH)


def validate(**overrides: object) -> dict[str, object]:
    """Run the gate against repository authorities by default."""
    arguments: dict[str, object] = {
        "tag": "v1.5.8",
        "workspace_manifest": ROOT / "type-bridge-core/Cargo.toml",
        "root_python_manifest": ROOT / "pyproject.toml",
        "core_python_manifest": ROOT / "type-bridge-core/pyproject.toml",
        "node_package": ROOT / "type-bridge-core/crates/node/package.json",
        "release_workflow": ROOT / ".github/workflows/release.yml",
    }
    arguments.update(overrides)
    return validator.validate_release_identity(**arguments)


def copy_workspace_manifests(tmp_path: Path) -> Path:
    """Copy the workspace manifest graph without copying source files."""
    source_root = ROOT / "type-bridge-core"
    target_root = tmp_path / "type-bridge-core"
    workspace_payload = tomllib.loads((source_root / "Cargo.toml").read_text())
    target_root.mkdir()
    shutil.copyfile(source_root / "Cargo.toml", target_root / "Cargo.toml")
    for member in workspace_payload["workspace"]["members"]:
        target = target_root / member
        target.mkdir(parents=True)
        shutil.copyfile(source_root / member / "Cargo.toml", target / "Cargo.toml")
    return target_root / "Cargo.toml"


def test_repository_release_identity_is_complete() -> None:
    report = validate()

    assert report["status"] == "ok"
    assert report["version"] == "1.5.8"
    assert report["published_crates"] == list(validator.PUBLISHED_CRATES)
    assert report["unpublished_crates"] == [
        "type-bridge-core",
        "type-bridge-migration",
        "type-bridge-node",
        "type-bridge-toml-transpiler",
    ]
    cargo_packages = report["cargo_packages"]
    assert isinstance(cargo_packages, dict)
    assert all(
        isinstance(package, str) and isinstance(version, str)
        for package, version in cargo_packages.items()
    )
    assert set(cargo_packages) == {
        "type-bridge-core",
        "type-bridge-core-lib",
        "type-bridge-migration",
        "type-bridge-node",
        "type-bridge-orm",
        "type-bridge-orm-derive",
        "type-bridge-server",
        "type-bridge-toml-transpiler",
        "type-bridge-typedb-driver-b7",
        "type-bridge-typedb-driver-b9",
        "type-bridge-typedb-protocol-b7",
        "type-bridge-typedb-protocol-b9",
        "type-bridge-typedb-runtime",
    }


def test_release_tag_must_match_all_public_manifests() -> None:
    with pytest.raises(validator.ValidationError, match="root Python manifest"):
        validate(tag="v9.9.9")


def test_first_party_cargo_version_drift_hard_fails(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "crates/orm/Cargo.toml"
    manifest.write_text(manifest.read_text().replace('version = "1.5.8"', 'version = "1.5.7"', 1))

    with pytest.raises(validator.ValidationError, match="type-bridge-orm version"):
        validate(workspace_manifest=workspace)


def test_unpublished_first_party_version_drift_hard_fails(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "crates/migration/Cargo.toml"
    manifest.write_text(manifest.read_text().replace('version = "1.5.8"', 'version = "1.5.7"', 1))

    with pytest.raises(validator.ValidationError, match="type-bridge-migration version"):
        validate(workspace_manifest=workspace)


def test_vendor_identity_drift_hard_fails(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "vendor/typedb-driver-b7/Cargo.toml"
    manifest.write_text(manifest.read_text().replace('version = "3.8.1"', 'version = "3.8.2"', 1))

    with pytest.raises(validator.ValidationError, match="typedb-driver-b7 version"):
        validate(workspace_manifest=workspace)


def test_band9_vendor_identity_drift_hard_fails(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "vendor/typedb-driver-b9/Cargo.toml"
    manifest.write_text(manifest.read_text().replace('version = "3.12.0"', 'version = "3.12.1"', 1))

    with pytest.raises(validator.ValidationError, match="typedb-driver-b9 version"):
        validate(workspace_manifest=workspace)


def test_publish_sequence_must_be_complete_and_ordered(tmp_path: Path) -> None:
    workflow = tmp_path / "release.yml"
    workflow.write_text(
        (ROOT / ".github/workflows/release.yml")
        .read_text()
        .replace(
            '"$RUNNER_TEMP/publish-crate.sh type-bridge-server"',
            'echo "server publish accidentally omitted"',
        )
    )

    with pytest.raises(validator.ValidationError, match="publish sequence"):
        validate(release_workflow=workflow)


def test_internal_crate_cannot_be_implicitly_publishable(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "crates/python/Cargo.toml"
    manifest.write_text(manifest.read_text().replace("publish = false\n", "", 1))

    with pytest.raises(validator.ValidationError, match="publishable package set"):
        validate(workspace_manifest=workspace)


def test_planned_public_crate_cannot_be_marked_unpublished(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "crates/orm/Cargo.toml"
    manifest.write_text(
        manifest.read_text().replace(
            "authors.workspace = true\n", "authors.workspace = true\npublish = false\n", 1
        )
    )

    with pytest.raises(validator.ValidationError, match="publishable package set"):
        validate(workspace_manifest=workspace)


def test_unpinned_publishable_vendor_crate_hard_fails(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "vendor/typedb-driver-b7/Cargo.toml"
    manifest.write_text(
        manifest.read_text().replace(
            'name = "type-bridge-typedb-driver-b7"',
            'name = "hostile-unpinned-vendor"',
            1,
        )
    )

    with pytest.raises(validator.ValidationError, match="Unpinned workspace vendor"):
        validate(workspace_manifest=workspace)
