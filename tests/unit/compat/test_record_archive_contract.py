"""Fail-closed checks for the frozen Record/archive record/archive and ABI contracts."""

from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[3]


def _load(relative: str) -> dict[str, Any]:
    return json.loads((ROOT / relative).read_text())


def test_record_and_archive_formats_are_distinct_and_closed() -> None:
    contract = _load("tests/contracts/projected-record-v1.json")
    record = contract["record"]
    archive = contract["archive"]

    assert record["format"] == "typebridge.projected-record/v1"
    assert archive["format"] == "typebridge.projected-archive/v1"
    assert record["fingerprint_domain"] == "typebridge.projected-record"
    assert archive["fingerprint_domain"] == "typebridge.projected-archive"
    assert record["fingerprint_domain"] != archive["fingerprint_domain"]
    assert record["record_kinds"] == [
        "attribute_value",
        "struct_value",
        "entity_create",
        "relation_create",
        "entity_snapshot",
        "relation_snapshot",
        "reference",
    ]
    assert archive["mode"] == "ordered"
    assert archive["partial_publication"] is False


def test_schema_fence_and_detached_boundary_exclude_target_authority() -> None:
    contract = _load("tests/contracts/projected-record-v1.json")
    fence = contract["schema_fence"]
    snapshot = contract["snapshot"]

    assert fence["declared_schema_identity_domain"] == ("typebridge.schema.declared-identity")
    assert fence["target_projection_fingerprint_in_wire"] is False
    assert fence["installed_projection_required_for_decode"] is True
    assert snapshot == {
        "decoded_state": "detached",
        "database_origin_in_wire": False,
        "transaction_identity_in_wire": False,
        "remote_transport_identity_in_wire": False,
        "mutation_authority_after_decode": False,
    }


def test_limits_and_strict_decode_are_exact() -> None:
    contract = _load("tests/contracts/projected-record-v1.json")
    assert contract["limits"] == {
        "record_input_bytes": 16 * 1024 * 1024,
        "record_output_bytes": 16 * 1024 * 1024,
        "archive_input_bytes": 32 * 1024 * 1024,
        "archive_output_bytes": 32 * 1024 * 1024,
        "depth": 64,
        "records": 4096,
        "members_per_record": 65536,
        "values_per_member": 65536,
        "references_per_role": 65536,
        "string_bytes": 1024 * 1024,
        "decoded_object_weight": 65536,
    }
    decode = contract["decode"]
    assert decode["requires_unique_object_keys"] is True
    assert decode["requires_exact_canonical_bytes"] is True
    assert decode["requires_complete_archive_validation"] is True
    assert decode["numeric_coercion"] is False
    assert decode["best_effort_decode"] is False


def test_current_abi_has_one_complete_header_and_export_inventory() -> None:
    contract = _load("tests/contracts/c-abi.json")
    header = ROOT / contract["header"]
    assert hashlib.sha256(header.read_bytes()).hexdigest() == contract["header_sha256"]
    assert contract["version"] == "1.7.0"
    assert not any(key.startswith("parent_") for key in contract)
    exports = set(re.findall(r"\b(type_bridge_[a-z0-9_]+)\s*\(", header.read_text()))
    assert contract["exports"] == sorted(exports)
    assert len(exports) == 394
    previous = ROOT / "type-bridge-core/crates/c/tests/fixtures/abi-1.6/typebridge/type_bridge.h"
    assert hashlib.sha256(previous.read_bytes()).hexdigest() == (
        "4cdc5195b1da40333406ae162d05bd932ed90e3e8d4d30bcebda21c2bea38f2d"
    )
    previous_exports = set(re.findall(r"\b(type_bridge_[a-z0-9_]+)\s*\(", previous.read_text()))
    # ABI 1.7 is additive: keep-open rollback, the staged transaction surface,
    # the plan builder and the typed failure accessor.
    builder = {
        f"type_bridge_plan_builder_{name}"
        for name in (
            "new free type owns role exact subtypes field links compare input let "
            "expr_arith expr_call update update_player insert put thing_field "
            "thing_player delete delete_field delete_player row returning count "
            "exists build value_string value_long value_double value_boolean "
            "value_date value_datetime value_datetime_tz value_decimal value_duration"
        ).split()
    }
    assert exports == previous_exports | builder | {
        "type_bridge_write_transaction_rollback_keep",
        "type_bridge_plan_v3_decode",
        "type_bridge_plan_v3_canonical_bytes",
        "type_bridge_plan_free",
        "type_bridge_database_transaction",
        "type_bridge_staged_transaction_stage",
        "type_bridge_staged_transaction_commit",
        "type_bridge_staged_transaction_commit_retry",
        "type_bridge_staged_transaction_rollback",
        "type_bridge_staged_transaction_close",
        "type_bridge_pending_answer",
        "type_bridge_pending_free",
        "type_bridge_execution_diagnostics_failure_get_v1",
    }
    assert contract["options"]["size_64_bit"] == 72
    assert contract["options"]["alignment_64_bit"] == 8
    assert contract["ownership"]["partial_archive_result"] is False


def test_sdk_serialization_history_is_preserved_after_final_broad_transition() -> None:
    catalog = _load("tests/contracts/sdk_conformance/serialization/catalog.json")
    manifest = _load("tests/contracts/sdk_conformance/manifest.json")

    assert catalog["authority_state"] == "finalized"
    assert catalog["report_bindings"] == ["python", "node", "rust", "c"]
    assert catalog["manifest_transition_cases"] == ["sdk.model.serialization"]
    assert catalog["evidence_only_gap_cases"] == [
        "sdk.runtime.cancellation",
        "sdk.runtime.timeout-resource-limits",
        "sdk.diagnostic.all-workflows",
        "sdk.runtime.explicit-close",
    ]
    assert [proof["case_id"] for proof in catalog["selected_proofs"]] == [
        case["id"] for case in catalog["cases"]
    ]
    capabilities = {item["code"]: item for item in manifest["capabilities"]}
    assert capabilities["G07"]["case_ids"] == ["sdk.model.serialization"]
    assert capabilities["G07"]["binding_profile"] == "current_and_c_live_future_planned"
    for code in ("G05", "G06", "G08", "G13"):
        assert capabilities[code]["binding_profile"] == "terminal_broad_live_future_planned"
