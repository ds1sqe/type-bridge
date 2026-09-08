"""Closed-contract tests for the Cargo release inventory."""

from __future__ import annotations

import importlib.util
import json
import re
import subprocess
import sys
from pathlib import Path
from types import ModuleType

import pytest

ROOT = Path(__file__).resolve().parents[3]
INVENTORY_MODULE = ROOT / "scripts/ci/cargo_release_inventory.py"
INVENTORY_FILE = ROOT / "scripts/ci/cargo_release_inventory.toml"
CARGO_PACKAGE_INDEX = ROOT / "docs/guide/cargo-packages.md"


def load_module(name: str, path: Path) -> ModuleType:
    """Load one standalone CI helper without making scripts a package."""
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


inventory_module = load_module("cargo_release_inventory", INVENTORY_MODULE)


def hostile_inventory(tmp_path: Path, old: str, new: str) -> Path:
    """Write one deliberately mutated inventory."""
    source = INVENTORY_FILE.read_text(encoding="utf-8")
    assert source.count(old) == 1
    path = tmp_path / "cargo-release-inventory.toml"
    path.write_text(source.replace(old, new), encoding="utf-8")
    return path


def test_repository_inventory_closes_every_cargo_product_class() -> None:
    inventory = inventory_module.load_inventory()

    assert inventory.release_version == "2.1.0"
    assert inventory.first_party_msrv == "1.88"
    assert len(inventory.packages) == 21
    assert len(inventory.public_packages) == 19
    assert len(inventory.first_party_packages) == 17
    assert len(inventory.immutable_packages) == 2
    assert len(inventory.private_packages) == 2
    assert [package.publish_order for package in inventory.public_packages] == list(range(1, 20))
    assert {package.name for package in inventory.private_packages} == {
        "type-bridge-core",
        "type-bridge-node",
    }
    server = next(
        package
        for package in inventory.first_party_packages
        if package.name == "type-bridge-server"
    )
    assert server.public is True
    assert server.version == "2.1.0"
    assert server.documentation == "https://docs.rs/type-bridge-server/2.1.0"


def test_public_cargo_package_index_is_inventory_closed_and_linked() -> None:
    inventory = inventory_module.load_inventory()
    source = CARGO_PACKAGE_INDEX.read_text(encoding="utf-8")
    row_pattern = re.compile(
        r"^\| \[`(?P<name>[^`]+)`\]\((?P<crate>https://crates\.io/crates/[^)]+)\) "
        r"· \[rustdoc\]\((?P<docs>https://docs\.rs/[^)]+)\) \|",
        re.MULTILINE,
    )
    rows = {
        match.group("name"): (match.group("crate"), match.group("docs"))
        for match in row_pattern.finditer(source)
    }

    assert len(rows) == len(inventory.public_packages) == 19
    assert set(rows) == {package.name for package in inventory.public_packages}
    for package in inventory.public_packages:
        assert rows[package.name] == (
            f"https://crates.io/crates/{package.name}/{package.version}",
            package.documentation,
        )
    assert set(rows).isdisjoint(package.name for package in inventory.private_packages)

    nav = (ROOT / "mkdocs.yml").read_text(encoding="utf-8")
    readme = (ROOT / "README.md").read_text(encoding="utf-8")
    assert "Cargo Package Index: guide/cargo-packages.md" in nav
    assert "https://ds1sqe.github.io/type-bridge/guide/cargo-packages/" in readme
    assert len(readme.splitlines()) < 200


def test_inventory_classifies_the_real_workspace_without_omissions() -> None:
    inventory = inventory_module.load_inventory()
    result = subprocess.run(
        [
            "cargo",
            "metadata",
            "--manifest-path",
            str(ROOT / "type-bridge-core/Cargo.toml"),
            "--no-deps",
            "--format-version",
            "1",
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    metadata = json.loads(result.stdout)
    packages = {package["name"]: package for package in metadata["packages"]}

    assert set(packages) == {package.name for package in inventory.packages}
    for expected in inventory.packages:
        actual = packages[expected.name]
        manifest = Path(actual["manifest_path"]).relative_to(ROOT / "type-bridge-core")
        assert manifest.as_posix() == expected.manifest
        assert actual["version"] == expected.version
        if expected.classification == inventory_module.PUBLIC_FIRST_PARTY:
            assert actual["publish"] == ["crates-io"]
            assert actual["readme"] == expected.readme
            assert actual["documentation"] == expected.documentation
            assert actual["rust_version"] == inventory.first_party_msrv
        elif expected.classification == inventory_module.PUBLIC_IMMUTABLE:
            # Accepted compatibility sources remain byte-identical to their
            # upstream manifests; release tooling verifies them in place.
            assert actual["publish"] is None
        else:
            assert actual["publish"] == []


@pytest.mark.parametrize(
    ("old", "new", "message"),
    [
        (
            "schema-version = 1",
            "schema-version = 2",
            "schema-version must be 1",
        ),
        (
            'name = "type-bridge-node"',
            'name = "type-bridge-core"',
            "duplicate Cargo package name",
        ),
        (
            "publish-order = 19",
            "publish-order = 20",
            "publish-order must be contiguous",
        ),
        (
            'name = "type-bridge-core"\nmanifest = "crates/python/Cargo.toml"\nclassification = "private-binding"\nrole = "binding"\nversion-policy = "lockstep"\ndocs-target = "none"',
            'name = "type-bridge-core"\nmanifest = "crates/python/Cargo.toml"\nclassification = "private-binding"\nrole = "binding"\nversion-policy = "lockstep"\npublish-order = 20\ndocs-target = "none"',
            "private package type-bridge-core cannot have publish-order",
        ),
        (
            'version-policy = "fixed"\nversion = "3.11.0"',
            'version-policy = "fixed"\nversion = ""',
            "type-bridge-typedb-protocol-b8.version must be a non-empty string",
        ),
    ],
)
def test_inventory_rejects_identity_and_order_drift(
    tmp_path: Path,
    old: str,
    new: str,
    message: str,
) -> None:
    path = hostile_inventory(tmp_path, old, new)

    with pytest.raises(inventory_module.InventoryError, match=message):
        inventory_module.load_inventory(path)
