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


class RustSeniority(String):
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
            "overrides": None,
            "is_abstract": False,
        },
        {
            "role_name": "employer",
            "player_type_names": ["rust-company"],
            "cardinality": [1, 1],
            "overrides": None,
            "is_abstract": False,
        },
    ]


def test_subtype_relation_descriptor_flattens_inherited_roles() -> None:
    """A subtype relation flattens plain-inherited roles into its own descriptor.

    Plain-inherited roles are in the effective set — they are playable on subtype
    instances.  The parent relation is unchanged.
    """
    descriptor = descriptor_for_model(RustCollaboration)

    assert descriptor["type_name"] == "rust-collaboration"
    assert descriptor["parent_type"] == "rust-interaction"
    # Inherited role is flattened into the child's effective set.
    assert descriptor["roles"] == [
        {
            "role_name": "participant",
            "player_type_names": ["rust-person"],
            "cardinality": None,
            "overrides": None,
            "is_abstract": False,
        }
    ]

    # The parent's own descriptor is unchanged.
    parent = descriptor_for_model(RustInteraction)
    assert parent["roles"] == [
        {
            "role_name": "participant",
            "player_type_names": ["rust-person"],
            "cardinality": None,
            "overrides": None,
            "is_abstract": False,
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


# ---------------------------------------------------------------------------
# Effective-set role descriptor tests (contribution / authoring pair)
# ---------------------------------------------------------------------------


class RustWork(Entity):
    flags = TypeFlags(name="rust-work")

    name: RustName = Flag(Key)


class RustAuthor(Entity):
    flags = TypeFlags(name="rust-author")

    name: RustName = Flag(Key)


class RustContribution(Relation):
    """Root relation: two plain roles, no specialization."""

    flags = TypeFlags(name="rust-contribution")

    contributor: Role[RustPerson] = Role("contributor", RustPerson)
    work: Role[RustWork] = Role("work", RustWork)


class RustAuthoring(RustContribution):
    """Subtype: specializes 'contributor' as 'author'; 'work' is plain-inherited."""

    flags = TypeFlags(name="rust-authoring")

    author: Role[RustAuthor] = Role("author", RustAuthor, overrides="contributor")


def test_relation_subtype_effective_roles_ordered() -> None:
    """Relation subtype descriptor emits the canonical effective role set.

    Expected: plain-inherited parent roles first (in parent order, excluding
    overridden ones), then child specializing roles.  For the contribution/authoring
    pair this is [work, author]: 'contributor' is excluded because 'author' overrides
    it, and 'work' is plain-inherited.
    """
    descriptor = descriptor_for_model(RustAuthoring)

    assert descriptor["type_name"] == "rust-authoring"
    assert descriptor["parent_type"] == "rust-contribution"
    role_names = [r["role_name"] for r in descriptor["roles"]]
    assert role_names == ["work", "author"], f"unexpected role order: {role_names}"

    # 'work' is the plain-inherited role; it carries no overrides marker.
    work_role = next(r for r in descriptor["roles"] if r["role_name"] == "work")
    assert work_role["player_type_names"] == ["rust-work"]
    assert work_role["cardinality"] is None
    assert work_role["overrides"] is None
    assert work_role["is_abstract"] is False

    # 'author' is the specializing role; it records which parent role it overrides.
    author_role = next(r for r in descriptor["roles"] if r["role_name"] == "author")
    assert author_role["player_type_names"] == ["rust-author"]
    assert author_role["cardinality"] is None
    assert author_role["overrides"] == "contributor"
    assert author_role["is_abstract"] is False


def test_unset_inherited_role_reads_none_and_stays_out_of_inputs() -> None:
    """An unset inherited role must read as None, not the class-level RoleRef.

    Subclass model building captures the parent descriptor's class-level access
    (a RoleRef) as the field default; validation must normalize it away so the
    role-player input builder skips the role instead of treating the sentinel
    as a player.
    """
    from type_bridge._rust_runtime import role_player_inputs

    authoring = RustAuthoring(author=RustAuthor(name=RustName("a")))
    assert authoring.work is None

    role_names = [item["role_name"] for item in role_player_inputs(authoring)]
    assert role_names == ["author"]

    both = RustAuthoring(
        author=RustAuthor(name=RustName("a")),
        work=RustWork(name=RustName("w")),
    )
    role_names = [item["role_name"] for item in role_player_inputs(both)]
    assert role_names == ["work", "author"]


def test_parent_relation_descriptor_unchanged() -> None:
    """The parent relation's own descriptor is not affected by subtype specialization."""
    descriptor = descriptor_for_model(RustContribution)

    assert descriptor["type_name"] == "rust-contribution"
    assert descriptor["parent_type"] is None
    role_names = [r["role_name"] for r in descriptor["roles"]]
    assert role_names == ["contributor", "work"]

    # Plain roles on the root relation carry no overrides / abstract markers.
    for role in descriptor["roles"]:
        assert role["overrides"] is None, f"{role['role_name']} should not override anything"
        assert role["is_abstract"] is False, f"{role['role_name']} should not be abstract"


def test_entity_subtype_owned_attributes_regression() -> None:
    """Entity subtypes still flatten owned_attributes; relation changes must not regress this."""

    class RustEmployee(RustPerson):
        flags = TypeFlags(name="rust-employee")
        seniority: RustSeniority | None = None

    descriptor = descriptor_for_model(RustEmployee)
    field_names = [a["field_name"] for a in descriptor["owned_attributes"]]
    # Inherited attrs from RustPerson are re-listed in the child descriptor,
    # followed by the child-local attr.
    assert "name" in field_names
    assert "score" in field_names
    assert "seniority" in field_names
    assert field_names.index("name") < field_names.index("seniority")


# ---------------------------------------------------------------------------
# Abstract-role descriptor tests (Phase 3)
# ---------------------------------------------------------------------------


class RustAbstractRoleBase(Relation):
    """Relation with an abstract role at its declaring scope."""

    flags = TypeFlags(name="rust-abstract-role-base")

    participant: Role[RustPerson] = Role("participant", RustPerson, abstract=True)


class RustAbstractRoleChild(RustAbstractRoleBase):
    """Child that plain-inherits the abstract parent role."""

    flags = TypeFlags(name="rust-abstract-role-child")


class RustAbstractRoleOverride(RustAbstractRoleBase):
    """Child that overrides the abstract parent role with a specializing role."""

    flags = TypeFlags(name="rust-abstract-role-override")

    actor: Role[RustPerson] = Role("actor", RustPerson, overrides="participant")


def test_abstract_role_descriptor_is_abstract_true() -> None:
    """A Role declared with abstract=True must carry is_abstract=True in the descriptor."""
    descriptor = descriptor_for_model(RustAbstractRoleBase)

    assert descriptor["type_name"] == "rust-abstract-role-base"
    assert len(descriptor["roles"]) == 1
    participant = descriptor["roles"][0]
    assert participant["role_name"] == "participant"
    assert participant["is_abstract"] is True


def test_abstract_role_plain_inherited_carries_flag() -> None:
    """A plain-inherited abstract role keeps is_abstract=True in the child descriptor."""
    descriptor = descriptor_for_model(RustAbstractRoleChild)

    participant = next(r for r in descriptor["roles"] if r["role_name"] == "participant")
    assert participant["is_abstract"] is True


def test_abstract_role_inputs_raises_at_declaring_scope() -> None:
    """Building role-player inputs for an abstract role at its declaring scope raises ValueError.

    The engine rejects direct players for an abstract role on the declaring
    relation; the Python layer mirrors this at input-build time.
    """
    from type_bridge._rust_runtime import role_player_inputs

    instance = RustAbstractRoleBase(
        participant=RustPerson(
            name=RustName("X"),
            score=RustScore(1.0),
            active=RustActive(True),
            birth_date=RustBirthDate(date(2000, 1, 1)),
            created_at=RustCreatedAt(datetime(2000, 1, 1)),
            seen_at=RustSeenAt(datetime(2000, 1, 1, tzinfo=UTC)),
            balance=RustBalance(DecimalType("1.0")),
        )
    )
    with pytest.raises(ValueError, match="abstract"):
        role_player_inputs(instance)


def test_abstract_role_inputs_ok_on_plain_inherited_child() -> None:
    """Building role-player inputs for a plain-inherited abstract role on a child does NOT raise.

    The engine accepts players for an abstract role that is plain-inherited on a
    sub-relation — only the declaring scope is blocked.
    """
    from type_bridge._rust_runtime import role_player_inputs

    person = RustPerson(
        name=RustName("Y"),
        score=RustScore(1.0),
        active=RustActive(True),
        birth_date=RustBirthDate(date(2000, 1, 1)),
        created_at=RustCreatedAt(datetime(2000, 1, 1)),
        seen_at=RustSeenAt(datetime(2000, 1, 1, tzinfo=UTC)),
        balance=RustBalance(DecimalType("1.0")),
    )
    person._iid = "0x123"  # Provide a fake IID so key lookup is skipped.

    instance = RustAbstractRoleChild(participant=person)
    # Must NOT raise — plain-inherited abstract role on a subtype is playable.
    inputs = role_player_inputs(instance)
    assert len(inputs) == 1
    assert inputs[0]["role_name"] == "participant"
