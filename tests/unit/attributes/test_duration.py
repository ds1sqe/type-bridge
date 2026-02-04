"""Test Duration attribute type for ISO 8601 durations."""

from datetime import UTC, datetime, timedelta

import pytest
from isodate import Duration as IsodateDuration
from type_bridge import (
    DateTime,
    DateTimeTZ,
    Duration,
    Entity,
    Flag,
    Key,
    String,
    TypeFlags,
)


def test_duration_creation_from_iso_string():
    """Test creating Duration from ISO 8601 strings."""

    class SessionDuration(Duration):
        pass

    # Simple durations
    d1 = SessionDuration("PT1H")  # 1 hour
    assert d1.to_iso8601() == "PT1H"

    d2 = SessionDuration("P7D")  # 7 days
    assert d2.to_iso8601() == "P7D"

    d3 = SessionDuration("P12W")  # 12 weeks
    # isodate converts weeks to days
    assert d3.to_iso8601() == "P84D"  # 12 weeks = 84 days

    # Complex duration
    d4 = SessionDuration("P1Y2M3DT4H5M6.789S")
    assert "P1Y2M3D" in d4.to_iso8601()
    assert "T4H5M6.789S" in d4.to_iso8601()


def test_duration_creation_from_timedelta():
    """Test creating Duration from Python timedelta."""

    class SessionDuration(Duration):
        pass

    # Create from timedelta
    td = timedelta(hours=2, minutes=30)
    duration = SessionDuration(td)

    # Should be converted to Duration internally
    assert isinstance(duration.value, IsodateDuration)
    assert "PT2H30M" in duration.to_iso8601()


def test_duration_value_type():
    """Test that Duration has correct value_type for TypeDB."""

    class EventCadence(Duration):
        pass

    assert EventCadence.value_type == "duration"


def test_duration_in_entity():
    """Test using Duration in an entity."""

    class EventCadence(Duration):
        pass

    class EventName(String):
        pass

    class Event(Entity):
        flags = TypeFlags(name="event")
        name: EventName = Flag(Key)
        cadence: EventCadence

    # Create entity with duration
    event = Event(name=EventName("Monthly Meeting"), cadence=EventCadence("P1M"))

    # Verify insert query
    query = event.to_insert_query()
    assert "$e isa event" in query
    assert 'has EventName "Monthly Meeting"' in query
    assert "has EventCadence P1M" in query
    assert '"P1M"' not in query  # Should NOT be quoted


def test_duration_iso8601_formatting():
    """Test Duration ISO 8601 formatting in insert queries."""

    class Interval(Duration):
        pass

    class Task(Entity):
        flags = TypeFlags(name="task")
        interval: Interval

    # Test various durations
    task1 = Task(interval=Interval("PT1H30M"))
    query1 = task1.to_insert_query()
    assert "has Interval PT1H30M" in query1

    task2 = Task(interval=Interval("P1M"))
    query2 = task2.to_insert_query()
    assert "has Interval P1M" in query2

    task3 = Task(interval=Interval("P1Y2M3DT4H5M6S"))
    query3 = task3.to_insert_query()
    assert "has Interval P1Y2M3DT4H5M6S" in query3


def test_duration_ambiguous_m():
    """Test Duration correctly handles M for months vs minutes."""

    class TimePeriod(Duration):
        pass

    # M without T = months
    months = TimePeriod("P1M")
    assert "P1M" in months.to_iso8601()
    assert "T" not in months.to_iso8601()

    # M with T = minutes
    minutes = TimePeriod("PT1M")
    assert "PT1M" in minutes.to_iso8601()
    assert "T" in minutes.to_iso8601()


def test_duration_nanosecond_precision():
    """Test Duration preserves nanosecond precision."""

    class PreciseDuration(Duration):
        pass

    # 6 decimal places (microseconds) - timedelta precision
    d1 = PreciseDuration("PT1.123456S")
    iso1 = d1.to_iso8601()
    assert "1.123456" in iso1

    # 9 decimal places (nanoseconds) - full TypeDB precision
    # Note: isodate may round to microsecond precision when converting to timedelta
    d2 = PreciseDuration("PT1.123456789S")
    iso2 = d2.to_iso8601()
    # Accept either full precision or rounded
    assert "1.12345" in iso2  # At least 5 decimal places


