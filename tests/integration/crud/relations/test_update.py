"""Integration tests for relation update operations.

Tests cover the RelationManager.update() method with filter-based updates
supporting both attribute and role player filters.
"""

import pytest
from type_bridge import (
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    Role,
    SchemaManager,
    String,
    TypeFlags,
)


@pytest.mark.integration
@pytest.mark.order(130)
def test_relation_manager_has_update_method(db_with_schema):
    """Test that RelationManager has an update() method."""

    # Arrange
    class Name(String):
        pass

    class Position(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    class Company(Entity):
        flags = TypeFlags(name="company")
        name: Name = Flag(Key)

    class Employment(Relation):
        flags = TypeFlags(name="employment")
        employee: Role[Person] = Role("employee", Person)
        employer: Role[Company] = Role("employer", Company)
        position: Position

    schema_manager = SchemaManager(db_with_schema)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    manager = Employment.manager(db_with_schema)

    # Act & Assert - RelationManager should have update() method
    expected = True
    actual = hasattr(manager, "update")
    assert expected == actual


@pytest.mark.integration
@pytest.mark.order(131)
def test_update_relation_by_role_players(db_with_schema):
    """Test updating relations by filtering on role players."""

    # Arrange
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Position(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age

    class Company(Entity):
        flags = TypeFlags(name="company")
        name: Name = Flag(Key)

    class Employment(Relation):
        flags = TypeFlags(name="employment")
        employee: Role[Person] = Role("employee", Person)
        employer: Role[Company] = Role("employer", Company)
        position: Position

    schema_manager = SchemaManager(db_with_schema)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    # Insert entities
    person_mgr = Person.manager(db_with_schema)
    company_mgr = Company.manager(db_with_schema)
    employment_mgr = Employment.manager(db_with_schema)

    alice = Person(name=Name("Alice"), age=Age(30))
    techcorp = Company(name=Name("TechCorp"))
    person_mgr.insert(alice)
    company_mgr.insert(techcorp)

    # Insert initial relation
    emp1 = Employment(employee=alice, employer=techcorp, position=Position("Engineer"))
    employment_mgr.insert(emp1)

    # Verify initial state
    relations = employment_mgr.get(employee=alice)
    expected = 1
    actual = len(relations)
    assert expected == actual
    assert relations[0].position.value == "Engineer"

    # Act - Fetch, modify, and update
    emp = employment_mgr.get(employee=alice)[0]
    emp.position = Position("Senior Engineer")
    employment_mgr.update(emp)

    # Assert - Verify update worked
    updated_relations = employment_mgr.get(employee=alice)
    expected = 1
    actual = len(updated_relations)
    assert expected == actual
    assert updated_relations[0].position.value == "Senior Engineer"


@pytest.mark.integration
@pytest.mark.order(132)
def test_relation_attribute_update_via_direct_typeql(db_with_schema):
    """Test updating relation attributes using direct TypeQL."""

    # Arrange
    class Name(String):
        pass

    class Position(String):
        pass

    class Salary(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    class Company(Entity):
        flags = TypeFlags(name="company")
        name: Name = Flag(Key)

    class Employment(Relation):
        flags = TypeFlags(name="employment")
        employee: Role[Person] = Role("employee", Person)
        employer: Role[Company] = Role("employer", Company)
        position: Position
        salary: Salary

    schema_manager = SchemaManager(db_with_schema)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    # Setup data
    person_mgr = Person.manager(db_with_schema)
    company_mgr = Company.manager(db_with_schema)
    employment_mgr = Employment.manager(db_with_schema)

    bob = Person(name=Name("Bob"))
    acme = Company(name=Name("Acme"))
    person_mgr.insert(bob)
    company_mgr.insert(acme)

    emp = Employment(
        employee=bob, employer=acme, position=Position("Developer"), salary=Salary(80000)
    )
    employment_mgr.insert(emp)

    # Act - Fetch, modify salary, and update
    fetched = employment_mgr.get(employee=bob)[0]
    fetched.salary = Salary(90000)
    employment_mgr.update(fetched)

    # Assert
    result = employment_mgr.get(employee=bob)
    expected = 1
    actual = len(result)
    assert expected == actual

    expected_salary = 90000
    actual_salary = result[0].salary.value
    assert expected_salary == actual_salary


@pytest.mark.integration
@pytest.mark.order(133)
def test_updating_relation_preserves_role_players(db_with_schema):
    """Test that updating relation attributes doesn't change role players."""

    # Arrange
    class Name(String):
        pass

    class Position(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    class Company(Entity):
        flags = TypeFlags(name="company")
        name: Name = Flag(Key)

    class Employment(Relation):
        flags = TypeFlags(name="employment")
        employee: Role[Person] = Role("employee", Person)
        employer: Role[Company] = Role("employer", Company)
        position: Position

    schema_manager = SchemaManager(db_with_schema)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    # Setup
    person_mgr = Person.manager(db_with_schema)
    company_mgr = Company.manager(db_with_schema)
    employment_mgr = Employment.manager(db_with_schema)

    charlie = Person(name=Name("Charlie"))
    bigcorp = Company(name=Name("BigCorp"))
    person_mgr.insert(charlie)
    company_mgr.insert(bigcorp)

    emp = Employment(employee=charlie, employer=bigcorp, position=Position("Intern"))
    employment_mgr.insert(emp)

    # Act - Fetch, modify, and update
    fetched = employment_mgr.get(employee=charlie, employer=bigcorp)[0]
    fetched.position = Position("Junior Developer")
    employment_mgr.update(fetched)

    # Assert - Role players should be unchanged
    result = employment_mgr.get(employee=charlie, employer=bigcorp)
    expected = 1
    actual = len(result)
    assert expected == actual

    # Verify role players are preserved
    assert result[0].employee.name.value == "Charlie"
    assert result[0].employer.name.value == "BigCorp"

    # Verify attribute was updated
    assert result[0].position.value == "Junior Developer"


@pytest.mark.integration
@pytest.mark.order(134)
def test_update_relation_with_optional_attribute(db_with_schema):
    """Test updating optional attributes on relations."""

    # Arrange
    class Name(String):
        pass

    class Position(String):
        pass

    class Notes(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    class Company(Entity):
        flags = TypeFlags(name="company")
        name: Name = Flag(Key)

    class Employment(Relation):
        flags = TypeFlags(name="employment")
        employee: Role[Person] = Role("employee", Person)
        employer: Role[Company] = Role("employer", Company)
        position: Position
        notes: Notes | None = None

    schema_manager = SchemaManager(db_with_schema)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    # Setup
    person_mgr = Person.manager(db_with_schema)
    company_mgr = Company.manager(db_with_schema)
    employment_mgr = Employment.manager(db_with_schema)

    diana = Person(name=Name("Diana"))
    startup = Company(name=Name("Startup"))
    person_mgr.insert(diana)
    company_mgr.insert(startup)

    # Insert without optional notes
    emp = Employment(employee=diana, employer=startup, position=Position("CTO"), notes=None)
    employment_mgr.insert(emp)

    # Act - Fetch, add notes, and update
    fetched = employment_mgr.get(employee=diana)[0]
    fetched.notes = Notes("Founding team member")
    employment_mgr.update(fetched)

    # Assert
    result = employment_mgr.get(employee=diana)
    expected = 1
    actual = len(result)
    assert expected == actual

    assert result[0].notes is not None
    assert result[0].notes.value == "Founding team member"


@pytest.mark.integration
@pytest.mark.order(135)
def test_update_multiple_relations(db_with_schema):
    """Test updating multiple relations individually."""

    # Arrange
    class Name(String):
        pass

    class Position(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    class Company(Entity):
        flags = TypeFlags(name="company")
        name: Name = Flag(Key)

    class Employment(Relation):
        flags = TypeFlags(name="employment")
        employee: Role[Person] = Role("employee", Person)
        employer: Role[Company] = Role("employer", Company)
        position: Position

    schema_manager = SchemaManager(db_with_schema)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    # Setup multiple employments
    person_mgr = Person.manager(db_with_schema)
    company_mgr = Company.manager(db_with_schema)
    employment_mgr = Employment.manager(db_with_schema)

    mega = Company(name=Name("MegaCorp"))
    company_mgr.insert(mega)

    employees = [
        Person(name=Name("Eve")),
        Person(name=Name("Frank")),
        Person(name=Name("Grace")),
    ]
    person_mgr.insert_many(employees)

    employments = [
        Employment(employee=emp, employer=mega, position=Position("Developer")) for emp in employees
    ]
    employment_mgr.insert_many(employments)

    # Act - Fetch all developers and update them to "Senior Developer"
    developers = employment_mgr.get(position="Developer")
    for dev in developers:
        dev.position = Position("Senior Developer")
        employment_mgr.update(dev)

    # Assert - All 3 should be updated
    all_employments = employment_mgr.all()
    expected_count = 3
    actual_count = len(all_employments)
    assert expected_count == actual_count

    for emp in all_employments:
        expected_position = "Senior Developer"
        actual_position = emp.position.value
        assert expected_position == actual_position
