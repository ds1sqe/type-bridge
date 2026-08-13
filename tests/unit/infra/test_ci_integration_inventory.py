"""The ordinary Python integration tree must be represented in live CI."""

from __future__ import annotations

import re
from pathlib import Path

import yaml

REPO_ROOT = Path(__file__).resolve().parents[3]


def test_live_ci_matrix_covers_every_ordinary_integration_group() -> None:
    workflow = (REPO_ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
    match = re.search(r"^\s*test-group:\s*\n\s*\[([^]]+)]", workflow, re.MULTILINE)
    assert match is not None, "test-integration matrix group list is missing"
    configured = {entry.strip() for entry in match.group(1).split(",")}

    integration_root = REPO_ROOT / "tests/integration"
    dedicated = {"parity", "proxy"}
    ordinary = {
        directory.name
        for directory in integration_root.iterdir()
        if directory.is_dir()
        and not directory.name.startswith("__")
        and directory.name not in dedicated
        and any(directory.glob("test_*.py"))
    }

    assert configured == ordinary
    assert {"queries", "schema", "expressions", "session"} == configured


def test_rust_orm_live_suite_is_serialized_locally_and_in_ci() -> None:
    """Keep the shared-database suite serial at the test-harness boundary."""
    workflow = yaml.safe_load((REPO_ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8"))
    rust_steps = {step["name"]: step for step in workflow["jobs"]["rust-integration"]["steps"]}
    ci_command = " ".join(rust_steps["Run Rust ORM integration suite"]["run"].split())

    expected_tail = (
        "-p type-bridge-orm --features integration-tests "
        "--test integration -- --nocapture --test-threads=1"
    )
    assert expected_tail in ci_command
    assert ci_command.count("--test-threads=1") == 1

    local_source = (REPO_ROOT / "test.sh").read_text(encoding="utf-8")
    start = local_source.index(
        'run_step "cargo test -p type-bridge-orm --features integration-tests --test integration"'
    )
    end = local_source.index('printf "${BOLD}━━━ Production V2 server', start)
    local_command = " ".join(local_source[start:end].replace("\\\n", " ").split())

    assert expected_tail in local_command
    assert local_command.count("--test-threads=1") == 1


def test_ordered_four_binding_compiler_smoke_runs_after_node_build() -> None:
    """Run the combined toolchain smoke only where all four compilers exist."""
    workflow = yaml.safe_load((REPO_ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8"))
    node_steps = workflow["jobs"]["node-check"]["steps"]
    names = [step["name"] for step in node_steps]
    ordered_step = node_steps[names.index("Compile ordered packages for all four bindings")]
    ci_command = " ".join(ordered_step["run"].split())

    selector = "ordered_generated_packages_pass_all_four_language_compilers"
    assert selector in ci_command
    assert "-- --exact --ignored" in ci_command
    assert names.index("Build native module and TypeScript surface") < names.index(
        "Compile ordered packages for all four bindings"
    )

    for path in (REPO_ROOT / "test.sh", REPO_ROOT / "scripts/check.sh"):
        source = path.read_text(encoding="utf-8")
        assert source.count(selector) == 1
        assert source.index("npm run build") < source.index(selector)
        command = source[source.index(selector) : source.index(selector) + 160]
        assert "-- --exact --ignored" in command
