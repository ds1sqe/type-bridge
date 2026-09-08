#!/usr/bin/env python3
"""Validate and compare four artifact-bound Sdk V6 artifact reports."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path
from typing import Any, NoReturn

ROOT = Path(__file__).resolve().parents[2]
MANIFEST = "tests/contracts/sdk_conformance/manifest-v1.json"
CATALOG = "tests/contracts/sdk_conformance/sdk-v6/catalog-v6.json"
JOURNEY = "tests/contracts/sdk_conformance/sdk-v6/journey-v6.json"
LEDGER = "tests/contracts/c-broad-case-ledger-v1.json"
REPORT_SCHEMA = "tests/contracts/sdk_conformance/sdk-v6/report-schema-v6.json"
PRODUCER_SCHEMA = "tests/contracts/sdk_conformance/sdk-v6/producer-evidence-schema-v6.json"
BINDINGS = ("python", "node", "rust", "c")
CASES = (
    "sdk.runtime.cancellation",
    "sdk.runtime.timeout-resource-limits",
    "sdk.diagnostic.all-workflows",
    "sdk.distribution.standalone-cli",
    "sdk.runtime.explicit-close",
)
PROOFS = ("direct_runtime", "direct_runtime", "diagnostic", "artifact", "lifecycle")
CAPABILITIES = (
    "runtime.cancellation",
    "runtime.timeout-and-resource-limits",
    "diagnostic.all-workflows-structured",
    "distribution.standalone-cli",
    "runtime.explicit-close",
)
GAP_PROFILE = "current_gap_future_planned"
BROAD_PROFILE = "terminal_broad_live_future_planned"
DISTRIBUTION_PROFILE = "standalone_distribution_offline_future_planned"
SURFACE_FORMATS = {
    "python": "typebridge.python-generated-wheel/v1",
    "node": "typebridge.node-generated-tarball/v1",
    "rust": "typebridge.rust-generated-crate/v1",
    "c": "typebridge.c-generated-package/v1",
}


def predecessor_versions(binding: str) -> tuple[int, ...]:
    """Return the real historical report inventory for one binding."""
    if binding not in BINDINGS:
        reject("predecessor_report_identity_drift", f"unknown predecessor binding {binding!r}")
    return (2, 3, 4, 5) if binding == "c" else (1, 2, 3, 4, 5)


class ContractError(ValueError):
    """Stable fail-closed V6 rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


def reject(code: str, message: str) -> NoReturn:
    raise ContractError(code, message)


def unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            reject("duplicate_json_key", f"duplicate JSON key {key!r}")
        value[key] = item
    return value


def canonical_json_bytes(value: Any) -> bytes:
    return json.dumps(
        value, ensure_ascii=False, allow_nan=False, sort_keys=True, separators=(",", ":")
    ).encode()


def historical_report_is_exact(raw: bytes, value: Any, version: int) -> bool:
    """Apply only the byte-canonicality requirement owned by each predecessor."""
    if version == 4:
        # V4 deliberately validates structure but does not define a canonical JSON spelling.
        return True
    body = canonical_json_bytes(value)
    expected = body + b"\n" if version in (1, 2, 3) else body
    return raw == expected


def load_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_bytes(), object_pairs_hook=unique_object)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        reject("invalid_json_source", f"cannot load {path}: {error}")
    if not isinstance(value, dict):
        reject("invalid_json_shape", f"{path} must contain one object")
    return value


def load_report(path: Path) -> dict[str, Any]:
    try:
        raw = path.read_bytes()
    except OSError as error:
        reject("invalid_json_source", f"cannot read {path}: {error}")
    report = load_json(path)
    if raw != canonical_json_bytes(report):
        reject("noncanonical_report_json", f"{path} is not canonical JSON")
    return report


def sha256(path: Path) -> str:
    try:
        return hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError as error:
        reject("unreadable_source", f"cannot read {path}: {error}")


def exact_keys(value: Any, keys: set[str], label: str) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != keys:
        reject("invalid_report_shape", f"{label} keys are not exact")
    return value


def exact_sha(value: Any, label: str) -> None:
    if (
        not isinstance(value, str)
        or len(value) != 64
        or any(c not in "0123456789abcdef" for c in value)
    ):
        reject("invalid_digest", f"{label} is not lowercase SHA-256")


def exact_artifact_id(value: Any, label: str) -> None:
    if not isinstance(value, str) or not value.startswith("sha256:"):
        reject("invalid_artifact_identity", f"{label} is not a SHA-256 artifact ID")
    exact_sha(value.removeprefix("sha256:"), label)


