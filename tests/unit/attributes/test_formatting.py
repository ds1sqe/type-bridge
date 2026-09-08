"""Test attribute formatting in insert queries with multiple attribute types."""

from datetime import UTC, datetime

from tests.utils.handwritten import (
    Boolean,
    Card,
    DateTime,
    Double,
    Entity,
    Flag,
    Integer,
    Key,
    String,
    TypeFlags,
)


def test_all_attribute_types_insert_query():
    """Test insert query generation with all attribute types in one entity."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Salary(Double):
        pass

    class IsActive(Boolean):
        pass

    class HireDate(DateTime):
        pass

    class Employee(Entity):
        flags = TypeFlags(name="employee")
        name: Name = Flag(Key)
        age: Age
        salary: Salary
        is_active: IsActive
        hire_date: HireDate

    # Create entity with all attribute types
    hire_dt = datetime(2024, 1, 15, 9, 0, 0)
    employee = Employee(
        name=Name("Alice Smith"),
        age=Age(30),
        salary=Salary(75000.50),
        is_active=IsActive(True),
        hire_date=HireDate(hire_dt),
    )

    query = employee.to_insert_query()

    # Validate entity type
    assert "$e isa employee" in query

    # Validate String attribute (quoted)
    assert 'has Name "Alice Smith"' in query

    # Validate Integer attribute (unquoted number)
    assert "has Age 30" in query

    # Validate Double attribute (unquoted float)
    assert "has Salary 75000.5" in query

    # Validate Boolean attribute (unquoted lowercase)
    assert "has IsActive true" in query

    # Validate DateTime attribute (unquoted ISO 8601)
    assert "has HireDate 2024-01-15T09:00:00" in query


def test_optional_attribute_insert_query():
    """Test that optional attributes with None values are excluded from insert query."""

    class Name(String):
        pass

    class Email(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        email: Email | None

    # Create entity with None optional attribute
    person = Person(name=Name("Bob"), email=None)
    query = person.to_insert_query()

    assert "$e isa person" in query
    assert 'has Name "Bob"' in query
    # Email should NOT appear in query when None
    assert "has Email" not in query


def test_mixed_optional_and_required_attributes():
    """Test insert query with mix of required and optional attributes."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Email(String):
        pass

    class Phone(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age
        email: Email | None
        phone: Phone | None

    # Create entity with some optional attributes set, some None
    person = Person(
        name=Name("Charlie"), age=Age(25), email=Email("charlie@example.com"), phone=None
    )
    query = person.to_insert_query()

    assert "$e isa person" in query
    assert 'has Name "Charlie"' in query
    assert "has Age 25" in query
    assert 'has Email "charlie@example.com"' in query
    # Phone should NOT appear when None
    assert "has Phone" not in query


def test_datetime_insert_query():
    """Test insert query generation with DateTime attributes."""

    class CreatedAt(DateTime):
        pass

    class Event(Entity):
        flags = TypeFlags(name="event")
        created_at: CreatedAt

    # Test with naive datetime
    dt_naive = datetime(2024, 1, 15, 10, 30, 45, 123456)
    event_naive = Event(created_at=CreatedAt(dt_naive))
    query_naive = event_naive.to_insert_query()

    assert "$e isa event" in query_naive
    assert "has CreatedAt 2024-01-15T10:30:45.123456" in query_naive
    # Ensure datetime is NOT quoted (should be datetime literal, not string)
    assert '"2024-01-15T10:30:45.123456"' not in query_naive

    # Test with timezone-aware datetime
    dt_aware = datetime(2024, 1, 15, 10, 30, 45, 123456, tzinfo=UTC)
    event_aware = Event(created_at=CreatedAt(dt_aware))
    query_aware = event_aware.to_insert_query()

    assert "$e isa event" in query_aware
    assert "has CreatedAt 2024-01-15T10:30:45.123456+00:00" in query_aware
    # Ensure datetime is NOT quoted
    assert '"2024-01-15T10:30:45.123456+00:00"' not in query_aware


def test_multi_value_datetime_insert_query():
    """Test insert query generation with multi-value DateTime attributes."""

    class EventDate(DateTime):
        pass

    class Schedule(Entity):
        flags = TypeFlags(name="schedule")
        event_dates: list[EventDate] = Flag(Card(min=1))

    # Create entity with multiple datetime values
    dates = [
        EventDate(datetime(2024, 1, 15, 9, 0, 0)),
        EventDate(datetime(2024, 2, 20, 14, 30, 0)),
        EventDate(datetime(2024, 3, 10, 18, 0, 0)),
    ]
    schedule = Schedule(event_dates=dates)
    query = schedule.to_insert_query()

    assert "$e isa schedule" in query
    assert "has EventDate 2024-01-15T09:00:00" in query
    assert "has EventDate 2024-02-20T14:30:00" in query
    assert "has EventDate 2024-03-10T18:00:00" in query
