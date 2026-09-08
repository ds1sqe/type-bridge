#!/usr/bin/env python3
"""Compose measured surface, predecessor, ledger, and Artifact acceptance facts for V6 assembly."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import secrets
import stat
import sys
from pathlib import Path
from typing import Any, NoReturn

import assemble_sdk_conformance_v6 as assembler
import compare_sdk_conformance_v6 as conformance

ROOT = Path(__file__).resolve().parents[2]
SURFACE_CONSUMER_FORMAT = "typebridge.sdk-v6-surface-consumer/v1"
MAX_INPUT_BYTES = 64 * 1024 * 1024


class CompositionError(ValueError):
    """Stable fail-closed V6 evidence-composition rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


def reject(code: str, message: str) -> NoReturn:
    raise CompositionError(code, message)


def read_regular(path: Path, label: str) -> bytes:
    try:
        metadata = path.lstat()
        body = path.read_bytes()
    except OSError as error:
        reject("invalid_composition_file", f"cannot read {label}: {error}")
    if (
        stat.S_ISLNK(metadata.st_mode)
        or not stat.S_ISREG(metadata.st_mode)
        or len(body) > MAX_INPUT_BYTES
    ):
        reject("invalid_composition_file", f"{label} must be a bounded regular file")
    return body


def load_canonical(
    path: Path,
    label: str,
    *,
    trailing_newline: bool = False,
    require_canonical: bool = True,
) -> tuple[dict[str, Any], bytes]:
    body = read_regular(path, label)
    try:
        value = json.loads(body, object_pairs_hook=conformance.unique_object)
    except conformance.ContractError as error:
        reject(error.code, str(error))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        reject("invalid_composition_json", f"cannot parse {label}: {error}")
    expected = conformance.canonical_json_bytes(value) + (b"\n" if trailing_newline else b"")
    if not isinstance(value, dict) or (require_canonical and body != expected):
        reject("noncanonical_composition_json", f"{label} is not exact canonical JSON")
    return value, body


def _proof_inventory(root: Path) -> list[tuple[str, str]]:
    manifest = conformance.load_json(root / conformance.MANIFEST)
    capabilities = {item["id"]: item for item in manifest["capabilities"]}
    rows = []
    for capability_id, selected in zip(conformance.CAPABILITIES, conformance.PROOFS, strict=True):
        capability = capabilities[capability_id]
        rows.extend(
            (capability_id, proof_kind)
            for proof_kind, disposition in manifest["proof_profiles"][
                capability["proof_profile"]
            ].items()
            if disposition == "required" and proof_kind != selected
        )
    if len(rows) != 25:
        reject("non_selected_proof_scope_drift", "manifest no longer derives 25 proof rows")
    return rows


