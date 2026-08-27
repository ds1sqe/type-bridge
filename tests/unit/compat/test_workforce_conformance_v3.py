"""Fail-closed tests for the four-binding workforce-v3 transition gate."""

from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
import shutil
import sys
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

SOURCE_ROOT = Path(__file__).resolve().parents[3]
COMPARATOR_PATH = SOURCE_ROOT / "scripts/ci/compare_workforce_conformance_v3.py"


def _load_comparator() -> ModuleType:
    spec = importlib.util.spec_from_file_location(
        "workforce_conformance_v3_comparator",
        COMPARATOR_PATH,
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


comparator = _load_comparator()


def _write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def _copy_authority(tmp_path: Path) -> None:
    for relative in (
        comparator.MANIFEST_RELATIVE,
        comparator.CATALOG_RELATIVE,
        comparator.JOURNEY_RELATIVE,
        comparator.REPORT_SCHEMA_RELATIVE,
        comparator.SCHEMA_RELATIVE,
        comparator.PROVIDER_SCHEMA_RELATIVE,
    ):
        target = tmp_path / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(SOURCE_ROOT / relative, target)


def _source_identity(root: Path, relative: str) -> dict[str, str]:
    return {
        "path": relative,
        "sha256": hashlib.sha256((root / relative).read_bytes()).hexdigest(),
    }


def _finalize_catalog(tmp_path: Path) -> None:
    catalog_path = tmp_path / comparator.CATALOG_RELATIVE
    catalog = json.loads(catalog_path.read_text(encoding="utf-8"))
    catalog["authority_state"] = "finalized"
    catalog["expected_fingerprints"] = {
        "semantic": {
            "domain": "typebridge.schema.semantic",
            "algorithm": "sha256",
            "canonicalization": "typebridge.schema-canonical-json/v1",
            "semantic_profile": "typedb-3.12.1/v1",
            "digest": "a" * 64,
        },
        "projections": {
            binding: {
                "domain": "typebridge.binding.projection",
                "algorithm": "sha256",
                "canonicalization": "typebridge.binding-projection/v1",
                "semantic_profile": "typedb-3.12.1/v1",
                "digest": digest * 64,
            }
            for binding, digest in zip(
                comparator.REPORT_BINDINGS,
                ("b", "c", "d", "e"),
                strict=True,
            )
        },
    }
    _write_json(catalog_path, catalog)


def _restore_candidate_manifest(tmp_path: Path) -> None:
    profiles = {
        "workforce.schema.generate.atomic": "current_live_future_planned",
        "workforce.model.constraints": "current_offline_future_planned",
        "workforce.crud.entity-batch-insert-put": "current_live_future_planned",
        "workforce.crud.relation-batch-insert-put": "current_live_future_planned",
        "workforce.crud.entity-batch-update-delete": "python_rust_live_node_gap_future_planned",
        "workforce.crud.relation-batch-update-delete": "python_rust_live_node_gap_future_planned",
        "workforce.transaction.borrowed": "current_live_future_planned",
        "workforce.manager.filter": "current_live_future_planned",
        "workforce.projection.field-name-identity": "current_live_future_planned",
        "workforce.model.integer-key-polymorphic-role": "current_live_future_planned",
        "workforce.model.inherited-relation-role": "current_live_future_planned",
        "workforce.crud.unkeyed-entity": "current_live_future_planned",
        "workforce.crud.unkeyed-relation": "current_live_future_planned",
        "workforce.projection.evidence-integrity": "current_offline_future_planned",
        "workforce.projection.token-package-fencing": "current_offline_future_planned",
        "workforce.schema.ordered-distinct": "current_gap_future_planned",
        "workforce.runtime.connection-policy": "current_gap_future_planned",
    }
    manifest_path = tmp_path / comparator.MANIFEST_RELATIVE
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    for capability in manifest["capabilities"]:
        case_id = capability["case_ids"][0]
        if case_id in profiles:
            capability["binding_profile"] = profiles[case_id]
    reasons = {
        "workforce.crud.entity-batch-update-delete": "the generated Node manager does not expose the required atomic batch update/delete outcome",
        "workforce.crud.relation-batch-update-delete": "the generated Node manager does not expose the required atomic batch update/delete outcome",
        "workforce.schema.ordered-distinct": "the guides promise ordered/list schema facts but Split-YAML and the canonical schema fact algebra do not represent them",
        "workforce.runtime.connection-policy": "address, credentials, TLS roots, compatibility, timeout, and resource policy are not exposed and proven uniformly by current SDK facades",
    }
    for capability in manifest["capabilities"]:
        case_id = capability["case_ids"][0]
        if case_id in reasons:
            capability["gap_reason"] = reasons[case_id]
    _write_json(manifest_path, manifest)


def _valid_report(binding: str, contracts: Any) -> dict[str, Any]:
    results = []
    for case_id, proof_kind, observation_ref in contracts.selected:
        results.append(
            {
                "case_id": case_id,
                "capability_id": contracts.cases[case_id]["capability"]["id"],
                "proof_kind": proof_kind,
                "outcome": "passed",
                "observation": copy.deepcopy(contracts.observations[observation_ref]),
            }
        )
    return {
        "format": comparator.REPORT_FORMAT,
        "binding": binding,
        "manifest": _source_identity(comparator.ROOT, comparator.MANIFEST_RELATIVE),
        "catalog": _source_identity(comparator.ROOT, comparator.CATALOG_RELATIVE),
        "fixture": {
            "id": "workforce-v3",
            "version": 3,
            "semantic_profile": "typedb-3.12.1/v1",
            "schema": _source_identity(comparator.ROOT, comparator.SCHEMA_RELATIVE),
            "provider_schema": _source_identity(
                comparator.ROOT, comparator.PROVIDER_SCHEMA_RELATIVE
            ),
            "journey": _source_identity(comparator.ROOT, comparator.JOURNEY_RELATIVE),
            "semantic_fingerprint": contracts.semantic_fingerprint,
            "projection_target": contracts.projection_targets[binding],
            "projection_fingerprint": contracts.projection_fingerprints[binding],
        },
        "results": results,
    }


def _write_reports(tmp_path: Path, contracts: Any) -> list[Path]:
    paths = []
    for binding in comparator.REPORT_BINDINGS:
        path = tmp_path / f"{binding}.json"
        path.write_bytes(comparator.canonical_json_bytes(_valid_report(binding, contracts)))
        paths.append(path)
    return paths


def test_finalized_contract_loads_exact_ledger_and_fingerprint_authority() -> None:
    contracts = comparator.load_contracts()

    assert contracts.authority_state == "finalized"
    assert contracts.semantic_fingerprint is not None
    assert set(contracts.projection_fingerprints) == set(comparator.REPORT_BINDINGS)
    assert contracts.selected == comparator.EXPECTED_SELECTED_PROOFS
    assert contracts.manifest_transition_cases == comparator.EXPECTED_MANIFEST_TRANSITION_CASES
    assert len(contracts.selected) == 21
    assert len(contracts.manifest_transition_cases) == 17
    assert {case_id for case_id, _, _ in contracts.selected} - set(
        contracts.manifest_transition_cases
    ) == comparator.EVIDENCE_ONLY_CASES

    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports([])
    assert rejected.value.code == "missing_binding"


@pytest.mark.parametrize(
    ("mutation", "code"),
    [
        ("selected_reordered", "selected_proof_mismatch"),
        ("transition_reordered", "manifest_transition_case_mismatch"),
        ("broad_case_transition", "manifest_transition_case_mismatch"),
        ("report_producer", "invalid_report_producer"),
        ("claimed_fingerprints", "invalid_object_fields"),
    ],
)
def test_finalized_catalog_mutations_fail_closed(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    mutation: str,
    code: str,
) -> None:
    _copy_authority(tmp_path)
    catalog_path = tmp_path / comparator.CATALOG_RELATIVE
    catalog = json.loads(catalog_path.read_text(encoding="utf-8"))
    if mutation == "selected_reordered":
        catalog["selected_proofs"][0], catalog["selected_proofs"][1] = (
            catalog["selected_proofs"][1],
            catalog["selected_proofs"][0],
        )
    elif mutation == "transition_reordered":
        catalog["manifest_transition_cases"][0], catalog["manifest_transition_cases"][1] = (
            catalog["manifest_transition_cases"][1],
            catalog["manifest_transition_cases"][0],
        )
    elif mutation == "broad_case_transition":
        catalog["manifest_transition_cases"][-1] = "workforce.runtime.cancellation"
    elif mutation == "report_producer":
        catalog["report_producers"]["c"]["id"] = "type-bridge-c.uncommitted"
    elif mutation == "claimed_fingerprints":
        catalog["expected_fingerprints"] = {}
    else:
        raise AssertionError(f"unhandled mutation {mutation}")
    _write_json(catalog_path, catalog)
    monkeypatch.setattr(comparator, "ROOT", tmp_path)

    with pytest.raises(comparator.ContractError) as rejected:
        comparator.load_contracts()
    assert rejected.value.code == code


@pytest.mark.parametrize(
    ("mutation", "code"),
    [
        ("missing_observation", "invalid_object_fields"),
        ("extra_observation_field", "observation_shape_mismatch"),
        ("plain_activity_role", "invalid_journey_record"),
    ],
)
def test_journey_observation_shape_drift_fails_closed(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    mutation: str,
    code: str,
) -> None:
    _copy_authority(tmp_path)
    journey_path = tmp_path / comparator.JOURNEY_RELATIVE
    journey = json.loads(journey_path.read_text(encoding="utf-8"))
    if mutation == "missing_observation":
        journey["expected_observations"].pop("ordered_distinct_collections")
    elif mutation == "extra_observation_field":
        journey["expected_observations"]["ordered_distinct_collections"]["unexpected"] = True
    elif mutation == "plain_activity_role":
        journey["records"]["plain_activity"]["roles"] = {
            "employee": [{"model": "person", "key": "data-ada"}]
        }
    else:
        raise AssertionError(f"unhandled mutation {mutation}")
    _write_json(journey_path, journey)
    monkeypatch.setattr(comparator, "ROOT", tmp_path)

    with pytest.raises(comparator.ContractError) as rejected:
        comparator.load_contracts()
    assert rejected.value.code == code


def test_finalized_fixture_derives_only_exact17_pending_after_successor_transition(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _copy_authority(tmp_path)
    _restore_candidate_manifest(tmp_path)
    _finalize_catalog(tmp_path)
    monkeypatch.setattr(comparator, "ROOT", tmp_path)
    contracts = comparator.load_contracts()

    summary = comparator.compare_reports(_write_reports(tmp_path, contracts))

    assert len(summary["passed_proofs"]) == 21
    assert [item["case_id"] for item in summary["pending_manifest_promotions"]] == list(
        comparator.EXPECTED_MANIFEST_TRANSITION_CASES
    )
    assert {item["case_id"] for item in summary["current_gaps"]}.isdisjoint(
        comparator.EVIDENCE_ONLY_CASES
    )
    assert comparator.EVIDENCE_ONLY_CASES.isdisjoint(
        item["case_id"] for item in summary["pending_manifest_promotions"]
    )


def test_finalized_reports_still_fail_closed_on_observation_mutation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _copy_authority(tmp_path)
    _finalize_catalog(tmp_path)
    monkeypatch.setattr(comparator, "ROOT", tmp_path)
    contracts = comparator.load_contracts()
    reports = {binding: _valid_report(binding, contracts) for binding in comparator.REPORT_BINDINGS}
    reports["python"]["results"][0]["observation"]["empty"]["provider_calls"] = 1
    paths = []
    for binding, report in reports.items():
        path = tmp_path / f"mutated-{binding}.json"
        path.write_bytes(comparator.canonical_json_bytes(report))
        paths.append(path)

    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports(paths)
    assert rejected.value.code == "observation_mismatch"
