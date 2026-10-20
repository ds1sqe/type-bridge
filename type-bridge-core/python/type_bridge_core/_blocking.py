"""A blocking facade over objects with coroutine methods.

Generated query sessions run every terminal as a coroutine. Scripts that do
not run an event loop wrap a session in :func:`blocking`: each call that
returns an awaitable is run to completion on one private event-loop thread,
and the caller gets its result. The facade holds no query logic; it only
forwards attribute access and calls.
"""

from __future__ import annotations

import asyncio
import functools
import inspect
import threading
from collections.abc import Awaitable, Callable, Coroutine
from typing import Any

__all__ = ["blocking"]

_runner_lock = threading.Lock()
_runner_loop: asyncio.AbstractEventLoop | None = None


def _loop() -> asyncio.AbstractEventLoop:
    """Return the private runner loop, starting its daemon thread once."""
    global _runner_loop
    with _runner_lock:
        if _runner_loop is None or _runner_loop.is_closed():
            loop = asyncio.new_event_loop()
            thread = threading.Thread(
                target=loop.run_forever,
                name="type-bridge-blocking",
                daemon=True,
            )
            thread.start()
            _runner_loop = loop
        return _runner_loop


async def _await[T](awaitable: Awaitable[T]) -> T:
    return await awaitable


def _run[T](awaitable: Awaitable[T]) -> T:
    try:
        asyncio.get_running_loop()
    except RuntimeError:
        pass
    else:
        if inspect.iscoroutine(awaitable):
            awaitable.close()
        raise RuntimeError(
            "type_bridge_core.blocking cannot run inside a running event loop; "
            "await the call instead"
        )
    coroutine: Coroutine[Any, Any, T] = (
        awaitable if inspect.iscoroutine(awaitable) else _await(awaitable)
    )
    return asyncio.run_coroutine_threadsafe(coroutine, _loop()).result()


@functools.cache
def _has_coroutine_methods(cls: type) -> bool:
    """Whether `cls` defines a coroutine method, the facade's wrapping rule."""
    return any(
        inspect.iscoroutinefunction(member)
        for name, member in inspect.getmembers(cls)
        if not name.startswith("__")
    )


def _wrap(value: object) -> Any:
    if isinstance(value, _Blocking) or not _has_coroutine_methods(type(value)):
        return value
    return _Blocking(value)


def _unwrap(value: object) -> object:
    return object.__getattribute__(value, "_target") if isinstance(value, _Blocking) else value


def _result(value: object) -> Any:
    if inspect.isawaitable(value):
        value = _run(value)
    return _wrap(value)


class _Blocking:
    """Forward to a target; run awaitable call results to completion."""

    __slots__ = ("_target",)

    def __init__(self, target: object) -> None:
        object.__setattr__(self, "_target", target)

    def __getattr__(self, name: str) -> Any:
        attribute: object = getattr(object.__getattribute__(self, "_target"), name)
        if not callable(attribute) or isinstance(attribute, type):
            return _wrap(attribute)
        method: Callable[..., object] = attribute

        @functools.wraps(method)
        def call(*arguments: object, **keywords: object) -> Any:
            return _result(
                method(
                    *(_unwrap(argument) for argument in arguments),
                    **{key: _unwrap(value) for key, value in keywords.items()},
                )
            )

        return call

    def __call__(self, *arguments: object, **keywords: object) -> Any:
        target: Callable[..., object] = object.__getattribute__(self, "_target")
        return _result(
            target(
                *(_unwrap(argument) for argument in arguments),
                **{key: _unwrap(value) for key, value in keywords.items()},
            )
        )

    def __setattr__(self, name: str, value: object) -> None:
        setattr(object.__getattribute__(self, "_target"), name, value)

    def __repr__(self) -> str:
        return f"blocking({object.__getattribute__(self, '_target')!r})"


def blocking(target: object) -> Any:
    """Return a blocking view of `target`.

    Attribute access forwards to `target`. A call whose result is awaitable
    runs to completion on a private event-loop thread and returns its
    result. A result whose class defines coroutine methods, such as a query
    from a blocking session, is wrapped again, so a chain stays blocking.
    Calling a blocking method inside a running event loop raises
    ``RuntimeError``; await the call there instead.
    """
    return _Blocking(target)
