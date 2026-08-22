#!/usr/bin/env python3
"""Validate and compare four real Workforce V4 reports."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, NoReturn

ROOT = Path(__file__).resolve().parents[2]
MANIFEST_RELATIVE = "tests/contracts/sdk_conformance/manifest-v1.json"
CATALOG_RELATIVE = "tests/contracts/sdk_conformance/workforce-v4/catalog-v4.json"
JOURNEY_RELATIVE = "tests/contracts/sdk_conformance/workforce-v4/journey-v4.json"
REPORT_SCHEMA_RELATIVE = "tests/contracts/sdk_conformance/workforce-v4/report-schema-v4.json"
OBSERVATION_SCHEMA_RELATIVE = (
    "tests/contracts/sdk_conformance/workforce-v4/observation-schema-v4.json"
)
EXPECTED_BINDINGS = ("python", "node", "rust", "c")
EXPECTED_TRANSITIONS = (
    "workforce.runtime.database-administration",
    "workforce.migration.rollback",
    "workforce.migration.backfill",
    "workforce.migration.runtime-facade",
)
EXPECTED_RETAINED_GAPS = (
    "workforce.runtime.cancellation",
    "workforce.runtime.timeout-resource-limits",
    "workforce.diagnostic.all-workflows",
    "workforce.runtime.explicit-close",
)
EXPECTED_MIGRATION_ORDER = (
    "workforcev4/0001_initial",
    "workforcev4/0002_expand-display-name",
    "workforcev4/0003_backfill-display-name",
    "workforcev4/0004_contract-legacy-name",
)
EXPECTED_CATALOG_FINGERPRINT = "b59eb4988620a941a7531432eb622d04fc0aafe0238dabf047056138c78ea99c"


class ContractError(ValueError):
    """A stable fail-closed Workforce V4 rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


def _reject(code: str, message: str) -> NoReturn:
    raise ContractError(code, message)


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            _reject("duplicate_json_key", f"duplicate JSON key {key!r}")
        result[key] = value
    return result


def _load_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_bytes(), object_pairs_hook=_unique_object)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        _reject("invalid_json_source", f"cannot load {path}: {error}")
    if not isinstance(value, dict):
        _reject("invalid_json_shape", f"{path} must contain one JSON object")
    return value


def _sha256(path: Path) -> str:
    try:
        return hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError as error:
        _reject("unreadable_source", f"cannot read {path}: {error}")


def _exact_keys(value: dict[str, Any], expected: set[str], label: str) -> None:
    if set(value) != expected:
        _reject("invalid_contract_shape", f"{label} keys are not exact")


@dataclass(frozen=True)
class Contracts:
    manifest: dict[str, Any]
    catalog: dict[str, Any]
    journey: dict[str, Any]
    report_schema: dict[str, Any]
    observation_schema: dict[str, Any]
    selected_cases: tuple[str, ...]
    observation_refs: tuple[str, ...]


