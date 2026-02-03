"""Demonstration of automatic schema conflict detection.

This example shows how sync_schema automatically detects existing schema
and prevents accidental overwrites that could cause data loss.
"""

from type_bridge import (
    Entity,
    Flag,
    Key,
    SchemaConflictError,
    SchemaManager,
    String,
    TypeFlags,
)
from type_bridge.session import Database


# Initial schema V1
class Name(String):
    pass


class Email(String):
    pass


class PersonV1(Entity):
    flags: TypeFlags = TypeFlags(name="person")

    email: Email
    name: Name = Flag(Key)


# Modified schema V2 - removed email field
class PersonV2(Entity):
    flags: TypeFlags = TypeFlags(name="person")

    # email removed - breaking change!
    name: Name = Flag(Key)


def main():
    """Demonstrate automatic schema conflict detection."""
    print("=" * 70)
    print("Schema Conflict Detection Demo")
    print("=" * 70)

    # Connect to database
    db = Database(address="localhost:1729", database="conflict_demo")
    db.connect()

    # Clean slate
    if db.database_exists():
        db.delete_database()

    print("\n1. Creating initial schema (V1)...")
    print("-" * 70)

    # Create initial schema V1
    manager_v1 = SchemaManager(db)
    manager_v1.register(PersonV1)

    # First sync - database is empty, should work
    manager_v1.sync_schema()
    print("✓ Initial schema created successfully")
    print("  - PersonV1 with fields: name (key), email")

    # Show the schema
    print("\n2. Current database schema:")
    print("-" * 70)
    schema = db.get_schema()
    # Show just the person-related parts
    for line in schema.split("\n"):
        if "person" in line.lower() or "email" in line.lower():
            print(f"  {line}")

    print("\n3. Attempting to sync modified schema (V2) without force...")
    print("-" * 70)

    # Try to sync modified schema
    manager_v2 = SchemaManager(db)
    manager_v2.register(PersonV2)  # Registered V2 which removed email

    try:
        # This should raise SchemaConflictError
        manager_v2.sync_schema()
        print("❌ ERROR: Should have raised SchemaConflictError!")
    except SchemaConflictError as e:
        print("✓ Conflict detected! Exception raised as expected:")
        print("")
        print(str(e))

    print("\n4. Using force=True to override (recreates database)...")
    print("-" * 70)

    # Use force to override
    manager_v2.sync_schema(force=True)
    print("✓ Schema forcefully updated (database recreated)")
    print("  ⚠️  Note: All existing data would be lost!")

    # Show the new schema
    print("\n5. New database schema:")
    print("-" * 70)
    schema = db.get_schema()
    for line in schema.split("\n"):
        if "person" in line.lower() or "email" in line.lower():
            print(f"  {line}")

    print("\n" + "=" * 70)
    print("Summary")
    print("=" * 70)
    print("✓ sync_schema automatically detects existing schema")
    print("✓ Prevents accidental overwrites that could cause data loss")
    print("✓ Requires explicit force=True to override existing schema")
    print("✓ Provides clear error messages with resolution options")
    print("\n" + "=" * 70)

    # Cleanup
    db.delete_database()
    db.close()


if __name__ == "__main__":
    main()
