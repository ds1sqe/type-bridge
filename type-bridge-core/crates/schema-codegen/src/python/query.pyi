from collections.abc import Awaitable, Callable, Iterable, Sequence
from os import PathLike
from typing import Generic, Literal, overload

from type_bridge_core import (
    Iid,
    Pending,
    PyRuntimeProjection,
)
from type_bridge_core import (
    ProxyConnection as ProxyConnection,
)
from type_bridge_core import (
    QueryCancellation as QueryCancellation,
)
from type_bridge_core import (
    QueryExecutionResourceLimits as QueryExecutionResourceLimits,
)
from type_bridge_core import (
    QueryLimits as QueryLimits,
)
from type_bridge_core import Transaction as StagedTransaction
from typing_extensions import TypeVar

from type_bridge.session import Database, TransactionContext

from ._runtime import (
    AttributeBase,
    DoubleAttributeBase,
    FieldToken,
    FunctionRef,
    LongAttributeBase,
    ModelBase,
    RelationBase,
    RoleToken,
)

_Mode = TypeVar("_Mode", bound=bool, covariant=True, default=Literal[False])

def install_projection(projection: PyRuntimeProjection) -> None: ...

class Predicate:
    def and_(self, other: Predicate) -> Predicate: ...
    def or_(self, other: Predicate) -> Predicate: ...
    def not_(self) -> Predicate: ...
    def __and__(self, other: Predicate) -> Predicate: ...
    def __or__(self, other: Predicate) -> Predicate: ...
    def __invert__(self) -> Predicate: ...

class FunctionInput[DomainT]:
    def native_argument(self) -> object: ...

class FunctionCall[DomainT]:
    def eq_field[ValueT: AttributeBase](self, field: BoundField[ValueT]) -> Predicate: ...
    def ne_field[ValueT: AttributeBase](self, field: BoundField[ValueT]) -> Predicate: ...
    def lt_field[ValueT: AttributeBase](self, field: BoundField[ValueT]) -> Predicate: ...
    def le_field[ValueT: AttributeBase](self, field: BoundField[ValueT]) -> Predicate: ...
    def gt_field[ValueT: AttributeBase](self, field: BoundField[ValueT]) -> Predicate: ...
    def ge_field[ValueT: AttributeBase](self, field: BoundField[ValueT]) -> Predicate: ...
    def eq_value(self, value: FunctionInput[DomainT]) -> Predicate: ...
    def ne_value(self, value: FunctionInput[DomainT]) -> Predicate: ...
    def lt_value(self, value: FunctionInput[DomainT]) -> Predicate: ...
    def le_value(self, value: FunctionInput[DomainT]) -> Predicate: ...
    def gt_value(self, value: FunctionInput[DomainT]) -> Predicate: ...
    def ge_value(self, value: FunctionInput[DomainT]) -> Predicate: ...
    def eq_call(self, other: FunctionCall[DomainT]) -> Predicate: ...
    def ne_call(self, other: FunctionCall[DomainT]) -> Predicate: ...
    def lt_call(self, other: FunctionCall[DomainT]) -> Predicate: ...
    def le_call(self, other: FunctionCall[DomainT]) -> Predicate: ...
    def gt_call(self, other: FunctionCall[DomainT]) -> Predicate: ...
    def ge_call(self, other: FunctionCall[DomainT]) -> Predicate: ...
    def native_argument(self) -> object: ...

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
    def eq_field(self, field: BoundField[AttributeBase]) -> Predicate: ...
    def ne(self, value: AttributeT | BoundField[AttributeT]) -> Predicate: ...
    def ne_field(self, field: BoundField[AttributeBase]) -> Predicate: ...
    def lt(self, value: AttributeT | BoundField[AttributeT]) -> Predicate: ...
    def lt_field(self, field: BoundField[AttributeBase]) -> Predicate: ...
    def le(self, value: AttributeT | BoundField[AttributeT]) -> Predicate: ...
    def le_field(self, field: BoundField[AttributeBase]) -> Predicate: ...
    def gt(self, value: AttributeT | BoundField[AttributeT]) -> Predicate: ...
    def gt_field(self, field: BoundField[AttributeBase]) -> Predicate: ...
    def ge(self, value: AttributeT | BoundField[AttributeT]) -> Predicate: ...
    def ge_field(self, field: BoundField[AttributeBase]) -> Predicate: ...
    def contains(self, value: AttributeT) -> Predicate: ...
    def starts_with(self, value: AttributeT) -> Predicate: ...
    def ends_with(self, value: AttributeT) -> Predicate: ...
    def like(self, value: AttributeT) -> Predicate: ...
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
    def links(self, player: CompatibleBindingT) -> Predicate: ...

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
    def iid(self, iid: Iid) -> Predicate: ...
    def iid_in(self, iids: Iterable[Iid]) -> Predicate: ...
    def collect(self) -> Collected[ModelT]: ...

