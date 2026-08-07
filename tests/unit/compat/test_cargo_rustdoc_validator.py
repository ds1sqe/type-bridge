"""Closed-contract tests for the first-party Cargo rustdoc gate."""

from __future__ import annotations

import importlib.util
import json
import re
import subprocess
import sys
from dataclasses import replace
from pathlib import Path
from types import ModuleType

import pytest

ROOT = Path(__file__).resolve().parents[3]
CORE = ROOT / "type-bridge-core"
INVENTORY_MODULE = ROOT / "scripts/ci/cargo_release_inventory.py"
VALIDATOR_MODULE = ROOT / "scripts/ci/validate_cargo_rustdoc.py"
VALIDATOR_COMMAND = "python scripts/ci/validate_cargo_rustdoc.py"


def load_module(name: str, path: Path) -> ModuleType:
    """Load one standalone CI helper without making scripts a package."""
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


inventory_module = load_module("cargo_release_inventory", INVENTORY_MODULE)
validator = load_module("validate_cargo_rustdoc", VALIDATOR_MODULE)


def synthetic_metadata() -> dict[str, object]:
    """Return target metadata for every classified workspace package."""
    packages: list[dict[str, object]] = []
    inventory = inventory_module.load_inventory()
    for package in inventory.packages:
        if package.name == "type-bridge-cli":
            targets = [
                {"name": "type_bridge_cli", "kind": ["lib"], "doc": True},
                {"name": "type-bridge", "kind": ["bin"], "doc": True},
            ]
        elif package.name == "type-bridge-orm-derive":
            targets = [{"name": "type_bridge_orm_derive", "kind": ["proc-macro"], "doc": True}]
        else:
            targets = [{"name": package.name.replace("-", "_"), "kind": ["lib"], "doc": True}]
        packages.append(
            {
                "name": package.name,
                "manifest_path": str(CORE / package.manifest),
                "targets": targets,
            }
        )
    return {"packages": packages}


def test_target_plan_is_exactly_the_first_party_public_inventory() -> None:
    inventory = inventory_module.load_inventory()

    targets = validator.plan_rustdoc_targets(
        inventory,
        synthetic_metadata(),
        workspace_root=CORE,
    )

    assert [target.package_name for target in targets] == [
        package.name for package in inventory.first_party_packages
    ]
    assert len(targets) == 17
    assert {target.package_name for target in targets}.isdisjoint(
        package.name for package in (*inventory.immutable_packages, *inventory.private_packages)
    )
    selectors = {target.package_name: target.selector for target in targets}
    assert selectors["type-bridge-cli"] == ("--lib",)
    assert all(selector == ("--lib",) for selector in selectors.values())


@pytest.mark.parametrize(
    ("package_name", "replacement", "message"),
    [
        (
            "type-bridge-contract",
            [{"name": "contract", "kind": ["bin"], "doc": True}],
            "requires exactly one library target",
        ),
        (
            "type-bridge-cli",
            [
                {"name": "type_bridge_cli", "kind": ["lib"], "doc": True},
                {"name": "type_bridge_cli_shadow", "kind": ["lib"], "doc": True},
                {"name": "type-bridge", "kind": ["bin"], "doc": True},
            ],
            "requires exactly one library target",
        ),
    ],
)
def test_target_plan_rejects_missing_or_ambiguous_documentation_targets(
    package_name: str,
    replacement: list[dict[str, object]],
    message: str,
) -> None:
    metadata = synthetic_metadata()
    packages = metadata["packages"]
    assert isinstance(packages, list)
    package = next(candidate for candidate in packages if candidate["name"] == package_name)
    package["targets"] = replacement

    with pytest.raises(validator.RustdocValidationError, match=message):
        validator.plan_rustdoc_targets(
            inventory_module.load_inventory(),
            metadata,
            workspace_root=CORE,
        )


def test_target_plan_rejects_manifest_and_documentation_policy_drift() -> None:
    inventory = inventory_module.load_inventory()
    metadata = synthetic_metadata()
    packages = metadata["packages"]
    assert isinstance(packages, list)
    contract = next(
        candidate for candidate in packages if candidate["name"] == "type-bridge-contract"
    )
    contract["manifest_path"] = str(CORE / "crates/query/Cargo.toml")

    with pytest.raises(validator.RustdocValidationError, match="manifest disagrees"):
        validator.plan_rustdoc_targets(inventory, metadata, workspace_root=CORE)

    first_party = inventory.first_party_packages[0]
    packages_with_none = tuple(
        replace(package, docs_target="none") if package.name == first_party.name else package
        for package in inventory.packages
    )
    invalid_inventory = replace(inventory, packages=packages_with_none)
    with pytest.raises(validator.RustdocValidationError, match="cannot have docs-target"):
        validator.plan_rustdoc_targets(
            invalid_inventory,
            synthetic_metadata(),
            workspace_root=CORE,
        )

    metadata = synthetic_metadata()
    packages = metadata["packages"]
    assert isinstance(packages, list)
    contract = next(
        candidate for candidate in packages if candidate["name"] == "type-bridge-contract"
    )
    contract_targets = contract["targets"]
    assert isinstance(contract_targets, list)
    contract_targets[0]["doc"] = False
    with pytest.raises(validator.RustdocValidationError, match="doc = false"):
        validator.plan_rustdoc_targets(inventory, metadata, workspace_root=CORE)


