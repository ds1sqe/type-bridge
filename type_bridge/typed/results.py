"""Fail-closed model construction from opaque validated native result views."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import fields, is_dataclass
from typing import Any, Never

from type_bridge_core import (
    ValidatedMatchAttributeHandle as _NativeAttribute,
)
from type_bridge_core import (
    ValidatedMatchResultHandle as _NativeResult,
)
from type_bridge_core import (
    ValidatedMatchRolePlayerHandle as _NativeRolePlayer,
)
from type_bridge_core import (
    ValidatedMatchThingHandle as _NativeThing,
)

from type_bridge._rust_runtime import rust_value_type
from type_bridge.crud.types import is_multi_value_attribute
from type_bridge.models.base import TypeDBType
from type_bridge.models.entity import Entity
from type_bridge.models.relation import Relation
from type_bridge.typed.page import Page


class TypedQueryMaterializationError(RuntimeError):
    """Validated native data cannot map exactly to registered Python models."""

    __slots__ = ("code",)

    category = "result_decode"

    def __init__(self, code: str, message: str) -> None:
        self.code = code
        super().__init__(message)


def _materialize_one(
    result: _NativeResult,
    models: Mapping[str, type[TypeDBType]],
    declaration: type[object] | None,
) -> object:
    """Materialize exactly one native-proven selected row."""
    count = result.row_count()
    if count != 1:
        _fail(
            "materialized_one_cardinality",
            f"exactly-one validated result exposed {count} rows",
        )
    return _materialize_row(result.row(0), models, declaration)


def _materialize_rows(
    result: _NativeResult,
    models: Mapping[str, type[TypeDBType]],
    declaration: type[object] | None,
) -> list[object]:
    """Materialize every bounded native-proven selected row in stable order."""
    return [
        _materialize_row(result.row(index), models, declaration)
        for index in range(result.row_count())
    ]


def _materialize_page(
    result: _NativeResult,
    models: Mapping[str, type[TypeDBType]],
    declaration: type[object] | None,
) -> Page[object]:
    """Materialize a validated distinct-root page with immutable collections."""
    items = [
        _materialize_row(
            result.page_entry(index),
            models,
            declaration,
            allow_collections=True,
        )
        for index in range(result.page_entry_count())
    ]
    return Page(
        items,
        offset=result.page_offset(),
        limit=result.page_limit(),
        total=result.page_total(),
    )


def _materialize_count(result: _NativeResult) -> int:
    """Return one lossless native-proven unsigned distinct-root count."""
    value = result.count_value()
    if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value < 1 << 64:
        _fail("invalid_count_value", "validated count is not an unsigned 64-bit integer")
    return value


def _materialize_exists(result: _NativeResult) -> bool:
    """Return one native-proven distinct-root existence value."""
    value = result.exists_value()
    if type(value) is not bool:
        _fail("invalid_exists_value", "validated exists result is not a boolean")
    return value


def _materialize_row(
    row: Any,
    models: Mapping[str, type[TypeDBType]],
    declaration: type[object] | None,
    *,
    allow_collections: bool = False,
) -> object:
    slot_count = row.slot_count()
    if not 1 <= slot_count <= 16:
        _fail(
            "materialized_slot_arity",
            f"validated row exposed unsupported slot count {slot_count}",
        )

    names: list[str | None] = []
    values: list[object] = []
    for index in range(slot_count):
        slot = row.slot(index)
        names.append(slot.name())
        if slot.is_collection():
            if not allow_collections:
                _fail(
                    "collection_in_fetch_rows",
                    "selected-row execution exposed a collection-bearing slot",
                )
            values.append(
                tuple(
                    _materialize_thing(slot.thing(thing_index), models)
                    for thing_index in range(slot.thing_count())
                )
            )
            continue
        if slot.thing_count() != 1:
            _fail(
                "singular_slot_cardinality",
                "selected-row execution exposed a non-singular slot",
            )
        values.append(_materialize_thing(slot.thing(0), models))

    if declaration is None:
        if any(name is not None for name in names):
            _fail(
                "unexpected_named_result",
                "positional query received named native result slots",
            )
        return values[0] if len(values) == 1 else tuple(values)

    if any(name is None for name in names):
        _fail(
            "missing_named_result_member",
            "declared row received positional native result slots",
        )
    member_names = [name for name in names if name is not None]
    if len(set(member_names)) != len(member_names):
        _fail(
            "duplicate_named_result_member",
            "validated named row contains duplicate member names",
        )
    expected_names = _declaration_names(declaration)
    if tuple(member_names) != expected_names:
        _fail(
            "named_result_declaration_mismatch",
            "validated named row no longer matches its Python declaration",
        )
    try:
        materialized = declaration(**dict(zip(member_names, values, strict=True)))
    except Exception as error:
        raise TypedQueryMaterializationError(
            "named_row_construction_failed",
            f"failed to construct declared row {declaration.__name__}",
        ) from error
    if type(materialized) is not declaration:
        _fail(
            "named_row_constructor_substitution",
            "declared row constructor returned a different runtime type",
        )
    return materialized


def _materialize_thing(
    thing: _NativeThing,
    models: Mapping[str, type[TypeDBType]],
) -> TypeDBType:
    model = _resolve_model(
        models,
        declared_type=thing.declared_type_name(),
        concrete_type=thing.concrete_type_name(),
        kind=thing.kind(),
    )
    values = _materialize_attributes(thing, model)
    if issubclass(model, Relation):
        values.update(_materialize_roles(thing, model, models))
    try:
        instance = model(**values)
    except Exception as error:
        raise TypedQueryMaterializationError(
            "model_construction_failed",
            f"failed to construct validated model {model.__name__}",
        ) from error
    if type(instance) is not model:
        _fail(
            "model_constructor_substitution",
            f"constructor for {model.__name__} returned a different runtime type",
        )
    _assign_iid(instance, thing.iid(), "model")
    return instance


def _materialize_role_player(
    player: _NativeRolePlayer,
    models: Mapping[str, type[TypeDBType]],
    allowed_types: tuple[type[TypeDBType], ...],
) -> TypeDBType:
    model = _resolve_model(
        models,
        declared_type=player.declared_type_name(),
        concrete_type=player.concrete_type_name(),
        kind=player.kind(),
    )
    if not any(issubclass(model, allowed) for allowed in allowed_types):
        _fail(
            "role_player_python_type_mismatch",
            f"concrete role player {model.__name__} is not compatible with its Python role",
        )
    if issubclass(model, Relation):
        _fail(
            "nested_relation_role_player_unsupported",
            "Python typed queries cannot materialize a relation used as a role player "
            "without a cycle-safe result contract",
        )
    values = _materialize_attributes(player, model)
    try:
        instance = model(**values)
    except Exception as error:
        raise TypedQueryMaterializationError(
            "role_player_construction_failed",
            f"failed to construct validated role player {model.__name__}",
        ) from error
    if type(instance) is not model:
        _fail(
            "role_player_constructor_substitution",
            f"constructor for role player {model.__name__} returned a different runtime type",
        )
    _assign_iid(instance, player.iid(), "role player")
    return instance


def _resolve_model(
    models: Mapping[str, type[TypeDBType]],
    *,
    declared_type: str,
    concrete_type: str,
    kind: str,
) -> type[TypeDBType]:
    declared = models.get(declared_type)
    concrete = models.get(concrete_type)
    if declared is None:
        _fail(
            "missing_declared_model_constructor",
            f"no Python constructor is registered for declared type {declared_type!r}",
        )
    if concrete is None:
        _fail(
            "missing_concrete_model_constructor",
            f"no Python constructor is registered for concrete type {concrete_type!r}",
        )
    if (
        not isinstance(declared, type)
        or not issubclass(declared, TypeDBType)
        or declared.get_type_name() != declared_type
    ):
        _fail(
            "declared_model_name_mismatch",
            f"registered constructor for {declared_type!r} reports another TypeDB name",
        )
    if not isinstance(concrete, type) or not issubclass(concrete, TypeDBType):
        _fail(
            "invalid_concrete_model_constructor",
            f"registered constructor for {concrete_type!r} is not a TypeDB model class",
        )
    if concrete.get_type_name() != concrete_type:
        _fail(
            "concrete_model_name_mismatch",
            f"registered constructor for {concrete_type!r} reports another TypeDB name",
        )
    if not issubclass(concrete, declared):
        _fail(
            "concrete_model_subtype_mismatch",
            f"concrete type {concrete_type!r} is not a Python subtype of {declared_type!r}",
        )
    expected_kind = (
        "relation"
        if issubclass(concrete, Relation)
        else "entity"
        if issubclass(concrete, Entity)
        else None
    )
    if expected_kind is None or kind != expected_kind:
        _fail(
            "model_kind_mismatch",
            f"validated {kind!r} thing cannot use constructor {concrete.__name__}",
        )
    return concrete


def _assign_iid(instance: TypeDBType, iid: str, owner: str) -> None:
    if not isinstance(iid, str) or not iid:
        _fail("invalid_model_iid", f"validated {owner} IID is not a non-empty string")
    try:
        instance._set_backend_iid(iid)
    except Exception as error:
        raise TypedQueryMaterializationError(
            "model_iid_assignment_failed",
            f"failed to assign validated IID to {owner} {type(instance).__name__}",
        ) from error
    if instance._iid != iid:
        _fail(
            "model_iid_assignment_mismatch",
            f"validated IID was not retained by {owner} {type(instance).__name__}",
        )


def _materialize_attributes(
    owner: _NativeThing | _NativeRolePlayer,
    model: type[TypeDBType],
) -> dict[str, object]:
    declared = model.get_all_attributes()
    values: dict[str, object] = {}
    for index in range(owner.attribute_count()):
        attribute = owner.attribute(index)
        field_name = attribute.field_name()
        if field_name in values:
            _fail(
                "duplicate_model_field",
                f"validated model {model.__name__} repeats field {field_name!r}",
            )
        field = declared.get(field_name)
        if field is None:
            _fail(
                "missing_model_field",
                f"validated field {field_name!r} does not exist on {model.__name__}",
            )
        try:
            expected_value_type = rust_value_type(field.typ)
        except Exception as error:
            raise TypedQueryMaterializationError(
                "missing_model_field_type",
                f"Python field {model.__name__}.{field_name} has no supported native value type",
            ) from error
        raw_values = _attribute_values(attribute, expected_value_type)
        if is_multi_value_attribute(field.flags):
            values[field_name] = raw_values
        elif len(raw_values) > 1:
            _fail(
                "singular_attribute_cardinality",
                f"validated singular field {model.__name__}.{field_name} has multiple values",
            )
        else:
            values[field_name] = raw_values[0] if raw_values else None

    for field_name, field in declared.items():
        if field_name in values:
            continue
        values[field_name] = [] if is_multi_value_attribute(field.flags) else None
    return values


def _attribute_values(attribute: _NativeAttribute, expected_type: str) -> list[object]:
    values: list[object] = []
    for index in range(attribute.value_count()):
        actual_type = attribute.value_type(index)
        if actual_type != expected_type:
            _fail(
                "model_field_type_mismatch",
                f"validated value type {actual_type!r} does not match Python type {expected_type!r}",
            )
        values.append(attribute.value(index))
    return values


def _materialize_roles(
    thing: _NativeThing,
    model: type[Relation],
    models: Mapping[str, type[TypeDBType]],
) -> dict[str, object]:
    declared_by_schema_name = {
        role.role_name: (field_name, role) for field_name, role in model.get_roles().items()
    }
    values: dict[str, object] = {}
    present: set[str] = set()
    for index in range(thing.role_count()):
        native_role = thing.role(index)
        role_name = native_role.role_name()
        if role_name in present:
            _fail(
                "duplicate_model_role",
                f"validated relation {model.__name__} repeats role {role_name!r}",
            )
        present.add(role_name)
        declared = declared_by_schema_name.get(role_name)
        if declared is None:
            _fail(
                "missing_model_role",
                f"validated role {role_name!r} does not exist on {model.__name__}",
            )
        field_name, role = declared
        if role.is_relates_only:
            if native_role.player_count() != 0:
                _fail(
                    "relates_only_role_player",
                    f"relates-only role {model.__name__}.{field_name} exposed a player",
                )
            continue
        players = [
            _materialize_role_player(
                native_role.player(player_index),
                models,
                role.player_entity_types,
            )
            for player_index in range(native_role.player_count())
        ]
        if role.is_multi_player:
            values[field_name] = players
        elif len(players) > 1:
            _fail(
                "singular_role_cardinality",
                f"validated singular role {model.__name__}.{field_name} has multiple players",
            )
        else:
            values[field_name] = players[0] if players else None

    for field_name, role in model.get_roles().items():
        if field_name in values or role.role_name in present or role.is_relates_only:
            continue
        values[field_name] = [] if role.is_multi_player else None
    return values


def _declaration_names(declaration: type[object]) -> tuple[str, ...]:
    if is_dataclass(declaration):
        return tuple(field.name for field in fields(declaration))
    names = getattr(declaration, "_fields", None)
    if isinstance(names, tuple) and all(isinstance(name, str) for name in names):
        return names
    _fail(
        "unsupported_named_row_declaration",
        "query lost its frozen dataclass or NamedTuple declaration metadata",
    )


def _fail(code: str, message: str) -> Never:
    raise TypedQueryMaterializationError(code, message)


__all__ = [
    "TypedQueryMaterializationError",
]
