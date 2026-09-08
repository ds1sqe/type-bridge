"""Date attribute type for TypeDB."""

from datetime import date as date_type
from datetime import datetime as datetime_type
from typing import Any, ClassVar, Self, TypeVar

from pydantic_core import core_schema

from type_bridge.attribute.base import _QueryAttribute

# TypeVar for proper type checking
DateValue = TypeVar("DateValue", bound=date_type)


class Date(_QueryAttribute):
    """Date attribute type that accepts date values (date only, no time).

    This maps to TypeDB's 'date' type, which is an ISO 8601 compliant date
    without time information.

    Range: January 1, 262144 BCE to December 31, 262142 CE

    Example:
        from datetime import date

        class PublishDate(Date):
            pass

        class BirthDate(Date):
            pass

        # Usage with date values
        published = PublishDate(date(2024, 3, 30))
        birthday = BirthDate(date(1990, 5, 15))
    """

    value_type: ClassVar[str] = "date"

    def __init__(self, value: date_type | str):
        """Initialize Date attribute with a date value.

        Args:
            value: The date value to store. Can be:
                - datetime.date instance
                - str in ISO 8601 format (YYYY-MM-DD)

        Example:
            from datetime import date

            # From date instance
            publish_date = PublishDate(date(2024, 3, 30))

            # From ISO string
            publish_date = PublishDate("2024-03-30")
        """
        if isinstance(value, str):
            value = date_type.fromisoformat(value)
        elif isinstance(value, datetime_type):
            # If passed a datetime, extract just the date part
            value = value.date()
        super().__init__(value)

    @property
    def value(self) -> date_type:
        """Get the stored date value."""
        return self._value if self._value is not None else date_type.today()

    def __add__(self, other: Any) -> Self:
        """Add a Duration to this Date.

        Args:
            other: A Duration to add to this date

        Returns:
            New Date with the duration added

        Example:
            from type_bridge import Duration
            d = Date(date(2024, 1, 31))
            duration = Duration("P1M")
            result = d + duration  # Date(2024-02-29)
        """
        from type_bridge.attribute.duration import _QueryDuration

        if isinstance(other, _QueryDuration):
            new_date = self.value + other.value
            # isodate returns datetime when adding Duration to date, extract .date()
            if isinstance(new_date, datetime_type):
                new_date = new_date.date()
            return type(self)(new_date)
        return NotImplemented

    def __radd__(self, other: Any) -> Self:
        """Reverse addition for Duration + Date."""
        return self.__add__(other)

    def __sub__(self, other: Any) -> Self:
        """Subtract a Duration from this Date.

        Args:
            other: A Duration to subtract from this date

        Returns:
            New Date with the duration subtracted

        Example:
            from type_bridge import Duration
            d = Date(date(2024, 3, 31))
            duration = Duration("P1M")
            result = d - duration  # Date(2024-02-29)
        """
        from type_bridge.attribute.duration import _QueryDuration

        if isinstance(other, _QueryDuration):
            new_date = self.value - other.value
            if isinstance(new_date, datetime_type):
                new_date = new_date.date()
            return type(self)(new_date)
        return NotImplemented

    # Pydantic integration hooks (used by base class __get_pydantic_core_schema__)

    @classmethod
    def _get_default_value(cls) -> date_type:
        """Default value for Date is today()."""
        return date_type.today()

    @classmethod
    def _get_pydantic_return_schema(cls) -> core_schema.CoreSchema:
        """Return schema for date serialization."""
        return core_schema.date_schema()

    @classmethod
    def _pydantic_serialize(cls, value: Any) -> date_type:
        """Serialize Date to raw date value."""
        if isinstance(value, cls):
            return value._value if value._value is not None else date_type.today()
        if isinstance(value, date_type):
            return value
        if isinstance(value, datetime_type):
            return value.date()
        return date_type.fromisoformat(str(value))

    @classmethod
    def _pydantic_validate(cls, value: Any) -> Self:
        """Validate and wrap value in Date instance."""
        if isinstance(value, cls):
            return value
        if isinstance(value, date_type):
            return cls(value)
        if isinstance(value, datetime_type):
            return cls(value.date())
        return cls(date_type.fromisoformat(str(value)))


_QueryDate = Date
del Date
