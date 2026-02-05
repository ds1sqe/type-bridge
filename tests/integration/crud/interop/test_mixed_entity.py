"""Integration tests for entities with mixed attribute types."""

from datetime import UTC, date, datetime, timedelta
from decimal import Decimal as PyDecimal

import pytest

from type_bridge import (
    Boolean,
    Card,
    Date,
    DateTime,
    DateTimeTZ,
    Decimal,
    Double,
    Duration,
    Entity,
    Flag,
    Integer,
    Key,
    SchemaManager,
    String,
    TypeFlags,
)


@pytest.mark.integration
@pytest.mark.order(68)
def test_insert_entity_with_all_types(clean_db):
    """Test inserting entity with all 9 attribute types."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class IsActive(Boolean):
        pass

    class Score(Double):
        pass

    class BirthDate(Date):
        pass

    class CreatedAt(DateTime):
        pass

    class UpdatedAt(DateTimeTZ):
        pass

    class Balance(Decimal):
        pass

    class SessionTime(Duration):
        pass

    class Record(Entity):
        flags = TypeFlags(name="mixed_record")
        name: Name = Flag(Key)
        age: Age
        is_active: IsActive
        score: Score
        birth_date: BirthDate
        created_at: CreatedAt
        updated_at: UpdatedAt
        balance: Balance
        session_time: SessionTime

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Record)
    schema_manager.sync_schema(force=True)

    manager = Record.manager(clean_db)

    # Insert record with all types
    record = Record(
        name=Name("Alice"),
        age=Age(30),
        is_active=IsActive(True),
        score=Score(95.5),
        birth_date=BirthDate(date(1994, 3, 15)),
        created_at=CreatedAt(datetime(2024, 1, 1, 10, 0, 0)),
        updated_at=UpdatedAt(datetime(2024, 1, 2, 15, 30, 0, tzinfo=UTC)),
        balance=Balance(PyDecimal("1234.56")),
        session_time=SessionTime(timedelta(hours=2, minutes=30)),
    )
    manager.insert(record)

    # Verify insertion
    results = manager.get(name="Alice")
    assert len(results) == 1
    r = results[0]
    assert r.age.value == 30
    assert r.is_active.value is True
    assert abs(r.score.value - 95.5) < 0.01
    assert r.birth_date.value == date(1994, 3, 15)
    assert r.balance.value == PyDecimal("1234.56")


@pytest.mark.integration
@pytest.mark.order(69)
def test_fetch_entity_with_all_types(clean_db):
    """Test fetching entity by different attribute types."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class IsActive(Boolean):
        pass

    class Score(Double):
        pass

    class Record(Entity):
        flags = TypeFlags(name="fetch_mixed")
        name: Name = Flag(Key)
        age: Age
        is_active: IsActive
        score: Score

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Record)
    schema_manager.sync_schema(force=True)

    manager = Record.manager(clean_db)

    # Insert test data
    records = [
        Record(name=Name("Bob"), age=Age(25), is_active=IsActive(True), score=Score(80.0)),
        Record(name=Name("Charlie"), age=Age(30), is_active=IsActive(False), score=Score(90.0)),
        Record(name=Name("Diana"), age=Age(25), is_active=IsActive(True), score=Score(85.0)),
    ]
    manager.insert_many(records)

    # Fetch by Integer
    age_25 = manager.get(age=25)
    assert len(age_25) == 2

    # Fetch by Boolean
    active = manager.get(is_active=True)
    assert len(active) == 2

    # Fetch by Double
    score_90 = manager.get(score=90.0)
    assert len(score_90) == 1
    assert score_90[0].name.value == "Charlie"


