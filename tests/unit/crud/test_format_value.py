"""Unit tests for CRUD utility functions."""

from datetime import UTC, date, datetime, timedelta, timezone
from decimal import Decimal

import isodate

from type_bridge import Integer, String
from type_bridge.attribute import AttributeFlags
from type_bridge.crud.utils import format_value, is_multi_value_attribute


class TestFormatValueStrings:
    """Tests for format_value with string inputs."""

    def test_format_string_simple(self):
        """Simple strings should be quoted."""
        assert format_value("hello") == '"hello"'

    def test_format_string_with_double_quotes(self):
        """Strings with double quotes should be escaped."""
        assert format_value('say "hello"') == '"say \\"hello\\""'

    def test_format_string_with_single_quotes(self):
        """Strings with single quotes should be preserved (not escaped)."""
        assert format_value("it's") == '"it\'s"'

    def test_format_string_with_backslash(self):
        """Strings with backslashes should be escaped."""
        assert format_value("path\\to\\file") == '"path\\\\to\\\\file"'

    def test_format_string_with_multiple_backslashes(self):
        """Multiple consecutive backslashes should all be escaped."""
        assert format_value("\\\\network") == '"\\\\\\\\network"'

    def test_format_string_with_newline(self):
        """Strings with newlines should escape them for TypeQL."""
        result = format_value("line1\nline2")
        assert result == r'"line1\nline2"'

    def test_format_string_with_tab(self):
        """Strings with tabs should escape them for TypeQL."""
        result = format_value("col1\tcol2")
        assert result == r'"col1\tcol2"'

    def test_format_string_empty(self):
        """Empty strings should be quoted."""
        assert format_value("") == '""'

    def test_format_string_with_unicode(self):
        """Unicode strings should be preserved."""
        assert format_value("こんにちは") == '"こんにちは"'

    def test_format_string_with_emoji(self):
        """Strings with emojis should be preserved."""
        assert format_value("Hello 👋") == '"Hello 👋"'

    def test_format_string_with_mixed_escapes(self):
        """Strings with both quotes and backslashes should be properly escaped."""
        result = format_value('path\\to\\"file"')
        assert result == '"path\\\\to\\\\\\"file\\""'


class TestFormatValueBooleans:
    """Tests for format_value with boolean inputs."""

    def test_format_boolean_true_returns_lowercase(self):
        """Boolean True should be 'true' (lowercase)."""
        assert format_value(True) == "true"

    def test_format_boolean_false_returns_lowercase(self):
        """Boolean False should be 'false' (lowercase)."""
        assert format_value(False) == "false"


class TestFormatValueNumbers:
    """Tests for format_value with numeric inputs."""

    def test_format_integer_positive(self):
        """Positive integers should be formatted as strings."""
        assert format_value(42) == "42"

    def test_format_integer_negative(self):
        """Negative integers should include the sign."""
        assert format_value(-5) == "-5"

    def test_format_integer_zero(self):
        """Zero should be formatted as '0'."""
        assert format_value(0) == "0"

    def test_format_integer_large(self):
        """Large integers should be formatted correctly."""
        assert format_value(9999999999) == "9999999999"

    def test_format_float_positive(self):
        """Positive floats should be formatted as strings."""
        assert format_value(3.14) == "3.14"

    def test_format_float_negative(self):
        """Negative floats should include the sign."""
        assert format_value(-2.5) == "-2.5"

    def test_format_float_zero(self):
        """Float zero should be formatted as '0.0'."""
        assert format_value(0.0) == "0.0"

    def test_format_float_small(self):
        """Small floats should preserve precision."""
        result = format_value(0.001)
        assert "0.001" in result

    def test_format_decimal_adds_dec_suffix(self):
        """Decimal values should have 'dec' suffix."""
        result = format_value(Decimal("123.45"))
        assert result == "123.45dec"

    def test_format_decimal_integer_value(self):
        """Decimal with integer value should still have 'dec' suffix."""
        result = format_value(Decimal("100"))
        assert result == "100dec"

    def test_format_decimal_negative(self):
        """Negative Decimal should include sign and 'dec' suffix."""
        result = format_value(Decimal("-50.25"))
        assert result == "-50.25dec"

    def test_format_decimal_high_precision(self):
        """Decimal with high precision should preserve digits."""
        result = format_value(Decimal("123.456789012345"))
        assert result == "123.456789012345dec"


class TestFormatValueDateTimes:
    """Tests for format_value with date/time inputs."""

    def test_format_datetime_naive(self):
        """Naive datetime should be ISO formatted."""
        dt = datetime(2024, 1, 15, 10, 30, 0)
        result = format_value(dt)
        assert result == "2024-01-15T10:30:00"

    def test_format_datetime_with_microseconds(self):
        """Datetime with microseconds should include them."""
        dt = datetime(2024, 1, 15, 10, 30, 0, 123456)
        result = format_value(dt)
        assert "2024-01-15T10:30:00" in result
        assert "123456" in result

    def test_format_datetime_with_utc_timezone(self):
        """UTC timezone-aware datetime should include timezone."""
        dt = datetime(2024, 1, 15, 10, 30, 0, tzinfo=UTC)
        result = format_value(dt)
        assert "2024-01-15T10:30:00" in result

    def test_format_datetime_with_offset_timezone(self):
        """Offset timezone-aware datetime should include offset."""
        tz = timezone(timedelta(hours=5, minutes=30))
        dt = datetime(2024, 1, 15, 10, 30, 0, tzinfo=tz)
        result = format_value(dt)
        assert "2024-01-15T10:30:00" in result
        assert "+05:30" in result

    def test_format_date(self):
        """Date should be ISO formatted."""
        d = date(2024, 1, 15)
        result = format_value(d)
        assert result == "2024-01-15"

    def test_format_date_first_of_month(self):
        """Date at start of month should format correctly."""
        d = date(2024, 1, 1)
        result = format_value(d)
        assert result == "2024-01-01"

    def test_format_date_end_of_year(self):
        """Date at end of year should format correctly."""
        d = date(2024, 12, 31)
        result = format_value(d)
        assert result == "2024-12-31"


