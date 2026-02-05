"""Integration tests for type-safe role-player field expressions.

Tests the new type-safe syntax for filtering role player attributes:
    Employment.manager(db).filter(Employment.employee.age.gt(Age(30)))
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
    flags = TypeFlags(name="rfe_person")
    name: Name = Flag(Key)
    age: Age | None = None
    city: City | None = None


class Company(Entity):
    flags = TypeFlags(name="rfe_company")
    name: Name = Flag(Key)
    industry: Industry | None = None


class Employment(Relation):
    flags = TypeFlags(name="rfe_employment")
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
            salary=Salary(90000),
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


class TestRoleFieldExpressionComparisons:
    """Tests for type-safe comparison expressions on role-player fields."""

    @pytest.mark.integration
    @pytest.mark.order(300)
    def test_gt_filter(self, setup_employment_data):
        """Filter by role-player attribute greater than value."""
        db = setup_employment_data["db"]
        manager = Employment.manager(db)

        # Find employments where employee age > 25
        results = manager.filter(Employment.employee.age.gt(Age(25))).execute()

        assert len(results) == 2
        names = {r.employee.name.value for r in results}
        assert names == {"Alice", "Charlie"}

    @pytest.mark.integration
    @pytest.mark.order(301)
    def test_lt_filter(self, setup_employment_data):
        """Filter by role-player attribute less than value."""
        db = setup_employment_data["db"]
        manager = Employment.manager(db)

        # Find employments where employee age < 35
        results = manager.filter(Employment.employee.age.lt(Age(35))).execute()

        assert len(results) == 2
        names = {r.employee.name.value for r in results}
        assert names == {"Alice", "Bob"}

    @pytest.mark.integration
    @pytest.mark.order(302)
    def test_gte_filter(self, setup_employment_data):
        """Filter by role-player attribute greater than or equal value."""
        db = setup_employment_data["db"]
        manager = Employment.manager(db)

        # Find employments where employee age >= 30
        results = manager.filter(Employment.employee.age.gte(Age(30))).execute()

        assert len(results) == 2
        names = {r.employee.name.value for r in results}
        assert names == {"Alice", "Charlie"}

    @pytest.mark.integration
    @pytest.mark.order(303)
    def test_lte_filter(self, setup_employment_data):
        """Filter by role-player attribute less than or equal value."""
        db = setup_employment_data["db"]
        manager = Employment.manager(db)

        # Find employments where employee age <= 30
        results = manager.filter(Employment.employee.age.lte(Age(30))).execute()

        assert len(results) == 2
        names = {r.employee.name.value for r in results}
        assert names == {"Alice", "Bob"}

    @pytest.mark.integration
    @pytest.mark.order(304)
    def test_eq_filter(self, setup_employment_data):
        """Filter by role-player attribute equals value."""
        db = setup_employment_data["db"]
        manager = Employment.manager(db)

        # Find employments where employee age == 30
        results = manager.filter(Employment.employee.age.eq(Age(30))).execute()

        assert len(results) == 1
        assert results[0].employee.name.value == "Alice"

    @pytest.mark.integration
    @pytest.mark.order(305)
    def test_neq_filter(self, setup_employment_data):
        """Filter by role-player attribute not equals value."""
        db = setup_employment_data["db"]
        manager = Employment.manager(db)

        # Find employments where employee age != 30
        results = manager.filter(Employment.employee.age.neq(Age(30))).execute()

        assert len(results) == 2
        names = {r.employee.name.value for r in results}
        assert names == {"Bob", "Charlie"}


class TestRoleFieldExpressionStringMethods:
    """Tests for string-specific methods on role-player fields."""

    @pytest.mark.integration
    @pytest.mark.order(310)
    def test_contains_filter(self, setup_employment_data):
        """Filter by role-player string attribute contains."""
        from type_bridge.fields.role import RolePlayerStringFieldRef

        db = setup_employment_data["db"]
        manager = Employment.manager(db)

        # Find employments where employer name contains "Tech"
        employer_name_ref = Employment.employer.name
        assert isinstance(employer_name_ref, RolePlayerStringFieldRef)
        results = manager.filter(employer_name_ref.contains(Name("Tech"))).execute()

        assert len(results) == 2
        # Both Alice and Bob work at TechCorp
        names = {r.employee.name.value for r in results}
        assert names == {"Alice", "Bob"}

    @pytest.mark.integration
    @pytest.mark.order(311)
    def test_like_filter(self, setup_employment_data):
        """Filter by role-player string attribute like pattern."""
        from type_bridge.fields.role import RolePlayerStringFieldRef

        db = setup_employment_data["db"]
        manager = Employment.manager(db)

        # Find employments where employee city contains "Y" (NYC)
        employee_city_ref = Employment.employee.city
        assert isinstance(employee_city_ref, RolePlayerStringFieldRef)
        results = manager.filter(employee_city_ref.contains(City("Y"))).execute()

        assert len(results) == 2
        names = {r.employee.name.value for r in results}
        assert names == {"Alice", "Charlie"}


class TestCombinedExpressions:
    """Tests for combining type-safe expressions with other filters."""

    @pytest.mark.integration
    @pytest.mark.order(320)
    def test_combine_with_relation_attribute(self, setup_employment_data):
        """Combine role-player expression with relation's own attribute."""
        db = setup_employment_data["db"]
        manager = Employment.manager(db)

        # Find employments where employee age > 25 AND salary > 85000
        # Use manager.filter() for kwargs, then chain expressions
        results = manager.filter(Employment.employee.age.gt(Age(25)), salary__gt=85000).execute()

        assert len(results) == 2
        names = {r.employee.name.value for r in results}
        assert names == {"Alice", "Charlie"}

    @pytest.mark.integration
    @pytest.mark.order(321)
    def test_combine_with_django_style_lookup(self, setup_employment_data):
        """Combine type-safe expression with Django-style lookup."""
        db = setup_employment_data["db"]
        manager = Employment.manager(db)

        # Type-safe expression + Django-style lookup in same filter call
        results = manager.filter(
            Employment.employee.age.gt(Age(25)), employer__industry__eq="Technology"
        ).execute()

        assert len(results) == 1
        assert results[0].employee.name.value == "Alice"

    @pytest.mark.integration
    @pytest.mark.order(322)
    def test_multiple_role_field_expressions(self, setup_employment_data):
        """Combine multiple type-safe role field expressions."""
        from type_bridge.fields.role import RolePlayerStringFieldRef

        db = setup_employment_data["db"]
        manager = Employment.manager(db)

        # Filter on both employee and employer attributes
        employer_name_ref = Employment.employer.name
        assert isinstance(employer_name_ref, RolePlayerStringFieldRef)
        results = (
            manager.filter(Employment.employee.age.gte(Age(25)))
            .filter(employer_name_ref.contains(Name("Tech")))
            .execute()
        )

        assert len(results) == 2
        names = {r.employee.name.value for r in results}
        assert names == {"Alice", "Bob"}


