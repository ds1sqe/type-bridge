#!/usr/bin/env python3
"""Validate the source-bound Workforce V3 atomic-generation artifact."""

from __future__ import annotations

import argparse
import hashlib
import json
import stat
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
FORMAT = "typebridge.workforce-v3-artifact-observation/v1"
SEMANTIC_PROFILE = "typedb-3.12.1/v1"
SOURCE_PATH = "type-bridge-core/crates/cli/src/lib.rs"
PRODUCER_ID = "type-bridge-cli.atomic-multibinding-v3-artifact"
TEST_ID = (
    "schema_generation_atomicity_tests::"
    "injected_c_emitter_failure_preserves_all_four_ordered_packages"
)
MAX_ARTIFACT_BYTES = 64 * 1024


class ArtifactError(ValueError):
    """A stable fail-closed atomic-generation artifact rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code


def canonical_json_bytes(value: Any) -> bytes:
    try:
        encoded = json.dumps(
            value,
            allow_nan=False,
            ensure_ascii=False,
            separators=(",", ":"),
            sort_keys=True,
        )
    except (TypeError, ValueError, RecursionError) as error:
        raise ArtifactError("invalid_json_value", "artifact cannot be canonicalized") from error
    return f"{encoded}\n".encode()


def _duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise ArtifactError("duplicate_json_key", f"duplicate JSON key {key!r}")
        value[key] = item
    return value


def _exact_object(value: Any, keys: set[str], label: str) -> dict[str, Any]:
    if type(value) is not dict:
        raise ArtifactError("invalid_object", f"{label} must be an object")
    actual = set(value)
    if actual != keys:
        raise ArtifactError(
            "invalid_object_fields",
            f"{label} fields differ; missing={sorted(keys - actual)}, "
            f"extra={sorted(actual - keys)}",
        )
    return value


def _regular_bytes(path: Path, label: str, limit: int) -> bytes:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise ArtifactError("invalid_file", f"{label} cannot be inspected") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        raise ArtifactError("invalid_file", f"{label} must be a regular non-symlink file")
    if metadata.st_size > limit:
        raise ArtifactError("file_size_limit", f"{label} exceeds {limit} bytes")
    try:
        raw = path.read_bytes()
    except OSError as error:
        raise ArtifactError("invalid_file", f"{label} cannot be read") from error
    if len(raw) > limit:
        raise ArtifactError("file_size_limit", f"{label} exceeds {limit} bytes")
    return raw


def validate_artifact(path: Path, *, root: Path = ROOT) -> dict[str, Any]:
    raw = _regular_bytes(path, "atomic-generation artifact", MAX_ARTIFACT_BYTES)
    try:
        artifact = json.loads(raw.decode(), object_pairs_hook=_duplicate_keys)
    except ArtifactError:
        raise
    except (UnicodeDecodeError, json.JSONDecodeError, RecursionError) as error:
        raise ArtifactError("malformed_json", "artifact is not bounded UTF-8 JSON") from error
    if raw != canonical_json_bytes(artifact):
        raise ArtifactError("noncanonical_json", "artifact JSON is not canonical")
    artifact = _exact_object(
        artifact,
        {"format", "semantic_profile", "producer", "result"},
        "artifact",
    )
    if artifact["format"] != FORMAT or artifact["semantic_profile"] != SEMANTIC_PROFILE:
        raise ArtifactError("artifact_identity_mismatch", "artifact format or profile drifted")

    producer = _exact_object(artifact["producer"], {"id", "test_id", "source"}, "producer")
    if producer["id"] != PRODUCER_ID or producer["test_id"] != TEST_ID:
        raise ArtifactError("producer_identity_mismatch", "artifact producer identity drifted")
    source = _exact_object(producer["source"], {"path", "sha256"}, "producer source")
    if source["path"] != SOURCE_PATH:
        raise ArtifactError("source_path_mismatch", "artifact source path drifted")
    current_source = _regular_bytes(root / SOURCE_PATH, "atomic-generation source", 2 * 1024 * 1024)
    if source["sha256"] != hashlib.sha256(current_source).hexdigest():
        raise ArtifactError("source_digest_mismatch", "artifact source digest is stale")

    result = _exact_object(
        artifact["result"],
        {"observation_ref", "proof_kind", "outcome", "observation"},
        "result",
    )
    if (
        result["observation_ref"] != "atomic_multibinding_generation"
        or result["proof_kind"] != "artifact"
        or result["outcome"] != "passed"
    ):
        raise ArtifactError("result_identity_mismatch", "artifact result lane drifted")
    observation = _exact_object(
        result["observation"],
        {
            "targets",
            "common_authority_identity",
            "package_identities_distinct",
            "generated_sidecars",
            "no_sidecar_runtime_dependency",
            "deterministic_rerun",
            "injected_failure",
        },
        "atomic-generation observation",
    )
    expected = {
        "targets": ["python", "typescript", "rust", "c"],
        "common_authority_identity": {
            "schema_source_equal": True,
            "semantic_profile": SEMANTIC_PROFILE,
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
    }
    if observation != expected:
        raise ArtifactError("observation_mismatch", "atomic-generation observation drifted")
    return observation


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifact", type=Path)
    parser.add_argument("--observation-json", action="store_true")
    arguments = parser.parse_args(argv)
    try:
        observation = validate_artifact(arguments.artifact)
    except ArtifactError as error:
        print(f"Workforce V3 atomic-generation artifact rejected: {error}", file=sys.stderr)
        return 1
    if arguments.observation_json:
        sys.stdout.buffer.write(canonical_json_bytes(observation))
    else:
        print("validated Workforce V3 atomic-generation artifact")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