def test_rustdoc_probes_deny_warnings_and_collect_all_package_failures() -> None:
    inventory = inventory_module.load_inventory()
    targets = validator.plan_rustdoc_targets(
        inventory,
        synthetic_metadata(),
        workspace_root=CORE,
    )
    commands: list[tuple[str, ...]] = []

    def fake_runner(command: tuple[str, ...], **kwargs: object) -> subprocess.CompletedProcess[str]:
        commands.append(command)
        environment = kwargs["env"]
        assert isinstance(environment, dict)
        assert "RUSTDOCFLAGS" not in environment
        assert "CARGO_ENCODED_RUSTDOCFLAGS" not in environment
        assert environment["PYO3_USE_ABI3_FORWARD_COMPATIBILITY"] == "1"
        package_name = command[command.index("-p") + 1]
        returncode = 2 if package_name in {"type-bridge-contract", "type-bridge-migration"} else 0
        return subprocess.CompletedProcess(
            command,
            returncode,
            stdout="",
            stderr=f"{package_name} failed" if returncode else "",
        )

    failures = validator.run_rustdoc_probes(
        targets,
        cargo="cargo",
        workspace_manifest=CORE / "Cargo.toml",
        repository_root=ROOT,
        runner=fake_runner,
    )

    assert len(commands) == 17
    assert {failure.package_name for failure in failures} == {
        "type-bridge-contract",
        "type-bridge-migration",
    }
    assert {failure.phase for failure in failures} == {"rustdoc"}
    for command in commands:
        assert command[:5] == (
            "cargo",
            "rustdoc",
            "--locked",
            "--quiet",
            "--all-features",
        )
        assert "--no-deps" not in command
        assert command.count("--manifest-path") == 1
        assert command.count("-p") == 1
        assert command[-5:] == ("--", "-D", "warnings", "-D", "missing_docs")


def test_msrv_doctest_probes_cover_every_inventory_library() -> None:
    inventory = inventory_module.load_inventory()
    targets = validator.plan_rustdoc_targets(
        inventory,
        synthetic_metadata(),
        workspace_root=CORE,
    )
    commands: list[tuple[str, ...]] = []

    def fake_runner(command: tuple[str, ...], **_: object) -> subprocess.CompletedProcess[str]:
        commands.append(command)
        package_name = command[command.index("-p") + 1]
        returncode = 3 if package_name == "type-bridge-server" else 0
        return subprocess.CompletedProcess(command, returncode, stdout="", stderr="doctest failed")

    failures = validator.run_doctest_probes(
        targets,
        cargo="cargo",
        workspace_manifest=CORE / "Cargo.toml",
        repository_root=ROOT,
        toolchain="1.88.0",
        runner=fake_runner,
    )

    assert len(commands) == 17
    assert [(failure.package_name, failure.phase) for failure in failures] == [
        ("type-bridge-server", "doctest")
    ]
    for command in commands:
        assert command[:6] == (
            "cargo",
            "+1.88.0",
            "test",
            "--locked",
            "--quiet",
            "--all-features",
        )
        assert command[-1] == "--doc"


@pytest.mark.parametrize("toolchain", ("", " 1.88.0", "+1.88.0"))
def test_cargo_prefix_rejects_ambiguous_toolchain_overrides(toolchain: str) -> None:
    with pytest.raises(validator.RustdocValidationError, match="invalid Rust toolchain"):
        validator.cargo_prefix(cargo="cargo", toolchain=toolchain)


def test_metadata_loader_is_locked_and_rejects_invalid_json() -> None:
    commands: list[tuple[str, ...]] = []

    def fake_runner(command: tuple[str, ...], **_: object) -> subprocess.CompletedProcess[str]:
        commands.append(command)
        return subprocess.CompletedProcess(command, 0, stdout="not-json", stderr="")

    with pytest.raises(validator.RustdocValidationError, match="invalid JSON"):
        validator.load_workspace_metadata(
            cargo="cargo",
            workspace_manifest=CORE / "Cargo.toml",
            repository_root=ROOT,
            runner=fake_runner,
        )

    assert commands == [
        (
            "cargo",
            "metadata",
            "--locked",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
            str(CORE / "Cargo.toml"),
        )
    ]


