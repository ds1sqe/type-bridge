"""Cross-language parity helpers for Python writer and Node reader tests."""

from __future__ import annotations

import difflib
import json
import os
import shutil
import subprocess
from datetime import datetime
from pathlib import Path
from typing import Any

import pytest

from tests.integration.parity.canonical import canonical_json, load_fixture_contract
from tests.integration.parity.models import (
    ParityActive,
    ParityAge,
    ParityBalance,
    ParityBirthDate,
    ParityCompany,
    ParityConfidence,
    ParityEmail,
    ParityEmailMessage,
    ParityId,
    ParityKind,
    ParityLoginAt,
    ParityMembership,
    ParityName,
    ParityNote,
    ParityPerson,
    ParityScore,
    ParitySeenAt,
    ParitySessionLength,
    ParitySince,
    ParityTag,
    ParityTokenOrigin,
)

REPO_ROOT = Path(__file__).resolve().parents[3]
DEFAULT_NODE_NATIVE = REPO_ROOT / "tmp" / "type_bridge_node.node"
NODE_READER = Path(__file__).with_name("node_reader.cjs")

ENTITY_CLASSES = {
    "parity-person": ParityPerson,
    "parity-company": ParityCompany,
    "parity-email-message": ParityEmailMessage,
}

ATTRIBUTE_CLASSES = {
    "id": ParityId,
    "name": ParityName,
    "email": ParityEmail,
    "age": ParityAge,
    "score": ParityScore,
    "active": ParityActive,
    "birth_date": ParityBirthDate,
    "login_at": ParityLoginAt,
    "seen_at": ParitySeenAt,
    "balance": ParityBalance,
    "session_length": ParitySessionLength,
    "tags": ParityTag,
    "note": ParityNote,
    "since": ParitySince,
    "confidence": ParityConfidence,
    "kind": ParityKind,
}

NODE_VALUE_TYPES = {
    "String": "string",
    "Long": "long",
    "Double": "double",
    "Boolean": "boolean",
    "Date": "date",
    "DateTime": "datetime",
    "DateTimeTZ": "datetime-tz",
    "Decimal": "decimal",
    "Duration": "duration",
}


def load_parity_schema(db: Any) -> None:
    """Load the shared parity TypeQL schema into a fresh TypeDB database."""
    db.execute_query(load_fixture_contract()["schema"], transaction_type="schema")


def write_fixture_with_python(db: Any) -> None:
    """Write the shared fixture through public Python model manager APIs."""
    contract = load_fixture_contract()
    entities_by_id: dict[str, Any] = {}

    for row in contract["write_data"]["entities"]:
        entity = _build_entity(row)
        ENTITY_CLASSES[row["type"]].manager(db).insert(entity)
        entities_by_id[row["stable_id"]] = entity

    for row in contract["write_data"]["relations"]:
        relation = _build_relation(row, entities_by_id)
        relation.__class__.manager(db).insert(relation)


