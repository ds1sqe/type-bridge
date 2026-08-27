"""Fail-closed checks for the frozen Plan 08 distribution and V6 authority."""

from __future__ import annotations

import copy
import importlib.util
import json
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "scripts/ci/validate_c_distribution_contract.py"
SPEC = importlib.util.spec_from_file_location("validate_c_distribution_contract", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
VALIDATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VALIDATOR)


def _contract() -> dict[str, Any]:
    return json.loads((ROOT / "tests/contracts/c-distribution-v1.json").read_text())


def _write(tmp_path: Path, value: dict[str, Any]) -> Path:
    path = tmp_path / "contract.json"
    path.write_text(json.dumps(value), encoding="utf-8")
    return path


def test_distribution_contract_matches_every_live_source_authority() -> None:
    contract = VALIDATOR.validate()
    assert contract["abi"]["decision"] == "no-change"
    assert contract["abi"]["version"] == "1.6.0"
    assert contract["matrix"]["public_supported"] == []
    assert contract["publication_disposition"] == "candidate-only-unpublished-unsupported"


@pytest.mark.parametrize(
    ("path", "value"),
    [
        (("release_gate", "publication_authorized"), True),
        (("abi", "decision"), "abi-1.7"),
        (("abi", "static_linkage"), "candidate"),
        (("compatibility", "provider_io_on_failure"), True),
        (("artifact_policy", "symlinks"), True),
        (("artifact_policy", "debug_symbols"), "retained"),
        (("matrix", "public_supported"), ["x86_64-unknown-linux-gnu"]),
    ],
)
def test_distribution_contract_rejects_authority_widening(
    tmp_path: Path, path: tuple[str, str], value: object
) -> None:
    contract = copy.deepcopy(_contract())
    contract[path[0]][path[1]] = value
    with pytest.raises(VALIDATOR.ContractError):
        VALIDATOR.validate(_write(tmp_path, contract))


def test_distribution_contract_rejects_stale_source_digest(tmp_path: Path) -> None:
    contract = copy.deepcopy(_contract())
    contract["source_authorities"][0]["sha256"] = "0" * 64
    with pytest.raises(VALIDATOR.ContractError, match="stale source authority"):
        VALIDATOR.validate(_write(tmp_path, contract))


def test_v6_freezes_only_the_five_terminal_transition_cases() -> None:
    catalog = json.loads(
        (ROOT / "tests/contracts/sdk_conformance/workforce-v6/catalog-v6.json").read_text()
    )
    assert catalog["authority_state"] == "frozen"
    assert catalog["manifest_transition_cases"] == VALIDATOR.TRANSITION_CASES
    assert catalog["full_c"] == {
        "capability_count": 44,
        "required_predecessor_reports": [1, 2, 3, 4, 5, 6],
        "manual_badge": False,
        "fails_on_gap_or_planned_c_cell": True,
        "publication_authority": False,
    }


def test_full_c_audit_inventory_is_exactly_the_manifest() -> None:
    audit = json.loads((ROOT / "tests/contracts/c-full-sdk-audit-v1.json").read_text())
    manifest = json.loads((ROOT / "tests/contracts/sdk_conformance/manifest-v1.json").read_text())
    assert audit["capability_codes"] == [item["code"] for item in manifest["capabilities"]]
    assert len(audit["capability_codes"]) == 44
    assert audit["report_versions"] == [1, 2, 3, 4, 5, 6]
    assert audit["output"]["manual_override"] is False
    assert audit["output"]["publication_authority"] is False
