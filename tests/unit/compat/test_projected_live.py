"""Fail-closed tests for the exact-TypeDB-3.12.1 Projected live comparator."""

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
COMPARATOR_PATH = ROOT / "scripts/ci/compare_projected_live.py"


def _load_comparator() -> ModuleType:
    spec = importlib.util.spec_from_file_location(
        "projected_live_comparator",
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


def _all_keys(value: Any) -> set[str]:
    if isinstance(value, dict):
        return set(value).union(*(_all_keys(item) for item in value.values()))
    if isinstance(value, list):
        return set().union(*(_all_keys(item) for item in value))
    return set()


def test_live_contract_is_exactly_source_bound_and_binding_neutral() -> None:
    contract = comparator.load_contract()

    assert comparator.REPORT_FORMAT == "typebridge.projected-live-report/v1"
    assert comparator.SUMMARY_FORMAT == "typebridge.projected-live-summary/v1"
    assert comparator.SEMANTIC_PROFILE == "typedb-3.12.1/v1"
    assert comparator.REPORT_BINDINGS == ("python", "node", "rust", "c")
    assert comparator.REPORT_OUTPUT_ENV == "TYPE_BRIDGE_PROJECTED_LIVE_REPORT"
    assert comparator.REPORT_WRITE_SEMANTICS == "create_new"
    assert tuple(sorted(contract.observations)) == tuple(sorted(comparator.OBSERVATION_REFS))
    assert tuple(sorted(contract.observations)) == (
        "canonical_scalar_values",
        "cleanup",
        "inherited_plain_activity_role_lifecycle",
        "integer_key_polymorphic_optional_role",
        "relation_as_player",
    )

    assert set(contract.authority) == {"schema", "journey", "provider"}
    for label, relative in comparator.SOURCE_PATHS.items():
        assert contract.authority[label] == {
            "path": relative,
            "sha256": comparator.SOURCE_SHA256[label],
        }
        assert (
            comparator.SOURCE_SHA256[label]
            == hashlib.sha256((ROOT / relative).read_bytes()).hexdigest()
        )


def test_authority_byte_drift_fails_closed(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
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


def test_live_contract_derives_only_the_engine_supported_observations() -> None:
    observations = comparator.load_contract().observations

    assert observations["canonical_scalar_values"] == {
        "hydrated": {
            "boolean": {"kind": "boolean", "value": False},
            "date": {"kind": "date", "value": "2026-08-12"},
            "datetime": {"kind": "datetime", "value": "2026-08-12T09:30:00"},
            "datetime_tz": {
                "kind": "datetime_tz",
                "value": "2026-08-12T09:30:00Z",
            },
            "decimal": {"kind": "decimal", "value": "38.5"},
            "double": {"bits": "4043000000000000", "kind": "double"},
            "duration": {"kind": "duration", "value": "PT38S"},
            "long": {"kind": "long", "value": "38"},
            "string": {"kind": "string", "value": "Ada"},
        },
        "model": "person",
        "ref": "data-ada",
    }
    assert observations["integer_key_polymorphic_optional_role"] == {
        "integer_keys": [
            {"model": "robot", "ref": "robot-negative-7", "value": "-7"},
            {"model": "robot", "ref": "robot-7", "value": "7"},
        ],
        "optional_role": "actor",
        "relation": "interaction",
        "states": [
            {"actor": None, "relation_ref": "interaction-absent"},
            {
                "actor": {"key": "data-ada", "model": "person"},
                "relation_ref": "interaction-person",
            },
            {
                "actor": {"key": "7", "model": "robot"},
                "relation_ref": "interaction-robot",
            },
        ],
    }
    assert observations["inherited_plain_activity_role_lifecycle"] == {
        "count_after_delete": 0,
        "created": True,
        "deleted": True,
        "inherited_relation": "base-activity",
        "model": "plain-activity",
        "participant": {"key": "data-ada", "model": "person"},
        "read_after_create": True,
        "read_after_delete": False,
        "ref": "plain-activity-ada",
        "role": "participant",
        "role_identity_preserved": True,
    }
    assert observations["relation_as_player"] == {
        "owner": {"model": "container", "ref": "container-event"},
        "player": {"model": "event", "ref": "event-ada"},
        "preserved": True,
        "role": "item",
    }
    assert observations["cleanup"] == {
        "order": [
            "container-event",
            "event-ada",
            "plain-activity-ada",
            "interaction-person",
            "interaction-absent",
            "interaction-robot",
            "robot-negative-7",
            "robot-7",
            "data-dana",
            "data-ada",
        ],
        "zero_checks": [
            {"count_after_cleanup": 0, "model": "container"},
            {"count_after_cleanup": 0, "model": "event"},
            {"count_after_cleanup": 0, "model": "interaction"},
            {"count_after_cleanup": 0, "model": "person"},
            {"count_after_cleanup": 0, "model": "plain-activity"},
            {"count_after_cleanup": 0, "model": "robot"},
        ],
    }
    assert _all_keys(observations).isdisjoint(comparator.FORBIDDEN_REPORT_KEYS)
    assert "aliases" not in _all_keys(observations)


def test_reports_and_summary_have_exact_canonical_bytes(tmp_path: Path) -> None:
    paths = _write_reports(tmp_path)
    contract = comparator.load_contract()
    for path in paths:
        value = _read_report(path)
        assert path.read_bytes() == comparator.canonical_json_bytes(value)
        assert path.read_bytes().endswith(b"\n")
        assert not path.read_bytes().endswith(b"\n\n")
        assert value == comparator.expected_report(value["binding"], contract)

    summary = comparator.compare_reports(paths)
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
    assert summary == {
        "authority": contract.authority,
        "bindings": ["python", "node", "rust", "c"],
        "format": "typebridge.projected-live-summary/v1",
        "observation_sha256": hashlib.sha256(
            comparator.canonical_json_bytes(contract.observations)
        ).hexdigest(),
        "semantic_profile": "typedb-3.12.1/v1",
    }


def test_four_exact_reports_compare_in_any_order(tmp_path: Path) -> None:
    paths = _write_reports(tmp_path)

    first = comparator.compare_reports(paths)
    second = comparator.compare_reports(list(reversed(paths)))

    assert first == second
    assert first["bindings"] == list(comparator.REPORT_BINDINGS)


@pytest.mark.parametrize(
    ("mutation", "code"),
    [
        ("observation", "projected_live_observation_mismatch"),
        ("authority", "projected_live_observation_mismatch"),
        ("format", "projected_live_observation_mismatch"),
        ("profile", "projected_live_observation_mismatch"),
        ("extra_field", "invalid_object_fields"),
        ("noncanonical", "noncanonical_report_json"),
    ],
)
def test_mutated_reports_fail_closed(
    tmp_path: Path,
    mutation: str,
    code: str,
) -> None:
    paths = _write_reports(tmp_path)
    report = _read_report(paths[0])
    if mutation == "observation":
        report["observations"]["canonical_scalar_values"]["hydrated"]["long"]["value"] = "39"
    elif mutation == "authority":
        report["authority"]["provider"]["sha256"] = "0" * 64
    elif mutation == "format":
        report["format"] = "typebridge.projected-live-report/v2"
    elif mutation == "profile":
        report["semantic_profile"] = "typedb-3.12.2/v1"
    elif mutation == "extra_field":
        report["runtime_identity"] = "forbidden"
    elif mutation == "noncanonical":
        paths[0].write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    else:
        raise AssertionError(f"unknown mutation {mutation}")
    if mutation != "noncanonical":
        paths[0].write_bytes(comparator.canonical_json_bytes(report))

    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports(paths)
    assert rejected.value.code == code


def test_duplicate_missing_and_unknown_bindings_fail_closed(tmp_path: Path) -> None:
    paths = _write_reports(tmp_path)
    duplicate = _read_report(paths[0])
    duplicate["binding"] = _read_report(paths[1])["binding"]
    paths[0].write_bytes(comparator.canonical_json_bytes(duplicate))
    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports(paths)
    assert rejected.value.code == "duplicate_binding_report"

    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports(paths[1:])
    assert rejected.value.code == "report_count_mismatch"

    paths = _write_reports(tmp_path / "unknown")
    unknown = _read_report(paths[0])
    unknown["binding"] = "haskell"
    paths[0].write_bytes(comparator.canonical_json_bytes(unknown))
    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports(paths)
    assert rejected.value.code == "unknown_binding"


def test_duplicate_json_keys_fail_closed(tmp_path: Path) -> None:
    paths = _write_reports(tmp_path)
    paths[0].write_bytes(b'{"binding":"c","binding":"c"}\n')

    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports(paths)
    assert rejected.value.code == "duplicate_json_key"


def test_symlink_and_nonregular_reports_fail_closed(tmp_path: Path) -> None:
    paths = _write_reports(tmp_path)
    link = tmp_path / "linked.json"
    link.symlink_to(paths[0])
    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports([link, *paths[1:]])
    assert rejected.value.code == "invalid_source_file"

    directory = tmp_path / "directory-report"
    directory.mkdir()
    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports([directory, *paths[1:]])
    assert rejected.value.code == "invalid_source_file"


@pytest.mark.parametrize(
    ("payload", "code"),
    [
        (b"{", "malformed_json"),
        (b"\xff", "invalid_json_utf8"),
        (b" " * (comparator.MAX_REPORT_BYTES + 1), "source_size_limit"),
    ],
)
def test_malformed_non_utf8_and_oversized_reports_fail_closed(
    tmp_path: Path,
    payload: bytes,
    code: str,
) -> None:
    paths = _write_reports(tmp_path)
    paths[0].write_bytes(payload)

    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports(paths)
    assert rejected.value.code == code


@pytest.mark.parametrize(
    ("claim", "value"),
    [
        ("aliases", []),
        ("ordered_list_persistence", True),
        ("database_name", "projected-live"),
        ("endpoint", "typedb://localhost:1729"),
        ("iid", "0x0123abcd"),
        ("batch", {"inserted": 1}),
        ("filter", {"count": 1}),
    ],
)
def test_forbidden_live_claims_fail_closed(
    tmp_path: Path,
    claim: str,
    value: Any,
) -> None:
    paths = _write_reports(tmp_path)
    report = _read_report(paths[0])
    report["observations"]["canonical_scalar_values"][claim] = value
    paths[0].write_bytes(comparator.canonical_json_bytes(report))

    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports(paths)
    assert rejected.value.code == "forbidden_report_claim"


def test_provider_local_identifier_values_fail_closed(tmp_path: Path) -> None:
    paths = _write_reports(tmp_path)
    report = _read_report(paths[0])
    report["observations"]["canonical_scalar_values"]["runtime_ref"] = "0x0123abcd"
    paths[0].write_bytes(comparator.canonical_json_bytes(report))

    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports(paths)
    assert rejected.value.code == "forbidden_report_claim"


@pytest.mark.parametrize("marker", comparator.FORBIDDEN_PRODUCER_IMPORT_MARKERS)
def test_producer_expected_observation_imports_are_forbidden(marker: str) -> None:
    source = f"producer_observation = {marker!r}\n"

    with pytest.raises(comparator.ContractError) as rejected:
        comparator.validate_producer_source(source)
    assert rejected.value.code == "expected_observation_import"

    comparator.validate_producer_source(
        "producer_observation = hydrate_generated_value_from_live_database()\n"
    )


def test_expected_report_returns_an_independent_value() -> None:
    contract = comparator.load_contract()
    first = comparator.expected_report("python", contract)
    second = comparator.expected_report("python", contract)

    mutated = copy.deepcopy(first)
    mutated["observations"]["cleanup"]["order"].clear()

    assert second == comparator.expected_report("python", contract)
    assert mutated != second


def test_unknown_expected_report_binding_is_rejected() -> None:
    with pytest.raises(comparator.ContractError) as rejected:
        comparator.expected_report("haskell", comparator.load_contract())
    assert rejected.value.code == "unknown_binding"
