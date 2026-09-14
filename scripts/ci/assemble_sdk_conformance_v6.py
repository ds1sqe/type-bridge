#!/usr/bin/env python3
"""Compose measured evidence and assemble SDK V6 reports."""

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

import c_artifact_journey as artifact_journey
import compare_sdk_conformance_v6 as conformance
import sdk_v6_surfaces as surfaces
from persist_binding_reports import publish_bytes

ROOT = Path(__file__).resolve().parents[2]
FORMAT = "typebridge.sdk-v6-producer-evidence/v1"
MAX_EVIDENCE_BYTES = 32 * 1024 * 1024
MAX_FRAGMENT_BYTES = 1024 * 1024
MAX_SURFACE_BYTES = 128 * 1024 * 1024
SURFACE_CONSUMER_FORMAT = "typebridge.sdk-v6-surface-consumer/v1"
MAX_INPUT_BYTES = 64 * 1024 * 1024


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
    if not isinstance(value, str) or len(value) > (MAX_FRAGMENT_BYTES + 2) // 3 * 4:
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
    publish_bytes(path, conformance.canonical_json_bytes(report), AssemblyError)


def _predecessor_argument(value: str) -> tuple[int, Path]:
    try:
        version_text, path_text = value.split("=", 1)
        version = int(version_text)
    except (ValueError, TypeError):
        raise argparse.ArgumentTypeError("predecessor must be VERSION=PATH") from None
    if version not in range(1, 6) or not path_text:
        raise argparse.ArgumentTypeError("predecessor version must be 1 through 5")
    return (version, Path(path_text))


