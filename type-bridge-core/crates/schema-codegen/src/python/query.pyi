from collections.abc import Awaitable, Callable, Iterable, Sequence
from typing import Literal, overload

from type_bridge_core import PyRuntimeProjection

from type_bridge.query_v2 import QueryV2Authority
from type_bridge.session import Database, TransactionContext

from ._runtime import (
    AttributeBase,
    DoubleAttributeBase,
    FieldToken,
    LongAttributeBase,
    ModelBase,
    RelationBase,
    RoleToken,
)

def install_projection(projection: PyRuntimeProjection) -> None: ...

class Predicate:
    def and_(self, other: Predicate) -> Predicate: ...
    def or_(self, other: Predicate) -> Predicate: ...
    def not_(self) -> Predicate: ...
    def __and__(self, other: Predicate) -> Predicate: ...
    def __or__(self, other: Predicate) -> Predicate: ...
    def __invert__(self) -> Predicate: ...

class QueryOrder: ...
class Aggregate[OutputT_co]: ...

class _AggregateFactory:
    def count(self) -> Aggregate[int]: ...
    @overload
    def sum[AttributeT: LongAttributeBase](
        self,
        field: BoundField[AttributeT],
    ) -> Aggregate[int]: ...
    @overload
    def sum[AttributeT: DoubleAttributeBase](
        self,
        field: BoundField[AttributeT],
    ) -> Aggregate[float]: ...
    @overload
    def min[AttributeT: LongAttributeBase](
        self,
        field: BoundField[AttributeT],
    ) -> Aggregate[int | None]: ...
    @overload
    def min[AttributeT: DoubleAttributeBase](
        self,
        field: BoundField[AttributeT],
    ) -> Aggregate[float | None]: ...
    @overload
    def max[AttributeT: LongAttributeBase](
        self,
        field: BoundField[AttributeT],
    ) -> Aggregate[int | None]: ...
    @overload
    def max[AttributeT: DoubleAttributeBase](
        self,
        field: BoundField[AttributeT],
    ) -> Aggregate[float | None]: ...
    def mean[AttributeT: LongAttributeBase | DoubleAttributeBase](
        self,
        field: BoundField[AttributeT],
    ) -> Aggregate[float | None]: ...
    def median[AttributeT: LongAttributeBase | DoubleAttributeBase](
        self,
        field: BoundField[AttributeT],
    ) -> Aggregate[float | None]: ...
    def std[AttributeT: LongAttributeBase | DoubleAttributeBase](
        self,
        field: BoundField[AttributeT],
    ) -> Aggregate[float | None]: ...

aggregate: _AggregateFactory

class Page[T](Sequence[T]):
    @property
    def items(self) -> tuple[T, ...]: ...
    @property
    def offset(self) -> int: ...
    @property
    def limit(self) -> int: ...
    @property
    def total(self) -> int | None: ...

class BoundField[AttributeT: AttributeBase]:
    def eq(self, value: AttributeT | BoundField[AttributeT]) -> Predicate: ...
    def neq(self, value: AttributeT | BoundField[AttributeT]) -> Predicate: ...
    def lt(self, value: AttributeT | BoundField[AttributeT]) -> Predicate: ...
    def lte(self, value: AttributeT | BoundField[AttributeT]) -> Predicate: ...
    def gt(self, value: AttributeT | BoundField[AttributeT]) -> Predicate: ...
    def gte(self, value: AttributeT | BoundField[AttributeT]) -> Predicate: ...
    def contains(self, value: AttributeT) -> Predicate: ...
    def starts_with(self, value: AttributeT) -> Predicate: ...
    def ends_with(self, value: AttributeT) -> Predicate: ...
    def regex(self, value: AttributeT) -> Predicate: ...
    def is_present(self) -> Predicate: ...
    def is_missing(self) -> Predicate: ...
    def asc(
        self,
        *,
        missing: Literal["reject", "first", "last"] = ...,
    ) -> QueryOrder: ...
    def desc(
        self,
        *,
        missing: Literal["reject", "first", "last"] = ...,
    ) -> QueryOrder: ...

class BoundRole[CompatibleBindingT]:
    def connects(self, player: CompatibleBindingT) -> Predicate: ...
    def is_(self, player: CompatibleBindingT) -> Predicate: ...

class Selection[OutputT_co]: ...
class _MatchBinding: ...

