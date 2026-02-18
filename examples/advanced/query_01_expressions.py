"""Advanced query expressions with type-safe filtering, aggregations, and grouping.

This example demonstrates the complete query expression API for building
complex, type-safe queries with TypeBridge.

Features demonstrated:
- Value comparisons (>, <, >=, <=, ==, !=)
- String operations (contains, like, regex)
- Boolean logic (AND, OR, NOT)
- Database-side aggregations (sum, avg, max, min)
- Group-by queries with aggregations

NEW: Uses the concise attribute type-based API (e.g., Age.gt(Age(30)))
     instead of field-based API (e.g., Employee.age.gt(Age(30)))
     This provides better type checking and shorter syntax!
"""

from type_bridge import Database, Entity, TypeFlags
from type_bridge.attribute import Double, Integer, String
from type_bridge.attribute.flags import Flag, Key

# ============================================================================
# Schema Definition
# ============================================================================


class EmployeeID(String):
    """Unique employee identifier."""

    pass


class FullName(String):
    """Employee full name."""

    pass


class Email(String):
    """Employee email address."""

    pass


class Age(Integer):
    """Employee age in years."""

    pass


class Salary(Double):
    """Employee salary."""

    pass


class Department(String):
    """Department name."""

    pass


class JobTitle(String):
    """Job title/position."""

    pass


class PerformanceScore(Double):
    """Performance rating (0.0 - 100.0)."""

    pass


class Employee(Entity):
    """Employee entity with comprehensive attributes."""

    flags = TypeFlags(name="employee")

    employee_id: EmployeeID = Flag(Key)
    name: FullName
    email: Email
    age: Age
    salary: Salary
    department: Department
    job_title: JobTitle
    performance_score: PerformanceScore | None


# ============================================================================
# Example Usage
# ============================================================================


def example_comparison_queries(db: Database):
    """Demonstrate value comparison queries."""
    print("\n" + "=" * 70)
    print("1. VALUE COMPARISON QUERIES")
    print("=" * 70)

    manager = Employee.manager(db)

    # Simple comparison: age > 30
    print("\n📊 Employees older than 30:")
    older_employees = manager.filter(Age.gt(Age(30))).execute()
    print(f"   Found {len(older_employees)} employees")

    # Range query: 25 <= age <= 40
    print("\n📊 Employees aged 25-40:")
    mid_age_employees = manager.filter(Age.gte(Age(25)), Age.lte(Age(40))).execute()
    print(f"   Found {len(mid_age_employees)} employees")

    # Salary filtering: salary >= 80000
    print("\n💰 High earners (>= $80k):")
    high_earners = manager.filter(Salary.gte(Salary(80000.0))).execute()
    print(f"   Found {len(high_earners)} employees")

    # Exact match: age == 35
    print("\n🎯 Employees exactly 35 years old:")
    age_35 = manager.filter(Age.eq(Age(35))).execute()
    print(f"   Found {len(age_35)} employees")


def example_string_queries(db: Database):
    """Demonstrate string operation queries."""
    print("\n" + "=" * 70)
    print("2. STRING OPERATION QUERIES")
    print("=" * 70)

    manager = Employee.manager(db)

    # Contains: email contains '@company.com'
    print("\n📧 Company email addresses:")
    company_emails = manager.filter(Email.contains(Email("@company.com"))).execute()
    print(f"   Found {len(company_emails)} employees")

    # Like (regex): names starting with 'A'
    print("\n👤 Names starting with 'A':")
    a_names = manager.filter(FullName.like(FullName("^A.*"))).execute()
    print(f"   Found {len(a_names)} employees")

    # Department filtering
    print("\n🏢 Engineering department:")
    engineers = manager.filter(Department.eq(Department("Engineering"))).execute()
    print(f"   Found {len(engineers)} engineers")


