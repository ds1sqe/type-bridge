"""Double attribute type for TypeDB."""

from typing import Any, ClassVar, Self, TypeVar

from pydantic_core import core_schema

from type_bridge.attribute.numeric import NumericAttribute

# TypeVar for proper type checking
FloatValue = TypeVar("FloatValue", bound=float)


class Double(NumericAttribute):
    """Double precision float attribute type that accepts float values.

    Example:
        class Price(Double):
            pass

        class Score(Double):
            pass
    """

    value_type: ClassVar[str] = "double"
    _accepted_types: ClassVar[tuple[type, ...]] = (int, float)

    def __init__(self, value: float):
        """Initialize Double attribute with a float value.

        Args:
            value: The float value to store

        Raises:
            ValueError: If value violates range_constraint
        """
        float_value = float(value)

        # Check range constraint if defined on the class
        range_constraint = getattr(self.__class__, "range_constraint", None)
        if range_constraint is not None:
            range_min, range_max = range_constraint
            if range_min is not None:
                min_val = float(range_min)
                if float_value < min_val:
                    raise ValueError(
                        f"{self.__class__.__name__} value {float_value} is below minimum {min_val}"
                    )
            if range_max is not None:
                max_val = float(range_max)
                if float_value > max_val:
                    raise ValueError(
                        f"{self.__class__.__name__} value {float_value} is above maximum {max_val}"
                    )

        super().__init__(float_value)

    def _coerce_value(self, value: Any) -> float:
        """Coerce value to float."""
        return float(value)

    @property
    def value(self) -> float:
        """Get the stored float value."""
        return self._value if self._value is not None else 0.0

    def __float__(self) -> float:
        """Convert to float."""
        return float(self.value)

    # Note: Arithmetic operators (__add__, __sub__, __mul__, etc.) are inherited
    # from NumericAttribute base class

    # Pydantic integration hooks (used by base class __get_pydantic_core_schema__)

    @classmethod
    def _get_default_value(cls) -> float:
        """Default value for Double is 0.0."""
        return 0.0

    @classmethod
    def _get_pydantic_return_schema(cls) -> core_schema.CoreSchema:
        """Return schema for float serialization."""
        return core_schema.float_schema()

    @classmethod
    def _pydantic_serialize(cls, value: Any) -> float:
        """Serialize Double to raw float value."""
        if isinstance(value, cls):
            return float(value._value) if value._value is not None else 0.0
        return float(value)

    @classmethod
    def _pydantic_validate(cls, value: Any) -> Self:
        """Validate and wrap value in Double instance with range checking."""
        if isinstance(value, cls):
            float_value = value._value
        else:
            float_value = float(value)

        # Check range constraint if defined on the class
        range_constraint = getattr(cls, "range_constraint", None)
        if range_constraint is not None:
            range_min, range_max = range_constraint
            if range_min is not None:
                min_val = float(range_min)
                if float_value < min_val:
                    raise ValueError(
                        f"{cls.__name__} value {float_value} is below minimum {min_val}"
                    )
            if range_max is not None:
                max_val = float(range_max)
                if float_value > max_val:
                    raise ValueError(
                        f"{cls.__name__} value {float_value} is above maximum {max_val}"
                    )

        if isinstance(value, cls):
            return value  # Return attribute instance as-is
        return cls(float_value)  # Wrap raw float in attribute instance


_QueryDouble = Double
del Double
