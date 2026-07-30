"""Checked Flight 5 inventory for generated Rust cross-binding parity."""

from __future__ import annotations

import json
import tomllib
from pathlib import Path
from typing import Any

ROOT = Path(__file__).parents[3]
INVENTORY = ROOT / "tests/fixtures/rust-generated-parity-inventory.json"

REQUIRED_CLIENT_ACCEPTANCE = {
    "public_rust_client_crate",
    "dependency_direction",
    "generated_external_consumer",
    "generated_model_contract",
    "exact_and_subtype_results",
    "inheritance_narrowing",
    "relation_role_ownership",
    "typed_crud_and_transactions",
    "owner_bound_query_algebra",
    "selected_result_shapes",
    "compile_fail_boundaries",
    "representative_live_parity",
    "local_remote_values_and_classified_errors",
    "no_internal_consumer_surfaces",
    "distribution_identity",
}

REQUIRED_SCENARIOS = {
    "entity_crud_and_batches",
    "inheritance_exact_and_subtype_reads",
    "optional_multivalue_and_nine_domains",
    "relation_crud_attributes_repeated_roles_keys_and_relation_players",
    "expressions_boolean_order_window_and_terminals",
    "typed_aggregates_and_grouping",
    "selected_rows_named_and_collected_pages",
    "bounded_cycles_and_shared_subtrees",
    "write_and_reusable_read_transactions",
    "local_and_one_exchange_remote_parity",
    "schema_generation_and_compile_rejection",
    "external_consumer_dependency_isolation",
}

REQUIRED_SYSTEM_GATES = {
    "single_isolated_python_packed_node_generated_rust_smoke",
    "generated_rust_tls",
    "generated_rust_msrv",
    "exact_server_oci_acceptance",
    "final_rust_distribution_identity",
}
EXPECTED_SYSTEM_GATE_STATUSES = {
    "single_isolated_python_packed_node_generated_rust_smoke": "accepted_live",
    "generated_rust_tls": "accepted_live",
    "generated_rust_msrv": "accepted_offline",
    "exact_server_oci_acceptance": "open",
    "final_rust_distribution_identity": "accepted_offline",
}


def _load() -> dict[str, Any]:
    return json.loads(INVENTORY.read_text(encoding="utf-8"))


def test_core_crates_do_not_depend_upward_on_the_public_rust_client() -> None:
    offenders: list[str] = []
    for manifest in sorted((ROOT / "type-bridge-core/crates").glob("*/Cargo.toml")):
        if manifest.parent.name == "rust":
            continue
        document = tomllib.loads(manifest.read_text(encoding="utf-8"))
        dependency_tables = [
            document.get("dependencies", {}),
            document.get("dev-dependencies", {}),
            document.get("build-dependencies", {}),
        ]
        for target in document.get("target", {}).values():
            dependency_tables.extend(
                [
                    target.get("dependencies", {}),
                    target.get("dev-dependencies", {}),
                    target.get("build-dependencies", {}),
                ]
            )
        for dependencies in dependency_tables:
            for name, specification in dependencies.items():
                package = specification.get("package") if isinstance(specification, dict) else None
                if name == "type-bridge" or package == "type-bridge":
                    offenders.append(f"{manifest.relative_to(ROOT)}:{name}")
    assert not offenders


def _assert_source_evidence(item: dict[str, Any]) -> None:
    assert item["evidence"], item["id"]
    for evidence in item["evidence"]:
        path = ROOT / evidence["path"]
        assert path.is_file(), path
        source = path.read_text(encoding="utf-8")
        assert evidence["anchors"], evidence
        for anchor in evidence["anchors"]:
            assert anchor in source, f"{item['id']}: {path}: {anchor}"


def test_rust_generated_parity_inventory_is_complete_and_source_tied() -> None:
    inventory = _load()
    assert inventory["format"] == "typebridge.rust-generated-parity-inventory/v1"
    assert inventory["statuses"] == ["accepted_live", "accepted_offline", "open"]

    client_acceptance = inventory["client_acceptance"]
    assert {criterion["id"] for criterion in client_acceptance} == (REQUIRED_CLIENT_ACCEPTANCE)
    assert all(
        criterion["status"] in {"accepted_live", "accepted_offline"}
        for criterion in client_acceptance
    )
    for criterion in client_acceptance:
        _assert_source_evidence(criterion)

    scenarios = inventory["scenarios"]
    assert isinstance(scenarios, list)
    assert {scenario["id"] for scenario in scenarios} == REQUIRED_SCENARIOS

    for scenario in scenarios:
        assert scenario["classification"] == "semantic_parity"
        assert scenario["status"] in inventory["statuses"]
        assert scenario["release_exclusion"] is None
        assert scenario["python"], scenario["id"]
        assert scenario["node"], scenario["id"]
        if scenario["status"] == "open":
            assert scenario["open_requirements"], scenario["id"]
        else:
            assert scenario["rust"], scenario["id"]

        for binding in ("python", "node", "rust"):
            for evidence in scenario[binding]:
                path = ROOT / evidence["path"]
                assert path.is_file(), path
                source = path.read_text(encoding="utf-8")
                assert evidence["anchors"], evidence
                for anchor in evidence["anchors"]:
                    assert anchor in source, f"{scenario['id']}: {path}: {anchor}"

    gates = inventory["system_gates"]
    assert {gate["id"] for gate in gates} == REQUIRED_SYSTEM_GATES
    assert {gate["id"]: gate["status"] for gate in gates} == EXPECTED_SYSTEM_GATE_STATUSES
    for gate in gates:
        _assert_source_evidence(gate)
        if gate["status"] == "open":
            assert gate["open_requirements"], gate["id"]


def test_inventory_does_not_turn_open_gaps_into_release_exclusions() -> None:
    inventory = _load()
    open_scenarios = [
        scenario for scenario in inventory["scenarios"] if scenario["status"] == "open"
    ]
    assert not open_scenarios
    assert all(scenario["release_exclusion"] is None for scenario in open_scenarios)
