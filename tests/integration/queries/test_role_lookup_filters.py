"""Integration tests for role-player lookup filter queries.

Tests Django-style lookup filters on role player attributes:
    Employment.manager(db).filter(employee__age__gt=30)
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


class Name(String):
    pass


class Age(Integer):
    pass


class City(String):
    pass


class Industry(String):
    pass


class Salary(Integer):
    pass


class Position(String):
    pass


class Person(Entity):
    flags = TypeFlags(name="rp_person")
    name: Name = Flag(Key)
    age: Age | None = None
    city: City | None = None


class Company(Entity):
    flags = TypeFlags(name="rp_company")
    name: Name = Flag(Key)
    industry: Industry | None = None


class Employment(Relation):
    flags = TypeFlags(name="rp_employment")
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)
    position: Position | None = None
    salary: Salary | None = None


@pytest.fixture
def setup_employment_data(clean_db):
    """Setup test data for employment relations."""
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    person_manager = Person.manager(clean_db)
    company_manager = Company.manager(clean_db)
    employment_manager = Employment.manager(clean_db)

    # Create persons with different ages and cities
    alice = Person(name=Name("Alice"), age=Age(30), city=City("NYC"))
    bob = Person(name=Name("Bob"), age=Age(25), city=City("LA"))
    charlie = Person(name=Name("Charlie"), age=Age(40), city=City("NYC"))

    # Create companies with different industries
    techcorp = Company(name=Name("TechCorp"), industry=Industry("Technology"))
    finco = Company(name=Name("FinCo"), industry=Industry("Finance"))

    person_manager.insert_many([alice, bob, charlie])
    company_manager.insert_many([techcorp, finco])

    # Create employment relations
    employments = [
        Employment(
            employee=alice,
            employer=techcorp,
            position=Position("Engineer"),
            salary=Salary(100000),
        ),
        Employment(
            employee=bob,
            employer=techcorp,
            position=Position("Designer"),
            salary=Salary(80000),
        ),
        Employment(
            employee=charlie,
            employer=finco,
            position=Position("Analyst"),
            salary=Salary(120000),
        ),
        Employment(
            employee=alice,
            employer=finco,
            position=Position("Consultant"),
            salary=Salary(150000),
        ),
    ]
    employment_manager.insert_many(employments)

    return {
        "db": clean_db,
        "alice": alice,
        "bob": bob,
        "charlie": charlie,
        "techcorp": techcorp,
        "finco": finco,
    }


# ============================================================
# Basic role-player lookup tests
# ============================================================


@pytest.mark.integration
@pytest.mark.order(200)
def test_filter_by_role_player_attribute_exact(setup_employment_data):
    """Test filtering relations by role player attribute exact match."""
    db = setup_employment_data["db"]

    manager = Employment.manager(db)

    # Filter by employee name exact match
    results = manager.filter(employee__name="Alice").execute()

    assert len(results) == 2
    for emp in results:
        assert emp.employee.name.value == "Alice"


@pytest.mark.integration
@pytest.mark.order(201)
def test_filter_by_role_player_attribute_gt(setup_employment_data):
    """Test filtering relations by role player attribute greater than."""
    db = setup_employment_data["db"]

    manager = Employment.manager(db)

    # Filter by employee age > 28
    results = manager.filter(employee__age__gt=28).execute()

    assert len(results) == 3  # Alice (30) x2 and Charlie (40)
    for emp in results:
        assert emp.employee.age is not None
        assert emp.employee.age.value > 28


@pytest.mark.integration
@pytest.mark.order(202)
def test_filter_by_role_player_attribute_gte(setup_employment_data):
    """Test filtering relations by role player attribute greater than or equal."""
    db = setup_employment_data["db"]

    manager = Employment.manager(db)

    # Filter by employee age >= 30
    results = manager.filter(employee__age__gte=30).execute()

    assert len(results) == 3  # Alice (30) x2 and Charlie (40)
    for emp in results:
        assert emp.employee.age is not None
        assert emp.employee.age.value >= 30


@pytest.mark.integration
@pytest.mark.order(203)
def test_filter_by_role_player_attribute_lt(setup_employment_data):
    """Test filtering relations by role player attribute less than."""
    db = setup_employment_data["db"]

    manager = Employment.manager(db)

    # Filter by employee age < 30
    results = manager.filter(employee__age__lt=30).execute()

    assert len(results) == 1  # Bob (25)
    assert results[0].employee.name.value == "Bob"


@pytest.mark.integration
@pytest.mark.order(204)
def test_filter_by_role_player_attribute_lte(setup_employment_data):
    """Test filtering relations by role player attribute less than or equal."""
    db = setup_employment_data["db"]

    manager = Employment.manager(db)

    # Filter by employee age <= 30
    results = manager.filter(employee__age__lte=30).execute()

    assert len(results) == 3  # Alice (30) x2 and Bob (25)


# ============================================================
# String lookup tests
# ============================================================


@pytest.mark.integration
@pytest.mark.order(210)
def test_filter_by_role_player_string_contains(setup_employment_data):
    """Test filtering by role player string attribute contains."""
    db = setup_employment_data["db"]

    manager = Employment.manager(db)

    # Filter by employer name containing "Tech"
    results = manager.filter(employer__name__contains="Tech").execute()

    assert len(results) == 2  # Alice and Bob at TechCorp
    for emp in results:
        assert "Tech" in emp.employer.name.value


@pytest.mark.integration
@pytest.mark.order(211)
def test_filter_by_role_player_string_startswith(setup_employment_data):
    """Test filtering by role player string attribute starts with."""
    db = setup_employment_data["db"]

    manager = Employment.manager(db)

    # Filter by employer name starting with "Fin"
    results = manager.filter(employer__name__startswith="Fin").execute()

    assert len(results) == 2  # Charlie and Alice at FinCo
    for emp in results:
        assert emp.employer.name.value.startswith("Fin")


@pytest.mark.integration
@pytest.mark.order(212)
def test_filter_by_role_player_string_endswith(setup_employment_data):
    """Test filtering by role player string attribute ends with."""
    db = setup_employment_data["db"]

    manager = Employment.manager(db)

    # Filter by employer name ending with "Co"
    results = manager.filter(employer__name__endswith="Co").execute()

    assert len(results) == 2  # FinCo employees
    for emp in results:
        assert emp.employer.name.value.endswith("Co")


# ============================================================
# Combined filter tests
# ============================================================


@pytest.mark.integration
@pytest.mark.order(220)
def test_filter_role_player_and_relation_attribute(setup_employment_data):
    """Test combining role player lookup with relation attribute filter."""
    db = setup_employment_data["db"]

    manager = Employment.manager(db)

    # Filter by employee age > 25 AND salary > 90000
    results = manager.filter(Salary.gt(Salary(90000)), employee__age__gt=25).execute()

    # Alice (30) at TechCorp (100k), Alice (30) at FinCo (150k), Charlie (40) at FinCo (120k)
    assert len(results) == 3
    for emp in results:
        assert emp.employee.age is not None
        assert emp.salary is not None
        assert emp.employee.age.value > 25
        assert emp.salary.value > 90000


@pytest.mark.integration
@pytest.mark.order(221)
def test_filter_multiple_role_players(setup_employment_data):
    """Test filtering by multiple role player attributes."""
    db = setup_employment_data["db"]

    manager = Employment.manager(db)

    # Filter by employee age >= 30 AND employer industry = Technology
    results = manager.filter(employee__age__gte=30, employer__industry="Technology").execute()

    # Only Alice (30) at TechCorp
    assert len(results) == 1
    assert results[0].employee.name.value == "Alice"
    assert results[0].employer.name.value == "TechCorp"


@pytest.mark.integration
@pytest.mark.order(222)
def test_filter_multiple_lookups_same_role(setup_employment_data):
    """Test multiple lookups on the same role (range query)."""
    db = setup_employment_data["db"]

    manager = Employment.manager(db)

    # Filter by employee age between 25 and 35 (exclusive)
    results = manager.filter(employee__age__gt=25, employee__age__lt=35).execute()

    # Alice (30) x2
    assert len(results) == 2
    for emp in results:
        assert emp.employee.age is not None
        assert 25 < emp.employee.age.value < 35


# ============================================================
# Membership and null tests
# ============================================================


@pytest.mark.integration
@pytest.mark.order(230)
def test_filter_role_player_in_list(setup_employment_data):
    """Test filtering by role player attribute in list."""
    db = setup_employment_data["db"]

    manager = Employment.manager(db)

    # Filter by employee name in list
    results = manager.filter(employee__name__in=["Alice", "Charlie"]).execute()

    assert len(results) == 3  # Alice x2 + Charlie
    names = {emp.employee.name.value for emp in results}
    assert names == {"Alice", "Charlie"}


@pytest.mark.integration
@pytest.mark.order(231)
def test_filter_role_player_city_exact(setup_employment_data):
    """Test filtering by role player city attribute."""
    db = setup_employment_data["db"]

    manager = Employment.manager(db)

    # Filter by employee city = NYC
    results = manager.filter(employee__city="NYC").execute()

    # Alice x2 and Charlie are from NYC
    assert len(results) == 3
    for emp in results:
        assert emp.employee.city is not None
        assert emp.employee.city.value == "NYC"


# ============================================================
# Delete with role player filters
# ============================================================


@pytest.mark.integration
@pytest.mark.order(240)
def test_delete_with_role_player_filter(setup_employment_data):
    """Test deleting relations with role player filter."""
    db = setup_employment_data["db"]

    manager = Employment.manager(db)

    # Count before delete
    all_before = manager.all()
    assert len(all_before) == 4

    # Delete employments where employee age < 30
    deleted_count = manager.filter(employee__age__lt=30).delete()

    assert deleted_count == 1  # Only Bob (25)

    # Verify remaining
    all_after = manager.all()
    assert len(all_after) == 3

    # Bob should not be in any remaining employments
    for emp in all_after:
        assert emp.employee.name.value != "Bob"


@pytest.mark.integration
@pytest.mark.order(241)
def test_delete_with_combined_role_player_filters(setup_employment_data):
    """Test deleting with combined role player and expression filters."""
    db = setup_employment_data["db"]

    manager = Employment.manager(db)

    # Delete high salary employments where employee is older than 35
    deleted_count = manager.filter(Salary.gt(Salary(100000)), employee__age__gt=35).delete()

    assert deleted_count == 1  # Only Charlie (40) at FinCo (120k)

    # Verify remaining
    remaining = manager.all()
    assert len(remaining) == 3


# ============================================================
# Error handling tests
# ============================================================


@pytest.mark.integration
@pytest.mark.order(250)
def test_filter_unknown_role_raises_error(setup_employment_data):
    """Test that filtering by unknown role raises ValueError."""
    db = setup_employment_data["db"]

    manager = Employment.manager(db)

    with pytest.raises(ValueError, match="Unknown filter field 'unknown'"):
        manager.filter(unknown__age__gt=30).execute()


@pytest.mark.integration
@pytest.mark.order(251)
def test_filter_unknown_role_attribute_raises_error(setup_employment_data):
    """Test that filtering by unknown role attribute raises ValueError."""
    db = setup_employment_data["db"]

    manager = Employment.manager(db)

    with pytest.raises(ValueError, match="do not have attribute 'unknown'"):
        manager.filter(employee__unknown="value").execute()


@pytest.mark.integration
@pytest.mark.order(252)
def test_filter_string_lookup_on_integer_raises_error(setup_employment_data):
    """Test that string lookup on integer attribute raises ValueError."""
    db = setup_employment_data["db"]

    manager = Employment.manager(db)

    with pytest.raises(ValueError, match="Unsupported lookup operator 'contains' for Age"):
        manager.filter(employee__age__contains="30").execute()
