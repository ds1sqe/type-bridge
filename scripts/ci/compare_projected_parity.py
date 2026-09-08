#!/usr/bin/env python3
"""Compare the four provider-free Projected generated projection reports."""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import stat
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
SCHEMA_RELATIVE = "tests/contracts/sdk_conformance/sdk-v3/schema-v3.yaml"
JOURNEY_RELATIVE = "tests/contracts/sdk_conformance/sdk-v3/journey-v3.json"

REPORT_FORMAT = "typebridge.projected-parity-report/v1"
SUMMARY_FORMAT = "typebridge.projected-parity-summary/v1"
SEMANTIC_PROFILE = "typedb-3.12.1/v1"
REPORT_BINDINGS = ("python", "node", "rust", "c")
OBSERVATION_REFS = (
    "canonical_scalar_values",
    "field_name_identity",
    "inherited_relation_role",
    "integer_key_polymorphic_role",
    "ordered_distinct_collections",
    "projected_constraint_validation",
    "token_package_fencing",
)
MAX_REPORT_BYTES = 256 * 1024
MAX_AUTHORITY_BYTES = 1024 * 1024


class ContractError(ValueError):
    """A stable fail-closed Projected parity rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code


@dataclass(frozen=True)
class SourceIdentity:
    path: str
    sha256: str

    def as_json(self) -> dict[str, str]:
        return {"path": self.path, "sha256": self.sha256}


@dataclass(frozen=True)
class ProjectedParityContract:
    authority: dict[str, dict[str, str]]
    observations: dict[str, Any]


def _duplicate_key_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise ContractError("duplicate_json_key", f"duplicate JSON key {key!r}")
        value[key] = item
    return value


def canonical_json_bytes(value: Any) -> bytes:
    """Return the parity contract's compact deterministic JSON spelling."""

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
    with path.open("rb") as source:
        raw = source.read(limit + 1)
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


def _source_identity(relative: str, label: str) -> tuple[SourceIdentity, bytes]:
    path = ROOT / relative
    raw = _regular_bytes(path, label, MAX_AUTHORITY_BYTES)
    return SourceIdentity(relative, hashlib.sha256(raw).hexdigest()), raw


def _required_observation(expected: dict[str, Any], name: str) -> dict[str, Any]:
    value = expected.get(name)
    if type(value) is not dict:
        raise ContractError(
            "invalid_journey_observation",
            f"journey observation {name!r} is absent or malformed",
        )
    return value


def _inherited_relation_role(expected: dict[str, Any]) -> dict[str, Any]:
    lifecycle = _exact_object(
        _required_observation(expected, "inherited_relation_role_lifecycle"),
        {
            "model",
            "inherited_relation",
            "inherited_role",
            "player_model",
            "created",
            "read_after_create",
            "role_identity_preserved",
            "deleted",
            "read_after_delete",
            "count_after_delete",
        },
        "inherited relation-role observation",
    )
    return {
        "model": lifecycle["model"],
        "inherited_relation": lifecycle["inherited_relation"],
        "inherited_role": lifecycle["inherited_role"],
        "player_model": lifecycle["player_model"],
        "constructed": lifecycle["created"],
        "hydrated": lifecycle["read_after_create"],
        "role_identity_preserved": lifecycle["role_identity_preserved"],
    }


def _canonical_scalar_values(journey: dict[str, Any]) -> dict[str, Any]:
    records = journey["records"]
    if type(records) is not dict or type(records.get("people")) is not list:
        raise ContractError("invalid_journey_record", "journey people records are malformed")
    ada = next(
        (
            record
            for record in records["people"]
            if type(record) is dict and record.get("ref") == "data-ada"
        ),
        None,
    )
    if type(ada) is not dict or type(ada.get("fields")) is not dict:
        raise ContractError("invalid_journey_record", "Ada's sdk record is absent")
    fields = ada["fields"]
    field_by_domain = {
        "boolean": "val_bool",
        "date": "val_date",
        "datetime": "val_datetime",
        "datetime_tz": "val_datetime_tz",
        "decimal": "val_decimal",
        "double": "val_double",
        "duration": "val_duration",
        "long": "score",
        "string": "nickname",
    }
    values = {}
    for domain, field in field_by_domain.items():
        value = fields.get(field)
        if type(value) is not dict or value.get("kind") != domain:
            raise ContractError(
                "invalid_journey_record",
                f"Ada's {field!r} value does not carry the {domain!r} domain",
            )
        values[domain] = copy.deepcopy(value)
    return {
        "authored": copy.deepcopy(values),
        "constructed": copy.deepcopy(values),
        "hydrated": copy.deepcopy(values),
    }


def _field_name_identity(expected: dict[str, Any]) -> dict[str, Any]:
    identity = _exact_object(
        _required_observation(expected, "field_name_identity"),
        {
            "generated_tokens",
            "token_identities_distinct",
            "package_branded",
            "compatibility_lookup",
            "generated_token_string_parser_used",
        },
        "field-name identity observation",
    )
    return {
        "generated_tokens": copy.deepcopy(identity["generated_tokens"]),
        "token_identities_distinct": identity["token_identities_distinct"],
        "package_branded": identity["package_branded"],
    }


