"""Returning deletion hydrates stored data and makes snapshots read-only."""

from typing import Any, cast

import pytest

from tests.utils.handwritten import Entity, Flag, Key, String, TypeFlags
from type_bridge.crud.rust_manager import _QueryRustTypeDBManager


class SnapshotName(String):
    pass


class SnapshotPerson(Entity):
    flags = TypeFlags(name="delete_snapshot_person")
    name: SnapshotName = Flag(Key)


class NativeManager:
    def __init__(self) -> None:
        self.calls: list[list[str]] = []

    def delete_many_returning(self, iids: list[str]) -> list[dict[str, Any]]:
        self.calls.append(iids)
        return [{"_iid": "0x10", "_type": "delete_snapshot_person", "name": "stored"}]


def test_returning_delete_uses_stored_fields_and_refuses_later_mutation() -> None:
    manager = _QueryRustTypeDBManager(cast(Any, object()), SnapshotPerson)
    native = NativeManager()
    manager._manager_instance = native
    original = SnapshotPerson(name=SnapshotName("caller"))
    original._set_backend_iid("0x10")

    snapshot = manager.delete_returning(original)

    assert snapshot is not original
    assert snapshot.name.value == "stored"
    assert snapshot.deleted
    assert not original.deleted
    assert native.calls == [["0x10"]]
    with pytest.raises(ValueError, match="thing_deleted"):
        snapshot.name = SnapshotName("changed")
    with pytest.raises(ValueError, match="thing_deleted"):
        manager.delete(snapshot)
    with pytest.raises(ValueError, match="thing_deleted"):
        manager.update(snapshot)
    assert native.calls == [["0x10"]]


def test_marking_a_deleted_root_does_not_mark_other_model_values() -> None:
    root = SnapshotPerson(name=SnapshotName("root"))
    player = SnapshotPerson(name=SnapshotName("player"))
    root._mark_deleted()
    player.name = SnapshotName("still live")
    assert root.deleted
    assert not player.deleted
