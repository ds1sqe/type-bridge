"""Integration tests for filter validation with attribute ownership checking."""

import pytest

from type_bridge import Entity, Flag, Integer, Key, SchemaManager, String, TypeFlags


@pytest.mark.integration
@pytest.mark.order(120)
def test_filter_with_unowned_attribute_raises_error(clean_db):
    """Test that filtering by unowned attribute raises ValueError."""

    # Arrange
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Email(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age  # Person owns age

    class Company(Entity):
        flags = TypeFlags(name="company")
        name: Name = Flag(Key)
        # Company does NOT own age

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, Company)
    schema_manager.sync_schema(force=True)

    manager = Company.manager(clean_db)

    # Act & Assert - Try to filter Company by Age (not owned)
    with pytest.raises(ValueError) as exc_info:
        manager.filter(Age.gt(Age(30)))

    # Assert error message mentions the unowned attribute
    error_msg = str(exc_info.value)
    expected_substring = "Company does not own attribute type Age"
    assert expected_substring in error_msg


@pytest.mark.integration
@pytest.mark.order(121)
def test_filter_with_multiple_unowned_attributes(clean_db):
    """Test that filtering with multiple unowned attributes raises error."""

    # Arrange
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Salary(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age
        salary: Salary

    class Company(Entity):
        flags = TypeFlags(name="company")
        name: Name = Flag(Key)
        # Company does NOT own age or salary

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, Company)
    schema_manager.sync_schema(force=True)

    manager = Company.manager(clean_db)

    # Act & Assert - First unowned attribute should raise error
    with pytest.raises(ValueError) as exc_info:
        manager.filter(Age.gt(Age(30)), Salary.gt(Salary(50000)))

    error_msg = str(exc_info.value)
    # Should mention at least one of the unowned attributes
    assert "Age" in error_msg or "Salary" in error_msg
    assert "does not own" in error_msg


@pytest.mark.integration
@pytest.mark.order(122)
def test_filter_validation_error_message_includes_available_types(clean_db):
    """Test that error message lists available attribute types."""

    # Arrange
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Email(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        email: Email
        # Person does NOT own age

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)

    # Act & Assert
    with pytest.raises(ValueError) as exc_info:
        manager.filter(Age.gt(Age(30)))

    error_msg = str(exc_info.value)

    # Assert error message format
    assert "Person does not own attribute type Age" in error_msg
    assert "Available attribute types:" in error_msg
    assert "Name" in error_msg
    assert "Email" in error_msg


@pytest.mark.integration
@pytest.mark.order(123)
def test_filter_with_inherited_attributes_passes(clean_db):
    """Test that filtering by inherited attributes is allowed."""

    # Arrange
    class Name(String):
        pass

    class Age(Integer):
        pass

    class BasePerson(Entity):
        flags = TypeFlags(name="base_person")
        name: Name = Flag(Key)
        age: Age

    class Employee(BasePerson):
        flags = TypeFlags(name="employee")
        # Inherits name and age from BasePerson

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(BasePerson, Employee)
    schema_manager.sync_schema(force=True)

    # Insert test data
    employee_manager = Employee.manager(clean_db)
    emp = Employee(name=Name("Alice"), age=Age(30))
    employee_manager.insert(emp)

    # Act - Filter by inherited attribute should work
    result = employee_manager.filter(Age.gt(Age(25))).execute()

    # Assert
    expected = 1
    actual = len(result)
    assert expected == actual
    assert result[0].name.value == "Alice"


@pytest.mark.integration
@pytest.mark.order(124)
def test_filter_ownership_check_on_chained_calls(clean_db):
    """Test that ownership validation occurs on each chained filter call."""

    # Arrange
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Salary(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age
        # Person does NOT own salary

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)

    # Act - First filter is valid
    query = manager.filter(Age.gt(Age(25)))

    # Act & Assert - Second filter with unowned attribute should raise
    with pytest.raises(ValueError) as exc_info:
        query.filter(Salary.gt(Salary(50000)))

    error_msg = str(exc_info.value)
    assert "Person does not own attribute type Salary" in error_msg


@pytest.mark.integration
@pytest.mark.order(125)
def test_filter_with_owned_attribute_succeeds(clean_db):
    """Test that filtering by owned attributes works correctly (no error)."""

    # Arrange
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)

    # Insert test data
    alice = Person(name=Name("Alice"), age=Age(30))
    bob = Person(name=Name("Bob"), age=Age(25))
    manager.insert_many([alice, bob])

    # Act - Filter by owned attributes should work without error
    result = manager.filter(Age.gt(Age(27))).execute()

    # Assert
    expected = 1
    actual = len(result)
    assert expected == actual
    assert result[0].name.value == "Alice"
    assert result[0].age.value == 30
