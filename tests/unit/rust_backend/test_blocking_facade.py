"""The blocking facade runs coroutine results on a private event-loop thread."""

from __future__ import annotations

import asyncio
import threading

import pytest
from type_bridge_core import blocking


class _Terminal:
    """A stand-in query: synchronous composition, coroutine terminals."""

    def __init__(self, value: int) -> None:
        self.value = value

    def where(self, delta: int) -> _Terminal:
        return _Terminal(self.value + delta)

    def same(self, other: _Terminal) -> bool:
        return type(other) is _Terminal and other.value == self.value

    async def one(self) -> int:
        await asyncio.sleep(0)
        return self.value

    async def thread_name(self) -> str:
        return threading.current_thread().name

    async def fail(self) -> int:
        raise LookupError("terminal refused")


class _Session:
    """A stand-in session: no coroutine methods, returns queries."""

    def query(self, value: int) -> _Terminal:
        return _Terminal(value)

    def label(self) -> str:
        return "session"


def test_awaitable_results_run_to_completion() -> None:
    query = blocking(_Terminal(3))
    assert query.one() == 3


def test_chains_stay_blocking() -> None:
    session = blocking(_Session())
    assert session.label() == "session"
    assert session.query(1).where(2).where(3).one() == 6


def test_wrapped_arguments_are_unwrapped() -> None:
    query = blocking(_Terminal(5))
    assert query.same(blocking(_Terminal(5))) is True


def test_plain_attributes_forward() -> None:
    assert blocking(_Terminal(7)).value == 7


def test_terminals_share_one_private_thread() -> None:
    first = blocking(_Terminal(1)).thread_name()
    second = blocking(_Terminal(2)).thread_name()
    assert first == second == "type-bridge-blocking"
    assert first != threading.current_thread().name


def test_errors_propagate() -> None:
    with pytest.raises(LookupError, match="terminal refused"):
        blocking(_Terminal(1)).fail()


def test_inside_a_running_loop_refuses_instead_of_deadlocking() -> None:
    query = blocking(_Terminal(1))

    async def inside() -> None:
        with pytest.raises(RuntimeError, match="running event loop"):
            query.one()

    asyncio.run(inside())


def test_concurrent_callers_from_threads() -> None:
    results: list[int] = []
    lock = threading.Lock()

    def call(value: int) -> None:
        result = blocking(_Terminal(value)).one()
        with lock:
            results.append(result)

    threads = [threading.Thread(target=call, args=(index,)) for index in range(8)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    assert sorted(results) == list(range(8))
