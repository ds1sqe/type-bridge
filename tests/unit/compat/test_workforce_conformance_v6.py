"""Fail-closed tests for artifact-bound Workforce V6 fan-in."""

from __future__ import annotations

import importlib.util
import json
import shutil
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "scripts/ci/compare_workforce_conformance_v6.py"
SPEC = importlib.util.spec_from_file_location("workforce_v6_comparator", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
COMPARATOR = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = COMPARATOR
SPEC.loader.exec_module(COMPARATOR)


def _stage(tmp_path: Path) -> Path:
    for relative in (
        COMPARATOR.MANIFEST,
        COMPARATOR.CATALOG,
        COMPARATOR.JOURNEY,
        COMPARATOR.LEDGER,
        COMPARATOR.REPORT_SCHEMA,
        COMPARATOR.PRODUCER_SCHEMA,
    ):
        target = tmp_path / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / relative, target)
    return tmp_path


def _identity(root: Path, relative: str) -> dict[str, str]:
    return {"path": relative, "sha256": COMPARATOR.sha256(root / relative)}


def _report(root: Path, binding: str) -> dict[str, object]:
    catalog = json.loads((root / COMPARATOR.CATALOG).read_text(encoding="utf-8"))
    manifest = json.loads((root / COMPARATOR.MANIFEST).read_text(encoding="utf-8"))
    artifacts = {
        "cli": {"candidate-id": f"sha256:{'1' * 64}", "sha256": "2" * 64},
        "generated-package": {"candidate-id": f"sha256:{'3' * 64}", "sha256": "4" * 64},
        "runtime": {"candidate-id": f"sha256:{'5' * 64}", "sha256": "6" * 64},
    }
    observations = [
        {"code": "operation_cancelled", "partial_output": False, "terminal": True},
        {
            "deadline_code": "deadline_exceeded",
            "limit_code": "resource_limit_exceeded",
            "partial_output": False,
            "tighten_only": True,
        },
        {
            "category": "invalid_input",
            "code": "artifact_or_operation_rejected",
            "details_redacted": True,
            "path_present": True,
        },
        {
            "candidate-id": artifacts["cli"]["candidate-id"],
            "offline_commands": 5,
            "relocated": True,
            "sdk_independent": True,
            "sha256": artifacts["cli"]["sha256"],
        },
        {
            "explicit_close": True,
            "native_library_unmapped": True,
            "parent_child_order": True,
            "post_close_rejected": True,
            "repeat_close": True,
            "sibling_independent": True,
        },
    ]
    return {
        "format": "typebridge.sdk-conformance-report/v6",
        "binding": binding,
        "producer": catalog["candidate_report_producers"][binding],
        "source_commit": "7" * 40,
        "manifest": _identity(root, COMPARATOR.MANIFEST),
        "catalog": _identity(root, COMPARATOR.CATALOG),
        "journey": _identity(root, COMPARATOR.JOURNEY),
        "broad_case_ledger": _identity(root, COMPARATOR.LEDGER),
        "phase4_report_sha256": "8" * 64,
        "artifacts": artifacts,
        "generated_surface": {
            "format": COMPARATOR.SURFACE_FORMATS[binding],
            "candidate-id": (
                artifacts["generated-package"]["candidate-id"]
                if binding == "c"
                else f"sha256:{binding.encode().hex():0<64}"
            ),
            "sha256": (
                artifacts["generated-package"]["sha256"]
                if binding == "c"
                else f"{COMPARATOR.BINDINGS.index(binding) + 9:x}" * 64
            ),
        },
        "predecessor_reports": [
            {"version": version, "sha256": f"{version:x}" * 64}
            for version in COMPARATOR.predecessor_versions(binding)
        ],
        "non_selected_proofs": [
            {
                "capability_id": capability_id,
                "proof_kind": proof_kind,
                "evidence_sha256": f"{index % 15 + 1:x}" * 64,
            }
            for index, (capability_id, proof_kind) in enumerate(
                (
                    (capability_id, proof_kind)
                    for capability_id, selected in zip(
                        COMPARATOR.CAPABILITIES, COMPARATOR.PROOFS, strict=True
                    )
                    for capability in manifest["capabilities"]
                    if capability["id"] == capability_id
                    for proof_kind, disposition in manifest["proof_profiles"][
                        capability["proof_profile"]
                    ].items()
                    if disposition == "required" and proof_kind != selected
                )
            )
        ],
        "results": [
            {
                "case_id": case_id,
                "capability_id": capability_id,
                "proof_kind": proof_kind,
                "outcome": "passed",
                "observation": observation,
            }
            for case_id, capability_id, proof_kind, observation in zip(
                COMPARATOR.CASES,
                COMPARATOR.CAPABILITIES,
                COMPARATOR.PROOFS,
                observations,
                strict=True,
            )
        ],
        "cleanup": {
            "managed_database_absent": True,
            "migration_database_absent": True,
            "native_library_unmapped": True,
            "temporary_evidence_absent": True,
        },
        "publication_authority": False,
    }


