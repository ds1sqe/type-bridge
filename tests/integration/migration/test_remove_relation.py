"""Whole-relation removal integration regression for #168.

A relation present in the base schema and absent from the target must be
removed by a single ``RemoveRelation`` execution step. The legacy granular
decomposition (``RemoveRolePlayer``/``RemoveRole``/``RemoveOwnership`` before
``RemoveRelation``) cannot be committed step by step: TypeDB rejects the
commit that removes the final role of a concrete relation, stranding the
migration after partial schema changes.
"""

import json
from pathlib import Path

import pytest

from type_bridge import Entity, Flag, Key, Relation, Role, String, TypeFlags
from type_bridge.attribute import AttributeFlags
from type_bridge.migration import (
    MigrationExecutor,
    MigrationGenerator,
    SchemaIntrospector,
)
from type_bridge.migration.executor import MigrationError


class RmName(String):
    flags = AttributeFlags(name="rm-name")


class RmLinkId(String):
    flags = AttributeFlags(name="rm-link-id")


class RmPerson(Entity):
    flags = TypeFlags(name="rm-person")
    name: RmName = Flag(Key)


class RmBadge(Entity):
    flags = TypeFlags(name="rm-badge")
    name: RmName = Flag(Key)


class RmLegacyLink(Relation):
    flags = TypeFlags(name="rm-legacy-link")

    subject: Role[RmPerson] = Role("subject", RmPerson)
    badge: Role[RmBadge] = Role("badge", RmBadge)

    link_id: RmLinkId = Flag(Key)


V1_MODELS = [RmPerson, RmBadge, RmLegacyLink]
V2_MODELS = [RmPerson, RmBadge]


def _apply_v1(db, migrations_dir: Path) -> MigrationExecutor:
    generator = MigrationGenerator(db, migrations_dir)
    executor = MigrationExecutor(db, migrations_dir)
    assert generator.generate(models=V1_MODELS, name="initial") is not None
    results = executor.migrate()
    assert all(result.success for result in results)
    return executor


def _insert_link_instances(db) -> None:
    db.execute_query(
        'insert $p isa rm-person, has rm-name "alice";',
        transaction_type="write",
    )
    db.execute_query(
        'insert $b isa rm-badge, has rm-name "temp-1";',
        transaction_type="write",
    )
    db.execute_query(
        "match\n"
        '$p isa rm-person, has rm-name "alice";\n'
        '$b isa rm-badge, has rm-name "temp-1";\n'
        "insert\n"
        '(subject: $p, badge: $b) isa rm-legacy-link, has rm-link-id "L1";',
        transaction_type="write",
    )


def _cleanup_link_instances(db) -> None:
    db.execute_query(
        "match $l isa rm-legacy-link; delete $l;",
        transaction_type="write",
    )
    db.execute_query(
        "match $a isa rm-link-id; delete $a;",
        transaction_type="write",
    )


@pytest.mark.integration
@pytest.mark.order(330)
def test_remove_relation_generates_single_operation(clean_db, tmp_path: Path):
    """The generated artifact carries one RemoveRelation and no granular unwind."""
    migrations_dir = tmp_path / "migrations"
    generator = MigrationGenerator(clean_db, migrations_dir)
    _apply_v1(clean_db, migrations_dir)

    path = generator.generate(models=V2_MODELS, name="drop_link")

    assert path is not None
    content = path.read_text()
    assert content.count("ops.RemoveRelation(") == 1
    assert "RemoveRole(" not in content
    assert "RemoveRolePlayer(" not in content
    assert "RemoveOwnership(" not in content

    sidecar = json.loads(path.with_suffix(".json").read_text())
    kinds = [operation["kind"] for operation in sidecar["operations"]]
    assert kinds.count("remove_relation") == 1
    assert not {"remove_role", "remove_role_player", "remove_ownership"} & set(kinds)


@pytest.mark.integration
@pytest.mark.order(331)
def test_remove_relation_applies_after_data_cleanup(clean_db, tmp_path: Path):
    """The single-step removal succeeds against real data after cleanup."""
    migrations_dir = tmp_path / "migrations"
    generator = MigrationGenerator(clean_db, migrations_dir)
    executor = _apply_v1(clean_db, migrations_dir)

    _insert_link_instances(clean_db)
    _cleanup_link_instances(clean_db)

    assert generator.generate(models=V2_MODELS, name="drop_link") is not None
    results = executor.migrate()

    assert all(result.success for result in results)
    schema = SchemaIntrospector(clean_db).introspect()
    assert "rm-legacy-link" not in schema.get_relation_names()
    assert "rm-link-id" not in schema.get_attribute_names()
    assert "rm-person" in schema.get_entity_names()
    assert "rm-badge" in schema.get_entity_names()


