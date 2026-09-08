"""Hostile tests for the pre-publication release identity gate."""

from __future__ import annotations

import hashlib
import importlib.util
import io
import json
import shutil
import sys
import tarfile
import tomllib
from dataclasses import replace
from pathlib import Path
from types import ModuleType
from typing import Any

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
SYNTHETIC_DRIVER_NAME = "synthetic-driver"
SYNTHETIC_PROTOCOL_NAME = "synthetic-protocol"
validator.LEGACY_VENDOR_DESCRIPTIONS[SYNTHETIC_DRIVER_NAME] = (
    "Synthetic renamed TypeDB driver used by provenance tests"
)
validator.LEGACY_VENDOR_DESCRIPTIONS[SYNTHETIC_PROTOCOL_NAME] = (
    "Synthetic renamed TypeDB protocol used by provenance tests"
)


def validate(**overrides: object) -> dict[str, Any]:
    """Run the gate against repository authorities by default."""
    arguments: dict[str, object] = {
        "tag": "v2.1.0",
        "artifact_contract": validator.ARTIFACT_CONTRACT_CARGO_INCLUSIVE,
        "release_channel": validator.RELEASE_CHANNEL_STABLE,
        "workspace_manifest": ROOT / "type-bridge-core/Cargo.toml",
        "root_python_manifest": ROOT / "pyproject.toml",
        "core_python_manifest": ROOT / "type-bridge-core/pyproject.toml",
        "node_package": ROOT / "type-bridge-core/crates/node/package.json",
        "release_workflow": ROOT / ".github/workflows/release.yml",
    }
    arguments.update(overrides)
    return validator.validate_release_identity(**arguments)


def copy_root_python_authorities(tmp_path: Path) -> tuple[Path, Path]:
    """Copy the facade manifest and import-visible version authority."""
    manifest = tmp_path / "pyproject.toml"
    package_init = tmp_path / "type_bridge/__init__.py"
    package_init.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(ROOT / "pyproject.toml", manifest)
    shutil.copyfile(ROOT / "type_bridge/__init__.py", package_init)
    return manifest, package_init


def copy_workspace_manifests(tmp_path: Path) -> Path:
    """Copy the manifest graph plus source-backed release identity constants."""
    source_root = ROOT / "type-bridge-core"
    target_root = tmp_path / "type-bridge-core"
    workspace_payload = tomllib.loads((source_root / "Cargo.toml").read_text())
    target_root.mkdir()
    shutil.copyfile(source_root / "Cargo.toml", target_root / "Cargo.toml")
    shutil.copyfile(source_root / "Cargo.lock", target_root / "Cargo.lock")
    shutil.copyfile(source_root.parent / "LICENSE", target_root.parent / "LICENSE")
    shutil.copyfile(source_root / "LICENSE", target_root / "LICENSE")
    for member in workspace_payload["workspace"]["members"]:
        target = target_root / member
        target.mkdir(parents=True)
        shutil.copyfile(source_root / member / "Cargo.toml", target / "Cargo.toml")
    for relative in validator.HISTORICAL_BAND9_PACKAGES:
        target = target_root / relative
        target.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source_root / relative / "Cargo.toml", target / "Cargo.toml")
        shutil.copyfile(source_root / relative / "README.md", target / "README.md")
        shutil.copyfile(source_root / relative / "LICENSE", target / "LICENSE")
    for component in validator.LEGACY_TYPEDB_COMPONENTS:
        relative = Path(component.vendor_directory)
        shutil.copyfile(
            source_root / relative / "LICENSE",
            target_root / relative / "LICENSE",
        )
    runtime_source = target_root / "crates/typedb-runtime/src"
    runtime_source.mkdir()
    shutil.copyfile(
        source_root / "crates/typedb-runtime/src/lib.rs",
        runtime_source / "lib.rs",
    )
    python_notice = target_root / "python/type_bridge_core/THIRD_PARTY_NOTICES.md"
    python_notice.parent.mkdir(parents=True)
    shutil.copyfile(
        source_root / "python/type_bridge_core/THIRD_PARTY_NOTICES.md",
        python_notice,
    )
    shutil.copyfile(
        source_root / "crates/node/THIRD_PARTY_NOTICES.md",
        target_root / "crates/node/THIRD_PARTY_NOTICES.md",
    )
    shutil.copyfile(source_root / "vendor/README.md", target_root / "vendor/README.md")
    return target_root / "Cargo.toml"


def copy_release_graph_authorities(tmp_path: Path) -> tuple[Path, Path, Path]:
    """Copy the ordinary workflow and exact Cargo candidate authorities."""
    workflow = tmp_path / ".github/workflows/release.yml"
    builder = tmp_path / "scripts/ci/cargo_release_candidate.py"
    publisher = tmp_path / "scripts/ci/publish_cargo_release_candidate.py"
    workflow.parent.mkdir(parents=True)
    builder.parent.mkdir(parents=True)
    shutil.copyfile(ROOT / ".github/workflows/release.yml", workflow)
    shutil.copyfile(ROOT / "scripts/ci/cargo_release_candidate.py", builder)
    shutil.copyfile(ROOT / "scripts/ci/publish_cargo_release_candidate.py", publisher)
    return workflow, builder, publisher


def replace_lock_package_text(
    workspace: Path,
    package: str,
    old: str,
    new: str,
) -> None:
    """Replace text only inside one copied Cargo.lock package block."""
    lock = workspace.parent / "Cargo.lock"
    source = lock.read_text()
    marker = f'[[package]]\nname = "{package}"'
    prefix, found, remainder = source.partition(marker)
    assert found
    block, next_marker, suffix = remainder.partition("\n[[package]]")
    assert old in block
    block = block.replace(old, new, 1)
    lock.write_text(prefix + marker + block + next_marker + suffix)


def replace_both_native_notices(workspace: Path, old: str, new: str) -> None:
    """Apply one hostile mutation without tripping byte-parity first."""
    for relative in (
        "python/type_bridge_core/THIRD_PARTY_NOTICES.md",
        "crates/node/THIRD_PARTY_NOTICES.md",
    ):
        notice = workspace.parent / relative
        source = notice.read_text()
        assert old in source
        notice.write_text(source.replace(old, new, 1))


def test_repository_cargo_inclusive_stable_identity_is_complete() -> None:
    report = validate()

    assert report["status"] == "ok"
    assert report["artifact_contract"] == "cargo-inclusive"
    assert report["crates_io_mutation"] is True
    assert report["release_channel"] == "stable"
    assert report["tag"] == "v2.1.0"
    assert report["version"] == "2.1.0"
    assert report["python_version"] == "2.1.0"
    assert report["python_core_requirement"] == "type-bridge-core==2.1.0"
    assert report["python_package_version"] == "2.1.0"
    assert report["node_package_lock_version"] == "2.1.0"
    assert report["server_oci_stable_aliases"] == ["2.1", "2", "latest"]
    assert report["server_oci_recovery_aliases"] == ["2.0", "2", "latest"]
    assert report["server_oci_stable_signing_identity"].endswith("release.yml@refs/tags/v2.1.0")
    assert report["server_oci_recovery_signing_identity"].endswith("release.yml@refs/heads/master")
    assert set(report["cargo_licenses"].values()) == {
        "MIT",
        "Apache-2.0",
        "MPL-2.0",
    }
    assert "typedb_runtime_band7_driver_pin" not in report
    assert report["typedb_runtime_driver_pin"] == "3.11.5"
    assert report["typedb_runtime_band9_driver_pin"] == "3.12.1"
    assert report["typedb_runtime_band9_components"] == {
        "typedb-driver": {
            "checksum": "b7daa941ffe0f6e6cb17e2e831e13b338a9db23551414f877c7fb64ce05f9f46",
            "source": "registry+https://github.com/rust-lang/crates.io-index",
            "version": "3.12.1",
        },
        "typedb-protocol": {
            "checksum": "01f6b7eb813a853349ff22f385c120c61d04d4648318c92072e7e04dd81cdc3f",
            "source": "registry+https://github.com/rust-lang/crates.io-index",
            "version": "3.12.0",
        },
    }
    assert report["historical_band9_quarantine"] == [
        "type-bridge-typedb-driver-b9",
        "type-bridge-typedb-protocol-b9",
    ]


def test_repository_cargo_graph_is_complete_and_ordered() -> None:
    report = validate()

    assert report["status"] == "ok"
    assert report["crates_io_mutation"] is True
    assert report["cargo_publication_plan"] == list(validator.EXPECTED_NEW_CRATES)
    assert report["legacy_vendor_identities"] == [
        "|".join(
            (
                component.manifest_path,
                component.downstream_name,
                component.downstream_version,
                component.license,
            )
        )
        for component in validator.LEGACY_TYPEDB_COMPONENTS
    ]
    assert report["cargo_manifest_publishable_crates"] == list(validator.PUBLISHED_CRATES)
    assert report["unpublished_v2_crates"] == []
    assert validator.PREEXISTING_CRATES == (
        "type-bridge-typedb-protocol-b8",
        "type-bridge-typedb-driver-b8",
    )
    assert "type-bridge-typedb-protocol-b8" in validator.PACKAGED_RELEASE_CRATES
    assert "type-bridge-typedb-driver-b8" in validator.PACKAGED_RELEASE_CRATES
    assert "type-bridge-typedb-protocol-b8" not in validator.EXPECTED_NEW_CRATES
    assert "type-bridge-typedb-driver-b8" not in validator.EXPECTED_NEW_CRATES
    assert not set(validator.PREEXISTING_CRATES) & set(validator.EXPECTED_NEW_CRATES)
    assert report["unpublished_crates"] == [
        "type-bridge-core",
        "type-bridge-node",
    ]
    dependency_order = report["cargo_manifest_dependency_order"]
    assert isinstance(dependency_order, dict)
    assert dependency_order["type-bridge-core-lib"] == ["type-bridge-contract"]
    assert dependency_order["type-bridge-migration"] == [
        "type-bridge-contract",
        "type-bridge-core-lib",
        "type-bridge-schema-compat",
        "type-bridge-orm",
    ]
    assert {
        (entry["package"], entry["unpublished_dependency"])
        for entry in report["rust_publication_blockers"]
    } == validator.KNOWN_PUBLICATION_BLOCKER_EDGES
    cargo_packages = report["cargo_packages"]
    assert isinstance(cargo_packages, dict)
    assert all(
        isinstance(package, str) and isinstance(version, str)
        for package, version in cargo_packages.items()
    )
    assert set(cargo_packages) == {
        "type-bridge",
        "type-bridge-cli",
        "type-bridge-contract",
        "type-bridge-core",
        "type-bridge-core-lib",
        "type-bridge-migration",
        "type-bridge-node",
        "type-bridge-orm",
        "type-bridge-orm-derive",
        "type-bridge-query",
        "type-bridge-schema",
        "type-bridge-schema-codegen",
        "type-bridge-schema-compat",
        "type-bridge-schema-migration",
        "type-bridge-schema-migration-typedb",
        "type-bridge-server",
        "type-bridge-toml-transpiler",
        "type-bridge-typedb-driver-b8",
        "type-bridge-typedb-protocol-b8",
        "type-bridge-workspace",
        "type-bridge-typedb-runtime",
    }


