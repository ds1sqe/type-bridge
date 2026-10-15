"""TypeDB version matrix of the SDK conformance suites."""

from __future__ import annotations

import copy
import importlib.util
import json
import shlex
import subprocess
import sys
from pathlib import Path
from types import ModuleType
from typing import Any
from unittest.mock import Mock

import pytest

ROOT = Path(__file__).resolve().parents[3]
CI = ROOT / "scripts/ci"
sys.path.insert(0, str(CI))

BASE = "typedb-3.12.1/v1"
NEXT = "typedb-3.13.6/v1"
SERVERS = {BASE: "3.12.3", NEXT: "3.13.6"}


def _load(name: str) -> ModuleType:
    spec = importlib.util.spec_from_file_location(f"profile_matrix_test_{name}", CI / f"{name}.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


matrix = _load("sdk_profile_matrix")
v2 = _load("sdk_queries_compare")
runner = _load("run_sdk_conformance")
fragments = _load("proof_fragments")


@pytest.mark.parametrize("suite", matrix.SUITES)
def test_every_set_lists_both_profiles_and_its_catalog_entry_is_unchanged(suite: str) -> None:
    catalog = json.loads((ROOT / matrix.catalog_relative(suite)).read_text(encoding="utf-8"))
    entries = matrix.load_matrix(suite)

    assert set(entries) == {BASE, NEXT}
    for profile, entry in entries.items():
        assert entry.live_server_version == SERVERS[profile]
        assert ("expected_fingerprints" in catalog) == (entry.expected_fingerprints is not None)
    own = entries[catalog["fixture"]["semantic_profile"]]
    if "live_server_version" in catalog["fixture"]:
        assert own.live_server_version == catalog["fixture"]["live_server_version"]
    if "expected_fingerprints" in catalog:
        assert own.expected_fingerprints == catalog["expected_fingerprints"]
        other = entries[NEXT].expected_fingerprints
        assert other is not None
        assert other["semantic"]["digest"] != own.expected_fingerprints["semantic"]["digest"]


def _reports(module: ModuleType, contracts: Any, profile: str) -> dict[str, dict[str, Any]]:
    version = int(contracts.catalog.value["fixture"]["version"])
    fingerprints = matrix.select(
        contracts.catalog.value["fixture"]["id"], profile
    ).expected_fingerprints
    reports = {}
    for binding in module.REPORT_BINDINGS:
        results = [
            {
                "capability_id": contracts.cases[case_id]["capability"]["id"],
                "case_id": case_id,
                "observation": copy.deepcopy(contracts.observations[observation_ref]),
                "outcome": "passed",
                "proof_kind": proof_kind,
            }
            for case_id, proof_kind, observation_ref in contracts.selected
        ]
        reports[binding] = {
            "binding": binding,
            "catalog": {"path": module.CATALOG_RELATIVE, "sha256": contracts.catalog.sha256},
            "fixture": {
                "id": contracts.catalog.value["fixture"]["id"],
                "journey": {"path": module.JOURNEY_RELATIVE, "sha256": contracts.journey.sha256},
                "projection_fingerprint": fingerprints["projections"][binding],
                "projection_target": contracts.projection_targets[binding],
                "provider_schema": {
                    "path": module.PROVIDER_SCHEMA_RELATIVE,
                    "sha256": contracts.provider_schema.sha256,
                },
                "schema": {"path": module.SCHEMA_RELATIVE, "sha256": contracts.schema.sha256},
                "semantic_fingerprint": fingerprints["semantic"],
                "semantic_profile": profile,
                "version": version,
            },
            "format": module.REPORT_FORMAT,
            "manifest": {"path": module.MANIFEST_RELATIVE, "sha256": contracts.manifest.sha256},
            "results": results,
        }
    return reports


def _write(module: ModuleType, directory: Path, reports: dict[str, dict[str, Any]]) -> list[Path]:
    directory.mkdir(parents=True, exist_ok=True)
    paths = []
    for binding, report in reports.items():
        path = directory / f"{binding}.json"
        path.write_bytes(module.canonical_json_bytes(report))
        paths.append(path)
    return paths


@pytest.mark.parametrize("profile", [BASE, NEXT])
def test_queries_report_set_compares_against_its_profile_entry(
    tmp_path: Path, profile: str
) -> None:
    reports = _reports(v2, v2.load_contracts(), profile)

    summary = v2.compare_reports(_write(v2, tmp_path, reports))

    assert summary["fixture"]["semantic_profile"] == profile
    expected = matrix.select("queries", profile).expected_fingerprints
    assert summary["fixture"]["semantic_fingerprint"] == expected["semantic"]


def test_v2_report_set_on_3_13_6_compares_against_its_entry(tmp_path: Path) -> None:
    reports = _reports(v2, v2.load_contracts(), NEXT)

    summary = v2.compare_reports(_write(v2, tmp_path, reports))

    assert summary["fixture"]["semantic_profile"] == NEXT


def test_mixed_profile_report_set_is_refused(tmp_path: Path) -> None:
    contracts = v2.load_contracts()
    reports = _reports(v2, contracts, NEXT)
    reports["rust"] = _reports(v2, contracts, BASE)["rust"]

    with pytest.raises(v2.ContractError) as rejected:
        v2.compare_reports(_write(v2, tmp_path, reports))
    assert rejected.value.code == "mixed_semantic_profile"


def test_unsupported_profile_and_cross_profile_fingerprints_are_refused(tmp_path: Path) -> None:
    contracts = v2.load_contracts()
    unsupported = _reports(v2, contracts, NEXT)
    for report in unsupported.values():
        report["fixture"]["semantic_profile"] = "typedb-3.11.5/v1"
    with pytest.raises(v2.ContractError) as rejected:
        v2.compare_reports(_write(v2, tmp_path / "unsupported", unsupported))
    assert rejected.value.code == "semantic_profile_mismatch"

    # 3.12.1 fingerprints relabelled as 3.13.6 do not match the 3.13.6 entry.
    relabelled = _reports(v2, contracts, BASE)
    for report in relabelled.values():
        report["fixture"]["semantic_profile"] = NEXT
    with pytest.raises(v2.ContractError) as rejected:
        v2.compare_reports(_write(v2, tmp_path / "relabelled", relabelled))
    assert rejected.value.code == "semantic_profile_mismatch"


def test_server_versions_must_be_matrix_servers_and_shared() -> None:
    v4 = _load("sdk_administration_compare")
    v5 = _load("sdk_serialization_compare")
    assert v4._matrix_servers() == v5.matrix_servers() == set(SERVERS.values())
    with pytest.raises(v4.ContractError) as rejected:
        matrix.shared_value(["3.12.3", "3.13.6"], "server version", error=v4.ContractError)
    assert rejected.value.code == "mixed_server_version"
    assert matrix.select_server("administration", "3.13.6").semantic_profile == NEXT


def test_runner_selects_the_profile_and_exact_server() -> None:
    assert runner.lane(None, {}).live_server_version == "3.12.3"
    assert runner.lane(NEXT, {}).live_server_version == "3.13.6"
    environment = {matrix.SEMANTIC_PROFILE_ENV: NEXT}
    assert runner.lane(None, environment).semantic_profile == NEXT
    assert runner.typedb_version(environment) == "3.13.6"
    assert runner.typedb_version({}) == "3.12.3"
    with pytest.raises(runner.RunnerError):
        runner.lane("typedb-3.11.5/v1", {})


def test_proof_fragments_refuse_a_profile_outside_the_matrix() -> None:
    with pytest.raises(fragments.ProofFragmentError) as rejected:
        fragments.CONTRACTS["queries"].load_proof_fragments(
            [],
            expected_binding="python",
            run_nonce="0" * 64,
            semantic_profile="typedb-3.11.5/v1",
        )
    assert rejected.value.code == "semantic_profile_mismatch"


def test_administration_pins_distinct_native_history_per_profile() -> None:
    entries = matrix.load_matrix("administration")
    digests = [entry.expected_history_fingerprint for entry in entries.values()]
    assert len(set(digests)) == 2
    assert all(isinstance(value, str) and len(value) == 64 for value in digests)


@pytest.mark.parametrize("digest", [None, 12, "abc", "G" * 64])
def test_administration_rejects_missing_or_malformed_history_digest(
    tmp_path: Path, digest: Any
) -> None:
    relative = matrix.matrix_relative("administration")
    value = json.loads((ROOT / relative).read_text())
    value["profiles"][BASE]["expected_history_fingerprint"] = digest
    path = tmp_path / relative
    path.parent.mkdir(parents=True)
    path.write_text(json.dumps(value))
    with pytest.raises(matrix.ProfileMatrixError, match="history digest is malformed"):
        matrix.load_matrix("administration", root=tmp_path)


@pytest.mark.parametrize(
    ("profile", "supported"),
    [("typedb-3.11.5/v1", "no"), (BASE, "yes"), (NEXT, "yes")],
)
def test_live_fixture_selects_the_models_suite(profile: str, supported: str) -> None:
    script = (CI / "prepare_generated_live_fixture.sh").read_text()
    command = next(
        shlex.split(line.strip().removesuffix("\\"))
        for line in script.splitlines()
        if "import sdk_profile_matrix as m" in line
    )
    result = subprocess.run(
        [sys.executable, *command[1:], str(CI), profile],
        check=True,
        capture_output=True,
        text=True,
    )
    assert result.stdout.strip() == supported


@pytest.mark.parametrize(("profile", "server"), SERVERS.items())
def test_models_live_profile_gate_admits_the_named_suite_before_schema_io(
    profile: str, server: str
) -> None:
    from tests.integration.schema import test_generated_projection_live as live

    generated = ModuleType("generated_models")
    generated.SEMANTIC_SCHEMA_FINGERPRINT_JSON = json.dumps({"semantic_profile": profile})
    database = Mock()
    database.detected_server_version.return_value = server
    database.execute_query.side_effect = RuntimeError("schema I/O reached")

    with pytest.raises(RuntimeError, match="schema I/O reached"):
        live.sdk_models_data_model_runtime_live(database, generated)
