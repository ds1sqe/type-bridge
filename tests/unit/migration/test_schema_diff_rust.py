from __future__ import annotations

# pyright: reportMissingImports=false
import pytest

from type_bridge import Card, Entity, Integer, Relation, Role, String, TypeFlags
from type_bridge._rust_runtime import (
    compute_schema_diff,
    generate_define_block,
    schema_diff_is_breaking,
)
from type_bridge.attribute import AttributeFlags
from type_bridge.migration import author_migration
from type_bridge.migration.info import SchemaInfo
from type_bridge.migration.introspection import (
    IntrospectedAttribute,
    IntrospectedEntity,
    IntrospectedOwnership,
    IntrospectedSchema,
)


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


class DiffEmail(String):
    flags = AttributeFlags(name="diff-email")
    regex_pattern = r"^[a-z]+@[a-z]+\.[a-z]+$"


class DiffStatus(String):
    flags = AttributeFlags(name="diff-status")
    independent = True
    allowed_values = ("active", "inactive")


class DiffAge(Integer):
    flags = AttributeFlags(name="diff-age")
    range_constraint = ("0", "150")


class DiffEmailPlain(String):
    flags = AttributeFlags(name="diff-email-change")


class DiffEmailConstrained(String):
    flags = AttributeFlags(name="diff-email-change")
    regex_pattern = r"^[a-z]+$"


class DiffAnnotatedPerson(Entity):
    flags = TypeFlags(name="diff-annotated-person")

    email: DiffEmail
    status: DiffStatus
    age: DiffAge


class DiffEmailChangePersonV1(Entity):
    flags = TypeFlags(name="diff-email-change-person")

    email: DiffEmailPlain


class DiffEmailChangePersonV2(Entity):
    flags = TypeFlags(name="diff-email-change-person")

    email: DiffEmailConstrained


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


def test_python_schema_info_preserves_attribute_type_annotations_in_rust_emit() -> None:
    pytest.importorskip("type_bridge_core")

    schema = SchemaInfo()
    schema.entities = [DiffAnnotatedPerson]

    typeql = generate_define_block(schema.to_rust_schema_info())

    assert 'attribute diff-email, value string @regex("^[a-z]+@[a-z]+\\.[a-z]+$");' in typeql
    assert (
        'attribute diff-status @independent, value string @values("active", "inactive");' in typeql
    )
    assert "attribute diff-age, value integer @range(0..150);" in typeql


def test_schema_diff_exposes_attribute_type_annotation_changes() -> None:
    pytest.importorskip("type_bridge_core")

    current = SchemaInfo()
    current.entities = [DiffEmailChangePersonV1]
    current.attribute_classes = {DiffEmailPlain}

    target = SchemaInfo()
    target.entities = [DiffEmailChangePersonV2]
    target.attribute_classes = {DiffEmailConstrained}

    rust_diff = compute_schema_diff(current.to_rust_schema_info(), target.to_rust_schema_info())

    assert "diff-email-change" in rust_diff["modified_attributes"]
    assert rust_diff["modified_attributes"]["diff-email-change"]["regex_changed"][1] == (
        r"^[a-z]+$"
    )
    assert schema_diff_is_breaking(rust_diff) is True

    public_diff = current.compare(target)
    assert DiffEmailConstrained in public_diff.modified_attributes
    assert "regex" in public_diff.summary()


def test_autogenerate_emits_redefine_for_attribute_type_annotation_changes() -> None:
    pytest.importorskip("type_bridge_core")

    db_schema = IntrospectedSchema(
        entities={"diff-email-change-person": IntrospectedEntity(name="diff-email-change-person")},
        attributes={
            "diff-email-change": IntrospectedAttribute(
                name="diff-email-change",
                value_type="string",
            )
        },
        ownerships=[
            IntrospectedOwnership(
                owner_name="diff-email-change-person",
                attribute_name="diff-email-change",
            )
        ],
    )
    target = SchemaInfo()
    target.entities = [DiffEmailChangePersonV2]
    target.attribute_classes = {DiffEmailConstrained}

    authored = author_migration(
        db_schema.to_rust_schema_info(),
        target.to_rust_schema_info(),
        app_label="migrations",
        name="0002_constrain",
        snapshot_version="v0002",
        generated_at="t",
    )

    assert authored is not None
    define_ops = [
        operation
        for operation in authored.spec["operations"]
        if operation["kind"] == "run_typeql" and operation["forward"].startswith("define\n")
    ]
    assert len(define_ops) == 1
    assert define_ops[0]["forward"] == (
        'define\nattribute diff-email-change, value string @regex("^[a-z]+$");'
    )


def test_introspected_schema_round_trips_rust_annotations() -> None:
    rust_info = {
        "entities": {
            "diff-person": {
                "type_name": "diff-person",
                "is_abstract": True,
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
                "is_abstract": True,
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
            "diff-name": {
                "attr_name": "diff-name",
                "value_type": "string",
                "regex": r"^[a-z]+$",
                "allowed_values": ["active", "inactive"],
                "range": ["0", "150"],
                "is_independent": True,
            },
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
    assert schema.to_rust_schema_info()["entities"]["diff-person"]["is_abstract"] is True
    assert schema.to_rust_schema_info()["relations"]["diff-engagement"]["is_abstract"] is True
    assert schema.to_rust_schema_info()["relations"]["diff-engagement"]["roles"] == [
        {
            "role_name": "participant",
            "player_type_names": ["diff-person"],
            "cardinality": (1, 1),
        }
    ]
    assert schema.to_rust_schema_info()["attributes"]["diff-name"] == {
        "attr_name": "diff-name",
        "value_type": "string",
        "regex": r"^[a-z]+$",
        "allowed_values": ["active", "inactive"],
        "range": ["0", "150"],
        "is_independent": True,
    }
