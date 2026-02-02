"""Demonstration of schema comparison for migration planning.

This example shows how to:
1. Define two different schema versions
2. Compare them to find differences
3. Generate a human-readable migration summary
"""

from type_bridge import (
    Boolean,
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


# Version 1: Initial schema
class Name(String):
    pass


class Email(String):
    pass


class Age(Integer):
    pass


class PersonV1(Entity):
    flags: TypeFlags = TypeFlags(name="person")

    email: Email
    name: Name = Flag(Key)


# Version 2: Updated schema with new fields
class Active(Boolean):
    pass


class Phone(String):
    pass


class PersonV2(Entity):
    flags: TypeFlags = TypeFlags(name="person")

    email: Email
    age: Age | None  # Added optional age
    active: Active  # Added active status
    name: Name = Flag(Key)


class CompanyV2(Entity):
    """New entity in V2."""

    flags: TypeFlags = TypeFlags(name="company")

    name: Name = Flag(Key)


class EmploymentV2(Relation):
    """New relation in V2."""

    flags: TypeFlags = TypeFlags(name="employment")

    employee: Role[PersonV2] = Role("employee", PersonV2)
    employer: Role[CompanyV2] = Role("employer", CompanyV2)


def main():
    """Demonstrate schema comparison."""
    print("=" * 70)
    print("Schema Comparison Demo")
    print("=" * 70)

    # Create schema info for V1 (old schema)
    from type_bridge import Database

    db_v1 = Database(address="localhost:1729", database="demo_v1")
    schema_v1 = SchemaManager(db_v1)
    schema_v1.register(PersonV1)
    info_v1 = schema_v1.collect_schema_info()

    print("\n1. Schema V1 (Old):")
    print("-" * 70)
    print(f"   Entities: {[e.__name__ for e in info_v1.entities]}")
    print(f"   Relations: {[r.__name__ for r in info_v1.relations]}")
    print(f"   Attributes: {[a.get_attribute_name() for a in info_v1.attribute_classes]}")

    # Create schema info for V2 (new schema)
    db_v2 = Database(address="localhost:1729", database="demo_v2")
    schema_v2 = SchemaManager(db_v2)
    schema_v2.register(PersonV2, CompanyV2, EmploymentV2)
    info_v2 = schema_v2.collect_schema_info()

    print("\n2. Schema V2 (New):")
    print("-" * 70)
    print(f"   Entities: {[e.__name__ for e in info_v2.entities]}")
    print(f"   Relations: {[r.__name__ for r in info_v2.relations]}")
    print(f"   Attributes: {[a.get_attribute_name() for a in info_v2.attribute_classes]}")

    # Compare schemas
    print("\n3. Schema Comparison:")
    print("-" * 70)
    diff = info_v1.compare(info_v2)

    print(diff.summary())

    # Migration recommendations
    print("\n4. Migration Recommendations:")
    print("-" * 70)
    if diff.has_changes():
        print("   ⚠️  Schema changes detected!")
        print("   📝 Review the changes above before migration")
        print("   ✅ Create migration scripts for:")
        if diff.added_entities:
            print("      - New entities (define in TypeDB)")
        if diff.added_relations:
            print("      - New relations (define in TypeDB)")
        if diff.added_attributes:
            print("      - New attributes (define in TypeDB)")
        if diff.modified_entities:
            print("      - Modified entities (update ownership)")
        if diff.removed_entities or diff.removed_relations or diff.removed_attributes:
            print("      - Removed items (⚠️ data migration may be needed)")
    else:
        print("   ✅ No schema changes - no migration needed")

    print("\n" + "=" * 70)
    print("Demo Complete")
    print("=" * 70)


if __name__ == "__main__":
    main()
