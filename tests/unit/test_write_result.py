"""Single-write results retain all roots without choosing one on fan-out."""

import pytest
from type_bridge_core import WriteResult as CoreWriteResult

from type_bridge import WriteResult


@pytest.mark.parametrize("items", [[], [object()], [object(), object()]])
def test_write_result_reports_all_items_and_only_a_single_item(items: list[object]) -> None:
    result = WriteResult(items)
    assert result.count == len(items)
    assert result.items is items
    assert result.item is (items[0] if len(items) == 1 else None)


def test_public_result_is_the_core_dto() -> None:
    assert WriteResult is CoreWriteResult
