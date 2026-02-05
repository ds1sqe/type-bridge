"""Integration tests for simple match queries."""

import pytest

from type_bridge import Entity, Flag, Integer, Key, String, TypeFlags


@pytest.mark.integration
@pytest.mark.order(91)
def test_simple_match_query(db_with_schema):
    """Test simple match query execution."""

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
        Person(name=Name("Alice"), age=Age(30)),
        Person(name=Name("Bob"), age=Age(25)),
        Person(name=Name("Charlie"), age=Age(30)),
    ]
    manager.insert_many(persons)

    # Execute query
    results = manager.get(age=30)

    assert len(results) == 2
    names = {p.name.value for p in results}
    assert names == {"Alice", "Charlie"}
