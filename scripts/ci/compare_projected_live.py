#!/usr/bin/env python3
"""Compare the four exact-TypeDB-3.12.3 Projected live projection reports."""

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
SCHEMA_RELATIVE = "tests/contracts/sdk_conformance/sdk-v3/schema-v3.yaml"
JOURNEY_RELATIVE = "tests/contracts/sdk_conformance/sdk-v3/journey-v3.json"
PROVIDER_RELATIVE = "tests/contracts/sdk_conformance/sdk-v3/provider-3.12.1-v3.tql"

REPORT_FORMAT = "typebridge.projected-live-report/v1"
SUMMARY_FORMAT = "typebridge.projected-live-summary/v1"
SEMANTIC_PROFILE = "typedb-3.12.1/v1"
REPORT_BINDINGS = ("python", "node", "rust", "c")
REPORT_OUTPUT_ENV = "TYPE_BRIDGE_PROJECTED_LIVE_REPORT"
REPORT_WRITE_SEMANTICS = "create_new"
OBSERVATION_REFS = (
    "canonical_scalar_values",
    "cleanup",
    "inherited_plain_activity_role_lifecycle",
    "integer_key_polymorphic_optional_role",
    "relation_as_player",
)

SOURCE_SHA256 = {
    "schema": "74b9161e3d8fd70a1f22c3a8b7fc70a823b18cb07973064e2e10ce0940f76f1f",
    "journey": "c5679b428c22e2bff7989f6d674cde3780b752a40f6bbce6ccb3aa451797b1c0",
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

# None of these concepts belong in this deliberately narrow live report. Exact
# report equality would reject them too; this scan gives scope creep a stable,
# immediate failure instead of allowing it to masquerade as an observation.
FORBIDDEN_REPORT_KEYS = frozenset(
    {
        "address",
        "aliases",
        "batch",
        "batch_insert",
        "batch_update",
        "database",
        "database_name",
        "endpoint",
        "filter",
        "filters",
        "host",
        "iid",
        "ordered",
        "ordered_distinct",
        "ordered_list",
        "ordered_list_persistence",
        "ordered_persistence",
        "persisted_order",
        "port",
    }
)
FORBIDDEN_PRODUCER_IMPORT_MARKERS = (
    "compare_projected_live",
    "compare_projected_parity",
    "expected_observations",
    "expected_report",
    "load_contract",
)
TYPEDB_IID_RE = re.compile(r"^0x[0-9a-f]{2,}$")
ENDPOINT_RE = re.compile(r"^(?:https?|typedb)://", re.IGNORECASE)

SCALAR_FIELDS = {
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
LIVE_RECORD_REFS = frozenset(
    {
        "container-event",
        "data-ada",
        "data-dana",
        "event-ada",
        "interaction-absent",
        "interaction-person",
        "interaction-robot",
        "plain-activity-ada",
        "robot-7",
        "robot-negative-7",
    }
)


class ContractError(ValueError):
    """A stable fail-closed Projected live-contract rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code


@dataclass(frozen=True)
class ProjectedLiveContract:
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
    """Return the live contract's compact deterministic JSON spelling."""

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


def _exact_list(value: Any, label: str) -> list[Any]:
    if type(value) is not list:
        raise ContractError("invalid_list", f"{label} must be an array")
    return value


def _string(value: Any, label: str) -> str:
    if type(value) is not str or not value:
        raise ContractError("invalid_string", f"{label} must be a non-empty string")
    return value


def _normalize_claim_key(key: str) -> str:
    return key.casefold().replace("-", "_")


def _reject_forbidden_claims(value: Any, path: str = "report") -> None:
    if type(value) is dict:
        for key, item in value.items():
            normalized = _normalize_claim_key(key)
            if normalized in FORBIDDEN_REPORT_KEYS:
                raise ContractError(
                    "forbidden_report_claim",
                    f"{path}.{key} is outside the Projected live contract",
                )
            _reject_forbidden_claims(item, f"{path}.{key}")
    elif type(value) is list:
        for index, item in enumerate(value):
            _reject_forbidden_claims(item, f"{path}[{index}]")
    elif type(value) is str:
        if TYPEDB_IID_RE.fullmatch(value) or ENDPOINT_RE.match(value):
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
    raw = _regular_bytes(ROOT / relative, f"Projected {label}", MAX_AUTHORITY_BYTES)
    digest = hashlib.sha256(raw).hexdigest()
    if digest != SOURCE_SHA256[label]:
        raise ContractError(
            "authority_hash_mismatch",
            f"Projected {label} is not the frozen exact-3.12.1 authority",
        )
    return {"path": relative, "sha256": digest}, raw


def _record_index(journey: dict[str, Any]) -> dict[str, dict[str, Any]]:
    records = _exact_object(
        journey["records"],
        {
            "people",
            "robots",
            "counters",
            "memberships",
            "network_links",
            "interactions",
            "plain_activity",
            "event",
            "container",
        },
        "journey records",
    )
    index: dict[str, dict[str, Any]] = {}
    for group in (
        "people",
        "robots",
        "counters",
        "memberships",
        "network_links",
        "interactions",
    ):
        values = _exact_list(records[group], f"journey records.{group}")
        for offset, value in enumerate(values):
            if type(value) is not dict:
                raise ContractError(
                    "invalid_journey_record",
                    f"journey records.{group}[{offset}] must be an object",
                )
            ref = _string(value.get("ref"), f"journey records.{group}[{offset}].ref")
            _string(value.get("model"), f"journey records.{group}[{offset}].model")
            if ref in index:
                raise ContractError("duplicate_journey_ref", f"duplicate journey ref {ref!r}")
            index[ref] = value
    for group in ("plain_activity", "event", "container"):
        value = records[group]
        if type(value) is not dict:
            raise ContractError(
                "invalid_journey_record",
                f"journey records.{group} must be an object",
            )
        ref = _string(value.get("ref"), f"journey records.{group}.ref")
        _string(value.get("model"), f"journey records.{group}.model")
        if ref in index:
            raise ContractError("duplicate_journey_ref", f"duplicate journey ref {ref!r}")
        index[ref] = value
    return index


def _typed_scalar(fields: dict[str, Any], domain: str, field: str) -> dict[str, Any]:
    value = fields.get(field)
    if type(value) is not dict or value.get("kind") != domain:
        raise ContractError(
            "invalid_journey_record",
            f"Ada's {field!r} value does not carry the {domain!r} domain",
        )
    expected_keys = {"kind", "bits"} if domain == "double" else {"kind", "value"}
    _exact_object(value, expected_keys, f"Ada scalar {field}")
    return copy.deepcopy(value)


def _ada_scalars(records: dict[str, dict[str, Any]]) -> dict[str, Any]:
    ada = records.get("data-ada")
    if type(ada) is not dict or ada.get("model") != "person":
        raise ContractError("invalid_journey_record", "Ada's person record is absent")
    fields = ada.get("fields")
    if type(fields) is not dict:
        raise ContractError("invalid_journey_record", "Ada's scalar fields are malformed")
    return {
        "hydrated": {
            domain: _typed_scalar(fields, domain, field) for domain, field in SCALAR_FIELDS.items()
        },
        "model": "person",
        "ref": "data-ada",
    }


def _robot_keys(records: dict[str, dict[str, Any]]) -> dict[str, Any]:
    keys = []
    for ref in ("robot-negative-7", "robot-7"):
        record = records.get(ref)
        if type(record) is not dict or record.get("model") != "robot":
            raise ContractError("invalid_journey_record", f"robot record {ref!r} is absent")
        fields = record.get("fields")
        if type(fields) is not dict:
            raise ContractError("invalid_journey_record", f"robot record {ref!r} is malformed")
        robot_id = _exact_object(
            fields.get("robot_id"),
            {"kind", "value"},
            f"robot record {ref!r} key",
        )
        if robot_id["kind"] != "long" or robot_id["value"] not in {"-7", "7"}:
            raise ContractError("invalid_journey_record", f"robot record {ref!r} key drifted")
        keys.append({"model": "robot", "ref": ref, "value": robot_id["value"]})
    if [item["value"] for item in keys] != ["-7", "7"]:
        raise ContractError("invalid_journey_record", "Robot signed-key order drifted")
    return {"integer_keys": keys}


def _single_role_player(record: dict[str, Any], role: str, label: str) -> dict[str, Any] | None:
    roles = record.get("roles")
    if type(roles) is not dict:
        raise ContractError("invalid_journey_record", f"{label} roles are malformed")
    if role not in roles:
        return None
    players = _exact_list(roles[role], f"{label} role {role}")
    if len(players) != 1 or type(players[0]) is not dict:
        raise ContractError(
            "invalid_journey_record",
            f"{label} role {role} must have exactly one player",
        )
    player = players[0]
    if set(player) != {"model", "key"}:
        raise ContractError("invalid_journey_record", f"{label} role {role} player drifted")
    key = player["key"]
    if type(key) not in {str, int}:
        raise ContractError("invalid_journey_record", f"{label} role {role} key drifted")
    return {"key": str(key), "model": _string(player["model"], f"{label} player model")}


def _interaction_actors(records: dict[str, dict[str, Any]]) -> dict[str, Any]:
    states = []
    for ref in ("interaction-absent", "interaction-person", "interaction-robot"):
        record = records.get(ref)
        if type(record) is not dict or record.get("model") != "interaction":
            raise ContractError(
                "invalid_journey_record",
                f"interaction record {ref!r} is absent",
            )
        states.append(
            {
                "actor": _single_role_player(record, "actor", f"interaction {ref}"),
                "relation_ref": ref,
            }
        )
    if states != [
        {"actor": None, "relation_ref": "interaction-absent"},
        {
            "actor": {"key": "data-ada", "model": "person"},
            "relation_ref": "interaction-person",
        },
        {
            "actor": {"key": "7", "model": "robot"},
            "relation_ref": "interaction-robot",
        },
    ]:
        raise ContractError("invalid_journey_record", "Interaction actor states drifted")
    return {"optional_role": "actor", "relation": "interaction", "states": states}


def _plain_activity_lifecycle(
    records: dict[str, dict[str, Any]], expected: dict[str, Any]
) -> dict[str, Any]:
    record = records.get("plain-activity-ada")
    if type(record) is not dict or record.get("model") != "plain-activity":
        raise ContractError("invalid_journey_record", "PlainActivity record is absent")
    player = _single_role_player(record, "participant", "PlainActivity")
    if player != {"key": "data-ada", "model": "person"}:
        raise ContractError("invalid_journey_record", "PlainActivity participant drifted")
    lifecycle = _exact_object(
        expected.get("inherited_relation_role_lifecycle"),
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
        "inherited-role lifecycle",
    )
    if lifecycle != {
        "model": "plain-activity",
        "inherited_relation": "base-activity",
        "inherited_role": "participant",
        "player_model": "person",
        "created": True,
        "read_after_create": True,
        "role_identity_preserved": True,
        "deleted": True,
        "read_after_delete": False,
        "count_after_delete": 0,
    }:
        raise ContractError("invalid_journey_observation", "PlainActivity lifecycle drifted")
    return {
        "count_after_delete": lifecycle["count_after_delete"],
        "created": lifecycle["created"],
        "deleted": lifecycle["deleted"],
        "inherited_relation": lifecycle["inherited_relation"],
        "model": lifecycle["model"],
        "participant": player,
        "read_after_create": lifecycle["read_after_create"],
        "read_after_delete": lifecycle["read_after_delete"],
        "ref": "plain-activity-ada",
        "role": lifecycle["inherited_role"],
        "role_identity_preserved": lifecycle["role_identity_preserved"],
    }


def _relation_as_player(
    records: dict[str, dict[str, Any]], expected: dict[str, Any]
) -> dict[str, Any]:
    event = records.get("event-ada")
    container = records.get("container-event")
    if type(event) is not dict or event.get("model") != "event":
        raise ContractError("invalid_journey_record", "Event relation record is absent")
    if type(container) is not dict or container.get("model") != "container":
        raise ContractError("invalid_journey_record", "Container relation record is absent")
    roles = container.get("roles")
    if type(roles) is not dict:
        raise ContractError("invalid_journey_record", "Container roles are malformed")
    players = _exact_list(roles.get("item"), "Container item role")
    if players != [{"model": "event", "ref": "event-ada"}]:
        raise ContractError("invalid_journey_record", "Event-as-Container player drifted")
    integer_role = _exact_object(
        expected.get("integer_key_polymorphic_role"),
        {
            "integer_keys",
            "relation",
            "optional_role",
            "polymorphic_players_observed",
            "present",
            "absent",
            "relation_as_player",
        },
        "integer-key polymorphic-role observation",
    )
    relation_player = _exact_object(
        integer_role["relation_as_player"],
        {"player_model", "owner_model", "role", "preserved"},
        "relation-as-player observation",
    )
    if relation_player != {
        "player_model": "event",
        "owner_model": "container",
        "role": "item",
        "preserved": True,
    }:
        raise ContractError("invalid_journey_observation", "relation-as-player proof drifted")
    return {
        "owner": {"model": "container", "ref": "container-event"},
        "player": {"model": "event", "ref": "event-ada"},
        "preserved": relation_player["preserved"],
        "role": relation_player["role"],
    }


def _cleanup(journey: dict[str, Any], records: dict[str, dict[str, Any]]) -> dict[str, Any]:
    create_order = _exact_list(journey["create_order"], "journey create order")
    cleanup_order = _exact_list(journey["cleanup_order"], "journey cleanup order")
    if any(type(ref) is not str for ref in create_order + cleanup_order):
        raise ContractError("invalid_operation_order", "journey operation refs must be strings")
    if len(set(create_order)) != len(create_order) or set(create_order) != set(records):
        raise ContractError("invalid_operation_order", "journey create order does not name records")
    if cleanup_order != list(reversed(create_order)):
        raise ContractError("invalid_operation_order", "journey cleanup is not reversed")
    if not LIVE_RECORD_REFS.issubset(records):
        raise ContractError("invalid_operation_order", "live record subset is incomplete")
    live_cleanup = [ref for ref in cleanup_order if ref in LIVE_RECORD_REFS]
    expected_live_cleanup = list(reversed([ref for ref in create_order if ref in LIVE_RECORD_REFS]))
    if live_cleanup != expected_live_cleanup:
        raise ContractError("invalid_operation_order", "live cleanup is not dependency-reversed")
    models = sorted({records[ref]["model"] for ref in LIVE_RECORD_REFS})
    return {
        "order": live_cleanup,
        "zero_checks": [{"count_after_cleanup": 0, "model": model} for model in models],
    }


def load_contract() -> ProjectedLiveContract:
    """Derive the exact live subset independently from the committed journey."""

    authority: dict[str, dict[str, str]] = {}
    journey_raw = b""
    for label in ("schema", "journey", "provider"):
        identity, raw = _source_identity(label)
        authority[label] = identity
        if label == "journey":
            journey_raw = raw
    journey = _exact_object(
        _json_value(journey_raw, "Projected journey"),
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
        "Projected journey",
    )
    if (
        journey["format"] != "typebridge.sdk-journey/v3"
        or journey["fixture_id"] != "sdk-v3"
        or journey["version"] != 3
        or journey["semantic_profile"] != SEMANTIC_PROFILE
    ):
        raise ContractError("invalid_journey_authority", "sdk-v3 identity drifted")
    expected = journey["expected_observations"]
    if type(expected) is not dict:
        raise ContractError("invalid_journey_observation", "journey observations are malformed")
    records = _record_index(journey)
    observations = {
        "canonical_scalar_values": _ada_scalars(records),
        "cleanup": _cleanup(journey, records),
        "inherited_plain_activity_role_lifecycle": _plain_activity_lifecycle(records, expected),
        "integer_key_polymorphic_optional_role": {
            **_robot_keys(records),
            **_interaction_actors(records),
        },
        "relation_as_player": _relation_as_player(records, expected),
    }
    if tuple(sorted(observations)) != tuple(sorted(OBSERVATION_REFS)):
        raise ContractError("invalid_observation_ledger", "live observation ledger drifted")
    _reject_forbidden_claims(observations, "contract.observations")
    return ProjectedLiveContract(authority=authority, observations=observations)


def expected_report(binding: str, contract: ProjectedLiveContract) -> dict[str, Any]:
    """Return the one exact canonical value a live binding report may contain."""

    if binding not in REPORT_BINDINGS:
        raise ContractError("unknown_binding", f"unknown Projected binding {binding!r}")
    return {
        "authority": copy.deepcopy(contract.authority),
        "binding": binding,
        "format": REPORT_FORMAT,
        "observations": copy.deepcopy(contract.observations),
        "semantic_profile": SEMANTIC_PROFILE,
    }


def _load_report(path: Path, contract: ProjectedLiveContract) -> tuple[str, dict[str, Any]]:
    raw = _regular_bytes(path, f"report {path}", MAX_REPORT_BYTES)
    value = _json_value(raw, f"report {path}")
    if raw != canonical_json_bytes(value):
        raise ContractError(
            "noncanonical_report_json",
            f"report {path} must be compact key-sorted JSON with one trailing newline",
        )
    report = _exact_object(
        value,
        {"authority", "binding", "format", "observations", "semantic_profile"},
        f"report {path}",
    )
    _reject_forbidden_claims(report)
    binding = report["binding"]
    if type(binding) is not str or binding not in REPORT_BINDINGS:
        raise ContractError("unknown_binding", f"report {path} has an unknown binding")
    if report != expected_report(binding, contract):
        raise ContractError(
            "projected_live_observation_mismatch",
            f"{binding} report differs from the independently derived live contract",
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
    authority = next(iter(reports.values()))["authority"]
    observations = next(iter(reports.values()))["observations"]
    if any(report["authority"] != authority for report in reports.values()):
        raise ContractError("authority_mismatch", "binding authorities differ")
    if any(report["observations"] != observations for report in reports.values()):
        raise ContractError("observation_mismatch", "binding observations differ")
    return {
        "authority": copy.deepcopy(contract.authority),
        "bindings": list(REPORT_BINDINGS),
        "format": SUMMARY_FORMAT,
        "observation_sha256": hashlib.sha256(
            canonical_json_bytes(contract.observations)
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
        help="one canonical live report each for Python, Node, Rust, and C",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    arguments = _parser().parse_args(argv)
    try:
        summary = compare_reports(arguments.reports)
    except (ContractError, OSError) as error:
        print(f"Projected projected live parity rejected: {error}", file=sys.stderr)
        return 1
    sys.stdout.buffer.write(canonical_json_bytes(summary))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
