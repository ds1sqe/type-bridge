"""Lifecycle hook system for CRUD operations.

Hooks are duck-typed classes that implement only the methods they care about.
Register them on a manager instance via ``manager.add_hook(hook)``.

Pre-hooks run in registration order and may raise ``HookCancelled`` to abort.
Post-hooks run in reverse registration order; errors are logged, not propagated.
"""

from __future__ import annotations

import logging
from enum import Enum
from typing import TYPE_CHECKING, Any, Protocol, runtime_checkable

if TYPE_CHECKING:
    from type_bridge.models.base import TypeDBType

logger = logging.getLogger(__name__)


class CrudEvent(Enum):
    """CRUD lifecycle events."""

    PRE_INSERT = "pre_insert"
    POST_INSERT = "post_insert"
    PRE_UPDATE = "pre_update"
    POST_UPDATE = "post_update"
    PRE_DELETE = "pre_delete"
    POST_DELETE = "post_delete"
    PRE_PUT = "pre_put"
    POST_PUT = "post_put"


class HookCancelled(Exception):  # noqa: N818 — not an error, a control-flow signal
    """Raise in a pre-hook to abort the operation.

    Attributes:
        reason: Human-readable explanation.
        event: The event that was cancelled (set by HookRunner).
        hook: The hook instance that raised the cancellation (set by HookRunner).
    """

    def __init__(
        self,
        reason: str = "",
        *,
        event: CrudEvent | None = None,
        hook: Any = None,
    ):
        self.reason = reason
        self.event = event
        self.hook = hook
        super().__init__(reason)


@runtime_checkable
class CrudHook(Protocol):
    """Protocol for CRUD lifecycle hooks.

    Implement only the methods you need.  All methods are optional —
    ``HookRunner`` uses ``hasattr`` / ``getattr`` to discover them.
    """

    def should_run(self, event: CrudEvent, sender: type[TypeDBType]) -> bool: ...
    def pre_insert(self, sender: type[TypeDBType], instance: Any) -> None: ...
    def post_insert(self, sender: type[TypeDBType], instance: Any) -> None: ...
    def pre_update(self, sender: type[TypeDBType], instance: Any) -> None: ...
    def post_update(self, sender: type[TypeDBType], instance: Any) -> None: ...
    def pre_delete(self, sender: type[TypeDBType], instance: Any) -> None: ...
    def post_delete(self, sender: type[TypeDBType], instance: Any) -> None: ...
    def pre_put(self, sender: type[TypeDBType], instance: Any) -> None: ...
    def post_put(self, sender: type[TypeDBType], instance: Any) -> None: ...


class HookRunner:
    """Manages hook registration and execution.

    Pre-hooks run in registration order.
    Post-hooks run in reverse registration order (middleware unwinding).
    """

    __slots__ = ("_hooks",)

    def __init__(self) -> None:
        self._hooks: list[Any] = []

    @property
    def has_hooks(self) -> bool:
        """Fast guard — skip all hook logic when the list is empty."""
        return len(self._hooks) > 0

    def add(self, hook: Any) -> None:
        """Register a hook."""
        self._hooks.append(hook)

    def remove(self, hook: Any) -> None:
        """Unregister a hook.

        Raises ``ValueError`` if the hook is not registered.
        """
        self._hooks.remove(hook)

    def run_pre(self, event: CrudEvent, sender: type, instance: Any) -> None:
        """Run pre-hooks in registration order.

        Raises ``HookCancelled`` if any hook cancels the operation.
        """
        method_name = event.value  # e.g. "pre_insert"
        for hook in self._hooks:
            if not self._should_run(hook, event, sender):
                continue
            method = getattr(hook, method_name, None)
            if method is not None:
                try:
                    method(sender, instance)
                except HookCancelled as exc:
                    # Enrich with context if not already set
                    if exc.event is None:
                        exc.event = event
                    if exc.hook is None:
                        exc.hook = hook
                    raise

    def run_post(self, event: CrudEvent, sender: type, instance: Any) -> None:
        """Run post-hooks in reverse registration order.

        Errors are logged but do **not** propagate.
        """
        method_name = event.value  # e.g. "post_insert"
        for hook in reversed(self._hooks):
            if not self._should_run(hook, event, sender):
                continue
            method = getattr(hook, method_name, None)
            if method is not None:
                try:
                    method(sender, instance)
                except Exception:
                    logger.exception(
                        "Post-hook %r failed for %s on %s",
                        hook,
                        event.value,
                        sender.__name__,
                    )

    @staticmethod
    def _should_run(hook: Any, event: CrudEvent, sender: type) -> bool:
        """Check if *hook* wants to run for this event/sender combination."""
        should_run_method = getattr(hook, "should_run", None)
        if should_run_method is not None:
            return should_run_method(event, sender)
        return True
