"""Experimental Rust-backed manager facade."""

from __future__ import annotations

import re
from typing import TYPE_CHECKING, Any, Never, Self, cast

from type_bridge._rust_runtime import (
    key_filter_for_entity,
    normalize_attributes,
    normalize_value,
    register_model_descriptor,
    relation_role_fields,
    role_player_inputs,
    rust_manager_for_entity,
    rust_manager_for_relation,
)
from type_bridge.crud.hooks import CrudEvent, HookRunner
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
        if self._manager_instance is None:
            if self._kind == "entity":
                self._manager_instance = rust_manager_for_entity(self._connection, self._descriptor)
            else:
                self._manager_instance = rust_manager_for_relation(
                    self._connection, self._descriptor
                )
        return self._manager_instance

    def add_hook(self, hook: Any) -> Self:
        self._hook_runner.add(hook)
        return self

    def remove_hook(self, hook: Any) -> None:
        self._hook_runner.remove(hook)

    def insert(self, instance: T) -> T:
        self._run_pre(CrudEvent.PRE_INSERT, instance)
        if isinstance(instance, Entity):
            attributes = normalize_attributes(self.model_class, instance.to_dict())
            iid = self._manager.insert(attributes)
            instance._set_backend_iid(iid)
            self._run_post(CrudEvent.POST_INSERT, instance)
            return instance

        if isinstance(instance, Relation):
            attributes = normalize_attributes(self.model_class, _relation_attribute_dict(instance))
            iid = self._manager.insert(attributes, role_player_inputs(instance))
            instance._set_backend_iid(iid)
            self._run_post(CrudEvent.POST_INSERT, instance)
            return instance

        raise TypeError(f"Unsupported instance type: {type(instance)}")

    def get(self, **filters: Any) -> list[T]:
        _reject_lookup_filters(filters)
        if self._kind == "relation":
            attr_filters, role_filters = _split_relation_filters(self.model_class, filters)
            normalized = normalize_attributes(self.model_class, attr_filters)
            if role_filters:
                rows = self._manager.get_with_role_players(normalized, role_filters)
                return self._hydrate_relation_rows(rows)
        normalized = normalize_attributes(self.model_class, filters)
        return self._get_with_filter_input(normalized)

    def _get_with_filter_input(self, filters: Any) -> list[T]:
        rows = self._manager.get(filters)
        if self._kind == "relation":
            return self._hydrate_relation_rows(rows)
        return [self._hydrate_entity(row) for row in rows]

    def all(self) -> list[T]:
        rows = self._manager.all()
        if self._kind == "relation":
            return self._hydrate_relation_rows(rows)
        return [self._hydrate_entity(row) for row in rows]

    def count(self, **filters: Any) -> int:
        _reject_lookup_filters(filters)
        normalized = normalize_attributes(self.model_class, filters)
        return int(self._manager.count(normalized))

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
            self._manager.update(attributes, getattr(instance, "_iid", None))
            self._run_post(CrudEvent.POST_UPDATE, instance)
            return instance

        if isinstance(instance, Relation):
            attributes = normalize_attributes(self.model_class, _relation_attribute_dict(instance))
            self._manager.update(
                attributes, role_player_inputs(instance), getattr(instance, "_iid", None)
            )
            self._run_post(CrudEvent.POST_UPDATE, instance)
            return instance

        raise TypeError(f"Unsupported instance type: {type(instance)}")

    def put(self, instance: T) -> T:
        self._run_pre(CrudEvent.PRE_PUT, instance)
        if isinstance(instance, Entity):
            attributes = normalize_attributes(self.model_class, instance.to_dict())
            iid = self._manager.put(attributes)
            instance._set_backend_iid(iid)
            self._run_post(CrudEvent.POST_PUT, instance)
            return instance

        if isinstance(instance, Relation):
            attributes = normalize_attributes(self.model_class, _relation_attribute_dict(instance))
            iid = self._manager.put(attributes, role_player_inputs(instance))
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
            iids = self._manager.insert_many(attributes)
            _set_iids(instances, iids)
            self._run_post_many(CrudEvent.POST_INSERT, instances)
            return instances

        if all(isinstance(instance, Relation) for instance in instances):
            self._run_pre_many(CrudEvent.PRE_INSERT, instances)
            items = [
                _relation_write_item(cast(Relation, instance), self.model_class)
                for instance in instances
            ]
            iids = self._manager.insert_many(items)
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
            iids = self._manager.put_many(attributes)
            _set_iids(instances, iids)
            self._run_post_many(CrudEvent.POST_PUT, instances)
            return instances

        if all(isinstance(instance, Relation) for instance in instances):
            self._run_pre_many(CrudEvent.PRE_PUT, instances)
            items = [
                _relation_write_item(cast(Relation, instance), self.model_class)
                for instance in instances
            ]
            iids = self._manager.put_many(items)
            _set_iids(instances, iids)
            self._run_post_many(CrudEvent.POST_PUT, instances)
            return instances

        raise TypeError("Rust backend put_many requires all instances to share one model kind")

    def update_many(self, instances: list[T]) -> list[T]:
        for instance in instances:
            self.update(instance)
        return instances

    def filter(self, *expressions: Any, **filters: Any) -> RustTypeDBQuery[T]:
        _reject_lookup_filters(filters)
        return RustTypeDBQuery(self, filters, expressions)

    def group_by(self, *fields: Any) -> RustTypeDBGroupByQuery[T]:
        return RustTypeDBQuery(self, {}).group_by(*fields)

    def get_by_iid(self, iid: str) -> T | None:
        if not iid or not re.match(r"^0x[0-9a-fA-F]+$", iid):
            return None

        row = self._manager.get_by_iid(iid)
        if row is None:
            return None
        if self._kind == "relation":
            rows = row if isinstance(row, list) else [row]
            if not rows:
                return None
            hydrated = self._hydrate_relation_rows(rows)
            return hydrated[0] if hydrated else None
        return self._hydrate_entity(row)

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

        raise ValueError("Rust backend delete requires a relation instance with _iid populated")

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


