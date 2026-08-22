"""Frozen Phase-0 contract checks for Workforce V4."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[3]
CONTRACT_ROOT = ROOT / "tests/contracts/sdk_conformance/workforce-v4"
MANIFEST = ROOT / "tests/contracts/sdk_conformance/manifest-v1.json"

TRANSITION_CASES = [
    "workforce.runtime.database-administration",
    "workforce.migration.rollback",
    "workforce.migration.backfill",
    "workforce.migration.runtime-facade",
]
EVIDENCE_ONLY_CASES = [
    "workforce.runtime.cancellation",
    "workforce.runtime.timeout-resource-limits",
    "workforce.diagnostic.all-workflows",
    "workforce.runtime.explicit-close",
]
EXPECTED_OBSERVATIONS = [
    "bound_database_administration",
    "rollback_reapply_recovery",
    "binding_neutral_backfill",
    "migration_runtime_facade",
    "administration_migration_cancellation",
    "administration_migration_resource_limits",
    "administration_migration_structured_diagnostic",
    "administration_migration_resource_lifecycle",
]


def _load(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def test_v4_freezes_exact_plan06_case_partition() -> None:
    catalog = _load(CONTRACT_ROOT / "catalog-v4.json")

    assert catalog["format"] == "typebridge.workforce-catalog/v4"
    assert catalog["authority_state"] == "finalized"
    assert catalog["report_bindings"] == ["python", "node", "rust", "c"]
    assert catalog["manifest_transition_cases"] == TRANSITION_CASES
    assert catalog["evidence_only_gap_cases"] == EVIDENCE_ONLY_CASES

    cases = catalog["cases"]
    assert isinstance(cases, list)
    assert [case["id"] for case in cases] == TRANSITION_CASES + EVIDENCE_ONLY_CASES
    assert [case["disposition"] for case in cases[:4]] == ["shared_smoke"] * 4
    assert [case["disposition"] for case in cases[4:]] == ["retained_gap_evidence"] * 4


def test_v4_rows_are_exact_manifest_cases_and_capabilities() -> None:
    manifest = _load(MANIFEST)
    catalog = _load(CONTRACT_ROOT / "catalog-v4.json")
    capabilities = {
        capability["id"]: capability
        for capability in manifest["capabilities"]
        if isinstance(capability, dict)
    }

    for case in catalog["cases"]:
        capability = capabilities[case["capability_id"]]
        assert capability["case_ids"] == [case["id"]]

    transitioned_codes = [
        capabilities[case["capability_id"]]["code"] for case in catalog["cases"][:4]
    ]
    retained_codes = [capabilities[case["capability_id"]]["code"] for case in catalog["cases"][4:]]
    assert transitioned_codes == ["G04", "G09", "G10", "G11"]
    assert retained_codes == ["G05", "G06", "G08", "G13"]


def test_v4_selected_proofs_and_journey_are_closed_and_ordered() -> None:
    catalog = _load(CONTRACT_ROOT / "catalog-v4.json")
    journey = _load(CONTRACT_ROOT / "journey-v4.json")
    proofs = catalog["selected_proofs"]

    assert [proof["case_id"] for proof in proofs] == TRANSITION_CASES + EVIDENCE_ONLY_CASES
    assert [proof["observation_ref"] for proof in proofs] == EXPECTED_OBSERVATIONS
    assert journey["expected_observation_refs"] == EXPECTED_OBSERVATIONS
    assert journey["semantic_profile"] == "typedb-3.12.1/v1"
    assert journey["live_server_version"] == "3.12.3"
    assert journey["shared_fixture_oracles"] == {
        "binding_neutral_backfill": {
            "conflict_certainty": "definitely_aborted",
            "conflict_code": "migration_typedb_backfill_destination_conflict",
            "conflict_visible_destination_count": 1,
            "forward_changed": 2,
            "forward_transaction_groups": 2,
            "equal_copy_count": 2,
            "retry_changed": 0,
            "reverse_changed": 2,
            "remaining_destination_count": 0,
        },
        "rollback_reapply_recovery": {
            "apply_status": "applied",
            "rollback_without_approval_code": "migration_rollback_approval_required",
            "rollback_status": "rolled_back",
            "unknown_target_code": "migration_history_unknown_rollback_target",
            "repeat_rollback_status": "up_to_date",
            "reapply_status": "applied",
        },
    }
    assert journey["cleanup_invariant"] == {
        "managed_database_absent": True,
        "journal_database_absent": True,
        "temporary_evidence_absent": True,
    }


def test_v4_producer_and_report_envelopes_are_exact() -> None:
    catalog = _load(CONTRACT_ROOT / "catalog-v4.json")
    schema = _load(CONTRACT_ROOT / "report-schema-v4.json")
    observations = _load(CONTRACT_ROOT / "observation-schema-v4.json")

    assert list(catalog["report_producers"]) == ["python", "node", "rust", "c"]
    assert schema["properties"]["format"] == {"const": "typebridge.sdk-conformance-report/v4"}
    assert schema["properties"]["binding"] == {"enum": ["python", "node", "rust", "c"]}
    assert schema["properties"]["server_version"] == {"const": "3.12.3"}
    assert schema["properties"]["results"]["minItems"] == 8
    assert schema["properties"]["results"]["maxItems"] == 8
    assert schema["$defs"]["result"]["properties"]["observation"] == {
        "$ref": "observation-schema-v4.json"
    }
    assert catalog["observation_schema_path"].endswith("observation-schema-v4.json")
    assert len(observations["oneOf"]) == 8
    assert all(
        definition.get("additionalProperties") is False
        for name, definition in observations["$defs"].items()
        if name != "migrationIdentities"
    )
