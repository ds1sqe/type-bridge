"""Example 5: Put operations (insert if does not exist).

This example demonstrates TypeDB's 'put' operation which provides idempotent
inserts - it ensures data exists without creating duplicates.

Put semantics:
- Matches the pattern first
- If complete match found: no changes, returns matched instances
- If any part fails to match: inserts the entire pattern
- All-or-nothing behavior for bulk operations

Run this example:
    uv run python examples/basic/crud_05_put.py
"""

from type_bridge import (
    Database,
    Entity,
    EntityFlags,
    Flag,
    Integer,
    Key,
    Relation,
    RelationFlags,
    Role,
    String,
)
from type_bridge.schema import SchemaManager


# ============================================================================
# Step 1: Define attribute types
# ============================================================================
class Name(String):
    """Name attribute - reusable across entities."""

    pass


class Email(String):
    """Email attribute."""

    pass


class Age(Integer):
    """Age attribute."""

    pass


class Position(String):
    """Job position attribute."""

    pass


class Salary(Integer):
    """Salary attribute."""

    pass


# ============================================================================
# Step 2: Define entities
# ============================================================================
class Person(Entity):
    """Person entity with name as key."""

    flags = EntityFlags(type_name="person")
    name: Name = Flag(Key)  # Key attribute - ensures uniqueness
    email: Email | None
    age: Age | None


class Company(Entity):
    """Company entity."""

    flags = EntityFlags(type_name="company")
    name: Name = Flag(Key)


# ============================================================================
# Step 3: Define relations
# ============================================================================
class Employment(Relation):
    """Employment relation between person and company."""

    flags = RelationFlags(type_name="employment")
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)
    position: Position | None
    salary: Salary | None