def identity(value: Any, relative: str, root: Path, label: str) -> None:
    item = exact_keys(value, {"path", "sha256"}, label)
    if item["path"] != relative or item["sha256"] != sha256(root / relative):
        reject("stale_source_identity", f"{label} identity is stale")


def validate_observation(index: int, value: Any) -> None:
    expected = (
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
        None,
        {
            "explicit_close": True,
            "native_library_unmapped": True,
            "parent_child_order": True,
            "post_close_rejected": True,
            "repeat_close": True,
            "sibling_independent": True,
        },
    )[index]
    if index == 3:
        item = exact_keys(
            value,
            {"artifact-id", "offline_commands", "relocated", "sdk_independent", "sha256"},
            "standalone CLI observation",
        )
        exact_sha(item["sha256"], "standalone CLI digest")
        exact_artifact_id(item["artifact-id"], "standalone CLI artifact ID")
        if item["offline_commands"] != 5 or any(
            item[field] is not True for field in ("relocated", "sdk_independent")
        ):
            reject("invalid_observation", "standalone CLI observation did not pass")
    elif value != expected:
        reject("invalid_observation", f"terminal observation {index} drifted")


def validate_report(report: dict[str, Any], root: Path = ROOT) -> None:
    exact_keys(
        report,
        {
            "format",
            "binding",
            "producer",
            "source_commit",
            "manifest",
            "catalog",
            "journey",
            "broad_case_ledger",
            "acceptance_report_sha256",
            "artifacts",
            "generated_surface",
            "predecessor_reports",
            "non_selected_proofs",
            "results",
            "cleanup",
            "publication_authority",
        },
        "report",
    )
    if (
        report["format"] != "typebridge.sdk-conformance-report/v6"
        or report["binding"] not in BINDINGS
    ):
        reject("invalid_report_identity", "report format or binding is invalid")
    catalog = load_json(root / CATALOG)
    if report["producer"] != catalog["artifact_report_producers"][report["binding"]]:
        reject("producer_identity_mismatch", "producer is not frozen by V6")
    commit = report["source_commit"]
    if (
        not isinstance(commit, str)
        or len(commit) != 40
        or any(c not in "0123456789abcdef" for c in commit)
    ):
        reject("invalid_source_commit", "source commit is not exact")
    identity(report["manifest"], MANIFEST, root, "manifest")
    identity(report["catalog"], CATALOG, root, "catalog")
    identity(report["journey"], JOURNEY, root, "journey")
    identity(report["broad_case_ledger"], LEDGER, root, "broad-case ledger")
    exact_sha(report["acceptance_report_sha256"], "Artifact acceptance report")
    artifacts = exact_keys(
        report["artifacts"], {"cli", "generated-package", "runtime"}, "artifact set"
    )
    for name, artifact in artifacts.items():
        item = exact_keys(artifact, {"artifact-id", "sha256"}, f"{name} artifact")
        exact_sha(item["sha256"], f"{name} artifact")
        exact_artifact_id(item["artifact-id"], f"{name} artifact ID")
    surface = exact_keys(
        report["generated_surface"], {"format", "artifact-id", "sha256"}, "generated surface"
    )
    exact_sha(surface["sha256"], "generated surface")
    exact_artifact_id(surface["artifact-id"], "generated surface artifact ID")
    if surface["format"] != SURFACE_FORMATS[report["binding"]]:
        reject("invalid_generated_surface", "generated surface identity is invalid")
    if report["binding"] == "c" and surface != {
        "format": SURFACE_FORMATS["c"],
        **artifacts["generated-package"],
    }:
        reject(
            "c_generated_surface_drift",
            "C generated surface differs from Artifact acceptance artifact",
        )
    predecessors = report["predecessor_reports"]
    expected_predecessors = predecessor_versions(report["binding"])
    if (
        not isinstance(predecessors, list)
        or tuple(item.get("version") for item in predecessors if isinstance(item, dict))
        != expected_predecessors
    ):
        reject(
            "predecessor_report_scope_drift",
            f"predecessor inventory is not exact for {report['binding']}",
        )
    for item in predecessors:
        exact_keys(item, {"version", "sha256"}, "predecessor report")
        exact_sha(item["sha256"], "predecessor report")
    manifest = load_json(root / MANIFEST)
    capability_by_id = {item["id"]: item for item in manifest["capabilities"]}
    expected_non_selected = []
    for capability_id, selected_proof in zip(CAPABILITIES, PROOFS, strict=True):
        capability = capability_by_id[capability_id]
        profile = manifest["proof_profiles"][capability["proof_profile"]]
        expected_non_selected.extend(
            (capability_id, proof_kind)
            for proof_kind, disposition in profile.items()
            if disposition == "required" and proof_kind != selected_proof
        )
    proofs = report["non_selected_proofs"]
    if not isinstance(proofs, list) or len(proofs) != 25:
        reject("non_selected_proof_scope_drift", "V6 must contain 25 non-selected proofs")
    actual_non_selected = []
    for item in proofs:
        exact_keys(item, {"capability_id", "proof_kind", "evidence_sha256"}, "proof ledger row")
        exact_sha(item["evidence_sha256"], "non-selected proof evidence")
        actual_non_selected.append((item["capability_id"], item["proof_kind"]))
    if actual_non_selected != expected_non_selected:
        reject("non_selected_proof_inventory_drift", "non-selected proof inventory drifted")
    results = report["results"]
    if not isinstance(results, list) or len(results) != 5:
        reject("report_row_scope_drift", "V6 must contain five results")
    for index, result in enumerate(results):
        exact_keys(
            result, {"case_id", "capability_id", "proof_kind", "outcome", "observation"}, "result"
        )
        if (
            result["case_id"],
            result["capability_id"],
            result["proof_kind"],
            result["outcome"],
        ) != (CASES[index], CAPABILITIES[index], PROOFS[index], "passed"):
            reject("report_row_binding_drift", f"V6 result {index} is rebound")
        validate_observation(index, result["observation"])
    if (
        results[3]["observation"]["artifact-id"] != artifacts["cli"]["artifact-id"]
        or results[3]["observation"]["sha256"] != artifacts["cli"]["sha256"]
    ):
        reject("standalone_cli_identity_drift", "G12 observation differs from the shared CLI")
    if report["cleanup"] != {
        "managed_database_absent": True,
        "migration_database_absent": True,
        "native_library_unmapped": True,
        "temporary_evidence_absent": True,
    }:
        reject("cleanup_failure", "V6 cleanup invariants did not pass")
    if report["publication_authority"] is not False:
        reject("authority_widening", "V6 report grants publication authority")


