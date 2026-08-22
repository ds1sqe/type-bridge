#!/usr/bin/env python3
"""Validate and compare four real Workforce V5 serialization reports."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, NoReturn, cast

ROOT = Path(__file__).resolve().parents[2]
MANIFEST = "tests/contracts/sdk_conformance/manifest-v1.json"
CATALOG = "tests/contracts/sdk_conformance/workforce-v5/catalog-v5.json"
JOURNEY = "tests/contracts/sdk_conformance/workforce-v5/journey-v5.json"
RECORD_CONTRACT = "tests/contracts/projected-record-v1.json"
REPORT_SCHEMA = "tests/contracts/sdk_conformance/workforce-v5/report-schema-v5.json"
OBSERVATION_SCHEMA = "tests/contracts/sdk_conformance/workforce-v5/observation-schema-v5.json"
BINDINGS = ("python", "node", "rust", "c")
TRANSITIONS = ("workforce.model.serialization",)
RETAINED_GAPS = (
    "workforce.runtime.cancellation",
    "workforce.runtime.timeout-resource-limits",
    "workforce.diagnostic.all-workflows",
    "workforce.runtime.explicit-close",
)
OBSERVATIONS = (
    "canonical_record_archive",
    "serialization_cancellation",
    "serialization_resource_limits",
    "serialization_structured_diagnostic",
    "serialization_resource_lifecycle",
)
JOURNEY_RECORDS = [
    {
        "id": "integer-key",
        "kind": "attribute_value",
        "type": {"kind": "attribute", "label": "robot_id"},
        "semantic_value": {"kind": "long", "value": "9007199254740993"},
    },
    {
        "id": "player-stats",
        "kind": "struct_value",
        "type": {"kind": "struct", "label": "player-stats"},
        "member_order": ["nickname", "wins"],
    },
    {
        "id": "person-create",
        "kind": "entity_create",
        "type": {"kind": "entity", "label": "person"},
        "requires_absent_vs_empty": True,
    },
    {
        "id": "person-snapshot",
        "kind": "entity_snapshot",
        "type": {"kind": "entity", "label": "person"},
        "requires_canonical_iid": True,
        "decoded_state": "detached",
    },
    {
        "id": "membership-create",
        "kind": "relation_create",
        "type": {"kind": "relation", "label": "membership"},
        "requires_polymorphic_optional_role": True,
        "requires_relation_as_player": True,
    },
    {
        "id": "membership-snapshot",
        "kind": "relation_snapshot",
        "type": {"kind": "relation", "label": "membership"},
        "requires_inherited_roles": True,
        "decoded_state": "detached",
    },
    {
        "id": "player-reference",
        "kind": "reference",
        "requires_iid_or_exact_key": True,
        "decoded_state": "detached",
    },
]
JOURNEY_HOSTILE_MUTATIONS = [
    "foreign-declared-schema",
    "wrong-semantic-profile",
    "wrong-record-kind",
    "wrong-concrete-type",
    "duplicate-object-key",
    "reordered-object-key",
    "unknown-member",
    "noncanonical-long",
    "noncanonical-double-bits",
    "oversize-archive",
    "invalid-final-record",
]


class ContractError(ValueError):
    """One stable fail-closed V5 report rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


def reject(code: str, message: str) -> NoReturn:
    raise ContractError(code, message)


def unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            reject("duplicate_json_key", f"duplicate JSON key {key!r}")
        result[key] = value
    return result


def load_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_bytes(), object_pairs_hook=unique_object)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        reject("invalid_json_source", f"cannot load {path}: {error}")
    if not isinstance(value, dict):
        reject("invalid_json_shape", f"{path} must contain one JSON object")
    return value


