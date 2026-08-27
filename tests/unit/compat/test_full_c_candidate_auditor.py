"""Fail-closed unit tests for the independent FULL-C candidate auditor."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
CI = ROOT / "scripts/ci"
sys.path.insert(0, str(CI))
SPEC = importlib.util.spec_from_file_location(
    "full_c_candidate_auditor", CI / "audit_full_c_candidate.py"
)
assert SPEC is not None and SPEC.loader is not None
AUDITOR = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = AUDITOR
SPEC.loader.exec_module(AUDITOR)


def _manifest() -> dict[str, object]:
    return json.loads((ROOT / "tests/contracts/sdk_conformance/manifest-v1.json").read_text())


def _contract() -> dict[str, object]:
    return json.loads((ROOT / "tests/contracts/c-full-sdk-audit-v1.json").read_text())


def test_all_44_current_and_c_cells_are_accepted() -> None:
    rows = AUDITOR.validate_capabilities(_manifest(), _contract())
    assert len(rows) == 44
    assert [row["code"] for row in rows] == _contract()["capability_codes"]
    assert all(
        row[binding] in {"accepted_offline", "accepted_live"}
        for row in rows
        for binding in AUDITOR.BINDINGS
    )


def test_gap_or_planned_c_cell_fails_closed() -> None:
    manifest = _manifest()
    manifest["capabilities"][0]["binding_profile"] = "current_live_future_planned"
    with pytest.raises(AUDITOR.AuditError) as raised:
        AUDITOR.validate_capabilities(manifest, _contract())
    assert raised.value.code == "gap_or_planned_cell"


def test_documentation_fences_remain_candidate_only() -> None:
    rows = AUDITOR.validate_documentation()
    assert [row["path"] for row in rows] == list(AUDITOR.DOCUMENTATION_FENCES)


def test_audit_pair_publication_is_atomic_and_create_new(tmp_path: Path) -> None:
    report_path = tmp_path / "full-c.json"
    summary_path = tmp_path / "full-c.md"
    report = {"format": "typebridge.c-full-sdk-report/v1", "publication_authority": False}
    AUDITOR.publish(report_path, summary_path, report, "# accepted\n")
    assert report_path.read_bytes() == AUDITOR.canonical_json(report)
    assert summary_path.read_text() == "# accepted\n"
    with pytest.raises(AUDITOR.AuditError) as raised:
        AUDITOR.publish(report_path, tmp_path / "other.md", report, "# accepted\n")
    assert raised.value.code == "invalid_output_path"
