"""Tests for TypeDB put operations (insert if does not exist)."""

import pytest

from type_bridge import (
    Database,
    Entity,
    EntityFlags,
    Flag,
    Integer,
    Key,
    Relation,
    RelationFlags,
    Role,
    String,
)
from type_bridge.schema import SchemaManager


# Define test models
class Name(String):
    pass


class Email(String):
    pass


class Age(Integer):
    pass


class Position(String):
    pass


class Salary(Integer):
    pass


class Person(Entity):
    flags = EntityFlags(type_name="person")
    name: Name = Flag(Key)
    email: Email | None
    age: Age | None


class Company(Entity):
    flags = EntityFlags(type_name="company")
    name: Name = Flag(Key)


class Employment(Relation):
    flags = RelationFlags(type_name="employment")
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)
    position: Position | None
    salary: Salary | None


@pytest.fixture
def db():
    """Create a test database connection."""
    database = Database(address="localhost:1729", database="test_put_operations")
    database.connect()

    # Clean up existing database if it exists
    try:
        database.delete_database()
    except Exception:
        pass

    # Create fresh database
    database.create_database()

    # Set up schema
    schema_manager = SchemaManager(database)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    yield database

    # Clean up
    database.delete_database()
    database.close()


def test_entity_put_single(db):
    """Test putting a single entity."""
    person_manager = Person.manager(db)

    # Create entity
    alice = Person(name=Name("Alice"), age=Age(30), email=Email("alice@example.com"))

    # First put should insert
    person_manager.put(alice)

    # Verify entity exists
    results = person_manager.get(name="Alice")
    assert len(results) == 1
    assert results[0].name.value == "Alice"
    assert results[0].age.value == 30
    assert results[0].email.value == "alice@example.com"

    # Second put should not create duplicate (idempotent)
    person_manager.put(alice)

    # Should still have only 1 person
    results = person_manager.all()
    assert len(results) == 1


def test_entity_put_many(db):
    """Test putting multiple entities."""
    person_manager = Person.manager(db)

    # Create entities
    persons = [
        Person(name=Name("Alice"), age=Age(30), email=None),
        Person(name=Name("Bob"), age=Age(25), email=None),
        Person(name=Name("Charlie"), age=Age(35), email=None),
    ]

    # First put_many should insert all
    person_manager.put_many(persons)

    # Verify all entities exist
    results = person_manager.all()
    assert len(results) == 3

    # Second put_many should not create duplicates
    person_manager.put_many(persons)

    # Should still have only 3 persons
    results = person_manager.all()
    assert len(results) == 3


def test_entity_put_partial_match(db):
    """Test put with all-or-nothing semantics.

    According to TypeDB docs, put works on all-or-nothing basis:
    - If the entire pattern matches, nothing is inserted
    - If any part fails to match, the entire pattern is inserted

    This means putting [Alice, Bob] when Alice exists will try to insert
    both Alice and Bob, causing a key constraint violation.
    """
    person_manager = Person.manager(db)

    # Insert Alice
    alice = Person(name=Name("Alice"), age=Age(30), email=None)
    person_manager.insert(alice)

    # Put both Alice and Bob (Alice exists, Bob doesn't)
    # This should fail because put will try to insert the entire pattern
    # including Alice (who already exists), violating @key constraint
    persons = [
        Person(name=Name("Alice"), age=Age(30), email=None),
        Person(name=Name("Bob"), age=Age(25), email=None),
    ]

    # Expect constraint violation due to all-or-nothing semantics
    with pytest.raises(Exception) as exc_info:
        person_manager.put_many(persons)

    # Verify it's a key constraint violation
    assert "unique" in str(exc_info.value).lower() or "key" in str(exc_info.value).lower()

    # Only Alice should exist (Bob was not inserted due to failure)
    results = person_manager.all()
    assert len(results) == 1
    assert results[0].name.value == "Alice"


def test_relation_put_single(db):
    """Test putting a single relation."""
    person_manager = Person.manager(db)
    company_manager = Company.manager(db)
    employment_manager = Employment.manager(db)

    # Create entities first
    alice = Person(name=Name("Alice"), age=Age(30), email=None)
    tech_corp = Company(name=Name("TechCorp"))

    person_manager.insert(alice)
    company_manager.insert(tech_corp)

    # Create relation
    employment = Employment(
        employee=alice, employer=tech_corp, position=Position("Engineer"), salary=Salary(100000)
    )

    # First put should insert
    employment_manager.put(employment)

    # Verify relation exists
    results = employment_manager.get(position="Engineer")
    assert len(results) == 1
    assert results[0].position.value == "Engineer"
    assert results[0].salary.value == 100000

    # Second put should not create duplicate
    employment_manager.put(employment)

    # Should still have only 1 employment
    results = employment_manager.all()
    assert len(results) == 1


def test_relation_put_many(db):
    """Test putting multiple relations."""
    person_manager = Person.manager(db)
    company_manager = Company.manager(db)
    employment_manager = Employment.manager(db)

    # Create entities first
    alice = Person(name=Name("Alice"), age=Age(30), email=None)
    bob = Person(name=Name("Bob"), age=Age(25), email=None)
    tech_corp = Company(name=Name("TechCorp"))

    person_manager.insert(alice)
    person_manager.insert(bob)
    company_manager.insert(tech_corp)

    # Create relations
    employments = [
        Employment(
            employee=alice,
            employer=tech_corp,
            position=Position("Engineer"),
            salary=Salary(100000),
        ),
        Employment(
            employee=bob, employer=tech_corp, position=Position("Manager"), salary=Salary(120000)
        ),
    ]

    # First put_many should insert all
    employment_manager.put_many(employments)

    # Verify all relations exist
    results = employment_manager.all()
    assert len(results) == 2

    # Second put_many should not create duplicates
    employment_manager.put_many(employments)

    # Should still have only 2 employments
    results = employment_manager.all()
    assert len(results) == 2


def test_put_vs_insert_duplicates(db):
    """Test that put prevents duplicates while insert creates them."""
    person_manager = Person.manager(db)

    alice = Person(name=Name("Alice"), age=Age(30), email=None)

    # Using insert creates duplicates (if key constraint allows)
    person_manager.insert(alice)

    # Using put should be idempotent
    person_manager.put(alice)
    person_manager.put(alice)

    # Get all persons - put should not have created extra duplicates
    # Note: The first insert will fail if @key constraint is enforced properly
    # This test verifies idempotent behavior
    results = person_manager.all()

    # With @key constraint, we should have exactly 1 person
    assert len(results) == 1
    assert results[0].name.value == "Alice"
