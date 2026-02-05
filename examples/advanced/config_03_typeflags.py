"""Example: TypeFlags - Unified flags for Entities and Relations.

This example demonstrates the TypeFlags class for configuring
Entity and Relation types.

Key features:
- Use TypeFlags for both entities and relations
- Use 'name' parameter to set type name
- Use 'abstract' and 'base' flags for type configuration
"""

from type_bridge import (
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    Role,
    String,
    TypeFlags,
    TypeNameCase,
)


# Define attributes
class Name(String):
    pass


class Age(Integer):
    pass


class Position(String):
    pass


print("=" * 80)
print("TypeFlags - Unified Flags for Entities and Relations")
print("=" * 80)
print()

# Example 1: Entity with TypeFlags
print("Example 1: Entity with TypeFlags")
print("-" * 80)


class Person(Entity):
    flags: TypeFlags = TypeFlags(name="person")  # Use 'name' instead of 'type_name'
    age: Age
    name: Name = Flag(Key)


print(f"Person type name: {Person.get_type_name()}")  # → "person"
print()
print("Schema:")
print(Person.to_schema_definition())
print()

# Example 2: Relation with TypeFlags
print("Example 2: Relation with TypeFlags")
print("-" * 80)


class Company(Entity):
    flags: TypeFlags = TypeFlags(name="company")
    name: Name = Flag(Key)


class Employment(Relation):
    flags: TypeFlags = TypeFlags(name="employment")  # Same TypeFlags class!
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)
    position: Position


print(f"Employment type name: {Employment.get_type_name()}")  # → "employment"
print()
print("Schema:")
print(Employment.to_schema_definition())
print()

# Example 3: Using case formatting
print("Example 3: Case formatting with TypeFlags")
print("-" * 80)


class PersonData(Entity):
    flags: TypeFlags = TypeFlags(
        case=TypeNameCase.SNAKE_CASE
    )  # No 'name', uses CLASS_NAME → snake_case
    name: Name = Flag(Key)


class CompanyData(Entity):
    flags: TypeFlags = TypeFlags()  # No 'name', uses CLASS_NAME default
    name: Name = Flag(Key)


print(f"PersonData type name: {PersonData.get_type_name()}")  # → "person_data"
print(f"CompanyData type name: {CompanyData.get_type_name()}")  # → "CompanyData"
print()

# Example 4: Abstract and base flags
print("Example 4: Abstract and base flags")
print("-" * 80)


class AbstractEntity(Entity):
    flags: TypeFlags = TypeFlags(abstract=True, name="abstract_entity")
    name: Name


class BaseEntity(Entity):
    flags: TypeFlags = TypeFlags(base=True)  # Python-only base class


print(f"AbstractEntity is abstract: {AbstractEntity.is_abstract()}")
print(f"BaseEntity is base: {BaseEntity.is_base()}")
print(
    f"BaseEntity schema: {BaseEntity.to_schema_definition()}"
)  # → None (base classes don't generate schema)
print()

# Summary
print("=" * 80)
print("Summary")
print("=" * 80)
print()
print("TypeFlags API:")
print("  • Use TypeFlags for both entities and relations")
print("  • Use 'name' parameter to set type name")
print("  • Use 'case' parameter for automatic name formatting")
print("  • Use 'abstract=True' for abstract types")
print("  • Use 'base=True' for Python-only base classes")
print("  • Example: flags = TypeFlags(name='person')")
print()
print("=" * 80)