class RustTypeDBQuery[T: "TypeDBType"]:
    """Small Rust-backed chainable query for exact-match Phase 3 filters."""

    def __init__(
        self,
        manager: RustTypeDBManager[T],
        filters: dict[str, Any],
        expressions: tuple[Any, ...] = (),
    ):
        self._manager = manager
        self._filters = dict(filters)
        self._expressions = list(expressions)
        self._limit_value: int | None = None
        self._offset_value: int | None = None

    def filter(self, *expressions: Any, **filters: Any) -> Self:
        _reject_lookup_filters(filters)
        self._expressions.extend(expressions)
        self._filters.update(filters)
        return self

    def limit(self, n: int) -> Self:
        self._limit_value = n
        return self

    def offset(self, n: int) -> Self:
        self._offset_value = n
        return self

    def order_by(self, *fields: str) -> Never:
        del fields
        self._manager._unsupported("ordering")

    def execute(self) -> list[T]:
        results = self._execute_uncapped()
        if self._offset_value is not None:
            results = results[self._offset_value :]
        if self._limit_value is not None:
            results = results[: self._limit_value]
        return results

    def _execute_uncapped(self) -> list[T]:
        if self._manager._kind == "relation":
            attr_filters, role_filters = _split_relation_filters(
                self._manager.model_class,
                self._filters,
            )
            filters = _filter_input(self._manager.model_class, attr_filters, self._expressions)
            if role_filters:
                rows = self._manager._manager.get_with_role_players(filters, role_filters)
                return self._manager._hydrate_relation_rows(rows)
            return self._manager._get_with_filter_input(filters)

        filters = _filter_input(self._manager.model_class, self._filters, self._expressions)
        return self._manager._get_with_filter_input(filters)

    def all(self) -> list[T]:
        return self.execute()

    def first(self) -> T | None:
        original_limit = self._limit_value
        self._limit_value = 1
        results = self.execute()
        self._limit_value = original_limit
        return results[0] if results else None

    def count(self) -> int:
        if self._manager._kind == "relation" and _has_relation_role_filters(
            self._manager.model_class,
            self._filters,
        ):
            return len(self._execute_uncapped())
        if not self._expressions:
            return self._manager.count(**self._filters)
        filters = _filter_input(self._manager.model_class, self._filters, self._expressions)
        return int(self._manager._manager.count(filters))

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
        manager: RustTypeDBManager[T],
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


def _reject_lookup_filters(filters: dict[str, Any]) -> None:
    if any("__" in key for key in filters):
        raise NotImplementedError(
            "TYPE_BRIDGE_BACKEND=rust does not support lookup filters in Phase 3"
        )


def _relation_write_item(instance: Relation, model_class: type[Any]) -> dict[str, Any]:
    return {
        "attributes": normalize_attributes(model_class, _relation_attribute_dict(instance)),
        "role_players": role_player_inputs(instance),
    }


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


def _role_player_key(field_name: str, player: Any) -> tuple[Any, ...]:
    iid = getattr(player, "_iid", None)
    if iid:
        return (field_name, "iid", iid)
    return (
        field_name,
        "dump",
        tuple(sorted(player.model_dump(mode="json").items())),
    )


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
