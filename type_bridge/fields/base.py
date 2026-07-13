# ruff: noqa: UP046 -- the defaulted contravariant owner preserves legacy generic arity.

"""Field reference system for type-safe query building.

This module provides field descriptors and references that enable type-safe
query expressions like Person.age.gt(Age(30)).
"""

from typing import TYPE_CHECKING, Any, Generic, Never, TypeVar, overload
from weakref import ReferenceType, WeakKeyDictionary, ref

from type_bridge.models.base import TypeDBType

if TYPE_CHECKING:
    from type_bridge.attribute.base import Attribute
    from type_bridge.attribute.string import String
    from type_bridge.expressions import AggregateExpr, ComparisonExpr, StringExpr
    from type_bridge.models import Entity

# Type variables for constraints
T_Attribute = TypeVar("T_Attribute", bound="Attribute")
T_String = TypeVar("T_String", bound="String")
T_Numeric = TypeVar("T_Numeric")
T_Owner = TypeVar("T_Owner", bound=TypeDBType)
T_Owner_contra = TypeVar(
    "T_Owner_contra",
    bound=TypeDBType,
    contravariant=True,
    default=Never,
)


class FieldRef(Generic[T_Attribute, T_Owner_contra]):
    """Type-safe reference to an entity field.

    Returned when accessing entity class attributes (e.g., Person.age).
    Provides query methods like .gt(), .lt(), etc. that return typed expressions.
    """

    def __init__(
        self,
        field_name: str,
        attr_type: type[T_Attribute],
        entity_type: type[T_Owner_contra],
    ):
        """Create a field reference.

        Args:
            field_name: Python field name
            attr_type: Attribute type class
            entity_type: Entity type that owns this field
        """
        self.field_name = field_name
        self.attr_type = attr_type
        self.entity_type = entity_type

    def lt(self, value: T_Attribute) -> "ComparisonExpr[T_Attribute]":
        """Create a less-than comparison expression.

        Args:
            value: Value to compare against

        Returns:
            ComparisonExpr for this field < value
        """
        # Delegate to attribute class method
        return self.attr_type.lt(value)

    def gt(self, value: T_Attribute) -> "ComparisonExpr[T_Attribute]":
        """Create a greater-than comparison expression.

        Args:
            value: Value to compare against

        Returns:
            ComparisonExpr for this field > value
        """
        # Delegate to attribute class method
        return self.attr_type.gt(value)

    def lte(self, value: T_Attribute) -> "ComparisonExpr[T_Attribute]":
        """Create a less-than-or-equal comparison expression.

        Args:
            value: Value to compare against

        Returns:
            ComparisonExpr for this field <= value
        """
        # Delegate to attribute class method
        return self.attr_type.lte(value)

    def gte(self, value: T_Attribute) -> "ComparisonExpr[T_Attribute]":
        """Create a greater-than-or-equal comparison expression.

        Args:
            value: Value to compare against

        Returns:
            ComparisonExpr for this field >= value
        """
        # Delegate to attribute class method
        return self.attr_type.gte(value)

    def eq(self, value: T_Attribute) -> "ComparisonExpr[T_Attribute]":
        """Create an equality comparison expression.

        Args:
            value: Value to compare against

        Returns:
            ComparisonExpr for this field == value
        """
        # Delegate to attribute class method
        return self.attr_type.eq(value)

    def neq(self, value: T_Attribute) -> "ComparisonExpr[T_Attribute]":
        """Create a not-equal comparison expression.

        Args:
            value: Value to compare against

        Returns:
            ComparisonExpr for this field != value
        """
        # Delegate to attribute class method
        return self.attr_type.neq(value)


