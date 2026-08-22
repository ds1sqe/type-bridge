"""Fail-closed tests for the four-binding workforce-v2 transition gate."""

from __future__ import annotations

import copy
import importlib.util
import json
import sys
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

SOURCE_ROOT = Path(__file__).resolve().parents[3]
COMPARATOR_PATH = SOURCE_ROOT / "scripts/ci/compare_workforce_conformance_v2.py"


def _load_comparator() -> ModuleType:
    spec = importlib.util.spec_from_file_location(
        "workforce_conformance_v2_comparator",
        COMPARATOR_PATH,
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


comparator = _load_comparator()

EVIDENCE_ONLY_SELECTED_CASES = {
    "workforce.diagnostic.all-workflows",
    "workforce.runtime.cancellation",
    "workforce.runtime.explicit-close",
    "workforce.runtime.timeout-resource-limits",
}
FOUR_LIVE_PROFILE = "current_and_c_live_future_planned"
PREPROMOTION_BINDING_PROFILES = {
    "workforce.model.values-and-references": "current_live_future_planned",
    "workforce.crud.entity-single": "current_live_future_planned",
    "workforce.crud.relation-single": "current_live_future_planned",
    "workforce.query.owner-iid-set": "current_live_future_planned",
    "workforce.query.exact-subtypes": "current_live_future_planned",
    "workforce.query.scalar-boolean-predicates": "current_live_future_planned",
    "workforce.query.roles": "current_live_future_planned",
    "workforce.query.topology": "current_live_future_planned",
    "workforce.query.selection-shapes": "current_live_future_planned",
    "workforce.query.terminals": "current_live_future_planned",
    "workforce.query.reducers-direct": "current_live_future_planned",
    "workforce.query.remote-one-exchange": "current_live_future_planned",
    "workforce.query.remote-hydration": "current_live_future_planned",
    "workforce.diagnostic.remote-structured": "current_offline_future_planned",
    "workforce.value.scalar-domain-comparison": "current_live_future_planned",
    "workforce.query.reducers-remote": "current_live_future_planned",
    "workforce.query.schema-function": "current_gap_future_planned",
}


def _write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(comparator.canonical_json_bytes(value))


def _assign_transition_profiles(manifest: dict[str, Any], profiles: dict[str, str]) -> None:
    transition_cases = set(comparator.EXPECTED_MANIFEST_TRANSITION_CASES)
    assert set(profiles) == transition_cases
    assigned: set[str] = set()
    for capability in manifest["capabilities"]:
        case_id = capability["case_ids"][0]
        if case_id in profiles:
            capability["binding_profile"] = profiles[case_id]
            assigned.add(case_id)
    assert assigned == transition_cases


def _promote_report_bindings(manifest: dict[str, Any]) -> None:
    manifest["binding_profiles"][FOUR_LIVE_PROFILE] = {
        "accepted_offline": [],
        "accepted_live": list(comparator.REPORT_BINDINGS),
        "gap": [],
        "planned": ["kotlin-jvm", "haskell", "go", "dotnet"],
    }
    _assign_transition_profiles(
        manifest,
        dict.fromkeys(comparator.EXPECTED_MANIFEST_TRANSITION_CASES, FOUR_LIVE_PROFILE),
    )


def _demote_report_bindings(manifest: dict[str, Any]) -> None:
    _assign_transition_profiles(manifest, PREPROMOTION_BINDING_PROFILES)


def _write_authority(root: Path, *, promoted: bool) -> None:
    manifest = json.loads((SOURCE_ROOT / comparator.MANIFEST_RELATIVE).read_text(encoding="utf-8"))
    if promoted:
        _promote_report_bindings(manifest)
    else:
        _demote_report_bindings(manifest)
    _write_json(root / comparator.MANIFEST_RELATIVE, manifest)

    journey = json.loads((SOURCE_ROOT / comparator.JOURNEY_RELATIVE).read_text(encoding="utf-8"))
    report_schema = json.loads(
        (SOURCE_ROOT / comparator.REPORT_SCHEMA_RELATIVE).read_text(encoding="utf-8")
    )
    _write_json(root / comparator.JOURNEY_RELATIVE, journey)
    _write_json(root / comparator.REPORT_SCHEMA_RELATIVE, report_schema)

    for relative in (comparator.SCHEMA_RELATIVE, comparator.PROVIDER_SCHEMA_RELATIVE):
        source = SOURCE_ROOT / relative
        destination = root / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(source.read_bytes())

    selected = [
        {
            "case_id": case_id,
            "proof_kind": proof_kind,
            "observation_ref": observation_ref,
        }
        for case_id, proof_kind, observation_ref in comparator.EXPECTED_SELECTED_PROOFS
    ]
    selected_cases = {item["case_id"] for item in selected}
    cases = []
    for capability in manifest["capabilities"]:
        case_id = capability["case_ids"][0]
        statuses = comparator._expand_binding_profile(
            manifest,
            capability["binding_profile"],
        )
        if case_id in selected_cases:
            disposition = "shared_smoke"
        elif all(statuses.get(binding) == "gap" for binding in comparator.REPORT_BINDINGS):
            disposition = "known_gap"
        else:
            disposition = "retained_evidence"
        cases.append(
            {
                "id": case_id,
                "capability_id": capability["id"],
                "disposition": disposition,
            }
        )

    fingerprint = {
        "algorithm": "sha256",
        "canonicalization": "typebridge.schema-canonical-json/v1",
        "digest": "a" * 64,
        "domain": "typebridge.schema.semantic",
        "semantic_profile": "typedb-3.12.1/v1",
    }
    projection_fingerprints = {}
    for index, binding in enumerate(comparator.REPORT_BINDINGS, start=11):
        projection_fingerprints[binding] = {
            "algorithm": "sha256",
            "canonicalization": "typebridge.binding-projection/v1",
            "digest": f"{index:x}" * 64,
            "domain": "typebridge.binding.projection",
            "semantic_profile": "typedb-3.12.1/v1",
        }
    catalog = {
        "format": comparator.CATALOG_FORMAT,
        "fixture": {
            "id": "workforce-v2",
            "version": 2,
            "semantic_profile": "typedb-3.12.1/v1",
            "schema_path": comparator.SCHEMA_RELATIVE,
            "provider_schema_path": comparator.PROVIDER_SCHEMA_RELATIVE,
        },
        "journey_path": comparator.JOURNEY_RELATIVE,
        "report_schema_path": comparator.REPORT_SCHEMA_RELATIVE,
        "report_bindings": list(comparator.REPORT_BINDINGS),
        "projection_targets": comparator.PROJECTION_TARGETS,
        "expected_fingerprints": {
            "semantic": fingerprint,
            "projections": projection_fingerprints,
        },
        "manifest_transition_cases": list(comparator.EXPECTED_MANIFEST_TRANSITION_CASES),
        "selected_proofs": selected,
        "cases": cases,
    }
    _write_json(root / comparator.CATALOG_RELATIVE, catalog)


def _valid_report(binding: str, contracts: Any) -> dict[str, Any]:
    results = []
    for case_id, proof_kind, observation_ref in contracts.selected:
        results.append(
            {
                "capability_id": contracts.cases[case_id]["capability"]["id"],
                "case_id": case_id,
                "observation": copy.deepcopy(contracts.observations[observation_ref]),
                "outcome": "passed",
                "proof_kind": proof_kind,
            }
        )
    return {
        "binding": binding,
        "catalog": {
            "path": comparator.CATALOG_RELATIVE,
            "sha256": contracts.catalog.sha256,
        },
        "fixture": {
            "id": "workforce-v2",
            "journey": {
                "path": comparator.JOURNEY_RELATIVE,
                "sha256": contracts.journey.sha256,
            },
            "projection_fingerprint": copy.deepcopy(contracts.projection_fingerprints[binding]),
            "projection_target": contracts.projection_targets[binding],
            "provider_schema": {
                "path": comparator.PROVIDER_SCHEMA_RELATIVE,
                "sha256": contracts.provider_schema.sha256,
            },
            "schema": {
                "path": comparator.SCHEMA_RELATIVE,
                "sha256": contracts.schema.sha256,
            },
            "semantic_fingerprint": copy.deepcopy(contracts.semantic_fingerprint),
            "semantic_profile": "typedb-3.12.1/v1",
            "version": 2,
        },
        "format": comparator.REPORT_FORMAT,
        "manifest": {
            "path": comparator.MANIFEST_RELATIVE,
            "sha256": contracts.manifest.sha256,
        },
        "results": results,
    }


def _write_reports(root: Path, contracts: Any) -> list[Path]:
    return _write_report_values(
        root,
        {binding: _valid_report(binding, contracts) for binding in comparator.REPORT_BINDINGS},
    )


def _write_report_values(root: Path, reports: dict[str, dict[str, Any]]) -> list[Path]:
    report_root = root / "reports"
    paths = []
    for binding, report in reports.items():
        path = report_root / f"{binding}.json"
        _write_json(path, report)
        paths.append(path)
    return paths


def test_committed_catalog_and_manifest_freeze_exact_transition_subset() -> None:
    contracts = comparator.load_contracts()

    assert contracts.manifest.value["phase"] == "phase-1-shared-workforce-baseline"
    assert contracts.manifest.value["canonical_case_catalog"] == {
        "state": "accepted_shared_runtime_baseline",
        "path": "tests/contracts/sdk_conformance/workforce-v1",
    }
    assert contracts.manifest_transition_cases == comparator.EXPECTED_MANIFEST_TRANSITION_CASES
    assert contracts.catalog.value["manifest_transition_cases"] == list(
        comparator.EXPECTED_MANIFEST_TRANSITION_CASES
    )
    selected_cases = {case_id for case_id, _, _ in contracts.selected}
    assert set(contracts.manifest_transition_cases) < selected_cases
    assert selected_cases - set(contracts.manifest_transition_cases) == (
        EVIDENCE_ONLY_SELECTED_CASES
    )
    promoted_cases = {
        capability["case_ids"][0]
        for capability in contracts.capabilities
        if capability["binding_profile"] == FOUR_LIVE_PROFILE
    }
    assert set(comparator.EXPECTED_MANIFEST_TRANSITION_CASES) <= promoted_cases
    for case_id in contracts.manifest_transition_cases:
        capability = contracts.cases[case_id]["capability"]
        statuses = comparator._expand_binding_profile(
            contracts.manifest.value,
            capability["binding_profile"],
        )
        assert all(statuses[binding] == "accepted_live" for binding in comparator.REPORT_BINDINGS)


def test_candidate_reports_pass_only_as_explicit_pending_manifest_promotions(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _write_authority(tmp_path, promoted=False)
    monkeypatch.setattr(comparator, "ROOT", tmp_path)
    contracts = comparator.load_contracts()

    summary = comparator.compare_reports(_write_reports(tmp_path, contracts))

    assert len(summary["passed_proofs"]) == 34
    assert [promotion["case_id"] for promotion in summary["pending_manifest_promotions"]] == list(
        comparator.EXPECTED_MANIFEST_TRANSITION_CASES
    )
    assert {
        item["current_status"]
        for promotion in summary["pending_manifest_promotions"]
        for item in promotion["bindings"]
    } == {"accepted_offline", "gap", "planned"}
    gap_cases = {gap["case_id"] for gap in summary["current_gaps"]}
    assert EVIDENCE_ONLY_SELECTED_CASES <= gap_cases
    assert EVIDENCE_ONLY_SELECTED_CASES.isdisjoint(
        promotion["case_id"] for promotion in summary["pending_manifest_promotions"]
    )
    assert all(
        contracts.cases[case_id]["catalog"]["disposition"] == "shared_smoke"
        for case_id in EVIDENCE_ONLY_SELECTED_CASES
    )
    encoded = comparator.canonical_json_bytes(summary).decode()
    assert '"accepted":true' not in encoded
    assert "full_support" not in encoded


def test_final_promoted_reports_have_no_pending_manifest_transition(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _write_authority(tmp_path, promoted=True)
    monkeypatch.setattr(comparator, "ROOT", tmp_path)
    contracts = comparator.load_contracts()

    summary = comparator.compare_reports(_write_reports(tmp_path, contracts))

    assert summary["pending_manifest_promotions"] == []
    gap_cases = {gap["case_id"] for gap in summary["current_gaps"]}
    assert EVIDENCE_ONLY_SELECTED_CASES <= gap_cases
    for case_id in EVIDENCE_ONLY_SELECTED_CASES:
        capability = contracts.cases[case_id]["capability"]
        statuses = comparator._expand_binding_profile(
            contracts.manifest.value,
            capability["binding_profile"],
        )
        assert {statuses[binding] for binding in ("python", "node", "rust")} == {"gap"}
        assert statuses["c"] == "planned"
    later_plan05 = contracts.cases["workforce.schema.ordered-distinct"]["capability"]
    later_plan05_statuses = comparator._expand_binding_profile(
        contracts.manifest.value,
        later_plan05["binding_profile"],
    )
    assert all(
        later_plan05_statuses[binding] == "accepted_live" for binding in comparator.REPORT_BINDINGS
    )


@pytest.mark.parametrize(
    ("mutation", "expected_code"),
    [
        ("missing", "manifest_transition_case_mismatch"),
        ("extra_unselected", "manifest_transition_case_not_selected"),
        ("duplicate", "duplicate_manifest_transition_case"),
        ("reordered", "manifest_transition_case_mismatch"),
    ],
)
def test_manifest_transition_case_inventory_fails_closed(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    mutation: str,
    expected_code: str,
) -> None:
    _write_authority(tmp_path, promoted=False)
    catalog_path = tmp_path / comparator.CATALOG_RELATIVE
    catalog = json.loads(catalog_path.read_text(encoding="utf-8"))
    cases = catalog["manifest_transition_cases"]
    if mutation == "missing":
        cases.pop()
    elif mutation == "extra_unselected":
        cases.append("workforce.schema.ordered-distinct")
    elif mutation == "duplicate":
        cases.append(cases[0])
    elif mutation == "reordered":
        cases[0], cases[1] = cases[1], cases[0]
    else:
        raise AssertionError(f"unhandled mutation {mutation}")
    _write_json(catalog_path, catalog)
    monkeypatch.setattr(comparator, "ROOT", tmp_path)

    with pytest.raises(comparator.ContractError) as rejected:
        comparator.load_contracts()
    assert rejected.value.code == expected_code


def test_candidate_transition_cannot_smuggle_an_unselected_proof(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _write_authority(tmp_path, promoted=False)
    monkeypatch.setattr(comparator, "ROOT", tmp_path)
    contracts = comparator.load_contracts()
    reports = {binding: _valid_report(binding, contracts) for binding in comparator.REPORT_BINDINGS}
    reports["c"]["results"][0]["case_id"] = "workforce.schema.ordered-distinct"
    paths = _write_report_values(tmp_path, reports)

    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports(paths)
    assert rejected.value.code in {"capability_mapping_mismatch", "result_coverage_mismatch"}


@pytest.mark.parametrize(
    ("mutation", "expected_code"),
    [
        ("manifest_digest", "manifest_digest_mismatch"),
        ("observation", "observation_mismatch"),
        ("runtime_identity", "runtime_identity_leak"),
        ("duplicate_result", "duplicate_result"),
        ("missing_result", "result_coverage_mismatch"),
    ],
)
def test_candidate_reports_fail_closed_on_hostile_mutations(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    mutation: str,
    expected_code: str,
) -> None:
    _write_authority(tmp_path, promoted=False)
    monkeypatch.setattr(comparator, "ROOT", tmp_path)
    contracts = comparator.load_contracts()
    reports = {binding: _valid_report(binding, contracts) for binding in comparator.REPORT_BINDINGS}
    if mutation == "manifest_digest":
        reports["python"]["manifest"]["sha256"] = "0" * 64
    elif mutation == "observation":
        reports["python"]["results"][0]["observation"]["created"] = False
    elif mutation == "runtime_identity":
        reports["python"]["results"][0]["observation"]["provider_iid"] = "0x1234"
    elif mutation == "duplicate_result":
        reports["python"]["results"][1] = copy.deepcopy(reports["python"]["results"][0])
    elif mutation == "missing_result":
        reports["python"]["results"].pop()
    else:
        raise AssertionError(f"unhandled mutation {mutation}")

    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports(_write_report_values(tmp_path, reports))
    assert rejected.value.code == expected_code


def test_candidate_requires_all_four_distinct_binding_reports(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _write_authority(tmp_path, promoted=False)
    monkeypatch.setattr(comparator, "ROOT", tmp_path)
    contracts = comparator.load_contracts()
    reports = {
        binding: _valid_report(binding, contracts)
        for binding in comparator.REPORT_BINDINGS
        if binding != "c"
    }

    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports(_write_report_values(tmp_path, reports))
    assert rejected.value.code == "missing_binding"