def test_ci_release_and_local_rust_checks_share_the_closed_rustdoc_gate() -> None:
    ci = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
    release = (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
    local_checks = (ROOT / "scripts/check.sh").read_text(encoding="utf-8")

    def job_block(workflow: str, name: str) -> str:
        jobs = workflow.split("\njobs:\n", maxsplit=1)[1]
        header = re.search(rf"^  {re.escape(name)}:\n", jobs, re.MULTILINE)
        assert header is not None
        next_header = re.search(r"^  [a-z][a-z0-9-]*:\n", jobs[header.end() :], re.MULTILINE)
        end = header.end() + next_header.start() if next_header is not None else len(jobs)
        return jobs[header.start() : end]

    assert ci.count(VALIDATOR_COMMAND) == 2
    assert job_block(ci, "rust-check").count(VALIDATOR_COMMAND) == 2
    assert ci.count(f"{VALIDATOR_COMMAND} --toolchain 1.88.0") == 1
    assert release.count(VALIDATOR_COMMAND) == 1
    assert job_block(release, "validate-release-identity").count(VALIDATOR_COMMAND) == 1
    assert local_checks.count(VALIDATOR_COMMAND) == 2
    assert local_checks.count(f"{VALIDATOR_COMMAND} --toolchain 1.88.0") == 1
    run_rust = local_checks.split("run_rust() {", maxsplit=1)[1].split("\n}", maxsplit=1)[0]
    assert run_rust.count(VALIDATOR_COMMAND) == 2


def test_repository_metadata_resolves_the_declared_target_shape() -> None:
    """The checked-in inventory must still resolve against Cargo's real target graph."""
    result = subprocess.run(
        validator.cargo_metadata_command(
            cargo="cargo",
            workspace_manifest=CORE / "Cargo.toml",
        ),
        check=True,
        capture_output=True,
        text=True,
        cwd=ROOT,
    )

    metadata = json.loads(result.stdout)
    targets = validator.plan_rustdoc_targets(
        inventory_module.load_inventory(),
        metadata,
        workspace_root=CORE,
    )

    assert len(targets) == 17
    assert all(target.selector == ("--lib",) for target in targets)
    first_party_names = {target.package_name for target in targets}
    for package in metadata["packages"]:
        if package["name"] not in first_party_names:
            continue
        library = next(
            target
            for target in package["targets"]
            if set(target["kind"]) & validator.LIBRARY_TARGET_KINDS
        )
        source = Path(library["src_path"]).read_text(encoding="utf-8")
        assert "#![deny(missing_docs)]" in source, package["name"]
    cli = next(package for package in metadata["packages"] if package["name"] == "type-bridge-cli")
    cli_library = next(target for target in cli["targets"] if "lib" in target["kind"])
    assert cli_library["doc"] is True


def test_ignored_first_party_rust_examples_state_the_accepted_reason() -> None:
    """An ignored example must explain its live-service or abstract-implementor boundary."""
    inventory = inventory_module.load_inventory()
    ignored_fence = re.compile(r"^\s*//[!/]?\s*```(?:rust,)?ignore\s*$")
    unexplained: list[str] = []

    for package in inventory.first_party_packages:
        source_root = CORE / Path(package.manifest).parent / "src"
        for source_path in sorted(source_root.rglob("*.rs")):
            lines = source_path.read_text(encoding="utf-8").splitlines()
            for index, line in enumerate(lines):
                if ignored_fence.fullmatch(line) is None:
                    continue
                explanation = " ".join(lines[max(0, index - 5) : index]).casefold()
                if "ignored because" not in explanation:
                    relative = source_path.relative_to(ROOT)
                    unexplained.append(f"{relative}:{index + 1}")

    assert unexplained == []


def test_runnable_readme_rust_examples_are_part_of_the_doctest_surface() -> None:
    """Every compilable crates.io README example must run in the crate doctest lane."""
    inventory = inventory_module.load_inventory()
    runnable_fence = re.compile(r"^```rust$")
    ignored_fence = re.compile(r"^```rust,ignore$")

    for package in inventory.first_party_packages:
        package_root = CORE / Path(package.manifest).parent
        readme = (package_root / "README.md").read_text(encoding="utf-8")
        lines = readme.splitlines()
        for index, line in enumerate(lines):
            if ignored_fence.fullmatch(line) is None:
                continue
            explanation = " ".join(lines[max(0, index - 5) : index]).casefold()
            assert "ignored because" in explanation, package.name
        if not any(runnable_fence.fullmatch(line) for line in lines):
            continue
        crate_root = (package_root / "src/lib.rs").read_text(encoding="utf-8")
        assert '#[doc = include_str!("../README.md")]' in crate_root, package.name
