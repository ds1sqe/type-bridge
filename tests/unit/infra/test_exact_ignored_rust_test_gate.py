"""Fail-closed execution for individually selected ignored Rust tests."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path
from typing import Any

import pytest
import yaml

REPO_ROOT = Path(__file__).resolve().parents[3]
GATE = REPO_ROOT / "scripts/ci/run_exact_ignored_rust_test.sh"


def _run_gate(tmp_path: Path, selection: str) -> tuple[subprocess.CompletedProcess[str], list[str]]:
    fake_bin = tmp_path / "bin"
    fake_bin.mkdir()
    log = tmp_path / "cargo.log"
    cargo = fake_bin / "cargo"
    cargo.write_text(
        "#!/usr/bin/env bash\n"
        "set -euo pipefail\n"
        'printf \'%s\\n\' "$*" >>"$FAKE_CARGO_LOG"\n'
        'if [[ " $* " == *" --list "* ]]; then\n'
        "    printf '%s\\n' \"$FAKE_SELECTION\"\n"
        "fi\n",
        encoding="utf-8",
    )
    cargo.chmod(0o755)
    environment = {
        **os.environ,
        "FAKE_CARGO_LOG": str(log),
        "FAKE_SELECTION": selection,
        "PATH": f"{fake_bin}{os.pathsep}{os.environ['PATH']}",
    }
    completed = subprocess.run(
        [
            "bash",
            str(GATE),
            "selected_live_test",
            "--manifest-path",
            "workspace/Cargo.toml",
            "-p",
            "live-package",
            "--test",
            "live-target",
        ],
        cwd=tmp_path,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )
    calls = log.read_text(encoding="utf-8").splitlines() if log.exists() else []
    return completed, calls


@pytest.mark.parametrize(
    "selection",
    (
        "",
        "different_live_test: test",
        "selected_live_test: test\nselected_live_test: test",
    ),
)
def test_gate_rejects_zero_wrong_or_duplicate_test_selections(
    tmp_path: Path,
    selection: str,
) -> None:
    completed, calls = _run_gate(tmp_path, selection)

    assert completed.returncode == 1
    assert "expected exactly one ignored Rust test" in completed.stderr
    assert len(calls) == 1
    assert "--ignored --exact --list" in calls[0]


def test_gate_executes_only_after_one_exact_selection(tmp_path: Path) -> None:
    completed, calls = _run_gate(tmp_path, "selected_live_test: test")

    assert completed.returncode == 0, completed.stderr
    assert len(calls) == 2
    assert "--ignored --exact --list" in calls[0]
    assert "--ignored --exact --nocapture" in calls[1]


def test_ci_and_local_harness_route_cli_live_tests_through_the_gate() -> None:
    workflow = (REPO_ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
    local = (REPO_ROOT / "test.sh").read_text(encoding="utf-8")
    gate_call = "scripts/ci/run_exact_ignored_rust_test.sh"
    positive_tests = {
        "empty_workspace_to_replayed_history_live",
        "documented_examples_initial_constraints_apply_and_verify_live",
        "verify_never_creates_databases_live",
        "adopt_legacy_history_then_evolve_live",
        "shipped_python_converter_to_native_adoption_live",
    }

    assert workflow.count(gate_call) == len(positive_tests) + 3
    assert local.count(gate_call) == 5
    assert "-- --ignored --exact --nocapture" not in local
    for test_name in positive_tests | {"unsupported_server_apply_creates_neither_database_live"}:
        assert workflow.count(test_name) == 1
    for test_name in positive_tests:
        assert local.count(test_name) == 1


def test_release_named_ignored_rust_tests_use_the_exact_selection_gate() -> None:
    workflow = yaml.safe_load(
        (REPO_ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
    )
    steps: dict[str, dict[str, Any]] = {
        step["name"]: step for step in workflow["jobs"]["accept-server-oci"]["steps"]
    }
    gate_call = "scripts/ci/run_exact_ignored_rust_test.sh"

    for name, test_name in {
        "Run exact-image V1 compatibility and authenticated V2 query": (
            "production_binary_serves_v1_health_and_v2_query"
        ),
        "Run external generated Rust application through the exact image": (
            "generated_rust_projection_round_trips_exact_live_models"
        ),
    }.items():
        command = steps[name]["run"]
        assert command.count(gate_call) == 1
        assert command.count(test_name) == 1
        assert "-- --ignored --exact" not in command

    release_source = (REPO_ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
    assert release_source.count(gate_call) == 2
    assert "-- --ignored --exact --nocapture" not in release_source


def test_migration_live_lanes_are_bound_to_the_exact_server_leg() -> None:
    workflow = yaml.safe_load((REPO_ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8"))
    steps: dict[str, dict[str, Any]] = {
        step["name"]: step for step in workflow["jobs"]["rust-integration"]["steps"]
    }

    unsupported = steps["Prove unsupported migration apply creates no databases"]
    assert unsupported["if"] == "matrix.typedb-server == 'typedb/typedb:3.11.5'"
    assert "unsupported_server_apply_creates_neither_database_live" in unsupported["run"]

    for name, test_name in {
        "Run CLI empty-workspace replay lifecycle": "empty_workspace_to_replayed_history_live",
        "Apply and verify the unchanged documented initial constraints": (
            "documented_examples_initial_constraints_apply_and_verify_live"
        ),
    }.items():
        step = steps[name]
        assert step["if"] == "matrix.typedb-server == 'typedb/typedb:3.12.1'"
        assert test_name in step["run"]


def test_fresh_replay_and_documented_constraint_probes_cannot_regress_to_offline_only() -> None:
    source = (REPO_ROOT / "type-bridge-core/crates/cli/tests/e2e_workspace_live.rs").read_text(
        encoding="utf-8"
    )
    fresh = source.split("async fn empty_workspace_to_replayed_history_live()", maxsplit=1)[
        1
    ].split("async fn documented_examples_initial_constraints_apply_and_verify_live()", maxsplit=1)[
        0
    ]
    documented = source.split(
        "async fn documented_examples_initial_constraints_apply_and_verify_live()",
        maxsplit=1,
    )[1].split("async fn unsupported_server_apply_creates_neither_database_live()", maxsplit=1)[0]

    assert "ensure_database_exists" not in fresh
    assert fresh.count('&["migration", "apply", "--environment"') == 3
    assert '&["migration", "verify", "--environment", "replay"]' in fresh

    assert '"schema/application.yaml"' in documented
    assert "fs::read(source.join(relative))" in documented
    assert documented.count('&["migration", "apply", "--environment", "development"]') == 2
    assert '&["migration", "verify", "--environment", "development"]' in documented
