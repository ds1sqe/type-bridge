"""Integration tests for entity fetch operations."""

import pytest

from type_bridge import Entity, Flag, Integer, Key, String, TypeFlags


@pytest.mark.integration
@pytest.mark.order(13)
def test_fetch_single_entity(db_with_schema):
    """Test fetching a single entity by attribute."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None

    manager = Person.manager(db_with_schema)

    # Insert test data
    alice = Person(name=Name("Alice"), age=Age(30))
    manager.insert(alice)

    # Fetch entity
    results = manager.get(name="Alice")

    assert len(results) == 1
    assert results[0].name.value == "Alice"
    assert isinstance(results[0].age, Age)
    assert results[0].age.value == 30


@pytest.mark.integration
@pytest.mark.order(14)
def test_fetch_all(db_with_schema):
    """Test fetching all entities."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None

    manager = Person.manager(db_with_schema)

    # Insert test data
    persons = [
        Person(name=Name("Bob"), age=Age(25)),
        Person(name=Name("Charlie"), age=Age(35)),
        Person(name=Name("Diana"), age=Age(28)),
    ]
    manager.insert_many(persons)

    # Fetch all
    all_persons = manager.all()

    assert len(all_persons) == 3
    names = {p.name.value for p in all_persons}
    assert names == {"Bob", "Charlie", "Diana"}


@pytest.mark.integration
@pytest.mark.order(15)
def test_fetch_with_filter(db_with_schema):
    """Test fetching entities with attribute filters."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None

    manager = Person.manager(db_with_schema)

    # Insert test data
    persons = [
        Person(name=Name("Eve"), age=Age(30)),
        Person(name=Name("Frank"), age=Age(30)),
        Person(name=Name("Grace"), age=Age(40)),
    ]
    manager.insert_many(persons)

    # Filter by age
    age_30 = manager.get(age=30)

    assert len(age_30) == 2
    names = {p.name.value for p in age_30}
    assert names == {"Eve", "Frank"}


@pytest.mark.integration
@pytest.mark.order(16)
def test_chainable_query(db_with_schema):
    """Test chainable query API."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None

    manager = Person.manager(db_with_schema)

    # Insert test data
    persons = [Person(name=Name(f"Person{i}"), age=Age(20 + i)) for i in range(10)]
    manager.insert_many(persons)

    # Chainable query: filter + limit
    query = manager.filter(age=22)
    results = query.limit(1).execute()

    assert len(results) == 1
    assert isinstance(results[0].age, Age)
    assert results[0].age.value == 22

    # Count query
    count = manager.filter(age=25).count()
    assert count == 1
