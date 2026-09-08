#!/usr/bin/env python3
"""Validate the frozen, non-publishing Plan 08 C distribution contract."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_CONTRACT = ROOT / "tests/contracts/c-distribution-v1.json"
V6_CATALOG = ROOT / "tests/contracts/sdk_conformance/workforce-v6/catalog-v6.json"
MANIFEST = ROOT / "tests/contracts/sdk_conformance/manifest-v1.json"
FULL_C_AUDIT = ROOT / "tests/contracts/c-full-sdk-audit-v1.json"
TRANSITION_CASES = [
    "workforce.runtime.cancellation",
    "workforce.runtime.timeout-resource-limits",
    "workforce.diagnostic.all-workflows",
    "workforce.distribution.standalone-cli",
    "workforce.runtime.explicit-close",
]


class ContractError(ValueError):
    """The distribution authority is malformed, stale, or unsafe."""


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ContractError(f"duplicate key: {key}")
        result[key] = value
    return result


def _load(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=_unique_object)
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ContractError(f"cannot read {path}: {error}") from error
    if not isinstance(value, dict):
        raise ContractError(f"{path} must contain one JSON object")
    return value


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise ContractError(message)


def validate(contract_path: Path = DEFAULT_CONTRACT, root: Path = ROOT) -> dict[str, Any]:
    contract = _load(contract_path)
    _require(contract.get("format") == "typebridge.c-distribution/v1", "format drifted")
    _require(contract.get("authority_state") == "frozen", "authority is not frozen")
    _require(
        contract.get("publication_disposition") == "candidate-only-unpublished-unsupported",
        "publication disposition widened",
    )

    gate = contract.get("release_gate")
    _require(isinstance(gate, dict) and gate.get("issue") == 189, "#189 gate is missing")
    _require(gate.get("state") == "awaiting-release-authorization", "#189 state drifted")
    for field in ("landing_authorized", "publication_authorized", "public_support_authorized"):
        _require(gate.get(field) is False, f"{field} must remain false")

    abi = contract.get("abi")
    _require(isinstance(abi, dict) and abi.get("decision") == "no-change", "ABI decision drifted")
    _require(abi.get("version") == "1.6.0", "distribution must retain ABI 1.6")
    _require(abi.get("static_linkage") == "unsupported", "static linkage was not selected")
    _require(abi.get("shared_linkage") == "candidate", "shared candidate linkage is missing")

    compatibility = contract.get("compatibility")
    _require(isinstance(compatibility, dict), "compatibility contract is missing")
    _require(
        compatibility.get("runtime_abi")
        == {"minimum_inclusive": "1.6.0", "maximum_exclusive": "2.0.0"},
        "runtime ABI range drifted",
    )
    _require(
        compatibility.get("provider_io_on_failure") is False, "failed handshake may perform I/O"
    )
    _require(
        compatibility.get("failure_publication") == "none", "failed handshake may publish output"
    )

    matrix = contract.get("matrix")
    _require(isinstance(matrix, dict), "platform matrix is missing")
    supported = matrix.get("candidate_supported")
    _require(isinstance(supported, list) and len(supported) == 1, "candidate matrix widened")
    _require(
        supported[0].get("target") == "x86_64-unknown-linux-gnu", "native prototype target drifted"
    )
    _require(matrix.get("public_supported") == [], "public platform support was claimed")
    _require(
        matrix.get("configured_unverified")
        == ["aarch64-apple-darwin", "x86_64-apple-darwin", "x86_64-pc-windows-msvc"],
        "unverified native matrix drifted",
    )

    policy = contract.get("artifact_policy")
    _require(isinstance(policy, dict), "artifact policy is missing")
    for field in (
        "absolute_paths",
        "parent_traversal",
        "symlinks",
        "case_collisions",
        "post_build_mutation",
        "source_checkout_at_consumer_time",
        "cargo_at_consumer_time",
        "python_at_consumer_time",
        "undeclared_downloads",
    ):
        _require(policy.get(field) is False, f"unsafe artifact policy enabled: {field}")
    _require(policy.get("regular_files_only") is True, "regular-file policy is missing")
    _require(
        policy.get("debug_symbols") == "stripped-no-separate-package",
        "debug-symbol disposition drifted",
    )

    security = contract.get("security_policy")
    _require(isinstance(security, dict), "security policy is missing")
    _require(security.get("cargo_audit_version") == "0.22.2", "cargo-audit pin drifted")
    _require(security.get("vulnerabilities") == "deny-all", "vulnerability policy widened")
    _require(
        security.get("warnings") == "deny-unless-exactly-adjudicated",
        "RustSec warning policy widened",
    )
    adjudications = security.get("adjudications")
    _require(
        isinstance(adjudications, list)
        and len(adjudications) == 1
        and adjudications[0].get("advisory") == "RUSTSEC-2025-0134"
        and adjudications[0].get("package") == "rustls-pemfile"
        and adjudications[0].get("version") == "2.2.0",
        "RustSec adjudication set drifted",
    )
    signature = security.get("protected_signature")
    _require(isinstance(signature, dict), "protected signature policy is missing")
    _require(signature.get("cosign_version") == "3.0.6", "Cosign pin drifted")
    _require(
        signature.get("issuer") == "https://token.actions.githubusercontent.com",
        "signature issuer drifted",
    )
    _require(
        signature.get("candidate_signatures") == "forbidden-before-authorization",
        "candidate signing boundary widened",
    )

    authorities = contract.get("source_authorities")
    _require(isinstance(authorities, list) and authorities, "source authorities are missing")
    seen: set[str] = set()
    for item in authorities:
        _require(
            isinstance(item, dict) and set(item) == {"path", "sha256"}, "invalid source authority"
        )
        relative = item["path"]
        _require(isinstance(relative, str) and relative not in seen, "duplicate source authority")
        seen.add(relative)
        source = root / relative
        _require(
            source.is_file() and not source.is_symlink(),
            f"source authority is not a regular file: {relative}",
        )
        digest = hashlib.sha256(source.read_bytes()).hexdigest()
        _require(digest == item["sha256"], f"stale source authority: {relative}")

    abi_path = root / abi["contract_path"]
    _require(
        hashlib.sha256(abi_path.read_bytes()).hexdigest() == abi["contract_sha256"],
        "ABI contract digest drifted",
    )
    cmake = (root / "type-bridge-core/crates/c/CMakeLists.txt").read_text(encoding="utf-8")
    generated_cmake = (
        root / "type-bridge-core/crates/schema-codegen/src/c/CMakeLists.abi-1-4.txt.in"
    ).read_text(encoding="utf-8")
    generated_pc = (
        root / "type-bridge-core/crates/schema-codegen/src/c/schema.abi-1-4.pc.in"
    ).read_text(encoding="utf-8")
    _require("project(TypeBridge VERSION 1.6.0" in cmake, "runtime CMake ABI version drifted")
    _require(
        "find_package(TypeBridge 1.6 CONFIG REQUIRED)" in generated_cmake,
        "generated CMake range drifted",
    )
    _require(
        "type-bridge >= 1.6.0, type-bridge < 2.0.0" in generated_pc,
        "generated pkg-config range drifted",
    )

    catalog = _load(V6_CATALOG)
    _require(catalog.get("authority_state") == "frozen", "V6 authority is not frozen")
    _require(
        catalog.get("broad_case_ledger_path") == "tests/contracts/c-broad-case-ledger-v1.json",
        "V6 broad-case ledger path drifted",
    )
    _require(
        catalog.get("manifest_transition_cases") == TRANSITION_CASES, "V6 transition set drifted"
    )
    _require(
        catalog.get("required_non_selected_proof_count_per_binding") == 25,
        "V6 non-selected proof count drifted",
    )
    _require(catalog.get("full_c", {}).get("capability_count") == 44, "FULL-C count drifted")
    _require(
        catalog.get("full_c", {}).get("publication_authority") is False,
        "V6 grants publication authority",
    )

    manifest = _load(MANIFEST)
    capabilities = {item["code"]: item for item in manifest["capabilities"]}
    full_c = _load(FULL_C_AUDIT)
    _require(full_c.get("authority_state") == "frozen", "FULL-C authority is not frozen")
    _require(
        full_c.get("capability_codes") == list(capabilities),
        "FULL-C capability inventory disagrees with the manifest",
    )
    _require(full_c.get("report_versions") == [1, 2, 3, 4, 5, 6], "FULL-C report fan-in drifted")
    _require(
        full_c.get("output", {}).get("manual_override") is False, "FULL-C permits manual override"
    )
    _require(
        full_c.get("output", {}).get("publication_authority") is False,
        "FULL-C grants publication authority",
    )
    selected = [
        case
        for code in ("G05", "G06", "G08", "G12", "G13")
        for case in capabilities[code]["case_ids"]
    ]
    _require(selected == TRANSITION_CASES, "V6 cases disagree with the manifest")
    for code in ("G05", "G06", "G08", "G13"):
        _require(
            capabilities[code]["binding_profile"] == "terminal_broad_live_future_planned",
            f"{code} final broad profile drifted",
        )
    _require(
        capabilities["G12"]["binding_profile"] == "standalone_distribution_offline_future_planned",
        "G12 final distribution profile drifted",
    )
    return contract


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--contract", type=Path, default=DEFAULT_CONTRACT)
    arguments = parser.parse_args()
    try:
        contract = validate(arguments.contract)
    except ContractError as error:
        print(f"C distribution contract rejected: {error}", file=sys.stderr)
        return 1
    print(
        "validated frozen C distribution contract: "
        f"abi={contract['abi']['version']} "
        f"targets={len(contract['matrix']['candidate_supported'])} "
        "publication=false"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