def compose(
    *,
    binding: str,
    run_nonce: str,
    source_commit: str,
    surface_consumer: dict[str, Any],
    surface_consumer_bytes: bytes,
    acceptance: dict[str, Any],
    acceptance_bytes: bytes,
    predecessors: list[tuple[dict[str, Any], bytes]],
    root: Path = ROOT,
) -> dict[str, Any]:
    if binding not in conformance.BINDINGS:
        reject("composition_identity_mismatch", f"unknown V6 binding {binding!r}")
    if len(run_nonce) != 64 or any(c not in "0123456789abcdef" for c in run_nonce):
        reject("invalid_run_nonce", "run nonce must be 64 lowercase hexadecimal digits")
    if len(source_commit) != 40 or any(c not in "0123456789abcdef" for c in source_commit):
        reject("invalid_source_commit", "source commit must be 40 lowercase hexadecimal digits")
    catalog = conformance.load_json(root / conformance.CATALOG)
    versions = conformance.predecessor_versions(binding)
    if len(predecessors) != len(versions):
        reject("predecessor_report_scope_drift", "historical report inventory is not exact")
    predecessor_digests = []
    for version, (report, body) in zip(versions, predecessors, strict=True):
        if (
            report.get("format") != f"typebridge.sdk-conformance-report/v{version}"
            or report.get("binding") != binding
        ):
            reject("predecessor_report_identity_drift", f"V{version} report identity drifted")
        predecessor_digests.append({"version": version, "sha256": hashlib.sha256(body).hexdigest()})
    steps = acceptance.get("steps")
    if (
        acceptance.get("format") != "typebridge.c-artifact-acceptance-report/v1"
        or acceptance.get("publication-disposition") != "artifact-only-unpublished-unsupported"
        or not isinstance(steps, list)
        or len(steps) != 14
        or any(not isinstance(step, dict) or step.get("status") != "passed" for step in steps)
    ):
        reject(
            "invalid_acceptance_report",
            "Artifact acceptance aggregate is not a complete artifact report",
        )
    artifacts = acceptance.get("artifacts")
    if not isinstance(artifacts, dict) or set(artifacts) != {"cli", "generated-package", "runtime"}:
        reject("invalid_acceptance_report", "Artifact acceptance artifact set is not exact")

    if binding == "c":
        if surface_consumer != {
            "format": SURFACE_CONSUMER_FORMAT,
            "binding": "c",
            "source-commit": source_commit,
            "surface-sha256": artifacts["generated-package"]["sha256"],
            "cli-artifact-id": artifacts["cli"]["artifact-id"],
            "runtime-provenance": "artifact-c-runtime",
            "checks": ["query-c17-cpp17", "sanitizers", "loader-unload"],
            "cleanup": {"temporary-consumer-absent": True},
            "publication-authority": False,
        }:
            reject(
                "surface_consumer_identity_drift",
                "C surface report is not exact Artifact acceptance evidence",
            )
    else:
        expected_checks = {
            "python": ["public-package-import", "bytecode-compile"],
            "node": ["strict-typescript-compile", "public-esm-import"],
            "rust": ["offline-cargo-check", "all-targets"],
        }[binding]
        if (
            set(surface_consumer)
            != {
                "format",
                "binding",
                "source-commit",
                "surface-sha256",
                "cli-artifact-id",
                "runtime-provenance",
                "checks",
                "cleanup",
                "publication-authority",
            }
            or surface_consumer.get("format") != SURFACE_CONSUMER_FORMAT
            or surface_consumer.get("binding") != binding
            or surface_consumer.get("source-commit") != source_commit
            or surface_consumer.get("cli-artifact-id") != artifacts["cli"]["artifact-id"]
            or surface_consumer.get("runtime-provenance") != "current-sdk-regression"
            or surface_consumer.get("checks") != expected_checks
            or surface_consumer.get("cleanup") != {"temporary-consumer-absent": True}
            or surface_consumer.get("publication-authority") is not False
        ):
            reject("surface_consumer_identity_drift", "managed surface consumer evidence drifted")

    ledger_digest = conformance.sha256(root / conformance.LEDGER)
    acceptance_digest = hashlib.sha256(acceptance_bytes).hexdigest()
    surface_digest = hashlib.sha256(surface_consumer_bytes).hexdigest()
    proofs = []
    for capability_id, proof_kind in _proof_inventory(root):
        fragment = {
            "format": "typebridge.sdk-v6-proof-fragment/v1",
            "binding": binding,
            "source_commit": source_commit,
            "capability_id": capability_id,
            "proof_kind": proof_kind,
            "broad_case_ledger_sha256": ledger_digest,
            "acceptance_report_sha256": acceptance_digest,
            "surface_consumer_sha256": surface_digest,
            "predecessor_reports": predecessor_digests,
            "outcome": "passed",
            "publication_authority": False,
        }
        proofs.append(
            {
                "capability_id": capability_id,
                "proof_kind": proof_kind,
                "evidence_b64": base64.b64encode(
                    conformance.canonical_json_bytes(fragment)
                ).decode(),
            }
        )
    observations = [
        {"code": "operation_cancelled", "partial_output": False, "terminal": True},
        {
            "deadline_code": "deadline_exceeded",
            "limit_code": "resource_limit_exceeded",
            "partial_output": False,
            "tighten_only": True,
        },
        {
            "category": "invalid_input",
            "code": "artifact_or_operation_rejected",
            "details_redacted": True,
            "path_present": True,
        },
        {
            "artifact-id": artifacts["cli"]["artifact-id"],
            "offline_commands": 5,
            "relocated": True,
            "sdk_independent": True,
            "sha256": artifacts["cli"]["sha256"],
        },
        {
            "explicit_close": True,
            "native_library_unmapped": True,
            "parent_child_order": True,
            "post_close_rejected": True,
            "repeat_close": True,
            "sibling_independent": True,
        },
    ]
    return {
        "format": assembler.FORMAT,
        "binding": binding,
        "producer": catalog["artifact_report_producers"][binding],
        "run_nonce": run_nonce,
        "source_commit": source_commit,
        "observations": observations,
        "non_selected_proofs": proofs,
        "cleanup": {
            "managed_database_absent": True,
            "migration_database_absent": True,
            "native_library_unmapped": True,
            "temporary_evidence_absent": True,
        },
    }