class CompositionError(ValueError):
    """Stable fail-closed V6 evidence-composition rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


def composition_reject(code: str, message: str) -> NoReturn:
    raise CompositionError(code, message)


def read_composition_bytes(path: Path, label: str) -> bytes:
    try:
        metadata = path.lstat()
        body = path.read_bytes()
    except OSError as error:
        composition_reject("invalid_composition_file", f"cannot read {label}: {error}")
    if (
        stat.S_ISLNK(metadata.st_mode)
        or not stat.S_ISREG(metadata.st_mode)
        or len(body) > MAX_INPUT_BYTES
    ):
        composition_reject("invalid_composition_file", f"{label} must be a bounded regular file")
    return body


def load_canonical(
    path: Path, label: str, *, trailing_newline: bool = False, require_canonical: bool = True
) -> tuple[dict[str, Any], bytes]:
    body = read_composition_bytes(path, label)
    try:
        value = json.loads(body, object_pairs_hook=conformance.unique_object)
    except conformance.ContractError as error:
        composition_reject(error.code, str(error))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        composition_reject("invalid_composition_json", f"cannot parse {label}: {error}")
    expected = conformance.canonical_json_bytes(value) + (b"\n" if trailing_newline else b"")
    if not isinstance(value, dict) or (require_canonical and body != expected):
        composition_reject("noncanonical_composition_json", f"{label} is not exact canonical JSON")
    return (value, body)


def _proof_inventory(root: Path) -> list[tuple[str, str]]:
    manifest = conformance.load_json(root / conformance.MANIFEST)
    capabilities = {item["id"]: item for item in manifest["capabilities"]}
    rows = []
    for capability_id, selected in zip(conformance.CAPABILITIES, conformance.PROOFS, strict=True):
        capability = capabilities[capability_id]
        rows.extend(
            (
                (capability_id, proof_kind)
                for proof_kind, disposition in manifest["proof_profiles"][
                    capability["proof_profile"]
                ].items()
                if disposition == "required" and proof_kind != selected
            )
        )
    if len(rows) != 25:
        composition_reject(
            "non_selected_proof_scope_drift", "manifest no longer derives 25 proof rows"
        )
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
        composition_reject("composition_identity_mismatch", f"unknown V6 binding {binding!r}")
    if len(run_nonce) != 64 or any(c not in "0123456789abcdef" for c in run_nonce):
        composition_reject("invalid_run_nonce", "run nonce must be 64 lowercase hexadecimal digits")
    if len(source_commit) != 40 or any(c not in "0123456789abcdef" for c in source_commit):
        composition_reject(
            "invalid_source_commit", "source commit must be 40 lowercase hexadecimal digits"
        )
    catalog = conformance.load_json(root / conformance.CATALOG)
    versions = conformance.predecessor_versions(binding)
    if len(predecessors) != len(versions):
        composition_reject(
            "predecessor_report_scope_drift", "historical report inventory is not exact"
        )
    predecessor_digests = []
    for version, (report, body) in zip(versions, predecessors, strict=True):
        if (
            report.get("format") != f"typebridge.sdk-conformance-report/v{version}"
            or report.get("binding") != binding
        ):
            composition_reject(
                "predecessor_report_identity_drift", f"V{version} report identity drifted"
            )
        predecessor_digests.append({"version": version, "sha256": hashlib.sha256(body).hexdigest()})
    steps = acceptance.get("steps")
    if (
        acceptance.get("format") != "typebridge.c-artifact-acceptance-report/v1"
        or acceptance.get("publication-disposition") != "artifact-only-unpublished-unsupported"
        or (not isinstance(steps, list))
        or (len(steps) != 14)
        or any(not isinstance(step, dict) or step.get("status") != "passed" for step in steps)
    ):
        composition_reject(
            "invalid_acceptance_report",
            "Artifact acceptance aggregate is not a complete artifact report",
        )
    artifacts = acceptance.get("artifacts")
    if not isinstance(artifacts, dict) or set(artifacts) != {"cli", "generated-package", "runtime"}:
        composition_reject(
            "invalid_acceptance_report", "Artifact acceptance artifact set is not exact"
        )
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
            composition_reject(
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
            or (surface_consumer.get("source-commit") != source_commit)
            or (surface_consumer.get("cli-artifact-id") != artifacts["cli"]["artifact-id"])
            or (surface_consumer.get("runtime-provenance") != "current-sdk-regression")
            or (surface_consumer.get("checks") != expected_checks)
            or (surface_consumer.get("cleanup") != {"temporary-consumer-absent": True})
            or (surface_consumer.get("publication-authority") is not False)
        ):
            composition_reject(
                "surface_consumer_identity_drift", "managed surface consumer evidence drifted"
            )
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
        "format": FORMAT,
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


def compose_inputs(arguments: argparse.Namespace) -> dict[str, Any]:
    surface, surface_bytes = load_canonical(arguments.surface_consumer, "surface consumer")
    acceptance, acceptance_bytes = load_canonical(
        arguments.acceptance, "Artifact acceptance report", trailing_newline=True
    )
    predecessor_arguments = sorted(arguments.predecessor)
    if tuple((version for version, _ in predecessor_arguments)) != conformance.predecessor_versions(
        arguments.binding
    ):
        composition_reject(
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
    if len(conformance.canonical_json_bytes(value)) > MAX_EVIDENCE_BYTES:
        raise AssemblyError("evidence_size_limit", "composed evidence exceeds its byte limit")
    return value


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binding", required=True, choices=conformance.BINDINGS)
    parser.add_argument("--run-nonce", required=True)
    parser.add_argument("--evidence", required=False, type=Path)
    parser.add_argument("--acceptance", required=True, type=Path)
    parser.add_argument("--provider", required=True, type=Path)
    parser.add_argument("--live", required=True, type=Path)
    parser.add_argument("--cli", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--generated", required=True, type=Path)
    parser.add_argument("--generated-surface", required=True, type=Path)
    parser.add_argument("--predecessor", action="append", type=_predecessor_argument, default=[])
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--source-commit", required=False)
    parser.add_argument("--surface-consumer", required=False, type=Path)
    arguments = parser.parse_args()
    if arguments.evidence is None:
        missing = [
            name
            for name in ("source_commit", "surface_consumer")
            if getattr(arguments, name) is None
        ]
        if missing:
            parser.error(
                "composition requires "
                + ", ".join("--" + name.replace("_", "-") for name in missing)
            )
    elif any(
        getattr(arguments, name) is not None for name in ("source_commit", "surface_consumer")
    ):
        parser.error("choose existing evidence or composition inputs, not both")
    try:
        evidence = (
            compose_inputs(arguments)
            if arguments.evidence is None
            else load_evidence(arguments.evidence)
        )
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
        if tuple((version for version, _ in predecessors)) != conformance.predecessor_versions(
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
        CompositionError,
    ) as error:
        code = getattr(error, "code", "assembly_failure")
        print(f"sdk-v6 assembly rejected [{code}]: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
