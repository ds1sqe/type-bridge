"""Tests for Rust-backed value coercion and format_value parity."""

from __future__ import annotations

from datetime import UTC, date, datetime, timedelta, timezone
from decimal import Decimal
from typing import Any

import isodate
import pytest

from type_bridge.coercion import RUST_AVAILABLE

pytestmark = pytest.mark.skipif(not RUST_AVAILABLE, reason="Rust extension not available")


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

# Import Rust functions directly (guarded by pytestmark skip).
# Use try/except so the module can still be collected when Rust is absent.
try:
    from type_bridge_core import (
        ValueCoercer,
        coerce_value,
    )
    from type_bridge_core import format_value as rust_format_value
except ImportError:  # pragma: no cover
    # Cast to Any so pyright doesn't narrow to None; tests are skipped at runtime.
    ValueCoercer: Any = None
    coerce_value: Any = None
    rust_format_value: Any = None

# Python reference implementation (always available)


def _python_format_value(value: Any) -> str:
    """Call the pure-Python format_value (bypass Rust fast path)."""
    # Reach into the Python implementation directly
    from datetime import date as _date
    from datetime import datetime as _datetime
    from datetime import timedelta as _timedelta
    from decimal import Decimal as _Decimal

    from isodate import Duration as _IsodateDuration

    # Duplicate the Python logic to ensure we're testing against the original
    if hasattr(value, "value"):
        value = value.value

    if isinstance(value, str):
        escaped = (
            value.replace("\\", "\\\\")
            .replace('"', '\\"')
            .replace("\n", "\\n")
            .replace("\r", "\\r")
            .replace("\t", "\\t")
        )
        return f'"{escaped}"'
    elif isinstance(value, bool):
        return "true" if value else "false"
    elif isinstance(value, _Decimal):
        return f"{value}dec"
    elif isinstance(value, (int, float)):
        return str(value)
    elif isinstance(value, _datetime):
        return value.isoformat()
    elif isinstance(value, _date):
        return value.isoformat()
    elif isinstance(value, (_IsodateDuration, _timedelta)):
        import isodate as _isodate

        return _isodate.duration_isoformat(value)
    else:
        str_value = str(value)
        escaped = (
            str_value.replace("\\", "\\\\")
            .replace('"', '\\"')
            .replace("\n", "\\n")
            .replace("\r", "\\r")
            .replace("\t", "\\t")
        )
        return f'"{escaped}"'


# ---------------------------------------------------------------------------
# Parity tests: Rust format_value matches Python format_value
# ---------------------------------------------------------------------------


class TestFormatValueParity:
    """Verify Rust format_value produces identical output to Python."""

    def test_string_simple(self) -> None:
        assert rust_format_value("hello") == _python_format_value("hello")

    def test_string_with_quotes(self) -> None:
        v = 'say "hello"'
        assert rust_format_value(v) == _python_format_value(v)

    def test_string_with_backslash(self) -> None:
        v = "path\\to\\file"
        assert rust_format_value(v) == _python_format_value(v)

    def test_string_with_newline(self) -> None:
        v = "line1\nline2"
        assert rust_format_value(v) == _python_format_value(v)

    def test_string_with_tab(self) -> None:
        v = "col1\tcol2"
        assert rust_format_value(v) == _python_format_value(v)

    def test_string_empty(self) -> None:
        assert rust_format_value("") == _python_format_value("")

    def test_string_unicode(self) -> None:
        v = "こんにちは"
        assert rust_format_value(v) == _python_format_value(v)

    def test_boolean_true(self) -> None:
        assert rust_format_value(True) == _python_format_value(True)

    def test_boolean_false(self) -> None:
        assert rust_format_value(False) == _python_format_value(False)

    def test_integer_positive(self) -> None:
        assert rust_format_value(42) == _python_format_value(42)

    def test_integer_negative(self) -> None:
        assert rust_format_value(-5) == _python_format_value(-5)

    def test_integer_zero(self) -> None:
        assert rust_format_value(0) == _python_format_value(0)

    def test_float_positive(self) -> None:
        assert rust_format_value(3.14) == _python_format_value(3.14)

    def test_float_negative(self) -> None:
        assert rust_format_value(-2.5) == _python_format_value(-2.5)

    def test_float_zero(self) -> None:
        assert rust_format_value(0.0) == _python_format_value(0.0)

    def test_decimal(self) -> None:
        v = Decimal("123.45")
        assert rust_format_value(v) == _python_format_value(v)

    def test_decimal_integer(self) -> None:
        v = Decimal("100")
        assert rust_format_value(v) == _python_format_value(v)

    def test_decimal_negative(self) -> None:
        v = Decimal("-50.25")
        assert rust_format_value(v) == _python_format_value(v)

    def test_decimal_high_precision(self) -> None:
        v = Decimal("123.456789012345")
        assert rust_format_value(v) == _python_format_value(v)

    def test_datetime_naive(self) -> None:
        v = datetime(2024, 1, 15, 10, 30, 0)
        assert rust_format_value(v) == _python_format_value(v)

    def test_datetime_with_microseconds(self) -> None:
        v = datetime(2024, 1, 15, 10, 30, 0, 123456)
        assert rust_format_value(v) == _python_format_value(v)

    def test_datetime_utc(self) -> None:
        v = datetime(2024, 1, 15, 10, 30, 0, tzinfo=UTC)
        assert rust_format_value(v) == _python_format_value(v)

    def test_datetime_offset(self) -> None:
        tz = timezone(timedelta(hours=5, minutes=30))
        v = datetime(2024, 1, 15, 10, 30, 0, tzinfo=tz)
        assert rust_format_value(v) == _python_format_value(v)

    def test_date(self) -> None:
        v = date(2024, 1, 15)
        assert rust_format_value(v) == _python_format_value(v)

    def test_date_end_of_year(self) -> None:
        v = date(2024, 12, 31)
        assert rust_format_value(v) == _python_format_value(v)

    def test_timedelta_days(self) -> None:
        v = timedelta(days=5)
        assert rust_format_value(v) == _python_format_value(v)

    def test_timedelta_complex(self) -> None:
        v = timedelta(days=1, hours=2, minutes=30)
        assert rust_format_value(v) == _python_format_value(v)

    def test_isodate_duration(self) -> None:
        v = isodate.parse_duration("P1DT2H30M")
        assert rust_format_value(v) == _python_format_value(v)

    def test_isodate_duration_months(self) -> None:
        v = isodate.parse_duration("P1Y2M")
        assert rust_format_value(v) == _python_format_value(v)

    def test_attribute_unwrap(self) -> None:
        class MockAttr:
            value = "Alice"

        assert rust_format_value(MockAttr()) == _python_format_value(MockAttr())

    def test_attribute_unwrap_integer(self) -> None:
        class MockAttr:
            value = 42

        assert rust_format_value(MockAttr()) == _python_format_value(MockAttr())

    def test_attribute_none_value(self) -> None:
        class MockAttr:
            value = None

        assert rust_format_value(MockAttr()) == _python_format_value(MockAttr())


