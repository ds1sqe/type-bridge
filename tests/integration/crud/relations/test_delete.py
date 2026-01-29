"""Integration tests for relation deletion operations.

Tests cover the RelationManager.delete() method with instance-based deletion
using role players' @key attributes to identify the relation.
"""

import pytest

from type_bridge import (
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    RelationNotFoundError,
    Role,
    SchemaManager,
    String,
    TypeFlags,
)


@pytest.mark.integration
@pytest.mark.order(140)
def test_relation_manager_has_delete_method(db_with_schema):
    """Test that RelationManager has a delete() method."""

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

    # Act & Assert - RelationManager should have delete() method
    expected = True
    actual = hasattr(manager, "delete")
    assert expected == actual


@pytest.mark.integration
@pytest.mark.order(141)
def test_delete_relation_instance(db_with_schema):
    """Test deleting specific relation by instance."""

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

    # Setup data
    person_mgr = Person.manager(db_with_schema)
    company_mgr = Company.manager(db_with_schema)
    employment_mgr = Employment.manager(db_with_schema)

    alice = Person(name=Name("Alice"))
    techcorp = Company(name=Name("TechCorp"))
    person_mgr.insert(alice)
    company_mgr.insert(techcorp)

    emp = Employment(employee=alice, employer=techcorp, position=Position("Engineer"))
    employment_mgr.insert(emp)

    # Verify insertion
    relations_before = employment_mgr.all()
    expected = 1
    actual = len(relations_before)
    assert expected == actual

    # Act - Delete relation using instance
    deleted = employment_mgr.delete(emp)

    # Assert - Returns the relation instance
    assert deleted is emp

    # Assert - Relation should be deleted
    relations_after = employment_mgr.all()
    expected = 0
    actual = len(relations_after)
    assert expected == actual


@pytest.mark.integration
@pytest.mark.order(142)
def test_delete_relation_returns_instance(db_with_schema):
    """Test that delete returns the relation instance, not a count."""

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

    person_mgr = Person.manager(db_with_schema)
    company_mgr = Company.manager(db_with_schema)
    employment_mgr = Employment.manager(db_with_schema)

    bob = Person(name=Name("Bob"))
    acme = Company(name=Name("Acme"))
    person_mgr.insert(bob)
    company_mgr.insert(acme)

    emp = Employment(employee=bob, employer=acme, position=Position("Manager"))
    employment_mgr.insert(emp)

    # Act
    deleted = employment_mgr.delete(emp)

    # Assert - Should return relation, not int
    assert isinstance(deleted, Employment)
    assert deleted.position.value == "Manager"


@pytest.mark.integration
@pytest.mark.order(143)
def test_delete_relation_missing_role_player_raises(db_with_schema):
    """Test that deleting relation with missing role player raises ValueError."""

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

    employment_mgr = Employment.manager(db_with_schema)

    # Create relation then manually remove role player to simulate missing data
    alice = Person(name=Name("Alice"))
    techcorp = Company(name=Name("TechCorp"))
    emp = Employment(employee=alice, employer=techcorp, position=Position("Engineer"))

    # Manually remove the role player to simulate a corrupt/incomplete relation
    emp.__dict__["employer"] = None  # type: ignore[index]

    # Act & Assert
    with pytest.raises(ValueError, match="Role player 'employer' is required for delete"):
        employment_mgr.delete(emp)


@pytest.mark.integration
@pytest.mark.order(144)
def test_delete_relation_preserves_role_player_entities(db_with_schema):
    """Test that deleting a relation does not delete the role player entities."""

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

    diana = Person(name=Name("Diana"))
    startup = Company(name=Name("Startup"))
    person_mgr.insert(diana)
    company_mgr.insert(startup)

    emp = Employment(employee=diana, employer=startup, position=Position("Founder"))
    employment_mgr.insert(emp)

    # Act - Delete relation using instance
    deleted = employment_mgr.delete(emp)

    # Assert - Relation deleted
    assert deleted is emp
    relations = employment_mgr.all()
    expected = 0
    actual = len(relations)
    assert expected == actual

    # Assert - Entities still exist
    people = person_mgr.all()
    expected_people = 1
    actual_people = len(people)
    assert expected_people == actual_people
    assert "Diana" == people[0].name.value

    companies = company_mgr.all()
    expected_companies = 1
    actual_companies = len(companies)
    assert expected_companies == actual_companies
    assert "Startup" == companies[0].name.value


