#!/usr/bin/env python3
"""Compare independently produced provider-free Sdk V5 canonical corpora."""

from __future__ import annotations

import argparse
import base64
import binascii
import hashlib
import json
import stat
import sys
from pathlib import Path
from typing import Any, NoReturn

FORMAT = "typebridge.sdk-v5-provider-free-corpus/v1"
BINDINGS = ("python", "node", "rust", "c")
MAX_CORPUS_BYTES = 64 * 1024 * 1024
MAX_RECORD_BYTES = 1024 * 1024
MAX_ARCHIVE_BYTES = 32 * 1024 * 1024


class CorpusError(ValueError):
    """Stable fail-closed corpus rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


def reject(code: str, message: str) -> NoReturn:
    raise CorpusError(code, message)


def unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            reject("duplicate_corpus_key", f"duplicate corpus key {key!r}")
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


def decode_b64(value: Any, label: str, maximum: int) -> bytes:
    if not isinstance(value, str) or len(value) > ((maximum + 2) // 3) * 4:
        reject("invalid_corpus_bytes", f"{label} is not bounded base64")
    try:
        decoded = base64.b64decode(value, validate=True)
    except (ValueError, binascii.Error):
        reject("invalid_corpus_bytes", f"{label} is not strict base64")
    if len(decoded) > maximum or base64.b64encode(decoded).decode("ascii") != value:
        reject("invalid_corpus_bytes", f"{label} is not canonical bounded base64")
    return decoded


def load_corpus(path: Path, expected_binding: str) -> tuple[list[bytes], bytes]:
    try:
        metadata = path.lstat()
    except OSError as error:
        reject("invalid_corpus_file", f"cannot inspect {expected_binding} corpus: {error}")
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        reject("invalid_corpus_file", f"{expected_binding} corpus must be a regular file")
    if metadata.st_size > MAX_CORPUS_BYTES:
        reject("corpus_size_limit", f"{expected_binding} corpus exceeds 64 MiB")
    try:
        raw = path.read_bytes()
        value = json.loads(raw, object_pairs_hook=unique_object)
    except CorpusError:
        raise
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        reject("invalid_corpus_json", f"cannot parse {expected_binding} corpus: {error}")
    if not isinstance(value, dict) or set(value) != {
        "archive_b64",
        "binding",
        "format",
        "record_b64",
    }:
        reject("invalid_corpus_shape", f"{expected_binding} corpus keys are not exact")
    try:
        canonical = canonical_json_bytes(value)
    except (TypeError, ValueError) as error:
        reject("invalid_corpus_json", f"cannot canonicalize {expected_binding} corpus: {error}")
    if raw != canonical:
        reject("noncanonical_corpus_json", f"{expected_binding} corpus JSON is not canonical")
    if value["format"] != FORMAT or value["binding"] != expected_binding:
        reject("corpus_identity_mismatch", f"{expected_binding} corpus identity differs")
    encoded_records = value["record_b64"]
    if not isinstance(encoded_records, list) or len(encoded_records) != 9:
        reject("invalid_corpus_shape", f"{expected_binding} corpus must contain nine records")
    records = [
        decode_b64(encoded, f"{expected_binding}.record[{index}]", MAX_RECORD_BYTES)
        for index, encoded in enumerate(encoded_records)
    ]
    archive = decode_b64(value["archive_b64"], f"{expected_binding}.archive", MAX_ARCHIVE_BYTES)
    if any(not record for record in records) or not archive:
        reject("invalid_corpus_bytes", f"{expected_binding} corpus contains an empty payload")
    return records, archive


def compare(paths: dict[str, Path]) -> dict[str, Any]:
    if set(paths) != set(BINDINGS):
        reject("corpus_binding_set_mismatch", "exactly four binding corpora are required")
    loaded = {binding: load_corpus(paths[binding], binding) for binding in BINDINGS}
    reference_records, reference_archive = loaded["rust"]
    for binding in BINDINGS:
        records, archive = loaded[binding]
        if records != reference_records:
            reject("record_bytes_mismatch", f"{binding} record bytes differ from Rust")
        if archive != reference_archive:
            reject("archive_bytes_mismatch", f"{binding} archive bytes differ from Rust")
    return {
        "archive_sha256": hashlib.sha256(reference_archive).hexdigest(),
        "format": "typebridge.sdk-v5-provider-free-comparison/v1",
        "record_sha256": [hashlib.sha256(record).hexdigest() for record in reference_records],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for binding in BINDINGS:
        parser.add_argument(f"--{binding}", required=True, type=Path)
    arguments = parser.parse_args()
    try:
        result = compare({binding: getattr(arguments, binding) for binding in BINDINGS})
    except CorpusError as error:
        print(f"sdk-v5 corpus rejected [{error.code}]: {error}", file=sys.stderr)
        return 1
    sys.stdout.buffer.write(canonical_json_bytes(result) + b"\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