class StringFieldRef(
    FieldRef[T_String, T_Owner_contra],
    Generic[T_String, T_Owner_contra],
):
    """Field reference for String attribute types.

    Provides additional string-specific operations like contains, like, regex.
    """

    def contains(self, value: T_String) -> "StringExpr[T_String]":
        """Create a string contains expression.

        Args:
            value: Substring to search for

        Returns:
            StringExpr for this field contains value
        """
        # Delegate to attribute class method
        return self.attr_type.contains(value)

    def like(self, pattern: T_String) -> "StringExpr[T_String]":
        """Create a string pattern matching expression (regex).

        Args:
            pattern: Regex pattern to match

        Returns:
            StringExpr for this field like pattern
        """
        # Delegate to attribute class method
        return self.attr_type.like(pattern)

    def regex(self, pattern: T_String) -> "StringExpr[T_String]":
        """Create a string regex expression (alias for like).

        Args:
            pattern: Regex pattern to match

        Returns:
            StringExpr for this field matching pattern
        """
        # Delegate to attribute class method
        return self.attr_type.regex(pattern)


class NumericFieldRef(
    FieldRef[T_Attribute, T_Owner_contra],
    Generic[T_Attribute, T_Owner_contra],
):
    """Field reference for numeric attribute types.

    Provides additional numeric-specific operations like sum, avg, max, min.
    """

    def sum(self) -> "AggregateExpr[T_Attribute]":
        """Create a sum aggregation expression.

        Returns:
            AggregateExpr for sum of this field
        """
        from type_bridge.expressions import AggregateExpr

        return AggregateExpr(attr_type=self.attr_type, function="sum", field_name=self.field_name)

    def avg(self) -> "AggregateExpr[T_Attribute]":
        """Create an average (mean) aggregation expression.

        Returns:
            AggregateExpr for average/mean of this field
        """
        from type_bridge.expressions import AggregateExpr

        return AggregateExpr(attr_type=self.attr_type, function="mean", field_name=self.field_name)

    def max(self) -> "AggregateExpr[T_Attribute]":
        """Create a maximum aggregation expression.

        Returns:
            AggregateExpr for maximum of this field
        """
        from type_bridge.expressions import AggregateExpr

        return AggregateExpr(attr_type=self.attr_type, function="max", field_name=self.field_name)

    def min(self) -> "AggregateExpr[T_Attribute]":
        """Create a minimum aggregation expression.

        Returns:
            AggregateExpr for minimum of this field
        """
        from type_bridge.expressions import AggregateExpr

        return AggregateExpr(attr_type=self.attr_type, function="min", field_name=self.field_name)

    def median(self) -> "AggregateExpr[T_Attribute]":
        """Create a median aggregation expression.

        Returns:
            AggregateExpr for median of this field
        """
        from type_bridge.expressions import AggregateExpr

        return AggregateExpr(
            attr_type=self.attr_type, function="median", field_name=self.field_name
        )

    def std(self) -> "AggregateExpr[T_Attribute]":
        """Create a standard deviation aggregation expression.

        Returns:
            AggregateExpr for standard deviation of this field
        """
        from type_bridge.expressions import AggregateExpr

        return AggregateExpr(attr_type=self.attr_type, function="std", field_name=self.field_name)


class OrderedFieldRef(
    FieldRef[T_Attribute, T_Owner_contra],
    Generic[T_Attribute, T_Owner_contra],
):
    """Marker reference for non-numeric fields with a total value order."""


type _FieldReferenceSnapshot = tuple[
    ReferenceType[object],
    str,
    type[Any],
    type[TypeDBType],
    str,
]
_TYPED_QUERY_FIELD_REFERENCES: WeakKeyDictionary[object, _FieldReferenceSnapshot] = (
    WeakKeyDictionary()
)


def _mark_typed_query_field_reference[ReferenceT: FieldRef[Any, Any]](
    reference: ReferenceT,
) -> ReferenceT:
    """Record that a field reference came from a real model descriptor."""
    _TYPED_QUERY_FIELD_REFERENCES[reference] = (
        ref(reference),
        reference.field_name,
        reference.attr_type,
        reference.entity_type,
        reference.entity_type.get_type_name(),
    )
    return reference


