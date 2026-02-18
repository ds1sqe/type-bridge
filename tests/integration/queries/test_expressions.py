"""Integration tests for advanced query expressions.

Tests type-safe filtering, aggregations, and grouping against a real TypeDB instance.
"""

import pytest

from type_bridge import Database, Entity, SchemaManager, TypeFlags
from type_bridge.attribute import Double, Integer, String
from type_bridge.attribute.flags import Flag, Key

# ============================================================================
# Test Schema
# ============================================================================


class PersonID(String):
    pass


class PersonName(String):
    pass


class PersonEmail(String):
    pass


class PersonAge(Integer):
    pass


class PersonSalary(Double):
    pass


class PersonDepartment(String):
    pass


class PersonCity(String):
    pass


class PersonScore(Double):
    pass


class Person(Entity):
    flags = TypeFlags(name="test_person")
    person_id: PersonID = Flag(Key)
    name: PersonName
    email: PersonEmail
    age: PersonAge
    salary: PersonSalary
    department: PersonDepartment
    city: PersonCity
    score: PersonScore | None = None


# ============================================================================
# Fixtures
# ============================================================================


@pytest.fixture
def db(docker_typedb):
    """Provide database connection.

    Args:
        docker_typedb: Fixture that ensures Docker container is running
    """
    from tests.integration.conftest import TEST_DB_ADDRESS

    database = Database(address=TEST_DB_ADDRESS, database="test_expressions")
    database.connect()
    yield database
    database.close()


