"""Example demonstrating SchemaManager for schema definition and data insertion.

This example shows:
1. Defining attributes, entities, and relations using Python classes
2. Using SchemaManager to generate and apply schema
3. Using EntityManager to insert entities
4. Using RelationManager to insert relations
5. Querying data to verify insertion
"""

from type_bridge import (
    Boolean,
    Card,
    DateTime,
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    Role,
    SchemaManager,
    String,
    TypeFlags,
    Unique,
)
from type_bridge.session import Database


# Define Attribute Types
class Name(String):
    """Name attribute."""


class Email(String):
    """Email attribute."""


class Age(Integer):
    """Age attribute."""


class Salary(Integer):
    """Salary attribute."""


class Founded(Integer):
    """Year founded attribute."""


class Industry(String):
    """Industry attribute."""


class Position(String):
    """Job position attribute."""


class StartDate(DateTime):
    """Employment start date."""


class Active(Boolean):
    """Active status."""


# Define Entity Types
class Person(Entity):
    """Person entity with various attribute types."""

    flags: TypeFlags = TypeFlags(name="person")

    age: Age | None  # Optional (0 or 1)
    active: Active  # Required, default cardinality
    name: Name = Flag(Key)  # Required, unique identifier
    email: Email = Flag(Unique)  # Required by the type, and must be unique


class Company(Entity):
    """Company entity with multi-value attributes."""

    flags: TypeFlags = TypeFlags(name="company")

    founded: Founded  # Required
    name: Name = Flag(Key)  # Required, unique identifier
    industry: list[Industry] = Flag(Card(1, 5))  # 1 to 5 industries


# Define Relation Types
class Employment(Relation):
    """Employment relation connecting persons and companies."""

    flags: TypeFlags = TypeFlags(name="employment")

    # Define roles (not ClassVar, so they can be instance attributes)
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)

    # Relation attributes
    position: Position  # Required
    salary: Salary | None  # Optional
    start_date: StartDate | None  # Optional
    active: Active  # Required


def main() -> None:
    """Demonstrate SchemaManager and data insertion."""
    print("=" * 70)
    print("SchemaManager and Data Insertion Example")
    print("=" * 70)

    # Step 1: Connect to database
    print("\n1. Connecting to database...")
    db = Database(address="localhost:1729", database="schema_manager_example")
    db.connect()

    # Clean slate - delete if exists
    if db.database_exists():
        print("   - Deleting existing database")
        db.delete_database()

    db.create_database()
    print("   ✓ Database created")

    # Step 2: Define schema using SchemaManager
    print("\n2. Defining schema using SchemaManager...")
    schema_manager = SchemaManager(db)

    # Register all model classes
    schema_manager.register(Person, Company, Employment)
    print(f"   - Registered {len(schema_manager.registered_models)} models")

    # Generate schema (for inspection)
    schema = schema_manager.generate_schema()
    print("\n   Generated TypeQL Schema:")
    print("   " + "-" * 66)
    for line in schema.split("\n")[:20]:  # Show first 20 lines
        print(f"   {line}")
    if len(schema.split("\n")) > 20:
        print(f"   ... ({len(schema.split('\n')) - 20} more lines)")
    print("   " + "-" * 66)

    # Apply schema to database
    schema_manager.sync_schema()
    print("   ✓ Schema applied to database")

    # Step 3: Insert entities using EntityManager
    print("\n3. Inserting entities using EntityManager...")

    # Insert persons - direct instantiation with wrapped types
    alice = Person(
        name=Name("Alice Johnson"),
        email=Email("alice@example.com"),
        age=Age(30),
        active=Active(True),
    )
    Person.manager(db).insert(alice)
    print(f"   ✓ Inserted person: {alice.name.value}")

    bob = Person(
        name=Name("Bob Smith"),
        email=Email("bob@example.com"),
        age=Age(35),
        active=Active(True),
    )
    Person.manager(db).insert(bob)
    print(f"   ✓ Inserted person: {bob.name.value}")

    charlie = Person(
        name=Name("Charlie Brown"),
        email=Email("charlie@example.com"),
        age=None,  # Optional field
        active=Active(False),
    )
    Person.manager(db).insert(charlie)
    print(f"   ✓ Inserted person: {charlie.name.value} (inactive, no age)")

    # Insert companies - direct instantiation with wrapped types
    tech_corp = Company(
        name=Name("TechCorp"),
        founded=Founded(2010),
        industry=[Industry("Technology"), Industry("Software"), Industry("AI")],
    )
    Company.manager(db).insert(tech_corp)
    print(f"   ✓ Inserted company: {tech_corp.name.value}")

    startup_co = Company(
        name=Name("StartupCo"),
        founded=Founded(2020),
        industry=[Industry("Technology"), Industry("Startup")],
    )
    Company.manager(db).insert(startup_co)
    print(f"   ✓ Inserted company: {startup_co.name.value}")

    # Step 4: Insert relations using RelationManager
    print("\n4. Inserting relations using RelationManager...")

    from datetime import datetime

    # Alice works at TechCorp - using typed instance
    employment1 = Employment(
        employee=alice,
        employer=tech_corp,
        position=Position("Senior Software Engineer"),
        salary=Salary(120000),
        start_date=StartDate(datetime(2020, 1, 15)),
        active=Active(True),
    )
    Employment.manager(db).insert(employment1)
    print("   ✓ Created employment: Alice → TechCorp as Senior Software Engineer")

    # Bob works at TechCorp - using new instance-based insert
    employment2 = Employment(
        position=Position("Product Manager"),
        salary=Salary(130000),
        start_date=StartDate(datetime(2019, 6, 1)),
        active=Active(True),
        employee=bob,
        employer=tech_corp,
    )
    employment2.insert(db)
    print("   ✓ Created employment: Bob → TechCorp as Product Manager")

    # Charlie works at StartupCo (no salary disclosed) - using typed instance
    employment3 = Employment(
        employee=charlie,
        employer=startup_co,
        position=Position("Founder"),
        salary=None,  # Optional field
        start_date=StartDate(datetime(2020, 3, 1)),
        active=Active(True),
    )
    Employment.manager(db).insert(employment3)
    print("   ✓ Created employment: Charlie → StartupCo as Founder (no salary)")

    # Step 5: Query data to verify
    print("\n5. Querying data to verify insertion...")

    # Count entities
    count_query = """
    match
    $p isa person;
    """
    with db.transaction("read") as tx:
        results = tx.execute(count_query)
        # Results are returned but counting them would require iterating
        print("   ✓ Successfully queried persons")

    # Query with relation
    relation_query = """
    match
    $e(employee: $p, employer: $c) isa employment;
    $p has name $pname;
    $c has name $cname;
    """
    with db.transaction("read") as tx:
        tx.execute(relation_query)
        print("   ✓ Successfully queried employment relations")

    # Step 6: Summary
    print("\n" + "=" * 70)
    print("Summary")
    print("=" * 70)
    print(f"✓ Schema defined with {len(schema_manager.registered_models)} models:")
    print("  - Entities: Person, Company")
    print("  - Relations: Employment")
    print("✓ Inserted 3 persons: Alice, Bob, Charlie")
    print("✓ Inserted 2 companies: TechCorp, StartupCo")
    print("✓ Inserted 3 employment relations")
    print("\nDatabase: schema_manager_example")
    print("Location: localhost:1729")
    print("\nNote: Database is kept for inspection. Delete manually if needed.")
    print("=" * 70)

    scm = db.get_schema()
    print("Schema")
    print(scm)

    # Close connection
    db.close()


if __name__ == "__main__":
    main()
