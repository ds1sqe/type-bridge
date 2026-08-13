"""Fail-closed coverage for the exact-TypeDB four-binding Phase-2 live fan-in."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
RUNNER_PATH = ROOT / "scripts/ci/run_phase2_projection_live.py"
CHECK = ROOT / "scripts/check.sh"
TEST = ROOT / "test.sh"
CI = ROOT / ".github/workflows/ci.yml"


def load_runner():
    spec = importlib.util.spec_from_file_location("phase2_projection_live_runner", RUNNER_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_command_plan_runs_four_live_producers_then_exactly_one_comparator(
    tmp_path: Path,
) -> None:
    runner = load_runner()
    layout = runner.Layout.under(tmp_path.resolve(), "0123456789abcdef01234567")
    fixture = runner.Fixture(address="127.0.0.1:32942", http_port="32943")
    plan = runner.command_plan(layout, fixture)

    assert [command.label for command in plan] == [
        "install the Python native facade",
        "verify exact fixture and four initially absent databases",
        "generate the Python package",
        "produce the Python report",
        "verify the Python database was removed",
        "build the Node package",
        "generate the TypeScript package",
        "compile the TypeScript package",
        "produce the Node report",
        "verify the Node database was removed",
        "produce the Rust report",
        "verify the Rust database was removed",
        "build the C shared library",
        "produce the C report",
        "verify the C database was removed",
        "compare exactly four live reports",
    ]
    reports = tuple(str(path) for path in layout.report_paths())
    databases = layout.database_names()
    assert len(set(reports)) == len(set(databases)) == 4
    assert all(Path(report).is_absolute() and not Path(report).exists() for report in reports)
    assert all(
        len(database) <= 64 and database.isascii() and database.replace("_", "").isalnum()
        for database in databases
    )

    producers = {
        command.label: command for command in plan if command.label.startswith("produce the ")
    }
    expected = {
        "produce the Python report": (reports[0], databases[0]),
        "produce the Node report": (reports[1], databases[1]),
        "produce the Rust report": (reports[2], databases[2]),
        "produce the C report": (reports[3], databases[3]),
    }
    for label, (report, database) in expected.items():
        environment = producers[label].environment
        assert environment is not None
        assert environment[runner.REPORT_ENV] == report
        assert environment[runner.DATABASE_ENV] == database
        assert environment[runner.ADDRESS_ENV] == fixture.address
        assert environment[runner.HTTP_PORT_ENV] == fixture.http_port
        assert environment[runner.REPOSITORY_ENV] == str(ROOT)
        assert producers[label].timeout_seconds <= 1200
    assert producers["produce the Rust report"].environment[runner.ACCEPTANCE_TARGET_ENV] == str(
        layout.scratch / "rust-target"
    )
    assert producers["produce the C report"].environment[runner.ACCEPTANCE_TARGET_ENV] == str(
        layout.scratch / "c-target"
    )
    assert layout.scratch / "rust-target" != layout.scratch / "c-target"

    assert plan[1].arguments[-4:] == databases
    assert (
        "generated_rust_phase2_live_producer_runs_when_explicitly_configured" in plan[10].arguments
    )
    assert "generated_c17_phase2_live_subset_round_trips_exact_3_12_1" in plan[13].arguments
    assert plan[-1].arguments[1] == str(runner.COMPARATOR)
    assert plan[-1].arguments[2:] == reports


def test_runner_validates_every_committed_producer_before_tools_or_staging() -> None:
    runner = load_runner()
    contract = runner._load_live_contract()

    runner._validate_producer_sources(contract)
    assert runner.PRODUCER_SOURCES == (
        ("Python producer", runner.PYTHON_PRODUCER),
        ("Node producer", runner.NODE_PRODUCER),
        ("Rust producer", runner.RUST_PRODUCER),
        ("C harness", runner.C_HARNESS),
        ("C consumer", runner.C_CONSUMER),
        ("C setup", runner.C_SETUP),
    )
    source = RUNNER_PATH.read_text(encoding="utf-8")
    run_source = source[source.index("def run(") :]
    assert run_source.index("_validate_producer_sources(contract)") < run_source.index(
        "_require_tools()"
    )
    assert run_source.index("_require_tools()") < run_source.index("tempfile.TemporaryDirectory(")


def test_runner_uses_one_stage_and_makes_no_ordered_instance_claim() -> None:
    source = RUNNER_PATH.read_text(encoding="utf-8")

    assert source.count("tempfile.TemporaryDirectory(") == 1
    assert "secrets.token_hex(12)" in source
    assert "compare_phase2_projection_live.py" in source
    assert "ordered_distinct_collections" not in source
    assert "ordered_list_persistence" not in source
    assert "persisted_order" not in source


def test_fixture_requires_caller_endpoint_and_rejects_runner_owned_overrides() -> None:
    runner = load_runner()
    valid = {
        runner.ADDRESS_ENV: "127.0.0.1:32942",
        runner.HTTP_PORT_ENV: "32943",
    }

    assert runner._required_fixture(valid) == runner.Fixture(
        address="127.0.0.1:32942",
        http_port="32943",
    )
    for owned in runner.RUNNER_OWNED_ENV:
        with pytest.raises(runner.RunnerError, match="runner-owned"):
            runner._required_fixture({**valid, owned: "caller-value"})
    for hostile in (
        {},
        {runner.ADDRESS_ENV: "https://127.0.0.1:32942", runner.HTTP_PORT_ENV: "32943"},
        {runner.ADDRESS_ENV: "127.0.0.1:32942", runner.HTTP_PORT_ENV: "0"},
        {runner.ADDRESS_ENV: "127.0.0.1:32942", runner.HTTP_PORT_ENV: "8000.0"},
    ):
        with pytest.raises(runner.RunnerError):
            runner._required_fixture(hostile)


def test_runner_fails_closed_when_a_required_tool_is_missing(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runner = load_runner()
    monkeypatch.setattr(
        runner.shutil,
        "which",
        lambda executable: None if executable == "clang" else f"/tools/{executable}",
    )

    with pytest.raises(runner.RunnerError, match="unavailable: clang"):
        runner._require_tools()


def test_runner_preserves_subprocess_logs_and_enforces_timeout(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runner = load_runner()
    command = runner.CommandSpec("hostile live producer", ("false",), ROOT, 38)

    class FakeProcess:
        pid = 4242

        def __init__(self, events, returncode):
            self.events = list(events)
            self.returncode = returncode

        def communicate(self, *, timeout):
            assert timeout in {10, 38}
            event = self.events.pop(0)
            if isinstance(event, BaseException):
                raise event
            return event

    popen_arguments = []
    normal = FakeProcess([("producer output", "producer failure")], 38)

    def popen(*args, **kwargs):
        popen_arguments.append((args, kwargs))
        return normal

    monkeypatch.setattr(runner.subprocess, "Popen", popen)

    with pytest.raises(runner.RunnerError, match="producer output") as failure:
        runner._run(command)
    assert "producer failure" in str(failure.value)
    assert popen_arguments[0][1]["start_new_session"] is True

    timed_out = FakeProcess(
        [
            subprocess.TimeoutExpired(command.arguments, 38, "initial out", "initial err"),
            ("partial out", "partial err"),
            ("partial out", "partial err"),
        ],
        -runner.signal.SIGKILL,
    )
    signals = []
    group_probes = 0

    def killpg(process_group, signal_number):
        nonlocal group_probes
        signals.append((process_group, signal_number))
        if signal_number == 0:
            group_probes += 1
            if group_probes > 1:
                raise ProcessLookupError

    monkeypatch.setattr(runner.subprocess, "Popen", lambda *args, **kwargs: timed_out)
    monkeypatch.setattr(runner.os, "killpg", killpg)
    with pytest.raises(runner.RunnerError, match="exceeded 38 seconds") as failure:
        runner._run(command)
    assert "partial out" in str(failure.value)
    assert "partial err" in str(failure.value)
    assert signals == [
        (timed_out.pid, runner.signal.SIGTERM),
        (timed_out.pid, 0),
        (timed_out.pid, runner.signal.SIGKILL),
        (timed_out.pid, 0),
    ]


def test_runner_cleans_owned_names_after_interrupt(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runner = load_runner()
    fixture_environment = {
        runner.ADDRESS_ENV: "127.0.0.1:32942",
        runner.HTTP_PORT_ENV: "32943",
    }
    initial_guard = runner.CommandSpec(
        "verify exact fixture and four initially absent databases",
        ("guard",),
        ROOT,
        38,
    )
    interrupted = runner.CommandSpec("interrupted producer", ("producer",), ROOT, 38)
    commands = iter(
        [
            subprocess.CompletedProcess(initial_guard.arguments, 0, "", ""),
            KeyboardInterrupt(),
        ]
    )
    cleanup_calls = []

    monkeypatch.setattr(runner, "_load_live_contract", lambda: object())
    monkeypatch.setattr(runner, "_validate_producer_sources", lambda contract: None)
    monkeypatch.setattr(runner, "_require_tools", lambda: None)
    monkeypatch.setattr(runner, "_prepare", lambda layout: None)
    monkeypatch.setattr(
        runner, "command_plan", lambda layout, fixture: (initial_guard, interrupted)
    )

    def run_command(command):
        event = next(commands)
        if isinstance(event, BaseException):
            raise event
        return event

    monkeypatch.setattr(runner, "_run", run_command)
    monkeypatch.setattr(
        runner,
        "_cleanup_after_failure",
        lambda layout, fixture: cleanup_calls.append(layout.database_names()) or "\ncleaned",
    )

    with pytest.raises(runner.RunnerError, match="interrupted by KeyboardInterrupt") as failure:
        runner.run(fixture_environment)
    assert "cleaned" in str(failure.value)
    assert len(cleanup_calls) == 1
    assert len(set(cleanup_calls[0])) == 4


# Fail-closed subprocess state table:
# - interrupted before Popen returns: no PGID is knowable -> Unsafe, no cleanup;
# - interrupted/timed out after Popen: terminate, drain, and prove PGID absent;
# - interrupted again during termination: stop is unproven -> Unsafe, no cleanup;
# - normal leader exit with a surviving descendant: kill/prove, then fail the command.
def test_runner_never_cleans_when_process_group_termination_is_interrupted(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runner = load_runner()
    fixture_environment = {
        runner.ADDRESS_ENV: "127.0.0.1:32942",
        runner.HTTP_PORT_ENV: "32943",
    }
    initial_guard = runner.CommandSpec(
        "verify exact fixture and four initially absent databases",
        ("guard",),
        ROOT,
        38,
    )
    unsafe = runner.CommandSpec("unsafe producer", ("producer",), ROOT, 38)
    cleanup_calls = []

    class FakeProcess:
        def __init__(self, pid, event, returncode):
            self.pid = pid
            self.event = event
            self.returncode = returncode

        def communicate(self, *, timeout):
            assert timeout == 38
            if isinstance(self.event, BaseException):
                raise self.event
            return self.event

    processes = iter(
        [
            FakeProcess(4242, ("", ""), 0),
            FakeProcess(4343, KeyboardInterrupt(), None),
        ]
    )

    monkeypatch.setattr(runner, "_load_live_contract", lambda: object())
    monkeypatch.setattr(runner, "_validate_producer_sources", lambda contract: None)
    monkeypatch.setattr(runner, "_require_tools", lambda: None)
    monkeypatch.setattr(runner, "_prepare", lambda layout: None)
    monkeypatch.setattr(runner, "command_plan", lambda layout, fixture: (initial_guard, unsafe))
    monkeypatch.setattr(runner.subprocess, "Popen", lambda *args, **kwargs: next(processes))
    monkeypatch.setattr(
        runner,
        "_terminate_process_group",
        lambda process: (_ for _ in ()).throw(KeyboardInterrupt()),
    )
    monkeypatch.setattr(
        runner,
        "_cleanup_after_failure",
        lambda layout, fixture: cleanup_calls.append(layout.database_names()) or "\ncleaned",
    )

    with pytest.raises(
        runner.UnsafeProcessGroupError, match="termination was interrupted"
    ) as failure:
        runner.run(fixture_environment)
    assert isinstance(failure.value.__cause__, KeyboardInterrupt)
    assert cleanup_calls == []


def test_runner_never_cleans_after_interrupted_popen_launch(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runner = load_runner()
    fixture_environment = {
        runner.ADDRESS_ENV: "127.0.0.1:32942",
        runner.HTTP_PORT_ENV: "32943",
    }
    initial_guard = runner.CommandSpec(
        "verify exact fixture and four initially absent databases",
        ("guard",),
        ROOT,
        38,
    )
    interrupted = runner.CommandSpec("interrupted launch", ("producer",), ROOT, 38)
    cleanup_calls = []

    class GuardProcess:
        pid = 4242
        returncode = 0

        def communicate(self, *, timeout):
            assert timeout == 38
            return "", ""

    launches = iter((GuardProcess(), KeyboardInterrupt()))

    monkeypatch.setattr(runner, "_load_live_contract", lambda: object())
    monkeypatch.setattr(runner, "_validate_producer_sources", lambda contract: None)
    monkeypatch.setattr(runner, "_require_tools", lambda: None)
    monkeypatch.setattr(runner, "_prepare", lambda layout: None)
    monkeypatch.setattr(
        runner, "command_plan", lambda layout, fixture: (initial_guard, interrupted)
    )
    monkeypatch.setattr(runner, "_process_group_exists", lambda process: False)

    def popen(*args, **kwargs):
        event = next(launches)
        if isinstance(event, BaseException):
            raise event
        return event

    monkeypatch.setattr(runner.subprocess, "Popen", popen)
    monkeypatch.setattr(
        runner,
        "_cleanup_after_failure",
        lambda layout, fixture: cleanup_calls.append(layout.database_names()) or "\ncleaned",
    )

    with pytest.raises(runner.UnsafeProcessGroupError, match="launch was interrupted") as failure:
        runner.run(fixture_environment)
    assert isinstance(failure.value.__cause__, KeyboardInterrupt)
    assert cleanup_calls == []


def test_runner_stops_a_descendant_left_after_normal_leader_exit(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runner = load_runner()
    command = runner.CommandSpec("leaky producer", ("producer",), ROOT, 38)

    class LeakyProcess:
        pid = 4242
        returncode = 0

        def communicate(self, *, timeout):
            assert timeout == 38
            return "producer output", "producer error"

    process = LeakyProcess()
    stopped = []
    monkeypatch.setattr(runner.subprocess, "Popen", lambda *args, **kwargs: process)
    monkeypatch.setattr(runner, "_process_group_exists", lambda actual: actual is process)
    monkeypatch.setattr(
        runner,
        "_terminate_process_group_or_unsafe",
        lambda actual: stopped.append(actual) or ("producer output", "producer error"),
    )

    with pytest.raises(runner.RunnerError, match="left a live descendant process group"):
        runner._run(command)
    assert stopped == [process]


def test_check_local_integration_and_ci_persist_the_same_exact_live_gate() -> None:
    check = CHECK.read_text(encoding="utf-8")
    local = TEST.read_text(encoding="utf-8")
    ci = CI.read_text(encoding="utf-8")

    assert "run_phase2_live()" in check
    assert "phase2-live) run_phase2_live" in check
    all_dispatch = next(line for line in check.splitlines() if line.lstrip().startswith("all)"))
    assert "run_phase2_live" not in all_dispatch
    for owned in (
        "TYPE_BRIDGE_PHASE2_LIVE_REPORT",
        "TYPE_BRIDGE_PHASE2_LIVE_DATABASE",
        "ACCEPTANCE_TARGET_DIR",
    ):
        assert owned in local
    local_gate = local.index("scripts/ci/run_phase2_projection_live.py")
    exact_detection = local.index('if [[ "$typedb_server_version" == "3.12.1" ]]')
    rust_integration = local.index('printf "${BOLD}━━━ Rust (integration)')
    assert exact_detection < local_gate < rust_integration

    live_job = ci.split("  phase2-live:", maxsplit=1)[1].split(
        "  # I6 single-band V2 legs", maxsplit=1
    )[0]
    assert "runs-on: ubuntu-latest" in live_job
    assert "image: typedb/typedb:3.12.1" in live_job
    assert "./scripts/check.sh phase2-live" in live_job
    assert "docker logs ${{ job.services.typedb.id }}" in live_job
