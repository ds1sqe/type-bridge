"""Rust-backed manager facade."""

from __future__ import annotations

import re
from typing import TYPE_CHECKING, Any, Never, Protocol, Self, cast

from type_bridge._rust_runtime import (
    key_filter_for_entity,
    normalize_attributes,
    normalize_value,
    register_model_descriptor,
    relation_role_fields,
    role_player_inputs,
    rust_core,
    rust_manager_for_entity,
    rust_manager_for_relation,
    rust_value_type,
)
from type_bridge.crud.hooks import CrudEvent, HookRunner
from type_bridge.models import Entity, Relation

if TYPE_CHECKING:
    from type_bridge.models.base import TypeDBType
    from type_bridge.session import Connection


class RustTypeDBManager[T: "TypeDBType"]:
    """Manager that delegates CRUD and query execution to Rust."""

    def __init__(self, connection: Connection, model_class: type[T]):
        self._connection = connection
        self.model_class = model_class
        self._descriptor = register_model_descriptor(model_class)
        self._manager_instance: Any | None = None
        if issubclass(model_class, Entity):
            self._kind = "entity"
        elif issubclass(model_class, Relation):
            self._kind = "relation"
        else:
            raise TypeError(f"Unsupported model type: {model_class}")
        self._hook_runner = HookRunner()

    @property
    def _manager(self) -> Any:
        # A database-backed native manager owns an Arc to the current Rust
        # connection.  Keeping that adapter here would outlive
        # ``Database.close()`` and, after a context-manager reconnect, route
        # released CRUD manager objects back to the terminal connection.  Build
        # database adapters per operation so close can release its final lease
        # and a later operation resolves the replacement connection.  Borrowed
        # transaction adapters remain cached because they must stay pinned to
        # exactly one transaction.  A non-None value assigned explicitly (the
        # frozen compatibility probe uses this seam) remains authoritative.
        if self._manager_instance is not None:
            return self._manager_instance

        from type_bridge.session import Database

        if self._kind == "entity":
            manager = rust_manager_for_entity(self._connection, self._descriptor)
        else:
            manager = rust_manager_for_relation(self._connection, self._descriptor)

        if isinstance(self._connection, Database):
            return manager
        self._manager_instance = manager
        return manager

    def add_hook(self, hook: Any) -> Self:
        self._hook_runner.add(hook)
        return self

    def remove_hook(self, hook: Any) -> None:
        self._hook_runner.remove(hook)

    def insert(self, instance: T) -> T:
        self._run_pre(CrudEvent.PRE_INSERT, instance)
        if isinstance(instance, Entity):
            attributes = normalize_attributes(self.model_class, instance.to_dict())
            iid = _rust_call(self._manager.insert, attributes)
            instance._set_backend_iid(iid)
            self._run_post(CrudEvent.POST_INSERT, instance)
            return instance

        if isinstance(instance, Relation):
            attributes = normalize_attributes(self.model_class, _relation_attribute_dict(instance))
            iid = _rust_call(self._manager.insert, attributes, role_player_inputs(instance))
            instance._set_backend_iid(iid)
            self._run_post(CrudEvent.POST_INSERT, instance)
            return instance

        raise TypeError(f"Unsupported instance type: {type(instance)}")

    def get(self, **filters: Any) -> list[T]:
        return RustTypeDBQuery(self, filters).execute()

    def _get_with_filter_input(self, filters: Any) -> list[T]:
        rows = self._manager.get(filters)
        if self._kind == "relation":
            return self._hydrate_relation_rows(rows)
        return [self._hydrate_entity(row) for row in rows]

    def all(self) -> list[T]:
        return RustTypeDBQuery(self, {}).execute()

    def count(self, **filters: Any) -> int:
        return RustTypeDBQuery(self, filters).count()

    def delete(self, instance: T) -> T:
        iid = self._resolve_delete_iid(instance)
        if iid is None:
            return instance
        self._run_pre(CrudEvent.PRE_DELETE, instance)
        self._manager.delete_by_iid(iid)
        self._run_post(CrudEvent.POST_DELETE, instance)
        return instance

    def delete_many(self, instances: list[T], *, strict: bool = False) -> list[T]:
        if not instances:
            return []

        resolved: list[tuple[T, str]] = []
        missing: list[T] = []
        for instance in instances:
            iid = self._resolve_delete_iid(instance)
            if iid is None:
                missing.append(instance)
            else:
                resolved.append((instance, iid))

        if strict and missing:
            from type_bridge.crud.exceptions import EntityNotFoundError

            raise EntityNotFoundError("entity(ies) not found")

        self._run_pre_many(CrudEvent.PRE_DELETE, [instance for instance, _ in resolved])
        for _, iid in resolved:
            self._manager.delete_by_iid(iid)
        self._run_post_many(CrudEvent.POST_DELETE, [instance for instance, _ in resolved])
        return [instance for instance, _ in resolved]

    def update(self, instance: T) -> T:
        self._run_pre(CrudEvent.PRE_UPDATE, instance)
        if isinstance(instance, Entity):
            attributes = normalize_attributes(self.model_class, instance.to_dict())
            clear_attrs = _clear_attribute_names(self.model_class, instance)
            if _has_non_key_update_attributes(self.model_class, attributes):
                _rust_call(self._manager.update, attributes, getattr(instance, "_iid", None))
            if clear_attrs:
                self._clear_attributes(instance, clear_attrs, "$e")
            self._run_post(CrudEvent.POST_UPDATE, instance)
            return instance

        if isinstance(instance, Relation):
            attributes = normalize_attributes(self.model_class, _relation_attribute_dict(instance))
            clear_attrs = _clear_attribute_names(self.model_class, instance)
            if _has_non_key_update_attributes(self.model_class, attributes):
                _rust_call(
                    self._manager.update,
                    attributes,
                    role_player_inputs(instance),
                    getattr(instance, "_iid", None),
                )
            if clear_attrs:
                self._clear_attributes(instance, clear_attrs, "$r")
            self._run_post(CrudEvent.POST_UPDATE, instance)
            return instance

        raise TypeError(f"Unsupported instance type: {type(instance)}")

    def put(self, instance: T) -> T:
        self._run_pre(CrudEvent.PRE_PUT, instance)
        if isinstance(instance, Entity):
            attributes = normalize_attributes(self.model_class, instance.to_dict())
            iid = _rust_call(self._manager.put, attributes)
            instance._set_backend_iid(iid)
            self._run_post(CrudEvent.POST_PUT, instance)
            return instance

        if isinstance(instance, Relation):
            attributes = normalize_attributes(self.model_class, _relation_attribute_dict(instance))
            iid = _rust_call(self._manager.put, attributes, role_player_inputs(instance))
            instance._set_backend_iid(iid)
            self._run_post(CrudEvent.POST_PUT, instance)
            return instance

        raise TypeError(f"Unsupported instance type: {type(instance)}")

    def insert_many(self, instances: list[T]) -> list[T]:
        if not instances:
            return instances

        if all(isinstance(instance, Entity) for instance in instances):
            self._run_pre_many(CrudEvent.PRE_INSERT, instances)
            attributes = [
                normalize_attributes(self.model_class, cast(Entity, instance).to_dict())
                for instance in instances
            ]
            iids = _rust_call(self._manager.insert_many, attributes)
            _set_iids(instances, iids)
            self._run_post_many(CrudEvent.POST_INSERT, instances)
            return instances

        if all(isinstance(instance, Relation) for instance in instances):
            self._run_pre_many(CrudEvent.PRE_INSERT, instances)
            items = [
                _relation_write_item(cast(Relation, instance), self.model_class)
                for instance in instances
            ]
            iids = _rust_call(self._manager.insert_many, items)
            _set_iids(instances, iids)
            self._run_post_many(CrudEvent.POST_INSERT, instances)
            return instances

        raise TypeError("Rust backend insert_many requires all instances to share one model kind")

    def put_many(self, instances: list[T]) -> list[T]:
        if not instances:
            return instances

        if all(isinstance(instance, Entity) for instance in instances):
            self._run_pre_many(CrudEvent.PRE_PUT, instances)
            attributes = [
                normalize_attributes(self.model_class, cast(Entity, instance).to_dict())
                for instance in instances
            ]
            iids = _rust_call(self._manager.put_many, attributes)
            _set_iids(instances, iids)
            self._run_post_many(CrudEvent.POST_PUT, instances)
            return instances

        if all(isinstance(instance, Relation) for instance in instances):
            self._run_pre_many(CrudEvent.PRE_PUT, instances)
            items = [
                _relation_write_item(cast(Relation, instance), self.model_class)
                for instance in instances
            ]
            iids = _rust_call(self._manager.put_many, items)
            _set_iids(instances, iids)
            self._run_post_many(CrudEvent.POST_PUT, instances)
            return instances

        raise TypeError("Rust backend put_many requires all instances to share one model kind")

    def update_many(self, instances: list[T]) -> list[T]:
        for instance in instances:
            self.update(instance)
        return instances

    def filter(self, *expressions: Any, **filters: Any) -> RustTypeDBQuery[T]:
        _validate_filter_expressions(self.model_class, self._kind, expressions)
        return RustTypeDBQuery(self, filters, expressions)

    def group_by(self, *fields: Any) -> RustTypeDBGroupByQuery[T]:
        return RustTypeDBQuery(self, {}).group_by(*fields)

    def get_by_iid(self, iid: str) -> T | None:
        if not iid or not re.match(r"^0x[0-9a-fA-F]+$", iid):
            return None

        matches = self.get(_iid=iid)
        return matches[0] if matches else None

    def _resolve_delete_iid(self, instance: T) -> str | None:
        iid = getattr(instance, "_iid", None)
        if iid:
            return str(iid)

        if isinstance(instance, Entity):
            filters = _key_filters_for_entity(instance)
            matches = self.get(**filters)
            if not matches:
                return None
            matched_iid = getattr(matches[0], "_iid", None)
            if matched_iid:
                return str(matched_iid)
            return None

        if isinstance(instance, Relation):
            filters = _relation_instance_match_filters(instance)
            matches = self.get(**filters)
            if not matches:
                return None
            matched_iid = getattr(matches[0], "_iid", None)
            if matched_iid:
                return str(matched_iid)
            return None

        raise ValueError(f"Unsupported instance type: {type(instance)}")

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

    def _hydrate_relation_rows(self, rows: list[dict[str, Any]]) -> list[T]:
        grouped: dict[str, list[dict[str, Any]]] = {}
        for index, row in enumerate(rows):
            iid = row.get("_iid")
            key = str(iid) if iid else f"__row_{index}"
            grouped.setdefault(key, []).append(row)

        hydrated = []
        for relation_iid, group in grouped.items():
            hydrated.append(self._hydrate_relation_group(relation_iid, group))
        return hydrated

    def _hydrate_relation_group(self, relation_iid: str, rows: list[dict[str, Any]]) -> T:
        from type_bridge.crud.role_players import extract_relation_attributes

        relation_model = cast(type[Relation], self.model_class)
        relation_attrs = extract_relation_attributes(relation_model, rows[0])
        role_players = self._hydrate_relation_role_players(rows)
        relation = relation_model(**{**relation_attrs, **role_players})
        relation._set_backend_iid(relation_iid if not relation_iid.startswith("__row_") else None)
        return cast(T, relation)

    def _hydrate_relation_role_players(self, rows: list[dict[str, Any]]) -> dict[str, Any]:
        role_fields = {
            role.role_name: (field_name, role)
            for field_name, role in relation_role_fields(self.model_class)
        }
        collected: dict[str, list[Any]] = {}
        seen: set[tuple[Any, ...]] = set()

        for row in rows:
            for player in row.get("role_players", []):
                if not isinstance(player, dict):
                    continue
                role_name = player.get("role_name")
                if role_name not in role_fields:
                    continue
                field_name, role = role_fields[role_name]
                player_instance = self._hydrate_role_player(player, role.player_types)
                player_key = _role_player_key(field_name, player_instance)
                if player_key in seen:
                    continue
                seen.add(player_key)
                collected.setdefault(field_name, []).append(player_instance)

        role_values: dict[str, Any] = {}
        for field_name, role in relation_role_fields(self.model_class):
            players = collected.get(field_name, [])
            if not players:
                continue
            if len(players) > 1 or _is_multi_player_role(role.cardinality):
                role_values[field_name] = players
            else:
                role_values[field_name] = players[0]
        return role_values

    def _hydrate_role_player(
        self, player: dict[str, Any], allowed_types: tuple[type[Any], ...]
    ) -> Any:
        from type_bridge.crud.role_players import resolve_entity_class_from_label
        from type_bridge.crud.types import hydrate_attributes

        type_label = player.get("player_type_name")
        player_class = resolve_entity_class_from_label(type_label, allowed_types)
        attrs, _ = hydrate_attributes(
            cast(type[Any], player_class),
            player.get("attributes", {}),
            wrap_values=True,
        )
        if issubclass(player_class, Relation):
            instance = player_class.model_construct(**attrs)
        else:
            instance = player_class(**attrs)
        instance._set_backend_iid(player.get("player_iid"))
        return instance

    def _unsupported(self, method: str) -> Never:
        raise NotImplementedError(f"TYPE_BRIDGE_BACKEND=rust does not support {method} in Phase 3")

    def _run_pre(self, event: CrudEvent, instance: T) -> None:
        if self._hook_runner.has_hooks:
            self._hook_runner.run_pre(event, self.model_class, instance)

    def _run_post(self, event: CrudEvent, instance: T) -> None:
        if self._hook_runner.has_hooks:
            self._hook_runner.run_post(event, self.model_class, instance)

    def _run_pre_many(self, event: CrudEvent, instances: list[T]) -> None:
        if self._hook_runner.has_hooks:
            for instance in instances:
                self._hook_runner.run_pre(event, self.model_class, instance)

    def _run_post_many(self, event: CrudEvent, instances: list[T]) -> None:
        if self._hook_runner.has_hooks:
            for instance in instances:
                self._hook_runner.run_post(event, self.model_class, instance)

    def _clear_attributes(self, instance: T, attr_names: list[str], var: str) -> None:
        iid = getattr(instance, "_iid", None)
        if iid:
            match_line = f"{var} isa {self.model_class.get_type_name()}, iid {iid};"
        elif isinstance(instance, Entity):
            filters = _key_filters_for_entity(instance)
            expressions = _entity_filter_expressions(self.model_class, filters)
            lowered = [
                _lower_expression(self.model_class, self._kind, expression)
                for expression in expressions
            ]
            rows = self._manager.get_with_query(lowered, [], 1, None)
            if not rows:
                return
            iid = rows[0].get("_iid")
            if not iid:
                return
            match_line = f"{var} isa {self.model_class.get_type_name()}, iid {iid};"
        elif isinstance(instance, Relation):
            iid = self._resolve_delete_iid(instance)
            if not iid:
                return
            match_line = f"{var} isa {self.model_class.get_type_name()}, iid {iid};"
        else:
            return

        match_parts = [f"match {match_line}"]
        delete_parts = []
        for index, attr_name in enumerate(attr_names):
            old_var = f"$old_attr_{index}"
            match_parts.append(
                f"try {{ {var} has {attr_name} {old_var}; {old_var} isa {attr_name}; }};"
            )
            delete_parts.append(f"try {{ {old_var} of {var}; }};")
        query = "\n".join(match_parts) + "\ndelete\n" + "\n".join(delete_parts)
        _execute_write_query(self._connection, query)


