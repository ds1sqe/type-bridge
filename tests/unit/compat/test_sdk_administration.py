"""Fail-closed tests for the SDK administration comparator."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import shutil
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "scripts/ci/sdk_administration_compare.py"
SPEC = importlib.util.spec_from_file_location("sdk_administration_comparator", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
comparator = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = comparator
SPEC.loader.exec_module(comparator)


def _stage_contracts(destination: Path) -> Path:
    source = ROOT / "tests/contracts/sdk_conformance"
    target = destination / "tests/contracts/sdk_conformance"
    (target / "administration").mkdir(parents=True)
    shutil.copy2(source / "manifest.json", target / "manifest.json")
    for name in [
        "catalog.json",
        "journey.json",
        "report-schema.json",
        "observation-schema.json",
    ]:
        shutil.copy2(source / "administration" / name, target / "administration" / name)
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
            "catalog_fingerprint": comparator.EXPECTED_CATALOG_FINGERPRINT,
            "apply_order": [
                "administration/0001_initial",
                "administration/0002_expand-display-name",
                "administration/0003_backfill-display-name",
                "administration/0004_contract-legacy-name",
            ],
            "rollback_order": [
                "administration/0004_contract-legacy-name",
                "administration/0003_backfill-display-name",
                "administration/0002_expand-display-name",
                "administration/0001_initial",
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
        "format": "typebridge.sdk-administration-report/v1",
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


def test_loads_exact_catalog() -> None:
    contracts = comparator.load_contracts(ROOT)

    assert contracts.selected_cases == comparator.EXPECTED_TRANSITIONS + (
        comparator.EXPECTED_RETAINED_GAPS
    )
    assert len(contracts.observation_refs) == 8


def test_reports_require_a_finalized_catalog(tmp_path: Path) -> None:
    root = _stage_contracts(tmp_path)
    path = root / comparator.CATALOG_RELATIVE
    catalog = json.loads(path.read_text())
    catalog["authority_state"] = "draft"
    path.write_text(json.dumps(catalog))
    with pytest.raises(comparator.ContractError) as raised:
        comparator.compare_reports([], root)
    assert raised.value.code == "invalid_administration_authority"


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
    manifest_path = root / comparator.MANIFEST_RELATIVE
    manifest = json.loads(manifest_path.read_text())
    transition_cases = set(comparator.EXPECTED_TRANSITIONS)
    for capability in manifest["capabilities"]:
        if capability["case_ids"][0] in transition_cases:
            capability["binding_profile"] = "current_and_c_live_future_planned"
            capability.pop("gap_reason", None)
    manifest_path.write_text(json.dumps(manifest))
    paths = []
    for binding in comparator.EXPECTED_BINDINGS:
        path = root / f"{binding}.json"
        path.write_text(json.dumps(_valid_report(root, binding)))
        paths.append(path)

    comparison = comparator.compare_reports(paths, root)
    assert comparison["authority_state"] == "finalized"
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
        ("manifest_transition_cases", ["sdk.runtime.cancellation"], "transition_scope_drift"),
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


@pytest.mark.parametrize("profile", ["typedb-3.12.1/v1", "typedb-3.13.6/v1"])
def test_report_history_fingerprint_is_bound_to_its_server(tmp_path: Path, profile: str) -> None:
    root = _stage_contracts(tmp_path)
    contracts = comparator.load_contracts(root)
    identity = comparator.profile_matrix.select("administration", profile)
    report = _valid_report(root, "python")
    report["server_version"] = identity.live_server_version
    results = report["results"]
    assert isinstance(results, list)
    result = results[3]
    assert isinstance(result, dict)
    observation = result["observation"]
    assert isinstance(observation, dict)
    observation["catalog_fingerprint"] = identity.expected_history_fingerprint
    comparator._validate_report(report, contracts.observation_refs)
    observation["catalog_fingerprint"] = "0" * 64
    with pytest.raises(comparator.ContractError) as rejected:
        comparator._validate_report(report, contracts.observation_refs)
    assert rejected.value.code == "migration_catalog_observation_drift"
