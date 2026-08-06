"""Test Date attribute type for date-only values."""

from datetime import date, datetime

from tests.utils.handwritten import Card, Date, Entity, Flag, Key, String, TypeFlags


def test_date_creation():
    """Test creating Date with various input types."""

    class PublishDate(Date):
        pass

    # Test with date instance
    date1 = PublishDate(date(2024, 3, 30))
    assert date1.value == date(2024, 3, 30)

    # Test with ISO string
    date2 = PublishDate("2024-03-30")
    assert date2.value == date(2024, 3, 30)

    # Test with datetime instance (extracts date part)
    date3 = PublishDate(datetime(2024, 3, 30, 10, 30, 45))
    assert date3.value == date(2024, 3, 30)


def test_date_value_type():
    """Test that Date has correct value_type for TypeDB."""

    class PublishDate(Date):
        pass

    assert PublishDate.value_type == "date"


def test_date_in_entity():
    """Test using Date in an entity."""

    class PublishDate(Date):
        pass

    class BookTitle(String):
        pass

    class Book(Entity):
        flags = TypeFlags(name="book")
        publish_date: PublishDate
        title: BookTitle = Flag(Key)

    # Create entity with date value
    book = Book(title=BookTitle("TypeDB Guide"), publish_date=PublishDate(date(2024, 3, 30)))

    # Verify insert query
    query = book.to_insert_query()
    assert "$e isa book" in query
    assert 'has BookTitle "TypeDB Guide"' in query
    assert "has PublishDate 2024-03-30" in query
    assert '"2024-03-30"' not in query  # Should NOT be quoted


def test_date_insert_query_formatting():
    """Test that Date values are formatted as ISO 8601 dates in insert queries."""

    class ReleaseDate(Date):
        pass

    class Product(Entity):
        flags = TypeFlags(name="product")
        release_date: ReleaseDate

    # Test with various dates
    product1 = Product(release_date=ReleaseDate(date(2025, 1, 10)))
    query1 = product1.to_insert_query()
    assert "has ReleaseDate 2025-01-10" in query1

    product2 = Product(release_date=ReleaseDate(date(2024, 12, 31)))
    query2 = product2.to_insert_query()
    assert "has ReleaseDate 2024-12-31" in query2


def test_date_iso_format():
    """Test Date uses ISO 8601 format (YYYY-MM-DD)."""

    class EventDate(Date):
        pass

    # Test with single-digit month and day
    event_date = EventDate(date(2024, 1, 5))
    query = EventDate.__name__

    # ISO format should be YYYY-MM-DD with zero padding
    assert event_date.value.isoformat() == "2024-01-05"


def test_date_year_range():
    """Test Date with various year formats."""

    class HistoricalDate(Date):
        pass

    class Event(Entity):
        flags = TypeFlags(name="event")
        event_date: HistoricalDate

    # Test with regular 4-digit year
    event1 = Event(event_date=HistoricalDate(date(2024, 3, 30)))
    query1 = event1.to_insert_query()
    assert "has HistoricalDate 2024-03-30" in query1

    # Test with year 1
    event2 = Event(event_date=HistoricalDate(date(1, 1, 1)))
    query2 = event2.to_insert_query()
    assert "has HistoricalDate 0001-01-01" in query2

    # Test with year 9999
    event3 = Event(event_date=HistoricalDate(date(9999, 12, 31)))
    query3 = event3.to_insert_query()
    assert "has HistoricalDate 9999-12-31" in query3


def test_date_optional_attribute():
    """Test Date as optional attribute."""

    class PublishDate(Date):
        pass

    class Title(String):
        pass

    class Document(Entity):
        flags = TypeFlags(name="document")
        title: Title = Flag(Key)
        publish_date: PublishDate | None

    # Test with None (optional)
    doc = Document(title=Title("Draft"), publish_date=None)
    query = doc.to_insert_query()
    assert 'has Title "Draft"' in query
    assert "has PublishDate" not in query  # Should not appear when None

    # Test with value
    doc2 = Document(title=Title("Published"), publish_date=PublishDate(date(2024, 3, 30)))
    query2 = doc2.to_insert_query()
    assert "has PublishDate 2024-03-30" in query2


def test_date_multi_value_attribute():
    """Test Date with multi-value attributes."""

    class ImportantDate(Date):
        pass

    class Timeline(Entity):
        flags = TypeFlags(name="timeline")
        dates: list[ImportantDate] = Flag(Card(min=1))

    # Create entity with multiple date values
    timeline = Timeline(
        dates=[
            ImportantDate(date(2024, 1, 15)),
            ImportantDate(date(2024, 6, 20)),
            ImportantDate(date(2024, 12, 31)),
        ]
    )
    query = timeline.to_insert_query()

    assert "$e isa timeline" in query
    assert "has ImportantDate 2024-01-15" in query
    assert "has ImportantDate 2024-06-20" in query
    assert "has ImportantDate 2024-12-31" in query


def test_date_pydantic_validation():
    """Test Pydantic validation for Date in entities."""

    class BirthDate(Date):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        birth_date: BirthDate

    # Test with Date instance
    person1 = Person(birth_date=BirthDate(date(1990, 5, 15)))
    assert person1.birth_date.value == date(1990, 5, 15)

    # Test with ISO string (Pydantic should convert)
    person2 = Person(birth_date=BirthDate("1990-05-15"))
    assert person2.birth_date.value == date(1990, 5, 15)


def test_date_comparison():
    """Test Date attribute comparison."""

    class EventDate(Date):
        pass

    date1 = EventDate(date(2024, 3, 30))
    date2 = EventDate(date(2024, 3, 30))
    date3 = EventDate(date(2024, 12, 31))

    # Same type, same value
    assert date1 == date2

    # Same type, different value
    assert date1 != date3


