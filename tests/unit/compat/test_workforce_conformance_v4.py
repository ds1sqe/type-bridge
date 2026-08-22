"""Fail-closed tests for the Workforce V4 comparator."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import shutil
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "scripts/ci/compare_workforce_conformance_v4.py"
SPEC = importlib.util.spec_from_file_location("workforce_v4_comparator", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
comparator = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = comparator
SPEC.loader.exec_module(comparator)


def _stage_contracts(destination: Path) -> Path:
    source = ROOT / "tests/contracts/sdk_conformance"
    target = destination / "tests/contracts/sdk_conformance"
    (target / "workforce-v4").mkdir(parents=True)
    shutil.copy2(source / "manifest-v1.json", target / "manifest-v1.json")
    for name in [
        "catalog-v4.json",
        "journey-v4.json",
        "report-schema-v4.json",
        "observation-schema-v4.json",
    ]:
        shutil.copy2(source / "workforce-v4" / name, target / "workforce-v4" / name)
    return destination


def _identity(root: Path, relative: str) -> dict[str, str]:
    return {
        "path": relative,
        "sha256": hashlib.sha256((root / relative).read_bytes()).hexdigest(),
    }


def _valid_report(root: Path, binding: str) -> dict[str, object]:
    catalog = json.loads((root / comparator.CATALOG_RELATIVE).read_text())
    journey = json.loads((root / comparator.JOURNEY_RELATIVE).read_text())
    observations = {
        "bound_database_administration": {
            "create": "created",
            "repeat_create": "already_exists",
            "pair_state": "standalone_managed",
            "delete": "deleted_standalone_managed",
            "repeat_delete": "already_absent",
        },
        **journey["shared_fixture_oracles"],
        "migration_runtime_facade": {
            "catalog_entries": 4,
            "catalog_fingerprint": "0" * 64,
            "apply_order": [
                "workforcev4/0001_initial",
                "workforcev4/0002_expand-display-name",
                "workforcev4/0003_backfill-display-name",
                "workforcev4/0004_contract-legacy-name",
            ],
            "rollback_order": [
                "workforcev4/0004_contract-legacy-name",
                "workforcev4/0003_backfill-display-name",
                "workforcev4/0002_expand-display-name",
                "workforcev4/0001_initial",
            ],
            "backfill_steps": 1,
        },
        "administration_migration_cancellation": {
            "code": "migration_execution_cancelled",
            "before_effect": True,
        },
        "administration_migration_resource_limits": {
            "code": "migration_execution_group_limit",
            "bounded": True,
        },
        "administration_migration_structured_diagnostic": {
            "code": "migration_execution_cancelled",
            "category": "cancelled",
            "provider_text_absent": True,
        },
        "administration_migration_resource_lifecycle": {
            "explicit_close": True,
            "repeat_close": True,
            "temporary_evidence_absent": True,
        },
    }
    results = []
    for proof, case in zip(catalog["selected_proofs"], catalog["cases"], strict=True):
        results.append(
            {
                "case_id": case["id"],
                "capability_id": case["capability_id"],
                "proof_kind": proof["proof_kind"],
                "outcome": "passed",
                "observation": observations[proof["observation_ref"]],
            }
        )
    return {
        "format": "typebridge.sdk-conformance-report/v4",
        "binding": binding,
        "manifest": _identity(root, comparator.MANIFEST_RELATIVE),
        "catalog": _identity(root, comparator.CATALOG_RELATIVE),
        "journey": _identity(root, comparator.JOURNEY_RELATIVE),
        "server_version": "3.12.3",
        "results": results,
        "cleanup": {
            "managed_database_absent": True,
            "journal_database_absent": True,
            "temporary_evidence_absent": True,
        },
    }


def test_loads_exact_phase0_contract() -> None:
    contracts = comparator.load_contracts(ROOT)

    assert contracts.selected_cases == comparator.EXPECTED_TRANSITIONS + (
        comparator.EXPECTED_RETAINED_GAPS
    )
    assert len(contracts.observation_refs) == 8


def test_report_fan_in_rejects_unfinalized_authority() -> None:
    with pytest.raises(comparator.ContractError) as raised:
        comparator.compare_reports([], ROOT)
    assert raised.value.code == "unfinalized_v4_authority"


def test_observation_algebra_is_closed() -> None:
    valid = {
        "create": "created",
        "repeat_create": "already_exists",
        "pair_state": "owned_pair",
        "delete": "deleted_owned_pair",
        "repeat_delete": "already_absent",
    }
    comparator._validate_observation("bound_database_administration", valid)

    with pytest.raises(comparator.ContractError) as raised:
        comparator._validate_observation(
            "bound_database_administration", {**valid, "raw_provider": "escape"}
        )
    assert raised.value.code == "invalid_contract_shape"

    with pytest.raises(comparator.ContractError) as raised:
        comparator._validate_observation(
            "administration_migration_cancellation",
            {"code": "binding-specific", "before_effect": True},
        )
    assert raised.value.code == "invalid_observation_shape"


def test_finalized_fan_in_requires_exact_live_oracles(tmp_path: Path) -> None:
    root = _stage_contracts(tmp_path)
    catalog_path = root / comparator.CATALOG_RELATIVE
    catalog = json.loads(catalog_path.read_text())
    catalog["authority_state"] = "finalized"
    catalog_path.write_text(json.dumps(catalog))
    paths = []
    for binding in comparator.EXPECTED_BINDINGS:
        path = root / f"{binding}.json"
        path.write_text(json.dumps(_valid_report(root, binding)))
        paths.append(path)

    comparison = comparator.compare_reports(paths, root)
    assert comparison["pending_promotions"] == []

    rust = json.loads(paths[2].read_text())
    rust["results"][1]["observation"]["reapply_status"] = "not-observed"
    paths[2].write_text(json.dumps(rust))
    with pytest.raises(comparator.ContractError) as raised:
        comparator.compare_reports(paths, root)
    assert raised.value.code == "invalid_observation_shape"


@pytest.mark.parametrize(
    ("field", "replacement", "code"),
    [
        ("manifest_transition_cases", ["workforce.runtime.cancellation"], "transition_scope_drift"),
        ("evidence_only_gap_cases", [], "retained_gap_scope_drift"),
        ("report_bindings", ["python", "node", "rust"], "binding_scope_drift"),
    ],
)
def test_rejects_catalog_scope_drift(
    tmp_path: Path, field: str, replacement: object, code: str
) -> None:
    root = _stage_contracts(tmp_path)
    path = root / comparator.CATALOG_RELATIVE
    catalog = json.loads(path.read_text(encoding="utf-8"))
    catalog[field] = replacement
    path.write_text(json.dumps(catalog), encoding="utf-8")

    with pytest.raises(comparator.ContractError) as raised:
        comparator.load_contracts(root)
    assert raised.value.code == code


def test_rejects_manifest_case_rebinding(tmp_path: Path) -> None:
    root = _stage_contracts(tmp_path)
    path = root / comparator.CATALOG_RELATIVE
    catalog = json.loads(path.read_text(encoding="utf-8"))
    catalog["cases"][0]["capability_id"] = "runtime.cancellation"
    path.write_text(json.dumps(catalog), encoding="utf-8")

    with pytest.raises(comparator.ContractError) as raised:
        comparator.load_contracts(root)
    assert raised.value.code == "manifest_case_drift"
