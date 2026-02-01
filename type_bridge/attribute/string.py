import re
from typing import TYPE_CHECKING, Any, ClassVar, Self, TypeVar

from pydantic_core import core_schema

from type_bridge.attribute.base import Attribute

if TYPE_CHECKING:
    from type_bridge.expressions import Expression, StringExpr

# TypeVar for proper type checking
StrValue = TypeVar("StrValue", bound=str)

# Type alias for String subclasses
StringType = TypeVar("StringType", bound="String")


class String(Attribute):
    """String attribute type that accepts str values.

    Example:
        class Name(String):
            pass

        class Email(String):
            pass

        # With Literal for type safety
        class Status(String):
            pass

        status: Literal["active", "inactive"] | Status
    """

    value_type: ClassVar[str] = "string"

    def __init__(self, value: str):
        """Initialize String attribute with a string value.

        Args:
            value: The string value to store
        """
        super().__init__(value)

    @property
    def value(self) -> str:
        """Get the stored string value."""
        return self._value if self._value is not None else ""

    def __str__(self) -> str:
        """Convert to string."""
        return str(self.value)

    def __add__(self, other: object) -> "String":
        """Concatenate strings."""
        if isinstance(other, str):
            return String(self.value + other)
        elif isinstance(other, String):
            return String(self.value + other.value)
        else:
            return NotImplemented

    def __radd__(self, other: object) -> "String":
        """Right-hand string concatenation."""
        if isinstance(other, str):
            return String(other + self.value)
        else:
            return NotImplemented

    # Pydantic integration hooks (used by base class __get_pydantic_core_schema__)

    @classmethod
    def _get_default_value(cls) -> str:
        """Default value for String is empty string."""
        return ""

    @classmethod
    def _get_pydantic_return_schema(cls) -> core_schema.CoreSchema:
        """Return schema for string serialization."""
        return core_schema.str_schema()

    @classmethod
    def _pydantic_serialize(cls, value: Any) -> str:
        """Serialize String to raw str value."""
        if isinstance(value, cls):
            return str(value._value) if value._value is not None else ""
        return str(value)

    @classmethod
    def _pydantic_validate(cls, value: Any) -> Self:
        """Validate and wrap value in String instance."""
        if isinstance(value, cls):
            return value
        return cls(str(value))

    @classmethod
    def _supports_literal_types(cls) -> bool:
        """String supports Literal type annotations."""
        return True

    # ========================================================================
    # String Query Expression Class Methods (Type-Safe API)
    # ========================================================================

    @classmethod
    def contains(cls, value: "String") -> "StringExpr":
        """Create contains string expression.

        Args:
            value: String value to search for

        Returns:
            StringExpr for attr contains value

        Example:
            Email.contains(Email("@company.com"))  # email contains "@company.com"
        """
        from type_bridge.expressions import StringExpr

        return StringExpr(attr_type=cls, operation="contains", pattern=value)

    @classmethod
    def like(cls, pattern: "String") -> "StringExpr":
        """Create regex pattern matching expression.

        Args:
            pattern: Regex pattern to match

        Returns:
            StringExpr for attr like pattern

        Example:
            Name.like(Name("^A.*"))  # name starts with 'A'
        """
        from type_bridge.expressions import StringExpr

        return StringExpr(attr_type=cls, operation="like", pattern=pattern)

    @classmethod
    def regex(cls, pattern: "String") -> "StringExpr":
        """Create regex pattern matching expression (alias for like).

        Note:
            Automatically converts to TypeQL 'like' operator.
            Both 'like' and 'regex' perform regex pattern matching in TypeDB.

        Args:
            pattern: Regex pattern to match

        Returns:
            StringExpr for attr like pattern

        Example:
            Email.regex(Email(".*@gmail\\.com"))  # Generates TypeQL: $email like ".*@gmail\\.com"
        """
        from type_bridge.expressions import StringExpr

        return StringExpr(attr_type=cls, operation="regex", pattern=pattern)

    @classmethod
    def startswith(cls, prefix: "String") -> "StringExpr":
        """Create startswith string expression.

        Args:
            prefix: Prefix string to check for

        Returns:
            StringExpr for attr like "^prefix.*"
        """
        # Unwrap if it's an Attribute instance to get the raw string for regex construction
        # Note: Type-safe signature says "String", but we need the raw value
        raw_prefix = prefix.value if isinstance(prefix, String) else str(prefix)
        pattern = f"^{re.escape(raw_prefix)}.*"
        return cls.regex(cls(pattern))

    @classmethod
    def endswith(cls, suffix: "String") -> "StringExpr":
        """Create endswith string expression.

        Args:
            suffix: Suffix string to check for

        Returns:
            StringExpr for attr like ".*suffix$"
        """
        # Unwrap if it's an Attribute instance to get the raw string for regex construction
        raw_suffix = suffix.value if isinstance(suffix, String) else str(suffix)
        pattern = f".*{re.escape(raw_suffix)}$"
        return cls.regex(cls(pattern))

    @classmethod
    def build_lookup(cls, lookup: str, value: Any) -> "Expression":
        """Build an expression for string-specific lookups.

        Overrides base method to handle contains, regex, startswith, endswith.
        """
        if lookup in ("contains", "regex", "startswith", "endswith", "like"):
            # Ensure value is wrapped in String for method calls
            wrapped_val = value if isinstance(value, cls) else cls(str(value))

            if lookup == "contains":
                return cls.contains(wrapped_val)
            elif lookup == "regex" or lookup == "like":
                return cls.regex(wrapped_val)
            elif lookup == "startswith":
                return cls.startswith(wrapped_val)
            elif lookup == "endswith":
                return cls.endswith(wrapped_val)

        # Delegate to base for standard operators (eq, in, isnull, etc.)
        return super().build_lookup(lookup, value)
