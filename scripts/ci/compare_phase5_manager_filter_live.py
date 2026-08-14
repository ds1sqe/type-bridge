#!/usr/bin/env python3
"""Compare four exact-TypeDB-3.12.1 Phase-5 manager-filter reports."""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import re
import stat
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
SCHEMA_RELATIVE = "tests/contracts/sdk_conformance/workforce-v3/schema-v3.yaml"
JOURNEY_RELATIVE = "tests/contracts/sdk_conformance/workforce-v3/journey-v3.json"
PROVIDER_RELATIVE = "tests/contracts/sdk_conformance/workforce-v3/provider-3.12.1-v3.tql"

REPORT_FORMAT = "typebridge.phase5-manager-filter-live-report/v1"
SUMMARY_FORMAT = "typebridge.phase5-manager-filter-live-summary/v1"
SEMANTIC_PROFILE = "typedb-3.12.1/v1"
REPORT_BINDINGS = ("python", "node", "rust", "c")
REPORT_OUTPUT_ENV = "TYPE_BRIDGE_PHASE5_MANAGER_LIVE_REPORT"
REPORT_WRITE_SEMANTICS = "create_new"

SOURCE_SHA256 = {
    "schema": "74b9161e3d8fd70a1f22c3a8b7fc70a823b18cb07973064e2e10ce0940f76f1f",
    "journey": "912130753fe7938a38c054cff16e202b312551a6aa65265148628bfbd2abbbef",
    "provider": "af61aaece22d666e19d2c1357f8ebc39b8cbcf4bff7b98a19a56daf171a1faba",
}
SOURCE_PATHS = {
    "schema": SCHEMA_RELATIVE,
    "journey": JOURNEY_RELATIVE,
    "provider": PROVIDER_RELATIVE,
}

MAX_REPORT_BYTES = 256 * 1024
MAX_AUTHORITY_BYTES = 1024 * 1024
MAX_PRODUCER_SOURCE_BYTES = 1024 * 1024
FORBIDDEN_REPORT_KEYS = frozenset(
    {
        "address",
        "database",
        "database_name",
        "endpoint",
        "host",
        "iid",
        "port",
    }
)
FORBIDDEN_PRODUCER_IMPORT_MARKERS = (
    "compare_phase5_manager_filter_live",
    "expected_observation",
    "expected_report",
    "load_contract",
)
TYPEDB_IID_RE = re.compile(r"^0x[0-9a-f]{2,}$")
ENDPOINT_RE = re.compile(r"^(?:https?|typedb)://", re.IGNORECASE)


