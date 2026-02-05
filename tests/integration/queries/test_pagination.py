"""Integration tests for query pagination."""

import pytest

from type_bridge import Entity, Flag, Integer, Key, String, TypeFlags


@pytest.mark.integration
@pytest.mark.order(94)
def test_query_with_limit_and_offset(db_with_schema):
    """Test query pagination with limit and offset."""

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

    # Query with limit
    page1 = manager.filter().limit(5).execute()
    assert len(page1) == 5

    # Query with offset
    page2 = manager.filter().limit(5).offset(5).execute()
    assert len(page2) == 5

    # Verify different results (though order might not be guaranteed)
    # At minimum, we should have 10 total unique persons
    all_names = {p.name.value for p in page1} | {p.name.value for p in page2}
    assert len(all_names) == 10


@pytest.mark.integration
@pytest.mark.order(95)
def test_query_first_method(db_with_schema):
    """Test first() method for getting single result."""

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

    # Get first result
    result = manager.filter(name="Alice").first()

    assert result is not None
    assert result.name.value == "Alice"
    assert isinstance(result.age, Age)
    assert result.age.value == 30

    # Query with no results
    no_result = manager.filter(name="NonExistent").first()
    assert no_result is None


@pytest.mark.integration
@pytest.mark.order(96)
def test_query_count_method(db_with_schema):
    """Test count() method for counting results."""

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
        Person(name=Name("Bob"), age=Age(30)),
        Person(name=Name("Charlie"), age=Age(25)),
    ]
    manager.insert_many(persons)

    # Count all
    total = manager.filter().count()
    assert total == 3

    # Count with filter
    age_30_count = manager.filter(age=30).count()
    assert age_30_count == 2