class BoundVar[ModelT: ModelBase](Selection[ModelT], _MatchBinding):
    @property
    def model(self) -> type[ModelT]: ...
    def field[AttributeT: AttributeBase](
        self,
        token: FieldToken[ModelT, AttributeT],
    ) -> BoundField[AttributeT]: ...
    def role[PlayerT: ModelBase, CompatibleBindingT](
        self,
        token: RoleToken[ModelT, PlayerT, CompatibleBindingT],
    ) -> BoundRole[CompatibleBindingT]: ...
    def iid(self, iid: str) -> Predicate: ...
    def iid_in(self, iids: Iterable[str]) -> Predicate: ...
    def collect(self) -> Collected[ModelT]: ...

class SubtypeBoundVar[ModelT: ModelBase](BoundVar[ModelT]): ...

class Collected[ModelT: ModelBase](Selection[tuple[ModelT, ...]]):
    def distinct(self, enabled: bool = ...) -> Collected[ModelT]: ...
    def order_by(self, order: QueryOrder) -> Collected[ModelT]: ...

class Query[*Slots]:
    def match(self, *bindings: _MatchBinding) -> Query[*Slots]: ...
    def where(self, *predicates: Predicate) -> Query[*Slots]: ...
    def allow_cross_join[LeftT: ModelBase, RightT: ModelBase](
        self,
        left: BoundVar[LeftT],
        right: BoundVar[RightT],
    ) -> Query[*Slots]: ...
    @overload
    def one[SlotT](self: Query[SlotT]) -> SlotT: ...
    @overload
    def one[Slot1T, Slot2T, *RestT](
        self: Query[Slot1T, Slot2T, *RestT],
    ) -> tuple[Slot1T, Slot2T, *RestT]: ...
    @overload
    def first[SlotT](
        self: Query[SlotT],
        *,
        order_by: Iterable[QueryOrder] = ...,
    ) -> SlotT | None: ...
    @overload
    def first[Slot1T, Slot2T, *RestT](
        self: Query[Slot1T, Slot2T, *RestT],
        *,
        order_by: Iterable[QueryOrder] = ...,
    ) -> tuple[Slot1T, Slot2T, *RestT] | None: ...
    @overload
    def rows[SlotT](
        self: Query[SlotT],
        *,
        limit: int,
        offset: int = ...,
        order_by: Iterable[QueryOrder] = ...,
    ) -> list[SlotT]: ...
    @overload
    def rows[Slot1T, Slot2T, *RestT](
        self: Query[Slot1T, Slot2T, *RestT],
        *,
        limit: int,
        offset: int = ...,
        order_by: Iterable[QueryOrder] = ...,
    ) -> list[tuple[Slot1T, Slot2T, *RestT]]: ...
    # BEGIN GENERATED PAGE OVERLOADS
    @overload
    def page_by[SlotT, RootT: ModelBase](
        self: Query[SlotT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[SlotT]: ...

    @overload
    def page_by[RootT: ModelBase, Collected2T: ModelBase](
        self: Query[RootT, tuple[Collected2T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase](
        self: Query[tuple[Collected1T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected12T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected13T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected14T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected15T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected16T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT, tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT, tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], RootT]]: ...

    # END GENERATED PAGE OVERLOADS
    def count_by[RootT: ModelBase](self, root: BoundVar[RootT]) -> int: ...
    def exists_by[RootT: ModelBase](self, root: BoundVar[RootT]) -> bool: ...
    # BEGIN GENERATED AGGREGATE OVERLOADS
    @overload
    def aggregate[RootT: ModelBase, Output1T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        /,
    ) -> tuple[Output1T]: ...

    @overload
    def aggregate[RootT: ModelBase, Output1T, Output2T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        /,
    ) -> tuple[Output1T, Output2T]: ...

    @overload
    def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T]: ...

    @overload
    def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T]: ...

    @overload
    def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T]: ...

    @overload
    def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T]: ...

    @overload
    def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T]: ...

    @overload
    def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T]: ...

    @overload
    def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T]: ...

    @overload
    def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T]: ...

    @overload
    def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T]: ...

    @overload
    def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        term12: Aggregate[Output12T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T]: ...

    @overload
    def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        term12: Aggregate[Output12T],
        term13: Aggregate[Output13T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T]: ...

    @overload
    def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        term12: Aggregate[Output12T],
        term13: Aggregate[Output13T],
        term14: Aggregate[Output14T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T]: ...

    @overload
    def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        term12: Aggregate[Output12T],
        term13: Aggregate[Output13T],
        term14: Aggregate[Output14T],
        term15: Aggregate[Output15T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T]: ...

    @overload
    def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T, Output16T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        term12: Aggregate[Output12T],
        term13: Aggregate[Output13T],
        term14: Aggregate[Output14T],
        term15: Aggregate[Output15T],
        term16: Aggregate[Output16T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T, Output16T]: ...

    # END GENERATED AGGREGATE OVERLOADS
    # BEGIN GENERATED GROUP BY OVERLOADS
    @overload
    def group_by[RootT: ModelBase, GroupT: ModelBase](
        self,
        root: BoundVar[RootT],
        group: BoundVar[GroupT],
    ) -> GroupedQuery[GroupT]: ...

    @overload
    def group_by[RootT: ModelBase, GroupT: AttributeBase](
        self,
        root: BoundVar[RootT],
        group: BoundField[GroupT],
    ) -> GroupedQuery[GroupT]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
    ) -> GroupedQuery[tuple[Group1T, Group2T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
    ) -> GroupedQuery[tuple[Group1T, Group2T, Group3T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
    ) -> GroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
    ) -> GroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase, Group6T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
        group6: BoundField[Group6T],
    ) -> GroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase, Group6T: AttributeBase, Group7T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
        group6: BoundField[Group6T],
        group7: BoundField[Group7T],
    ) -> GroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase, Group6T: AttributeBase, Group7T: AttributeBase, Group8T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
        group6: BoundField[Group6T],
        group7: BoundField[Group7T],
        group8: BoundField[Group8T],
    ) -> GroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase, Group6T: AttributeBase, Group7T: AttributeBase, Group8T: AttributeBase, Group9T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
        group6: BoundField[Group6T],
        group7: BoundField[Group7T],
        group8: BoundField[Group8T],
        group9: BoundField[Group9T],
    ) -> GroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase, Group6T: AttributeBase, Group7T: AttributeBase, Group8T: AttributeBase, Group9T: AttributeBase, Group10T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
        group6: BoundField[Group6T],
        group7: BoundField[Group7T],
        group8: BoundField[Group8T],
        group9: BoundField[Group9T],
        group10: BoundField[Group10T],
    ) -> GroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T, Group10T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase, Group6T: AttributeBase, Group7T: AttributeBase, Group8T: AttributeBase, Group9T: AttributeBase, Group10T: AttributeBase, Group11T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
        group6: BoundField[Group6T],
        group7: BoundField[Group7T],
        group8: BoundField[Group8T],
        group9: BoundField[Group9T],
        group10: BoundField[Group10T],
        group11: BoundField[Group11T],
    ) -> GroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T, Group10T, Group11T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase, Group6T: AttributeBase, Group7T: AttributeBase, Group8T: AttributeBase, Group9T: AttributeBase, Group10T: AttributeBase, Group11T: AttributeBase, Group12T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
        group6: BoundField[Group6T],
        group7: BoundField[Group7T],
        group8: BoundField[Group8T],
        group9: BoundField[Group9T],
        group10: BoundField[Group10T],
        group11: BoundField[Group11T],
        group12: BoundField[Group12T],
    ) -> GroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T, Group10T, Group11T, Group12T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase, Group6T: AttributeBase, Group7T: AttributeBase, Group8T: AttributeBase, Group9T: AttributeBase, Group10T: AttributeBase, Group11T: AttributeBase, Group12T: AttributeBase, Group13T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
        group6: BoundField[Group6T],
        group7: BoundField[Group7T],
        group8: BoundField[Group8T],
        group9: BoundField[Group9T],
        group10: BoundField[Group10T],
        group11: BoundField[Group11T],
        group12: BoundField[Group12T],
        group13: BoundField[Group13T],
    ) -> GroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T, Group10T, Group11T, Group12T, Group13T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase, Group6T: AttributeBase, Group7T: AttributeBase, Group8T: AttributeBase, Group9T: AttributeBase, Group10T: AttributeBase, Group11T: AttributeBase, Group12T: AttributeBase, Group13T: AttributeBase, Group14T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
        group6: BoundField[Group6T],
        group7: BoundField[Group7T],
        group8: BoundField[Group8T],
        group9: BoundField[Group9T],
        group10: BoundField[Group10T],
        group11: BoundField[Group11T],
        group12: BoundField[Group12T],
        group13: BoundField[Group13T],
        group14: BoundField[Group14T],
    ) -> GroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T, Group10T, Group11T, Group12T, Group13T, Group14T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase, Group6T: AttributeBase, Group7T: AttributeBase, Group8T: AttributeBase, Group9T: AttributeBase, Group10T: AttributeBase, Group11T: AttributeBase, Group12T: AttributeBase, Group13T: AttributeBase, Group14T: AttributeBase, Group15T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
        group6: BoundField[Group6T],
        group7: BoundField[Group7T],
        group8: BoundField[Group8T],
        group9: BoundField[Group9T],
        group10: BoundField[Group10T],
        group11: BoundField[Group11T],
        group12: BoundField[Group12T],
        group13: BoundField[Group13T],
        group14: BoundField[Group14T],
        group15: BoundField[Group15T],
    ) -> GroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T, Group10T, Group11T, Group12T, Group13T, Group14T, Group15T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase, Group6T: AttributeBase, Group7T: AttributeBase, Group8T: AttributeBase, Group9T: AttributeBase, Group10T: AttributeBase, Group11T: AttributeBase, Group12T: AttributeBase, Group13T: AttributeBase, Group14T: AttributeBase, Group15T: AttributeBase, Group16T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
        group6: BoundField[Group6T],
        group7: BoundField[Group7T],
        group8: BoundField[Group8T],
        group9: BoundField[Group9T],
        group10: BoundField[Group10T],
        group11: BoundField[Group11T],
        group12: BoundField[Group12T],
        group13: BoundField[Group13T],
        group14: BoundField[Group14T],
        group15: BoundField[Group15T],
        group16: BoundField[Group16T],
    ) -> GroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T, Group10T, Group11T, Group12T, Group13T, Group14T, Group15T, Group16T]]: ...

    # END GENERATED GROUP BY OVERLOADS

