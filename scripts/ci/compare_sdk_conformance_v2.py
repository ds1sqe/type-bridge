#!/usr/bin/env python3
"""Validate and compare the Python, Node, Rust, and C sdk-v2 reports."""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
from compare_sdk_conformance import (  # noqa: E402, F401
    FORBIDDEN_OBSERVATION_KEYS,
    MAX_JSON_BYTES,
    OBSERVATION_KEY_RE,
    SHA256_RE,
    TYPEDB_IID_RE,
    ContractError,
    LoadedJson,
    _duplicate_key_object,
    _exact_list,
    _exact_object,
    _expand_binding_profile,
    _inspect_observation,
    _integer,
    _load_json,
    _string,
    _validate_fingerprint,
    _validate_source_identity,
    canonical_json_bytes,
    compare_report_set,
    contract_path,
    report_parser,
    validate_report_schema,
)

ROOT = Path(__file__).resolve().parents[2]
MANIFEST_RELATIVE = "tests/contracts/sdk_conformance/manifest-v1.json"
CATALOG_RELATIVE = "tests/contracts/sdk_conformance/sdk-v2/catalog-v2.json"
JOURNEY_RELATIVE = "tests/contracts/sdk_conformance/sdk-v2/journey-v2.json"
REPORT_SCHEMA_RELATIVE = "tests/contracts/sdk_conformance/sdk-v2/report-schema-v2.json"
SCHEMA_RELATIVE = "type-bridge-core/crates/schema-codegen/tests/acceptance/schema.yaml"
PROVIDER_SCHEMA_RELATIVE = (
    "type-bridge-core/crates/schema-codegen/tests/acceptance/provider-3.12.1.tql"
)

