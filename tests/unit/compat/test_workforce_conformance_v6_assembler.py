"""Fail-closed tests for Workforce V6 producer-evidence assembly."""

from __future__ import annotations

import base64
import hashlib
import importlib.util
import json
import sys
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[3]
CI = ROOT / "scripts/ci"
sys.path.insert(0, str(CI))
SPEC = importlib.util.spec_from_file_location(
    "workforce_v6_assembler", CI / "assemble_workforce_conformance_v6.py"
)
assert SPEC is not None and SPEC.loader is not None
ASSEMBLER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = ASSEMBLER
SPEC.loader.exec_module(ASSEMBLER)


def _proof_rows(binding: str) -> list[dict[str, str]]:
    manifest = ASSEMBLER.conformance.load_json(ROOT / ASSEMBLER.conformance.MANIFEST)
    by_id = {item["id"]: item for item in manifest["capabilities"]}
    rows = []
    index = 0
    for capability_id, selected in zip(
        ASSEMBLER.conformance.CAPABILITIES, ASSEMBLER.conformance.PROOFS, strict=True
    ):
        capability = by_id[capability_id]
        for proof_kind, disposition in manifest["proof_profiles"][
            capability["proof_profile"]
        ].items():
            if disposition == "required" and proof_kind != selected:
                rows.append(
                    {
                        "capability_id": capability_id,
                        "proof_kind": proof_kind,
                        "evidence_b64": base64.b64encode(
                            f"{binding}:{index}:{capability_id}:{proof_kind}".encode()
                        ).decode(),
                    }
                )
                index += 1
    return rows


def _observations() -> list[dict[str, object]]:
    return [
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
            "candidate-id": f"sha256:{'1' * 64}",
            "offline_commands": 5,
            "relocated": True,
            "sdk_independent": True,
            "sha256": "2" * 64,
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


def _evidence(binding: str = "rust") -> dict[str, Any]:
    catalog = ASSEMBLER.conformance.load_json(ROOT / ASSEMBLER.conformance.CATALOG)
    return {
        "format": ASSEMBLER.FORMAT,
        "binding": binding,
        "producer": catalog["candidate_report_producers"][binding],
        "run_nonce": "a" * 64,
        "source_commit": "b" * 40,
        "observations": _observations(),
        "non_selected_proofs": _proof_rows(binding),
        "cleanup": {
            "managed_database_absent": True,
            "migration_database_absent": True,
            "native_library_unmapped": True,
            "temporary_evidence_absent": True,
        },
    }


def _phase4() -> dict[str, Any]:
    return {
        "format": "typebridge.c-artifact-phase4-report/v1",
        "publication-disposition": "candidate-only-unpublished-unsupported",
        "artifacts": {
            "cli": {"candidate-id": f"sha256:{'1' * 64}", "sha256": "2" * 64},
            "generated-package": {
                "candidate-id": f"sha256:{'3' * 64}",
                "sha256": "4" * 64,
            },
            "runtime": {"candidate-id": f"sha256:{'5' * 64}", "sha256": "6" * 64},
        },
    }


def _predecessors(binding: str = "rust") -> list[bytes]:
    values = []
    for version in range(1, 6):
        report = {
            "format": f"typebridge.sdk-conformance-report/v{version}",
            "binding": binding,
        }
        body = ASSEMBLER.conformance.canonical_json_bytes(report)
        if version in (1, 2, 3):
            body += b"\n"
        elif version == 4:
            body = json.dumps(report, indent=2).encode() + b"\n"
        values.append(body)
    return values


def _assemble(evidence: dict[str, Any] | None = None) -> dict[str, Any]:
    return ASSEMBLER.assemble_report(
        evidence or _evidence(),
        binding="rust",
        run_nonce="a" * 64,
        phase4=_phase4(),
        phase4_sha256="7" * 64,
        generated_surface=b"rust-generated-surface",
        predecessor_reports=_predecessors(),
        root=ROOT,
    )


def test_assembler_binds_real_fragment_surface_and_predecessor_bytes() -> None:
    evidence = _evidence()
    report = _assemble(evidence)
    assert (
        report["generated_surface"]["sha256"]
        == hashlib.sha256(b"rust-generated-surface").hexdigest()
    )
    assert (
        report["non_selected_proofs"][0]["evidence_sha256"]
        == hashlib.sha256(
            base64.b64decode(evidence["non_selected_proofs"][0]["evidence_b64"])
        ).hexdigest()
    )
    assert (
        report["predecessor_reports"][4]["sha256"] == hashlib.sha256(_predecessors()[4]).hexdigest()
    )
    ASSEMBLER.conformance.validate_report(report, ROOT)


def test_rejects_replayed_run_nonce() -> None:
    with pytest.raises(ASSEMBLER.AssemblyError) as raised:
        ASSEMBLER.assemble_report(
            _evidence(),
            binding="rust",
            run_nonce="c" * 64,
            phase4=_phase4(),
            phase4_sha256="7" * 64,
            generated_surface=b"rust-generated-surface",
            predecessor_reports=_predecessors(),
            root=ROOT,
        )
    assert raised.value.code == "run_nonce_mismatch"


def test_rejects_duplicate_proof_fragment() -> None:
    evidence = _evidence()
    evidence["non_selected_proofs"][1]["evidence_b64"] = evidence["non_selected_proofs"][0][
        "evidence_b64"
    ]
    with pytest.raises(ASSEMBLER.AssemblyError) as raised:
        _assemble(evidence)
    assert raised.value.code == "duplicate_proof_fragment"


def test_rejects_wrong_predecessor_binding() -> None:
    hostile = _predecessors()
    hostile[2] = (
        ASSEMBLER.conformance.canonical_json_bytes(
            {"format": "typebridge.sdk-conformance-report/v3", "binding": "node"}
        )
        + b"\n"
    )
    with pytest.raises(ASSEMBLER.AssemblyError) as raised:
        ASSEMBLER.assemble_report(
            _evidence(),
            binding="rust",
            run_nonce="a" * 64,
            phase4=_phase4(),
            phase4_sha256="7" * 64,
            generated_surface=b"rust-generated-surface",
            predecessor_reports=hostile,
            root=ROOT,
        )
    assert raised.value.code == "predecessor_report_identity_drift"


def test_c_predecessor_history_starts_at_v2() -> None:
    evidence = _evidence("c")
    predecessors = _predecessors("c")[1:]
    generated = b"c-generated-surface"
    phase4 = _phase4()
    phase4["artifacts"]["generated-package"]["sha256"] = hashlib.sha256(generated).hexdigest()
    report = ASSEMBLER.assemble_report(
        evidence,
        binding="c",
        run_nonce="a" * 64,
        phase4=phase4,
        phase4_sha256="7" * 64,
        generated_surface=generated,
        predecessor_reports=predecessors,
        root=ROOT,
    )
    assert [item["version"] for item in report["predecessor_reports"]] == [2, 3, 4, 5]


def test_publication_is_canonical_and_create_new(tmp_path: Path) -> None:
    path = tmp_path / "rust.json"
    report = _assemble()
    ASSEMBLER.publish(path, report)
    assert path.read_bytes() == ASSEMBLER.conformance.canonical_json_bytes(report)
    with pytest.raises(ASSEMBLER.AssemblyError) as raised:
        ASSEMBLER.publish(path, report)
    assert raised.value.code == "report_publication_failed"
