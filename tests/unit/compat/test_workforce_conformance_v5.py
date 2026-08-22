"""Fail-closed tests for the Workforce V5 serialization comparator."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import shutil
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "scripts/ci/compare_workforce_conformance_v5.py"
SPEC = importlib.util.spec_from_file_location("workforce_v5_comparator", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
comparator = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = comparator
SPEC.loader.exec_module(comparator)


def _stage_contracts(destination: Path) -> Path:
    sdk_source = ROOT / "tests/contracts/sdk_conformance"
    sdk_target = destination / "tests/contracts/sdk_conformance"
    (sdk_target / "workforce-v5").mkdir(parents=True)
    shutil.copy2(sdk_source / "manifest-v1.json", sdk_target / "manifest-v1.json")
    for name in (
        "catalog-v5.json",
        "journey-v5.json",
        "observation-schema-v5.json",
        "report-schema-v5.json",
    ):
        shutil.copy2(sdk_source / "workforce-v5" / name, sdk_target / "workforce-v5" / name)
    record_target = destination / comparator.RECORD_CONTRACT
    record_target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(ROOT / comparator.RECORD_CONTRACT, record_target)
    return destination


def _identity(root: Path, relative: str) -> dict[str, str]:
    return {
        "path": relative,
        "sha256": hashlib.sha256((root / relative).read_bytes()).hexdigest(),
    }


def _observations() -> list[dict[str, object]]:
    return [
        {
            "record_sha256": [hashlib.sha256(bytes([index])).hexdigest() for index in range(9)],
            "archive_sha256": hashlib.sha256(b"archive").hexdigest(),
            "round_trip": True,
            "foreign_schema_code": "projected_record_schema_mismatch",
            "direct_remote_equal": True,
            "detached_mutation_code": "projected_snapshot_detached",
        },
        {"code": "projected_codec_cancelled", "partial_output": False},
        {
            "input_code": "projected_codec_input_limit",
            "output_code": "projected_codec_output_limit",
            "member_code": "projected_codec_member_limit",
            "depth_code": "projected_codec_depth_limit",
            "partial_output": False,
        },
        {
            "code": "projected_record_schema_mismatch",
            "category": "invalid_input",
            "path": ["declared_schema_identity"],
            "payload_absent": True,
        },
        {
            "bytes_closed": True,
            "builder_closed": True,
            "archive_closed": True,
            "decoded_closed": True,
            "repeat_close": True,
            "sibling_usable": True,
        },
    ]


def _valid_report(root: Path, binding: str) -> dict[str, object]:
    catalog = json.loads((root / comparator.CATALOG).read_text(encoding="utf-8"))
    results = []
    for case, proof, observation in zip(
        catalog["cases"], catalog["selected_proofs"], _observations(), strict=True
    ):
        results.append(
            {
                "case_id": case["id"],
                "capability_id": case["capability_id"],
                "proof_kind": proof["proof_kind"],
                "outcome": "passed",
                "observation": observation,
            }
        )
    return {
        "format": "typebridge.sdk-conformance-report/v5",
        "binding": binding,
        "manifest": _identity(root, comparator.MANIFEST),
        "catalog": _identity(root, comparator.CATALOG),
        "journey": _identity(root, comparator.JOURNEY),
        "record_contract": _identity(root, comparator.RECORD_CONTRACT),
        "server_version": "3.12.3",
        "results": results,
        "cleanup": {
            "managed_database_absent": True,
            "temporary_evidence_absent": True,
            "partial_output_absent": True,
        },
    }


def _write_reports(root: Path) -> list[Path]:
    paths = []
    for binding in comparator.BINDINGS:
        path = root / f"{binding}.json"
        path.write_bytes(comparator.canonical_json_bytes(_valid_report(root, binding)))
        paths.append(path)
    return paths


def test_accepts_exact_four_binding_candidate_fan_in(tmp_path: Path) -> None:
    root = _stage_contracts(tmp_path)

    comparison = comparator.compare_reports(_write_reports(root), root)

    assert comparison["authority_state"] == "phase0_unfinalized"
    assert comparison["bindings"] == list(comparator.BINDINGS)
    assert [item["case_id"] for item in comparison["pending_promotions"]] == [
        "workforce.model.serialization"
    ]


def test_journey_corpus_uses_types_owned_by_its_frozen_schema() -> None:
    journey = json.loads((ROOT / comparator.JOURNEY).read_text(encoding="utf-8"))
    schema = (ROOT / "tests/contracts/sdk_conformance/workforce-v3/schema-v3.yaml").read_text(
        encoding="utf-8"
    )

    assert journey["records"] == comparator.JOURNEY_RECORDS
    for source_name in (
        "robot_id",
        "player-stats",
        "person",
        "membership",
        "interaction",
        "container",
        "employment",
        "event",
    ):
        assert f"{source_name}:" in schema


def test_rejects_cross_binding_digest_drift(tmp_path: Path) -> None:
    root = _stage_contracts(tmp_path)
    paths = _write_reports(root)
    report = json.loads(paths[1].read_text(encoding="utf-8"))
    report["results"][0]["observation"]["archive_sha256"] = "f" * 64
    paths[1].write_bytes(comparator.canonical_json_bytes(report))

    with pytest.raises(comparator.ContractError) as raised:
        comparator.compare_reports(paths, root)
    assert raised.value.code == "cross_binding_observation_drift"


def test_rejects_stale_source_identity(tmp_path: Path) -> None:
    root = _stage_contracts(tmp_path)
    paths = _write_reports(root)
    report = json.loads(paths[0].read_text(encoding="utf-8"))
    report["record_contract"]["sha256"] = "0" * 64
    paths[0].write_bytes(comparator.canonical_json_bytes(report))

    with pytest.raises(comparator.ContractError) as raised:
        comparator.compare_reports(paths, root)
    assert raised.value.code == "stale_source_identity"


def test_rejects_catalog_row_rebinding(tmp_path: Path) -> None:
    root = _stage_contracts(tmp_path)
    paths = _write_reports(root)
    report = json.loads(paths[0].read_text(encoding="utf-8"))
    report["results"][0]["proof_kind"] = "diagnostic"
    paths[0].write_bytes(comparator.canonical_json_bytes(report))

    with pytest.raises(comparator.ContractError) as raised:
        comparator.compare_reports(paths, root)
    assert raised.value.code == "report_row_binding_drift"


def test_rejects_retained_gap_promotion(tmp_path: Path) -> None:
    root = _stage_contracts(tmp_path)
    manifest_path = root / comparator.MANIFEST
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    cancellation = next(
        item for item in manifest["capabilities"] if item["id"] == "runtime.cancellation"
    )
    cancellation["binding_profile"] = "current_and_c_live_future_planned"
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

    with pytest.raises(comparator.ContractError) as raised:
        comparator.compare_reports(_write_reports(root), root)
    assert raised.value.code == "retained_gap_promoted"


def test_finalized_authority_rejects_pending_g07(tmp_path: Path) -> None:
    root = _stage_contracts(tmp_path)
    catalog_path = root / comparator.CATALOG
    catalog = json.loads(catalog_path.read_text(encoding="utf-8"))
    catalog["authority_state"] = "finalized"
    catalog_path.write_text(json.dumps(catalog), encoding="utf-8")

    with pytest.raises(comparator.ContractError) as raised:
        comparator.compare_reports(_write_reports(root), root)
    assert raised.value.code == "finalized_authority_has_pending_promotions"


def test_duplicate_json_keys_are_rejected(tmp_path: Path) -> None:
    path = tmp_path / "duplicate.json"
    path.write_text('{"binding":"python","binding":"node"}', encoding="utf-8")

    with pytest.raises(comparator.ContractError) as raised:
        comparator.load_json(path)
    assert raised.value.code == "duplicate_json_key"


def test_noncanonical_report_json_is_rejected(tmp_path: Path) -> None:
    root = _stage_contracts(tmp_path)
    paths = _write_reports(root)
    paths[0].write_text(json.dumps(_valid_report(root, "python"), indent=2), encoding="utf-8")

    with pytest.raises(comparator.ContractError) as raised:
        comparator.compare_reports(paths, root)
    assert raised.value.code == "noncanonical_report_json"