REPORT_FORMAT = "typebridge.sdk-conformance-report/v2"
SUMMARY_FORMAT = "typebridge.sdk-conformance-summary/v2"
CATALOG_FORMAT = "typebridge.sdk-catalog/v2"
JOURNEY_FORMAT = "typebridge.sdk-journey/v2"
REPORT_BINDINGS = ("python", "node", "rust", "c")
FINAL_BROAD_SUCCESSOR_CASES = frozenset(
    {
        "sdk.runtime.cancellation",
        "sdk.runtime.timeout-resource-limits",
        "sdk.diagnostic.all-workflows",
        "sdk.runtime.explicit-close",
    }
)
FINAL_DISTRIBUTION_SUCCESSOR_CASE = "sdk.distribution.standalone-cli"
CURRENT_BINDINGS = frozenset(REPORT_BINDINGS)
PROJECTION_TARGETS = {
    "python": "python",
    "node": "typescript",
    "rust": "rust",
    "c": "c",
}
PROOF_KINDS = (
    "compile_positive",
    "compile_negative",
    "direct_runtime",
    "remote_runtime",
    "diagnostic",
    "lifecycle",
    "artifact",
)
EXPECTED_SELECTED_PROOFS = (
    ("sdk.crud.entity-single", "direct_runtime", "entity_lifecycle"),
    ("sdk.crud.relation-single", "direct_runtime", "relation_lifecycle"),
    (
        "sdk.diagnostic.all-workflows",
        "diagnostic",
        "structured_query_diagnostic",
    ),
    (
        "sdk.diagnostic.remote-structured",
        "diagnostic",
        "remote_structured_diagnostic",
    ),
    (
        "sdk.model.values-and-references",
        "direct_runtime",
        "model_values_and_references",
    ),
    (
        "sdk.model.values-and-references",
        "remote_runtime",
        "model_values_and_references",
    ),
    ("sdk.query.exact-subtypes", "direct_runtime", "exact_subtypes"),
    ("sdk.query.exact-subtypes", "remote_runtime", "exact_subtypes"),
    ("sdk.query.owner-iid-set", "direct_runtime", "owner_iid_set"),
    ("sdk.query.owner-iid-set", "remote_runtime", "owner_iid_set"),
    (
        "sdk.query.reducers-direct",
        "direct_runtime",
        "grouped_reducer",
    ),
    (
        "sdk.query.reducers-remote",
        "remote_runtime",
        "grouped_reducer",
    ),
    (
        "sdk.query.remote-hydration",
        "direct_runtime",
        "hydrated_result",
    ),
    (
        "sdk.query.remote-hydration",
        "remote_runtime",
        "hydrated_result",
    ),
    (
        "sdk.query.remote-one-exchange",
        "remote_runtime",
        "remote_one_exchange",
    ),
    ("sdk.query.roles", "direct_runtime", "roles"),
    ("sdk.query.roles", "remote_runtime", "roles"),
    (
        "sdk.query.scalar-boolean-predicates",
        "direct_runtime",
        "scalar_boolean",
    ),
    (
        "sdk.query.scalar-boolean-predicates",
        "remote_runtime",
        "scalar_boolean",
    ),
    (
        "sdk.query.schema-function",
        "direct_runtime",
        "schema_function",
    ),
    (
        "sdk.query.schema-function",
        "remote_runtime",
        "schema_function",
    ),
    (
        "sdk.query.selection-shapes",
        "direct_runtime",
        "selection_shapes",
    ),
    (
        "sdk.query.selection-shapes",
        "remote_runtime",
        "selection_shapes",
    ),
    ("sdk.query.terminals", "direct_runtime", "terminals"),
    ("sdk.query.terminals", "remote_runtime", "terminals"),
    ("sdk.query.topology", "direct_runtime", "topology"),
    ("sdk.query.topology", "remote_runtime", "topology"),
    (
        "sdk.runtime.cancellation",
        "direct_runtime",
        "cancellation_direct",
    ),
    (
        "sdk.runtime.cancellation",
        "remote_runtime",
        "cancellation_remote",
    ),
    (
        "sdk.runtime.explicit-close",
        "lifecycle",
        "query_resource_lifecycle",
    ),
    (
        "sdk.runtime.timeout-resource-limits",
        "direct_runtime",
        "resource_limits",
    ),
    (
        "sdk.runtime.timeout-resource-limits",
        "remote_runtime",
        "resource_limits",
    ),
    (
        "sdk.value.scalar-domain-comparison",
        "direct_runtime",
        "scalar_domain",
    ),
    (
        "sdk.value.scalar-domain-comparison",
        "remote_runtime",
        "scalar_domain",
    ),
)
EXPECTED_MANIFEST_TRANSITION_CASES = (
    "sdk.model.values-and-references",
    "sdk.crud.entity-single",
    "sdk.crud.relation-single",
    "sdk.query.owner-iid-set",
    "sdk.query.exact-subtypes",
    "sdk.query.scalar-boolean-predicates",
    "sdk.query.roles",
    "sdk.query.topology",
    "sdk.query.selection-shapes",
    "sdk.query.terminals",
    "sdk.query.reducers-direct",
    "sdk.query.remote-one-exchange",
    "sdk.query.remote-hydration",
    "sdk.diagnostic.remote-structured",
    "sdk.value.scalar-domain-comparison",
    "sdk.query.reducers-remote",
    "sdk.query.schema-function",
)

CONTRACT_NAME_RE = re.compile(r"^[a-z0-9][a-z0-9._/-]*$")
EXPECTED_OBSERVATION_REFS = frozenset(
    observation_ref for _, _, observation_ref in EXPECTED_SELECTED_PROOFS
)


@dataclass(frozen=True)
class Contracts:
    manifest: LoadedJson
    catalog: LoadedJson
    journey: LoadedJson
    report_schema: LoadedJson
    schema: LoadedJson
    provider_schema: LoadedJson
    capabilities: tuple[dict[str, Any], ...]
    cases: dict[str, dict[str, Any]]
    selected: tuple[tuple[str, str, str], ...]
    manifest_transition_cases: tuple[str, ...]
    observations: dict[str, dict[str, Any]]
    projection_targets: dict[str, str]
    semantic_fingerprint: dict[str, Any]
    projection_fingerprints: dict[str, dict[str, Any]]


def _contract_path(value: Any, expected: str, label: str) -> Path:
    return contract_path(value, expected, label, ROOT)


def _validate_report_schema(value: Any) -> None:
    validate_report_schema(value, REPORT_FORMAT, REPORT_BINDINGS, len(EXPECTED_SELECTED_PROOFS))