def _reports(root: Path) -> list[Path]:
    paths = []
    for binding in COMPARATOR.BINDINGS:
        path = root / f"{binding}.json"
        path.write_bytes(COMPARATOR.canonical_json_bytes(_report(root, binding)))
        paths.append(path)
    return paths


def _set_pending(root: Path) -> None:
    manifest_path = root / COMPARATOR.MANIFEST
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    for capability in manifest["capabilities"]:
        if capability["case_ids"][0] in COMPARATOR.CASES:
            capability["binding_profile"] = COMPARATOR.GAP_PROFILE
            capability["gap_reason"] = "terminal V6 promotion is pending"
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")


def test_accepts_exact_five_pending_candidate_promotions(tmp_path: Path) -> None:
    root = _stage(tmp_path)
    _set_pending(root)
    comparison = COMPARATOR.compare_reports(_reports(root), root)
    assert comparison["authority_state"] == "candidate"
    assert comparison["pending_promotions"] == list(COMPARATOR.CASES)
    assert comparison["publication_authority"] is False


def test_accepts_only_complete_dedicated_profile_transition(tmp_path: Path) -> None:
    root = _stage(tmp_path)
    manifest_path = root / COMPARATOR.MANIFEST
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    manifest["binding_profiles"][COMPARATOR.BROAD_PROFILE] = {
        "accepted_offline": [],
        "accepted_live": list(COMPARATOR.BINDINGS),
        "gap": [],
        "planned": ["kotlin-jvm", "haskell", "go", "dotnet"],
    }
    manifest["binding_profiles"][COMPARATOR.DISTRIBUTION_PROFILE] = {
        "accepted_offline": list(COMPARATOR.BINDINGS),
        "accepted_live": [],
        "gap": [],
        "planned": ["kotlin-jvm", "haskell", "go", "dotnet"],
    }
    for capability in manifest["capabilities"]:
        if capability["case_ids"][0] in COMPARATOR.CASES:
            capability["binding_profile"] = (
                COMPARATOR.DISTRIBUTION_PROFILE
                if capability["case_ids"][0] == "workforce.distribution.standalone-cli"
                else COMPARATOR.BROAD_PROFILE
            )
            capability.pop("gap_reason", None)
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

    comparison = COMPARATOR.compare_reports(_reports(root), root)
    assert comparison["authority_state"] == "finalized"
    assert comparison["pending_promotions"] == []


def test_rejects_partial_manifest_transition(tmp_path: Path) -> None:
    root = _stage(tmp_path)
    _set_pending(root)
    manifest_path = root / COMPARATOR.MANIFEST
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    manifest["capabilities"][-9]["binding_profile"] = COMPARATOR.BROAD_PROFILE
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    with pytest.raises(COMPARATOR.ContractError) as raised:
        COMPARATOR.compare_reports(_reports(root), root)
    assert raised.value.code == "partial_manifest_transition"


def test_rejects_cross_binding_artifact_drift(tmp_path: Path) -> None:
    root = _stage(tmp_path)
    paths = _reports(root)
    report = json.loads(paths[1].read_text(encoding="utf-8"))
    report["artifacts"]["cli"]["sha256"] = "f" * 64
    report["results"][3]["observation"]["sha256"] = "f" * 64
    paths[1].write_bytes(COMPARATOR.canonical_json_bytes(report))
    with pytest.raises(COMPARATOR.ContractError) as raised:
        COMPARATOR.compare_reports(paths, root)
    assert raised.value.code == "cross_binding_evidence_drift"


def test_rejects_stale_ledger_identity(tmp_path: Path) -> None:
    root = _stage(tmp_path)
    paths = _reports(root)
    report = json.loads(paths[0].read_text(encoding="utf-8"))
    report["broad_case_ledger"]["sha256"] = "0" * 64
    paths[0].write_bytes(COMPARATOR.canonical_json_bytes(report))
    with pytest.raises(COMPARATOR.ContractError) as raised:
        COMPARATOR.compare_reports(paths, root)
    assert raised.value.code == "stale_source_identity"


def test_rejects_missing_non_selected_proof(tmp_path: Path) -> None:
    root = _stage(tmp_path)
    paths = _reports(root)
    report = json.loads(paths[2].read_text(encoding="utf-8"))
    report["non_selected_proofs"].pop()
    paths[2].write_bytes(COMPARATOR.canonical_json_bytes(report))
    with pytest.raises(COMPARATOR.ContractError) as raised:
        COMPARATOR.compare_reports(paths, root)
    assert raised.value.code == "non_selected_proof_scope_drift"


def test_rejects_noncanonical_report(tmp_path: Path) -> None:
    root = _stage(tmp_path)
    paths = _reports(root)
    paths[0].write_text(json.dumps(_report(root, "python"), indent=2), encoding="utf-8")
    with pytest.raises(COMPARATOR.ContractError) as raised:
        COMPARATOR.compare_reports(paths, root)
    assert raised.value.code == "noncanonical_report_json"
