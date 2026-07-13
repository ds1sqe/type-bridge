"""Whole-relation removal mapping regression tests (#168).

A relation present in the base schema and absent from the target must map to
exactly one ``RemoveRelation``: TypeDB cascades declared roles, player
capabilities, and ownerships inside the same schema transaction, and rejects
any intermediate schema where a concrete relation relates zero roles.
Granular role/player/ownership removals remain reserved for relations that
survive in the target schema.
"""

from __future__ import annotations

# pyright: reportMissingImports=false
import pytest

from type_bridge import Entity, Relation, Role, String, TypeFlags
from type_bridge.attribute import AttributeFlags
from type_bridge.migration import operations as ops
from type_bridge.migration import ref
from type_bridge.migration.executor import _normalized_operations
from type_bridge.migration.generator import MigrationGenerator
from type_bridge.migration.info import SchemaInfo
from type_bridge.migration.introspection import (
    IntrospectedAttribute,
    IntrospectedEntity,
    IntrospectedOwnership,
    IntrospectedRelation,
    IntrospectedRole,
    IntrospectedSchema,
)


class RrLinkId(String):
    flags = AttributeFlags(name="rr-link-id")


class RrNote(String):
    flags = AttributeFlags(name="rr-note")


class RrPerson(Entity):
    flags = TypeFlags(name="rr-person")


class RrBadge(Entity):
    flags = TypeFlags(name="rr-badge")


class RrEmploySlim(Relation):
    """Target shape of `rr-employ` after its `reviewer` role is removed."""

    flags = TypeFlags(name="rr-employ")

    employee: Role[RrPerson] = Role("employee", RrPerson)


class RrAssignSlim(Relation):
    """Target shape of `rr-assign` after `rr-badge` stops playing `worker`."""

    flags = TypeFlags(name="rr-assign")

    worker: Role[RrPerson] = Role("worker", RrPerson)


class RrOwnSlim(Relation):
    """Target shape of `rr-own` after its `rr-note` ownership is removed."""

    flags = TypeFlags(name="rr-own")

    holder: Role[RrPerson] = Role("holder", RrPerson)


@pytest.fixture(autouse=True)
def _requires_rust_extension() -> None:
    pytest.importorskip("type_bridge_core")


def _link_relation(*, abstract: bool = False) -> IntrospectedRelation:
    return IntrospectedRelation(
        name="rr-link",
        roles={
            "subject": IntrospectedRole(name="subject", player_types=["rr-person"]),
            "badge": IntrospectedRole(name="badge", player_types=["rr-badge"]),
        },
        is_abstract=abstract,
    )


def _base_with_link(*, abstract: bool = False) -> IntrospectedSchema:
    return IntrospectedSchema(
        entities={
            "rr-person": IntrospectedEntity(name="rr-person"),
            "rr-badge": IntrospectedEntity(name="rr-badge"),
        },
        relations={"rr-link": _link_relation(abstract=abstract)},
        attributes={"rr-link-id": IntrospectedAttribute(name="rr-link-id", value_type="string")},
        ownerships=[IntrospectedOwnership(owner_name="rr-link", attribute_name="rr-link-id")],
    )


def _target(
    entities: list[type[Entity]] | None = None,
    relations: list[type[Relation]] | None = None,
    attribute_classes: set[type] | None = None,
) -> SchemaInfo:
    info = SchemaInfo()
    info.entities = entities or []
    info.relations = relations or []
    info.attribute_classes = attribute_classes or set()
    return info


def _operations(base: IntrospectedSchema, target: SchemaInfo) -> list[ops.Operation]:
    generator = MigrationGenerator.__new__(MigrationGenerator)
    return generator._introspected_to_operations(base, target)


def _relation_scoped_granular(operations: list[ops.Operation]) -> list[ops.Operation]:
    return [
        operation
        for operation in operations
        if isinstance(operation, (ops.RemoveRole, ops.RemoveRolePlayer, ops.RemoveOwnership))
    ]


def test_removed_relation_maps_to_single_remove_relation() -> None:
    base = _base_with_link()
    target = _target(
        entities=[RrPerson, RrBadge],
        attribute_classes={RrLinkId},
    )

    operations = _operations(base, target)

    assert len(operations) == 1
    (removal,) = operations
    assert isinstance(removal, ops.RemoveRelation)
    assert removal.relation.get_type_name() == "rr-link"


def test_removed_abstract_relation_maps_to_single_remove_relation() -> None:
    base = _base_with_link(abstract=True)
    target = _target(
        entities=[RrPerson, RrBadge],
        attribute_classes={RrLinkId},
    )

    operations = _operations(base, target)

    assert [type(operation) for operation in operations] == [ops.RemoveRelation]


def test_removed_relation_and_attribute_keep_independent_remove_attribute() -> None:
    base = _base_with_link()
    target = _target(entities=[RrPerson, RrBadge])

    operations = _operations(base, target)

    assert [type(operation) for operation in operations] == [
        ops.RemoveRelation,
        ops.RemoveAttribute,
    ]
    remove_attribute = operations[1]
    assert isinstance(remove_attribute, ops.RemoveAttribute)
    assert remove_attribute.attribute.get_attribute_name() == "rr-link-id"