def _validate_journey(value: Any) -> dict[str, dict[str, Any]]:
    journey = _exact_object(
        value,
        {
            "format",
            "fixture_id",
            "version",
            "semantic_profile",
            "records",
            "create_order",
            "cleanup_order",
            "expected_observations",
        },
        "journey",
    )
    if journey["format"] != JOURNEY_FORMAT:
        raise ContractError("invalid_journey_format", "journey format is not v2")
    if journey["fixture_id"] != "sdk-v2" or journey["version"] != 2:
        raise ContractError("invalid_fixture_identity", "journey fixture identity is not v2")
    if journey["semantic_profile"] != "typedb-3.12.1/v1":
        raise ContractError("semantic_profile_mismatch", "journey profile is not 3.12.1/v1")

    records = _exact_object(
        journey["records"],
        {"people", "employee", "manager", "membership", "network_link"},
        "journey records",
    )
    people = _exact_list(records["people"], "journey people")
    if len(people) != 2:
        raise ContractError("invalid_journey_records", "journey must contain exactly two people")
    identifiers: list[Any] = []
    for index, person_value in enumerate(people):
        person = _exact_object(person_value, {"model", "fields"}, f"person {index}")
        if person["model"] != "person" or type(person["fields"]) is not dict:
            raise ContractError("invalid_journey_model", f"person {index} is malformed")
        identifier = person["fields"].get("identifier")
        if type(identifier) is not dict or identifier.get("kind") != "string":
            raise ContractError("invalid_journey_identity", f"person {index} lacks a string key")
        identifiers.append(identifier.get("value"))
    if identifiers != ["query-ada", "query-dana"]:
        raise ContractError("invalid_journey_identity", "person keys are not frozen")
    if records["employee"].get("model") != "employee":
        raise ContractError("invalid_journey_model", "employee record is malformed")
    if records["manager"].get("model") != "manager":
        raise ContractError("invalid_journey_model", "manager record is malformed")
    if records["membership"].get("model") != "membership":
        raise ContractError("invalid_journey_model", "membership record is malformed")
    if records["network_link"].get("model") != "network-link":
        raise ContractError("invalid_journey_model", "network-link record is malformed")

    create_order = _exact_list(journey["create_order"], "create order")
    cleanup_order = _exact_list(journey["cleanup_order"], "cleanup order")
    if create_order != [
        "query-ada",
        "query-dana",
        "query-employee",
        "query-manager",
        "query-membership",
        "query-link",
    ]:
        raise ContractError("invalid_operation_order", "create order is not frozen")
    if cleanup_order != list(reversed(create_order)):
        raise ContractError("invalid_operation_order", "cleanup is not dependency-reversed")

    observations = _exact_object(
        journey["expected_observations"],
        set(EXPECTED_OBSERVATION_REFS),
        "expected observations",
    )
    for name, observation in observations.items():
        if type(observation) is not dict:
            raise ContractError("invalid_observation", f"observation {name!r} must be an object")
        _inspect_observation(observation, f"journey observation {name}")
    return observations