# ---------------------------------------------------------------------------
# Coerce value tests
# ---------------------------------------------------------------------------


class TestCoerceValue:
    """Test coerce_value(value, target_type) for all TypeDB value types."""

    def test_string(self) -> None:
        result = coerce_value("hello", "string")
        assert result["value"] == "hello"
        assert result["value_type"] == "string"

    def test_string_from_int(self) -> None:
        result = coerce_value(42, "string")
        assert result["value"] == "42"

    def test_long(self) -> None:
        result = coerce_value(42, "long")
        assert result["value"] == 42
        assert result["value_type"] == "long"

    def test_long_from_string(self) -> None:
        result = coerce_value("123", "long")
        assert result["value"] == 123

    def test_long_rejects_float(self) -> None:
        with pytest.raises(ValueError, match="Type mismatch"):
            coerce_value(3.14, "long")

    def test_double(self) -> None:
        result = coerce_value(3.14, "double")
        assert result["value_type"] == "double"

    def test_double_from_int(self) -> None:
        result = coerce_value(42, "double")
        assert result["value_type"] == "double"

    def test_boolean_true(self) -> None:
        result = coerce_value(True, "boolean")
        assert result["value"] is True

    def test_boolean_false(self) -> None:
        result = coerce_value(False, "boolean")
        assert result["value"] is False

    def test_boolean_from_string(self) -> None:
        result = coerce_value("true", "boolean")
        assert result["value"] is True

    def test_boolean_invalid_string(self) -> None:
        with pytest.raises(ValueError, match="Expected 'true' or 'false'"):
            coerce_value("yes", "boolean")

    def test_decimal(self) -> None:
        result = coerce_value("123.45", "decimal")
        assert result["value"] == "123.45"
        assert result["value_type"] == "decimal"

    def test_decimal_with_suffix(self) -> None:
        result = coerce_value("100dec", "decimal")
        assert result["value"] == "100"

    def test_decimal_from_number(self) -> None:
        result = coerce_value(42, "decimal")
        assert result["value_type"] == "decimal"

    def test_date_valid(self) -> None:
        result = coerce_value("2024-01-15", "date")
        assert result["value"] == "2024-01-15"
        assert result["value_type"] == "date"

    def test_date_invalid(self) -> None:
        with pytest.raises(ValueError, match="Invalid date"):
            coerce_value("2024-02-30", "date")

    def test_datetime_valid(self) -> None:
        result = coerce_value("2024-01-15T10:30:00", "datetime")
        assert result["value"] == "2024-01-15T10:30:00"

    def test_datetime_with_fractional(self) -> None:
        result = coerce_value("2024-01-15T10:30:00.123456", "datetime")
        assert result["value"] == "2024-01-15T10:30:00.123456"

    def test_datetime_rejects_tz(self) -> None:
        with pytest.raises(ValueError, match="datetime"):
            coerce_value("2024-01-15T10:30:00+00:00", "datetime")

    def test_datetime_tz_valid(self) -> None:
        result = coerce_value("2024-01-15T10:30:00+00:00", "datetime-tz")
        assert result["value"] == "2024-01-15T10:30:00+00:00"

    def test_datetime_tz_rejects_naive(self) -> None:
        with pytest.raises(ValueError, match="datetime-tz"):
            coerce_value("2024-01-15T10:30:00", "datetime-tz")

    def test_duration_valid(self) -> None:
        result = coerce_value("P1DT2H30M", "duration")
        assert result["value"] == "P1DT2H30M"

    def test_duration_invalid(self) -> None:
        with pytest.raises(ValueError):
            coerce_value("not-a-duration", "duration")

    def test_unknown_type(self) -> None:
        with pytest.raises(ValueError, match="Unknown target type"):
            coerce_value("test", "unknown_type")