class TestCombinedWithPagination:
    """Tests for combining type-safe expressions with sorting and pagination."""

    @pytest.mark.integration
    @pytest.mark.order(323)
    def test_combined_with_order_by_and_limit(self, setup_employment_data):
        """Combine type-safe expression + Django-style + order_by + limit + offset."""
        db = setup_employment_data["db"]
        manager = Employment.manager(db)

        # Full combined query:
        # - Type-safe expression: Employment.employee.age.gte(Age(25))
        # - Django-style: salary__gte=80000
        # - Order by role-player attribute and relation attribute
        # - Pagination with limit and offset
        results = (
            manager.filter(Employment.employee.age.gte(Age(25)), salary__gte=80000)
            .order_by("employee__age", "-salary")
            .limit(10)
            .offset(0)
            .execute()
        )

        # All 3 employees match (age >= 25, salary >= 80000)
        assert len(results) == 3

        # Verify order: sorted by employee age asc, then salary desc
        # Bob (25, 80000), Alice (30, 100000), Charlie (40, 90000)
        assert results[0].employee.name.value == "Bob"
        assert results[1].employee.name.value == "Alice"
        assert results[2].employee.name.value == "Charlie"

    @pytest.mark.integration
    @pytest.mark.order(324)
    def test_chained_filter_with_pagination(self, setup_employment_data):
        """Chain multiple filter() calls with Django-style + pagination."""
        db = setup_employment_data["db"]
        manager = Employment.manager(db)

        # Chained filters with Django-style in second filter call
        results = (
            manager.filter(Employment.employee.age.gte(Age(25)))
            .filter(employer__industry__eq="Technology")
            .order_by("-salary")
            .limit(1)
            .execute()
        )

        # Alice and Bob work at TechCorp, Alice has higher salary
        assert len(results) == 1
        assert results[0].employee.name.value == "Alice"
        assert results[0].salary is not None
        assert results[0].salary.value == 100000


class TestBackwardCompatibilityWithDjangoStyle:
    """Tests verifying Django-style lookups still work alongside type-safe expressions."""

    @pytest.mark.integration
    @pytest.mark.order(330)
    def test_django_style_still_works(self, setup_employment_data):
        """Django-style lookups should continue to work."""
        db = setup_employment_data["db"]
        manager = Employment.manager(db)

        # Old Django-style syntax
        results = manager.filter(employee__age__gt=25).execute()

        assert len(results) == 2
        names = {r.employee.name.value for r in results}
        assert names == {"Alice", "Charlie"}

    @pytest.mark.integration
    @pytest.mark.order(331)
    def test_equivalent_results(self, setup_employment_data):
        """Type-safe and Django-style should produce equivalent results."""
        db = setup_employment_data["db"]
        manager = Employment.manager(db)

        # Type-safe
        results1 = manager.filter(Employment.employee.age.gt(Age(25))).execute()

        # Django-style
        results2 = manager.filter(employee__age__gt=25).execute()

        # Should have same results
        names1 = {r.employee.name.value for r in results1}
        names2 = {r.employee.name.value for r in results2}
        assert names1 == names2
