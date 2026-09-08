"""Asynchronous remote wrapper over one ordinary immutable typed query."""

from __future__ import annotations

from collections.abc import Iterable
from typing import overload

from type_bridge.models.base import _QueryTypeDBType as TypeDBType
from type_bridge.typed._remote_terminal import (
    _RemoteRuntime,
    execute_remote_count,
    execute_remote_exists,
    execute_remote_one,
    execute_remote_page,
    execute_remote_rows,
)
from type_bridge.typed.page import Page
from type_bridge.typed.query import Query
from type_bridge.typed.references import BoundVar, Predicate, QueryOrder, _PlayerBinding


class RemoteQuery[*Slots]:
    """One immutable typed query whose terminal performs one remote exchange."""

    __slots__ = ("__direct", "__runtime")

    def __init__(self) -> None:
        raise TypeError("RemoteQuery values are created by RemoteQuerySession.query")

    @classmethod
    def _from_direct(
        cls,
        direct: Query[*Slots],
        runtime: _RemoteRuntime,
    ) -> RemoteQuery[*Slots]:
        value = object.__new__(cls)
        object.__setattr__(value, "_RemoteQuery__direct", direct)
        object.__setattr__(value, "_RemoteQuery__runtime", runtime)
        return value

    def __setattr__(self, name: str, value: object) -> None:
        del name, value
        raise AttributeError("RemoteQuery values are immutable")

    def match(self, *bindings: _PlayerBinding[TypeDBType]) -> RemoteQuery[*Slots]:
        """Delegate hidden-witness composition to the released query grammar."""
        return RemoteQuery._from_direct(
            self.__direct.match(*bindings),
            self.__runtime,
        )

    def where(self, *predicates: Predicate) -> RemoteQuery[*Slots]:
        """Delegate predicate composition to the released query grammar."""
        return RemoteQuery._from_direct(
            self.__direct.where(*predicates),
            self.__runtime,
        )

    def allow_cross_join[LeftT: TypeDBType, RightT: TypeDBType](
        self,
        left: BoundVar[LeftT],
        right: BoundVar[RightT],
    ) -> RemoteQuery[*Slots]:
        """Delegate topology permission to the released query grammar."""
        return RemoteQuery._from_direct(
            self.__direct.allow_cross_join(left, right),
            self.__runtime,
        )

    @overload
    async def one[SlotT](self: RemoteQuery[SlotT]) -> SlotT: ...

    @overload
    async def one[Slot1T, Slot2T, *RestT](
        self: RemoteQuery[Slot1T, Slot2T, *RestT],
    ) -> tuple[Slot1T, Slot2T, *RestT]: ...

    async def one[Slot1T, Slot2T, *RestT](
        self: RemoteQuery[Slot1T] | RemoteQuery[Slot1T, Slot2T, *RestT],
    ) -> Slot1T | tuple[Slot1T, Slot2T, *RestT]:
        """Require exactly one distinct selected identity tuple remotely."""
        return await execute_remote_one(self.__direct, self.__runtime)

    @overload
    async def rows[SlotT](
        self: RemoteQuery[SlotT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
    ) -> list[SlotT]: ...

    @overload
    async def rows[Slot1T, Slot2T, *RestT](
        self: RemoteQuery[Slot1T, Slot2T, *RestT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
    ) -> list[tuple[Slot1T, Slot2T, *RestT]]: ...

    async def rows[Slot1T, Slot2T, *RestT](
        self: RemoteQuery[Slot1T] | RemoteQuery[Slot1T, Slot2T, *RestT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
    ) -> list[Slot1T] | list[tuple[Slot1T, Slot2T, *RestT]]:
        """Fetch stable bounded selected rows through exactly one exchange."""
        return await execute_remote_rows(
            self.__direct,
            self.__runtime,
            order_by,
            offset,
            limit,
        )

    # fmt: off
    # BEGIN GENERATED REMOTE PAGE OVERLOADS
    @overload
    async def page_by[SlotT, RootT: TypeDBType](
        self: RemoteQuery[SlotT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[SlotT]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected2T: TypeDBType](
        self: RemoteQuery[RootT, tuple[Collected2T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected6T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected7T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected8T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected9T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected10T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected11T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected12T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected13T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected14T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected15T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: RemoteQuery[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected16T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT, tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT, tuple[Collected16T, ...]]]: ...

    @overload
    async def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: RemoteQuery[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], RootT]]: ...

    # END GENERATED REMOTE PAGE OVERLOADS
    # fmt: on

    async def page_by[RootT: TypeDBType](
        self,
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[object]:
        """Page by one root in one server snapshot and one exchange."""
        return await execute_remote_page(
            self.__direct,
            self.__runtime,
            root,
            order_by,
            offset,
            limit,
            include_total,
        )

    async def count_by[RootT: TypeDBType](self, root: BoundVar[RootT]) -> int:
        """Count distinct root identities remotely without precision loss."""
        return await execute_remote_count(self.__direct, self.__runtime, root)

    async def exists_by[RootT: TypeDBType](self, root: BoundVar[RootT]) -> bool:
        """Test distinct-root existence remotely."""
        return await execute_remote_exists(self.__direct, self.__runtime, root)


__all__ = ["RemoteQuery"]