class _RustQueryManager[T: "TypeDBType"](Protocol):
    """Structural manager surface consumed by legacy query objects."""

    @property
    def model_class(self) -> type[T]: ...

    @property
    def _kind(self) -> str: ...

    @property
    def _manager(self) -> Any: ...

    def _hydrate_entity(self, row: dict[str, Any]) -> T: ...

    def _hydrate_relation_rows(self, rows: list[dict[str, Any]]) -> list[T]: ...

    def delete(self, instance: T) -> T: ...

    def update(self, instance: T) -> T: ...


class RustTypeDBQuery[T: "TypeDBType"]:
    """Rust-backed chainable query using the typed dynamic query surface."""

    def __init__(
        self,
        manager: _RustQueryManager[T],
        filters: dict[str, Any],
        expressions: tuple[Any, ...] = (),
    ):
        self._manager = manager
        self._filters = dict(filters)
        self._expressions = list(expressions)
        self._order_fields: list[tuple[str, bool]] = []
        self._limit_value: int | None = None
        self._offset_value: int | None = None

    def filter(self, *expressions: Any, **filters: Any) -> Self:
        _validate_filter_expressions(
            self._manager.model_class,
            self._manager._kind,
            expressions,
        )
        self._expressions.extend(expressions)
        self._filters.update(filters)
        return self

    def limit(self, n: int) -> Self:
        self._limit_value = n
        return self

    def offset(self, n: int) -> Self:
        self._offset_value = n
        return self

    def order_by(self, *fields: str) -> Self:
        for field in fields:
            if field.startswith("-"):
                self._order_fields.append((field[1:], True))
            else:
                self._order_fields.append((field, False))
        return self

    def execute(self) -> list[T]:
        expressions = _query_expressions(
            self._manager.model_class,
            self._manager._kind,
            self._filters,
            self._expressions,
        )
        sorts = _query_sorts(
            self._manager.model_class,
            self._manager._kind,
            self._order_fields,
        )
        rows = self._manager._manager.get_with_query(
            expressions,
            sorts,
            self._limit_value,
            self._offset_value,
        )
        if self._manager._kind == "relation":
            return self._manager._hydrate_relation_rows(rows)
        return [self._manager._hydrate_entity(row) for row in rows]

    def _execute_uncapped(self) -> list[T]:
        return self.execute()

    def all(self) -> list[T]:
        return self.execute()

    def first(self) -> T | None:
        original_limit = self._limit_value
        self._limit_value = 1
        results = self.execute()
        self._limit_value = original_limit
        return results[0] if results else None

    def count(self) -> int:
        expressions = _query_expressions(
            self._manager.model_class,
            self._manager._kind,
            self._filters,
            self._expressions,
        )
        return int(self._manager._manager.count_with_query(expressions))

    def delete(self) -> int:
        instances = self.execute()
        for instance in instances:
            self._manager.delete(instance)
        return len(instances)

    def exists(self) -> bool:
        return self.count() > 0

    def update_with(self, func: Any) -> list[T]:
        instances = self.execute()
        if not instances:
            return []

        for instance in instances:
            func(instance)

        for instance in instances:
            self._manager.update(instance)
        return instances

    def aggregate(self, *aggregates: Any) -> dict[str, Any]:
        specs = _aggregate_specs(self._manager.model_class, aggregates)
        filters = _filter_input(self._manager.model_class, self._filters, self._expressions)
        rows = self._manager._manager.aggregate(specs, filters)
        normalized = _normalize_reduce_rows(rows)
        return normalized[0] if normalized else {}

    def group_by(self, *fields: Any) -> RustTypeDBGroupByQuery[T]:
        group_fields = _group_field_names(self._manager.model_class, fields)
        return RustTypeDBGroupByQuery(
            self._manager,
            self._filters,
            self._expressions,
            group_fields,
        )


