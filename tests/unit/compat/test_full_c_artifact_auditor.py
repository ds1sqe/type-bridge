"""Fail-closed unit tests for the independent FULL-C artifact auditor."""

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
    "full_c_artifact_auditor", CI / "audit_full_c_artifact.py"
)
assert SPEC is not None and SPEC.loader is not None
AUDITOR = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = AUDITOR
SPEC.loader.exec_module(AUDITOR)


def _manifest() -> dict[str, object]:
    return json.loads((ROOT / "tests/contracts/sdk_conformance/manifest.json").read_text())


def _contract() -> dict[str, object]:
    return json.loads((ROOT / "tests/contracts/c-full-sdk-audit.json").read_text())


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
    capabilities = manifest["capabilities"]
    assert isinstance(capabilities, list)
    capabilities[0]["binding_profile"] = "current_live_future_planned"
    with pytest.raises(AUDITOR.AuditError) as raised:
        AUDITOR.validate_capabilities(manifest, _contract())
    assert raised.value.code == "gap_or_planned_cell"


def test_documentation_fences_require_public_support_boundary() -> None:
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


def test_audit_reports_supported_matrix_without_obsolete_gate(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # External producer validation is covered by its own hostile-evidence tests.
    monkeypatch.setattr(AUDITOR.artifact_journey, "validate_acceptance_report", lambda *args: None)
    monkeypatch.setattr(AUDITOR.security, "validate_evidence", lambda *args: None)
    source = "a" * 40
    acceptance = tmp_path / "acceptance.json"
    acceptance.write_bytes(AUDITOR.artifact_journey.packages.canonical_json({"artifacts": []}))
    artifact = tmp_path / "artifact.bin"
    artifact.write_bytes(b"fixture")
    security = tmp_path / "security"
    security.mkdir()
    (security / "artifact-manifest.json").write_bytes(
        AUDITOR.canonical_json(
            {
                "source-commit": source,
                "artifact-set-id": "sha256:" + "b" * 64,
            }
        )
    )
    reports = tmp_path / "reports"
    for suite in AUDITOR.COMPARATORS:
        directory = reports / suite
        directory.mkdir(parents=True)
        for binding in AUDITOR.BINDINGS:
            (directory / f"{binding}.json").write_bytes(
                AUDITOR.canonical_json({"source_commit": source})
            )
        monkeypatch.setitem(
            AUDITOR.COMPARATORS,
            suite,
            lambda paths: {
                "format": "fixture",
                "authority_state": "finalized",
                "pending_promotions": [],
            },
        )
    result, summary = AUDITOR.audit(
        reports_root=reports,
        acceptance_path=acceptance,
        provider=artifact,
        live=artifact,
        cli=artifact,
        runtime=artifact,
        generated=artifact,
        security_evidence=security,
    )
    assert result["matrix"]["public_supported"] == ["x86_64-unknown-linux-gnu"]
    assert result["matrix"]["artifact_supported"][0]["runner"] == "ubuntu-24.04"
    assert result["publication_authority"] is False
    assert result["release_gate"]["state"] == "closed"
    assert "Ubuntu 24.04 x86_64" in summary
    assert "#189" not in summary