class GroupedQuery[GroupT]:
    def match(
        self,
        *bindings: _MatchBinding,
    ) -> GroupedQuery[GroupT]: ...
    def where(self, *predicates: Predicate) -> GroupedQuery[GroupT]: ...
    def allow_cross_join[LeftT: ModelBase, RightT: ModelBase](
        self,
        left: BoundVar[LeftT],
        right: BoundVar[RightT],
    ) -> GroupedQuery[GroupT]: ...
    # BEGIN GENERATED GROUPED AGGREGATE OVERLOADS
    @overload
    def aggregate[Output1T](
        self,
        term1: Aggregate[Output1T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T]], ...]: ...

    @overload
    def aggregate[Output1T, Output2T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T]], ...]: ...

    @overload
    def aggregate[Output1T, Output2T, Output3T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T]], ...]: ...

    @overload
    def aggregate[Output1T, Output2T, Output3T, Output4T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T]], ...]: ...

    @overload
    def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T]], ...]: ...

    @overload
    def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T]], ...]: ...

    @overload
    def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T]], ...]: ...

    @overload
    def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T]], ...]: ...

    @overload
    def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T]], ...]: ...

    @overload
    def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T]], ...]: ...

    @overload
    def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T]], ...]: ...

    @overload
    def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        term12: Aggregate[Output12T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T]], ...]: ...

    @overload
    def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        term12: Aggregate[Output12T],
        term13: Aggregate[Output13T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T]], ...]: ...

    @overload
    def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        term12: Aggregate[Output12T],
        term13: Aggregate[Output13T],
        term14: Aggregate[Output14T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T]], ...]: ...

    @overload
    def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        term12: Aggregate[Output12T],
        term13: Aggregate[Output13T],
        term14: Aggregate[Output14T],
        term15: Aggregate[Output15T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T]], ...]: ...

    @overload
    def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T, Output16T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        term12: Aggregate[Output12T],
        term13: Aggregate[Output13T],
        term14: Aggregate[Output14T],
        term15: Aggregate[Output15T],
        term16: Aggregate[Output16T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T, Output16T]], ...]: ...

    # END GENERATED GROUPED AGGREGATE OVERLOADS

