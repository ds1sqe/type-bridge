"""Experimental Rust-backed manager facade."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, Never, Self, cast

from type_bridge._rust_runtime import (
    normalize_attributes,
    register_model_descriptor,
    role_player_inputs,
    rust_manager_for_entity,
    rust_manager_for_relation,
)
from type_bridge.models import Entity, Relation

if TYPE_CHECKING:
    from type_bridge.models.base import TypeDBType
    from type_bridge.session import Connection


class RustTypeDBManager[T: "TypeDBType"]:
    """Experimental manager that delegates supported operations to Rust."""

    def __init__(self, connection: Connection, model_class: type[T]):
        self._connection = connection
        self.model_class = model_class
        self._descriptor = register_model_descriptor(model_class)
        if issubclass(model_class, Entity):
            self._kind = "entity"
            self._manager = rust_manager_for_entity(connection, self._descriptor)
        elif issubclass(model_class, Relation):
            self._kind = "relation"
            self._manager = rust_manager_for_relation(connection, self._descriptor)
        else:
            raise TypeError(f"Unsupported model type: {model_class}")

    def add_hook(self, hook: Any) -> Self:
        self._unsupported("add_hook")

    def remove_hook(self, hook: Any) -> None:
        self._unsupported("remove_hook")

    def insert(self, instance: T) -> T:
        if isinstance(instance, Entity):
            attributes = normalize_attributes(self.model_class, instance.to_dict())
            iid = self._manager.insert(attributes)
            instance._set_backend_iid(iid)
            return instance

        if isinstance(instance, Relation):
            attributes = normalize_attributes(self.model_class, _relation_attribute_dict(instance))
            iid = self._manager.insert(attributes, role_player_inputs(instance))
            instance._set_backend_iid(iid)
            return instance

        raise TypeError(f"Unsupported instance type: {type(instance)}")

    def get(self, **filters: Any) -> list[T]:
        if self._kind != "entity":
            self._unsupported("relation get")
        normalized = normalize_attributes(self.model_class, filters)
        rows = self._manager.get(normalized)
        return [self._hydrate_entity(row) for row in rows]

    def all(self) -> list[T]:
        if self._kind != "entity":
            self._unsupported("relation all")
        rows = self._manager.all()
        return [self._hydrate_entity(row) for row in rows]

    def count(self, **filters: Any) -> int:
        normalized = normalize_attributes(self.model_class, filters)
        return int(self._manager.count(normalized))

    def delete(self, instance: T) -> T:
        iid = getattr(instance, "_iid", None)
        if not iid:
            raise ValueError("Rust backend delete requires an instance with _iid populated")
        self._manager.delete_by_iid(iid)
        return instance

    def update(self, instance: T) -> T:
        self._unsupported("update")

    def put(self, instance: T) -> T:
        self._unsupported("put")

    def insert_many(self, instances: list[T]) -> list[T]:
        self._unsupported("insert_many")

    def put_many(self, instances: list[T]) -> list[T]:
        self._unsupported("put_many")

    def filter(self, **filters: Any) -> Any:
        self._unsupported("filter")

    def get_by_iid(self, iid: str) -> T | None:
        self._unsupported("get_by_iid")

    def _hydrate_entity(self, row: dict[str, Any]) -> T:
        from type_bridge.crud.role_players import resolve_entity_class_from_label
        from type_bridge.models.registry import ModelRegistry

        row = dict(row)
        iid = row.pop("_iid", None)
        type_label = row.pop("_type", None)

        concrete_class: type[Any] | None = self.model_class
        if type_label and type_label != self.model_class.get_type_name():
            concrete_class = ModelRegistry.get(type_label)
            if concrete_class is None:
                concrete_class = resolve_entity_class_from_label(
                    type_label,
                    cast(tuple[type[Entity], ...], (self.model_class,)),
                )
        if concrete_class is None:
            raise ValueError(f"Could not resolve concrete type {type_label!r}")

        instance = concrete_class.from_dict(row, strict=False)
        instance._set_backend_iid(iid)
        return cast(T, instance)

    def _unsupported(self, method: str) -> Never:
        raise NotImplementedError(f"TYPE_BRIDGE_BACKEND=rust does not support {method} in Phase 2")


def _relation_attribute_dict(instance: Relation) -> dict[str, Any]:
    data: dict[str, Any] = {}
    for field_name in instance.__class__.get_all_attributes():
        data[field_name] = getattr(instance, field_name, None)
    return data
