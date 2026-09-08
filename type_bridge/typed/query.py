"""Immutable variadic typed-query facade over one opaque native query handle."""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from typing import overload

from type_bridge_core import (
    MatchOrderHandle,
    MatchQueryHandle,
    ValidatedMatchResultHandle,
    validate_match_order_term_count,
)
from type_bridge_core import (
    _QueryDescriptorRegistry as PyDescriptorRegistry,
)

from type_bridge.models.base import _QueryTypeDBType as TypeDBType
from type_bridge.session import Database, TransactionContext
from type_bridge.typed._terminal import (
    TypedQueryWindowError,
    execute_count_by,
    execute_exists_by,
    execute_fetch_rows,
    execute_page_by,
)
from type_bridge.typed.page import Page
from type_bridge.typed.references import (
    BoundVar,
    Predicate,
    QueryOrder,
    _PlayerBinding,
)
from type_bridge.typed.results import (
    _materialize_count,
    _materialize_exists,
    _materialize_one,
    _materialize_page,
    _materialize_rows,
)


class Query[*Slots]:
    """One immutable positional query whose semantic state stays native-owned."""

    __slots__ = (
        "__handle",
        "__registry",
        "__connection",
        "__models",
        "__declaration",
    )

    def __init__(self) -> None:
        raise TypeError("Query values are created by QuerySession.query")

    @classmethod
    def _from_native(
        cls,
        handle: MatchQueryHandle,
        registry: PyDescriptorRegistry,
        connection: Database | TransactionContext | None,
        models: Mapping[str, type[TypeDBType]],
        declaration: type[object] | None = None,
    ) -> Query[*Slots]:
        value = object.__new__(cls)
        object.__setattr__(value, "_Query__handle", handle)
        object.__setattr__(value, "_Query__registry", registry)
        object.__setattr__(value, "_Query__connection", connection)
        object.__setattr__(value, "_Query__models", models)
        object.__setattr__(value, "_Query__declaration", declaration)
        return value

    def __setattr__(self, name: str, value: object) -> None:
        del name, value
        raise AttributeError("Query values are immutable")

    def match(self, *bindings: _PlayerBinding[TypeDBType]) -> Query[*Slots]:
        """Return a sibling query with hidden witness bindings attached."""
        if not bindings:
            raise TypeError("Query.match requires at least one BoundVar")
        handle = self.__handle
        for binding in bindings:
            if not isinstance(binding, BoundVar):
                raise TypeError("Query.match requires BoundVar values")
            handle = handle.add_hidden(binding._native_binding())
        return Query._from_native(
            handle,
            self.__registry,
            self.__connection,
            self.__models,
            self.__declaration,
        )

    def where(self, *predicates: Predicate) -> Query[*Slots]:
        """Return a sibling query with predicates conjoined in call order."""
        if not predicates:
            raise TypeError("Query.where requires at least one Predicate")
        handle = self.__handle
        for predicate in predicates:
            if not isinstance(predicate, Predicate):
                raise TypeError("Query.where requires Predicate values")
            handle = handle.where_predicate(predicate._native_predicate())
        return Query._from_native(
            handle,
            self.__registry,
            self.__connection,
            self.__models,
            self.__declaration,
        )

    def allow_cross_join[LeftT: TypeDBType, RightT: TypeDBType](
        self,
        left: BoundVar[LeftT],
        right: BoundVar[RightT],
    ) -> Query[*Slots]:
        """Return a sibling query with one explicit topology permission."""
        if not isinstance(left, BoundVar) or not isinstance(right, BoundVar):
            raise TypeError("Query.allow_cross_join requires BoundVar values")
        handle = self.__handle.allow_cross_join(left._native_binding(), right._native_binding())
        return Query._from_native(
            handle,
            self.__registry,
            self.__connection,
            self.__models,
            self.__declaration,
        )

    @overload
    def one[SlotT](self: Query[SlotT]) -> SlotT: ...

    @overload
    def one[Slot1T, Slot2T, *RestT](
        self: Query[Slot1T, Slot2T, *RestT],
    ) -> tuple[Slot1T, Slot2T, *RestT]: ...

    def one[Slot1T, Slot2T, *RestT](
        self: Query[Slot1T] | Query[Slot1T, Slot2T, *RestT],
    ) -> Slot1T | tuple[Slot1T, Slot2T, *RestT]:
        """Require exactly one distinct selected identity tuple."""
        result = execute_fetch_rows(
            self.__handle,
            self.__registry,
            self.__connection,
            [],
            0,
            1,
            "exactly_one",
        )
        return _materialize_one_for(self, result, self.__models, self.__declaration)

    @overload
    def rows[SlotT](
        self: Query[SlotT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
    ) -> list[SlotT]: ...

    @overload
    def rows[Slot1T, Slot2T, *RestT](
        self: Query[Slot1T, Slot2T, *RestT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
    ) -> list[tuple[Slot1T, Slot2T, *RestT]]: ...

    def rows[Slot1T, Slot2T, *RestT](
        self: Query[Slot1T] | Query[Slot1T, Slot2T, *RestT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
    ) -> list[Slot1T] | list[tuple[Slot1T, Slot2T, *RestT]]:
        """Fetch a bounded list of distinct selected identity tuples."""
        orders = _native_orders(order_by)
        offset, limit = _window(offset, limit)
        result = execute_fetch_rows(
            self.__handle,
            self.__registry,
            self.__connection,
            orders,
            offset,
            limit,
            "bounded_many",
        )
        return _materialize_rows_for(self, result, self.__models, self.__declaration)

    # fmt: off
    # BEGIN GENERATED PAGE OVERLOADS
    @overload
    def page_by[SlotT, RootT: TypeDBType](
        self: Query[SlotT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[SlotT]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected2T: TypeDBType](
        self: Query[RootT, tuple[Collected2T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType](
        self: Query[tuple[Collected1T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected6T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected7T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected8T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected9T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected10T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected11T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected12T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected13T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected14T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected15T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: Query[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[RootT, tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: Query[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], RootT, tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], RootT, tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], RootT, tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], RootT, tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], RootT, tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], RootT, tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], RootT, tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], RootT, tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], RootT, tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], RootT, tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], RootT, tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], RootT, tuple[Collected14T, ...], tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected15T: TypeDBType, Collected16T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...], tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], RootT, tuple[Collected15T, ...], tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected16T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT, tuple[Collected16T, ...]],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], RootT, tuple[Collected16T, ...]]]: ...

    @overload
    def page_by[RootT: TypeDBType, Collected1T: TypeDBType, Collected2T: TypeDBType, Collected3T: TypeDBType, Collected4T: TypeDBType, Collected5T: TypeDBType, Collected6T: TypeDBType, Collected7T: TypeDBType, Collected8T: TypeDBType, Collected9T: TypeDBType, Collected10T: TypeDBType, Collected11T: TypeDBType, Collected12T: TypeDBType, Collected13T: TypeDBType, Collected14T: TypeDBType, Collected15T: TypeDBType](
        self: Query[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], RootT],
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[tuple[tuple[Collected1T, ...], tuple[Collected2T, ...], tuple[Collected3T, ...], tuple[Collected4T, ...], tuple[Collected5T, ...], tuple[Collected6T, ...], tuple[Collected7T, ...], tuple[Collected8T, ...], tuple[Collected9T, ...], tuple[Collected10T, ...], tuple[Collected11T, ...], tuple[Collected12T, ...], tuple[Collected13T, ...], tuple[Collected14T, ...], tuple[Collected15T, ...], RootT]]: ...

    # END GENERATED PAGE OVERLOADS
    # fmt: on

    def page_by[RootT: TypeDBType](
        self,
        root: BoundVar[RootT],
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page[object]:
        """Page a native shape by one singular selected root identity."""
        if not isinstance(root, BoundVar):
            raise TypeError("Query.page_by requires a BoundVar root")
        if not isinstance(include_total, bool):
            raise TypeError("include_total must be a bool")
        orders = _native_orders(order_by)
        offset, limit = _window(offset, limit)
        result = execute_page_by(
            self.__handle,
            self.__registry,
            self.__connection,
            root._native_binding(),
            orders,
            offset,
            limit,
            include_total,
        )
        return _materialize_page(result, self.__models, self.__declaration)

    def count_by[RootT: TypeDBType](self, root: BoundVar[RootT]) -> int:
        """Count distinct identities for one attached root binding."""
        if not isinstance(root, BoundVar):
            raise TypeError("Query.count_by requires a BoundVar root")
        result = execute_count_by(
            self.__handle,
            self.__registry,
            self.__connection,
            root._native_binding(),
        )
        return _materialize_count(result)

    def exists_by[RootT: TypeDBType](self, root: BoundVar[RootT]) -> bool:
        """Test whether one attached root binding has any distinct identity."""
        if not isinstance(root, BoundVar):
            raise TypeError("Query.exists_by requires a BoundVar root")
        result = execute_exists_by(
            self.__handle,
            self.__registry,
            self.__connection,
            root._native_binding(),
        )
        return _materialize_exists(result)

    def _native_query(self) -> MatchQueryHandle:
        """Return the private native handle for the executor landing in #176."""
        return self.__handle

    def _model_constructors(self) -> Mapping[str, type[TypeDBType]]:
        """Return immutable model metadata for validated result materialization."""
        return self.__models

    def _row_declaration(self) -> type[object] | None:
        """Return immutable named-row materialization metadata, when present."""
        return self.__declaration


def _native_orders(values: Iterable[QueryOrder]) -> list[MatchOrderHandle]:
    orders: list[MatchOrderHandle] = []
    for value in values:
        if not isinstance(value, QueryOrder):
            raise TypeError("order_by entries must be QueryOrder values")
        validate_match_order_term_count(len(orders) + 1)
        orders.append(value._native_order())
    return orders


def _window(offset: int, limit: int) -> tuple[int, int]:
    maximum = (1 << 64) - 1
    if isinstance(offset, bool) or not isinstance(offset, int):
        raise TypeError("offset must be an integer")
    if isinstance(limit, bool) or not isinstance(limit, int):
        raise TypeError("limit must be an integer")
    if offset < 0 or offset > maximum:
        raise TypedQueryWindowError(
            "invalid_window_offset",
            "offset must fit the canonical unsigned 64-bit range",
        )
    if limit <= 0 or limit > maximum:
        raise TypedQueryWindowError(
            "invalid_window_limit",
            "limit must be positive and fit the canonical unsigned 64-bit range",
        )
    if offset + limit > maximum:
        raise TypedQueryWindowError(
            "window_overflow",
            "offset plus limit exceeds the canonical unsigned 64-bit range",
        )
    return offset, limit


@overload
def _materialize_one_for[SlotT](
    query: Query[SlotT],
    result: ValidatedMatchResultHandle,
    models: Mapping[str, type[TypeDBType]],
    declaration: type[object] | None,
) -> SlotT: ...


@overload
def _materialize_one_for[Slot1T, Slot2T, *RestT](
    query: Query[Slot1T, Slot2T, *RestT],
    result: ValidatedMatchResultHandle,
    models: Mapping[str, type[TypeDBType]],
    declaration: type[object] | None,
) -> tuple[Slot1T, Slot2T, *RestT]: ...


def _materialize_one_for[*Slots](
    query: Query[*Slots],
    result: ValidatedMatchResultHandle,
    models: Mapping[str, type[TypeDBType]],
    declaration: type[object] | None,
) -> object:
    """Bind a native-validated row to the query shape that produced it."""
    del query
    return _materialize_one(result, models, declaration)


@overload
def _materialize_rows_for[SlotT](
    query: Query[SlotT],
    result: ValidatedMatchResultHandle,
    models: Mapping[str, type[TypeDBType]],
    declaration: type[object] | None,
) -> list[SlotT]: ...


@overload
def _materialize_rows_for[Slot1T, Slot2T, *RestT](
    query: Query[Slot1T, Slot2T, *RestT],
    result: ValidatedMatchResultHandle,
    models: Mapping[str, type[TypeDBType]],
    declaration: type[object] | None,
) -> list[tuple[Slot1T, Slot2T, *RestT]]: ...


def _materialize_rows_for[*Slots](
    query: Query[*Slots],
    result: ValidatedMatchResultHandle,
    models: Mapping[str, type[TypeDBType]],
    declaration: type[object] | None,
) -> object:
    """Bind native-validated rows to the query shape that produced them."""
    del query
    return _materialize_rows(result, models, declaration)


__all__ = ["Query"]