class QuerySession:
    def __init__(
        self,
        projection: PyRuntimeProjection,
        connection: Database | TransactionContext,
    ) -> None: ...
    @overload
    def var[ModelT: ModelBase](
        self,
        model: type[ModelT],
        *,
        subtypes: Literal[False] = ...,
    ) -> BoundVar[ModelT]: ...
    @overload
    def var[ModelT: ModelBase](
        self,
        model: type[ModelT],
        *,
        subtypes: Literal[True],
    ) -> SubtypeBoundVar[ModelT]: ...
    def exact[ModelT: ModelBase](self, model: type[ModelT]) -> BoundVar[ModelT]: ...
    def subtypes[ModelT: ModelBase](self, model: type[ModelT]) -> SubtypeBoundVar[ModelT]: ...
    def reachable[
        SourceT: ModelBase,
        TargetT: ModelBase,
        RelationT: RelationBase,
    ](
        self,
        source: BoundVar[SourceT],
        target: BoundVar[TargetT],
        relation: type[RelationT],
        role_from: RoleToken[RelationT, SourceT, BoundVar[SourceT]],
        role_to: RoleToken[RelationT, TargetT, BoundVar[TargetT]],
        *,
        min_depth: int,
        max_depth: int,
    ) -> Predicate: ...
    def query_as[DeclaredRowT](
        self,
        declaration: type[DeclaredRowT],
        /,
        **selections: Selection[ModelBase | tuple[ModelBase, ...]],
    ) -> Query[DeclaredRowT]: ...
    # BEGIN GENERATED QUERY OVERLOADS
    @overload
    def query[T1](
        self,
        selection1: Selection[T1],
        /,
    ) -> Query[T1]: ...

    @overload
    def query[T1, T2](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        /,
    ) -> Query[T1, T2]: ...

    @overload
    def query[T1, T2, T3](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        /,
    ) -> Query[T1, T2, T3]: ...

    @overload
    def query[T1, T2, T3, T4](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        /,
    ) -> Query[T1, T2, T3, T4]: ...

    @overload
    def query[T1, T2, T3, T4, T5](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        /,
    ) -> Query[T1, T2, T3, T4, T5]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        /,
    ) -> Query[T1, T2, T3, T4, T5, T6]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        /,
    ) -> Query[T1, T2, T3, T4, T5, T6, T7]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        /,
    ) -> Query[T1, T2, T3, T4, T5, T6, T7, T8]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        /,
    ) -> Query[T1, T2, T3, T4, T5, T6, T7, T8, T9]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        selection10: Selection[T10],
        /,
    ) -> Query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        selection10: Selection[T10],
        selection11: Selection[T11],
        /,
    ) -> Query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        selection10: Selection[T10],
        selection11: Selection[T11],
        selection12: Selection[T12],
        /,
    ) -> Query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        selection10: Selection[T10],
        selection11: Selection[T11],
        selection12: Selection[T12],
        selection13: Selection[T13],
        /,
    ) -> Query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        selection10: Selection[T10],
        selection11: Selection[T11],
        selection12: Selection[T12],
        selection13: Selection[T13],
        selection14: Selection[T14],
        /,
    ) -> Query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        selection10: Selection[T10],
        selection11: Selection[T11],
        selection12: Selection[T12],
        selection13: Selection[T13],
        selection14: Selection[T14],
        selection15: Selection[T15],
        /,
    ) -> Query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        selection10: Selection[T10],
        selection11: Selection[T11],
        selection12: Selection[T12],
        selection13: Selection[T13],
        selection14: Selection[T14],
        selection15: Selection[T15],
        selection16: Selection[T16],
        /,
    ) -> Query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16]: ...

    # END GENERATED QUERY OVERLOADS

