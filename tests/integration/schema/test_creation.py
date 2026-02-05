"""Integration tests for schema creation and updates."""

import pytest

from type_bridge import Entity, Flag, Integer, Key, SchemaManager, String, TypeFlags


@pytest.mark.integration
@pytest.mark.order(1)
def test_schema_creation_and_sync(clean_db):
    """Test creating and syncing a schema to TypeDB."""

    # Define schema
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None

    # Create schema manager
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)

    # Sync schema
    schema_manager.sync_schema(force=True)

    # Verify schema was created by collecting it back
    schema_info = schema_manager.collect_schema_info()

    entity_names = {e.get_type_name() for e in schema_info.entities}
    assert "person" in entity_names
    attr_names = {a.get_attribute_name() for a in schema_info.attribute_classes}
    assert "Name" in attr_names
    assert "Age" in attr_names


@pytest.mark.integration
@pytest.mark.order(7)
def test_schema_update_safe_changes(clean_db):
    """Test that safe schema changes (adding attributes) work."""

    class Name(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    # Create initial schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    # Add a new optional attribute (safe change)
    class Age(Integer):
        pass

    class PersonWithAge(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None  # New optional attribute

    # Sync updated schema with force=True
    schema_manager2 = SchemaManager(clean_db)
    schema_manager2.register(PersonWithAge)
    schema_manager2.sync_schema(force=True)

    # Verify new attribute was added
    schema_info = schema_manager2.collect_schema_info()
    person_entity = [e for e in schema_info.entities if e.get_type_name() == "person"][0]
    owned_attrs = person_entity.get_owned_attributes()

    assert "name" in owned_attrs
    assert "age" in owned_attrs