class RustTypeDBGroupByQuery[T: "TypeDBType"]:
    """Rust-backed grouped aggregation query for exact-match Phase 3 filters."""

    def __init__(
        self,
        manager: _RustQueryManager[T],
        filters: dict[str, Any],
        expressions: list[Any],
        group_fields: list[str],
    ):
        self._manager = manager
        self._filters = dict(filters)
        self._expressions = list(expressions)
        self._group_fields = group_fields

    def aggregate(self, *aggregates: Any) -> dict[Any, dict[str, Any]]:
        specs = _aggregate_specs(self._manager.model_class, aggregates)
        filters = _filter_input(self._manager.model_class, self._filters, self._expressions)
        rows = self._manager._manager.group_by_aggregate(
            self._group_fields,
            specs,
            filters,
        )
        return _normalize_grouped_reduce_rows(rows, len(self._group_fields))


def _relation_attribute_dict(instance: Relation) -> dict[str, Any]:
    data: dict[str, Any] = {}
    for field_name in instance.__class__.get_all_attributes():
        data[field_name] = getattr(instance, field_name, None)
    return data


def _relation_write_item(instance: Relation, model_class: type[Any]) -> dict[str, Any]:
    return {
        "attributes": normalize_attributes(model_class, _relation_attribute_dict(instance)),
        "role_players": role_player_inputs(instance),
    }