class RemoteQueryLimits:
    def __init__(
        self,
        *,
        max_items: int,
        max_bytes: int,
        max_collection_members: int,
        max_graph_nodes: int,
        max_attribute_values: int,
        max_role_players: int,
        deadline_ms: int | None = ...,
    ) -> None: ...
    @property
    def max_items(self) -> int: ...
    @property
    def max_bytes(self) -> int: ...
    @property
    def max_collection_members(self) -> int: ...
    @property
    def max_graph_nodes(self) -> int: ...
    @property
    def max_attribute_values(self) -> int: ...
    @property
    def max_role_players(self) -> int: ...
    @property
    def deadline_ms(self) -> int | None: ...

class RemoteQuery[*Slots]:
    def match(
        self,
        *bindings: _MatchBinding,
    ) -> RemoteQuery[*Slots]: ...
    def where(self, *predicates: Predicate) -> RemoteQuery[*Slots]: ...
    def allow_cross_join[LeftT: ModelBase, RightT: ModelBase](
        self,
        left: BoundVar[LeftT],
        right: BoundVar[RightT],
    ) -> RemoteQuery[*Slots]: ...
    @overload
    async def one[SlotT](self: RemoteQuery[SlotT]) -> SlotT: ...
    @overload
    async def one[Slot1T, Slot2T, *RestT](
        self: RemoteQuery[Slot1T, Slot2T, *RestT],
    ) -> tuple[Slot1T, Slot2T, *RestT]: ...
    @overload
    async def first[SlotT](
        self: RemoteQuery[SlotT],
        *,
        order_by: Iterable[QueryOrder] = ...,
    ) -> SlotT | None: ...
    @overload
    async def first[Slot1T, Slot2T, *RestT](
        self: RemoteQuery[Slot1T, Slot2T, *RestT],
        *,
        order_by: Iterable[QueryOrder] = ...,
    ) -> tuple[Slot1T, Slot2T, *RestT] | None: ...
    @overload
    async def rows[SlotT](
        self: RemoteQuery[SlotT],
        *,
        limit: int,
        offset: int = ...,
        order_by: Iterable[QueryOrder] = ...,
    ) -> list[SlotT]: ...
    @overload
    async def rows[Slot1T, Slot2T, *RestT](
        self: RemoteQuery[Slot1T, Slot2T, *RestT],
        *,
        limit: int,
        offset: int = ...,
        order_by: Iterable[QueryOrder] = ...,
    ) -> list[tuple[Slot1T, Slot2T, *RestT]]: ...
    # BEGIN GENERATED REMOTE PAGE OVERLOADS
    @overload
    async def page_by[SlotT, RootT: ModelBase](
        self: RemoteQuery[SlotT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[SlotT]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase](
        self: RemoteQuery[RootT, tuple[Collected2T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected12T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected13T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected14T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected15T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected16T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT, tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT, tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], RootT]]: ...

    # END GENERATED REMOTE PAGE OVERLOADS
    async def count_by[RootT: ModelBase](self, root: BoundVar[RootT]) -> int: ...
    async def exists_by[RootT: ModelBase](self, root: BoundVar[RootT]) -> bool: ...
    # BEGIN GENERATED REMOTE AGGREGATE OVERLOADS
    @overload
    async def aggregate[RootT: ModelBase, Output1T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        /,
    ) -> tuple[Output1T]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        /,
    ) -> tuple[Output1T, Output2T]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        term12: Aggregate[Output12T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        term12: Aggregate[Output12T],
        term13: Aggregate[Output13T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        term12: Aggregate[Output12T],
        term13: Aggregate[Output13T],
        term14: Aggregate[Output14T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        term12: Aggregate[Output12T],
        term13: Aggregate[Output13T],
        term14: Aggregate[Output14T],
        term15: Aggregate[Output15T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T, Output16T](
        self,
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        term12: Aggregate[Output12T],
        term13: Aggregate[Output13T],
        term14: Aggregate[Output14T],
        term15: Aggregate[Output15T],
        term16: Aggregate[Output16T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T, Output16T]: ...

    # END GENERATED REMOTE AGGREGATE OVERLOADS
    # BEGIN GENERATED REMOTE GROUP BY OVERLOADS
    @overload
    def group_by[RootT: ModelBase, GroupT: ModelBase](
        self,
        root: BoundVar[RootT],
        group: BoundVar[GroupT],
    ) -> RemoteGroupedQuery[GroupT]: ...

    @overload
    def group_by[RootT: ModelBase, GroupT: AttributeBase](
        self,
        root: BoundVar[RootT],
        group: BoundField[GroupT],
    ) -> RemoteGroupedQuery[GroupT]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
    ) -> RemoteGroupedQuery[tuple[Group1T, Group2T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
    ) -> RemoteGroupedQuery[tuple[Group1T, Group2T, Group3T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
    ) -> RemoteGroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
    ) -> RemoteGroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase, Group6T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
        group6: BoundField[Group6T],
    ) -> RemoteGroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase, Group6T: AttributeBase, Group7T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
        group6: BoundField[Group6T],
        group7: BoundField[Group7T],
    ) -> RemoteGroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase, Group6T: AttributeBase, Group7T: AttributeBase, Group8T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
        group6: BoundField[Group6T],
        group7: BoundField[Group7T],
        group8: BoundField[Group8T],
    ) -> RemoteGroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase, Group6T: AttributeBase, Group7T: AttributeBase, Group8T: AttributeBase, Group9T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
        group6: BoundField[Group6T],
        group7: BoundField[Group7T],
        group8: BoundField[Group8T],
        group9: BoundField[Group9T],
    ) -> RemoteGroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase, Group6T: AttributeBase, Group7T: AttributeBase, Group8T: AttributeBase, Group9T: AttributeBase, Group10T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
        group6: BoundField[Group6T],
        group7: BoundField[Group7T],
        group8: BoundField[Group8T],
        group9: BoundField[Group9T],
        group10: BoundField[Group10T],
    ) -> RemoteGroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T, Group10T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase, Group6T: AttributeBase, Group7T: AttributeBase, Group8T: AttributeBase, Group9T: AttributeBase, Group10T: AttributeBase, Group11T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
        group6: BoundField[Group6T],
        group7: BoundField[Group7T],
        group8: BoundField[Group8T],
        group9: BoundField[Group9T],
        group10: BoundField[Group10T],
        group11: BoundField[Group11T],
    ) -> RemoteGroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T, Group10T, Group11T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase, Group6T: AttributeBase, Group7T: AttributeBase, Group8T: AttributeBase, Group9T: AttributeBase, Group10T: AttributeBase, Group11T: AttributeBase, Group12T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
        group6: BoundField[Group6T],
        group7: BoundField[Group7T],
        group8: BoundField[Group8T],
        group9: BoundField[Group9T],
        group10: BoundField[Group10T],
        group11: BoundField[Group11T],
        group12: BoundField[Group12T],
    ) -> RemoteGroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T, Group10T, Group11T, Group12T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase, Group6T: AttributeBase, Group7T: AttributeBase, Group8T: AttributeBase, Group9T: AttributeBase, Group10T: AttributeBase, Group11T: AttributeBase, Group12T: AttributeBase, Group13T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
        group6: BoundField[Group6T],
        group7: BoundField[Group7T],
        group8: BoundField[Group8T],
        group9: BoundField[Group9T],
        group10: BoundField[Group10T],
        group11: BoundField[Group11T],
        group12: BoundField[Group12T],
        group13: BoundField[Group13T],
    ) -> RemoteGroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T, Group10T, Group11T, Group12T, Group13T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase, Group6T: AttributeBase, Group7T: AttributeBase, Group8T: AttributeBase, Group9T: AttributeBase, Group10T: AttributeBase, Group11T: AttributeBase, Group12T: AttributeBase, Group13T: AttributeBase, Group14T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
        group6: BoundField[Group6T],
        group7: BoundField[Group7T],
        group8: BoundField[Group8T],
        group9: BoundField[Group9T],
        group10: BoundField[Group10T],
        group11: BoundField[Group11T],
        group12: BoundField[Group12T],
        group13: BoundField[Group13T],
        group14: BoundField[Group14T],
    ) -> RemoteGroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T, Group10T, Group11T, Group12T, Group13T, Group14T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase, Group6T: AttributeBase, Group7T: AttributeBase, Group8T: AttributeBase, Group9T: AttributeBase, Group10T: AttributeBase, Group11T: AttributeBase, Group12T: AttributeBase, Group13T: AttributeBase, Group14T: AttributeBase, Group15T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
        group6: BoundField[Group6T],
        group7: BoundField[Group7T],
        group8: BoundField[Group8T],
        group9: BoundField[Group9T],
        group10: BoundField[Group10T],
        group11: BoundField[Group11T],
        group12: BoundField[Group12T],
        group13: BoundField[Group13T],
        group14: BoundField[Group14T],
        group15: BoundField[Group15T],
    ) -> RemoteGroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T, Group10T, Group11T, Group12T, Group13T, Group14T, Group15T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase, Group6T: AttributeBase, Group7T: AttributeBase, Group8T: AttributeBase, Group9T: AttributeBase, Group10T: AttributeBase, Group11T: AttributeBase, Group12T: AttributeBase, Group13T: AttributeBase, Group14T: AttributeBase, Group15T: AttributeBase, Group16T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
        group6: BoundField[Group6T],
        group7: BoundField[Group7T],
        group8: BoundField[Group8T],
        group9: BoundField[Group9T],
        group10: BoundField[Group10T],
        group11: BoundField[Group11T],
        group12: BoundField[Group12T],
        group13: BoundField[Group13T],
        group14: BoundField[Group14T],
        group15: BoundField[Group15T],
        group16: BoundField[Group16T],
    ) -> RemoteGroupedQuery[tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T, Group10T, Group11T, Group12T, Group13T, Group14T, Group15T, Group16T]]: ...

    # END GENERATED REMOTE GROUP BY OVERLOADS