def _validate_catalog(
    value: Any,
    manifest: dict[str, Any],
) -> tuple[
    dict[str, dict[str, Any]],
    tuple[tuple[str, str, str], ...],
    tuple[str, ...],
    dict[str, str],
    dict[str, Any],
    dict[str, dict[str, Any]],
]:
    catalog = _exact_object(
        value,
        {
            "format",
            "fixture",
            "journey_path",
            "report_schema_path",
            "report_bindings",
            "projection_targets",
            "expected_fingerprints",
            "manifest_transition_cases",
            "selected_proofs",
            "cases",
        },
        "catalog",
    )
    if catalog["format"] != CATALOG_FORMAT:
        raise ContractError("invalid_catalog_format", "catalog format is not v2")
    fixture = _exact_object(
        catalog["fixture"],
        {"id", "version", "semantic_profile", "schema_path", "provider_schema_path"},
        "catalog fixture",
    )
    if fixture != {
        "id": "sdk-v2",
        "version": 2,
        "semantic_profile": "typedb-3.12.1/v1",
        "schema_path": SCHEMA_RELATIVE,
        "provider_schema_path": PROVIDER_SCHEMA_RELATIVE,
    }:
        raise ContractError("invalid_fixture_identity", "catalog fixture is not frozen v2")
    if catalog["journey_path"] != JOURNEY_RELATIVE:
        raise ContractError("contract_path_mismatch", "catalog journey path is not frozen")
    if catalog["report_schema_path"] != REPORT_SCHEMA_RELATIVE:
        raise ContractError("contract_path_mismatch", "catalog report-schema path is not frozen")
    if catalog["report_bindings"] != list(REPORT_BINDINGS):
        raise ContractError("invalid_report_bindings", "catalog report bindings are not frozen")
    projection_targets = _exact_object(
        catalog["projection_targets"], set(REPORT_BINDINGS), "projection targets"
    )
    if projection_targets != PROJECTION_TARGETS:
        raise ContractError("invalid_projection_targets", "projection target mapping is not frozen")

    expected_fingerprints = _exact_object(
        catalog["expected_fingerprints"],
        {"semantic", "projections"},
        "expected fingerprints",
    )
    semantic_fingerprint = _validate_fingerprint(
        expected_fingerprints["semantic"],
        label="expected semantic fingerprint",
        domain="typebridge.schema.semantic",
        canonicalization="typebridge.schema-canonical-json/v1",
    )
    projection_values = _exact_object(
        expected_fingerprints["projections"],
        set(REPORT_BINDINGS),
        "expected projection fingerprints",
    )
    projection_fingerprints: dict[str, dict[str, Any]] = {}
    for binding in REPORT_BINDINGS:
        projection_fingerprints[binding] = _validate_fingerprint(
            projection_values[binding],
            label=f"expected {binding} projection fingerprint",
            domain="typebridge.binding.projection",
            canonicalization="typebridge.binding-projection/v1",
        )
    if len({item["digest"] for item in projection_fingerprints.values()}) != len(REPORT_BINDINGS):
        raise ContractError(
            "projection_fingerprint_collision",
            "expected binding projection fingerprints must differ",
        )

    capabilities = _exact_list(manifest.get("capabilities"), "manifest capabilities")
    manifest_cases: list[tuple[str, str, dict[str, Any]]] = []
    for capability in capabilities:
        if type(capability) is not dict:
            raise ContractError("invalid_manifest_capability", "manifest capability is malformed")
        case_ids = capability.get("case_ids")
        if type(case_ids) is not list or len(case_ids) != 1 or type(case_ids[0]) is not str:
            raise ContractError("invalid_manifest_case", "each manifest capability needs one case")
        manifest_cases.append(
            (case_ids[0], _string(capability.get("id"), "capability ID"), capability)
        )

    catalog_cases = _exact_list(catalog["cases"], "catalog cases")
    if len(catalog_cases) != len(manifest_cases):
        raise ContractError("catalog_coverage_mismatch", "catalog case count differs from manifest")
    selected_case_ids = {case_id for case_id, _, _ in EXPECTED_SELECTED_PROOFS}
    cases: dict[str, dict[str, Any]] = {}
    for index, (catalog_case, manifest_case) in enumerate(
        zip(catalog_cases, manifest_cases, strict=True)
    ):
        case = _exact_object(
            catalog_case,
            {"id", "capability_id", "disposition"},
            f"catalog case {index}",
        )
        case_id, capability_id, capability = manifest_case
        if case["id"] != case_id or case["capability_id"] != capability_id:
            raise ContractError(
                "catalog_coverage_mismatch",
                f"catalog case {index} does not match the manifest",
            )
        if case_id in cases:
            raise ContractError("duplicate_catalog_case", f"duplicate catalog case {case_id!r}")
        statuses = _expand_binding_profile(manifest, capability["binding_profile"])
        if case_id in selected_case_ids:
            expected_disposition = "shared_smoke"
        elif all(statuses.get(binding) == "gap" for binding in REPORT_BINDINGS):
            expected_disposition = "known_gap"
        else:
            expected_disposition = "retained_evidence"
        if case["disposition"] != expected_disposition:
            raise ContractError(
                "invalid_case_disposition",
                f"{case_id!r} must be {expected_disposition!r}",
            )
        cases[case_id] = {"catalog": case, "capability": capability}

    for case_id in FINAL_BROAD_SUCCESSOR_CASES:
        if cases[case_id]["capability"]["binding_profile"] not in {
            "current_gap_future_planned",
            "terminal_broad_live_future_planned",
        }:
            raise ContractError(
                "invalid_successor_profile", f"{case_id!r} successor profile drifted"
            )
    if cases[FINAL_DISTRIBUTION_SUCCESSOR_CASE]["capability"]["binding_profile"] not in {
        "current_gap_future_planned",
        "standalone_distribution_offline_future_planned",
    }:
        raise ContractError(
            "invalid_successor_profile",
            f"{FINAL_DISTRIBUTION_SUCCESSOR_CASE!r} successor profile drifted",
        )

    selected_values = _exact_list(catalog["selected_proofs"], "selected proofs")
    selected: list[tuple[str, str, str]] = []
    for index, value in enumerate(selected_values):
        proof = _exact_object(
            value,
            {"case_id", "proof_kind", "observation_ref"},
            f"selected proof {index}",
        )
        selected.append((proof["case_id"], proof["proof_kind"], proof["observation_ref"]))
    if tuple(selected) != EXPECTED_SELECTED_PROOFS:
        raise ContractError("selected_proof_mismatch", "selected proof set or order is not frozen")
    for case_id, proof_kind, _ in selected:
        capability = cases[case_id]["capability"]
        profile = manifest["proof_profiles"][capability["proof_profile"]]
        if profile.get(proof_kind) != "required":
            raise ContractError(
                "proof_not_applicable",
                f"{case_id!r}/{proof_kind!r} is not required",
            )
        statuses = _expand_binding_profile(manifest, capability["binding_profile"])
        if set(REPORT_BINDINGS) - statuses.keys():
            raise ContractError(
                "selected_proof_status_missing",
                f"{case_id!r} has no manifest status for every report binding",
            )

    transition_values = _exact_list(
        catalog["manifest_transition_cases"],
        "manifest transition cases",
    )
    transition_cases: list[str] = []
    for index, value in enumerate(transition_values):
        case_id = _string(value, f"manifest transition case {index}")
        if case_id in transition_cases:
            raise ContractError(
                "duplicate_manifest_transition_case",
                f"duplicate manifest transition case {case_id!r}",
            )
        if case_id not in selected_case_ids:
            raise ContractError(
                "manifest_transition_case_not_selected",
                f"manifest transition case {case_id!r} has no selected proof",
            )
        transition_cases.append(case_id)
    if tuple(transition_cases) != EXPECTED_MANIFEST_TRANSITION_CASES:
        raise ContractError(
            "manifest_transition_case_mismatch",
            "manifest transition case set or order is not frozen",
        )
    return (
        cases,
        tuple(selected),
        tuple(transition_cases),
        dict(projection_targets),
        semantic_fingerprint,
        projection_fingerprints,
    )


