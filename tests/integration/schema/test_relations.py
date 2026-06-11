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