class RemoteGroupedQuery[GroupT]:
    def match(
        self,
        *bindings: _MatchBinding,
    ) -> RemoteGroupedQuery[GroupT]: ...
    def where(self, *predicates: Predicate) -> RemoteGroupedQuery[GroupT]: ...
    def allow_cross_join[LeftT: ModelBase, RightT: ModelBase](
        self,
        left: BoundVar[LeftT],
        right: BoundVar[RightT],
    ) -> RemoteGroupedQuery[GroupT]: ...
    # BEGIN GENERATED REMOTE GROUPED AGGREGATE OVERLOADS
    @overload
    async def aggregate[Output1T](
        self,
        term1: Aggregate[Output1T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T]], ...]: ...

    @overload
    async def aggregate[Output1T, Output2T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T]], ...]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T]], ...]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T]], ...]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T]], ...]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T]], ...]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T]], ...]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T]], ...]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T]], ...]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T]], ...]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T]], ...]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        term12: Aggregate[Output12T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T]], ...]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        term12: Aggregate[Output12T],
        term13: Aggregate[Output13T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T]], ...]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        term12: Aggregate[Output12T],
        term13: Aggregate[Output13T],
        term14: Aggregate[Output14T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T]], ...]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        term12: Aggregate[Output12T],
        term13: Aggregate[Output13T],
        term14: Aggregate[Output14T],
        term15: Aggregate[Output15T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T]], ...]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T, Output16T](
        self,
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        term9: Aggregate[Output9T],
        term10: Aggregate[Output10T],
        term11: Aggregate[Output11T],
        term12: Aggregate[Output12T],
        term13: Aggregate[Output13T],
        term14: Aggregate[Output14T],
        term15: Aggregate[Output15T],
        term16: Aggregate[Output16T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T, Output16T]], ...]: ...

    # END GENERATED REMOTE GROUPED AGGREGATE OVERLOADS

