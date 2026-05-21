from __future__ import annotations

# pyright: reportMissingImports=false
from typing import Any

import pytest

from type_bridge import Entity, Flag, Key, String, TypeFlags
from type_bridge.crud.rust_manager import RustTypeDBManager
from type_bridge.session import Database


class RustManagerName(String):
    pass


class RustManagerPerson(Entity):
    flags = TypeFlags(name="rust-manager-person")

    name: RustManagerName = Flag(Key)


class FakeRustEntityManager:
    def __init__(self) -> None:
        self.inserted: dict[str, Any] | None = None
        self.deleted: str | None = None

    def insert(self, attributes: dict[str, Any]) -> str:
        self.inserted = attributes
        return "0xabc"

    def get(self, filters: dict[str, Any]) -> list[dict[str, Any]]:
        return [{"name": "Alice", "_iid": "0xabc", "_type": "rust-manager-person"}]

    def all(self) -> list[dict[str, Any]]:
        return self.get({})

    def count(self, filters: dict[str, Any]) -> int:
        return 1

    def delete_by_iid(self, iid: str) -> None:
        self.deleted = iid


def test_backend_iid_setter_uses_private_storage() -> None:
    person = RustManagerPerson(name=RustManagerName("Alice"))

    person._set_backend_iid("0xabc")

    assert person._iid == "0xabc"
    assert person.model_dump() == {"name": "Alice"}


def test_rust_manager_marshals_entity_insert_get_count_delete(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake = FakeRustEntityManager()
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.register_model_descriptor",
        lambda model_class: {"type_name": model_class.get_type_name(), "owned_attributes": []},
    )
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.rust_manager_for_entity",
        lambda connection, descriptor: fake,
    )

    manager = RustTypeDBManager(Database(), RustManagerPerson)
    person = RustManagerPerson(name=RustManagerName("Alice"))

    inserted = manager.insert(person)
    fetched = manager.get(name=RustManagerName("Alice"))
    count = manager.count(name="Alice")
    manager.delete(person)

    assert inserted is person
    assert person._iid == "0xabc"
    assert fake.inserted == {"name": "Alice"}
    assert fetched[0].name == RustManagerName("Alice")
    assert fetched[0]._iid == "0xabc"
    assert count == 1
    assert fake.deleted == "0xabc"


def test_rust_manager_reports_unsupported_methods(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.register_model_descriptor",
        lambda model_class: {"type_name": model_class.get_type_name(), "owned_attributes": []},
    )
    monkeypatch.setattr(
        "type_bridge.crud.rust_manager.rust_manager_for_entity",
        lambda connection, descriptor: FakeRustEntityManager(),
    )
    manager = RustTypeDBManager(Database(), RustManagerPerson)

    with pytest.raises(NotImplementedError, match="Phase 2"):
        manager.update(RustManagerPerson(name=RustManagerName("Alice")))
