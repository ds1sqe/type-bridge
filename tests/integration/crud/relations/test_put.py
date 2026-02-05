"""Integration tests for relation put operations."""

import pytest

from type_bridge import Entity, Flag, Integer, Key, Relation, Role, String, TypeFlags


@pytest.mark.integration
@pytest.mark.order(21)
def test_put_single_relation(db_with_schema):
    """Test putting a single relation."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None

    class Company(Entity):
        flags = TypeFlags(name="company")
        name: Name = Flag(Key)

    class Position(String):
        pass

    class Employment(Relation):
        flags = TypeFlags(name="employment")
        employee: Role[Person] = Role("employee", Person)
        employer: Role[Company] = Role("employer", Company)
        position: Position | None

    # Create entities first
    person_manager = Person.manager(db_with_schema)
    company_manager = Company.manager(db_with_schema)
    employment_manager = Employment.manager(db_with_schema)

    alice = Person(name=Name("Alice"), age=Age(30))
    tech_corp = Company(name=Name("TechCorp"))

    person_manager.insert(alice)
    company_manager.insert(tech_corp)

    # Create relation
    employment = Employment(employee=alice, employer=tech_corp, position=Position("Engineer"))

    # First put should insert
    employment_manager.put(employment)

    # Verify relation exists
    results = employment_manager.get(position="Engineer")
    assert len(results) == 1
    assert results[0].position is not None
    assert results[0].position.value == "Engineer"

    # Second put should not create duplicate
    employment_manager.put(employment)

    # Should still have only 1 employment
    results = employment_manager.all()
    assert len(results) == 1


@pytest.mark.integration
@pytest.mark.order(22)
def test_put_many_relations(db_with_schema):
    """Test putting multiple relations."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None

    class Company(Entity):
        flags = TypeFlags(name="company")
        name: Name = Flag(Key)

    class Position(String):
        pass

    class Employment(Relation):
        flags = TypeFlags(name="employment")
        employee: Role[Person] = Role("employee", Person)
        employer: Role[Company] = Role("employer", Company)
        position: Position | None

    # Create entities first
    person_manager = Person.manager(db_with_schema)
    company_manager = Company.manager(db_with_schema)
    employment_manager = Employment.manager(db_with_schema)

    alice = Person(name=Name("Alice"), age=Age(30))
    bob = Person(name=Name("Bob"), age=Age(25))
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
        ),
        Employment(employee=bob, employer=tech_corp, position=Position("Manager")),
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
