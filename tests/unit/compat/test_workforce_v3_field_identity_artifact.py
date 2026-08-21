"""Hostile checks for the Workforce V3 field-name identity artifact."""

from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
VALIDATOR_PATH = ROOT / "scripts/ci/validate_workforce_v3_field_identity.py"


def _load_validator():
    spec = importlib.util.spec_from_file_location(
        "workforce_v3_field_identity_validator", VALIDATOR_PATH
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


validator = _load_validator()


def _artifact(root: Path) -> dict:
    return {
        "format": validator.FORMAT,
        "semantic_profile": validator.SEMANTIC_PROFILE,
        "producer": {
            "id": validator.PRODUCER_ID,
            "test_id": validator.TEST_ID,
            "sources": [
                {"path": path, "sha256": hashlib.sha256((root / path).read_bytes()).hexdigest()}
                for path in validator.SOURCE_PATHS
            ],
        },
        "result": {
            "observation_ref": "field_name_identity",
            "proof_kind": "artifact",
            "outcome": "passed",
            "observation": copy.deepcopy(validator.EXPECTED_OBSERVATION),
        },
    }


def _write(path: Path, value: dict) -> None:
    path.write_bytes(validator.canonical_json_bytes(value))


def test_field_identity_artifact_accepts_exact_source_bound_observation(tmp_path: Path) -> None:
    artifact = _artifact(ROOT)
    path = tmp_path / "field.json"
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
def test_field_identity_artifact_rejects_hostile_drift(
    tmp_path: Path, mutation: str, code: str
) -> None:
    artifact = _artifact(ROOT)
    if mutation == "source":
        artifact["producer"]["sources"][0]["sha256"] = "0" * 64
    elif mutation == "producer":
        artifact["producer"]["id"] = "untrusted"
    elif mutation == "lane":
        artifact["result"]["observation_ref"] = "atomic_multibinding_generation"
    elif mutation == "observation":
        artifact["result"]["observation"]["package_branded"] = False
    elif mutation == "extra":
        artifact["result"]["unexpected"] = True
    path = tmp_path / "hostile.json"
    _write(path, artifact)
    with pytest.raises(validator.ArtifactError) as rejected:
        validator.validate_artifact(path)
    assert rejected.value.code == code


def test_field_identity_artifact_rejects_noncanonical_json(tmp_path: Path) -> None:
    path = tmp_path / "pretty.json"
    path.write_text(json.dumps(_artifact(ROOT), indent=2) + "\n", encoding="utf-8")
    with pytest.raises(validator.ArtifactError) as rejected:
        validator.validate_artifact(path)
    assert rejected.value.code == "noncanonical_json"
