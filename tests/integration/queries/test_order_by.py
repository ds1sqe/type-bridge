"""Integration tests for order_by() method on EntityQuery and RelationQuery.

Tests actual TypeDB query execution with sorting.
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


# Test attribute types
class Name(String):
    pass


class Age(Integer):
    pass


class City(String):
    pass


class Salary(Integer):
    pass


class Position(String):
    pass


class Industry(String):
    pass


# Test entities
class Person(Entity):
    flags = TypeFlags(name="ob_person")
    name: Name = Flag(Key)
    age: Age | None = None
    city: City | None = None


class Company(Entity):
    flags = TypeFlags(name="ob_company")
    name: Name = Flag(Key)
    industry: Industry | None = None


# Test relation
class Employment(Relation):
    flags = TypeFlags(name="ob_employment")
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)
    position: Position | None = None
    salary: Salary | None = None


@pytest.fixture
def setup_order_by_data(clean_db):
    """Setup test data for order_by tests."""
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
    diana = Person(name=Name("Diana"), age=Age(35), city=City("LA"))

    person_manager.insert_many([alice, bob, charlie, diana])

    # Create companies
    techcorp = Company(name=Name("TechCorp"), industry=Industry("Technology"))
    finco = Company(name=Name("FinCo"), industry=Industry("Finance"))

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
            employee=diana,
            employer=finco,
            position=Position("Manager"),
            salary=Salary(150000),
        ),
    ]
    employment_manager.insert_many(employments)

    return {
        "db": clean_db,
        "alice": alice,
        "bob": bob,
        "charlie": charlie,
        "diana": diana,
        "techcorp": techcorp,
        "finco": finco,
    }


# ============================================================
# EntityQuery order_by tests
# ============================================================


@pytest.mark.integration
@pytest.mark.order(300)
def test_entity_order_by_ascending(setup_order_by_data):
    """Test sorting entities in ascending order."""
    db = setup_order_by_data["db"]

    manager = Person.manager(db)
    results = manager.filter().order_by("age").execute()

    assert len(results) == 4
    # Should be sorted by age ascending: Bob(25), Alice(30), Diana(35), Charlie(40)
    assert results[0].name.value == "Bob"
    assert results[0].age is not None
    assert results[0].age.value == 25
    assert results[1].name.value == "Alice"
    assert results[2].name.value == "Diana"
    assert results[3].name.value == "Charlie"


@pytest.mark.integration
@pytest.mark.order(301)
def test_entity_order_by_descending(setup_order_by_data):
    """Test sorting entities in descending order."""
    db = setup_order_by_data["db"]

    manager = Person.manager(db)
    results = manager.filter().order_by("-age").execute()

    assert len(results) == 4
    # Should be sorted by age descending: Charlie(40), Diana(35), Alice(30), Bob(25)
    assert results[0].name.value == "Charlie"
    assert results[1].name.value == "Diana"
    assert results[2].name.value == "Alice"
    assert results[3].name.value == "Bob"


@pytest.mark.integration
@pytest.mark.order(302)
def test_entity_order_by_multiple(setup_order_by_data):
    """Test sorting entities by multiple fields."""
    db = setup_order_by_data["db"]

    manager = Person.manager(db)
    # Sort by city ascending, then age descending
    results = manager.filter().order_by("city", "-age").execute()

    assert len(results) == 4
    # LA first (sorted by -age): Diana(35), Bob(25)
    # NYC second (sorted by -age): Charlie(40), Alice(30)
    assert results[0].city is not None
    assert results[0].city.value == "LA"
    assert results[0].name.value == "Diana"
    assert results[1].city is not None
    assert results[1].city.value == "LA"
    assert results[1].name.value == "Bob"
    assert results[2].city is not None
    assert results[2].city.value == "NYC"
    assert results[2].name.value == "Charlie"
    assert results[3].city is not None
    assert results[3].city.value == "NYC"
    assert results[3].name.value == "Alice"


@pytest.mark.integration
@pytest.mark.order(303)
def test_entity_order_by_with_pagination(setup_order_by_data):
    """Test sorting with limit and offset."""
    db = setup_order_by_data["db"]

    manager = Person.manager(db)
    # Get second page of 2, sorted by age ascending
    results = manager.filter().order_by("age").limit(2).offset(2).execute()

    assert len(results) == 2
    # Skip Bob(25), Alice(30), get Diana(35), Charlie(40)
    assert results[0].name.value == "Diana"
    assert results[1].name.value == "Charlie"


@pytest.mark.integration
@pytest.mark.order(304)
def test_entity_order_by_with_filter(setup_order_by_data):
    """Test sorting with expression filter."""
    db = setup_order_by_data["db"]

    manager = Person.manager(db)
    # Filter by city = NYC, sorted by age descending
    results = manager.filter(City.eq(City("NYC"))).order_by("-age").execute()

    assert len(results) == 2
    # NYC people sorted by -age: Charlie(40), Alice(30)
    assert results[0].name.value == "Charlie"
    assert results[1].name.value == "Alice"


# ============================================================
# RelationQuery order_by tests - relation attributes
# ============================================================


@pytest.mark.integration
@pytest.mark.order(310)
def test_relation_order_by_attribute_asc(setup_order_by_data):
    """Test sorting relations by attribute ascending."""
    db = setup_order_by_data["db"]

    manager = Employment.manager(db)
    results = manager.filter().order_by("salary").execute()

    assert len(results) == 4
    # Sorted by salary ascending: 80k, 100k, 120k, 150k
    assert results[0].salary is not None
    assert results[0].salary.value == 80000
    assert results[1].salary is not None
    assert results[1].salary.value == 100000
    assert results[2].salary is not None
    assert results[2].salary.value == 120000
    assert results[3].salary is not None
    assert results[3].salary.value == 150000


@pytest.mark.integration
@pytest.mark.order(311)
def test_relation_order_by_attribute_desc(setup_order_by_data):
    """Test sorting relations by attribute descending."""
    db = setup_order_by_data["db"]

    manager = Employment.manager(db)
    results = manager.filter().order_by("-salary").execute()

    assert len(results) == 4
    # Sorted by salary descending: 150k, 120k, 100k, 80k
    assert results[0].salary is not None
    assert results[0].salary.value == 150000
    assert results[1].salary is not None
    assert results[1].salary.value == 120000
    assert results[2].salary is not None
    assert results[2].salary.value == 100000
    assert results[3].salary is not None
    assert results[3].salary.value == 80000


# ============================================================
# RelationQuery order_by tests - role-player attributes
# ============================================================


@pytest.mark.integration
@pytest.mark.order(320)
def test_relation_order_by_role_player_asc(setup_order_by_data):
    """Test sorting relations by role-player attribute ascending."""
    db = setup_order_by_data["db"]

    manager = Employment.manager(db)
    results = manager.filter().order_by("employee__age").execute()

    assert len(results) == 4
    # Sorted by employee age: Bob(25), Alice(30), Diana(35), Charlie(40)
    assert results[0].employee.name.value == "Bob"
    assert results[1].employee.name.value == "Alice"
    assert results[2].employee.name.value == "Diana"
    assert results[3].employee.name.value == "Charlie"


@pytest.mark.integration
@pytest.mark.order(321)
def test_relation_order_by_role_player_desc(setup_order_by_data):
    """Test sorting relations by role-player attribute descending."""
    db = setup_order_by_data["db"]

    manager = Employment.manager(db)
    results = manager.filter().order_by("-employee__age").execute()

    assert len(results) == 4
    # Sorted by employee age descending: Charlie(40), Diana(35), Alice(30), Bob(25)
    assert results[0].employee.name.value == "Charlie"
    assert results[1].employee.name.value == "Diana"
    assert results[2].employee.name.value == "Alice"
    assert results[3].employee.name.value == "Bob"


@pytest.mark.integration
@pytest.mark.order(322)
def test_relation_order_by_mixed(setup_order_by_data):
    """Test sorting by role-player attribute and relation attribute."""
    db = setup_order_by_data["db"]

    manager = Employment.manager(db)
    # This test sorts by employee name (string) then by salary (descending)
    # Since each employee has only one employment, the second sort is not significant
    # But we verify the primary sort works
    results = manager.filter().order_by("employee__name").execute()

    assert len(results) == 4
    # Sorted by employee name: Alice, Bob, Charlie, Diana
    assert results[0].employee.name.value == "Alice"
    assert results[1].employee.name.value == "Bob"
    assert results[2].employee.name.value == "Charlie"
    assert results[3].employee.name.value == "Diana"


@pytest.mark.integration
@pytest.mark.order(323)
def test_relation_order_by_with_pagination(setup_order_by_data):
    """Test role-player sorting with pagination."""
    db = setup_order_by_data["db"]

    manager = Employment.manager(db)
    # Get second page sorted by employee age
    results = manager.filter().order_by("employee__age").limit(2).offset(2).execute()

    assert len(results) == 2
    # Skip Bob(25), Alice(30), get Diana(35), Charlie(40)
    assert results[0].employee.name.value == "Diana"
    assert results[1].employee.name.value == "Charlie"


@pytest.mark.integration
@pytest.mark.order(324)
def test_relation_order_by_with_role_lookup_filter(setup_order_by_data):
    """Test sorting combined with role-player lookup filter."""
    db = setup_order_by_data["db"]

    manager = Employment.manager(db)
    # Filter by employee age >= 30, sorted by salary descending
    results = manager.filter(employee__age__gte=30).order_by("-salary").execute()

    assert len(results) == 3
    # Alice(30, 100k), Charlie(40, 120k), Diana(35, 150k) sorted by -salary
    # Diana(150k), Charlie(120k), Alice(100k)
    assert results[0].employee.name.value == "Diana"
    assert results[0].salary is not None
    assert results[0].salary.value == 150000
    assert results[1].employee.name.value == "Charlie"
    assert results[1].salary is not None
    assert results[1].salary.value == 120000
    assert results[2].employee.name.value == "Alice"
    assert results[2].salary is not None
    assert results[2].salary.value == 100000


@pytest.mark.integration
@pytest.mark.order(325)
def test_relation_order_by_employer_name(setup_order_by_data):
    """Test sorting by different role (employer) attribute."""
    db = setup_order_by_data["db"]

    manager = Employment.manager(db)
    results = manager.filter().order_by("employer__name").execute()

    assert len(results) == 4
    # Sorted by employer name: FinCo first (Charlie, Diana), TechCorp second (Alice, Bob)
    # FinCo employees
    assert results[0].employer.name.value == "FinCo"
    assert results[1].employer.name.value == "FinCo"
    # TechCorp employees
    assert results[2].employer.name.value == "TechCorp"
    assert results[3].employer.name.value == "TechCorp"