def load_contracts() -> Contracts:
    """Load and validate the committed V2 authority and shared manifest."""

    expected_manifest = ROOT / MANIFEST_RELATIVE
    expected_catalog = ROOT / CATALOG_RELATIVE
    manifest = _load_json(expected_manifest, "manifest")
    catalog = _load_json(expected_catalog, "catalog")
    manifest_value = _exact_object(
        manifest.value,
        {
            "format",
            "scope",
            "baseline",
            "seed_inventory",
            "canonical_case_catalog",
            "bindings",
            "implementation_order",
            "statuses",
            "proof_kinds",
            "proof_profiles",
            "binding_profiles",
            "owners",
            "capabilities",
            "seed_workflows",
            "non_operations",
        },
        "manifest",
    )
    if manifest_value["format"] != "typebridge.sdk-conformance/v1":
        raise ContractError("invalid_manifest_format", "manifest format is not v1")
    if manifest_value["proof_kinds"] != list(PROOF_KINDS):
        raise ContractError("invalid_proof_kinds", "manifest proof kinds are not frozen")

    (
        cases,
        selected,
        manifest_transition_cases,
        projection_targets,
        semantic_fingerprint,
        projection_fingerprints,
    ) = _validate_catalog(catalog.value, manifest_value)
    fixture = catalog.value["fixture"]
    journey_path = _contract_path(catalog.value["journey_path"], JOURNEY_RELATIVE, "journey path")
    report_schema_path = _contract_path(
        catalog.value["report_schema_path"],
        REPORT_SCHEMA_RELATIVE,
        "report schema path",
    )
    schema_path = _contract_path(fixture["schema_path"], SCHEMA_RELATIVE, "schema path")
    provider_path = _contract_path(
        fixture["provider_schema_path"],
        PROVIDER_SCHEMA_RELATIVE,
        "provider schema path",
    )
    journey = _load_json(journey_path, "journey")
    report_schema = _load_json(report_schema_path, "report schema")
    schema = LoadedJson(schema_path, schema_path.read_bytes(), None)
    provider_schema = LoadedJson(provider_path, provider_path.read_bytes(), None)
    observations = _validate_journey(journey.value)
    _validate_report_schema(report_schema.value)
    return Contracts(
        manifest=manifest,
        catalog=catalog,
        journey=journey,
        report_schema=report_schema,
        schema=schema,
        provider_schema=provider_schema,
        capabilities=tuple(manifest_value["capabilities"]),
        cases=cases,
        selected=selected,
        manifest_transition_cases=manifest_transition_cases,
        observations=observations,
        projection_targets=projection_targets,
        semantic_fingerprint=semantic_fingerprint,
        projection_fingerprints=projection_fingerprints,
    )


