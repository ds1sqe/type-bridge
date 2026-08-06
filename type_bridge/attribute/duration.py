"""Duration attribute type for TypeDB."""

from datetime import timedelta
from typing import Any, ClassVar, Self, TypeVar

import isodate
from isodate import Duration as IsodateDuration
from pydantic_core import core_schema

from type_bridge.attribute.base import _QueryAttribute

# TypeVar for proper type checking
DurationValue = TypeVar("DurationValue", bound=timedelta | IsodateDuration)

# Storage limits for TypeDB duration
MAX_MONTHS = 2**31 - 1  # 32-bit signed integer
MAX_DAYS = 2**31 - 1  # 32-bit signed integer
MAX_NANOSECONDS = 2**63 - 1  # 64-bit signed integer


def _validate_duration_limits(duration: IsodateDuration) -> None:
    """Validate that duration components fit within TypeDB storage limits.

    Args:
        duration: The duration to validate

    Raises:
        ValueError: If any component exceeds storage limits
    """
    months = duration.months if hasattr(duration, "months") else 0
    days = duration.days if hasattr(duration, "days") else 0

    # Calculate total nanoseconds from timedelta
    if hasattr(duration, "tdelta") and duration.tdelta:
        total_seconds = duration.tdelta.total_seconds()
        nanoseconds = int(total_seconds * 1_000_000_000)
    else:
        nanoseconds = 0

    if abs(months) > MAX_MONTHS:
        raise ValueError(
            f"Duration months component ({months}) exceeds 32-bit limit ({MAX_MONTHS})"
        )
    if abs(days) > MAX_DAYS:
        raise ValueError(f"Duration days component ({days}) exceeds 32-bit limit ({MAX_DAYS})")
    if abs(nanoseconds) > MAX_NANOSECONDS:
        raise ValueError(
            f"Duration nanoseconds component ({nanoseconds}) exceeds 64-bit limit ({MAX_NANOSECONDS})"
        )


def _timedelta_to_duration(td: timedelta) -> IsodateDuration:
    """Convert Python timedelta to isodate.Duration for consistent handling.

    Args:
        td: The timedelta to convert

    Returns:
        IsodateDuration with equivalent value
    """
    # timedelta only has days, seconds, microseconds
    # Convert to Duration with 0 months and the timedelta component
    return IsodateDuration(months=0, days=td.days, seconds=td.seconds, microseconds=td.microseconds)


class Duration(_QueryAttribute):
    """Duration attribute type that accepts ISO 8601 duration values.

    This maps to TypeDB's 'duration' type, which represents calendar-aware time spans
    using months, days, and nanoseconds.

    TypeDB duration format: ISO 8601 duration (e.g., P1Y2M3DT4H5M6.789S)
    Storage: 32-bit months, 32-bit days, 64-bit nanoseconds

    Important notes:
    - Durations are partially ordered (P1M and P30D cannot be compared)
    - P1D ≠ PT24H (calendar day vs 24 hours)
    - P1M ≠ P30D (months vary in length)
    - Addition is not commutative with calendar components

    Example:
        from datetime import timedelta

        class SessionDuration(Duration):
            pass

        class EventCadence(Duration):
            pass

        # From ISO 8601 string
        cadence = EventCadence("P1M")  # 1 month
        interval = SessionDuration("PT1H30M")  # 1 hour 30 minutes

        # From timedelta (converted to Duration internally)
        session = SessionDuration(timedelta(hours=2))

        # Complex duration
        complex = EventCadence("P1Y2M3DT4H5M6.789S")
    """

    value_type: ClassVar[str] = "duration"

    def __init__(self, value: str | timedelta | IsodateDuration):
        """Initialize Duration attribute with a duration value.

        Args:
            value: The duration value to store. Can be:
                - str: ISO 8601 duration string (e.g., "P1Y2M3DT4H5M6S")
                - timedelta: Python timedelta (converted to Duration)
                - isodate.Duration: Direct Duration object

        Raises:
            ValueError: If duration components exceed storage limits

        Example:
            # From ISO string
            duration1 = Duration("P1M")  # 1 month
            duration2 = Duration("PT1H30M")  # 1 hour 30 minutes

            # From timedelta
            from datetime import timedelta
            duration3 = Duration(timedelta(hours=2, minutes=30))

            # Complex duration
            duration4 = Duration("P1Y2M3DT4H5M6.789S")
        """
        if isinstance(value, str):
            value = isodate.parse_duration(value)
        elif isinstance(value, timedelta) and not isinstance(value, IsodateDuration):
            # Convert plain timedelta to Duration for consistent handling
            value = _timedelta_to_duration(value)

        # Validate storage limits
        if isinstance(value, IsodateDuration):
            _validate_duration_limits(value)

        super().__init__(value)

    @property
    def value(self) -> IsodateDuration:
        """Get the stored duration value.

        Returns:
            isodate.Duration instance (zero duration if None)
        """
        return self._value if self._value is not None else IsodateDuration()

    def to_iso8601(self) -> str:
        """Convert duration to ISO 8601 string format.

        Returns:
            ISO 8601 duration string (e.g., "P1Y2M3DT4H5M6S")

        Example:
            duration = Duration("P1M")
            assert duration.to_iso8601() == "P1M"
        """
        return isodate.duration_isoformat(self.value)

    def __add__(self, other: Any) -> Self:
        """Add two durations.

        Args:
            other: Another Duration to add

        Returns:
            New Duration with sum

        Example:
            d1 = Duration("P1M")
            d2 = Duration("P15D")
            result = d1 + d2  # P1M15D
        """
        if isinstance(other, _QueryDuration):
            # Both are Durations, add their components
            result = self.value + other.value
            return type(self)(result)
        return NotImplemented

    def __radd__(self, other: Any) -> Self:
        """Reverse addition for Duration."""
        return self.__add__(other)

    def __sub__(self, other: Any) -> Self:
        """Subtract two durations.

        Args:
            other: Another Duration to subtract

        Returns:
            New Duration with difference

        Example:
            d1 = Duration("P1M")
            d2 = Duration("P15D")
            result = d1 - d2  # P1M-15D
        """
        if isinstance(other, _QueryDuration):
            # Both are Durations, subtract their components
            result = self.value - other.value
            return type(self)(result)
        return NotImplemented

    # Pydantic integration hooks (used by base class __get_pydantic_core_schema__)

    @classmethod
    def _get_default_value(cls) -> IsodateDuration:
        """Default value for Duration is empty duration."""
        return IsodateDuration()

    @classmethod
    def _get_pydantic_return_schema(cls) -> core_schema.CoreSchema:
        """Return schema for ISO 8601 duration serialization."""
        return core_schema.str_schema()

    @classmethod
    def _pydantic_serialize(cls, value: Any) -> str:
        """Serialize Duration to an ISO 8601 string."""
        if isinstance(value, cls):
            duration = value._value if value._value is not None else IsodateDuration()
            return isodate.duration_isoformat(duration)
        if isinstance(value, IsodateDuration):
            return isodate.duration_isoformat(value)
        if isinstance(value, timedelta):
            return isodate.duration_isoformat(_timedelta_to_duration(value))
        return isodate.duration_isoformat(isodate.parse_duration(str(value)))

    @classmethod
    def _pydantic_validate(cls, value: Any) -> Self:
        """Validate and wrap value in Duration instance."""
        if isinstance(value, cls):
            return value
        if isinstance(value, (IsodateDuration, timedelta)):
            return cls(value)
        return cls(isodate.parse_duration(str(value)))


_QueryDuration = Duration
del Duration