def read_with_node(address: str, database: str) -> dict[str, Any]:
    """Read fixture rows through the public Node package dynamic manager surface."""
    if shutil.which("node") is None:
        pytest.skip("node executable is not installed")

    env = dict(os.environ)
    env["TYPEDB_ADDRESS"] = address
    env["TYPE_BRIDGE_PARITY_DATABASE"] = database
    if "TYPE_BRIDGE_NODE_NATIVE_PATH" not in env and DEFAULT_NODE_NATIVE.exists():
        env["TYPE_BRIDGE_NODE_NATIVE_PATH"] = str(DEFAULT_NODE_NATIVE)

    completed = subprocess.run(
        ["node", str(NODE_READER)],
        check=False,
        cwd=REPO_ROOT,
        env=env,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise AssertionError(
            f"Node parity reader failed\nstdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )
    return json.loads(completed.stdout)


def assert_node_output_matches_expected(raw_output: dict[str, Any]) -> None:
    contract = load_fixture_contract()
    actual = canonicalize_node_reader_output(raw_output, contract)
    expected = contract["expected"]
    actual_json = canonical_json(actual)
    expected_json = canonical_json(expected)
    if actual_json != expected_json:
        diff = "\n".join(
            difflib.unified_diff(
                expected_json.splitlines(),
                actual_json.splitlines(),
                fromfile="expected-canonical.json",
                tofile="node-reader",
                lineterm="",
            )
        )
        raise AssertionError(f"Node reader canonical output drifted:\n{diff}")


def canonicalize_node_reader_output(
    raw_output: dict[str, Any],
    contract: dict[str, Any],
) -> dict[str, Any]:
    descriptors = _descriptor_maps(contract["descriptors"])
    entities = [
        _canonical_entity(row, descriptors[section["type_name"]])
        for section in raw_output["entities"]
        for row in section["rows"]
    ]
    relations = [
        _canonical_relation(row, descriptors[section["type_name"]], contract)
        for section in raw_output["relations"]
        for row in _merge_relation_rows(section["rows"])
    ]
    return {
        "fixture_id": contract["expected"]["fixture_id"],
        "version": contract["expected"]["version"],
        "entities": sorted(entities, key=lambda row: row["stable_id"]),
        "relations": sorted(relations, key=lambda row: row["stable_id"]),
    }


def _build_entity(row: dict[str, Any]) -> Any:
    kwargs = _build_attribute_kwargs(row["attributes"])
    if row["type"] == "parity-person":
        kwargs.setdefault("tags", [])
    return ENTITY_CLASSES[row["type"]](**kwargs)


def _build_relation(row: dict[str, Any], entities_by_id: dict[str, Any]) -> Any:
    kwargs = _build_attribute_kwargs(row["attributes"])
    roles = {
        role_name: [entities_by_id[player["stable_id"]] for player in players]
        for role_name, players in row["roles"].items()
    }
    if row["type"] == "parity-membership":
        return ParityMembership(
            member=roles["member"][0],
            organization=roles["organization"][0],
            evidence=roles["evidence"],
            **kwargs,
        )
    if row["type"] == "parity-token-origin":
        return ParityTokenOrigin(
            token=roles["token"][0],
            issue=roles["issue"][0],
            **kwargs,
        )
    raise AssertionError(f"unknown relation fixture type: {row['type']}")


def _build_attribute_kwargs(attributes: dict[str, Any]) -> dict[str, Any]:
    kwargs: dict[str, Any] = {}
    for field_name, value in attributes.items():
        attr_cls = ATTRIBUTE_CLASSES[field_name]
        if isinstance(value, list):
            kwargs[field_name] = [attr_cls(_python_value(item)) for item in value]
        else:
            kwargs[field_name] = attr_cls(_python_value(value))
    return kwargs


def _python_value(value: dict[str, Any]) -> Any:
    raw_value = value["value"]
    if value["type"] in {"datetime", "datetime-tz"}:
        return datetime.fromisoformat(raw_value)
    return raw_value


def _descriptor_maps(descriptors: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {
        descriptor["type_name"]: descriptor
        for section in ("entities", "relations")
        for descriptor in descriptors[section]
    }


def _canonical_entity(row: dict[str, Any], descriptor: dict[str, Any]) -> dict[str, Any]:
    attributes = _canonical_attributes(row["attributes"], descriptor)
    return {
        "stable_id": attributes["id"]["value"],
        "type": row["type_name"],
        "attributes": attributes,
    }


def _canonical_relation(
    row: dict[str, Any],
    descriptor: dict[str, Any],
    contract: dict[str, Any],
) -> dict[str, Any]:
    attributes = _canonical_attributes(row["attributes"], descriptor)
    roles = _canonical_roles(row["role_players"])
    relation = {
        "stable_id": _relation_stable_id(row["type_name"], attributes, roles, contract),
        "type": row["type_name"],
        "attributes": attributes,
        "roles": roles,
    }
    return relation


def _canonical_attributes(
    raw_attributes: list[list[Any]],
    descriptor: dict[str, Any],
) -> dict[str, Any]:
    values_by_attr: dict[str, list[Any]] = {}
    for attr_name, value in raw_attributes:
        values_by_attr.setdefault(attr_name, []).append(value)

    attributes: dict[str, Any] = {}
    for attr in descriptor["owned_attributes"]:
        values = values_by_attr.get(attr["attr_name"], [])
        if not values:
            continue
        canonical_values = [_canonical_value(value, attr["value_type"]) for value in values]
        canonical_values.sort(key=canonical_json)
        if _is_multi_value_attribute(attr):
            attributes[attr["field_name"]] = canonical_values
        else:
            attributes[attr["field_name"]] = canonical_values[0]
    return dict(sorted(attributes.items()))


def _canonical_roles(raw_players: list[dict[str, Any]]) -> dict[str, list[dict[str, str]]]:
    roles: dict[str, list[dict[str, str]]] = {}
    for player in raw_players:
        stable_id = _stable_id_from_player(player)
        roles.setdefault(player["role_name"], []).append(
            {
                "stable_id": stable_id,
                "type": player["player_type_name"],
            }
        )
    return {
        role_name: sorted(players, key=lambda player: player["stable_id"])
        for role_name, players in sorted(roles.items())
    }


def _canonical_value(value: Any, value_type: str) -> dict[str, Any]:
    raw_value = _unwrap_node_value(value)
    if value_type == "long":
        raw_value = str(raw_value)
    elif value_type == "decimal":
        raw_value = str(raw_value).removesuffix("dec")
    elif value_type == "datetime":
        raw_value = _trim_zero_nanoseconds(str(raw_value))
    elif value_type == "datetime-tz":
        raw_value = _trim_zero_nanoseconds(str(raw_value).replace("Z", "+00:00"))
    return {
        "type": value_type,
        "value": raw_value,
    }


def _unwrap_node_value(value: Any) -> Any:
    if isinstance(value, dict):
        for key in NODE_VALUE_TYPES:
            if key in value:
                return value[key]
        if "value" in value:
            return _unwrap_node_value(value["value"])
    return value


def _trim_zero_nanoseconds(value: str) -> str:
    return value.replace(".000000000", "")


def _is_multi_value_attribute(attribute: dict[str, Any]) -> bool:
    for annotation in attribute["annotations"]:
        if isinstance(annotation, dict) and "Card" in annotation:
            _, max_card = annotation["Card"]
            return max_card is None or max_card > 1
    return False


def _stable_id_from_player(player: dict[str, Any]) -> str:
    for attr_name, value in player.get("attributes", []):
        if attr_name == "parity-id":
            return str(_unwrap_node_value(value))
    raise AssertionError(f"role player is missing parity-id: {player}")


def _merge_relation_rows(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    merged: dict[tuple[Any, ...], dict[str, Any]] = {}
    seen_players: dict[tuple[Any, ...], set[tuple[Any, ...]]] = {}
    for row in rows:
        key = (
            row.get("iid"),
            row.get("type_name"),
            canonical_json(row.get("attributes", [])),
        )
        target = merged.setdefault(
            key,
            {
                **row,
                "role_players": [],
            },
        )
        player_keys = seen_players.setdefault(key, set())
        for player in row.get("role_players", []):
            player_key = (
                player.get("role_name"),
                player.get("player_type_name"),
                _stable_id_from_player(player),
            )
            if player_key in player_keys:
                continue
            player_keys.add(player_key)
            target["role_players"].append(player)
    return list(merged.values())


def _relation_stable_id(
    type_name: str,
    attributes: dict[str, Any],
    roles: dict[str, list[dict[str, str]]],
    contract: dict[str, Any],
) -> str:
    signatures = {
        _relation_signature(row["type"], row["attributes"], row["roles"]): row["stable_id"]
        for row in contract["write_data"]["relations"]
    }
    signature = _relation_signature(type_name, attributes, roles)
    try:
        return signatures[signature]
    except KeyError as exc:
        raise AssertionError(f"could not match relation fixture signature: {signature}") from exc


def _relation_signature(
    type_name: str,
    attributes: dict[str, Any],
    roles: dict[str, list[dict[str, str]]],
) -> str:
    value = {
        "type": type_name,
        "attributes": attributes,
        "roles": {
            role_name: sorted(players, key=lambda player: player["stable_id"])
            for role_name, players in sorted(roles.items())
        },
    }
    return canonical_json(value)
