"""Integration tests for chainable RelationQuery operations (delete and update_with)."""

import pytest

from type_bridge import (
    Card,
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
@pytest.mark.order(170)
def test_chainable_delete_with_expression_filter(clean_db):
    """Test RelationQuery.delete() with expression-based filters."""

    # Arrange
    class Name(String):
        pass

    class Salary(Integer):
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
        salary: Salary

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    # Insert test data
    person_mgr = Person.manager(clean_db)
    company_mgr = Company.manager(clean_db)
    employment_mgr = Employment.manager(clean_db)

    alice = Person(name=Name("Alice"))
    bob = Person(name=Name("Bob"))
    techcorp = Company(name=Name("TechCorp"))

    person_mgr.insert_many([alice, bob])
    company_mgr.insert(techcorp)

    employments = [
        Employment(
            employee=alice,
            employer=techcorp,
            position=Position("Engineer"),
            salary=Salary(80000),
        ),
        Employment(
            employee=bob,
            employer=techcorp,
            position=Position("Engineer"),
            salary=Salary(150000),
        ),
        Employment(
            employee=alice,
            employer=techcorp,
            position=Position("Manager"),
            salary=Salary(180000),
        ),
    ]
    employment_mgr.insert_many(employments)

    # Verify initial state
    all_employments = employment_mgr.all()
    expected_count = 3
    actual_count = len(all_employments)
    assert expected_count == actual_count

    # Act - Delete employments with salary > 100000 using chainable filter
    deleted_count = employment_mgr.filter(Salary.gt(Salary(100000))).delete()

    # Assert
    expected_deleted = 2  # Bob's employment and Alice's Manager position
    assert expected_deleted == deleted_count

    # Verify remaining employments
    remaining = employment_mgr.all()
    expected_remaining = 1
    actual_remaining = len(remaining)
    assert expected_remaining == actual_remaining

    # Verify only low salary employment remains
    for employment in remaining:
        assert employment.salary.value <= 100000


@pytest.mark.integration
@pytest.mark.order(171)
def test_chainable_delete_with_multiple_filters(clean_db):
    """Test RelationQuery.delete() with multiple expression filters."""

    # Arrange
    class Name(String):
        pass

    class Salary(Integer):
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
        salary: Salary

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    # Insert test data
    person_mgr = Person.manager(clean_db)
    company_mgr = Company.manager(clean_db)
    employment_mgr = Employment.manager(clean_db)

    charlie = Person(name=Name("Charlie"))
    diana = Person(name=Name("Diana"))
    techcorp = Company(name=Name("TechCorp"))

    person_mgr.insert_many([charlie, diana])
    company_mgr.insert(techcorp)

    employments = [
        Employment(
            employee=charlie,
            employer=techcorp,
            position=Position("Intern"),
            salary=Salary(25000),
        ),
        Employment(
            employee=diana,
            employer=techcorp,
            position=Position("Intern"),
            salary=Salary(28000),
        ),
        Employment(
            employee=charlie,
            employer=techcorp,
            position=Position("Engineer"),
            salary=Salary(90000),
        ),
    ]
    employment_mgr.insert_many(employments)

    # Act - Delete low-paid interns (salary < 30000 AND position = "Intern")
    deleted_count = employment_mgr.filter(
        Salary.lt(Salary(30000)), Position.eq(Position("Intern"))
    ).delete()

    # Assert
    expected_deleted = 2  # Both intern positions
    assert expected_deleted == deleted_count

    # Verify remaining
    remaining = employment_mgr.all()
    expected_remaining = 1  # Charlie's Engineer position
    assert expected_remaining == len(remaining)

    # Verify Engineer position remains
    assert remaining[0].position.value == "Engineer"
    assert remaining[0].salary.value == 90000


@pytest.mark.integration
@pytest.mark.order(172)
def test_chainable_delete_returns_zero_for_no_matches(clean_db):
    """Test RelationQuery.delete() returns 0 when no relations match."""

    # Arrange
    class Name(String):
        pass

    class Salary(Integer):
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
        salary: Salary

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    # Insert test data
    person_mgr = Person.manager(clean_db)
    company_mgr = Company.manager(clean_db)
    employment_mgr = Employment.manager(clean_db)

    eve = Person(name=Name("Eve"))
    techcorp = Company(name=Name("TechCorp"))

    person_mgr.insert(eve)
    company_mgr.insert(techcorp)

    employment = Employment(
        employee=eve,
        employer=techcorp,
        position=Position("Engineer"),
        salary=Salary(80000),
    )
    employment_mgr.insert(employment)

    # Act - Delete employments with salary > 1000000 (none exist)
    deleted_count = employment_mgr.filter(Salary.gt(Salary(1000000))).delete()

    # Assert
    expected = 0
    assert expected == deleted_count

    # Verify employment still exists
    remaining = employment_mgr.all()
    assert len(remaining) == 1
    assert remaining[0].salary.value == 80000


@pytest.mark.integration
@pytest.mark.order(173)
def test_chainable_delete_with_range_filter(clean_db):
    """Test RelationQuery.delete() with range filters (gte and lt)."""

    # Arrange
    class Name(String):
        pass

    class Salary(Integer):
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
        salary: Salary

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    # Insert test data
    person_mgr = Person.manager(clean_db)
    company_mgr = Company.manager(clean_db)
    employment_mgr = Employment.manager(clean_db)

    frank = Person(name=Name("Frank"))
    grace = Person(name=Name("Grace"))
    henry = Person(name=Name("Henry"))
    techcorp = Company(name=Name("TechCorp"))

    person_mgr.insert_many([frank, grace, henry])
    company_mgr.insert(techcorp)

    # Salaries: 70000, 85000, 95000, 110000, 130000
    employments = [
        Employment(
            employee=frank,
            employer=techcorp,
            position=Position("Junior"),
            salary=Salary(70000),
        ),
        Employment(
            employee=grace,
            employer=techcorp,
            position=Position("Mid"),
            salary=Salary(85000),
        ),
        Employment(
            employee=henry,
            employer=techcorp,
            position=Position("Mid"),
            salary=Salary(95000),
        ),
        Employment(
            employee=frank,
            employer=techcorp,
            position=Position("Senior"),
            salary=Salary(110000),
        ),
        Employment(
            employee=grace,
            employer=techcorp,
            position=Position("Lead"),
            salary=Salary(130000),
        ),
    ]
    employment_mgr.insert_many(employments)

    # Act - Delete employments in range [80000, 100000) - i.e., >= 80000 and < 100000
    deleted_count = employment_mgr.filter(
        Salary.gte(Salary(80000)), Salary.lt(Salary(100000))
    ).delete()

    # Assert - Should delete Grace's Mid (85000) and Henry's Mid (95000)
    expected_deleted = 2
    assert expected_deleted == deleted_count

    # Verify remaining employments
    remaining = employment_mgr.all()
    expected_remaining = 3  # Frank's Junior (70000), Frank's Senior (110000), Grace's Lead (130000)
    assert expected_remaining == len(remaining)

    # Verify correct salaries remain
    remaining_salaries = {emp.salary.value for emp in remaining}
    expected_salaries = {70000, 110000, 130000}
    assert expected_salaries == remaining_salaries


@pytest.mark.integration
@pytest.mark.order(174)
def test_update_with_lambda_increments_salary(clean_db):
    """Test RelationQuery.update_with() using lambda to increment salary."""

    # Arrange
    class Name(String):
        pass

    class Salary(Integer):
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
        salary: Salary

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    # Insert test data
    person_mgr = Person.manager(clean_db)
    company_mgr = Company.manager(clean_db)
    employment_mgr = Employment.manager(clean_db)

    iris = Person(name=Name("Iris"))
    jack = Person(name=Name("Jack"))
    techcorp = Company(name=Name("TechCorp"))

    person_mgr.insert_many([iris, jack])
    company_mgr.insert(techcorp)

    employments = [
        Employment(
            employee=iris,
            employer=techcorp,
            position=Position("Engineer"),
            salary=Salary(90000),
        ),
        Employment(
            employee=jack,
            employer=techcorp,
            position=Position("Engineer"),
            salary=Salary(110000),
        ),
        Employment(
            employee=iris,
            employer=techcorp,
            position=Position("Manager"),
            salary=Salary(85000),
        ),
    ]
    employment_mgr.insert_many(employments)

    # Act - Give 5% raise to salaries > 95000
    updated = employment_mgr.filter(Salary.gt(Salary(95000))).update_with(
        lambda emp: setattr(emp, "salary", Salary(int(emp.salary.value * 1.05)))
    )

    # Assert - Should update Jack's employment
    expected_count = 1
    actual_count = len(updated)
    assert expected_count == actual_count

    # Verify updates persisted
    all_employments = employment_mgr.all()
    salaries = {emp.salary.value for emp in all_employments}

    # Jack's salary increased: 110000 * 1.05 = 115500
    # Others unchanged: 90000, 85000
    expected_salaries = {90000, 115500, 85000}
    assert expected_salaries == salaries


@pytest.mark.integration
@pytest.mark.order(175)
def test_update_with_function_modifies_position(clean_db):
    """Test RelationQuery.update_with() using named function."""

    # Arrange
    class Name(String):
        pass

    class Salary(Integer):
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
        salary: Salary

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    # Insert test data
    person_mgr = Person.manager(clean_db)
    company_mgr = Company.manager(clean_db)
    employment_mgr = Employment.manager(clean_db)

    kate = Person(name=Name("Kate"))
    leo = Person(name=Name("Leo"))
    techcorp = Company(name=Name("TechCorp"))

    person_mgr.insert_many([kate, leo])
    company_mgr.insert(techcorp)

    employments = [
        Employment(
            employee=kate,
            employer=techcorp,
            position=Position("Engineer"),
            salary=Salary(90000),
        ),
        Employment(
            employee=leo,
            employer=techcorp,
            position=Position("Engineer"),
            salary=Salary(95000),
        ),
        Employment(
            employee=kate,
            employer=techcorp,
            position=Position("Manager"),
            salary=Salary(120000),
        ),
    ]
    employment_mgr.insert_many(employments)

    # Define update function
    def promote_to_senior(employment):
        """Promote engineer to senior engineer."""
        employment.position = Position("Senior " + employment.position.value)

    # Act - Promote all engineers
    updated = employment_mgr.filter(Position.eq(Position("Engineer"))).update_with(
        promote_to_senior
    )

    # Assert
    expected_count = 2  # Kate and Leo's Engineer positions
    assert expected_count == len(updated)

    # Verify updates persisted
    all_employments = employment_mgr.all()
    positions = {emp.position.value for emp in all_employments}

    expected_positions = {"Senior Engineer", "Manager"}
    assert expected_positions == positions


@pytest.mark.integration
@pytest.mark.order(176)
def test_update_with_returns_empty_list_for_no_matches(clean_db):
    """Test RelationQuery.update_with() returns empty list when no relations match."""

    # Arrange
    class Name(String):
        pass

    class Salary(Integer):
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
        salary: Salary

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    # Insert test data
    person_mgr = Person.manager(clean_db)
    company_mgr = Company.manager(clean_db)
    employment_mgr = Employment.manager(clean_db)

    mia = Person(name=Name("Mia"))
    techcorp = Company(name=Name("TechCorp"))

    person_mgr.insert(mia)
    company_mgr.insert(techcorp)

    employment = Employment(
        employee=mia,
        employer=techcorp,
        position=Position("Engineer"),
        salary=Salary(80000),
    )
    employment_mgr.insert(employment)

    # Act - Update employments with salary > 1000000 (none exist)
    updated = employment_mgr.filter(Salary.gt(Salary(1000000))).update_with(
        lambda emp: setattr(emp, "salary", Salary(999999))
    )

    # Assert
    expected = []
    assert expected == updated
    assert len(updated) == 0

    # Verify employment unchanged
    remaining = employment_mgr.all()
    assert len(remaining) == 1
    assert remaining[0].salary.value == 80000


@pytest.mark.integration
@pytest.mark.order(177)
def test_update_with_complex_function_multiple_attributes(clean_db):
    """Test RelationQuery.update_with() modifying multiple attributes."""

    # Arrange
    class Name(String):
        pass

    class Salary(Integer):
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
        salary: Salary

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    # Insert test data
    person_mgr = Person.manager(clean_db)
    company_mgr = Company.manager(clean_db)
    employment_mgr = Employment.manager(clean_db)

    noah = Person(name=Name("Noah"))
    oliver = Person(name=Name("Oliver"))
    techcorp = Company(name=Name("TechCorp"))

    person_mgr.insert_many([noah, oliver])
    company_mgr.insert(techcorp)

    employments = [
        Employment(
            employee=noah,
            employer=techcorp,
            position=Position("Engineer"),
            salary=Salary(100000),
        ),
        Employment(
            employee=oliver,
            employer=techcorp,
            position=Position("Engineer"),
            salary=Salary(95000),
        ),
        Employment(
            employee=noah,
            employer=techcorp,
            position=Position("Manager"),
            salary=Salary(85000),
        ),
    ]
    employment_mgr.insert_many(employments)

    # Define complex update function
    def promote_and_raise(employment):
        """Promote to senior and give 20% raise."""
        employment.position = Position("Senior " + employment.position.value)
        if employment.salary:
            employment.salary = Salary(int(employment.salary.value * 1.2))

    # Act - Promote engineers with salary >= 95000
    updated = employment_mgr.filter(
        Position.eq(Position("Engineer")), Salary.gte(Salary(95000))
    ).update_with(promote_and_raise)

    # Assert
    expected_count = 2  # Noah and Oliver's Engineer positions
    assert expected_count == len(updated)

    # Verify updates persisted
    all_employments = employment_mgr.all()

    # Find the updated employments
    senior_engineers = [emp for emp in all_employments if emp.position.value == "Senior Engineer"]
    assert len(senior_engineers) == 2

    # Noah's Engineer: 100000 * 1.2 = 120000
    # Oliver's Engineer: 95000 * 1.2 = 114000
    senior_salaries = {emp.salary.value for emp in senior_engineers}
    expected_salaries = {120000, 114000}
    assert expected_salaries == senior_salaries

    # Noah's Manager position should be unchanged
    managers = [emp for emp in all_employments if "Manager" in emp.position.value]
    assert len(managers) == 1
    assert managers[0].salary.value == 85000  # Unchanged


@pytest.mark.integration
@pytest.mark.order(178)
def test_update_with_atomic_transaction(clean_db):
    """Test RelationQuery.update_with() uses atomic transaction (all or nothing)."""

    # Arrange
    class Name(String):
        pass

    class Salary(Integer):
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
        salary: Salary

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    # Insert test data
    person_mgr = Person.manager(clean_db)
    company_mgr = Company.manager(clean_db)
    employment_mgr = Employment.manager(clean_db)

    penny = Person(name=Name("Penny"))
    quinn = Person(name=Name("Quinn"))
    techcorp = Company(name=Name("TechCorp"))

    person_mgr.insert_many([penny, quinn])
    company_mgr.insert(techcorp)

    employments = [
        Employment(
            employee=penny,
            employer=techcorp,
            position=Position("Engineer"),
            salary=Salary(90000),
        ),
        Employment(
            employee=quinn,
            employer=techcorp,
            position=Position("Engineer"),
            salary=Salary(95000),
        ),
    ]
    employment_mgr.insert_many(employments)

    # Define function that will fail on second relation
    call_count = [0]

    def failing_update(employment):
        """Update function that fails on second call."""
        call_count[0] += 1
        if call_count[0] == 2:
            raise ValueError("Intentional failure on second employment")
        employment.salary = Salary(employment.salary.value + 5000)

    # Act & Assert - Should raise error and not update any relations
    with pytest.raises(ValueError) as exc_info:
        employment_mgr.filter(Position.eq(Position("Engineer"))).update_with(failing_update)

    assert "Intentional failure" in str(exc_info.value)

    # Verify no updates persisted (atomic transaction rolled back)
    all_employments = employment_mgr.all()
    salaries = {emp.salary.value for emp in all_employments}

    # Both salaries should be unchanged
    expected_salaries = {90000, 95000}
    assert expected_salaries == salaries


@pytest.mark.integration
@pytest.mark.order(179)
def test_update_with_optional_attribute(clean_db):
    """Test RelationQuery.update_with() with optional attributes."""

    # Arrange
    class Name(String):
        pass

    class Salary(Integer):
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
        salary: Salary
        notes: Notes | None = None

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    # Insert test data
    person_mgr = Person.manager(clean_db)
    company_mgr = Company.manager(clean_db)
    employment_mgr = Employment.manager(clean_db)

    rose = Person(name=Name("Rose"))
    sam = Person(name=Name("Sam"))
    techcorp = Company(name=Name("TechCorp"))

    person_mgr.insert_many([rose, sam])
    company_mgr.insert(techcorp)

    employments = [
        Employment(
            employee=rose,
            employer=techcorp,
            position=Position("Engineer"),
            salary=Salary(90000),
            notes=None,  # No notes initially
        ),
        Employment(
            employee=sam,
            employer=techcorp,
            position=Position("Engineer"),
            salary=Salary(95000),
            notes=Notes("High performer"),
        ),
    ]
    employment_mgr.insert_many(employments)

    # Define update function to add notes
    def add_promotion_note(employment):
        """Add promotion note to employment."""
        employment.notes = Notes("Eligible for promotion")

    # Act - Add notes to all engineers
    updated = employment_mgr.filter(Position.eq(Position("Engineer"))).update_with(
        add_promotion_note
    )

    # Assert
    expected_count = 2
    assert expected_count == len(updated)

    # Verify all employments now have the new notes
    all_employments = employment_mgr.all()
    for emp in all_employments:
        assert emp.notes is not None
        assert emp.notes.value == "Eligible for promotion"


@pytest.mark.integration
@pytest.mark.order(180)
def test_update_with_multi_value_attribute(clean_db):
    """Test RelationQuery.update_with() with multi-value list attributes."""

    # Arrange
    class Name(String):
        pass

    class Salary(Integer):
        pass

    class Position(String):
        pass

    class Tag(String):
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
        tags: list[Tag] = Flag(Card(min=0))

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    # Insert test data
    person_mgr = Person.manager(clean_db)
    company_mgr = Company.manager(clean_db)
    employment_mgr = Employment.manager(clean_db)

    tina = Person(name=Name("Tina"))
    uma = Person(name=Name("Uma"))
    techcorp = Company(name=Name("TechCorp"))

    person_mgr.insert_many([tina, uma])
    company_mgr.insert(techcorp)

    employments = [
        Employment(
            employee=tina,
            employer=techcorp,
            position=Position("Engineer"),
            salary=Salary(100000),
            tags=[Tag("python")],
        ),
        Employment(
            employee=uma,
            employer=techcorp,
            position=Position("Engineer"),
            salary=Salary(110000),
            tags=[Tag("java"), Tag("kotlin")],
        ),
    ]
    employment_mgr.insert_many(employments)

    # Define update function to add "senior" tag
    def add_senior_tag(employment):
        """Add senior tag to employment."""
        current_tags = list(employment.tags) if employment.tags else []
        current_tags.append(Tag("senior"))
        employment.tags = current_tags

    # Act - Add senior tag to high salary engineers
    updated = employment_mgr.filter(
        Position.eq(Position("Engineer")), Salary.gte(Salary(105000))
    ).update_with(add_senior_tag)

    # Assert
    expected_count = 1  # Only Uma
    assert expected_count == len(updated)

    # Verify Uma's employment has senior tag
    all_employments = employment_mgr.all()
    for emp in all_employments:
        if emp.salary.value >= 105000:
            tag_values = {tag.value for tag in emp.tags}
            assert "senior" in tag_values
            assert len(emp.tags) == 3  # java, kotlin, senior
        else:
            tag_values = {tag.value for tag in emp.tags}
            assert "senior" not in tag_values


@pytest.mark.integration
@pytest.mark.order(181)
def test_update_with_preserves_role_players(clean_db):
    """Test RelationQuery.update_with() preserves role players."""

    # Arrange
    class Name(String):
        pass

    class Salary(Integer):
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
        salary: Salary

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    # Insert test data
    person_mgr = Person.manager(clean_db)
    company_mgr = Company.manager(clean_db)
    employment_mgr = Employment.manager(clean_db)

    victor = Person(name=Name("Victor"))
    techcorp = Company(name=Name("TechCorp"))

    person_mgr.insert(victor)
    company_mgr.insert(techcorp)

    employment = Employment(
        employee=victor,
        employer=techcorp,
        position=Position("Engineer"),
        salary=Salary(90000),
    )
    employment_mgr.insert(employment)

    # Act - Update salary
    updated = employment_mgr.filter(Position.eq(Position("Engineer"))).update_with(
        lambda emp: setattr(emp, "salary", Salary(95000))
    )

    # Assert
    assert len(updated) == 1

    # Verify role players preserved
    all_employments = employment_mgr.all()
    assert len(all_employments) == 1

    emp = all_employments[0]
    assert emp.employee.name.value == "Victor"  # Preserved
    assert emp.employer.name.value == "TechCorp"  # Preserved
    assert emp.salary.value == 95000  # Updated


@pytest.mark.integration
@pytest.mark.order(182)
def test_delete_with_multiple_role_players(clean_db):
    """Test RelationQuery.delete() with relations having 3+ role players."""

    # Arrange
    class Name(String):
        pass

    class Amount(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    class Transaction(Relation):
        flags = TypeFlags(name="transaction")
        sender: Role[Person] = Role("sender", Person)
        receiver: Role[Person] = Role("receiver", Person)
        approver: Role[Person] = Role("approver", Person)
        amount: Amount

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, Transaction)
    schema_manager.sync_schema(force=True)

    # Insert test data
    person_mgr = Person.manager(clean_db)
    transaction_mgr = Transaction.manager(clean_db)

    wendy = Person(name=Name("Wendy"))
    xavier = Person(name=Name("Xavier"))
    yara = Person(name=Name("Yara"))

    person_mgr.insert_many([wendy, xavier, yara])

    transactions = [
        Transaction(sender=wendy, receiver=xavier, approver=yara, amount=Amount(1000)),
        Transaction(sender=xavier, receiver=wendy, approver=yara, amount=Amount(5000)),
        Transaction(sender=yara, receiver=xavier, approver=wendy, amount=Amount(500)),
    ]
    transaction_mgr.insert_many(transactions)

    # Act - Delete large transactions (amount > 2000)
    deleted_count = transaction_mgr.filter(Amount.gt(Amount(2000))).delete()

    # Assert
    expected_deleted = 1  # Xavier -> Wendy transaction (5000)
    assert expected_deleted == deleted_count

    # Verify remaining transactions
    remaining = transaction_mgr.all()
    assert len(remaining) == 2

    amounts = {txn.amount.value for txn in remaining}
    expected_amounts = {1000, 500}
    assert expected_amounts == amounts
