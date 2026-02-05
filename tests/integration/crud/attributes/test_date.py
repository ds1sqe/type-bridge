"""Integration tests for Date attribute CRUD operations."""

from datetime import date

import pytest

from type_bridge import Date, Entity, Flag, Key, SchemaManager, String, TypeFlags


@pytest.mark.integration
@pytest.mark.order(39)
def test_date_insert(clean_db):
    """Test inserting entity with Date attribute."""

    class Name(String):
        pass

    class BirthDate(Date):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person_date")
        name: Name = Flag(Key)
        birth_date: BirthDate

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)

    # Insert person with Date
    person = Person(name=Name("Alice"), birth_date=BirthDate(date(1990, 5, 15)))
    manager.insert(person)

    # Verify insertion
    results = manager.get(name="Alice")
    assert len(results) == 1
    assert results[0].birth_date.value == date(1990, 5, 15)


@pytest.mark.integration
@pytest.mark.order(40)
def test_date_fetch(clean_db):
    """Test fetching entity by Date attribute."""

    class Name(String):
        pass

    class BirthDate(Date):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person_date_fetch")
        name: Name = Flag(Key)
        birth_date: BirthDate

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)

    # Insert persons
    persons = [
        Person(name=Name("Bob"), birth_date=BirthDate(date(1985, 3, 20))),
        Person(name=Name("Charlie"), birth_date=BirthDate(date(1990, 7, 10))),
    ]
    manager.insert_many(persons)

    # Fetch by Date value
    results = manager.get(birth_date=date(1985, 3, 20))
    assert len(results) == 1
    assert results[0].name.value == "Bob"


@pytest.mark.integration
@pytest.mark.order(41)
def test_date_update(clean_db):
    """Test updating Date attribute."""

    class Name(String):
        pass

    class BirthDate(Date):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person_date_update")
        name: Name = Flag(Key)
        birth_date: BirthDate

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)

    # Insert person
    person = Person(name=Name("Diana"), birth_date=BirthDate(date(1995, 1, 1)))
    manager.insert(person)

    # Fetch and update
    results = manager.get(name="Diana")
    person_fetched = results[0]
    person_fetched.birth_date = BirthDate(date(1995, 1, 2))
    manager.update(person_fetched)

    # Verify update
    updated = manager.get(name="Diana")
    assert updated[0].birth_date.value == date(1995, 1, 2)


@pytest.mark.integration
@pytest.mark.order(42)
def test_date_delete(clean_db):
    """Test deleting entity with Date attribute."""

    class Name(String):
        pass

    class BirthDate(Date):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person_date_delete")
        name: Name = Flag(Key)
        birth_date: BirthDate

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)

    # Insert person
    person = Person(name=Name("Eve"), birth_date=BirthDate(date(2000, 12, 31)))
    manager.insert(person)

    # Delete by Date attribute using filter
    deleted_count = manager.filter(birth_date=date(2000, 12, 31)).delete()
    assert deleted_count == 1

    # Verify deletion
    results = manager.get(name="Eve")
    assert len(results) == 0