def _relation_instance_match_filters(instance: Relation) -> dict[str, Any]:
    filters: dict[str, Any] = {}
    for field_name in instance.__class__.get_all_attributes():
        value = getattr(instance, field_name, None)
        if value is not None:
            filters[field_name] = value

    for field_name, role in relation_role_fields(instance.__class__):
        value = getattr(instance, field_name, None)
        if value is None:
            if _is_required_role(role.cardinality):
                raise ValueError(f"Role player '{field_name}' is required for matching")
            continue
        filters[field_name] = value
    return filters


def _split_relation_filters(
    model_class: type[Any],
    filters: dict[str, Any],
) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    role_fields = {field_name: role for field_name, role in relation_role_fields(model_class)}
    attr_filters: dict[str, Any] = {}
    role_filters: list[dict[str, Any]] = []
    for field_name, value in filters.items():
        role = role_fields.get(field_name)
        if role is None:
            attr_filters[field_name] = value
            continue
        if value is None:
            continue
        players = value if isinstance(value, list) else [value]
        for player in players:
            player_type = player.__class__
            item = {
                "role_name": role.role_name,
                "player_type_name": player_type.get_type_name(),
            }
            iid = getattr(player, "_iid", None)
            if iid:
                item["iid"] = iid
            else:
                key = key_filter_for_entity(player)
                if key is None:
                    raise ValueError(
                        f"Role player for role '{role.role_name}' needs _iid or a key attribute"
                    )
                item.update(key)
            role_filters.append(item)
    return attr_filters, role_filters


def _has_relation_role_filters(model_class: type[Any], filters: dict[str, Any]) -> bool:
    role_fields = {field_name for field_name, _ in relation_role_fields(model_class)}
    return any(
        field_name in role_fields and value is not None for field_name, value in filters.items()
    )


