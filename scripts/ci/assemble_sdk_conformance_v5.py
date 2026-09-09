#!/usr/bin/env python3
"""Compose measured evidence and assemble SDK V5 reports."""

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

import compare_sdk_conformance_v5 as conformance
import compare_sdk_v5_corpora as corpora
import compare_sdk_v5_operational as operational
from persist_binding_reports import publish_bytes

ROOT = Path(__file__).resolve().parents[2]
FORMAT = "typebridge.sdk-v5-producer-evidence/v1"
MAX_EVIDENCE_BYTES = 64 * 1024 * 1024
MAX_RECORD_BYTES = 1024 * 1024
MAX_ARCHIVE_BYTES = 32 * 1024 * 1024
LIVE_FORMAT = "typebridge.sdk-v5-live-codec-evidence/v1"
CLEANUP_FORMAT = "typebridge.sdk-v5-cleanup-evidence/v1"
MAX_SMALL_EVIDENCE_BYTES = 64 * 1024
MAX_SNAPSHOT_BYTES = 1024 * 1024


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
    if not isinstance(value, str) or len(value) > (maximum + 2) // 3 * 4:
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
    evidence: dict[str, Any], *, binding: str, run_nonce: str, root: Path = ROOT
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
    publish_bytes(path, conformance.canonical_json_bytes(report), AssemblyError)


class CompositionError(ValueError):
    """Stable fail-closed producer-evidence composition rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


def composition_reject(code: str, message: str) -> NoReturn:
    raise CompositionError(code, message)


def composition_exact_keys(value: Any, expected: set[str], label: str) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != expected:
        composition_reject("invalid_composition_shape", f"{label} keys are not exact")
    return value


def load_small(path: Path, label: str) -> dict[str, Any]:
    try:
        metadata = path.lstat()
    except OSError as error:
        composition_reject("invalid_composition_file", f"cannot inspect {label}: {error}")
    if (
        stat.S_ISLNK(metadata.st_mode)
        or not stat.S_ISREG(metadata.st_mode)
        or metadata.st_size > MAX_SMALL_EVIDENCE_BYTES
    ):
        composition_reject("invalid_composition_file", f"{label} must be a bounded regular file")
    try:
        raw = path.read_bytes()
        value = json.loads(raw, object_pairs_hook=conformance.unique_object)
    except conformance.ContractError as error:
        composition_reject(error.code, str(error))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        composition_reject("invalid_composition_json", f"cannot parse {label}: {error}")
    canonical = conformance.canonical_json_bytes(value)
    if not isinstance(value, dict) or raw not in {canonical, canonical + b"\n"}:
        composition_reject("noncanonical_composition_json", f"{label} is not canonical JSON")
    return value


def decode_snapshot(value: Any, label: str) -> bytes:
    if not isinstance(value, str) or len(value) > (MAX_SNAPSHOT_BYTES + 2) // 3 * 4:
        composition_reject("invalid_live_snapshot", f"{label} is not bounded base64")
    try:
        decoded = base64.b64decode(value, validate=True)
    except (ValueError, binascii.Error):
        composition_reject("invalid_live_snapshot", f"{label} is not strict base64")
    if not decoded or base64.b64encode(decoded).decode("ascii") != value:
        composition_reject("invalid_live_snapshot", f"{label} is not canonical non-empty base64")
    return decoded


def validate_live(value: dict[str, Any], binding: str) -> None:
    composition_exact_keys(
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
        composition_reject("live_identity_mismatch", "live evidence identity differs")
    if value["direct_remote_equal"] is not True or value["remote_exchange_count"] != 1:
        composition_reject("live_parity_failure", "live direct/remote evidence is not exact")
    if (
        value["detached_mutation_code"] != "projected_snapshot_detached"
        or value["rebound_mutation"] is not True
    ):
        composition_reject("live_detached_failure", "live detached/rebound evidence is not exact")
    decode_snapshot(value["entity_snapshot_b64"], "entity snapshot")
    decode_snapshot(value["relation_snapshot_b64"], "relation snapshot")


def validate_cleanup(value: dict[str, Any], binding: str) -> dict[str, bool]:
    composition_exact_keys(
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
        composition_reject("cleanup_identity_mismatch", "cleanup evidence identity differs")
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
        composition_reject(
            "cleanup_invariant_failed", "cleanup evidence contains a false invariant"
        )
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
        composition_reject("composition_identity_mismatch", f"unknown binding {binding!r}")
    if len(run_nonce) != 64 or any(character not in "0123456789abcdef" for character in run_nonce):
        composition_reject("invalid_run_nonce", "run nonce must be 64 lowercase hexadecimal digits")
    try:
        records, archive = corpora.load_corpus(corpus_path, binding)
        controls = operational.load_evidence(operational_path, binding)
    except (corpora.CorpusError, operational.OperationalError) as error:
        composition_reject(error.code, str(error))
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


def compose_inputs(arguments: argparse.Namespace) -> dict[str, Any]:
    value = compose(
        binding=arguments.binding,
        run_nonce=arguments.run_nonce,
        corpus_path=arguments.corpus,
        live_path=arguments.live_evidence,
        operational_path=arguments.operational_evidence,
        cleanup_path=arguments.cleanup_evidence,
    )
    if len(conformance.canonical_json_bytes(value)) > MAX_EVIDENCE_BYTES:
        raise AssemblyError("evidence_size_limit", "composed evidence exceeds its byte limit")
    return value


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binding", required=True, choices=conformance.BINDINGS)
    parser.add_argument("--run-nonce", required=True)
    parser.add_argument("--evidence", required=False, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--corpus", required=False, type=Path)
    parser.add_argument("--live-evidence", required=False, type=Path)
    parser.add_argument("--operational-evidence", required=False, type=Path)
    parser.add_argument("--cleanup-evidence", required=False, type=Path)
    arguments = parser.parse_args()
    if arguments.evidence is None:
        missing = [
            name
            for name in ("corpus", "live_evidence", "operational_evidence", "cleanup_evidence")
            if getattr(arguments, name) is None
        ]
        if missing:
            parser.error(
                "composition requires "
                + ", ".join("--" + name.replace("_", "-") for name in missing)
            )
    elif any(
        getattr(arguments, name) is not None
        for name in ("corpus", "live_evidence", "operational_evidence", "cleanup_evidence")
    ):
        parser.error("choose existing evidence or composition inputs, not both")
    try:
        evidence = (
            compose_inputs(arguments)
            if arguments.evidence is None
            else load_evidence(arguments.evidence)
        )
        report = assemble_report(evidence, binding=arguments.binding, run_nonce=arguments.run_nonce)
        publish(arguments.output, report)
    except (AssemblyError, conformance.ContractError, CompositionError) as error:
        code = getattr(error, "code", "assembly_failure")
        print(f"sdk-v5 assembly rejected [{code}]: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
