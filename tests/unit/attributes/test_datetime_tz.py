"""Test DateTimeTZ attribute type and conversions between DateTime and DateTimeTZ."""

from datetime import UTC, datetime, timedelta, timezone

import pytest

from type_bridge import DateTime, DateTimeTZ, Entity, TypeFlags


def test_datetimetz_creation():
    """Test creating DateTimeTZ with timezone-aware datetime."""

    class CreatedAt(DateTimeTZ):
        pass

    # Test with UTC timezone
    dt_utc = datetime(2024, 1, 15, 10, 30, 45, tzinfo=UTC)
    created_at = CreatedAt(dt_utc)
    assert created_at.value == dt_utc
    assert created_at.value.tzinfo == UTC


def test_datetimetz_requires_timezone():
    """Test that DateTimeTZ requires timezone-aware datetime."""

    class CreatedAt(DateTimeTZ):
        pass

    # Test with naive datetime - should raise ValueError
    dt_naive = datetime(2024, 1, 15, 10, 30, 45)
    with pytest.raises(ValueError, match="DateTimeTZ requires timezone-aware datetime"):
        CreatedAt(dt_naive)


def test_datetimetz_value_type():
    """Test that DateTimeTZ has correct value_type for TypeDB."""

    class CreatedAt(DateTimeTZ):
        pass

    assert CreatedAt.value_type == "datetime-tz"


def test_datetimetz_in_entity():
    """Test using DateTimeTZ in an entity."""

    class CreatedAt(DateTimeTZ):
        pass

    class Event(Entity):
        flags = TypeFlags(name="event")
        created_at: CreatedAt

    # Create entity with timezone-aware datetime
    dt_utc = datetime(2024, 1, 15, 10, 30, 45, 123456, tzinfo=UTC)
    event = Event(created_at=CreatedAt(dt_utc))

    # Verify insert query
    query = event.to_insert_query()
    assert "$e isa event" in query
    assert "has CreatedAt 2024-01-15T10:30:45.123456+00:00" in query


def test_datetime_add_timezone_implicit():
    """Test implicit conversion from DateTime to DateTimeTZ (adds system timezone)."""

    class CreatedAt(DateTime):
        pass

    # Create naive datetime
    dt_naive = datetime(2024, 1, 15, 10, 30, 45)
    created_at = CreatedAt(dt_naive)

    # Convert to DateTimeTZ (implicit - adds system timezone)
    aware_dt = created_at.add_timezone()

    # Verify it's a DateTimeTZ instance
    assert isinstance(aware_dt, DateTimeTZ)
    # Verify it has timezone info
    assert aware_dt.value.tzinfo is not None
    # Verify the datetime values match (ignoring timezone)
    assert aware_dt.value.replace(tzinfo=None) == dt_naive


def test_datetime_add_timezone_explicit():
    """Test explicit conversion from DateTime to DateTimeTZ (adds specified timezone)."""

    class CreatedAt(DateTime):
        pass

    # Create naive datetime
    dt_naive = datetime(2024, 1, 15, 10, 30, 45)
    created_at = CreatedAt(dt_naive)

    # Convert to DateTimeTZ with UTC
    aware_utc = created_at.add_timezone(UTC)
    assert isinstance(aware_utc, DateTimeTZ)
    assert aware_utc.value.tzinfo == UTC
    assert aware_utc.value == dt_naive.replace(tzinfo=UTC)

    # Convert to DateTimeTZ with JST (+9)
    jst = timezone(timedelta(hours=9))
    aware_jst = created_at.add_timezone(jst)
    assert isinstance(aware_jst, DateTimeTZ)
    assert aware_jst.value.tzinfo == jst
    assert aware_jst.value == dt_naive.replace(tzinfo=jst)


def test_datetimetz_strip_timezone_implicit():
    """Test implicit conversion from DateTimeTZ to DateTime (strips timezone)."""

    class CreatedAt(DateTimeTZ):
        pass

    # Create timezone-aware datetime
    dt_utc = datetime(2024, 1, 15, 10, 30, 45, tzinfo=UTC)
    created_at = CreatedAt(dt_utc)

    # Convert to DateTime (implicit - just strip timezone)
    naive_dt = created_at.strip_timezone()

    # Verify it's a DateTime instance
    assert isinstance(naive_dt, DateTime)
    # Verify it has no timezone info
    assert naive_dt.value.tzinfo is None
    # Verify the datetime values match
    assert naive_dt.value == datetime(2024, 1, 15, 10, 30, 45)