def compare_reports(paths: list[Path], root: Path = ROOT) -> dict[str, Any]:
    load_json(root / REPORT_SCHEMA)
    load_json(root / PRODUCER_SCHEMA)
    catalog = load_json(root / CATALOG)
    if (
        catalog.get("report_schema_path") != REPORT_SCHEMA
        or catalog.get("producer_evidence_schema_path") != PRODUCER_SCHEMA
    ):
        reject("evidence_schema_inventory_drift", "V6 evidence schema paths drifted")
    if catalog.get("manifest_transition_cases") != list(CASES):
        reject("transition_scope_drift", "V6 transition cases drifted")
    if len(paths) != 4:
        reject("missing_binding_report", "exactly four V6 reports are required")
    reports: dict[str, dict[str, Any]] = {}
    shared: tuple[Any, ...] | None = None
    source_commit: str | None = None
    for path in paths:
        report = load_report(path)
        validate_report(report, root)
        binding = report["binding"]
        if binding in reports:
            reject("duplicate_binding_report", f"duplicate {binding} report")
        common = (
            report["source_commit"],
            report["acceptance_report_sha256"],
            report["artifacts"],
            tuple(result["observation"] for result in report["results"]),
        )
        if shared is None:
            shared = common
            source_commit = report["source_commit"]
        elif common != shared:
            reject("cross_binding_evidence_drift", "V6 shared evidence differs by binding")
        reports[binding] = report
    if tuple(reports) != BINDINGS:
        reject("binding_order_drift", "V6 report order is not canonical")

    manifest = load_json(root / MANIFEST)
    capabilities = {item["case_ids"][0]: item for item in manifest["capabilities"]}
    profiles = tuple(capabilities[case]["binding_profile"] for case in CASES)
    pending = [
        case for case, profile in zip(CASES, profiles, strict=True) if profile == GAP_PROFILE
    ]
    final_profiles = (
        BROAD_PROFILE,
        BROAD_PROFILE,
        BROAD_PROFILE,
        DISTRIBUTION_PROFILE,
        BROAD_PROFILE,
    )
    if profiles not in ((GAP_PROFILE,) * 5, final_profiles):
        reject("partial_manifest_transition", "V6 transition is partial or uses the wrong profiles")
    return {
        "format": "typebridge.sdk-v6-comparison/v1",
        "authority_state": "artifact" if pending else "finalized",
        "bindings": list(BINDINGS),
        "source_commit": source_commit,
        "manifest_transition_cases": list(CASES),
        "pending_promotions": pending,
        "publication_authority": False,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reports", nargs="*", type=Path)
    arguments = parser.parse_args()
    try:
        comparison = compare_reports(arguments.reports)
    except ContractError as error:
        print(f"sdk-v6 conformance rejected [{error.code}]: {error}", file=sys.stderr)
        return 1
    print(canonical_json_bytes(comparison).decode())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
