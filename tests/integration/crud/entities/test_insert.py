"""Integration tests for entity insert operations."""

import pytest

from type_bridge import Entity, Flag, Integer, Key, String, TypeFlags


@pytest.mark.integration
@pytest.mark.order(10)
def test_insert_single_entity(db_with_schema):
    """Test inserting a single entity."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None

    # Create manager
    manager = Person.manager(db_with_schema)

    # Insert entity
    alice = Person(name=Name("Alice"), age=Age(30))
    manager.insert(alice)

    # Verify insertion by fetching
    results = manager.get(name="Alice")
    assert len(results) == 1
    assert results[0].name.value == "Alice"
    assert isinstance(results[0].age, Age)
    assert results[0].age.value == 30


@pytest.mark.integration
@pytest.mark.order(11)
def test_insert_many(db_with_schema):
    """Test bulk insertion of entities."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None

    manager = Person.manager(db_with_schema)

    # Bulk insert
    persons = [
        Person(name=Name("Bob"), age=Age(25)),
        Person(name=Name("Charlie"), age=Age(35)),
        Person(name=Name("Diana"), age=Age(28)),
    ]
    manager.insert_many(persons)

    # Verify insertion by fetching all
    all_persons = manager.all()
    assert len(all_persons) == 3
    names = {p.name.value for p in all_persons}
    assert names == {"Bob", "Charlie", "Diana"}


@pytest.mark.integration
@pytest.mark.order(12)
def test_insert_with_optional_attributes(db_with_schema):
    """Test inserting entities with optional attributes."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None

    manager = Person.manager(db_with_schema)

    # Insert entity without optional attribute
    kate = Person(name=Name("Kate"), age=None)
    manager.insert(kate)

    # Fetch and verify
    results = manager.get(name="Kate")
    assert len(results) == 1
    assert results[0].name.value == "Kate"
    # Age should be None or not set
    assert results[0].age is None or not hasattr(results[0], "age")
