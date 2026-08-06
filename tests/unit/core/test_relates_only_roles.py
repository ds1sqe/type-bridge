from __future__ import annotations

from typing import assert_type

import pytest

from tests.utils.handwritten import Card, Entity, Relation, Role, RoleRef, TypeFlags
from type_bridge._rust_runtime import generate_define_block
from type_bridge.migration._lower import _schema_info_for_models


class RelatesOnlyPerson(Entity):
    flags = TypeFlags(name="relates-only-person")


class RelatesOnlyRelation(Relation):
    flags = TypeFlags(name="relates-only-rel")

    definition: Role
    allowed_value: Role = Role("allowed_value")
    actor: Role[RelatesOnlyPerson] = Role("actor", RelatesOnlyPerson)


def test_bare_and_explicit_relates_only_roles_register_without_players() -> None:
    roles = RelatesOnlyRelation.get_roles()

    assert set(roles) == {"definition", "allowed_value", "actor"}
    assert roles["definition"].role_name == "definition"
    assert roles["allowed_value"].role_name == "allowed_value"
    assert roles["definition"].player_types == ()
    assert roles["allowed_value"].player_types == ()
    assert roles["actor"].player_types == ("relates-only-person",)


def test_relates_only_roles_emit_empty_player_type_names() -> None:
    roles = _schema_info_for_models([RelatesOnlyPerson, RelatesOnlyRelation])["relations"][
        "relates-only-rel"
    ]["roles"]

    base = {
        "cardinality": None,
        "overrides": None,
        "is_abstract": False,
        "ordered": False,
        "distinct": False,
    }
    assert roles == [
        {"role_name": "definition", "player_type_names": [], **base},
        {"role_name": "allowed_value", "player_type_names": [], **base},
        {"role_name": "actor", "player_type_names": ["relates-only-person"], **base},
    ]


def test_bound_role_class_access_type_stays_precise() -> None:
    assert_type(
        RelatesOnlyRelation.actor,
        RoleRef[RelatesOnlyPerson, RelatesOnlyRelation],
    )


def test_relates_only_instance_access_returns_none_and_assignment_is_descriptive() -> None:
    person = RelatesOnlyPerson()
    relation = RelatesOnlyRelation.model_validate({"actor": person})

    assert relation.definition is None
    with pytest.raises(TypeError, match="relates-only; it declares no player"):
        relation.definition = person


class PlaysCardCompany(Entity):
    flags = TypeFlags(name="plays-card-company")


class PlaysCardEmployment(Relation):
    flags = TypeFlags(name="plays-card-employment")

    employer: Role[PlaysCardCompany] = Role(
        "employer", PlaysCardCompany, plays_cardinality=Card(0, 1)
    )


def test_role_surfaces_plays_cardinality() -> None:
    role = PlaysCardEmployment.get_roles()["employer"]

    assert role.plays_cardinality is not None
    assert (role.plays_cardinality.min, role.plays_cardinality.max) == (0, 1)
    # plays-side and relates-side cardinality are independent; only plays was set.
    assert role.cardinality is None


def test_plays_cardinality_preserves_bound_role_type() -> None:
    # plays_cardinality is a runtime-only schema constraint; class-level access
    # must keep the precise RoleRef[player] generic, not widen to Any.
    assert_type(
        PlaysCardEmployment.employer,
        RoleRef[PlaysCardCompany, PlaysCardEmployment],
    )


def test_plays_cardinality_on_relates_only_role_is_rejected() -> None:
    with pytest.raises(TypeError, match="no player type"):
        Role("orphan", plays_cardinality=Card(0, 1))


def test_schema_to_typeql_mixes_relates_only_and_bound_roles() -> None:
    pytest.importorskip("type_bridge_core")
    typeql = generate_define_block(
        _schema_info_for_models([RelatesOnlyPerson, RelatesOnlyRelation])
    )

    assert "    relates definition," in typeql
    assert "    relates allowed_value," in typeql
    assert "relates-only-person plays relates-only-rel:actor;" in typeql
    assert "plays relates-only-rel:definition" not in typeql
    assert "plays relates-only-rel:allowed_value" not in typeql
