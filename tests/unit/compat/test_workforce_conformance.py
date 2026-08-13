"""Closed-contract tests for the shared generated-SDK workforce checkpoint."""

from __future__ import annotations

import copy
import importlib.util
import json
import sys
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]
COMPARATOR_PATH = ROOT / "scripts/ci/compare_workforce_conformance.py"

EXPECTED_SELECTED_PROOFS = [
    ("workforce.crud.entity-single", "direct_runtime", "entity_lifecycle"),
    ("workforce.crud.relation-single", "direct_runtime", "relation_lifecycle"),
    (
        "workforce.model.values-and-references",
        "direct_runtime",
        "model_values_and_references",
    ),
    (
        "workforce.model.values-and-references",
        "remote_runtime",
        "model_values_and_references",
    ),
    (
        "workforce.query.remote-hydration",
        "direct_runtime",
        "hydrated_role_result",
    ),
    (
        "workforce.query.remote-hydration",
        "remote_runtime",
        "hydrated_role_result",
    ),
    (
        "workforce.query.remote-one-exchange",
        "remote_runtime",
        "remote_one_exchange",
    ),
    ("workforce.query.roles", "direct_runtime", "role_traversal"),
    ("workforce.query.roles", "remote_runtime", "role_traversal"),
]


def _load_comparator() -> ModuleType:
    spec = importlib.util.spec_from_file_location(
        "workforce_conformance_comparator", COMPARATOR_PATH
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


comparator = _load_comparator()


def _source_identity(path: str, digest: str) -> dict[str, str]:
    return {"path": path, "sha256": digest}


def _valid_report(binding: str, contracts: Any) -> dict[str, Any]:
    results = []
    for case_id, proof_kind, observation_ref in contracts.selected:
        capability = contracts.cases[case_id]["capability"]
        results.append(
            {
                "capability_id": capability["id"],
                "case_id": case_id,
                "observation": copy.deepcopy(contracts.observations[observation_ref]),
                "outcome": "passed",
                "proof_kind": proof_kind,
            }
        )
    return {
        "binding": binding,
        "catalog": _source_identity(comparator.CATALOG_RELATIVE, contracts.catalog.sha256),
        "fixture": {
            "id": "workforce-v1",
            "journey": _source_identity(comparator.JOURNEY_RELATIVE, contracts.journey.sha256),
            "projection_fingerprint": copy.deepcopy(contracts.projection_fingerprints[binding]),
            "projection_target": contracts.projection_targets[binding],
            "provider_schema": _source_identity(
                comparator.PROVIDER_SCHEMA_RELATIVE,
                contracts.provider_schema.sha256,
            ),
            "schema": _source_identity(comparator.SCHEMA_RELATIVE, contracts.schema.sha256),
            "semantic_fingerprint": copy.deepcopy(contracts.semantic_fingerprint),
            "semantic_profile": "typedb-3.12.1/v1",
            "version": 1,
        },
        "format": comparator.REPORT_FORMAT,
        "manifest": _source_identity(comparator.MANIFEST_RELATIVE, contracts.manifest.sha256),
        "results": results,
    }


def _valid_reports(contracts: Any) -> dict[str, dict[str, Any]]:
    return {binding: _valid_report(binding, contracts) for binding in comparator.REPORT_BINDINGS}


def _write_reports(
    directory: Path,
    reports: dict[str, dict[str, Any]],
) -> dict[str, Path]:
    directory.mkdir(parents=True, exist_ok=True)
    paths = {}
    for binding, report in reports.items():
        path = directory / f"{binding}.json"
        path.write_bytes(comparator.canonical_json_bytes(report))
        paths[binding] = path
    return paths


def _assert_rejected(paths: list[Path], expected_code: str) -> None:
    with pytest.raises(comparator.ContractError) as rejected:
        comparator.compare_reports(paths)
    assert rejected.value.code == expected_code


def test_catalog_covers_manifest_and_freezes_fixture_authority() -> None:
    contracts = comparator.load_contracts()
    catalog = contracts.catalog.value
    manifest = contracts.manifest.value

    manifest_cases = [capability["case_ids"][0] for capability in manifest["capabilities"]]
    catalog_cases = [case["id"] for case in catalog["cases"]]
    dispositions = [case["disposition"] for case in catalog["cases"]]
    assert catalog_cases == manifest_cases
    assert len(catalog_cases) == len(set(catalog_cases)) == 44
    assert dispositions.count("shared_smoke") == 6
    assert dispositions.count("retained_evidence") == 26
    assert dispositions.count("known_gap") == 12
    assert [tuple(proof.values()) for proof in catalog["selected_proofs"]] == (
        EXPECTED_SELECTED_PROOFS
    )
    assert catalog["report_bindings"] == ["python", "node", "rust"]
    assert catalog["projection_targets"] == {
        "node": "typescript",
        "python": "python",
        "rust": "rust",
    }
    assert catalog["expected_fingerprints"] == {
        "projections": contracts.projection_fingerprints,
        "semantic": contracts.semantic_fingerprint,
    }
    assert catalog["fixture"] == {
        "id": "workforce-v1",
        "provider_schema_path": comparator.PROVIDER_SCHEMA_RELATIVE,
        "schema_path": comparator.SCHEMA_RELATIVE,
        "semantic_profile": "typedb-3.12.1/v1",
        "version": 1,
    }
    assert (
        contracts.schema.sha256
        == "b9f85a9c11198752dd94efeed06a9a2d42ca3007d93354c35a8abf884f25923a"
    )
    assert contracts.provider_schema.sha256 == (
        "9f204ec146fe1b82eb199715825c3c0a28ccaaf911c087a581276a5d0ab6de59"
    )


def test_journey_is_binding_neutral_and_freezes_exact_observations() -> None:
    contracts = comparator.load_contracts()
    journey = contracts.journey.value
    person = journey["records"]["person"]

    assert tuple(contracts.selected) == tuple(EXPECTED_SELECTED_PROOFS)
    assert person["fields"]["identifier"] == {"kind": "string", "value": "workforce-ada"}
    assert person["fields"]["aliases"] == [
        {"kind": "string", "value": "analyst"},
        {"kind": "string", "value": "mathematician"},
    ]
    assert person["update"] == {"nickname": {"kind": "string", "value": "Ada Lovelace"}}
    assert journey["records"]["membership"] == {
        "model": "membership",
        "player": {"key": "workforce-ada", "model": "person"},
        "role": "member",
    }
    assert journey["operation_order"] == [
        "insert_person",
        "read_person",
        "update_person",
        "insert_membership",
        "read_membership",
        "direct_role_query_one",
        "remote_role_query_one",
        "delete_membership",
        "delete_person",
    ]
    assert journey["expected_observations"]["remote_one_exchange"] == {
        "exchange_count": 1,
        "terminal": "one",
    }
    serialized = json.dumps(journey, sort_keys=True)
    for forbidden in (
        '"address"',
        '"database"',
        '"iid"',
        '"port"',
        '"timestamp"',
        "TypeDBOptions",
        "type_bridge",
    ):
        assert forbidden not in serialized


def test_report_schema_closes_source_identities_fixture_and_rows() -> None:
    contracts = comparator.load_contracts()
    schema = contracts.report_schema.value

    assert schema["required"] == ["format", "binding", "manifest", "catalog", "fixture", "results"]
    assert schema["additionalProperties"] is False
    assert schema["properties"]["manifest"] == {"$ref": "#/$defs/sourceIdentity"}
    assert schema["properties"]["catalog"] == {"$ref": "#/$defs/sourceIdentity"}
    assert schema["properties"]["results"]["minItems"] == 9
    assert schema["properties"]["results"]["maxItems"] == 9
    assert schema["$defs"]["sourceIdentity"]["required"] == ["path", "sha256"]
    fixture = schema["$defs"]["fixture"]
    assert {"schema", "provider_schema", "journey", "projection_target"} <= set(fixture["required"])
    assert fixture["additionalProperties"] is False


def test_valid_three_report_set_is_order_independent_and_derives_only_unresolved_state(
    tmp_path: Path,
) -> None:
    contracts = comparator.load_contracts()
    paths = _write_reports(tmp_path, _valid_reports(contracts))

    forward = comparator.compare_reports([paths["python"], paths["node"], paths["rust"]])
    reverse = comparator.compare_reports([paths["rust"], paths["node"], paths["python"]])
    assert comparator.canonical_json_bytes(forward) == comparator.canonical_json_bytes(reverse)
    assert forward["format"] == comparator.SUMMARY_FORMAT
    assert forward["bindings"] == ["python", "node", "rust"]
    assert len(forward["passed_proofs"]) == 9
    assert forward["passed_proofs"] == [
        {
            "capability_id": contracts.cases[case_id]["capability"]["id"],
            "case_id": case_id,
            "observation": contracts.observations[observation_ref],
            "proof_kind": proof_kind,
        }
        for case_id, proof_kind, observation_ref in contracts.selected
    ]

    gaps = {item["case_id"]: item["bindings"] for item in forward["current_gaps"]}
    assert gaps["workforce.crud.entity-batch-update-delete"] == ["node"]
    assert gaps["workforce.crud.relation-batch-update-delete"] == ["node"]
    assert "workforce.query.reducers-remote" not in gaps
    assert "workforce.query.schema-function" not in gaps
    assert len(gaps) == 14
    assert forward["uncovered_required_proofs"]
    summary_text = comparator.canonical_json_bytes(forward).decode()
    assert '"accepted"' not in summary_text
    assert "full_support" not in summary_text


@pytest.mark.parametrize(
    ("identity", "expected_code"),
    [
        ("manifest", "manifest_digest_mismatch"),
        ("catalog", "catalog_digest_mismatch"),
        ("schema", "schema_digest_mismatch"),
        ("provider_schema", "provider_schema_digest_mismatch"),
        ("journey", "journey_digest_mismatch"),
    ],
)
def test_wrong_source_digest_fails_for_exact_identity(
    tmp_path: Path,
    identity: str,
    expected_code: str,
) -> None:
    contracts = comparator.load_contracts()
    reports = _valid_reports(contracts)
    owner = (
        reports["python"] if identity in {"manifest", "catalog"} else reports["python"]["fixture"]
    )
    owner[identity]["sha256"] = "0" * 64
    paths = _write_reports(tmp_path, reports)

    _assert_rejected(list(paths.values()), expected_code)


def test_semantic_fingerprint_mismatch_fails_closed(tmp_path: Path) -> None:
    contracts = comparator.load_contracts()
    reports = _valid_reports(contracts)
    reports["node"]["fixture"]["semantic_fingerprint"]["digest"] = "e" * 64
    paths = _write_reports(tmp_path, reports)

    _assert_rejected(list(paths.values()), "semantic_fingerprint_authority_mismatch")


def test_common_fabricated_semantic_fingerprint_fails_authority(tmp_path: Path) -> None:
    contracts = comparator.load_contracts()
    reports = _valid_reports(contracts)
    for report in reports.values():
        report["fixture"]["semantic_fingerprint"]["digest"] = "e" * 64
    paths = _write_reports(tmp_path, reports)

    _assert_rejected(list(paths.values()), "semantic_fingerprint_authority_mismatch")


def test_projection_target_must_follow_catalog_mapping(tmp_path: Path) -> None:
    contracts = comparator.load_contracts()
    reports = _valid_reports(contracts)
    reports["node"]["fixture"]["projection_target"] = "python"
    paths = _write_reports(tmp_path, reports)

    _assert_rejected(list(paths.values()), "projection_target_mismatch")


def test_projection_fingerprints_must_match_frozen_targets(tmp_path: Path) -> None:
    contracts = comparator.load_contracts()
    reports = _valid_reports(contracts)
    reports["node"]["fixture"]["projection_fingerprint"] = copy.deepcopy(
        reports["python"]["fixture"]["projection_fingerprint"]
    )
    paths = _write_reports(tmp_path, reports)

    _assert_rejected(list(paths.values()), "projection_fingerprint_authority_mismatch")


def test_distinct_fabricated_projection_fingerprints_fail_authority(tmp_path: Path) -> None:
    contracts = comparator.load_contracts()
    reports = _valid_reports(contracts)
    for index, binding in enumerate(comparator.REPORT_BINDINGS, start=1):
        reports[binding]["fixture"]["projection_fingerprint"]["digest"] = f"{index:x}" * 64
    paths = _write_reports(tmp_path, reports)

    _assert_rejected(list(paths.values()), "projection_fingerprint_authority_mismatch")


def test_each_observation_must_equal_the_committed_journey(tmp_path: Path) -> None:
    contracts = comparator.load_contracts()
    reports = _valid_reports(contracts)
    reports["python"]["results"][0]["observation"]["created"] = False
    paths = _write_reports(tmp_path, reports)

    _assert_rejected(list(paths.values()), "observation_mismatch")


def test_direct_and_remote_rows_for_one_observation_must_agree(tmp_path: Path) -> None:
    contracts = comparator.load_contracts()
    reports = _valid_reports(contracts)
    reports["python"]["results"][2]["observation"]["nickname"] = "Wrong"
    paths = _write_reports(tmp_path, reports)

    _assert_rejected(list(paths.values()), "observation_ref_mismatch")


def test_duplicate_unknown_missing_and_failed_rows_fail_closed(tmp_path: Path) -> None:
    contracts = comparator.load_contracts()

    duplicate = _valid_reports(contracts)
    duplicate["python"]["results"][1] = copy.deepcopy(duplicate["python"]["results"][0])
    paths = _write_reports(tmp_path / "duplicate", duplicate)
    _assert_rejected(list(paths.values()), "duplicate_result")

    unknown_case = _valid_reports(contracts)
    unknown_case["python"]["results"][0]["case_id"] = "workforce.unknown"
    paths = _write_reports(tmp_path / "unknown-case", unknown_case)
    _assert_rejected(list(paths.values()), "unknown_result_case")

    unknown_proof = _valid_reports(contracts)
    unknown_proof["python"]["results"][0]["proof_kind"] = "invented_runtime"
    paths = _write_reports(tmp_path / "unknown-proof", unknown_proof)
    _assert_rejected(list(paths.values()), "unknown_proof_kind")

    missing = _valid_reports(contracts)
    missing["python"]["results"].pop()
    paths = _write_reports(tmp_path / "missing", missing)
    _assert_rejected(list(paths.values()), "result_coverage_mismatch")

    failed = _valid_reports(contracts)
    failed["python"]["results"][0]["outcome"] = "failed"
    paths = _write_reports(tmp_path / "failed", failed)
    _assert_rejected(list(paths.values()), "nonpassing_result")


def test_missing_binding_fails_before_any_summary_is_derived(tmp_path: Path) -> None:
    contracts = comparator.load_contracts()
    paths = _write_reports(tmp_path, _valid_reports(contracts))

    _assert_rejected([paths["python"], paths["rust"]], "missing_binding")


def test_runtime_identity_leak_is_rejected_before_comparison(tmp_path: Path) -> None:
    contracts = comparator.load_contracts()
    reports = _valid_reports(contracts)
    reports["python"]["results"][0]["observation"]["provider_iid"] = "0x012345"
    paths = _write_reports(tmp_path, reports)

    _assert_rejected(list(paths.values()), "runtime_identity_leak")


def test_manifest_gap_cannot_be_reported_as_passing_evidence(tmp_path: Path) -> None:
    contracts = comparator.load_contracts()
    reports = _valid_reports(contracts)
    gap_case = "workforce.schema.ordered-distinct"
    gap_capability = contracts.cases[gap_case]["capability"]
    reports["python"]["results"][0] = {
        "capability_id": gap_capability["id"],
        "case_id": gap_case,
        "observation": {},
        "outcome": "passed",
        "proof_kind": "remote_runtime",
    }
    paths = _write_reports(tmp_path, reports)

    _assert_rejected(list(paths.values()), "manifest_gap_pass")


def test_reports_require_canonical_json_and_unique_keys(tmp_path: Path) -> None:
    contracts = comparator.load_contracts()
    reports = _valid_reports(contracts)
    paths = _write_reports(tmp_path, reports)
    paths["python"].write_text(json.dumps(reports["python"], indent=2) + "\n", encoding="utf-8")
    _assert_rejected(list(paths.values()), "noncanonical_report_json")

    paths = _write_reports(tmp_path / "duplicate-key", reports)
    python_body = paths["python"].read_text(encoding="utf-8")
    paths["python"].write_text(
        python_body.replace('"binding":"python"', '"binding":"python","binding":"node"', 1),
        encoding="utf-8",
    )
    _assert_rejected(list(paths.values()), "duplicate_json_key")


def test_oversized_report_is_rejected_before_json_parsing(tmp_path: Path) -> None:
    contracts = comparator.load_contracts()
    paths = _write_reports(tmp_path, _valid_reports(contracts))
    paths["python"].write_bytes(b" " * (comparator.MAX_JSON_BYTES + 1))

    _assert_rejected(list(paths.values()), "json_size_limit_exceeded")


def test_report_inputs_reject_regular_and_dangling_symlinks(tmp_path: Path) -> None:
    contracts = comparator.load_contracts()
    paths = _write_reports(tmp_path / "reports", _valid_reports(contracts))

    linked = tmp_path / "linked-python.json"
    linked.symlink_to(paths["python"])
    _assert_rejected([linked, paths["node"], paths["rust"]], "invalid_source_file")

    dangling = tmp_path / "dangling-python.json"
    dangling.symlink_to(tmp_path / "absent.json")
    _assert_rejected([dangling, paths["node"], paths["rust"]], "invalid_source_file")


def test_report_publication_contract_is_opt_in_atomic_and_no_clobber() -> None:
    readme = (ROOT / "tests/contracts/sdk_conformance/workforce-v1/README.md").read_text(
        encoding="utf-8"
    )
    prose = " ".join(readme.split())

    assert "TYPE_BRIDGE_WORKFORCE_REPORT" in prose
    assert "absolute UTF-8 path of at most 4096 bytes" in prose
    assert "existing non-symlink parent directory" in prose
    assert "absent destination" in prose
    assert "atomically from a temporary file in that parent" in prose
    assert "only after all assertions and cleanup succeed" in prose


def test_ci_fans_in_exact_pinned_3_12_workforce_artifacts() -> None:
    workflow = yaml.safe_load((ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8"))
    jobs = workflow["jobs"]
    upload_action = "actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02"
    download_action = "actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093"
    producers = {
        "python": ("test-integration", "matrix.test-group == 'schema'"),
        "node": ("node-integration", None),
        "rust": ("rust-integration", None),
    }

    all_steps = [step for job in jobs.values() for step in job.get("steps", [])]
    for binding, (job_name, extra_condition) in producers.items():
        artifact_name = f"workforce-conformance-{binding}"
        uploads = [
            step
            for step in all_steps
            if step.get("with", {}).get("name") == artifact_name
            and str(step.get("uses", "")).startswith("actions/upload-artifact@")
        ]
        assert len(uploads) == 1
        upload = uploads[0]
        assert upload in jobs[job_name]["steps"]
        assert upload["uses"] == upload_action
        assert "matrix.typedb-server == 'typedb/typedb:3.12.1'" in upload["if"]
        if extra_condition is not None:
            assert extra_condition in upload["if"]
        assert upload["with"] == {
            "if-no-files-found": "error",
            "name": artifact_name,
            "path": f"${{{{ runner.temp }}}}/typebridge-workforce/{binding}.json",
            "retention-days": 7,
        }

    aggregate = jobs["workforce-conformance"]
    assert set(aggregate["needs"]) == {job_name for job_name, _ in producers.values()}
    aggregate_steps = aggregate["steps"]
    downloads = [step for step in aggregate_steps if step.get("uses") == download_action]
    assert len(downloads) == 3
    assert {step["with"]["name"] for step in downloads} == {
        f"workforce-conformance-{binding}" for binding in producers
    }
    assert {step["with"]["path"] for step in downloads} == {
        "${{ runner.temp }}/typebridge-workforce"
    }
    assert not [
        step
        for step in aggregate_steps
        if str(step.get("uses", "")).startswith("actions/download-artifact@")
        and step.get("uses") != download_action
    ]
    compare_steps = [
        step
        for step in aggregate_steps
        if "scripts/ci/compare_workforce_conformance.py" in step.get("run", "")
    ]
    assert len(compare_steps) == 1
    assert compare_steps[0]["run"].split() == [
        "python",
        "scripts/ci/compare_workforce_conformance.py",
        '"$RUNNER_TEMP/typebridge-workforce/python.json"',
        '"$RUNNER_TEMP/typebridge-workforce/node.json"',
        '"$RUNNER_TEMP/typebridge-workforce/rust.json"',
    ]


def test_local_runner_allocates_and_compares_three_distinct_reports() -> None:
    source = (ROOT / "test.sh").read_text(encoding="utf-8")
    collapsed = " ".join(source.replace("\\\n", " ").split())

    assert 'if [[ "$typedb_server_version" == "3.12.1" ]]; then' in source
    assert 'mktemp -d "${TMPDIR:-/tmp}/typebridge-workforce.XXXXXXXXXX"' in source
    for binding in comparator.REPORT_BINDINGS:
        assignment = f'"TYPE_BRIDGE_WORKFORCE_REPORT=$workforce_report_dir/{binding}.json"'
        assert source.count(assignment) == 1
    assert (
        "uv run python scripts/ci/compare_workforce_conformance.py "
        '"$workforce_report_dir/python.json" '
        '"$workforce_report_dir/node.json" '
        '"$workforce_report_dir/rust.json"'
    ) in collapsed
