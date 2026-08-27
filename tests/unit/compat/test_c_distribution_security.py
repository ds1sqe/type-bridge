"""Fail-closed tests for C candidate supply-chain evidence."""

from __future__ import annotations

import copy
import importlib.util
import sys
import tomllib
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT_DIRECTORY = ROOT / "scripts/ci"
sys.path.insert(0, str(SCRIPT_DIRECTORY))
SPEC = importlib.util.spec_from_file_location(
    "c_distribution_security", SCRIPT_DIRECTORY / "c_distribution_security.py"
)
assert SPEC is not None and SPEC.loader is not None
SECURITY = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = SECURITY
SPEC.loader.exec_module(SECURITY)


def clean_audit_report() -> dict[str, object]:
    adjudication = SECURITY.AUDIT_ADJUDICATIONS[0]
    return {
        "database": {"last-commit": "1" * 40},
        "vulnerabilities": {"count": 0, "list": []},
        "warnings": {
            "unmaintained": [
                {
                    "advisory": {"id": adjudication["advisory"]},
                    "package": {
                        "name": adjudication["package"],
                        "version": adjudication["version"],
                    },
                }
            ]
        },
    }


def test_pruned_candidate_lock_contains_only_runtime_closure() -> None:
    metadata = SECURITY.cargo_metadata()
    closure = SECURITY.runtime_closure(metadata, ("type-bridge-cli", "type-bridge-c"))
    lock = tomllib.loads(SECURITY.pruned_lock_payload(metadata, closure).decode())
    packages = {(item["name"], item["version"]) for item in lock["package"]}

    assert ("type-bridge-cli", "2.1.0") in packages
    assert ("type-bridge-c", "2.1.0") in packages
    assert ("pyo3", "0.23.5") not in packages
    assert ("h2", "0.4.16") in packages
    assert ("rustls-webpki", "0.103.13") in packages


def test_exact_current_rustsec_report_and_adjudication_pass() -> None:
    report = clean_audit_report()

    assert SECURITY.validate_audit(report) == report
    assert SECURITY.AUDIT_ADJUDICATIONS[0]["advisory"] == "RUSTSEC-2025-0134"


def test_vulnerability_or_unadjudicated_warning_fails_closed() -> None:
    report = clean_audit_report()
    vulnerable = copy.deepcopy(report)
    vulnerable["vulnerabilities"] = {"count": 1, "list": [{"advisory": {}}]}
    with pytest.raises(SECURITY.SecurityError, match="vulnerabilities"):
        SECURITY.validate_audit(vulnerable)

    warning = copy.deepcopy(report)
    warning["warnings"]["unsound"] = [
        {
            "advisory": {"id": "RUSTSEC-2099-0001"},
            "package": {"name": "hostile", "version": "1.0.0"},
        }
    ]
    with pytest.raises(SECURITY.SecurityError, match="unadjudicated"):
        SECURITY.validate_audit(warning)


@pytest.mark.parametrize(
    "payload",
    [
        b"AKIAABCDEFGHIJKLMNOP",
        b"-----BEGIN PRIVATE KEY-----",
        str(ROOT).encode(),
    ],
)
def test_secret_and_source_path_scanner_rejects_hostile_payloads(payload: bytes) -> None:
    with pytest.raises(SECURITY.SecurityError):
        SECURITY.scan_payloads({"hostile": payload}, (str(ROOT).encode(),))


def test_signature_policy_is_candidate_only_and_identity_bound() -> None:
    policy = SECURITY.signature_policy()

    assert policy["candidate-signatures"] == []
    assert policy["publication-disposition"] == SECURITY.DISPOSITION
    assert policy["protected-release"]["issuer"] == ("https://token.actions.githubusercontent.com")
    assert "release\\.yml@refs/tags/v2\\.1\\.0" in policy["protected-release"]["identity-regexp"]

    hostile = copy.deepcopy(policy)
    hostile["protected-release"]["issuer"] = "https://hostile.example"
    with pytest.raises(SECURITY.SecurityError, match="signature identity"):
        SECURITY.validate_signature_document(hostile)


def test_individually_tampered_evidence_and_provenance_fail_closed(tmp_path: Path) -> None:
    for name in SECURITY.EVIDENCE_NAMES:
        if name != "candidate-manifest.json":
            (tmp_path / name).write_bytes(SECURITY.canonical_json({"name": name}))
    records = SECURITY.evidence_records(tmp_path)
    SECURITY.validate_evidence_records(records, tmp_path)

    (tmp_path / "runtime.spdx.json").write_bytes(b'{"tampered":true}\n')
    with pytest.raises(SECURITY.SecurityError, match="evidence digest"):
        SECURITY.validate_evidence_records(records, tmp_path)

    artifacts = [{"filename": "runtime.tar.gz", "sha256": "2" * 64}]
    accepted = SECURITY.provenance(artifacts, "3" * 40, "4" * 40)
    hostile_provenance = copy.deepcopy(accepted)
    hostile_provenance["source"]["commit"] = "5" * 40
    with pytest.raises(SECURITY.SecurityError, match="provenance"):
        SECURITY.validate_provenance_document(hostile_provenance, artifacts, "3" * 40, "4" * 40)
