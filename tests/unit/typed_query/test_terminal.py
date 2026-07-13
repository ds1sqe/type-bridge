"""Execution-target routing for selected-row typed-query terminals."""

from __future__ import annotations

from typing import Protocol, runtime_checkable

import pytest

from type_bridge.session import Database, TransactionContext
from type_bridge.typed import TypedQueryConnectionError, _terminal


class _TerminalQueryHandle(Protocol):
    def execute_fetch_rows_owned(self, *arguments: object) -> object: ...

    def execute_fetch_rows_borrowed(self, *arguments: object) -> object: ...

    def fetch_rows_diagnostic(self, *arguments: object) -> str: ...

    def page_by_diagnostic(self, *arguments: object) -> str: ...

    def count_by_diagnostic(self, *arguments: object) -> str: ...

    def exists_by_diagnostic(self, *arguments: object) -> str: ...

    def execute_page_by_owned(self, *arguments: object) -> object: ...

    def execute_page_by_borrowed(self, *arguments: object) -> object: ...

    def execute_count_by_owned(self, *arguments: object) -> object: ...

    def execute_count_by_borrowed(self, *arguments: object) -> object: ...

    def execute_exists_by_owned(self, *arguments: object) -> object: ...

    def execute_exists_by_borrowed(self, *arguments: object) -> object: ...


class _Registry:
    """Opaque registry sentinel accepted only by the patched diagnostic seam."""


class _Root:
    """Opaque root-binding sentinel passed through the fake native handle."""


class _Order(str):
    """String-compatible order sentinel retained in exact call assertions."""


@runtime_checkable
class _FetchRowsTerminal(Protocol):
    def __call__(
        self,
        handle: _TerminalQueryHandle,
        registry: _Registry,
        connection: Database | TransactionContext | None,
        order: list[_Order],
        offset: int,
        limit: int,
        cardinality: str,
    ) -> object: ...


@runtime_checkable
class _PageByTerminal(Protocol):
    def __call__(
        self,
        handle: _TerminalQueryHandle,
        registry: _Registry,
        connection: Database | TransactionContext | None,
        root: _Root,
        order: list[_Order],
        offset: int,
        limit: int,
        include_total: bool,
    ) -> object: ...


@runtime_checkable
class _RootTerminal(Protocol):
    def __call__(
        self,
        handle: _TerminalQueryHandle,
        registry: _Registry,
        connection: Database | TransactionContext | None,
        root: _Root,
    ) -> object: ...


def _checked_callable[CallableT](value: object, expected: type[CallableT]) -> CallableT:
    if not isinstance(value, expected):
        raise AssertionError(f"{value!r} does not satisfy {expected.__name__}")
    return value


_execute_fetch_rows = _checked_callable(_terminal.execute_fetch_rows, _FetchRowsTerminal)
_execute_page_by = _checked_callable(_terminal.execute_page_by, _PageByTerminal)
_execute_count_by = _checked_callable(_terminal.execute_count_by, _RootTerminal)
_execute_exists_by = _checked_callable(_terminal.execute_exists_by, _RootTerminal)


class _QueryHandle(_TerminalQueryHandle):
    def __init__(self, proof: object) -> None:
        self.proof = proof
        self.calls: list[tuple[object, ...]] = []

    def execute_fetch_rows_owned(self, *arguments: object) -> object:
        self.calls.append(("owned", *arguments))
        return self.proof

    def execute_fetch_rows_borrowed(self, *arguments: object) -> object:
        self.calls.append(("borrowed", *arguments))
        return self.proof

    def fetch_rows_diagnostic(self, *arguments: object) -> str:
        self.calls.append(("diagnostic", *arguments))
        return "canonical-diagnostic"

    def page_by_diagnostic(self, *arguments: object) -> str:
        self.calls.append(("page_diagnostic", *arguments))
        return "canonical-page"

    def count_by_diagnostic(self, *arguments: object) -> str:
        self.calls.append(("count_diagnostic", *arguments))
        return "canonical-count"

    def exists_by_diagnostic(self, *arguments: object) -> str:
        self.calls.append(("exists_diagnostic", *arguments))
        return "canonical-exists"

    def execute_page_by_owned(self, *arguments: object) -> object:
        self.calls.append(("page_owned", *arguments))
        return self.proof

    def execute_page_by_borrowed(self, *arguments: object) -> object:
        self.calls.append(("page_borrowed", *arguments))
        return self.proof

    def execute_count_by_owned(self, *arguments: object) -> object:
        self.calls.append(("count_owned", *arguments))
        return self.proof

    def execute_count_by_borrowed(self, *arguments: object) -> object:
        self.calls.append(("count_borrowed", *arguments))
        return self.proof

    def execute_exists_by_owned(self, *arguments: object) -> object:
        self.calls.append(("exists_owned", *arguments))
        return self.proof

    def execute_exists_by_borrowed(self, *arguments: object) -> object:
        self.calls.append(("exists_borrowed", *arguments))
        return self.proof