def _validate_report(report: LoadedJson, contracts: Contracts) -> dict[str, Any]:
    value = _exact_object(
        report.value,
        {"format", "binding", "manifest", "catalog", "fixture", "results"},
        f"report {report.path}",
    )
    if value["format"] != REPORT_FORMAT:
        raise ContractError("invalid_report_format", f"{report.path} has the wrong format")
    binding = _string(value["binding"], "report binding")
    if binding not in REPORT_BINDINGS:
        raise ContractError("unknown_report_binding", f"unknown report binding {binding!r}")
    _validate_source_identity(
        value["manifest"],
        expected_path=MANIFEST_RELATIVE,
        expected_sha256=contracts.manifest.sha256,
        label="manifest identity",
        digest_code="manifest_digest_mismatch",
    )
    _validate_source_identity(
        value["catalog"],
        expected_path=CATALOG_RELATIVE,
        expected_sha256=contracts.catalog.sha256,
        label="catalog identity",
        digest_code="catalog_digest_mismatch",
    )
    fixture = _exact_object(
        value["fixture"],
        {
            "id",
            "version",
            "semantic_profile",
            "schema",
            "provider_schema",
            "journey",
            "semantic_fingerprint",
            "projection_target",
            "projection_fingerprint",
        },
        "report fixture",
    )
    if fixture["id"] != "sdk-v2" or _integer(fixture["version"], "fixture version") != 2:
        raise ContractError("invalid_fixture_identity", f"{binding} report fixture is not v2")
    if fixture["semantic_profile"] != "typedb-3.12.1/v1":
        raise ContractError("semantic_profile_mismatch", f"{binding} report profile is invalid")
    for field, relative, loaded, code in (
        ("schema", SCHEMA_RELATIVE, contracts.schema, "schema_digest_mismatch"),
        (
            "provider_schema",
            PROVIDER_SCHEMA_RELATIVE,
            contracts.provider_schema,
            "provider_schema_digest_mismatch",
        ),
        ("journey", JOURNEY_RELATIVE, contracts.journey, "journey_digest_mismatch"),
    ):
        _validate_source_identity(
            fixture[field],
            expected_path=relative,
            expected_sha256=loaded.sha256,
            label=f"{field} identity",
            digest_code=code,
        )
    semantic_fingerprint = _validate_fingerprint(
        fixture["semantic_fingerprint"],
        label="semantic fingerprint",
        domain="typebridge.schema.semantic",
        canonicalization="typebridge.schema-canonical-json/v1",
    )
    if semantic_fingerprint != contracts.semantic_fingerprint:
        raise ContractError(
            "semantic_fingerprint_authority_mismatch",
            f"{binding} semantic fingerprint differs from the catalog",
        )
    target = contracts.projection_targets[binding]
    if fixture["projection_target"] != target:
        raise ContractError(
            "projection_target_mismatch",
            f"{binding} report target is not {target!r}",
        )
    projection_fingerprint = _validate_fingerprint(
        fixture["projection_fingerprint"],
        label="projection fingerprint",
        domain="typebridge.binding.projection",
        canonicalization="typebridge.binding-projection/v1",
    )
    if projection_fingerprint != contracts.projection_fingerprints[binding]:
        raise ContractError(
            "projection_fingerprint_authority_mismatch",
            f"{binding} projection fingerprint differs from the catalog",
        )

    results = _exact_list(value["results"], "report results")
    observed_keys: list[tuple[str, str]] = []
    normalized: list[dict[str, Any]] = []
    for index, result_value in enumerate(results):
        result = _exact_object(
            result_value,
            {"case_id", "capability_id", "proof_kind", "outcome", "observation"},
            f"report result {index}",
        )
        case_id = _string(result["case_id"], f"result {index} case ID")
        proof_kind = _string(result["proof_kind"], f"result {index} proof kind")
        if case_id not in contracts.cases:
            raise ContractError("unknown_result_case", f"unknown case {case_id!r}")
        if proof_kind not in PROOF_KINDS:
            raise ContractError("unknown_proof_kind", f"unknown proof {proof_kind!r}")
        capability = contracts.cases[case_id]["capability"]
        if result["capability_id"] != capability["id"]:
            raise ContractError("capability_mapping_mismatch", f"{case_id!r} maps incorrectly")
        profile = contracts.manifest.value["proof_profiles"][capability["proof_profile"]]
        if profile.get(proof_kind) != "required":
            raise ContractError(
                "proof_not_applicable",
                f"{case_id!r}/{proof_kind!r} is not required",
            )
        if result["outcome"] != "passed":
            raise ContractError("nonpassing_result", f"{case_id!r}/{proof_kind!r} did not pass")
        if type(result["observation"]) is not dict:
            raise ContractError("invalid_observation", "result observation must be an object")
        _inspect_observation(result["observation"], f"{binding} {case_id}/{proof_kind}")
        key = (case_id, proof_kind)
        if key in observed_keys:
            raise ContractError("duplicate_result", f"duplicate result {key!r}")
        observed_keys.append(key)
        normalized.append(result)

    if observed_keys != sorted(observed_keys):
        raise ContractError("result_order_mismatch", f"{binding} results are not sorted")
    expected_keys = [(case_id, proof_kind) for case_id, proof_kind, _ in contracts.selected]
    if observed_keys != expected_keys:
        raise ContractError(
            "result_coverage_mismatch",
            f"{binding} result keys differ from the selected proof set",
        )
    by_ref: dict[str, dict[str, Any]] = {}
    for result, (_, _, observation_ref) in zip(normalized, contracts.selected, strict=True):
        prior = by_ref.setdefault(observation_ref, result["observation"])
        if prior != result["observation"]:
            raise ContractError(
                "observation_ref_mismatch",
                f"{binding} rows for {observation_ref!r} differ",
            )
    for observation_ref, observation in by_ref.items():
        if observation != contracts.observations[observation_ref]:
            raise ContractError(
                "observation_mismatch",
                f"{binding} observation {observation_ref!r} differs from the journey",
            )
    return {
        "binding": binding,
        "semantic_fingerprint": semantic_fingerprint,
        "projection_target": target,
        "projection_fingerprint": projection_fingerprint,
        "results": normalized,
    }


