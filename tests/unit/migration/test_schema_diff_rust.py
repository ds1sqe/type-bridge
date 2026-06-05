from __future__ import annotations

# pyright: reportMissingImports=false
import pytest

from type_bridge import Card, Entity, Relation, Role, String, TypeFlags
from type_bridge._rust_runtime import (
    compute_schema_diff,
    generate_define_block,
    schema_diff_is_breaking,
)
from type_bridge.attribute import AttributeFlags
from type_bridge.migration.info import SchemaInfo
from type_bridge.migration.introspection import IntrospectedSchema


class DiffName(String):
    flags = AttributeFlags(name="diff-name")


class DiffPersonV1(Entity):
    flags = TypeFlags(name="diff-person")

    name: DiffName


class DiffCompanyV1(Entity):
    flags = TypeFlags(name="diff-company")

    name: DiffName


class DiffContractorV2(Entity):
    flags = TypeFlags(name="diff-contractor")

    name: DiffName


class DiffEngagementV1(Relation):
    flags = TypeFlags(name="diff-engagement")

    participant: Role[DiffPersonV1 | DiffCompanyV1] = Role(
        "participant",
        DiffPersonV1,
        DiffCompanyV1,
        cardinality=Card(0),
    )


class DiffEngagementV2(Relation):
    flags = TypeFlags(name="diff-engagement")

    participant: Role[DiffPersonV1 | DiffContractorV2] = Role(
        "participant",
        DiffPersonV1,
        DiffContractorV2,
        cardinality=Card(1, 1),
    )


def test_rust_schema_diff_exposes_role_players_and_cardinality() -> None:
    pytest.importorskip("type_bridge_core")

    current = SchemaInfo()
    current.entities = [DiffPersonV1, DiffCompanyV1]
    current.relations = [DiffEngagementV1]
    current.attribute_classes = {DiffName}

    target = SchemaInfo()
    target.entities = [DiffPersonV1, DiffContractorV2]
    target.relations = [DiffEngagementV2]
    target.attribute_classes = {DiffName}

    diff = compute_schema_diff(current.to_rust_schema_info(), target.to_rust_schema_info())

    assert diff["added_entities"] == ["diff-contractor"]
    assert diff["removed_entities"] == ["diff-company"]
    relation_changes = diff["modified_relations"]["diff-engagement"]
    assert relation_changes["modified_role_players"] == [
        {
            "role_name": "participant",
            "added_player_types": ["diff-contractor"],
            "removed_player_types": ["diff-company"],
        }
    ]
    assert relation_changes["modified_role_cardinality"] == [
        {
            "role_name": "participant",
            "old_cardinality": (0, None),
            "new_cardinality": (1, 1),
        }
    ]
    assert schema_diff_is_breaking(diff) is True

    typeql = generate_define_block(target.to_rust_schema_info())
    assert "relates participant @card(1..1)" in typeql
    assert "diff-contractor plays diff-engagement:participant;" in typeql


def test_introspected_schema_round_trips_rust_annotations() -> None:
    rust_info = {
        "entities": {
            "diff-person": {
                "type_name": "diff-person",
                "is_abstract": False,
                "parent_type": None,
                "owned_attributes": [
                    {
                        "attr_name": "diff-name",
                        "value_type": "string",
                        "annotations": ["Key"],
                    }
                ],
            }
        },
        "relations": {
            "diff-engagement": {
                "type_name": "diff-engagement",
                "is_abstract": False,
                "parent_type": "diff-parent-relation",
                "owned_attributes": [],
                "roles": [
                    {
                        "role_name": "participant",
                        "player_type_names": ["diff-person"],
                        "cardinality": (1, 1),
                    }
                ],
            }
        },
        "attributes": {
            "diff-name": {"attr_name": "diff-name", "value_type": "string"},
        },
    }

    schema = IntrospectedSchema.from_rust_schema_info(rust_info)

    assert schema.to_rust_schema_info()["entities"]["diff-person"]["owned_attributes"] == [
        {
            "attr_name": "diff-name",
            "value_type": "string",
            "annotations": ["Key"],
        }
    ]
    assert schema.to_rust_schema_info()["relations"]["diff-engagement"]["parent_type"] == (
        "diff-parent-relation"
    )
    assert schema.to_rust_schema_info()["relations"]["diff-engagement"]["roles"] == [
        {
            "role_name": "participant",
            "player_type_names": ["diff-person"],
            "cardinality": (1, 1),
        }
    ]
