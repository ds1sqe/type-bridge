"""Fixture loading and canonical JSON validation for cross-language parity."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

FIXTURES_DIR = Path(__file__).parent / "fixtures"

MVP_PRIMITIVES = {
    "string",
    "long",
    "double",
    "boolean",
    "date",
    "datetime",
    "datetime-tz",
    "decimal",
    "duration",
}

REQUIRED_FEATURES = {
    "entity",
    "relation",
    "role-player",
    "three-or-more-roles",
    "multi-player-role",
    "abstract-role-player",
    "cardinality",
    "key",
    "unique",
    "optional",
    "inherited",
    "multi-value",
}


def load_json(name: str) -> Any:
    return json.loads((FIXTURES_DIR / name).read_text(encoding="utf-8"))


def load_fixture_contract() -> dict[str, Any]:
    return {
        "metadata": load_json("metadata.json"),
        "descriptors": load_json("descriptors.json"),
        "write_data": load_json("write-data.json"),
        "expected": load_json("expected-canonical.json"),
        "schema": (FIXTURES_DIR / "schema.tql").read_text(encoding="utf-8"),
    }


def validate_fixture_contract(contract: dict[str, Any]) -> None:
    metadata = contract["metadata"]
    expected = contract["expected"]
    write_data = contract["write_data"]
    descriptors = contract["descriptors"]
    schema = contract["schema"]

    primitives = set(metadata["mvp_primitives"])
    missing_primitives = MVP_PRIMITIVES - primitives
    assert not missing_primitives, f"missing primitive coverage: {sorted(missing_primitives)}"

    features = set(metadata["features"])
    missing_features = REQUIRED_FEATURES - features
    assert not missing_features, f"missing feature coverage: {sorted(missing_features)}"

    assert metadata["fixture_id"] == expected["fixture_id"] == write_data["fixture_id"]
    assert metadata["version"] == expected["version"] == write_data["version"]

    descriptor_types = {
        descriptor["type_name"]
        for section in ("entities", "relations")
        for descriptor in descriptors[section]
    }
    expected_types = {
        row["type"] for section in ("entities", "relations") for row in expected[section]
    }
    assert expected_types <= descriptor_types

    stable_ids = {
        row["stable_id"] for section in ("entities", "relations") for row in expected[section]
    }
    assert len(stable_ids) == sum(len(expected[section]) for section in ("entities", "relations"))

    entity_ids = {row["stable_id"] for row in expected["entities"]}
    for relation in expected["relations"]:
        for players in relation["roles"].values():
            for player in players:
                assert player["stable_id"] in entity_ids

    primitive_values = {
        value["type"]
        for row in expected["entities"]
        for value in row["attributes"].values()
        if isinstance(value, dict) and "type" in value
    }
    assert MVP_PRIMITIVES <= primitive_values

    for label in metadata["required_schema_labels"]:
        assert label in schema


def canonical_json(value: Any) -> str:
    """Return stable JSON for snapshot comparisons."""
    return json.dumps(value, indent=2, sort_keys=True, separators=(",", ": ")) + "\n"


def normalize_descriptor_snapshot(snapshot: dict[str, Any]) -> dict[str, Any]:
    """Return the canonical descriptor snapshot shape for parity checks.

    Python model metadata encodes scalar fields as implicit ``Card(0, 1)`` or
    ``Card(1, 1)`` annotations. The shared parity descriptor records scalar
    optionality with ``is_optional`` only, so this helper removes only those
    implicit scalar-card annotations. Explicit multi-value cardinalities remain
    part of the descriptor.
    """
    return {
        "version": snapshot["version"],
        "entities": sorted(
            (_normalize_type_descriptor(descriptor) for descriptor in snapshot["entities"]),
            key=lambda descriptor: descriptor["type_name"],
        ),
        "relations": sorted(
            (_normalize_type_descriptor(descriptor) for descriptor in snapshot["relations"]),
            key=lambda descriptor: descriptor["type_name"],
        ),
    }


def _normalize_type_descriptor(descriptor: dict[str, Any]) -> dict[str, Any]:
    normalized = dict(descriptor)
    normalized["owned_attributes"] = sorted(
        (
            _normalize_attribute_descriptor(attribute)
            for attribute in descriptor["owned_attributes"]
        ),
        key=lambda attribute: attribute["field_name"],
    )
    if "roles" in descriptor:
        normalized["roles"] = sorted(
            (_normalize_role_descriptor(role) for role in descriptor["roles"]),
            key=lambda role: role["role_name"],
        )
    return normalized


def _normalize_attribute_descriptor(attribute: dict[str, Any]) -> dict[str, Any]:
    normalized = dict(attribute)
    normalized["annotations"] = _normalize_annotations(
        attribute["annotations"],
        is_optional=attribute["is_optional"],
    )
    return normalized


def _normalize_role_descriptor(role: dict[str, Any]) -> dict[str, Any]:
    normalized = dict(role)
    normalized["player_type_names"] = sorted(role["player_type_names"])
    return normalized


def _normalize_annotations(
    annotations: list[Any],
    *,
    is_optional: bool,
) -> list[Any]:
    normalized = []
    for annotation in annotations:
        if annotation == {"Card": [0, 1]} and is_optional:
            continue
        if annotation == {"Card": [1, 1]}:
            continue
        normalized.append(annotation)
    return sorted(normalized, key=lambda annotation: json.dumps(annotation, sort_keys=True))