def canonical_json_bytes(value: Any) -> bytes:
    return json.dumps(
        value,
        ensure_ascii=False,
        allow_nan=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")


def load_report(path: Path) -> dict[str, Any]:
    try:
        raw = path.read_bytes()
    except OSError as error:
        reject("invalid_json_source", f"cannot load {path}: {error}")
    report = load_json(path)
    try:
        canonical = canonical_json_bytes(report)
    except (TypeError, ValueError) as error:
        reject("invalid_json_shape", f"cannot canonicalize {path}: {error}")
    if raw != canonical:
        reject("noncanonical_report_json", f"{path} is not exact canonical JSON")
    return report


def sha256(path: Path) -> str:
    try:
        return hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError as error:
        reject("unreadable_source", f"cannot read {path}: {error}")


def exact_keys(value: Any, expected: set[str], label: str) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != expected:
        reject("invalid_contract_shape", f"{label} keys are not exact")
    return value


def exact_bool(value: Any, label: str) -> None:
    if type(value) is not bool:
        reject("invalid_observation_shape", f"{label} must be Boolean")


def exact_sha256(value: Any, label: str) -> None:
    if (
        not isinstance(value, str)
        or len(value) != 64
        or any(character not in "0123456789abcdef" for character in value)
    ):
        reject("invalid_observation_shape", f"{label} must be lowercase SHA-256")


@dataclass(frozen=True)
class Contracts:
    manifest: dict[str, Any]
    catalog: dict[str, Any]
    selected_cases: tuple[str, ...]
    selected_rows: tuple[tuple[str, str, str], ...]


def load_contracts(root: Path = ROOT) -> Contracts:
    manifest = load_json(root / MANIFEST)
    catalog = load_json(root / CATALOG)
    journey = load_json(root / JOURNEY)
    report_schema = load_json(root / REPORT_SCHEMA)
    observation_schema = load_json(root / OBSERVATION_SCHEMA)
    if catalog.get("report_bindings") != list(BINDINGS):
        reject("binding_scope_drift", "V5 binding order is not exact")
    if tuple(catalog.get("manifest_transition_cases", ())) != TRANSITIONS:
        reject("transition_scope_drift", "V5 transition scope is not exact")
    if tuple(catalog.get("evidence_only_gap_cases", ())) != RETAINED_GAPS:
        reject("retained_gap_scope_drift", "V5 retained-gap scope is not exact")
    cases = catalog.get("cases")
    proofs = catalog.get("selected_proofs")
    if not isinstance(cases, list) or not isinstance(proofs, list) or len(cases) != 5:
        reject("row_scope_drift", "V5 must contain five exact case/proof rows")
    selected_values = tuple(case.get("id") for case in cases if isinstance(case, dict))
    if len(selected_values) != len(cases) or not all(
        isinstance(case_id, str) for case_id in selected_values
    ):
        reject("row_scope_drift", "V5 case identifiers must be exact strings")
    selected = cast(tuple[str, ...], selected_values)
    proof_cases = tuple(proof.get("case_id") for proof in proofs if isinstance(proof, dict))
    references = tuple(proof.get("observation_ref") for proof in proofs if isinstance(proof, dict))
    if selected != TRANSITIONS + RETAINED_GAPS or proof_cases != selected:
        reject("row_order_drift", "V5 case/proof order is not exact")
    if references != OBSERVATIONS:
        reject("proof_inventory_drift", "V5 observation references are not exact")
    if journey.get("required_equalities") != [
        "python-node-rust-c-record-bytes",
        "python-node-rust-c-archive-bytes",
        "direct-remote-entity-snapshot-bytes",
        "direct-remote-relation-snapshot-bytes",
    ]:
        reject("journey_equality_drift", "V5 required equalities are not exact")
    if journey.get("records") != JOURNEY_RECORDS:
        reject("journey_record_drift", "V5 record corpus is not exact")
    if journey.get("archive") != {
        "mode": "ordered",
        "record_ids": [record["id"] for record in JOURNEY_RECORDS],
        "whole_archive_rejection": True,
    }:
        reject("journey_archive_drift", "V5 archive corpus is not exact")
    if journey.get("hostile_mutations") != JOURNEY_HOSTILE_MUTATIONS:
        reject("journey_hostility_drift", "V5 hostile corpus is not exact")
    if report_schema.get("$defs", {}).get("result", {}).get("properties", {}).get(
        "observation"
    ) != {"$ref": "observation-schema-v5.json"}:
        reject("observation_schema_drift", "V5 report schema does not bind its algebra")
    if len(observation_schema.get("oneOf", ())) != 5:
        reject("observation_schema_drift", "V5 observation algebra must contain five lanes")
    capabilities = {
        item.get("id"): item for item in manifest.get("capabilities", ()) if isinstance(item, dict)
    }
    for case in cases:
        capability = capabilities.get(case.get("capability_id"))
        if capability is None or capability.get("case_ids") != [case.get("id")]:
            reject("manifest_case_drift", f"manifest does not own {case.get('id')!r}")
    rows = tuple(
        (case["id"], case["capability_id"], proof["proof_kind"])
        for case, proof in zip(cases, proofs, strict=True)
    )
    return Contracts(manifest, catalog, selected, rows)


def validate_identity(value: Any, relative: str, root: Path, label: str) -> None:
    identity = exact_keys(value, {"path", "sha256"}, f"{label} identity")
    if identity["path"] != relative or identity["sha256"] != sha256(root / relative):
        reject("stale_source_identity", f"{label} identity is stale")


def validate_observation(index: int, value: Any) -> None:
    if index == 0:
        observation = exact_keys(
            value,
            {
                "record_sha256",
                "archive_sha256",
                "round_trip",
                "foreign_schema_code",
                "direct_remote_equal",
                "detached_mutation_code",
            },
            "canonical record/archive observation",
        )
        records = observation["record_sha256"]
        if not isinstance(records, list) or len(records) != 7:
            reject("invalid_observation_shape", "V5 requires seven record digests")
        for ordinal, digest in enumerate(records):
            exact_sha256(digest, f"record digest {ordinal}")
        exact_sha256(observation["archive_sha256"], "archive digest")
        if observation != {
            **observation,
            "round_trip": True,
            "foreign_schema_code": "projected_record_schema_mismatch",
            "direct_remote_equal": True,
            "detached_mutation_code": "projected_snapshot_detached",
        }:
            reject("invalid_observation_value", "canonical record/archive facts differ")
    elif index == 1:
        observation = exact_keys(value, {"code", "partial_output"}, "cancellation")
        if observation != {"code": "projected_codec_cancelled", "partial_output": False}:
            reject("invalid_observation_value", "cancellation facts differ")
    elif index == 2:
        observation = exact_keys(
            value,
            {"input_code", "output_code", "member_code", "depth_code", "partial_output"},
            "resource limits",
        )
        if observation != {
            "input_code": "projected_codec_input_limit",
            "output_code": "projected_codec_output_limit",
            "member_code": "projected_codec_member_limit",
            "depth_code": "projected_codec_depth_limit",
            "partial_output": False,
        }:
            reject("invalid_observation_value", "resource-limit facts differ")
    elif index == 3:
        observation = exact_keys(
            value, {"code", "category", "path", "payload_absent"}, "diagnostic"
        )
        if observation != {
            "code": "projected_record_schema_mismatch",
            "category": "invalid_input",
            "path": ["declared_schema_identity"],
            "payload_absent": True,
        }:
            reject("invalid_observation_value", "diagnostic facts differ")
    else:
        observation = exact_keys(
            value,
            {
                "bytes_closed",
                "builder_closed",
                "archive_closed",
                "decoded_closed",
                "repeat_close",
                "sibling_usable",
            },
            "lifecycle",
        )
        for field, item in observation.items():
            exact_bool(item, f"lifecycle.{field}")
            if not item:
                reject("invalid_observation_value", f"lifecycle.{field} is false")


def validate_report(report: dict[str, Any], contracts: Contracts, root: Path) -> None:
    exact_keys(
        report,
        {
            "format",
            "binding",
            "manifest",
            "catalog",
            "journey",
            "record_contract",
            "server_version",
            "results",
            "cleanup",
        },
        "report",
    )
    if report["format"] != "typebridge.sdk-conformance-report/v5":
        reject("invalid_report_schema", "report format is not V5")
    if report["binding"] not in BINDINGS or report["server_version"] != "3.12.3":
        reject("invalid_report_schema", "report binding or server version is invalid")
    validate_identity(report["manifest"], MANIFEST, root, "manifest")
    validate_identity(report["catalog"], CATALOG, root, "catalog")
    validate_identity(report["journey"], JOURNEY, root, "journey")
    validate_identity(report["record_contract"], RECORD_CONTRACT, root, "record contract")
    results = report["results"]
    if not isinstance(results, list) or len(results) != 5:
        reject("invalid_report_schema", "report must contain five exact results")
    if tuple(result.get("case_id") for result in results if isinstance(result, dict)) != (
        contracts.selected_cases
    ):
        reject("report_row_order_drift", "report row order is not exact")
    for index, result in enumerate(results):
        exact_keys(
            result,
            {"case_id", "capability_id", "proof_kind", "outcome", "observation"},
            "report result",
        )
        if result["outcome"] != "passed":
            reject("invalid_report_schema", "report result did not pass")
        if (
            result["case_id"],
            result["capability_id"],
            result["proof_kind"],
        ) != contracts.selected_rows[index]:
            reject("report_row_binding_drift", "report row is not bound to its catalog proof")
        validate_observation(index, result["observation"])
    if report["cleanup"] != {
        "managed_database_absent": True,
        "temporary_evidence_absent": True,
        "partial_output_absent": True,
    }:
        reject("invalid_report_schema", "report cleanup invariant is false")


def compare_reports(paths: list[Path], root: Path = ROOT) -> dict[str, Any]:
    contracts = load_contracts(root)
    state = contracts.catalog.get("authority_state")
    if state not in {"phase0_unfinalized", "finalized"}:
        reject("invalid_v5_authority", "V5 authority state is invalid")
    if len(paths) != 4:
        reject("missing_binding_report", "exactly four V5 reports are required")
    reports: dict[str, dict[str, Any]] = {}
    shared: tuple[Any, ...] | None = None
    for path in paths:
        report = load_report(path)
        validate_report(report, contracts, root)
        binding = report["binding"]
        if binding in reports:
            reject("duplicate_binding_report", f"duplicate {binding!r} report")
        observations = tuple(result["observation"] for result in report["results"])
        if shared is None:
            shared = observations
        elif observations != shared:
            reject("cross_binding_observation_drift", "V5 observations differ by binding")
        reports[binding] = report
    if tuple(reports) != BINDINGS:
        reject("binding_order_drift", "V5 reports are not in canonical binding order")

    capabilities = {
        item.get("case_ids", ())[0]: item
        for item in contracts.manifest.get("capabilities", ())
        if isinstance(item, dict) and len(item.get("case_ids", ())) == 1
    }
    profiles = contracts.manifest.get("binding_profiles", {})
    pending = []
    for case_id in TRANSITIONS:
        capability = capabilities[case_id]
        accepted = tuple(profiles[capability["binding_profile"]].get("accepted_live", ()))
        if accepted != BINDINGS:
            pending.append(
                {
                    "case_id": case_id,
                    "capability_id": capability["id"],
                    "binding_profile": capability["binding_profile"],
                }
            )
    for case_id in RETAINED_GAPS:
        capability = capabilities[case_id]
        accepted = profiles[capability["binding_profile"]].get("accepted_live", ())
        if set(accepted) & set(BINDINGS):
            reject("retained_gap_promoted", f"retained V5 gap {case_id!r} became accepted")
    if state == "finalized" and pending:
        reject("finalized_authority_has_pending_promotions", "finalized V5 has pending promotion")
    return {
        "format": "typebridge.workforce-v5-comparison/v1",
        "authority_state": state,
        "bindings": list(BINDINGS),
        "selected_cases": list(contracts.selected_cases),
        "manifest_transition_cases": list(TRANSITIONS),
        "retained_gap_cases": list(RETAINED_GAPS),
        "pending_promotions": pending,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("reports", nargs="*", type=Path)
    args = parser.parse_args()
    try:
        comparison = compare_reports(args.reports)
    except ContractError as error:
        print(f"workforce-v5 conformance rejected [{error.code}]: {error}", file=sys.stderr)
        return 1
    print(json.dumps(comparison, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