@pytest.mark.integration
@pytest.mark.order(70)
def test_update_multiple_types(clean_db):
    """Test updating multiple attribute types at once."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Score(Double):
        pass

    class IsActive(Boolean):
        pass

    class Record(Entity):
        flags = TypeFlags(name="update_mixed")
        name: Name = Flag(Key)
        age: Age
        score: Score
        is_active: IsActive

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Record)
    schema_manager.sync_schema(force=True)

    manager = Record.manager(clean_db)

    # Insert record
    record = Record(name=Name("Eve"), age=Age(28), score=Score(75.0), is_active=IsActive(False))
    manager.insert(record)

    # Fetch and update multiple types
    results = manager.get(name="Eve")
    r = results[0]
    r.age = Age(29)
    r.score = Score(85.0)
    r.is_active = IsActive(True)
    manager.update(r)

    # Verify updates
    updated = manager.get(name="Eve")
    assert updated[0].age.value == 29
    assert abs(updated[0].score.value - 85.0) < 0.01
    assert updated[0].is_active.value is True


@pytest.mark.integration
@pytest.mark.order(71)
def test_optional_types_mixed_with_required(clean_db):
    """Test entity with optional and required attributes of different types."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Score(Double):
        pass

    class Bio(String):
        pass

    class Record(Entity):
        flags = TypeFlags(name="optional_mixed")
        name: Name = Flag(Key)
        age: Age  # Required
        score: Score | None  # Optional
        bio: Bio | None  # Optional

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Record)
    schema_manager.sync_schema(force=True)

    manager = Record.manager(clean_db)

    # Insert with some optional fields
    record = Record(name=Name("Frank"), age=Age(35), score=Score(88.0), bio=None)
    manager.insert(record)

    # Verify
    results = manager.get(name="Frank")
    assert len(results) == 1
    assert results[0].age.value == 35
    assert isinstance(results[0].score, Score)
    assert abs(results[0].score.value - 88.0) < 0.01


@pytest.mark.integration
@pytest.mark.order(72)
def test_multi_value_mixed_with_single_value(clean_db):
    """Test entity with both single-value and multi-value attributes."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Tag(String):
        pass

    class Score(Double):
        pass

    class Record(Entity):
        flags = TypeFlags(name="multi_mixed")
        name: Name = Flag(Key)
        age: Age  # Single-value
        tags: list[Tag] = Flag(Card(min=1))  # Multi-value
        scores: list[Score] = Flag(Card(min=1, max=5))  # Multi-value

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Record)
    schema_manager.sync_schema(force=True)

    manager = Record.manager(clean_db)

    # Insert with mixed cardinality
    record = Record(
        name=Name("Grace"),
        age=Age(27),
        tags=[Tag("python"), Tag("database")],
        scores=[Score(90.0), Score(85.0), Score(92.0)],
    )
    manager.insert(record)

    # Verify
    results = manager.get(name="Grace")
    assert len(results) == 1
    assert results[0].age.value == 27
    assert len(results[0].tags) == 2
    assert len(results[0].scores) == 3


@pytest.mark.integration
@pytest.mark.order(73)
def test_verify_serialization_all_types(clean_db):
    """Test that all types serialize and deserialize correctly."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class IsActive(Boolean):
        pass

    class Score(Double):
        pass

    class BirthDate(Date):
        pass

    class CreatedAt(DateTime):
        pass

    class Record(Entity):
        flags = TypeFlags(name="serialize_mixed")
        name: Name = Flag(Key)
        age: Age
        is_active: IsActive
        score: Score
        birth_date: BirthDate
        created_at: CreatedAt

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Record)
    schema_manager.sync_schema(force=True)

    manager = Record.manager(clean_db)

    # Insert record
    original = Record(
        name=Name("Henry"),
        age=Age(42),
        is_active=IsActive(True),
        score=Score(99.9),
        birth_date=BirthDate(date(1982, 6, 20)),
        created_at=CreatedAt(datetime(2024, 1, 15, 10, 30, 0)),
    )
    manager.insert(original)

    # Fetch and verify all values match
    results = manager.get(name="Henry")
    fetched = results[0]

    assert fetched.name.value == "Henry"
    assert fetched.age.value == 42
    assert fetched.is_active.value is True
    assert abs(fetched.score.value - 99.9) < 0.01
    assert fetched.birth_date.value == date(1982, 6, 20)
    assert fetched.created_at.value == datetime(2024, 1, 15, 10, 30, 0)
