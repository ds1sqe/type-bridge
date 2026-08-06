"""Executable audit linking removed handwritten tests to retained outcomes."""

from __future__ import annotations

import json
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any

ROOT = Path(__file__).parents[3]
MAPPING = ROOT / "tests/fixtures/handwritten-operation-removal-map.json"
ALLOWED_DISPOSITIONS = {"generated_successor", "retained_contract", "retained_engine"}
BINDINGS = {"python", "node", "rust"}


def _load(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def test_every_handwritten_family_has_an_exact_retained_disposition() -> None:
    mapping = _load(MAPPING)
    assert mapping["format"] == "typebridge.handwritten-operation-removal-map/v2"
    assert set(mapping["scope"]) == BINDINGS
    assert set(mapping["cutover_bindings"]) <= BINDINGS

    generated = _load(ROOT / mapping["generated_inventory"])
    generated_operations = {operation["id"]: operation for operation in generated["operations"]}
    retained_contracts = set(mapping["retained_contracts"])
    families = mapping["families"]
    assert len({family["id"] for family in families}) == len(families)

    inventory_by_family: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for item in mapping["case_inventory"]:
        inventory_by_family[item["family_id"]].append(item)

    for family in families:
        binding = family["binding"]
        disposition = family["disposition"]
        assert binding in BINDINGS, family["id"]
        assert disposition in ALLOWED_DISPOSITIONS, family["id"]
        assert inventory_by_family[family["id"]], family["id"]

        if disposition == "generated_successor":
            assert "retained_contract_ids" not in family, family["id"]
            for operation_id in family["successor_operations"]:
                operation = generated_operations[operation_id]
                required = set(operation.get("required_bindings", BINDINGS))
                assert binding in required, (family["id"], operation_id, binding)
                assert operation["status"] in {
                    "accepted_live",
                    "accepted_offline",
                    "uniform_unsupported",
                }
        else:
            assert "successor_operations" not in family, family["id"]
            assert set(family["retained_contract_ids"]) <= retained_contracts


def test_cut_over_handwritten_sources_are_absent_but_their_audit_is_frozen() -> None:
    mapping = _load(MAPPING)
    families = {family["id"]: family for family in mapping["families"]}
    cutover_bindings = set(mapping["cutover_bindings"])
    actual_counts: Counter[str] = Counter()
    identities: set[tuple[str, str]] = set()

    for item in mapping["case_inventory"]:
        family = families[item["family_id"]]
        assert item["binding"] == family["binding"]
        assert len(item["source_sha256"]) == 64
        assert item["test_cases"]
        actual_counts[item["binding"]] += len(item["test_cases"])
        for case in item["test_cases"]:
            identity = (item["path"], case)
            assert identity not in identities, identity
            identities.add(identity)

        path = ROOT / item["path"]
        if item["binding"] in cutover_bindings and family["disposition"] == "generated_successor":
            assert not path.exists(), f"retired handwritten evidence still active: {path}"
        elif family["disposition"] != "generated_successor":
            assert path.is_file(), f"retained contract evidence is absent: {path}"

    for family in families.values():
        if family["binding"] in cutover_bindings and family["disposition"] == "generated_successor":
            for source in family.get("supporting_sources", []):
                assert not (ROOT / source).exists(), (
                    f"retired handwritten parity support still active: {source}"
                )

    assert dict(actual_counts) == mapping["expected_case_counts"]


def test_removal_map_never_uses_a_private_authoring_successor() -> None:
    mapping = _load(MAPPING)
    serialized = json.dumps(
        {
            "retained_contracts": mapping["retained_contracts"],
            "family_outcomes": [
                {
                    "id": family["id"],
                    "disposition": family["disposition"],
                    "successor_operations": family.get("successor_operations", []),
                    "retained_contract_ids": family.get("retained_contract_ids", []),
                }
                for family in mapping["families"]
            ],
        },
        sort_keys=True,
    )
    assert "_legacy" not in serialized
    assert "TypeDBType -> object" not in serialized
