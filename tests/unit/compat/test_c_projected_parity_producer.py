"""Fail-closed tests for C's provider-free Projected report producer."""

from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
HARNESS = ROOT / "type-bridge-core/crates/c/tests/projected_parity.rs"
CONSUMER = ROOT / "type-bridge-core/crates/schema-codegen/tests/c_projected_parity/consumer.c"
CHECK = ROOT / "scripts/check.sh"
CI = ROOT / ".github/workflows/ci.yml"


def test_c_projected_producer_uses_generated_public_projection_paths() -> None:
    harness = HARNESS.read_text()
    consumer = CONSUMER.read_text()

    assert "sdk-v3/schema-v3.yaml" in harness
    assert "CEmitter::new()" in harness
    assert '"projected"' in harness and '"projected_foreign"' in harness
    assert "type_bridge_projected_thing_open_v1" in consumer
    assert "projected_person_create_open" in consumer
    assert "projected_plainzhactivity_create_open" in consumer
    assert "projected_networkzhlink_create_open" in consumer
    assert "projected_interaction_create_open" in consumer
    assert "projected_container_create_open" in consumer
    assert "generated_token_package_mismatch" in consumer
    assert "type_bridge_database_" not in consumer
    assert "type_bridge_runtime_open" not in consumer


def test_c_projected_producer_does_not_import_expected_parity_evidence() -> None:
    harness = HARNESS.read_text()
    consumer = CONSUMER.read_text()

    for forbidden in (
        "expected_report",
        'journey-v3.json")',
        "batch_parity",
        "filter_parity",
    ):
        assert forbidden not in harness
        assert forbidden not in consumer
    assert "compare_projected_parity" not in consumer


def test_c_projected_report_is_canonical_bounded_create_new_and_compared() -> None:
    harness = HARNESS.read_text()

    assert "MAX_REPORT_BYTES" in harness
    assert ".create_new(true)" in harness
    assert ".sync_all()" in harness
    assert "symlink_metadata" in harness
    assert "to_canonical_json" in harness
    assert "m._load_report" in harness


def test_c_projected_producer_is_persistently_gated_after_shared_build() -> None:
    check = CHECK.read_text()
    ci = CI.read_text()
    check_build = 'run_step "build the isolated C ABI shared library"'
    check_gate = 'run_step "C provider-free Projected parity producer"'
    ci_build = "- name: Build C ABI shared library"
    ci_gate = "- name: Check C provider-free Projected parity producer"

    assert check.index(check_build) < check.index(check_gate)
    assert ci.index(ci_build) < ci.index(ci_gate)
    for source in (check, ci):
        gate = source[source.index("C provider-free Projected parity producer") :]
        assert "TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER" in gate
        assert "--test projected_parity" in gate
