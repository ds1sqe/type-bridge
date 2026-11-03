"""Complete results of single-input manager writes."""

from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class WriteResult[T]:
    """Every hydrated root written, in answer order.

    Non-key references can produce several roots. ``item`` is present only
    when exactly one root was written. Python attaches the input instance IID
    only in that case; fan-out leaves the input instance unchanged.
    """

    items: list[T]

    @property
    def count(self) -> int:
        """Exact number of root things written."""
        return len(self.items)

    @property
    def item(self) -> T | None:
        """The single root, or ``None`` for zero or several roots."""
        return self.items[0] if len(self.items) == 1 else None
