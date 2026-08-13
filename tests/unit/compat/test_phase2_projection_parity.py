"""Fail-closed tests for the provider-free Phase-2 projection parity gate."""

from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
import sys
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[3]
COMPARATOR_PATH = ROOT / "scripts/ci/compare_phase2_projection_parity.py"


def _load_comparator() -> ModuleType:
    spec = importlib.util.spec_from_file_location(
        "phase2_projection_parity_comparator",
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


def _read_report(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    assert isinstance(value, dict)
    return value


def _object_keys(value: Any) -> set[str]:
    if isinstance(value, dict):
        return set(value).union(*(_object_keys(item) for item in value.values()))
    if isinstance(value, list):
        return set().union(*(_object_keys(item) for item in value))
    return set()


def test_phase2_contract_selects_only_the_provider_free_projection_slice() -> None:
    contract = comparator.load_contract()

    assert tuple(sorted(contract.observations)) == tuple(sorted(comparator.OBSERVATION_REFS))
    assert contract.observations["projected_constraint_validation"]["scalar_domains"] == [
        "boolean",
        "date",
        "datetime",
        "datetime_tz",
        "decimal",
        "double",
        "duration",
        "long",
        "string",
    ]
    scalar_values = contract.observations["canonical_scalar_values"]
    assert scalar_values["authored"] == scalar_values["constructed"]
    assert scalar_values["constructed"] == scalar_values["hydrated"]
    assert scalar_values["authored"] == {
        "boolean": {"kind": "boolean", "value": False},
        "date": {"kind": "date", "value": "2026-08-12"},
        "datetime": {"kind": "datetime", "value": "2026-08-12T09:30:00"},
        "datetime_tz": {"kind": "datetime_tz", "value": "2026-08-12T09:30:00Z"},
        "decimal": {"kind": "decimal", "value": "38.5"},
        "double": {"bits": "4043000000000000", "kind": "double"},
        "duration": {"kind": "duration", "value": "PT38S"},
        "long": {"kind": "long", "value": "38"},
        "string": {"kind": "string", "value": "Ada"},
    }
    assert contract.observations["inherited_relation_role"] == {
        "model": "plain-activity",
        "inherited_relation": "base-activity",
        "inherited_role": "participant",
        "player_model": "person",
        "constructed": True,
        "hydrated": True,
        "role_identity_preserved": True,
    }
    fencing = contract.observations["token_package_fencing"]
    assert set(fencing["accepted_local"]) == {"construction", "hydration"}
    assert set(fencing["foreign_rejections"]) == {"construction", "hydration"}
    assert "batch" not in fencing["accepted_local"]
    assert "filter" not in fencing["accepted_local"]
    assert _object_keys(contract.observations).isdisjoint(
        {
            "batch",
            "compatibility_lookup",
            "filter",
            "generated_token_string_parser_used",
            "operator",
            "operator_suffix",
        }
    )

    for label, relative in (
        ("schema", comparator.SCHEMA_RELATIVE),
        ("journey", comparator.JOURNEY_RELATIVE),
    ):
        assert contract.authority[label] == {
            "path": relative,
            "sha256": hashlib.sha256((ROOT / relative).read_bytes()).hexdigest(),
        }


def test_four_exact_reports_compare_in_any_order(tmp_path: Path) -> None:
    summary = comparator.compare_reports(_write_reports(tmp_path))

    assert summary["format"] == comparator.SUMMARY_FORMAT
    assert summary["semantic_profile"] == comparator.SEMANTIC_PROFILE
    assert summary["bindings"] == list(comparator.REPORT_BINDINGS)
    assert summary["authority"] == comparator.load_contract().authority
    assert len(summary["observation_sha256"]) == 64


@pytest.mark.parametrize(
    ("mutation", "code"),
    [
        ("observation", "phase2_observation_mismatch"),
        ("authority", "phase2_observation_mismatch"),
        ("extra_field", "invalid_object_fields"),
        ("duplicate_binding", "duplicate_binding_report"),
        ("noncanonical", "noncanonical_report_json"),
    ],
)
def test_report_mutations_fail_closed(
    tmp_path: Path,
    mutation: str,
    code: str,
) -> None:
    paths = _write_reports(tmp_path)
    report = _read_report(paths[0])
    if mutation == "observation":
        report["observations"]["ordered_distinct_collections"]["owns"]["hydrated"].reverse()
    elif mutation == "authority":
        report["authority"]["schema"]["sha256"] = "0" * 64
    elif mutation == "extra_field":
        report["runtime_identity"] = "forbidden"
    elif mutation == "duplicate_binding":
        report["binding"] = _read_report(paths[1])["binding"]
    elif mutation == "noncanonical":
        paths[0].write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    else:
        raise AssertionError(f"unknown mutation {mutation}")
    if mutation != "noncanonical":
        paths[0].write_bytes(comparator.canonical_json_bytes(report))

    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports(paths)
    assert rejected.value.code == code


def test_duplicate_json_keys_and_symlink_reports_are_rejected(tmp_path: Path) -> None:
    paths = _write_reports(tmp_path)
    duplicate = paths[0]
    duplicate.write_bytes(b'{"binding":"c","binding":"c"}\n')
    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports(paths)
    assert rejected.value.code == "duplicate_json_key"

    paths = _write_reports(tmp_path / "second")
    target = paths[0]
    link = target.with_name("linked.json")
    link.symlink_to(target)
    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports([link, *paths[1:]])
    assert rejected.value.code == "invalid_source_file"


@pytest.mark.parametrize(
    ("payload", "code"),
    [
        (b"{", "malformed_json"),
        (b"\xff", "invalid_json_utf8"),
        (b" " * (comparator.MAX_REPORT_BYTES + 1), "source_size_limit"),
    ],
)
def test_malformed_bounded_report_inputs_are_rejected(
    tmp_path: Path,
    payload: bytes,
    code: str,
) -> None:
    paths = _write_reports(tmp_path)
    paths[0].write_bytes(payload)

    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports(paths)
    assert rejected.value.code == code


def test_unknown_and_missing_binding_reports_are_rejected(tmp_path: Path) -> None:
    paths = _write_reports(tmp_path)
    unknown = _read_report(paths[0])
    unknown["binding"] = "haskell"
    paths[0].write_bytes(comparator.canonical_json_bytes(unknown))
    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports(paths)
    assert rejected.value.code == "unknown_binding"

    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports(paths[1:])
    assert rejected.value.code == "report_count_mismatch"


def test_expected_report_returns_an_independent_value() -> None:
    contract = comparator.load_contract()
    first = comparator.expected_report("python", contract)
    second = comparator.expected_report("python", contract)

    mutated = copy.deepcopy(first)
    mutated["observations"]["integer_key_polymorphic_role"]["integer_keys"].clear()
    assert second == comparator.expected_report("python", contract)
    assert mutated != second
