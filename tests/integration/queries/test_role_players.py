"""Integration tests for querying relations by role players."""

import pytest

from type_bridge import Entity, Flag, Key, Relation, Role, String, TypeFlags


@pytest.mark.integration
@pytest.mark.order(97)
def test_relation_query_by_role_player(db_with_schema):
    """Test querying relations by role player."""

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
    bob = Person(name=Name("Bob"))
    techcorp = Company(name=Name("TechCorp"))
    startupco = Company(name=Name("StartupCo"))

    person_manager.insert_many([alice, bob])
    company_manager.insert_many([techcorp, startupco])

    # Create relations
    employment_manager = Employment.manager(db_with_schema)
    employments = [
        Employment(employee=alice, employer=techcorp, position=Position("Engineer")),
        Employment(employee=bob, employer=techcorp, position=Position("Designer")),
        Employment(employee=alice, employer=startupco, position=Position("Consultant")),
    ]
    employment_manager.insert_many(employments)

    # Query by employee
    alice_jobs = employment_manager.get(employee=alice)
    assert len(alice_jobs) == 2
    positions = {job.position.value for job in alice_jobs}
    assert positions == {"Engineer", "Consultant"}

    # Query by employer
    techcorp_employees = employment_manager.get(employer=techcorp)
    assert len(techcorp_employees) == 2


@pytest.mark.integration
@pytest.mark.order(98)
def test_complex_query_with_relations(db_with_schema):
    """Test complex queries involving relations and attributes."""

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

    # Setup data
    person_manager = Person.manager(db_with_schema)
    company_manager = Company.manager(db_with_schema)
    employment_manager = Employment.manager(db_with_schema)

    alice = Person(name=Name("Alice"))
    techcorp = Company(name=Name("TechCorp"))

    person_manager.insert(alice)
    company_manager.insert(techcorp)

    employment = Employment(employee=alice, employer=techcorp, position=Position("Senior Engineer"))
    employment_manager.insert(employment)

    # Query by both role player and attribute
    results = employment_manager.get(employee=alice, position="Senior Engineer")

    assert len(results) == 1
    assert results[0].position.value == "Senior Engineer"