def example_boolean_logic(db: Database):
    """Demonstrate boolean logic with AND, OR, NOT."""
    print("\n" + "=" * 70)
    print("3. BOOLEAN LOGIC QUERIES")
    print("=" * 70)

    manager = Employee.manager(db)

    # OR: age < 25 OR age > 60 (early career or near retirement)
    print("\n👥 Early career OR near retirement:")
    young_or_senior = manager.filter(Age.lt(Age(25)).or_(Age.gt(Age(60)))).execute()
    print(f"   Found {len(young_or_senior)} employees")

    # AND: Engineering department AND senior level
    print("\n🔧 Senior engineers:")
    senior_engineers = manager.filter(
        Department.eq(Department("Engineering")).and_(JobTitle.contains(JobTitle("Senior")))
    ).execute()
    print(f"   Found {len(senior_engineers)} senior engineers")

    # NOT: not in Sales department
    print("\n🚫 Non-sales employees:")
    non_sales = manager.filter(Department.eq(Department("Sales")).not_()).execute()
    print(f"   Found {len(non_sales)} employees")

    # Complex: (age > 40 AND salary > 100k) OR performance > 90
    print("\n⭐ High performers OR experienced high earners:")
    top_talent = manager.filter(
        Age.gt(Age(40))
        .and_(Salary.gt(Salary(100000.0)))
        .or_(PerformanceScore.gt(PerformanceScore(90.0)))
    ).execute()
    print(f"   Found {len(top_talent)} employees")


def example_aggregations(db: Database):
    """Demonstrate database-side aggregations."""
    print("\n" + "=" * 70)
    print("4. AGGREGATION QUERIES")
    print("=" * 70)

    manager = Employee.manager(db)

    # Average age across all employees
    print("\n📊 Company-wide statistics:")
    result = manager.filter().aggregate(
        Age.avg(), Salary.avg(), Salary.sum(), PerformanceScore.avg()
    )
    print(f"   Average age: {result.get('avg_age', 'N/A')}")
    print(f"   Average salary: ${result.get('avg_salary', 0):,.2f}")
    print(f"   Total payroll: ${result.get('sum_salary', 0):,.2f}")
    print(f"   Average performance: {result.get('avg_performancescore', 'N/A')}")

    # Aggregation with filtering: Engineering department only
    print("\n🔧 Engineering department statistics:")
    eng_stats = manager.filter(Department.eq(Department("Engineering"))).aggregate(
        Age.avg(), Salary.avg(), Salary.max(), Salary.min()
    )
    print(f"   Average age: {eng_stats.get('avg_age', 'N/A')}")
    print(f"   Average salary: ${eng_stats.get('avg_salary', 0):,.2f}")
    print(f"   Max salary: ${eng_stats.get('max_salary', 0):,.2f}")
    print(f"   Min salary: ${eng_stats.get('min_salary', 0):,.2f}")

    # Count employees with high performance
    print("\n⭐ High performers (score > 85):")
    high_performers = manager.filter(PerformanceScore.gt(PerformanceScore(85.0))).execute()
    print(f"   Count: {len(high_performers)}")


def example_group_by(db: Database):
    """Demonstrate group-by queries with aggregations."""
    print("\n" + "=" * 70)
    print("5. GROUP-BY QUERIES")
    print("=" * 70)

    manager = Employee.manager(db)

    # Group by department
    print("\n🏢 Statistics by department:")
    dept_stats = manager.group_by(Employee.department).aggregate(Age.avg(), Salary.avg())
    for dept, stats in dept_stats.items():
        print(f"\n   {dept}:")
        print(f"      Average age: {stats.get('avg_age', 'N/A')}")
        print(f"      Average salary: ${stats.get('avg_salary', 0):,.2f}")

    # Group by job title
    print("\n💼 Statistics by job title:")
    title_stats = manager.group_by(Employee.job_title).aggregate(
        Salary.avg(), PerformanceScore.avg()
    )
    for title, stats in title_stats.items():
        print(f"\n   {title}:")
        print(f"      Average salary: ${stats.get('avg_salary', 0):,.2f}")
        print(f"      Average performance: {stats.get('avg_performancescore', 'N/A')}")


