"""Owner-aware typed-query references backed by opaque native handles."""

# ruff: noqa: UP046 -- Selection/player views require explicit covariance.

from __future__ import annotations

from abc import ABC, abstractmethod
from typing import Any, Generic, Literal, TypeVar, overload

from type_bridge_core import (
    MatchBindingHandle as _NativeBindingHandle,
)
from type_bridge_core import (
    MatchFieldHandle as _NativeFieldHandle,
)
from type_bridge_core import (
    MatchOrderHandle as _NativeOrderHandle,
)
from type_bridge_core import (
    MatchPredicateHandle as _NativePredicateHandle,
)
from type_bridge_core import (
    MatchRoleHandle as _NativeRoleHandle,
)
from type_bridge_core import (
    MatchSelectionHandle as _NativeSelectionHandle,
)

from type_bridge._rust_runtime import normalize_value, rust_core, rust_value_type
from type_bridge.attribute.base import Attribute
from type_bridge.attribute.date import Date
from type_bridge.attribute.datetime import DateTime
from type_bridge.attribute.datetimetz import DateTimeTZ
from type_bridge.attribute.numeric import NumericAttribute
from type_bridge.attribute.string import String
from type_bridge.fields.base import (
    FieldRef,
    NumericFieldRef,
    OrderedFieldRef,
    StringFieldRef,
    _typed_query_field_reference_owner,
)
from type_bridge.fields.role import (
    RoleRef,
    _typed_query_role_reference_owner,
)
from type_bridge.models.base import TypeDBType

OutputT_co = TypeVar("OutputT_co", covariant=True)
ModelT = TypeVar("ModelT", bound=TypeDBType)
PlayerT_co = TypeVar("PlayerT_co", bound=TypeDBType, covariant=True)
PlayerT = TypeVar("PlayerT", bound=TypeDBType)
AttributeT = TypeVar("AttributeT", bound=Attribute)
StringAttributeT = TypeVar("StringAttributeT", bound=String)
NumericAttributeT = TypeVar("NumericAttributeT", bound=NumericAttribute)
OrderedAttributeT = TypeVar("OrderedAttributeT", bound=Date | DateTime | DateTimeTZ)

MissingOrder = Literal["reject", "first", "last"]


class _ImmutableNativeValue:
    """Reject reassignment after native-backed wrapper construction."""

    __slots__ = ()

    def __setattr__(self, name: str, value: object) -> None:
        del name, value
        raise AttributeError(f"{type(self).__name__} values are immutable")


class Selection(
    _ImmutableNativeValue,
    Generic[OutputT_co],
    ABC,  # noqa: UP046 - explicit covariance
):
    """Covariant output-only view of one native selection handle."""

    __slots__ = ()

    @abstractmethod
    def _native_selection(self) -> _NativeSelectionHandle:
        """Return the private native selection during later query construction."""

    @abstractmethod
    def _selection_type_brand(self) -> type[OutputT_co]:
        """Retain the static output brand."""


class _PlayerBinding(Generic[PlayerT_co], ABC):  # noqa: UP046 - explicit covariance
    """Covariant role-player view while ``BoundVar`` itself stays invariant."""

    __slots__ = ()

    @abstractmethod
    def _native_binding(self) -> _NativeBindingHandle:
        """Return the native binding to the owner-aware role wrapper."""

    @abstractmethod
    def _player_type_brand(self) -> type[PlayerT_co]:
        """Retain the static player brand without storing runtime model metadata."""


class Predicate(_ImmutableNativeValue):
    """Immutable boolean predicate whose referenced bindings remain native."""

    __slots__ = ("__handle",)

    def __init__(self) -> None:
        raise TypeError("Predicate values are created by bound reference operations")

    @classmethod
    def _from_native(cls, handle: _NativePredicateHandle) -> Predicate:
        value = object.__new__(cls)
        object.__setattr__(value, "_Predicate__handle", handle)
        return value

    def and_(self, other: Predicate) -> Predicate:
        """Return a persistent conjunction of this predicate and ``other``."""
        return Predicate._from_native(self.__handle.and_(other.__handle))

    def or_(self, other: Predicate) -> Predicate:
        """Return a persistent disjunction of this predicate and ``other``."""
        return Predicate._from_native(self.__handle.or_(other.__handle))

    def not_(self) -> Predicate:
        """Return a persistent negation of this predicate."""
        return Predicate._from_native(self.__handle.not_())

    def __and__(self, other: Predicate) -> Predicate:
        return self.and_(other)

    def __or__(self, other: Predicate) -> Predicate:
        return self.or_(other)

    def __invert__(self) -> Predicate:
        return self.not_()

    def _native_predicate(self) -> _NativePredicateHandle:
        return self.__handle


