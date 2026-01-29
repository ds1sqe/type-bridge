"""
Field reference system for type-safe query building.

This module provides field descriptors and references that enable type-safe
query expressions like Person.age.gt(Age(30)).
"""

from typing import TYPE_CHECKING, Any, TypeVar, cast, overload

if TYPE_CHECKING:
    from type_bridge.attribute.base import Attribute
    from type_bridge.attribute.string import String
    from type_bridge.expressions import AggregateExpr, ComparisonExpr, StringExpr
    from type_bridge.models import Entity

# Type variables for constraints
T_Attribute = TypeVar("T_Attribute", bound="Attribute")
T_String = TypeVar("T_String", bound="String")
T_Numeric = TypeVar("T_Numeric")


class FieldRef[T: "Attribute"]:
    """
    Type-safe reference to an entity field.

    Returned when accessing entity class attributes (e.g., Person.age).
    Provides query methods like .gt(), .lt(), etc. that return typed expressions.
    """

    def __init__(self, field_name: str, attr_type: type[T], entity_type: Any):
        """
        Create a field reference.

        Args:
            field_name: Python field name
            attr_type: Attribute type class
            entity_type: Entity type that owns this field
        """
        self.field_name = field_name
        self.attr_type = attr_type
        self.entity_type: Any = entity_type

    def lt(self, value: T) -> "ComparisonExpr[T]":
        """
        Create a less-than comparison expression.

        Args:
            value: Value to compare against

        Returns:
            ComparisonExpr for this field < value
        """
        # Delegate to attribute class method
        return self.attr_type.lt(value)

    def gt(self, value: T) -> "ComparisonExpr[T]":
        """
        Create a greater-than comparison expression.

        Args:
            value: Value to compare against

        Returns:
            ComparisonExpr for this field > value
        """
        # Delegate to attribute class method
        return self.attr_type.gt(value)

    def lte(self, value: T) -> "ComparisonExpr[T]":
        """
        Create a less-than-or-equal comparison expression.

        Args:
            value: Value to compare against

        Returns:
            ComparisonExpr for this field <= value
        """
        # Delegate to attribute class method
        return self.attr_type.lte(value)

    def gte(self, value: T) -> "ComparisonExpr[T]":
        """
        Create a greater-than-or-equal comparison expression.

        Args:
            value: Value to compare against

        Returns:
            ComparisonExpr for this field >= value
        """
        # Delegate to attribute class method
        return self.attr_type.gte(value)

    def eq(self, value: T) -> "ComparisonExpr[T]":
        """
        Create an equality comparison expression.

        Args:
            value: Value to compare against

        Returns:
            ComparisonExpr for this field == value
        """
        # Delegate to attribute class method
        return self.attr_type.eq(value)

    def neq(self, value: T) -> "ComparisonExpr[T]":
        """
        Create a not-equal comparison expression.

        Args:
            value: Value to compare against

        Returns:
            ComparisonExpr for this field != value
        """
        # Delegate to attribute class method
        return self.attr_type.neq(value)


class StringFieldRef[T: "String"](FieldRef[T]):
    """
    Field reference for String attribute types.

    Provides additional string-specific operations like contains, like, regex.
    """

    def contains(self, value: T) -> "StringExpr[T]":
        """
        Create a string contains expression.

        Args:
            value: Substring to search for

        Returns:
            StringExpr for this field contains value
        """
        # Delegate to attribute class method
        return self.attr_type.contains(value)

    def like(self, pattern: T) -> "StringExpr[T]":
        """
        Create a string pattern matching expression (regex).

        Args:
            pattern: Regex pattern to match

        Returns:
            StringExpr for this field like pattern
        """
        # Delegate to attribute class method
        return self.attr_type.like(pattern)

    def regex(self, pattern: T) -> "StringExpr[T]":
        """
        Create a string regex expression (alias for like).

        Args:
            pattern: Regex pattern to match

        Returns:
            StringExpr for this field matching pattern
        """
        # Delegate to attribute class method
        return self.attr_type.regex(pattern)


