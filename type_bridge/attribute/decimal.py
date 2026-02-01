"""Decimal attribute type for TypeDB."""

from decimal import Decimal as DecimalType
from typing import Any, ClassVar, Self, TypeVar

from pydantic_core import core_schema

from type_bridge.attribute.numeric import NumericAttribute

# TypeVar for proper type checking
DecimalValue = TypeVar("DecimalValue", bound=DecimalType)


class Decimal(NumericAttribute):
    """Decimal attribute type that accepts fixed-point decimal values.

    This maps to TypeDB's 'decimal' type, which is a fixed-point signed decimal number
    with 64 bits to the left of the decimal point and 19 decimal digits of precision
    after the point.

    Range: −2^63 to 2^63 − 10^−19 (inclusive)

    Example:
        from decimal import Decimal as DecimalType

        class AccountBalance(Decimal):
            pass

        class Price(Decimal):
            pass

        # Usage with decimal values
        balance = AccountBalance(DecimalType("1234.567890"))
        price = Price(DecimalType("0.02"))
    """

    value_type: ClassVar[str] = "decimal"
    _accepted_types: ClassVar[tuple[type, ...]] = (DecimalType, int, float, str)

    def __init__(self, value: DecimalType | str | int | float):
        """Initialize Decimal attribute with a decimal value.

        Args:
            value: The decimal value to store. Can be:
                - decimal.Decimal instance
                - str that can be parsed as decimal
                - int or float (will be converted to Decimal)

        Example:
            from decimal import Decimal as DecimalType

            # From Decimal
            balance = AccountBalance(DecimalType("123.45"))

            # From string (recommended for precision)
            balance = AccountBalance("123.45")

            # From int or float (may lose precision)
            balance = AccountBalance(123.45)
        """
        if not isinstance(value, DecimalType):
            value = DecimalType(str(value))
        super().__init__(value)

    def _coerce_value(self, value: Any) -> DecimalType:
        """Coerce value to DecimalType."""
        if isinstance(value, DecimalType):
            return value
        return DecimalType(str(value))

    @property
    def value(self) -> DecimalType:
        """Get the stored decimal value."""
        return self._value if self._value is not None else DecimalType("0")

    # Note: Arithmetic operators (__add__, __sub__, __mul__, etc.) are inherited
    # from NumericAttribute base class

    # Pydantic integration hooks (used by base class __get_pydantic_core_schema__)

    @classmethod
    def _get_default_value(cls) -> DecimalType:
        """Default value for Decimal is 0."""
        return DecimalType("0")

    @classmethod
    def _get_pydantic_return_schema(cls) -> core_schema.CoreSchema:
        """Return schema for Decimal serialization."""
        return core_schema.decimal_schema()

    @classmethod
    def _pydantic_serialize(cls, value: Any) -> DecimalType:
        """Serialize Decimal to raw DecimalType value."""
        if isinstance(value, cls):
            return value._value if value._value is not None else DecimalType("0")
        if isinstance(value, DecimalType):
            return value
        return DecimalType(str(value))

    @classmethod
    def _pydantic_validate(cls, value: Any) -> Self:
        """Validate and wrap value in Decimal instance.

        Handles TypeDB's 'dec' suffix stripping.
        """
        if isinstance(value, cls):
            return value
        if isinstance(value, DecimalType):
            return cls(value)
        # Try to parse from string, int, or float
        # Strip 'dec' suffix if present (TypeDB returns decimals with 'dec' suffix)
        value_str = str(value)
        if value_str.endswith("dec"):
            value_str = value_str[:-3]  # Remove 'dec' suffix
        return cls(DecimalType(value_str))