class QueryOrder(_ImmutableNativeValue):
    """Opaque stable-order term created from an order-capable bound field."""

    __slots__ = ("__handle",)

    def __init__(self) -> None:
        raise TypeError("QueryOrder values are created by BoundField.asc/desc")

    @classmethod
    def _from_native(cls, handle: _NativeOrderHandle) -> QueryOrder:
        value = object.__new__(cls)
        object.__setattr__(value, "_QueryOrder__handle", handle)
        return value

    def _native_order(self) -> _NativeOrderHandle:
        return self.__handle


class BoundField[AttributeT: Attribute](_ImmutableNativeValue):
    """Opaque field bound to one invariant native variable occurrence."""

    __slots__ = ("__handle",)

    def __init__(self) -> None:
        raise TypeError("BoundField values are created by BoundVar.field")

    @classmethod
    def _from_native(cls, handle: _NativeFieldHandle) -> BoundField[AttributeT]:
        value = object.__new__(cls)
        object.__setattr__(value, "_BoundField__handle", handle)
        return value

    def eq(self, value: AttributeT | BoundField[AttributeT]) -> Predicate:
        """Compare this field for equality with a typed literal or bound field."""
        return self._compare("equal", value)

    def eq_field(self, field: BoundField[AttributeT]) -> Predicate:
        """Compare this field with another compatible bound field."""
        return self._compare("equal", field)

    def neq(self, value: AttributeT | BoundField[AttributeT]) -> Predicate:
        """Compare this field for inequality with a typed literal or bound field."""
        return self._compare("not_equal", value)

    def _compare(self, operator: str, value: AttributeT | BoundField[AttributeT]) -> Predicate:
        if isinstance(value, BoundField):
            handle = self.__handle.compare_field(operator, value.__handle)
        elif isinstance(value, Attribute):
            handle = self.__handle.compare_value(operator, _dynamic_value(value))
        else:
            raise TypeError("bound-field comparisons require an Attribute or BoundField")
        return Predicate._from_native(handle)

    def _native_field(self) -> _NativeFieldHandle:
        return self.__handle


class _OrderedBoundField[AttributeT: Attribute](BoundField[AttributeT]):
    __slots__ = ()

    def lt(self, value: AttributeT | BoundField[AttributeT]) -> Predicate:
        return self._compare("less_than", value)

    def lte(self, value: AttributeT | BoundField[AttributeT]) -> Predicate:
        return self._compare("less_than_or_equal", value)

    def gt(self, value: AttributeT | BoundField[AttributeT]) -> Predicate:
        return self._compare("greater_than", value)

    def gte(self, value: AttributeT | BoundField[AttributeT]) -> Predicate:
        return self._compare("greater_than_or_equal", value)

    def asc(self, *, missing: MissingOrder = "reject") -> QueryOrder:
        """Order ascending with explicit missing-value behavior."""
        return QueryOrder._from_native(self._native_field().order("ascending", missing))

    def desc(self, *, missing: MissingOrder = "reject") -> QueryOrder:
        """Order descending with explicit missing-value behavior."""
        return QueryOrder._from_native(self._native_field().order("descending", missing))


class _StringBoundField[StringAttributeT: String](_OrderedBoundField[StringAttributeT]):
    __slots__ = ()

    def contains(self, value: StringAttributeT) -> Predicate:
        return self._compare("contains", value)

    def starts_with(self, value: StringAttributeT) -> Predicate:
        return self._compare("starts_with", value)

    def ends_with(self, value: StringAttributeT) -> Predicate:
        return self._compare("ends_with", value)

    def regex(self, value: StringAttributeT) -> Predicate:
        return self._compare("regex", value)


