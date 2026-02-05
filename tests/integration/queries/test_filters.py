"""Integration tests for query filtering."""

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
@pytest.mark.order(92)
def test_query_with_multiple_filters(db_with_schema):
    """Test query with multiple attribute filters."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class City(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person3")
        name: Name = Flag(Key)
        age: Age | None
        city: City | None

    # Create schema
    schema_manager = SchemaManager(db_with_schema)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(db_with_schema)

    # Insert test data
    persons = [
        Person(name=Name("Alice"), age=Age(30), city=City("NYC")),
        Person(name=Name("Bob"), age=Age(25), city=City("NYC")),
        Person(name=Name("Charlie"), age=Age(30), city=City("LA")),
    ]
    manager.insert_many(persons)

    # Query with multiple filters
    results = manager.get(age=30, city="NYC")

    assert len(results) == 1
    assert results[0].name.value == "Alice"


@pytest.mark.integration
@pytest.mark.order(93)
def test_query_with_multi_value_attributes(db_with_schema):
    """Test querying entities with multi-value attributes."""

    class Name(String):
        pass

    class Tag(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person4")
        name: Name = Flag(Key)
        tags: list[Tag] = Flag(Card(min=1))

    # Create schema
    schema_manager = SchemaManager(db_with_schema)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(db_with_schema)

    # Insert entities with tags
    persons = [
        Person(name=Name("Alice"), tags=[Tag("python"), Tag("ai")]),
        Person(name=Name("Bob"), tags=[Tag("python"), Tag("web")]),
        Person(name=Name("Charlie"), tags=[Tag("java"), Tag("backend")]),
    ]
    manager.insert_many(persons)

    # Fetch all and verify tags
    all_persons = manager.all()
    assert len(all_persons) == 3

    # Find person with specific tag
    alice = manager.get(name="Alice")[0]
    alice_tags = {tag.value for tag in alice.tags}
    assert alice_tags == {"python", "ai"}
