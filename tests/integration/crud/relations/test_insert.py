"""Integration tests for relation insert operations."""

import pytest

from type_bridge import Entity, Flag, Key, Relation, Role, String, TypeFlags


@pytest.mark.integration
@pytest.mark.order(20)
def test_insert_relation(db_with_schema):
    """Test inserting relations with role players."""

    class Name(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    class Company(Entity):
        flags = TypeFlags(name="company")
        name: Name = Flag(Key)

    class Position(String):
        pass

    class Employment(Relation):
        flags = TypeFlags(name="employment")
        employee: Role[Person] = Role("employee", Person)
        employer: Role[Company] = Role("employer", Company)
        position: Position

    # Create entities
    person_manager = Person.manager(db_with_schema)
    company_manager = Company.manager(db_with_schema)

    alice = Person(name=Name("Alice"))
    techcorp = Company(name=Name("TechCorp"))

    person_manager.insert(alice)
    company_manager.insert(techcorp)

    # Create relation
    employment_manager = Employment.manager(db_with_schema)
    employment = Employment(employee=alice, employer=techcorp, position=Position("Engineer"))
    employment_manager.insert(employment)

    # Verify insertion by fetching
    results = employment_manager.get(employee=alice)
    assert len(results) == 1
    assert results[0].position.value == "Engineer"