def test_date_string_representation():
    """Test string representation of Date attributes."""

    class PublishDate(Date):
        pass

    publish_date = PublishDate(date(2024, 3, 30))

    # Test __str__
    assert str(publish_date) == "2024-03-30"

    # Test __repr__
    assert "PublishDate" in repr(publish_date)
    assert "2024" in repr(publish_date)


def test_date_leap_year():
    """Test Date with leap year date."""

    class EventDate(Date):
        pass

    class Event(Entity):
        flags = TypeFlags(name="event")
        event_date: EventDate

    # Test with leap day (Feb 29)
    event = Event(event_date=EventDate(date(2024, 2, 29)))
    query = event.to_insert_query()
    assert "has EventDate 2024-02-29" in query


def test_date_boundary_dates():
    """Test Date with boundary dates (first and last day of month/year)."""

    class SpecialDate(Date):
        pass

    class Record(Entity):
        flags = TypeFlags(name="record")
        special_date: SpecialDate

    # First day of year
    record1 = Record(special_date=SpecialDate(date(2024, 1, 1)))
    query1 = record1.to_insert_query()
    assert "has SpecialDate 2024-01-01" in query1

    # Last day of year
    record2 = Record(special_date=SpecialDate(date(2024, 12, 31)))
    query2 = record2.to_insert_query()
    assert "has SpecialDate 2024-12-31" in query2


def test_date_from_datetime_extraction():
    """Test that Date correctly extracts date from datetime."""

    class EventDate(Date):
        pass

    # Create Date from datetime - should extract only the date part
    dt = datetime(2024, 3, 30, 15, 45, 30)
    event_date = EventDate(dt)

    # Should have date only, not time
    assert event_date.value == date(2024, 3, 30)
    assert isinstance(event_date.value, date)
    assert not isinstance(event_date.value, datetime)


def test_date_today_default():
    """Test that Date.value returns today's date when _value is None."""

    class SomeDate(Date):
        pass

    # Manually set _value to None (edge case)
    some_date = SomeDate.__new__(SomeDate)
    some_date._value = None

    # Should return today's date
    today = date.today()
    assert some_date.value == today


def test_date_ordering():
    """Test Date value ordering/comparison."""

    class EventDate(Date):
        pass

    early = EventDate(date(2024, 1, 15))
    late = EventDate(date(2024, 12, 31))

    # Values should be comparable
    assert early.value < late.value
    assert late.value > early.value


def test_date_add_duration():
    """Test adding Duration to Date."""
    from tests.utils.handwritten import Duration

    class EventDate(Date):
        pass

    class Interval(Duration):
        pass

    d = EventDate(date(2024, 1, 31))
    duration = Interval("P1M")

    # Add 1 month: Jan 31 -> Feb 29 (2024 is leap year)
    result = d + duration
    assert isinstance(result, Date)
    assert result.value == date(2024, 2, 29)


def test_date_radd_duration():
    """Test reverse addition: Duration + Date."""
    from tests.utils.handwritten import Duration

    class EventDate(Date):
        pass

    class Interval(Duration):
        pass

    d = EventDate(date(2024, 3, 15))
    duration = Interval("P10D")

    # Duration + Date (reverse)
    result = duration + d
    assert isinstance(result, Date)
    assert result.value == date(2024, 3, 25)


def test_date_sub_duration():
    """Test subtracting Duration from Date."""
    from tests.utils.handwritten import Duration

    class EventDate(Date):
        pass

    class Interval(Duration):
        pass

    d = EventDate(date(2024, 3, 31))
    duration = Interval("P1M")

    # Subtract 1 month: Mar 31 -> Feb 29 (2024 is leap year)
    result = d - duration
    assert isinstance(result, Date)
    assert result.value.month == 2


def test_date_add_invalid_type():
    """Test that adding non-Duration returns NotImplemented."""

    class EventDate(Date):
        pass

    d = EventDate(date(2024, 1, 1))
    result = d.__add__(42)
    assert result is NotImplemented


def test_date_sub_invalid_type():
    """Test that subtracting non-Duration returns NotImplemented."""

    class EventDate(Date):
        pass

    d = EventDate(date(2024, 1, 1))
    result = d.__sub__("not a duration")
    assert result is NotImplemented


def test_date_vs_datetime_distinction():
    """Test that Date and DateTime are distinct types."""
    from tests.utils.handwritten import DateTime

    class EventDate(Date):
        pass

    class EventDateTime(DateTime):
        pass

    # Date should use date type
    event_date = EventDate(date(2024, 3, 30))
    assert isinstance(event_date.value, date)
    assert not isinstance(event_date.value, datetime)

    # DateTime should use datetime type
    event_datetime = EventDateTime(datetime(2024, 3, 30, 10, 30, 45))
    assert isinstance(event_datetime.value, datetime)

    # They should generate different TypeQL
    class Event1(Entity):
        flags = TypeFlags(name="event1")
        date_only: EventDate

    class Event2(Entity):
        flags = TypeFlags(name="event2")
        date_time: EventDateTime

    e1 = Event1(date_only=EventDate(date(2024, 3, 30)))
    e2 = Event2(date_time=EventDateTime(datetime(2024, 3, 30, 10, 30, 45)))

    query1 = e1.to_insert_query()
    query2 = e2.to_insert_query()

    # Date: YYYY-MM-DD
    assert "has EventDate 2024-03-30" in query1
    assert "T" not in query1.split("has EventDate")[1].split(",")[0]  # No time component

    # DateTime: YYYY-MM-DDTHH:MM:SS
    assert "has EventDateTime 2024-03-30T10:30:45" in query2
    assert "T" in query2.split("has EventDateTime")[1].split(",")[0]  # Has time component
