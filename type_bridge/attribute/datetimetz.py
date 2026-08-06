"""DateTimeTZ attribute type for TypeDB."""

from datetime import UTC
from datetime import datetime as datetime_type
from datetime import timezone as timezone_type
from typing import TYPE_CHECKING, Any, ClassVar, Self, TypeVar

from pydantic_core import core_schema

from type_bridge.attribute.base import _QueryAttribute

if TYPE_CHECKING:
    from type_bridge.attribute.datetime import _QueryDateTime

# TypeVar for proper type checking
DateTimeTZValue = TypeVar("DateTimeTZValue", bound=datetime_type)


class DateTimeTZ(_QueryAttribute):
    """DateTimeTZ attribute type that accepts timezone-aware datetime values.

    This maps to TypeDB's 'datetime-tz' type, which requires timezone information.
    The datetime must have tzinfo set (e.g., using datetime.timezone.utc or zoneinfo).

    Example:
        from datetime import datetime, timezone

        class CreatedAt(DateTimeTZ):
            pass

        # Usage with timezone
        event = Event(created_at=CreatedAt(datetime(2024, 1, 15, 10, 30, 45, tzinfo=timezone.utc)))

        # Convert to DateTime
        naive_dt = created_at.strip_timezone()  # Implicit: just strip tz
        naive_dt_jst = created_at.strip_timezone(timezone(timedelta(hours=9)))  # Explicit: convert to JST, then strip
    """

    value_type: ClassVar[str] = "datetime-tz"

    def __init__(self, value: datetime_type):
        """Initialize DateTimeTZ attribute with a timezone-aware datetime value.

        Args:
            value: The timezone-aware datetime value to store

        Raises:
            ValueError: If the datetime does not have timezone information
        """
        if value.tzinfo is None:
            raise ValueError(
                "DateTimeTZ requires timezone-aware datetime. "
                "Use DateTime for naive datetime or add tzinfo (e.g., datetime.timezone.utc)"
            )
        super().__init__(value)

    @property
    def value(self) -> datetime_type:
        """Get the stored datetime value."""
        if self._value is None:
            return datetime_type.now(UTC)
        return self._value

    def strip_timezone(self, tz: timezone_type | None = None) -> "_QueryDateTime":
        """Convert DateTimeTZ to DateTime by stripping timezone information.

        Implicit conversion (tz=None): Just strip timezone as-is
        Explicit conversion (tz provided): Convert to specified timezone first, then strip

        Args:
            tz: Optional timezone to convert to before stripping.
                If None, strips timezone without conversion.
                If provided, converts to that timezone first.

        Returns:
            DateTime instance with naive datetime

        Example:
            # Implicit: strip timezone as-is
            naive = dt_tz.strip_timezone()

            # Explicit: convert to JST (+9), then strip
            from datetime import timezone, timedelta
            jst = timezone(timedelta(hours=9))
            naive_jst = dt_tz.strip_timezone(jst)
        """
        from type_bridge.attribute.datetime import _QueryDateTime

        dt_value = self.value
        if tz is not None:
            # Explicit: convert to specified timezone first
            dt_value = dt_value.astimezone(tz)

        # Strip timezone info
        naive_dt = dt_value.replace(tzinfo=None)
        return _QueryDateTime(naive_dt)

    def __add__(self, other: Any) -> Self:
        """Add a Duration to this DateTimeTZ.

        Args:
            other: A Duration to add to this timezone-aware datetime

        Returns:
            New DateTimeTZ with the duration added

        Note:
            Duration addition respects timezone changes (DST, etc.)

        Example:
            from type_bridge import Duration
            from datetime import datetime, timezone
            dt = DateTimeTZ(datetime(2024, 1, 31, 14, 0, 0, tzinfo=timezone.utc))
            duration = Duration("P1M")
            result = dt + duration  # DateTimeTZ(2024-02-28 14:00:00+00:00)
        """
        from type_bridge.attribute.duration import _QueryDuration

        if isinstance(other, _QueryDuration):
            # Add duration to timezone-aware datetime
            # isodate handles timezone-aware datetime + duration correctly
            new_dt = self.value + other.value
            return type(self)(new_dt)
        return NotImplemented

    def __radd__(self, other: Any) -> Self:
        """Reverse addition for Duration + DateTimeTZ."""
        return self.__add__(other)

    # Pydantic integration hooks (used by base class __get_pydantic_core_schema__)

    @classmethod
    def _get_default_value(cls) -> datetime_type:
        """Default value for DateTimeTZ is now(UTC)."""
        return datetime_type.now(UTC)

    @classmethod
    def _get_pydantic_return_schema(cls) -> core_schema.CoreSchema:
        """Return schema for datetime serialization."""
        return core_schema.datetime_schema()

    @classmethod
    def _pydantic_serialize(cls, value: Any) -> datetime_type:
        """Serialize DateTimeTZ to raw datetime value."""
        if isinstance(value, cls):
            if value._value is None:
                return datetime_type.now(UTC)
            return value._value
        if isinstance(value, datetime_type):
            if value.tzinfo is None:
                raise ValueError("DateTimeTZ requires timezone-aware datetime")
            return value
        # Try to parse ISO string with timezone
        dt = datetime_type.fromisoformat(str(value))
        if dt.tzinfo is None:
            raise ValueError("DateTimeTZ requires timezone-aware datetime")
        return dt

    @classmethod
    def _pydantic_validate(cls, value: Any) -> Self:
        """Validate and wrap value in DateTimeTZ instance.

        Validates that datetime has timezone information.
        """
        if isinstance(value, cls):
            return value
        if isinstance(value, datetime_type):
            if value.tzinfo is None:
                raise ValueError("DateTimeTZ requires timezone-aware datetime")
            return cls(value)
        # Try to parse ISO string with timezone
        dt = datetime_type.fromisoformat(str(value))
        if dt.tzinfo is None:
            raise ValueError("DateTimeTZ requires timezone-aware datetime")
        return cls(dt)


_QueryDateTimeTZ = DateTimeTZ
del DateTimeTZ