class NumericFieldRef[T: "Attribute"](FieldRef[T]):
    """
    Field reference for numeric attribute types.

    Provides additional numeric-specific operations like sum, avg, max, min.
    """

    def sum(self) -> "AggregateExpr[T]":
        """
        Create a sum aggregation expression.

        Returns:
            AggregateExpr for sum of this field
        """
        from type_bridge.expressions import AggregateExpr

        return AggregateExpr(attr_type=self.attr_type, function="sum", field_name=self.field_name)

    def avg(self) -> "AggregateExpr[T]":
        """
        Create an average (mean) aggregation expression.

        Returns:
            AggregateExpr for average/mean of this field
        """
        from type_bridge.expressions import AggregateExpr

        return AggregateExpr(attr_type=self.attr_type, function="mean", field_name=self.field_name)

    def max(self) -> "AggregateExpr[T]":
        """
        Create a maximum aggregation expression.

        Returns:
            AggregateExpr for maximum of this field
        """
        from type_bridge.expressions import AggregateExpr

        return AggregateExpr(attr_type=self.attr_type, function="max", field_name=self.field_name)

    def min(self) -> "AggregateExpr[T]":
        """
        Create a minimum aggregation expression.

        Returns:
            AggregateExpr for minimum of this field
        """
        from type_bridge.expressions import AggregateExpr

        return AggregateExpr(attr_type=self.attr_type, function="min", field_name=self.field_name)

    def median(self) -> "AggregateExpr[T]":
        """
        Create a median aggregation expression.

        Returns:
            AggregateExpr for median of this field
        """
        from type_bridge.expressions import AggregateExpr

        return AggregateExpr(
            attr_type=self.attr_type, function="median", field_name=self.field_name
        )

    def std(self) -> "AggregateExpr[T]":
        """
        Create a standard deviation aggregation expression.

        Returns:
            AggregateExpr for standard deviation of this field
        """
        from type_bridge.expressions import AggregateExpr

        return AggregateExpr(attr_type=self.attr_type, function="std", field_name=self.field_name)


class FieldDescriptor[T: "Attribute"]:
    """
    Descriptor for entity fields that supports dual behavior:
    - Class-level access: Returns FieldRef[T] for query building
    - Instance-level access: Returns T (the attribute value)
    """

    def __init__(self, field_name: str, attr_type: type[T]):
        """
        Create a field descriptor.

        Args:
            field_name: Python field name
            attr_type: Attribute type class
        """
        self.field_name = field_name
        self.attr_type = attr_type

    @overload
    def __get__(self, instance: None, owner: Any) -> FieldRef[T]: ...

    @overload
    def __get__(self, instance: "Entity", owner: Any) -> T | None: ...

    def __get__(self, instance: "Entity | None", owner: Any) -> "FieldRef[T] | T | None":
        """
        Get field value or field reference.

        Args:
            instance: Entity instance (None for class-level access)
            owner: Entity class

        Returns:
            FieldRef[T] for class-level access, T | None for instance-level access
        """
        if instance is None:
            # Class-level access: return FieldRef for query building
            return self._make_field_ref(owner)
        # Instance-level access: return attribute value from Pydantic model
        # Pydantic stores field values in instance.__dict__
        return instance.__dict__.get(self.field_name)

    def __set__(self, instance: "Entity", value: T) -> None:
        """
        Set field value on instance.

        Args:
            instance: Entity instance
            value: Attribute value to set
        """
        # Delegate to Pydantic's field validation and storage
        # Use cast to satisfy pyright (instance.__dict__ is always a writable dict at runtime)
        inst_dict: dict[str, Any] = instance.__dict__  # type: ignore[assignment]
        inst_dict[self.field_name] = value
        # Trigger Pydantic validation if needed
        if hasattr(instance, "__pydantic_validator__"):
            instance.__pydantic_validator__.validate_assignment(instance, self.field_name, value)

    def _make_field_ref(self, entity_type: Any) -> FieldRef[T]:
        """
        Create appropriate FieldRef subclass based on attribute type.

        Args:
            entity_type: Entity class that owns this field

        Returns:
            FieldRef subclass instance (FieldRef, StringFieldRef, or NumericFieldRef)
        """
        from type_bridge.attribute.decimal import Decimal
        from type_bridge.attribute.double import Double
        from type_bridge.attribute.integer import Integer
        from type_bridge.attribute.string import String

        # Check if this is a String subclass
        if issubclass(self.attr_type, String):
            return cast(
                FieldRef[T],
                StringFieldRef(
                    field_name=self.field_name,
                    attr_type=self.attr_type,
                    entity_type=entity_type,
                ),
            )

        # Check if this is a numeric type
        if issubclass(self.attr_type, (Integer, Double, Decimal)):
            return cast(
                FieldRef[T],
                NumericFieldRef(
                    field_name=self.field_name,
                    attr_type=self.attr_type,
                    entity_type=entity_type,
                ),
            )

        # Default to base FieldRef
        return FieldRef(
            field_name=self.field_name,
            attr_type=self.attr_type,
            entity_type=entity_type,
        )
