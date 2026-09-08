"""Provider-free tests for the Sdk V3 report assembler."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
CI = ROOT / "scripts/ci"
ASSEMBLER_PATH = CI / "assemble_sdk_conformance_v3.py"

FRAGMENT_LANES = {
    ("complete_connection_policy", "direct_runtime"),
    ("data_operation_cancellation", "direct_runtime"),
    ("data_operation_resource_limits", "direct_runtime"),
    ("data_operation_structured_diagnostic", "diagnostic"),
    ("projected_constraint_validation", "diagnostic"),
    ("projection_evidence_integrity", "diagnostic"),
    ("token_package_fencing", "diagnostic"),
}


def _load_assembler():
    sys.path.insert(0, str(CI))
    spec = importlib.util.spec_from_file_location("sdk_v3_assembler", ASSEMBLER_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


assembler = _load_assembler()


def _inputs(binding: str = "rust"):
    contracts = assembler.conformance.load_contracts()
    proof = {lane: contracts.observations[lane[0]] for lane in FRAGMENT_LANES}
    live_lanes = sorted(
        {(observation_ref, proof_kind) for _, proof_kind, observation_ref in contracts.selected}
        - FRAGMENT_LANES
    )
    live = {
        "format": assembler.LIVE_FORMAT,
        "binding": binding,
        "semantic_profile": assembler.SEMANTIC_PROFILE,
        "producer": contracts.report_producers[binding]["id"],
        "semantic_fingerprint": contracts.semantic_fingerprint,
        "projection_fingerprint": contracts.projection_fingerprints[binding],
        "results": [
            {
                "observation_ref": observation_ref,
                "proof_kind": proof_kind,
                "outcome": "passed",
                "observation": contracts.observations[observation_ref],
            }
            for observation_ref, proof_kind in live_lanes
        ],
    }
    return contracts, proof, live


def test_assembler_builds_one_comparator_valid_exact_21_row_report() -> None:
    contracts, proof, live = _inputs()

    report = assembler.assemble_report(
        live,
        binding="rust",
        contracts=contracts,
        proof_observations=proof,
    )

    assert len(live["results"]) == 14
    assert len(report["results"]) == 21
    assert report["fixture"]["projection_target"] == "rust"
    assembler.conformance._validate_report(
        assembler.conformance.LoadedJson(
            path=Path("rust-v3.json"),
            raw=assembler.conformance.canonical_json_bytes(report),
            value=report,
        ),
        contracts,
    )


def test_assembler_rejects_missing_live_lane_and_fragment_overlap() -> None:
    contracts, proof, live = _inputs()
    live["results"].pop()
    with pytest.raises(assembler.AssemblyError) as missing:
        assembler.assemble_report(
            live,
            binding="rust",
            contracts=contracts,
            proof_observations=proof,
        )
    assert missing.value.code == "live_lane_coverage_mismatch"

    contracts, proof, live = _inputs()
    overlap = (live["results"][0]["observation_ref"], live["results"][0]["proof_kind"])
    proof[overlap] = live["results"][0]["observation"]
    with pytest.raises(assembler.AssemblyError) as rejected:
        assembler.assemble_report(
            live,
            binding="rust",
            contracts=contracts,
            proof_observations=proof,
        )
    assert rejected.value.code == "unexpected_live_lane"


def test_assembler_source_cannot_import_expected_journey_objects() -> None:
    source = ASSEMBLER_PATH.read_text(encoding="utf-8")

    assert "expected_observations" not in source
    assert "contracts.observations" not in source


def test_report_publication_is_create_new_and_leaves_no_temporary_file(tmp_path: Path) -> None:
    destination = tmp_path / "rust-v3.json"
    assembler._publish(destination, {"status": "passed"})

    assert destination.read_bytes() == b'{"status":"passed"}\n'
    assert list(tmp_path.iterdir()) == [destination]

    with pytest.raises(assembler.AssemblyError) as rejected:
        assembler._publish(destination, {"status": "replaced"})
    assert rejected.value.code == "report_publication_failed"
    assert destination.read_bytes() == b'{"status":"passed"}\n'
    assert list(tmp_path.iterdir()) == [destination]
