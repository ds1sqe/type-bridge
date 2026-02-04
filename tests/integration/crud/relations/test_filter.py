"""Integration tests for relation filtering operations.

Tests cover both get() method with filters and the new filter() method with
expression-based filtering, limit/offset, and aggregations.
"""

import pytest
from type_bridge import (
    Entity,
    Flag,
    Key,
    Relation,
    Role,
    SchemaManager,
    String,
    TypeFlags,
)


@pytest.mark.integration
@pytest.mark.order(150)
def test_filter_relations_by_attribute(db_with_schema):
    """Test filtering relations by attribute value using get()."""

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

    alice = Person(name=Name("Alice"))
    bob = Person(name=Name("Bob"))
    tech = Company(name=Name("Tech"))

    person_mgr.insert_many([alice, bob])
    company_mgr.insert(tech)

    employments = [
        Employment(employee=alice, employer=tech, position=Position("Engineer")),
        Employment(employee=bob, employer=tech, position=Position("Manager")),
    ]
    employment_mgr.insert_many(employments)

    # Act - Filter by position attribute
    result = employment_mgr.get(position="Engineer")

    # Assert
    expected = 1
    actual = len(result)
    assert expected == actual
    assert result[0].employee.name.value == "Alice"
    assert result[0].position.value == "Engineer"


@pytest.mark.integration
@pytest.mark.order(151)
def test_filter_relations_by_role_player(db_with_schema):
    """Test filtering relations by role player entity using get()."""

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
    diana = Person(name=Name("Diana"))
    acme = Company(name=Name("Acme"))
    bigco = Company(name=Name("BigCo"))

    person_mgr.insert_many([charlie, diana])
    company_mgr.insert_many([acme, bigco])

    employments = [
        Employment(employee=charlie, employer=acme, position=Position("Dev")),
        Employment(employee=diana, employer=bigco, position=Position("Dev")),
    ]
    employment_mgr.insert_many(employments)

    # Act - Filter by employee role player
    result = employment_mgr.get(employee=charlie)

    # Assert
    expected = 1
    actual = len(result)
    assert expected == actual
    assert result[0].employee.name.value == "Charlie"
    assert result[0].employer.name.value == "Acme"


@pytest.mark.integration
@pytest.mark.order(152)
def test_filter_relations_combined_attribute_and_role(db_with_schema):
    """Test filtering relations by both attribute and role player."""

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

    eve = Person(name=Name("Eve"))
    frank = Person(name=Name("Frank"))
    startup = Company(name=Name("Startup"))

    person_mgr.insert_many([eve, frank])
    company_mgr.insert(startup)

    employments = [
        Employment(employee=eve, employer=startup, position=Position("CTO")),
        Employment(employee=frank, employer=startup, position=Position("CEO")),
    ]
    employment_mgr.insert_many(employments)

    # Act - Filter by both position AND employer
    result = employment_mgr.get(position="CTO", employer=startup)

    # Assert
    expected = 1
    actual = len(result)
    assert expected == actual
    assert result[0].employee.name.value == "Eve"
    assert result[0].position.value == "CTO"


@pytest.mark.integration
@pytest.mark.order(153)
def test_filter_relations_with_invalid_attribute_raises_error(db_with_schema):
    """Test that filtering by invalid attribute raises ValueError."""

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
        # Employment does NOT have a salary attribute

    schema_manager = SchemaManager(db_with_schema)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    employment_mgr = Employment.manager(db_with_schema)

    # Act & Assert - Filter by non-existent attribute should raise error
    with pytest.raises(ValueError) as exc_info:
        employment_mgr.get(salary=100000)

    error_msg = str(exc_info.value)
    assert "Unknown filter" in error_msg or "salary" in error_msg


@pytest.mark.integration
@pytest.mark.order(154)
def test_filter_relations_empty_result(db_with_schema):
    """Test filtering relations that match no results."""

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

    grace = Person(name=Name("Grace"))
    mega = Company(name=Name("Mega"))

    person_mgr.insert(grace)
    company_mgr.insert(mega)

    emp = Employment(employee=grace, employer=mega, position=Position("Intern"))
    employment_mgr.insert(emp)

    # Act - Filter for non-existent position
    result = employment_mgr.get(position="CEO")

    # Assert - Should return empty list
    expected = 0
    actual = len(result)
    assert expected == actual


