"""Boolean attribute type for TypeDB."""

from typing import Any, ClassVar, Self, TypeVar

from pydantic_core import core_schema

from type_bridge.attribute.base import Attribute

# TypeVar for proper type checking
BoolValue = TypeVar("BoolValue", bound=bool)


class Boolean(Attribute):
    """Boolean attribute type that accepts bool values.

    Example:
        class IsActive(Boolean):
            pass

        class IsVerified(Boolean):
            pass
    """

    value_type: ClassVar[str] = "boolean"

    def __init__(self, value: bool):
        """Initialize Boolean attribute with a bool value.

        Args:
            value: The boolean value to store
        """
        if not isinstance(value, bool):
            raise TypeError(f"{self.__class__.__name__} expects bool, got {type(value).__name__}")
        super().__init__(value)

    @property
    def value(self) -> bool:
        """Get the stored boolean value."""
        return self._value if self._value is not None else False

    def __bool__(self) -> bool:
        """Convert to bool."""
        return bool(self.value)

    # Pydantic integration hooks (used by base class __get_pydantic_core_schema__)

    @classmethod
    def _get_default_value(cls) -> bool:
        """Default value for Boolean is False."""
        return False

    @classmethod
    def _get_pydantic_return_schema(cls) -> core_schema.CoreSchema:
        """Return schema for bool serialization."""
        return core_schema.bool_schema()

    @classmethod
    def _pydantic_serialize(cls, value: Any) -> bool:
        """Serialize Boolean to raw bool value."""
        if isinstance(value, cls):
            return value.value
        if isinstance(value, bool):
            return value
        raise TypeError(f"{cls.__name__} expects bool, got {type(value).__name__}")

    @classmethod
    def _pydantic_validate(cls, value: Any) -> Self:
        """Validate and wrap value in Boolean instance."""
        if isinstance(value, cls):
            return value
        if isinstance(value, bool):
            return cls(value)
        raise TypeError(f"{cls.__name__} expects bool, got {type(value).__name__}")