# ---------------------------------------------------------------------------
# ValueCoercer class tests
# ---------------------------------------------------------------------------


class TestValueCoercer:
    """Test the ValueCoercer class."""

    def test_coerce(self) -> None:
        vc = ValueCoercer()
        result = vc.coerce(42, "long")
        assert result["value"] == 42

    def test_format_typeql_string(self) -> None:
        vc = ValueCoercer()
        assert vc.format_typeql("hello", "string") == '"hello"'

    def test_format_typeql_boolean(self) -> None:
        vc = ValueCoercer()
        assert vc.format_typeql(True, "boolean") == "true"

    def test_format_typeql_long(self) -> None:
        vc = ValueCoercer()
        assert vc.format_typeql(42, "long") == "42"

    def test_format_typeql_decimal(self) -> None:
        vc = ValueCoercer()
        assert vc.format_typeql("123.45", "decimal") == "123.45dec"

    def test_format_typeql_date(self) -> None:
        vc = ValueCoercer()
        assert vc.format_typeql("2024-01-15", "date") == "2024-01-15"


# ---------------------------------------------------------------------------
# Batch coercion tests
# ---------------------------------------------------------------------------


class TestCoerceBatch:
    """Test batch coercion."""

    def test_batch_all_valid(self) -> None:
        vc = ValueCoercer()
        results = vc.coerce_batch([("hello", "string"), (42, "long")])
        assert len(results) == 2
        assert results[0]["value"] == "hello"
        assert results[1]["value"] == 42


# ---------------------------------------------------------------------------
# Edge case tests
# ---------------------------------------------------------------------------


class TestEdgeCases:
    """Edge cases: overflow, precision, timezone, unicode, empty strings."""

    def test_large_integer(self) -> None:
        result = coerce_value(9999999999, "long")
        assert result["value"] == 9999999999

    def test_unicode_string(self) -> None:
        result = coerce_value("こんにちは", "string")
        assert result["value"] == "こんにちは"

    def test_empty_string(self) -> None:
        result = coerce_value("", "string")
        assert result["value"] == ""

    def test_leap_year_date(self) -> None:
        result = coerce_value("2024-02-29", "date")
        assert result["value"] == "2024-02-29"

    def test_non_leap_year_feb_29(self) -> None:
        with pytest.raises(ValueError):
            coerce_value("2023-02-29", "date")

    def test_datetime_tz_z_suffix(self) -> None:
        result = coerce_value("2024-01-15T10:30:00Z", "datetime-tz")
        assert result["value"] == "2024-01-15T10:30:00Z"

    def test_duration_fractional_seconds(self) -> None:
        result = coerce_value("PT0.5S", "duration")
        assert result["value"] == "PT0.5S"

    def test_integer_alias(self) -> None:
        """'integer' should be accepted as alias for 'long'."""
        result = coerce_value(42, "integer")
        assert result["value"] == 42
        assert result["value_type"] == "long"


# ---------------------------------------------------------------------------
# Integration: format_value used via formatting.py (now Rust-backed)
# ---------------------------------------------------------------------------


class TestFormattingIntegration:
    """Verify that crud/formatting.py now uses Rust and produces correct output."""

    def test_formatting_module_uses_rust(self) -> None:
        from type_bridge.crud.formatting import _USE_RUST

        assert _USE_RUST is True

    def test_formatting_string(self) -> None:
        from type_bridge.crud.formatting import format_value

        assert format_value("hello") == '"hello"'

    def test_formatting_boolean(self) -> None:
        from type_bridge.crud.formatting import format_value

        assert format_value(True) == "true"

    def test_formatting_integer(self) -> None:
        from type_bridge.crud.formatting import format_value

        assert format_value(42) == "42"

    def test_formatting_decimal(self) -> None:
        from type_bridge.crud.formatting import format_value

        assert format_value(Decimal("123.45")) == "123.45dec"

    def test_formatting_datetime(self) -> None:
        from type_bridge.crud.formatting import format_value

        dt = datetime(2024, 1, 15, 10, 30, 0)
        assert format_value(dt) == "2024-01-15T10:30:00"

    def test_formatting_date(self) -> None:
        from type_bridge.crud.formatting import format_value

        d = date(2024, 1, 15)
        assert format_value(d) == "2024-01-15"