@pytest.mark.integration
@pytest.mark.order(155)
def test_filter_relations_with_multiple_matching_results(db_with_schema):
    """Test filtering relations that return multiple matches."""

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

    # Setup multiple employments with same position
    person_mgr = Person.manager(db_with_schema)
    company_mgr = Company.manager(db_with_schema)
    employment_mgr = Employment.manager(db_with_schema)

    company = Company(name=Name("DevCorp"))
    company_mgr.insert(company)

    people = [Person(name=Name(f"Dev{i}")) for i in range(3)]
    person_mgr.insert_many(people)

    employments = [
        Employment(employee=person, employer=company, position=Position("Developer"))
        for person in people
    ]
    employment_mgr.insert_many(employments)

    # Act - Filter for all developers
    result = employment_mgr.get(position="Developer")

    # Assert - Should return all 3
    expected = 3
    actual = len(result)
    assert expected == actual

    # Verify all have Developer position
    for emp in result:
        assert emp.position.value == "Developer"


@pytest.mark.integration
@pytest.mark.order(156)
def test_filter_relations_by_both_role_players(db_with_schema):
    """Test filtering relations by multiple role players."""

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

    henry = Person(name=Name("Henry"))
    iris = Person(name=Name("Iris"))
    corp1 = Company(name=Name("Corp1"))
    corp2 = Company(name=Name("Corp2"))

    person_mgr.insert_many([henry, iris])
    company_mgr.insert_many([corp1, corp2])

    employments = [
        Employment(employee=henry, employer=corp1, position=Position("Tech Lead")),
        Employment(employee=henry, employer=corp2, position=Position("Consultant")),
        Employment(employee=iris, employer=corp1, position=Position("Manager")),
    ]
    employment_mgr.insert_many(employments)

    # Act - Filter by both employee AND employer
    result = employment_mgr.get(employee=henry, employer=corp1)

    # Assert - Should return only Henry at Corp1
    expected = 1
    actual = len(result)
    assert expected == actual
    assert result[0].employee.name.value == "Henry"
    assert result[0].employer.name.value == "Corp1"
    assert result[0].position.value == "Tech Lead"


# New filter() method tests


@pytest.mark.integration
@pytest.mark.order(157)
def test_relation_manager_has_filter_method(db_with_schema):
    """Test that RelationManager has a filter() method."""
    # Arrange
    from type_bridge import Integer

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

    manager = Employment.manager(db_with_schema)

    # Act & Assert - RelationManager should have filter() method
    expected = True
    actual = hasattr(manager, "filter")
    assert expected == actual


@pytest.mark.integration
@pytest.mark.order(158)
def test_filter_with_limit_and_offset(db_with_schema):
    """Test filter() with limit and offset for pagination."""
    # Arrange
    from type_bridge import Integer

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

    # Setup multiple employments
    person_mgr = Person.manager(db_with_schema)
    company_mgr = Company.manager(db_with_schema)
    employment_mgr = Employment.manager(db_with_schema)

    company = Company(name=Name("TechCorp"))
    company_mgr.insert(company)

    employees = [Person(name=Name(f"Employee{i}")) for i in range(5)]
    person_mgr.insert_many(employees)

    employments = [
        Employment(
            employee=emp,
            employer=company,
            position=Position("Engineer"),
            salary=Salary(100000 + i * 10000),
        )
        for i, emp in enumerate(employees)
    ]
    employment_mgr.insert_many(employments)

    # Act - Test limit
    result_limited = employment_mgr.filter(position="Engineer").limit(2).execute()

    # Assert - Should return at most 2 results
    assert len(result_limited) <= 2

    # Act - Test offset
    result_all = employment_mgr.filter(position="Engineer").execute()
    result_offset = employment_mgr.filter(position="Engineer").offset(2).limit(2).execute()

    # Assert - offset should skip results
    assert len(result_all) == 5
    assert len(result_offset) <= 2


@pytest.mark.integration
@pytest.mark.order(159)
def test_filter_first_and_count(db_with_schema):
    """Test filter() with first() and count() methods."""
    # Arrange
    from type_bridge import Integer

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

    # Setup
    person_mgr = Person.manager(db_with_schema)
    company_mgr = Company.manager(db_with_schema)
    employment_mgr = Employment.manager(db_with_schema)

    company = Company(name=Name("DevCorp"))
    company_mgr.insert(company)

    people = [Person(name=Name(f"Dev{i}")) for i in range(3)]
    person_mgr.insert_many(people)

    employments = [
        Employment(
            employee=person,
            employer=company,
            position=Position("Developer"),
            salary=Salary(90000),
        )
        for person in people
    ]
    employment_mgr.insert_many(employments)

    # Act - Test first()
    first_result = employment_mgr.filter(position="Developer").first()

    # Assert - first() should return one relation
    assert first_result is not None
    assert first_result.position.value == "Developer"

    # Act - Test count()
    count_result = employment_mgr.filter(position="Developer").count()

    # Assert - count() should return 3
    expected_count = 3
    assert expected_count == count_result