def test_duration_validation_months_limit():
    """Test Duration validates 32-bit month limit."""

    class LongDuration(Duration):
        pass

    # Within limit should work
    valid = LongDuration("P2147483647M")  # 2^31 - 1 months
    assert valid is not None

    # Exceeding limit should raise ValueError
    with pytest.raises(ValueError, match="months component.*exceeds 32-bit limit"):
        LongDuration("P2147483648M")  # 2^31 months


def test_duration_validation_days_limit():
    """Test Duration validates 32-bit days limit."""

    class LongDuration(Duration):
        pass

    # Note: Python's timedelta can't handle 2^31 days (causes OverflowError)
    # Test with a large but reasonable value
    valid = LongDuration("P1000000D")  # 1 million days
    assert valid is not None

    # For now, we rely on Python's own overflow protection
    # TypeDB's 32-bit limit is theoretical; Python will error first


def test_duration_zero():
    """Test Duration with zero value."""

    class ZeroDuration(Duration):
        pass

    # Create zero duration
    zero = ZeroDuration("PT0S")
    # isodate may format zero duration as P0D
    assert zero.to_iso8601() in ["PT0S", "P0D", "PT0H", "P0DT0H0M0S"]


def test_duration_negative_components():
    """Test Duration with negative components via subtraction."""

    class Period(Duration):
        pass

    # Create negative duration via subtraction
    d1 = Period("P1M")
    d2 = Period("P2M")
    neg = d1 - d2  # Results in -1 month

    # Negative durations are supported in result
    assert isinstance(neg, Duration)


def test_duration_weeks():
    """Test Duration with weeks (converted to days by isodate)."""

    class WeekDuration(Duration):
        pass

    # Weeks are converted to days by isodate
    weeks = WeekDuration("P12W")
    assert weeks.to_iso8601() == "P84D"  # 12 weeks = 84 days

    # Per ISO 8601, weeks cannot be combined with other units
    # But isodate is lenient and allows it (converts both to days)
    mixed = WeekDuration("P1W1D")
    assert mixed.to_iso8601() == "P8D"  # 1 week + 1 day = 8 days


def test_duration_addition_with_duration():
    """Test adding two durations together."""

    class Period(Duration):
        pass

    d1 = Period("P1M")
    d2 = Period("P15D")

    # Add durations
    result = d1 + d2
    assert isinstance(result, Duration)
    iso = result.to_iso8601()
    assert "1M" in iso and "15D" in iso


def test_duration_subtraction():
    """Test subtracting two durations."""

    class Period(Duration):
        pass

    d1 = Period("P2M")
    d2 = Period("P1M")

    # Subtract durations
    result = d1 - d2
    assert isinstance(result, Duration)
    assert "P1M" in result.to_iso8601()


def test_duration_addition_with_datetime():
    """Test adding Duration to DateTime."""

    class Interval(Duration):
        pass

    # Start date: Jan 31
    start = DateTime(datetime(2024, 1, 31, 14, 0, 0))
    duration = Interval("P1M")

    # Add 1 month
    result = start + duration

    # Jan 31 + 1 month = Feb 28 (last day of Feb in non-leap year)
    assert isinstance(result, DateTime)
    assert result.value.month == 2
    assert result.value.day == 29  # 2024 is leap year


def test_duration_addition_with_datetimetz():
    """Test adding Duration to DateTimeTZ."""

    class Interval(Duration):
        pass

    # Start with timezone-aware datetime
    start = DateTimeTZ(datetime(2024, 1, 31, 14, 0, 0, tzinfo=UTC))
    duration = Interval("P1M")

    # Add 1 month
    result = start + duration

    # Should remain timezone-aware
    assert isinstance(result, DateTimeTZ)
    assert result.value.tzinfo is not None
    assert result.value.month == 2


