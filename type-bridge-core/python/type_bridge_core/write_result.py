"""Complete results of single-input and batch manager writes."""

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


@dataclass(frozen=True, slots=True)
class BatchWriteResult[T]:
    """Every batch root and its original input row.

    Missing inputs contribute no items. Several roots from one input repeat
    its index in ``input_indexes``. The SDK keeps both lists parallel and
    orders items by input row while preserving each input's answer order.
    """

    items: list[T]
    input_indexes: list[int]

    @property
    def count(self) -> int:
        """Exact number of root things written."""
        return len(self.items)
