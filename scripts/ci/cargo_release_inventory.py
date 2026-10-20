#!/usr/bin/env python3
"""Load and validate the closed Cargo product inventory."""

from __future__ import annotations

import argparse
import re
import tomllib
from dataclasses import dataclass
from pathlib import Path

INVENTORY_PATH = Path(__file__).with_name("cargo_release_inventory.toml")
PUBLIC_FIRST_PARTY = "public-first-party"
PUBLIC_IMMUTABLE = "public-immutable"
PRIVATE_BINDING = "private-binding"
PRIVATE_TOOLING = "private-tooling"
CLASSIFICATIONS = frozenset(
    {PUBLIC_FIRST_PARTY, PUBLIC_IMMUTABLE, PRIVATE_BINDING, PRIVATE_TOOLING}
)
EXPECTED_PRIVATE_BINDINGS = frozenset(
    {
        "type-bridge-c",
        "type-bridge-core",
        "type-bridge-node",
    }
)
# Repository tooling that measures the product; never a dependency of it.
EXPECTED_PRIVATE_TOOLING = frozenset({"type-bridge-perf", "type-bridge-perf-stages"})
# The one way a package may link tooling: a private binding's optional
# dependency enabled only by this off-by-default feature.
PERF_STAGES_FEATURE = "perf-stages"


class InventoryError(RuntimeError):
    """The Cargo product inventory is malformed or incomplete."""


@dataclass(frozen=True)
class CargoInventoryPackage:
    """One classified workspace package."""

    name: str
    manifest: str
    classification: str
    role: str
    version_policy: str
    version: str
    publish_order: int | None
    docs_target: str
    registry_checksum: str | None

    @property
    def public(self) -> bool:
        """Return whether this package is part of the crates.io product."""
        return self.classification in {PUBLIC_FIRST_PARTY, PUBLIC_IMMUTABLE}

    @property
    def immutable(self) -> bool:
        """Return whether publication verifies an existing immutable package."""
        return self.classification == PUBLIC_IMMUTABLE

    @property
    def readme(self) -> str | None:
        """Return the required package-local README for public packages."""
        return "README.md" if self.public else None

    @property
    def documentation(self) -> str | None:
        """Return the canonical docs.rs landing URL for public packages."""
        return f"https://docs.rs/{self.name}/{self.version}" if self.public else None


@dataclass(frozen=True)
class CargoReleaseInventory:
    """Validated release-wide Cargo identity and package ordering."""

    release_version: str
    first_party_msrv: str
    candidate_toolchain: str
    repository: str
    packages: tuple[CargoInventoryPackage, ...]

    @property
    def public_packages(self) -> tuple[CargoInventoryPackage, ...]:
        """Return all public packages in dependency-safe publication order."""
        return tuple(
            sorted((p for p in self.packages if p.public), key=lambda p: p.publish_order or 0)
        )

    @property
    def first_party_packages(self) -> tuple[CargoInventoryPackage, ...]:
        """Return first-party public packages in publication order."""
        return tuple(p for p in self.public_packages if not p.immutable)

    @property
    def immutable_packages(self) -> tuple[CargoInventoryPackage, ...]:
        """Return immutable public compatibility packages in publication order."""
        return tuple(p for p in self.public_packages if p.immutable)

    @property
    def private_packages(self) -> tuple[CargoInventoryPackage, ...]:
        """Return every unpublished package: native bindings and tooling."""
        return tuple(p for p in self.packages if not p.public)

    @property
    def binding_packages(self) -> tuple[CargoInventoryPackage, ...]:
        """Return private native binding packages."""
        return tuple(p for p in self.packages if p.classification == PRIVATE_BINDING)

    @property
    def tooling_packages(self) -> tuple[CargoInventoryPackage, ...]:
        """Return private repository tooling packages."""
        return tuple(p for p in self.packages if p.classification == PRIVATE_TOOLING)


