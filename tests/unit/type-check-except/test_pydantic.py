"""Tests for Pydantic integration features.

NOTE: This test file intentionally uses raw values (strings, ints, etc.) instead of
wrapped Attribute instances to verify Pydantic's type coercion and validation works correctly.
The type: ignore comments are expected and indicate we're testing runtime validation.
"""

import pytest
from pydantic import ValidationError
from type_bridge import Entity, Integer, String, TypeFlags


def test_pydantic_validation():
    """Test that Pydantic validates attribute types."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name
        age: Age

    # Valid creation - testing type coercion
    alice = Person(name="Alice", age=30)
    assert alice.name != "Alice"  # Strict type safety: not equal to raw value
    assert alice.name.value == "Alice"  # Access raw value via .value
    assert alice.age != 30  # Strict type safety: not equal to raw value
    assert alice.age.value == 30  # Access raw value via .value
    assert isinstance(alice.age, Age)  # Wrapped in Age instance
    assert not isinstance(alice.age, int)  # Not a raw int

    # Type coercion works
    bob = Person(name="Bob", age="25")  # String will be converted to int
    assert bob.age != 25  # Strict type safety: not equal to raw value
    assert bob.age.value == 25  # Access raw value via .value
    assert isinstance(bob.age, Age)  # Wrapped in Age instance
    assert not isinstance(bob.age, int)  # Not a raw int
    assert isinstance(bob.age.value, int)  # Not a raw int


def test_pydantic_validation_on_assignment():
    """Test that Pydantic validates on attribute assignment."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name
        age: Age

    alice = Person(name="Alice", age=30)

    # Valid assignment - testing type coercion
    alice.age = 31
    assert alice.age != 31  # Strict type safety: not equal to raw value
    assert alice.age.value == 31  # Access raw value via .value
    assert isinstance(alice.age, Age)  # Wrapped in Age instance
    assert not isinstance(alice.age, int)  # Not a raw int

    # Type coercion on assignment - testing string to int coercion
    alice.age = "32"
    assert alice.age != 32  # Strict type safety: not equal to raw value
    assert alice.age.value == 32  # Access raw value via .value
    assert isinstance(alice.age, Age)  # Wrapped in Age instance
    assert not isinstance(alice.age, int)  # Not a raw int


def test_pydantic_json_serialization():
    """Test Pydantic's JSON serialization."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name
        age: Age

    alice = Person(name="Alice", age=30)

    # Serialize to dict
    alice_dict = alice.model_dump()
    assert alice_dict == {"name": "Alice", "age": 30}

    # Serialize to JSON
    alice_json = alice.model_dump_json()
    assert '"name":"Alice"' in alice_json
    assert '"age":30' in alice_json


def test_pydantic_json_deserialization():
    """Test Pydantic's JSON deserialization."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name
        age: Age

    # Deserialize from dict
    person_data = {"name": "Bob", "age": 25}
    bob = Person(**person_data)
    assert bob.name != "Bob"  # Not equal to raw value
    assert bob.name == Name("Bob")  # Equal to wrapped value
    assert bob.name.value == "Bob"
    assert bob.age != 25  # Not equal to raw value
    assert bob.age == Age(25)  # Equal to wrapped value
    assert bob.age.value == 25

    # Deserialize from JSON
    json_data = '{"name": "Charlie", "age": 35}'
    charlie = Person.model_validate_json(json_data)
    assert charlie.name != "Charlie"  # Not equal to raw value
    assert charlie.name == Name("Charlie")  # Equal to wrapped value
    assert charlie.age != 35  # Not equal to raw value
    assert charlie.age == Age(35)  # Equal to wrapped value


def test_pydantic_model_copy():
    """Test Pydantic's model copy functionality.

    Raw values in update dict are automatically wrapped in Attribute instances.
    """

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name
        age: Age

    alice = Person(name="Alice", age=30)

    # Create a copy with raw values (automatically wrapped)
    alice_older = alice.model_copy(update={"age": 31})
    assert alice_older.name == Name("Alice")  # Equal to wrapped value
    assert alice_older.age == Age(31)  # Equal to wrapped value
    assert alice_older.age != 31  # Not equal to raw value (strict type safety)
    assert alice.age == Age(30)  # Original unchanged


def test_pydantic_with_optional_fields():
    """Test Pydantic with optional fields."""

    class Name(String):
        pass

    class Email(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name
        email: Email | None = None  # Optional field with explicit default

    # Create without optional field - testing type coercion
    alice = Person(name="Alice")
    assert alice.name == Name("Alice")  # Equal to wrapped value
    assert alice.email is None

    # Create with optional field - testing type coercion
    bob = Person(name="Bob", email="bob@example.com")
    assert bob.name == Name("Bob")  # Equal to wrapped value
    assert bob.email == Email("bob@example.com")  # Equal to wrapped value


def test_pydantic_with_default_values():
    """Test Pydantic with default values."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name
        age: Age | None = Age(0)  # Default value (properly wrapped)

    # Create without age - testing type coercion
    alice = Person(name="Alice")
    assert alice.name == Name("Alice")  # Equal to wrapped value
    assert alice.name != "Alice"  # Not Equal to raw value
    assert alice.age == Age(0)  # Equal to wrapped default value

    # Create with age - testing type coercion
    bob = Person(name="Bob", age=25)
    assert bob.name == Name("Bob")  # Equal to wrapped value
    assert bob.age == Age(25)  # Equal to wrapped value


def test_pydantic_type_coercion():
    """Test Pydantic's type validation."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name
        age: Age

    # Valid string accepted and wrapped in Attribute instances - testing type coercion
    alice = Person(name="Alice", age=30)
    assert isinstance(alice.name, Name)  # Wrapped in Name instance
    assert not isinstance(alice.name, str)  # Not a raw str
    assert alice.name != "Alice"  # Not equal to raw value
    assert alice.name == Name("Alice")  # Equal to wrapped value
    assert isinstance(alice.age, Age)  # Wrapped in Age instance
    assert not isinstance(alice.age, int)  # Not a raw int
    assert alice.age != 30  # Not equal to raw value
    assert alice.age == Age(30)  # Equal to wrapped value

    # Another valid instance - testing type coercion
    bob = Person(name="Bob", age=25)
    assert isinstance(bob.name, Name)  # Wrapped in Name instance
    assert bob.name == Name("Bob")  # Equal to wrapped value
    assert isinstance(bob.age, Age)  # Wrapped in Age instance
    assert bob.age == Age(25)  # Equal to wrapped value


def test_pydantic_validation_errors():
    """Test that Pydantic raises validation errors for invalid data."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name
        age: Age

    # Invalid type that can't be coerced - testing validation error
    with pytest.raises(ValidationError) as exc_info:
        Person(name="Alice", age="not_a_number")

    assert "age" in str(exc_info.value)