def test_datetimetz_strip_timezone_explicit():
    """Test explicit conversion from DateTimeTZ to DateTime (converts to tz, then strips)."""

    class CreatedAt(DateTimeTZ):
        pass

    # Create timezone-aware datetime in UTC
    dt_utc = datetime(2024, 1, 15, 10, 30, 45, tzinfo=UTC)
    created_at = CreatedAt(dt_utc)

    # Convert to JST (+9), then strip timezone
    jst = timezone(timedelta(hours=9))
    naive_jst = created_at.strip_timezone(jst)

    # Verify it's a DateTime instance
    assert isinstance(naive_jst, DateTime)
    # Verify it has no timezone info
    assert naive_jst.value.tzinfo is None
    # Verify the datetime is converted to JST
    # 2024-01-15 10:30:45 UTC = 2024-01-15 19:30:45 JST (+9 hours)
    assert naive_jst.value == datetime(2024, 1, 15, 19, 30, 45)


def test_round_trip_conversions():
    """Test round-trip conversions between DateTime and DateTimeTZ."""

    class CreatedAt(DateTime):
        pass

    # Start with naive datetime
    original_naive = datetime(2024, 1, 15, 10, 30, 45)
    dt = CreatedAt(original_naive)

    # Convert to DateTimeTZ with UTC
    aware = dt.add_timezone(UTC)
    assert isinstance(aware, DateTimeTZ)
    assert aware.value.tzinfo == UTC

    # Convert back to DateTime
    naive = aware.strip_timezone()
    assert isinstance(naive, DateTime)
    assert naive.value.tzinfo is None
    assert naive.value == original_naive


def test_timezone_conversion_chain():
    """Test chaining timezone conversions."""

    class CreatedAt(DateTime):
        pass

    # Start with naive datetime in JST time (19:30:45)
    dt_naive = datetime(2024, 1, 15, 19, 30, 45)
    dt = CreatedAt(dt_naive)

    # Add JST timezone
    jst = timezone(timedelta(hours=9))
    dt_jst = dt.add_timezone(jst)
    assert dt_jst.value == datetime(2024, 1, 15, 19, 30, 45, tzinfo=jst)

    # Convert to UTC timezone, then strip
    dt_utc_naive = dt_jst.strip_timezone(UTC)
    # 19:30:45 JST = 10:30:45 UTC (-9 hours)
    assert dt_utc_naive.value == datetime(2024, 1, 15, 10, 30, 45)
    assert dt_utc_naive.value.tzinfo is None


def test_datetimetz_pydantic_validation():
    """Test Pydantic validation for DateTimeTZ in entities."""

    class UpdatedAt(DateTimeTZ):
        pass

    class Event(Entity):
        flags = TypeFlags(name="event")
        updated_at: UpdatedAt

    # Test with timezone-aware datetime
    dt_utc = datetime(2024, 1, 15, 10, 30, 45, tzinfo=UTC)
    event = Event(updated_at=UpdatedAt(dt_utc))
    assert event.updated_at.value == dt_utc

    # Test that naive datetime raises error through Pydantic
    dt_naive = datetime(2024, 1, 15, 10, 30, 45)
    with pytest.raises(ValueError, match="DateTimeTZ requires timezone-aware datetime"):
        Event(updated_at=UpdatedAt(dt_naive))


def test_datetime_value_type():
    """Test that DateTime has correct value_type for TypeDB."""

    class CreatedAt(DateTime):
        pass

    assert CreatedAt.value_type == "datetime"


def test_mixed_datetime_types_in_entity():
    """Test using both DateTime and DateTimeTZ in the same entity."""

    class CreatedAt(DateTime):
        pass

    class UpdatedAt(DateTimeTZ):
        pass

    class Event(Entity):
        flags = TypeFlags(name="event")
        created_at: CreatedAt
        updated_at: UpdatedAt

    # Create entity with both types
    dt_naive = datetime(2024, 1, 15, 10, 30, 45)
    dt_aware = datetime(2024, 1, 15, 11, 0, 0, tzinfo=UTC)

    event = Event(created_at=CreatedAt(dt_naive), updated_at=UpdatedAt(dt_aware))

    # Verify insert query has both
    query = event.to_insert_query()
    assert "$e isa event" in query
    assert "has CreatedAt 2024-01-15T10:30:45" in query
    assert "has UpdatedAt 2024-01-15T11:00:00+00:00" in query


def test_datetimetz_with_different_timezones():
    """Test DateTimeTZ with various timezone formats."""

    class EventTime(DateTimeTZ):
        pass

    # UTC timezone
    dt_utc = datetime(2024, 1, 15, 10, 30, 45, tzinfo=UTC)
    event_utc = EventTime(dt_utc)
    assert "2024-01-15T10:30:45+00:00" in str(event_utc.value.isoformat())

    # Positive offset timezone (JST +9)
    jst = timezone(timedelta(hours=9))
    dt_jst = datetime(2024, 1, 15, 19, 30, 45, tzinfo=jst)
    event_jst = EventTime(dt_jst)
    assert "2024-01-15T19:30:45+09:00" in str(event_jst.value.isoformat())

    # Negative offset timezone (EST -5)
    est = timezone(timedelta(hours=-5))
    dt_est = datetime(2024, 1, 15, 5, 30, 45, tzinfo=est)
    event_est = EventTime(dt_est)
    assert "2024-01-15T05:30:45-05:00" in str(event_est.value.isoformat())