@pytest.mark.integration
@pytest.mark.order(160)
def test_filter_with_dict_filters(db_with_schema):
    """Test filter() with dictionary-based filters (exact match)."""
    # Arrange
    from type_bridge import Integer

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

    # Setup
    person_mgr = Person.manager(db_with_schema)
    company_mgr = Company.manager(db_with_schema)
    employment_mgr = Employment.manager(db_with_schema)

    alice = Person(name=Name("Alice"))
    bob = Person(name=Name("Bob"))
    tech = Company(name=Name("TechCo"))

    person_mgr.insert_many([alice, bob])
    company_mgr.insert(tech)

    employments = [
        Employment(
            employee=alice, employer=tech, position=Position("Engineer"), salary=Salary(100000)
        ),
        Employment(
            employee=bob, employer=tech, position=Position("Manager"), salary=Salary(120000)
        ),
    ]
    employment_mgr.insert_many(employments)

    # Act - Filter by attribute
    result = employment_mgr.filter(position="Engineer").execute()

    # Assert
    expected = 1
    actual = len(result)
    assert expected == actual
    assert result[0].employee.name.value == "Alice"


@pytest.mark.integration
@pytest.mark.order(161)
def test_filter_combining_role_players_and_attributes(db_with_schema):
    """Test filter() combining role player and attribute filters."""
    # Arrange
    from type_bridge import Integer

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

    # Setup
    person_mgr = Person.manager(db_with_schema)
    company_mgr = Company.manager(db_with_schema)
    employment_mgr = Employment.manager(db_with_schema)

    charlie = Person(name=Name("Charlie"))
    startup = Company(name=Name("Startup"))

    person_mgr.insert(charlie)
    company_mgr.insert(startup)

    employments = [
        Employment(
            employee=charlie, employer=startup, position=Position("CTO"), salary=Salary(150000)
        ),
        Employment(
            employee=charlie, employer=startup, position=Position("Founder"), salary=Salary(50000)
        ),
    ]
    employment_mgr.insert_many(employments)

    # Act - Filter by both role player and attribute
    result = employment_mgr.filter(employee=charlie, position="CTO").execute()

    # Assert
    expected = 1
    actual = len(result)
    assert expected == actual
    assert result[0].position.value == "CTO"
    assert result[0].salary.value == 150000


@pytest.mark.integration
@pytest.mark.order(162)
def test_relation_query_delete(db_with_schema):
    """Test RelationQuery.delete() method."""
    # Arrange
    from type_bridge import Integer

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

    # Setup
    person_mgr = Person.manager(db_with_schema)
    company_mgr = Company.manager(db_with_schema)
    employment_mgr = Employment.manager(db_with_schema)

    alice = Person(name=Name("Alice"))
    bob = Person(name=Name("Bob"))
    techcorp = Company(name=Name("TechCorp"))

    person_mgr.insert_many([alice, bob])
    company_mgr.insert(techcorp)

    employments = [
        Employment(
            employee=alice, employer=techcorp, position=Position("Engineer"), salary=Salary(120000)
        ),
        Employment(
            employee=bob, employer=techcorp, position=Position("Intern"), salary=Salary(40000)
        ),
        Employment(
            employee=alice,
            employer=techcorp,
            position=Position("Consultant"),
            salary=Salary(150000),
        ),
    ]
    employment_mgr.insert_many(employments)

    # Verify we have 3 employments
    all_employments = employment_mgr.all()
    assert len(all_employments) == 3

    # Act - Delete low-salary employments using filter().delete()
    deleted_count = employment_mgr.filter(Salary.lt(Salary(50000))).delete()

    # Assert
    expected_deleted = 1  # Only the intern with salary 40000
    assert expected_deleted == deleted_count

    # Verify remaining employments
    remaining = employment_mgr.all()
    expected_remaining = 2
    assert expected_remaining == len(remaining)

    # Verify that the intern employment was deleted
    intern_employments = [e for e in remaining if e.position.value == "Intern"]
    assert len(intern_employments) == 0


@pytest.mark.integration
@pytest.mark.order(163)
def test_relation_query_delete_with_role_player_filter(db_with_schema):
    """Test RelationQuery.delete() with role player filters."""
    # Arrange
    from type_bridge import Integer

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

    # Setup
    person_mgr = Person.manager(db_with_schema)
    company_mgr = Company.manager(db_with_schema)
    employment_mgr = Employment.manager(db_with_schema)

    alice = Person(name=Name("Alice"))
    bob = Person(name=Name("Bob"))
    techcorp = Company(name=Name("TechCorp"))
    startup = Company(name=Name("Startup"))

    person_mgr.insert_many([alice, bob])
    company_mgr.insert_many([techcorp, startup])

    employments = [
        Employment(
            employee=alice, employer=techcorp, position=Position("Engineer"), salary=Salary(120000)
        ),
        Employment(
            employee=bob, employer=techcorp, position=Position("Engineer"), salary=Salary(100000)
        ),
        Employment(
            employee=alice, employer=startup, position=Position("CTO"), salary=Salary(180000)
        ),
    ]
    employment_mgr.insert_many(employments)

    # Act - Delete all Alice's employments
    deleted_count = employment_mgr.filter(employee=alice).delete()

    # Assert
    expected_deleted = 2  # Alice has 2 employments
    assert expected_deleted == deleted_count

    # Verify remaining employments
    remaining = employment_mgr.all()
    expected_remaining = 1
    assert expected_remaining == len(remaining)

    # Verify only Bob's employment remains
    assert remaining[0].employee.name.value == "Bob"


