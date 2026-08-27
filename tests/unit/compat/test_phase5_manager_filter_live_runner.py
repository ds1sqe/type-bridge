"""Provider-free checks for the focused Phase-5 manager live fan-in."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
RUNNER_PATH = ROOT / "scripts/ci/run_phase5_manager_filter_live.py"


def load_runner():
    spec = importlib.util.spec_from_file_location("phase5_manager_filter_live_runner", RUNNER_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_command_plan_is_one_four_binding_fan_in(tmp_path: Path) -> None:
    runner = load_runner()
    layout = runner.Layout.under(tmp_path.resolve(), "0123456789abcdef01234567")
    fixture = runner.Fixture(address="127.0.0.1:1729", http_port="8000")
    plan = runner.command_plan(layout, fixture)

    assert [command.label for command in plan] == [
        "install the Python native facade",
        "verify exact fixture and four initially absent databases",
        "generate the local Python package",
        "generate the foreign Python package",
        "produce the Python report",
        "verify the Python database was removed",
        "build the Node package",
        "generate the local TypeScript package",
        "generate the foreign TypeScript package",
        "compile the local TypeScript package",
        "compile the foreign TypeScript package",
        "produce the Node report",
        "verify the Node database was removed",
        "produce the Rust report",
        "verify the Rust database was removed",
        "build the C shared library",
        "produce the C report",
        "verify the C database was removed",
        "compare exactly four focused live reports",
    ]
    reports = tuple(str(path) for path in layout.report_paths())
    assert plan[-1].arguments[2:] == reports
    assert plan[1].arguments[-4:] == layout.database_names()
    assert "rust_phase5_manager_live" in plan[13].arguments
    assert "c_phase5_manager_live" in plan[16].arguments
    c_build = next(command for command in plan if command.label == "build the C shared library")
    c_producer = next(command for command in plan if command.label == "produce the C report")
    assert c_build.environment is not None and c_producer.environment is not None
    assert (
        c_build.environment[runner.ACCEPTANCE_TARGET_ENV]
        == c_producer.environment[runner.ACCEPTANCE_TARGET_ENV]
    )
    assert c_build.environment["CARGO_TARGET_DIR"] == c_producer.environment["CARGO_TARGET_DIR"]


def test_each_producer_has_one_unique_report_and_database(tmp_path: Path) -> None:
    runner = load_runner()
    layout = runner.Layout.under(tmp_path.resolve(), "0123456789abcdef01234567")
    fixture = runner.Fixture(address="127.0.0.1:1729", http_port="8000")
    plan = runner.command_plan(layout, fixture)
    producers = {
        command.label: command
        for command in plan
        if command.label
        in {
            "produce the Python report",
            "produce the Node report",
            "produce the Rust report",
            "produce the C report",
        }
    }
    assert len(producers) == 4
    reports = []
    databases = []
    for command in producers.values():
        assert command.environment is not None
        reports.append(command.environment[runner.REPORT_ENV])
        databases.append(command.environment[runner.DATABASE_ENV])
        assert command.environment[runner.ADDRESS_ENV] == fixture.address
        assert command.environment[runner.HTTP_PORT_ENV] == fixture.http_port
        assert command.environment[runner.REPOSITORY_ENV] == str(ROOT)
    assert len(set(reports)) == len(set(databases)) == 4
    assert all(len(name) <= 64 for name in databases)


def test_prepare_mutates_only_selected_foreign_field(tmp_path: Path) -> None:
    runner = load_runner()
    layout = runner.Layout.under(tmp_path.resolve(), "0123456789abcdef01234567")
    runner._prepare(layout)
    local = runner.SCHEMA.read_text(encoding="utf-8")
    foreign = layout.foreign_schema.read_text(encoding="utf-8")

    assert local.count(runner.FOREIGN_SCHEMA_NEEDLE) == 1
    assert runner.FOREIGN_SCHEMA_NEEDLE not in foreign
    assert foreign.count(runner.FOREIGN_SCHEMA_REPLACEMENT) == 1
    assert foreign == local.replace(
        runner.FOREIGN_SCHEMA_NEEDLE, runner.FOREIGN_SCHEMA_REPLACEMENT, 1
    )
    assert (layout.node / "node_modules/@type-bridge/node").is_symlink()


def test_runner_validates_all_committed_producer_sources() -> None:
    runner = load_runner()
    contract = runner._load_live_contract()

    runner._validate_producer_sources(contract)
    assert runner.PRODUCER_SOURCES == (
        ("Python producer", runner.PYTHON_PRODUCER),
        ("Node producer", runner.NODE_PRODUCER),
        ("Rust harness", runner.RUST_HARNESS),
        ("Rust producer", runner.RUST_PRODUCER),
        ("C harness", runner.C_HARNESS),
        ("C consumer", runner.C_CONSUMER),
        ("C setup", runner.C_SETUP),
    )


def test_fixture_requires_only_caller_endpoint() -> None:
    runner = load_runner()
    valid = {
        runner.ADDRESS_ENV: "127.0.0.1:1729",
        runner.HTTP_PORT_ENV: "8000",
    }
    assert runner._required_fixture(valid) == runner.Fixture(
        address="127.0.0.1:1729", http_port="8000"
    )
    for owned in runner.RUNNER_OWNED_ENV:
        with pytest.raises(runner.RunnerError, match="runner-owned"):
            runner._required_fixture({**valid, owned: "caller-value"})
    for hostile in (
        {},
        {runner.ADDRESS_ENV: "https://127.0.0.1:1729", runner.HTTP_PORT_ENV: "8000"},
        {runner.ADDRESS_ENV: "127.0.0.1:1729", runner.HTTP_PORT_ENV: "0"},
        {runner.ADDRESS_ENV: "127.0.0.1:1729", runner.HTTP_PORT_ENV: "8.0"},
    ):
        with pytest.raises(runner.RunnerError):
            runner._required_fixture(hostile)


def test_runner_has_one_ephemeral_stage_and_no_promotion_wiring() -> None:
    source = RUNNER_PATH.read_text(encoding="utf-8")

    assert source.count("tempfile.TemporaryDirectory(") == 1
    assert "secrets.token_hex(12)" in source
    assert "compare_phase5_manager_filter_live.py" in source
    assert "catalog" not in source.casefold()
    assert ".github/workflows" not in source
    assert "scripts/check.sh" not in source
