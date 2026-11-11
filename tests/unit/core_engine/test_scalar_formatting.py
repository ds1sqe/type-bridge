"""Scalar formatting retains the released output after legacy AST removal."""

import datetime
import decimal

import pytest
from type_bridge_core import ValueCoercer, format_value


@pytest.mark.parametrize(
    ("value", "expected"),
    [
        ('quote " slash \\ newline\n', '"quote \\" slash \\\\ newline\\n"'),
        (True, "true"),
        (False, "false"),
        (42, "42"),
        (0.0, "0.0"),
        (2.5, "2.5"),
        (decimal.Decimal("12.340"), "12.340dec"),
        (datetime.date(2025, 1, 2), "2025-01-02"),
        (datetime.datetime(2025, 1, 2, 3, 4, 5), "2025-01-02T03:04:05"),
        (
            datetime.datetime(2025, 1, 2, 3, 4, 5, tzinfo=datetime.UTC),
            "2025-01-02T03:04:05+00:00",
        ),
        (datetime.timedelta(days=1, seconds=2), "P1DT2S"),
    ],
)
def test_format_value_preserves_literal_bytes(value: object, expected: str) -> None:
    assert format_value(value) == expected


@pytest.mark.parametrize(
    ("value", "kind", "expected"),
    [
        ('quote "', "string", '"quote \\""'),
        (True, "boolean", "true"),
        (42, "long", "42"),
        (2.5, "double", "2.5"),
        ("12.340", "decimal", "12.340dec"),
        ("2025-01-02", "date", "2025-01-02"),
        ("2025-01-02T03:04:05", "datetime", "2025-01-02T03:04:05"),
        ("2025-01-02T03:04:05+00:00", "datetime-tz", "2025-01-02T03:04:05+00:00"),
        ("P1DT2S", "duration", "P1DT2S"),
    ],
)
def test_coercer_preserves_literal_bytes(value: object, kind: str, expected: str) -> None:
    assert ValueCoercer().format_typeql(value, kind) == expected
