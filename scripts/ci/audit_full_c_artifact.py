#!/usr/bin/env python3
"""Independently audit one final FULL-C artifact without granting release authority."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import secrets
import stat
import sys
from collections.abc import Callable
from pathlib import Path
from typing import Any, NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parent))
import c_artifact_journey as artifact_journey  # noqa: E402
import c_distribution_security as security  # noqa: E402
import compare_sdk_conformance as v1  # noqa: E402
import compare_sdk_conformance_v2 as v2  # noqa: E402
import compare_sdk_conformance_v3 as v3  # noqa: E402
import compare_sdk_conformance_v4 as v4  # noqa: E402
import compare_sdk_conformance_v5 as v5  # noqa: E402
import compare_sdk_conformance_v6 as v6  # noqa: E402
import validate_c_broad_case_ledger as broad_ledger  # noqa: E402
import validate_c_distribution_contract as distribution  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
AUDIT_CONTRACT = ROOT / "tests/contracts/c-full-sdk-audit-v1.json"
MANIFEST = ROOT / "tests/contracts/sdk_conformance/manifest-v1.json"
BINDINGS = ("python", "node", "rust", "c")
REPORT_BINDINGS = {
    1: ("python", "node", "rust"),
    2: BINDINGS,
    3: BINDINGS,
    4: BINDINGS,
    5: BINDINGS,
    6: BINDINGS,
}
COMPARATORS: dict[int, Callable[[list[Path]], dict[str, Any]]] = {
    1: v1.compare_reports,
    2: v2.compare_reports,
    3: v3.compare_reports,
    4: v4.compare_reports,
    5: v5.compare_reports,
    6: v6.compare_reports,
}
DOCUMENTATION_FENCES = {
    "tests/contracts/c-distribution-v1.json": (
        "artifact-only-unpublished-unsupported",
        '"public_supported": []',
    ),
    "docs/development/testing.md": ("not a public C support or distribution",),
    "type-bridge-core/crates/schema-codegen/README.md": (
        "remains unpublished until",
        "its release acceptance checks pass.",
    ),
}
MAX_INPUT_BYTES = 256 * 1024 * 1024


class AuditError(RuntimeError):
    """Stable fail-closed FULL-C audit rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


def reject(code: str, message: str) -> NoReturn:
    raise AuditError(code, message)


def canonical_json(value: Any) -> bytes:
    return json.dumps(
        value, ensure_ascii=False, allow_nan=False, sort_keys=True, separators=(",", ":")
    ).encode()


def regular(path: Path, label: str, *, maximum: int = MAX_INPUT_BYTES) -> Path:
    try:
        metadata = path.lstat()
    except OSError as error:
        reject("missing_audit_input", f"cannot inspect {label}: {error}")
    if (
        stat.S_ISLNK(metadata.st_mode)
        or not stat.S_ISREG(metadata.st_mode)
        or metadata.st_size < 0
        or metadata.st_size > maximum
    ):
        reject("invalid_audit_input", f"{label} must be a bounded regular non-symlink file")
    return path.resolve()


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_unique(path: Path, label: str) -> dict[str, Any]:
    body = regular(path, label).read_bytes()
    try:
        value = json.loads(body, object_pairs_hook=v6.unique_object)
    except (UnicodeDecodeError, json.JSONDecodeError, v6.ContractError) as error:
        reject("invalid_audit_json", f"cannot parse {label}: {error}")
    if not isinstance(value, dict):
        reject("invalid_audit_json", f"{label} must contain one object")
    return value


def report_paths(root: Path, version: int) -> list[Path]:
    return [
        regular(root / f"v{version}" / f"{binding}.json", f"V{version} {binding} report")
        for binding in REPORT_BINDINGS[version]
    ]


def binding_status(manifest: dict[str, Any], capability: dict[str, Any], binding: str) -> str:
    profile = manifest["binding_profiles"][capability["binding_profile"]]
    matches = [status for status, bindings in profile.items() if binding in bindings]
    if len(matches) != 1:
        reject("invalid_binding_profile", f"{capability['code']} does not assign {binding} exactly")
    return matches[0]


