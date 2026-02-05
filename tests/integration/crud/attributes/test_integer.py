"""Integration tests for Integer attribute CRUD operations."""

import pytest

from type_bridge import Entity, Flag, Integer, Key, SchemaManager, String, TypeFlags


@pytest.mark.integration
@pytest.mark.order(27)
def test_integer_insert(clean_db):
    """Test inserting entity with Integer attribute."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person_int")
        name: Name = Flag(Key)
        age: Age

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)

    # Insert person with Integer
    person = Person(name=Name("Alice"), age=Age(30))
    manager.insert(person)

    # Verify insertion
    results = manager.get(name="Alice")
    assert len(results) == 1
    assert results[0].age.value == 30


@pytest.mark.integration
@pytest.mark.order(28)
def test_integer_fetch(clean_db):
    """Test fetching entity by Integer attribute."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person_int_fetch")
        name: Name = Flag(Key)
        age: Age

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)

    # Insert persons
    persons = [
        Person(name=Name("Bob"), age=Age(25)),
        Person(name=Name("Charlie"), age=Age(30)),
        Person(name=Name("Diana"), age=Age(25)),
    ]
    manager.insert_many(persons)

    # Fetch by Integer value
    age_25 = manager.get(age=25)
    assert len(age_25) == 2
    names = {p.name.value for p in age_25}
    assert names == {"Bob", "Diana"}


@pytest.mark.integration
@pytest.mark.order(29)
def test_integer_update(clean_db):
    """Test updating Integer attribute."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person_int_update")
        name: Name = Flag(Key)
        age: Age

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)

    # Insert person
    person = Person(name=Name("Eve"), age=Age(28))
    manager.insert(person)

    # Fetch and update
    results = manager.get(name="Eve")
    person_fetched = results[0]
    person_fetched.age = Age(29)
    manager.update(person_fetched)

    # Verify update
    updated = manager.get(name="Eve")
    assert updated[0].age.value == 29


@pytest.mark.integration
@pytest.mark.order(30)
def test_integer_delete(clean_db):
    """Test deleting entity with Integer attribute."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person_int_delete")
        name: Name = Flag(Key)
        age: Age

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)

    # Insert person
    person = Person(name=Name("Frank"), age=Age(35))
    manager.insert(person)

    # Delete by Integer attribute using filter
    deleted_count = manager.filter(age=35).delete()
    assert deleted_count == 1

    # Verify deletion
    results = manager.get(name="Frank")
    assert len(results) == 0
