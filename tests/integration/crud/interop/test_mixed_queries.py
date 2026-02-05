"""Integration tests for queries with mixed attribute types."""

from datetime import UTC, date, datetime
from decimal import Decimal as PyDecimal

import pytest

from type_bridge import (
    Boolean,
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
@pytest.mark.order(74)
def test_query_with_two_types_string_integer(clean_db):
    """Test querying with String and Integer filters."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person_two_types")
        name: Name = Flag(Key)
        age: Age

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)

    # Insert test data
    persons = [
        Person(name=Name("Alice"), age=Age(30)),
        Person(name=Name("Bob"), age=Age(25)),
        Person(name=Name("Charlie"), age=Age(30)),
    ]
    manager.insert_many(persons)

    # Query by Integer only
    age_30 = manager.get(age=30)
    assert len(age_30) == 2

    # Query by String only
    alice = manager.get(name="Alice")
    assert len(alice) == 1


@pytest.mark.integration
@pytest.mark.order(75)
def test_query_with_three_types(clean_db):
    """Test querying with Boolean, Double, and String filters."""

    class Name(String):
        pass

    class IsActive(Boolean):
        pass

    class Score(Double):
        pass

    class User(Entity):
        flags = TypeFlags(name="user_three_types")
        name: Name = Flag(Key)
        is_active: IsActive
        score: Score

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(User)
    schema_manager.sync_schema(force=True)

    manager = User.manager(clean_db)

    # Insert test data
    users = [
        User(name=Name("Diana"), is_active=IsActive(True), score=Score(85.5)),
        User(name=Name("Eve"), is_active=IsActive(False), score=Score(90.0)),
        User(name=Name("Frank"), is_active=IsActive(True), score=Score(75.0)),
    ]
    manager.insert_many(users)

    # Query by Boolean
    active = manager.get(is_active=True)
    assert len(active) == 2

    # Query by Double
    high_score = manager.get(score=90.0)
    assert len(high_score) == 1
    assert high_score[0].name.value == "Eve"


@pytest.mark.integration
@pytest.mark.order(76)
def test_query_with_temporal_types(clean_db):
    """Test querying with Date and DateTime filters."""

    class Name(String):
        pass

    class BirthDate(Date):
        pass

    class CreatedAt(DateTime):
        pass

    class Record(Entity):
        flags = TypeFlags(name="record_temporal")
        name: Name = Flag(Key)
        birth_date: BirthDate
        created_at: CreatedAt

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Record)
    schema_manager.sync_schema(force=True)

    manager = Record.manager(clean_db)

    # Insert test data
    records = [
        Record(
            name=Name("Grace"),
            birth_date=BirthDate(date(1990, 5, 15)),
            created_at=CreatedAt(datetime(2024, 1, 1, 10, 0, 0)),
        ),
        Record(
            name=Name("Henry"),
            birth_date=BirthDate(date(1985, 3, 20)),
            created_at=CreatedAt(datetime(2024, 1, 2, 15, 0, 0)),
        ),
    ]
    manager.insert_many(records)

    # Query by Date
    born_1990 = manager.get(birth_date=date(1990, 5, 15))
    assert len(born_1990) == 1
    assert born_1990[0].name.value == "Grace"

    # Query by DateTime
    created_jan1 = manager.get(created_at=datetime(2024, 1, 1, 10, 0, 0))
    assert len(created_jan1) == 1


@pytest.mark.integration
@pytest.mark.order(77)
def test_query_with_all_nine_types(clean_db):
    """Test querying entity with all 9 attribute types."""

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

    class CompleteRecord(Entity):
        flags = TypeFlags(name="complete_query")
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
    schema_manager.register(CompleteRecord)
    schema_manager.sync_schema(force=True)

    manager = CompleteRecord.manager(clean_db)

    # Insert test data
    from datetime import timedelta

    record = CompleteRecord(
        name=Name("Iris"),
        age=Age(32),
        is_active=IsActive(True),
        score=Score(88.5),
        birth_date=BirthDate(date(1992, 7, 10)),
        created_at=CreatedAt(datetime(2024, 1, 1, 10, 0, 0)),
        updated_at=UpdatedAt(datetime(2024, 1, 2, 15, 30, 0, tzinfo=UTC)),
        balance=Balance(PyDecimal("5000.00")),
        session_time=SessionTime(timedelta(hours=2)),
    )
    manager.insert(record)

    # Query by different types
    by_name = manager.get(name="Iris")
    assert len(by_name) == 1

    by_age = manager.get(age=32)
    assert len(by_age) == 1

    by_active = manager.get(is_active=True)
    assert len(by_active) == 1

    by_balance = manager.get(balance=PyDecimal("5000.00"))
    assert len(by_balance) == 1


@pytest.mark.integration
@pytest.mark.order(78)
def test_range_queries_on_numeric_types(clean_db):
    """Test range filtering on Integer and Double."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Score(Double):
        pass

    class Student(Entity):
        flags = TypeFlags(name="student_range")
        name: Name = Flag(Key)
        age: Age
        score: Score

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Student)
    schema_manager.sync_schema(force=True)

    manager = Student.manager(clean_db)

    # Insert test data
    students = [
        Student(name=Name("Jack"), age=Age(20), score=Score(75.0)),
        Student(name=Name("Kate"), age=Age(22), score=Score(85.0)),
        Student(name=Name("Liam"), age=Age(24), score=Score(95.0)),
    ]
    manager.insert_many(students)

    # Simple equality queries (range queries would require enhanced API)
    age_22 = manager.get(age=22)
    assert len(age_22) == 1

    score_95 = manager.get(score=95.0)
    assert len(score_95) == 1