def load_contracts(root: Path = ROOT) -> Contracts:
    manifest = _load_json(root / MANIFEST_RELATIVE)
    catalog = _load_json(root / CATALOG_RELATIVE)
    journey = _load_json(root / JOURNEY_RELATIVE)
    report_schema = _load_json(root / REPORT_SCHEMA_RELATIVE)
    observation_schema = _load_json(root / OBSERVATION_SCHEMA_RELATIVE)
    if catalog.get("observation_schema_path") != OBSERVATION_SCHEMA_RELATIVE:
        _reject("observation_schema_drift", "V4 observation-schema path is not exact")
    if report_schema.get("$defs", {}).get("result", {}).get("properties", {}).get(
        "observation"
    ) != {"$ref": "observation-schema-v4.json"}:
        _reject("observation_schema_drift", "V4 report schema does not bind the closed algebra")
    if len(observation_schema.get("oneOf", ())) != 8:
        _reject("observation_schema_drift", "V4 observation algebra must contain eight lanes")

    transitions = tuple(catalog.get("manifest_transition_cases", ()))
    retained = tuple(catalog.get("evidence_only_gap_cases", ()))
    if transitions != EXPECTED_TRANSITIONS:
        _reject("transition_scope_drift", "V4 transition cases are not exact")
    if retained != EXPECTED_RETAINED_GAPS:
        _reject("retained_gap_scope_drift", "V4 retained-gap cases are not exact")
    if set(transitions) & set(retained):
        _reject("case_partition_overlap", "transition and retained-gap cases overlap")
    if tuple(catalog.get("report_bindings", ())) != EXPECTED_BINDINGS:
        _reject("binding_scope_drift", "V4 report binding order is not exact")

    cases = catalog.get("cases")
    proofs = catalog.get("selected_proofs")
    if not isinstance(cases, list) or not isinstance(proofs, list) or len(cases) != 8:
        _reject("row_scope_drift", "V4 must select exactly eight case rows")
    selected_cases = tuple(case.get("id") for case in cases if isinstance(case, dict))
    if selected_cases != transitions + retained or len(proofs) != 8:
        _reject("row_order_drift", "V4 case/proof order is not exact")
    proof_cases = tuple(proof.get("case_id") for proof in proofs if isinstance(proof, dict))
    observation_refs = tuple(
        proof.get("observation_ref") for proof in proofs if isinstance(proof, dict)
    )
    if (
        proof_cases != selected_cases
        or tuple(journey.get("expected_observation_refs", ())) != observation_refs
    ):
        _reject("proof_inventory_drift", "V4 proof and journey inventories disagree")

    capabilities = {
        capability.get("id"): capability
        for capability in manifest.get("capabilities", ())
        if isinstance(capability, dict)
    }
    for case in cases:
        capability = capabilities.get(case.get("capability_id"))
        if capability is None or capability.get("case_ids") != [case.get("id")]:
            _reject("manifest_case_drift", f"manifest does not own {case.get('id')!r}")

    return Contracts(
        manifest=manifest,
        catalog=catalog,
        journey=journey,
        report_schema=report_schema,
        observation_schema=observation_schema,
        selected_cases=selected_cases,
        observation_refs=observation_refs,
    )


def _validate_source_identity(identity: Any, relative: str, root: Path, label: str) -> None:
    if not isinstance(identity, dict):
        _reject("invalid_source_identity", f"{label} source identity is absent")
    _exact_keys(identity, {"path", "sha256"}, f"{label} source identity")
    if identity["path"] != relative or identity["sha256"] != _sha256(root / relative):
        _reject("stale_source_identity", f"{label} source identity is stale")


def _validate_report(report: dict[str, Any], observation_refs: tuple[str, ...]) -> None:
    _exact_keys(
        report,
        {
            "format",
            "binding",
            "manifest",
            "catalog",
            "journey",
            "server_version",
            "results",
            "cleanup",
        },
        "report",
    )
    if report["format"] != "typebridge.sdk-conformance-report/v4":
        _reject("invalid_report_schema", "report format is not V4")
    if report["binding"] not in EXPECTED_BINDINGS or report["server_version"] != "3.12.3":
        _reject("invalid_report_schema", "report binding or server version is invalid")
    if not isinstance(report["results"], list) or len(report["results"]) != 8:
        _reject("invalid_report_schema", "report must contain exactly eight results")
    if report["cleanup"] != {
        "managed_database_absent": True,
        "journal_database_absent": True,
        "temporary_evidence_absent": True,
    }:
        _reject("invalid_report_schema", "report cleanup invariant is false")
    for index, result in enumerate(report["results"]):
        if not isinstance(result, dict):
            _reject("invalid_report_schema", "report result must be an object")
        _exact_keys(
            result,
            {"case_id", "capability_id", "proof_kind", "outcome", "observation"},
            "report result",
        )
        if result["outcome"] != "passed" or not isinstance(result["observation"], dict):
            _reject("invalid_report_schema", "report result outcome is invalid")
        _validate_observation(observation_refs[index], result["observation"])


def _bool(value: Any, label: str) -> None:
    if type(value) is not bool:
        _reject("invalid_observation_shape", f"{label} must be boolean")


def _uint(value: Any, label: str) -> None:
    if type(value) is not int or value < 0:
        _reject("invalid_observation_shape", f"{label} must be an unsigned integer")


def _enum(value: Any, allowed: set[str], label: str) -> None:
    if not isinstance(value, str) or value not in allowed:
        _reject("invalid_observation_shape", f"{label} is outside its closed vocabulary")