def _key_filters_for_entity(instance: Entity) -> dict[str, Any]:
    from type_bridge.crud.exceptions import KeyAttributeError

    key_filters: dict[str, Any] = {}
    all_fields = list(instance.__class__.get_all_attributes())
    for field_name, attr_info in instance.__class__.get_all_attributes().items():
        if not attr_info.flags.is_key:
            continue
        value = getattr(instance, field_name, None)
        if value is None:
            raise KeyAttributeError(
                entity_type=instance.__class__.__name__,
                operation="delete",
                field_name=field_name,
            )
        key_filters[field_name] = value

    if not key_filters:
        raise ValueError(
            f"Entity '{instance.__class__.__name__}' cannot be identified: "
            "no _iid set and no @key attributes defined."
            f" Defined attributes: {all_fields}"
        )
    return key_filters


def _set_iids(instances: list[Any], iids: list[str]) -> None:
    if len(instances) != len(iids):
        raise ValueError(f"Rust backend returned {len(iids)} IIDs for {len(instances)} instances")
    for instance, iid in zip(instances, iids, strict=True):
        instance._set_backend_iid(iid)


def _is_multi_player_role(cardinality: Any) -> bool:
    if cardinality is None:
        return False
    max_value = getattr(cardinality, "max", None)
    return max_value is None or max_value > 1


def _is_required_role(cardinality: Any) -> bool:
    if cardinality is None:
        return True
    if isinstance(cardinality, (list, tuple)):
        min_value = cardinality[0] if cardinality else None
    else:
        min_value = getattr(cardinality, "min", None)
    return min_value is None or min_value > 0


def _role_player_key(field_name: str, player: Any) -> tuple[Any, ...]:
    iid = getattr(player, "_iid", None)
    if iid:
        return (field_name, "iid", iid)
    return (
        field_name,
        "dump",
        tuple(sorted(player.model_dump(mode="json").items())),
    )


def _validate_filter_expressions(
    model_class: type[Any],
    kind: str,
    expressions: tuple[Any, ...],
) -> None:
    for expression in expressions:
        _validate_filter_expression(model_class, kind, expression)


def _validate_filter_expression(
    model_class: type[Any],
    kind: str,
    expression: Any,
    *,
    role_player_types: tuple[type[Any], ...] | None = None,
) -> None:
    from type_bridge.expressions import BooleanExpr, RolePlayerExpr

    core = rust_core()
    if isinstance(expression, core.DynamicExpr):
        return

    if isinstance(expression, BooleanExpr):
        for operand in expression.operands:
            _validate_filter_expression(
                model_class,
                kind,
                operand,
                role_player_types=role_player_types,
            )
        return

    if isinstance(expression, RolePlayerExpr):
        if kind != "relation":
            raise ValueError("RolePlayerExpr can only be used with relation queries")
        role = _relation_role(model_class, expression.role_name)
        _validate_filter_expression(
            model_class,
            kind,
            expression.inner_expr,
            role_player_types=role.player_types,
        )
        return

    for attr_type in expression.get_attribute_types():
        if role_player_types is not None:
            if any(
                attr_info.typ is attr_type
                for player_type in role_player_types
                for attr_info in player_type.get_all_attributes().values()
            ):
                continue
            player_names = ", ".join(player.__name__ for player in role_player_types)
            raise ValueError(
                f"Role player types {player_names} do not own attribute type {attr_type.__name__}"
            )

        owned_attr_types = {
            attr_info.typ for attr_info in model_class.get_all_attributes().values()
        }
        if attr_type in owned_attr_types:
            continue
        available = ", ".join(sorted(attr_type.__name__ for attr_type in owned_attr_types))
        raise ValueError(
            f"{model_class.__name__} does not own attribute type {attr_type.__name__}. "
            f"Available attribute types: {available}"
        )


def _has_non_key_update_attributes(model_class: type[Any], attributes: dict[str, Any]) -> bool:
    owned_attrs = model_class.get_all_attributes()
    for field_name, value in attributes.items():
        attr_info = owned_attrs.get(field_name)
        if attr_info is None:
            continue
        if attr_info.flags.is_key:
            continue
        if isinstance(value, list) and not value:
            continue
        if value is None:
            continue
        return True
    return False


def _clear_attribute_names(model_class: type[Any], instance: Any) -> list[str]:
    attr_names: list[str] = []
    for field_name, attr_info in model_class.get_all_attributes().items():
        if attr_info.flags.is_key:
            continue
        value = getattr(instance, field_name, None)
        if value is None or (isinstance(value, list) and not value):
            attr_names.append(attr_info.typ.get_attribute_name())
    return attr_names


def _execute_write_query(connection: Any, query: str) -> None:
    execute_query = getattr(connection, "execute_query", None)
    try:
        if execute_query is not None:
            execute_query(query, "write")
            return
        connection.execute(query)
    except RuntimeError as exc:
        _raise_typedb_driver_exception(exc)


def _rust_call(func: Any, *args: Any, **kwargs: Any) -> Any:
    try:
        return func(*args, **kwargs)
    except RuntimeError as exc:
        _raise_typedb_driver_exception(exc)


def _raise_typedb_driver_exception(exc: RuntimeError) -> Never:
    try:
        import typedb.driver
    except ModuleNotFoundError:
        raise exc

    exception_class = getattr(typedb.driver, "TypeDBDriverException")
    raise exception_class(str(exc)) from exc


def _query_expressions(
    model_class: type[Any],
    kind: str,
    filters: dict[str, Any],
    expressions: list[Any],
) -> list[Any]:
    if kind == "relation":
        parsed_filters = _relation_filter_expressions(model_class, filters)
    else:
        parsed_filters = _entity_filter_expressions(model_class, filters)

    return [
        _lower_expression(model_class, kind, expression)
        for expression in [*parsed_filters, *expressions]
    ]


