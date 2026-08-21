"""Hostile checks for the Workforce V3 atomic-generation artifact."""

from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
VALIDATOR_PATH = ROOT / "scripts/ci/validate_workforce_v3_atomic_generation.py"


def _load_validator():
    spec = importlib.util.spec_from_file_location(
        "workforce_v3_atomic_generation_validator", VALIDATOR_PATH
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


validator = _load_validator()


def _artifact(root: Path) -> dict:
    source = root / validator.SOURCE_PATH
    return {
        "format": validator.FORMAT,
        "semantic_profile": validator.SEMANTIC_PROFILE,
        "producer": {
            "id": validator.PRODUCER_ID,
            "test_id": validator.TEST_ID,
            "source": {
                "path": validator.SOURCE_PATH,
                "sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
            },
        },
        "result": {
            "observation_ref": "atomic_multibinding_generation",
            "proof_kind": "artifact",
            "outcome": "passed",
            "observation": {
                "targets": ["python", "typescript", "rust", "c"],
                "common_authority_identity": {
                    "schema_source_equal": True,
                    "semantic_profile": validator.SEMANTIC_PROFILE,
                    "semantic_fingerprint_equal": True,
                    "resource_ledger_equal": True,
                },
                "package_identities_distinct": True,
                "generated_sidecars": [],
                "no_sidecar_runtime_dependency": True,
                "deterministic_rerun": {"byte_identical": True, "published_targets": 4},
                "injected_failure": {
                    "failed_target": "c",
                    "published_targets": 0,
                    "previous_outputs_unchanged": True,
                    "staging_artifacts_remaining": 0,
                },
            },
        },
    }


def _write(path: Path, value: dict) -> None:
    path.write_bytes(validator.canonical_json_bytes(value))


def test_atomic_generation_artifact_accepts_exact_source_bound_observation(tmp_path: Path) -> None:
    path = tmp_path / "atomic.json"
    artifact = _artifact(ROOT)
    _write(path, artifact)

    assert validator.validate_artifact(path) == artifact["result"]["observation"]


@pytest.mark.parametrize(
    ("mutation", "code"),
    [
        ("source", "source_digest_mismatch"),
        ("producer", "producer_identity_mismatch"),
        ("lane", "result_identity_mismatch"),
        ("observation", "observation_mismatch"),
        ("extra", "invalid_object_fields"),
    ],
)
def test_atomic_generation_artifact_rejects_hostile_drift(
    tmp_path: Path, mutation: str, code: str
) -> None:
    artifact = copy.deepcopy(_artifact(ROOT))
    if mutation == "source":
        artifact["producer"]["source"]["sha256"] = "0" * 64
    elif mutation == "producer":
        artifact["producer"]["id"] = "type-bridge-cli.untrusted"
    elif mutation == "lane":
        artifact["result"]["observation_ref"] = "field_name_identity"
    elif mutation == "observation":
        artifact["result"]["observation"]["injected_failure"]["published_targets"] = 3
    elif mutation == "extra":
        artifact["result"]["unexpected"] = True
    else:
        raise AssertionError(mutation)
    path = tmp_path / "hostile.json"
    _write(path, artifact)

    with pytest.raises(validator.ArtifactError) as rejected:
        validator.validate_artifact(path)
    assert rejected.value.code == code


def test_atomic_generation_artifact_rejects_noncanonical_json(tmp_path: Path) -> None:
    path = tmp_path / "pretty.json"
    path.write_text(json.dumps(_artifact(ROOT), indent=2) + "\n", encoding="utf-8")

    with pytest.raises(validator.ArtifactError) as rejected:
        validator.validate_artifact(path)
    assert rejected.value.code == "noncanonical_json"
