#!/usr/bin/env python3
"""Compose one exact V3 live bundle from validated feature reports and a supplement."""

from __future__ import annotations

import argparse
import copy
import sys
from pathlib import Path
from typing import Any

import assemble_sdk_conformance_v3 as assembler
import compare_manager_filter_live as manager_comparator
import compare_projected_live as projected_live
import compare_projected_parity as projected_parity
import compare_sdk_conformance_v3 as conformance
import validate_sdk_v3_atomic_generation as atomic_generation
import validate_sdk_v3_field_identity as field_identity

SUPPLEMENT_FORMAT = "typebridge.sdk-v3-live-supplement/v1"
REUSED_LANES = {
    ("atomic_multibinding_generation", "artifact"),
    ("field_name_identity", "artifact"),
    ("inherited_relation_role_lifecycle", "direct_runtime"),
    ("integer_key_polymorphic_role", "direct_runtime"),
    ("manager_field_token_filter", "direct_runtime"),
    ("ordered_distinct_collections", "direct_runtime"),
}


def _validated_feature_reports(
    parity_path: Path,
    live_path: Path,
    manager_path: Path,
    *,
    binding: str,
) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any]]:
    parity_binding, parity = projected_parity._load_report(
        parity_path, projected_parity.load_contract()
    )
    live_binding, live = projected_live._load_report(live_path, projected_live.load_contract())
    manager_binding, manager = manager_comparator._load_report(
        manager_path, manager_comparator.load_contract()
    )
    if {parity_binding, live_binding, manager_binding} != {binding}:
        raise assembler.AssemblyError(
            "component_binding_mismatch", "feature reports do not share the requested binding"
        )
    return parity, live, manager


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
        raise assembler.AssemblyError("invalid_supplement_format", "supplement format is not V3")
    normalized = copy.deepcopy(supplement)
    normalized["format"] = assembler.LIVE_FORMAT
    observations = assembler._live_observations(
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
        raise assembler.AssemblyError(
            "composed_lane_coverage_mismatch", "composed live lanes are not exact"
        )
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
        "format": assembler.LIVE_FORMAT,
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


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binding", required=True, choices=conformance.REPORT_BINDINGS)
    parser.add_argument("--projected-parity", required=True, type=Path)
    parser.add_argument("--projected-live", required=True, type=Path)
    parser.add_argument("--manager", required=True, type=Path)
    parser.add_argument("--atomic-generation", required=True, type=Path)
    parser.add_argument("--field-identity", required=True, type=Path)
    parser.add_argument("--supplement", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args(argv)
    try:
        contracts = conformance.load_contracts()
        parity, live, manager = _validated_feature_reports(
            arguments.projected_parity,
            arguments.projected_live,
            arguments.manager,
            binding=arguments.binding,
        )
        supplement = assembler._load_live(arguments.supplement)
        atomic_generation_observation = atomic_generation.validate_artifact(
            arguments.atomic_generation
        )
        field_identity_observation = field_identity.validate_artifact(arguments.field_identity)
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
        assembler._publish(arguments.output, bundle)
    except (
        assembler.AssemblyError,
        conformance.ContractError,
        projected_live.ContractError,
        projected_parity.ContractError,
        manager.ContractError,
        atomic_generation.ArtifactError,
        field_identity.ArtifactError,
    ) as error:
        print(f"sdk-v3 live composition rejected: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
