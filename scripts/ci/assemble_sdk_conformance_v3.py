#!/usr/bin/env python3
"""Compose measured evidence and assemble SDK V3 reports."""

from __future__ import annotations

import argparse
import copy
import json
import stat
import sys
from pathlib import Path
from typing import Any

import compare_manager_filter_live as manager_comparator
import compare_projected_live as projected_live
import compare_projected_parity as projected_parity
import compare_sdk_conformance_v3 as conformance
import proof_fragments as fragments
import validate_generation_artifact as generation
from persist_binding_reports import publish_bytes

ROOT = Path(__file__).resolve().parents[2]
LIVE_FORMAT = "typebridge.sdk-v3-live-observations/v1"
REPORT_FORMAT = "typebridge.sdk-conformance-report/v3"
SEMANTIC_PROFILE = "typedb-3.12.1/v1"
MAX_LIVE_BYTES = 512 * 1024
MAX_REPORT_BYTES = 1024 * 1024
SUPPLEMENT_FORMAT = "typebridge.sdk-v3-live-supplement/v1"
REUSED_LANES = {
    ("atomic_multibinding_generation", "artifact"),
    ("field_name_identity", "artifact"),
    ("inherited_relation_role_lifecycle", "direct_runtime"),
    ("integer_key_polymorphic_role", "direct_runtime"),
    ("manager_field_token_filter", "direct_runtime"),
    ("ordered_distinct_collections", "direct_runtime"),
}


