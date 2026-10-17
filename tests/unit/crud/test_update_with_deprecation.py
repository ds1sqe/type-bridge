"""``update_with`` warns once per call and still updates every match."""

from __future__ import annotations

import warnings
from typing import Any

import pytest

from type_bridge.crud.rust_manager import RustTypeDBQuery


class _Row:
    def __init__(self, value: int) -> None:
        self.value = value


class _Manager:
    """Stands in for the manager: ``execute`` is replaced on the query."""

    def __init__(self) -> None:
        self.updated: list[_Row] = []

    def update(self, instance: _Row) -> _Row:
        self.updated.append(instance)
        return instance


def _query(manager: _Manager, rows: list[_Row]) -> RustTypeDBQuery[Any]:
    query = RustTypeDBQuery(manager, {})  # type: ignore[arg-type]
    query.execute = lambda: rows  # type: ignore[method-assign]
    return query


def test_update_with_warns_and_names_the_replacement() -> None:
    manager = _Manager()
    rows = [_Row(1), _Row(2)]

    with pytest.warns(DeprecationWarning, match=r"removed in 3\.0.*let_\(\) in an update plan"):
        updated = _query(manager, rows).update_with(lambda row: setattr(row, "value", 10))

    assert updated == rows
    assert manager.updated == rows
    assert [row.value for row in rows] == [10, 10]


def test_update_with_warning_points_at_the_caller() -> None:
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        _query(_Manager(), []).update_with(lambda row: None)

    assert [w.category for w in caught] == [DeprecationWarning]
    assert caught[0].filename == __file__
