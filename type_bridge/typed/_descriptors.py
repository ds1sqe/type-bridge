"""Static descriptor shapes emitted by the Python model generator.

Generated models keep their existing Pydantic declarations at runtime.  Under
``TYPE_CHECKING`` they use these data-descriptor protocols so static analyzers
can distinguish class-level owner-aware references from instance values.
"""

from __future__ import annotations

from typing import Protocol, overload

from type_bridge.attribute.base import _QueryAttribute as Attribute
from type_bridge.attribute.date import _QueryDate as Date
from type_bridge.attribute.datetime import _QueryDateTime as DateTime
from type_bridge.attribute.datetimetz import _QueryDateTimeTZ as DateTimeTZ
from type_bridge.attribute.numeric import NumericAttribute
from type_bridge.attribute.string import _QueryString as String
from type_bridge.fields.base import (
    _QueryFieldRef as FieldRef,
)
from type_bridge.fields.base import (
    _QueryNumericFieldRef as NumericFieldRef,
)
from type_bridge.fields.base import (
    _QueryOrderedFieldRef as OrderedFieldRef,
)
from type_bridge.fields.base import (
    _QueryStringFieldRef as StringFieldRef,
)
from type_bridge.models.base import _QueryTypeDBType as TypeDBType


class GeneratedFieldDescriptor[
    DeclaredOwnerT: TypeDBType,
    AttributeT: Attribute,
    InstanceT,
](Protocol):
    """Class reference plus exact instance value for a generated field."""

    @overload
    def __get__[ActualOwnerT: TypeDBType](
        self,
        instance: None,
        owner: type[ActualOwnerT],
    ) -> FieldRef[AttributeT, ActualOwnerT]: ...

    @overload
    def __get__(
        self,
        instance: DeclaredOwnerT,
        owner: type[DeclaredOwnerT],
    ) -> InstanceT: ...

    def __set__(self, instance: DeclaredOwnerT, value: InstanceT) -> None: ...


class GeneratedStringFieldDescriptor[
    DeclaredOwnerT: TypeDBType,
    AttributeT: String,
    InstanceT,
](Protocol):
    """Generated string field with string-specific bound operations."""

    @overload
    def __get__[ActualOwnerT: TypeDBType](
        self,
        instance: None,
        owner: type[ActualOwnerT],
    ) -> StringFieldRef[AttributeT, ActualOwnerT]: ...

    @overload
    def __get__(
        self,
        instance: DeclaredOwnerT,
        owner: type[DeclaredOwnerT],
    ) -> InstanceT: ...

    def __set__(self, instance: DeclaredOwnerT, value: InstanceT) -> None: ...


class GeneratedNumericFieldDescriptor[
    DeclaredOwnerT: TypeDBType,
    AttributeT: NumericAttribute,
    InstanceT,
](Protocol):
    """Generated numeric field with ordered bound operations."""

    @overload
    def __get__[ActualOwnerT: TypeDBType](
        self,
        instance: None,
        owner: type[ActualOwnerT],
    ) -> NumericFieldRef[AttributeT, ActualOwnerT]: ...

    @overload
    def __get__(
        self,
        instance: DeclaredOwnerT,
        owner: type[DeclaredOwnerT],
    ) -> InstanceT: ...

    def __set__(self, instance: DeclaredOwnerT, value: InstanceT) -> None: ...


class GeneratedOrderedFieldDescriptor[
    DeclaredOwnerT: TypeDBType,
    AttributeT: Date | DateTime | DateTimeTZ,
    InstanceT,
](Protocol):
    """Generated date-like field with ordered bound operations."""

    @overload
    def __get__[ActualOwnerT: TypeDBType](
        self,
        instance: None,
        owner: type[ActualOwnerT],
    ) -> OrderedFieldRef[AttributeT, ActualOwnerT]: ...

    @overload
    def __get__(
        self,
        instance: DeclaredOwnerT,
        owner: type[DeclaredOwnerT],
    ) -> InstanceT: ...

    def __set__(self, instance: DeclaredOwnerT, value: InstanceT) -> None: ...


def generated_descriptor_default[DescriptorT](
    descriptor_type: type[DescriptorT] | None = None,
) -> DescriptorT:
    """Mark a generated descriptor-backed constructor parameter as optional.

    Generated modules call this only in a ``TYPE_CHECKING`` branch. Runtime
    defaults continue to come from their unchanged Pydantic declarations.
    """

    del descriptor_type
    raise RuntimeError("generated descriptor defaults are static-only")


__all__ = [
    "GeneratedFieldDescriptor",
    "GeneratedNumericFieldDescriptor",
    "GeneratedOrderedFieldDescriptor",
    "GeneratedStringFieldDescriptor",
    "generated_descriptor_default",
]