def _typed_query_field_reference_owner(
    reference: object,
) -> tuple[type[TypeDBType], str] | None:
    """Return immutable owner provenance for one genuine, unchanged reference."""
    if not isinstance(reference, FieldRef):
        return None
    try:
        snapshot = _TYPED_QUERY_FIELD_REFERENCES.get(reference)
    except TypeError:
        return None
    if snapshot is None:
        return None
    original, field_name, attr_type, entity_type, owner_type_name = snapshot
    if (
        original() is not reference
        or reference.field_name != field_name
        or reference.attr_type is not attr_type
        or reference.entity_type is not entity_type
    ):
        return None
    try:
        current_owner_type_name = entity_type.get_type_name()
    except Exception:
        return None
    if current_owner_type_name != owner_type_name:
        return None
    return entity_type, owner_type_name


def _typed_query_field_reference_owner_name(reference: object) -> str | None:
    """Return the immutable owner label for one genuine, unchanged reference."""
    owner = _typed_query_field_reference_owner(reference)
    return owner[1] if owner is not None else None


def _is_typed_query_field_reference(reference: object) -> bool:
    """Return whether a field reference came from a real model descriptor."""
    return _typed_query_field_reference_owner_name(reference) is not None


class FieldDescriptor[T_Attribute: "Attribute"]:
    """Descriptor for entity fields that supports dual behavior:
    - Class-level access: Returns FieldRef[T] for query building
    - Instance-level access: Returns T (the attribute value)
    """

    def __init__(self, field_name: str, attr_type: type[T_Attribute]):
        """Create a field descriptor.

        Args:
            field_name: Python field name
            attr_type: Attribute type class
        """
        self.field_name = field_name
        self.attr_type = attr_type

    @overload
    def __get__(self, instance: None, owner: type[T_Owner]) -> FieldRef[T_Attribute, T_Owner]: ...

    @overload
    def __get__(self, instance: "Entity", owner: type[T_Owner]) -> T_Attribute | None: ...

    def __get__(
        self, instance: "Entity | None", owner: type[T_Owner]
    ) -> "FieldRef[T_Attribute, T_Owner] | T_Attribute | None":
        """Get field value or field reference.

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

    def __set__(self, instance: "Entity", value: T_Attribute) -> None:
        """Set field value on instance.

        Args:
            instance: Entity instance
            value: Attribute value to set
        """
        # Store directly in instance __dict__
        # Note: We don't call validate_assignment() here because:
        # 1. The model_validator _wrap_raw_values already handles attribute wrapping
        # 2. Calling validate_assignment triggers the model validator which would
        #    call object.__setattr__ and trigger this __set__ again (infinite recursion)
        vars(instance)[self.field_name] = value

    def _make_field_ref(self, entity_type: type[T_Owner]) -> FieldRef[T_Attribute, T_Owner]:
        """Create appropriate FieldRef subclass based on attribute type.

        Args:
            entity_type: Entity class that owns this field

        Returns:
            FieldRef subclass instance (FieldRef, StringFieldRef, or NumericFieldRef)
        """
        from type_bridge.attribute.date import Date
        from type_bridge.attribute.datetime import DateTime
        from type_bridge.attribute.datetimetz import DateTimeTZ
        from type_bridge.attribute.decimal import Decimal
        from type_bridge.attribute.double import Double
        from type_bridge.attribute.integer import Integer
        from type_bridge.attribute.string import String

        # Check if this is a String subclass
        if issubclass(self.attr_type, String):
            return _mark_typed_query_field_reference(
                StringFieldRef(
                    field_name=self.field_name,
                    attr_type=self.attr_type,
                    entity_type=entity_type,
                ),
            )

        # Check if this is a numeric type
        if issubclass(self.attr_type, (Integer, Double, Decimal)):
            return _mark_typed_query_field_reference(
                NumericFieldRef(
                    field_name=self.field_name,
                    attr_type=self.attr_type,
                    entity_type=entity_type,
                ),
            )

        if issubclass(self.attr_type, (Date, DateTime, DateTimeTZ)):
            return _mark_typed_query_field_reference(
                OrderedFieldRef(
                    field_name=self.field_name,
                    attr_type=self.attr_type,
                    entity_type=entity_type,
                ),
            )

        # Default to base FieldRef
        return _mark_typed_query_field_reference(
            FieldRef(
                field_name=self.field_name,
                attr_type=self.attr_type,
                entity_type=entity_type,
            )
        )