# ============================================================================
# Main example
# ============================================================================
def main():
    """Demonstrate put operations."""
    print("=" * 80)
    print("TypeBridge Example 5: Put Operations (Insert if Does Not Exist)")
    print("=" * 80)

    # Connect to database
    db = Database(address="localhost:1729", database="example_put")
    db.connect()

    # Clean up and recreate database
    try:
        db.delete_database()
    except Exception:
        pass
    db.create_database()

    # Set up schema
    schema_manager = SchemaManager(db)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    # Create managers
    person_manager = Person.manager(db)
    company_manager = Company.manager(db)
    employment_manager = Employment.manager(db)

    # ========================================================================
    # Example 1: put() - Single entity (idempotent insert)
    # ========================================================================
    print("\n[Example 1] put() - Single entity")
    print("-" * 80)

    alice = Person(name=Name("Alice"), age=Age(30), email=Email("alice@example.com"))

    # First put - should insert
    print("First put(alice)...")
    person_manager.put(alice)
    print(f"  → People count: {len(person_manager.all())}")

    # Second put - should NOT create duplicate (idempotent)
    print("Second put(alice)...")
    person_manager.put(alice)
    print(f"  → People count: {len(person_manager.all())}")  # Still 1

    # Third put - still no duplicate
    print("Third put(alice)...")
    person_manager.put(alice)
    print(f"  → People count: {len(person_manager.all())}")  # Still 1

    # ========================================================================
    # Example 2: put_many() - Multiple entities
    # ========================================================================
    print("\n[Example 2] put_many() - Bulk idempotent insert")
    print("-" * 80)

    persons = [
        Person(name=Name("Bob"), age=Age(25), email=Email("bob@example.com")),
        Person(name=Name("Charlie"), age=Age(35), email=Email("charlie@example.com")),
        Person(name=Name("Diana"), age=Age(28), email=Email("diana@example.com")),
    ]

    # First put_many - should insert all
    print("First put_many([Bob, Charlie, Diana])...")
    person_manager.put_many(persons)
    print(f"  → People count: {len(person_manager.all())}")  # 4 (Alice + 3 new)

    # Second put_many - should NOT create duplicates
    print("Second put_many([Bob, Charlie, Diana])...")
    person_manager.put_many(persons)
    print(f"  → People count: {len(person_manager.all())}")  # Still 4

    # ========================================================================
    # Example 3: put() vs insert() - Duplicate behavior
    # ========================================================================
    print("\n[Example 3] put() vs insert() - Duplicate prevention")
    print("-" * 80)

    eve = Person(name=Name("Eve"), age=Age(32))

    # Using put - idempotent
    print("Using put(eve) three times...")
    person_manager.put(eve)
    person_manager.put(eve)
    person_manager.put(eve)
    count_after_put = len(person_manager.get(name="Eve"))
    print(f"  → Eves count: {count_after_put}")  # Should be 1

    # Clean up Eve
    person_manager.delete(name="Eve")

    # Using insert - creates duplicates (may fail with @key constraint)
    print("\nUsing insert(eve) - may fail due to @key constraint...")
    try:
        person_manager.insert(eve)
        person_manager.insert(eve)  # This should fail with @key constraint
        print("  → insert() allowed duplicate (unexpected)")
    except Exception as e:
        print(f"  → insert() prevented duplicate: {type(e).__name__}")

    # ========================================================================
    # Example 4: put() with relations
    # ========================================================================
    print("\n[Example 4] put() - Relations (idempotent)")
    print("-" * 80)

    # Create company
    tech_corp = Company(name=Name("TechCorp"))
    company_manager.put(tech_corp)

    # Get Alice (already exists)
    alice_from_db = person_manager.get(name="Alice")[0]

    # Create employment relation
    employment = Employment(
        employee=alice_from_db,
        employer=tech_corp,
        position=Position("Engineer"),
        salary=Salary(100000),
    )

    # First put - should insert
    print("First put(employment)...")
    employment_manager.put(employment)
    print(f"  → Employments count: {len(employment_manager.all())}")

    # Second put - should NOT create duplicate
    print("Second put(employment)...")
    employment_manager.put(employment)
    print(f"  → Employments count: {len(employment_manager.all())}")  # Still 1

    # ========================================================================
    # Example 5: put_many() with relations
    # ========================================================================
    print("\n[Example 5] put_many() - Multiple relations")
    print("-" * 80)

    # Get Bob and Charlie
    bob_from_db = person_manager.get(name="Bob")[0]
    charlie_from_db = person_manager.get(name="Charlie")[0]

    employments = [
        Employment(
            employee=bob_from_db,
            employer=tech_corp,
            position=Position("Manager"),
            salary=Salary(120000),
        ),
        Employment(
            employee=charlie_from_db,
            employer=tech_corp,
            position=Position("Senior Engineer"),
            salary=Salary(110000),
        ),
    ]

    # First put_many - should insert all
    print("First put_many([Bob's job, Charlie's job])...")
    employment_manager.put_many(employments)
    print(f"  → Employments count: {len(employment_manager.all())}")  # 3 total

    # Second put_many - should NOT create duplicates
    print("Second put_many([Bob's job, Charlie's job])...")
    employment_manager.put_many(employments)
    print(f"  → Employments count: {len(employment_manager.all())}")  # Still 3

    # ========================================================================
    # Summary: When to use put vs insert
    # ========================================================================
    print("\n[Summary] put() vs insert()")
    print("-" * 80)
    print("Use put() when:")
    print("  • You want idempotent operations (safe to run multiple times)")
    print("  • Loading data from external sources (avoid duplicates)")
    print("  • Ensuring data exists without worrying about duplicates")
    print("  • Working with @key attributes for natural deduplication")
    print()
    print("Use insert() when:")
    print("  • You know the data is new and doesn't exist")
    print("  • You want to fail fast if duplicates occur")
    print("  • Performance is critical (insert is slightly faster)")
    print()

    # Clean up
    db.delete_database()
    db.close()
    print("Done! Database cleaned up.")


if __name__ == "__main__":
    main()