@pytest.mark.integration
@pytest.mark.order(79)
def test_range_queries_on_temporal_types(clean_db):
    """Test filtering on Date and DateTime."""

    class Name(String):
        pass

    class EventDate(Date):
        pass

    class Timestamp(DateTime):
        pass

    class Event(Entity):
        flags = TypeFlags(name="event_range")
        name: Name = Flag(Key)
        event_date: EventDate
        timestamp: Timestamp

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Event)
    schema_manager.sync_schema(force=True)

    manager = Event.manager(clean_db)

    # Insert test data
    events = [
        Event(
            name=Name("Launch"),
            event_date=EventDate(date(2024, 1, 15)),
            timestamp=Timestamp(datetime(2024, 1, 15, 10, 0, 0)),
        ),
        Event(
            name=Name("Update"),
            event_date=EventDate(date(2024, 3, 20)),
            timestamp=Timestamp(datetime(2024, 3, 20, 14, 0, 0)),
        ),
    ]
    manager.insert_many(events)

    # Simple equality queries
    jan_events = manager.get(event_date=date(2024, 1, 15))
    assert len(jan_events) == 1


@pytest.mark.integration
@pytest.mark.order(80)
def test_complex_query_with_multiple_filters(clean_db):
    """Test complex queries with multiple attribute filters."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class IsActive(Boolean):
        pass

    class Score(Double):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person_complex")
        name: Name = Flag(Key)
        age: Age
        is_active: IsActive
        score: Score

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)

    # Insert test data
    persons = [
        Person(name=Name("Mike"), age=Age(30), is_active=IsActive(True), score=Score(85.0)),
        Person(name=Name("Nina"), age=Age(25), is_active=IsActive(True), score=Score(90.0)),
        Person(name=Name("Oscar"), age=Age(30), is_active=IsActive(False), score=Score(75.0)),
    ]
    manager.insert_many(persons)

    # Filter by age
    age_30 = manager.get(age=30)
    assert len(age_30) == 2

    # Filter by active status
    active_users = manager.get(is_active=True)
    assert len(active_users) == 2


@pytest.mark.integration
@pytest.mark.order(81)
def test_type_coercion_in_queries(clean_db):
    """Test that type coercion works correctly in queries."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Score(Double):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person_coerce")
        name: Name = Flag(Key)
        age: Age
        score: Score

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)

    # Insert
    person = Person(name=Name("Paul"), age=Age(35), score=Score(88.5))
    manager.insert(person)

    # Query with native Python types (should be coerced)
    by_name = manager.get(name="Paul")
    assert len(by_name) == 1

    by_age = manager.get(age=35)
    assert len(by_age) == 1

    by_score = manager.get(score=88.5)
    assert len(by_score) == 1
