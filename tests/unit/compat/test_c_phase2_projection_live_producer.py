"""Source and lifecycle contract for the C Phase-2 exact-live producer."""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
HARNESS = ROOT / "type-bridge-core/crates/schema-codegen/tests/c_phase2_live.rs"
CONSUMER = ROOT / "type-bridge-core/crates/schema-codegen/tests/c_phase2_live/consumer.c"
SETUP = ROOT / "type-bridge-core/crates/schema-codegen/tests/c_phase2_live/setup.rs"

FORBIDDEN_MARKERS = (
    "compare_phase2_projection_live",
    "compare_phase2_projection_parity",
    "expected_observations",
    "expected_report",
    "load_contract",
)


def _sources() -> tuple[str, str, str]:
    return (
        HARNESS.read_text(encoding="utf-8"),
        CONSUMER.read_text(encoding="utf-8"),
        SETUP.read_text(encoding="utf-8"),
    )


def test_c_phase2_live_producer_is_independent_and_exactly_authority_bound() -> None:
    harness, consumer, setup = _sources()
    joined = "\n".join((harness, consumer, setup))
    assert not any(marker in joined for marker in FORBIDDEN_MARKERS)
    for digest in (
        "74b9161e3d8fd70a1f22c3a8b7fc70a823b18cb07973064e2e10ce0940f76f1f",
        "912130753fe7938a38c054cff16e202b312551a6aa65265148628bfbd2abbbef",
        "af61aaece22d666e19d2c1357f8ebc39b8cbcf4bff7b98a19a56daf171a1faba",
    ):
        assert digest in harness
    assert 'const REPORT_FORMAT: &str = "typebridge.phase2-projected-live-report/v1"' in harness
    assert 'const SEMANTIC_PROFILE: &str = "typedb-3.12.1/v1"' in harness
    assert '"binding": "c"' in harness
    assert "foreign_fingerprint" in harness
    assert "assert_ne!(" in harness
    assert "phase2_foreign_schema_package_open" in consumer


def test_c_phase2_live_consumer_owns_data_crud_and_omits_ordered_persistence() -> None:
    harness, consumer, setup = _sources()
    assert "execute_raw" not in consumer
    assert "execute_query" not in consumer
    assert "_database_insert(" in consumer
    assert "_database_get_by_iid(" in consumer
    assert "_database_delete_by_iid(" in consumer
    assert "_database_count(" in consumer
    assert "field_aliases_chunks" not in consumer
    assert "networkzhlink_database" not in consumer
    assert "aliases" not in consumer.casefold()
    assert "participant[]" not in consumer
    assert "execute_raw(PROVIDER_SCHEMA, TxType::Schema)" in setup
    assert "execute_raw" not in harness


def test_c_phase2_live_observation_ledger_is_exact_and_runtime_checked() -> None:
    _, consumer, _ = _sources()
    emitted = re.findall(r'puts\("TYPE_BRIDGE_C_PHASE2_LIVE_FACT\\t([^\\]+)\\t', consumer)
    assert emitted == [
        "canonical_scalar_values",
        "cleanup",
        "inherited_plain_activity_role_lifecycle",
        "integer_key_polymorphic_optional_role",
        "relation_as_player",
    ]
    for model in ("person", "robot", "interaction", "plainzhactivity", "event", "container"):
        assert f"phase2_{model}_database_count(" in consumer
    assert "interaction_actor_is(interaction, expected_kinds[index]" in consumer
    assert "phase2_plainzhactivity_database_get_by_iid(" in consumer
    assert "plain != NULL && diagnostics == NULL" in consumer
    assert "plain == NULL" in consumer
    assert "phase2_container_item_player_as_event(" in consumer
    assert "memcmp(hydrated_iid.data, iids[8].bytes" in consumer


def test_c_phase2_live_lifecycle_is_fail_closed_and_publishes_last() -> None:
    harness, consumer, setup = _sources()
    assert "OpenOptions::new()\n        .write(true)\n        .create_new(true)" in harness
    assert "bytes.len() <= MAX_REPORT_BYTES" in harness
    assert "metadata.is_dir() && !metadata.file_type().is_symlink()" in harness
    assert "isolated.cleanup();\n    publish_report(" in harness
    assert "impl Drop for IsolatedDatabase" in harness
    assert "if self.active" in harness
    assert "cleanup_with_retries" in harness
    assert "self.active = stdout.lines().any" in harness
    assert "database_exists()" in setup
    assert "must name an absent database" in setup
    assert "delete_database()" in setup
    assert "type_bridge_database_close(&database" in consumer
    assert "type_bridge_runtime_close(&runtime" in consumer
    assert consumer.rfind("type_bridge_schema_package_close") < consumer.rfind(
        "TYPE_BRIDGE_C_PHASE2_LIVE_FACT"
    )


def test_c_phase2_live_requires_exact_ports_database_and_provider_version() -> None:
    harness, consumer, setup = _sources()
    for name in (
        "TYPE_BRIDGE_PHASE2_LIVE_REPORT",
        "TYPE_BRIDGE_PHASE2_LIVE_ADDRESS",
        "TYPE_BRIDGE_PHASE2_LIVE_HTTP_PORT",
        "TYPE_BRIDGE_PHASE2_LIVE_DATABASE",
    ):
        assert name in harness
    assert "byte.is_ascii_digit()" in harness
    assert "byte.is_ascii_digit()" in setup
    assert "*cursor < (unsigned char)'0'" in consumer
    assert "parsed > 65535ul" in consumer
    assert "(version.major, version.minor, version.patch),\n        (3, 12, 1)" in setup
    assert 'text_is(version, "3.12.3")' in consumer
