"""Immutable typed-query page-envelope tests."""

from __future__ import annotations

from dataclasses import FrozenInstanceError
from typing import assert_type

import pytest

from tests.unit.typed_query._support import invoke_untyped
from type_bridge.typed.page import Page


def test_page_defensively_freezes_items_and_preserves_exact_type() -> None:
    source = ["first", "second"]
    page = Page(items=source, offset=2, limit=10, total=7)
    source.append("third")

    assert_type(page, Page[str])
    assert page.items == ("first", "second")
    assert page.total == 7

    with pytest.raises(FrozenInstanceError):
        setattr(page, "limit", 11)


@pytest.mark.parametrize(
    ("arguments", "error_type"),
    [
        ({"items": (), "offset": -1, "limit": 1}, ValueError),
        ({"items": (), "offset": 0, "limit": 0}, ValueError),
        ({"items": (), "offset": 0, "limit": 1, "total": -1}, ValueError),
        ({"items": (), "offset": True, "limit": 1}, TypeError),
        ({"items": (), "offset": 0, "limit": False}, TypeError),
    ],
)
def test_page_rejects_invalid_public_metadata(
    arguments: dict[str, object], error_type: type[Exception]
) -> None:
    with pytest.raises(error_type):
        invoke_untyped(Page, **arguments)
