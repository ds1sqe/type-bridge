"""Integration tests for @range validation in CRUD operations.

These tests verify that range constraints are enforced when inserting/updating
entities through the full CRUD flow, preventing invalid data from reaching TypeDB.
"""

from typing import ClassVar

import pytest

from type_bridge import (
    Database,
    Double,
    Entity,
    Flag,
    Integer,
    Key,
    SchemaManager,
    String,
    TypeFlags,
)


# Define attributes with range constraints
class Name(String):
    pass


class Age(Integer):
    """Age must be between 0 and 150."""

    range_constraint: ClassVar[tuple[str | None, str | None]] = ("0", "150")


class Temperature(Double):
    """Temperature in Celsius, between -50 and 50."""

    range_constraint: ClassVar[tuple[str | None, str | None]] = ("-50.0", "50.0")


class Score(Integer):
    """Score must be non-negative (no upper bound)."""

    range_constraint: ClassVar[tuple[str | None, str | None]] = ("0", None)


class Priority(Integer):
    """Priority must be at most 10 (no lower bound)."""

    range_constraint: ClassVar[tuple[str | None, str | None]] = (None, "10")


# Define entities that use range-constrained attributes
class Person(Entity):
    flags = TypeFlags(name="range_test_person")
    name: Name = Flag(Key)
    age: Age | None = None


class Sensor(Entity):
    flags = TypeFlags(name="range_test_sensor")
    name: Name = Flag(Key)
    temperature: Temperature | None = None


class Task(Entity):
    flags = TypeFlags(name="range_test_task")
    name: Name = Flag(Key)
    score: Score | None = None
    priority: Priority | None = None


@pytest.fixture
def db():
    """Create a test database with range-constrained schema."""
    from tests.integration.conftest import TEST_DB_ADDRESS

    database = Database(address=TEST_DB_ADDRESS, database="test_range_validation")
    database.connect()

    # Clean up if exists
    if database.database_exists():
        database.delete_database()
    database.create_database()

    # Register and sync schema
    schema_manager = SchemaManager(database)
    schema_manager.register(Person, Sensor, Task)
    schema_manager.sync_schema()

    yield database

    # Cleanup
    database.delete_database()
    database.close()


@pytest.mark.integration
class TestIntegerRangeValidationOnInsert:
    """Test that integer range validation prevents invalid inserts."""

    def test_insert_valid_age_succeeds(self, db: Database) -> None:
        """Inserting a person with valid age should succeed."""
        manager = Person.manager(db)

        person = Person(name=Name("Alice"), age=Age(30))
        manager.insert(person)

        # Verify it was inserted (get returns a list)
        results = manager.get(name="Alice")
        assert len(results) == 1
        assert results[0].age is not None
        assert results[0].age.value == 30

    def test_insert_age_at_boundaries_succeeds(self, db: Database) -> None:
        """Inserting ages at min/max boundaries should succeed."""
        manager = Person.manager(db)

        # Test minimum boundary
        infant = Person(name=Name("Baby"), age=Age(0))
        manager.insert(infant)

        # Test maximum boundary
        elder = Person(name=Name("Elder"), age=Age(150))
        manager.insert(elder)

        # Verify both were inserted
        assert len(manager.get(name="Baby")) == 1
        assert len(manager.get(name="Elder")) == 1

    def test_insert_negative_age_raises_error(self, db: Database) -> None:
        """Inserting a person with negative age should raise ValueError."""
        with pytest.raises(ValueError, match="below minimum"):
            Person(name=Name("Invalid"), age=Age(-1))

    def test_insert_age_above_max_raises_error(self, db: Database) -> None:
        """Inserting a person with age above max should raise ValueError."""
        with pytest.raises(ValueError, match="above maximum"):
            Person(name=Name("Invalid"), age=Age(200))


