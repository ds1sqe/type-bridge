from __future__ import annotations

# pyright: reportMissingImports=false
from datetime import UTC, date, datetime
from decimal import Decimal as DecimalType

import pytest

from type_bridge import (
    Boolean,
    Card,
    Date,
    DateTime,
    DateTimeTZ,
    Decimal,
    Double,
    Duration,
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    Role,
    String,
    TypeFlags,
    Unique,
)
from type_bridge._rust_runtime import (
    descriptor_for_model,
    normalize_attributes,
    register_model_descriptor,
)


class RustName(String):
    flags = None


class RustAge(Integer):
    pass


class RustScore(Double):
    pass


class RustActive(Boolean):
    pass


class RustBirthDate(Date):
    pass


class RustCreatedAt(DateTime):
    pass


class RustSeenAt(DateTimeTZ):
    pass


class RustBalance(Decimal):
    pass


class RustSpan(Duration):
    pass


class RustPerson(Entity):
    flags = TypeFlags(name="rust-person")

    name: RustName = Flag(Key)
    age: RustAge | None = None
    score: RustScore
    active: RustActive
    birth_date: RustBirthDate
    created_at: RustCreatedAt
    seen_at: RustSeenAt
    balance: RustBalance
    spans: list[RustSpan] = Flag(Card(min=0))


class RustPersonProjection(Entity):
    flags = TypeFlags(name="rust-person")

    name: RustName = Flag(Key)


class RustCompany(Entity):
    flags = TypeFlags(name="rust-company")

    name: RustName = Flag(Unique)


class RustEmployment(Relation):
    flags = TypeFlags(name="rust-employment")

    employee: Role[RustPerson] = Role("employee", RustPerson)
    employer: Role[RustCompany] = Role("employer", RustCompany, cardinality=Card(1, 1))
    score: RustScore | None = None


class RustInteraction(Relation):
    flags = TypeFlags(name="rust-interaction", abstract=True)

    participant: Role[RustPerson] = Role("participant", RustPerson)


class RustCollaboration(RustInteraction):
    """Subtype relation that declares no own roles, only an extra attribute."""

    flags = TypeFlags(name="rust-collaboration", abstract=True)

    score: RustScore | None = None


def test_entity_descriptor_translates_all_value_types() -> None:
    descriptor = descriptor_for_model(RustPerson)

    assert descriptor["type_name"] == "rust-person"
    values = {attr["field_name"]: attr["value_type"] for attr in descriptor["owned_attributes"]}
    assert values == {
        "name": "string",
        "age": "long",
        "score": "double",
        "active": "boolean",
        "birth_date": "date",
        "created_at": "datetime",
        "seen_at": "datetime-tz",
        "balance": "decimal",
        "spans": "duration",
    }

    name = next(attr for attr in descriptor["owned_attributes"] if attr["field_name"] == "name")
    assert name["annotations"] == ["Key"]
    age = next(attr for attr in descriptor["owned_attributes"] if attr["field_name"] == "age")
    assert age["is_optional"] is True
    assert age["annotations"] == [{"Card": [0, 1]}]


def test_relation_descriptor_translates_roles() -> None:
    descriptor = descriptor_for_model(RustEmployment)

    assert descriptor["type_name"] == "rust-employment"
    assert descriptor["roles"] == [
        {
            "role_name": "employee",
            "player_type_names": ["rust-person"],
            "cardinality": None,
        },
        {
            "role_name": "employer",
            "player_type_names": ["rust-company"],
            "cardinality": [1, 1],
        },
    ]


def test_subtype_relation_descriptor_excludes_inherited_roles() -> None:
    """A subtype relation with no own roles must not carry inherited roles.

    An inherited role belongs to the declaring relation; stamping it onto the
    subtype descriptor would scope a player's ``plays`` edge to the subtype
    (``plays rust-collaboration:participant``), which TypeDB rejects.
    """
    descriptor = descriptor_for_model(RustCollaboration)

    assert descriptor["type_name"] == "rust-collaboration"
    assert descriptor["parent_type"] == "rust-interaction"
    assert descriptor["roles"] == []

    # The declaring relation still carries the role.
    parent = descriptor_for_model(RustInteraction)
    assert parent["roles"] == [
        {
            "role_name": "participant",
            "player_type_names": ["rust-person"],
            "cardinality": None,
        }
    ]


def test_normalize_attributes_converts_python_values() -> None:
    instance = RustPerson(
        name=RustName("Alice"),
        score=RustScore(9.5),
        active=RustActive(True),
        birth_date=RustBirthDate(date(2000, 1, 2)),
        created_at=RustCreatedAt(datetime(2026, 5, 21, 8, 30, 0)),
        seen_at=RustSeenAt(datetime(2026, 5, 21, 8, 30, 0, tzinfo=UTC)),
        balance=RustBalance(DecimalType("12.30")),
        spans=[RustSpan("P1D")],
    )

    normalized = normalize_attributes(RustPerson, instance.to_dict())

    assert normalized["birth_date"] == "2000-01-02"
    assert normalized["created_at"] == "2026-05-21T08:30:00"
    assert normalized["seen_at"] == "2026-05-21T08:30:00+00:00"
    assert normalized["balance"] == "12.30"
    assert normalized["spans"] == ["P1D"]


def test_pyo3_registry_roundtrip_when_available() -> None:
    pytest.importorskip("type_bridge_core")

    registered = register_model_descriptor(RustPerson)

    assert registered["type_name"] == "rust-person"
    assert registered["owned_attributes"][0]["field_name"] == "name"


def test_register_model_descriptor_allows_same_label_projection() -> None:
    pytest.importorskip("type_bridge_core")

    register_model_descriptor(RustPerson)
    projected = register_model_descriptor(RustPersonProjection)

    assert projected["type_name"] == "rust-person"
    assert [attr["field_name"] for attr in projected["owned_attributes"]] == ["name"]