@pytest.mark.integration
@pytest.mark.order(332)
def test_remove_relation_with_instances_fails_atomically_and_retries(clean_db, tmp_path: Path):
    """Rejection caused by remaining data leaves the relation schema intact and retryable."""
    migrations_dir = tmp_path / "migrations"
    generator = MigrationGenerator(clean_db, migrations_dir)
    executor = _apply_v1(clean_db, migrations_dir)

    _insert_link_instances(clean_db)
    assert generator.generate(models=V2_MODELS, name="drop_link") is not None

    with pytest.raises(MigrationError):
        executor.migrate()

    # The single schema step was rejected without partial relation dismantling.
    schema = SchemaIntrospector(clean_db).introspect()
    relation = schema.relations.get("rm-legacy-link")
    assert relation is not None
    assert set(relation.roles) == {"subject", "badge"}
    assert any(
        ownership.owner_name == "rm-legacy-link" and ownership.attribute_name == "rm-link-id"
        for ownership in schema.ownerships
    )

    # After data cleanup the unchanged artifact applies successfully.
    _cleanup_link_instances(clean_db)
    results = executor.migrate()
    assert all(result.success for result in results)
    schema = SchemaIntrospector(clean_db).introspect()
    assert "rm-legacy-link" not in schema.get_relation_names()


@pytest.mark.integration
@pytest.mark.order(333)
def test_legacy_decomposed_artifact_normalizes_to_single_step(clean_db, tmp_path: Path):
    """A v1.5.5/v1.5.6 checked artifact executes as one RemoveRelation step."""
    migrations_dir = tmp_path / "migrations"
    executor = _apply_v1(clean_db, migrations_dir)

    (migrations_dir / "0002_drop_link.py").write_text(
        f"""
from typing import ClassVar

from type_bridge.migration import Migration, operations as ops, ref
from type_bridge.migration.operations import Operation


class DropLinkMigration(Migration):
    dependencies: ClassVar[list[tuple[str, str]]] = [
        ({migrations_dir.name!r}, "0001_initial"),
    ]
    operations: ClassVar[list[Operation]] = [
        ops.RemoveRolePlayer(ref.relation('rm-legacy-link'), 'subject', 'rm-person'),
        ops.RemoveRole(ref.relation('rm-legacy-link'), 'subject'),
        ops.RemoveRolePlayer(ref.relation('rm-legacy-link'), 'badge', 'rm-badge'),
        ops.RemoveRole(ref.relation('rm-legacy-link'), 'badge'),
        ops.RemoveOwnership(ref.relation('rm-legacy-link'), ref.attribute('rm-link-id')),
        ops.RemoveRelation(ref.relation('rm-legacy-link')),
    ]
""".lstrip()
    )

    results = executor.migrate()

    assert all(result.success for result in results)
    schema = SchemaIntrospector(clean_db).introspect()
    assert "rm-legacy-link" not in schema.get_relation_names()
    # The legacy artifact carried no RemoveAttribute: the cascade removes the
    # ownership capability, not the independently defined attribute type.
    assert "rm-link-id" in schema.get_attribute_names()


@pytest.mark.integration
@pytest.mark.order(334)
def test_decomposed_sequence_premise_final_role_commit_fails(clean_db, tmp_path: Path):
    """Document the TypeDB premise: the legacy unwind cannot commit its last role removal."""
    migrations_dir = tmp_path / "migrations"
    _apply_v1(clean_db, migrations_dir)

    clean_db.execute_query(
        "undefine plays rm-legacy-link:subject from rm-person;",
        transaction_type="schema",
    )
    clean_db.execute_query(
        "undefine relates subject from rm-legacy-link;",
        transaction_type="schema",
    )
    clean_db.execute_query(
        "undefine plays rm-legacy-link:badge from rm-badge;",
        transaction_type="schema",
    )

    # TypeDB rejects the commit that would leave a concrete relation with
    # zero roles. The session layer wraps the commit-time validation error,
    # so assert on behavior (raise + surviving schema), not the message.
    with pytest.raises(Exception):  # noqa: B017,PT011 - band-dependent wrapper error
        clean_db.execute_query(
            "undefine relates badge from rm-legacy-link;",
            transaction_type="schema",
        )

    # Earlier granular commits already dismantled part of the schema while the
    # relation itself survives - the drift #168 protects against.
    schema = SchemaIntrospector(clean_db).introspect()
    relation = schema.relations.get("rm-legacy-link")
    assert relation is not None
    assert set(relation.roles) == {"badge"}
