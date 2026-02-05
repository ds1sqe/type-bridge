"""Integration tests for inherited attribute filtering in subtypes.

Tests that dictionary-based filters (get, delete) work correctly with
attributes inherited from parent classes.

Bug reference: Filters on inherited attributes were silently ignored because
get_owned_attributes() only returns directly-owned attributes.
"""

import pytest

from type_bridge import Entity, Flag, Integer, Key, SchemaManager, String, TypeFlags


class LivingName(String):
    """Name attribute for Living entities."""

    pass


class LivingAge(Integer):
    """Age attribute for Living entities."""

    pass


class Living(Entity):
    """Abstract base entity with name key."""

    flags = TypeFlags(abstract=True, name="living")
    name: LivingName = Flag(Key)


class Animal(Living):
    """Abstract animal entity inheriting from Living."""

    flags = TypeFlags(abstract=True, name="animal")


class Dog(Animal):
    """Concrete Dog entity - inherits name from Living."""

    flags = TypeFlags(name="dog")
    age: LivingAge | None = None


class Cat(Animal):
    """Concrete Cat entity - inherits name from Living."""

    flags = TypeFlags(name="cat")


@pytest.mark.integration
@pytest.mark.order(130)
def test_get_subtype_by_inherited_key_attribute(clean_db):
    """Test that get() with inherited key attribute filter returns correct results.

    This tests the fix for the bug where dictionary-based filters on inherited
    attributes were silently ignored.
    """
    # Arrange
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Living, Animal, Dog, Cat)
    schema_manager.sync_schema(force=True)

    dog_manager = Dog.manager(clean_db)

    # Insert test data
    buddy = Dog(name=LivingName("Buddy"), age=LivingAge(3))
    max_dog = Dog(name=LivingName("Max"), age=LivingAge(5))
    dog_manager.insert_many([buddy, max_dog])

    cat_manager = Cat.manager(clean_db)
    whiskers = Cat(name=LivingName("Whiskers"))
    cat_manager.insert(whiskers)

    # Act - Query Dog by inherited 'name' attribute
    result = dog_manager.get(name=LivingName("Buddy"))

    # Assert - Should return only the dog named "Buddy"
    assert len(result) == 1
    assert result[0].name.value == "Buddy"
    assert result[0].age is not None
    assert result[0].age.value == 3


@pytest.mark.integration
@pytest.mark.order(131)
def test_get_subtype_by_inherited_attribute_no_match(clean_db):
    """Test that get() with inherited attribute returns empty when no match."""
    # Arrange
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Living, Animal, Dog, Cat)
    schema_manager.sync_schema(force=True)

    dog_manager = Dog.manager(clean_db)
    buddy = Dog(name=LivingName("Buddy"))
    dog_manager.insert(buddy)

    # Act - Query for non-existent name
    result = dog_manager.get(name=LivingName("NonExistent"))

    # Assert - Should return empty list
    assert len(result) == 0


@pytest.mark.integration
@pytest.mark.order(132)
def test_delete_subtype_by_inherited_attribute(clean_db):
    """Test that delete() with inherited attribute filter works correctly."""
    # Arrange
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Living, Animal, Dog, Cat)
    schema_manager.sync_schema(force=True)

    dog_manager = Dog.manager(clean_db)
    buddy = Dog(name=LivingName("Buddy"))
    max_dog = Dog(name=LivingName("Max"))
    dog_manager.insert_many([buddy, max_dog])

    # Act - Delete dog by inherited 'name' attribute using filter
    dog_manager.filter(name=LivingName("Buddy")).delete()

    # Assert - Only Max should remain
    remaining = dog_manager.all()
    assert len(remaining) == 1
    assert remaining[0].name.value == "Max"


@pytest.mark.integration
@pytest.mark.order(133)
def test_query_delete_with_inherited_attribute_filter(clean_db):
    """Test that EntityQuery.delete() works with inherited attribute filters."""
    # Arrange
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Living, Animal, Dog, Cat)
    schema_manager.sync_schema(force=True)

    dog_manager = Dog.manager(clean_db)
    buddy = Dog(name=LivingName("Buddy"), age=LivingAge(3))
    max_dog = Dog(name=LivingName("Max"), age=LivingAge(5))
    luna = Dog(name=LivingName("Luna"), age=LivingAge(2))
    dog_manager.insert_many([buddy, max_dog, luna])

    # Act - Delete using query with dictionary filter on inherited attribute
    dog_manager.filter(name=LivingName("Max")).delete()

    # Assert - Max should be deleted
    remaining = dog_manager.all()
    assert len(remaining) == 2
    names = {dog.name.value for dog in remaining}
    assert names == {"Buddy", "Luna"}


@pytest.mark.integration
@pytest.mark.order(134)
def test_get_with_both_inherited_and_owned_attributes(clean_db):
    """Test filtering by both inherited and directly-owned attributes."""
    # Arrange
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Living, Animal, Dog, Cat)
    schema_manager.sync_schema(force=True)

    dog_manager = Dog.manager(clean_db)
    buddy = Dog(name=LivingName("Buddy"), age=LivingAge(3))
    max_dog = Dog(name=LivingName("Max"), age=LivingAge(5))
    luna = Dog(name=LivingName("Luna"), age=LivingAge(3))
    dog_manager.insert_many([buddy, max_dog, luna])

    # Act - Query by inherited name AND owned age
    result = dog_manager.get(name=LivingName("Buddy"), age=LivingAge(3))

    # Assert - Should return only Buddy (name matches AND age matches)
    assert len(result) == 1
    assert result[0].name.value == "Buddy"
    assert result[0].age is not None
    assert result[0].age.value == 3
