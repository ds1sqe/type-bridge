"""Fail-closed tests for the Workforce V4 comparator."""

from __future__ import annotations

import importlib.util
import json
import shutil
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "scripts/ci/compare_workforce_conformance_v4.py"
SPEC = importlib.util.spec_from_file_location("workforce_v4_comparator", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
comparator = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = comparator
SPEC.loader.exec_module(comparator)


def _stage_contracts(destination: Path) -> Path:
    source = ROOT / "tests/contracts/sdk_conformance"
    target = destination / "tests/contracts/sdk_conformance"
    (target / "workforce-v4").mkdir(parents=True)
    shutil.copy2(source / "manifest-v1.json", target / "manifest-v1.json")
    for name in ["catalog-v4.json", "journey-v4.json", "report-schema-v4.json"]:
        shutil.copy2(source / "workforce-v4" / name, target / "workforce-v4" / name)
    return destination


def test_loads_exact_phase0_contract() -> None:
    contracts = comparator.load_contracts(ROOT)

    assert contracts.selected_cases == comparator.EXPECTED_TRANSITIONS + (
        comparator.EXPECTED_RETAINED_GAPS
    )
    assert len(contracts.observation_refs) == 8


def test_report_fan_in_rejects_unfinalized_authority() -> None:
    with pytest.raises(comparator.ContractError) as raised:
        comparator.compare_reports([], ROOT)
    assert raised.value.code == "unfinalized_v4_authority"


@pytest.mark.parametrize(
    ("field", "replacement", "code"),
    [
        ("manifest_transition_cases", ["workforce.runtime.cancellation"], "transition_scope_drift"),
        ("evidence_only_gap_cases", [], "retained_gap_scope_drift"),
        ("report_bindings", ["python", "node", "rust"], "binding_scope_drift"),
    ],
)
def test_rejects_catalog_scope_drift(
    tmp_path: Path, field: str, replacement: object, code: str
) -> None:
    root = _stage_contracts(tmp_path)
    path = root / comparator.CATALOG_RELATIVE
    catalog = json.loads(path.read_text(encoding="utf-8"))
    catalog[field] = replacement
    path.write_text(json.dumps(catalog), encoding="utf-8")

    with pytest.raises(comparator.ContractError) as raised:
        comparator.load_contracts(root)
    assert raised.value.code == code


def test_rejects_manifest_case_rebinding(tmp_path: Path) -> None:
    root = _stage_contracts(tmp_path)
    path = root / comparator.CATALOG_RELATIVE
    catalog = json.loads(path.read_text(encoding="utf-8"))
    catalog["cases"][0]["capability_id"] = "runtime.cancellation"
    path.write_text(json.dumps(catalog), encoding="utf-8")

    with pytest.raises(comparator.ContractError) as raised:
        comparator.load_contracts(root)
    assert raised.value.code == "manifest_case_drift"