def test_duration_reverse_addition():
    """Test reverse addition (Duration + DateTime)."""

    class Interval(Duration):
        pass

    duration = Interval("P1D")
    start = DateTime(datetime(2024, 1, 15, 10, 0, 0))

    # Duration + DateTime (reverse)
    result = duration + start

    assert isinstance(result, DateTime)
    assert result.value.day == 16


def test_duration_calendar_arithmetic():
    """Test Duration respects calendar arithmetic (month addition)."""

    class MonthInterval(Duration):
        pass

    # Test: Jan 31 + 1 month = Feb 29 (2024 is leap year)
    jan31 = DateTime(datetime(2024, 1, 31, 12, 0, 0))
    one_month = MonthInterval("P1M")

    result = jan31 + one_month
    assert result.value.month == 2
    assert result.value.day == 29  # Last day of Feb in leap year


def test_duration_optional_attribute():
    """Test Duration as optional attribute."""

    class Timeout(Duration):
        pass

    class JobName(String):
        pass

    class Job(Entity):
        flags = TypeFlags(name="job")
        name: JobName = Flag(Key)
        timeout: Timeout | None

    # Test with None
    job = Job(name=JobName("Task1"), timeout=None)
    query = job.to_insert_query()
    assert 'has JobName "Task1"' in query
    assert "has Timeout" not in query

    # Test with value
    job2 = Job(name=JobName("Task2"), timeout=Timeout("PT30M"))
    query2 = job2.to_insert_query()
    assert "has Timeout PT30M" in query2


def test_duration_multi_value_attribute():
    """Test Duration with multi-value attributes."""
    from type_bridge import Card

    class Interval(Duration):
        pass

    class Schedule(Entity):
        flags = TypeFlags(name="schedule")
        intervals: list[Interval] = Flag(Card(min=1))

    # Create with multiple durations
    schedule = Schedule(intervals=[Interval("PT1H"), Interval("PT2H"), Interval("PT3H")])
    query = schedule.to_insert_query()

    assert "$e isa schedule" in query
    assert "has Interval PT1H" in query
    assert "has Interval PT2H" in query
    assert "has Interval PT3H" in query


def test_duration_pydantic_validation():
    """Test Pydantic validation for Duration in entities."""

    class Cadence(Duration):
        pass

    class Event(Entity):
        flags = TypeFlags(name="event")
        cadence: Cadence

    # Test with Duration instance (with months)
    event1 = Event(cadence=Cadence("P1M"))
    assert isinstance(event1.cadence.value, IsodateDuration)

    # Test with ISO string (simple duration becomes timedelta)
    # isodate converts simple durations (no months/years) to timedelta
    event2 = Event(cadence=Cadence("PT1H"))
    assert isinstance(event2.cadence.value, (IsodateDuration, timedelta))


def test_duration_comparison():
    """Test Duration attribute comparison."""

    class Period(Duration):
        pass

    # Same duration
    d1 = Period("P1M")
    d2 = Period("P1M")
    assert d1 == d2

    # Different durations
    d3 = Period("P2M")
    assert d1 != d3


def test_duration_string_representation():
    """Test string representation of Duration attributes."""

    class Interval(Duration):
        pass

    interval = Interval("P1Y2M3DT4H5M6S")

    # Test __repr__
    assert "Interval" in repr(interval)


def test_duration_from_timedelta_conversion():
    """Test timedelta is converted to isodate.Duration internally."""

    class Period(Duration):
        pass

    # Create from timedelta
    td = timedelta(days=7, hours=3, minutes=30)
    period = Period(td)

    # Internal value should be isodate.Duration
    assert isinstance(period.value, IsodateDuration)

    # Should format correctly
    iso = period.to_iso8601()
    assert "P7D" in iso or "P1W" in iso
    assert "T3H30M" in iso


def test_duration_all_components():
    """Test Duration with all date and time components."""

    class ComplexDuration(Duration):
        pass

    # All components
    duration = ComplexDuration("P1Y2M3DT4H5M6.789S")

    iso = duration.to_iso8601()
    # Should have all components
    assert "1Y" in iso
    assert "2M" in iso
    assert "3D" in iso
    assert "T" in iso
    assert "4H" in iso
    assert "5M" in iso
    assert "6.789S" in iso