class BoundRole[PlayerT: TypeDBType](_ImmutableNativeValue):
    """Opaque relation role bound to one native relation variable."""

    __slots__ = ("__handle",)

    def __init__(self) -> None:
        raise TypeError("BoundRole values are created by BoundVar.role")

    @classmethod
    def _from_native(cls, handle: _NativeRoleHandle) -> BoundRole[PlayerT]:
        value = object.__new__(cls)
        object.__setattr__(value, "_BoundRole__handle", handle)
        return value

    def connects(self, player: _PlayerBinding[PlayerT]) -> Predicate:
        """Require this role to connect a compatible native bound variable."""
        if not isinstance(player, BoundVar):
            raise TypeError("role players must be BoundVar values")
        return Predicate._from_native(self.__handle.connects(player._native_binding()))

    def is_(self, player: _PlayerBinding[PlayerT]) -> Predicate:
        """Require this role to be played by a compatible bound variable."""
        return self.connects(player)


class BoundVar[ModelT: TypeDBType](Selection[ModelT], _PlayerBinding[ModelT]):
    """Invariant model variable backed by one fresh native binding handle."""

    __slots__ = ("__handle", "__model", "__model_type_name")

    def __init__(self) -> None:
        raise TypeError("BoundVar values are created by QuerySession.var")

    @classmethod
    def _from_native(
        cls,
        handle: _NativeBindingHandle,
        model: type[ModelT],
        model_type_name: str,
    ) -> BoundVar[ModelT]:
        value = object.__new__(cls)
        object.__setattr__(value, "_BoundVar__handle", handle)
        object.__setattr__(value, "_BoundVar__model", model)
        object.__setattr__(value, "_BoundVar__model_type_name", model_type_name)
        return value

    @overload
    def field(self, reference: type[StringAttributeT]) -> _StringBoundField[StringAttributeT]: ...

    @overload
    def field(
        self, reference: type[NumericAttributeT]
    ) -> _OrderedBoundField[NumericAttributeT]: ...

    @overload
    def field(
        self, reference: type[OrderedAttributeT]
    ) -> _OrderedBoundField[OrderedAttributeT]: ...

    @overload
    def field(self, reference: type[AttributeT]) -> BoundField[AttributeT]: ...

    @overload
    def field(
        self, reference: StringFieldRef[StringAttributeT, ModelT]
    ) -> _StringBoundField[StringAttributeT]: ...

    @overload
    def field(
        self, reference: NumericFieldRef[NumericAttributeT, ModelT]
    ) -> _OrderedBoundField[NumericAttributeT]: ...

    @overload
    def field(
        self, reference: OrderedFieldRef[OrderedAttributeT, ModelT]
    ) -> _OrderedBoundField[OrderedAttributeT]: ...

    @overload
    def field(self, reference: FieldRef[AttributeT, ModelT]) -> BoundField[AttributeT]: ...

    def field(self, reference: Any) -> Any:
        """Bind an owned attribute class or legacy model field reference."""
        if isinstance(reference, type) and issubclass(reference, Attribute):
            field_name = self.__field_name_for_attribute(reference)
            handle = self.__handle.field(field_name)
            if issubclass(reference, String):
                return _StringBoundField._from_native(handle)
            if issubclass(reference, (NumericAttribute, Date, DateTime, DateTimeTZ)):
                return _OrderedBoundField._from_native(handle)
            return BoundField._from_native(handle)

        if not isinstance(reference, FieldRef):
            raise TypeError(
                "BoundVar.field requires an owned Attribute class or owner-aware FieldRef"
            )
        model_type_name = self.__stable_model_type_name()
        owner = _typed_query_field_reference_owner(reference)
        if owner is None:
            raise TypeError("BoundVar.field requires a FieldRef emitted by a model descriptor")
        owner_type, owner_type_name = owner
        if owner_type_name == model_type_name and owner_type is not self.__model:
            raise TypeError("BoundVar.field reference owner does not match the bound model")
        handle = self.__handle.field_owned_by(owner_type_name, reference.field_name)
        if isinstance(reference, StringFieldRef):
            return _StringBoundField._from_native(handle)
        if isinstance(reference, (NumericFieldRef, OrderedFieldRef)):
            return _OrderedBoundField._from_native(handle)
        return BoundField._from_native(handle)

    def __field_name_for_attribute(self, attribute: type[Attribute]) -> str:
        """Resolve one exact attribute class through this binding's model owner."""
        self.__stable_model_type_name()
        matches = [
            field_name
            for field_name, info in self.__model.get_all_attributes().items()
            if info.typ is attribute
        ]
        if not matches:
            raise TypeError(f"{self.__model.__name__} does not own attribute {attribute.__name__}")
        if len(matches) != 1:
            joined = ", ".join(sorted(matches))
            raise TypeError(
                f"{self.__model.__name__} owns attribute {attribute.__name__} through "
                f"multiple fields ({joined}); use the owner-aware model field reference"
            )
        return matches[0]

    def role(self, reference: RoleRef[PlayerT, ModelT]) -> BoundRole[PlayerT]:
        """Bind one owner-aware relation role reference to this variable."""
        if not isinstance(reference, RoleRef):
            raise TypeError("BoundVar.role requires an owner-aware RoleRef")
        model_type_name = self.__stable_model_type_name()
        owner = _typed_query_role_reference_owner(reference)
        if owner is None:
            raise TypeError("BoundVar.role requires a RoleRef emitted by a model descriptor")
        owner_type, owner_type_name = owner
        if owner_type_name == model_type_name and owner_type is not self.__model:
            raise TypeError("BoundVar.role reference owner does not match the bound model")
        return BoundRole._from_native(
            self.__handle.role_owned_by(owner_type_name, reference.role_name)
        )

    def __stable_model_type_name(self) -> str:
        try:
            current_type_name = self.__model.get_type_name()
        except Exception as error:
            raise TypeError(
                "bound variable model type name changed after QuerySession.var"
            ) from error
        if current_type_name != self.__model_type_name:
            raise TypeError("bound variable model type name changed after QuerySession.var")
        return self.__model_type_name

    def collect(self) -> Collected[ModelT]:
        """Return a persistent collected selection for this variable."""
        return Collected._from_native(self.__handle.collect())

    def _native_selection(self) -> _NativeSelectionHandle:
        return self.__handle.one()

    def _native_binding(self) -> _NativeBindingHandle:
        return self.__handle

    def _selection_type_brand(self) -> type[ModelT]:
        raise RuntimeError("static selection brands have no runtime value")

    def _player_type_brand(self) -> type[ModelT]:
        raise RuntimeError("static player brands have no runtime value")

    def _invariant_model_brand(self, value: ModelT) -> ModelT:
        """Keep model variables invariant without storing a runtime type brand."""
        raise RuntimeError("static variable brands have no runtime value")