def test_surviving_relation_role_removal_stays_granular() -> None:
    base = IntrospectedSchema(
        entities={"rr-person": IntrospectedEntity(name="rr-person")},
        relations={
            "rr-employ": IntrospectedRelation(
                name="rr-employ",
                roles={
                    "employee": IntrospectedRole(name="employee", player_types=["rr-person"]),
                    "reviewer": IntrospectedRole(name="reviewer", player_types=["rr-person"]),
                },
            )
        },
    )
    target = _target(entities=[RrPerson], relations=[RrEmploySlim])

    operations = _operations(base, target)

    assert [type(operation) for operation in operations] == [ops.RemoveRole]
    remove_role = operations[0]
    assert isinstance(remove_role, ops.RemoveRole)
    assert remove_role.role_name == "reviewer"


def test_surviving_relation_player_removal_stays_granular() -> None:
    base = IntrospectedSchema(
        entities={
            "rr-person": IntrospectedEntity(name="rr-person"),
            "rr-badge": IntrospectedEntity(name="rr-badge"),
        },
        relations={
            "rr-assign": IntrospectedRelation(
                name="rr-assign",
                roles={
                    "worker": IntrospectedRole(
                        name="worker", player_types=["rr-person", "rr-badge"]
                    )
                },
            )
        },
    )
    target = _target(entities=[RrPerson, RrBadge], relations=[RrAssignSlim])

    operations = _operations(base, target)

    assert [type(operation) for operation in operations] == [ops.RemoveRolePlayer]
    remove_player = operations[0]
    assert isinstance(remove_player, ops.RemoveRolePlayer)
    assert remove_player.role_name == "worker"
    assert remove_player.player_type == "rr-badge"


def test_surviving_relation_ownership_removal_stays_granular() -> None:
    base = IntrospectedSchema(
        entities={"rr-person": IntrospectedEntity(name="rr-person")},
        relations={
            "rr-own": IntrospectedRelation(
                name="rr-own",
                roles={"holder": IntrospectedRole(name="holder", player_types=["rr-person"])},
            )
        },
        attributes={"rr-note": IntrospectedAttribute(name="rr-note", value_type="string")},
        ownerships=[IntrospectedOwnership(owner_name="rr-own", attribute_name="rr-note")],
    )
    target = _target(
        entities=[RrPerson],
        relations=[RrOwnSlim],
        attribute_classes={RrNote},
    )

    operations = _operations(base, target)

    assert [type(operation) for operation in operations] == [ops.RemoveOwnership]


def test_wholesale_and_granular_mappings_stay_independent() -> None:
    base = IntrospectedSchema(
        entities={"rr-person": IntrospectedEntity(name="rr-person")},
        relations={
            "rr-link": IntrospectedRelation(
                name="rr-link",
                roles={"subject": IntrospectedRole(name="subject", player_types=["rr-person"])},
            ),
            "rr-employ": IntrospectedRelation(
                name="rr-employ",
                roles={
                    "employee": IntrospectedRole(name="employee", player_types=["rr-person"]),
                    "reviewer": IntrospectedRole(name="reviewer", player_types=["rr-person"]),
                },
            ),
        },
    )
    target = _target(entities=[RrPerson], relations=[RrEmploySlim])

    operations = _operations(base, target)

    removals = [op for op in operations if isinstance(op, ops.RemoveRelation)]
    assert [removal.relation.get_type_name() for removal in removals] == ["rr-link"]

    granular = _relation_scoped_granular(operations)
    assert [type(operation) for operation in granular] == [ops.RemoveRole]
    remove_role = granular[0]
    assert isinstance(remove_role, ops.RemoveRole)
    assert remove_role.relation.get_type_name() == "rr-employ"


# ── executor normalization of legacy v1.5.5/v1.5.6 artifacts ────────────────


def _legacy_decomposed_removal() -> list[ops.Operation]:
    """The operation shape v1.5.5/v1.5.6 authored for a whole-relation removal."""
    link = ref.relation("legacy-link")
    return [
        ops.RemoveRolePlayer(link, "subject", "person"),
        ops.RemoveRole(link, "subject"),
        ops.RemoveRolePlayer(link, "badge", "temporary-badge"),
        ops.RemoveRole(link, "badge"),
        ops.RemoveOwnership(link, ref.attribute("legacy-link-id")),
        ops.RemoveRelation(link),
    ]


def test_normalized_operations_drops_shadowed_granular_removals() -> None:
    legacy = _legacy_decomposed_removal()

    assert _normalized_operations(legacy) == [legacy[-1]]


def test_normalized_operations_keeps_granular_ops_for_surviving_relations() -> None:
    operations: list[ops.Operation] = [
        ops.RemoveRolePlayer(ref.relation("employment"), "employee", "contractor"),
        ops.RemoveRole(ref.relation("employment"), "reviewer"),
        ops.RemoveOwnership(ref.relation("employment"), ref.attribute("note")),
    ]

    assert _normalized_operations(operations) == operations


def test_normalized_operations_is_scoped_to_the_removed_relation() -> None:
    survivor_ops: list[ops.Operation] = [
        ops.RemoveRolePlayer(ref.relation("employment"), "employee", "contractor"),
        ops.RemoveOwnership(ref.entity("person"), ref.attribute("nickname")),
    ]
    legacy = _legacy_decomposed_removal()

    normalized = _normalized_operations([*legacy, *survivor_ops])

    assert normalized == [legacy[-1], *survivor_ops]