@pytest.mark.integration
@pytest.mark.order(145)
def test_delete_many_relations(db_with_schema):
    """Test batch deletion of multiple relations."""

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

    person_mgr = Person.manager(db_with_schema)
    company_mgr = Company.manager(db_with_schema)
    employment_mgr = Employment.manager(db_with_schema)

    alice = Person(name=Name("Alice"))
    bob = Person(name=Name("Bob"))
    charlie = Person(name=Name("Charlie"))
    techcorp = Company(name=Name("TechCorp"))

    person_mgr.insert_many([alice, bob, charlie])
    company_mgr.insert(techcorp)

    emp1 = Employment(employee=alice, employer=techcorp, position=Position("Engineer"))
    emp2 = Employment(employee=bob, employer=techcorp, position=Position("Manager"))
    emp3 = Employment(employee=charlie, employer=techcorp, position=Position("Intern"))

    employment_mgr.insert_many([emp1, emp2, emp3])

    # Verify insertion
    assert len(employment_mgr.all()) == 3

    # Act - Delete multiple relations
    deleted = employment_mgr.delete_many([emp1, emp2])

    # Assert - Returns list of deleted relations
    assert len(deleted) == 2
    assert emp1 in deleted
    assert emp2 in deleted

    # Verify only emp3 remains
    remaining = employment_mgr.all()
    assert len(remaining) == 1
    assert remaining[0].position.value == "Intern"


@pytest.mark.integration
@pytest.mark.order(146)
def test_delete_many_empty_list(db_with_schema):
    """Test that delete_many with empty list returns empty list."""

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

    employment_mgr = Employment.manager(db_with_schema)

    # Act
    deleted = employment_mgr.delete_many([])

    # Assert
    assert deleted == []


@pytest.mark.integration
@pytest.mark.order(147)
def test_relation_delete_instance_method(db_with_schema):
    """Test relation.delete(connection) instance method."""

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

    person_mgr = Person.manager(db_with_schema)
    company_mgr = Company.manager(db_with_schema)
    employment_mgr = Employment.manager(db_with_schema)

    eve = Person(name=Name("Eve"))
    mega = Company(name=Name("MegaCorp"))
    person_mgr.insert(eve)
    company_mgr.insert(mega)

    emp = Employment(employee=eve, employer=mega, position=Position("CEO"))
    employment_mgr.insert(emp)

    # Verify insertion
    assert len(employment_mgr.all()) == 1

    # Act - Delete using instance method
    result = emp.delete(db_with_schema)

    # Assert - Returns self for chaining
    assert result is emp

    # Verify deletion
    assert len(employment_mgr.all()) == 0


@pytest.mark.integration
@pytest.mark.order(148)
def test_filter_based_delete_still_works(db_with_schema):
    """Test that filter-based deletion via manager.filter(...).delete() still works."""

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

    person_mgr = Person.manager(db_with_schema)
    company_mgr = Company.manager(db_with_schema)
    employment_mgr = Employment.manager(db_with_schema)

    frank = Person(name=Name("Frank"))
    grace = Person(name=Name("Grace"))
    corp = Company(name=Name("Corp"))

    person_mgr.insert_many([frank, grace])
    company_mgr.insert(corp)

    emp1 = Employment(
        employee=frank, employer=corp, position=Position("Junior"), salary=Salary(50000)
    )
    emp2 = Employment(
        employee=grace, employer=corp, position=Position("Senior"), salary=Salary(150000)
    )

    employment_mgr.insert_many([emp1, emp2])

    # Act - Delete using filter (old-style deletion via query)
    count = employment_mgr.filter(Salary.gt(Salary(100000))).delete()

    # Assert
    assert count == 1

    # Only Junior should remain
    remaining = employment_mgr.all()
    assert len(remaining) == 1
    assert remaining[0].position.value == "Junior"


@pytest.mark.integration
@pytest.mark.order(149)
def test_delete_nonexistent_relation_raises(db_with_schema):
    """Test that deleting relation that doesn't exist raises RelationNotFoundError."""

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

    person_mgr = Person.manager(db_with_schema)
    company_mgr = Company.manager(db_with_schema)
    employment_mgr = Employment.manager(db_with_schema)

    # Insert entities but NOT the relation
    alice = Person(name=Name("Alice"))
    techcorp = Company(name=Name("TechCorp"))
    person_mgr.insert(alice)
    company_mgr.insert(techcorp)

    # Create relation but don't insert it
    emp = Employment(employee=alice, employer=techcorp, position=Position("Engineer"))

    # Act & Assert - Should raise RelationNotFoundError
    with pytest.raises(RelationNotFoundError, match="not found with given role players"):
        employment_mgr.delete(emp)