def _validate_observation(reference: str, observation: dict[str, Any]) -> None:
    """Validate one independently observed lane against the closed V4 algebra."""
    if reference == "bound_database_administration":
        _exact_keys(
            observation,
            {"create", "repeat_create", "pair_state", "delete", "repeat_delete"},
            reference,
        )
        _enum(observation["create"], {"created"}, f"{reference}.create")
        _enum(
            observation["repeat_create"],
            {"already_exists"},
            f"{reference}.repeat_create",
        )
        _enum(
            observation["pair_state"],
            {"standalone_managed", "owned_pair"},
            f"{reference}.pair_state",
        )
        _enum(
            observation["delete"],
            {"deleted_standalone_managed", "deleted_owned_pair"},
            f"{reference}.delete",
        )
        _enum(
            observation["repeat_delete"],
            {"already_absent"},
            f"{reference}.repeat_delete",
        )
    elif reference == "rollback_reapply_recovery":
        _exact_keys(
            observation,
            {
                "apply_status",
                "rollback_without_approval_code",
                "rollback_status",
                "unknown_target_code",
                "repeat_rollback_status",
                "reapply_status",
            },
            reference,
        )
        for field in ["apply_status", "reapply_status"]:
            _enum(observation[field], {"applied"}, f"{reference}.{field}")
        _enum(observation["rollback_status"], {"rolled_back"}, f"{reference}.rollback_status")
        _enum(
            observation["repeat_rollback_status"],
            {"up_to_date"},
            f"{reference}.repeat_rollback_status",
        )
        _enum(
            observation["rollback_without_approval_code"],
            {"migration_rollback_approval_required"},
            f"{reference}.rollback_without_approval_code",
        )
        _enum(
            observation["unknown_target_code"],
            {"migration_history_unknown_rollback_target"},
            f"{reference}.unknown_target_code",
        )
    elif reference == "binding_neutral_backfill":
        _exact_keys(
            observation,
            {
                "conflict_certainty",
                "conflict_code",
                "conflict_visible_destination_count",
                "forward_changed",
                "forward_transaction_groups",
                "equal_copy_count",
                "retry_changed",
                "reverse_changed",
                "remaining_destination_count",
            },
            reference,
        )
        _enum(
            observation["conflict_certainty"],
            {"definitely_aborted"},
            f"{reference}.conflict_certainty",
        )
        _enum(
            observation["conflict_code"],
            {"migration_typedb_backfill_destination_conflict"},
            f"{reference}.conflict_code",
        )
        for field in set(observation) - {"conflict_certainty", "conflict_code"}:
            _uint(observation[field], f"{reference}.{field}")
    elif reference == "migration_runtime_facade":
        _exact_keys(
            observation,
            {
                "catalog_entries",
                "catalog_fingerprint",
                "apply_order",
                "rollback_order",
                "backfill_steps",
            },
            reference,
        )
        _uint(observation["catalog_entries"], f"{reference}.catalog_entries")
        _uint(observation["backfill_steps"], f"{reference}.backfill_steps")
        if (
            not isinstance(observation["catalog_fingerprint"], str)
            or len(observation["catalog_fingerprint"]) != 64
            or any(
                character not in "0123456789abcdef"
                for character in observation["catalog_fingerprint"]
            )
        ):
            _reject(
                "invalid_observation_shape",
                f"{reference}.catalog_fingerprint must be lowercase SHA-256",
            )
        for field in ["apply_order", "rollback_order"]:
            values = observation[field]
            if (
                not isinstance(values, list)
                or not values
                or not all(
                    isinstance(value, str) and value.startswith("workforcev4/") for value in values
                )
            ):
                _reject(
                    "invalid_observation_shape",
                    f"{reference}.{field} must be canonical migration identities",
                )
        if (
            observation["catalog_entries"] != 4
            or observation["backfill_steps"] != 1
            or observation["catalog_fingerprint"] != EXPECTED_CATALOG_FINGERPRINT
            or tuple(observation["apply_order"]) != EXPECTED_MIGRATION_ORDER
            or tuple(observation["rollback_order"]) != tuple(reversed(EXPECTED_MIGRATION_ORDER))
        ):
            _reject(
                "migration_catalog_observation_drift",
                "generated runtime facade did not observe the exact V4 catalog",
            )
    elif reference == "administration_migration_cancellation":
        _exact_keys(observation, {"code", "before_effect"}, reference)
        _enum(observation["code"], {"migration_execution_cancelled"}, f"{reference}.code")
        _bool(observation["before_effect"], f"{reference}.before_effect")
    elif reference == "administration_migration_resource_limits":
        _exact_keys(observation, {"code", "bounded"}, reference)
        _enum(observation["code"], {"migration_execution_group_limit"}, f"{reference}.code")
        _bool(observation["bounded"], f"{reference}.bounded")
    elif reference == "administration_migration_structured_diagnostic":
        _exact_keys(observation, {"code", "category", "provider_text_absent"}, reference)
        _enum(observation["code"], {"migration_execution_cancelled"}, f"{reference}.code")
        _enum(observation["category"], {"cancelled"}, f"{reference}.category")
        _bool(observation["provider_text_absent"], f"{reference}.provider_text_absent")
    elif reference == "administration_migration_resource_lifecycle":
        _exact_keys(
            observation, {"explicit_close", "repeat_close", "temporary_evidence_absent"}, reference
        )
        _bool(observation["explicit_close"], f"{reference}.explicit_close")
        _bool(observation["repeat_close"], f"{reference}.repeat_close")
        _bool(observation["temporary_evidence_absent"], f"{reference}.temporary_evidence_absent")
    else:
        _reject("unknown_observation_reference", f"unknown V4 observation {reference!r}")


