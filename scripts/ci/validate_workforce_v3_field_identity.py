#!/usr/bin/env python3
"""Validate the source-bound Workforce V3 field-name identity artifact."""

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
PRODUCER_ID = "type-bridge-schema-codegen.field-name-v3-artifact"
TEST_ID = "exact_workforce_v3_field_name_identity_is_source_bound"
SOURCE_PATHS = [
    "type-bridge-core/crates/schema-codegen/tests/workforce_v3_fingerprints.rs",
    "type-bridge-core/crates/contract/src/projection.rs",
]
MAX_ARTIFACT_BYTES = 64 * 1024


class ArtifactError(ValueError):
    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code


def canonical_json_bytes(value: Any) -> bytes:
    try:
        encoded = json.dumps(
            value, allow_nan=False, ensure_ascii=False, separators=(",", ":"), sort_keys=True
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
    if set(value) != keys:
        raise ArtifactError("invalid_object_fields", f"{label} fields differ")
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


EXPECTED_OBSERVATION = {
    "generated_tokens": [
        {
            "binding_name": "foo__bar",
            "canonical_owner": "entity:person",
            "canonical_attribute": "attribute:foo__bar",
            "owns_fact": "person:foo__bar",
        },
        {
            "binding_name": "score__gte",
            "canonical_owner": "entity:person",
            "canonical_attribute": "attribute:score__gte",
            "owns_fact": "person:score__gte",
        },
    ],
    "token_identities_distinct": True,
    "package_branded": True,
    "compatibility_lookup": [
        {
            "input": "foo__bar",
            "resolved_attribute": "foo__bar",
            "operator": "eq",
            "literal_double_underscore": True,
        },
        {
            "input": "score__gte",
            "resolved_attribute": "score",
            "operator": "gte",
            "operator_suffix": True,
        },
        {
            "input": "score__gte__eq",
            "resolved_attribute": "score__gte",
            "operator": "eq",
            "literal_double_underscore": True,
        },
    ],
    "generated_token_string_parser_used": False,
}


def validate_artifact(path: Path, *, root: Path = ROOT) -> dict[str, Any]:
    raw = _regular_bytes(path, "field-identity artifact", MAX_ARTIFACT_BYTES)
    try:
        artifact = json.loads(raw.decode(), object_pairs_hook=_duplicate_keys)
    except ArtifactError:
        raise
    except (UnicodeDecodeError, json.JSONDecodeError, RecursionError) as error:
        raise ArtifactError("malformed_json", "artifact is not bounded UTF-8 JSON") from error
    if raw != canonical_json_bytes(artifact):
        raise ArtifactError("noncanonical_json", "artifact JSON is not canonical")
    artifact = _exact_object(
        artifact, {"format", "semantic_profile", "producer", "result"}, "artifact"
    )
    if artifact["format"] != FORMAT or artifact["semantic_profile"] != SEMANTIC_PROFILE:
        raise ArtifactError("artifact_identity_mismatch", "artifact format or profile drifted")
    producer = _exact_object(artifact["producer"], {"id", "test_id", "sources"}, "producer")
    if producer["id"] != PRODUCER_ID or producer["test_id"] != TEST_ID:
        raise ArtifactError("producer_identity_mismatch", "artifact producer identity drifted")
    sources = producer["sources"]
    if type(sources) is not list or len(sources) != len(SOURCE_PATHS):
        raise ArtifactError("source_set_mismatch", "artifact source set drifted")
    for source, expected_path in zip(sources, SOURCE_PATHS, strict=True):
        source = _exact_object(source, {"path", "sha256"}, "producer source")
        if source["path"] != expected_path:
            raise ArtifactError("source_path_mismatch", "artifact source path drifted")
        current = _regular_bytes(root / expected_path, "field-identity source", 2 * 1024 * 1024)
        if source["sha256"] != hashlib.sha256(current).hexdigest():
            raise ArtifactError("source_digest_mismatch", "artifact source digest is stale")
    result = _exact_object(
        artifact["result"], {"observation_ref", "proof_kind", "outcome", "observation"}, "result"
    )
    if (result["observation_ref"], result["proof_kind"], result["outcome"]) != (
        "field_name_identity",
        "artifact",
        "passed",
    ):
        raise ArtifactError("result_identity_mismatch", "artifact result lane drifted")
    if result["observation"] != EXPECTED_OBSERVATION:
        raise ArtifactError("observation_mismatch", "field-name identity observation drifted")
    return result["observation"]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifact", type=Path)
    parser.add_argument("--observation-json", action="store_true")
    arguments = parser.parse_args(argv)
    try:
        observation = validate_artifact(arguments.artifact)
    except ArtifactError as error:
        print(f"Workforce V3 field-identity artifact rejected: {error}", file=sys.stderr)
        return 1
    if arguments.observation_json:
        sys.stdout.buffer.write(canonical_json_bytes(observation))
    else:
        print("validated Workforce V3 field-identity artifact")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