def _token_package_fencing(expected: dict[str, Any]) -> dict[str, Any]:
    fencing = _exact_object(
        _required_observation(expected, "token_package_fencing"),
        {"accepted_local", "rejections", "rejected_token_states", "provider_text_exposed"},
        "token-package observation",
    )
    accepted = _exact_object(
        fencing["accepted_local"],
        {"construction", "batch", "filter", "hydration"},
        "accepted local package operations",
    )
    rejections = _exact_object(
        fencing["rejections"],
        {"construction", "batch", "filter", "hydration"},
        "token-package rejections",
    )
    states = fencing["rejected_token_states"]
    if type(states) is not list or "foreign" not in states:
        raise ContractError(
            "invalid_journey_observation",
            "token-package states must include the foreign-package fence",
        )
    return {
        "accepted_local": {
            "construction": accepted["construction"],
            "hydration": accepted["hydration"],
        },
        "foreign_rejections": {
            "construction": copy.deepcopy(rejections["construction"]),
            "hydration": copy.deepcopy(rejections["hydration"]),
        },
        "provider_text_exposed": fencing["provider_text_exposed"],
    }


def load_contract() -> ProjectedParityContract:
    """Load the Projected subset without finalizing the broader V3 authority."""

    schema_identity, _ = _source_identity(SCHEMA_RELATIVE, "Projected schema")
    journey_identity, journey_raw = _source_identity(JOURNEY_RELATIVE, "Projected journey")
    journey = _exact_object(
        _json_value(journey_raw, "Projected journey"),
        {
            "format",
            "fixture_id",
            "version",
            "semantic_profile",
            "create_order",
            "cleanup_order",
            "records",
            "expected_observations",
        },
        "Projected journey",
    )
    if (
        journey["format"] != "typebridge.sdk-journey/v3"
        or journey["fixture_id"] != "sdk-v3"
        or journey["version"] != 3
        or journey["semantic_profile"] != SEMANTIC_PROFILE
    ):
        raise ContractError("invalid_journey_authority", "sdk-v3 journey identity drifted")
    expected = journey["expected_observations"]
    if type(expected) is not dict:
        raise ContractError("invalid_journey_observation", "expected observations are malformed")
    observations = {
        "canonical_scalar_values": _canonical_scalar_values(journey),
        "field_name_identity": _field_name_identity(expected),
        "inherited_relation_role": _inherited_relation_role(expected),
        "integer_key_polymorphic_role": copy.deepcopy(
            _required_observation(expected, "integer_key_polymorphic_role")
        ),
        "ordered_distinct_collections": copy.deepcopy(
            _required_observation(expected, "ordered_distinct_collections")
        ),
        "projected_constraint_validation": copy.deepcopy(
            _required_observation(expected, "projected_constraint_validation")
        ),
        "token_package_fencing": _token_package_fencing(expected),
    }
    if tuple(sorted(observations)) != tuple(sorted(OBSERVATION_REFS)):
        raise ContractError("invalid_observation_ledger", "Projected observation ledger drifted")
    return ProjectedParityContract(
        authority={
            "journey": journey_identity.as_json(),
            "schema": schema_identity.as_json(),
        },
        observations=observations,
    )


def expected_report(binding: str, contract: ProjectedParityContract) -> dict[str, Any]:
    """Return the one exact report shape a binding producer must emit."""

    if binding not in REPORT_BINDINGS:
        raise ContractError("unknown_binding", f"unknown Projected binding {binding!r}")
    return {
        "format": REPORT_FORMAT,
        "binding": binding,
        "semantic_profile": SEMANTIC_PROFILE,
        "authority": copy.deepcopy(contract.authority),
        "observations": copy.deepcopy(contract.observations),
    }


def _load_report(path: Path, contract: ProjectedParityContract) -> tuple[str, dict[str, Any]]:
    raw = _regular_bytes(path, f"report {path}", MAX_REPORT_BYTES)
    value = _json_value(raw, f"report {path}")
    if raw != canonical_json_bytes(value):
        raise ContractError(
            "noncanonical_report_json",
            f"report {path} must be compact key-sorted JSON with one trailing newline",
        )
    report = _exact_object(
        value,
        {"format", "binding", "semantic_profile", "authority", "observations"},
        f"report {path}",
    )
    binding = report["binding"]
    if type(binding) is not str or binding not in REPORT_BINDINGS:
        raise ContractError("unknown_binding", f"report {path} has an unknown binding")
    expected = expected_report(binding, contract)
    if report != expected:
        raise ContractError(
            "projected_observation_mismatch",
            f"{binding} report differs from the frozen Projected observations",
        )
    return binding, report


def compare_reports(paths: list[Path]) -> dict[str, Any]:
    """Validate exactly one report per binding and return a canonical summary value."""

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
    observation_sha256 = hashlib.sha256(canonical_json_bytes(contract.observations)).hexdigest()
    return {
        "format": SUMMARY_FORMAT,
        "semantic_profile": SEMANTIC_PROFILE,
        "bindings": list(REPORT_BINDINGS),
        "authority": copy.deepcopy(contract.authority),
        "observation_sha256": observation_sha256,
    }


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "reports",
        nargs=4,
        type=Path,
        metavar="REPORT",
        help="one canonical report each for Python, Node, Rust, and C (order-independent)",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    arguments = _parser().parse_args(argv)
    try:
        summary = compare_reports(arguments.reports)
    except (ContractError, OSError) as error:
        print(f"Projected projected parity rejected: {error}", file=sys.stderr)
        return 1
    sys.stdout.buffer.write(canonical_json_bytes(summary))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
