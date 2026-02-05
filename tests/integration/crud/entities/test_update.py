"""Integration tests for entity update operations."""

import pytest

from type_bridge import (
    Card,
    Entity,
    Flag,
    Integer,
    Key,
    SchemaManager,
    String,
    TypeFlags,
)


@pytest.mark.integration
@pytest.mark.order(17)
def test_update_single_value_attribute(db_with_schema):
    """Test updating a single-value attribute."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None

    manager = Person.manager(db_with_schema)

    # Insert entity
    henry = Person(name=Name("Henry"), age=Age(25))
    manager.insert(henry)

    # Fetch and modify
    results = manager.get(name="Henry")
    henry_fetched = results[0]
    henry_fetched.age = Age(26)

    # Update
    manager.update(henry_fetched)

    # Verify update
    updated = manager.get(name="Henry")
    assert isinstance(updated[0].age, Age)
    assert updated[0].age.value == 26


@pytest.mark.integration
@pytest.mark.order(18)
def test_update_multi_value_attribute(db_with_schema):
    """Test updating multi-value attributes."""

    # Define extended schema with multi-value attribute
    class Name(String):
        pass

    class Tag(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person2")
        name: Name = Flag(Key)
        tags: list[Tag] = Flag(Card(min=1))

    # Create schema
    schema_manager = SchemaManager(db_with_schema)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(db_with_schema)

    # Insert entity with tags
    iris = Person(name=Name("Iris"), tags=[Tag("python"), Tag("typedb")])
    manager.insert(iris)

    # Fetch and modify tags
    results = manager.get(name="Iris")
    iris_fetched = results[0]
    iris_fetched.tags = [Tag("python"), Tag("typedb"), Tag("ai")]

    # Update
    manager.update(iris_fetched)

    # Verify update
    updated = manager.get(name="Iris")
    tag_values = {tag.value for tag in updated[0].tags}
    assert tag_values == {"python", "typedb", "ai"}
