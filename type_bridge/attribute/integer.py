"""Integer attribute type for TypeDB."""

from typing import Any, ClassVar, Self, TypeVar

from pydantic_core import core_schema

from type_bridge.attribute.numeric import NumericAttribute

# TypeVar for proper type checking
IntValue = TypeVar("IntValue", bound=int)


class Integer(NumericAttribute):
    """Integer attribute type that accepts int values.

    Example:
        class Age(Integer):
            pass

        class Count(Integer):
            pass

        # With Literal for type safety
        class Priority(Integer):
            pass

        priority: Literal[1, 2, 3] | Priority
    """

    value_type: ClassVar[str] = "integer"
    _accepted_types: ClassVar[tuple[type, ...]] = (int,)

    def __init__(self, value: int):
        """Initialize Integer attribute with an integer value.

        Args:
            value: The integer value to store

        Raises:
            ValueError: If value violates range_constraint
        """
        int_value = int(value)

        # Check range constraint if defined on the class
        range_constraint = getattr(self.__class__, "range_constraint", None)
        if range_constraint is not None:
            range_min, range_max = range_constraint
            if range_min is not None:
                min_val = int(range_min)
                if int_value < min_val:
                    raise ValueError(
                        f"{self.__class__.__name__} value {int_value} is below minimum {min_val}"
                    )
            if range_max is not None:
                max_val = int(range_max)
                if int_value > max_val:
                    raise ValueError(
                        f"{self.__class__.__name__} value {int_value} is above maximum {max_val}"
                    )

        super().__init__(int_value)

    def _coerce_value(self, value: Any) -> int:
        """Coerce value to int."""
        return int(value)

    @property
    def value(self) -> int:
        """Get the stored integer value."""
        return self._value if self._value is not None else 0

    def __int__(self) -> int:
        """Convert to int."""
        return int(self.value)

    # Note: Arithmetic operators (__add__, __sub__, __mul__, etc.) are inherited
    # from NumericAttribute base class

    # Pydantic integration hooks (used by base class __get_pydantic_core_schema__)

    @classmethod
    def _get_default_value(cls) -> int:
        """Default value for Integer is 0."""
        return 0

    @classmethod
    def _get_pydantic_return_schema(cls) -> core_schema.CoreSchema:
        """Return schema for integer serialization."""
        return core_schema.int_schema()

    @classmethod
    def _pydantic_serialize(cls, value: Any) -> int:
        """Serialize Integer to raw int value."""
        if isinstance(value, cls):
            return int(value._value) if value._value is not None else 0
        return int(value)

    @classmethod
    def _pydantic_validate(cls, value: Any) -> Self:
        """Validate and wrap value in Integer instance with range checking."""
        if isinstance(value, cls):
            int_value = value._value
        else:
            int_value = int(value)

        # Check range constraint if defined on the class
        range_constraint = getattr(cls, "range_constraint", None)
        if range_constraint is not None:
            range_min, range_max = range_constraint
            if range_min is not None:
                min_val = int(range_min)
                if int_value < min_val:
                    raise ValueError(f"{cls.__name__} value {int_value} is below minimum {min_val}")
            if range_max is not None:
                max_val = int(range_max)
                if int_value > max_val:
                    raise ValueError(f"{cls.__name__} value {int_value} is above maximum {max_val}")

        if isinstance(value, cls):
            return value  # Return attribute instance as-is
        return cls(int_value)  # Wrap raw int in attribute instance

    @classmethod
    def _supports_literal_types(cls) -> bool:
        """Integer supports Literal type annotations."""
        return True
