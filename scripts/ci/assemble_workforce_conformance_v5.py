#!/usr/bin/env python3
"""Assemble one Workforce V5 report from a binding's measured codec evidence."""

from __future__ import annotations

import argparse
import base64
import binascii
import hashlib
import json
import os
import secrets
import stat
import sys
from pathlib import Path
from typing import Any, NoReturn

import compare_workforce_conformance_v5 as conformance

ROOT = Path(__file__).resolve().parents[2]
FORMAT = "typebridge.workforce-v5-producer-evidence/v1"
MAX_EVIDENCE_BYTES = 64 * 1024 * 1024
MAX_RECORD_BYTES = 1024 * 1024
MAX_ARCHIVE_BYTES = 32 * 1024 * 1024


class AssemblyError(ValueError):
    """One stable fail-closed producer-evidence rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


def reject(code: str, message: str) -> NoReturn:
    raise AssemblyError(code, message)


def exact_keys(value: Any, expected: set[str], label: str) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != expected:
        reject("invalid_evidence_shape", f"{label} keys are not exact")
    return value


def load_evidence(path: Path) -> dict[str, Any]:
    try:
        metadata = path.lstat()
    except OSError as error:
        reject("invalid_evidence_file", f"cannot inspect evidence: {error}")
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        reject("invalid_evidence_file", "evidence must be a regular non-symlink file")
    if metadata.st_size > MAX_EVIDENCE_BYTES:
        reject("evidence_size_limit", "producer evidence exceeds 64 MiB")
    try:
        raw = path.read_bytes()
        value = json.loads(raw, object_pairs_hook=conformance.unique_object)
    except conformance.ContractError as error:
        reject(error.code, str(error))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        reject("invalid_evidence_json", f"cannot parse producer evidence: {error}")
    if not isinstance(value, dict):
        reject("invalid_evidence_shape", "producer evidence must be one object")
    try:
        canonical = conformance.canonical_json_bytes(value)
    except (TypeError, ValueError) as error:
        reject("invalid_evidence_json", f"cannot canonicalize producer evidence: {error}")
    if raw != canonical:
        reject("noncanonical_evidence_json", "producer evidence is not exact canonical JSON")
    return value


def decode_b64(value: Any, label: str, maximum: int) -> bytes:
    if not isinstance(value, str) or len(value) > ((maximum + 2) // 3) * 4:
        reject("invalid_evidence_bytes", f"{label} is not bounded base64")
    try:
        decoded = base64.b64decode(value, validate=True)
    except (ValueError, binascii.Error):
        reject("invalid_evidence_bytes", f"{label} is not strict base64")
    if len(decoded) > maximum or base64.b64encode(decoded).decode("ascii") != value:
        reject("invalid_evidence_bytes", f"{label} is not canonical bounded base64")
    return decoded


def validate_nonce(value: Any) -> str:
    if (
        not isinstance(value, str)
        or len(value) != 64
        or any(character not in "0123456789abcdef" for character in value)
    ):
        reject("invalid_run_nonce", "run nonce must be 64 lowercase hexadecimal digits")
    return value


def assemble_report(
    evidence: dict[str, Any],
    *,
    binding: str,
    run_nonce: str,
    root: Path = ROOT,
) -> dict[str, Any]:
    contracts = conformance.load_contracts(root)
    exact_keys(
        evidence,
        {
            "format",
            "binding",
            "producer",
            "test_id",
            "run_nonce",
            "record_b64",
            "archive_b64",
            "round_trip",
            "foreign_schema_code",
            "direct_remote_equal",
            "detached_mutation_code",
            "serialization_cancellation",
            "serialization_resource_limits",
            "serialization_structured_diagnostic",
            "serialization_resource_lifecycle",
            "cleanup",
        },
        "producer evidence",
    )
    if evidence["format"] != FORMAT or evidence["binding"] != binding:
        reject("evidence_identity_mismatch", "evidence format or binding differs")
    if binding not in conformance.BINDINGS:
        reject("evidence_identity_mismatch", f"unknown V5 binding {binding!r}")
    producer = contracts.catalog.get("report_producers", {}).get(binding)
    if not isinstance(producer, dict) or evidence["producer"] != producer.get("id"):
        reject("producer_identity_mismatch", "producer ID is not frozen by the V5 catalog")
    if evidence["test_id"] != producer.get("test_id"):
        reject("producer_test_mismatch", "producer test ID is not frozen by the V5 catalog")
    if validate_nonce(evidence["run_nonce"]) != validate_nonce(run_nonce):
        reject("run_nonce_mismatch", "producer evidence is not from this requested run")

    record_values = evidence["record_b64"]
    if not isinstance(record_values, list) or len(record_values) != 9:
        reject("invalid_evidence_shape", "producer must publish nine exact records")
    records = [
        decode_b64(value, f"record[{index}]", MAX_RECORD_BYTES)
        for index, value in enumerate(record_values)
    ]
    if any(not record for record in records):
        reject("invalid_evidence_bytes", "canonical record bytes cannot be empty")
    archive = decode_b64(evidence["archive_b64"], "archive", MAX_ARCHIVE_BYTES)
    if not archive:
        reject("invalid_evidence_bytes", "canonical archive bytes cannot be empty")
    canonical = {
        "record_sha256": [hashlib.sha256(record).hexdigest() for record in records],
        "archive_sha256": hashlib.sha256(archive).hexdigest(),
        "round_trip": evidence["round_trip"],
        "foreign_schema_code": evidence["foreign_schema_code"],
        "direct_remote_equal": evidence["direct_remote_equal"],
        "detached_mutation_code": evidence["detached_mutation_code"],
    }
    observations = [
        canonical,
        evidence["serialization_cancellation"],
        evidence["serialization_resource_limits"],
        evidence["serialization_structured_diagnostic"],
        evidence["serialization_resource_lifecycle"],
    ]
    for index, observation in enumerate(observations):
        conformance.validate_observation(index, observation)

    cases = contracts.catalog["cases"]
    proofs = contracts.catalog["selected_proofs"]
    results = [
        {
            "case_id": case["id"],
            "capability_id": case["capability_id"],
            "proof_kind": proof["proof_kind"],
            "outcome": "passed",
            "observation": observation,
        }
        for case, proof, observation in zip(cases, proofs, observations, strict=True)
    ]
    report = {
        "format": "typebridge.sdk-conformance-report/v5",
        "binding": binding,
        "manifest": {
            "path": conformance.MANIFEST,
            "sha256": conformance.sha256(root / conformance.MANIFEST),
        },
        "catalog": {
            "path": conformance.CATALOG,
            "sha256": conformance.sha256(root / conformance.CATALOG),
        },
        "journey": {
            "path": conformance.JOURNEY,
            "sha256": conformance.sha256(root / conformance.JOURNEY),
        },
        "record_contract": {
            "path": conformance.RECORD_CONTRACT,
            "sha256": conformance.sha256(root / conformance.RECORD_CONTRACT),
        },
        "server_version": "3.12.3",
        "results": results,
        "cleanup": evidence["cleanup"],
    }
    conformance.validate_report(report, contracts, root)
    return report


def publish(path: Path, report: dict[str, Any]) -> None:
    if not path.is_absolute():
        reject("invalid_output_path", "report output must be absolute")
    try:
        parent = path.parent.lstat()
    except OSError as error:
        reject("invalid_output_path", f"report parent cannot be inspected: {error}")
    if stat.S_ISLNK(parent.st_mode) or not stat.S_ISDIR(parent.st_mode):
        reject("invalid_output_path", "report parent must be a real directory")
    temporary = path.parent / f".{path.name}.{os.getpid()}.{secrets.token_hex(16)}.tmp"
    raw = conformance.canonical_json_bytes(report)
    try:
        descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "wb") as output:
            output.write(raw)
            output.flush()
            os.fsync(output.fileno())
        os.link(temporary, path, follow_symlinks=False)
    except OSError as error:
        reject("report_publication_failed", f"cannot publish create-new report: {error}")
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binding", required=True, choices=conformance.BINDINGS)
    parser.add_argument("--run-nonce", required=True)
    parser.add_argument("--evidence", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()
    try:
        evidence = load_evidence(arguments.evidence)
        report = assemble_report(
            evidence,
            binding=arguments.binding,
            run_nonce=arguments.run_nonce,
        )
        publish(arguments.output, report)
    except (AssemblyError, conformance.ContractError) as error:
        code = getattr(error, "code", "assembly_failure")
        print(f"workforce-v5 assembly rejected [{code}]: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
