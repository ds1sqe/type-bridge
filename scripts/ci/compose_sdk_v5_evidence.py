#!/usr/bin/env python3
"""Compose one V5 producer artifact from measured corpus, live, and control evidence."""

from __future__ import annotations

import argparse
import base64
import binascii
import json
import os
import secrets
import stat
import sys
from pathlib import Path
from typing import Any, NoReturn

import compare_sdk_conformance_v5 as conformance
import compare_sdk_v5_corpora as corpora
import compare_sdk_v5_operational as operational

ROOT = Path(__file__).resolve().parents[2]
FORMAT = "typebridge.sdk-v5-producer-evidence/v1"
LIVE_FORMAT = "typebridge.sdk-v5-live-codec-evidence/v1"
CLEANUP_FORMAT = "typebridge.sdk-v5-cleanup-evidence/v1"
MAX_SMALL_EVIDENCE_BYTES = 64 * 1024
MAX_SNAPSHOT_BYTES = 1024 * 1024


class CompositionError(ValueError):
    """Stable fail-closed producer-evidence composition rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


def reject(code: str, message: str) -> NoReturn:
    raise CompositionError(code, message)


def exact_keys(value: Any, expected: set[str], label: str) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != expected:
        reject("invalid_composition_shape", f"{label} keys are not exact")
    return value


def load_small(path: Path, label: str) -> dict[str, Any]:
    try:
        metadata = path.lstat()
    except OSError as error:
        reject("invalid_composition_file", f"cannot inspect {label}: {error}")
    if (
        stat.S_ISLNK(metadata.st_mode)
        or not stat.S_ISREG(metadata.st_mode)
        or metadata.st_size > MAX_SMALL_EVIDENCE_BYTES
    ):
        reject("invalid_composition_file", f"{label} must be a bounded regular file")
    try:
        raw = path.read_bytes()
        value = json.loads(raw, object_pairs_hook=conformance.unique_object)
    except conformance.ContractError as error:
        reject(error.code, str(error))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        reject("invalid_composition_json", f"cannot parse {label}: {error}")
    canonical = conformance.canonical_json_bytes(value)
    if not isinstance(value, dict) or raw not in {canonical, canonical + b"\n"}:
        reject("noncanonical_composition_json", f"{label} is not canonical JSON")
    return value


def decode_snapshot(value: Any, label: str) -> bytes:
    if not isinstance(value, str) or len(value) > ((MAX_SNAPSHOT_BYTES + 2) // 3) * 4:
        reject("invalid_live_snapshot", f"{label} is not bounded base64")
    try:
        decoded = base64.b64decode(value, validate=True)
    except (ValueError, binascii.Error):
        reject("invalid_live_snapshot", f"{label} is not strict base64")
    if not decoded or base64.b64encode(decoded).decode("ascii") != value:
        reject("invalid_live_snapshot", f"{label} is not canonical non-empty base64")
    return decoded


def validate_live(value: dict[str, Any], binding: str) -> None:
    exact_keys(
        value,
        {
            "binding",
            "detached_mutation_code",
            "direct_remote_equal",
            "entity_snapshot_b64",
            "format",
            "relation_snapshot_b64",
            "remote_exchange_count",
            "rebound_mutation",
        },
        "live evidence",
    )
    if value["format"] != LIVE_FORMAT or value["binding"] != binding:
        reject("live_identity_mismatch", "live evidence identity differs")
    if value["direct_remote_equal"] is not True or value["remote_exchange_count"] != 1:
        reject("live_parity_failure", "live direct/remote evidence is not exact")
    if (
        value["detached_mutation_code"] != "projected_snapshot_detached"
        or value["rebound_mutation"] is not True
    ):
        reject("live_detached_failure", "live detached/rebound evidence is not exact")
    decode_snapshot(value["entity_snapshot_b64"], "entity snapshot")
    decode_snapshot(value["relation_snapshot_b64"], "relation snapshot")


def validate_cleanup(value: dict[str, Any], binding: str) -> dict[str, bool]:
    exact_keys(
        value,
        {
            "binding",
            "format",
            "managed_database_absent",
            "partial_output_absent",
            "temporary_evidence_absent",
        },
        "cleanup evidence",
    )
    if value["format"] != CLEANUP_FORMAT or value["binding"] != binding:
        reject("cleanup_identity_mismatch", "cleanup evidence identity differs")
    cleanup = {
        "managed_database_absent": value["managed_database_absent"],
        "temporary_evidence_absent": value["temporary_evidence_absent"],
        "partial_output_absent": value["partial_output_absent"],
    }
    if cleanup != {
        "managed_database_absent": True,
        "temporary_evidence_absent": True,
        "partial_output_absent": True,
    }:
        reject("cleanup_invariant_failed", "cleanup evidence contains a false invariant")
    return cleanup


def compose(
    *,
    binding: str,
    run_nonce: str,
    corpus_path: Path,
    live_path: Path,
    operational_path: Path,
    cleanup_path: Path,
    root: Path = ROOT,
) -> dict[str, Any]:
    if binding not in conformance.BINDINGS:
        reject("composition_identity_mismatch", f"unknown binding {binding!r}")
    if len(run_nonce) != 64 or any(character not in "0123456789abcdef" for character in run_nonce):
        reject("invalid_run_nonce", "run nonce must be 64 lowercase hexadecimal digits")
    try:
        records, archive = corpora.load_corpus(corpus_path, binding)
        controls = operational.load_evidence(operational_path, binding)
    except (corpora.CorpusError, operational.OperationalError) as error:
        reject(error.code, str(error))
    live = load_small(live_path, "live evidence")
    validate_live(live, binding)
    cleanup = validate_cleanup(load_small(cleanup_path, "cleanup evidence"), binding)
    contracts = conformance.load_contracts(root)
    producer = contracts.catalog["report_producers"][binding]
    return {
        "archive_b64": base64.b64encode(archive).decode("ascii"),
        "binding": binding,
        "cleanup": cleanup,
        "detached_mutation_code": live["detached_mutation_code"],
        "direct_remote_equal": live["direct_remote_equal"],
        "foreign_schema_code": controls["diagnostic"]["code"],
        "format": FORMAT,
        "producer": producer["id"],
        "record_b64": [base64.b64encode(record).decode("ascii") for record in records],
        "round_trip": True,
        "run_nonce": run_nonce,
        "serialization_cancellation": controls["cancellation"],
        "serialization_resource_lifecycle": controls["lifecycle"],
        "serialization_resource_limits": controls["resource_limits"],
        "serialization_structured_diagnostic": controls["diagnostic"],
        "test_id": producer["test_id"],
    }


def publish(path: Path, value: dict[str, Any]) -> None:
    if not path.is_absolute():
        reject("invalid_output_path", "producer-evidence output must be absolute")
    temporary = path.parent / f".{path.name}.{os.getpid()}.{secrets.token_hex(16)}.tmp"
    try:
        parent = path.parent.lstat()
        if stat.S_ISLNK(parent.st_mode) or not stat.S_ISDIR(parent.st_mode):
            reject("invalid_output_path", "producer-evidence parent must be a real directory")
        descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "wb") as output:
            output.write(conformance.canonical_json_bytes(value))
            output.flush()
            os.fsync(output.fileno())
        os.link(temporary, path, follow_symlinks=False)
    except CompositionError:
        raise
    except OSError as error:
        reject("evidence_publication_failed", f"cannot publish producer evidence: {error}")
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binding", required=True, choices=conformance.BINDINGS)
    parser.add_argument("--run-nonce", required=True)
    parser.add_argument("--corpus", required=True, type=Path)
    parser.add_argument("--live-evidence", required=True, type=Path)
    parser.add_argument("--operational-evidence", required=True, type=Path)
    parser.add_argument("--cleanup-evidence", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()
    try:
        value = compose(
            binding=arguments.binding,
            run_nonce=arguments.run_nonce,
            corpus_path=arguments.corpus,
            live_path=arguments.live_evidence,
            operational_path=arguments.operational_evidence,
            cleanup_path=arguments.cleanup_evidence,
        )
        publish(arguments.output, value)
    except (CompositionError, conformance.ContractError) as error:
        code = getattr(error, "code", "composition_failure")
        print(f"sdk-v5 composition rejected [{code}]: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