class SubtypeBoundVar[ModelT: ModelBase](BoundVar[ModelT]): ...

class Collected[ModelT: ModelBase](Selection[tuple[ModelT, ...]]):
    def distinct(self, enabled: bool = ...) -> Collected[ModelT]: ...
    def sort(self, order: QueryOrder) -> Collected[ModelT]: ...

type Query[*Slots] = _Query[Literal[False], *Slots]

class _Query[ModeT: bool, *Slots]:
    @property
    def is_closed(self) -> bool: ...
    def close(self) -> None: ...
    def clone(self) -> _Query[ModeT, *Slots]: ...
    def match(self, *bindings: _MatchBinding) -> _Query[ModeT, *Slots]: ...
    def where(self, *predicates: Predicate) -> _Query[ModeT, *Slots]: ...
    def allow_cross_join[LeftT: ModelBase, RightT: ModelBase](
        self,
        left: BoundVar[LeftT],
        right: BoundVar[RightT],
    ) -> _Query[ModeT, *Slots]: ...
    @overload
    async def one[SlotT](self: _Query[Literal[False], SlotT]) -> SlotT: ...
    @overload
    async def one[SlotT](self: _Query[Literal[True], SlotT]) -> Pending[SlotT]: ...
    @overload
    async def one[Slot1T, Slot2T, *RestT](
        self: _Query[Literal[False], Slot1T, Slot2T, *RestT],
    ) -> tuple[Slot1T, Slot2T, *RestT]: ...
    @overload
    async def one[Slot1T, Slot2T, *RestT](
        self: _Query[Literal[True], Slot1T, Slot2T, *RestT],
    ) -> Pending[tuple[Slot1T, Slot2T, *RestT]]: ...
    @overload
    async def first[SlotT](
        self: _Query[Literal[False], SlotT],
        *,
        sort: Iterable[QueryOrder] = ...,
    ) -> SlotT | None: ...
    @overload
    async def first[SlotT](
        self: _Query[Literal[True], SlotT],
        *,
        sort: Iterable[QueryOrder] = ...,
    ) -> Pending[SlotT | None]: ...
    @overload
    async def first[Slot1T, Slot2T, *RestT](
        self: _Query[Literal[False], Slot1T, Slot2T, *RestT],
        *,
        sort: Iterable[QueryOrder] = ...,
    ) -> tuple[Slot1T, Slot2T, *RestT] | None: ...
    @overload
    async def first[Slot1T, Slot2T, *RestT](
        self: _Query[Literal[True], Slot1T, Slot2T, *RestT],
        *,
        sort: Iterable[QueryOrder] = ...,
    ) -> Pending[tuple[Slot1T, Slot2T, *RestT] | None]: ...
    @overload
    async def rows[SlotT](
        self: _Query[Literal[False], SlotT],
        *,
        limit: int,
        offset: int = ...,
        sort: Iterable[QueryOrder] = ...,
    ) -> list[SlotT]: ...
    @overload
    async def rows[SlotT](
        self: _Query[Literal[True], SlotT],
        *,
        limit: int,
        offset: int = ...,
        sort: Iterable[QueryOrder] = ...,
    ) -> Pending[list[SlotT]]: ...
    @overload
    async def rows[Slot1T, Slot2T, *RestT](
        self: _Query[Literal[False], Slot1T, Slot2T, *RestT],
        *,
        limit: int,
        offset: int = ...,
        sort: Iterable[QueryOrder] = ...,
    ) -> list[tuple[Slot1T, Slot2T, *RestT]]: ...
    @overload
    async def rows[Slot1T, Slot2T, *RestT](
        self: _Query[Literal[True], Slot1T, Slot2T, *RestT],
        *,
        limit: int,
        offset: int = ...,
        sort: Iterable[QueryOrder] = ...,
    ) -> Pending[list[tuple[Slot1T, Slot2T, *RestT]]]: ...
    # BEGIN GENERATED PAGE OVERLOADS
    @overload
    async def page_by[SlotT, RootT: ModelBase](
        self: _Query[Literal[False], SlotT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[SlotT]: ...
    @overload
    async def page_by[SlotT, RootT: ModelBase](
        self: _Query[Literal[True], SlotT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[SlotT]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase](
        self: _Query[Literal[False], RootT, tuple[Collected2T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase](
        self: _Query[Literal[True], RootT, tuple[Collected2T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[RootT, tuple[Collected2T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], RootT]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase](
        self: _Query[Literal[False], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase](
        self: _Query[Literal[True], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase](
        self: _Query[Literal[False], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase](
        self: _Query[Literal[True], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase](
        self: _Query[Literal[False], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase](
        self: _Query[Literal[True], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase](
        self: _Query[Literal[False], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase](
        self: _Query[Literal[True], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: _Query[Literal[False], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: _Query[Literal[True], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: _Query[Literal[False], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: _Query[Literal[True], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: _Query[Literal[False], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: _Query[Literal[True], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: _Query[Literal[False], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: _Query[Literal[True], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: _Query[Literal[False], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: _Query[Literal[True], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[False], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[True], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[False], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[True], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[False], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[True], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[False], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[True], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[False], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[True], RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected15T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...], tuple[Collected16T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT, tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT, tuple[Collected16T, ...]]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected16T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT, tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT, tuple[Collected16T, ...]]]]: ...

    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[False], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], RootT]]: ...
    @overload
    async def page_by[RootT: ModelBase, Collected1T: ModelBase, Collected2T: ModelBase, Collected3T: ModelBase, Collected4T: ModelBase, Collected5T: ModelBase, Collected6T: ModelBase, Collected7T: ModelBase, Collected8T: ModelBase, Collected9T: ModelBase, Collected10T: ModelBase, Collected11T: ModelBase, Collected12T: ModelBase, Collected13T: ModelBase, Collected14T: ModelBase, Collected15T: ModelBase](
        self: _Query[Literal[True], tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        sort: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Pending[Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], RootT]]]: ...

    # END GENERATED PAGE OVERLOADS
    @overload
    async def count_by[RootT: ModelBase](self: _Query[Literal[False], *Slots], root: BoundVar[RootT]) -> int: ...
    @overload
    async def count_by[RootT: ModelBase](self: _Query[Literal[True], *Slots], root: BoundVar[RootT]) -> Pending[int]: ...
    @overload
    async def exists_by[RootT: ModelBase](self: _Query[Literal[False], *Slots], root: BoundVar[RootT]) -> bool: ...
    @overload
    async def exists_by[RootT: ModelBase](self: _Query[Literal[True], *Slots], root: BoundVar[RootT]) -> Pending[bool]: ...
    # BEGIN GENERATED AGGREGATE OVERLOADS
    @overload
    async def aggregate[RootT: ModelBase, Output1T](
        self: _Query[Literal[False], *Slots],
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        /,
    ) -> tuple[Output1T]: ...
    @overload
    async def aggregate[RootT: ModelBase, Output1T](
        self: _Query[Literal[True], *Slots],
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        /,
    ) -> Pending[tuple[Output1T]]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T](
        self: _Query[Literal[False], *Slots],
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        /,
    ) -> tuple[Output1T, Output2T]: ...
    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T](
        self: _Query[Literal[True], *Slots],
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        /,
    ) -> Pending[tuple[Output1T, Output2T]]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T](
        self: _Query[Literal[False], *Slots],
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T]: ...
    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T](
        self: _Query[Literal[True], *Slots],
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        /,
    ) -> Pending[tuple[Output1T, Output2T, Output3T]]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T](
        self: _Query[Literal[False], *Slots],
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T]: ...
    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T](
        self: _Query[Literal[True], *Slots],
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        /,
    ) -> Pending[tuple[Output1T, Output2T, Output3T, Output4T]]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T](
        self: _Query[Literal[False], *Slots],
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        /,
    ) -> tuple[Output1T, Output2T, Output3T, Output4T, Output5T]: ...
    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T](
        self: _Query[Literal[True], *Slots],
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        /,
    ) -> Pending[tuple[Output1T, Output2T, Output3T, Output4T, Output5T]]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T](
        self: _Query[Literal[False], *Slots],
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
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T](
        self: _Query[Literal[True], *Slots],
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        /,
    ) -> Pending[tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T]]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T](
        self: _Query[Literal[False], *Slots],
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
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T](
        self: _Query[Literal[True], *Slots],
        root: BoundVar[RootT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        /,
    ) -> Pending[tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T]]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T](
        self: _Query[Literal[False], *Slots],
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
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T](
        self: _Query[Literal[True], *Slots],
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
    ) -> Pending[tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T]]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T](
        self: _Query[Literal[False], *Slots],
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
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T](
        self: _Query[Literal[True], *Slots],
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
    ) -> Pending[tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T]]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T](
        self: _Query[Literal[False], *Slots],
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
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T](
        self: _Query[Literal[True], *Slots],
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
    ) -> Pending[tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T]]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T](
        self: _Query[Literal[False], *Slots],
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
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T](
        self: _Query[Literal[True], *Slots],
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
    ) -> Pending[tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T]]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T](
        self: _Query[Literal[False], *Slots],
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
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T](
        self: _Query[Literal[True], *Slots],
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
    ) -> Pending[tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T]]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T](
        self: _Query[Literal[False], *Slots],
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
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T](
        self: _Query[Literal[True], *Slots],
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
    ) -> Pending[tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T]]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T](
        self: _Query[Literal[False], *Slots],
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
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T](
        self: _Query[Literal[True], *Slots],
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
    ) -> Pending[tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T]]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T](
        self: _Query[Literal[False], *Slots],
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
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T](
        self: _Query[Literal[True], *Slots],
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
    ) -> Pending[tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T]]: ...

    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T, Output16T](
        self: _Query[Literal[False], *Slots],
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
    @overload
    async def aggregate[RootT: ModelBase, Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T, Output16T](
        self: _Query[Literal[True], *Slots],
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
    ) -> Pending[tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T, Output16T]]: ...

    # END GENERATED AGGREGATE OVERLOADS
    # BEGIN GENERATED GROUP BY OVERLOADS
    @overload
    def group_by[RootT: ModelBase, GroupT: ModelBase](
        self,
        root: BoundVar[RootT],
        group: BoundVar[GroupT],
    ) -> _GroupedQuery[ModeT, GroupT]: ...

    @overload
    def group_by[RootT: ModelBase, GroupT: AttributeBase](
        self,
        root: BoundVar[RootT],
        group: BoundField[GroupT],
    ) -> _GroupedQuery[ModeT, GroupT]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
    ) -> _GroupedQuery[ModeT, tuple[Group1T, Group2T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
    ) -> _GroupedQuery[ModeT, tuple[Group1T, Group2T, Group3T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
    ) -> _GroupedQuery[ModeT, tuple[Group1T, Group2T, Group3T, Group4T]]: ...

    @overload
    def group_by[RootT: ModelBase, Group1T: AttributeBase, Group2T: AttributeBase, Group3T: AttributeBase, Group4T: AttributeBase, Group5T: AttributeBase](
        self,
        root: BoundVar[RootT],
        group1: BoundField[Group1T],
        group2: BoundField[Group2T],
        group3: BoundField[Group3T],
        group4: BoundField[Group4T],
        group5: BoundField[Group5T],
    ) -> _GroupedQuery[ModeT, tuple[Group1T, Group2T, Group3T, Group4T, Group5T]]: ...

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
    ) -> _GroupedQuery[ModeT, tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T]]: ...

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
    ) -> _GroupedQuery[ModeT, tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T]]: ...

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
    ) -> _GroupedQuery[ModeT, tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T]]: ...

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
    ) -> _GroupedQuery[ModeT, tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T]]: ...

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
    ) -> _GroupedQuery[ModeT, tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T, Group10T]]: ...

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
    ) -> _GroupedQuery[ModeT, tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T, Group10T, Group11T]]: ...

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
    ) -> _GroupedQuery[ModeT, tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T, Group10T, Group11T, Group12T]]: ...

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
    ) -> _GroupedQuery[ModeT, tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T, Group10T, Group11T, Group12T, Group13T]]: ...

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
    ) -> _GroupedQuery[ModeT, tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T, Group10T, Group11T, Group12T, Group13T, Group14T]]: ...

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
    ) -> _GroupedQuery[ModeT, tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T, Group10T, Group11T, Group12T, Group13T, Group14T, Group15T]]: ...

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
    ) -> _GroupedQuery[ModeT, tuple[Group1T, Group2T, Group3T, Group4T, Group5T, Group6T, Group7T, Group8T, Group9T, Group10T, Group11T, Group12T, Group13T, Group14T, Group15T, Group16T]]: ...

    # END GENERATED GROUP BY OVERLOADS

