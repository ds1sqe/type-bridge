#!/usr/bin/env python3
"""Validate and compare the Python, Node, Rust, and C sdk-v3 reports."""

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
CATALOG_RELATIVE = "tests/contracts/sdk_conformance/sdk-v3/catalog-v3.json"
JOURNEY_RELATIVE = "tests/contracts/sdk_conformance/sdk-v3/journey-v3.json"
REPORT_SCHEMA_RELATIVE = "tests/contracts/sdk_conformance/sdk-v3/report-schema-v3.json"
SCHEMA_RELATIVE = "tests/contracts/sdk_conformance/sdk-v3/schema-v3.yaml"
PROVIDER_SCHEMA_RELATIVE = "tests/contracts/sdk_conformance/sdk-v3/provider-3.12.1-v3.tql"

REPORT_FORMAT = "typebridge.sdk-conformance-report/v3"
SUMMARY_FORMAT = "typebridge.sdk-conformance-summary/v3"
CATALOG_FORMAT = "typebridge.sdk-catalog/v3"
JOURNEY_FORMAT = "typebridge.sdk-journey/v3"
REPORT_BINDINGS = ("python", "node", "rust", "c")
CURRENT_BINDINGS = frozenset(REPORT_BINDINGS)
PROJECTION_TARGETS = {
    "python": "python",
    "node": "typescript",
    "rust": "rust",
    "c": "c",
}
REPORT_PRODUCERS = {
    "python": {
        "id": "python.generated-data-model-runtime-v3-live",
        "test_id": "python.generated_data_model_runtime_v3_live",
    },
    "node": {
        "id": "node.generated-data-model-runtime-v3-live",
        "test_id": "node.generated_data_model_runtime_v3_live",
    },
    "rust": {
        "id": "type-bridge-rust.generated-data-model-runtime-v3-live",
        "test_id": "rust_projection_live::generated_data_model_runtime_v3_live",
    },
    "c": {
        "id": "type-bridge-c.generated-data-model-runtime-v3-live",
        "test_id": "c_projection_live::generated_data_model_runtime_v3_live",
    },
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
    (
        "sdk.crud.entity-batch-insert-put",
        "direct_runtime",
        "entity_batch_insert_put",
    ),
    (
        "sdk.crud.entity-batch-update-delete",
        "direct_runtime",
        "entity_batch_update_delete_atomic",
    ),
    (
        "sdk.crud.relation-batch-insert-put",
        "direct_runtime",
        "relation_batch_insert_put",
    ),
    (
        "sdk.crud.relation-batch-update-delete",
        "direct_runtime",
        "relation_batch_update_delete_atomic",
    ),
    (
        "sdk.crud.unkeyed-entity",
        "direct_runtime",
        "unkeyed_entity_iid_lifecycle",
    ),
    (
        "sdk.crud.unkeyed-relation",
        "direct_runtime",
        "unkeyed_relation_iid_lifecycle",
    ),
    (
        "sdk.diagnostic.all-workflows",
        "diagnostic",
        "data_operation_structured_diagnostic",
    ),
    (
        "sdk.manager.filter",
        "direct_runtime",
        "manager_field_token_filter",
    ),
    (
        "sdk.model.constraints",
        "diagnostic",
        "projected_constraint_validation",
    ),
    (
        "sdk.model.inherited-relation-role",
        "direct_runtime",
        "inherited_relation_role_lifecycle",
    ),
    (
        "sdk.model.integer-key-polymorphic-role",
        "direct_runtime",
        "integer_key_polymorphic_role",
    ),
    (
        "sdk.projection.evidence-integrity",
        "diagnostic",
        "projection_evidence_integrity",
    ),
    (
        "sdk.projection.field-name-identity",
        "artifact",
        "field_name_identity",
    ),
    (
        "sdk.projection.token-package-fencing",
        "diagnostic",
        "token_package_fencing",
    ),
    (
        "sdk.runtime.cancellation",
        "direct_runtime",
        "data_operation_cancellation",
    ),
    (
        "sdk.runtime.connection-policy",
        "direct_runtime",
        "complete_connection_policy",
    ),
    (
        "sdk.runtime.explicit-close",
        "lifecycle",
        "data_resource_lifecycle",
    ),
    (
        "sdk.runtime.timeout-resource-limits",
        "direct_runtime",
        "data_operation_resource_limits",
    ),
    (
        "sdk.schema.generate.atomic",
        "artifact",
        "atomic_multibinding_generation",
    ),
    (
        "sdk.schema.ordered-distinct",
        "direct_runtime",
        "ordered_distinct_collections",
    ),
    (
        "sdk.transaction.borrowed",
        "lifecycle",
        "borrowed_transaction_lifecycle",
    ),
)
EXPECTED_MANIFEST_TRANSITION_CASES = (
    "sdk.schema.generate.atomic",
    "sdk.model.constraints",
    "sdk.crud.entity-batch-insert-put",
    "sdk.crud.relation-batch-insert-put",
    "sdk.crud.entity-batch-update-delete",
    "sdk.crud.relation-batch-update-delete",
    "sdk.transaction.borrowed",
    "sdk.manager.filter",
    "sdk.projection.field-name-identity",
    "sdk.model.integer-key-polymorphic-role",
    "sdk.model.inherited-relation-role",
    "sdk.crud.unkeyed-entity",
    "sdk.crud.unkeyed-relation",
    "sdk.projection.evidence-integrity",
    "sdk.projection.token-package-fencing",
    "sdk.schema.ordered-distinct",
    "sdk.runtime.connection-policy",
)
EVIDENCE_ONLY_CASES = frozenset(
    {
        "sdk.diagnostic.all-workflows",
        "sdk.runtime.cancellation",
        "sdk.runtime.explicit-close",
        "sdk.runtime.timeout-resource-limits",
    }
)
FINAL_DISTRIBUTION_SUCCESSOR_CASE = "sdk.distribution.standalone-cli"
EXPECTED_CREATE_ORDER = (
    "data-ada",
    "data-dana",
    "robot-7",
    "robot-negative-7",
    "counter-left",
    "counter-right",
    "membership-ada",
    "membership-robot",
    "link-forward",
    "link-return",
    "interaction-robot",
    "interaction-absent",
    "interaction-person",
    "plain-activity-ada",
    "event-ada",
    "container-event",
)
EXPECTED_PLAIN_ACTIVITY_RECORD = {
    "ref": "plain-activity-ada",
    "model": "plain-activity",
    "roles": {
        "participant": [
            {
                "model": "person",
                "key": "data-ada",
            }
        ]
    },
}