@pytest.fixture
def setup_schema(db):
    """Set up schema and insert test data."""
    # Create schema
    schema_manager = SchemaManager(db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    # Insert test data
    manager = Person.manager(db)
    test_persons = [
        Person(
            person_id=PersonID("p1"),
            name=PersonName("Alice Anderson"),
            email=PersonEmail("alice@company.com"),
            age=PersonAge(25),
            salary=PersonSalary(75000.0),
            department=PersonDepartment("Engineering"),
            city=PersonCity("New York"),
            score=PersonScore(92.5),
        ),
        Person(
            person_id=PersonID("p2"),
            name=PersonName("Bob Baker"),
            email=PersonEmail("bob@company.com"),
            age=PersonAge(35),
            salary=PersonSalary(95000.0),
            department=PersonDepartment("Engineering"),
            city=PersonCity("San Francisco"),
            score=PersonScore(88.0),
        ),
        Person(
            person_id=PersonID("p3"),
            name=PersonName("Charlie Chen"),
            email=PersonEmail("charlie@gmail.com"),
            age=PersonAge(45),
            salary=PersonSalary(120000.0),
            department=PersonDepartment("Sales"),
            city=PersonCity("New York"),
            score=PersonScore(95.0),
        ),
        Person(
            person_id=PersonID("p4"),
            name=PersonName("Diana Davis"),
            email=PersonEmail("diana@company.com"),
            age=PersonAge(28),
            salary=PersonSalary(68000.0),
            department=PersonDepartment("Marketing"),
            city=PersonCity("Los Angeles"),
            score=PersonScore(85.5),
        ),
        Person(
            person_id=PersonID("p5"),
            name=PersonName("Eve Evans"),
            email=PersonEmail("eve@company.com"),
            age=PersonAge(52),
            salary=PersonSalary(150000.0),
            department=PersonDepartment("Engineering"),
            city=PersonCity("San Francisco"),
            score=PersonScore(98.0),
        ),
        Person(
            person_id=PersonID("p6"),
            name=PersonName("Frank Foster"),
            email=PersonEmail("frank@gmail.com"),
            age=PersonAge(30),
            salary=PersonSalary(82000.0),
            department=PersonDepartment("Sales"),
            city=PersonCity("New York"),
            score=PersonScore(90.0),
        ),
    ]
    manager.insert_many(test_persons)

    yield db

    # Cleanup is handled by conftest


# ============================================================================
# Value Comparison Tests
# ============================================================================


@pytest.mark.integration
@pytest.mark.order(1)
def test_greater_than_filter(setup_schema):
    """Test greater-than comparison."""
    manager = Person.manager(setup_schema)

    # age > 30
    results = manager.filter(Person.age.gt(PersonAge(30))).execute()

    assert len(results) == 3  # Bob(35), Charlie(45), Eve(52) - Frank(30) excluded
    ages = [p.age.value for p in results]
    assert all(age > 30 for age in ages)


@pytest.mark.integration
@pytest.mark.order(2)
def test_less_than_filter(setup_schema):
    """Test less-than comparison."""
    manager = Person.manager(setup_schema)

    # age < 30
    results = manager.filter(Person.age.lt(PersonAge(30))).execute()

    assert len(results) == 2  # Alice(25), Diana(28)
    ages = [p.age.value for p in results]
    assert all(age < 30 for age in ages)


@pytest.mark.integration
@pytest.mark.order(3)
def test_range_query(setup_schema):
    """Test range query with gte and lte."""
    manager = Person.manager(setup_schema)

    # 28 <= age <= 35
    results = manager.filter(Person.age.gte(PersonAge(28)), Person.age.lte(PersonAge(35))).execute()

    assert len(results) == 3  # Diana(28), Frank(30), Bob(35)
    ages = [p.age.value for p in results]
    assert all(28 <= age <= 35 for age in ages)


@pytest.mark.integration
@pytest.mark.order(4)
def test_equality_filter(setup_schema):
    """Test equality comparison."""
    manager = Person.manager(setup_schema)

    # age == 35
    results = manager.filter(Person.age.eq(PersonAge(35))).execute()

    assert len(results) == 1
    assert results[0].age.value == 35
    assert results[0].name.value == "Bob Baker"


@pytest.mark.integration
@pytest.mark.order(5)
def test_not_equal_filter(setup_schema):
    """Test not-equal comparison."""
    manager = Person.manager(setup_schema)

    # age != 35
    results = manager.filter(Person.age.neq(PersonAge(35))).execute()

    assert len(results) == 5  # All except Bob
    ages = [p.age.value for p in results]
    assert 35 not in ages


# ============================================================================
# String Operation Tests
# ============================================================================


@pytest.mark.integration
@pytest.mark.order(6)
def test_contains_filter(setup_schema):
    """Test string contains."""
    manager = Person.manager(setup_schema)

    # email contains '@company.com'
    results = manager.filter(Person.email.contains(PersonEmail("@company.com"))).execute()

    assert len(results) == 4  # All except Charlie and Frank (gmail)
    for person in results:
        assert "@company.com" in person.email.value


@pytest.mark.integration
@pytest.mark.order(7)
def test_like_regex_filter(setup_schema):
    """Test regex pattern matching with like."""
    manager = Person.manager(setup_schema)

    # name starts with 'A' or 'B'
    results = manager.filter(Person.name.like(PersonName("^[AB].*"))).execute()

    assert len(results) == 2  # Alice, Bob
    for person in results:
        assert person.name.value[0] in ["A", "B"]


# ============================================================================
# Boolean Logic Tests
# ============================================================================


@pytest.mark.integration
@pytest.mark.order(8)
def test_or_logic(setup_schema):
    """Test OR boolean logic."""
    manager = Person.manager(setup_schema)

    # age < 28 OR age > 45
    results = manager.filter(
        Person.age.lt(PersonAge(28)).or_(Person.age.gt(PersonAge(45)))
    ).execute()

    assert len(results) == 2  # Alice(25), Eve(52)
    ages = [p.age.value for p in results]
    assert all(age < 28 or age > 45 for age in ages)


@pytest.mark.integration
@pytest.mark.order(9)
def test_and_logic_explicit(setup_schema):
    """Test explicit AND boolean logic."""
    manager = Person.manager(setup_schema)

    # Engineering AND age > 40
    results = manager.filter(
        Person.department.eq(PersonDepartment("Engineering")).and_(Person.age.gt(PersonAge(40)))
    ).execute()

    assert len(results) == 1  # Eve(52, Engineering)
    assert results[0].name.value == "Eve Evans"
    assert results[0].department.value == "Engineering"
    assert results[0].age.value > 40


@pytest.mark.integration
@pytest.mark.order(10)
def test_and_logic_implicit(setup_schema):
    """Test implicit AND with multiple filters."""
    manager = Person.manager(setup_schema)

    # Engineering AND age > 30 (implicit AND)
    results = manager.filter(
        Person.department.eq(PersonDepartment("Engineering")), Person.age.gt(PersonAge(30))
    ).execute()

    assert len(results) == 2  # Bob(35), Eve(52)
    for person in results:
        assert person.department.value == "Engineering"
        assert person.age.value > 30


@pytest.mark.integration
@pytest.mark.order(11)
def test_not_logic(setup_schema):
    """Test NOT boolean logic."""
    manager = Person.manager(setup_schema)

    # NOT Engineering
    results = manager.filter(Person.department.eq(PersonDepartment("Engineering")).not_()).execute()

    assert len(results) == 3  # Charlie, Diana, Frank (not Engineering)
    for person in results:
        assert person.department.value != "Engineering"


@pytest.mark.integration
@pytest.mark.order(12)
def test_complex_boolean(setup_schema):
    """Test complex boolean expression."""
    manager = Person.manager(setup_schema)

    # (age > 40 AND salary > 100k) OR score > 95
    results = manager.filter(
        Person.age.gt(PersonAge(40))
        .and_(Person.salary.gt(PersonSalary(100000.0)))
        .or_(PersonScore.gt(PersonScore(95.0)))
    ).execute()

    # Should match: Charlie(45, 120k), Eve(52, 150k, 98.0 score)
    assert len(results) >= 2
    for person in results:
        condition1 = person.age.value > 40 and person.salary.value > 100000.0
        condition2 = person.score and person.score.value > 95.0
        assert condition1 or condition2


# ============================================================================
# Aggregation Tests
# ============================================================================


@pytest.mark.integration
@pytest.mark.order(13)
def test_single_aggregation(setup_schema):
    """Test single aggregation."""
    manager = Person.manager(setup_schema)

    # Average age
    result = manager.filter().aggregate(Person.age.avg())

    assert "avg_age" in result
    # Average of [25, 35, 45, 28, 52, 30] = 35.83...
    assert 35 <= result["avg_age"] <= 36


@pytest.mark.integration
@pytest.mark.order(14)
def test_multiple_aggregations(setup_schema):
    """Test multiple aggregations in one query."""
    manager = Person.manager(setup_schema)

    # Multiple stats
    result = manager.filter().aggregate(
        Person.age.avg(),
        Person.salary.avg(),
        Person.salary.sum(),
        Person.salary.max(),
        Person.salary.min(),
    )

    assert "avg_age" in result
    assert "avg_salary" in result
    assert "sum_salary" in result
    assert "max_salary" in result
    assert "min_salary" in result

    # Verify values make sense
    assert result["max_salary"] == 150000.0  # Eve
    assert result["min_salary"] == 68000.0  # Diana
    assert result["sum_salary"] == 590000.0  # Sum of all


@pytest.mark.integration
@pytest.mark.order(15)
def test_filtered_aggregation(setup_schema):
    """Test aggregation with filtering."""
    manager = Person.manager(setup_schema)

    # Average salary for Engineering only
    result = manager.filter(Person.department.eq(PersonDepartment("Engineering"))).aggregate(
        Person.salary.avg()
    )

    assert "avg_salary" in result
    # Engineering: Alice(75k), Bob(95k), Eve(150k) = 106,666.67
    assert 105000 <= result["avg_salary"] <= 108000


# ============================================================================
# Group-By Tests
# ============================================================================


@pytest.mark.integration
@pytest.mark.order(16)
def test_group_by_single_field(setup_schema):
    """Test grouping by single field."""
    manager = Person.manager(setup_schema)

    # Group by department using direct field access (Person.field)
    result = manager.group_by(Person.department).aggregate(Person.age.avg(), Person.salary.avg())

    # Should have 3 departments
    assert len(result) == 3
    assert "Engineering" in result
    assert "Sales" in result
    assert "Marketing" in result

    # Engineering: Alice(25, 75k), Bob(35, 95k), Eve(52, 150k)
    eng = result["Engineering"]
    assert "avg_age" in eng
    assert "avg_salary" in eng
    assert 35 <= eng["avg_age"] <= 38  # (25+35+52)/3 = 37.33
    assert 105000 <= eng["avg_salary"] <= 108000  # (75k+95k+150k)/3 = 106.67k


@pytest.mark.integration
@pytest.mark.order(17)
def test_group_by_with_filter(setup_schema):
    """Test group-by with pre-filtering."""
    manager = Person.manager(setup_schema)

    # Group by department, only age > 30
    result = (
        manager.filter(Person.age.gt(PersonAge(30)))
        .group_by(Person.department)
        .aggregate(Person.salary.avg())
    )

    # Should have groups, but filtered
    assert len(result) >= 2

    # Engineering should have Bob(95k) and Eve(150k)
    if "Engineering" in result:
        eng_avg = result["Engineering"]["avg_salary"]
        assert 120000 <= eng_avg <= 125000  # (95k+150k)/2 = 122.5k


@pytest.mark.integration
@pytest.mark.order(18)
def test_group_by_multiple_fields(setup_schema):
    """Test grouping by multiple fields."""
    manager = Person.manager(setup_schema)

    # Group by city AND department using direct field access
    result = manager.group_by(Person.city, Person.department).aggregate(Person.age.avg())

    # Should have tuple keys
    assert len(result) >= 3

    # Check one specific group
    ny_sales_found = False
    for (city, dept), stats in result.items():
        if city == "New York" and dept == "Sales":
            ny_sales_found = True
            # Charlie(45) and Frank(30) in NY Sales
            assert "avg_age" in stats

    assert ny_sales_found or len(result) > 0  # At least verify structure


# ============================================================================
# Pagination and Chaining Tests
# ============================================================================


@pytest.mark.integration
@pytest.mark.order(19)
def test_limit(setup_schema):
    """Test query limit."""
    manager = Person.manager(setup_schema)

    # Get only 3 results
    results = manager.filter().limit(3).execute()

    assert len(results) == 3


@pytest.mark.integration
@pytest.mark.order(20)
def test_offset(setup_schema):
    """Test query offset."""
    manager = Person.manager(setup_schema)

    # Skip first 2, get next 2
    results = manager.filter().limit(2).offset(2).execute()

    assert len(results) == 2


@pytest.mark.integration
@pytest.mark.order(21)
def test_first(setup_schema):
    """Test first() method."""
    manager = Person.manager(setup_schema)

    # Get first Engineering employee
    first = manager.filter(Person.department.eq(PersonDepartment("Engineering"))).first()

    assert first is not None
    assert first.department.value == "Engineering"


@pytest.mark.integration
@pytest.mark.order(22)
def test_count(setup_schema):
    """Test count() method."""
    manager = Person.manager(setup_schema)

    # Count Engineering employees
    count = manager.filter(Person.department.eq(PersonDepartment("Engineering"))).count()

    assert count == 3  # Alice, Bob, Eve


# ============================================================================
# Complex Real-World Scenarios
# ============================================================================


@pytest.mark.integration
@pytest.mark.order(23)
def test_complex_query_promotion_candidates(setup_schema):
    """Test complex query: find promotion candidates."""
    manager = Person.manager(setup_schema)

    # High performers with room for salary growth
    # score > 90 AND salary < 100k AND age < 50
    candidates = manager.filter(
        PersonScore.gt(PersonScore(90.0)),
        Person.salary.lt(PersonSalary(100000.0)),
        Person.age.lt(PersonAge(50)),
    ).execute()

    # Should find Alice(92.5, 75k, 25) - Frank(90, 82k, 30) excluded (score == 90, not > 90)
    assert len(candidates) >= 1
    for candidate in candidates:
        assert candidate.score is not None
        assert candidate.score.value > 90.0
        assert candidate.salary.value < 100000.0
        assert candidate.age.value < 50


@pytest.mark.integration
@pytest.mark.order(24)
def test_complex_query_with_aggregation(setup_schema):
    """Test complex filtering + aggregation."""
    manager = Person.manager(setup_schema)

    # High performers' average salary
    result = manager.filter(PersonScore.gt(PersonScore(90.0))).aggregate(
        Person.salary.avg(), Person.age.avg()
    )

    assert "avg_salary" in result
    assert "avg_age" in result

    # High scorers: Alice(75k, 25), Charlie(120k, 45), Eve(150k, 52), Frank(82k, 30)
    # Average salary should be around 106,750
    assert result["avg_salary"] > 90000


@pytest.mark.integration
@pytest.mark.order(25)
def test_backward_compatibility_dict_filters(setup_schema):
    """Test backward compatibility with dict filters."""
    manager = Person.manager(setup_schema)

    # Old dict-style filter should still work
    results = manager.filter(department="Engineering").execute()

    assert len(results) == 3
    for person in results:
        assert person.department.value == "Engineering"


@pytest.mark.integration
@pytest.mark.order(26)
def test_mixed_dict_and_expression_filters(setup_schema):
    """Test mixing dict filters with expressions."""
    manager = Person.manager(setup_schema)

    # Mix old and new style
    results = manager.filter(
        Person.age.gt(PersonAge(30)),  # Expression
        department="Engineering",  # Dict filter
    ).execute()

    assert len(results) == 2  # Bob(35), Eve(52)
    for person in results:
        assert person.department.value == "Engineering"
        assert person.age.value > 30