class RemoteQuerySession:
    def __init__(
        self,
        authority: QueryV2Authority,
        advertisement: bytes,
        exchange: Callable[[bytes], Awaitable[bytes]],
        limits: RemoteQueryLimits,
    ) -> None: ...
    @overload
    def var[ModelT: ModelBase](
        self,
        model: type[ModelT],
        *,
        subtypes: Literal[False] = ...,
    ) -> BoundVar[ModelT]: ...
    @overload
    def var[ModelT: ModelBase](
        self,
        model: type[ModelT],
        *,
        subtypes: Literal[True],
    ) -> SubtypeBoundVar[ModelT]: ...
    def exact[ModelT: ModelBase](self, model: type[ModelT]) -> BoundVar[ModelT]: ...
    def subtypes[ModelT: ModelBase](self, model: type[ModelT]) -> SubtypeBoundVar[ModelT]: ...
    def reachable[
        SourceT: ModelBase,
        TargetT: ModelBase,
        RelationT: RelationBase,
    ](
        self,
        source: BoundVar[SourceT],
        target: BoundVar[TargetT],
        relation: type[RelationT],
        role_from: RoleToken[RelationT, SourceT, BoundVar[SourceT]],
        role_to: RoleToken[RelationT, TargetT, BoundVar[TargetT]],
        *,
        min_depth: int,
        max_depth: int,
    ) -> Predicate: ...
    def query_as[DeclaredRowT](
        self,
        declaration: type[DeclaredRowT],
        /,
        **selections: Selection[ModelBase | tuple[ModelBase, ...]],
    ) -> RemoteQuery[DeclaredRowT]: ...
    # BEGIN GENERATED REMOTE QUERY OVERLOADS
    @overload
    def query[T1](
        self,
        selection1: Selection[T1],
        /,
    ) -> RemoteQuery[T1]: ...

    @overload
    def query[T1, T2](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        /,
    ) -> RemoteQuery[T1, T2]: ...

    @overload
    def query[T1, T2, T3](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        /,
    ) -> RemoteQuery[T1, T2, T3]: ...

    @overload
    def query[T1, T2, T3, T4](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        /,
    ) -> RemoteQuery[T1, T2, T3, T4]: ...

    @overload
    def query[T1, T2, T3, T4, T5](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        /,
    ) -> RemoteQuery[T1, T2, T3, T4, T5]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        /,
    ) -> RemoteQuery[T1, T2, T3, T4, T5, T6]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        /,
    ) -> RemoteQuery[T1, T2, T3, T4, T5, T6, T7]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        /,
    ) -> RemoteQuery[T1, T2, T3, T4, T5, T6, T7, T8]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        /,
    ) -> RemoteQuery[T1, T2, T3, T4, T5, T6, T7, T8, T9]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        selection10: Selection[T10],
        /,
    ) -> RemoteQuery[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        selection10: Selection[T10],
        selection11: Selection[T11],
        /,
    ) -> RemoteQuery[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        selection10: Selection[T10],
        selection11: Selection[T11],
        selection12: Selection[T12],
        /,
    ) -> RemoteQuery[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        selection10: Selection[T10],
        selection11: Selection[T11],
        selection12: Selection[T12],
        selection13: Selection[T13],
        /,
    ) -> RemoteQuery[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        selection10: Selection[T10],
        selection11: Selection[T11],
        selection12: Selection[T12],
        selection13: Selection[T13],
        selection14: Selection[T14],
        /,
    ) -> RemoteQuery[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        selection10: Selection[T10],
        selection11: Selection[T11],
        selection12: Selection[T12],
        selection13: Selection[T13],
        selection14: Selection[T14],
        selection15: Selection[T15],
        /,
    ) -> RemoteQuery[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15]: ...

    @overload
    def query[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        selection6: Selection[T6],
        selection7: Selection[T7],
        selection8: Selection[T8],
        selection9: Selection[T9],
        selection10: Selection[T10],
        selection11: Selection[T11],
        selection12: Selection[T12],
        selection13: Selection[T13],
        selection14: Selection[T14],
        selection15: Selection[T15],
        selection16: Selection[T16],
        /,
    ) -> RemoteQuery[T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16]: ...

    # END GENERATED REMOTE QUERY OVERLOADS
