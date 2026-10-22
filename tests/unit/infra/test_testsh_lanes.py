"""`./test.sh --typedb`: per-line live lanes and the two-server upgrade lane."""

from __future__ import annotations

import os
import shutil
import signal
import subprocess
import time
from contextlib import suppress
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
TEST_SH = REPO_ROOT / "test.sh"

# Caller pins the lane option refuses or captures; cleared so the probe runs
# under a neutral environment.
PINNED = (
    "TYPEDB_IMAGE",
    "TYPEDB_PORT",
    "TYPEDB_HTTP_PORT",
    "TYPEDB_ADDRESS",
    "TYPEDB_TLS_PORT",
    "TYPEDB_TLS_HTTP_PORT",
    "TYPEDB_TLS_ADDRESS",
    "TYPEDB_TLS_ROOT_CA",
    "TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE",
    "TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE",
)


def _run(*arguments: str, **pins: str) -> subprocess.CompletedProcess[str]:
    environment = {
        name: value
        for name, value in os.environ.items()
        if name not in PINNED
        and not name.startswith(("TYPE_BRIDGE_SDK_", "TYPE_BRIDGE_PROJECTED_"))
        and name != "ACCEPTANCE_TARGET_DIR"
    }
    environment.update(pins)
    # Every case below must be refused before the offline tiers start.
    return subprocess.run(
        ["bash", str(TEST_SH), *arguments],
        cwd=REPO_ROOT,
        env=environment,
        capture_output=True,
        text=True,
        timeout=60,
        check=False,
    )


def test_unknown_line_is_refused_with_usage() -> None:
    result = _run("--typedb", "3.10.0")
    assert result.returncode == 1
    assert "--typedb must be one of 3.11.5 3.12.3 3.13.6 3.14.0 or all, got 3.10.0" in result.stderr
    assert "--typedb LINE" in result.stdout


def test_lane_option_needs_a_managed_server() -> None:
    result = _run("--no-isolated", "--typedb", "all")
    assert result.returncode == 2
    assert "with --no-isolated the running server is the lane" in result.stderr


@pytest.mark.parametrize(
    ("request_", "pins", "refused"),
    [
        ("3.12.3", {"TYPEDB_IMAGE": "typedb/typedb:custom"}, "TYPEDB_IMAGE"),
        (
            "3.13.6",
            {"TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE": "typedb-3.12.1/v1"},
            "TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE",
        ),
        ("all", {"TYPEDB_PORT": "1729"}, "TYPEDB_PORT"),
    ],
)
def test_lane_option_refuses_caller_pins(request_: str, pins: dict[str, str], refused: str) -> None:
    result = _run(f"--typedb={request_}", **pins)
    assert result.returncode == 2
    assert f"{refused} pin one server or profile; unset them with --typedb {request_}" in (
        result.stderr
    )


def test_offline_tiers_run_once_and_live_steps_run_per_lane() -> None:
    source = TEST_SH.read_text(encoding="utf-8")
    offline = source.index("# ── Offline tiers (always)")
    dispatch = source.index('    for typedb_lane in "${typedb_lanes[@]}"; do')
    assert source.index("run_live_lane() {") > offline
    assert source.index('run_step "cargo test --all-targets"') < dispatch
    assert source.count('run_step "pytest tests/unit/"') == 1
    assert 'if [[ "$typedb_request" == all ]]; then\n        run_upgrade_lane' in source
    # Each managed server is its own compose project and is torn down.
    assert 'lane_compose_project "typedb-$requested_server"' in source
    assert "trap stop_all_typedb EXIT" in source
    assert '$compose -f docker-compose.yml -p "$proj" down -v' in source


def test_live_step_gates_follow_the_lane_not_a_pinned_server() -> None:
    source = TEST_SH.read_text(encoding="utf-8")
    assert '"$typedb_server_version" == "3.12.3"' not in source
    lane = source[source.index("run_live_lane() {") : source.index("finish_sdk_evidence() {")]
    # Projected exact-live parity, C live, the fan-in skip notice, and the
    # staged C ABI plans with the four-language writes.
    assert lane.count('if [[ "$lane_in_sdk_matrix" == 1 ]]; then') == 4
    assert 'if [[ "$lane_migrations" == 1 ]]; then\n        run_live_migration_steps' in lane
    assert '[[ "$typedb_server_version" == 3.11.* ]]' in lane
    tls = source[source.index("run_lane_tls() {") :]
    assert 'if [[ "$lane_in_sdk_matrix" != 1 ]]; then' in tls


