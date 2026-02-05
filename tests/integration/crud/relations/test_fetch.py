"""Integration tests for relation fetch operations."""

import pytest

from type_bridge import Entity, Flag, Key, Relation, Role, String, TypeFlags


@pytest.mark.integration
@pytest.mark.order(21)
def test_fetch_relation_by_role_player(db_with_schema):
    """Test fetching relations by role player."""

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

    bob = Person(name=Name("Bob"))
    devco = Company(name=Name("DevCo"))

    person_manager.insert(bob)
    company_manager.insert(devco)

    # Create relation
    employment_manager = Employment.manager(db_with_schema)
    employment = Employment(employee=bob, employer=devco, position=Position("Developer"))
    employment_manager.insert(employment)

    # Fetch relation by role player
    results = employment_manager.get(employee=bob)
    assert len(results) == 1
    assert results[0].position.value == "Developer"


@pytest.mark.integration
@pytest.mark.order(22)
def test_fetch_relation_by_attribute(db_with_schema):
    """Test fetching relations by attribute."""

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

    charlie = Person(name=Name("Charlie"))
    startup = Company(name=Name("StartupXYZ"))

    person_manager.insert(charlie)
    company_manager.insert(startup)

    # Create relation
    employment_manager = Employment.manager(db_with_schema)
    employment = Employment(employee=charlie, employer=startup, position=Position("CTO"))
    employment_manager.insert(employment)

    # Fetch relation by attribute
    results_by_pos = employment_manager.get(position="CTO")
    assert len(results_by_pos) == 1
    assert results_by_pos[0].position.value == "CTO"