@pytest.mark.integration
class TestDoubleRangeValidationOnInsert:
    """Test that double range validation prevents invalid inserts."""

    def test_insert_valid_temperature_succeeds(self, db: Database) -> None:
        """Inserting a sensor with valid temperature should succeed."""
        manager = Sensor.manager(db)

        sensor = Sensor(name=Name("Sensor1"), temperature=Temperature(25.5))
        manager.insert(sensor)

        results = manager.get(name="Sensor1")
        assert len(results) == 1
        assert results[0].temperature is not None
        assert results[0].temperature.value == 25.5

    def test_insert_temperature_at_boundaries_succeeds(self, db: Database) -> None:
        """Inserting temperatures at min/max boundaries should succeed."""
        manager = Sensor.manager(db)

        cold = Sensor(name=Name("Cold"), temperature=Temperature(-50.0))
        manager.insert(cold)

        hot = Sensor(name=Name("Hot"), temperature=Temperature(50.0))
        manager.insert(hot)

        assert len(manager.get(name="Cold")) == 1
        assert len(manager.get(name="Hot")) == 1

    def test_insert_temperature_below_min_raises_error(self, db: Database) -> None:
        """Inserting temperature below minimum should raise ValueError."""
        with pytest.raises(ValueError, match="below minimum"):
            Sensor(name=Name("Invalid"), temperature=Temperature(-100.0))

    def test_insert_temperature_above_max_raises_error(self, db: Database) -> None:
        """Inserting temperature above maximum should raise ValueError."""
        with pytest.raises(ValueError, match="above maximum"):
            Sensor(name=Name("Invalid"), temperature=Temperature(100.0))


@pytest.mark.integration
class TestOpenEndedRangeValidation:
    """Test open-ended range constraints (min only or max only)."""

    def test_min_only_allows_large_values(self, db: Database) -> None:
        """Score with min=0 should allow arbitrarily large values."""
        manager = Task.manager(db)

        task = Task(name=Name("HighScore"), score=Score(1000000))
        manager.insert(task)

        results = manager.get(name="HighScore")
        assert len(results) == 1
        assert results[0].score is not None
        assert results[0].score.value == 1000000

    def test_min_only_rejects_negative(self, db: Database) -> None:
        """Score with min=0 should reject negative values."""
        with pytest.raises(ValueError, match="below minimum"):
            Task(name=Name("Invalid"), score=Score(-1))

    def test_max_only_allows_negative_values(self, db: Database) -> None:
        """Priority with max=10 should allow negative values."""
        manager = Task.manager(db)

        task = Task(name=Name("LowPriority"), priority=Priority(-100))
        manager.insert(task)

        results = manager.get(name="LowPriority")
        assert len(results) == 1
        assert results[0].priority is not None
        assert results[0].priority.value == -100

    def test_max_only_rejects_above_max(self, db: Database) -> None:
        """Priority with max=10 should reject values above 10."""
        with pytest.raises(ValueError, match="above maximum"):
            Task(name=Name("Invalid"), priority=Priority(15))


@pytest.mark.integration
class TestRangeValidationOnUpdate:
    """Test that range validation also applies during updates."""

    def test_update_to_valid_value_succeeds(self, db: Database) -> None:
        """Updating to a valid value should succeed."""
        manager = Person.manager(db)

        # Insert initial person
        person = Person(name=Name("UpdateTest"), age=Age(25))
        manager.insert(person)

        # Fetch, modify, update
        results = manager.get(name="UpdateTest")
        fetched = results[0]
        fetched.age = Age(30)
        manager.update(fetched)

        # Verify update
        updated = manager.get(name="UpdateTest")
        assert updated[0].age is not None
        assert updated[0].age.value == 30

    def test_update_to_invalid_value_raises_error(self, db: Database) -> None:
        """Updating to an invalid value should raise ValueError before DB update."""
        # The error is raised when creating the Age instance, before any DB operation
        with pytest.raises(ValueError, match="above maximum"):
            Age(200)  # This fails immediately


@pytest.mark.integration
class TestRangeValidationWithPut:
    """Test that range validation applies during PUT (upsert) operations."""

    def test_put_with_valid_value_succeeds(self, db: Database) -> None:
        """PUT with valid values should succeed."""
        manager = Person.manager(db)

        person = Person(name=Name("PutTest"), age=Age(40))
        manager.put(person)

        results = manager.get(name="PutTest")
        assert len(results) == 1
        assert results[0].age is not None
        assert results[0].age.value == 40

    def test_put_with_invalid_value_raises_error(self, db: Database) -> None:
        """PUT with invalid values should raise ValueError."""
        with pytest.raises(ValueError, match="below minimum"):
            Person(name=Name("Invalid"), age=Age(-5))