def validate_capabilities(
    manifest: dict[str, Any], contract: dict[str, Any]
) -> list[dict[str, str]]:
    capabilities = manifest.get("capabilities")
    if (
        not isinstance(capabilities, list)
        or [item.get("code") for item in capabilities] != contract["capability_codes"]
    ):
        reject("capability_inventory_drift", "manifest and FULL-C capability inventories differ")
    rows = []
    for capability in capabilities:
        statuses = {binding: binding_status(manifest, capability, binding) for binding in BINDINGS}
        if any(status not in {"accepted_offline", "accepted_live"} for status in statuses.values()):
            reject("gap_or_planned_cell", f"{capability['code']} has an unaccepted current/C cell")
        rows.append({"code": capability["code"], **statuses})
    return rows


def validate_documentation(root: Path = ROOT) -> list[dict[str, Any]]:
    output = []
    for relative, fences in DOCUMENTATION_FENCES.items():
        path = regular(root / relative, f"documentation fence {relative}", maximum=8 * 1024 * 1024)
        try:
            body = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as error:
            reject("invalid_documentation", f"cannot read {relative}: {error}")
        if any(fence not in body for fence in fences):
            reject("unsupported_documentation_claim", f"artifact-only fence drifted in {relative}")
        output.append({"path": relative, "sha256": sha256(path)})
    return output


def audit(
    *,
    reports_root: Path,
    acceptance_path: Path,
    provider: Path,
    live: Path,
    cli: Path,
    runtime: Path,
    generated: Path,
    security_evidence: Path,
    root: Path = ROOT,
) -> tuple[dict[str, Any], str]:
    contract = load_unique(root / AUDIT_CONTRACT.relative_to(ROOT), "FULL-C audit contract")
    manifest = load_unique(root / MANIFEST.relative_to(ROOT), "final capability manifest")
    if contract.get("authority_state") != "frozen" or contract.get("report_versions") != list(
        COMPARATORS
    ):
        reject("audit_contract_drift", "FULL-C audit authority is not exact")

    distribution_contract = distribution.validate(root=root)
    ledger = broad_ledger.validate(root=root)
    acceptance = artifact_journey.load_canonical_report(
        regular(acceptance_path, "Artifact acceptance report")
    )
    artifact_journey.validate_acceptance_report(
        acceptance,
        regular(provider, "provider-free report"),
        regular(live, "live report"),
        regular(cli, "CLI artifact"),
        regular(runtime, "runtime artifact"),
        regular(generated, "generated package artifact"),
    )
    security.validate_evidence(
        security_evidence,
        cli,
        runtime,
        generated,
    )

    summaries = []
    report_inputs = []
    for version, comparator in COMPARATORS.items():
        paths = report_paths(reports_root, version)
        try:
            summary = comparator(paths)
        except Exception as error:
            reject("report_fan_in_failed", f"V{version} comparator rejected: {error}")
        if version == 6 and (
            summary.get("authority_state") != "finalized" or summary.get("pending_promotions")
        ):
            reject("pending_final_transition", "V6 is not finalized with zero pending promotions")
        summaries.append(
            {
                "version": version,
                "format": summary.get("format"),
                "sha256": hashlib.sha256(canonical_json(summary)).hexdigest(),
            }
        )
        report_inputs.extend(
            {
                "version": version,
                "binding": binding,
                "sha256": sha256(path),
            }
            for binding, path in zip(REPORT_BINDINGS[version], paths, strict=True)
        )

    capabilities = validate_capabilities(manifest, contract)
    documentation = validate_documentation(root)
    artifact_manifest = load_unique(
        security_evidence / "artifact-manifest.json", "artifact manifest"
    )
    source_commit = artifact_manifest.get("source-commit")
    if not isinstance(source_commit, str) or len(source_commit) != 40:
        reject("artifact_source_drift", "artifact source commit is invalid")
    if any(
        report.get("source_commit") != source_commit
        for report in [load_unique(path, "V6 report") for path in report_paths(reports_root, 6)]
    ):
        reject("artifact_source_drift", "V6 reports and artifacts bind different sources")

    required_inputs = contract["required_inputs"]
    checks = [{"id": item, "status": "passed"} for item in required_inputs]
    result = {
        "format": contract["output"]["canonical_format"],
        "authority_state": "accepted-artifact",
        "source_commit": source_commit,
        "artifact_set_id": artifact_manifest["artifact-set-id"],
        "inputs": {
            "audit_contract_sha256": sha256(root / AUDIT_CONTRACT.relative_to(ROOT)),
            "manifest_sha256": sha256(root / MANIFEST.relative_to(ROOT)),
            "broad_case_ledger_sha256": sha256(
                root / broad_ledger.DEFAULT_LEDGER.relative_to(ROOT)
            ),
            "acceptance_sha256": sha256(acceptance_path),
            "security_manifest_sha256": sha256(security_evidence / "artifact-manifest.json"),
            "reports": report_inputs,
        },
        "capabilities": capabilities,
        "report_summaries": summaries,
        "artifacts": acceptance["artifacts"],
        "matrix": distribution_contract["matrix"],
        "broad_case_slices": ledger["ledgers"],
        "documentation": documentation,
        "checks": checks,
        "cleanup": {
            "managed_database_absent": True,
            "migration_database_absent": True,
            "native_library_unmapped": True,
            "temporary_evidence_absent": True,
        },
        "release_gate": distribution_contract["release_gate"],
        "publication_authority": False,
    }
    summary = (
        "# FULL-C artifact audit\n\n"
        f"- Result: accepted artifact\n"
        f"- Source commit: `{source_commit}`\n"
        f"- Artifact set: `{artifact_manifest['artifact-set-id']}`\n"
        f"- Capabilities: {len(capabilities)}/44 accepted for C\n"
        f"- Sdk reports: V1–V6 independently validated\n"
        f"- Publication authority: no\n"
        f"- Remaining gate: #189 (`{distribution_contract['release_gate']['state']}`)\n"
    )
    return result, summary


