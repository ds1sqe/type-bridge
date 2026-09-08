"""One native preparation, one caller exchange, and one native reply decode."""

from __future__ import annotations

from collections.abc import Awaitable, Iterable
from dataclasses import dataclass
from typing import Protocol, overload

from type_bridge_core import (
    PendingRemoteModelQuery,
    RemoteModelQueryContext,
    ValidatedMatchResultHandle,
    query_v2_prepare_remote_model_count,
    query_v2_prepare_remote_model_exists,
    query_v2_prepare_remote_model_page,
    query_v2_prepare_remote_model_rows,
)

from type_bridge.models.base import _QueryTypeDBType as TypeDBType
from type_bridge.typed.page import Page
from type_bridge.typed.query import Query, _native_orders, _window
from type_bridge.typed.references import BoundVar, QueryOrder
from type_bridge.typed.results import (
    _materialize_count,
    _materialize_exists,
    _materialize_one,
    _materialize_page,
    _materialize_rows,
)


class RemoteQueryExchange(Protocol):
    """One caller-owned transport exchange over immutable request bytes."""

    def __call__(self, request: bytes, /) -> Awaitable[bytes]: ...


@dataclass(frozen=True, slots=True)
class _RemoteRuntime:
    context: RemoteModelQueryContext
    exchange: RemoteQueryExchange


@overload
async def execute_remote_one[SlotT](
    query: Query[SlotT],
    runtime: _RemoteRuntime,
) -> SlotT: ...


@overload
async def execute_remote_one[Slot1T, Slot2T, *RestT](
    query: Query[Slot1T, Slot2T, *RestT],
    runtime: _RemoteRuntime,
) -> tuple[Slot1T, Slot2T, *RestT]: ...


async def execute_remote_one[*Slots](
    query: Query[*Slots],
    runtime: _RemoteRuntime,
) -> object:
    pending = query_v2_prepare_remote_model_rows(
        query._native_query(),
        runtime.context,
        [],
        0,
        1,
        "exactly_one",
    )
    result = await _exchange_once(pending, runtime.exchange)
    return _materialize_one(
        result,
        query._model_constructors(),
        query._row_declaration(),
    )


@overload
async def execute_remote_rows[SlotT](
    query: Query[SlotT],
    runtime: _RemoteRuntime,
    order_by: Iterable[QueryOrder],
    offset: int,
    limit: int,
) -> list[SlotT]: ...


@overload
async def execute_remote_rows[Slot1T, Slot2T, *RestT](
    query: Query[Slot1T, Slot2T, *RestT],
    runtime: _RemoteRuntime,
    order_by: Iterable[QueryOrder],
    offset: int,
    limit: int,
) -> list[tuple[Slot1T, Slot2T, *RestT]]: ...


async def execute_remote_rows[*Slots](
    query: Query[*Slots],
    runtime: _RemoteRuntime,
    order_by: Iterable[QueryOrder],
    offset: int,
    limit: int,
) -> object:
    orders = _native_orders(order_by)
    offset, limit = _window(offset, limit)
    pending = query_v2_prepare_remote_model_rows(
        query._native_query(),
        runtime.context,
        orders,
        offset,
        limit,
        "bounded_many",
    )
    result = await _exchange_once(pending, runtime.exchange)
    return _materialize_rows(
        result,
        query._model_constructors(),
        query._row_declaration(),
    )


async def execute_remote_page[*Slots, RootT: TypeDBType](
    query: Query[*Slots],
    runtime: _RemoteRuntime,
    root: BoundVar[RootT],
    order_by: Iterable[QueryOrder],
    offset: int,
    limit: int,
    include_total: bool,
) -> Page[object]:
    if not isinstance(root, BoundVar):
        raise TypeError("Query.page_by requires a BoundVar root")
    if not isinstance(include_total, bool):
        raise TypeError("include_total must be a bool")
    orders = _native_orders(order_by)
    offset, limit = _window(offset, limit)
    pending = query_v2_prepare_remote_model_page(
        query._native_query(),
        runtime.context,
        root._native_binding(),
        orders,
        offset,
        limit,
        include_total,
    )
    result = await _exchange_once(pending, runtime.exchange)
    return _materialize_page(
        result,
        query._model_constructors(),
        query._row_declaration(),
    )


async def execute_remote_count[*Slots, RootT: TypeDBType](
    query: Query[*Slots],
    runtime: _RemoteRuntime,
    root: BoundVar[RootT],
) -> int:
    if not isinstance(root, BoundVar):
        raise TypeError("Query.count_by requires a BoundVar root")
    pending = query_v2_prepare_remote_model_count(
        query._native_query(),
        runtime.context,
        root._native_binding(),
    )
    result = await _exchange_once(pending, runtime.exchange)
    return _materialize_count(result)


async def execute_remote_exists[*Slots, RootT: TypeDBType](
    query: Query[*Slots],
    runtime: _RemoteRuntime,
    root: BoundVar[RootT],
) -> bool:
    if not isinstance(root, BoundVar):
        raise TypeError("Query.exists_by requires a BoundVar root")
    pending = query_v2_prepare_remote_model_exists(
        query._native_query(),
        runtime.context,
        root._native_binding(),
    )
    result = await _exchange_once(pending, runtime.exchange)
    return _materialize_exists(result)


async def _exchange_once(
    pending: PendingRemoteModelQuery,
    exchange: RemoteQueryExchange,
) -> ValidatedMatchResultHandle:
    request = bytes(pending.request_bytes())
    response = await exchange(request)
    if type(response) is not bytes:
        raise TypeError("remote query exchange must resolve to bytes")
    return pending.decode_reply(response)


__all__ = ["RemoteQueryExchange"]
