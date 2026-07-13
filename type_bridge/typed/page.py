"""Immutable result envelopes for canonical typed-query pages."""

from __future__ import annotations

from collections.abc import Iterable
from dataclasses import dataclass


@dataclass(frozen=True, slots=True, init=False)
class Page[T]:
    """One bounded page of distinct, fully validated root results.

    ``items`` is defensively converted to a tuple.  The native executor is the
    authority for root identity, ordering, hydration, and totals; this wrapper
    only preserves those already-validated values as an immutable public
    envelope.
    """

    items: tuple[T, ...]
    offset: int
    limit: int
    total: int | None = None

    def __init__(
        self,
        items: Iterable[T],
        offset: int,
        limit: int,
        total: int | None = None,
    ) -> None:
        if isinstance(offset, bool) or not isinstance(offset, int):
            raise TypeError("page offset must be an integer")
        if offset < 0:
            raise ValueError("page offset must be non-negative")
        if isinstance(limit, bool) or not isinstance(limit, int):
            raise TypeError("page limit must be an integer")
        if limit <= 0:
            raise ValueError("page limit must be positive")
        if total is not None:
            if isinstance(total, bool) or not isinstance(total, int):
                raise TypeError("page total must be an integer or None")
            if total < 0:
                raise ValueError("page total must be non-negative")
        object.__setattr__(self, "items", tuple(items))
        object.__setattr__(self, "offset", offset)
        object.__setattr__(self, "limit", limit)
        object.__setattr__(self, "total", total)