class TestFormatValueDurations:
    """Tests for format_value with duration inputs."""

    def test_format_timedelta_days(self):
        """Timedelta with days should be ISO duration formatted."""
        td = timedelta(days=5)
        result = format_value(td)
        assert "P" in result
        assert "5" in result

    def test_format_timedelta_hours_minutes(self):
        """Timedelta with hours and minutes should format correctly."""
        td = timedelta(hours=2, minutes=30)
        result = format_value(td)
        assert "P" in result
        assert "T" in result  # Time component marker

    def test_format_timedelta_complex(self):
        """Timedelta with days, hours, and minutes should format correctly."""
        td = timedelta(days=1, hours=2, minutes=30)
        result = format_value(td)
        assert "P" in result

    def test_format_duration_isodate(self):
        """Isodate Duration should be ISO formatted."""
        duration = isodate.parse_duration("P1DT2H30M")
        result = format_value(duration)
        assert "P" in result

    def test_format_duration_isodate_months(self):
        """Isodate Duration with months should format correctly."""
        duration = isodate.parse_duration("P1M")
        result = format_value(duration)
        assert "P" in result
        assert "M" in result


class TestFormatValueAttributeExtraction:
    """Tests for format_value with Attribute instances."""

    def test_format_value_extracts_from_string_attribute(self):
        """String attribute instances should have their value extracted."""

        class Name(String):
            pass

        name = Name("Alice")
        result = format_value(name)
        assert result == '"Alice"'

    def test_format_value_extracts_from_integer_attribute(self):
        """Integer attribute instances should have their value extracted."""

        class Age(Integer):
            pass

        age = Age(25)
        result = format_value(age)
        assert result == "25"

    def test_format_value_with_none_attribute_value(self):
        """Attribute with None value should stringify None."""
        # Create a mock object with value=None

        class MockAttr:
            value = None

        result = format_value(MockAttr())
        assert result == '"None"'


class TestFormatValueCustomObjects:
    """Tests for format_value with custom/unknown objects."""

    def test_format_custom_object_stringifies(self):
        """Unknown objects should be converted to quoted strings."""

        class CustomObj:
            def __str__(self):
                return "custom_value"

        result = format_value(CustomObj())
        assert result == '"custom_value"'

    def test_format_custom_object_with_quotes(self):
        """Unknown objects with quotes in __str__ should be escaped."""

        class CustomObj:
            def __str__(self):
                return 'has "quotes"'

        result = format_value(CustomObj())
        assert result == '"has \\"quotes\\""'

    def test_format_list_stringifies(self):
        """Lists should be converted to quoted strings."""
        result = format_value([1, 2, 3])
        assert result == '"[1, 2, 3]"'

    def test_format_dict_stringifies(self):
        """Dicts should be converted to quoted strings."""
        result = format_value({"key": "value"})
        # The exact format depends on dict repr
        assert result.startswith('"')
        assert result.endswith('"')


class TestIsMultiValueAttribute:
    """Tests for is_multi_value_attribute function."""

    def test_card_0_1_is_single_value(self):
        """Card(0..1) should be single-value."""
        flags = AttributeFlags(card_min=0, card_max=1)
        assert is_multi_value_attribute(flags) is False

    def test_card_1_1_is_single_value(self):
        """Card(1..1) should be single-value."""
        flags = AttributeFlags(card_min=1, card_max=1)
        assert is_multi_value_attribute(flags) is False

    def test_card_0_5_is_multi_value(self):
        """Card(0..5) should be multi-value."""
        flags = AttributeFlags(card_min=0, card_max=5)
        assert is_multi_value_attribute(flags) is True

    def test_card_1_5_is_multi_value(self):
        """Card(1..5) should be multi-value."""
        flags = AttributeFlags(card_min=1, card_max=5)
        assert is_multi_value_attribute(flags) is True

    def test_card_2_2_is_multi_value(self):
        """Card(2..2) should be multi-value (max > 1)."""
        flags = AttributeFlags(card_min=2, card_max=2)
        assert is_multi_value_attribute(flags) is True

    def test_card_none_max_is_multi_value(self):
        """Card(0..None) unbounded should be multi-value."""
        flags = AttributeFlags(card_min=0, card_max=None)
        assert is_multi_value_attribute(flags) is True

    def test_card_1_none_is_multi_value(self):
        """Card(1..None) unbounded should be multi-value."""
        flags = AttributeFlags(card_min=1, card_max=None)
        assert is_multi_value_attribute(flags) is True

    def test_default_flags_is_multi_value(self):
        """Default AttributeFlags should be multi-value (unbounded)."""
        flags = AttributeFlags()
        # Default card_max is None (unbounded)
        assert is_multi_value_attribute(flags) is True