def test_upgrade_lane_always_runs_two_servers() -> None:
    source = TEST_SH.read_text(encoding="utf-8")
    upgrade = source[source.index("run_upgrade_lane() {") :]
    upgrade = upgrade[: upgrade.index("\n}\n")]
    assert 'start_typedb 3.12.3 "$from_project" 0' in upgrade
    assert 'start_typedb 3.13.6 "$to_project" 0' in upgrade
    assert "CONSOLE" not in upgrade
    assert "return 0" not in upgrade
    for variable in (
        "TYPE_BRIDGE_MIGRATION_UPGRADE_FROM=",
        "TYPE_BRIDGE_MIGRATION_UPGRADE_FROM_HTTP_PORT=",
        "TYPE_BRIDGE_MIGRATION_UPGRADE_TO=",
        "TYPE_BRIDGE_MIGRATION_UPGRADE_TO_HTTP_PORT=",
    ):
        assert variable in upgrade
    assert 'stop_typedb "$from_project"' in upgrade
    assert 'stop_typedb "$to_project"' in upgrade


@pytest.mark.parametrize("profile", ["typedb-3.12.1/v1", "typedb-3.13.6/v1"])
def test_migration_steps_forward_profile_and_gate_documented_examples(profile: str) -> None:
    source = TEST_SH.read_text(encoding="utf-8")
    function = source[source.index("run_live_migration_steps() {") :]
    function = function[: function.index("\n}\n") + 3]
    # Stub execution to exercise both branches without Cargo or a live server.
    probe = "\n".join(
        [
            'run_step() { printf "%s\\n" "$*"; }',
            function,
            'migration_profile_env=("TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE=$lane_profile")',
            "run_live_migration_steps",
        ]
    )
    result = subprocess.run(
        ["bash", "-c", probe],
        env={**os.environ, "lane_profile": profile},
        capture_output=True,
        text=True,
        check=True,
    )
    lines = result.stdout.splitlines()
    for label in ("connected rollback and reapply", "interrupted-plan fenced recovery"):
        command = next(line for line in lines if line.startswith(label))
        assert f"TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE={profile}" in command
    examples = "documented_examples_initial_constraints_apply_and_verify_live"
    if profile == "typedb-3.13.6/v1":
        assert any(line.startswith("cargo test") and examples in line for line in lines)
    else:
        assert not any(line.startswith("cargo test") and examples in line for line in lines)
        assert "examples require typedb-3.13.6/v1" in result.stdout


@pytest.mark.skipif(os.name != "posix" or shutil.which("timeout") is None, reason="GNU timeout")
def test_step_timeout_terminates_and_reaps_a_descendant(tmp_path: Path) -> None:
    source = TEST_SH.read_text()
    start = source.index("run_with_timeout() {")
    helper = source[start : source.index("\n}\n", start) + 3]
    ready = tmp_path / "ready"
    worker = tmp_path / "worker.sh"
    worker.write_text(
        "sleep 3600 &\n"
        "child=$!\n"
        "trap 'wait \"$child\"; exit 0' TERM\n"
        'printf \'%s %s\' "$PPID" "$child" > "$1"\n'
        'wait "$child"\n'
    )
    process = subprocess.Popen(
        [
            "bash",
            "-c",
            helper + '\nrun_with_timeout 1h bash "$1" "$2"',
            "_",
            str(worker),
            str(ready),
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )
    timeout_pid = None
    try:
        deadline = time.monotonic() + 60
        while not ready.exists() or not ready.read_text():
            assert process.poll() is None
            assert time.monotonic() < deadline, "timeout worker did not become ready"
            time.sleep(0.01)
        timeout_pid, descendant = map(int, ready.read_text().split())
        # Trigger the real timeout path after the descendant is known to exist.
        os.kill(timeout_pid, signal.SIGALRM)
        _, stderr = process.communicate(timeout=60)
        assert process.returncode == 124, stderr
        with pytest.raises(ProcessLookupError):
            os.kill(descendant, 0)
    finally:
        for group in (process.pid, timeout_pid):
            if group is not None:
                with suppress(ProcessLookupError):
                    os.killpg(group, signal.SIGKILL)
        process.wait(timeout=60)