class ContractError(ValueError):
    """A stable fail-closed manager-filter live-contract rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code


@dataclass(frozen=True)
class Phase5ManagerLiveContract:
    authority: dict[str, dict[str, str]]
    observation: dict[str, Any]


def _duplicate_key_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise ContractError("duplicate_json_key", f"duplicate JSON key {key!r}")
        value[key] = item
    return value


def canonical_json_bytes(value: Any) -> bytes:
    """Return compact JSON with ascending object keys and one newline."""

    try:
        encoded = json.dumps(
            value,
            ensure_ascii=False,
            allow_nan=False,
            separators=(",", ":"),
            sort_keys=True,
        )
    except (TypeError, ValueError, RecursionError) as error:
        raise ContractError("invalid_json_value", "value cannot be canonicalized") from error
    return f"{encoded}\n".encode()


def _regular_bytes(path: Path, label: str, limit: int) -> bytes:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise ContractError("invalid_source_file", f"{label} cannot be inspected") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        raise ContractError(
            "invalid_source_file",
            f"{label} must be a regular non-symlink file",
        )
    if metadata.st_size > limit:
        raise ContractError("source_size_limit", f"{label} exceeds {limit} bytes")
    try:
        with path.open("rb") as source:
            raw = source.read(limit + 1)
    except OSError as error:
        raise ContractError("invalid_source_file", f"{label} cannot be read") from error
    if len(raw) > limit:
        raise ContractError("source_size_limit", f"{label} exceeds {limit} bytes")
    return raw


def _json_value(raw: bytes, label: str) -> Any:
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ContractError("invalid_json_utf8", f"{label} is not UTF-8") from error
    try:
        return json.loads(text, object_pairs_hook=_duplicate_key_object)
    except ContractError:
        raise
    except (json.JSONDecodeError, RecursionError) as error:
        raise ContractError("malformed_json", f"{label} is not valid bounded JSON") from error


def _exact_object(value: Any, keys: set[str], label: str) -> dict[str, Any]:
    if type(value) is not dict:
        raise ContractError("invalid_object", f"{label} must be an object")
    actual = set(value)
    if actual != keys:
        raise ContractError(
            "invalid_object_fields",
            f"{label} fields differ; missing={sorted(keys - actual)}, "
            f"extra={sorted(actual - keys)}",
        )
    return value


def _reject_forbidden_claims(value: Any, path: str = "report") -> None:
    if type(value) is dict:
        for key, item in value.items():
            normalized = key.casefold().replace("-", "_")
            if normalized in FORBIDDEN_REPORT_KEYS:
                raise ContractError(
                    "forbidden_report_claim",
                    f"{path}.{key} is outside the manager-filter live contract",
                )
            _reject_forbidden_claims(item, f"{path}.{key}")
    elif type(value) is list:
        for index, item in enumerate(value):
            _reject_forbidden_claims(item, f"{path}[{index}]")
    elif type(value) is str and (TYPEDB_IID_RE.fullmatch(value) or ENDPOINT_RE.match(value)):
        raise ContractError(
            "forbidden_report_claim",
            f"{path} carries a provider-local identifier or endpoint",
        )


def validate_producer_source(source: str | bytes) -> None:
    """Reject a producer that imports or names comparator-side expectations."""

    if type(source) is bytes:
        if len(source) > MAX_PRODUCER_SOURCE_BYTES:
            raise ContractError(
                "producer_source_size_limit",
                f"producer source exceeds {MAX_PRODUCER_SOURCE_BYTES} bytes",
            )
        try:
            text = source.decode("utf-8")
        except UnicodeDecodeError as error:
            raise ContractError(
                "invalid_producer_source_utf8",
                "producer source is not UTF-8",
            ) from error
    elif type(source) is str:
        if len(source.encode("utf-8")) > MAX_PRODUCER_SOURCE_BYTES:
            raise ContractError(
                "producer_source_size_limit",
                f"producer source exceeds {MAX_PRODUCER_SOURCE_BYTES} bytes",
            )
        text = source
    else:
        raise ContractError("invalid_producer_source", "producer source must be text")
    for marker in FORBIDDEN_PRODUCER_IMPORT_MARKERS:
        if marker in text:
            raise ContractError(
                "expected_observation_import",
                f"producer source contains forbidden marker {marker!r}",
            )


def _source_identity(label: str) -> tuple[dict[str, str], bytes]:
    relative = SOURCE_PATHS[label]
    raw = _regular_bytes(ROOT / relative, f"Phase-5 {label}", MAX_AUTHORITY_BYTES)
    digest = hashlib.sha256(raw).hexdigest()
    if digest != SOURCE_SHA256[label]:
        raise ContractError(
            "authority_hash_mismatch",
            f"Phase-5 {label} is not the frozen exact-3.12.1 authority",
        )
    return {"path": relative, "sha256": digest}, raw


def _manager_observation(journey: dict[str, Any]) -> dict[str, Any]:
    expected = journey.get("expected_observations")
    if type(expected) is not dict:
        raise ContractError("invalid_journey_observation", "journey observations are malformed")
    observation = _exact_object(
        expected.get("manager_field_token_filter"),
        {
            "model",
            "field_token",
            "operator_literal",
            "operator_outcomes",
            "conjunction",
            "terminals",
            "first",
            "rejections",
            "borrowed_read",
        },
        "manager-filter observation",
    )
    if observation.get("model") != "person":
        raise ContractError("invalid_journey_observation", "manager model drifted")
    field_token = _exact_object(
        observation.get("field_token"),
        {"owner", "attribute", "binding_name"},
        "manager field token",
    )
    if field_token != {
        "owner": "entity:person",
        "attribute": "attribute:foo__bar",
        "binding_name": "foo__bar",
    }:
        raise ContractError("invalid_journey_observation", "manager field token drifted")
    literal = _exact_object(
        observation.get("operator_literal"),
        {"kind", "value"},
        "manager operator literal",
    )
    if literal != {"kind": "long", "value": "7"}:
        raise ContractError("invalid_journey_observation", "manager literal drifted")
    outcomes = observation.get("operator_outcomes")
    if type(outcomes) is not list or [item.get("operator") for item in outcomes] != [
        "eq",
        "ne",
        "gt",
        "gte",
        "lt",
        "lte",
    ]:
        raise ContractError("invalid_journey_observation", "manager operator ledger drifted")
    for index, outcome in enumerate(outcomes):
        _exact_object(outcome, {"operator", "normalized_keys"}, f"operator outcome {index}")
        keys = outcome["normalized_keys"]
        if (
            type(keys) is not list
            or keys != sorted(keys)
            or any(type(key) is not str for key in keys)
        ):
            raise ContractError(
                "invalid_journey_observation",
                f"operator outcome {index} keys are not normalized ascending",
            )
    conjunction = _exact_object(
        observation.get("conjunction"),
        {"authored_order", "normalized_keys"},
        "manager conjunction",
    )
    if conjunction["authored_order"] != ["foo__bar:gte:7", "score:gt:40"]:
        raise ContractError("invalid_journey_observation", "authored predicate order drifted")
    if conjunction["normalized_keys"] != sorted(conjunction["normalized_keys"]):
        raise ContractError("invalid_journey_observation", "conjunction keys are not normalized")
    terminals = _exact_object(
        observation.get("terminals"),
        {"all_normalization", "count", "exists"},
        "manager terminals",
    )
    if terminals != {
        "all_normalization": "reference_key_ascending",
        "count": 2,
        "exists": True,
    }:
        raise ContractError("invalid_journey_observation", "manager terminals drifted")
    first = _exact_object(
        observation.get("first"),
        {"identity_predicate", "strict_singular", "result", "nonsingular_rejection"},
        "manager first",
    )
    if (
        first["identity_predicate"] != "identifier:eq:data-ada"
        or first["strict_singular"] is not True
        or first["result"] != "data-ada"
    ):
        raise ContractError("invalid_journey_observation", "strict manager first drifted")
    rejection = _exact_object(
        first["nonsingular_rejection"],
        {"category", "code", "rejected_before_provider_io"},
        "nonsingular first rejection",
    )
    if rejection != {
        "category": "invalid_input",
        "code": "manager_first_requires_identity",
        "rejected_before_provider_io": True,
    }:
        raise ContractError("invalid_journey_observation", "nonsingular first rejection drifted")
    rejections = observation.get("rejections")
    if type(rejections) is not list or [item.get("kind") for item in rejections] != [
        "wrong_field_owner",
        "wrong_scalar_domain",
        "wrong_package",
        "boolean_ordering",
    ]:
        raise ContractError("invalid_journey_observation", "manager rejection ledger drifted")
    for index, item in enumerate(rejections):
        _exact_object(
            item,
            {"kind", "category", "code", "rejected_before_provider_io"},
            f"manager rejection {index}",
        )
        if item["rejected_before_provider_io"] is not True:
            raise ContractError(
                "invalid_journey_observation",
                f"manager rejection {index} is not pre-I/O",
            )
    borrowed = _exact_object(
        observation.get("borrowed_read"),
        {"reusable_after_each_terminal", "sibling_filter_usable", "final_state"},
        "borrowed-read observation",
    )
    if borrowed != {
        "reusable_after_each_terminal": True,
        "sibling_filter_usable": True,
        "final_state": "active",
    }:
        raise ContractError("invalid_journey_observation", "borrowed-read lifecycle drifted")
    _reject_forbidden_claims(observation, "contract.observation")
    return copy.deepcopy(observation)


def load_contract() -> Phase5ManagerLiveContract:
    """Derive the focused live observation from the frozen Workforce V3 journey."""

    authority: dict[str, dict[str, str]] = {}
    journey_raw = b""
    for label in ("schema", "journey", "provider"):
        identity, raw = _source_identity(label)
        authority[label] = identity
        if label == "journey":
            journey_raw = raw
    journey = _exact_object(
        _json_value(journey_raw, "Phase-5 journey"),
        {
            "format",
            "fixture_id",
            "version",
            "semantic_profile",
            "records",
            "create_order",
            "cleanup_order",
            "expected_observations",
        },
        "Phase-5 journey",
    )
    if (
        journey["format"] != "typebridge.workforce-journey/v3"
        or journey["fixture_id"] != "workforce-v3"
        or journey["version"] != 3
        or journey["semantic_profile"] != SEMANTIC_PROFILE
    ):
        raise ContractError("invalid_journey_authority", "workforce-v3 identity drifted")
    return Phase5ManagerLiveContract(
        authority=authority,
        observation=_manager_observation(journey),
    )


def expected_report(binding: str, contract: Phase5ManagerLiveContract) -> dict[str, Any]:
    """Return the one exact canonical report value admitted for a binding."""

    if binding not in REPORT_BINDINGS:
        raise ContractError("unknown_binding", f"unknown Phase-5 binding {binding!r}")
    return {
        "authority": copy.deepcopy(contract.authority),
        "binding": binding,
        "format": REPORT_FORMAT,
        "observation": copy.deepcopy(contract.observation),
        "semantic_profile": SEMANTIC_PROFILE,
    }


def _load_report(
    path: Path,
    contract: Phase5ManagerLiveContract,
) -> tuple[str, dict[str, Any]]:
    raw = _regular_bytes(path, f"report {path}", MAX_REPORT_BYTES)
    value = _json_value(raw, f"report {path}")
    if raw != canonical_json_bytes(value):
        raise ContractError(
            "noncanonical_report_json",
            f"report {path} must be compact key-sorted JSON with one trailing newline",
        )
    report = _exact_object(
        value,
        {"authority", "binding", "format", "observation", "semantic_profile"},
        f"report {path}",
    )
    _reject_forbidden_claims(report)
    binding = report["binding"]
    if type(binding) is not str or binding not in REPORT_BINDINGS:
        raise ContractError("unknown_binding", f"report {path} has an unknown binding")
    if report != expected_report(binding, contract):
        raise ContractError(
            "phase5_manager_live_observation_mismatch",
            f"{binding} report differs from the independently derived manager contract",
        )
    return binding, report


def compare_reports(paths: list[Path]) -> dict[str, Any]:
    """Validate exactly one report per binding and return a canonical summary."""

    if len(paths) != len(REPORT_BINDINGS):
        raise ContractError("report_count_mismatch", "exactly four reports are required")
    contract = load_contract()
    reports: dict[str, dict[str, Any]] = {}
    for path in paths:
        binding, report = _load_report(path, contract)
        if binding in reports:
            raise ContractError("duplicate_binding_report", f"duplicate {binding} report")
        reports[binding] = report
    missing = set(REPORT_BINDINGS) - set(reports)
    if missing:
        raise ContractError("missing_binding_report", f"missing reports: {sorted(missing)}")
    if any(report["authority"] != contract.authority for report in reports.values()):
        raise ContractError("authority_mismatch", "binding authorities differ")
    if any(report["observation"] != contract.observation for report in reports.values()):
        raise ContractError("observation_mismatch", "binding observations differ")
    return {
        "authority": copy.deepcopy(contract.authority),
        "bindings": list(REPORT_BINDINGS),
        "format": SUMMARY_FORMAT,
        "observation_sha256": hashlib.sha256(
            canonical_json_bytes(contract.observation)
        ).hexdigest(),
        "semantic_profile": SEMANTIC_PROFILE,
    }


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "reports",
        nargs=4,
        type=Path,
        metavar="REPORT",
        help="one canonical manager-filter report each for Python, Node, Rust, and C",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    arguments = _parser().parse_args(argv)
    try:
        summary = compare_reports(arguments.reports)
    except (ContractError, OSError) as error:
        print(f"Phase-5 manager-filter live parity rejected: {error}", file=sys.stderr)
        return 1
    sys.stdout.buffer.write(canonical_json_bytes(summary))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
