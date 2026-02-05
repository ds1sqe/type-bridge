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
