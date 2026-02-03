"""Numeric attribute base class with shared arithmetic operators."""

from abc import abstractmethod
from typing import Any, ClassVar, Self

from type_bridge.attribute.base import Attribute


class NumericAttribute(Attribute):
    """Base class for numeric attribute types with arithmetic operators.

    This provides shared implementation for arithmetic operations
    that Integer, Double, and Decimal can inherit.

    Subclasses must implement:
    - _accepted_types: tuple of Python types accepted for operations
    - _coerce_value: convert raw value to the appropriate type
    - __init__: initialize with type-specific value handling
    """

    # Subclasses should define the Python types they accept for arithmetic
    # e.g., Integer: (int,), Double: (int, float)
    _accepted_types: ClassVar[tuple[type, ...]]

    @abstractmethod
    def _coerce_value(self, value: Any) -> Any:
        """Coerce a raw value to the appropriate numeric type.

        Args:
            value: Raw value to coerce

        Returns:
            Value coerced to the appropriate type (int, float, Decimal)
        """
        ...

    def _create_result(self, value: Any) -> Self:
        """Create a new instance of the same type with the given value.

        Args:
            value: The computed value

        Returns:
            New instance of the same attribute type
        """
        return self.__class__(value)

    def _get_operand_value(self, other: object) -> Any | None:
        """Extract numeric value from operand.

        Args:
            other: The other operand (raw value or NumericAttribute)

        Returns:
            The numeric value, or None if not a supported type
        """
        if isinstance(other, self._accepted_types):
            return other
        elif isinstance(other, NumericAttribute):
            return other.value
        return None

    # Arithmetic operators

    def __add__(self, other: object) -> Self:
        """Add two numeric values."""
        other_val = self._get_operand_value(other)
        if other_val is not None:
            return self._create_result(self.value + other_val)
        return NotImplemented

    def __radd__(self, other: object) -> Self:
        """Right-hand add."""
        if isinstance(other, self._accepted_types):
            return self._create_result(other + self.value)
        return NotImplemented

    def __sub__(self, other: object) -> Self:
        """Subtract two numeric values."""
        other_val = self._get_operand_value(other)
        if other_val is not None:
            return self._create_result(self.value - other_val)
        return NotImplemented

    def __rsub__(self, other: object) -> Self:
        """Right-hand subtract."""
        if isinstance(other, self._accepted_types):
            return self._create_result(other - self.value)
        return NotImplemented

    def __mul__(self, other: object) -> Self:
        """Multiply two numeric values."""
        other_val = self._get_operand_value(other)
        if other_val is not None:
            return self._create_result(self.value * other_val)
        return NotImplemented

    def __rmul__(self, other: object) -> Self:
        """Right-hand multiply."""
        if isinstance(other, self._accepted_types):
            return self._create_result(other * self.value)
        return NotImplemented

    def __truediv__(self, other: object) -> Self:
        """True division."""
        other_val = self._get_operand_value(other)
        if other_val is not None:
            return self._create_result(self.value / other_val)
        return NotImplemented

    def __rtruediv__(self, other: object) -> Self:
        """Right-hand true division."""
        if isinstance(other, self._accepted_types):
            return self._create_result(other / self.value)
        return NotImplemented

    def __floordiv__(self, other: object) -> Self:
        """Floor division."""
        other_val = self._get_operand_value(other)
        if other_val is not None:
            return self._create_result(self.value // other_val)
        return NotImplemented

    def __rfloordiv__(self, other: object) -> Self:
        """Right-hand floor division."""
        if isinstance(other, self._accepted_types):
            return self._create_result(other // self.value)
        return NotImplemented

    def __mod__(self, other: object) -> Self:
        """Modulo operation."""
        other_val = self._get_operand_value(other)
        if other_val is not None:
            return self._create_result(self.value % other_val)
        return NotImplemented

    def __rmod__(self, other: object) -> Self:
        """Right-hand modulo."""
        if isinstance(other, self._accepted_types):
            return self._create_result(other % self.value)
        return NotImplemented

    def __pow__(self, other: object) -> Self:
        """Power operation."""
        other_val = self._get_operand_value(other)
        if other_val is not None:
            return self._create_result(self.value**other_val)
        return NotImplemented

    def __rpow__(self, other: object) -> Self:
        """Right-hand power."""
        if isinstance(other, self._accepted_types):
            return self._create_result(other**self.value)
        return NotImplemented

    def __neg__(self) -> Self:
        """Negate the value."""
        return self._create_result(-self.value)

    def __pos__(self) -> Self:
        """Positive (unary +)."""
        return self._create_result(+self.value)

    def __abs__(self) -> Self:
        """Absolute value."""
        return self._create_result(abs(self.value))
