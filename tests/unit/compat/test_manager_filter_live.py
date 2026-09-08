"""Provider-free tests for the focused Manager-filter comparator."""

from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
COMPARATOR_PATH = ROOT / "scripts/ci/compare_manager_filter_live.py"


def _load_comparator():
    spec = importlib.util.spec_from_file_location(
        "manager_filter_live_comparator",
        COMPARATOR_PATH,
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


comparator = _load_comparator()


def _write_reports(tmp_path: Path) -> list[Path]:
    contract = comparator.load_contract()
    tmp_path.mkdir(parents=True, exist_ok=True)
    paths = []
    for binding in reversed(comparator.REPORT_BINDINGS):
        path = tmp_path / f"{binding}.json"
        path.write_bytes(
            comparator.canonical_json_bytes(comparator.expected_report(binding, contract))
        )
        paths.append(path)
    return paths


def test_contract_is_exactly_sdk_v3_bound_without_promoting_catalog() -> None:
    contract = comparator.load_contract()

    assert comparator.REPORT_BINDINGS == ("python", "node", "rust", "c")
    assert comparator.SEMANTIC_PROFILE == "typedb-3.12.1/v1"
    assert comparator.REPORT_FORMAT == "typebridge.manager-filter-live-report/v1"
    assert comparator.SUMMARY_FORMAT == "typebridge.manager-filter-live-summary/v1"
    assert set(contract.authority) == {"schema", "journey", "provider"}
    for label, relative in comparator.SOURCE_PATHS.items():
        assert contract.authority[label] == {
            "path": relative,
            "sha256": comparator.SOURCE_SHA256[label],
        }
        assert (
            hashlib.sha256((ROOT / relative).read_bytes()).hexdigest()
            == (comparator.SOURCE_SHA256[label])
        )
    source = COMPARATOR_PATH.read_text(encoding="utf-8")
    assert "catalog-v3.json" not in source
    assert "sdk-conformance-report/v3" not in source


def test_contract_preserves_authored_order_and_only_normalizes_result_keys() -> None:
    observation = comparator.load_contract().observation

    assert observation["operator_outcomes"] == [
        {"operator": "eq", "normalized_keys": ["data-ada"]},
        {"operator": "ne", "normalized_keys": ["data-dana"]},
        {"operator": "gt", "normalized_keys": ["data-dana"]},
        {"operator": "gte", "normalized_keys": ["data-ada", "data-dana"]},
        {"operator": "lt", "normalized_keys": []},
        {"operator": "lte", "normalized_keys": ["data-ada"]},
    ]
    assert observation["conjunction"] == {
        "authored_order": ["foo__bar:gte:7", "score:gt:40"],
        "normalized_keys": ["data-dana"],
    }
    assert observation["terminals"] == {
        "all_normalization": "reference_key_ascending",
        "count": 2,
        "exists": True,
    }


def test_contract_freezes_strict_first_rejections_and_borrowed_reuse() -> None:
    observation = comparator.load_contract().observation

    assert observation["first"] == {
        "identity_predicate": "identifier:eq:data-ada",
        "strict_singular": True,
        "result": "data-ada",
        "nonsingular_rejection": {
            "category": "invalid_input",
            "code": "manager_first_requires_identity",
            "rejected_before_provider_io": True,
        },
    }
    assert observation["rejections"] == [
        {
            "kind": "wrong_field_owner",
            "category": "integrity",
            "code": "field_owner_mismatch",
            "rejected_before_provider_io": True,
        },
        {
            "kind": "wrong_scalar_domain",
            "category": "invalid_input",
            "code": "wrong_scalar_domain",
            "rejected_before_provider_io": True,
        },
        {
            "kind": "wrong_package",
            "category": "integrity",
            "code": "generated_token_package_mismatch",
            "rejected_before_provider_io": True,
        },
        {
            "kind": "boolean_ordering",
            "category": "invalid_input",
            "code": "invalid_operator_for_type",
            "rejected_before_provider_io": True,
        },
    ]
    assert observation["borrowed_read"] == {
        "reusable_after_each_terminal": True,
        "sibling_filter_usable": True,
        "final_state": "active",
    }


def test_four_exact_reports_compare_canonically(tmp_path: Path) -> None:
    paths = _write_reports(tmp_path)
    contract = comparator.load_contract()
    summary = comparator.compare_reports(paths)

    assert summary == {
        "authority": contract.authority,
        "bindings": ["python", "node", "rust", "c"],
        "format": "typebridge.manager-filter-live-summary/v1",
        "observation_sha256": hashlib.sha256(
            comparator.canonical_json_bytes(contract.observation)
        ).hexdigest(),
        "semantic_profile": "typedb-3.12.1/v1",
    }
    encoded = comparator.canonical_json_bytes(summary)
    assert (
        encoded
        == (
            json.dumps(
                summary,
                ensure_ascii=False,
                allow_nan=False,
                separators=(",", ":"),
                sort_keys=True,
            )
            + "\n"
        ).encode()
    )


@pytest.mark.parametrize(
    ("mutation", "code"),
    [
        (
            lambda report: report["observation"]["operator_outcomes"].reverse(),
            "manager_live_observation_mismatch",
        ),
        (
            lambda report: report["observation"]["conjunction"]["authored_order"].reverse(),
            "manager_live_observation_mismatch",
        ),
        (
            lambda report: report["observation"]["rejections"].pop(),
            "manager_live_observation_mismatch",
        ),
        (lambda report: report.update({"database": "secret"}), "invalid_object_fields"),
    ],
)
def test_report_drift_fails_closed(tmp_path: Path, mutation, code: str) -> None:
    paths = _write_reports(tmp_path)
    report = json.loads(paths[0].read_text(encoding="utf-8"))
    mutation(report)
    paths[0].write_bytes(comparator.canonical_json_bytes(report))

    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports(paths)
    assert rejected.value.code == code


def test_noncanonical_duplicate_and_foreign_reports_fail_closed(tmp_path: Path) -> None:
    paths = _write_reports(tmp_path)
    paths[0].write_bytes(json.dumps(json.loads(paths[0].read_text())).encode())
    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports(paths)
    assert rejected.value.code == "noncanonical_report_json"

    paths = _write_reports(tmp_path / "duplicates")
    duplicate = json.loads(paths[0].read_text(encoding="utf-8"))
    duplicate["binding"] = "python"
    paths[0].write_bytes(comparator.canonical_json_bytes(duplicate))
    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports(paths)
    assert rejected.value.code == "duplicate_binding_report"


def test_authority_drift_and_expectation_import_fail_closed(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    for relative in comparator.SOURCE_PATHS.values():
        destination = tmp_path / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes((ROOT / relative).read_bytes())
    provider = tmp_path / comparator.PROVIDER_RELATIVE
    provider.write_bytes(provider.read_bytes() + b"\n")
    monkeypatch.setattr(comparator, "ROOT", tmp_path)
    with pytest.raises(comparator.ContractError) as rejected:
        comparator.load_contract()
    assert rejected.value.code == "authority_hash_mismatch"

    for marker in comparator.FORBIDDEN_PRODUCER_IMPORT_MARKERS:
        with pytest.raises(comparator.ContractError) as rejected:
            comparator.validate_producer_source(f"producer uses {marker}")
        assert rejected.value.code == "expected_observation_import"


def test_source_identity_and_report_paths_reject_symlinks(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    for relative in comparator.SOURCE_PATHS.values():
        destination = tmp_path / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes((ROOT / relative).read_bytes())
    schema = tmp_path / comparator.SCHEMA_RELATIVE
    target = tmp_path / "schema-target.yaml"
    schema.rename(target)
    schema.symlink_to(target)
    monkeypatch.setattr(comparator, "ROOT", tmp_path)
    with pytest.raises(comparator.ContractError) as rejected:
        comparator.load_contract()
    assert rejected.value.code == "invalid_source_file"

    monkeypatch.setattr(comparator, "ROOT", ROOT)
    reports = _write_reports(tmp_path / "reports")
    target_report = reports[0]
    link = target_report.with_name("report-link.json")
    link.symlink_to(target_report)
    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports([link, *reports[1:]])
    assert rejected.value.code == "invalid_source_file"


def test_report_never_accepts_provider_local_identity_or_endpoint(tmp_path: Path) -> None:
    paths = _write_reports(tmp_path)
    report = copy.deepcopy(json.loads(paths[0].read_text(encoding="utf-8")))
    report["observation"]["first"]["result"] = "0x1234"
    paths[0].write_bytes(comparator.canonical_json_bytes(report))
    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports(paths)
    assert rejected.value.code == "forbidden_report_claim"