def _derive_summary(reports: dict[str, dict[str, Any]], contracts: Contracts) -> dict[str, Any]:
    first = reports[REPORT_BINDINGS[0]]
    passed_proofs = [
        {
            "case_id": result["case_id"],
            "capability_id": result["capability_id"],
            "proof_kind": result["proof_kind"],
            "observation": result["observation"],
        }
        for result in first["results"]
    ]
    covered = {(item["case_id"], item["proof_kind"]) for item in passed_proofs}

    current_gaps: list[dict[str, Any]] = []
    uncovered: list[dict[str, Any]] = []
    pending_promotions: list[dict[str, Any]] = []
    manifest_transition_cases = set(contracts.manifest_transition_cases)
    for capability in contracts.capabilities:
        case_id = capability["case_ids"][0]
        statuses = _expand_binding_profile(
            contracts.manifest.value,
            capability["binding_profile"],
        )
        gap_bindings = [binding for binding in REPORT_BINDINGS if statuses.get(binding) == "gap"]
        if gap_bindings:
            reason = capability.get("gap_reason")
            if type(reason) is not str or not reason.strip():
                raise ContractError(
                    "missing_gap_reason",
                    f"manifest gap {capability['id']!r} has no reason",
                )
            current_gaps.append(
                {
                    "bindings": gap_bindings,
                    "capability_id": capability["id"],
                    "case_id": case_id,
                    "reason": reason,
                }
            )
        if case_id in manifest_transition_cases:
            pending_bindings = [
                {"binding": binding, "current_status": statuses[binding]}
                for binding in REPORT_BINDINGS
                if statuses[binding] != "accepted_live"
            ]
            if pending_bindings:
                pending_promotions.append(
                    {
                        "bindings": pending_bindings,
                        "capability_id": capability["id"],
                        "case_id": case_id,
                    }
                )
        profile = contracts.manifest.value["proof_profiles"][capability["proof_profile"]]
        for proof_kind in PROOF_KINDS:
            if profile[proof_kind] != "required" or (case_id, proof_kind) in covered:
                continue
            bindings = [
                binding
                for binding in REPORT_BINDINGS
                if statuses.get(binding) in {"accepted_offline", "accepted_live"}
            ]
            if bindings:
                uncovered.append(
                    {
                        "bindings": bindings,
                        "capability_id": capability["id"],
                        "case_id": case_id,
                        "proof_kind": proof_kind,
                    }
                )

    return {
        "format": SUMMARY_FORMAT,
        "bindings": list(REPORT_BINDINGS),
        "fixture": {
            "id": "sdk-v2",
            "version": 2,
            "semantic_profile": "typedb-3.12.1/v1",
            "manifest": {"path": MANIFEST_RELATIVE, "sha256": contracts.manifest.sha256},
            "catalog": {"path": CATALOG_RELATIVE, "sha256": contracts.catalog.sha256},
            "schema": {"path": SCHEMA_RELATIVE, "sha256": contracts.schema.sha256},
            "provider_schema": {
                "path": PROVIDER_SCHEMA_RELATIVE,
                "sha256": contracts.provider_schema.sha256,
            },
            "journey": {"path": JOURNEY_RELATIVE, "sha256": contracts.journey.sha256},
            "semantic_fingerprint": first["semantic_fingerprint"],
            "projection_fingerprints": {
                binding: {
                    "target": reports[binding]["projection_target"],
                    "fingerprint": reports[binding]["projection_fingerprint"],
                }
                for binding in REPORT_BINDINGS
            },
        },
        "passed_proofs": passed_proofs,
        "current_gaps": current_gaps,
        "pending_manifest_promotions": pending_promotions,
        "uncovered_required_proofs": uncovered,
    }


def compare_reports(report_paths: list[Path]) -> dict[str, Any]:
    contracts = load_contracts()
    return compare_report_set(
        report_paths,
        REPORT_BINDINGS,
        lambda path: _validate_report(
            _load_json(path, f"report {path}", require_canonical=True), contracts
        ),
        lambda reports: _derive_summary(reports, contracts),
    )


def _parser() -> argparse.ArgumentParser:
    return report_parser(__doc__, REPORT_BINDINGS)


def main(argv: list[str] | None = None) -> int:
    arguments = _parser().parse_args(argv)
    try:
        summary = compare_reports(arguments.reports)
    except (ContractError, OSError) as error:
        print(f"sdk-v2 conformance rejected: {error}", file=sys.stderr)
        return 1
    sys.stdout.buffer.write(canonical_json_bytes(summary))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