def example_complex_queries(db: Database):
    """Demonstrate complex real-world query patterns."""
    print("\n" + "=" * 70)
    print("6. COMPLEX REAL-WORLD QUERIES")
    print("=" * 70)

    manager = Employee.manager(db)

    # Find promotion candidates: high performance, reasonable tenure, not maxed salary
    print("\n🎯 Promotion candidates:")
    print("   (Performance > 85, Age 30-50, Salary < $120k)")
    candidates = manager.filter(
        PerformanceScore.gt(PerformanceScore(85.0)),
        Age.gte(Age(30)),
        Age.lte(Age(50)),
        Salary.lt(Salary(120000.0)),
    ).execute()
    print(f"   Found {len(candidates)} candidates")

    # Diversity analysis: age distribution by department
    print("\n📊 Age distribution by department:")
    for dept_name in ["Engineering", "Sales", "Marketing", "HR"]:
        dept_age = manager.filter(Department.eq(Department(dept_name))).aggregate(
            Age.avg(), Age.min(), Age.max()
        )
        print(f"\n   {dept_name}:")
        print(f"      Average: {dept_age.get('avg_age', 'N/A')}")
        print(f"      Range: {dept_age.get('min_age', 'N/A')}-{dept_age.get('max_age', 'N/A')}")

    # Compensation analysis: salary vs performance correlation
    print("\n💰 High performers vs compensation:")
    high_perf_low_pay = manager.filter(
        PerformanceScore.gt(PerformanceScore(90.0)), Salary.lt(Salary(70000.0))
    ).execute()
    print(f"   High performers with low salary: {len(high_perf_low_pay)}")
    print("   (May indicate underpaid talent)")


def example_pagination_and_limits(db: Database):
    """Demonstrate pagination with expressions."""
    print("\n" + "=" * 70)
    print("7. PAGINATION AND LIMITS")
    print("=" * 70)

    manager = Employee.manager(db)

    # First 10 high earners
    print("\n💰 Top 10 highest earners:")
    top_10 = manager.filter(Salary.gte(Salary(0.0))).limit(10).execute()
    print(f"   Retrieved {len(top_10)} employees")

    # Paginated results: page 2 (items 11-20)
    print("\n📄 Page 2 of high earners:")
    page_2 = manager.filter(Salary.gte(Salary(0.0))).limit(10).offset(10).execute()
    print(f"   Retrieved {len(page_2)} employees")

    # Get first matching employee
    print("\n🎯 First engineer:")
    first_engineer = manager.filter(Department.eq(Department("Engineering"))).first()
    if first_engineer:
        print(
            f"   Found: {first_engineer.name.value if hasattr(first_engineer.name, 'value') else first_engineer.name}"
        )


def main():
    """Run all examples (requires TypeDB connection)."""
    print("\n" + "=" * 70)
    print("TYPE-SAFE QUERY EXPRESSIONS - ADVANCED EXAMPLES")
    print("=" * 70)
    print("\n✨ NEW API: Concise attribute type-based expressions!")
    print("   Old: Employee.age.gt(Age(30))")
    print("   New: Age.gt(Age(30))  # Shorter and type-safe!")
    print("\n⚠️  These examples require a running TypeDB instance with data.")
    print("    To run: start TypeDB and populate with employee data.")
    print("\nExample usage pattern shown below (uncomment to run with real DB):")
    print("""
    # Connect to database
    db = Database(address="localhost:1729", database="employees")
    db.connect()

    # Run examples
    example_comparison_queries(db)
    example_string_queries(db)
    example_boolean_logic(db)
    example_aggregations(db)
    example_group_by(db)
    example_complex_queries(db)
    example_pagination_and_limits(db)
    """)

    print("\n" + "=" * 70)
    print("✅ All query patterns are type-safe!")
    print("=" * 70)
    print("\nKey benefits:")
    print("  1. Full IDE autocomplete for all methods")
    print("  2. Compile-time type checking with pyright")
    print("  3. Database-side filtering and aggregation (efficient)")
    print("  4. Composable expressions for complex queries")
    print("  5. Concise syntax with attribute type methods")


if __name__ == "__main__":
    main()
