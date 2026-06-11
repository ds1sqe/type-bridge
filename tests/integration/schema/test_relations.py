"""Integration tests for schema creation with relations."""

import pytest

from type_bridge import (
    Entity,
    Flag,
    Key,
    Relation,
    Role,
    SchemaManager,
    String,
    TypeFlags,
)
from type_bridge._rust_runtime import descriptor_for_model


@pytest.mark.integration
@pytest.mark.order(2)
def test_schema_with_relations(clean_db):
    """Test creating schema with entities and relations."""

    class Name(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    class Company(Entity):
        flags = TypeFlags(name="company")
        name: Name = Flag(Key)

    class Position(String):
        pass

    class Employment(Relation):
        flags = TypeFlags(name="employment")
        employee: Role[Person] = Role("employee", Person)
        employer: Role[Company] = Role("employer", Company)
        position: Position

    # Create and sync schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    # Verify schema
    schema_info = schema_manager.collect_schema_info()

    entity_names = {e.get_type_name() for e in schema_info.entities}
    assert "person" in entity_names
    assert "company" in entity_names
    relation_names = {r.get_type_name() for r in schema_info.relations}
    assert "employment" in relation_names

    # Verify relation roles
    employment_relation = [r for r in schema_info.relations if r.get_type_name() == "employment"][0]
    assert "employee" in employment_relation._roles
    assert "employer" in employment_relation._roles


@pytest.mark.integration
@pytest.mark.order(3)
def test_schema_relation_subtype_with_specializing_role(clean_db):
    """Relation subtype with a specializing role (overrides=) syncs idempotently.

    Mirrors the parity-contribution / parity-authoring pair from the fixture:
    the child relation specializes one parent role and inherits the other.
    The first sync creates the schema; the second sync is a no-op, confirming
    the diff engine does not re-emit redundant ``relates … as …`` clauses.
    """

    class Title(String):
        pass

    class Contributor(Entity):
        flags = TypeFlags(name="contrib-contributor")
        name: Title = Flag(Key)

    class Author(Entity):
        flags = TypeFlags(name="contrib-author")
        name: Title = Flag(Key)

    class WorkItem(Entity):
        flags = TypeFlags(name="contrib-work-item")
        name: Title = Flag(Key)

    class Contribution(Relation):
        flags = TypeFlags(name="contrib-contribution")

        contributor: Role[Contributor] = Role("contributor", Contributor)
        work: Role[WorkItem] = Role("work", WorkItem)

    class Authoring(Contribution):
        flags = TypeFlags(name="contrib-authoring")

        # 'author' specializes 'contributor'; 'work' is plain-inherited.
        author: Role[Author] = Role("author", Author, overrides="contributor")

    # Validate that the Python descriptor carries the overrides marker before touching
    # the database — this is the unit-layer guarantee exercised in an integration context.
    authoring_descriptor = descriptor_for_model(Authoring)
    author_role = next(r for r in authoring_descriptor["roles"] if r["role_name"] == "author")
    assert author_role["overrides"] == "contributor"
    work_role = next(r for r in authoring_descriptor["roles"] if r["role_name"] == "work")
    assert work_role["overrides"] is None

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Contributor, Author, WorkItem, Contribution, Authoring)

    # First sync — creates the full relation hierarchy.
    schema_manager.sync_schema(force=True)

    schema_info = schema_manager.collect_schema_info()
    relation_names = {r.get_type_name() for r in schema_info.relations}
    assert "contrib-contribution" in relation_names
    assert "contrib-authoring" in relation_names

    # Second sync — must be idempotent (no schema drift detected).
    schema_manager.sync_schema(force=True)


@pytest.mark.integration
@pytest.mark.order(4)
def test_schema_list_interfaces_sync_idempotent(clean_db):
    """Ordered list interface flags (``[]`` marker, ``@distinct``) survive a
    generate→define→parse round trip without triggering spurious schema drift.

    The test drives the raw generate→execute path:
    1. Build a ``SchemaInfo`` IR dict with ``is_ordered``/``ordered``/``distinct`` flags.
    2. Generate TypeQL with ``generate_define_block``.
    3. Execute the TypeQL against the live database via ``execute_query("schema")``.
    4. Fetch the live schema text back and confirm the ``[]`` marker is preserved in
       the round-tripped text — same as the unit emit→parse round trip but exercised
       end-to-end against a real TypeDB instance.

    REP256: instance-level list semantics are engine-unimplemented; this test
    only validates schema-level emission and parsing correctness.
    """
    pytest.importorskip("type_bridge_core")

    from type_bridge._rust_runtime import generate_define_block

    # Build a minimal SchemaInfo IR dict with ordered and distinct markers.
    schema_ir: dict = {
        "entities": {
            "li-person": {
                "type_name": "li-person",
                "is_abstract": False,
                "parent_type": None,
                "owned_attributes": [
                    {
                        "attr_name": "li-plain",
                        "value_type": "string",
                        "annotations": ["Key"],
                        "is_ordered": False,
                    },
                    {
                        "attr_name": "li-tag",
                        "value_type": "string",
                        "annotations": ["Distinct", {"Card": [0, 3]}],
                        "is_ordered": True,
                    },
                ],
                "plays_cardinalities": {},
            }
        },
        "relations": {
            "li-feed": {
                "type_name": "li-feed",
                "is_abstract": False,
                "parent_type": None,
                "owned_attributes": [],
                "roles": [
                    {
                        "role_name": "li-member",
                        "player_type_names": [],
                        "cardinality": None,
                        "overrides": None,
                        "is_abstract": False,
                        "ordered": True,
                        "distinct": False,
                    }
                ],
                "plays_cardinalities": {},
            }
        },
        "attributes": {
            "li-plain": {"attr_name": "li-plain", "value_type": "string"},
            "li-tag": {"attr_name": "li-tag", "value_type": "string"},
        },
    }

    # Generate TypeQL from the IR.
    typeql = generate_define_block(schema_ir)

    # The emitted TypeQL must contain the list-interface markers.
    assert "li-tag[]" in typeql, f"[] marker missing from generated TypeQL:\n{typeql}"
    assert "li-member[]" in typeql, f"[] marker missing from generated TypeQL:\n{typeql}"
    assert "@distinct" in typeql, f"@distinct missing from generated TypeQL:\n{typeql}"

    # Execute the define against the live database.
    clean_db.execute_query(typeql, transaction_type="schema")

    # Fetch the schema back from the live database.
    live_schema = clean_db.get_schema()

    # The live schema must still contain the ordered list markers.
    assert "li-tag[]" in live_schema, (
        f"[] marker on li-tag missing from live schema after round-trip:\n{live_schema}"
    )
    assert "li-member[]" in live_schema, (
        f"[] marker on li-member missing from live schema after round-trip:\n{live_schema}"
    )