def _entity_filter_expressions(
    model_class: type[Any],
    filters: dict[str, Any],
) -> list[Any]:
    from type_bridge.crud.lookup import build_lookup_expression
    from type_bridge.expressions import BooleanExpr, Expression, IidExpr

    owned_attrs = model_class.get_all_attributes()
    expressions: list[Any] = []

    for raw_key, raw_value in filters.items():
        if raw_key in {"iid", "_iid"}:
            expressions.append(IidExpr(str(raw_value)))
            continue
        if raw_key in {"iid__in", "_iid__in"}:
            expressions.append(_iid_in_expression(raw_value))
            continue

        if "__" not in raw_key:
            attr_info = _owned_attr_info(model_class, raw_key)
            expressions.append(_exact_attr_expression(attr_info.typ, raw_value))
            continue

        field_name, lookup = raw_key.split("__", 1)
        if field_name not in owned_attrs:
            raise ValueError(f"Unknown filter field '{field_name}' for {model_class.__name__}")

        attr_type = owned_attrs[field_name].typ
        if lookup in ("exact", "eq"):
            expressions.append(_exact_attr_expression(attr_type, raw_value))
            continue

        expression = build_lookup_expression(attr_type, lookup, raw_value)
        if isinstance(expression, BooleanExpr):
            expressions.append(expression)
        else:
            typed_expr: Expression = expression
            expressions.append(typed_expr)

    return expressions


def _relation_filter_expressions(
    model_class: type[Any],
    filters: dict[str, Any],
) -> list[Any]:
    from type_bridge.crud.role_lookup import parse_role_lookup_filters
    from type_bridge.expressions import IidExpr
    from type_bridge.models import Relation

    expressions: list[Any] = []
    relation_filters: dict[str, Any] = {}
    for raw_key, raw_value in filters.items():
        if raw_key in {"iid", "_iid"}:
            expressions.append(IidExpr(str(raw_value)))
            continue
        if raw_key in {"iid__in", "_iid__in"}:
            expressions.append(_iid_in_expression(raw_value))
            continue
        relation_filters[raw_key] = raw_value

    attr_filters, role_filters, role_exprs, attr_exprs = parse_role_lookup_filters(
        cast(type[Relation], model_class),
        relation_filters,
    )

    for field_name, value in attr_filters.items():
        attr_info = _owned_attr_info(model_class, field_name)
        expressions.append(_exact_attr_expression(attr_info.typ, value))

    for role_name, value in role_filters.items():
        role = _relation_role(model_class, role_name)
        players = value if isinstance(value, list) else [value]
        for player in players:
            expressions.append(_role_player_instance_expression(role.role_name, player))

    expressions.extend(attr_exprs)
    for items in role_exprs.values():
        expressions.extend(items)
    return expressions


def _iid_in_expression(value: Any) -> Any:
    from type_bridge.expressions import BooleanExpr, Expression, IidExpr

    if not isinstance(value, (list, tuple, set)):
        raise ValueError("iid__in lookup requires an iterable of IID strings")
    iids = list(value)
    if not iids:
        raise ValueError("iid__in lookup requires a non-empty iterable")
    expressions: list[Expression] = [IidExpr(iid) for iid in iids]
    if len(expressions) == 1:
        return expressions[0]
    return BooleanExpr("or", expressions)


def _exact_attr_expression(attr_type: type[Any], value: Any) -> Any:
    from type_bridge.expressions import AttributeExistsExpr

    if value is None:
        return AttributeExistsExpr(attr_type, present=False)
    wrapped = value if isinstance(value, attr_type) else attr_type(value)
    return attr_type.eq(wrapped)


def _lower_expression(
    model_class: type[Any],
    kind: str,
    expression: Any,
    *,
    role_player_types: tuple[type[Any], ...] | None = None,
) -> Any:
    from type_bridge.expressions import (
        AttributeExistsExpr,
        BooleanExpr,
        ComparisonExpr,
        IidExpr,
        RolePlayerExpr,
        StringExpr,
    )

    core = rust_core()

    if isinstance(expression, core.DynamicExpr):
        return expression

    if isinstance(expression, ComparisonExpr):
        attr_info = _attr_info_for_expression(
            model_class,
            expression.attr_type,
            role_player_types=role_player_types,
        )
        raw_value = _raw_attr_value(expression.value)
        value = _dynamic_value(raw_value, attr_info.typ)
        attr_name = expression.attr_type.get_attribute_name()
        match expression.operator:
            case "==":
                return core.DynamicExpr.eq(attr_name, value)
            case "!=":
                return core.DynamicExpr.neq(attr_name, value)
            case ">":
                return core.DynamicExpr.gt(attr_name, value)
            case ">=":
                return core.DynamicExpr.gte(attr_name, value)
            case "<":
                return core.DynamicExpr.lt(attr_name, value)
            case "<=":
                return core.DynamicExpr.lte(attr_name, value)
        raise ValueError(f"Unsupported comparison operator {expression.operator!r}")

    if isinstance(expression, StringExpr):
        _attr_info_for_expression(
            model_class,
            expression.attr_type,
            role_player_types=role_player_types,
        )
        attr_name = expression.attr_type.get_attribute_name()
        pattern = str(_raw_attr_value(expression.pattern))
        if expression.operation == "contains":
            return core.DynamicExpr.contains(attr_name, pattern)
        if expression.operation in {"like", "regex"}:
            return core.DynamicExpr.like(attr_name, pattern)
        raise ValueError(f"Unsupported string operation {expression.operation!r}")

    if isinstance(expression, AttributeExistsExpr):
        _attr_info_for_expression(
            model_class,
            expression.attr_type,
            role_player_types=role_player_types,
        )
        attr_name = expression.attr_type.get_attribute_name()
        if expression.present:
            return core.DynamicExpr.is_not_null(attr_name)
        return core.DynamicExpr.is_null(attr_name)

    if isinstance(expression, IidExpr):
        return core.DynamicExpr.iid(expression.iid)

    if isinstance(expression, BooleanExpr):
        lowered = [
            _lower_expression(
                model_class,
                kind,
                operand,
                role_player_types=role_player_types,
            )
            for operand in expression.operands
        ]
        if expression.operation == "and":
            return core.DynamicExpr.and_(lowered)
        if expression.operation == "or":
            return core.DynamicExpr.or_(lowered)
        if expression.operation == "not":
            return core.DynamicExpr.not_(lowered[0])
        raise ValueError(f"Unsupported boolean operation {expression.operation!r}")

    if isinstance(expression, RolePlayerExpr):
        if kind != "relation":
            raise ValueError("RolePlayerExpr can only be used with relation queries")
        role = _relation_role(model_class, expression.role_name)
        inner = _lower_expression(
            model_class,
            kind,
            expression.inner_expr,
            role_player_types=role.player_types,
        )
        return core.DynamicExpr.role_player(role.role_name, inner)

    raise NotImplementedError(
        f"TYPE_BRIDGE_BACKEND=rust cannot lower expression {type(expression).__name__}"
    )


