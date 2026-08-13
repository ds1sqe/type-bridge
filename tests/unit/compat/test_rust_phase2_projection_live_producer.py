"""Fail-closed contract tests for Rust's exact-3.12.1 Phase-2 live producer."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path
from types import ModuleType

ROOT = Path(__file__).resolve().parents[3]
PRODUCER_PATH = ROOT / "type-bridge-core/crates/schema-codegen/tests/rust_acceptance/phase2_live.rs"
HARNESS_PATH = ROOT / "type-bridge-core/crates/schema-codegen/tests/rust_phase2_live.rs"
COMPARATOR_PATH = ROOT / "scripts/ci/compare_phase2_projection_live.py"


def _load_comparator() -> ModuleType:
    spec = importlib.util.spec_from_file_location("rust_phase2_live_contract", COMPARATOR_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_producer_is_expectation_independent_and_exactly_authority_bound() -> None:
    comparator = _load_comparator()
    source = PRODUCER_PATH.read_bytes()
    text = source.decode()

    comparator.validate_producer_source(source)
    assert len(source) <= comparator.MAX_PRODUCER_SOURCE_BYTES
    assert "typebridge.phase2-projected-live-report/v1" in text
    assert "typedb-3.12.1/v1" in text
    assert "3c8d072b60c575b4c0381c1ea44088a9c18e01ed52e90730311aadccb72cd0c8" in text
    for digest in comparator.SOURCE_SHA256.values():
        assert digest in text
    for environment in (
        "TYPE_BRIDGE_PHASE2_LIVE_REPORT",
        "TYPE_BRIDGE_PHASE2_LIVE_ADDRESS",
        "TYPE_BRIDGE_PHASE2_LIVE_DATABASE",
        "TYPE_BRIDGE_PHASE2_LIVE_HTTP_PORT",
        "TYPE_BRIDGE_PHASE2_REPOSITORY_ROOT",
    ):
        assert environment in text


def test_live_data_uses_only_generated_crud_and_omits_ordered_persistence() -> None:
    source = PRODUCER_PATH.read_text()

    assert "GeneratedDatabase<AppSchema>" in source
    assert ".entities::<Person>()" in source
    assert ".entities::<Robot>()" in source
    assert ".relations::<Interaction>()" in source
    assert ".relations::<PlainActivity>()" in source
    assert ".relations::<Event>()" in source
    assert ".relations::<Container>()" in source
    assert source.count("execute_raw(") == 1
    assert "TxType::Schema" in source
    assert "type_bridge_orm::_" not in source
    assert "Aliases::new" not in source
    assert "NetworkLinkCreate" not in source
    assert "aliases" not in source.casefold()
    assert "participant[]" not in source


def test_create_and_cleanup_follow_the_frozen_live_subset_order() -> None:
    source = PRODUCER_PATH.read_text()
    create_markers = (
        '"insert Ada"',
        '"insert Dana"',
        '"insert positive robot"',
        '"insert negative robot"',
        '"insert robot interaction"',
        '"insert absent interaction"',
        '"insert person interaction"',
        '"insert plain activity"',
        '"insert event"',
        '"insert container"',
    )
    cleanup_markers = (
        'order.push("container-event")',
        'order.push("event-ada")',
        'order.push("plain-activity-ada")',
        '"interaction-person",',
        '"interaction-absent",',
        '"interaction-robot",',
        'order.push("robot-negative-7")',
        'order.push("robot-7")',
        'order.push("data-dana")',
        'order.push("data-ada")',
    )

    create_positions = [source.index(marker) for marker in create_markers]
    cleanup_start = source.index("async fn cleanup_live_records")
    cleanup_source = source[cleanup_start:]
    cleanup_positions = [cleanup_source.index(marker) for marker in cleanup_markers]
    assert create_positions == sorted(create_positions)
    assert cleanup_positions == sorted(cleanup_positions)


def test_owned_database_teardown_precedes_create_new_publication() -> None:
    source = PRODUCER_PATH.read_text()
    execute_start = source.index("async fn execute_live")
    execute_end = source.index("fn sorted_json", execute_start)
    execute = source[execute_start:execute_end]
    run_start = source.index("async fn run()")
    run = source[run_start:]

    assert execute.index("database_exists().await") < execute.index("create_database().await")
    assert execute.index("run_journey(&database).await") < execute.index("drop(database)")
    assert execute.index("drop(database)") < execute.index("delete_database().await")
    assert execute.index("delete_database().await") < execute.index("admin.close()")
    assert run.index("execute_live(") < run.index("canonical_report_bytes(")
    assert run.index("canonical_report_bytes(") < run.index("publish_create_new(")
    assert ".create_new(true)" in source


def test_harness_generates_local_and_foreign_packages_then_validates_report() -> None:
    harness = HARNESS_PATH.read_text()

    assert 'include_str!("rust_acceptance/phase2_live.rs")' in harness
    assert "emit_from_source(&workforce_source())" in harness
    assert "emit_from_source(&foreign_workforce_source())" in harness
    assert "generated_package_mismatch" in harness
    assert "validate_producer_source" in harness
    assert "module._load_report" not in harness
    assert "m._load_report" in harness
    assert "m.load_contract()" in harness
    assert "binding=='rust'" in harness