def _execute(
    handle: _TerminalQueryHandle,
    connection: Database | TransactionContext,
) -> object:
    return _execute_fetch_rows(
        handle,
        _Registry(),
        connection,
        [_Order("order")],
        3,
        7,
        "bounded_many",
    )


@pytest.fixture(autouse=True)
def _accept_test_diagnostics(monkeypatch: pytest.MonkeyPatch) -> list[str]:
    validated: list[str] = []
    monkeypatch.setattr(
        _terminal,
        "revalidate_match_diagnostic",
        lambda _registry, diagnostic: validated.append(diagnostic),
    )
    return validated


def test_owned_database_routes_only_through_native_owned_execution(
    monkeypatch: pytest.MonkeyPatch,
    _accept_test_diagnostics: list[str],
) -> None:
    proof = object()
    handle = _QueryHandle(proof)
    database = Database(server_version="3.11.0")
    native_database = object()
    resolved: list[Database] = []

    def resolve_database(value: Database) -> object:
        resolved.append(value)
        return native_database

    monkeypatch.setattr(_terminal, "rust_database_for", resolve_database)

    assert _execute(handle, database) is proof
    assert resolved == [database]
    assert _accept_test_diagnostics == ["canonical-diagnostic"]
    assert handle.calls == [
        ("diagnostic", ["order"], 3, 7, "bounded_many"),
        ("owned", native_database, ["order"], 3, 7, "bounded_many"),
    ]


def test_active_read_context_routes_borrowed_without_consuming_lifecycle(
    _accept_test_diagnostics: list[str],
) -> None:
    proof = object()
    handle = _QueryHandle(proof)
    transaction = Database(server_version="3.11.0").transaction("read")
    native_transaction = object()
    transaction._rust_tx = native_transaction
    transaction._rust_finalized = False

    assert _execute(handle, transaction) is proof
    assert _accept_test_diagnostics == ["canonical-diagnostic"]
    assert handle.calls == [
        ("diagnostic", ["order"], 3, 7, "bounded_many"),
        ("borrowed", native_transaction, ["order"], 3, 7, "bounded_many"),
    ]
    assert transaction._rust_tx is native_transaction
    assert not transaction._rust_finalized


def test_page_count_and_exists_use_operation_specific_borrowed_seams(
    _accept_test_diagnostics: list[str],
) -> None:
    proof = object()
    handle = _QueryHandle(proof)
    transaction = Database(server_version="3.11.0").transaction("read")
    native_transaction = object()
    transaction._rust_tx = native_transaction
    root = _Root()
    registry = _Registry()

    assert (
        _execute_page_by(
            handle,
            registry,
            transaction,
            root,
            [_Order("order")],
            4,
            9,
            True,
        )
        is proof
    )
    assert _execute_count_by(handle, registry, transaction, root) is proof
    assert _execute_exists_by(handle, registry, transaction, root) is proof

    assert _accept_test_diagnostics == [
        "canonical-page",
        "canonical-count",
        "canonical-exists",
    ]
    assert handle.calls == [
        ("page_diagnostic", root, ["order"], 4, 9, True),
        ("page_borrowed", native_transaction, root, ["order"], 4, 9, True),
        ("count_diagnostic", root),
        ("count_borrowed", native_transaction, root),
        ("exists_diagnostic", root),
        ("exists_borrowed", native_transaction, root),
    ]
    assert transaction._rust_tx is native_transaction
    assert not transaction._rust_finalized


def test_active_non_read_target_reaches_native_canonical_target_preflight(
    _accept_test_diagnostics: list[str],
) -> None:
    proof = object()
    handle = _QueryHandle(proof)
    transaction = Database(server_version="3.11.0").transaction("write")
    native_transaction = object()
    transaction._rust_tx = native_transaction

    assert _execute(handle, transaction) is proof
    assert _accept_test_diagnostics == ["canonical-diagnostic"]
    assert handle.calls == [
        ("diagnostic", ["order"], 3, 7, "bounded_many"),
        ("borrowed", native_transaction, ["order"], 3, 7, "bounded_many"),
    ]


def test_inactive_borrowed_target_revalidates_before_connection_error(
    _accept_test_diagnostics: list[str],
) -> None:
    handle = _QueryHandle(object())
    transaction = Database(server_version="3.11.0").transaction("read")
    transaction._rust_tx = None
    with pytest.raises(TypedQueryConnectionError) as raised:
        _execute(handle, transaction)

    assert raised.value.category == "invalid_plan"
    assert raised.value.code == "inactive_transaction_context"
    assert _accept_test_diagnostics == ["canonical-diagnostic"]
    assert handle.calls == [("diagnostic", ["order"], 3, 7, "bounded_many")]
