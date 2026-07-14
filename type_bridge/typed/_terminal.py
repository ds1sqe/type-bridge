"""The sole pre-executor terminal boundary for the typed-query facade."""

from __future__ import annotations

from typing import Literal, Protocol

from type_bridge_core import (
    MatchBindingHandle,
    MatchOrderHandle,
    MatchQueryHandle,
    PyDescriptorRegistry,
    PyRustTransactionContext,
    ValidatedMatchResultHandle,
    revalidate_match_diagnostic,
)

from type_bridge._rust_runtime import rust_database_for
from type_bridge.session import Database, TransactionContext

MatchOperation = Literal["fetch_rows", "page_by", "count_by", "exists_by"]
RowCardinality = Literal["exactly_one", "bounded_many"]


class _BorrowedTransactionState(Protocol):
    """Native transaction state required by borrowed typed-query terminals."""

    _rust_tx: PyRustTransactionContext | None
    _rust_finalized: bool


class TypedQueryCapabilityError(RuntimeError):
    """A validated typed request cannot run with the available native runtime."""

    __slots__ = ("operation", "cardinality")

    category = "unsupported_capability"
    code = "native_match_executor_unavailable"

    def __init__(
        self,
        operation: MatchOperation,
        cardinality: RowCardinality | None,
    ) -> None:
        self.operation = operation
        self.cardinality = cardinality
        super().__init__(
            "the native typed-query executor/result materializer is unavailable; "
            f"validated {operation} request was not executed"
        )


class TypedQueryWindowError(ValueError):
    """A Python integer cannot form one canonical unsigned result window."""

    __slots__ = ("code",)

    category = "invalid_plan"

    def __init__(self, code: str, message: str) -> None:
        self.code = code
        super().__init__(message)


class TypedQueryConnectionError(RuntimeError):
    """A typed-query terminal has no usable owned or borrowed read target."""

    __slots__ = ("code",)

    category = "invalid_plan"

    def __init__(self, code: str, message: str) -> None:
        self.code = code
        super().__init__(message)


def execute_fetch_rows(
    handle: MatchQueryHandle,
    registry: PyDescriptorRegistry,
    connection: Database | TransactionContext | None,
    order: list[MatchOrderHandle],
    offset: int,
    limit: int,
    cardinality: RowCardinality,
) -> ValidatedMatchResultHandle:
    """Execute one singular-slot request and return only its opaque proof handle."""
    _validate_without_execution(handle, registry, order, offset, limit, cardinality)
    if connection is None:
        raise TypedQueryConnectionError(
            "execution_connection_required",
            "Query.one/rows requires a Database or active read TransactionContext",
        )
    if isinstance(connection, Database):
        database = rust_database_for(connection)
        return handle.execute_fetch_rows_owned(
            database,
            order,
            offset,
            limit,
            cardinality,
        )

    transaction = _active_borrowed_transaction(connection)
    return handle.execute_fetch_rows_borrowed(
        transaction,
        order,
        offset,
        limit,
        cardinality,
    )


def execute_page_by(
    handle: MatchQueryHandle,
    registry: PyDescriptorRegistry,
    connection: Database | TransactionContext | None,
    root: MatchBindingHandle,
    order: list[MatchOrderHandle],
    offset: int,
    limit: int,
    include_total: bool,
) -> ValidatedMatchResultHandle:
    """Execute one root page and retain its exact native validation proof."""
    diagnostic = handle.page_by_diagnostic(root, order, offset, limit, include_total)
    revalidate_match_diagnostic(registry, diagnostic)
    if connection is None:
        raise TypedQueryConnectionError(
            "execution_connection_required",
            "Query.page_by requires a Database or active read TransactionContext",
        )
    if isinstance(connection, Database):
        return handle.execute_page_by_owned(
            rust_database_for(connection),
            root,
            order,
            offset,
            limit,
            include_total,
        )
    transaction = _active_borrowed_transaction(connection)
    return handle.execute_page_by_borrowed(
        transaction,
        root,
        order,
        offset,
        limit,
        include_total,
    )


def execute_count_by(
    handle: MatchQueryHandle,
    registry: PyDescriptorRegistry,
    connection: Database | TransactionContext | None,
    root: MatchBindingHandle,
) -> ValidatedMatchResultHandle:
    """Execute one lossless distinct-root count."""
    diagnostic = handle.count_by_diagnostic(root)
    revalidate_match_diagnostic(registry, diagnostic)
    if connection is None:
        raise TypedQueryConnectionError(
            "execution_connection_required",
            "Query.count_by requires a Database or active read TransactionContext",
        )
    if isinstance(connection, Database):
        return handle.execute_count_by_owned(rust_database_for(connection), root)
    transaction = _active_borrowed_transaction(connection)
    return handle.execute_count_by_borrowed(transaction, root)


def execute_exists_by(
    handle: MatchQueryHandle,
    registry: PyDescriptorRegistry,
    connection: Database | TransactionContext | None,
    root: MatchBindingHandle,
) -> ValidatedMatchResultHandle:
    """Execute one distinct-root existence check."""
    diagnostic = handle.exists_by_diagnostic(root)
    revalidate_match_diagnostic(registry, diagnostic)
    if connection is None:
        raise TypedQueryConnectionError(
            "execution_connection_required",
            "Query.exists_by requires a Database or active read TransactionContext",
        )
    if isinstance(connection, Database):
        return handle.execute_exists_by_owned(rust_database_for(connection), root)
    transaction = _active_borrowed_transaction(connection)
    return handle.execute_exists_by_borrowed(transaction, root)


def _validate_without_execution(
    handle: MatchQueryHandle,
    registry: PyDescriptorRegistry,
    order: list[MatchOrderHandle],
    offset: int,
    limit: int,
    cardinality: RowCardinality,
) -> None:
    diagnostic = handle.fetch_rows_diagnostic(order, offset, limit, cardinality)
    revalidate_match_diagnostic(registry, diagnostic)


def _active_borrowed_transaction(
    connection: _BorrowedTransactionState,
) -> PyRustTransactionContext:
    """Return the active native transaction carried by a borrowed context."""
    transaction = connection._rust_tx
    if transaction is None or connection._rust_finalized:
        raise TypedQueryConnectionError(
            "inactive_transaction_context",
            "borrowed TransactionContext must be active and unconsumed",
        )
    return transaction


__all__ = [
    "TypedQueryCapabilityError",
    "TypedQueryConnectionError",
    "TypedQueryWindowError",
]