def _query_sorts(
    model_class: type[Any],
    kind: str,
    order_fields: list[tuple[str, bool]],
) -> list[Any]:
    core = rust_core()
    sorts: list[Any] = []
    for field_name, descending in order_fields:
        direction = core.DynamicSortDir.desc() if descending else core.DynamicSortDir.asc()
        if kind == "relation" and "__" in field_name:
            role_name, player_field = field_name.split("__", 1)
            role = _relation_role(model_class, role_name)
            attr_info = _role_player_attr_info(role.player_types, player_field)
            sorts.append(
                core.DynamicSort.role_player_attribute(
                    role.role_name,
                    attr_info.typ.get_attribute_name(),
                    direction,
                )
            )
            continue

        attr_info = _owned_attr_info(model_class, field_name)
        sorts.append(
            core.DynamicSort.attribute(
                attr_info.typ.get_attribute_name(),
                direction,
            )
        )
    return sorts


def _dynamic_value(raw_value: Any, attr_type: type[Any]) -> Any:
    core = rust_core()
    value_type = rust_value_type(attr_type)
    value = normalize_value(raw_value, value_type)
    if value_type == "string":
        return core.DynamicValue.string(str(value))
    if value_type == "long":
        return core.DynamicValue.long(int(value))
    if value_type == "double":
        return core.DynamicValue.double(float(value))
    if value_type == "boolean":
        return core.DynamicValue.boolean(_strict_bool(value))
    if value_type == "date":
        return core.DynamicValue.date(str(value))
    if value_type == "datetime":
        return core.DynamicValue.datetime(str(value))
    if value_type == "datetime-tz":
        return core.DynamicValue.datetime_tz(str(value))
    if value_type == "decimal":
        return core.DynamicValue.decimal(str(value))
    if value_type == "duration":
        return core.DynamicValue.duration(str(value))
    raise ValueError(f"Unsupported Rust dynamic value type {value_type!r}")


def _dynamic_value_from_rust_type(raw_value: Any, value_type: str) -> Any:
    core = rust_core()
    if value_type == "string":
        return core.DynamicValue.string(str(raw_value))
    if value_type == "long":
        return core.DynamicValue.long(int(raw_value))
    if value_type == "double":
        return core.DynamicValue.double(float(raw_value))
    if value_type == "boolean":
        return core.DynamicValue.boolean(_strict_bool(raw_value))
    if value_type == "date":
        return core.DynamicValue.date(str(raw_value))
    if value_type == "datetime":
        return core.DynamicValue.datetime(str(raw_value))
    if value_type == "datetime-tz":
        return core.DynamicValue.datetime_tz(str(raw_value))
    if value_type == "decimal":
        return core.DynamicValue.decimal(str(raw_value))
    if value_type == "duration":
        return core.DynamicValue.duration(str(raw_value))
    raise ValueError(f"Unsupported Rust dynamic value type {value_type!r}")


def _strict_bool(value: Any) -> bool:
    if isinstance(value, bool):
        return value
    raise TypeError(f"boolean values must be bool, got {type(value).__name__}")


def _raw_attr_value(value: Any) -> Any:
    return value.value if hasattr(value, "value") else value


def _owned_attr_info(model_class: type[Any], field_name: str) -> Any:
    attrs = model_class.get_all_attributes()
    attr_info = attrs.get(field_name)
    if attr_info is None:
        raise ValueError(f"Unknown filter field '{field_name}' for {model_class.__name__}")
    return attr_info


def _attr_info_for_expression(
    model_class: type[Any],
    attr_type: type[Any],
    *,
    role_player_types: tuple[type[Any], ...] | None = None,
) -> Any:
    if role_player_types is not None:
        for player_type in role_player_types:
            for attr_info in player_type.get_all_attributes().values():
                if attr_info.typ is attr_type:
                    return attr_info
        player_names = ", ".join(player.__name__ for player in role_player_types)
        raise ValueError(
            f"Role player types {player_names} do not own attribute type {attr_type.__name__}"
        )

    for attr_info in model_class.get_all_attributes().values():
        if attr_info.typ is attr_type:
            return attr_info
    raise ValueError(f"{model_class.__name__} does not own attribute type {attr_type.__name__}")


def _relation_role(model_class: type[Any], field_or_role_name: str) -> Any:
    for field_name, role in relation_role_fields(model_class):
        if field_or_role_name in {field_name, role.role_name}:
            return role
    available = [field_name for field_name, _ in relation_role_fields(model_class)]
    raise ValueError(
        f"Unknown role '{field_or_role_name}' for {model_class.__name__}. "
        f"Available roles: {available}"
    )


