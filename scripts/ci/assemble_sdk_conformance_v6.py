#!/usr/bin/env python3
"""Assemble one V6 report from measured clean-consumer and artifact evidence."""

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

import c_artifact_journey as artifact_journey
import compare_sdk_conformance_v6 as conformance
import sdk_v6_surfaces as surfaces

ROOT = Path(__file__).resolve().parents[2]
FORMAT = "typebridge.sdk-v6-producer-evidence/v1"
MAX_EVIDENCE_BYTES = 32 * 1024 * 1024
MAX_FRAGMENT_BYTES = 1024 * 1024
MAX_SURFACE_BYTES = 128 * 1024 * 1024


class AssemblyError(ValueError):
    """Stable fail-closed V6 assembly rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


def reject(code: str, message: str) -> NoReturn:
    raise AssemblyError(code, message)


def exact_keys(value: Any, expected: set[str], label: str) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != expected:
        reject("invalid_evidence_shape", f"{label} keys are not exact")
    return value


def read_regular(path: Path, maximum: int, label: str) -> bytes:
    try:
        metadata = path.lstat()
    except OSError as error:
        reject("invalid_evidence_file", f"cannot inspect {label}: {error}")
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        reject("invalid_evidence_file", f"{label} must be a regular non-symlink file")
    if metadata.st_size > maximum:
        reject("evidence_size_limit", f"{label} exceeds its byte limit")
    try:
        return path.read_bytes()
    except OSError as error:
        reject("invalid_evidence_file", f"cannot read {label}: {error}")


def load_evidence(path: Path) -> dict[str, Any]:
    raw = read_regular(path, MAX_EVIDENCE_BYTES, "producer evidence")
    try:
        value = json.loads(raw, object_pairs_hook=conformance.unique_object)
    except conformance.ContractError as error:
        reject(error.code, str(error))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        reject("invalid_evidence_json", f"cannot parse producer evidence: {error}")
    if not isinstance(value, dict) or raw != conformance.canonical_json_bytes(value):
        reject("noncanonical_evidence_json", "producer evidence is not exact canonical JSON")
    return value


def decode_fragment(value: Any, label: str) -> bytes:
    if not isinstance(value, str) or len(value) > ((MAX_FRAGMENT_BYTES + 2) // 3) * 4:
        reject("invalid_proof_fragment", f"{label} is not bounded base64")
    try:
        decoded = base64.b64decode(value, validate=True)
    except (ValueError, binascii.Error):
        reject("invalid_proof_fragment", f"{label} is not strict base64")
    if not decoded or base64.b64encode(decoded).decode("ascii") != value:
        reject("invalid_proof_fragment", f"{label} is not canonical non-empty base64")
    return decoded


def _identity(relative: str, root: Path) -> dict[str, str]:
    return {"path": relative, "sha256": conformance.sha256(root / relative)}


def _predecessors(values: list[bytes], binding: str) -> list[dict[str, Any]]:
    versions = conformance.predecessor_versions(binding)
    if len(values) != len(versions):
        reject("predecessor_report_scope_drift", "historical report inventory is not exact")
    output = []
    for version, raw in zip(versions, values, strict=True):
        try:
            report = json.loads(raw, object_pairs_hook=conformance.unique_object)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            reject("invalid_predecessor_report", f"cannot parse V{version} report: {error}")
        if not isinstance(report, dict) or not conformance.historical_report_is_exact(
            raw, report, version
        ):
            reject("noncanonical_predecessor_report", f"V{version} report is not canonical")
        if report.get("format") != f"typebridge.sdk-conformance-report/v{version}":
            reject("predecessor_report_identity_drift", f"V{version} format drifted")
        if report.get("binding") != binding:
            reject("predecessor_report_identity_drift", f"V{version} binding drifted")
        output.append({"version": version, "sha256": hashlib.sha256(raw).hexdigest()})
    return output


def assemble_report(
    evidence: dict[str, Any],
    *,
    binding: str,
    run_nonce: str,
    acceptance: dict[str, Any],
    acceptance_sha256: str,
    generated_surface: bytes,
    predecessor_reports: list[bytes],
    root: Path = ROOT,
) -> dict[str, Any]:
    exact_keys(
        evidence,
        {
            "format",
            "binding",
            "producer",
            "run_nonce",
            "source_commit",
            "observations",
            "non_selected_proofs",
            "cleanup",
        },
        "producer evidence",
    )
    catalog = conformance.load_json(root / conformance.CATALOG)
    if evidence["format"] != FORMAT or evidence["binding"] != binding:
        reject("evidence_identity_mismatch", "evidence format or binding differs")
    if binding not in conformance.BINDINGS:
        reject("evidence_identity_mismatch", f"unknown V6 binding {binding!r}")
    if evidence["producer"] != catalog["artifact_report_producers"][binding]:
        reject("producer_identity_mismatch", "producer is not frozen by V6")
    if (
        evidence["run_nonce"] != run_nonce
        or not isinstance(run_nonce, str)
        or len(run_nonce) != 64
        or any(c not in "0123456789abcdef" for c in run_nonce)
    ):
        reject("run_nonce_mismatch", "producer evidence is not from this exact run")
    commit = evidence["source_commit"]
    if (
        not isinstance(commit, str)
        or len(commit) != 40
        or any(c not in "0123456789abcdef" for c in commit)
    ):
        reject("invalid_source_commit", "producer source commit is invalid")
    if (
        acceptance.get("format") != "typebridge.c-artifact-acceptance-report/v1"
        or acceptance.get("publication-disposition") != "artifact-only-unpublished-unsupported"
    ):
        reject("invalid_acceptance_report", "Artifact acceptance aggregate identity is invalid")
    conformance.exact_sha(acceptance_sha256, "Artifact acceptance aggregate")
    artifacts = acceptance.get("artifacts")
    if not isinstance(artifacts, dict):
        reject("invalid_acceptance_report", "Artifact acceptance artifact inventory is missing")

    surface_sha = hashlib.sha256(generated_surface).hexdigest()
    if not generated_surface:
        reject("invalid_generated_surface", "generated surface cannot be empty")
    surface = {
        "format": conformance.SURFACE_FORMATS[binding],
        "artifact-id": f"sha256:{surface_sha}",
        "sha256": surface_sha,
    }
    if binding == "c":
        surface = {"format": conformance.SURFACE_FORMATS["c"], **artifacts["generated-package"]}
        if hashlib.sha256(generated_surface).hexdigest() != surface["sha256"]:
            reject(
                "c_generated_surface_drift",
                "C surface bytes differ from Artifact acceptance package",
            )

    proof_evidence = evidence["non_selected_proofs"]
    if not isinstance(proof_evidence, list) or len(proof_evidence) != 25:
        reject("non_selected_proof_scope_drift", "producer must emit 25 proof fragments")
    proofs = []
    seen_digests: set[str] = set()
    for index, item in enumerate(proof_evidence):
        exact_keys(item, {"capability_id", "proof_kind", "evidence_b64"}, "proof fragment")
        digest = hashlib.sha256(
            decode_fragment(item["evidence_b64"], f"proof[{index}]")
        ).hexdigest()
        if digest in seen_digests:
            reject("duplicate_proof_fragment", "proof fragments must be independently bound")
        seen_digests.add(digest)
        proofs.append(
            {
                "capability_id": item["capability_id"],
                "proof_kind": item["proof_kind"],
                "evidence_sha256": digest,
            }
        )

    observations = evidence["observations"]
    if not isinstance(observations, list) or len(observations) != 5:
        reject("invalid_observation_scope", "producer must emit five terminal observations")
    results = [
        {
            "case_id": case_id,
            "capability_id": capability_id,
            "proof_kind": proof_kind,
            "outcome": "passed",
            "observation": observation,
        }
        for case_id, capability_id, proof_kind, observation in zip(
            conformance.CASES,
            conformance.CAPABILITIES,
            conformance.PROOFS,
            observations,
            strict=True,
        )
    ]
    report = {
        "format": "typebridge.sdk-conformance-report/v6",
        "binding": binding,
        "producer": evidence["producer"],
        "source_commit": commit,
        "manifest": _identity(conformance.MANIFEST, root),
        "catalog": _identity(conformance.CATALOG, root),
        "journey": _identity(conformance.JOURNEY, root),
        "broad_case_ledger": _identity(conformance.LEDGER, root),
        "acceptance_report_sha256": acceptance_sha256,
        "artifacts": artifacts,
        "generated_surface": surface,
        "predecessor_reports": _predecessors(predecessor_reports, binding),
        "non_selected_proofs": proofs,
        "results": results,
        "cleanup": evidence["cleanup"],
        "publication_authority": False,
    }
    conformance.validate_report(report, root)
    return report


def publish(path: Path, report: dict[str, Any]) -> None:
    if not path.is_absolute():
        reject("invalid_output_path", "report output must be absolute")
    try:
        parent = path.parent.lstat()
    except OSError as error:
        reject("invalid_output_path", f"cannot inspect report parent: {error}")
    if stat.S_ISLNK(parent.st_mode) or not stat.S_ISDIR(parent.st_mode):
        reject("invalid_output_path", "report parent must be a real directory")
    temporary = path.parent / f".{path.name}.{os.getpid()}.{secrets.token_hex(16)}.tmp"
    try:
        descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "wb") as output:
            output.write(conformance.canonical_json_bytes(report))
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
    parser.add_argument("--evidence", required=True, type=Path)
    parser.add_argument("--acceptance", required=True, type=Path)
    parser.add_argument("--provider", required=True, type=Path)
    parser.add_argument("--live", required=True, type=Path)
    parser.add_argument("--cli", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--generated", required=True, type=Path)
    parser.add_argument("--generated-surface", required=True, type=Path)
    parser.add_argument("--predecessor", action="append", type=_predecessor_argument, default=[])
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()
    try:
        evidence = load_evidence(arguments.evidence)
        acceptance = artifact_journey.load_canonical_report(arguments.acceptance)
        artifact_journey.validate_acceptance_report(
            acceptance,
            arguments.provider,
            arguments.live,
            arguments.cli,
            arguments.runtime,
            arguments.generated,
        )
        artifact_manifests = (
            artifact_journey.cli.validate(arguments.cli),
            artifact_journey.packages.validate_runtime(arguments.runtime),
            artifact_journey.packages.validate_generated(arguments.generated),
        )
        if {manifest["source-commit"] for manifest in artifact_manifests} != {
            evidence.get("source_commit")
        }:
            reject(
                "artifact_source_identity_drift",
                "producer and all three artifact archives must bind one source commit",
            )
        if arguments.binding != "c":
            surface_manifest = surfaces.validate_surface(
                arguments.generated_surface, arguments.binding
            )
            if (
                surface_manifest["source-commit"] != evidence["source_commit"]
                or surface_manifest["cli-artifact-id"] != artifact_manifests[0]["artifact-id"]
            ):
                reject(
                    "generated_surface_source_drift",
                    "generated surface does not bind the exact CLI and source commit",
                )
        predecessors = sorted(arguments.predecessor)
        if tuple(version for version, _ in predecessors) != conformance.predecessor_versions(
            arguments.binding
        ):
            reject(
                "predecessor_report_scope_drift",
                "predecessor arguments do not match the binding's history",
            )
        report = assemble_report(
            evidence,
            binding=arguments.binding,
            run_nonce=arguments.run_nonce,
            acceptance=acceptance,
            acceptance_sha256=hashlib.sha256(arguments.acceptance.read_bytes()).hexdigest(),
            generated_surface=read_regular(
                arguments.generated_surface, MAX_SURFACE_BYTES, "generated surface"
            ),
            predecessor_reports=[
                read_regular(path, MAX_EVIDENCE_BYTES, f"V{version} report")
                for version, path in predecessors
            ],
        )
        publish(arguments.output, report)
    except (
        AssemblyError,
        conformance.ContractError,
        artifact_journey.JourneyError,
        OSError,
        json.JSONDecodeError,
    ) as error:
        code = getattr(error, "code", "assembly_failure")
        print(f"sdk-v6 assembly rejected [{code}]: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