def _required_string(value: object, *, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise InventoryError(f"{label} must be a non-empty string")
    return value


def load_inventory(path: Path = INVENTORY_PATH) -> CargoReleaseInventory:
    """Read and structurally validate the Cargo product inventory."""
    try:
        payload = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        raise InventoryError(f"could not read Cargo inventory {path}: {error}") from error
    if payload.get("schema-version") != 1:
        raise InventoryError("Cargo inventory schema-version must be 1")
    release_version = _required_string(payload.get("release-version"), label="release-version")
    first_party_msrv = _required_string(payload.get("first-party-msrv"), label="first-party-msrv")
    candidate_toolchain = _required_string(
        payload.get("candidate-toolchain"), label="candidate-toolchain"
    )
    if re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", candidate_toolchain) is None:
        raise InventoryError("candidate-toolchain must be an exact numeric Rust version")
    repository = _required_string(payload.get("repository"), label="repository")
    raw_packages = payload.get("packages")
    if not isinstance(raw_packages, list):
        raise InventoryError("Cargo inventory packages must be an array")

    packages: list[CargoInventoryPackage] = []
    names: set[str] = set()
    manifests: set[str] = set()
    orders: set[int] = set()
    for index, raw in enumerate(raw_packages):
        if not isinstance(raw, dict):
            raise InventoryError(f"packages[{index}] must be a table")
        name = _required_string(raw.get("name"), label=f"packages[{index}].name")
        manifest = _required_string(raw.get("manifest"), label=f"{name}.manifest")
        classification = _required_string(raw.get("classification"), label=f"{name}.classification")
        if classification not in CLASSIFICATIONS:
            raise InventoryError(f"{name}.classification is unknown: {classification!r}")
        role = _required_string(raw.get("role"), label=f"{name}.role")
        version_policy = _required_string(raw.get("version-policy"), label=f"{name}.version-policy")
        if version_policy not in {"lockstep", "fixed"}:
            raise InventoryError(f"{name}.version-policy is unknown: {version_policy!r}")
        raw_version = raw.get("version")
        version = (
            release_version
            if version_policy == "lockstep"
            else _required_string(raw_version, label=f"{name}.version")
        )
        raw_order = raw.get("publish-order")
        public = classification in {PUBLIC_FIRST_PARTY, PUBLIC_IMMUTABLE}
        if public:
            if not isinstance(raw_order, int) or isinstance(raw_order, bool) or raw_order < 1:
                raise InventoryError(f"{name}.publish-order must be a positive integer")
            publish_order: int | None = raw_order
        elif raw_order is not None:
            raise InventoryError(f"private package {name} cannot have publish-order")
        else:
            publish_order = None
        docs_target = _required_string(raw.get("docs-target"), label=f"{name}.docs-target")
        if docs_target not in {"lib", "bin", "none"}:
            raise InventoryError(f"{name}.docs-target is unknown: {docs_target!r}")
        raw_registry_checksum = raw.get("registry-checksum")
        if classification == PUBLIC_IMMUTABLE:
            registry_checksum = _required_string(
                raw_registry_checksum,
                label=f"{name}.registry-checksum",
            )
            if re.fullmatch(r"[0-9a-f]{64}", registry_checksum) is None:
                raise InventoryError(f"{name}.registry-checksum must be a lowercase SHA-256")
        elif raw_registry_checksum is not None:
            raise InventoryError(f"non-immutable package {name} cannot declare registry-checksum")
        else:
            registry_checksum = None
        if name in names:
            raise InventoryError(f"duplicate Cargo package name: {name}")
        if manifest in manifests:
            raise InventoryError(f"duplicate Cargo manifest path: {manifest}")
        if publish_order is not None and publish_order in orders:
            raise InventoryError(f"duplicate Cargo publish-order: {publish_order}")
        names.add(name)
        manifests.add(manifest)
        if publish_order is not None:
            orders.add(publish_order)
        packages.append(
            CargoInventoryPackage(
                name=name,
                manifest=manifest,
                classification=classification,
                role=role,
                version_policy=version_policy,
                version=version,
                publish_order=publish_order,
                docs_target=docs_target,
                registry_checksum=registry_checksum,
            )
        )

    inventory = CargoReleaseInventory(
        release_version=release_version,
        first_party_msrv=first_party_msrv,
        candidate_toolchain=candidate_toolchain,
        repository=repository,
        packages=tuple(packages),
    )
    expected_orders = set(range(1, len(inventory.public_packages) + 1))
    if orders != expected_orders:
        raise InventoryError(
            f"public publish-order must be contiguous: expected {sorted(expected_orders)}, "
            f"found {sorted(orders)}"
        )
    if len(inventory.first_party_packages) != 17:
        raise InventoryError("Cargo inventory must contain exactly 17 first-party public packages")
    if len(inventory.immutable_packages) != 2:
        raise InventoryError("Cargo inventory must contain exactly two immutable packages")
    private_bindings = frozenset(package.name for package in inventory.binding_packages)
    if private_bindings != EXPECTED_PRIVATE_BINDINGS:
        raise InventoryError(
            "Cargo inventory private binding package set must be exact: "
            f"expected {sorted(EXPECTED_PRIVATE_BINDINGS)!r}, "
            f"found {sorted(private_bindings)!r}"
        )
    private_tooling = frozenset(package.name for package in inventory.tooling_packages)
    if private_tooling != EXPECTED_PRIVATE_TOOLING:
        raise InventoryError(
            "Cargo inventory private tooling package set must be exact: "
            f"expected {sorted(EXPECTED_PRIVATE_TOOLING)!r}, "
            f"found {sorted(private_tooling)!r}"
        )
    return inventory


def _dependency_tables(manifest: dict[str, object]) -> list[tuple[str, dict[str, object]]]:
    tables: list[tuple[str, dict[str, object]]] = []
    for kind in ("dependencies", "dev-dependencies", "build-dependencies"):
        table = manifest.get(kind)
        if isinstance(table, dict):
            tables.append((kind, table))
    targets = manifest.get("target")
    if isinstance(targets, dict):
        for target, entry in targets.items():
            if isinstance(entry, dict):
                for kind, table in _dependency_tables(entry):
                    tables.append((f"target.{target}.{kind}", table))
    return tables


def _names(dependency: str, specification: object) -> set[str]:
    package = specification.get("package") if isinstance(specification, dict) else None
    return {dependency, package} if isinstance(package, str) else {dependency}


def tooling_dependency_violations(inventory: CargoReleaseInventory, workspace: Path) -> list[str]:
    """Return every manifest dependency on private tooling the product forbids.

    ``workspace`` is the directory the inventory's manifest paths are relative
    to. Only a private binding may depend on tooling, and only as an optional
    normal dependency without default features that the binding's
    ``perf-stages`` feature alone enables; ``default`` must not enable it.
    """
    tooling = {package.name for package in inventory.tooling_packages}
    violations: list[str] = []
    for package in inventory.packages:
        if package.classification == PRIVATE_TOOLING:
            continue
        try:
            manifest = tomllib.loads((workspace / package.manifest).read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
            violations.append(f"{package.manifest}: cannot read: {error}")
            continue
        features = manifest.get("features")
        features = features if isinstance(features, dict) else {}
        for kind, table in _dependency_tables(manifest):
            for dependency, specification in table.items():
                linked = _names(dependency, specification) & tooling
                if not linked:
                    continue
                where = f"{package.manifest}: {kind}.{dependency}"
                if package.classification != PRIVATE_BINDING:
                    violations.append(f"{where}: only a private binding may link tooling")
                    continue
                if (
                    kind != "dependencies"
                    or not isinstance(specification, dict)
                    or specification.get("optional") is not True
                    or specification.get("default-features") is not False
                ):
                    violations.append(
                        f"{where}: must be an optional normal dependency with "
                        "default-features = false"
                    )
                enabling = sorted(
                    feature
                    for feature, members in features.items()
                    if isinstance(members, list)
                    and any(
                        isinstance(member, str)
                        and member.removeprefix("dep:").split("/")[0].removesuffix("?")
                        == dependency
                        for member in members
                    )
                )
                if enabling != [PERF_STAGES_FEATURE]:
                    violations.append(
                        f"{where}: must be enabled by {PERF_STAGES_FEATURE!r} alone, "
                        f"found {enabling!r}"
                    )
                default = features.get("default", [])
                if isinstance(default, list) and PERF_STAGES_FEATURE in default:
                    violations.append(f"{where}: {PERF_STAGES_FEATURE!r} must be off by default")
    return violations


def main() -> int:
    """Print one inventory view for shell release tooling."""
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "view",
        choices=("public", "publish", "preexisting", "first-party", "private", "manifests"),
    )
    args = parser.parse_args()
    inventory = load_inventory()
    if args.view == "public":
        values = (package.name for package in inventory.public_packages)
    elif args.view == "publish":
        values = (package.name for package in inventory.public_packages if not package.immutable)
    elif args.view == "preexisting":
        values = (package.name for package in inventory.immutable_packages)
    elif args.view == "first-party":
        values = (package.name for package in inventory.first_party_packages)
    elif args.view == "private":
        values = (package.name for package in inventory.private_packages)
    else:
        values = (package.manifest for package in inventory.packages)
    print("\n".join(values))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
