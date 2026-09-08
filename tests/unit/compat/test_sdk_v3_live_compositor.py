"""Provider-free checks for compositional Sdk V3 live bundles."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
CI = ROOT / "scripts/ci"
COMPOSITOR_PATH = CI / "compose_sdk_v3_live_observations.py"

FRAGMENT_LANES = {
    ("complete_connection_policy", "direct_runtime"),
    ("data_operation_cancellation", "direct_runtime"),
    ("data_operation_resource_limits", "direct_runtime"),
    ("data_operation_structured_diagnostic", "diagnostic"),
    ("projected_constraint_validation", "diagnostic"),
    ("projection_evidence_integrity", "diagnostic"),
    ("token_package_fencing", "diagnostic"),
}


def _load_compositor():
    sys.path.insert(0, str(CI))
    spec = importlib.util.spec_from_file_location("sdk_v3_live_compositor", COMPOSITOR_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


compositor = _load_compositor()


def test_compositor_reuses_six_observed_lanes_and_requires_eight_supplement_lanes() -> None:
    contracts = compositor.conformance.load_contracts()
    binding = "rust"
    supplement_lanes = sorted(
        {(observation_ref, proof_kind) for _, proof_kind, observation_ref in contracts.selected}
        - FRAGMENT_LANES
        - compositor.REUSED_LANES
    )
    supplement = {
        "format": compositor.SUPPLEMENT_FORMAT,
        "binding": binding,
        "semantic_profile": compositor.assembler.SEMANTIC_PROFILE,
        "producer": contracts.report_producers[binding]["id"],
        "semantic_fingerprint": contracts.semantic_fingerprint,
        "projection_fingerprint": contracts.projection_fingerprints[binding],
        "results": [
            {
                "observation_ref": observation_ref,
                "proof_kind": proof_kind,
                "outcome": "passed",
                "observation": contracts.observations[observation_ref],
            }
            for observation_ref, proof_kind in supplement_lanes
        ],
    }
    parity = {
        "observations": {
            name: contracts.observations[name]
            for name in ("integer_key_polymorphic_role", "ordered_distinct_collections")
        }
    }
    inherited = contracts.observations["inherited_relation_role_lifecycle"]
    live = {
        "observations": {
            "inherited_plain_activity_role_lifecycle": {
                "count_after_delete": inherited["count_after_delete"],
                "created": inherited["created"],
                "deleted": inherited["deleted"],
                "inherited_relation": inherited["inherited_relation"],
                "model": inherited["model"],
                "participant": {"key": "data-ada", "model": inherited["player_model"]},
                "read_after_create": inherited["read_after_create"],
                "read_after_delete": inherited["read_after_delete"],
                "ref": "plain-activity-ada",
                "role": inherited["inherited_role"],
                "role_identity_preserved": inherited["role_identity_preserved"],
            }
        }
    }
    manager = {"observation": contracts.observations["manager_field_token_filter"]}

    bundle = compositor.compose_bundle(
        supplement,
        binding=binding,
        contracts=contracts,
        parity=parity,
        live=live,
        manager=manager,
        atomic_generation_observation=contracts.observations["atomic_multibinding_generation"],
        field_identity_observation=contracts.observations["field_name_identity"],
    )

    assert len(supplement_lanes) == 8
    assert len(bundle["results"]) == 14
    observations = compositor.assembler._live_observations(
        bundle,
        binding=binding,
        contracts=contracts,
        fragment_lanes=FRAGMENT_LANES,
    )
    assert {lane: observation for lane, observation in observations.items()} == {
        (observation_ref, proof_kind): contracts.observations[observation_ref]
        for _, proof_kind, observation_ref in contracts.selected
        if (observation_ref, proof_kind) not in FRAGMENT_LANES
    }

    supplement["format"] = compositor.assembler.LIVE_FORMAT
    with pytest.raises(compositor.assembler.AssemblyError) as rejected:
        compositor.compose_bundle(
            supplement,
            binding=binding,
            contracts=contracts,
            parity=parity,
            live=live,
            manager=manager,
            atomic_generation_observation=contracts.observations["atomic_multibinding_generation"],
            field_identity_observation=contracts.observations["field_name_identity"],
        )
    assert rejected.value.code == "invalid_supplement_format"


def test_compositor_source_does_not_read_expected_journey_objects() -> None:
    source = COMPOSITOR_PATH.read_text(encoding="utf-8")

    assert "expected_observations" not in source
    assert "contracts.observations" not in source