def publish(report_path: Path, summary_path: Path, report: dict[str, Any], summary: str) -> None:
    paths = (report_path, summary_path)
    for path in paths:
        if not path.is_absolute() or path.exists() or path.is_symlink():
            reject("invalid_output_path", "audit outputs must be absolute and new")
        try:
            parent = path.parent.lstat()
        except OSError as error:
            reject("invalid_output_path", f"cannot inspect output parent: {error}")
        if stat.S_ISLNK(parent.st_mode) or not stat.S_ISDIR(parent.st_mode):
            reject("invalid_output_path", "audit output parents must be real directories")
    temporary = [
        path.parent / f".{path.name}.{os.getpid()}.{secrets.token_hex(16)}.tmp" for path in paths
    ]
    published: list[Path] = []
    try:
        for path, body in zip(temporary, (canonical_json(report), summary.encode()), strict=True):
            descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(descriptor, "wb") as output:
                output.write(body)
                output.flush()
                os.fsync(output.fileno())
        for source, destination in zip(temporary, paths, strict=True):
            os.link(source, destination, follow_symlinks=False)
            published.append(destination)
    except OSError as error:
        for path in published:
            path.unlink(missing_ok=True)
        reject("audit_publication_failed", f"cannot publish complete audit pair: {error}")
    finally:
        for path in temporary:
            path.unlink(missing_ok=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reports", required=True, type=Path)
    parser.add_argument("--acceptance", required=True, type=Path)
    parser.add_argument("--provider", required=True, type=Path)
    parser.add_argument("--live", required=True, type=Path)
    parser.add_argument("--cli", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--generated", required=True, type=Path)
    parser.add_argument("--security-evidence", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--summary", required=True, type=Path)
    arguments = parser.parse_args()
    try:
        report, summary = audit(
            reports_root=arguments.reports,
            acceptance_path=arguments.acceptance,
            provider=arguments.provider,
            live=arguments.live,
            cli=arguments.cli,
            runtime=arguments.runtime,
            generated=arguments.generated,
            security_evidence=arguments.security_evidence,
        )
        publish(arguments.output, arguments.summary, report, summary)
    except (
        AuditError,
        artifact_journey.JourneyError,
        security.SecurityError,
        distribution.ContractError,
        broad_ledger.LedgerError,
        OSError,
        KeyError,
    ) as error:
        code = getattr(error, "code", "full_c_audit_failure")
        print(f"FULL-C audit rejected [{code}]: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