def compare_reports(paths: list[Path], root: Path = ROOT) -> dict[str, Any]:
    contracts = load_contracts(root)
    if contracts.catalog.get("authority_state") != "finalized":
        _reject(
            "unfinalized_v4_authority",
            "Workforce V4 reports are disabled until real evidence authority is finalized",
        )
    if len(paths) != len(EXPECTED_BINDINGS):
        _reject("missing_binding_report", "exactly four V4 reports are required")

    reports: dict[str, dict[str, Any]] = {}
    expected_observations: tuple[Any, ...] | None = None
    for path in paths:
        report = _load_json(path)
        _validate_report(report, contracts.observation_refs)
        binding = report["binding"]
        if binding in reports:
            _reject("duplicate_binding_report", f"duplicate {binding!r} report")
        _validate_source_identity(report["manifest"], MANIFEST_RELATIVE, root, "manifest")
        _validate_source_identity(report["catalog"], CATALOG_RELATIVE, root, "catalog")
        _validate_source_identity(report["journey"], JOURNEY_RELATIVE, root, "journey")

        results = report["results"]
        case_ids = tuple(result["case_id"] for result in results)
        observations = tuple(result["observation"] for result in results)
        if case_ids != contracts.selected_cases:
            _reject("report_row_order_drift", f"{binding} report row order is not exact")
        for reference in ("rollback_reapply_recovery", "binding_neutral_backfill"):
            index = contracts.observation_refs.index(reference)
            expected = contracts.journey["shared_fixture_oracles"][reference]
            if observations[index] != expected:
                _reject(
                    "exact_live_oracle_drift",
                    f"{binding} independently observed {reference} differs from the frozen exact-live oracle",
                )
        if expected_observations is None:
            expected_observations = observations
        elif observations != expected_observations:
            _reject("cross_binding_observation_drift", "V4 observations differ by binding")
        reports[binding] = report

    if tuple(reports) != EXPECTED_BINDINGS:
        _reject("binding_order_drift", "V4 reports are not supplied in canonical order")
    return {
        "format": "typebridge.workforce-v4-comparison/v1",
        "bindings": list(EXPECTED_BINDINGS),
        "selected_cases": list(contracts.selected_cases),
        "manifest_transition_cases": list(EXPECTED_TRANSITIONS),
        "retained_gap_cases": list(EXPECTED_RETAINED_GAPS),
        "pending_promotions": [],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("reports", nargs="*", type=Path)
    args = parser.parse_args()
    try:
        comparison = compare_reports(args.reports)
    except ContractError as error:
        print(f"workforce-v4 conformance rejected [{error.code}]: {error}", file=sys.stderr)
        return 1
    print(json.dumps(comparison, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
