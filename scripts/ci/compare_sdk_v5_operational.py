#!/usr/bin/env python3
"""Validate and compare four provider-free Sdk V5 operational artifacts."""

from __future__ import annotations

import argparse
import json
import stat
import sys
from pathlib import Path
from typing import Any, NoReturn

FORMAT = "typebridge.sdk-v5-operational-evidence/v1"
COMPARISON_FORMAT = "typebridge.sdk-v5-operational-comparison/v1"
BINDINGS = ("python", "node", "rust", "c")
TEST_IDS = {
    "python": "python.generated_sdk_v5_canonical_codec",
    "node": "node.generated_sdk_v5_canonical_codec",
    "rust": "rust_acceptance::generated_rust_sdk_v5_canonical_codec",
    "c": "c.generated_sdk_v5_canonical_codec",
}
TOP_LEVEL_KEYS = {
    "binding",
    "cancellation",
    "deadline",
    "diagnostic",
    "format",
    "lifecycle",
    "resource_limits",
    "test_id",
}
EXPECTED = {
    "cancellation": {"code": "projected_codec_cancelled", "partial_output": False},
    "deadline": {
        "code": "projected_codec_deadline_exceeded",
        "partial_output": False,
    },
    "diagnostic": {
        "category": "invalid_input",
        "code": "projected_record_schema_mismatch",
        "path": ["declared_schema_identity"],
        "payload_absent": True,
    },
    "lifecycle": {
        "archive_closed": True,
        "builder_closed": True,
        "bytes_closed": True,
        "decoded_closed": True,
        "repeat_close": True,
        "sibling_usable": True,
    },
    "resource_limits": {
        "depth_code": "projected_codec_depth_limit",
        "input_code": "projected_codec_input_limit",
        "member_code": "projected_codec_member_limit",
        "output_code": "projected_codec_output_limit",
        "partial_output": False,
    },
}
MAX_EVIDENCE_BYTES = 64 * 1024


class OperationalError(ValueError):
    """Stable fail-closed operational-evidence rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


def reject(code: str, message: str) -> NoReturn:
    raise OperationalError(code, message)


def unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            reject("duplicate_operational_key", f"duplicate operational key {key!r}")
        result[key] = value
    return result


def canonical_json_bytes(value: Any) -> bytes:
    return json.dumps(
        value,
        ensure_ascii=False,
        allow_nan=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")


def load_evidence(path: Path, expected_binding: str) -> dict[str, Any]:
    try:
        metadata = path.lstat()
    except OSError as error:
        reject("invalid_operational_file", f"cannot inspect {expected_binding}: {error}")
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        reject("invalid_operational_file", f"{expected_binding} evidence must be regular")
    if metadata.st_size > MAX_EVIDENCE_BYTES:
        reject("operational_size_limit", f"{expected_binding} evidence exceeds 64 KiB")
    try:
        raw = path.read_bytes()
        value = json.loads(raw, object_pairs_hook=unique_object)
    except OperationalError:
        raise
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        reject("invalid_operational_json", f"cannot parse {expected_binding}: {error}")
    if not isinstance(value, dict) or set(value) != TOP_LEVEL_KEYS:
        reject("invalid_operational_shape", f"{expected_binding} keys are not exact")
    try:
        canonical = canonical_json_bytes(value)
    except (TypeError, ValueError) as error:
        reject("invalid_operational_json", f"cannot canonicalize {expected_binding}: {error}")
    if raw != canonical:
        reject("noncanonical_operational_json", f"{expected_binding} JSON is not canonical")
    if (
        value["format"] != FORMAT
        or value["binding"] != expected_binding
        or value["test_id"] != TEST_IDS[expected_binding]
    ):
        reject("operational_identity_mismatch", f"{expected_binding} identity differs")
    for lane, expected in EXPECTED.items():
        if value[lane] != expected:
            reject("operational_observation_drift", f"{expected_binding}.{lane} drifted")
    return value


def compare(paths: dict[str, Path]) -> dict[str, Any]:
    if set(paths) != set(BINDINGS):
        reject("operational_binding_set_mismatch", "exactly four binding artifacts are required")
    loaded = {binding: load_evidence(paths[binding], binding) for binding in BINDINGS}
    reference = loaded["rust"]
    for binding, value in loaded.items():
        for lane in EXPECTED:
            if value[lane] != reference[lane]:
                reject("cross_binding_operational_drift", f"{binding}.{lane} differs from Rust")
    return {
        "bindings": list(BINDINGS),
        "cancellation": reference["cancellation"],
        "deadline": reference["deadline"],
        "diagnostic": reference["diagnostic"],
        "format": COMPARISON_FORMAT,
        "lifecycle": reference["lifecycle"],
        "resource_limits": reference["resource_limits"],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for binding in BINDINGS:
        parser.add_argument(f"--{binding}", required=True, type=Path)
    arguments = parser.parse_args()
    try:
        result = compare({binding: getattr(arguments, binding) for binding in BINDINGS})
    except OperationalError as error:
        print(f"sdk-v5 operational evidence rejected [{error.code}]: {error}", file=sys.stderr)
        return 1
    sys.stdout.buffer.write(canonical_json_bytes(result) + b"\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