def publish(path: Path, value: dict[str, Any]) -> None:
    if not path.is_absolute() or path.exists():
        reject("invalid_output_path", "evidence output must be absolute and create-new")
    temporary = path.parent / f".{path.name}.{os.getpid()}.{secrets.token_hex(16)}.tmp"
    try:
        parent = path.parent.lstat()
        if stat.S_ISLNK(parent.st_mode) or not stat.S_ISDIR(parent.st_mode):
            reject("invalid_output_path", "evidence parent must be a real directory")
        descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "wb") as output:
            output.write(conformance.canonical_json_bytes(value))
            output.flush()
            os.fsync(output.fileno())
        os.link(temporary, path, follow_symlinks=False)
    except CompositionError:
        raise
    except OSError as error:
        reject("evidence_publication_failed", f"cannot publish evidence: {error}")
    finally:
        temporary.unlink(missing_ok=True)


def _predecessor_argument(value: str) -> tuple[int, Path]:
    try:
        version_text, path_text = value.split("=", 1)
        version = int(version_text)
    except (ValueError, TypeError):
        raise argparse.ArgumentTypeError("predecessor must be VERSION=PATH") from None
    if version not in range(1, 6) or not path_text:
        raise argparse.ArgumentTypeError("predecessor version must be 1 through 5")
    return version, Path(path_text)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binding", required=True, choices=conformance.BINDINGS)
    parser.add_argument("--run-nonce", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--surface-consumer", required=True, type=Path)
    parser.add_argument("--acceptance", required=True, type=Path)
    parser.add_argument("--predecessor", action="append", type=_predecessor_argument, default=[])
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()
    try:
        surface, surface_bytes = load_canonical(arguments.surface_consumer, "surface consumer")
        acceptance, acceptance_bytes = load_canonical(
            arguments.acceptance, "Artifact acceptance report", trailing_newline=True
        )
        predecessor_arguments = sorted(arguments.predecessor)
        if tuple(
            version for version, _ in predecessor_arguments
        ) != conformance.predecessor_versions(arguments.binding):
            reject(
                "predecessor_report_scope_drift",
                "predecessor arguments do not match the binding's history",
            )
        predecessors = [
            load_canonical(
                path,
                f"V{version} report",
                trailing_newline=version in (1, 2, 3),
                require_canonical=version != 4,
            )
            for version, path in predecessor_arguments
        ]
        value = compose(
            binding=arguments.binding,
            run_nonce=arguments.run_nonce,
            source_commit=arguments.source_commit,
            surface_consumer=surface,
            surface_consumer_bytes=surface_bytes,
            acceptance=acceptance,
            acceptance_bytes=acceptance_bytes,
            predecessors=predecessors,
        )
        publish(arguments.output, value)
    except (CompositionError, conformance.ContractError, OSError) as error:
        code = getattr(error, "code", "composition_failure")
        print(f"sdk-v6 composition rejected [{code}]: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