def _role_player_attr_info(
    player_types: tuple[type[Any], ...],
    field_name: str,
) -> Any:
    for player_type in player_types:
        attr_info = player_type.get_all_attributes().get(field_name)
        if attr_info is not None:
            return attr_info
    available = sorted(
        {
            attr_name
            for player_type in player_types
            for attr_name in player_type.get_all_attributes()
        }
    )
    raise ValueError(
        f"Role players do not have attribute '{field_name}'. Available attributes: {available}"
    )


def _role_player_instance_expression(role_name: str, player: Any) -> Any:
    core = rust_core()
    iid = getattr(player, "_iid", None)
    if iid:
        inner = core.DynamicExpr.iid(str(iid))
        return core.DynamicExpr.role_player(role_name, inner)

    key = key_filter_for_entity(player)
    if key is None:
        raise ValueError(f"Role player for role '{role_name}' needs _iid or a key attribute")
    value = _dynamic_value_from_rust_type(
        key["key_value"],
        key["key_value_type"],
    )
    inner = core.DynamicExpr.eq(key["key_attr"], value)
    return core.DynamicExpr.role_player(role_name, inner)


def _aggregate_specs(model_class: type[Any], aggregates: tuple[Any, ...]) -> list[dict[str, Any]]:
    from type_bridge.expressions import AggregateExpr

    if not aggregates:
        raise ValueError("At least one aggregation expression required")

    specs: list[dict[str, Any]] = []
    owned_attrs = model_class.get_all_attributes()
    owned_attr_types = {attr_info.typ for attr_info in owned_attrs.values()}
    for aggregate in aggregates:
        if not isinstance(aggregate, AggregateExpr):
            raise TypeError(f"Expected AggregateExpr, got {type(aggregate).__name__}")
        attr_name = None
        if aggregate.attr_type is not None:
            if aggregate.attr_type not in owned_attr_types:
                raise ValueError(
                    f"{model_class.__name__} does not own attribute type "
                    f"{aggregate.attr_type.__name__}"
                )
            attr_name = aggregate.attr_type.get_attribute_name()
        specs.append(
            {
                "result_key": aggregate.get_fetch_key(),
                "function": aggregate.function,
                "attr_name": attr_name,
            }
        )
    return specs


def _filter_input(
    model_class: type[Any],
    filters: dict[str, Any],
    expressions: list[Any],
) -> Any:
    if not expressions:
        return normalize_attributes(model_class, filters)

    specs = _exact_filter_specs(model_class, filters)
    specs.extend(_expression_filter_specs(model_class, expressions))
    return specs


def _exact_filter_specs(model_class: type[Any], filters: dict[str, Any]) -> list[dict[str, Any]]:
    normalized = normalize_attributes(model_class, filters)
    return [
        {"attr_name": attr_name, "operator": "==", "value": value}
        for attr_name, value in normalized.items()
    ]


def _expression_filter_specs(
    model_class: type[Any], expressions: list[Any]
) -> list[dict[str, Any]]:
    from type_bridge.expressions import ComparisonExpr

    owned_attrs = model_class.get_all_attributes()
    owned_attr_types = {attr_info.typ for attr_info in owned_attrs.values()}
    specs: list[dict[str, Any]] = []
    for expression in expressions:
        if not isinstance(expression, ComparisonExpr):
            raise NotImplementedError(
                "TYPE_BRIDGE_BACKEND=rust supports only comparison filter expressions in Phase 3"
            )
        if expression.attr_type not in owned_attr_types:
            raise ValueError(
                f"{model_class.__name__} does not own attribute type "
                f"{expression.attr_type.__name__}"
            )
        attr_info = next(
            attr_info for attr_info in owned_attrs.values() if attr_info.typ is expression.attr_type
        )
        raw_value = (
            expression.value.value if hasattr(expression.value, "value") else expression.value
        )
        value_type = attr_info.typ.get_value_type()
        specs.append(
            {
                "attr_name": expression.attr_type.get_attribute_name(),
                "operator": expression.operator,
                "value": normalize_value(raw_value, value_type),
            }
        )
    return specs


def _group_field_names(model_class: type[Any], fields: tuple[Any, ...]) -> list[str]:
    if not fields:
        raise ValueError("At least one group-by field required")

    owned_attrs = model_class.get_all_attributes()
    field_names: list[str] = []
    for field in fields:
        attr_type = getattr(field, "attr_type", None)
        if attr_type is not None:
            if not any(attr_info.typ is attr_type for attr_info in owned_attrs.values()):
                raise ValueError(f"{model_class.__name__} does not own group-by field {field!r}")
            field_names.append(attr_type.get_attribute_name())
            continue
        if isinstance(field, str):
            attr_info = owned_attrs.get(field)
            if attr_info is None:
                raise ValueError(f"Unknown group-by field '{field}' for {model_class.__name__}")
            field_names.append(attr_info.typ.get_attribute_name())
            continue
        raise TypeError(f"Unsupported group-by field {field!r}")
    return field_names


def _normalize_reduce_rows(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [
        {_normalize_reduce_key(key): _unwrap_reduce_value(value) for key, value in row.items()}
        for row in rows
    ]


def _normalize_grouped_reduce_rows(
    rows: list[dict[str, Any]],
    group_count: int,
) -> dict[Any, dict[str, Any]]:
    output: dict[Any, dict[str, Any]] = {}
    for row in _normalize_reduce_rows(rows):
        group_values = [row.pop(f"group{index}") for index in range(group_count)]
        group_key: Any = group_values[0] if len(group_values) == 1 else tuple(group_values)
        output[group_key] = row
    return output


def _normalize_reduce_key(key: str) -> str:
    return key[1:] if key.startswith("$") else key


def _unwrap_reduce_value(value: Any) -> Any:
    if isinstance(value, dict) and "value" in value:
        return value["value"]
    return value