type GroupedQuery[GroupT] = _GroupedQuery[Literal[False], GroupT]

class _GroupedQuery[ModeT: bool, GroupT]:
    @property
    def is_closed(self) -> bool: ...
    def close(self) -> None: ...
    def match(
        self,
        *bindings: _MatchBinding,
    ) -> _GroupedQuery[ModeT, GroupT]: ...
    def where(self, *predicates: Predicate) -> _GroupedQuery[ModeT, GroupT]: ...
    def allow_cross_join[LeftT: ModelBase, RightT: ModelBase](
        self,
        left: BoundVar[LeftT],
        right: BoundVar[RightT],
    ) -> _GroupedQuery[ModeT, GroupT]: ...
    # BEGIN GENERATED GROUPED AGGREGATE OVERLOADS
    @overload
    async def aggregate[Output1T](
        self: _GroupedQuery[Literal[False], GroupT],
        term1: Aggregate[Output1T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T]], ...]: ...
    @overload
    async def aggregate[Output1T](
        self: _GroupedQuery[Literal[True], GroupT],
        term1: Aggregate[Output1T],
        /,
    ) -> Pending[tuple[tuple[GroupT, tuple[Output1T]], ...]]: ...

    @overload
    async def aggregate[Output1T, Output2T](
        self: _GroupedQuery[Literal[False], GroupT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T]], ...]: ...
    @overload
    async def aggregate[Output1T, Output2T](
        self: _GroupedQuery[Literal[True], GroupT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        /,
    ) -> Pending[tuple[tuple[GroupT, tuple[Output1T, Output2T]], ...]]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T](
        self: _GroupedQuery[Literal[False], GroupT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T]], ...]: ...
    @overload
    async def aggregate[Output1T, Output2T, Output3T](
        self: _GroupedQuery[Literal[True], GroupT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        /,
    ) -> Pending[tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T]], ...]]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T](
        self: _GroupedQuery[Literal[False], GroupT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T]], ...]: ...
    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T](
        self: _GroupedQuery[Literal[True], GroupT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        /,
    ) -> Pending[tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T]], ...]]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T](
        self: _GroupedQuery[Literal[False], GroupT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T]], ...]: ...
    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T](
        self: _GroupedQuery[Literal[True], GroupT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        /,
    ) -> Pending[tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T]], ...]]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T](
        self: _GroupedQuery[Literal[False], GroupT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        /,
    ) -> tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T]], ...]: ...
    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T](
        self: _GroupedQuery[Literal[True], GroupT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        /,
    ) -> Pending[tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T]], ...]]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T](
        self: _GroupedQuery[Literal[False], GroupT],
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
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T](
        self: _GroupedQuery[Literal[True], GroupT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        /,
    ) -> Pending[tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T]], ...]]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T](
        self: _GroupedQuery[Literal[False], GroupT],
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
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T](
        self: _GroupedQuery[Literal[True], GroupT],
        term1: Aggregate[Output1T],
        term2: Aggregate[Output2T],
        term3: Aggregate[Output3T],
        term4: Aggregate[Output4T],
        term5: Aggregate[Output5T],
        term6: Aggregate[Output6T],
        term7: Aggregate[Output7T],
        term8: Aggregate[Output8T],
        /,
    ) -> Pending[tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T]], ...]]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T](
        self: _GroupedQuery[Literal[False], GroupT],
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
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T](
        self: _GroupedQuery[Literal[True], GroupT],
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
    ) -> Pending[tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T]], ...]]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T](
        self: _GroupedQuery[Literal[False], GroupT],
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
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T](
        self: _GroupedQuery[Literal[True], GroupT],
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
    ) -> Pending[tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T]], ...]]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T](
        self: _GroupedQuery[Literal[False], GroupT],
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
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T](
        self: _GroupedQuery[Literal[True], GroupT],
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
    ) -> Pending[tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T]], ...]]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T](
        self: _GroupedQuery[Literal[False], GroupT],
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
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T](
        self: _GroupedQuery[Literal[True], GroupT],
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
    ) -> Pending[tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T]], ...]]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T](
        self: _GroupedQuery[Literal[False], GroupT],
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
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T](
        self: _GroupedQuery[Literal[True], GroupT],
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
    ) -> Pending[tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T]], ...]]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T](
        self: _GroupedQuery[Literal[False], GroupT],
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
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T](
        self: _GroupedQuery[Literal[True], GroupT],
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
    ) -> Pending[tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T]], ...]]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T](
        self: _GroupedQuery[Literal[False], GroupT],
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
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T](
        self: _GroupedQuery[Literal[True], GroupT],
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
    ) -> Pending[tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T]], ...]]: ...

    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T, Output16T](
        self: _GroupedQuery[Literal[False], GroupT],
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
    @overload
    async def aggregate[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T, Output16T](
        self: _GroupedQuery[Literal[True], GroupT],
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
    ) -> Pending[tuple[tuple[GroupT, tuple[Output1T, Output2T, Output3T, Output4T, Output5T, Output6T, Output7T, Output8T, Output9T, Output10T, Output11T, Output12T, Output13T, Output14T, Output15T, Output16T]], ...]]: ...

    # END GENERATED GROUPED AGGREGATE OVERLOADS

class QuerySession(Generic[_Mode]):
    @overload
    def __init__(
        self: QuerySession[Literal[True]],
        connection: StagedTransaction,
        *,
        resources: QueryExecutionResourceLimits | None = ...,
        cancellation: QueryCancellation | None = ...,
        query_limits: QueryLimits | None = ...,
    ) -> None: ...
    @overload
    def __init__(
        self: QuerySession[Literal[False]],
        connection: DirectQueryConnection,
        *,
        resources: QueryExecutionResourceLimits | None = ...,
        cancellation: QueryCancellation | None = ...,
        query_limits: QueryLimits | None = ...,
    ) -> None: ...
    @property
    def is_closed(self) -> bool: ...
    def close(self) -> None: ...
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
    def _function_input[DomainT](
        self, value: AttributeBase, domain: type[DomainT]
    ) -> FunctionInput[DomainT]: ...
    def _call_function[DomainT](
        self,
        function: FunctionRef[..., object],
        arguments: tuple[object, ...],
        domain: type[DomainT],
    ) -> FunctionCall[DomainT]: ...
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
    ) -> _Query[_Mode, DeclaredRowT]: ...
    # BEGIN GENERATED QUERY OVERLOADS
    @overload
    def query[T1](
        self,
        selection1: Selection[T1],
        /,
    ) -> _Query[_Mode, T1]: ...

    @overload
    def query[T1, T2](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        /,
    ) -> _Query[_Mode, T1, T2]: ...

    @overload
    def query[T1, T2, T3](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        /,
    ) -> _Query[_Mode, T1, T2, T3]: ...

    @overload
    def query[T1, T2, T3, T4](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        /,
    ) -> _Query[_Mode, T1, T2, T3, T4]: ...

    @overload
    def query[T1, T2, T3, T4, T5](
        self,
        selection1: Selection[T1],
        selection2: Selection[T2],
        selection3: Selection[T3],
        selection4: Selection[T4],
        selection5: Selection[T5],
        /,
    ) -> _Query[_Mode, T1, T2, T3, T4, T5]: ...

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
    ) -> _Query[_Mode, T1, T2, T3, T4, T5, T6]: ...

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
    ) -> _Query[_Mode, T1, T2, T3, T4, T5, T6, T7]: ...

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
    ) -> _Query[_Mode, T1, T2, T3, T4, T5, T6, T7, T8]: ...

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
    ) -> _Query[_Mode, T1, T2, T3, T4, T5, T6, T7, T8, T9]: ...

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
    ) -> _Query[_Mode, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10]: ...

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
    ) -> _Query[_Mode, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11]: ...

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
    ) -> _Query[_Mode, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12]: ...

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
    ) -> _Query[_Mode, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13]: ...

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
    ) -> _Query[_Mode, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14]: ...

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
    ) -> _Query[_Mode, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15]: ...

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
    ) -> _Query[_Mode, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16]: ...

    # END GENERATED QUERY OVERLOADS

class ExchangeConnection:
    def __init__(
        self,
        advertisement: bytes,
        exchange: Callable[[bytes], Awaitable[bytes]],
        *,
        answer_limits: QueryExecutionResourceLimits | None = ...,
    ) -> None: ...
    @property
    def advertisement(self) -> bytes: ...
    @property
    def exchange(self) -> Callable[[bytes], Awaitable[bytes]]: ...
    @property
    def answer_limits(self) -> QueryExecutionResourceLimits | None: ...

type DirectQueryConnection = Database | TransactionContext | ProxyConnection | ExchangeConnection
type QueryConnection = DirectQueryConnection | StagedTransaction

def connect_proxy(
    endpoint: str,
    *,
    tls_mode: Literal["disabled", "native_roots", "custom_root"] = ...,
    tls_root_ca: str | PathLike[str] | None = ...,
    headers: Sequence[tuple[str, str]] | None = ...,
    advertisement: bytes | None = ...,
    connection_limits: QueryExecutionResourceLimits | None = ...,
    answer_limits: QueryExecutionResourceLimits | None = ...,
    query_limits: QueryLimits | None = ...,
    cancellation: QueryCancellation | None = ...,
) -> ProxyConnection: ...