class Collected[ModelT: TypeDBType](Selection[tuple[ModelT, ...]]):
    """Persistent collected-output selection for one bound model variable."""

    __slots__ = ("__handle",)

    def __init__(self) -> None:
        raise TypeError("Collected values are created by BoundVar.collect")

    @classmethod
    def _from_native(cls, handle: _NativeSelectionHandle) -> Collected[ModelT]:
        value = object.__new__(cls)
        object.__setattr__(value, "_Collected__handle", handle)
        return value

    def distinct(self) -> Collected[ModelT]:
        """Return an identity-distinct persistent collection selection."""
        return Collected._from_native(self.__handle.distinct())

    def order_by(self, order: QueryOrder) -> Collected[ModelT]:
        """Append one native collection-member order term persistently."""
        return Collected._from_native(self.__handle.order_by(order._native_order()))

    def _native_selection(self) -> _NativeSelectionHandle:
        return self.__handle

    def _selection_type_brand(self) -> type[tuple[ModelT, ...]]:
        raise RuntimeError("static selection brands have no runtime value")


def _dynamic_value(value: Attribute):
    """Convert one typed Attribute to the existing native value wrapper."""
    core = rust_core()
    value_type = rust_value_type(type(value))
    normalized = normalize_value(value, value_type)
    if value_type == "string":
        return core.DynamicValue.string(str(normalized))
    if value_type == "long":
        return core.DynamicValue.long(int(normalized))
    if value_type == "double":
        return core.DynamicValue.double(float(normalized))
    if value_type == "boolean":
        if not isinstance(normalized, bool):
            raise TypeError("boolean attribute values must normalize to bool")
        return core.DynamicValue.boolean(normalized)
    if value_type == "date":
        return core.DynamicValue.date(str(normalized))
    if value_type == "datetime":
        return core.DynamicValue.datetime(str(normalized))
    if value_type == "datetime-tz":
        return core.DynamicValue.datetime_tz(str(normalized))
    if value_type == "decimal":
        return core.DynamicValue.decimal(str(normalized))
    if value_type == "duration":
        return core.DynamicValue.duration(str(normalized))
    raise ValueError(f"unsupported typed-match value type {value_type!r}")


__all__ = [
    "BoundField",
    "BoundRole",
    "BoundVar",
    "Collected",
    "Predicate",
    "QueryOrder",
    "Selection",
]