class AssemblyError(ValueError):
    """A stable fail-closed V3 report-assembly rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code


def _exact_object(value: Any, keys: set[str], label: str) -> dict[str, Any]:
    if type(value) is not dict:
        raise AssemblyError("invalid_object", f"{label} must be an object")
    actual = set(value)
    if actual != keys:
        raise AssemblyError(
            "invalid_object_fields",
            f"{label} fields differ; missing={sorted(keys - actual)}, extra={sorted(actual - keys)}",
        )
    return value


def _regular_bytes(path: Path, label: str, limit: int) -> bytes:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise AssemblyError("invalid_input_file", f"{label} cannot be inspected") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        raise AssemblyError("invalid_input_file", f"{label} must be a regular non-symlink file")
    if metadata.st_size > limit:
        raise AssemblyError("input_size_limit", f"{label} exceeds {limit} bytes")
    try:
        raw = path.read_bytes()
    except OSError as error:
        raise AssemblyError("invalid_input_file", f"{label} cannot be read") from error
    if len(raw) > limit:
        raise AssemblyError("input_size_limit", f"{label} exceeds {limit} bytes")
    return raw


def _load_live(path: Path) -> dict[str, Any]:
    raw = _regular_bytes(path, "live observation bundle", MAX_LIVE_BYTES)
    try:
        value = json.loads(
            raw.decode("utf-8"),
            object_pairs_hook=conformance._duplicate_key_object,
            parse_constant=lambda value: (_ for _ in ()).throw(
                AssemblyError("invalid_json_number", f"non-finite number {value!r}")
            ),
        )
    except AssemblyError:
        raise
    except (UnicodeDecodeError, json.JSONDecodeError, RecursionError) as error:
        raise AssemblyError("malformed_live_bundle", "live bundle is not bounded JSON") from error
    if raw != conformance.canonical_json_bytes(value):
        raise AssemblyError(
            "noncanonical_live_bundle",
            "live bundle must be compact key-sorted JSON with one trailing newline",
        )
    return _exact_object(
        value,
        {
            "format",
            "binding",
            "semantic_profile",
            "producer",
            "semantic_fingerprint",
            "projection_fingerprint",
            "results",
        },
        "live observation bundle",
    )


def _live_observations(
    live: dict[str, Any],
    *,
    binding: str,
    contracts: conformance.Contracts,
    fragment_lanes: set[tuple[str, str]],
) -> dict[tuple[str, str], dict[str, Any]]:
    if live["format"] != LIVE_FORMAT:
        raise AssemblyError("invalid_live_format", "live bundle format is not V3")
    if live["binding"] != binding or live["semantic_profile"] != SEMANTIC_PROFILE:
        raise AssemblyError("live_identity_mismatch", "live binding or semantic profile differs")
    if live["producer"] != contracts.report_producers[binding]["id"]:
        raise AssemblyError("producer_identity_mismatch", "live producer identity is not frozen")
    if live["semantic_fingerprint"] != contracts.semantic_fingerprint:
        raise AssemblyError("semantic_fingerprint_mismatch", "live semantic fingerprint drifted")
    if live["projection_fingerprint"] != contracts.projection_fingerprints[binding]:
        raise AssemblyError(
            "projection_fingerprint_mismatch", "live projection fingerprint drifted"
        )
    if type(live["results"]) is not list:
        raise AssemblyError("invalid_results", "live results must be an array")
    expected = {
        (observation_ref, proof_kind) for _, proof_kind, observation_ref in contracts.selected
    } - fragment_lanes
    observations: dict[tuple[str, str], dict[str, Any]] = {}
    ordered_lanes: list[tuple[str, str]] = []
    for index, value in enumerate(live["results"]):
        result = _exact_object(
            value,
            {"observation_ref", "proof_kind", "outcome", "observation"},
            f"live results[{index}]",
        )
        lane = (result["observation_ref"], result["proof_kind"])
        if lane not in expected:
            raise AssemblyError("unexpected_live_lane", f"live result carries {lane!r}")
        if result["outcome"] != "passed" or type(result["observation"]) is not dict:
            raise AssemblyError("invalid_live_result", f"live result {lane!r} did not pass")
        if lane in observations:
            raise AssemblyError("duplicate_live_lane", f"live result {lane!r} occurs twice")
        observations[lane] = result["observation"]
        ordered_lanes.append(lane)
    if ordered_lanes != sorted(ordered_lanes):
        raise AssemblyError("unordered_live_results", "live results must be lane-sorted")
    if set(observations) != expected:
        raise AssemblyError("live_lane_coverage_mismatch", "live results do not cover exact lanes")
    return observations


def _source_identity(relative: str, loaded: conformance.LoadedJson) -> dict[str, str]:
    return {"path": relative, "sha256": loaded.sha256}


def assemble_report(
    live: dict[str, Any],
    *,
    binding: str,
    contracts: conformance.Contracts,
    proof_observations: dict[tuple[str, str], dict[str, Any]],
) -> dict[str, Any]:
    live_observations = _live_observations(
        live, binding=binding, contracts=contracts, fragment_lanes=set(proof_observations)
    )
    observations = {**live_observations, **proof_observations}
    selected_lanes = {
        (observation_ref, proof_kind) for _, proof_kind, observation_ref in contracts.selected
    }
    if set(observations) != selected_lanes:
        raise AssemblyError("lane_coverage_mismatch", "combined observations are incomplete")
    results = []
    for case_id, proof_kind, observation_ref in contracts.selected:
        results.append(
            {
                "case_id": case_id,
                "capability_id": contracts.cases[case_id]["capability"]["id"],
                "proof_kind": proof_kind,
                "outcome": "passed",
                "observation": observations[observation_ref, proof_kind],
            }
        )
    return {
        "format": REPORT_FORMAT,
        "binding": binding,
        "manifest": _source_identity(conformance.MANIFEST_RELATIVE, contracts.manifest),
        "catalog": _source_identity(conformance.CATALOG_RELATIVE, contracts.catalog),
        "fixture": {
            "id": "sdk-v3",
            "version": 3,
            "semantic_profile": SEMANTIC_PROFILE,
            "schema": _source_identity(conformance.SCHEMA_RELATIVE, contracts.schema),
            "provider_schema": _source_identity(
                conformance.PROVIDER_SCHEMA_RELATIVE, contracts.provider_schema
            ),
            "journey": _source_identity(conformance.JOURNEY_RELATIVE, contracts.journey),
            "semantic_fingerprint": contracts.semantic_fingerprint,
            "projection_target": contracts.projection_targets[binding],
            "projection_fingerprint": contracts.projection_fingerprints[binding],
        },
        "results": results,
    }


def _publish(path: Path, report: dict[str, Any]) -> None:
    publish_bytes(
        path, conformance.canonical_json_bytes(report), AssemblyError, maximum=MAX_REPORT_BYTES
    )


def _validated_feature_reports(
    parity_path: Path, live_path: Path, manager_path: Path, *, binding: str
) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any]]:
    parity_binding, parity = projected_parity._load_report(
        parity_path, projected_parity.load_contract()
    )
    live_binding, live = projected_live._load_report(live_path, projected_live.load_contract())
    manager_binding, manager = manager_comparator._load_report(
        manager_path, manager_comparator.load_contract()
    )
    if {parity_binding, live_binding, manager_binding} != {binding}:
        raise AssemblyError(
            "component_binding_mismatch", "feature reports do not share the requested binding"
        )
    return (parity, live, manager)


def _inherited_observation(value: dict[str, Any]) -> dict[str, Any]:
    return {
        "model": value["model"],
        "inherited_relation": value["inherited_relation"],
        "inherited_role": value["role"],
        "player_model": value["participant"]["model"],
        "created": value["created"],
        "read_after_create": value["read_after_create"],
        "role_identity_preserved": value["role_identity_preserved"],
        "deleted": value["deleted"],
        "read_after_delete": value["read_after_delete"],
        "count_after_delete": value["count_after_delete"],
    }


def compose_bundle(
    supplement: dict[str, Any],
    *,
    binding: str,
    contracts: conformance.Contracts,
    parity: dict[str, Any],
    live: dict[str, Any],
    manager: dict[str, Any],
    atomic_generation_observation: dict[str, Any],
    field_identity_observation: dict[str, Any],
) -> dict[str, Any]:
    if supplement["format"] != SUPPLEMENT_FORMAT:
        raise AssemblyError("invalid_supplement_format", "supplement format is not V3")
    normalized = copy.deepcopy(supplement)
    normalized["format"] = LIVE_FORMAT
    observations = _live_observations(
        normalized,
        binding=binding,
        contracts=contracts,
        fragment_lanes=REUSED_LANES
        | {
            ("complete_connection_policy", "direct_runtime"),
            ("data_operation_cancellation", "direct_runtime"),
            ("data_operation_resource_limits", "direct_runtime"),
            ("data_operation_structured_diagnostic", "diagnostic"),
            ("projected_constraint_validation", "diagnostic"),
            ("projection_evidence_integrity", "diagnostic"),
            ("token_package_fencing", "diagnostic"),
        },
    )
    observations.update(
        {
            ("inherited_relation_role_lifecycle", "direct_runtime"): _inherited_observation(
                live["observations"]["inherited_plain_activity_role_lifecycle"]
            ),
            ("integer_key_polymorphic_role", "direct_runtime"): copy.deepcopy(
                parity["observations"]["integer_key_polymorphic_role"]
            ),
            ("manager_field_token_filter", "direct_runtime"): copy.deepcopy(manager["observation"]),
            ("ordered_distinct_collections", "direct_runtime"): copy.deepcopy(
                parity["observations"]["ordered_distinct_collections"]
            ),
            ("atomic_multibinding_generation", "artifact"): copy.deepcopy(
                atomic_generation_observation
            ),
            ("field_name_identity", "artifact"): copy.deepcopy(field_identity_observation),
        }
    )
    selected_live = {
        (observation_ref, proof_kind) for _, proof_kind, observation_ref in contracts.selected
    } - {
        ("complete_connection_policy", "direct_runtime"),
        ("data_operation_cancellation", "direct_runtime"),
        ("data_operation_resource_limits", "direct_runtime"),
        ("data_operation_structured_diagnostic", "diagnostic"),
        ("projected_constraint_validation", "diagnostic"),
        ("projection_evidence_integrity", "diagnostic"),
        ("token_package_fencing", "diagnostic"),
    }
    if set(observations) != selected_live:
        raise AssemblyError("composed_lane_coverage_mismatch", "composed live lanes are not exact")
    return {
        key: copy.deepcopy(supplement[key])
        for key in (
            "binding",
            "producer",
            "projection_fingerprint",
            "semantic_fingerprint",
            "semantic_profile",
        )
    } | {
        "format": LIVE_FORMAT,
        "results": [
            {
                "observation_ref": observation_ref,
                "proof_kind": proof_kind,
                "outcome": "passed",
                "observation": observation,
            }
            for (observation_ref, proof_kind), observation in sorted(observations.items())
        ],
    }


def compose_inputs(arguments: argparse.Namespace) -> dict[str, Any]:
    contracts = conformance.load_contracts()
    parity, live, manager = _validated_feature_reports(
        arguments.projected_parity,
        arguments.projected_live,
        arguments.manager,
        binding=arguments.binding,
    )
    supplement = _load_live(arguments.supplement)
    atomic_generation_observation = generation.validate_artifact(
        arguments.atomic_generation, "atomic"
    )
    field_identity_observation = generation.validate_artifact(
        arguments.field_identity, "field-identity"
    )
    bundle = compose_bundle(
        supplement,
        binding=arguments.binding,
        contracts=contracts,
        parity=parity,
        live=live,
        manager=manager,
        atomic_generation_observation=atomic_generation_observation,
        field_identity_observation=field_identity_observation,
    )
    if len(conformance.canonical_json_bytes(bundle)) > MAX_LIVE_BYTES:
        raise AssemblyError("input_size_limit", "composed evidence exceeds its byte limit")
    return bundle


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binding", required=True, choices=conformance.REPORT_BINDINGS)
    parser.add_argument("--live-observations", required=False, type=Path)
    parser.add_argument("--run-nonce", required=True)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("fragments", nargs="+", type=Path)
    parser.add_argument("--projected-parity", required=False, type=Path)
    parser.add_argument("--projected-live", required=False, type=Path)
    parser.add_argument("--manager", required=False, type=Path)
    parser.add_argument("--atomic-generation", required=False, type=Path)
    parser.add_argument("--field-identity", required=False, type=Path)
    parser.add_argument("--supplement", required=False, type=Path)
    arguments = parser.parse_args(argv)
    if arguments.live_observations is None:
        missing = [
            name
            for name in (
                "projected_parity",
                "projected_live",
                "manager",
                "atomic_generation",
                "field_identity",
                "supplement",
            )
            if getattr(arguments, name) is None
        ]
        if missing:
            parser.error(
                "composition requires "
                + ", ".join("--" + name.replace("_", "-") for name in missing)
            )
    elif any(
        getattr(arguments, name) is not None
        for name in (
            "projected_parity",
            "projected_live",
            "manager",
            "atomic_generation",
            "field_identity",
            "supplement",
        )
    ):
        parser.error("choose existing evidence or composition inputs, not both")
    try:
        contracts = conformance.load_contracts()
        proof_observations = fragments.CONTRACTS[3].load_proof_fragments(
            arguments.fragments,
            expected_binding=arguments.binding,
            run_nonce=arguments.run_nonce,
            root=ROOT,
        )
        live = (
            compose_inputs(arguments)
            if arguments.live_observations is None
            else _load_live(arguments.live_observations)
        )
        report = assemble_report(
            live,
            binding=arguments.binding,
            contracts=contracts,
            proof_observations=proof_observations,
        )
        conformance._validate_report(
            conformance.LoadedJson(
                path=arguments.output, raw=conformance.canonical_json_bytes(report), value=report
            ),
            contracts,
        )
        _publish(arguments.output, report)
    except (
        AssemblyError,
        conformance.ContractError,
        fragments.ProofFragmentError,
        projected_live.ContractError,
        projected_parity.ContractError,
        manager_comparator.ContractError,
        generation.ArtifactError,
    ) as error:
        print(f"sdk-v3 report assembly rejected: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