CONTRACT_NAME_RE = re.compile(r"^[a-z0-9][a-z0-9._/-]*$")
EXPECTED_OBSERVATION_REFS = frozenset(
    observation_ref for _, _, observation_ref in EXPECTED_SELECTED_PROOFS
)
EXPECTED_OBSERVATION_FIELDS = {
    "atomic_multibinding_generation": frozenset(
        {
            "targets",
            "common_authority_identity",
            "package_identities_distinct",
            "generated_sidecars",
            "no_sidecar_runtime_dependency",
            "deterministic_rerun",
            "injected_failure",
        }
    ),
    "borrowed_transaction_lifecycle": frozenset(
        {"read", "commit_visibility", "rollback_visibility", "poison", "post_rollback"}
    ),
    "complete_connection_policy": frozenset(
        {
            "endpoint_input",
            "database_input",
            "credentials",
            "http_probe",
            "trust",
            "compatibility",
            "deadline_clock",
            "resources_tighten_only",
            "configuration_rejections",
            "version_rejection",
            "diagnostics_redacted",
            "close_idempotent",
        }
    ),
    "data_operation_cancellation": frozenset(
        {"pre_dispatch", "owned_in_flight", "borrowed_in_flight", "after_commit"}
    ),
    "data_operation_resource_limits": frozenset(
        {
            "dimensions",
            "absolute_deadline_reused",
            "connection_ceiling_inherited",
            "operation_policy_tightens_only",
            "representative_crossing",
            "owned_failure",
            "borrowed_failure",
        }
    ),
    "data_operation_structured_diagnostic": frozenset(
        {
            "version",
            "representative",
            "provider_text_exposed",
            "secrets_exposed",
            "deterministic_field_order",
        }
    ),
    "data_resource_lifecycle": frozenset(
        {
            "resources",
            "close_contract",
            "parent_child",
            "session_rules",
            "result_survival",
            "cancellation_close_idempotent",
            "projected_value_close_idempotent",
            "projected_thing_close_idempotent",
        }
    ),
    "entity_batch_insert_put": frozenset(
        {"empty", "insert", "duplicate_key", "put", "late_failure"}
    ),
    "entity_batch_update_delete_atomic": frozenset(
        {"update", "duplicate_target", "delete_failure", "delete_success"}
    ),
    "field_name_identity": frozenset(
        {
            "generated_tokens",
            "token_identities_distinct",
            "package_branded",
            "compatibility_lookup",
            "generated_token_string_parser_used",
        }
    ),
    "inherited_relation_role_lifecycle": frozenset(
        {
            "model",
            "inherited_relation",
            "inherited_role",
            "player_model",
            "created",
            "read_after_create",
            "role_identity_preserved",
            "deleted",
            "read_after_delete",
            "count_after_delete",
        }
    ),
    "integer_key_polymorphic_role": frozenset(
        {
            "integer_keys",
            "relation",
            "optional_role",
            "polymorphic_players_observed",
            "present",
            "absent",
            "relation_as_player",
        }
    ),
    "manager_field_token_filter": frozenset(
        {
            "model",
            "field_token",
            "operator_literal",
            "operator_outcomes",
            "conjunction",
            "terminals",
            "first",
            "rejections",
            "borrowed_read",
        }
    ),
    "ordered_distinct_collections": frozenset(
        {
            "owns",
            "relates",
            "scalar_duplicate",
            "player_duplicate",
            "unordered_compatibility_default",
        }
    ),
    "projected_constraint_validation": frozenset(
        {
            "scalar_domains",
            "rejection_families",
            "provider_enforced_families",
            "representative_diagnostic",
        }
    ),
    "projection_evidence_integrity": frozenset(
        {
            "rejected_mutations",
            "representative_mutation",
            "diagnostic",
            "rejected_before_provider_io",
        }
    ),
    "relation_batch_insert_put": frozenset(
        {"empty", "insert", "duplicate_key", "put", "late_failure"}
    ),
    "relation_batch_update_delete_atomic": frozenset(
        {"update", "duplicate_target", "delete_failure", "delete_success"}
    ),
    "token_package_fencing": frozenset(
        {"accepted_local", "rejections", "rejected_token_states", "provider_text_exposed"}
    ),
    "unkeyed_entity_iid_lifecycle": frozenset(
        {
            "model",
            "identity_kind",
            "surface",
            "insert",
            "get_by_identity",
            "update_by_identity",
            "delete_by_identity",
            "count_after_cleanup",
        }
    ),
    "unkeyed_relation_iid_lifecycle": frozenset(
        {
            "model",
            "identity_kind",
            "surface",
            "insert",
            "get_by_identity",
            "update_by_identity",
            "delete_by_identity",
            "count_after_cleanup",
        }
    ),
}


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
    report_producers: dict[str, dict[str, str]]
    authority_state: str
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
        raise ContractError("invalid_journey_format", "journey format is not v3")
    if journey["fixture_id"] != "sdk-v3" or journey["version"] != 3:
        raise ContractError("invalid_fixture_identity", "journey fixture identity is not v3")
    if journey["semantic_profile"] != "typedb-3.12.1/v1":
        raise ContractError("semantic_profile_mismatch", "journey profile is not 3.12.1/v1")

    records = _exact_object(
        journey["records"],
        {
            "people",
            "robots",
            "counters",
            "memberships",
            "network_links",
            "interactions",
            "plain_activity",
            "event",
            "container",
        },
        "journey records",
    )
    expected_models = {
        "people": ("person", 2),
        "robots": ("robot", 2),
        "counters": ("counter", 2),
        "memberships": ("membership", 2),
        "network_links": ("network-link", 2),
        "interactions": ("interaction", 3),
    }
    observed_refs: list[str] = []
    for name, (model, count) in expected_models.items():
        items = _exact_list(records[name], f"journey {name}")
        if len(items) != count:
            raise ContractError(
                "invalid_journey_records",
                f"journey {name} must contain exactly {count} records",
            )
        if name in {"people", "robots", "counters"}:
            item_fields = {"ref", "model", "fields"}
        elif name == "memberships":
            item_fields = {"ref", "model", "roles"}
        else:
            item_fields = {"ref", "model", "fields", "roles"}
        for index, item_value in enumerate(items):
            item = _exact_object(item_value, item_fields, f"journey {name}[{index}]")
            if item["model"] != model:
                raise ContractError(
                    "invalid_journey_model",
                    f"journey {name}[{index}] has the wrong model",
                )
            observed_refs.append(_string(item["ref"], f"journey {name}[{index}] ref"))
    for name, model, item_fields in (
        ("plain_activity", "plain-activity", {"ref", "model", "roles"}),
        ("event", "event", {"ref", "model", "roles"}),
        ("container", "container", {"ref", "model", "roles"}),
    ):
        item = _exact_object(records[name], item_fields, f"journey {name}")
        if item["model"] != model:
            raise ContractError("invalid_journey_model", f"journey {name} has the wrong model")
        observed_refs.append(_string(item["ref"], f"journey {name} ref"))
    if records["plain_activity"] != EXPECTED_PLAIN_ACTIVITY_RECORD:
        raise ContractError(
            "invalid_journey_record",
            "journey plain_activity record is not the frozen inherited-role lifecycle",
        )

    create_order = _exact_list(journey["create_order"], "create order")
    cleanup_order = _exact_list(journey["cleanup_order"], "cleanup order")
    if tuple(create_order) != EXPECTED_CREATE_ORDER or set(observed_refs) != set(
        EXPECTED_CREATE_ORDER
    ):
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
        expected_fields = EXPECTED_OBSERVATION_FIELDS[name]
        if set(observation) != expected_fields:
            raise ContractError(
                "observation_shape_mismatch",
                f"observation {name!r} top-level fields are not frozen",
            )
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
    dict[str, dict[str, str]],
    str,
    dict[str, Any],
    dict[str, dict[str, Any]],
]:
    catalog = _exact_object(
        value,
        {
            "format",
            "authority_state",
            "fixture",
            "journey_path",
            "report_schema_path",
            "report_bindings",
            "projection_targets",
            "report_producers",
            "expected_fingerprints",
            "manifest_transition_cases",
            "selected_proofs",
            "cases",
        },
        "catalog",
    )
    if catalog["format"] != CATALOG_FORMAT:
        raise ContractError("invalid_catalog_format", "catalog format is not v3")
    authority_state = catalog["authority_state"]
    if authority_state != "finalized":
        raise ContractError("invalid_authority_state", "catalog authority state is unknown")
    fixture = _exact_object(
        catalog["fixture"],
        {"id", "version", "semantic_profile", "schema_path", "provider_schema_path"},
        "catalog fixture",
    )
    if fixture != {
        "id": "sdk-v3",
        "version": 3,
        "semantic_profile": "typedb-3.12.1/v1",
        "schema_path": SCHEMA_RELATIVE,
        "provider_schema_path": PROVIDER_SCHEMA_RELATIVE,
    }:
        raise ContractError("invalid_fixture_identity", "catalog fixture is not frozen v3")
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

    report_producers = _exact_object(
        catalog["report_producers"], set(REPORT_BINDINGS), "report producers"
    )
    for binding in REPORT_BINDINGS:
        producer = _exact_object(
            report_producers[binding], {"id", "test_id"}, f"{binding} report producer"
        )
        if producer != REPORT_PRODUCERS[binding]:
            raise ContractError(
                "invalid_report_producer",
                f"{binding} report producer identity is not frozen",
            )

    expected_fingerprints = catalog["expected_fingerprints"]
    expected_fingerprints = _exact_object(
        expected_fingerprints,
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
    for binding, value in projection_values.items():
        projection_fingerprints[binding] = _validate_fingerprint(
            value,
            label=f"expected {binding} projection fingerprint",
            domain="typebridge.binding.projection",
            canonicalization="typebridge.binding-projection/v1",
        )
    if projection_fingerprints and len(
        {item["digest"] for item in projection_fingerprints.values()}
    ) != len(REPORT_BINDINGS):
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
    if selected_case_ids - set(transition_cases) != EVIDENCE_ONLY_CASES:
        raise ContractError(
            "evidence_only_case_mismatch",
            "selected non-transition cases are not the frozen broad-gap set",
        )
    for case_id in EVIDENCE_ONLY_CASES:
        if cases[case_id]["capability"]["binding_profile"] not in {
            "current_gap_future_planned",
            "terminal_broad_live_future_planned",
        }:
            raise ContractError(
                "invalid_successor_profile",
                f"{case_id!r} successor profile drifted",
            )
    if cases[FINAL_DISTRIBUTION_SUCCESSOR_CASE]["capability"]["binding_profile"] not in {
        "current_gap_future_planned",
        "standalone_distribution_offline_future_planned",
    }:
        raise ContractError(
            "invalid_successor_profile",
            f"{FINAL_DISTRIBUTION_SUCCESSOR_CASE!r} successor profile drifted",
        )
    return (
        cases,
        tuple(selected),
        tuple(transition_cases),
        dict(projection_targets),
        {binding: dict(report_producers[binding]) for binding in REPORT_BINDINGS},
        authority_state,
        semantic_fingerprint,
        projection_fingerprints,
    )


def load_contracts() -> Contracts:
    """Load and validate the committed V3 authority and shared manifest."""

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
        report_producers,
        authority_state,
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
        report_producers=report_producers,
        authority_state=authority_state,
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
    if fixture["id"] != "sdk-v3" or _integer(fixture["version"], "fixture version") != 3:
        raise ContractError("invalid_fixture_identity", f"{binding} report fixture is not v3")
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
            "id": "sdk-v3",
            "version": 3,
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
        print(f"sdk-v3 conformance rejected: {error}", file=sys.stderr)
        return 1
    sys.stdout.buffer.write(canonical_json_bytes(summary))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