@pytest.mark.parametrize(
    ("directory", "package_name"),
    [
        ("contract", "type-bridge-contract"),
        ("schema", "type-bridge-schema"),
        ("query", "type-bridge-query"),
        ("schema-migration", "type-bridge-schema-migration"),
        ("schema-migration-typedb", "type-bridge-schema-migration-typedb"),
        ("schema-codegen", "type-bridge-schema-codegen"),
        ("schema-compat", "type-bridge-schema-compat"),
        ("workspace", "type-bridge-workspace"),
        ("cli", "type-bridge-cli"),
    ],
)
def test_v2_crate_manifest_is_first_party_and_crates_io_publishable(
    directory: str,
    package_name: str,
) -> None:
    manifest = tomllib.loads(
        (ROOT / "type-bridge-core/crates" / directory / "Cargo.toml").read_text()
    )["package"]

    assert manifest["name"] == package_name
    assert manifest["version"] == "2.1.0"
    assert manifest["publish"] == ["crates-io"]


@pytest.mark.parametrize(
    ("directory", "package_name"),
    [
        ("python", "type-bridge-core"),
        ("node", "type-bridge-node"),
    ],
)
def test_binding_crate_manifest_remains_first_party_and_unpublished(
    directory: str,
    package_name: str,
) -> None:
    manifest = tomllib.loads(
        (ROOT / "type-bridge-core/crates" / directory / "Cargo.toml").read_text()
    )["package"]

    assert manifest["name"] == package_name
    assert manifest["version"] == "2.1.0"
    assert manifest["publish"] is False


def test_repository_workflow_uses_the_cargo_inclusive_contract() -> None:
    workflow = ROOT / ".github/workflows/release.yml"
    source = workflow.read_text()

    assert "validate_historical_band9_registry.py" in source
    assert "validate_latest_typedb_driver_pin.py" in source
    assert "--artifact-contract cargo-inclusive" in source
    validator.workflow_preflight_sequences(workflow)
    validator.workflow_registry_preflight_sequences(workflow)


def test_release_workflow_must_bind_the_centralized_cargo_graph(tmp_path: Path) -> None:
    workflow, _, _ = copy_release_graph_authorities(tmp_path)
    workflow.write_text(
        workflow.read_text().replace(
            "python scripts/ci/publish_cargo_release_candidate.py publish",
            "echo cargo publish omitted",
            1,
        )
    )

    with pytest.raises(validator.ValidationError, match="exactly once with publish"):
        validator.workflow_publish_sequence(workflow)


def test_centralized_cargo_publish_order_cannot_drift(tmp_path: Path) -> None:
    workflow, _, publisher = copy_release_graph_authorities(tmp_path)
    source = publisher.read_text()
    marker = "for candidate in bundle.packages:"
    assert source.count(marker) == 1
    publisher.write_text(source.replace(marker, "for candidate in reversed(bundle.packages):", 1))

    with pytest.raises(validator.ValidationError, match="publication loop is malformed"):
        validate(release_workflow=workflow)


def test_centralized_cargo_preflight_loop_cannot_be_bypassed(tmp_path: Path) -> None:
    workflow, builder, _ = copy_release_graph_authorities(tmp_path)
    source = builder.read_text()
    marker = "for package in inventory.public_packages:"
    assert source.count(marker) == 1
    builder.write_text(source.replace(marker, "for package in ():", 1))

    with pytest.raises(validator.ValidationError, match="staged package loop is malformed"):
        validator.workflow_preflight_sequences(workflow)


@pytest.mark.parametrize(
    ("old", "new", "message"),
    [
        (
            "tool: cargo-about@0.9.1",
            "tool: cargo-about@latest",
            "pinned cargo-about 0.9.1",
        ),
        (
            "python scripts/ci/generate_native_dependency_notice.py --check",
            "echo native notice check omitted",
            "freshness gate",
        ),
        (
            "python scripts/ci/validate_cargo_rustdoc.py",
            "echo public Cargo rustdoc gate omitted",
            "Cargo rustdoc gate",
        ),
        (
            "python scripts/ci/cargo_release_candidate.py build",
            "echo Cargo graph packaging omitted",
            "package the Cargo graph",
        ),
        (
            "python scripts/ci/validate_rust_release_artifacts.py",
            "echo Rust archive gate omitted",
            "Rust archive-content gate",
        ),
    ],
)
def test_release_workflow_requires_ordered_native_artifact_gates(
    tmp_path: Path,
    old: str,
    new: str,
    message: str,
) -> None:
    workflow = tmp_path / "release.yml"
    source = (ROOT / ".github/workflows/release.yml").read_text()
    assert source.count(old) == 1
    workflow.write_text(source.replace(old, new, 1))

    with pytest.raises(validator.ValidationError, match=message):
        validator.validate_native_notice_workflow(workflow)


@pytest.mark.parametrize(
    ("old", "new"),
    [
        (
            "inputs.release_channel == 'recovery' && '2.0' || '2.1' }}",
            "inputs.release_channel == 'recovery' && '2.0' || '2.0' }}",
        ),
        (
            'for alias in "$SERVER_OCI_MINOR_ALIAS" 2 latest; do',
            "for alias in 2.0 2 latest; do",
        ),
        (
            "release.yml@refs/tags/v2[.]1[.]0$'",
            "release.yml@refs/tags/v2[.]0[.]0$'",
        ),
        (
            '"aliases": [os.environ["SERVER_OCI_MINOR_ALIAS"], "2", "latest"],',
            '"aliases": ["2.0", "2", "latest"],',
        ),
    ],
)
def test_server_oci_channel_identity_drift_hard_fails(
    tmp_path: Path,
    old: str,
    new: str,
) -> None:
    workflow = tmp_path / "release.yml"
    source = (ROOT / ".github/workflows/release.yml").read_text()
    assert source.count(old) == 1
    workflow.write_text(source.replace(old, new, 1))

    with pytest.raises(
        validator.ValidationError,
        match="release/OCI channel identities|OCI minor-alias selector",
    ):
        validator.validate_server_oci_release_channels(workflow)


def test_release_tag_freeze_and_publisher_rechecks_cannot_be_bypassed(tmp_path: Path) -> None:
    workflow = tmp_path / "release.yml"
    source = (ROOT / ".github/workflows/release.yml").read_text()
    node = validator._release_workflow_job(source, "publish-node-npm")
    check = '          test "$tag_object" = "$EXPECTED_RELEASE_TAG_OBJECT"\n'
    assert node.count(check) == 1
    workflow.write_text(source.replace(node, node.replace(check, "", 1), 1))

    with pytest.raises(validator.ValidationError, match="publish-node-npm tag-object equality"):
        validator.validate_server_oci_release_channels(workflow)


def test_recovery_cosign_identity_cannot_be_broadened(tmp_path: Path) -> None:
    workflow = tmp_path / "release.yml"
    source = (ROOT / ".github/workflows/release.yml").read_text()
    exact = "release.yml@refs/heads/master$"
    assert source.count(exact) == 1
    workflow.write_text(source.replace(exact, "release.yml@refs/heads/.*$", 1))

    with pytest.raises(validator.ValidationError, match="Cosign identities"):
        validator.validate_server_oci_release_channels(workflow)


def test_cargo_publication_must_follow_the_first_npm_mutation(tmp_path: Path) -> None:
    workflow = tmp_path / "release.yml"
    source = (ROOT / ".github/workflows/release.yml").read_text()
    crates = validator._release_workflow_job(source, "publish-crates")
    guarded = "    needs: [release-tag-preflight, publish-node-npm, validate-release-identity]\n"
    assert guarded in crates
    workflow.write_text(
        source.replace(
            crates,
            crates.replace(guarded, "    needs: release-tag-preflight\n", 1),
            1,
        )
    )

    with pytest.raises(validator.ValidationError, match="npm-first Cargo publication"):
        validator.validate_server_oci_release_channels(workflow)


def test_publisher_without_needs_is_reported_as_validation_failure(tmp_path: Path) -> None:
    workflow = tmp_path / "release.yml"
    source = (ROOT / ".github/workflows/release.yml").read_text()
    crates = validator._release_workflow_job(source, "publish-crates")
    needs = "    needs: [release-tag-preflight, publish-node-npm, validate-release-identity]\n"
    assert crates.count(needs) == 1
    workflow.write_text(source.replace(crates, crates.replace(needs, "", 1), 1))

    with pytest.raises(validator.ValidationError, match="frozen-tag dependency"):
        validator.validate_server_oci_release_channels(workflow)