@pytest.mark.integration
@pytest.mark.order(164)
def test_relation_query_update_with(db_with_schema):
    """Test RelationQuery.update_with() method."""
    # Arrange
    from type_bridge import Integer

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

    # Setup
    person_mgr = Person.manager(db_with_schema)
    company_mgr = Company.manager(db_with_schema)
    employment_mgr = Employment.manager(db_with_schema)

    alice = Person(name=Name("Alice"))
    bob = Person(name=Name("Bob"))
    techcorp = Company(name=Name("TechCorp"))

    person_mgr.insert_many([alice, bob])
    company_mgr.insert(techcorp)

    employments = [
        Employment(
            employee=alice, employer=techcorp, position=Position("Engineer"), salary=Salary(100000)
        ),
        Employment(
            employee=bob, employer=techcorp, position=Position("Engineer"), salary=Salary(90000)
        ),
        Employment(
            employee=alice, employer=techcorp, position=Position("Manager"), salary=Salary(120000)
        ),
    ]
    employment_mgr.insert_many(employments)

    # Act - Give all engineers a 10% raise using update_with()
    updated = employment_mgr.filter(position="Engineer").update_with(
        lambda emp: setattr(emp, "salary", Salary(int(emp.salary.value * 1.1)))
    )

    # Assert - Check that 2 employments were updated
    expected_updated_count = 2
    assert expected_updated_count == len(updated)

    # Verify the salary updates in database
    all_employments = employment_mgr.all()

    # Find engineers and verify their salaries
    engineers = [e for e in all_employments if e.position.value == "Engineer"]
    assert len(engineers) == 2

    # Check that salaries were increased by 10%
    engineer_salaries = sorted([e.salary.value for e in engineers])
    expected_salaries = sorted([99000, 110000])  # 90000*1.1=99000, 100000*1.1=110000
    assert expected_salaries == engineer_salaries

    # Verify manager salary unchanged
    managers = [e for e in all_employments if e.position.value == "Manager"]
    assert len(managers) == 1
    assert managers[0].salary.value == 120000


@pytest.mark.integration
@pytest.mark.order(165)
def test_relation_query_update_with_complex_function(db_with_schema):
    """Test RelationQuery.update_with() with complex update function."""
    # Arrange
    from type_bridge import Integer

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

    # Setup
    person_mgr = Person.manager(db_with_schema)
    company_mgr = Company.manager(db_with_schema)
    employment_mgr = Employment.manager(db_with_schema)

    alice = Person(name=Name("Alice"))
    techcorp = Company(name=Name("TechCorp"))

    person_mgr.insert(alice)
    company_mgr.insert(techcorp)

    employments = [
        Employment(
            employee=alice, employer=techcorp, position=Position("Engineer"), salary=Salary(100000)
        ),
        Employment(
            employee=alice, employer=techcorp, position=Position("Developer"), salary=Salary(95000)
        ),
    ]
    employment_mgr.insert_many(employments)

    # Define a complex update function that promotes and gives raise
    def promote_to_senior(emp):
        # Add "Senior" prefix to position
        emp.position = Position(f"Senior {emp.position.value}")
        # Give 20% raise
        emp.salary = Salary(int(emp.salary.value * 1.2))

    # Act - Promote all alice's employments to senior
    updated = employment_mgr.filter(employee=alice).update_with(promote_to_senior)

    # Assert
    expected_updated_count = 2
    assert expected_updated_count == len(updated)

    # Verify updates in database
    all_employments = employment_mgr.all()
    assert len(all_employments) == 2

    # Verify positions have "Senior" prefix
    positions = sorted([e.position.value for e in all_employments])
    expected_positions = sorted(["Senior Engineer", "Senior Developer"])
    assert expected_positions == positions

    # Verify salaries were increased by 20%
    salaries = sorted([e.salary.value for e in all_employments])
    expected_salaries = sorted([120000, 114000])  # 100000*1.2=120000, 95000*1.2=114000
    assert expected_salaries == salaries