def test_v2_crate_cannot_be_marked_unpublished(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "crates/contract/Cargo.toml"
    manifest.write_text(
        manifest.read_text().replace('publish = ["crates-io"]', "publish = false", 1)
    )

    with pytest.raises(validator.ValidationError, match="publishable package set"):
        validate(workspace_manifest=workspace)


def test_repository_driver_components_are_packaging_only() -> None:
    drivers = tuple(
        component
        for component in validator.LEGACY_TYPEDB_COMPONENTS
        if component.upstream_name == "typedb-driver"
    )

    assert {(component.band, component.downstream_version) for component in drivers} == {
        (8, "3.11.5"),
    }
    for component in drivers:
        assert component.downstream_version == component.upstream_version
        assert component.license_status == (
            "Apache-2.0 namespaced packaging-only package; source behavior unchanged"
            "; owner-authorized for TypeBridge Cargo distribution"
        )
        disclosure = validator.legacy_vendor_readme_disclosure(component)
        assert b"behavioral change" not in disclosure
        assert b"transaction close" not in disclosure


def test_historical_band9_quarantine_docs_state_external_compatibility_policy() -> None:
    for relative in validator.HISTORICAL_BAND9_PACKAGES:
        readme = " ".join((ROOT / "type-bridge-core" / relative / "README.md").read_text().split())
        assert "not a current/2.0 release input" in readme
        assert "type-bridge-typedb-driver-b9@3.12.0" in readme
        assert "type-bridge-typedb-protocol-b9@3.12.0" in readme
        assert "must remain non-yanked" in readme
        assert "must never republish or" in readme
        assert "yank them" in readme


@pytest.mark.parametrize("relative", validator.HISTORICAL_BAND9_PACKAGES)
def test_historical_band9_package_must_remain_publish_false(
    tmp_path: Path,
    relative: str,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / relative / "Cargo.toml"
    manifest.write_text(manifest.read_text().replace("publish = false", "publish = true", 1))

    with pytest.raises(validator.ValidationError, match="must remain publish=false"):
        validate(workspace_manifest=workspace)


@pytest.mark.parametrize("relative", validator.HISTORICAL_BAND9_PACKAGES)
def test_historical_band9_package_must_retain_quarantine_wording(
    tmp_path: Path,
    relative: str,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / relative / "Cargo.toml"
    manifest.write_text(
        manifest.read_text().replace(
            "Historical quarantined",
            "Republished compatibility",
            1,
        )
    )

    with pytest.raises(validator.ValidationError, match="quarantine warning"):
        validate(workspace_manifest=workspace)


@pytest.mark.parametrize("relative", validator.HISTORICAL_BAND9_PACKAGES)
def test_historical_band9_readme_must_forbid_consumption(
    tmp_path: Path,
    relative: str,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    readme = workspace.parent / relative / "README.md"
    readme.write_text(readme.read_text().replace("forbidden for", "available for", 1))

    with pytest.raises(validator.ValidationError, match="README.*missing"):
        validate(workspace_manifest=workspace)


@pytest.mark.parametrize("relative", validator.HISTORICAL_BAND9_PACKAGES)
def test_historical_band9_package_must_remain_explicitly_excluded(
    tmp_path: Path,
    relative: str,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    workspace.write_text(workspace.read_text().replace(f'    "{relative}",\n', "", 1))

    with pytest.raises(validator.ValidationError, match="explicitly excluded"):
        validate(workspace_manifest=workspace)


@pytest.mark.parametrize("relative", validator.HISTORICAL_BAND9_PACKAGES)
def test_historical_band9_package_cannot_reenter_workspace_members(
    tmp_path: Path,
    relative: str,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    workspace.write_text(
        workspace.read_text().replace(
            "members = [\n",
            f'members = [\n    "{relative}",\n',
            1,
        )
    )

    with pytest.raises(validator.ValidationError, match="must not be workspace members"):
        validate(workspace_manifest=workspace)


def test_workspace_dependency_cannot_rename_a_historical_band9_package(
    tmp_path: Path,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    with workspace.open("a", encoding="utf-8") as manifest:
        manifest.write(
            "\n[workspace.dependencies]\n"
            'retired-driver = { package = "type-bridge-typedb-driver-b9", '
            'path = "vendor/typedb-driver-b9" }\n'
        )

    with pytest.raises(validator.ValidationError, match="forbidden historical band-9"):
        validate(workspace_manifest=workspace)


@pytest.mark.parametrize(
    ("relative", "section", "declaration"),
    [
        (
            "crates/python/Cargo.toml",
            "dependencies",
            'type_bridge_typedb_driver_b9 = { package = "typedb-driver", version = "=3.12.0" }',
        ),
        (
            "crates/node/Cargo.toml",
            "build-dependencies",
            'retired-driver = { package = "type-bridge-typedb-driver-b9", '
            'path = "../../vendor/typedb-driver-b9" }',
        ),
    ],
)
def test_unpublished_binding_manifest_cannot_reference_historical_band9(
    tmp_path: Path,
    relative: str,
    section: str,
    declaration: str,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / relative
    marker = f"[{section}]\n"
    source = manifest.read_text()
    assert marker in source
    manifest.write_text(source.replace(marker, marker + declaration + "\n", 1))

    with pytest.raises(validator.ValidationError, match="forbidden historical band-9"):
        validate(workspace_manifest=workspace)


@pytest.mark.parametrize("section", ("dependencies", "build-dependencies"))
def test_target_specific_manifest_cannot_reference_historical_band9(
    tmp_path: Path,
    section: str,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "crates/python/Cargo.toml"
    with manifest.open("a", encoding="utf-8") as source:
        source.write(
            f"\n[target.'cfg(any())'.{section}]\n"
            'retired-protocol = { package = "typedb-protocol", '
            'path = "../../vendor/typedb-protocol-b9" }\n'
        )

    with pytest.raises(validator.ValidationError, match="forbidden historical band-9"):
        validate(workspace_manifest=workspace)


def test_historical_band9_package_must_be_absent_from_cargo_lock(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    lock = workspace.parent / "Cargo.lock"
    with lock.open("a", encoding="utf-8") as source:
        source.write('\n[[package]]\nname = "type-bridge-typedb-driver-b9"\nversion = "3.12.0"\n')

    with pytest.raises(validator.ValidationError, match="lockfile contains a forbidden"):
        validate(workspace_manifest=workspace)


def test_historical_band9_dependency_must_be_absent_from_cargo_lock(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    replace_lock_package_text(
        workspace,
        "typedb-driver",
        ' "typedb-protocol",',
        ' "typedb-protocol",\n "type-bridge-typedb-protocol-b9 3.12.0",',
    )

    with pytest.raises(validator.ValidationError, match="lockfile contains a forbidden"):
        validate(workspace_manifest=workspace)


@pytest.mark.parametrize(
    "tag",
    (
        "v2.1.0-pre0",
        "v2.1.0-pre.0",
        "v2.1.0rc0",
    ),
)
def test_candidate_channel_rejects_prerelease_tags(tag: str) -> None:
    with pytest.raises(validator.ValidationError, match="not armed for this channel"):
        validator.validate_release_identity(
            tag=tag,
            artifact_contract=validator.ARTIFACT_CONTRACT_PYTHON_NPM_ONLY,
            release_channel=validator.RELEASE_CHANNEL_CANDIDATE,
            workspace_manifest=ROOT / "type-bridge-core/Cargo.toml",
            root_python_manifest=ROOT / "pyproject.toml",
            core_python_manifest=ROOT / "type-bridge-core/pyproject.toml",
            node_package=ROOT / "type-bridge-core/crates/node/package.json",
            release_workflow=ROOT / ".github/workflows/release.yml",
        )


def test_release_channel_identity_mapping_is_exact() -> None:
    assert validator.release_identity_versions("v2.1.0", "candidate") == (
        "2.1.0",
        "2.1.0",
    )
    assert validator.release_identity_versions("v2.1.0", "stable") == (
        "2.1.0",
        "2.1.0",
    )


def test_release_artifact_contract_must_be_known() -> None:
    with pytest.raises(validator.ValidationError, match="Unknown release artifact contract"):
        validate(artifact_contract="python-maybe")


@pytest.mark.parametrize(
    "replacement",
    (
        "type-bridge-core>=2.1.0",
        "type-bridge-core==2.0.2; python_version >= '3.12'",
        "Type-Bridge-Core==2.1.0",
        "type_bridge_core==2.1.0",
        "type.bridge.core==2.1.0",
    ),
)
def test_root_python_core_requirement_must_be_canonical_exact_and_unmarked(
    tmp_path: Path,
    replacement: str,
) -> None:
    manifest, package_init = copy_root_python_authorities(tmp_path)
    source = manifest.read_text(encoding="utf-8")
    assert "type-bridge-core==2.1.0" in source
    manifest.write_text(
        source.replace("type-bridge-core==2.1.0", replacement, 1),
        encoding="utf-8",
    )

    with pytest.raises(validator.ValidationError, match="canonical, unmarked, exact"):
        validate(root_python_manifest=manifest, root_python_init=package_init)


def test_root_python_core_requirement_cannot_be_duplicated_under_an_alias(
    tmp_path: Path,
) -> None:
    manifest, package_init = copy_root_python_authorities(tmp_path)
    source = manifest.read_text(encoding="utf-8")
    manifest.write_text(
        source.replace(
            '"type-bridge-core==2.1.0",',
            '"type-bridge-core==2.1.0",\n'
            "    \"TYPE_BRIDGE_CORE==2.1.0; python_version >= '3.12'\",",
            1,
        ),
        encoding="utf-8",
    )

    with pytest.raises(validator.ValidationError, match="exactly one type-bridge-core"):
        validate(root_python_manifest=manifest, root_python_init=package_init)


def test_import_visible_python_version_must_match_manifest_and_tag(tmp_path: Path) -> None:
    manifest, package_init = copy_root_python_authorities(tmp_path)
    package_init.write_text(
        package_init.read_text(encoding="utf-8").replace(
            '__version__ = "2.1.0"',
            '__version__ = "2.0.2"',
            1,
        ),
        encoding="utf-8",
    )

    with pytest.raises(validator.ValidationError, match="type_bridge.__version__"):
        validate(root_python_manifest=manifest, root_python_init=package_init)


@pytest.mark.parametrize("location", ("root", "package"))
def test_node_package_lock_versions_must_match_package_and_tag(
    tmp_path: Path,
    location: str,
) -> None:
    package = tmp_path / "package.json"
    package_lock = tmp_path / "package-lock.json"
    shutil.copyfile(ROOT / "type-bridge-core/crates/node/package.json", package)
    payload = json.loads(
        (ROOT / "type-bridge-core/crates/node/package-lock.json").read_text(encoding="utf-8")
    )
    if location == "root":
        payload["version"] = "2.1.0-rc.1"
        expected = "package-lock root identity"
    else:
        payload["packages"][""]["version"] = "2.1.0-rc.1"
        expected = r"package-lock packages\[''\] identity"
    package_lock.write_text(json.dumps(payload), encoding="utf-8")

    with pytest.raises(validator.ValidationError, match=expected):
        validate(node_package=package, node_package_lock=package_lock)


def test_node_package_and_lock_cannot_drift_together_from_tag(tmp_path: Path) -> None:
    package = tmp_path / "package.json"
    package_lock = tmp_path / "package-lock.json"
    package_payload = json.loads(
        (ROOT / "type-bridge-core/crates/node/package.json").read_text(encoding="utf-8")
    )
    lock_payload = json.loads(
        (ROOT / "type-bridge-core/crates/node/package-lock.json").read_text(encoding="utf-8")
    )
    package_payload["version"] = "2.1.0-rc.1"
    lock_payload["version"] = "2.1.0-rc.1"
    lock_payload["packages"][""]["version"] = "2.1.0-rc.1"
    package.write_text(json.dumps(package_payload), encoding="utf-8")
    package_lock.write_text(json.dumps(lock_payload), encoding="utf-8")

    with pytest.raises(validator.ValidationError, match="Node package version disagrees"):
        validate(node_package=package, node_package_lock=package_lock)


def test_first_party_cargo_version_drift_hard_fails(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "crates/orm/Cargo.toml"
    manifest.write_text(
        manifest.read_text().replace(
            'version = "2.1.0"',
            'version = "2.0.2"',
            1,
        )
    )

    with pytest.raises(validator.ValidationError, match="type-bridge-orm version"):
        validate(workspace_manifest=workspace)


def test_unpublished_binding_crate_version_drift_hard_fails(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "crates/python/Cargo.toml"
    manifest.write_text(
        manifest.read_text().replace(
            'version = "2.1.0"',
            'version = "2.0.2"',
            1,
        )
    )

    with pytest.raises(validator.ValidationError, match="type-bridge-core version"):
        validate(workspace_manifest=workspace)


def test_workspace_cargo_license_must_remain_mit(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    workspace.write_text(
        workspace.read_text().replace('license = "MIT"', 'license = "Apache-2.0"', 1)
    )

    with pytest.raises(validator.ValidationError, match="workspace license must remain MIT"):
        validate(workspace_manifest=workspace)


def test_workspace_cargo_license_file_must_name_the_root_mit_body(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    workspace.write_text(
        workspace.read_text().replace(
            'license-file = "LICENSE"',
            'license-file = "vendor/typedb-driver-b8/LICENSE"',
            1,
        )
    )

    with pytest.raises(validator.ValidationError, match="workspace license-file"):
        validate(workspace_manifest=workspace)


def test_workspace_cargo_license_file_body_must_be_canonical_mit(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    license_file = workspace.parent / "LICENSE"
    license_file.write_text(
        license_file.read_text().replace(
            "Permission is hereby granted",
            "Permission is hereby withheld",
            1,
        )
    )

    with pytest.raises(validator.ValidationError, match="not the canonical MIT body"):
        validate(workspace_manifest=workspace)


def test_every_first_party_cargo_member_must_resolve_to_mit(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "crates/query/Cargo.toml"
    manifest.write_text(
        manifest.read_text().replace("license.workspace = true", 'license = "MPL-2.0"', 1)
    )

    with pytest.raises(
        validator.ValidationError,
        match="type-bridge-query effective license drifted",
    ):
        validate(workspace_manifest=workspace)


def test_first_party_cargo_member_cannot_omit_its_license(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "crates/cli/Cargo.toml"
    manifest.write_text(manifest.read_text().replace("license.workspace = true\n", "", 1))

    with pytest.raises(validator.ValidationError, match="license.workspace = true"):
        validate(workspace_manifest=workspace)


@pytest.mark.parametrize(
    "replacement",
    ("", 'license-file = "../../../LICENSE"\n'),
)
def test_first_party_cargo_member_must_inherit_workspace_license_file(
    tmp_path: Path,
    replacement: str,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "crates/query/Cargo.toml"
    manifest.write_text(
        manifest.read_text().replace("license-file.workspace = true\n", replacement, 1)
    )

    with pytest.raises(validator.ValidationError, match="license-file.workspace = true"):
        validate(workspace_manifest=workspace)


@pytest.mark.parametrize(
    ("relative", "old_license", "new_license"),
    [
        ("vendor/typedb-driver-b8/Cargo.toml", "Apache-2.0", "MIT"),
        ("vendor/typedb-protocol-b8/Cargo.toml", "MPL-2.0", "MIT"),
    ],
)
def test_legacy_vendor_manifest_license_cannot_drift(
    tmp_path: Path,
    relative: str,
    old_license: str,
    new_license: str,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / relative
    manifest.write_text(
        manifest.read_text().replace(
            f'license = "{old_license}"',
            f'license = "{new_license}"',
            1,
        )
    )

    with pytest.raises(validator.ValidationError, match="effective license drifted"):
        validate(workspace_manifest=workspace)


@pytest.mark.parametrize(
    "relative",
    [
        component.manifest_path
        for component in validator.LEGACY_TYPEDB_COMPONENTS
        if component.downstream_name not in validator.IMMUTABLE_BASELINE_CRATES
    ],
)
def test_active_legacy_vendor_must_use_its_local_license_file(
    tmp_path: Path,
    relative: str,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / relative
    manifest.write_text(
        manifest.read_text().replace(
            'license-file = "LICENSE"',
            'license-file = "../../LICENSE"',
            1,
        )
    )

    with pytest.raises(validator.ValidationError, match="license-file drifted"):
        validate(workspace_manifest=workspace)


def test_no_retained_compatibility_manifest_uses_the_removed_baseline_policy() -> None:
    assert validator.IMMUTABLE_BASELINE_CRATES == ()
    assert {component.band for component in validator.LEGACY_TYPEDB_COMPONENTS} == {8}
    for component in validator.LEGACY_TYPEDB_COMPONENTS:
        manifest = ROOT / "type-bridge-core" / component.manifest_path
        assert 'license-file = "LICENSE"' in manifest.read_text()


def test_legacy_vendor_cannot_relocate_and_claim_the_workspace_mit_license(
    tmp_path: Path,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    original = workspace.parent / "vendor/typedb-driver-b8"
    relocated = workspace.parent / "vendor/relocated-driver"
    shutil.move(original, relocated)
    manifest = relocated / "Cargo.toml"
    manifest.write_text(manifest.read_text().replace('license = "Apache-2.0"', 'license = "MIT"'))
    workspace.write_text(
        workspace.read_text().replace(
            '"vendor/typedb-driver-b8"',
            '"vendor/relocated-driver"',
        )
    )

    with pytest.raises(validator.ValidationError, match="canonical paths"):
        validate(workspace_manifest=workspace)


def test_unexpected_workspace_vendor_member_hard_fails(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    unexpected = workspace.parent / "vendor/unexpected"
    unexpected.mkdir()
    (unexpected / "Cargo.toml").write_text(
        '[package]\nname = "unexpected-vendor"\nversion = "2.1.0-rc.0"\nlicense = "MIT"\n'
    )
    workspace.write_text(
        workspace.read_text().replace(
            "members = [\n",
            'members = [\n    "vendor/unexpected",\n',
            1,
        )
    )

    with pytest.raises(validator.ValidationError, match="unexpected=.*vendor/unexpected"):
        validate(workspace_manifest=workspace)


@pytest.mark.parametrize(
    ("relative", "old_license"),
    [
        ("vendor/typedb-driver-b9/Cargo.toml", "Apache-2.0"),
        ("vendor/typedb-protocol-b9/Cargo.toml", "MPL-2.0"),
    ],
)
def test_historical_band9_manifest_license_cannot_drift(
    tmp_path: Path,
    relative: str,
    old_license: str,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / relative
    manifest.write_text(
        manifest.read_text().replace(f'license = "{old_license}"', 'license = "MIT"', 1)
    )

    with pytest.raises(validator.ValidationError, match="Historical band-9 package.*license"):
        validate(workspace_manifest=workspace)


@pytest.mark.parametrize("relative", validator.HISTORICAL_BAND9_PACKAGES)
def test_historical_band9_manifest_must_use_its_local_license_file(
    tmp_path: Path,
    relative: str,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / relative / "Cargo.toml"
    manifest.write_text(
        manifest.read_text().replace(
            'license-file = "LICENSE"',
            'license-file = "../../LICENSE"',
            1,
        )
    )

    with pytest.raises(validator.ValidationError, match="must use its local LICENSE"):
        validate(workspace_manifest=workspace)


def test_root_python_manifest_license_must_remain_mit(tmp_path: Path) -> None:
    manifest = tmp_path / "pyproject.toml"
    shutil.copyfile(ROOT / "pyproject.toml", manifest)
    manifest.write_text(
        manifest.read_text().replace('license = {text = "MIT"}', 'license = "GPL-3.0"')
    )

    with pytest.raises(validator.ValidationError, match="root Python manifest license"):
        validate(root_python_manifest=manifest)


def test_core_python_manifest_license_must_remain_mit(tmp_path: Path) -> None:
    manifest = tmp_path / "pyproject.toml"
    shutil.copyfile(ROOT / "type-bridge-core/pyproject.toml", manifest)
    manifest.write_text(
        manifest.read_text().replace('license = { text = "MIT" }', 'license = "GPL-3.0"')
    )

    with pytest.raises(validator.ValidationError, match="core Python manifest license"):
        validate(core_python_manifest=manifest)


def test_node_package_license_must_remain_mit(tmp_path: Path) -> None:
    package = tmp_path / "package.json"
    shutil.copyfile(ROOT / "type-bridge-core/crates/node/package.json", package)
    package.write_text(package.read_text().replace('"license": "MIT"', '"license": "ISC"'))

    with pytest.raises(validator.ValidationError, match="Node package license must remain MIT"):
        validate(node_package=package)


def test_retired_band7_packages_are_absent_from_the_current_workspace() -> None:
    workspace_members = (ROOT / "type-bridge-core/Cargo.toml").read_text()
    assert "typedb-driver-b7" not in workspace_members
    assert "typedb-protocol-b7" not in workspace_members
    assert not (ROOT / "type-bridge-core/vendor/typedb-driver-b7").exists()
    assert not (ROOT / "type-bridge-core/vendor/typedb-protocol-b7").exists()


def test_band8_protocol_vendor_identity_drift_hard_fails(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "vendor/typedb-protocol-b8/Cargo.toml"
    manifest.write_text(manifest.read_text().replace('version = "3.11.0"', 'version = "3.11.1"', 1))

    with pytest.raises(validator.ValidationError, match="immutable package metadata drifted"):
        validate(workspace_manifest=workspace)


def test_band8_vendor_identity_drift_hard_fails(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "vendor/typedb-driver-b8/Cargo.toml"
    manifest.write_text(manifest.read_text().replace('version = "3.11.5"', 'version = "3.11.6"', 1))

    with pytest.raises(validator.ValidationError, match="immutable package metadata drifted"):
        validate(workspace_manifest=workspace)


@pytest.mark.parametrize(
    ("old", "new"),
    [
        (
            "Renamed package of upstream typedb-driver 3.11.5 "
            "(TypeDB protocol band 8); source-unmodified compatibility package "
            "authorized for TypeBridge Cargo distribution",
            "generic driver package",
        ),
        ("https://github.com/typedb/typedb-driver", "https://example.invalid/upstream"),
        ("https://github.com/ds1sqe/type-bridge", "https://example.invalid/downstream"),
        ('readme = "README.md"', 'readme = "OTHER.md"'),
    ],
)
def test_active_legacy_vendor_crates_io_metadata_is_immutable(
    tmp_path: Path,
    old: str,
    new: str,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "vendor/typedb-driver-b8/Cargo.toml"
    source = manifest.read_text()
    assert old in source
    manifest.write_text(source.replace(old, new, 1))

    with pytest.raises(validator.ValidationError, match="immutable package metadata drifted"):
        validate(workspace_manifest=workspace)


def test_band8_driver_requirement_must_exactly_match_runtime_constant(
    tmp_path: Path,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "crates/typedb-runtime/Cargo.toml"
    manifest.write_text(
        manifest.read_text().replace(
            'version = "=3.11.5", optional = true }',
            'version = "3", optional = true }',
            1,
        )
    )

    with pytest.raises(
        validator.ValidationError,
        match="type-bridge-typedb-driver-b8 dependency must exactly match its runtime pin",
    ):
        validate(workspace_manifest=workspace)


def test_retired_band7_dependency_cannot_reenter_the_runtime_manifest(
    tmp_path: Path,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "crates/typedb-runtime/Cargo.toml"
    manifest.write_text(
        manifest.read_text().replace(
            "[dependencies]",
            "[dependencies]\n"
            'type-bridge-typedb-driver-b7 = { version = "=3.8.1", optional = true }',
            1,
        )
    )

    with pytest.raises(
        validator.ValidationError,
        match="reintroduces retired band-7 support",
    ):
        validate(workspace_manifest=workspace)


def test_band8_driver_dependency_must_name_the_renamed_package(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "crates/typedb-runtime/Cargo.toml"
    manifest.write_text(
        manifest.read_text().replace(
            "type-bridge-typedb-driver-b8 = { path",
            'type-bridge-typedb-driver-b8 = { package = "type-bridge-typedb-driver-b7", path',
            1,
        )
    )

    with pytest.raises(
        validator.ValidationError,
        match="band-8 package dependency has the wrong package identity",
    ):
        validate(workspace_manifest=workspace)


def test_band8_runtime_constant_drift_hard_fails(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    source = workspace.parent / "crates/typedb-runtime/src/lib.rs"
    source.write_text(
        source.read_text().replace(
            'pub const PINNED_DRIVER_VERSION: &str = "3.11.5";',
            'pub const PINNED_DRIVER_VERSION: &str = "3.11.4";',
            1,
        )
    )

    with pytest.raises(
        validator.ValidationError,
        match="actual='=3.11.5', expected='=3.11.4'",
    ):
        validate(workspace_manifest=workspace)


def test_band9_driver_requirement_must_exactly_match_runtime_constant(
    tmp_path: Path,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "crates/typedb-runtime/Cargo.toml"
    manifest.write_text(
        manifest.read_text().replace(
            'typedb-driver = { version = "=3.12.1", optional = true }',
            'typedb-driver = { version = "3", optional = true }',
            1,
        )
    )

    with pytest.raises(
        validator.ValidationError,
        match="typedb-driver dependency must exactly match its runtime pin",
    ):
        validate(workspace_manifest=workspace)


def test_band9_runtime_constant_drift_hard_fails(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    source = workspace.parent / "crates/typedb-runtime/src/lib.rs"
    source.write_text(
        source.read_text().replace(
            'pub const PINNED_DRIVER_VERSION_B9: &str = "3.12.1";',
            'pub const PINNED_DRIVER_VERSION_B9: &str = "3.12.2";',
            1,
        )
    )

    with pytest.raises(
        validator.ValidationError,
        match="actual='=3.12.1', expected='=3.12.2'",
    ):
        validate(workspace_manifest=workspace)


def test_band9_lockfile_source_must_be_official_crates_io(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    replace_lock_package_text(
        workspace,
        "typedb-driver",
        'source = "registry+https://github.com/rust-lang/crates.io-index"',
        'source = "git+https://example.invalid/typedb-driver"',
    )

    with pytest.raises(validator.ValidationError, match="must resolve from official crates.io"):
        validate(workspace_manifest=workspace)


def test_band9_pin_refresh_cannot_leave_native_provenance_stale(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "crates/typedb-runtime/Cargo.toml"
    manifest.write_text(
        manifest.read_text().replace(
            'typedb-driver = { version = "=3.12.1", optional = true }',
            'typedb-driver = { version = "=3.12.2", optional = true }',
            1,
        )
    )
    runtime_source = workspace.parent / "crates/typedb-runtime/src/lib.rs"
    runtime_source.write_text(
        runtime_source.read_text().replace(
            'pub const PINNED_DRIVER_VERSION_B9: &str = "3.12.1";',
            'pub const PINNED_DRIVER_VERSION_B9: &str = "3.12.2";',
            1,
        )
    )
    replace_lock_package_text(
        workspace,
        "typedb-driver",
        'version = "3.12.1"',
        'version = "3.12.2"',
    )
    replace_lock_package_text(
        workspace,
        "typedb-driver",
        "b7daa941ffe0f6e6cb17e2e831e13b338a9db23551414f877c7fb64ce05f9f46",
        "1" * 64,
    )

    with pytest.raises(validator.ValidationError, match="driver version disagrees"):
        validate(workspace_manifest=workspace)


def test_band9_lockfile_must_bind_driver_to_audited_protocol(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    replace_lock_package_text(
        workspace,
        "typedb-driver",
        ' "typedb-protocol",',
        ' "typedb-protocol 3.12.9",',
    )

    with pytest.raises(validator.ValidationError, match="audited official typedb-protocol"):
        validate(workspace_manifest=workspace)


def test_band9_lockfile_checksum_must_match_packaged_notices(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    replace_lock_package_text(
        workspace,
        "typedb-driver",
        "b7daa941ffe0f6e6cb17e2e831e13b338a9db23551414f877c7fb64ce05f9f46",
        "0" * 64,
    )

    with pytest.raises(validator.ValidationError, match="archive provenance disagrees"):
        validate(workspace_manifest=workspace)


def test_band9_notice_version_must_match_resolved_protocol(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    replace_both_native_notices(
        workspace,
        "official `typedb-protocol` 3.12.0",
        "official `typedb-protocol` 3.12.9",
    )

    with pytest.raises(validator.ValidationError, match="protocol version disagrees"):
        validate(workspace_manifest=workspace)


def test_band9_notice_source_must_name_exact_official_crates_io_package(
    tmp_path: Path,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    replace_both_native_notices(
        workspace,
        "TypeDB official crates.io package [3.12.1](https://crates.io/crates/typedb-driver/3.12.1)",
        "TypeDB tag [3.12.1]"
        "(https://github.com/typedb/typedb-driver/tree/0000000000000000000000000000000000000000)",
    )

    with pytest.raises(validator.ValidationError, match="official crates.io package"):
        validate(workspace_manifest=workspace)


@pytest.mark.parametrize(
    ("old", "new"),
    [
        (
            "TypeDB `typedb-driver` tag [3.11.5]"
            "(https://github.com/typedb/typedb-driver/tree/"
            "7e669e41d9fee22fde8d5e60be7edbf00c6ec64b) "
            "(commit `7e669e41d9fee22fde8d5e60be7edbf00c6ec64b`)",
            "TypeDB `typedb-driver` tag [3.11.6]"
            "(https://github.com/typedb/typedb-driver/tree/"
            "7e669e41d9fee22fde8d5e60be7edbf00c6ec64b) "
            "(commit `7e669e41d9fee22fde8d5e60be7edbf00c6ec64b`)",
        ),
        (
            "MPL-2.0 namespaced packaging-only package; generated protocol source unchanged; "
            "owner-authorized for TypeBridge Cargo distribution",
            "MIT namespaced packaging-only package; generated protocol source unchanged; "
            "owner-authorized for TypeBridge Cargo distribution",
        ),
    ],
)
def test_legacy_notice_component_rows_are_exact(
    tmp_path: Path,
    old: str,
    new: str,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    replace_both_native_notices(workspace, old, new)

    with pytest.raises(validator.ValidationError, match="component provenance drifted"):
        validate(workspace_manifest=workspace)


def test_native_notices_describe_only_the_retained_compatibility_band() -> None:
    python_notice = ROOT / "type-bridge-core/python/type_bridge_core/THIRD_PARTY_NOTICES.md"
    node_notice = ROOT / "type-bridge-core/crates/node/THIRD_PARTY_NOTICES.md"
    python_body = python_notice.read_bytes()

    assert python_body == node_notice.read_bytes()
    compact = " ".join(python_body.decode().split())
    assert "band-7" not in compact.lower()
    assert (
        "band-8 compatibility copies are source-unmodified and owner-authorized "
        "for TypeBridge Cargo distribution"
    ) in compact
    assert "authorized first publication of the band-8 packages on 2026-08-03" in compact
    assert "These exact source-unmodified packages" in compact
    assert "protocol package preceding the driver package" in compact
    assert "renamed crates are also distributed as immutable crates.io" not in compact
    assert "currently absent" not in compact
    assert "currently unpublished" not in compact


def test_native_notice_cannot_remove_band8_publication_authorization(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    replace_both_native_notices(
        workspace,
        "authorized first publication of the band-8 packages on\n2026-08-03",
        "did not authorize publication of the band-8 packages",
    )

    with pytest.raises(validator.ValidationError, match="band-8 registry disposition"):
        validate(workspace_manifest=workspace)


def test_band8_artifact_text_is_time_stable() -> None:
    paths = (
        ROOT / "type-bridge-core/vendor/typedb-driver-b8/Cargo.toml",
        ROOT / "type-bridge-core/vendor/typedb-driver-b8/README.md",
        ROOT / "type-bridge-core/vendor/typedb-protocol-b8/Cargo.toml",
        ROOT / "type-bridge-core/vendor/typedb-protocol-b8/README.md",
        ROOT / "type-bridge-core/vendor/README.md",
        ROOT / "type-bridge-core/python/type_bridge_core/THIRD_PARTY_NOTICES.md",
        ROOT / "type-bridge-core/crates/node/THIRD_PARTY_NOTICES.md",
    )
    for path in paths:
        body = " ".join(path.read_text().split()).lower()
        assert "separate explicit typedb" not in body
        assert "authorized" in body
        assert "cargo distribution" in body
        assert "currently absent" not in body
        assert "currently unpublished" not in body


@pytest.mark.parametrize(
    ("old", "new"),
    [
        (
            "https://static.crates.io/crates/typedb-driver/typedb-driver-3.11.5.crate",
            "https://example.invalid/typedb-driver-3.11.5.crate",
        ),
        (
            "71c456fc6fb8f9112236fc088569cbe47f620443629ef8c81b1d79aec7b49fc6",
            "0" * 64,
        ),
    ],
)
def test_legacy_notice_archive_rows_are_exact(
    tmp_path: Path,
    old: str,
    new: str,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    replace_both_native_notices(workspace, old, new)

    with pytest.raises(validator.ValidationError, match="archive provenance drifted"):
        validate(workspace_manifest=workspace)


def test_native_notice_copies_must_remain_byte_identical(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    node_notice = workspace.parent / "crates/node/THIRD_PARTY_NOTICES.md"
    node_notice.write_text(node_notice.read_text().replace("TypeDB-owned", "TypeDB-derived", 1))

    with pytest.raises(validator.ValidationError, match="must be byte-identical"):
        validate(workspace_manifest=workspace)


def test_native_notice_requires_generated_dependency_appendix(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    replace_both_native_notices(
        workspace,
        "## Locked Rust dependency closure",
        "## Unverified Rust dependency list",
    )

    with pytest.raises(validator.ValidationError, match="generated dependency appendix"):
        validate(workspace_manifest=workspace)


@pytest.mark.parametrize(
    ("old", "new", "license_name"),
    [
        (
            "Permission is hereby granted, free of charge",
            "Permission is hereby withheld, free of charge",
            "MIT License",
        ),
        (
            "TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION",
            "TERMS AND CONDITIONS FOR USE AND DISTRIBUTION",
            "Apache License",
        ),
        (
            "Mozilla Public License Version 2.0",
            "Mozilla Public License Version 2.1",
            "Mozilla Public License",
        ),
        (
            "Copyright (c) 2017-2019 isis agora lovecruft.",
            "Copyright (c) 2017-2020 isis agora lovecruft.",
            "ed25519-dalek 2.2.0",
        ),
        (
            "Copyright (c) 2016-2021 Henry de Valence.",
            "Copyright (c) 2016-2022 Henry de Valence.",
            "curve25519-dalek 4.1.3",
        ),
    ],
)
def test_embedded_notice_license_bodies_must_be_canonical(
    tmp_path: Path,
    old: str,
    new: str,
    license_name: str,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    replace_both_native_notices(workspace, old, new)

    with pytest.raises(validator.ValidationError, match=rf"{license_name}.*canonical LICENSE"):
        validate(workspace_manifest=workspace)


@pytest.mark.parametrize(
    ("relative", "old", "new", "message"),
    [
        (
            "vendor/typedb-driver-b8/LICENSE",
            "Apache License",
            "Apache Licence",
            "canonical Apache-2.0 body",
        ),
        (
            "vendor/typedb-protocol-b8/LICENSE",
            "Mozilla Public License",
            "Mozilla Public Licence",
            "canonical MPL-2.0 body",
        ),
    ],
)
def test_legacy_license_family_files_must_be_byte_identical(
    tmp_path: Path,
    relative: str,
    old: str,
    new: str,
    message: str,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    license_path = workspace.parent / relative
    license_path.write_text(license_path.read_text().replace(old, new, 1))

    with pytest.raises(validator.ValidationError, match=message):
        validate(workspace_manifest=workspace)


def test_root_mit_license_body_is_canonical(tmp_path: Path) -> None:
    root_manifest, root_python_init = copy_root_python_authorities(tmp_path)
    root_license = tmp_path / "LICENSE"
    shutil.copyfile(ROOT / "LICENSE", root_license)
    root_license.write_text(
        root_license.read_text().replace(
            "Permission is hereby granted",
            "Permission is hereby withheld",
            1,
        )
    )

    with pytest.raises(validator.ValidationError, match="not the canonical MIT body"):
        validate(
            root_python_manifest=root_manifest,
            root_python_init=root_python_init,
        )


def test_band9_notice_cannot_relicense_official_driver(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    replace_both_native_notices(
        workspace,
        "Apache-2.0, unmodified official package",
        "MIT, unmodified official package",
    )

    with pytest.raises(validator.ValidationError, match="license/status drifted"):
        validate(workspace_manifest=workspace)


def test_vendor_provenance_band9_versions_must_match_lockfile(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    readme = workspace.parent / "vendor/README.md"
    readme.write_text(
        readme.read_text().replace(
            "official `typedb-driver` 3.12.1 and `typedb-protocol` 3.12.0",
            "official `typedb-driver` 3.12.9 and `typedb-protocol` 3.12.0",
            1,
        )
    )

    with pytest.raises(validator.ValidationError, match="versions disagree with Cargo.lock"):
        validate(workspace_manifest=workspace)


def test_vendor_provenance_current_driver_prose_tracks_resolved_pin(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    readme = workspace.parent / "vendor/README.md"
    readme.write_text(
        readme.read_text().replace(
            "currently that is 3.12.1, exercised",
            "currently that is 3.12.9, exercised",
            1,
        )
    )

    with pytest.raises(validator.ValidationError, match="Vendor provenance is missing"):
        validate(workspace_manifest=workspace)


@pytest.mark.parametrize(
    "marker",
    (
        "CARGO_REGISTRY_TOKEN",
        "--verify-preexisting type-bridge-core-lib",
        "cargo package -p type-bridge-core-lib",
        "cargo publish",
        "patch.crates-io.type-bridge-core-lib.path=crates/core",
        "needs: publish-crates",
        "publish_crate_idempotently.sh",
        "cargo_release_candidate.py",
        "publish_cargo_release_candidate.py",
        "cargo-release-candidate",
        "publish-crates:",
        "name: rust-crates",
        "path: type-bridge-core/target/package",
        "type-bridge-typedb-driver-b8",
        "type-bridge-typedb-protocol-b8",
        "validate_fresh_typedb_runtime_package.sh",
        "validate_rust_release_artifacts.py",
    ),
)
def test_python_npm_workflow_rejects_every_cargo_artifact_path(
    tmp_path: Path,
    marker: str,
) -> None:
    workflow = tmp_path / "release.yml"
    workflow.write_text(f"name: hostile\n# {marker}\n")

    with pytest.raises(validator.ValidationError, match="forbidden crates.io paths"):
        validator.validate_python_npm_only_workflow(workflow)


def test_internal_crate_cannot_be_implicitly_publishable(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "crates/python/Cargo.toml"
    manifest.write_text(manifest.read_text().replace("publish = false\n", "", 1))

    with pytest.raises(validator.ValidationError, match="publish = false exactly"):
        validate(workspace_manifest=workspace)


def test_public_crate_path_dependency_requires_a_release_version(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "crates/orm/Cargo.toml"
    manifest.write_text(
        manifest.read_text().replace(
            'type-bridge-contract = { path = "../contract", version = "2.1.0" }',
            'type-bridge-contract = { path = "../contract" }',
            1,
        )
    )

    with pytest.raises(validator.ValidationError, match="must declare the release version"):
        validate(workspace_manifest=workspace)


def test_repository_cargo_graph_has_no_publication_blocker() -> None:
    assert validate()["rust_publication_blockers"] == []


def test_public_crate_cannot_depend_on_an_unpublished_workspace_crate(
    tmp_path: Path,
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "crates/migration/Cargo.toml"
    manifest.write_text(
        manifest.read_text().replace(
            'type-bridge-schema-compat = { path = "../schema-compat", version = "2.1.0" }',
            'type-bridge-schema-compat = { package = "type-bridge-core", path = "../python", version = "2.1.0" }',
            1,
        )
    )

    with pytest.raises(validator.ValidationError, match="depends on unpublished workspace crate"):
        validate(workspace_manifest=workspace)


@pytest.mark.parametrize(
    "manifest_path",
    [
        "crates/orm/Cargo.toml",
        "crates/migration/Cargo.toml",
        "crates/toml-transpiler/Cargo.toml",
    ],
)
def test_planned_public_crate_cannot_be_marked_unpublished(
    tmp_path: Path, manifest_path: str
) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / manifest_path
    manifest.write_text(
        manifest.read_text().replace('publish = ["crates-io"]', "publish = false", 1)
    )

    with pytest.raises(validator.ValidationError, match="publishable package set"):
        validate(workspace_manifest=workspace)


def test_unpinned_publishable_vendor_crate_hard_fails(tmp_path: Path) -> None:
    workspace = copy_workspace_manifests(tmp_path)
    manifest = workspace.parent / "vendor/typedb-driver-b8/Cargo.toml"
    manifest.write_text(
        manifest.read_text().replace(
            'name = "type-bridge-typedb-driver-b8"',
            'name = "hostile-unpinned-vendor"',
            1,
        )
    )

    with pytest.raises(validator.ValidationError, match="immutable package metadata drifted"):
        validate(workspace_manifest=workspace)


def synthetic_crate_archive(
    package: str,
    version: str,
    files: dict[str, bytes],
    *,
    symlink: tuple[str, str] | None = None,
) -> bytes:
    """Build a deterministic-enough in-memory .crate fixture without network access."""
    stream = io.BytesIO()
    root = f"{package}-{version}"
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


def write_synthetic_vendor_tree(root: Path, relative: str, files: dict[str, bytes]) -> None:
    """Write one synthetic local tree whose inventory matches the release gate."""
    component_root = root / relative
    for path, body in files.items():
        target = component_root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(body)


def synthetic_driver_provenance_fixture(
    tmp_path: Path,
    *,
    band: int = 8,
) -> tuple[Path, Any, bytes, dict[str, bytes]]:
    """Create one byte-identical synthetic downstream driver package."""
    protocol = validator.legacy_protocol_component(band)
    protocol_version = f"={protocol.upstream_version}"
    upstream_source = {
        "src/lib.rs": b"upstream lib\n",
        "src/connection/message.rs": b"upstream message\n",
        "src/connection/network/proto/message.rs": b"upstream proto message\n",
        "src/connection/network/transmitter/transaction.rs": b"upstream transaction\n",
    }
    upstream_manifest = f"""
[features]
sync = []

[package]
license = "Apache-2.0"
name = "typedb-driver"
edition = "2024"
description = "TypeDB Rust Driver"
readme = "README.md"
repository = "https://github.com/typedb/typedb-driver"
version = "9.8.1"
licenseFile = "LICENSE"
authors = []
homepage = "https://github.com/typedb/typedb-driver"

[lib]
path = "src/lib.rs"

[dependencies.typedb-protocol]
features = []
version = "{protocol_version}"

[dependencies.tonic]
version = "0.12"
features = ["tls", "tls-roots"]
""".lstrip().encode()
    upstream_files = {
        "Cargo.toml": upstream_manifest,
        "LICENSE": b"synthetic Apache-2.0 license\n",
        "README.md": b"TypeDB upstream\n",
        **upstream_source,
    }
    archive = synthetic_crate_archive("typedb-driver", "9.8.1", upstream_files)
    component = validator.LegacyTypeDbComponent(
        archive_checksum=hashlib.sha256(archive).hexdigest(),
        band=band,
        downstream_name=SYNTHETIC_DRIVER_NAME,
        downstream_version="9.8.1",
        license=validator.APACHE_2_LICENSE,
        license_status="synthetic Apache compatibility package",
        manifest_path="vendor/synthetic-driver/Cargo.toml",
        upstream_commit="0" * 40,
        upstream_name="typedb-driver",
        upstream_version="9.8.1",
    )
    license_file = 'license-file = "LICENSE"\n' if band == 8 else ""
    local_manifest = f"""
[package]
name = "{SYNTHETIC_DRIVER_NAME}"
version = "9.8.1"
edition = "2024"
license = "Apache-2.0"
{license_file}description = "{validator.LEGACY_VENDOR_DESCRIPTIONS[SYNTHETIC_DRIVER_NAME]}"
readme = "README.md"
repository = "{validator.TYPEBRIDGE_REPOSITORY}"
homepage = "https://github.com/typedb/typedb-driver"

[features]
sync = []

[lints.rust]
unused = "allow"
dead_code = "allow"
private_interfaces = "allow"

[lints.clippy]
all = "allow"

[lib]
path = "src/lib.rs"
doctest = false

[dependencies]
typedb-protocol = {{ package = "{protocol.downstream_name}", path = "../{Path(protocol.vendor_directory).name}", version = "{protocol_version}" }}
tonic = {{ version = "0.12", features = ["tls", "tls-roots"] }}

[dev-dependencies]
rand = "0.8"
serde_json = "1"
""".lstrip().encode()
    local_files = {
        "Cargo.toml": local_manifest,
        "LICENSE": upstream_files["LICENSE"],
        "README.md": (
            validator.legacy_vendor_readme_disclosure(component) + upstream_files["README.md"]
        ),
        **upstream_source,
    }
    write_synthetic_vendor_tree(tmp_path, "vendor/synthetic-driver", local_files)
    return tmp_path, component, archive, upstream_source


def synthetic_protocol_provenance_fixture(
    tmp_path: Path,
    *,
    band: int = 8,
) -> tuple[Path, Any, bytes]:
    """Create one byte-identical synthetic downstream protocol package."""
    upstream_manifest = b"""
[package]
license = "MPL-2.0"
name = "typedb-protocol"
edition = "2024"
description = "TypeDB Protocol"
readme = "README.md"
repository = "https://github.com/typedb/typedb-protocol"
version = "9.8.0"
licenseFile = "LICENSE"
authors = []
homepage = "https://github.com/typedb/typedb-protocol"

[lib]
path = "src/typedb.protocol.rs"

[dependencies.prost]
version = "=0.13.5"

[dependencies.tonic]
version = "=0.12.3"
""".lstrip()
    upstream_files = {
        "Cargo.toml": upstream_manifest,
        "LICENSE": b"synthetic MPL-2.0 license\n",
        "README.md": b"TypeDB upstream\n",
        "src/typedb.protocol.rs": b"// generated protocol\n",
    }
    archive = synthetic_crate_archive("typedb-protocol", "9.8.0", upstream_files)
    component = validator.LegacyTypeDbComponent(
        archive_checksum=hashlib.sha256(archive).hexdigest(),
        band=band,
        downstream_name=SYNTHETIC_PROTOCOL_NAME,
        downstream_version="9.8.0",
        license=validator.MPL_2_LICENSE,
        license_status="synthetic MPL package",
        manifest_path="vendor/synthetic-protocol/Cargo.toml",
        upstream_commit="0" * 40,
        upstream_name="typedb-protocol",
        upstream_version="9.8.0",
    )
    license_file = 'license-file = "LICENSE"\n' if band == 8 else ""
    local_manifest = f"""
[package]
name = "{SYNTHETIC_PROTOCOL_NAME}"
version = "9.8.0"
edition = "2024"
license = "MPL-2.0"
{license_file}description = "{validator.LEGACY_VENDOR_DESCRIPTIONS[SYNTHETIC_PROTOCOL_NAME]}"
readme = "README.md"
repository = "{validator.TYPEBRIDGE_REPOSITORY}"
homepage = "https://github.com/typedb/typedb-protocol"

[lints.rust]
unused = "allow"
dead_code = "allow"

[lints.clippy]
all = "allow"

[lib]
path = "src/typedb.protocol.rs"
doctest = false

[dependencies]
prost = "=0.13.5"
tonic = "=0.12.3"
""".lstrip().encode()
    local_files = {
        **upstream_files,
        "Cargo.toml": local_manifest,
        "README.md": (
            validator.legacy_vendor_readme_disclosure(component) + upstream_files["README.md"]
        ),
    }
    write_synthetic_vendor_tree(tmp_path, "vendor/synthetic-protocol", local_files)
    return tmp_path, component, archive


def test_synthetic_driver_source_and_license_remain_byte_identical(tmp_path: Path) -> None:
    root, component, archive, _ = synthetic_driver_provenance_fixture(tmp_path)

    assert validator.validate_legacy_component_tree(root, component, archive) == ()


def test_synthetic_protocol_source_and_license_can_remain_byte_identical(
    tmp_path: Path,
) -> None:
    root, component, archive = synthetic_protocol_provenance_fixture(tmp_path)

    assert validator.validate_legacy_component_tree(root, component, archive) == ()


@pytest.mark.parametrize(
    ("needle", "replacement"),
    (
        ("sync = []", 'sync = ["hostile"]'),
        ('path = "src/lib.rs"', 'path = "src/hostile.rs"'),
        ('version = "0.12", features', 'version = "9.99", features'),
        ('package = "type-bridge-typedb-protocol-b8"', 'package = "hostile-protocol"'),
        ('path = "../typedb-protocol-b8"', 'path = "../hostile-protocol"'),
        ('private_interfaces = "allow"', 'private_interfaces = "deny"'),
        ('rand = "0.8"', 'rand = "9"'),
    ),
)
def test_driver_manifest_behavior_must_match_packaging_only_transform(
    tmp_path: Path,
    needle: str,
    replacement: str,
) -> None:
    root, component, archive, _ = synthetic_driver_provenance_fixture(tmp_path)
    manifest = root / component.vendor_directory / "Cargo.toml"
    body = manifest.read_text()
    assert needle in body
    manifest.write_text(body.replace(needle, replacement, 1))

    with pytest.raises(validator.ValidationError, match="packaging-only transform"):
        validator.validate_legacy_component_tree(root, component, archive)


def test_driver_manifest_cannot_add_build_behavior(tmp_path: Path) -> None:
    root, component, archive, _ = synthetic_driver_provenance_fixture(tmp_path)
    manifest = root / component.vendor_directory / "Cargo.toml"
    manifest.write_text(manifest.read_text() + '\n[build-dependencies]\ncc = "1"\n')

    with pytest.raises(validator.ValidationError, match="packaging-only transform"):
        validator.validate_legacy_component_tree(root, component, archive)


def test_protocol_manifest_dependency_must_match_upstream(tmp_path: Path) -> None:
    root, component, archive = synthetic_protocol_provenance_fixture(tmp_path)
    manifest = root / component.vendor_directory / "Cargo.toml"
    manifest.write_text(manifest.read_text().replace('tonic = "=0.12.3"', 'tonic = "9"'))

    with pytest.raises(validator.ValidationError, match="packaging-only transform"):
        validator.validate_legacy_component_tree(root, component, archive)


def test_retired_band7_has_no_current_provenance_component() -> None:
    assert all(component.band == 8 for component in validator.LEGACY_TYPEDB_COMPONENTS)
    with pytest.raises(validator.ValidationError, match="exactly one protocol component"):
        validator.legacy_protocol_component(7)


def test_driver_readme_disclosure_prefix_is_exact(tmp_path: Path) -> None:
    root, component, archive, _ = synthetic_driver_provenance_fixture(tmp_path)
    readme = root / component.vendor_directory / "README.md"
    readme.write_bytes(readme.read_bytes().replace(b"SHA-256", b"SHA256", 1))

    with pytest.raises(validator.ValidationError, match="README disclosure drifted"):
        validator.validate_legacy_component_tree(root, component, archive)


def test_driver_readme_upstream_suffix_is_exact(tmp_path: Path) -> None:
    root, component, archive, _ = synthetic_driver_provenance_fixture(tmp_path)
    readme = root / component.vendor_directory / "README.md"
    readme.write_bytes(readme.read_bytes() + b"downstream appendix\n")

    with pytest.raises(validator.ValidationError, match="exact upstream suffix"):
        validator.validate_legacy_component_tree(root, component, archive)


def test_band8_protocol_readme_disclosure_prefix_is_exact(tmp_path: Path) -> None:
    root, component, archive = synthetic_protocol_provenance_fixture(tmp_path)
    readme = root / component.vendor_directory / "README.md"
    readme.write_bytes(readme.read_bytes().replace(b"original source", b"mirror", 1))

    with pytest.raises(validator.ValidationError, match="README disclosure drifted"):
        validator.validate_legacy_component_tree(root, component, archive)


@pytest.mark.parametrize("package_kind", ("driver", "protocol"))
def test_retained_band8_readme_cannot_drop_its_disclosure(
    tmp_path: Path,
    package_kind: str,
) -> None:
    if package_kind == "driver":
        root, component, archive, _ = synthetic_driver_provenance_fixture(tmp_path)
    else:
        root, component, archive = synthetic_protocol_provenance_fixture(tmp_path)
    readme = root / component.vendor_directory / "README.md"
    readme.write_bytes(b"TypeDB upstream\n")
    with pytest.raises(validator.ValidationError, match="README disclosure drifted"):
        validator.validate_legacy_component_tree(root, component, archive)


def test_archive_checksum_is_rejected_before_archive_parsing(tmp_path: Path) -> None:
    _, component, _, _ = synthetic_driver_provenance_fixture(tmp_path)

    with pytest.raises(validator.ValidationError, match="archive checksum mismatch"):
        validator.verified_crate_archive_files(component, b"not even a tar archive")


def test_archive_traversal_path_hard_fails(tmp_path: Path) -> None:
    _, component, _, _ = synthetic_driver_provenance_fixture(tmp_path)
    archive = synthetic_crate_archive(
        "typedb-driver",
        "9.8.1",
        {"../escape.rs": b"hostile\n"},
    )
    component = replace(component, archive_checksum=hashlib.sha256(archive).hexdigest())

    with pytest.raises(validator.ValidationError, match="unsafe path"):
        validator.verified_crate_archive_files(component, archive)


def test_archive_symlink_hard_fails(tmp_path: Path) -> None:
    _, component, _, _ = synthetic_driver_provenance_fixture(tmp_path)
    archive = synthetic_crate_archive(
        "typedb-driver",
        "9.8.1",
        {"LICENSE": b"license\n"},
        symlink=("src/lib.rs", "../../host-file"),
    )
    component = replace(component, archive_checksum=hashlib.sha256(archive).hexdigest())

    with pytest.raises(validator.ValidationError, match="symlink or non-regular"):
        validator.verified_crate_archive_files(component, archive)


@pytest.mark.parametrize(
    "relative",
    (
        "src/lib.rs",
        "src/connection/message.rs",
        "src/connection/network/proto/message.rs",
        "src/connection/network/transmitter/transaction.rs",
    ),
)
def test_driver_source_mutation_hard_fails(tmp_path: Path, relative: str) -> None:
    root, component, archive, _ = synthetic_driver_provenance_fixture(tmp_path)
    changed = root / component.vendor_directory / relative
    changed.write_bytes(changed.read_bytes() + b"hostile delta\n")

    with pytest.raises(validator.ValidationError, match="source must remain byte-identical"):
        validator.validate_legacy_component_tree(root, component, archive)


def test_driver_source_path_inventory_must_exactly_match_upstream(tmp_path: Path) -> None:
    root, component, archive, _ = synthetic_driver_provenance_fixture(tmp_path)
    unexpected = root / component.vendor_directory / "src/hostile.rs"
    unexpected.write_text("// undisclosed source\n")

    with pytest.raises(validator.ValidationError, match="source path inventory drifted"):
        validator.validate_legacy_component_tree(root, component, archive)


@pytest.mark.parametrize("relative", ("LICENSE", "src/typedb.protocol.rs"))
def test_protocol_source_and_license_mutations_hard_fail(
    tmp_path: Path,
    relative: str,
) -> None:
    root, component, archive = synthetic_protocol_provenance_fixture(tmp_path)
    changed = root / component.vendor_directory / relative
    changed.write_bytes(changed.read_bytes() + b"hostile delta\n")

    with pytest.raises(validator.ValidationError, match="byte-identical"):
        validator.validate_legacy_component_tree(root, component, archive)


def test_unexpected_local_vendor_file_hard_fails(tmp_path: Path) -> None:
    root, component, archive, _ = synthetic_driver_provenance_fixture(tmp_path)
    (root / component.vendor_directory / "build.rs").write_text("fn main() {}\n")

    with pytest.raises(validator.ValidationError, match="root inventory drifted"):
        validator.validate_legacy_component_tree(root, component, archive)


class FakeArchiveResponse:
    """Minimal bounded HTTP response used by the no-network downloader tests."""

    status = 200

    def __init__(self, url: str, body: bytes, *, content_length: str | None = None) -> None:
        self.url = url
        self.body = io.BytesIO(body)
        self.headers = {} if content_length is None else {"Content-Length": content_length}

    def __enter__(self) -> FakeArchiveResponse:
        return self

    def __exit__(self, *_: object) -> None:
        return None

    def geturl(self) -> str:
        return self.url

    def read(self, size: int) -> bytes:
        return self.body.read(size)


@pytest.mark.parametrize("declared_length", ("invalid", "9"))
def test_archive_download_declared_length_is_bounded_and_fail_closed(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    declared_length: str,
) -> None:
    _, component, _, _ = synthetic_driver_provenance_fixture(tmp_path)
    monkeypatch.setattr(validator, "MAX_LEGACY_ARCHIVE_BYTES", 8)

    def opener(request: object, *, timeout: int) -> FakeArchiveResponse:
        assert timeout == validator.LEGACY_ARCHIVE_HTTP_TIMEOUT_SECONDS
        return FakeArchiveResponse(component.archive_url, b"", content_length=declared_length)

    with pytest.raises(validator.ValidationError, match="Content-Length|compressed byte budget"):
        validator.download_legacy_crate_archive(component, opener=opener)


def test_archive_download_stream_is_bounded_without_content_length(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _, component, _, _ = synthetic_driver_provenance_fixture(tmp_path)
    monkeypatch.setattr(validator, "MAX_LEGACY_ARCHIVE_BYTES", 8)

    def opener(request: object, *, timeout: int) -> FakeArchiveResponse:
        assert timeout == validator.LEGACY_ARCHIVE_HTTP_TIMEOUT_SECONDS
        return FakeArchiveResponse(component.archive_url, b"123456789")

    with pytest.raises(validator.ValidationError, match="compressed byte budget while reading"):
        validator.download_legacy_crate_archive(component, opener=opener)
